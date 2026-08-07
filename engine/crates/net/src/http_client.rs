use crate::request::{Method, NetworkRequest};
use crate::response::NetworkResponse;
use thiserror::Error;
use std::collections::HashMap;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes as HyperBytes;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use hyper_rustls::HttpsConnector;

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
    client: Client<HttpsConnector<HttpConnector>, Empty<HyperBytes>>,
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
        Self { client }
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
        let hyper_req = builder
            .body(Empty::<HyperBytes>::new())
            .map_err(|e| NetworkError::RequestBuild(e.to_string()))?;

        let res = self
            .client
            .request(hyper_req)
            .await
            .map_err(|e| NetworkError::Http(e.to_string()))?;

        let status_code = res.status().as_u16();
        let status_text = res.status().to_string();

        let mut headers = HashMap::new();
        for (name, value) in res.headers() {
            if let Ok(val) = value.to_str() {
                headers.insert(name.as_str().to_lowercase(), val.to_string());
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
            body: bytes::Bytes::from(decompressed),
        })
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
}
