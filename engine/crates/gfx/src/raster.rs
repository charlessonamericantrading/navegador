//! Rasterizado headless compartido por el puente IPC y la ventana nativa.
//!
//! Produce PNG desde el mismo `DisplayList` que usa la ventana winit. No
//! introduce otro renderer: solo separa la superficie de salida (pixmap en
//! memoria frente a softbuffer) de las operaciones de pintado, que viven en
//! `paint.rs` (compartidas con `window.rs` desde la Fase 3.5 - antes de esa
//! tarea este archivo tenia su PROPIA copia del bucle de pintado, ver el
//! doc-comment de `paint.rs` para el bug real que costo esa duplicacion).

use crate::display_list::DisplayList;
use crate::paint::paint_display_list;
use engine_layout::{ImageMap, LayoutBox};
use engine_text::FontSet;
use tiny_skia::{Color, Pixmap};

pub fn render_layout_to_png(
    layout_root: &LayoutBox,
    font_set: Option<&FontSet>,
    images: &ImageMap,
    width: u32,
    height: u32,
    scroll_offset_y: f32,
) -> Result<Vec<u8>, String> {
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| format!("no se pudo crear la superficie de rasterizado {width}x{height}"))?;
    pixmap.fill(Color::from_rgba8(245, 245, 245, 255));

    let display_list = DisplayList::build(layout_root, images);
    paint_display_list(&mut pixmap, &display_list.items, font_set, scroll_offset_y);

    pixmap
        .encode_png()
        .map_err(|error| format!("no se pudo codificar la captura PNG: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_layout::BoxType;

    #[test]
    fn renders_a_non_empty_png_from_a_layout() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions.width = 120.0;
        root.dimensions.height = 80.0;
        let png = render_layout_to_png(&root, None, &engine_layout::ImageMap::new(), 120, 80, 0.0).expect("PNG should encode");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    /// Regresion del panic real encontrado en vivo (Fase 3.4, ver el
    /// doc-comment de `paint::border_strip_rects`): una caja con `border`
    /// en una posicion X FRACCIONARIA (como una columna de tabla de 500px /
    /// 3) disparaba un `debug_assert!(false)` dentro de tiny-skia al pintar
    /// el borde. Sin el redondeo a pixel entero, este test hace panic; con
    /// el, simplemente produce un PNG valido.
    #[test]
    fn renders_without_panicking_when_a_bordered_box_sits_at_a_fractional_x() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions.width = 500.0;
        root.dimensions.height = 80.0;

        let mut child = LayoutBox::new(BoxType::Block);
        // El mismo tipo de ancho fraccionario que produce
        // `flow_table_children` para 3 columnas en 500px (500.0 / 3.0).
        child.dimensions.x = 166.66667;
        child.dimensions.y = 0.0;
        child.dimensions.width = 166.66667;
        child.dimensions.height = 40.0;
        child.computed_style.insert("border".to_string(), "1px solid".to_string());
        root.children.push(child);

        let png = render_layout_to_png(&root, None, &engine_layout::ImageMap::new(), 500, 80, 0.0).expect("PNG should encode");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    /// Regresion de Fase 3.5: `border-radius`/`box-shadow`/`overflow:
    /// hidden` combinados no deberian hacer panic ni producir un PNG
    /// invalido - mismo criterio que el test anterior, ahora contra las
    /// propiedades nuevas en vez del bug de coordenadas fraccionarias.
    #[test]
    fn renders_without_panicking_with_border_radius_shadow_and_overflow_hidden() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions.width = 200.0;
        root.dimensions.height = 150.0;
        root.computed_style.insert("overflow".to_string(), "hidden".to_string());

        let mut child = LayoutBox::new(BoxType::Block);
        child.dimensions.x = 10.0;
        child.dimensions.y = 10.0;
        child.dimensions.width = 300.0; // se desborda del padre a proposito
        child.dimensions.height = 60.0;
        child.computed_style.insert("border".to_string(), "3px solid".to_string());
        child.computed_style.insert("border-radius".to_string(), "12px".to_string());
        child.computed_style.insert("background-color".to_string(), "#ffffff".to_string());
        child.computed_style.insert("box-shadow".to_string(), "4px 4px 8px #000000".to_string());
        root.children.push(child);

        let png = render_layout_to_png(&root, None, &engine_layout::ImageMap::new(), 200, 150, 0.0).expect("PNG should encode");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
