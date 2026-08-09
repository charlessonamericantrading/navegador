use winit::{
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::EventLoop,
    window::WindowBuilder,
};
use tiny_skia::{Pixmap, Color};
use engine_layout::{ImageMap, LayoutBox};
use engine_text::FontSet;
use crate::display_list::DisplayList;
use crate::paint::paint_display_list;
use std::rc::Rc;
use std::num::NonZeroU32;

pub struct NativeEngineWindow;

impl NativeEngineWindow {
    /// Abre una ventana nativa real y pinta en ella. `layout_root` es el
    /// layout inicial (al tamaño de ventana con el que se crea); al
    /// redimensionar la ventana se llama a `relayout(nuevo_ancho,
    /// nuevo_alto)` para recalcular el layout de verdad al nuevo tamaño, no
    /// solo estirar el backbuffer sobre el layout congelado del arranque
    /// (lo que hacia esta funcion antes). `relayout` vive en `core/main.rs`
    /// (captura `dom_root`+`stylesheet`, ver `PageResult`) para que este
    /// crate (`gfx`) no tenga que depender de `engine-css`/`engine-dom`
    /// solo para saber reconstruir un layout - sigue sin saber que es un
    /// DOM o una hoja de estilos, solo pide "dame un arbol de cajas para
    /// este tamaño".
    ///
    /// El scroll real de la rueda del raton SI esta cableado (offset
    /// aplicado en pintado y en hit-testing, sin relayout - ver
    /// `scroll_offset_y` mas abajo y `clamp_scroll_offset`, al final del
    /// archivo); el teclado todavia no tiene ninguna fuente de eventos
    /// (eso sigue siendo Fase 3, ver ARCHITECTURE.md). Clic izquierdo si,
    /// via `on_click` (mas abajo).
    ///
    /// `font_set`: las MISMAS 4 variantes de fuente que ya uso el layout
    /// para medir el texto (`engine_layout::LayoutTreeBuilder::build`, ver
    /// `core/main.rs`) - se reciben ya cargadas en vez de cargarlas aqui
    /// otra vez, para que quien mide el texto y quien lo pinta usen
    /// exactamente la misma fuente/variante (antes se cargaba solo aqui, y
    /// el layout ni la usaba: medir con una fuente y pintar con otra podria
    /// desalinear cajas y glifos).
    ///
    /// `on_click`: se llama con el `LayoutBox` ACTUALMENTE mostrado (no el
    /// inicial - este crate ahora retiene su propio arbol de layout, mas
    /// abajo, para poder volver a hit-testear tras un resize) y las
    /// coordenadas del click (en `WindowEvent::MouseInput` con boton
    /// izquierdo, disparado en `Released` - mas cerca del `click` real del
    /// spec, que exige un press+release sobre el mismo objetivo, que
    /// solo en `Pressed`). Si devuelve `Some(nuevo_layout)`, se repinta con
    /// el; si devuelve `None` (el click no toco ningun elemento, o quien
    /// llama decidio que no hacia falta repintar), no se hace nada. Igual
    /// que `relayout`, vive en `core/main.rs` para que este crate no tenga
    /// que saber que es un DOM o un `JsRuntime` - solo hace hit-testing
    /// sobre cajas (`LayoutBox::hit_test`, que SI conoce este crate porque
    /// vive en `engine-layout`, su propia dependencia) y delega el "que
    /// hacer con el nodo encontrado" a quien lo llame.
    pub fn run(
        title: &str,
        layout_root: LayoutBox,
        font_set: Option<FontSet>,
        images: ImageMap,
        relayout: impl Fn(f32, f32) -> LayoutBox + 'static,
        mut on_click: impl FnMut(&LayoutBox, f32, f32) -> Option<LayoutBox> + 'static,
    ) {
        let event_loop = EventLoop::new().unwrap();
        let window = Rc::new(
            WindowBuilder::new()
                .with_title(title)
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
                .build(&event_loop)
                .unwrap(),
        );

        // softbuffer es lo que de verdad lleva los pixeles calculados a la
        // pantalla; sin este contexto+superficie, pintar en un Pixmap no
        // tiene ningun efecto visible.
        let context = softbuffer::Context::new(window.clone()).expect("crear contexto softbuffer");
        let mut surface = softbuffer::Surface::new(&context, window.clone()).expect("crear superficie softbuffer");

        // Antes, `layout_root` (el parametro) solo se usaba UNA VEZ para
        // construir `display_list` y se perdia - un resize reconstruia una
        // caja local que tampoco se retenia mas alla de aplanarla. Sin
        // retener el arbol de cajas de verdad no hay como hit-testear un
        // click posterior (`display_list` ya perdio el backref a los
        // `Node` reales - ver `LayoutBox::dom_node`), asi que ahora se
        // guarda como estado mutable del bucle de eventos, igual que
        // `display_list`.
        let mut current_layout_root = layout_root;
        let mut display_list = DisplayList::build(&current_layout_root, &images);
        let mut cursor_position: (f32, f32) = (0.0, 0.0);
        // Desplazamiento vertical actual, en content-space (mismas unidades
        // que `LayoutBox::dimensions`) - NUNCA se rehace `display_list` por
        // esto (`DisplayList::build` sigue sin saber que existe el scroll):
        // el pintado resta este valor a cada `rect.y` en el momento de
        // pintar (ver `RedrawRequested`, mas abajo), y el hit-test hace la
        // traduccion inversa sobre las coordenadas de pantalla del clic.
        let mut scroll_offset_y: f32 = 0.0;

        tracing::info!("Ventana nativa iniciada: '{}' ({} elementos de pintado)", title, display_list.items.len());

        // `ControlFlow::Wait` (el valor por defecto real de winit - lo
        // fijamos aqui explicito para que quede documentado, no porque
        // hiciera falta cambiarlo) hace que el bucle de eventos DUERMA
        // hasta el siguiente evento real del SO, en vez de girar sin parar.
        // Eso solo funciona si nada pide un redraw sin necesidad: antes,
        // `AboutToWait` pedia uno incondicional en cada vuelta, lo que
        // encadenaba RedrawRequested -> AboutToWait -> RedrawRequested para
        // siempre y mantenia el bucle ocupado aunque ControlFlow ya fuera
        // Wait - medido en vivo: ~97% de un nucleo, sin ninguna interaccion,
        // solo por eso. Ahora solo se pide un redraw cuando algo cambia de
        // verdad: aqui, una vez, para el primer pintado, y en el handler de
        // `Resized` (mas abajo), tras recalcular el layout.
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        window.request_redraw();

        let _ = event_loop.run(move |event, elwt| {
            match event {
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => elwt.exit(),
                Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                    if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                        surface.resize(w, h).ok();
                    }
                    // Reflow real: se recalcula el arbol de layout entero al
                    // nuevo tamaño (no solo se estira el backbuffer sobre
                    // las mismas cajas de antes) - ver el aviso de `run`.
                    current_layout_root = relayout(size.width as f32, size.height as f32);
                    display_list = DisplayList::build(&current_layout_root, &images);
                    // El contenido puede haber cambiado de alto (un ancho
                    // distinto envuelve el texto en mas o menos lineas) -
                    // re-acotar aqui evita quedarse "scrolleado" mas alla
                    // del nuevo fondo real de la pagina.
                    scroll_offset_y = clamp_scroll_offset(scroll_offset_y, current_layout_root.content_extent(), size.height as f32);
                    tracing::info!(
                        "Ventana redimensionada a {}x{} - layout recalculado ({} elementos de pintado)",
                        size.width, size.height, display_list.items.len(),
                    );
                    window.request_redraw();
                }
                Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                    // No hay relayout aqui a proposito: hacer scroll no
                    // cambia el layout de la pagina, solo que parte de el
                    // esta visible - misma logica que un navegador real.
                    // LineDelta llega en "lineas" (ratones fisicos con
                    // muescas discretas); PixelDelta ya viene en pixeles
                    // (trackpads). PIXELS_PER_LINE es una aproximacion
                    // razonable, no medida contra un raton real. El signo
                    // exacto (que direccion de giro debe SUMAR vs RESTAR)
                    // varia por plataforma/driver y no se verifico en vivo
                    // (misma limitacion ya aceptada para el clic real del
                    // SO en ARCHITECTURE.md, "Clic real del SO cableado de
                    // punta a punta" - no se usa control de raton/teclado
                    // real sobre la ventana del propio motor) - si al
                    // probarlo el sentido sale invertido, el arreglo es
                    // cambiar este `-=` por `+=`, nada mas.
                    const PIXELS_PER_LINE: f32 = 40.0;
                    let delta_y = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y * PIXELS_PER_LINE,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                    };
                    let size = window.inner_size();
                    scroll_offset_y = clamp_scroll_offset(
                        scroll_offset_y - delta_y,
                        current_layout_root.content_extent(),
                        size.height as f32,
                    );
                    window.request_redraw();
                }
                Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                    // `MouseInput` (mas abajo) no trae coordenadas propias -
                    // solo se sabe donde esta el cursor por el ultimo
                    // `CursorMoved` recibido, igual que cualquier motor real
                    // tiene que rastrearlo el mismo.
                    cursor_position = (position.x as f32, position.y as f32);
                }
                Event::WindowEvent { event: WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. }, .. } => {
                    // Hit-test sobre el arbol de layout ACTUAL (no el
                    // inicial - `current_layout_root` ya refleja el ultimo
                    // resize, si hubo alguno). `on_click` decide que hacer
                    // con el nodo encontrado (tipicamente: disparar un
                    // "click" real con `JsRuntime::dispatch_event` y
                    // reconstruir el layout por si un listener mutó el DOM,
                    // ver `core/main.rs`) - este crate solo hace hit-testing,
                    // sigue sin saber que es un DOM o un evento.
                    // `cursor_position` es screen-space (lo que winit
                    // reporta, sin saber que existe el scroll); hit_test
                    // exige content-space (las mismas coordenadas en las
                    // que estan `dimensions` de cada caja) - sumar el
                    // offset deshace exactamente el `-scroll_offset_y` que
                    // se aplica al pintar, mas abajo.
                    let content_y = cursor_position.1 + scroll_offset_y;
                    if let Some(new_layout_root) = on_click(&current_layout_root, cursor_position.0, content_y) {
                        current_layout_root = new_layout_root;
                        display_list = DisplayList::build(&current_layout_root, &images);
                        window.request_redraw();
                    }
                }
                Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                    let size = window.inner_size();
                    let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
                        return;
                    };

                    let Some(mut pixmap) = Pixmap::new(size.width, size.height) else { return };
                    pixmap.fill(Color::from_rgba8(245, 245, 245, 255));

                    // `display_list` guarda geometria en content-space (las
                    // mismas coordenadas que calculo el layout, ajenas al
                    // scroll); la transformacion a screen-space (restar
                    // `scroll_offset_y` a cada `y`), el redondeo/recorte de
                    // border-radius, la sombra y el recorte de `overflow:
                    // hidden` (Fase 3.5) viven en `paint::paint_display_list`,
                    // compartido con el rasterizado headless (`raster.rs`) -
                    // ver su doc-comment para el porque de este reparto.
                    paint_display_list(&mut pixmap, &display_list.items, font_set.as_ref(), scroll_offset_y);

                    // Copiar el Pixmap (RGBA8) al buffer de softbuffer (0RGB32).
                    if let Ok(mut buffer) = surface.buffer_mut() {
                        let pixels = pixmap.pixels();
                        for (dst, src) in buffer.iter_mut().zip(pixels.iter()) {
                            let c = src.demultiply();
                            *dst = ((c.red() as u32) << 16) | ((c.green() as u32) << 8) | (c.blue() as u32);
                        }
                        let _ = buffer.present();
                    }
                    let _ = w;
                    let _ = h;
                }
                _ => (),
            }
        });
    }
}

