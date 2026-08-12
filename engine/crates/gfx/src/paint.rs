//! Pintado real de un `DisplayList` sobre un `tiny_skia::Pixmap` -
//! compartido entre `raster.rs` (captura PNG headless, puente IPC) y
//! `window.rs` (ventana nativa) para no duplicar la logica de pintado dos
//! veces. Antes de esta tarea (Fase 3.5) cada archivo tenia su propia copia
//! del bucle de pintado Y su propia copia de `border_strip_rects` - un bug
//! real de tiny-skia con bordes en coordenadas fraccionarias (Fase 3.4) se
//! encontro y arreglo DOS veces, una por copia, precisamente por esa
//! duplicacion (ver ARCHITECTURE.md). `border-radius`, `box-shadow` y
//! `overflow: hidden` (Fase 3.5) se añaden aqui, una sola vez.

use crate::display_list::DisplayItem;
use crate::image_paint::paint_image;
use engine_layout::Rect;
use engine_text::{measure_text, shape_text, wrap_text, FontSet};
use tiny_skia::{FillRule, Mask, Paint, Path, PathBuilder, Pixmap, Rect as SkiaRect, Stroke, Transform};

/// Pinta cada `DisplayItem` de `items`, EN ORDEN, sobre `pixmap`.
/// `scroll_offset_y` se resta de cada coordenada Y (transformacion
/// content-space -> screen-space, unica aqui - `DisplayList` en si nunca se
/// reconstruye por scroll). Se redondea a pixel entero ANTES de usarse en
/// cualquier calculo: sin esto, un scroll a un offset fraccionario (posible
/// con `MouseScrollDelta::PixelDelta` de un trackpad) podria reintroducir
/// una coordenada fraccionaria en una franja de `border` YA redondeada por
/// `border_strip_rects` (mismo bug de tiny-skia que la Fase 3.4 encontro y
/// arreglo para columnas de tabla - ver su doc-comment) por la puerta de
/// atras.
pub fn paint_display_list(pixmap: &mut Pixmap, items: &[DisplayItem], font_set: Option<&FontSet>, scroll_offset_y: f32) {
    let scroll_offset_y = scroll_offset_y.round();
    let (width, height) = (pixmap.width(), pixmap.height());

    // Pila de rectangulos de recorte activos (`overflow: hidden`, Fase
    // 3.5) - la mascara real solo se reconstruye cuando la pila CAMBIA
    // (`PushClip`/`PopClip`), no en cada item individual, para no repetir
    // el coste de construirla por cada `fill_rect`/`fill_path` de un
    // subarbol recortado.
    let mut clip_stack: Vec<Rect> = Vec::new();
    let mut current_mask: Option<Mask> = None;

    for item in items {
        // Descarte por viewport (Fase 5): un item cuyo rectangulo cae
        // ENTERO por encima o por debajo del pixmap no puede pintar ni un
        // pixel visible, asi que hacerlo es trabajo tirado. No es una
        // optimizacion cosmetica: medido sobre una pagina de 200 filas
        // (~4800px de contenido en un viewport de 800px), pintar era el
        // 98% del coste de cada respuesta del servidor y ~5/6 de ese
        // trabajo era para pixeles fuera de pantalla.
        //
        // `PushClip`/`PopClip` NUNCA se descartan aunque su rectangulo
        // este fuera: no pintan nada, cambian ESTADO (la pila de recorte),
        // y saltarselos desemparejaria la pila y recortaria mal todo lo
        // que viniera despues.
        if let Some(rect) = item_rect(item) {
            if is_offscreen(rect, height, scroll_offset_y) {
                continue;
            }
        }
        match item {
            DisplayItem::PushClip { rect } => {
                clip_stack.push(rect.clone());
                current_mask = build_clip_mask(width, height, &clip_stack, scroll_offset_y);
            }
            DisplayItem::PopClip => {
                clip_stack.pop();
                current_mask = build_clip_mask(width, height, &clip_stack, scroll_offset_y);
            }
            // `Shadow` y `SolidRect` son el mismo relleno (un rectangulo,
            // posiblemente redondeado) - solo cambia de donde sale el
            // color/rect, ya resueltos por `DisplayList::build`.
            DisplayItem::Shadow { rect, color, radius } | DisplayItem::SolidRect { rect, color, radius } => {
                fill_shape(pixmap, rect, *radius, &paint_of(*color), scroll_offset_y, current_mask.as_ref());
            }
            DisplayItem::Text { rect, text, color, font_size, bold, italic } => {
                paint_text(pixmap, rect, text, *color, *font_size, *bold, *italic, font_set, scroll_offset_y, current_mask.as_ref());
            }
            DisplayItem::Border { rect, width: border_width, color, radius } => {
                paint_border(pixmap, rect, *border_width, *color, *radius, scroll_offset_y, current_mask.as_ref());
            }
            DisplayItem::Image { rect, image } => {
                paint_image(pixmap, rect, image, scroll_offset_y, current_mask.as_ref());
            }
        }
    }
}

