//! Politica de mismo origen y CORS reales (Fase 20).
//!
//! Antes de esta fase esto era un stub que devolvia `true` siempre y que
//! ademas **nadie llamaba**: `NetworkEngine::fetch` no lo invocaba, asi que
//! un `fetch()` desde JavaScript podia leer la respuesta de CUALQUIER
//! dominio. Sin politica de mismo origen no hay modelo de seguridad web:
//! es la primitiva sobre la que descansa todo lo demas.
//!
//! ## Donde se aplica y donde no (la parte que mas importa)
//!
//! CORS solo gobierna las peticiones que un SCRIPT inicia y cuya respuesta
//! quiere LEER (`fetch()`, `XMLHttpRequest`). Deliberadamente NO se aplica
//! a:
//! - La **navegacion** de la pagina (`core::server::navigate`): escribir
//!   una URL en la barra no es una peticion de origen cruzado, es cambiar
//!   de origen.
//! - Los **subrecursos** (`<link rel=stylesheet>`, `<script src>`,
//!   `<img>`): en el spec real van en modo "no-cors", que permite
//!   descargarlos de otro dominio pero no leer su contenido desde JS. Este
//!   motor tampoco los expone a JS, asi que el efecto es el mismo.
//!
//! Esa frontera es exactamente la que separa `engine-js` (fetch/XHR, con
//! origen) de `core::server` (navegacion y subrecursos, sin origen), asi
//! que el modelo se aplica solo pasando `origin` o no.
//!
//! ## Lo que SI implementa
//!
//! - Comparacion de origen real (`esquema://host:puerto`): mismo origen
//!   pasa sin ninguna comprobacion; distinto origen exige CORS.
//! - Cabecera `Origin` en las peticiones de origen cruzado.
//! - Comprobacion de `Access-Control-Allow-Origin` en la respuesta (`*` o
//!   coincidencia exacta).
//! - Distincion entre peticion **simple** y con **preflight**, con la lista
//!   segura de metodos y cabeceras del spec.
//! - **Credenciales**: las cookies NO viajan a otro origen salvo que se
//!   pidan explicitamente, que es el valor por defecto real del fetch spec
//!   (`credentials: "same-origin"`). Y con credenciales, `*` NO vale como
//!   `Access-Control-Allow-Origin` y hace falta ademas
//!   `Access-Control-Allow-Credentials: true` - las dos reglas que impiden
//!   que un servidor abra su API a todo el mundo por accidente.
//!
//! ## Lo que NO implementa, declarado
//!
//! - **Cache de preflight** (`Access-Control-Max-Age`): cada peticion no
//!   simple manda su OPTIONS, sin reutilizar el permiso anterior. Correcto
//!   pero mas lento que un navegador real.
//! - `Access-Control-Expose-Headers`: este motor no filtra que cabeceras
//!   de respuesta ve el script, asi que las expone todas. Es mas permisivo
//!   que el spec y queda declarado aqui en vez de fingirse.

use crate::request::{Method, NetworkRequest};
use crate::response::NetworkResponse;
use url::Url;

/// El origen de una URL (`esquema://host:puerto`) - la unidad de
/// aislamiento de toda la seguridad web. Identico al de
/// `crate::storage::origin_of`; se duplica la llamada y no el codigo.
pub fn origin_of(url: &Url) -> String {
    crate::storage::origin_of(url)
}

/// Metodos que no necesitan preflight (fetch spec, "CORS-safelisted
/// method").
fn is_safelisted_method(method: &Method) -> bool {
    matches!(method, Method::Get | Method::Head | Method::Post)
}

/// Cabeceras que un script puede poner sin disparar preflight (fetch
/// spec, "CORS-safelisted request-header"). `content-type` solo cuenta
/// como segura con ciertos valores - ver `is_safelisted_content_type`.
const SAFELISTED_HEADERS: &[&str] = &["accept", "accept-language", "content-language", "content-type"];

