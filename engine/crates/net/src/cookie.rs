//! Cookies HTTP reales (RFC 6265) - almacen, parseo de `Set-Cookie`, y
//! seleccion de que cookies viajan en cada peticion.
//!
//! Antes de esta fase esto era un `HashMap<String, String>` en memoria sin
//! ninguna semantica de cookie, que ademas nadie instanciaba: ninguna
//! peticion enviaba ni recibia cookies. La consecuencia real (encontrada
//! auditando el motor) era que **no se podia iniciar sesion en ningun sitio
//! web**, porque toda sesion HTTP se sostiene sobre una cookie.
//!
//! Lo que SI implementa, del RFC 6265:
//! - Parseo del `Set-Cookie` completo: `name=value` mas los atributos
//!   `Domain`, `Path`, `Expires`, `Max-Age`, `Secure`, `HttpOnly` y
//!   `SameSite`.
//! - `Max-Age` gana sobre `Expires` cuando ambos estan presentes (§5.2.2),
//!   y `Max-Age<=0` borra la cookie de inmediato - que es como un servidor
//!   real cierra una sesion.
//! - Alcance por dominio (§5.1.3): coincidencia exacta, o sufijo de dominio
//!   con separador de etiqueta real (`ejemplo.test` cubre `www.ejemplo.test`
//!   pero NUNCA `malejemplo.test`). Una cookie SIN atributo `Domain` es
//!   "host-only": solo vuelve al host exacto que la puso.
//! - Alcance por ruta (§5.1.4), incluida la ruta por defecto derivada de la
//!   URL cuando no hay atributo `Path`.
//! - Expiracion real: una cookie caducada no se devuelve nunca y se purga
//!   del almacen al consultarlo.
//! - `Secure`: esas cookies solo viajan por `https:`.
//! - Identidad `(name, domain, path)` (§5.3.11): un `Set-Cookie` con la
//!   misma terna SUSTITUYE al anterior en vez de acumularse.
//!
//! Simplificaciones declaradas (no implementado a proposito):
//! - **Sin lista de sufijos publicos (PSL)**: no se rechaza un `Domain=
//!   .co.uk` o `.com`, que un navegador real bloquea para impedir que un
//!   sitio ponga una cookie a todo un TLD. Se mitiga parcialmente exigiendo
//!   que el `Domain` declarado cubra de verdad al host de la peticion (un
//!   sitio no puede poner cookies a un dominio ajeno), pero un dominio
//!   demasiado ancho de su propio arbol si se aceptaria.
//! - **`SameSite` se parsea y se guarda pero no se aplica todavia**: exige
//!   distinguir peticion "de primera parte" de "de tercera parte", concepto
//!   que este motor no tiene (no hay `<iframe>` ni contexto de navegacion
//!   anidado). Declarado aqui en vez de fingir la defensa CSRF que aporta.
//! - **`HttpOnly` se parsea y se guarda pero no protege nada todavia**: solo
//!   tendria efecto ante un `document.cookie` en JS, que no existe aun.
//!   Cuando se implemente, esta bandera ya esta lista para filtrarlo.
//! - Sin persistencia a disco: el almacen vive en memoria, asi que las
//!   sesiones no sobreviven a cerrar la aplicacion.

use std::time::{Duration, SystemTime};
use url::Url;

/// Valor de `SameSite` tal como lo declaro el servidor - se guarda para no
/// perder informacion, aunque todavia no se aplique (ver doc del modulo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

#[derive(Debug, Clone)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Siempre en minusculas y SIN el punto inicial que el servidor pueda
    /// haber escrito (`.ejemplo.test` y `ejemplo.test` son la misma cosa
    /// para el RFC 6265, que descarta ese punto historico).
    pub domain: String,
    /// `true` cuando el `Set-Cookie` NO traia atributo `Domain`: la cookie
    /// solo vuelve al host EXACTO que la puso, nunca a un subdominio.
    pub host_only: bool,
    pub path: String,
    /// `None` = cookie de sesion (vive mientras viva el proceso).
    pub expires: Option<SystemTime>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

impl Cookie {
    fn is_expired(&self, now: SystemTime) -> bool {
        self.expires.is_some_and(|exp| exp <= now)
    }
}

/// Ruta por defecto de una cookie sin atributo `Path` (RFC 6265 §5.1.4):
/// el directorio de la URL que la puso, NO la URL entera - una cookie
/// puesta en `/carpeta/pagina.html` tiene ruta `/carpeta`, asi que tambien
/// vuelve en `/carpeta/otra.html` pero no en `/otra-carpeta/`.
fn default_path(url: &Url) -> String {
    let path = url.path();
    if !path.starts_with('/') {
        return "/".to_string();
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => path[..idx].to_string(),
    }
}

