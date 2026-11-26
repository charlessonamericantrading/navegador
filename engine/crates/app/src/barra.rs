//! La barra del navegador: pestañas, atrás/adelante, recargar/detener y la
//! barra de direcciones. Se escribe en HTML/CSS y la pinta nuestro motor; los
//! iconos son SVG que también decodifica el motor (`engine-image`).

use crate::lienzo::{caja_con_id, escapar, pintar_html_escalado, Lienzo};
use engine_gfx::DisplayItem;
use engine_layout::{ImageMap, Rect};
use engine_text::{FontSet, SystemFont};

pub const ALTO_PESTANAS: u32 = 36;
pub const ALTO_HERRAMIENTAS: u32 = 42;
/// Alto total de la barra: la página empieza justo debajo.
pub const ALTO_BARRA: u32 = ALTO_PESTANAS + ALTO_HERRAMIENTAS + 2;

const TAM_TEXTO_DIRECCION: f32 = 14.0;
const TAM_TEXTO_PESTANA: f32 = 12.0;
/// Hueco a la izquierda: 8 de margen + tres botones de 32 + 4 de separación.
const X_DIRECCION: f32 = 8.0 + 3.0 * 36.0 + 4.0;
/// Relleno (16 + 16) y borde (2 + 2) del campo de dirección.
const EXTRA_DIRECCION: f32 = 36.0;

/// Una pestaña tal como la muestra la barra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pestana {
    pub id: u32,
    pub titulo: String,
    pub url: String,
}

/// Lo que se puede pulsar en la barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accion {
    Atras,
    Adelante,
    Recargar,
    Detener,
    Direccion,
    NuevaPestana,
    Pestana(u32),
    CerrarPestana(u32),
}

impl Accion {
    /// Lee el valor de `data-accion`.
    pub fn desde(texto: &str) -> Option<Self> {
        let (nombre, argumento) = texto.split_once(':').unwrap_or((texto, ""));
        let numero = || argumento.parse::<u32>().ok();
        Some(match nombre {
            "atras" => Self::Atras,
            "adelante" => Self::Adelante,
            "recargar" => Self::Recargar,
            "detener" => Self::Detener,
            "direccion" => Self::Direccion,
            "nueva" => Self::NuevaPestana,
            "pestana" => Self::Pestana(numero()?),
            "cerrar" => Self::CerrarPestana(numero()?),
            _ => return None,
        })
    }
}

/// Todo lo que la barra necesita para pintarse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VistaBarra {
    pub pestanas: Vec<Pestana>,
    pub activa: u32,
    /// Lo que se ve en la barra de direcciones: la URL o lo que se escribe.
    pub direccion: String,
    pub editando: bool,
    /// Posición del cursor, en caracteres.
    pub cursor: usize,
    pub todo_seleccionado: bool,
    pub cargando: bool,
    pub puede_atras: bool,
    pub puede_adelante: bool,
}

/// Dónde quedó el texto de la dirección al pintarlo (para poner el cursor
/// donde se pulsa).
#[derive(Debug, Clone, PartialEq)]
pub struct CampoPintado {
    pub x: f32,
    /// Primer carácter visible (si el texto no cabe, se ve un trozo).
    pub inicio: usize,
    pub visible: String,
}

pub struct BarraPintada {
    pub lienzo: Lienzo,
    pub campo: Option<CampoPintado>,
}

impl BarraPintada {
    pub fn accion_en(&self, x: f32, y: f32) -> Option<Accion> {
        crate::lienzo::accion_en(&self.lienzo.regiones, x, y).and_then(Accion::desde)
    }
}

/// Los iconos de la barra, decodificados por el motor a escala 1x.
pub fn iconos() -> ImageMap {
    iconos_con_escala(1.0)
}