/// El rectangulo de un item que PINTA, o `None` para los que solo cambian
/// estado (`PushClip`/`PopClip`) - devolver `None` es lo que garantiza que
/// esos dos nunca se descarten.
fn item_rect(item: &DisplayItem) -> Option<&Rect> {
    match item {
        DisplayItem::PushClip { .. } | DisplayItem::PopClip => None,
        DisplayItem::Shadow { rect, .. }
        | DisplayItem::SolidRect { rect, .. }
        | DisplayItem::Text { rect, .. }
        | DisplayItem::Border { rect, .. }
        | DisplayItem::Image { rect, .. } => Some(rect),
    }
}

/// Cuanto se ensancha el viewport antes de decidir que algo esta fuera.
/// No es paranoia gratuita: el rectangulo de un item no siempre acota todo
/// lo que ese item llega a pintar - una sombra se difumina mas alla de su
/// caja, y los glifos de un texto pueden sobresalir del alto de linea por
/// ascendentes/descendentes. Con este margen, cualquier item que pudiera
/// tocar aunque fuera un pixel del borde se sigue pintando; lo que se
/// descarta esta fuera con holgura.
const MARGEN_DESCARTE: f32 = 64.0;

/// `true` si `rect` (en coordenadas de documento) cae entero fuera del alto
/// del pixmap una vez aplicado el scroll.
///
/// Solo se comprueba el eje vertical: es donde el contenido se desborda de
/// verdad (una pagina larga) y donde esta todo el ahorro. En horizontal el
/// layout ya acota las cajas al ancho del viewport y no hay scroll lateral,
/// asi que comprobarlo no descartaria practicamente nada.
fn is_offscreen(rect: &Rect, viewport_height: u32, scroll_offset_y: f32) -> bool {
    let top = rect.y - scroll_offset_y;
    let bottom = top + rect.height;
    bottom < -MARGEN_DESCARTE || top > viewport_height as f32 + MARGEN_DESCARTE
}

fn paint_of(color: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint
}

/// Interseccion geometrica de dos rectangulos - `None` si no se solapan en
/// absoluto.
fn intersect_rects(a: &Rect, b: &Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    if right <= x || bottom <= y {
        return None;
    }
    Some(Rect { x, y, width: right - x, height: bottom - y })
}

/// Construye la mascara para la pila de recortes ACTIVA - la interseccion
/// de TODOS los `overflow: hidden` activos en ese punto del arbol, no solo
/// el mas cercano (varios anidados recortan al mas pequeño de todos, como
/// en un navegador real). `None` cuando la pila esta vacia (sin recorte
/// activo - pintar normal, sin mascara). Cuando la pila NO esta vacia pero
/// la interseccion resulta vacia (los recortes activos no se solapan en
/// absoluto), devuelve una mascara TODA A CERO (`Mask::new` la crea asi) en
/// vez de `None` - todo recortado, nada visible, que es lo correcto.
fn build_clip_mask(width: u32, height: u32, stack: &[Rect], scroll_offset_y: f32) -> Option<Mask> {
    if stack.is_empty() {
        return None;
    }
    let mut mask = Mask::new(width, height)?;
    let mut iter = stack.iter();
    let first = iter.next()?.clone();
    let intersection = iter.try_fold(first, |acc, r| intersect_rects(&acc, r));
    if let Some(rect) = intersection {
        if let Some(sk_rect) = SkiaRect::from_xywh(rect.x, rect.y - scroll_offset_y, rect.width.max(0.0), rect.height.max(0.0)) {
            let path = PathBuilder::from_rect(sk_rect);
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        }
    }
    Some(mask)
}

