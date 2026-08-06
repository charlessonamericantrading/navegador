use engine_layout::{LayoutBox, BoxType, Rect};
use std::collections::HashMap;

/// Valores iniciales CSS reales para las dos propiedades heredables que
/// aplicamos (ver `INHERITABLE_PROPERTIES` en engine-layout): un navegador
/// real tampoco pinta texto sin color ni tamaño, usa estos mismos por
/// defecto cuando nada en la cascada los redefine.
const INITIAL_COLOR: [u8; 4] = [0, 0, 0, 255];
pub const INITIAL_FONT_SIZE: f32 = 16.0;

#[derive(Debug, Clone)]
pub enum DisplayItem {
    SolidRect { rect: Rect, color: [u8; 4] },
    Text { rect: Rect, text: String, color: [u8; 4], font_size: f32 },
    /// `rect` es el border-box COMPLETO (`layout_box.dimensions`, que ya
    /// incluye el propio border - ver `engine_layout::tree::
    /// flow_block_children`); quien pinta esto (`engine-gfx/src/window.rs`)
    /// dibuja un marco de `width` de grosor hacia adentro desde el borde de
    /// `rect`, no un rectangulo aparte fuera de `dimensions`.
    Border { rect: Rect, width: f32, color: [u8; 4] },
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    pub fn build(layout_root: &LayoutBox) -> Self {
        let mut list = Self::default();
        Self::build_items(layout_root, &mut list);
        list
    }

    fn build_items(layout_box: &LayoutBox, list: &mut DisplayList) {
        match &layout_box.box_type {
            BoxType::Block | BoxType::Inline => {
                // Sin background-color explicito en la cascada, las cajas de
                // bloque no pintan fondo propio (transparente = se ve el
                // fondo de la ventana), en vez del blanco solido fijo de
                // antes que ocultaba cualquier fondo heredado del padre.
                // background-color pinta sobre TODO `dimensions` (el
                // border-box completo) a proposito: asi es el valor inicial
                // real de `background-clip` (`border-box`) - el border, si
                // lo hay, se pinta DESPUES y encima, tapando esa franja.
                if let Some(color) = layout_box.computed_style.get("background-color").and_then(|v| parse_css_color(v)) {
                    list.items.push(DisplayItem::SolidRect { rect: layout_box.dimensions.clone(), color });
                }
                if let Some((width, color)) = parse_css_border(&layout_box.computed_style) {
                    list.items.push(DisplayItem::Border { rect: layout_box.dimensions.clone(), width, color });
                }
            }
            BoxType::Text(content) => {
                let color = layout_box.computed_style.get("color").and_then(|v| parse_css_color(v)).unwrap_or(INITIAL_COLOR);
                let font_size = layout_box.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
                list.items.push(DisplayItem::Text {
                    rect: layout_box.dimensions.clone(),
                    text: content.clone(),
                    color,
                    font_size,
                });
            }
        }

        for child in &layout_box.children {
            Self::build_items(child, list);
        }
    }
}

/// Parseo de color CSS deliberadamente minimo: solo hex (#rgb, #rrggbb).
/// Nombres de color (`red`, `white`...), `rgb()`/`rgba()`/`hsl()` no estan
/// implementados todavia - devuelve None y la caja se queda sin pintar en
/// vez de fingir un color por defecto. Ver ARCHITECTURE.md.
fn parse_css_color(value: &str) -> Option<[u8; 4]> {
    let hex = value.trim().strip_prefix('#')?;
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some([r, g, b, 255])
        }
        3 => {
            let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
            let mut chars = hex.chars();
            let r = expand(chars.next()?)?;
            let g = expand(chars.next()?)?;
            let b = expand(chars.next()?)?;
            Some([r, g, b, 255])
        }
        _ => None,
    }
}

/// Parseo de `font-size` deliberadamente minimo: solo `<numero>px`. Unidades
/// relativas (`em`, `rem`, `%`) exigirian conocer el tamaño heredado del
/// padre y la raiz del documento, que no rastreamos todavia - se devuelve
/// `None` (cae al tamaño inicial) en vez de fingir una conversion.
fn parse_css_font_size(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok().filter(|size| *size > 0.0)
}

/// Igual que `parse_css_font_size`, pero SI acepta cero (un ancho de
/// border de 0 es valido) - copia deliberada de `parse_css_length` en
/// `engine-layout/src/tree.rs`, misma razon que `INITIAL_FONT_SIZE`/
/// `parse_css_font_size` de arriba: dos crates que no deben depender
/// entre si, y unas pocas lineas no justifican enredar la dependencia.
fn parse_css_length(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok().filter(|n| *n >= 0.0)
}

