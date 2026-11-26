//! Atributos HTML de presentacion (tarea 2.8 del plan): `bgcolor`, `width`,
//! `height`, `border`, `cellpadding`, `cellspacing`, `align`, `<font color>`...
//!
//! La web antigua (y todavia Hacker News) dibuja con atributos en vez de CSS:
//! `<table width="85%" bgcolor="#f6f6ef" cellpadding="0">`. Un navegador los
//! trata como CSS de especificidad cero, que cae entre la hoja del agente de
//! usuario y la del autor: cualquier regla CSS de la pagina los vence. Aqui se
//! traducen a declaraciones CSS y `cascade::resolve_style` las aplica en ese
//! mismo punto.

use engine_dom::{Node, NodeType};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Gris por defecto de los bordes de tabla de un navegador.
const TABLE_BORDER_COLOR: &str = "#808080";

/// `bgcolor="#ff6600"`, `bgcolor="ff6600"` (sin almohadilla, habitual en HTML
/// antiguo) o un nombre (`bgcolor="yellow"`).
fn legacy_color(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let hex = value.strip_prefix('#').unwrap_or(value);
    let is_hex = !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
    if is_hex && (hex.len() == 6 || (hex.len() == 3 && value.starts_with('#'))) {
        return Some(format!("#{hex}"));
    }
    if value.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some(value.to_ascii_lowercase());
    }
    None
}

/// `width="85%"` o `width="400"` (pixeles). Valores no positivos o ilegibles
/// se ignoran.
fn legacy_length(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        let n = percent.trim().parse::<f32>().ok().filter(|n| *n > 0.0)?;
        return Some(format!("{n}%"));
    }
    let digits: String = value.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n = digits.parse::<f32>().ok().filter(|n| *n > 0.0)?;
    Some(format!("{n}px"))
}

fn legacy_count(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok()
}

/// `<font size>` 1..7 a pixeles, como el valor por defecto de un navegador.
fn font_size_px(value: &str) -> Option<u32> {
    let value = value.trim();
    let level: i32 = if let Some(rest) = value.strip_prefix('+') {
        3 + rest.parse::<i32>().ok()?
    } else if let Some(rest) = value.strip_prefix('-') {
        3 - rest.parse::<i32>().ok()?
    } else {
        value.parse::<i32>().ok()?
    };
    Some(match level.clamp(1, 7) {
        1 => 10,
        2 => 13,
        3 => 16,
        4 => 18,
        5 => 24,
        6 => 32,
        _ => 48,
    })
}

fn parent_of(node: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
    node.read().unwrap().parent.as_ref().and_then(std::sync::Weak::upgrade)
}

/// El `<table>` mas cercano que contiene a esta celda, con sus atributos.
fn enclosing_table(cell: &Arc<RwLock<Node>>) -> Option<HashMap<String, String>> {
    let mut current = parent_of(cell);
    while let Some(node) = current {
        let found = {
            let guard = node.read().unwrap();
            match &guard.node_type {
                NodeType::Element { tag_name, attributes } if tag_name == "table" => Some(attributes.clone()),
                _ => None,
            }
        };
        if found.is_some() {
            return found;
        }
        current = parent_of(&node);
    }
    None
}

fn parent_tag_is_center(node: &Arc<RwLock<Node>>) -> bool {
    let Some(parent) = parent_of(node) else { return false };
    let guard = parent.read().unwrap();
    matches!(&guard.node_type, NodeType::Element { tag_name, .. } if tag_name == "center")
}

/// Si el atributo `border` de una tabla pide borde: `border="2"` da 2px y
/// `<table border>` (vacio) da 1px, como un navegador.
fn table_border_width(attributes: &HashMap<String, String>) -> Option<u32> {
    let raw = attributes.get("border")?;
    let width = if raw.trim().is_empty() { 1 } else { legacy_count(raw)? };
    (width > 0).then_some(width)
}