/// Los iconos de la barra decodificados directamente al tamaño físico que pide
/// la pantalla (`escala`), para que se vean nítidos en pantallas HiDPI.
pub fn iconos_con_escala(escala: f32) -> ImageMap {
    let escala = if escala.is_finite() && escala > 0.0 { escala.clamp(0.5, 4.0) } else { 1.0 };
    let tam_20 = ((20.0 * escala).round() as u32).max(16);
    let tam_16 = ((16.0 * escala).round() as u32).max(12);
    const OSCURO: &str = "#3c4043";
    const APAGADO: &str = "#bdc1c6";
    let trazos = [
        ("atras", "M19 12H5M11 6l-6 6 6 6"),
        ("adelante", "M5 12h14M13 6l6 6-6 6"),
        ("recargar", "M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4.5H15"),
        ("detener", "M6 6l12 12M18 6L6 18"),
        ("nueva", "M12 5v14M5 12h14"),
    ];
    let mut mapa = ImageMap::new();
    for (nombre, trazo) in trazos {
        for (sufijo, color) in [("", OSCURO), ("-off", APAGADO)] {
            if let Some(imagen) = engine_image::decode_svg(svg(trazo, color, tam_20).as_bytes()) {
                mapa.insert(format!("icono:{nombre}{sufijo}"), imagen);
            }
        }
    }
    if let Some(imagen) = engine_image::decode_svg(svg("M7 7l10 10M17 7L7 17", OSCURO, tam_16).as_bytes()) {
        mapa.insert("icono:cerrar".to_string(), imagen);
    }
    mapa
}

fn svg(trazo: &str, color: &str, tam: u32) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{tam}" height="{tam}" viewBox="0 0 24 24"><path d="{trazo}" fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
    )
}

/// Ancho de cada pestaña: hasta 220 px, menos si no caben.
fn ancho_pestana(ancho: u32, n: usize) -> f32 {
    let disponible = ancho as f32 - 8.0 - 40.0;
    (disponible / n.max(1) as f32 - 2.0).clamp(60.0, 220.0).floor()
}

fn ancho_direccion(ancho: u32) -> f32 {
    (ancho as f32 - X_DIRECCION - 8.0 - EXTRA_DIRECCION).max(40.0).floor()
}