/// Construye el contorno de un rectangulo con esquinas redondeadas -
/// tiny-skia no trae un `push_round_rect`, asi que se hace a mano con 4
/// curvas cuadraticas (`quad_to`, control point EN la esquina exacta) por
/// cada esquina: no es un arco circular matematicamente perfecto (eso
/// exigiria curvas cubicas con la constante magica ~0.5522847498 de la
/// aproximacion estandar de un arco de 90 grados), pero visualmente es
/// indistinguible a los radios tipicos de una UI (unos pocos a unas pocas
/// decenas de pixeles) - simplificacion declarada, suficiente sin inventar
/// mas matematica de la que este motor necesita. `radius` ya viene
/// clampado a la mitad del lado mas corto por quien llama (evita un
/// rectangulo "imposible" con esquinas que se solaparian). `None` si
/// `radius <= 0` (sin nada que redondear - quien llama cae al `fill_rect`
/// normal en ese caso).
fn rounded_rect_path(rect: SkiaRect, radius: f32) -> Option<Path> {
    if radius <= 0.0 {
        return None;
    }
    let (x, y, w, h) = (rect.left(), rect.top(), rect.width(), rect.height());
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return None;
    }

    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Fondo (`SolidRect`) o sombra (`Shadow`) - el mismo relleno, redondeado
/// si `radius > 0` (via `rounded_rect_path`), rectangular normal si no.
fn fill_shape(pixmap: &mut Pixmap, rect: &Rect, radius: f32, paint: &Paint<'static>, scroll_offset_y: f32, mask: Option<&Mask>) {
    let Some(sk_rect) = SkiaRect::from_xywh(rect.x, rect.y - scroll_offset_y, rect.width.max(1.0), rect.height.max(1.0)) else { return };
    if let Some(path) = rounded_rect_path(sk_rect, radius) {
        pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), mask);
    } else {
        pixmap.fill_rect(sk_rect, paint, Transform::identity(), mask);
    }
}

fn paint_text(
    pixmap: &mut Pixmap,
    rect: &Rect,
    text: &str,
    color: [u8; 4],
    font_size: f32,
    bold: bool,
    italic: bool,
    font_set: Option<&FontSet>,
    scroll_offset_y: f32,
    mask: Option<&Mask>,
) {
    let paint = paint_of(color);
    let screen_y = rect.y - scroll_offset_y;

    match font_set.and_then(|set| set.pick(bold, italic)) {
        Some(font) => {
            // Mismo `wrap_text` que ya uso el layout para reservar el alto
            // de esta caja (mismo font_size, mismo ancho) - el numero de
            // lineas que se pinta aqui coincide con el que se reservo alli
            // por construccion, no por coincidencia.
            let lines = wrap_text(font, text, font_size, rect.width);
            let line_height = measure_text(font, "", font_size).line_height;
            for (index, line) in lines.iter().enumerate() {
                let line_y = screen_y + index as f32 * line_height;
                for glyph in shape_text(font, line, font_size, rect.x, line_y) {
                    pixmap.fill_path(&glyph.path, &paint, FillRule::Winding, Transform::identity(), mask);
                }
            }
        }
        None => {
            // Sin fuente de sistema disponible: bloque de relleno en vez
            // de fingir que hay glifos.
            if let Some(sk_rect) = SkiaRect::from_xywh(rect.x, screen_y, rect.width.max(1.0), rect.height.max(1.0)) {
                pixmap.fill_rect(sk_rect, &paint, Transform::identity(), mask);
            }
        }
    }
}

