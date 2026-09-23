//! El protocolo entre un renderer y el broker (ADR 0001, etapa 2).
//!
//! Cada mensaje es una **trama**: `[u32 LE largo de cabecera][cabecera JSON]
//! [u32 LE largo del cuerpo][cuerpo]`. La cabecera dice qué es; el cuerpo son
//! los bytes de una petición o respuesta HTTP, en crudo. No es NDJSON como el
//! protocolo con Electron porque aquí viajan imágenes y fuentes: en base64
//! dentro de JSON costarían un tercio más y otra copia.
//!
//! La conversación es:
//!
//! 1. El renderer abre el canal y manda `Hello` con su token.
//! 2. El broker contesta `Welcome`: aceptado (con el nombre del renderer que
//!    le corresponde a ese token) o rechazado. Si rechaza, cierra.
//! 3. A partir de ahí, `Call { id, op }` → `Reply { id, outcome }`. Las
//!    respuestas pueden llegar en otro orden que las llamadas (una descarga
//!    lenta no bloquea un `localStorage.getItem`); `id` las empareja.
//!
//! Los tipos de la red (`NetworkRequest`, `NetworkResponse`) no se serializan
//! tal cual: aquí hay tipos propios del cable y conversiones explícitas, para
//! que cambiar uno no cambie el otro sin que nadie se entere.

use crate::http_client::NetworkError;
use crate::request::{Method, NetworkRequest};
use crate::response::NetworkResponse;
use crate::storage::StorageKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// La cabecera más grande: la que lleva un valor de `localStorage` entero
/// (la cuota es de 5 MiB por origen, y escapado en JSON puede crecer hasta
/// seis veces en el peor caso, `\u0001`). 32 MiB lo cubre y sigue cortando a
/// quien intente agotar la memoria del otro lado con un largo inventado.
pub const MAX_HEADER_BYTES: usize = 32 * 1024 * 1024;
/// El cuerpo más grande que cruza el canal: una respuesta HTTP entera.
pub const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

/// Una trama leída: cabecera todavía sin interpretar y cuerpo.
#[derive(Debug)]
pub struct Frame {
    pub header: Vec<u8>,
    pub body: Vec<u8>,
}

impl Frame {
    /// Interpreta la cabecera como `T`. Un JSON que no encaja es
    /// `InvalidData`: el otro lado habla otro protocolo o está roto.
    pub fn parse<T: for<'de> Deserialize<'de>>(&self) -> io::Result<T> {
        serde_json::from_slice(&self.header).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// Serializa una cabecera. Aparte de `write_frame` porque el cliente
/// serializa en el hilo que llama y escribe desde otra tarea.
pub fn encode_header(header: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(header).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if bytes.len() > MAX_HEADER_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("cabecera de {} bytes, el máximo es {MAX_HEADER_BYTES}", bytes.len())));
    }
    Ok(bytes)
}

/// Escribe una trama con la cabecera ya serializada.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, header: &[u8], body: &[u8]) -> io::Result<()> {
    if header.len() > MAX_HEADER_BYTES || body.len() > MAX_BODY_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "trama demasiado grande para el canal del broker"));
    }
    writer.write_all(&(header.len() as u32).to_le_bytes()).await?;
    writer.write_all(header).await?;
    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

/// Serializa y escribe en un paso.
pub async fn send<W: AsyncWrite + Unpin>(writer: &mut W, header: &impl Serialize, body: &[u8]) -> io::Result<()> {
    write_frame(writer, &encode_header(header)?, body).await
}

/// Lee una trama. `Ok(None)` es que el otro lado cerró limpiamente entre dos
/// tramas; un cierre a mitad de una es `UnexpectedEof`.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let header = read_block(reader, u32::from_le_bytes(len) as usize, MAX_HEADER_BYTES).await?;
    reader.read_exact(&mut len).await?;
    let body = read_block(reader, u32::from_le_bytes(len) as usize, MAX_BODY_BYTES).await?;
    Ok(Some(Frame { header, body }))
}