/// El HTML de la barra.
pub fn html(vista: &VistaBarra, ancho: u32, fuente: Option<&SystemFont>) -> (String, Option<(usize, String)>) {
    let medir = |texto: &str, tam: f32| fuente.map_or(texto.chars().count() as f32 * tam * 0.55, |f| engine_text::text_width(f, texto, tam));
    let tw = ancho_pestana(ancho, vista.pestanas.len());
    let aw = ancho_direccion(ancho);

    let mut pestanas = String::new();
    let varias = vista.pestanas.len() > 1;
    for p in &vista.pestanas {
        let clase = if p.id == vista.activa { "pestana activa" } else { "pestana" };
        let titulo = if p.titulo.trim().is_empty() {
            if p.url.is_empty() { "Nueva pestaña" } else { p.url.as_str() }
        } else {
            p.titulo.as_str()
        };
        let ancho_titulo = tw - 12.0 - if varias { 26.0 } else { 10.0 };
        let titulo = recortar(titulo, ancho_titulo, |t| medir(t, TAM_TEXTO_PESTANA));
        let cerrar = if varias {
            format!(r#"<div class="cerrar" data-accion="cerrar:{}"><img src="icono:cerrar" width="16" height="16"></div>"#, p.id)
        } else {
            String::new()
        };
        pestanas.push_str(&format!(
            r#"<div class="{clase}" data-accion="pestana:{id}"><div class="titulo" style="width:{ancho_caja}px">{titulo}</div>{cerrar}</div>"#,
            id = p.id,
            ancho_caja = ancho_titulo + 12.0,
            titulo = escapar(&titulo),
        ));
    }

    let icono = |nombre: &str, activo: bool| if activo { format!("icono:{nombre}") } else { format!("icono:{nombre}-off") };
    let (recargar, icono_recargar) = if vista.cargando { ("detener", icono("detener", true)) } else { ("recargar", icono("recargar", true)) };

    // Texto de la dirección: si no cabe, se ve el trozo que lleva el cursor.
    let disponible = aw - 4.0;
    let (texto_visible, campo, color_texto) = if vista.editando {
        let chars: Vec<char> = vista.direccion.chars().collect();
        let cursor = vista.cursor.min(chars.len());
        let (inicio, fin) = ventana_visible(&chars, cursor, disponible, |t| medir(t, TAM_TEXTO_DIRECCION));
        let visible: String = chars[inicio..fin].iter().collect();
        (visible.clone(), Some((inicio, visible)), "#202124")
    } else if vista.direccion.is_empty() {
        ("Busca o escribe una dirección".to_string(), None, "#80868b")
    } else {
        (recortar(&vista.direccion, disponible, |t| medir(t, TAM_TEXTO_DIRECCION)), None, "#202124")
    };
    let (borde, fondo) = if vista.editando { ("#1a73e8", "#ffffff") } else { ("#f1f3f4", "#f1f3f4") };
    let progreso = if vista.cargando { "#1a73e8" } else { "#dadce0" };

    let html = format!(
        r#"<html><head><style>
body {{ margin: 0; font-family: sans-serif; background: #dfe3e8; }}
#pestanas {{ display: flex; height: {alto_pestanas}px; padding-left: 8px; }}
.pestana {{ display: flex; width: {tw}px; height: 30px; margin-top: 6px; margin-right: 2px; border-radius: 8px; color: #3c4043; font-size: 12px; }}
.activa {{ background: #ffffff; color: #202124; }}
.titulo {{ box-sizing: border-box; height: 30px; padding-left: 12px; line-height: 30px; white-space: nowrap; }}
.cerrar {{ width: 16px; height: 16px; margin-top: 7px; border-radius: 8px; }}
.nueva {{ box-sizing: border-box; width: 30px; height: 30px; padding: 5px; margin-top: 3px; margin-left: 4px; border-radius: 15px; }}
#herramientas {{ display: flex; height: {alto_herramientas}px; background: #ffffff; padding-left: 8px; }}
.boton {{ box-sizing: border-box; width: 32px; height: 32px; padding: 6px; margin-top: 5px; margin-right: 4px; border-radius: 16px; }}
#direccion {{ box-sizing: border-box; width: {ancho_total}px; height: 32px; margin-top: 5px; margin-left: 4px; padding-left: 16px; padding-right: 16px; border: 2px solid {borde}; border-radius: 16px; background: {fondo}; }}
#texto-direccion {{ font-size: 14px; line-height: 28px; height: 28px; color: {color_texto}; white-space: nowrap; }}
#progreso {{ height: 2px; background: {progreso}; }}
</style></head><body>
<div id="pestanas">{pestanas}<div class="nueva" data-accion="nueva"><img src="icono:nueva" width="20" height="20"></div></div>
<div id="herramientas">
<div class="boton" data-accion="atras"><img src="{atras}" width="20" height="20"></div>
<div class="boton" data-accion="adelante"><img src="{adelante}" width="20" height="20"></div>
<div class="boton" data-accion="{recargar}"><img src="{icono_recargar}" width="20" height="20"></div>
<div id="direccion" data-accion="direccion"><div id="texto-direccion">{texto}</div></div>
</div>
<div id="progreso"></div>
</body></html>"#,
        alto_pestanas = ALTO_PESTANAS,
        alto_herramientas = ALTO_HERRAMIENTAS,
        atras = icono("atras", vista.puede_atras),
        adelante = icono("adelante", vista.puede_adelante),
        texto = escapar(&texto_visible),
        ancho_total = aw + EXTRA_DIRECCION,
    );
    (html, campo)
}

/// Pinta la barra entera con el motor a escala 1x.
pub fn pintar(vista: &VistaBarra, ancho: u32, fuentes: &FontSet, iconos: &ImageMap) -> Result<BarraPintada, String> {
    pintar_con_escala(vista, ancho, 1.0, fuentes, iconos)
}

/// Pinta la barra entera con el motor a `escala` píxeles físicos por CSS.
pub fn pintar_con_escala(vista: &VistaBarra, ancho: u32, escala: f32, fuentes: &FontSet, iconos: &ImageMap) -> Result<BarraPintada, String> {
    let fuente = fuentes.pick(false, false);
    let (html, campo) = html(vista, ancho, fuente);
    let mut campo_pintado = None;
    let lienzo = pintar_html_escalado(&html, ancho, ALTO_BARRA, escala, fuentes, iconos, |layout| {
        let (Some((inicio, visible)), Some(caja)) = (campo.as_ref(), caja_con_id(layout, "texto-direccion")) else {
            return Vec::new();
        };
        let x = caja.dimensions.x;
        let y = caja.dimensions.y;
        let medir = |t: &str| fuente.map_or(0.0, |f| engine_text::text_width(f, t, TAM_TEXTO_DIRECCION));
        campo_pintado = Some(CampoPintado { x, inicio: *inicio, visible: visible.clone() });
        let mut encima = Vec::new();
        if vista.todo_seleccionado && !visible.is_empty() {
            encima.push(DisplayItem::SolidRect {
                rect: Rect { x, y: y + 5.0, width: medir(visible), height: 18.0 },
                color: [26, 115, 232, 70],
                radius: 0.0,
            });
        }
        let antes: String = visible.chars().take(vista.cursor.saturating_sub(*inicio)).collect();
        encima.push(DisplayItem::SolidRect {
            rect: Rect { x: (x + medir(&antes)).round(), y: y + 6.0, width: 1.0, height: 16.0 },
            color: [32, 33, 36, 255],
            radius: 0.0,
        });
        encima
    })?;
    Ok(BarraPintada { lienzo, campo: campo_pintado })
}

/// El carácter más cercano a `x` dentro del campo pintado.
pub fn cursor_en_x(campo: &CampoPintado, x: f32, fuentes: &FontSet) -> usize {
    let Some(fuente) = fuentes.pick(false, false) else { return campo.inicio };
    let chars: Vec<char> = campo.visible.chars().collect();
    let mut mejor = (0usize, f32::MAX);
    for i in 0..=chars.len() {
        let prefijo: String = chars[..i].iter().collect();
        let distancia = (campo.x + engine_text::text_width(fuente, &prefijo, TAM_TEXTO_DIRECCION) - x).abs();
        if distancia < mejor.1 {
            mejor = (i, distancia);
        }
    }
    campo.inicio + mejor.0
}

/// El texto entero si cabe; si no, recortado con "…".
fn recortar(texto: &str, max: f32, medir: impl Fn(&str) -> f32) -> String {
    if medir(texto) <= max {
        return texto.to_string();
    }
    let chars: Vec<char> = texto.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let candidato: String = chars[..n].iter().collect::<String>() + "…";
        if medir(&candidato) <= max {
            return candidato;
        }
    }
    String::new()
}

/// Qué trozo del texto se ve para que el cursor quede siempre a la vista.
fn ventana_visible(chars: &[char], cursor: usize, max: f32, medir: impl Fn(&str) -> f32) -> (usize, usize) {
    let trozo = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    if medir(&trozo(0, chars.len())) <= max {
        return (0, chars.len());
    }
    let mut inicio = 0;
    while inicio < cursor && medir(&trozo(inicio, cursor)) > max - 8.0 {
        inicio += 1;
    }
    let mut fin = cursor;
    while fin < chars.len() && medir(&trozo(inicio, fin + 1)) <= max {
        fin += 1;
    }
    (inicio, fin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vista(pestanas: usize) -> VistaBarra {
        VistaBarra {
            pestanas: (0..pestanas as u32).map(|id| Pestana { id, titulo: format!("Página {id}"), url: format!("https://e{id}.com") }).collect(),
            activa: 0,
            direccion: "https://e0.com".to_string(),
            editando: false,
            cursor: 0,
            todo_seleccionado: false,
            cargando: false,
            puede_atras: true,
            puede_adelante: false,
        }
    }

    fn centro(barra: &BarraPintada, accion: &str) -> (f32, f32) {
        let region = barra.lienzo.regiones.iter().find(|r| r.accion == accion).unwrap_or_else(|| panic!("falta la zona {accion}: {:?}", barra.lienzo.regiones));
        (region.rect.x + region.rect.width / 2.0, region.rect.y + region.rect.height / 2.0)
    }

    #[test]
    fn cada_boton_tiene_su_zona_y_su_accion() {
        let fuentes = FontSet::load_default_sans_serif();
        let barra = pintar(&vista(2), 1000, &fuentes, &iconos()).unwrap();
        for (zona, accion) in [
            ("atras", Accion::Atras),
            ("adelante", Accion::Adelante),
            ("recargar", Accion::Recargar),
            ("direccion", Accion::Direccion),
            ("nueva", Accion::NuevaPestana),
            ("cerrar:1", Accion::CerrarPestana(1)),
        ] {
            let (x, y) = centro(&barra, zona);
            assert_eq!(barra.accion_en(x, y), Some(accion), "zona {zona} en ({x}, {y})");
        }
        // La pestaña se pulsa por su título (a la izquierda del botón de cerrar).
        let pestana = barra.lienzo.regiones.iter().find(|r| r.accion == "pestana:1").unwrap();
        assert_eq!(barra.accion_en(pestana.rect.x + 20.0, pestana.rect.y + 15.0), Some(Accion::Pestana(1)));
    }

    #[test]
    fn la_barra_queda_en_su_sitio() {
        let fuentes = FontSet::load_default_sans_serif();
        let barra = pintar(&vista(1), 1000, &fuentes, &iconos()).unwrap();
        let (x, y) = centro(&barra, "direccion");
        assert!(y > ALTO_PESTANAS as f32 && y < ALTO_BARRA as f32, "la dirección va en la fila de herramientas: y = {y}");
        assert!(x > X_DIRECCION && x < 1000.0, "x = {x}");
        // Con una sola pestaña no hay botón de cerrar.
        assert!(!barra.lienzo.regiones.iter().any(|r| r.accion.starts_with("cerrar")));
        // Mientras carga, el botón de recargar es el de detener.
        let cargando = VistaBarra { cargando: true, ..vista(1) };
        let barra = pintar(&cargando, 1000, &fuentes, &iconos()).unwrap();
        let (x, y) = centro(&barra, "detener");
        assert_eq!(barra.accion_en(x, y), Some(Accion::Detener));
    }

    #[test]
    fn el_campo_pinta_el_cursor_donde_toca() {
        let fuentes = FontSet::load_default_sans_serif();
        let editando = VistaBarra { editando: true, direccion: "hola".to_string(), cursor: 2, ..vista(1) };
        let barra = pintar(&editando, 800, &fuentes, &iconos()).unwrap();
        let campo = barra.campo.expect("al editar se sabe dónde está el texto");
        assert_eq!((campo.inicio, campo.visible.as_str()), (0, "hola"));
        // Pulsar a la izquierda del texto lleva el cursor al principio; muy a
        // la derecha, al final.
        assert_eq!(cursor_en_x(&campo, campo.x - 30.0, &fuentes), 0);
        assert_eq!(cursor_en_x(&campo, campo.x + 500.0, &fuentes), 4);
    }

    #[test]
    fn un_texto_largo_muestra_el_trozo_del_cursor() {
        let medir = |t: &str| t.chars().count() as f32 * 10.0;
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
        // Caben 10 caracteres. Con el cursor al final se ve el final.
        let (inicio, fin) = ventana_visible(&chars, 26, 100.0, medir);
        assert_eq!(fin, 26);
        assert!((16..=18).contains(&inicio), "inicio = {inicio}");
        // Con el cursor al principio se ve el principio.
        assert_eq!(ventana_visible(&chars, 0, 100.0, medir), (0, 10));
        assert_eq!(recortar("abcdefghij", 50.0, medir), "abcd…");
        assert_eq!(recortar("abc", 50.0, medir), "abc");
    }

    #[test]
    fn acciones_desde_texto() {
        assert_eq!(Accion::desde("pestana:7"), Some(Accion::Pestana(7)));
        assert_eq!(Accion::desde("cerrar:x"), None);
        assert_eq!(Accion::desde("otra"), None);
    }
}
