use std::collections::HashMap;
use bytes::Bytes;
use url::Url;

#[derive(Debug, Clone)]
pub struct NetworkResponse {
    /// La URL que efectivamente produjo esta respuesta - tras seguir
    /// redirecciones (ver `NetworkEngine::fetch`), NO la URL pedida
    /// originalmente si hubo alguna. Hace falta para resolver rutas
    /// relativas de sub-recursos (`<link href>`, `<script src>`) contra la
    /// pagina donde realmente se aterrizo, igual que hace un navegador real
    /// (si `/viejo` redirige a `/nuevo/`, un `href="foo.css"` en la pagina
    /// final es `/nuevo/foo.css`, no `/foo.css`).
    pub url: Url,
    pub status_code: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
    /// Todas las cabeceras `Set-Cookie` de la respuesta, en orden - aparte
    /// de `headers` porque es la unica que un servidor repite de forma
    /// legitima y significativa (un `HashMap` solo guardaria la ultima).
    /// El almacen de cookies ya las consumio en `NetworkEngine::fetch_once`;
    /// se exponen aqui para depuracion y para quien necesite inspeccionarlas.
    pub set_cookie: Vec<String>,
    pub body: Bytes,
}

impl NetworkResponse {
    pub fn is_success(&self) -> bool {
        self.status_code >= 200 && self.status_code < 300
    }

    /// El cuerpo como texto, decodificado segun su codificacion real (tarea 1.6
    /// del plan). Ver `decode_text` para el orden en que se decide.
    pub fn text(&self) -> String {
        decode_text(&self.body, self.content_type())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.headers.get("content-type").map(|s| s.as_str())
    }
}

/// Decodifica un cuerpo a texto segun su codificacion (tarea 1.6 del plan).
///
/// Antes se leia siempre como UTF-8 con sustitucion: una pagina en ISO-8859-1
/// o Windows-1252 (muy comunes en sitios en espanol anteriores a ~2010) salia
/// con `\u{FFFD}` donde iba cada acento ("Espa\u{FFFD}a").
///
/// Orden, igual que el estandar de codificacion de HTML:
/// 1. Una marca de orden de bytes (BOM) manda sobre todo lo demas.
/// 2. `charset=` de la cabecera `Content-Type`.
/// 3. `<meta charset>` o `<meta http-equiv="Content-Type" content="...;
///    charset=...">` en los primeros 1024 bytes (solo documentos HTML).
/// 4. Sin declaracion: UTF-8 si el cuerpo es UTF-8 valido; si no, Windows-1252,
///    que es lo que usan los navegadores para el contenido heredado sin
///    etiquetar (y es mejor que llenarlo de caracteres de sustitucion).
///
/// Una etiqueta que no se conoce se ignora y se sigue con el siguiente paso.
/// Una declaracion UTF-16 en un `<meta>` se trata como UTF-8: un documento que
/// se lee como ASCII para encontrar su `<meta>` no puede ser UTF-16.
pub fn decode_text(body: &[u8], content_type: Option<&str>) -> String {
    use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252};

    if let Some((encoding, bom_length)) = Encoding::for_bom(body) {
        return encoding.decode_without_bom_handling(&body[bom_length..]).0.into_owned();
    }

    let from_header = content_type.and_then(charset_from_content_type).and_then(|label| Encoding::for_label(label.as_bytes()));
    if let Some(encoding) = from_header {
        return encoding.decode_without_bom_handling(body).0.into_owned();
    }

    let looks_like_html = content_type.is_none_or(|ct| {
        let ct = ct.to_ascii_lowercase();
        ct.contains("html") || ct.contains("xml")
    });
    if looks_like_html {
        if let Some(encoding) = sniff_meta_charset(body).and_then(|label| Encoding::for_label(label.as_bytes())) {
            let encoding = if encoding == UTF_16LE || encoding == UTF_16BE { UTF_8 } else { encoding };
            return encoding.decode_without_bom_handling(body).0.into_owned();
        }
    }

    match std::str::from_utf8(body) {
        Ok(text) => text.to_string(),
        Err(_) => WINDOWS_1252.decode_without_bom_handling(body).0.into_owned(),
    }
}

/// El valor de `charset=` de una cabecera `Content-Type`, sin comillas.
fn charset_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let after = &content_type[lower.find("charset")? + "charset".len()..];
    let after = after.trim_start().strip_prefix('=')?.trim_start();
    let value: String = after.trim_start_matches(['"', '\'']).chars().take_while(|c| !matches!(c, ';' | '"' | '\'' | ' ' | '\t')).collect();
    (!value.is_empty()).then_some(value)
}