/// Los tres tipos de contenido que un formulario HTML podia enviar ya
/// antes de que CORS existiera - de ahi que sean los unicos que no
/// disparan preflight: permitirlos no añade ninguna capacidad nueva a un
/// atacante.
fn is_safelisted_content_type(value: &str) -> bool {
    let value = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(value.as_str(), "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain")
}

/// Cabeceras que el propio motor pone en toda peticion y que por tanto no
/// deben contarse como "cabeceras de autor" al decidir el preflight - si
/// contaran, CUALQUIER peticion de origen cruzado necesitaria preflight y
/// la distincion no serviria de nada.
const ENGINE_HEADERS: &[&str] = &["user-agent", "accept-encoding", "cookie", "origin", "content-length", "referer"];

/// Si esta peticion se puede mandar directamente (simple) o necesita un
/// OPTIONS de permiso antes (preflight).
pub fn needs_preflight(request: &NetworkRequest) -> bool {
    if !is_safelisted_method(&request.method) {
        return true;
    }
    request.headers.iter().any(|(name, value)| {
        let lower = name.to_ascii_lowercase();
        if ENGINE_HEADERS.contains(&lower.as_str()) {
            return false;
        }
        if !SAFELISTED_HEADERS.contains(&lower.as_str()) {
            return true;
        }
        lower == "content-type" && !is_safelisted_content_type(value)
    })
}

/// Por que una respuesta de origen cruzado no se puede leer. Se distingue
/// el motivo para poder decirselo al desarrollador con precision: "no hay
/// cabecera" y "la cabecera no te incluye" se arreglan de formas
/// distintas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsRejection {
    MissingAllowOrigin,
    OriginNotAllowed { allowed: String },
    /// `Access-Control-Allow-Origin: *` NO vale cuando la peticion lleva
    /// credenciales - si valiera, un servidor que abre su API "a todos"
    /// estaria exponiendo tambien las sesiones de sus usuarios.
    WildcardWithCredentials,
    CredentialsNotAllowed,
}

impl CorsRejection {
    pub fn message(&self, origin: &str) -> String {
        match self {
            Self::MissingAllowOrigin => format!(
                "bloqueado por CORS: la respuesta no trae 'Access-Control-Allow-Origin', asi que el origen '{origin}' no puede leerla"
            ),
            Self::OriginNotAllowed { allowed } => format!(
                "bloqueado por CORS: 'Access-Control-Allow-Origin' vale '{allowed}', que no incluye a '{origin}'"
            ),
            Self::WildcardWithCredentials => format!(
                "bloqueado por CORS: con credenciales no se admite 'Access-Control-Allow-Origin: *'; el servidor debe nombrar a '{origin}' explicitamente"
            ),
            Self::CredentialsNotAllowed => {
                "bloqueado por CORS: la peticion lleva credenciales pero la respuesta no trae 'Access-Control-Allow-Credentials: true'".to_string()
            }
        }
    }
}

/// Decide si un script del origen `origin` puede LEER esta respuesta.
///
/// `Ok(())` para permitida. Solo debe llamarse con respuestas de ORIGEN
/// CRUZADO: una del mismo origen no pasa por aqui en absoluto (ver
/// `NetworkEngine::fetch`), porque exigirle cabeceras CORS a tu propio
/// servidor no tendria ningun sentido.
pub fn check_response(response: &NetworkResponse, origin: &str, with_credentials: bool) -> Result<(), CorsRejection> {
    let Some(allowed) = response.headers.get("access-control-allow-origin") else {
        return Err(CorsRejection::MissingAllowOrigin);
    };
    let allowed = allowed.trim();

    if with_credentials {
        let credentials_ok = response
            .headers
            .get("access-control-allow-credentials")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        if !credentials_ok {
            return Err(CorsRejection::CredentialsNotAllowed);
        }
        if allowed == "*" {
            return Err(CorsRejection::WildcardWithCredentials);
        }
    } else if allowed == "*" {
        return Ok(());
    }

    // Comparacion EXACTA y sensible a mayusculas del origen serializado -
    // el spec no admite coincidencias parciales ni comodines de subdominio
    // aqui, y aceptarlos seria justo el agujero que CORS existe para tapar.
    if allowed == origin {
        Ok(())
    } else {
        Err(CorsRejection::OriginNotAllowed { allowed: allowed.to_string() })
    }
}