async fn read_block<R: AsyncRead + Unpin>(reader: &mut R, len: usize, max: usize) -> io::Result<Vec<u8>> {
    // Se comprueba ANTES de reservar: el largo lo pone el otro lado, y un
    // `Vec::with_capacity(4 GiB)` sería un corte de memoria gratis.
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("trama de {len} bytes, el máximo es {max}")));
    }
    let mut block = vec![0u8; len];
    reader.read_exact(&mut block).await?;
    Ok(block)
}

// ---------------------------------------------------------------------------
// Mensajes
// ---------------------------------------------------------------------------

/// Primer mensaje del renderer.
#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub token: String,
}

/// Respuesta del broker al `Hello`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Welcome {
    Accepted { renderer: String },
    Rejected { reason: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Call {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// Lo que un renderer puede pedir: las nueve operaciones de
/// `ResourceBroker`, una a una.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// El cuerpo de la petición, si lo hay, va en el cuerpo de la trama;
    /// `has_body` distingue "sin cuerpo" de "cuerpo vacío".
    Fetch {
        url: String,
        method: String,
        headers: HashMap<String, String>,
        has_body: bool,
        origin: Option<String>,
        include_credentials: bool,
        #[serde(default)]
        navigation: bool,
    },
    CookieGet { page_url: String },
    CookieSet { raw: String, page_url: String },
    StorageGet { kind: StorageKind, origin: String, key: String },
    StorageSet { kind: StorageKind, origin: String, key: String, value: String },
    StorageRemove { kind: StorageKind, origin: String, key: String },
    StorageClear { kind: StorageKind, origin: String },
    StorageLength { kind: StorageKind, origin: String },
    StorageKey { kind: StorageKind, origin: String, index: usize },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    /// Respuesta HTTP; el cuerpo va en el de la trama.
    Response {
        url: String,
        status_code: u16,
        status_text: String,
        headers: HashMap<String, String>,
        set_cookie: Vec<String>,
    },
    /// La petición no se pudo hacer, o CORS no deja leer la respuesta.
    NetworkFailed { cors: bool, message: String },
    Text { value: String },
    MaybeText { value: Option<String> },
    Count { value: usize },
    Done,
    QuotaExceeded,
    /// El broker no hace esto para este renderer (política, no fallo).
    Denied { reason: String },
    /// La llamada no se entiende (método HTTP desconocido, URL rota...).
    Invalid { reason: String },
}

// ---------------------------------------------------------------------------
// Conversiones con los tipos de la red
// ---------------------------------------------------------------------------

impl Op {
    /// Una petición del motor como llamada al broker, con su cuerpo aparte.
    pub fn from_request(request: &NetworkRequest) -> (Op, Vec<u8>) {
        let op = Op::Fetch {
            url: request.url.to_string(),
            method: request.method.as_str().to_string(),
            headers: request.headers.clone(),
            has_body: request.body.is_some(),
            origin: request.origin.clone(),
            include_credentials: request.include_credentials,
            navigation: request.navigation,
        };
        (op, request.body.clone().unwrap_or_default())
    }
}

/// Reconstruye la petición en el broker. `Err` con el motivo si no es válida.
pub fn request_from_fetch(
    url: &str,
    method: &str,
    headers: HashMap<String, String>,
    body: Option<Vec<u8>>,
    origin: Option<String>,
    include_credentials: bool,
    navigation: bool,
) -> Result<NetworkRequest, String> {
    let url = url::Url::parse(url).map_err(|e| format!("URL inválida: {e}"))?;
    let method = Method::parse(method).ok_or_else(|| format!("método HTTP no soportado: {method}"))?;
    Ok(NetworkRequest { url, method, headers, body, origin, include_credentials, navigation })
}

impl Outcome {
    /// El resultado de `NetworkEngine::fetch` como respuesta del broker, con
    /// su cuerpo aparte.
    pub fn from_fetch(result: Result<NetworkResponse, NetworkError>) -> (Outcome, Vec<u8>) {
        match result {
            Ok(response) => (
                Outcome::Response {
                    url: response.url.to_string(),
                    status_code: response.status_code,
                    status_text: response.status_text,
                    headers: response.headers,
                    set_cookie: response.set_cookie,
                },
                response.body.to_vec(),
            ),
            Err(error) => (Outcome::NetworkFailed { cors: matches!(error, NetworkError::Cors(_)), message: error.to_string() }, Vec::new()),
        }
    }

    /// Lo inverso, en el renderer.
    pub fn into_fetch_result(self, body: Vec<u8>) -> Result<NetworkResponse, NetworkError> {
        match self {
            Outcome::Response { url, status_code, status_text, headers, set_cookie } => Ok(NetworkResponse {
                url: url::Url::parse(&url).map_err(|e| NetworkError::Remote(format!("el broker devolvió una URL inválida: {e}")))?,
                status_code,
                status_text,
                headers,
                set_cookie,
                body: body.into(),
            }),
            Outcome::NetworkFailed { cors: true, message } => Err(NetworkError::Cors(message)),
            Outcome::NetworkFailed { cors: false, message } => Err(NetworkError::Remote(message)),
            Outcome::Denied { reason } => Err(NetworkError::Remote(format!("el broker denegó la petición: {reason}"))),
            Outcome::Invalid { reason } => Err(NetworkError::Remote(format!("el broker rechazó la petición: {reason}"))),
            other => Err(NetworkError::Remote(format!("respuesta inesperada del broker a una petición: {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_frame_round_trips_header_and_binary_body() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let body: Vec<u8> = (0..=255).collect();
        let escritor = tokio::spawn(async move {
            send(&mut a, &Call { id: 7, op: Op::StorageLength { kind: StorageKind::Local, origin: "https://a.test".into() } }, &body).await.unwrap();
        });
        let frame = read_frame(&mut b).await.unwrap().unwrap();
        escritor.await.unwrap();
        let call: Call = frame.parse().unwrap();
        assert_eq!(call.id, 7);
        assert!(matches!(call.op, Op::StorageLength { kind: StorageKind::Local, .. }));
        assert_eq!(frame.body, (0..=255).collect::<Vec<u8>>());
    }

    #[tokio::test]
    async fn a_clean_close_between_frames_is_not_an_error() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    /// El largo lo pone el otro lado: uno absurdo se rechaza sin reservar.
    #[tokio::test]
    async fn an_oversized_length_is_rejected_before_allocating() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        let error = read_frame(&mut b).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_request_survives_the_wire() {
        let mut request = NetworkRequest::new("https://a.test/api").unwrap();
        request.method = Method::Post;
        request.body = Some(b"x=1".to_vec());
        request.origin = Some("https://a.test".into());
        let (op, body) = Op::from_request(&request);
        let Op::Fetch { url, method, headers, has_body, origin, include_credentials, navigation } = op else { panic!() };
        let back = request_from_fetch(&url, &method, headers, has_body.then_some(body), origin, include_credentials, navigation).unwrap();
        assert_eq!(back.url, request.url);
        assert_eq!(back.method.as_str(), "POST");
        assert_eq!(back.body.as_deref(), Some(&b"x=1"[..]));
        assert_eq!(back.origin.as_deref(), Some("https://a.test"));
        assert_eq!(back.headers, request.headers);
    }

    #[test]
    fn a_cors_failure_stays_a_cors_failure_across_the_wire() {
        let (outcome, body) = Outcome::from_fetch(Err(NetworkError::Cors("falta Access-Control-Allow-Origin".into())));
        let error = outcome.into_fetch_result(body).unwrap_err();
        assert!(matches!(error, NetworkError::Cors(ref m) if m.contains("Access-Control")));
    }

    #[test]
    fn an_unknown_method_is_rejected_not_turned_into_get() {
        assert!(request_from_fetch("https://a.test/", "TRACE", HashMap::new(), None, None, false, false).is_err());
    }
}
