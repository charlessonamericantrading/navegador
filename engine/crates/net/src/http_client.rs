use crate::request::NetworkRequest;
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

    pub async fn fetch(&self, req: &NetworkRequest) -> Result<NetworkResponse, NetworkError> {
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

        Ok(NetworkResponse {
            status_code,
            status_text,
            headers,
            body: bytes::Bytes::from(body_bytes.to_vec()),
        })
    }
}
