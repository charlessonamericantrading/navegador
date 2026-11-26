//! `ResourceBroker` sobre el canal: lo que usa un renderer cuyo broker vive
//! en otro proceso (ADR 0001, etapa 2).
//!
//! Dos tareas de Tokio atienden el canal: una escribe las llamadas en orden
//! y otra lee las respuestas y se las entrega a quien espera cada `id`. Hay
//! dos formas de esperar, porque la interfaz tiene de las dos:
//!
//! - `fetch` es asíncrona y espera con un `oneshot`.
//! - Cookies y almacenamiento son **síncronos** en JavaScript
//!   (`localStorage.getItem` devuelve el valor, no una promesa), así que el
//!   hilo que llama se bloquea en un canal de `std` hasta que la tarea
//!   lectora le da la respuesta. Por eso hace falta el runtime **multihilo**:
//!   con uno de un solo hilo, la tarea lectora nunca correría mientras el
//!   hilo espera, y la llamada no volvería.
//!
//! Si el broker cae, todas las llamadas pendientes y las siguientes fallan
//! enseguida con el motivo, en vez de colgarse: una red que falla, un
//! `localStorage` vacío y un `setItem` que lanza `QuotaExceededError`.

use crate::broker::{BrokerFuture, ResourceBroker};
use crate::broker_transport::connect;
use crate::broker_wire::{encode_header, read_frame, send, write_frame, Call, Hello, Op, Outcome, Reply, Welcome};
use crate::http_client::NetworkError;
use crate::request::NetworkRequest;
use crate::response::NetworkResponse;
use crate::storage::{QuotaExceeded, StorageKind};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Lo más que una llamada síncrona bloquea el JavaScript de la página. El
/// broker contesta a estas en microsegundos; si tarda esto, algo va mal, y
/// es mejor que la página vea un fallo que quedarse congelada.
const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

type Answer = Result<(Outcome, Vec<u8>), String>;

enum Waiter {
    Blocking(std::sync::mpsc::Sender<Answer>),
    Async(oneshot::Sender<Answer>),
}

impl Waiter {
    fn deliver(self, answer: Answer) {
        // Si quien esperaba ya se fue (timeout), la respuesta sobra.
        match self {
            Waiter::Blocking(tx) => drop(tx.send(answer)),
            Waiter::Async(tx) => drop(tx.send(answer)),
        }
    }
}

#[derive(Default)]
struct Pending {
    /// `Some(motivo)` en cuanto el canal deja de servir; ya no se aceptan
    /// llamadas. Vive en el mismo candado que `waiters` para que ninguna
    /// llamada se registre justo después de vaciarlos y se quede esperando
    /// para siempre.
    closed: Option<String>,
    waiters: HashMap<u64, Waiter>,
}

struct Channel {
    next_id: AtomicU64,
    pending: Mutex<Pending>,
    outgoing: mpsc::UnboundedSender<(Vec<u8>, Vec<u8>)>,
}

impl Channel {
    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn submit(&self, op: Op, body: Vec<u8>, waiter: Waiter) -> Result<u64, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let header = encode_header(&Call { id, op }).map_err(|e| e.to_string())?;
        let mut pending = self.pending();
        if let Some(reason) = &pending.closed {
            return Err(reason.clone());
        }
        pending.waiters.insert(id, waiter);
        if self.outgoing.send((header, body)).is_err() {
            pending.waiters.remove(&id);
            return Err("el canal hacia el broker está cerrado".to_string());
        }
        Ok(id)
    }

    fn deliver(&self, id: u64, answer: Answer) {
        let waiter = self.pending().waiters.remove(&id);
        match waiter {
            Some(waiter) => waiter.deliver(answer),
            None => tracing::debug!("[broker] respuesta {id} sin nadie esperándola (¿venció su plazo?)"),
        }
    }

    fn close(&self, reason: String) {
        let waiters = {
            let mut pending = self.pending();
            if pending.closed.is_none() {
                tracing::warn!("[broker] canal cerrado: {reason}");
                pending.closed = Some(reason.clone());
            }
            std::mem::take(&mut pending.waiters)
        };
        for waiter in waiters.into_values() {
            waiter.deliver(Err(reason.clone()));
        }
    }

    fn call_blocking(&self, op: Op) -> Answer {
        let (tx, rx) = std::sync::mpsc::channel();
        let id = self.submit(op, Vec::new(), Waiter::Blocking(tx))?;
        match rx.recv_timeout(SYNC_TIMEOUT) {
            Ok(answer) => answer,
            Err(_) => {
                self.pending().waiters.remove(&id);
                Err(format!("el broker no contestó en {SYNC_TIMEOUT:?}"))
            }
        }
    }

    async fn call_async(&self, op: Op, body: Vec<u8>) -> Answer {
        let (tx, rx) = oneshot::channel();
        self.submit(op, body, Waiter::Async(tx))?;
        rx.await.unwrap_or_else(|_| Err("el canal hacia el broker se cerró".to_string()))
    }
}

