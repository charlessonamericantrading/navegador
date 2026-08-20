//! `localStorage` / `sessionStorage` reales (Web Storage API).
//!
//! Antes de esta fase esto era un `HashMap<String, String>` suelto que
//! nadie instanciaba y que no estaba conectado a JS: `localStorage` no
//! existia como global, asi que cualquier pagina que lo usara moria con
//! `ReferenceError`. Y practicamente toda aplicacion web moderna lo usa
//! para estado de sesion, preferencias y cache de cliente.
//!
//! Lo que SI implementa del spec:
//! - **Alcance por ORIGEN** (`esquema://host:puerto`), que es la parte que
//!   de verdad importa para la seguridad: `https://a.test` y
//!   `https://b.test` no se ven el almacenamiento el uno al otro, ni
//!   siquiera `http://a.test` y `https://a.test` (esquema distinto =
//!   origen distinto, igual que el spec).
//! - Orden de insercion estable para `key(n)`/`length`, que es lo que
//!   hacen los navegadores reales en la practica.
//! - Cuota por origen con `QuotaExceededError` de verdad cuando se pasa,
//!   en vez de crecer sin limite hasta agotar la memoria del proceso.
//! - Los valores son SIEMPRE cadenas (`setItem(k, 42)` guarda `"42"`), y
//!   `getItem` de una clave inexistente devuelve `null`, no `undefined` -
//!   ambas son diferencias observables que el codigo real comprueba.
//!
//! Simplificaciones declaradas:
//! - **Sin persistencia a disco todavia**: `localStorage` deberia
//!   sobrevivir a cerrar la aplicacion y aqui no lo hace, asi que hoy se
//!   comporta como un `sessionStorage` de vida larga. La diferencia entre
//!   ambos SI esta modelada (son dos areas distintas que no se ven entre
//!   si), que es lo que nota una pagina en una misma sesion; lo que falta
//!   es solo el volcado a disco.
//! - Sin evento `storage` (el que avisa a OTRAS pestañas del mismo origen
//!   de un cambio): este motor no tiene comunicacion entre pestañas.

use std::collections::HashMap;
use url::Url;

/// Cuota por origen y por area. Los navegadores reales rondan los 5 MiB;
/// se elige el mismo orden de magnitud para que una pagina que compruebe
/// su limite encuentre algo realista en vez de un valor inventado.
pub const QUOTA_BYTES_PER_ORIGIN: usize = 5 * 1024 * 1024;

/// El origen de una URL en la forma `esquema://host:puerto` - la clave de
/// aislamiento de todo este modulo. El puerto se omite cuando es el que
/// corresponde por defecto al esquema, para que `https://a.test` y
/// `https://a.test:443` sean el MISMO origen (lo son en el spec).
pub fn origin_of(url: &Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

/// Un area de almacenamiento de UN origen. `Vec` y no `HashMap` a
/// proposito: `key(n)` y `length` exigen un orden estable, y el orden de
/// insercion es el que usan los navegadores reales en la practica.
#[derive(Debug, Clone, Default)]
struct StorageArea {
    items: Vec<(String, String)>,
}

impl StorageArea {
    fn used_bytes(&self) -> usize {
        self.items.iter().map(|(k, v)| k.len() + v.len()).sum()
    }
}

/// Error de cuota - se traduce en JS a un `QuotaExceededError` real
/// (ver `engine_js::storage`), que es lo que una pagina espera capturar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaExceeded;

/// Las dos areas del spec. Son almacenes SEPARADOS incluso para el mismo
/// origen: lo que escribe una no lo ve la otra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    Local,
    Session,
}

/// Almacen de Web Storage de toda la sesion del navegador - una sola
/// instancia viva, compartida por todas las paginas (cada una consulta
/// solo su propio origen). Mismo criterio que `CookieStore`: el estado del
/// navegador no vive en la pagina, porque tiene que sobrevivir a navegar a
/// otra.
#[derive(Debug, Clone, Default)]
pub struct WebStorage {
    local: HashMap<String, StorageArea>,
    session: HashMap<String, StorageArea>,
}

impl WebStorage {
    pub fn new() -> Self {
        Self::default()
    }

    fn area(&self, kind: StorageKind) -> &HashMap<String, StorageArea> {
        match kind {
            StorageKind::Local => &self.local,
            StorageKind::Session => &self.session,
        }
    }

