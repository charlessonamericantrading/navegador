use crate::cookie::CookieStore;
use crate::request::{Method, NetworkRequest};
use crate::response::NetworkResponse;
use thiserror::Error;
use std::collections::HashMap;
use url::Url;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes as HyperBytes;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use hyper_rustls::HttpsConnector;
use std::sync::Mutex;

#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("Failed to parse URL: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("invalid URI: {0}")]
    InvalidUri(#[from] hyper::http::uri::InvalidUri),
    #[error("failed to build request: {0}")]
    RequestBuild(String),
    #[error("failed to read response body: {0}")]
    Body(String),
    #[error("too many redirects (more than {0})")]
    TooManyRedirects(u8),
    #[error("failed to decompress response body ({0}): {1}")]
    Decompress(String, String),
    /// La respuesta llego bien pero el origen que la pidio no puede LEERLA
    /// (Fase 20, ver `crate::cors`). Es un error distinto de un fallo de
    /// red a proposito: la peticion si se hizo, el servidor si contesto, y
    /// el mensaje explica exactamente que cabecera falta - que es lo que
    /// un desarrollador necesita para arreglarlo en su servidor.
    #[error("{0}")]
    Cors(String),
}

/// Descomprime el cuerpo de la respuesta segun `Content-Encoding` - casi
/// todo servidor real comprime HTML/CSS/JS (gzip sobre todo, brotli cada
/// vez mas), y sin esto el parser HTML recibia bytes binarios en vez de
/// texto. `identity` (o cualquier valor no reconocido) devuelve el cuerpo
/// tal cual, igual que un navegador real que no soporta una codificacion
/// pedida - no es un error, es el caso "sin comprimir".
///
/// Logica pura (sin I/O de red) para poder probarla con cuerpos ya
/// comprimidos en memoria.
fn decompress_body(body: &[u8], content_encoding: Option<&str>) -> Result<Vec<u8>, NetworkError> {
    use std::io::Read;
    match content_encoding.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        Some("gzip") | Some("x-gzip") => {
            let mut out = Vec::new();
            flate2::read::MultiGzDecoder::new(body)
                .read_to_end(&mut out)
                .map_err(|e| NetworkError::Decompress("gzip".to_string(), e.to_string()))?;
            Ok(out)
        }
        // El "deflate" de HTTP es, en la practica, casi siempre zlib con su
        // cabecera de 2 bytes (RFC 1950), no el flujo deflate crudo (RFC
        // 1951) que el nombre sugiere - asi lo interpretan los navegadores
        // reales, y es lo que decodifica `ZlibDecoder`.
        Some("deflate") => {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(body)
                .read_to_end(&mut out)
                .map_err(|e| NetworkError::Decompress("deflate".to_string(), e.to_string()))?;
            Ok(out)
        }
        Some("br") => {
            let mut out = Vec::new();
            brotli::Decompressor::new(body, 4096)
                .read_to_end(&mut out)
                .map_err(|e| NetworkError::Decompress("br".to_string(), e.to_string()))?;
            Ok(out)
        }
        _ => Ok(body.to_vec()),
    }
}

/// Limite de saltos de redireccion en una sola cadena antes de rendirse -
/// mismo valor que usa el fetch spec de WHATWG (`redirect count` maximo),
/// para no quedar atrapados en un bucle de redirecciones que se apunten
/// entre si.
const MAX_REDIRECTS: u8 = 20;

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

enum RedirectDecision {
    Done,
    Follow(NetworkRequest),
}

/// Logica pura de "que peticion hacer despues de esta respuesta" -
/// deliberadamente separada de `NetworkEngine::fetch` (que si hace I/O de
/// red de verdad) para poder probarla con casos concretos sin levantar un
/// servidor HTTP.
fn redirect_decision(current: &NetworkRequest, response: &NetworkResponse) -> RedirectDecision {
    if !is_redirect_status(response.status_code) {
        return RedirectDecision::Done;
    }
    let Some(location) = response.headers.get("location") else {
        return RedirectDecision::Done;
    };
    let Ok(next_url) = current.url.join(location) else {
        return RedirectDecision::Done;
    };

    let mut next = current.clone();
    next.url = next_url;
    // 307/308 preservan metodo y cuerpo exactos (esa es su unica diferencia
    // frente a 301/302/303). El resto degrada a GET sin cuerpo cuando el
    // metodo original no era ya GET/HEAD, igual que hace un navegador real.
    if response.status_code != 307
        && response.status_code != 308
        && !matches!(current.method, Method::Get | Method::Head)
    {
        next.method = Method::Get;
        next.body = None;
    }
    RedirectDecision::Follow(next)
}

