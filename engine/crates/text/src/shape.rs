//! Shaping real de texto: convierte una cadena + una fuente en una lista de
//! glifos posicionados, con sus contornos ya rasterizables por tiny-skia.
//!
//! El shaping en si (que glifo va con que caracter, cuanto avanza cada uno -
//! ligaduras, kerning) lo hace `rustybuzz`. Lo unico que se escribe a mano
//! aqui es el puente entre el sistema de coordenadas de una fuente
//! (origen en el baseline, eje Y hacia arriba, unidades de "em") y el de la
//! pantalla (origen arriba a la izquierda, eje Y hacia abajo, pixeles).

use crate::font::SystemFont;
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};

#[derive(Debug, Clone)]
pub struct PositionedGlyph {
    pub path: tiny_skia::Path,
}

/// Ancho total (una sola linea) y alto de linea de un texto ya shapeado,
/// sin construir los contornos de cada glifo - mas barato que `shape_text`
/// para cuando solo hace falta saber cuanto espacio ocupa, no pintarlo. El
/// layout (`engine-layout`) necesita esto antes de pintar, en cada frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    pub width: f32,
    /// Alto tipografico de una linea (ascenso + descenso + salto de linea
    /// de la fuente, escalados a `font_size`) - no el alto visual de tinta
    /// de ningun glifo concreto (que varia segun tenga descendentes o no),
    /// el alto de caja real que usaria un navegador para apilar lineas.
    pub line_height: f32,
}

/// Hace el shaping (que glifo, cuanto avanza cada uno) una sola vez;
/// `shape_text` y `measure_text` construyen su resultado a partir de esto
/// sin repetir la llamada a `rustybuzz::shape`.
struct ShapedRun<'a> {
    face: rustybuzz::Face<'a>,
    scale: f32,
    glyph_buffer: rustybuzz::GlyphBuffer,
}

fn shape_run<'a>(face: rustybuzz::Face<'a>, text: &str, font_size: f32) -> Option<ShapedRun<'a>> {
    let units_per_em = face.units_per_em() as f32;
    if units_per_em <= 0.0 {
        return None;
    }
    let scale = font_size / units_per_em;

    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let glyph_buffer = rustybuzz::shape(&face, &[], buffer);

    Some(ShapedRun { face, scale, glyph_buffer })
}

/// Shape `text` con `font` a tamaño `font_size` (px), con el origen de la
/// linea en (`origin_x`, `line_top_y`) - `line_top_y` es la parte SUPERIOR
/// de la caja de texto (lo que ya calcula el layout), no el baseline; el
/// ascenso real de la fuente se usa para bajar hasta el baseline.
pub fn shape_text(font: &SystemFont, text: &str, font_size: f32, origin_x: f32, line_top_y: f32) -> Vec<PositionedGlyph> {
    let Some(face) = font.rustybuzz_face() else {
        tracing::warn!("[engine-text] No se pudo reconstruir rustybuzz::Face desde los bytes cargados");
        return Vec::new();
    };
    let Some(run) = shape_run(face, text, font_size) else { return Vec::new() };
    let baseline_y = line_top_y + run.face.ascender() as f32 * run.scale;

    let mut glyphs = Vec::new();
    let mut pen_x = origin_x;
    for (info, pos) in run.glyph_buffer.glyph_infos().iter().zip(run.glyph_buffer.glyph_positions()) {
        let glyph_id = GlyphId(info.glyph_id as u16);
        let mut outline = GlyphOutline::new(
            pen_x + pos.x_offset as f32 * run.scale,
            baseline_y - pos.y_offset as f32 * run.scale,
            run.scale,
        );
        if run.face.outline_glyph(glyph_id, &mut outline).is_some() {
            if let Some(path) = outline.finish() {
                glyphs.push(PositionedGlyph { path });
            }
        }
        pen_x += pos.x_advance as f32 * run.scale;
    }
    glyphs
}

