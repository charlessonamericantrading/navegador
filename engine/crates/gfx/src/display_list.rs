use engine_image::DecodedImage;
use engine_layout::{ImageMap, LayoutBox, BoxType, Rect};
use std::collections::HashMap;
use std::sync::Arc;

/// Valores iniciales CSS reales para las dos propiedades heredables que
/// aplicamos (ver `INHERITABLE_PROPERTIES` en engine-layout): un navegador
/// real tampoco pinta texto sin color ni tamaño, usa estos mismos por
/// defecto cuando nada en la cascada los redefine.
const INITIAL_COLOR: [u8; 4] = [0, 0, 0, 255];
pub const INITIAL_FONT_SIZE: f32 = 16.0;

#[derive(Debug, Clone)]
pub enum DisplayItem {
    SolidRect { rect: Rect, color: [u8; 4] },
    Text { rect: Rect, text: String, color: [u8; 4], font_size: f32, bold: bool, italic: bool },
    /// `rect` es el border-box COMPLETO (`layout_box.dimensions`, que ya
    /// incluye el propio border - ver `engine_layout::tree::
    /// flow_block_children`); quien pinta esto (`engine-gfx/src/window.rs`)
    /// dibuja un marco de `width` de grosor hacia adentro desde el borde de
    /// `rect`, no un rectangulo aparte fuera de `dimensions`.
    Border { rect: Rect, width: f32, color: [u8; 4] },
    /// `image` ya viene RESUELTA (el `Arc<DecodedImage>` real, no el `src`
    /// crudo) desde `DisplayList::build` - quien pinta esto (`raster.rs`/
    /// `window.rs`) no necesita saber nada de `ImageMap` ni de red, solo
    /// escalar+volcar los pixeles ya decodificados sobre `rect` (que ya es
    /// el tamaño FINAL resuelto por el layout - ver `resolve_image_dimensions`
    /// en `engine-layout::tree` - no necesariamente el tamaño natural de la
    /// imagen).
    Image { rect: Rect, image: Arc<DecodedImage> },
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    /// `z_layers` acumula el contenido de cada subarbol posicionado
    /// (`position: relative`/`absolute`/`fixed`, ver `engine-layout::tree`)
    /// que ademas trae un `z-index` numerico - se pintan DESPUES de todo el
    /// contenido normal, ordenados por z-index ascendente (mayor z-index
    /// pinta encima), en vez de en su orden de documento original. Un
    /// elemento posicionado SIN `z-index` (el caso mas comun -
    /// `position: relative` sin mas) sigue pintandose en su sitio normal,
    /// en orden de documento, exactamente como el resto del contenido -
    /// solo `z-index` cambia el orden de pintado, no `position` por si
    /// sola. Simplificacion declarada: no hay contextos de apilamiento
    /// anidados de verdad (un z-index dentro de otro z-index se aplana al
    /// mismo nivel que todos los demas, en vez de resolverse DENTRO de su
    /// propio contenedor primero) - cubre el caso real mas comun (un modal/
    /// tooltip/dropdown con z-index alto pintando por encima de todo lo
    /// demas), no el spec completo de contextos de apilamiento.
    pub fn build(layout_root: &LayoutBox, images: &ImageMap) -> Self {
        let mut list = Self::default();
        let mut z_layers: Vec<(i32, Vec<DisplayItem>)> = Vec::new();
        Self::build_items(layout_root, &mut list.items, images, &mut z_layers);
        z_layers.sort_by_key(|(z, _)| *z);
        for (_, items) in z_layers {
            list.items.extend(items);
        }
        list
    }

    fn build_items(layout_box: &LayoutBox, target: &mut Vec<DisplayItem>, images: &ImageMap, z_layers: &mut Vec<(i32, Vec<DisplayItem>)>) {
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
                    target.push(DisplayItem::SolidRect { rect: layout_box.dimensions.clone(), color });
                }
                if let Some((width, color)) = parse_css_border(&layout_box.computed_style) {
                    target.push(DisplayItem::Border { rect: layout_box.dimensions.clone(), width, color });
                }
            }
            BoxType::Text(content) => {
                let color = layout_box.computed_style.get("color").and_then(|v| parse_css_color(v)).unwrap_or(INITIAL_COLOR);
                let font_size = layout_box.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
                target.push(DisplayItem::Text {
                    rect: layout_box.dimensions.clone(),
                    text: content.clone(),
                    color,
                    font_size,
                    bold: resolve_font_weight_is_bold(&layout_box.computed_style),
                    italic: resolve_font_style_is_italic(&layout_box.computed_style),
                });
            }
            BoxType::Image(src) => {
                // Sin imagen resuelta (`src` vacio, descarga fallida,
                // formato no soportado) el layout ya dejo `dimensions` en
                // 0x0 (ver `resolve_image_dimensions` en
                // `engine-layout::tree`) - `images.get(src)` sera `None` en
                // ese mismo caso (misma fuente de verdad), asi que no hace
                // falta comprobar el tamaño por separado: sin imagen, no se
                // pinta nada, en vez de un rectangulo de relleno inventado.
                if let Some(image) = images.get(src) {
                    target.push(DisplayItem::Image { rect: layout_box.dimensions.clone(), image: image.clone() });
                }
            }
        }

        for child in &layout_box.children {
            match z_index_for_stacking(&child.computed_style) {
                Some(z) => {
                    let mut layer_items = Vec::new();
                    Self::build_items(child, &mut layer_items, images, z_layers);
                    z_layers.push((z, layer_items));
                }
                None => Self::build_items(child, target, images, z_layers),
            }
        }
    }
}