/// Dónde está el broker (lo que `engine_broker` anuncia en su `ready`).
pub const BROKER_ENDPOINT_ENV: &str = "NAVEGADOR_IA_BROKER";
/// El token de este renderer (lo que el broker devolvió al registrarlo).
pub const BROKER_TOKEN_ENV: &str = "NAVEGADOR_IA_BROKER_TOKEN";

/// El broker que pide el entorno del proceso, si pide alguno. `Ok(None)` es
/// que no se pidió; pedirlo a medias (canal sin token) o no poder conectar
/// es un error, nunca un `None`: quien lo pidió cuenta con él.
pub async fn from_env() -> io::Result<Option<RemoteBroker>> {
    let Some(endpoint) = std::env::var(BROKER_ENDPOINT_ENV).ok().filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let token = std::env::var(BROKER_TOKEN_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{BROKER_ENDPOINT_ENV} sin {BROKER_TOKEN_ENV}")))?;
    RemoteBroker::connect(&endpoint, &token)
        .await
        .map(Some)
        .map_err(|e| io::Error::new(e.kind(), format!("no se pudo conectar con el broker en {endpoint}: {e}")))
}

/// Broker en otro proceso, visto desde el renderer.
pub struct RemoteBroker {
    channel: Arc<Channel>,
    renderer: String,
}

impl RemoteBroker {
    /// Abre el canal, se presenta con `token` y arranca las dos tareas.
    /// Falla si el broker no está, si rechaza el token o si no se llama
    /// desde un runtime multihilo (ver el doc-comment del módulo).
    pub async fn connect(endpoint: &str, token: &str) -> io::Result<Self> {
        if tokio::runtime::Handle::current().runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err(io::Error::other(
                "RemoteBroker necesita el runtime multihilo de Tokio: las llamadas síncronas bloquean el hilo que llama mientras otra tarea lee la respuesta",
            ));
        }
        let stream = connect(endpoint).await?;
        let (mut reader, mut writer) = tokio::io::split(stream);

        send(&mut writer, &Hello { token: token.to_string() }, &[]).await?;
        let welcome = read_frame(&mut reader)
            .await?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "el broker cerró el canal sin contestar al saludo"))?
            .parse::<Welcome>()?;
        let renderer = match welcome {
            Welcome::Accepted { renderer } => renderer,
            Welcome::Rejected { reason } => return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("el broker rechazó el token: {reason}"))),
        };

        let (outgoing, mut queue) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();
        let channel = Arc::new(Channel { next_id: AtomicU64::new(1), pending: Mutex::default(), outgoing });

        // La escritora solo guarda un `Weak`: si el `RemoteBroker` se suelta,
        // el emisor de la cola muere con él y esta tarea termina.
        let for_writer: Weak<Channel> = Arc::downgrade(&channel);
        tokio::spawn(async move {
            while let Some((header, body)) = queue.recv().await {
                if let Err(e) = write_frame(&mut writer, &header, &body).await {
                    if let Some(channel) = for_writer.upgrade() {
                        channel.close(format!("no se pudo escribir al broker: {e}"));
                    }
                    break;
                }
            }
        });

        let for_reader = Arc::downgrade(&channel);
        tokio::spawn(async move {
            let reason = loop {
                match read_frame(&mut reader).await {
                    Ok(Some(frame)) => match frame.parse::<Reply>() {
                        Ok(reply) => match for_reader.upgrade() {
                            Some(channel) => channel.deliver(reply.id, Ok((reply.outcome, frame.body))),
                            None => return,
                        },
                        Err(e) => break format!("respuesta ilegible del broker: {e}"),
                    },
                    Ok(None) => break "el broker cerró el canal".to_string(),
                    Err(e) => break format!("error leyendo del broker: {e}"),
                }
            };
            if let Some(channel) = for_reader.upgrade() {
                channel.close(reason);
            }
        });

        Ok(Self { channel, renderer })
    }

    /// El nombre con el que el broker conoce a este renderer.
    pub fn renderer(&self) -> &str {
        &self.renderer
    }

    /// Llamada síncrona; `None` si no hubo respuesta (ya registrado).
    fn blocking(&self, op: Op) -> Option<Outcome> {
        let name = op_name(&op);
        match self.channel.call_blocking(op) {
            Ok((outcome, _)) => {
                if let Outcome::Denied { reason } | Outcome::Invalid { reason } = &outcome {
                    tracing::warn!("[broker] {name} rechazada: {reason}");
                }
                Some(outcome)
            }
            Err(reason) => {
                tracing::warn!("[broker] {name} sin respuesta: {reason}");
                None
            }
        }
    }
}