/// Ver `TextMetrics`. Si no se puede reconstruir la fuente o son 0 unidades
/// por em (fuente corrupta/incompleta), devuelve `width: 0.0` y
/// `line_height: font_size` como aproximacion honesta de respaldo - no
/// pretende medir un texto que no se pudo shapear.
pub fn measure_text(font: &SystemFont, text: &str, font_size: f32) -> TextMetrics {
    let fallback = TextMetrics { width: 0.0, line_height: font_size };

    let Some(face) = font.rustybuzz_face() else {
        tracing::warn!("[engine-text] No se pudo reconstruir rustybuzz::Face desde los bytes cargados");
        return fallback;
    };
    let Some(run) = shape_run(face, text, font_size) else { return fallback };

    let width: f32 = run.glyph_buffer.glyph_positions().iter().map(|pos| pos.x_advance as f32 * run.scale).sum();
    let line_height = (run.face.ascender() as f32 - run.face.descender() as f32 + run.face.line_gap() as f32) * run.scale;

    TextMetrics { width, line_height }
}

/// El desplazamiento vertical desde la parte SUPERIOR de una linea (el
/// mismo `line_top_y` que recibe `shape_text`) hasta su BASELINE - el
/// mismo calculo que `shape_text` hace internamente (`ascender * scale`),
/// expuesto aparte para quien necesite el baseline SIN construir ningun
/// glifo (`engine-gfx::paint_text`, para posicionar el subrayado de
/// `text-decoration: underline`, Fase 29). `None` en los mismos dos casos
/// de respaldo que `measure_text` (fuente no reconstruible, o 0 unidades
/// por em).
pub fn baseline_offset(font: &SystemFont, font_size: f32) -> Option<f32> {
    let face = font.rustybuzz_face()?;
    let units_per_em = face.units_per_em() as f32;
    if units_per_em <= 0.0 {
        return None;
    }
    let scale = font_size / units_per_em;
    Some(face.ascender() as f32 * scale)
}

/// Posicion y grosor REALES del subrayado (`text-decoration: underline`,
/// Fase 29), leidos de la propia tabla de metricas de la fuente
/// (`post`/`OS-2` via `ttf_parser::Face::underline_metrics`) en vez de una
/// fraccion inventada de `font_size` - cada fuente declara su propia
/// posicion/grosor de subrayado, y usar el valor real es lo que hace que
/// quede pegado al texto igual que en un navegador real, no flotando a
/// una distancia arbitraria.
///
/// Devuelve `(desplazamiento_bajo_baseline_px, grosor_px)`, los DOS ya
/// escalados a `font_size` y en coordenadas de PANTALLA (desplazamiento
/// POSITIVO = hacia ABAJO desde el baseline - la convencion de la propia
/// fuente es la contraria, `position` viene NEGATIVO porque su eje Y
/// apunta hacia arriba, de ahi el signo invertido de mas abajo).
///
/// `None` si la fuente no se pudo reconstruir, tiene 0 unidades por em, o
/// no declara metricas de subrayado (fuentes incompletas/sinteticas) -
/// quien llama cae entonces a una aproximacion basada solo en
/// `font_size`, igual criterio de respaldo que el resto de este modulo.
pub fn underline_metrics(font: &SystemFont, font_size: f32) -> Option<(f32, f32)> {
    let face = font.rustybuzz_face()?;
    let units_per_em = face.units_per_em() as f32;
    if units_per_em <= 0.0 {
        return None;
    }
    let scale = font_size / units_per_em;
    let metrics = face.underline_metrics()?;
    let offset_below_baseline = -(metrics.position as f32) * scale;
    let thickness = (metrics.thickness as f32 * scale).max(1.0);
    Some((offset_below_baseline, thickness))
}

