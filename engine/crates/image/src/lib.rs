//! Decodificacion real de imagenes de trama (PNG/JPEG/GIF/BMP/WebP/TIFF/ICO,
//! via el crate `image` - ver "Doctrina de dependencias" en ARCHITECTURE.md)
//! a RGBA8 en memoria, lista para pintar. Crate propio y minimo (no vive
//! dentro de `engine-gfx`) porque tanto `engine-layout` (necesita
//! ancho/alto reales para reservar espacio en el layout, ANTES de pintar
//! nada) como `engine-gfx` (necesita los pixeles para pintar) lo necesitan,
//! y `layout` no puede depender de `gfx` (es al reves: `gfx` depende de
//! `layout`, ver ARCHITECTURE.md "Arquitectura de crates").
//!
//! Sin SVG (`resvg` sigue sin integrarse - SVG es vectorial, no un simple
//! decode-a-RGBA como el resto de formatos, fuera del alcance de esta
//! tarea) y sin cache de imagenes repetidas entre navegaciones (mismo tipo
//! de simplificacion declarada que ya existe para las hojas de estilo
//! externas).

use std::sync::Arc;

/// Una imagen ya decodificada en memoria: dimensiones reales en pixeles mas
/// el buffer RGBA8 (straight alpha, no premultiplicado) fila por fila,
/// `width * height * 4` bytes exactos - listo para que `engine-gfx` lo
/// vuelque en un `tiny_skia::Pixmap` sin decodificar nada de nuevo.
#[derive(Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodifica bytes crudos ya descargados (ver `engine_net::NetworkResponse
/// ::body`) a una `DecodedImage`. `None` si el formato no esta soportado o
/// los bytes estan corruptos/incompletos - un fallo real de decodificacion,
/// no una excusa: se le devuelve a quien llama para que omita la imagen
/// (mismo criterio que una hoja de estilo o script externo que falla al
/// descargarse, ver `core/server.rs`), no una imagen inventada de
/// respaldo. Tambien `None` para una imagen de 0x0 (formalmente decodifica
/// pero no tiene ningun pixel que pintar).
///
/// `Arc` porque el resultado se comparte tal cual entre quien mide el
/// layout y quien pinta despues (mismo patron que `engine_text::FontSet`
/// compartida entre medir y pintar) - clonar varios megabytes de RGBA por
/// cada relayout/repintado seria un desperdicio real, no teorico.
pub fn decode_image(bytes: &[u8]) -> Option<Arc<DecodedImage>> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|error| tracing::warn!("[engine-image] no se pudo decodificar la imagen: {error}"))
        .ok()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    Some(Arc::new(DecodedImage { width, height, rgba: rgba.into_raw() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x2 PNG rojo solido, generado a mano (cabecera PNG minima real, no
    /// un mock) - evita depender de un archivo fixture en disco solo para
    /// probar que la decodificacion funciona.
    fn tiny_red_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 2);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([255, 0, 0, 255]);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("codificar el PNG de prueba no deberia fallar");
        bytes
    }

    #[test]
    fn decode_image_reads_real_dimensions_and_pixels_from_a_png() {
        let decoded = decode_image(&tiny_red_png()).expect("un PNG valido deberia decodificar");
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba.len(), 2 * 2 * 4);
        assert_eq!(&decoded.rgba[0..4], &[255, 0, 0, 255], "el primer pixel deberia ser rojo solido, igual que se genero");
    }

    #[test]
    fn decode_image_rejects_garbage_bytes_instead_of_fabricating_an_image() {
        assert!(decode_image(b"esto no es una imagen de verdad").is_none());
    }

    #[test]
    fn decode_image_rejects_empty_input() {
        assert!(decode_image(&[]).is_none());
    }
}
