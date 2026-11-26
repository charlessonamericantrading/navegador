//! Pintado real de `DisplayItem::Image` sobre un `tiny_skia::Pixmap` -
//! compartido entre `raster.rs` (captura PNG headless, puente IPC) y
//! `window.rs` (ventana nativa) para no duplicar la conversion
//! premultiplicada+escalado dos veces.

use engine_image::DecodedImage;
use engine_layout::Rect;
use crate::display_list::BackgroundPlacement;
use tiny_skia::{FilterQuality, IntSize, Mask, Pixmap, PixmapPaint, Transform};

/// `tiny_skia::Pixmap` exige RGBA8 PREMULTIPLICADO (`color * alpha / 255`
/// por canal) - `engine_image::DecodedImage` guarda RGBA8 SIN premultiplicar
/// (straight alpha, el formato en el que `image` decodifica y en el que
/// cualquier PNG/JPEG con transparencia real viene) - sin esta conversion,
/// cualquier pixel semitransparente se pintaria mas brillante de lo debido
/// (colores sin atenuar por su propio alpha). Para JPEG (siempre opaco,
/// alpha=255) es un no-op matematico (`c*255/255 == c`), asi que no hace
/// falta distinguir el formato aqui.
fn to_premultiplied_pixmap(image: &DecodedImage) -> Option<Pixmap> {
    let mut data = image.rgba.clone();
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        pixel[0] = ((pixel[0] as u32 * alpha) / 255) as u8;
        pixel[1] = ((pixel[1] as u32 * alpha) / 255) as u8;
        pixel[2] = ((pixel[2] as u32 * alpha) / 255) as u8;
    }
    let size = IntSize::from_wh(image.width, image.height)?;
    Pixmap::from_vec(data, size)
}

/// Pinta `image` escalada a `rect` (el tamaño YA RESUELTO por el layout -
/// `resolve_image_dimensions` en `engine-layout::tree` - que puede diferir
/// del tamaño natural de la imagen si la pagina puso `width`/`height`
/// explicitos) sobre `pixmap`, con `scroll_offset_y` restado de `rect.y`
/// igual que el resto de `DisplayItem` (`fill_rect`/texto) ya hacen.
/// Silenciosamente no pinta nada si la imagen decodificada tiene 0 filas/
/// columnas o si el tamaño resuelto es 0 (nada que escalar) - mismo criterio
/// que el resto del pintado: sin rectangulo de relleno inventado para un
/// caso que no deberia ocurrir en la practica (`decode_image` ya rechaza
/// imagenes 0x0).
/// `mask`: el recorte activo de `overflow: hidden` (Fase 3.5), si lo hay -
/// ver `engine-gfx::paint::build_clip_mask`. `None` pinta sin recortar,
/// igual que siempre antes de esa tarea.
pub fn paint_image(pixmap: &mut Pixmap, rect: &Rect, image: &DecodedImage, scroll_offset_y: f32, mask: Option<&Mask>) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let Some(source) = to_premultiplied_pixmap(image) else { return };

    let scale_x = rect.width / image.width as f32;
    let scale_y = rect.height / image.height as f32;
    let transform = Transform::from_row(scale_x, 0.0, 0.0, scale_y, rect.x, rect.y - scroll_offset_y);

    pixmap.draw_pixmap(0, 0, source.as_ref(), &PixmapPaint::default(), transform, mask);
}

/// Tope de mosaicos por llamada (Fase 40) - una imagen de pocos pixeles
/// sobre una caja grande generaria decenas de miles de `draw_pixmap`. No es
/// hipotetico: es el propio patron de "textura diminuta que se repite" que
/// usan sitios reales para fondos baratos. Pasado el tope, se deja de
/// pintar (el resto de la caja queda sin mosaico) en vez de colgar el
/// pintado - misma degradacion honesta que el resto de este archivo.
const MAX_BACKGROUND_TILES: usize = 8192;