/// Parte `text` en lineas reales que quepan en `max_width` (px), rompiendo
/// solo en limites de palabra (espacios) - nunca dentro de una palabra, no
/// hay hifenacion. Una palabra sola mas ancha que `max_width` ocupa su
/// propia linea igual (se desbordara visualmente; partirla exigiria
/// hifenacion real, que no esta implementada). `layout` (para el alto real
/// de la caja) y `gfx` (para pintar cada linea en su Y correcto) llaman a
/// esta misma funcion con los mismos argumentos, para que el numero de
/// lineas que uno calcula y el otro pinta sea siempre el mismo por
/// construccion, no por coincidencia.
///
/// Preserva un espacio inicial/final SIGNIFICATIVO de `text` (uno solo,
/// tras `collapse_whitespace` en `engine-layout`) en la primera/ultima
/// linea devuelta - `split_whitespace()` por si solo los descartaria
/// (produce tokens de PALABRA, sin bordes), lo cual era invisible mientras
/// nada llegaba aqui con espacios de borde (el motor recortaba todo el
/// texto por completo antes de la Fase 2.3), pero es un bug real ahora que
/// el flujo inline junta varias cajas de texto en la misma linea: un
/// espacio de borde perdido aqui pega visualmente dos fragmentos vecinos
/// sin el hueco que deberian tener entre si.
///
/// Nota de coste: mide la linea completa que se va formando en cada palabra
/// (via `measure_text`, que vuelve a shapear desde cero) en vez de sumar
/// anchos de palabras sueltas - mas simple y exacto (el kerning entre
/// palabras es real, no una suma ingenua), a costa de ser cuadratico en el
/// numero de palabras de una linea. Aceptable con las paginas de prueba
/// actuales; el mismo tipo de coste ya aceptado que el re-shaping por frame
/// (ver ARCHITECTURE.md), a revisar si algun dia se vuelve el cuello de
/// botella real.
pub fn wrap_text(font: &SystemFont, text: &str, font_size: f32, max_width: f32) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    if max_width <= 0.0 {
        return vec![text.to_string()];
    }

    let leading_space = text.starts_with(char::is_whitespace);
    let trailing_space = text.ends_with(char::is_whitespace);

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        let candidate = if current_line.is_empty() {
            word.to_string()
        } else {
            format!("{current_line} {word}")
        };

        if current_line.is_empty() || measure_text(font, &candidate, font_size).width <= max_width {
            current_line = candidate;
        } else {
            lines.push(std::mem::take(&mut current_line));
            current_line = word.to_string();
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if leading_space {
        if let Some(first) = lines.first_mut() {
            first.insert(0, ' ');
        }
    }
    if trailing_space {
        if let Some(last) = lines.last_mut() {
            last.push(' ');
        }
    }

    lines
}

/// Adaptador `ttf_parser::OutlineBuilder` -> `tiny_skia::PathBuilder`.
/// Aplica en cada punto la traduccion a la posicion de pantalla del glifo y
/// la inversion del eje Y (fuentes: Y hacia arriba: pantalla: Y hacia abajo)
/// en el momento de recibir cada punto, en vez de transformar el `Path`
/// despues - evita una pasada y una asignacion extra por glifo.
struct GlyphOutline {
    builder: tiny_skia::PathBuilder,
    origin_x: f32,
    origin_y: f32,
    scale: f32,
}

impl GlyphOutline {
    fn new(origin_x: f32, origin_y: f32, scale: f32) -> Self {
        Self { builder: tiny_skia::PathBuilder::new(), origin_x, origin_y, scale }
    }

    fn finish(self) -> Option<tiny_skia::Path> {
        self.builder.finish()
    }

    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.origin_x + x * self.scale, self.origin_y - y * self.scale)
    }
}