/// Coincidencia de dominio (RFC 6265 §5.1.3). `host` es el de la peticion,
/// `cookie_domain` el que la cookie declara (ya normalizado, sin punto).
///
/// El separador de etiqueta es lo que impide el ataque obvio: `ejemplo.test`
/// cubre `www.ejemplo.test` (hay un `.` justo antes del sufijo) pero NO
/// `malejemplo.test`, aunque textualmente tambien "termine en" lo mismo.
fn domain_matches(host: &str, cookie_domain: &str) -> bool {
    if host == cookie_domain {
        return true;
    }
    // Una IP literal nunca hace de sufijo de dominio (el spec lo excluye
    // explicitamente): `127.0.0.1` no cubre a nadie mas que a si misma, y
    // eso ya lo resolvio la igualdad de arriba.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    host.len() > cookie_domain.len()
        && host.ends_with(cookie_domain)
        && host.as_bytes()[host.len() - cookie_domain.len() - 1] == b'.'
}

/// Coincidencia de ruta (RFC 6265 §5.1.4) - prefijo, pero solo en un limite
/// de segmento real: `/foo` cubre `/foo` y `/foo/bar`, nunca `/foobar`.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

/// Parsea las tres formas de fecha que admite HTTP (RFC 7231 §7.1.1.1) para
/// el atributo `Expires`. Se escribe a mano aqui, en vez de traer un crate,
/// por ser un formato cerrado de tres variantes fijas y ~40 lineas; el
/// criterio del proyecto (ver ARCHITECTURE.md, "Doctrina de dependencias")
/// es no reimplementar cosas GRANDES y con superficie de seguridad (TLS,
/// parseo HTML/CSS), no evitar toda funcion propia.
///
/// Devuelve `None` ante cualquier cosa que no entienda, y quien llama trata
/// eso como "sin fecha" (cookie de sesion) - igual que un navegador real,
/// que ignora un `Expires` ilegible en vez de descartar la cookie entera.
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let value = value.trim();
    // Formato preferente: "Sun, 06 Nov 1994 08:49:37 GMT" (RFC 1123), y su
    // variante con guiones "Sunday, 06-Nov-94 08:49:37 GMT" (RFC 850).
    let after_comma = value.split_once(", ").map(|(_, rest)| rest).unwrap_or(value);
    let mut parts = after_comma.split_whitespace();
    let (day_str, month_str, year_str, time_str) = {
        let first = parts.next()?;
        if first.contains('-') {
            // RFC 850: "06-Nov-94 08:49:37 GMT"
            let mut dmy = first.split('-');
            let d = dmy.next()?.to_string();
            let m = dmy.next()?.to_string();
            let y = dmy.next()?.to_string();
            (d, m, y, parts.next()?.to_string())
        } else if first.len() <= 2 {
            // RFC 1123: "06 Nov 1994 08:49:37 GMT"
            (first.to_string(), parts.next()?.to_string(), parts.next()?.to_string(), parts.next()?.to_string())
        } else {
            // asctime: "Sun Nov  6 08:49:37 1994" - el primer token ya era
            // el mes porque el dia de la semana se consumio como `first`.
            let m = parts.next()?.to_string();
            let d = parts.next()?.to_string();
            let t = parts.next()?.to_string();
            let y = parts.next()?.to_string();
            (d, m, y, t)
        }
    };

    let day: u64 = day_str.trim().parse().ok()?;
    let month = match month_str.get(..3)?.to_ascii_lowercase().as_str() {
        "jan" => 1, "feb" => 2, "mar" => 3, "apr" => 4, "may" => 5, "jun" => 6,
        "jul" => 7, "aug" => 8, "sep" => 9, "oct" => 10, "nov" => 11, "dec" => 12,
        _ => return None,
    };
    let mut year: u64 = year_str.trim().parse().ok()?;
    // Años de dos digitos (RFC 850): la regla del spec es que un valor que
    // resulte mas de 50 años en el futuro se interpreta como pasado.
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }

    let mut hms = time_str.split(':');
    let hour: u64 = hms.next()?.parse().ok()?;
    let minute: u64 = hms.next()?.parse().ok()?;
    let second: u64 = hms.next()?.parse().ok()?;
    if month == 0 || day == 0 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second))
}

