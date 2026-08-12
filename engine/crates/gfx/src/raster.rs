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

    /// **La garantia de correccion del descarte por viewport (Fase 5)**:
    /// pintar una pagina larga tiene que dar EXACTAMENTE los mismos
    /// pixeles que pintar solo la parte que cae dentro del viewport. Si
    /// el descarte se comiera algo visible, o dejara de pintar algo que
    /// asoma por un borde, estos dos PNG diferirian.
    ///
    /// Se compara el PNG completo byte a byte, no una muestra: cualquier
    /// diferencia de un solo pixel cambia el resultado.
    fn fila(y: f32) -> LayoutBox {
        let mut fila = LayoutBox::new(BoxType::Block);
        fila.dimensions.x = 4.0;
        fila.dimensions.y = y;
        fila.dimensions.width = 200.0;
        fila.dimensions.height = 30.0;
        fila.computed_style.insert("background-color".to_string(), "#3355aa".to_string());
        fila.computed_style.insert("border".to_string(), "2px solid".to_string());
        fila
    }

    fn render(filas: &[f32], scroll: f32) -> Vec<u8> {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions.width = 300.0;
        root.dimensions.height = 200.0;
        for y in filas {
            root.children.push(fila(*y));
        }
        render_layout_to_png(&root, None, &engine_layout::ImageMap::new(), 300, 200, scroll).expect("deberia rasterizar")
    }

    #[test]
    fn culling_offscreen_boxes_produces_pixel_identical_output() {
        // Dentro del viewport (0..200) y muy lejos por debajo.
        let visibles = [10.0, 50.0, 90.0, 130.0];
        let con_lejanas: Vec<f32> = visibles.iter().copied().chain([2000.0, 4000.0, 9000.0]).collect();

        assert_eq!(
            render(&con_lejanas, 0.0),
            render(&visibles, 0.0),
            "las cajas muy por debajo del viewport no pueden pintar ningun pixel visible,              asi que descartarlas tiene que dar un PNG identico al de no tenerlas"
        );
    }

    /// La otra direccion: lo que queda muy por ENCIMA tras hacer scroll
    /// tampoco puede pintar nada.
    #[test]
    fn culling_boxes_scrolled_far_above_the_viewport_is_also_pixel_identical() {
        let visibles_tras_scroll = [3010.0, 3050.0, 3090.0];
        let con_las_de_arriba: Vec<f32> = [0.0, 40.0, 80.0].iter().copied().chain(visibles_tras_scroll).collect();

        assert_eq!(
            render(&con_las_de_arriba, 3000.0),
            render(&visibles_tras_scroll, 3000.0),
            "tras bajar 3000px, las cajas de arriba del todo quedan fuera y descartarlas no deberia cambiar nada"
        );
    }

    /// Y el caso que impide que el descarte sea demasiado agresivo: una
    /// caja que asoma por el borde inferior SI pinta, asi que quitarla SI
    /// cambia el resultado. Sin este test, un `is_offscreen` roto que
    /// descartara de mas pasaria inadvertido por los dos de arriba.
    #[test]
    fn a_box_straddling_the_viewport_edge_really_does_change_the_pixels() {
        let solo_visibles = [10.0, 50.0];
        let con_una_a_caballo: Vec<f32> = solo_visibles.iter().copied().chain([190.0]).collect();

        assert_ne!(
            render(&con_una_a_caballo, 0.0),
            render(&solo_visibles, 0.0),
            "una caja que asoma por el borde inferior TIENE que pintarse: si esto pasa, el descarte se esta comiendo pixeles visibles"
        );
    }

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
