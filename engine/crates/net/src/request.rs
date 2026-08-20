use url::Url;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Patch,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Patch => "PATCH",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkRequest {
    pub url: Url,
    pub method: Method,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
    /// El origen del SCRIPT que inicio esta peticion (Fase 20), si la
    /// inicio uno. Es lo que activa la politica de mismo origen:
    ///
    /// - `Some(origen)` = peticion de `fetch()`/`XMLHttpRequest`. Si el
    ///   destino es otro origen, se aplica CORS de verdad (ver
    ///   `crate::cors`): cabecera `Origin`, preflight si hace falta, y
    ///   comprobacion de `Access-Control-Allow-Origin` antes de devolver
    ///   la respuesta a quien la pidio.
    /// - `None` = navegacion o subrecurso (`core::server`). NO se
    ///   comprueba nada, que es correcto: escribir una URL en la barra no
    ///   es una peticion de origen cruzado, y los subrecursos van en modo
    ///   "no-cors" (se descargan de otro dominio pero su contenido no se
    ///   expone a JS).
    pub origin: Option<String>,
    /// Si las cookies viajan a un origen DISTINTO del que inicio la
    /// peticion. `false` por defecto, que es el valor real del fetch spec
    /// (`credentials: "same-origin"`): mandar la sesion del usuario a
    /// terceros sin que nadie lo pida seria justo lo que CORS existe para
    /// evitar. Al mismo origen las cookies viajan siempre, sin mirar esto.
    pub include_credentials: bool,
}

impl NetworkRequest {
    pub fn new(url_str: &str) -> Result<Self, url::ParseError> {
        let url = Url::parse(url_str)?;
        let mut headers = HashMap::new();
        headers.insert(
            "User-Agent".to_string(),
            "NextGenWebEngine/0.1.0 (Custom 100% Native Engine)".to_string(),
        );
        headers.insert("Accept".to_string(), "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string());
        headers.insert("Accept-Language".to_string(), "en-US,en;q=0.9,es;q=0.8".to_string());
        headers.insert("Accept-Encoding".to_string(), "gzip, deflate, br".to_string());

        Ok(Self {
            url,
            method: Method::Get,
            headers,
            body: None,
            origin: None,
            include_credentials: false,
        })
    }
}

/// Resuelve la URL que un script pidio contra la de la pagina, y devuelve
/// ademas el origen de la pagina (Fase 20.1).
///
/// `fetch('/api/datos')` y `xhr.open('GET', 'datos.json')` son comunisimos
/// en codigo real y antes fallaban al parsear: `Url::parse` exige una URL
/// ABSOLUTA. Resolver contra la pagina es lo que hace cualquier navegador.
///
/// `page_url` a `None` (documento sin URL propia, p.ej. construido en
/// memoria) deja el comportamiento anterior: solo se aceptan URLs
/// absolutas, y no hay origen contra el que aplicar CORS.
pub fn resolve_against_page(raw: &str, page_url: Option<&str>) -> Option<(Url, Option<String>)> {
    match page_url.and_then(|base| Url::parse(base).ok()) {
        Some(base) => {
            let origin = crate::storage::origin_of(&base);
            base.join(raw).ok().map(|url| (url, Some(origin)))
        }
        None => Url::parse(raw).ok().map(|url| (url, None)),
    }
}