/// Dias desde 1970-01-01 para una fecha civil - algoritmo de Howard Hinnant
/// (dominio publico), el mismo que usan las implementaciones de `<chrono>`
/// de C++. Vale para cualquier fecha posterior a 1970, que es todo lo que
/// un `Expires` de cookie necesita.
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parsea UNA cabecera `Set-Cookie` contra la URL que la produjo.
///
/// `None` cuando la cabecera es inservible (sin `=`, con nombre vacio) o
/// cuando el `Domain` declarado NO cubre al host de la peticion - esto
/// ultimo es una comprobacion de seguridad real del RFC (§5.3.6): sin ella,
/// cualquier sitio podria poner cookies a cualquier otro dominio.
pub fn parse_set_cookie(header: &str, url: &Url) -> Option<Cookie> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    let host = url.host_str()?.to_ascii_lowercase();
    let mut cookie = Cookie {
        name: name.to_string(),
        value: value.trim().to_string(),
        domain: host.clone(),
        host_only: true,
        path: default_path(url),
        expires: None,
        secure: false,
        http_only: false,
        same_site: SameSite::Lax,
    };

    // `Max-Age` gana sobre `Expires` (§5.2.2), sin importar el orden en que
    // aparezcan en la cabecera - de ahi que se recoja aparte y se aplique
    // al final en vez de escribir `expires` directamente.
    let mut max_age: Option<i64> = None;

    for attr in parts {
        let (key, val) = match attr.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => (attr.trim().to_ascii_lowercase(), String::new()),
        };
        match key.as_str() {
            "domain" => {
                let declared = val.trim_start_matches('.').to_ascii_lowercase();
                if declared.is_empty() {
                    continue;
                }
                // El servidor solo puede ampliar el alcance DENTRO de su
                // propio arbol de dominio, nunca hacia otro sitio.
                if !domain_matches(&host, &declared) && host != declared {
                    return None;
                }
                cookie.domain = declared;
                cookie.host_only = false;
            }
            "path" if val.starts_with('/') => cookie.path = val,
            "expires" => cookie.expires = parse_http_date(&val),
            "max-age" => max_age = val.parse::<i64>().ok(),
            "secure" => cookie.secure = true,
            "httponly" => cookie.http_only = true,
            "samesite" => {
                cookie.same_site = match val.to_ascii_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "none" => SameSite::None,
                    _ => SameSite::Lax,
                }
            }
            _ => {}
        }
    }

    if let Some(seconds) = max_age {
        cookie.expires = Some(if seconds <= 0 {
            // Ya caducada: es como un servidor real BORRA una cookie (cerrar
            // sesion). `UNIX_EPOCH` es cualquier instante del pasado.
            SystemTime::UNIX_EPOCH
        } else {
            SystemTime::now() + Duration::from_secs(seconds as u64)
        });
    }

    Some(cookie)
}

/// Almacen de cookies del navegador - una sola instancia viva por sesion,
/// compartida por todas las peticiones (ver `NetworkEngine`).
#[derive(Debug, Clone, Default)]
pub struct CookieStore {
    cookies: Vec<Cookie>,
}