fn op_name(op: &Op) -> &'static str {
    match op {
        Op::Fetch { .. } => "fetch",
        Op::CookieGet { .. } => "cookie_get",
        Op::CookieSet { .. } => "cookie_set",
        Op::StorageGet { .. } => "storage_get",
        Op::StorageSet { .. } => "storage_set",
        Op::StorageRemove { .. } => "storage_remove",
        Op::StorageClear { .. } => "storage_clear",
        Op::StorageLength { .. } => "storage_length",
        Op::StorageKey { .. } => "storage_key",
    }
}

impl ResourceBroker for RemoteBroker {
    fn fetch<'a>(&'a self, request: &'a NetworkRequest) -> BrokerFuture<'a, Result<NetworkResponse, NetworkError>> {
        Box::pin(async move {
            let (op, body) = Op::from_request(request);
            match self.channel.call_async(op, body).await {
                Ok((outcome, body)) => outcome.into_fetch_result(body),
                Err(reason) => Err(NetworkError::Remote(format!("broker no disponible: {reason}"))),
            }
        })
    }

    fn cookie_header_for_js(&self, page_url: &str) -> String {
        match self.blocking(Op::CookieGet { page_url: page_url.to_string() }) {
            Some(Outcome::Text { value }) => value,
            _ => String::new(),
        }
    }

    fn set_cookie_from_js(&self, raw: &str, page_url: &str) {
        // Síncrona aunque no devuelva nada: un `document.cookie` leído justo
        // después tiene que ver la cookie recién puesta.
        self.blocking(Op::CookieSet { raw: raw.to_string(), page_url: page_url.to_string() });
    }

    fn storage_get(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String> {
        match self.blocking(Op::StorageGet { kind, origin: origin.to_string(), key: key.to_string() }) {
            Some(Outcome::MaybeText { value }) => value,
            _ => None,
        }
    }

    fn storage_set(&self, kind: StorageKind, origin: &str, key: &str, value: &str) -> Result<(), QuotaExceeded> {
        match self.blocking(Op::StorageSet { kind, origin: origin.to_string(), key: key.to_string(), value: value.to_string() }) {
            Some(Outcome::Done) => Ok(()),
            // Sin broker o sin permiso tampoco se guardó nada: la página ve
            // el mismo error que ante un almacén lleno, que es el que sabe
            // manejar.
            _ => Err(QuotaExceeded),
        }
    }

    fn storage_remove(&self, kind: StorageKind, origin: &str, key: &str) {
        self.blocking(Op::StorageRemove { kind, origin: origin.to_string(), key: key.to_string() });
    }

    fn storage_clear(&self, kind: StorageKind, origin: &str) {
        self.blocking(Op::StorageClear { kind, origin: origin.to_string() });
    }

    fn storage_length(&self, kind: StorageKind, origin: &str) -> usize {
        match self.blocking(Op::StorageLength { kind, origin: origin.to_string() }) {
            Some(Outcome::Count { value }) => value,
            _ => 0,
        }
    }

    fn storage_key(&self, kind: StorageKind, origin: &str, index: usize) -> Option<String> {
        match self.blocking(Op::StorageKey { kind, origin: origin.to_string(), index }) {
            Some(Outcome::MaybeText { value }) => value,
            _ => None,
        }
    }
}
