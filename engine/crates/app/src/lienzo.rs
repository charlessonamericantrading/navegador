//! Pinta HTML/CSS propio (la barra, la página de inicio, la de error) con el
//! motor, igual que una página web, y dice qué zonas se pueden pulsar.
//!
//! Una zona pulsable es cualquier elemento con `data-accion="..."`. Al pulsar
//! gana la más interna (un botón de cerrar dentro de una pestaña).

use engine_core::pipeline::build_page;
use engine_dom::{Node, NodeType};
use engine_gfx::{render_layout_to_pixmap_scaled, DisplayItem};
use engine_layout::{ImageMap, LayoutBox, Rect};
use engine_text::FontSet;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tiny_skia::Pixmap;

/// Una zona pulsable y su acción (el valor de `data-accion`).
#[derive(Debug, Clone)]
pub struct Region {
    pub rect: Rect,
    pub accion: String,
}

/// El resultado de pintar: los píxeles, el layout y las zonas pulsables.
pub struct Lienzo {
    pub pixmap: Pixmap,
    pub layout: LayoutBox,
    pub regiones: Vec<Region>,
}

/// Pinta `html` en un lienzo de `ancho`×`alto`. `encima` recibe el layout ya
/// calculado y devuelve lo que se pinta sobre la página (el cursor de la barra,
/// por ejemplo), en las mismas coordenadas.
pub fn pintar_html(
    html: &str,
    ancho: u32,
    alto: u32,
    fuentes: &FontSet,
    imagenes: &ImageMap,
    encima: impl FnOnce(&LayoutBox) -> Vec<DisplayItem>,
) -> Result<Lienzo, String> {
    pintar_html_escalado(html, ancho, alto, 1.0, fuentes, imagenes, encima)
}

/// Igual que `pintar_html`, pero rasterizando directamente a `escala` píxeles
/// físicos por píxel CSS mientras `regiones` se mantiene en coordenadas CSS.
pub fn pintar_html_escalado(
    html: &str,
    ancho: u32,
    alto: u32,
    escala: f32,
    fuentes: &FontSet,
    imagenes: &ImageMap,
    encima: impl FnOnce(&LayoutBox) -> Vec<DisplayItem>,
) -> Result<Lienzo, String> {
    let pagina = build_page(html, "", ancho as f32, alto as f32, Some(fuentes), &HashMap::new(), imagenes);
    let extra = encima(&pagina.layout_root);
    let pixmap = render_layout_to_pixmap_scaled(&pagina.layout_root, Some(fuentes), imagenes, ancho.max(1), alto.max(1), 0.0, &extra, escala)?;
    let mut regiones = Vec::new();
    recoger_regiones(&pagina.layout_root, &mut regiones);
    Ok(Lienzo { pixmap, layout: pagina.layout_root, regiones })
}

/// La acción de la zona más interna que contiene el punto.
pub fn accion_en(regiones: &[Region], x: f32, y: f32) -> Option<&str> {
    regiones
        .iter()
        .rev()
        .find(|r| x >= r.rect.x && x < r.rect.x + r.rect.width && y >= r.rect.y && y < r.rect.y + r.rect.height)
        .map(|r| r.accion.as_str())
}

/// La caja del elemento con ese `id`.
pub fn caja_con_id<'a>(caja: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
    if atributo(caja.dom_node.as_ref(), "id").as_deref() == Some(id) {
        return Some(caja);
    }
    caja.children.iter().find_map(|hija| caja_con_id(hija, id))
}

/// El rectángulo de borde de una caja (contenido + relleno + borde).
pub fn rect_de(caja: &LayoutBox) -> Rect {
    let borde = caja.box_dimensions.border_box();
    if borde.width > 0.0 && borde.height > 0.0 {
        borde
    } else {
        caja.dimensions.clone()
    }
}

fn recoger_regiones(caja: &LayoutBox, regiones: &mut Vec<Region>) {
    if let Some(accion) = atributo(caja.dom_node.as_ref(), "data-accion") {
        // Un elemento puede tener varias cajas (anónimas, fragmentos de
        // línea): basta con la primera.
        let ya_esta = regiones.last().is_some_and(|r| r.accion == accion && caja_es_hija_de_la_anterior(caja, &r.rect));
        if !ya_esta {
            regiones.push(Region { rect: rect_de(caja), accion });
        }
    }
    for hija in &caja.children {
        recoger_regiones(hija, regiones);
    }
}

fn caja_es_hija_de_la_anterior(caja: &LayoutBox, rect: &Rect) -> bool {
    let r = rect_de(caja);
    r.x >= rect.x && r.y >= rect.y && r.x + r.width <= rect.x + rect.width + 0.5 && r.y + r.height <= rect.y + rect.height + 0.5
}

fn atributo(nodo: Option<&Arc<RwLock<Node>>>, nombre: &str) -> Option<String> {
    let nodo = nodo?.read().ok()?;
    match &nodo.node_type {
        NodeType::Element { attributes, .. } => attributes.get(nombre).cloned(),
        _ => None,
    }
}

/// Escapa texto para meterlo en HTML.
pub fn escapar(texto: &str) -> String {
    let mut salida = String::with_capacity(texto.len());
    for c in texto.chars() {
        match c {
            '&' => salida.push_str("&amp;"),
            '<' => salida.push_str("&lt;"),
            '>' => salida.push_str("&gt;"),
            '"' => salida.push_str("&quot;"),
            '\'' => salida.push_str("&#39;"),
            _ => salida.push(c),
        }
    }
    salida
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_zona_mas_interna_gana() {
        let fuentes = FontSet::load_default_sans_serif();
        let html = r#"<html><body style="margin:0">
            <div data-accion="fuera" style="width:200px;height:100px;padding:0;background:#eee">
              <div data-accion="dentro" style="width:50px;height:50px;background:#f00"></div>
            </div></body></html>"#;
        let lienzo = pintar_html(html, 300, 200, &fuentes, &ImageMap::new(), |_| Vec::new()).unwrap();
        assert_eq!(accion_en(&lienzo.regiones, 10.0, 10.0), Some("dentro"));
        assert_eq!(accion_en(&lienzo.regiones, 150.0, 80.0), Some("fuera"));
        assert_eq!(accion_en(&lienzo.regiones, 250.0, 150.0), None);
    }

    #[test]
    fn escapa_lo_que_romperia_el_html() {
        assert_eq!(escapar("<a href=\"x\">&'"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
    }
}