/// Si la respuesta a un OPTIONS de preflight autoriza la peticion real que
/// venia detras.
pub fn check_preflight(response: &NetworkResponse, request: &NetworkRequest, origin: &str, with_credentials: bool) -> Result<(), CorsRejection> {
    check_response(response, origin, with_credentials)?;

    // El metodo tiene que estar permitido explicitamente, salvo que ya sea
    // uno de los que nunca necesitan permiso.
    if !is_safelisted_method(&request.method) {
        let allowed_methods = response
            .headers
            .get("access-control-allow-methods")
            .map(|v| v.to_ascii_uppercase())
            .unwrap_or_default();
        let method_ok = allowed_methods.split(',').any(|m| m.trim() == request.method.as_str()) || allowed_methods.trim() == "*";
        if !method_ok {
            return Err(CorsRejection::OriginNotAllowed { allowed: allowed_methods });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn response_with(headers: &[(&str, &str)]) -> NetworkResponse {
        NetworkResponse {
            url: Url::parse("https://api.test/datos").unwrap(),
            status_code: 200,
            status_text: "OK".to_string(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            set_cookie: Vec::new(),
            body: bytes::Bytes::new(),
        }
    }

    fn request_with(method: Method, headers: &[(&str, &str)]) -> NetworkRequest {
        let mut r = NetworkRequest::new("https://api.test/datos").unwrap();
        r.method = method;
        r.headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        r
    }

    #[test]
    fn a_response_without_allow_origin_is_blocked() {
        let res = response_with(&[]);
        assert_eq!(check_response(&res, "https://web.test", false), Err(CorsRejection::MissingAllowOrigin));
    }

    #[test]
    fn a_wildcard_allows_a_request_without_credentials() {
        let res = response_with(&[("access-control-allow-origin", "*")]);
        assert!(check_response(&res, "https://web.test", false).is_ok());
    }

    #[test]
    fn an_exact_origin_match_is_allowed() {
        let res = response_with(&[("access-control-allow-origin", "https://web.test")]);
        assert!(check_response(&res, "https://web.test", false).is_ok());
    }

    /// El agujero que CORS existe para tapar: nada de coincidencias
    /// parciales ni comodines de subdominio.
    #[test]
    fn a_different_origin_is_blocked_even_if_it_looks_similar() {
        let res = response_with(&[("access-control-allow-origin", "https://web.test")]);
        assert!(matches!(
            check_response(&res, "https://malweb.test", false),
            Err(CorsRejection::OriginNotAllowed { .. })
        ));
        assert!(matches!(
            check_response(&res, "https://sub.web.test", false),
            Err(CorsRejection::OriginNotAllowed { .. })
        ));
        // Mismo host pero otro esquema TAMPOCO es el mismo origen.
        assert!(matches!(
            check_response(&res, "http://web.test", false),
            Err(CorsRejection::OriginNotAllowed { .. })
        ));
    }

    /// Con credenciales, `*` no vale: si valiera, un servidor que abre su
    /// API "a todos" estaria exponiendo tambien las sesiones de sus
    /// usuarios a cualquier web.
    #[test]
    fn a_wildcard_is_rejected_when_the_request_carries_credentials() {
        let res = response_with(&[("access-control-allow-origin", "*"), ("access-control-allow-credentials", "true")]);
        assert_eq!(check_response(&res, "https://web.test", true), Err(CorsRejection::WildcardWithCredentials));
    }

    #[test]
    fn credentials_require_the_allow_credentials_header() {
        let res = response_with(&[("access-control-allow-origin", "https://web.test")]);
        assert_eq!(check_response(&res, "https://web.test", true), Err(CorsRejection::CredentialsNotAllowed));

        let con_permiso = response_with(&[
            ("access-control-allow-origin", "https://web.test"),
            ("access-control-allow-credentials", "true"),
        ]);
        assert!(check_response(&con_permiso, "https://web.test", true).is_ok());
    }

    #[test]
    fn simple_methods_with_safelisted_headers_need_no_preflight() {
        assert!(!needs_preflight(&request_with(Method::Get, &[])));
        assert!(!needs_preflight(&request_with(Method::Post, &[("Content-Type", "application/x-www-form-urlencoded")])));
        assert!(!needs_preflight(&request_with(Method::Head, &[("Accept", "text/html")])));
    }

    #[test]
    fn a_non_safelisted_method_needs_preflight() {
        assert!(needs_preflight(&request_with(Method::Put, &[])));
        assert!(needs_preflight(&request_with(Method::Delete, &[])));
        assert!(needs_preflight(&request_with(Method::Patch, &[])));
    }

    /// `application/json` es el caso real mas comun de preflight: no esta
    /// entre los tres tipos que un formulario HTML podia enviar antes de
    /// que CORS existiera.
    #[test]
    fn a_json_content_type_needs_preflight_but_a_form_one_does_not() {
        assert!(needs_preflight(&request_with(Method::Post, &[("Content-Type", "application/json")])));
        assert!(!needs_preflight(&request_with(Method::Post, &[("Content-Type", "text/plain; charset=utf-8")])));
    }

    #[test]
    fn a_custom_header_needs_preflight() {
        assert!(needs_preflight(&request_with(Method::Get, &[("X-Mi-Token", "abc")])));
    }

    /// Las cabeceras que pone el propio motor no cuentan: si contaran,
    /// TODA peticion de origen cruzado necesitaria preflight y la
    /// distincion no serviria de nada.
    #[test]
    fn engine_added_headers_do_not_trigger_a_preflight() {
        let mut r = request_with(Method::Get, &[]);
        r.headers.insert("User-Agent".to_string(), "motor/1".to_string());
        r.headers.insert("Accept-Encoding".to_string(), "gzip".to_string());
        r.headers.insert("Cookie".to_string(), "sesion=abc".to_string());
        assert!(!needs_preflight(&r));
    }

    #[test]
    fn preflight_must_also_allow_the_real_method() {
        let req = request_with(Method::Delete, &[]);
        let sin_metodo = response_with(&[("access-control-allow-origin", "https://web.test")]);
        assert!(check_preflight(&sin_metodo, &req, "https://web.test", false).is_err());

        let con_metodo = response_with(&[
            ("access-control-allow-origin", "https://web.test"),
            ("access-control-allow-methods", "GET, POST, DELETE"),
        ]);
        assert!(check_preflight(&con_metodo, &req, "https://web.test", false).is_ok());
    }

    #[test]
    fn origin_of_matches_the_storage_definition() {
        assert_eq!(origin_of(&Url::parse("https://a.test/x?y=1").unwrap()), "https://a.test");
        assert_eq!(origin_of(&Url::parse("http://a.test:8080/").unwrap()), "http://a.test:8080");
    }

    #[test]
    fn the_rejection_message_names_the_blocked_origin() {
        let msg = CorsRejection::MissingAllowOrigin.message("https://web.test");
        assert!(msg.contains("https://web.test"));
        assert!(msg.contains("Access-Control-Allow-Origin"));
    }

    // Silencia el aviso de import no usado en compilaciones sin este test.
    #[allow(dead_code)]
    fn _unused(_: HashMap<String, String>) {}
}