/// Busca `<meta charset=...>` o `<meta http-equiv=... content="...charset=...">`
/// en los primeros 1024 bytes, que es hasta donde el estandar manda mirar.
fn sniff_meta_charset(body: &[u8]) -> Option<String> {
    let head = &body[..body.len().min(1024)];
    // En minusculas y con los bytes no ASCII convertidos a '?': solo se
    // buscan etiquetas y atributos, que son ASCII, y asi no hay que decodificar.
    let text: String = head.iter().map(|b| if b.is_ascii() { b.to_ascii_lowercase() as char } else { '?' }).collect();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<meta") {
        let tag = &rest[start..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag = &tag[..end];
        if let Some(position) = tag.find("charset") {
            let value = tag[position + "charset".len()..].trim_start().strip_prefix('=')?;
            let value: String = value.trim_start().trim_start_matches(['"', '\'']).chars().take_while(|c| !matches!(c, ';' | '"' | '\'' | ' ' | '>' | '/' | '\t' | '\n' | '\r')).collect();
            if !value.is_empty() {
                return Some(value);
            }
        }
        rest = &rest[start + "<meta".len()..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_from_the_content_type_header_is_decoded() {
        let body = b"<html><body>Espa\xf1a camin\xf3 a\xf1o</body></html>";
        assert_eq!(decode_text(body, Some("text/html; charset=iso-8859-1")), "<html><body>Espa\u{f1}a camin\u{f3} a\u{f1}o</body></html>");
        assert_eq!(decode_text(body, Some("text/html; charset=\"ISO-8859-1\"")), decode_text(body, Some("text/html;charset=latin1")), "comillas, mayusculas y alias");
    }

    #[test]
    fn a_meta_charset_is_used_when_the_header_says_nothing() {
        let con_meta = b"<html><head><meta charset=\"windows-1252\"></head><body>\x93hola\x94 a\xf1o</body></html>";
        assert!(decode_text(con_meta, Some("text/html")).contains("\u{201c}hola\u{201d} a\u{f1}o"), "las comillas curvas de Windows-1252 (0x93/0x94) y la enye");

        let http_equiv = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=ISO-8859-1\"></head><body>a\xf1o</body></html>";
        assert!(decode_text(http_equiv, None).contains("a\u{f1}o"));
    }

    #[test]
    fn the_header_wins_over_the_meta_tag() {
        let body = b"<meta charset=\"utf-8\">caf\xe9";
        assert!(decode_text(body, Some("text/html; charset=iso-8859-1")).ends_with("caf\u{e9}"));
    }

    #[test]
    fn a_bom_wins_over_everything_and_is_removed() {
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice("caf\u{e9}".as_bytes());
        assert_eq!(decode_text(&body, Some("text/html; charset=iso-8859-1")), "caf\u{e9}", "el BOM UTF-8 manda sobre la cabecera y no aparece en el texto");
    }

    #[test]
    fn unlabelled_content_is_utf8_when_valid_and_windows_1252_when_not() {
        assert_eq!(decode_text("camión ñandú €".as_bytes(), None), "camión ñandú €");
        assert_eq!(decode_text(b"a\xf1o \x80", Some("text/html")), "a\u{f1}o \u{20ac}", "UTF-8 invalido y sin etiqueta: Windows-1252 (0x80 es el euro)");
    }

    #[test]
    fn other_legacy_encodings_work() {
        // "日本語" en Shift_JIS.
        assert_eq!(decode_text(&[0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea], Some("text/html; charset=shift_jis")), "日本語");
    }

    #[test]
    fn an_unknown_label_is_ignored_and_the_next_step_applies() {
        assert_eq!(decode_text("ñu".as_bytes(), Some("text/html; charset=nada-conocido")), "ñu");
    }

    #[test]
    fn a_meta_declaring_utf16_is_treated_as_utf8() {
        assert_eq!(decode_text("<meta charset=\"utf-16\">ñu".as_bytes(), Some("text/html")), "<meta charset=\"utf-16\">ñu");
    }

    #[test]
    fn a_meta_is_not_looked_for_in_non_html_bodies() {
        // Un JS que contiene el texto "<meta charset=..." no cambia su codificacion.
        let js = "var s = '<meta charset=\"iso-8859-1\">'; var n = 'ñ';".as_bytes();
        assert!(decode_text(js, Some("application/javascript")).contains("ñ"));
    }
}