/// Pinta `image` en su TAMAÑO NATURAL (sin escalar, a diferencia de
/// `paint_image`), repetida en mosaico desde la esquina superior-izquierda
/// de `rect` hasta cubrirlo por completo - `background-repeat: repeat` y
/// `background-size: auto`, los dos valores iniciales reales del spec.
/// `mask` recorta el mosaico: quien llama (`paint::paint_display_list`) ya
/// lo construyo combinando cualquier `overflow: hidden` activo CON el
/// propio `rect` de esta caja - sin esa segunda parte, un mosaico se
/// saldria del borde de su propia caja, no solo del de un ancestro
/// recortado (a diferencia de `fill_rect`/`paint_image`, que nunca pintan
/// mas alla de su propio `rect` aunque no haya mascara).
pub fn paint_background_image(pixmap: &mut Pixmap, rect: &Rect, image: &DecodedImage, scroll_offset_y: f32, mask: Option<&Mask>) {
    paint_background_image_scaled(pixmap, rect, image, scroll_offset_y, mask, 1.0);
}

pub fn paint_background_image_scaled(pixmap: &mut Pixmap, rect: &Rect, image: &DecodedImage, scroll_offset_y: f32, mask: Option<&Mask>, scale: f32) {
    paint_background_placed(pixmap, rect, image, &BackgroundPlacement::natural(rect, image), scroll_offset_y, mask, scale);
}