    fn area_mut(&mut self, kind: StorageKind) -> &mut HashMap<String, StorageArea> {
        match kind {
            StorageKind::Local => &mut self.local,
            StorageKind::Session => &mut self.session,
        }
    }

    /// `null` (aqui `None`) para una clave que no existe - NO cadena vacia
    /// ni `undefined`. Es una diferencia observable: el codigo real hace
    /// `if (localStorage.getItem('x') === null)`.
    pub fn get_item(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String> {
        self.area(kind)
            .get(origin)?
            .items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    /// Sustituye el valor si la clave ya existia (conservando su POSICION
    /// original, que es lo que mantiene estable el orden de `key(n)`), o la
    /// añade al final si es nueva.
    pub fn set_item(&mut self, kind: StorageKind, origin: &str, key: &str, value: &str) -> Result<(), QuotaExceeded> {
        let area = self.area_mut(kind).entry(origin.to_string()).or_default();

        let previous_size = area
            .items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(k, v)| k.len() + v.len())
            .unwrap_or(0);
        let new_size = key.len() + value.len();
        if area.used_bytes() + new_size - previous_size > QUOTA_BYTES_PER_ORIGIN {
            return Err(QuotaExceeded);
        }

        match area.items.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => area.items.push((key.to_string(), value.to_string())),
        }
        Ok(())
    }

    /// Quitar una clave inexistente es un no-op silencioso, no un error -
    /// igual que el spec.
    pub fn remove_item(&mut self, kind: StorageKind, origin: &str, key: &str) {
        if let Some(area) = self.area_mut(kind).get_mut(origin) {
            area.items.retain(|(k, _)| k != key);
        }
    }

    pub fn clear(&mut self, kind: StorageKind, origin: &str) {
        if let Some(area) = self.area_mut(kind).get_mut(origin) {
            area.items.clear();
        }
    }

    pub fn length(&self, kind: StorageKind, origin: &str) -> usize {
        self.area(kind).get(origin).map(|a| a.items.len()).unwrap_or(0)
    }