/// Acota el scroll a `[0, max(0, content_extent - viewport_height)]`: nunca
/// negativo (no se puede subir mas alla del principio) y nunca mas alla de
/// donde el contenido real termina (`content_extent`, ver
/// `LayoutBox::content_extent`) - si el contenido cabe entero en el
/// viewport, el maximo es 0 y no hay scroll posible en absoluto.
fn clamp_scroll_offset(offset: f32, content_extent: f32, viewport_height: f32) -> f32 {
    let max_offset = (content_extent - viewport_height).max(0.0);
    offset.clamp(0.0, max_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_scroll_offset_stays_at_zero_when_content_fits_the_viewport() {
        assert_eq!(clamp_scroll_offset(0.0, 400.0, 720.0), 0.0);
        assert_eq!(clamp_scroll_offset(50.0, 400.0, 720.0), 0.0, "no hay nada que desplazar si el contenido cabe entero");
    }

    #[test]
    fn clamp_scroll_offset_never_goes_negative() {
        assert_eq!(clamp_scroll_offset(-100.0, 2000.0, 720.0), 0.0);
    }

    #[test]
    fn clamp_scroll_offset_caps_at_the_real_overflow_amount() {
        assert_eq!(clamp_scroll_offset(10000.0, 2000.0, 720.0), 1280.0, "el maximo es exactamente lo que se desborda: 2000 - 720");
    }

    #[test]
    fn clamp_scroll_offset_passes_through_valid_values_unchanged() {
        assert_eq!(clamp_scroll_offset(300.0, 2000.0, 720.0), 300.0);
    }
}