fn paint_border(pixmap: &mut Pixmap, border_box: &Rect, width: f32, color: [u8; 4], radius: f32, scroll_offset_y: f32, mask: Option<&Mask>) {
    let paint = paint_of(color);

    if radius > 0.0 {
        // Un border redondeado se pinta como un TRAZO (stroke) sobre el
        // contorno redondeado, no como 4 franjas rectangulares (esas no
        // encajarian en las esquinas) - inset la MITAD del grosor para que
        // el trazo caiga DENTRO del border-box (tiny-skia centra un stroke
        // sobre su path, mitad hacia afuera/mitad hacia adentro por
        // defecto), igual que un border real (nunca se sale de
        // `dimensions`).
        let inset = width / 2.0;
        let inset_w = (border_box.width - width).max(0.0);
        let inset_h = (border_box.height - width).max(0.0);
        if inset_w > 0.0 && inset_h > 0.0 {
            if let Some(sk_rect) = SkiaRect::from_xywh(border_box.x + inset, border_box.y - scroll_offset_y + inset, inset_w, inset_h) {
                let stroke_radius = (radius - inset).max(0.0);
                if let Some(path) = rounded_rect_path(sk_rect, stroke_radius) {
                    let stroke = Stroke { width, ..Default::default() };
                    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
                    return;
                }
            }
        }
        // Radio efectivo <= 0 tras el inset (esquinas practicamente
        // cuadradas a este grosor) o geometria degenerada - cae al
        // trazado recto de abajo, visualmente equivalente a esta escala.
    }

    for strip in border_strip_rects(border_box, width) {
        if let Some(sk_rect) = SkiaRect::from_xywh(strip.x, strip.y - scroll_offset_y, strip.width.max(1.0), strip.height.max(1.0)) {
            pixmap.fill_rect(sk_rect, &paint, Transform::identity(), mask);
        }
    }
}

/// Los 4 rectangulos (arriba/abajo/izquierda/derecha) que forman el marco
/// de un border RECTO (sin `border-radius`) - unica copia compartida (antes
/// de esta tarea vivia duplicada en `raster.rs` y `window.rs`, ver el
/// doc-comment del modulo). TODAS las coordenadas se redondean a PIXEL
/// ENTERO antes de construir los rectangulos (`x`/`y` y el borde OPUESTO
/// `x+width`/`y+height` cada uno por separado, no solo `width`/`height`,
/// para que cajas vecinas sigan encajando sin huecos - "pixel snapping").
///
/// Encontrado en vivo verificando la Fase 3.4 (celdas de tabla con columnas
/// de ancho FRACCIONARIO, p.ej. 500px / 3 = 166.66...px): un border de 1px
/// en una `x` fraccionaria le llega a `tiny_skia::Pixmap::fill_rect` como
/// un rectangulo "hairline" (mas fino que 1px tras redondeo interno de
/// punto fijo) en una posicion no alineada a pixel, lo que dispara un
/// `debug_assert!(false)` real DENTRO de tiny-skia
/// (`scan::hairline_aa::fill_dot8`, tiny-skia 0.11.4) cuando el ancho en
/// punto fijo de la franja se reduce a cero o negativo tras sus propios
/// ajustes de sub-pixel - un limite conocido de su rasterizador de
/// rectangulos finos, no una entrada invalida por nuestra parte.
fn border_strip_rects(border_box: &Rect, width: f32) -> [Rect; 4] {
    let x = border_box.x.round();
    let y = border_box.y.round();
    let right = (border_box.x + border_box.width).round();
    let bottom = (border_box.y + border_box.height).round();
    let w = (right - x).max(0.0);
    let h = (bottom - y).max(0.0);
    let stroke = width.round().max(1.0);

    [
        Rect { x, y, width: w, height: stroke },
        Rect { x, y: bottom - stroke, width: w, height: stroke },
        Rect { x, y, width: stroke, height: h },
        Rect { x: right - stroke, y, width: stroke, height: h },
    ]
}

#[cfg(test)]
mod tests {
    use super::{is_offscreen, item_rect, MARGEN_DESCARTE};