    /// La clave en la posicion `index`, o `None` si esta fuera de rango
    /// (que en JS se traduce a `null`, no a una excepcion).
    pub fn key_at(&self, kind: StorageKind, origin: &str, index: usize) -> Option<String> {
        self.area(kind).get(origin)?.items.get(index).map(|(k, _)| k.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("URL de prueba valida")
    }

    #[test]
    fn origin_includes_scheme_host_and_non_default_port() {
        assert_eq!(origin_of(&url("https://ejemplo.test/una/ruta?x=1")), "https://ejemplo.test");
        assert_eq!(origin_of(&url("http://ejemplo.test:8080/")), "http://ejemplo.test:8080");
    }

    /// El puerto por defecto del esquema NO forma parte del origen - si
    /// no, `https://a.test` y `https://a.test:443` serian origenes
    /// distintos y no compartirian almacenamiento, cuando son el mismo.
    #[test]
    fn the_default_port_is_not_part_of_the_origin() {
        assert_eq!(origin_of(&url("https://ejemplo.test:443/")), origin_of(&url("https://ejemplo.test/")));
        assert_eq!(origin_of(&url("http://ejemplo.test:80/")), origin_of(&url("http://ejemplo.test/")));
    }

    #[test]
    fn set_and_get_round_trip() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "https://a.test", "tema", "oscuro").unwrap();
        assert_eq!(s.get_item(StorageKind::Local, "https://a.test", "tema"), Some("oscuro".to_string()));
    }

    #[test]
    fn a_missing_key_is_none_not_an_empty_string() {
        let s = WebStorage::new();
        assert_eq!(s.get_item(StorageKind::Local, "https://a.test", "noexiste"), None);
    }

    /// El aislamiento que de verdad importa: un origen no puede leer el
    /// almacenamiento de otro.
    #[test]
    fn one_origin_cannot_see_another_origins_storage() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "https://a.test", "secreto", "1234").unwrap();
        assert_eq!(s.get_item(StorageKind::Local, "https://b.test", "secreto"), None);
    }

    /// Mismo host pero esquema distinto = origen distinto, igual que el
    /// spec: `http://` no debe ver lo que guardo `https://`.
    #[test]
    fn http_and_https_of_the_same_host_are_different_origins() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "https://a.test", "k", "v").unwrap();
        assert_eq!(s.get_item(StorageKind::Local, "http://a.test", "k"), None);
    }

    #[test]
    fn local_and_session_are_separate_areas_for_the_same_origin() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "https://a.test", "k", "de-local").unwrap();
        s.set_item(StorageKind::Session, "https://a.test", "k", "de-sesion").unwrap();
        assert_eq!(s.get_item(StorageKind::Local, "https://a.test", "k"), Some("de-local".to_string()));
        assert_eq!(s.get_item(StorageKind::Session, "https://a.test", "k"), Some("de-sesion".to_string()));
    }

    #[test]
    fn setting_an_existing_key_replaces_the_value_and_keeps_its_position() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "o", "primera", "1").unwrap();
        s.set_item(StorageKind::Local, "o", "segunda", "2").unwrap();
        s.set_item(StorageKind::Local, "o", "primera", "nuevo").unwrap();

        assert_eq!(s.length(StorageKind::Local, "o"), 2, "sustituir no deberia añadir una entrada nueva");
        assert_eq!(s.key_at(StorageKind::Local, "o", 0), Some("primera".to_string()), "deberia conservar su posicion original");
        assert_eq!(s.get_item(StorageKind::Local, "o", "primera"), Some("nuevo".to_string()));
    }

    #[test]
    fn length_and_key_follow_insertion_order() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "o", "a", "1").unwrap();
        s.set_item(StorageKind::Local, "o", "b", "2").unwrap();
        assert_eq!(s.length(StorageKind::Local, "o"), 2);
        assert_eq!(s.key_at(StorageKind::Local, "o", 0), Some("a".to_string()));
        assert_eq!(s.key_at(StorageKind::Local, "o", 1), Some("b".to_string()));
        assert_eq!(s.key_at(StorageKind::Local, "o", 2), None, "fuera de rango deberia ser None (null en JS), no un panico");
    }

    #[test]
    fn remove_item_deletes_only_that_key_and_missing_ones_are_a_no_op() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "o", "a", "1").unwrap();
        s.set_item(StorageKind::Local, "o", "b", "2").unwrap();
        s.remove_item(StorageKind::Local, "o", "a");
        s.remove_item(StorageKind::Local, "o", "noexiste");
        assert_eq!(s.get_item(StorageKind::Local, "o", "a"), None);
        assert_eq!(s.get_item(StorageKind::Local, "o", "b"), Some("2".to_string()));
    }

    #[test]
    fn clear_empties_only_the_given_origin_and_area() {
        let mut s = WebStorage::new();
        s.set_item(StorageKind::Local, "a", "k", "v").unwrap();
        s.set_item(StorageKind::Local, "b", "k", "v").unwrap();
        s.set_item(StorageKind::Session, "a", "k", "v").unwrap();
        s.clear(StorageKind::Local, "a");
        assert_eq!(s.length(StorageKind::Local, "a"), 0);
        assert_eq!(s.length(StorageKind::Local, "b"), 1, "clear no deberia tocar a otro origen");
        assert_eq!(s.length(StorageKind::Session, "a"), 1, "clear no deberia tocar a la otra area");
    }

    #[test]
    fn exceeding_the_quota_is_an_error_instead_of_growing_without_limit() {
        let mut s = WebStorage::new();
        let enorme = "x".repeat(QUOTA_BYTES_PER_ORIGIN + 1);
        assert_eq!(s.set_item(StorageKind::Local, "o", "k", &enorme), Err(QuotaExceeded));
        assert_eq!(s.get_item(StorageKind::Local, "o", "k"), None, "un set que falla no deberia dejar nada a medias");
    }

    /// Sustituir un valor grande por uno pequeño LIBERA su espacio - si el
    /// calculo de cuota no descontara el tamaño anterior, una pagina que
    /// reescribe la misma clave muchas veces agotaria la cuota enseguida
    /// aunque nunca crezca de verdad.
    #[test]
    fn replacing_a_value_frees_the_space_of_the_old_one() {
        let mut s = WebStorage::new();
        let casi_todo = "x".repeat(QUOTA_BYTES_PER_ORIGIN - 10);
        s.set_item(StorageKind::Local, "o", "k", &casi_todo).unwrap();
        s.set_item(StorageKind::Local, "o", "k", "pequeño").expect("reescribir mas pequeño deberia caber");
        assert_eq!(s.get_item(StorageKind::Local, "o", "k"), Some("pequeño".to_string()));
    }
}
