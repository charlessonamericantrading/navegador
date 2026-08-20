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

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.headers.get("content-type").map(|s| s.as_str())
    }
}