/// `z-index` SOLO participa en el orden de pintado si el elemento tambien
/// esta posicionado (`position: relative`/`absolute`/`fixed`) - en un
/// elemento `static` (el valor inicial real), `z-index` no tiene ningun
/// efecto (asi es el spec real: `z-index` sin `position` se ignora por
/// completo), asi que ese caso deliberadamente devuelve `None` en vez de
/// crear una capa de apilamiento que el spec real nunca crearia.
fn z_index_for_stacking(computed_style: &HashMap<String, String>) -> Option<i32> {
    let is_positioned = matches!(computed_style.get("position").map(String::as_str), Some("relative") | Some("absolute") | Some("fixed"));
    if !is_positioned {
        return None;
    }
    computed_style.get("z-index").and_then(|v| v.trim().parse::<i32>().ok())
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

/// Copia deliberada de `resolve_font_weight_is_bold` en
/// `engine-layout::tree` (misma razon de siempre: dos crates que no deben
/// depender entre si, y unas pocas lineas no justifican enredar la
/// dependencia) - decide que variante de `FontSet` pedir al pintar
/// (`engine-gfx/src/raster.rs`, `engine-gfx/src/window.rs`), igual que la
/// copia de layout decide que variante pedir al MEDIR. Misma
/// simplificacion binaria: negrita si/no, no el espacio 1-1000 completo del
/// spec.
fn resolve_font_weight_is_bold(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("font-weight") else { return false };
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("bold") || trimmed.eq_ignore_ascii_case("bolder") {
        return true;
    }
    trimmed.parse::<u16>().map(|weight| weight >= 600).unwrap_or(false)
}

/// Copia deliberada de `resolve_font_style_is_italic` en
/// `engine-layout::tree` - misma razon que `resolve_font_weight_is_bold`
/// arriba.
fn resolve_font_style_is_italic(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("font-style") else { return false };
    let trimmed = raw.trim();
    trimmed.eq_ignore_ascii_case("italic") || trimmed.eq_ignore_ascii_case("oblique")
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
    fn resolve_font_weight_is_bold_recognizes_keywords_and_heavy_numeric_weights() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("font-weight".to_string(), value.to_string()); m };

        assert!(resolve_font_weight_is_bold(&style("bold")));
        assert!(resolve_font_weight_is_bold(&style("700")));
        assert!(!resolve_font_weight_is_bold(&style("normal")));
        assert!(!resolve_font_weight_is_bold(&HashMap::new()));
    }

    #[test]
    fn z_index_for_stacking_only_applies_to_positioned_elements() {
        let style = |position: Option<&str>, z: &str| {
            let mut m = HashMap::new();
            if let Some(p) = position {
                m.insert("position".to_string(), p.to_string());
            }
            m.insert("z-index".to_string(), z.to_string());
            m
        };
        assert_eq!(z_index_for_stacking(&style(Some("relative"), "5")), Some(5));
        assert_eq!(z_index_for_stacking(&style(Some("static"), "5")), None, "z-index sin position real (static) no deberia tener efecto, igual que el spec real");
        assert_eq!(z_index_for_stacking(&style(None, "5")), None, "sin position en absoluto (static por defecto), z-index se ignora igual");
    }

    /// El punto real del pintado por z-index: un elemento posicionado con
    /// `z-index` alto deberia pintarse DESPUES (encima) de contenido sin
    /// z-index, sin importar que venga ANTES en el documento.
    #[test]
    fn a_document_earlier_box_with_a_higher_z_index_paints_after_a_later_one() {
        let mut low_in_document_but_high_z_index = LayoutBox::new(BoxType::Block);
        low_in_document_but_high_z_index.computed_style.insert("position".to_string(), "relative".to_string());
        low_in_document_but_high_z_index.computed_style.insert("z-index".to_string(), "10".to_string());
        low_in_document_but_high_z_index.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
        low_in_document_but_high_z_index.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let mut later_in_document_no_z_index = LayoutBox::new(BoxType::Block);
        later_in_document_no_z_index.computed_style.insert("background-color".to_string(), "#00ff00".to_string());
        later_in_document_no_z_index.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let mut root = LayoutBox::new(BoxType::Block);
        root.children.push(low_in_document_but_high_z_index);
        root.children.push(later_in_document_no_z_index);

        let list = DisplayList::build(&root, &ImageMap::new());

        let red_index = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [255, 0, 0, 255]))
            .expect("deberia existir el rectangulo rojo (z-index alto)");
        let green_index = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [0, 255, 0, 255]))
            .expect("deberia existir el rectangulo verde (sin z-index)");

        assert!(red_index > green_index, "el z-index alto (rojo, primero en el documento) deberia pintarse DESPUES del verde (sin z-index), pese a venir antes en el DOM");
    }

    #[test]
    fn resolve_font_style_is_italic_recognizes_italic_and_oblique() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("font-style".to_string(), value.to_string()); m };

        assert!(resolve_font_style_is_italic(&style("italic")));
        assert!(resolve_font_style_is_italic(&style("oblique")));
        assert!(!resolve_font_style_is_italic(&HashMap::new()));
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