impl OutlineBuilder for GlyphOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.point(x1, y1);
        let (x, y) = self.point(x, y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.point(x1, y1);
        let (x2, y2) = self.point(x2, y2);
        let (x, y) = self.point(x, y);
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SystemFont` gano `Clone` para poder reconstruir el layout al
    /// redimensionar la ventana (ver `gfx/src/window.rs`) sin volver a leer
    /// el disco - prueba que la copia clonada shapea real, no solo que el
    /// clone compila.
    #[test]
    fn cloned_system_font_still_shapes_text_correctly() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let cloned = font.clone();
        let glyphs = shape_text(&cloned, "Hi", 16.0, 0.0, 0.0);
        assert_eq!(glyphs.len(), 2);
        for glyph in &glyphs {
            assert!(glyph.path.bounds().width() > 0.0);
        }
    }

    #[test]
    fn shapes_ascii_text_into_nonempty_glyph_paths_with_sane_bounds() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };

        let glyphs = shape_text(&font, "Hi", 16.0, 0.0, 0.0);
        assert_eq!(glyphs.len(), 2, "\"Hi\" deberia producir 2 glifos (sin ligaduras en latin basico)");

        for glyph in &glyphs {
            let bounds = glyph.path.bounds();
            assert!(bounds.width() > 0.0, "el contorno de un glifo visible no deberia tener ancho cero");
            assert!(bounds.height() > 0.0, "el contorno de un glifo visible no deberia tener alto cero");
        }
    }

    #[test]
    fn glyphs_advance_left_to_right_for_latin_text() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };

        let glyphs = shape_text(&font, "AB", 16.0, 0.0, 0.0);
        assert_eq!(glyphs.len(), 2);
        let first_left = glyphs[0].path.bounds().left();
        let second_left = glyphs[1].path.bounds().left();
        assert!(second_left > first_left, "la 'B' deberia quedar a la derecha de la 'A'");
    }

    #[test]
    fn empty_string_produces_no_glyphs() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        assert!(shape_text(&font, "", 16.0, 0.0, 0.0).is_empty());
    }

    #[test]
    fn measure_text_produces_wider_text_for_longer_strings() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let short = measure_text(&font, "hi", 16.0);
        let long = measure_text(&font, "hi there friend", 16.0);
        assert!(long.width > short.width, "un texto mas largo deberia medir mas ancho");
    }

    #[test]
    fn measure_text_scales_with_font_size() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let small = measure_text(&font, "hello", 16.0);
        let big = measure_text(&font, "hello", 32.0);
        assert!(big.width > small.width, "el mismo texto a mayor font-size deberia medir mas ancho");
        assert!(big.line_height > small.line_height, "el mismo texto a mayor font-size deberia tener mas alto de linea");
    }

    #[test]
    fn measure_text_of_empty_string_has_zero_width_but_a_real_line_height() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let metrics = measure_text(&font, "", 16.0);
        assert_eq!(metrics.width, 0.0);
        assert!(metrics.line_height > 0.0, "una linea vacia sigue teniendo alto (el de la fuente), no colapsa a 0");
    }

    #[test]
    fn baseline_offset_is_positive_and_scales_with_font_size() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let small = baseline_offset(&font, 16.0).expect("deberia haber metricas de fuente");
        let big = baseline_offset(&font, 32.0).expect("deberia haber metricas de fuente");
        assert!(small > 0.0, "el baseline deberia quedar POR DEBAJO de la parte superior de la linea");
        assert!(big > small, "a mayor font-size, mayor distancia hasta el baseline");
    }

    #[test]
    fn underline_metrics_returns_a_positive_offset_and_thickness_that_scale_with_font_size() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let (offset_16, thickness_16) = underline_metrics(&font, 16.0).expect("deberia haber metricas de subrayado");
        assert!(offset_16 > 0.0, "el subrayado deberia quedar POR DEBAJO del baseline, no encima");
        assert!(thickness_16 >= 1.0, "un grosor menor de 1px no seria visible");

        let (offset_32, thickness_32) = underline_metrics(&font, 32.0).expect("deberia haber metricas de subrayado");
        assert!(offset_32 > offset_16, "a mayor font-size, mayor distancia del subrayado al baseline");
        assert!(thickness_32 > thickness_16, "a mayor font-size, mayor grosor de subrayado");
    }

    /// El subrayado real siempre deberia quedar por debajo del baseline
    /// pero, para un texto normal (sin descendentes exagerados), tambien
    /// por encima del final visual de la linea - si no, se pintaria fuera
    /// de la caja que el layout reservo.
    #[test]
    fn underline_sits_below_the_baseline_but_within_a_reasonable_line_height() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let baseline = baseline_offset(&font, 16.0).expect("deberia haber metricas de fuente");
        let (underline_offset, _) = underline_metrics(&font, 16.0).expect("deberia haber metricas de subrayado");
        let metrics = measure_text(&font, "", 16.0);
        assert!(baseline + underline_offset < metrics.line_height, "el subrayado no deberia caer fuera del alto de linea reservado");
    }

    #[test]
    fn wrap_text_keeps_short_text_on_a_single_line() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let lines = wrap_text(&font, "hola mundo", 16.0, 2000.0);
        assert_eq!(lines, vec!["hola mundo".to_string()]);
    }

    #[test]
    fn wrap_text_breaks_at_word_boundaries_never_inside_a_word() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let text = "este es un parrafo bastante largo que deberia necesitar mas de una linea";
        let lines = wrap_text(&font, text, 16.0, 150.0);

        assert!(lines.len() > 1, "un ancho tan estrecho deberia forzar mas de una linea");
        for line in &lines {
            for word in line.split_whitespace() {
                assert!(text.contains(word), "'{word}' deberia ser una palabra completa del texto original, no un fragmento partido a mitad");
            }
        }
        // Ninguna palabra deberia perderse ni reordenarse al envolver.
        let rejoined: Vec<&str> = lines.iter().flat_map(|l| l.split_whitespace()).collect();
        let original: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(rejoined, original);
    }

    #[test]
    fn wrap_text_gives_a_lone_word_its_own_line_even_if_wider_than_max_width() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        // Una sola palabra mas ancha que max_width no puede partirse sin
        // hifenacion (no implementada) - debe seguir apareciendo entera.
        let lines = wrap_text(&font, "supercalifragilisticexpialidocious", 32.0, 10.0);
        assert_eq!(lines, vec!["supercalifragilisticexpialidocious".to_string()]);
    }

    #[test]
    fn wrap_text_of_empty_string_produces_no_lines() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        assert!(wrap_text(&font, "", 16.0, 500.0).is_empty());
        assert!(wrap_text(&font, "   ", 16.0, 500.0).is_empty());
    }

    /// Regresion real, encontrada al renderizar contenido inline (Fase 2.3
    /// del motor de layout): `split_whitespace()` produce tokens de
    /// PALABRA sin bordes, asi que reconstruir lineas uniendo palabras con
    /// `format!("{a} {b}")` perdia el espacio inicial/final del texto
    /// original - invisible mientras nada le pasaba a esta funcion un
    /// texto con espacios de borde (el motor recortaba todo con
    /// `.trim()`), pero un bug real ahora que varios fragmentos de texto
    /// comparten linea: el espacio que separa "cursiva" de "y" en
    /// "cursiva" + " y un " + "enlace" desaparecia al pintar, pegando las
    /// palabras ("cursivay").
    #[test]
    fn wrap_text_preserves_a_significant_leading_and_trailing_space() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let lines = wrap_text(&font, " y un ", 16.0, 2000.0);
        assert_eq!(lines, vec![" y un ".to_string()], "el espacio inicial y final del texto original no deberian perderse en una linea unica");
    }

    /// Mismo punto que el test anterior, pero cuando el texto SI se parte
    /// en varias lineas: el espacio de borde solo deberia sobrevivir en el
    /// extremo que le corresponde (inicio de la primera linea, fin de la
    /// ultima), no en los saltos de linea intermedios.
    #[test]
    fn wrap_text_preserves_edge_spaces_only_on_the_first_and_last_line() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };
        let lines = wrap_text(&font, " uno dos tres ", 16.0, 60.0);
        assert!(lines.len() > 1, "un ancho estrecho deberia forzar varias lineas para este texto");
        assert!(lines.first().unwrap().starts_with(' '), "la PRIMERA linea deberia conservar el espacio inicial del texto original");
        assert!(lines.last().unwrap().ends_with(' '), "la ULTIMA linea deberia conservar el espacio final del texto original");
    }
}