/// Pinta `image` en `rect` segun `placement`: la primera baldosa en su sitio y
/// tamaño, repetida (o no) en cada eje hasta cubrir `rect`. `mask` recorta.
pub fn paint_background_placed(pixmap: &mut Pixmap, rect: &Rect, image: &DecodedImage, placement: &BackgroundPlacement, scroll_offset_y: f32, mask: Option<&Mask>, scale: f32) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (tile_w, tile_h) = (placement.tile.width * scale, placement.tile.height * scale);
    if !(tile_w > 0.0 && tile_h > 0.0 && image.width > 0 && image.height > 0) {
        return;
    }
    let Some(source) = to_premultiplied_pixmap(image) else { return };

    // Origen de la primera baldosa y limites de `rect`, en pixeles del dispositivo.
    let (tile_x, tile_y) = (placement.tile.x * scale, (placement.tile.y - scroll_offset_y) * scale);
    let (rect_x, rect_y) = (rect.x * scale, (rect.y - scroll_offset_y) * scale);
    let (rect_right, rect_bottom) = (rect_x + rect.width * scale, rect_y + rect.height * scale);

    // Posiciones de las baldosas en un eje: solo la primera, o todas las que tocan `rect`.
    let axis = |origin: f32, size: f32, start: f32, end: f32, repeat: bool| -> Vec<f32> {
        if !repeat {
            return vec![origin];
        }
        let first = origin + ((start - origin) / size).floor() * size;
        let count = (((end - first) / size).ceil().max(1.0) as usize).min(MAX_BACKGROUND_TILES);
        (0..count).map(|k| first + k as f32 * size).collect()
    };
    let xs = axis(tile_x, tile_w, rect_x, rect_right, placement.repeat_x);
    let ys = axis(tile_y, tile_h, rect_y, rect_bottom, placement.repeat_y);

    // Una baldosa que no coincide con el tamaño natural se reescala suavizando.
    let sx = tile_w / image.width as f32;
    let sy = tile_h / image.height as f32;
    let natural = (sx - scale).abs() < 0.001 && (sy - scale).abs() < 0.001;
    let paint = PixmapPaint { quality: if natural { FilterQuality::Nearest } else { FilterQuality::Bilinear }, ..PixmapPaint::default() };

    let mut drawn = 0usize;
    'rows: for &y in &ys {
        for &x in &xs {
            if drawn >= MAX_BACKGROUND_TILES {
                tracing::warn!("[image_paint] mosaico de fondo cortado en {drawn} tiles (limite {MAX_BACKGROUND_TILES})");
                break 'rows;
            }
            // Fuera de `rect` no se pinta nada: se salta (el recorte lo haria igual, pero mas caro).
            if x >= rect_right || y >= rect_bottom || x + tile_w <= rect_x || y + tile_h <= rect_y {
                continue;
            }
            pixmap.draw_pixmap(0, 0, source.as_ref(), &paint, Transform::from_row(sx, 0.0, 0.0, sy, x, y), mask);
            drawn += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una imagen de 4x12 con tres franjas de 4 filas: roja, verde, azul.
    fn sprite() -> DecodedImage {
        let mut rgba = Vec::new();
        for fila in 0..12 {
            let color: [u8; 4] = match fila / 4 {
                0 => [255, 0, 0, 255],
                1 => [0, 255, 0, 255],
                _ => [0, 0, 255, 255],
            };
            for _ in 0..4 {
                rgba.extend_from_slice(&color);
            }
        }
        DecodedImage { width: 4, height: 12, rgba }
    }

    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let p = pixmap.pixel(x, y).unwrap();
        (p.red(), p.green(), p.blue(), p.alpha())
    }

    #[test]
    fn a_sprite_shows_only_the_strip_the_position_selects_and_repeats_it_in_x() {
        // Caja de 12x4 en (0,0); la imagen sube 4 filas (`0 -4px`): se ve la franja VERDE.
        let mut pixmap = Pixmap::new(12, 8).unwrap();
        let rect = Rect { x: 0.0, y: 0.0, width: 12.0, height: 4.0 };
        let placement = BackgroundPlacement { tile: Rect { x: 0.0, y: -4.0, width: 4.0, height: 12.0 }, repeat_x: true, repeat_y: false };
        paint_background_placed(&mut pixmap, &rect, &sprite(), &placement, 0.0, None, 1.0);
        for x in [0, 5, 11] {
            assert_eq!(pixel(&pixmap, x, 0), (0, 255, 0, 255), "franja verde repetida en x={x}");
            assert_eq!(pixel(&pixmap, x, 3), (0, 255, 0, 255));
        }
        // El recorte a la caja lo pone el pintado completo (`paint_display_list` pasa una
        // mascara con su `rect`); aqui solo se comprueba la colocacion.
    }

    #[test]
    fn no_repeat_paints_a_single_tile_and_the_default_tiles_both_axes() {
        let mut solo = Pixmap::new(12, 12).unwrap();
        let rect = Rect { x: 0.0, y: 0.0, width: 12.0, height: 12.0 };
        let una = BackgroundPlacement { tile: Rect { x: 4.0, y: 0.0, width: 4.0, height: 12.0 }, repeat_x: false, repeat_y: false };
        paint_background_placed(&mut solo, &rect, &sprite(), &una, 0.0, None, 1.0);
        assert_eq!(pixel(&solo, 5, 1), (255, 0, 0, 255));
        assert_eq!(pixel(&solo, 1, 1).3, 0, "a la izquierda de la baldosa no hay nada");
        assert_eq!(pixel(&solo, 9, 1).3, 0, "y a la derecha tampoco: no repite");

        let mut mosaico = Pixmap::new(8, 24).unwrap();
        let rect = Rect { x: 0.0, y: 0.0, width: 8.0, height: 24.0 };
        paint_background_image_scaled(&mut mosaico, &rect, &sprite(), 0.0, None, 1.0);
        assert_eq!(pixel(&mosaico, 5, 1), (255, 0, 0, 255), "comportamiento de siempre: repite en x");
        assert_eq!(pixel(&mosaico, 1, 13), (255, 0, 0, 255), "y en y (la 2a copia empieza en la fila 12)");
    }

    #[test]
    fn a_tile_scaled_by_background_size_fills_its_new_size() {
        let mut pixmap = Pixmap::new(8, 24).unwrap();
        let rect = Rect { x: 0.0, y: 0.0, width: 8.0, height: 24.0 };
        // La imagen de 4x12 a 8x24 (doble): la franja roja pasa a ocupar 8 filas.
        let placement = BackgroundPlacement { tile: Rect { x: 0.0, y: 0.0, width: 8.0, height: 24.0 }, repeat_x: false, repeat_y: false };
        paint_background_placed(&mut pixmap, &rect, &sprite(), &placement, 0.0, None, 1.0);
        assert_eq!(pixel(&pixmap, 3, 6), (255, 0, 0, 255));
        assert_eq!(pixel(&pixmap, 3, 12), (0, 255, 0, 255));
        assert_eq!(pixel(&pixmap, 3, 20), (0, 0, 255, 255));
    }
}
