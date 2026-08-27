//! Pintado real de `DisplayItem::Image` sobre un `tiny_skia::Pixmap` -
//! compartido entre `raster.rs` (captura PNG headless, puente IPC) y
//! `window.rs` (ventana nativa) para no duplicar la conversion
//! premultiplicada+escalado dos veces.

use engine_image::DecodedImage;
use engine_layout::Rect;
use tiny_skia::{IntSize, Mask, Pixmap, PixmapPaint, Transform};

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
    for pixel in data.chunks_exact_mut(4) {
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
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let (tile_w, tile_h) = (image.width as f32, image.height as f32);
    if tile_w <= 0.0 || tile_h <= 0.0 {
        return;
    }
    let Some(source) = to_premultiplied_pixmap(image) else { return };

    let start_x = rect.x;
    let start_y = rect.y - scroll_offset_y;
    let cols = (rect.width / tile_w).ceil().max(1.0) as usize;
    let rows = (rect.height / tile_h).ceil().max(1.0) as usize;

    let mut drawn = 0usize;
    'rows: for row in 0..rows {
        for col in 0..cols {
            if drawn >= MAX_BACKGROUND_TILES {
                tracing::warn!("[image_paint] mosaico de fondo cortado en {drawn} tiles (limite {MAX_BACKGROUND_TILES})");
                break 'rows;
            }
            let transform = Transform::from_translate(start_x + col as f32 * tile_w, start_y + row as f32 * tile_h);
            pixmap.draw_pixmap(0, 0, source.as_ref(), &PixmapPaint::default(), transform, mask);
            drawn += 1;
        }
    }
}
