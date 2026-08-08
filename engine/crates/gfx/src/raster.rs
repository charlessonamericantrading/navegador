//! Rasterizado headless compartido por el puente IPC y la ventana nativa.
//!
//! Produce PNG desde el mismo `DisplayList` que usa la ventana winit. No
//! introduce otro renderer: solo separa la superficie de salida (pixmap en
//! memoria frente a softbuffer) de las operaciones de pintado.

use crate::display_list::{DisplayItem, DisplayList};
use engine_layout::{LayoutBox, Rect};
use engine_text::{measure_text, shape_text, wrap_text, FontSet};
use tiny_skia::{Color, FillRule, Paint, Pixmap, Rect as SkiaRect, Transform};

pub fn render_layout_to_png(
    layout_root: &LayoutBox,
    font_set: Option<&FontSet>,
    width: u32,
    height: u32,
    scroll_offset_y: f32,
) -> Result<Vec<u8>, String> {
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| format!("no se pudo crear la superficie de rasterizado {width}x{height}"))?;
    pixmap.fill(Color::from_rgba8(245, 245, 245, 255));

    for item in DisplayList::build(layout_root).items {
        match item {
            DisplayItem::SolidRect { rect, color } => {
                fill_rect(&mut pixmap, &rect, color, scroll_offset_y);
            }
            DisplayItem::Text {
                rect,
                text,
                color,
                font_size,
                bold,
                italic,
            } => {
                let mut paint = paint(color);
                match font_set.and_then(|set| set.pick(bold, italic)) {
                    Some(font) => {
                        let lines = wrap_text(font, &text, font_size, rect.width);
                        let line_height = measure_text(font, "", font_size).line_height;
                        for (index, line) in lines.iter().enumerate() {
                            let line_y = rect.y + index as f32 * line_height - scroll_offset_y;
                            for glyph in shape_text(font, line, font_size, rect.x, line_y) {
                                pixmap.fill_path(
                                    &glyph.path,
                                    &paint,
                                    FillRule::Winding,
                                    Transform::identity(),
                                    None,
                                );
                            }
                        }
                    }
                    None => fill_rect_with_paint(&mut pixmap, &rect, &mut paint, scroll_offset_y),
                }
            }
            DisplayItem::Border { rect, width, color } => {
                let mut paint = paint(color);
                for strip in border_strip_rects(&rect, width) {
                    fill_rect_with_paint(&mut pixmap, &strip, &mut paint, scroll_offset_y);
                }
            }
        }
    }

    pixmap
        .encode_png()
        .map_err(|error| format!("no se pudo codificar la captura PNG: {error}"))
}

fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint
}

fn fill_rect(pixmap: &mut Pixmap, rect: &Rect, color: [u8; 4], scroll_offset_y: f32) {
    let mut paint = paint(color);
    fill_rect_with_paint(pixmap, rect, &mut paint, scroll_offset_y);
}

fn fill_rect_with_paint(
    pixmap: &mut Pixmap,
    rect: &Rect,
    paint: &mut Paint<'static>,
    scroll_offset_y: f32,
) {
    if let Some(sk_rect) =
        SkiaRect::from_xywh(rect.x, rect.y - scroll_offset_y, rect.width.max(1.0), rect.height.max(1.0))
    {
        pixmap.fill_rect(sk_rect, paint, Transform::identity(), None);
    }
}

fn border_strip_rects(border_box: &Rect, width: f32) -> [Rect; 4] {
    [
        Rect {
            x: border_box.x,
            y: border_box.y,
            width: border_box.width,
            height: width,
        },
        Rect {
            x: border_box.x,
            y: border_box.y + border_box.height - width,
            width: border_box.width,
            height: width,
        },
        Rect {
            x: border_box.x,
            y: border_box.y,
            width,
            height: border_box.height,
        },
        Rect {
            x: border_box.x + border_box.width - width,
            y: border_box.y,
            width,
            height: border_box.height,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_layout::{BoxType, LayoutBox};

    #[test]
    fn renders_a_non_empty_png_from_a_layout() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions.width = 120.0;
        root.dimensions.height = 80.0;
        let png = render_layout_to_png(&root, None, 120, 80, 0.0).expect("PNG should encode");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
