//! La ventana nativa: `winit` para la ventana y los eventos, `softbuffer` para
//! poner en pantalla la imagen que compone `App` (barra + página).

use crate::app::{App, CursorTipo, Entrada, Modificadores, Tecla, PIXELES_POR_LINEA};
use crate::motor::{MensajeMotor, Motor};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, WindowBuilder};

/// Abre la ventana y no vuelve hasta que se cierra.
pub fn ejecutar(inicial: Option<String>) -> Result<(), String> {
    let event_loop = EventLoopBuilder::<MensajeMotor>::with_user_event().build().map_err(|e| format!("no se pudo abrir la ventana: {e}"))?;
    let proxy = event_loop.create_proxy();
    let mut motor = Motor::lanzar_este(move |mensaje| {
        let _ = proxy.send_event(mensaje);
    })
    .map_err(|e| format!("no se pudo arrancar el motor: {e}"))?;

    let ventana = Rc::new(
        WindowBuilder::new()
            .with_title("Navegador")
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0))
            .build(&event_loop)
            .map_err(|e| format!("no se pudo abrir la ventana: {e}"))?,
    );
    let contexto = softbuffer::Context::new(ventana.clone()).map_err(|e| format!("no se pudo preparar el pintado: {e}"))?;
    let mut superficie = softbuffer::Surface::new(&contexto, ventana.clone()).map_err(|e| format!("no se pudo preparar el pintado: {e}"))?;

    // La app trabaja en píxeles CSS para el layout/eventos y rasteriza
    // directamente a los píxeles físicos de la pantalla (`escala`).
    let mut escala = ventana.scale_factor().clamp(0.5, 4.0);
    let tamano = ventana.inner_size();
    if let (Some(w), Some(h)) = (NonZeroU32::new(tamano.width), NonZeroU32::new(tamano.height)) {
        let _ = superficie.resize(w, h);
    }
    let (ancho, alto) = logico(tamano.width, tamano.height, escala);
    let (mut app, peticiones) = App::nueva_con_escala(ancho, alto, escala as f32, inicial);
    enviar(&mut motor, &peticiones);

    let mut raton = (0.0_f32, 0.0_f32);
    let mut inicio_arrastre: Option<(f32, f32)> = None;
    let mut arrastrando_barra = false;
    let mut cursor_actual = CursorTipo::Normal;
    let mut modificadores = ModifiersState::empty();
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop
        .run(move |evento, bucle| {
            let entrada = match evento {
                Event::UserEvent(mensaje) => Some(Entrada::Motor(mensaje)),
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => {
                        motor.cerrar();
                        bucle.exit();
                        None
                    }
                    WindowEvent::Resized(tamano) => {
                        if let (Some(w), Some(h)) = (NonZeroU32::new(tamano.width), NonZeroU32::new(tamano.height)) {
                            let _ = superficie.resize(w, h);
                        }
                        let (ancho, alto) = logico(tamano.width, tamano.height, escala);
                        Some(Entrada::Tamano { ancho, alto })
                    }
                    WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                        escala = scale_factor.clamp(0.5, 4.0);
                        let salida_escala = app.manejar(Entrada::Escala(escala as f32));
                        enviar(&mut motor, &salida_escala.peticiones);
                        let tamano = ventana.inner_size();
                        if let (Some(w), Some(h)) = (NonZeroU32::new(tamano.width), NonZeroU32::new(tamano.height)) {
                            let _ = superficie.resize(w, h);
                        }
                        let (ancho, alto) = logico(tamano.width, tamano.height, escala);
                        let salida_tam = app.manejar(Entrada::Tamano { ancho, alto });
                        enviar(&mut motor, &salida_tam.peticiones);
                        if salida_escala.redibujar || salida_tam.redibujar {
                            ventana.set_title(&app.titulo_ventana());
                            ventana.request_redraw();
                        }
                        None
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        raton = ((position.x / escala) as f32, (position.y / escala) as f32);
                        let nuevo = app.cursor_en(raton.0, raton.1);
                        if nuevo != cursor_actual {
                            cursor_actual = nuevo;
                            ventana.set_cursor_icon(match nuevo {
                                CursorTipo::Normal => CursorIcon::Default,
                                CursorTipo::Mano => CursorIcon::Pointer,
                                CursorTipo::Texto => CursorIcon::Text,
                            });
                        }
                        if arrastrando_barra {
                            Some(Entrada::ArrastrarBarra { x: raton.0, y: raton.1 })
                        } else if inicio_arrastre.is_none() {
                            Some(Entrada::Raton { x: raton.0, y: raton.1 })
                        } else {
                            None
                        }
                    }
                    WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                        if raton.1 >= crate::barra::ALTO_BARRA as f32 {
                            inicio_arrastre = Some(raton);
                            None
                        } else {
                            if raton.1 < crate::barra::ALTO_PESTANAS as f32 {
                                arrastrando_barra = true;
                            }
                            Some(Entrada::Clic { x: raton.0, y: raton.1 })
                        }
                    }
                    WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                        if arrastrando_barra {
                            arrastrando_barra = false;
                            Some(Entrada::SoltarBarra)
                        } else if let Some((x0, y0)) = inicio_arrastre.take() {
                            if (raton.0 - x0).hypot(raton.1 - y0) >= 5.0 {
                                Some(Entrada::SeleccionarPagina { desde_x: x0, desde_y: y0, hasta_x: raton.0, hasta_y: raton.1 })
                            } else {
                                Some(Entrada::Clic { x: raton.0, y: raton.1 })
                            }
                        } else {
                            None
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let dy = match delta {
                            MouseScrollDelta::LineDelta(_, y) => -y * PIXELES_POR_LINEA,
                            MouseScrollDelta::PixelDelta(p) => (-p.y / escala) as f32,
                        };
                        Some(Entrada::Rueda { dy })
                    }
                    WindowEvent::ModifiersChanged(m) => {
                        modificadores = m.state();
                        None
                    }
                    WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => tecla_de(&event).map(|tecla| Entrada::Tecla {
                        tecla,
                        mods: Modificadores { ctrl: modificadores.control_key() || modificadores.super_key(), alt: modificadores.alt_key(), shift: modificadores.shift_key() },
                    }),
                    WindowEvent::RedrawRequested => {
                        let tamano = ventana.inner_size();
                        pintar(&mut app, &mut superficie, tamano.width, tamano.height);
                        None
                    }
                    _ => None,
                },
                _ => None,
            };
            if let Some(entrada) = entrada {
                let salida = app.manejar(entrada);
                enviar(&mut motor, &salida.peticiones);
                if salida.redibujar {
                    let nuevo = app.cursor_en(raton.0, raton.1);
                    if nuevo != cursor_actual {
                        cursor_actual = nuevo;
                        ventana.set_cursor_icon(match nuevo {
                            CursorTipo::Normal => CursorIcon::Default,
                            CursorTipo::Mano => CursorIcon::Pointer,
                            CursorTipo::Texto => CursorIcon::Text,
                        });
                    }
                    ventana.set_title(&app.titulo_ventana());
                    ventana.request_redraw();
                }
            }
        })
        .map_err(|e| e.to_string())
}