/// Ancho+color de `border` (forma abreviada `border: <ancho> <estilo>
/// <color>`, en cualquier orden). Mismo criterio que
/// `engine_layout::tree::resolve_border_width` (que resuelve solo el
/// ancho, para el layout): sin la palabra `solid`, el border no existe -
/// `None` aqui, no un rectangulo con ancho cero (que tampoco pintaria
/// nada, pero por las razones equivocadas). Color ausente cae al `color`
/// YA RESUELTO de la propia caja - equivalente a `currentColor`, el valor
/// inicial real de `border-color`.
fn parse_css_border(computed_style: &HashMap<String, String>) -> Option<(f32, [u8; 4])> {
    let raw = computed_style.get("border")?;
    let mut width: Option<f32> = None;
    let mut color: Option<[u8; 4]> = None;
    let mut is_solid = false;

    for token in raw.split_whitespace() {
        if let Some(w) = parse_css_length(token) {
            width = Some(w);
        } else if token.eq_ignore_ascii_case("solid") {
            is_solid = true;
        } else if let Some(c) = parse_css_color(token) {
            color = Some(c);
        }
    }

    if !is_solid {
        return None;
    }

    let resolved_width = width.unwrap_or(0.0);
    if resolved_width <= 0.0 {
        return None;
    }
    let resolved_color = color
        .or_else(|| computed_style.get("color").and_then(|v| parse_css_color(v)))
        .unwrap_or(INITIAL_COLOR);
    Some((resolved_width, resolved_color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pixel_font_size() {
        assert_eq!(parse_css_font_size("32px"), Some(32.0));
        assert_eq!(parse_css_font_size("  18px "), Some(18.0));
    }

    #[test]
    fn rejects_unsupported_units_and_garbage() {
        assert_eq!(parse_css_font_size("2em"), None);
        assert_eq!(parse_css_font_size("120%"), None);
        assert_eq!(parse_css_font_size("not-a-size"), None);
        assert_eq!(parse_css_font_size("-5px"), None, "un tamaño negativo no es valido");
        assert_eq!(parse_css_font_size("0px"), None, "un tamaño cero no pintaria nada visible");
    }

    fn style_with_border(value: &str) -> HashMap<String, String> {
        let mut style = HashMap::new();
        style.insert("border".to_string(), value.to_string());
        style
    }

    #[test]
    fn parse_css_border_reads_width_and_color_in_any_order() {
        assert_eq!(parse_css_border(&style_with_border("2px solid #ff0000")), Some((2.0, [255, 0, 0, 255])));
        assert_eq!(parse_css_border(&style_with_border("solid #ff0000 2px")), Some((2.0, [255, 0, 0, 255])), "el orden de ancho/estilo/color deberia ser libre, igual que el spec real");
        assert_eq!(parse_css_border(&style_with_border("#ff0000 2px solid")), Some((2.0, [255, 0, 0, 255])));
    }

    #[test]
    fn parse_css_border_without_solid_keyword_is_none() {
        assert_eq!(parse_css_border(&style_with_border("2px #ff0000")), None, "sin 'solid', border-style es 'none' - el border no deberia existir en absoluto");
        assert_eq!(parse_css_border(&style_with_border("2px none #ff0000")), None);
    }

    #[test]
    fn parse_css_border_falls_back_to_the_boxs_own_color_when_no_color_is_given() {
        let mut style = style_with_border("2px solid");
        style.insert("color".to_string(), "#00ff00".to_string());
        assert_eq!(parse_css_border(&style), Some((2.0, [0, 255, 0, 255])), "sin border-color, deberia caer al color ya resuelto de la caja (currentColor)");
    }

    #[test]
    fn parse_css_border_defaults_to_black_when_neither_border_color_nor_color_are_set() {
        assert_eq!(parse_css_border(&style_with_border("2px solid")), Some((2.0, [0, 0, 0, 255])));
    }

    #[test]
    fn parse_css_border_with_zero_width_is_none() {
        assert_eq!(parse_css_border(&style_with_border("0px solid #ff0000")), None, "un border de ancho cero no pinta nada, aunque el estilo sea solid");
    }

    #[test]
    fn missing_border_property_is_none() {
        assert_eq!(parse_css_border(&HashMap::new()), None);
    }
}