/// Cliente HTTP/1.1 real con soporte HTTPS (hyper + rustls, raices de
/// confianza de webpki-roots). Sustituye al cliente anterior de sockets TCP
/// en crudo, que no hacia TLS y por tanto no podia cargar ningun sitio
/// https:// (que es la inmensa mayoria de la web real).
pub struct NetworkEngine {
    /// `Full<Bytes>` y no `Empty` (Fase 16): el cuerpo de la peticion
    /// antes era SIEMPRE vacio por tipo, asi que ningun POST podia enviar
    /// datos aunque `NetworkRequest::body` los llevara - el campo existia
    /// y se ignoraba en silencio. `Full` con bytes vacios se comporta
    /// igual que `Empty` para GET/HEAD, asi que el cambio no altera nada
    /// de lo que ya funcionaba.
    client: Client<HttpsConnector<HttpConnector>, Full<HyperBytes>>,
    /// Almacen de cookies COMPARTIDO por todas las peticiones de esta
    /// sesion - es lo que hace que una sesion iniciada en una peticion
    /// siga viva en la siguiente (y a traves de redirecciones, ver
    /// `fetch`). `Mutex` y no `RwLock` porque casi todo acceso escribe
    /// (`header_for` purga caducadas de paso), asi que no habria lecturas
    /// concurrentes reales que ganar.
    cookies: Mutex<CookieStore>,
}

/// rustls 0.23 exige un CryptoProvider de proceso instalado explicitamente
/// antes de la primera conexion TLS cuando hay mas de un backend disponible
/// en el grafo de dependencias (aqui: 'ring', fijado en el Cargo.toml raiz
/// del workspace). Sin esto, la primera peticion https:// entra en panic en
/// tiempo de ejecucion pese a compilar sin ningun error ni warning - solo se
/// descubrio ejecutando el binario de verdad, no solo compilandolo.
static CRYPTO_PROVIDER_INIT: std::sync::Once = std::sync::Once::new();