fn enviar(motor: &mut Motor, peticiones: &[serde_json::Value]) {
    for peticion in peticiones {
        if let Err(error) = motor.enviar(peticion) {
            eprintln!("[navegador] no se pudo hablar con el motor: {error}");
        }
    }
}

/// Tamaño en píxeles CSS de una ventana de `ancho`×`alto` píxeles físicos.
fn logico(ancho: u32, alto: u32, escala: f64) -> (u32, u32) {
    (((f64::from(ancho) / escala).round() as u32).max(1), ((f64::from(alto) / escala).round() as u32).max(1))
}

fn pintar(app: &mut App, superficie: &mut softbuffer::Surface<Rc<winit::window::Window>, Rc<winit::window::Window>>, ancho: u32, alto: u32) {
    let mut marco = app.componer();
    if (marco.width(), marco.height()) != (ancho, alto) {
        marco = ampliar(&marco, ancho, alto).unwrap_or(marco);
    }
    let Ok(mut buffer) = superficie.buffer_mut() else { return };
    for (destino, origen) in buffer.iter_mut().zip(marco.pixels()) {
        let c = origen.demultiply();
        *destino = (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue());
    }
    let _ = buffer.present();
}

/// Amplía (o reduce) la imagen al tamaño físico de la ventana.
fn ampliar(marco: &tiny_skia::Pixmap, ancho: u32, alto: u32) -> Option<tiny_skia::Pixmap> {
    let mut destino = tiny_skia::Pixmap::new(ancho.max(1), alto.max(1))?;
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    let transformacion = tiny_skia::Transform::from_scale(ancho as f32 / marco.width() as f32, alto as f32 / marco.height() as f32);
    destino.draw_pixmap(0, 0, marco.as_ref(), &paint, transformacion, None);
    Some(destino)
}

/// La tecla de `winit` en el idioma de la app.
pub fn tecla_de(evento: &KeyEvent) -> Option<Tecla> {
    Some(match &evento.logical_key {
        Key::Named(nombre) => match nombre {
            NamedKey::Enter => Tecla::Enter,
            NamedKey::Escape => Tecla::Escape,
            NamedKey::Backspace => Tecla::Retroceso,
            NamedKey::Delete => Tecla::Suprimir,
            NamedKey::ArrowLeft => Tecla::Izquierda,
            NamedKey::ArrowRight => Tecla::Derecha,
            NamedKey::ArrowUp => Tecla::Arriba,
            NamedKey::ArrowDown => Tecla::Abajo,
            NamedKey::Home => Tecla::Inicio,
            NamedKey::End => Tecla::Fin,
            NamedKey::PageDown => Tecla::AvPag,
            NamedKey::PageUp => Tecla::RePag,
            NamedKey::Tab => Tecla::Tab,
            NamedKey::F5 => Tecla::F5,
            NamedKey::F6 => Tecla::F6,
            NamedKey::Space => Tecla::Caracter(" ".to_string()),
            _ => return None,
        },
        Key::Character(texto) => Tecla::Caracter(texto.to_string()),
        _ => return None,
    })
}
