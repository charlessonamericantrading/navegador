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
use usvg::{fontdb, Options, PostProcessingSteps, TreeParsing, TreePostProc};

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
/// ::body`) a una `DecodedImage`.
///
/// Soporta tanto imágenes de mapa de bits (PNG, JPEG, GIF, BMP, WebP, ICO, TIFF)
/// como imágenes vectoriales SVG mediante `resvg`.
///
/// `None` si el formato no está soportado o los bytes están corruptos/incompletos.
pub fn decode_image(bytes: &[u8]) -> Option<Arc<DecodedImage>> {
    if let Ok(decoded) = image::load_from_memory(bytes) {
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        if width > 0 && height > 0 {
            return Some(Arc::new(DecodedImage { width, height, rgba: rgba.into_raw() }));
        }
    }
    // Si no es un formato de trama estándar, intentar decodificar como SVG vectorial
    decode_svg(bytes)
}

/// Decodifica y rasteriza contenido vectorial SVG a RGBA8 en memoria mediante `resvg`.
pub fn decode_svg(bytes: &[u8]) -> Option<Arc<DecodedImage>> {
    let opt = Options::default();
    let mut tree = usvg::Tree::from_data(bytes, &opt).ok()?;
    let fontdb = fontdb::Database::new();
    tree.postprocess(PostProcessingSteps::default(), &fontdb);

    let size = tree.size.to_int_size();
    let width = size.width();
    let height = size.height();
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());

    Some(Arc::new(DecodedImage {
        width,
        height,
        rgba: pixmap.take(),
    }))
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

    #[test]
    fn decode_svg_renders_valid_svg_to_rgba_buffer() {
        let svg_data = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
            <rect width="20" height="20" fill="#00ff00" />
        </svg>"##;

        let decoded = decode_svg(svg_data).expect("el SVG válido debe decodificar");
        assert_eq!(decoded.width, 20);
        assert_eq!(decoded.height, 20);
        assert_eq!(decoded.rgba.len(), 20 * 20 * 4);
        assert_eq!(&decoded.rgba[0..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn decode_image_automatically_dispatches_svg() {
        let svg_data = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <rect width="10" height="10" fill="#0000ff" />
        </svg>"##;

        let decoded = decode_image(svg_data).expect("decode_image debe aceptar e identificar SVG");
        assert_eq!(decoded.width, 10);
        assert_eq!(decoded.height, 10);
        assert_eq!(&decoded.rgba[0..4], &[0, 0, 255, 255]);
    }
}