/// Las declaraciones CSS equivalentes a los atributos de presentacion del
/// elemento, de menor prioridad que cualquier regla del autor.
pub fn presentational_hints(dom_node: &Arc<RwLock<Node>>) -> Vec<(String, String)> {
    let mut hints: Vec<(String, String)> = Vec::new();
    let mut check_center_parent = false;
    let mut check_enclosing_table = false;

    {
        let guard = dom_node.read().unwrap();
        let NodeType::Element { tag_name, attributes } = &guard.node_type else {
            return hints;
        };
        let tag = tag_name.as_str();
        // Camino rapido: casi ningun elemento tiene atributos ni etiqueta
        // con pistas de presentacion, asi que evitamos clonar `attributes`.
        let needs_tag_work = matches!(tag, "table" | "tr" | "td" | "th" | "thead" | "tbody" | "tfoot" | "body" | "col" | "colgroup" | "font" | "img");
        if !needs_tag_work && !attributes.contains_key("align") {
            return hints;
        }
        let attr = |name: &str| attributes.get(name).map(|v| v.trim()).filter(|v| !v.is_empty());
        let mut push = |property: &str, value: String| hints.push((property.to_string(), value));

        let is_table_part = matches!(tag, "table" | "tr" | "td" | "th" | "thead" | "tbody" | "tfoot");

        if is_table_part || tag == "body" {
            if let Some(color) = attr("bgcolor").and_then(legacy_color) {
                push("background-color", color);
            }
        }
        if tag == "body" {
            if let Some(color) = attr("text").and_then(legacy_color) {
                push("color", color);
            }
        }
        if matches!(tag, "table" | "td" | "th" | "col" | "colgroup") {
            if let Some(width) = attr("width").and_then(legacy_length) {
                push("width", width);
            }
        }
        if matches!(tag, "table" | "tr" | "td" | "th") {
            if let Some(height) = attr("height").and_then(legacy_length) {
                push("height", height);
            }
        }

        match tag {
            "table" => {
                if let Some(width) = table_border_width(attributes) {
                    push("border", format!("{width}px solid {TABLE_BORDER_COLOR}"));
                }
                if let Some(spacing) = attr("cellspacing").and_then(legacy_count) {
                    push("border-spacing", format!("{spacing}px"));
                }
                match attr("align").map(str::to_ascii_lowercase).as_deref() {
                    Some("center") => {
                        push("margin-left", "auto".to_string());
                        push("margin-right", "auto".to_string());
                    }
                    Some("right") => {
                        push("margin-left", "auto".to_string());
                        push("margin-right", "0px".to_string());
                    }
                    _ => check_center_parent = true,
                }
            }
            "td" | "th" => check_enclosing_table = true,
            "font" => {
                if let Some(color) = attr("color").and_then(legacy_color) {
                    push("color", color);
                }
                if let Some(size) = attr("size").and_then(font_size_px) {
                    push("font-size", format!("{size}px"));
                }
                if let Some(face) = attr("face") {
                    push("font-family", face.to_string());
                }
            }
            _ => {}
        }

        // `align` en bloques y celdas es la alineacion del texto de dentro; en una
        // imagen, un flotante. (En una tabla ya se resolvio arriba.)
        if let Some(align) = attr("align").map(str::to_ascii_lowercase) {
            let is_text_block = matches!(tag, "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "center");
            if is_text_block && matches!(align.as_str(), "left" | "center" | "right" | "justify") {
                push("text-align", align);
            } else if tag == "img" && matches!(align.as_str(), "left" | "right") {
                push("float", align);
            }
        }
    }

    // Se suelta el candado de lectura de `dom_node` antes de subir por los ancestros.
    if check_center_parent && parent_tag_is_center(dom_node) {
        hints.push(("margin-left".to_string(), "auto".to_string()));
        hints.push(("margin-right".to_string(), "auto".to_string()));
    }
    if check_enclosing_table {
        if let Some(table) = enclosing_table(dom_node) {
            if let Some(padding) = table.get("cellpadding").and_then(|v| legacy_count(v)) {
                for side in ["top", "right", "bottom", "left"] {
                    hints.push((format!("padding-{side}"), format!("{padding}px")));
                }
            }
            if table_border_width(&table).is_some() {
                hints.push(("border".to_string(), format!("1px solid {TABLE_BORDER_COLOR}")));
            }
        }
    }

    hints
}