    fn r(y: f32, height: f32) -> Rect {
        Rect { x: 0.0, y, width: 100.0, height }
    }

    /// El punto de la Fase 5: lo que esta muy por debajo del viewport se
    /// descarta sin pintar.
    #[test]
    fn a_box_far_below_the_viewport_is_offscreen() {
        assert!(is_offscreen(&r(5000.0, 20.0), 800, 0.0));
    }

    #[test]
    fn a_box_inside_the_viewport_is_not_offscreen() {
        assert!(!is_offscreen(&r(400.0, 20.0), 800, 0.0));
    }

    /// La otra mitad, la que de verdad importa para no romper nada: hacer
    /// scroll cambia QUE esta fuera. Lo que estaba abajo entra, y lo que
    /// estaba arriba sale.
    #[test]
    fn scrolling_brings_a_far_box_into_view_and_pushes_the_top_one_out() {
        let abajo = r(5000.0, 20.0);
        let arriba = r(10.0, 20.0);
        assert!(is_offscreen(&abajo, 800, 0.0));
        assert!(!is_offscreen(&arriba, 800, 0.0));

        assert!(!is_offscreen(&abajo, 800, 4800.0), "tras bajar hasta ella, deberia pintarse");
        assert!(is_offscreen(&arriba, 800, 4800.0), "lo que quedo muy por encima deberia dejar de pintarse");
    }

    /// Una caja que asoma solo por el borde inferior TIENE que pintarse -
    /// descartarla dejaria una franja en blanco visible.
    #[test]
    fn a_box_straddling_the_bottom_edge_is_still_painted() {
        assert!(!is_offscreen(&r(790.0, 40.0), 800, 0.0));
    }

    /// Y una que asoma por el borde superior, igual.
    #[test]
    fn a_box_straddling_the_top_edge_is_still_painted() {
        assert!(!is_offscreen(&r(-10.0, 40.0), 800, 0.0));
    }

    /// El margen de seguridad existe para sombras y para glifos que
    /// sobresalen del alto de linea: algo justo fuera del viewport, pero
    /// dentro del margen, se sigue pintando.
    #[test]
    fn something_just_outside_the_viewport_but_within_the_margin_is_still_painted() {
        let justo_debajo = r(800.0 + MARGEN_DESCARTE / 2.0, 10.0);
        assert!(!is_offscreen(&justo_debajo, 800, 0.0));
    }

    /// La regla que impide el bug mas peligroso de esta optimizacion:
    /// `PushClip`/`PopClip` no tienen rectangulo con el que descartarlos,
    /// asi que nunca pueden saltarse - si se saltaran, la pila de recorte
    /// se desemparejaria y todo lo posterior se recortaria mal.
    #[test]
    fn clip_items_have_no_cullable_rect_so_they_can_never_be_skipped() {
        use crate::display_list::DisplayItem;
        let push = DisplayItem::PushClip { rect: r(9999.0, 10.0) };
        assert!(item_rect(&push).is_none(), "PushClip no deberia poder descartarse ni estando fuera de pantalla");
        assert!(item_rect(&DisplayItem::PopClip).is_none());
    }

    /// Un item que SI pinta expone su rectangulo, que es lo que permite
    /// descartarlo.
    #[test]
    fn painting_items_expose_their_rect_for_culling() {
        use crate::display_list::DisplayItem;
        let solido = DisplayItem::SolidRect { rect: r(10.0, 20.0), color: [0, 0, 0, 255], radius: 0.0 };
        assert!(item_rect(&solido).is_some());
    }

    use super::*;

    #[test]
    fn border_strip_rects_produces_four_strips_of_the_given_width() {
        let border_box = Rect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 };
        let [top, bottom, left, right] = border_strip_rects(&border_box, 5.0);