impl NetworkEngine {
    pub fn new() -> Self {
        CRYPTO_PROVIDER_INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });

        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new()).build(https);
        Self { client, cookies: Mutex::new(CookieStore::new()) }
    }

    /// Sigue redirecciones 301/302/303/307/308 de verdad en vez de devolver
    /// el cuerpo (normalmente vacio) de la respuesta de redireccion al
    /// llamador - la inmensa mayoria de la web real redirige al menos una
    /// vez (http -> https, sin `www` -> con `www`...), asi que sin esto
    /// ninguna pagina real llegaba a parsearse nunca.
    ///
    /// 301/302/303 con un metodo distinto de GET/HEAD degradan a GET sin
    /// cuerpo antes de repetir la peticion, igual que hace un navegador
    /// real (no lo que dice el RFC original de 301/302, sino el
    /// comportamiento de facto que fetch/XHR estandarizaron despues). 307
    /// y 308 preservan metodo y cuerpo exactos, que es su unica razon de
    /// existir frente a 301/302.
    pub async fn fetch(&self, req: &NetworkRequest) -> Result<NetworkResponse, NetworkError> {
        // Politica de mismo origen (Fase 20). Solo se aplica a peticiones
        // que trae un origen adjunto, es decir las que inicio un script
        // (`fetch`/XHR) - la navegacion y los subrecursos pasan de largo,
        // ver `crate::cors` para por que esa frontera es la correcta.
        let target_origin = crate::cors::origin_of(&req.url);
        let cross_origin = req.origin.as_ref().is_some_and(|o| *o != target_origin);

        if cross_origin {
            let origin = req.origin.clone().unwrap_or_default();

            // Preflight: una peticion que no es "simple" necesita permiso
            // ANTES de mandarse de verdad. Se hace asi y no despues
            // porque el objetivo del preflight es no ejecutar en el
            // servidor algo que quiza no estaba autorizado (un DELETE, por
            // ejemplo) - comprobar a posteriori no evitaria el daño.
            if crate::cors::needs_preflight(req) {
                let mut preflight = req.clone();
                preflight.method = Method::Options;
                preflight.body = None;
                preflight.headers.insert("Access-Control-Request-Method".to_string(), req.method.as_str().to_string());
                let author_headers: Vec<String> = req
                    .headers
                    .keys()
                    .map(|k| k.to_ascii_lowercase())
                    .filter(|k| !matches!(k.as_str(), "user-agent" | "accept-encoding" | "cookie" | "origin" | "content-length" | "referer" | "accept" | "accept-language"))
                    .collect();
                if !author_headers.is_empty() {
                    preflight.headers.insert("Access-Control-Request-Headers".to_string(), author_headers.join(", "));
                }

                let preflight_response = self.fetch_once(&preflight).await?;
                crate::cors::check_preflight(&preflight_response, req, &origin, req.include_credentials)
                    .map_err(|rejection| NetworkError::Cors(rejection.message(&origin)))?;
            }

            let response = self.fetch_redirect_chain(req).await?;
            crate::cors::check_response(&response, &origin, req.include_credentials)
                .map_err(|rejection| NetworkError::Cors(rejection.message(&origin)))?;
            return Ok(response);
        }

        self.fetch_redirect_chain(req).await
    }

    /// El bucle de redirecciones de siempre, sin ninguna comprobacion de
    /// origen - separado de `fetch` para que la politica de mismo origen
    /// quede en un solo sitio, envolviendo a esto, en vez de mezclada con
    /// la logica de saltos.
    async fn fetch_redirect_chain(&self, req: &NetworkRequest) -> Result<NetworkResponse, NetworkError> {
        let mut current = req.clone();
        for redirects_followed in 0..=MAX_REDIRECTS {
            let response = self.fetch_once(&current).await?;
            match redirect_decision(&current, &response) {
                RedirectDecision::Done => return Ok(response),
                RedirectDecision::Follow(next) => {
                    if redirects_followed == MAX_REDIRECTS {
                        return Err(NetworkError::TooManyRedirects(MAX_REDIRECTS));
                    }
                    current = next;
                }
            }
        }
        unreachable!("el bucle siempre retorna dentro de sus MAX_REDIRECTS+1 iteraciones")
    }

    async fn fetch_once(&self, req: &NetworkRequest) -> Result<NetworkResponse, NetworkError> {
        let uri: hyper::Uri = req.url.as_str().parse()?;

        tracing::info!("[http_client] Solicitando {} (hyper + rustls)", req.url);

        let mut builder = hyper::Request::builder().method(req.method.as_str()).uri(uri);
        for (k, v) in &req.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        // Cookies que aplican a ESTA URL (dominio/ruta/`Secure` ya
        // filtrados por el almacen). Se añade aqui, en `fetch_once`, y no
        // una sola vez en `fetch`, precisamente para que cada salto de una
        // redireccion recalcule sus propias cookies: un login real casi
        // siempre es POST -> 302 -> GET, y la cookie de sesion la pone la
        // respuesta del POST para que viaje ya en el GET siguiente.
        //
        // Fase 20: a otro origen NO viajan salvo que se pidan
        // explicitamente (`credentials: "same-origin"` es el valor por
        // defecto real del fetch spec). Mandar la sesion del usuario a un
        // tercero sin que nadie lo haya pedido es justo el ataque que la
        // politica de mismo origen existe para impedir.
        let cross_origin = req.origin.as_ref().is_some_and(|o| *o != crate::cors::origin_of(&req.url));
        if !cross_origin {
            if let Some(cookie_header) = self.cookies.lock().ok().and_then(|mut store| store.header_for(&req.url)) {
                builder = builder.header("Cookie", cookie_header);
            }
        } else if req.include_credentials {
            // Fase 30: `credentials: "include"` pide mandar la sesion a
            // OTRO origen a proposito, pero eso NO deberia saltarse
            // `SameSite` - solo las cookies `SameSite=None` (la unica
            // marca que declara "quiero viajar tambien de tercera parte")
            // van aqui, `Strict`/`Lax` se quedan en casa aunque el script
            // haya pedido `include`. Ver el aviso de `cookie.rs` para el
            // porque de la aproximacion por ORIGEN en vez de SITIO real.
            if let Some(cookie_header) = self.cookies.lock().ok().and_then(|mut store| store.header_for_cross_site(&req.url)) {
                builder = builder.header("Cookie", cookie_header);
            }
        }
        // La cabecera `Origin` le dice al servidor quien pregunta, para
        // que pueda decidir si le responde con permiso CORS.
        if cross_origin {
            if let Some(origin) = &req.origin {
                builder = builder.header("Origin", origin.as_str());
            }
        }
        // `Content-Length` explicito: hyper lo pondria solo para un
        // `Full`, pero un servidor real que reciba un POST sin el puede
        // rechazarlo, y dejarlo escrito aqui hace evidente que el cuerpo
        // viaja de verdad.
        let body_bytes = req.body.clone().unwrap_or_default();
        if !body_bytes.is_empty() {
            builder = builder.header("Content-Length", body_bytes.len().to_string());
        }
        let hyper_req = builder
            .body(Full::new(HyperBytes::from(body_bytes)))
            .map_err(|e| NetworkError::RequestBuild(e.to_string()))?;

        let res = self
            .client
            .request(hyper_req)
            .await
            .map_err(|e| NetworkError::Http(e.to_string()))?;

        let status_code = res.status().as_u16();
        let status_text = res.status().to_string();

        let mut headers = HashMap::new();
        // `Set-Cookie` es la unica cabecera que se recoge APARTE, en un
        // `Vec`: es la unica que un servidor real repite de forma
        // legitima y con significado (una respuesta de login deja varias
        // a la vez), y el `HashMap` de abajo solo puede guardar la
        // ultima. Antes de esto se perdian todas menos una - un bug que
        // no se veia porque nadie leia cookies todavia.
        let mut set_cookie = Vec::new();
        for (name, value) in res.headers() {
            if let Ok(val) = value.to_str() {
                if name.as_str().eq_ignore_ascii_case("set-cookie") {
                    set_cookie.push(val.to_string());
                }
                headers.insert(name.as_str().to_lowercase(), val.to_string());
            }
        }
        if !set_cookie.is_empty() {
            if let Ok(mut store) = self.cookies.lock() {
                store.store_from_response(&set_cookie, &req.url);
            }
        }

        let body_bytes = res
            .into_body()
            .collect()
            .await
            .map_err(|e| NetworkError::Body(e.to_string()))?
            .to_bytes();

        let decompressed = decompress_body(&body_bytes, headers.get("content-encoding").map(String::as_str))?;

        Ok(NetworkResponse {
            url: req.url.clone(),
            status_code,
            status_text,
            headers,
            set_cookie,
            body: bytes::Bytes::from(decompressed),
        })
    }

    /// El valor real de `document.cookie` para `page_url` (Fase 24, ver
    /// `engine_js::cookie`) - SIN las cookies `HttpOnly` (esa es su
    /// proteccion, ver `cookie::CookieStore::header_for_js`). Cadena vacia
    /// (nunca `None`, a diferencia de `header_for`) porque eso es lo que
    /// devuelve `document.cookie` en un documento sin cookies: no hay
    /// cabecera HTTP que omitir, es una propiedad de JS que siempre existe.
    /// `page_url` ilegible (documento sin URL propia) tambien da cadena
    /// vacia, no un error - no hay origen contra el que mirar nada.
    pub fn cookie_header_for_js(&self, page_url: &str) -> String {
        let Ok(url) = Url::parse(page_url) else { return String::new() };
        self.cookies.lock().ok().and_then(|mut store| store.header_for_js(&url)).unwrap_or_default()
    }

    /// Escribe una cookie desde `document.cookie = "..."` (Fase 24) contra
    /// `page_url`, en el MISMO almacen que ya usan las peticiones de red -
    /// una cookie puesta por JS viaja despues en el `Cookie:` de un
    /// `fetch()`/navegacion, igual que en un navegador real. No-op
    /// silencioso si `page_url` no es una URL valida.
    pub fn set_cookie_from_js(&self, raw: &str, page_url: &str) {
        let Ok(url) = Url::parse(page_url) else { return };
        if let Ok(mut store) = self.cookies.lock() {
            store.set_from_js(raw, &url);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn identity_content_encoding_is_returned_unchanged() {
        let plain = b"hola mundo sin comprimir";
        assert_eq!(decompress_body(plain, None).unwrap(), plain);
        assert_eq!(decompress_body(plain, Some("identity")).unwrap(), plain);
    }

    #[test]
    fn unknown_content_encoding_falls_back_to_the_raw_body_instead_of_erroring() {
        let bytes = b"cuerpo con una codificacion que no reconocemos";
        assert_eq!(decompress_body(bytes, Some("zstd")).unwrap(), bytes);
    }

    #[test]
    fn gzip_body_decompresses_to_the_original_text() {
        let original = b"<html><body>Hola, comprimido con gzip</body></html>";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        assert_eq!(decompress_body(&compressed, Some("gzip")).unwrap(), original);
        assert_eq!(decompress_body(&compressed, Some("GZIP")).unwrap(), original, "content-encoding deberia ser case-insensitive");
    }

    #[test]
    fn deflate_body_decompresses_to_the_original_text() {
        let original = b"<html><body>Hola, comprimido con deflate/zlib</body></html>";
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        assert_eq!(decompress_body(&compressed, Some("deflate")).unwrap(), original);
    }

    #[test]
    fn brotli_body_decompresses_to_the_original_text() {
        let original = b"<html><body>Hola, comprimido con brotli</body></html>";
        let mut compressed = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, 5, 22);
            writer.write_all(original).unwrap();
        }

        assert_eq!(decompress_body(&compressed, Some("br")).unwrap(), original);
    }

    #[test]
    fn malformed_gzip_body_is_a_decompress_error_not_a_silent_pass_through() {
        let not_actually_gzip = b"esto no es gzip de verdad";
        let err = decompress_body(not_actually_gzip, Some("gzip")).unwrap_err();
        assert!(matches!(err, NetworkError::Decompress(ref enc, _) if enc == "gzip"));
    }

    fn response_with(status_code: u16, location: Option<&str>) -> NetworkResponse {
        let mut headers = HashMap::new();
        if let Some(loc) = location {
            headers.insert("location".to_string(), loc.to_string());
        }
        NetworkResponse {
            url: "https://example.com/".parse().unwrap(),
            status_code,
            status_text: status_code.to_string(),
            headers,
            set_cookie: Vec::new(),
            body: bytes::Bytes::new(),
        }
    }

    #[test]
    fn non_redirect_status_is_done() {
        let req = NetworkRequest::new("https://example.com/").unwrap();
        let res = response_with(200, None);
        assert!(matches!(redirect_decision(&req, &res), RedirectDecision::Done));
    }

    #[test]
    fn redirect_status_without_location_header_is_done() {
        let req = NetworkRequest::new("https://example.com/").unwrap();
        let res = response_with(302, None);
        assert!(matches!(redirect_decision(&req, &res), RedirectDecision::Done));
    }

    #[test]
    fn redirect_status_with_unparseable_location_is_done() {
        let req = NetworkRequest::new("https://example.com/").unwrap();
        // Una direccion IPv6 mal formada no puede resolverse con `Url::join`
        // (`InvalidIpv6Address`) - un navegador real tampoco sigue una
        // redireccion asi, solo se rinde y entrega la respuesta tal cual.
        let res = response_with(302, Some("https://[invalid"));
        assert!(matches!(redirect_decision(&req, &res), RedirectDecision::Done));
    }

    #[test]
    fn get_request_follows_302_keeping_get() {
        let req = NetworkRequest::new("https://example.com/old").unwrap();
        let res = response_with(302, Some("https://example.com/new"));
        match redirect_decision(&req, &res) {
            RedirectDecision::Follow(next) => {
                assert_eq!(next.url.as_str(), "https://example.com/new");
                assert!(matches!(next.method, Method::Get));
            }
            RedirectDecision::Done => panic!("deberia seguir la redireccion"),
        }
    }

    #[test]
    fn relative_location_resolves_against_the_current_url() {
        let req = NetworkRequest::new("https://example.com/a/b").unwrap();
        let res = response_with(301, Some("/c"));
        match redirect_decision(&req, &res) {
            RedirectDecision::Follow(next) => assert_eq!(next.url.as_str(), "https://example.com/c"),
            RedirectDecision::Done => panic!("deberia resolver la ruta relativa"),
        }
    }

    #[test]
    fn post_request_downgrades_to_get_without_body_on_302() {
        let mut req = NetworkRequest::new("https://example.com/submit").unwrap();
        req.method = Method::Post;
        req.body = Some(b"campo=valor".to_vec());
        let res = response_with(302, Some("https://example.com/done"));
        match redirect_decision(&req, &res) {
            RedirectDecision::Follow(next) => {
                assert!(matches!(next.method, Method::Get), "302 deberia degradar POST a GET");
                assert!(next.body.is_none(), "el cuerpo no deberia sobrevivir a la degradacion a GET");
            }
            RedirectDecision::Done => panic!("deberia seguir la redireccion"),
        }
    }

    #[test]
    fn post_request_preserves_method_and_body_on_307() {
        let mut req = NetworkRequest::new("https://example.com/submit").unwrap();
        req.method = Method::Post;
        req.body = Some(b"campo=valor".to_vec());
        let res = response_with(307, Some("https://example.com/submit-de-nuevo"));
        match redirect_decision(&req, &res) {
            RedirectDecision::Follow(next) => {
                assert!(matches!(next.method, Method::Post), "307 no deberia cambiar el metodo");
                assert_eq!(next.body.as_deref(), Some(b"campo=valor".as_slice()), "307 no deberia tirar el cuerpo");
            }
            RedirectDecision::Done => panic!("deberia seguir la redireccion"),
        }
    }

    #[test]
    fn post_request_preserves_method_and_body_on_308() {
        let mut req = NetworkRequest::new("https://example.com/submit").unwrap();
        req.method = Method::Post;
        req.body = Some(b"campo=valor".to_vec());
        let res = response_with(308, Some("https://example.com/submit-de-nuevo"));
        match redirect_decision(&req, &res) {
            RedirectDecision::Follow(next) => {
                assert!(matches!(next.method, Method::Post), "308 no deberia cambiar el metodo");
                assert!(next.body.is_some(), "308 no deberia tirar el cuerpo");
            }
            RedirectDecision::Done => panic!("deberia seguir la redireccion"),
        }
    }

    #[test]
    fn head_request_is_not_downgraded_by_a_303() {
        let mut req = NetworkRequest::new("https://example.com/resource").unwrap();
        req.method = Method::Head;
        let res = response_with(303, Some("https://example.com/other"));
        match redirect_decision(&req, &res) {
            RedirectDecision::Follow(next) => assert!(matches!(next.method, Method::Head)),
            RedirectDecision::Done => panic!("deberia seguir la redireccion"),
        }
    }

    #[test]
    fn document_cookie_write_then_read_round_trips_through_the_same_store() {
        let engine = NetworkEngine::new();
        assert_eq!(engine.cookie_header_for_js("https://ejemplo.test/"), "", "sin cookies deberia ser cadena vacia, no un error");

        engine.set_cookie_from_js("tema=oscuro; Path=/", "https://ejemplo.test/pagina");
        assert_eq!(engine.cookie_header_for_js("https://ejemplo.test/"), "tema=oscuro");
    }

    #[test]
    fn document_cookie_never_exposes_an_http_only_cookie_set_by_a_server() {
        let engine = NetworkEngine::new();
        if let Ok(mut store) = engine.cookies.lock() {
            store.store_from_response(&["sesion=secreta; HttpOnly".to_string()], &Url::parse("https://ejemplo.test/").unwrap());
        }
        assert_eq!(engine.cookie_header_for_js("https://ejemplo.test/"), "", "una cookie HttpOnly no deberia llegar nunca a JS");
    }

    #[test]
    fn an_unparseable_page_url_is_a_silent_no_op_not_a_panic() {
        let engine = NetworkEngine::new();
        engine.set_cookie_from_js("a=1", "no-es-una-url");
        assert_eq!(engine.cookie_header_for_js("tampoco-una-url"), "");
    }
}
