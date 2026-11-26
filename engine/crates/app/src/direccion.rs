//! Qué hacer con lo que se escribe en la barra de direcciones: una dirección
//! ("wikipedia.org") se abre; cualquier otra cosa ("clima en madrid") se busca.

/// Buscador al que van las búsquedas escritas en la barra (versión HTML sin JS pesado).
pub const BUSCADOR: &str = "https://lite.duckduckgo.com/lite/?q=";

/// Convierte lo escrito en la URL a la que navegar. `None` si no hay nada.
pub fn resolver(entrada: &str) -> Option<String> {
    let texto = entrada.trim();
    if texto.is_empty() {
        return None;
    }
    if parece_direccion(texto) {
        let minusculas = texto.to_ascii_lowercase();
        if minusculas.starts_with("http://") || minusculas.starts_with("https://") || minusculas.starts_with("file://") {
            return Some(texto.to_string());
        }
        let esquema = if es_local(texto) { "http://" } else { "https://" };
        return Some(format!("{esquema}{texto}"));
    }
    let consulta: String = url::form_urlencoded::byte_serialize(texto.as_bytes()).collect();
    Some(format!("{BUSCADOR}{consulta}"))
}

fn parece_direccion(texto: &str) -> bool {
    if texto.chars().any(char::is_whitespace) {
        return false;
    }
    let minusculas = texto.to_ascii_lowercase();
    if ["http://", "https://", "file://"].iter().any(|p| minusculas.starts_with(p)) {
        return true;
    }
    if es_local(texto) {
        return true;
    }
    // Dominio con al menos un punto y una terminación de letras ("wikipedia.org",
    // "es.wikipedia.org/wiki/Rust"), con puerto opcional.
    let anfitrion = texto.split(['/', '?', '#']).next().unwrap_or("");
    let sin_puerto = match anfitrion.rsplit_once(':') {
        Some((nombre, puerto)) if !puerto.is_empty() && puerto.chars().all(|c| c.is_ascii_digit()) => nombre,
        _ => anfitrion,
    };
    let partes: Vec<&str> = sin_puerto.split('.').collect();
    let valido = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if partes.len() < 2 || !partes.iter().all(|p| valido(p)) {
        return false;
    }
    let ultima = partes[partes.len() - 1];
    let es_ip = partes.len() == 4 && partes.iter().all(|p| p.parse::<u8>().is_ok());
    es_ip || (ultima.len() >= 2 && ultima.chars().all(|c| c.is_ascii_alphabetic()))
}

fn es_local(texto: &str) -> bool {
    let anfitrion = texto.split(['/', '?', '#']).next().unwrap_or("");
    let nombre = anfitrion.split(':').next().unwrap_or("");
    nombre.eq_ignore_ascii_case("localhost") || nombre == "127.0.0.1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_direccion_se_abre_con_https() {
        assert_eq!(resolver("wikipedia.org").as_deref(), Some("https://wikipedia.org"));
        assert_eq!(resolver("  es.wikipedia.org/wiki/Rust ").as_deref(), Some("https://es.wikipedia.org/wiki/Rust"));
        assert_eq!(resolver("ejemplo.com:8080/a?b=1").as_deref(), Some("https://ejemplo.com:8080/a?b=1"));
    }

    #[test]
    fn una_url_completa_se_respeta() {
        assert_eq!(resolver("http://news.ycombinator.com").as_deref(), Some("http://news.ycombinator.com"));
        assert_eq!(resolver("HTTPS://Ejemplo.com").as_deref(), Some("HTTPS://Ejemplo.com"));
    }

    #[test]
    fn lo_local_va_por_http() {
        assert_eq!(resolver("localhost:8000").as_deref(), Some("http://localhost:8000"));
        assert_eq!(resolver("127.0.0.1:9001/x").as_deref(), Some("http://127.0.0.1:9001/x"));
        assert_eq!(resolver("192.168.1.1").as_deref(), Some("https://192.168.1.1"));
    }

    #[test]
    fn el_resto_se_busca() {
        assert_eq!(resolver("clima en madrid").as_deref(), Some("https://lite.duckduckgo.com/lite/?q=clima+en+madrid"));
        assert_eq!(resolver("rust").as_deref(), Some("https://lite.duckduckgo.com/lite/?q=rust"));
        assert_eq!(resolver("año.x1").as_deref(), Some("https://lite.duckduckgo.com/lite/?q=a%C3%B1o.x1"));
    }

    #[test]
    fn vacio_no_navega() {
        assert_eq!(resolver("   "), None);
    }
}