impl CookieStore {
    pub fn new() -> Self {
        Self { cookies: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Guarda todas las cabeceras `Set-Cookie` de una respuesta. Una
    /// respuesta real puede traer VARIAS (de ahi el slice y no un solo
    /// valor): un login tipico deja la cookie de sesion y alguna de estado
    /// en la misma respuesta.
    pub fn store_from_response(&mut self, headers: &[String], url: &Url) {
        for header in headers {
            if let Some(cookie) = parse_set_cookie(header, url) {
                self.insert(cookie);
            }
        }
    }

    /// Identidad `(name, domain, path)` (§5.3.11): sustituye en vez de
    /// acumular. Una cookie que llega ya caducada no se guarda - y ademas
    /// BORRA a la que sustituiria, que es exactamente el mecanismo con el
    /// que un servidor cierra sesion.
    pub fn insert(&mut self, cookie: Cookie) {
        self.cookies
            .retain(|c| !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path));
        if cookie.is_expired(SystemTime::now()) {
            return;
        }
        self.cookies.push(cookie);
    }

    /// El valor de la cabecera `Cookie:` para esta URL, o `None` si no
    /// aplica ninguna - en cuyo caso quien llama no debe mandar la cabecera
    /// vacia, sino omitirla por completo.
    ///
    /// De paso purga las caducadas, que es el momento natural de hacerlo
    /// (no hace falta ningun temporizador aparte).
    pub fn header_for(&mut self, url: &Url) -> Option<String> {
        let now = SystemTime::now();
        self.cookies.retain(|c| !c.is_expired(now));

        let host = url.host_str()?.to_ascii_lowercase();
        let is_secure = url.scheme() == "https";
        let request_path = url.path();

        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| {
                if c.secure && !is_secure {
                    return false;
                }
                let domain_ok = if c.host_only { host == c.domain } else { domain_matches(&host, &c.domain) };
                domain_ok && path_matches(request_path, &c.path)
            })
            .collect();

        if matching.is_empty() {
            return None;
        }
        // Orden del spec (§5.4.2): ruta mas especifica primero. Algunos
        // servidores reales leen solo el primer valor de un nombre repetido,
        // asi que el orden importa de verdad, no es cosmetico.
        matching.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

        Some(matching.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("URL de prueba valida")
    }

    #[test]
    fn parses_a_minimal_set_cookie_as_host_only_with_the_directory_path() {
        let c = parse_set_cookie("sesion=abc123", &url("https://ejemplo.test/carpeta/pagina.html")).expect("deberia parsear");
        assert_eq!(c.name, "sesion");
        assert_eq!(c.value, "abc123");
        assert_eq!(c.domain, "ejemplo.test");
        assert!(c.host_only, "sin atributo Domain, la cookie es host-only");
        assert_eq!(c.path, "/carpeta", "la ruta por defecto es el DIRECTORIO, no la pagina entera");
        assert!(c.expires.is_none(), "sin Expires/Max-Age es una cookie de sesion");
    }

    #[test]
    fn parses_every_supported_attribute() {
        let c = parse_set_cookie(
            "id=7; Domain=ejemplo.test; Path=/app; Max-Age=3600; Secure; HttpOnly; SameSite=Strict",
            &url("https://www.ejemplo.test/"),
        )
        .expect("deberia parsear");
        assert_eq!(c.domain, "ejemplo.test");
        assert!(!c.host_only, "con Domain explicito deja de ser host-only");
        assert_eq!(c.path, "/app");
        assert!(c.secure && c.http_only);
        assert_eq!(c.same_site, SameSite::Strict);
        assert!(c.expires.is_some_and(|e| e > SystemTime::now()), "Max-Age positivo deberia dar una fecha futura");
    }

    #[test]
    fn a_leading_dot_in_domain_is_ignored_as_the_rfc_requires() {
        let c = parse_set_cookie("a=1; Domain=.ejemplo.test", &url("https://www.ejemplo.test/")).expect("deberia parsear");
        assert_eq!(c.domain, "ejemplo.test", "el punto inicial historico se descarta");
    }

    /// Comprobacion de SEGURIDAD real del RFC: un sitio no puede poner
    /// cookies a un dominio que no sea suyo.
    #[test]
    fn a_set_cookie_for_an_unrelated_domain_is_rejected() {
        assert!(parse_set_cookie("robo=1; Domain=otrositio.test", &url("https://ejemplo.test/")).is_none());
    }

    #[test]
    fn max_age_wins_over_expires_regardless_of_order() {
        let c = parse_set_cookie(
            "a=1; Expires=Sun, 06 Nov 1994 08:49:37 GMT; Max-Age=3600",
            &url("https://ejemplo.test/"),
        )
        .expect("deberia parsear");
        assert!(c.expires.is_some_and(|e| e > SystemTime::now()), "Max-Age deberia ganar aunque Expires venga antes y sea pasado");
    }

    #[test]
    fn parses_the_three_http_date_formats() {
        let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(expected), "RFC 1123");
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(expected), "RFC 850");
        assert_eq!(parse_http_date("Sun Nov 6 08:49:37 1994"), Some(expected), "asctime");
    }

    #[test]
    fn an_unparseable_expires_is_treated_as_a_session_cookie_not_a_rejected_one() {
        let c = parse_set_cookie("a=1; Expires=basura-ilegible", &url("https://ejemplo.test/")).expect("la cookie deberia sobrevivir");
        assert!(c.expires.is_none());
    }

    #[test]
    fn domain_matching_requires_a_real_label_boundary() {
        assert!(domain_matches("www.ejemplo.test", "ejemplo.test"));
        assert!(domain_matches("ejemplo.test", "ejemplo.test"));
        assert!(!domain_matches("malejemplo.test", "ejemplo.test"), "un sufijo textual sin punto separador NO deberia coincidir - es el ataque obvio");
        assert!(!domain_matches("ejemplo.test", "www.ejemplo.test"), "el padre no coincide con el hijo");
    }

    #[test]
    fn path_matching_only_at_segment_boundaries() {
        assert!(path_matches("/foo", "/foo"));
        assert!(path_matches("/foo/bar", "/foo"));
        assert!(path_matches("/foo/bar", "/"));
        assert!(!path_matches("/foobar", "/foo"), "/foo no deberia cubrir /foobar");
    }

    #[test]
    fn a_stored_cookie_comes_back_on_a_matching_request() {
        let mut store = CookieStore::new();
        store.store_from_response(&["sesion=abc".to_string()], &url("https://ejemplo.test/"));
        assert_eq!(store.header_for(&url("https://ejemplo.test/otra")).as_deref(), Some("sesion=abc"));
    }

    #[test]
    fn a_host_only_cookie_does_not_leak_to_subdomains() {
        let mut store = CookieStore::new();
        store.store_from_response(&["a=1".to_string()], &url("https://ejemplo.test/"));
        assert!(store.header_for(&url("https://www.ejemplo.test/")).is_none(), "sin Domain explicito la cookie es solo del host exacto");
    }

    #[test]
    fn a_domain_cookie_does_reach_subdomains() {
        let mut store = CookieStore::new();
        store.store_from_response(&["a=1; Domain=ejemplo.test".to_string()], &url("https://ejemplo.test/"));
        assert_eq!(store.header_for(&url("https://www.ejemplo.test/")).as_deref(), Some("a=1"));
    }

    #[test]
    fn a_secure_cookie_never_travels_over_plain_http() {
        let mut store = CookieStore::new();
        store.store_from_response(&["a=1; Secure".to_string()], &url("https://ejemplo.test/"));
        assert!(store.header_for(&url("http://ejemplo.test/")).is_none());
        assert!(store.header_for(&url("https://ejemplo.test/")).is_some());
    }

    #[test]
    fn a_cookie_out_of_path_scope_is_not_sent() {
        let mut store = CookieStore::new();
        store.store_from_response(&["a=1; Path=/app".to_string()], &url("https://ejemplo.test/"));
        assert!(store.header_for(&url("https://ejemplo.test/otra")).is_none());
        assert!(store.header_for(&url("https://ejemplo.test/app/x")).is_some());
    }

    /// El mecanismo real de "cerrar sesion": el servidor reenvia la misma
    /// cookie ya caducada, y eso debe BORRAR la que habia.
    #[test]
    fn re_sending_an_expired_cookie_deletes_the_stored_one() {
        let mut store = CookieStore::new();
        store.store_from_response(&["sesion=abc".to_string()], &url("https://ejemplo.test/"));
        assert!(store.header_for(&url("https://ejemplo.test/")).is_some());

        store.store_from_response(&["sesion=; Max-Age=0".to_string()], &url("https://ejemplo.test/"));
        assert!(store.header_for(&url("https://ejemplo.test/")).is_none(), "Max-Age=0 deberia haber cerrado la sesion");
        assert!(store.is_empty());
    }

    #[test]
    fn the_same_name_domain_and_path_replaces_instead_of_accumulating() {
        let mut store = CookieStore::new();
        store.store_from_response(&["a=viejo".to_string()], &url("https://ejemplo.test/"));
        store.store_from_response(&["a=nuevo".to_string()], &url("https://ejemplo.test/"));
        assert_eq!(store.len(), 1);
        assert_eq!(store.header_for(&url("https://ejemplo.test/")).as_deref(), Some("a=nuevo"));
    }

    #[test]
    fn several_set_cookie_headers_in_one_response_are_all_stored() {
        let mut store = CookieStore::new();
        store.store_from_response(
            &["sesion=abc".to_string(), "tema=oscuro".to_string()],
            &url("https://ejemplo.test/"),
        );
        assert_eq!(store.len(), 2, "un login real deja varias cookies en la MISMA respuesta");
        let header = store.header_for(&url("https://ejemplo.test/")).expect("deberia haber cabecera");
        assert!(header.contains("sesion=abc") && header.contains("tema=oscuro"));
    }

    #[test]
    fn more_specific_paths_are_sent_first_as_the_spec_orders() {
        let mut store = CookieStore::new();
        store.store_from_response(&["a=raiz".to_string()], &url("https://ejemplo.test/"));
        store.store_from_response(&["b=hondo; Path=/app/sub".to_string()], &url("https://ejemplo.test/app/sub/x"));
        let header = store.header_for(&url("https://ejemplo.test/app/sub/x")).expect("deberia haber cabecera");
        assert!(header.starts_with("b=hondo"), "la ruta mas especifica va primero: {header}");
    }

    #[test]
    fn no_matching_cookies_yields_no_header_at_all_instead_of_an_empty_one() {
        let mut store = CookieStore::new();
        assert!(store.header_for(&url("https://ejemplo.test/")).is_none());
    }
}