        assert_eq!((top.x, top.y, top.width, top.height), (10.0, 20.0, 100.0, 5.0));
        assert_eq!((bottom.x, bottom.y, bottom.width, bottom.height), (10.0, 65.0, 100.0, 5.0));
        assert_eq!((left.x, left.y, left.width, left.height), (10.0, 20.0, 5.0, 50.0));
        assert_eq!((right.x, right.y, right.width, right.height), (105.0, 20.0, 5.0, 50.0));
    }

    #[test]
    fn border_strip_rects_snaps_fractional_coordinates_to_whole_pixels() {
        let border_box = Rect { x: 166.66667, y: 10.3, width: 166.66667, height: 40.7 };
        for strip in border_strip_rects(&border_box, 1.0) {
            assert_eq!(strip.x.fract(), 0.0);
            assert_eq!(strip.y.fract(), 0.0);
            assert_eq!(strip.width.fract(), 0.0);
            assert_eq!(strip.height.fract(), 0.0);
        }
    }

    #[test]
    fn intersect_rects_of_two_overlapping_rects_is_their_common_area() {
        let a = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let b = Rect { x: 50.0, y: 50.0, width: 100.0, height: 100.0 };
        let result = intersect_rects(&a, &b).expect("deberian solaparse");
        assert_eq!((result.x, result.y, result.width, result.height), (50.0, 50.0, 50.0, 50.0));
    }

    #[test]
    fn intersect_rects_of_two_disjoint_rects_is_none() {
        let a = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };
        let b = Rect { x: 100.0, y: 100.0, width: 10.0, height: 10.0 };
        assert!(intersect_rects(&a, &b).is_none());
    }

    #[test]
    fn rounded_rect_path_clamps_radius_to_half_the_shortest_side() {
        // Un radio mayor que la mitad del lado corto (aqui, 40 vs alto 20)
        // no deberia producir un path invalido/degenerado - simplemente se
        // clampa al maximo geometricamente posible (10, mitad de 20).
        let rect = SkiaRect::from_xywh(0.0, 0.0, 100.0, 20.0).unwrap();
        let path = rounded_rect_path(rect, 40.0).expect("un radio positivo deberia producir un path");
        assert!(!path.is_empty());
    }

    #[test]
    fn rounded_rect_path_is_none_for_a_non_positive_radius() {
        let rect = SkiaRect::from_xywh(0.0, 0.0, 100.0, 20.0).unwrap();
        assert!(rounded_rect_path(rect, 0.0).is_none());
        assert!(rounded_rect_path(rect, -5.0).is_none());
    }

    #[test]
    fn build_clip_mask_is_none_when_the_stack_is_empty() {
        assert!(build_clip_mask(100, 100, &[], 0.0).is_none());
    }

    #[test]
    fn build_clip_mask_exists_when_the_stack_has_an_active_clip() {
        let stack = [Rect { x: 10.0, y: 10.0, width: 50.0, height: 50.0 }];
        assert!(build_clip_mask(100, 100, &stack, 0.0).is_some());
    }

    #[test]
    fn paint_display_list_does_not_panic_on_a_box_shadow_and_border_radius() {
        let items = vec![
            DisplayItem::Shadow { rect: Rect { x: 5.0, y: 5.0, width: 50.0, height: 30.0 }, color: [0, 0, 0, 128], radius: 8.0 },
            DisplayItem::SolidRect { rect: Rect { x: 0.0, y: 0.0, width: 50.0, height: 30.0 }, color: [255, 255, 255, 255], radius: 8.0 },
            DisplayItem::Border { rect: Rect { x: 0.0, y: 0.0, width: 50.0, height: 30.0 }, width: 2.0, color: [0, 0, 0, 255], radius: 8.0 },
            DisplayItem::PushClip { rect: Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 } },
            DisplayItem::SolidRect { rect: Rect { x: 0.0, y: 0.0, width: 50.0, height: 30.0 }, color: [255, 0, 0, 255], radius: 0.0 },
            DisplayItem::PopClip,
        ];
        let mut pixmap = Pixmap::new(80, 60).unwrap();
        // La asercion real de este test es que la linea de abajo NO haga
        // panic (regresion de sombra/radio/recorte combinados) - un PNG
        // codificable de por medio confirma ademas que el pixmap quedo en
        // un estado valido tras pintar.
        paint_display_list(&mut pixmap, &items, None, 0.0);
        assert!(pixmap.encode_png().is_ok());
    }
}
