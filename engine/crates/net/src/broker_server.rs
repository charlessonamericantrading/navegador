//! El broker como proceso (ADR 0001, etapa 2): tiene la red, las cookies y
//! el almacenamiento, y los sirve a los renderers por el canal local.
//!
//! Dos canales, con dos niveles de confianza:
//!
//! - **Control** (stdin/stdout, NDJSON): lo usa el supervisor que arrancó el
//!   broker, que es de confianza. Por ahí registra cada renderer y recibe su
//!   token. Cuando stdin se cierra (el supervisor murió), el broker termina:
//!   no puede quedarse huérfano sirviendo a nadie.
//! - **Renderers** (`crate::broker_transport`, `crate::broker_wire`): contenido
//!   hostil al otro lado. Cada conexión se presenta con un token de un solo
//!   uso, y a partir de ahí todo lo que pide se atribuye al renderer de ese
//!   token. El token se gasta al usarse: si luego se filtra (el entorno de un
//!   proceso no es un secreto frente a otro proceso del mismo usuario), ya no
//!   abre nada.
//!
//! Todos los renderers comparten un `LocalBroker`: un solo tarro de cookies y
//! un solo `localStorage` por perfil, escritos por un único proceso. Eso
//! resuelve que dos pestañas en procesos distintos se pisaran los ficheros.
//! `sessionStorage`, en cambio, es de cada pestaña según la especificación, y
//! aquí se separa por renderer.

use crate::broker::LocalBroker;
use crate::broker_transport::{constant_time_eq, random_hex, Listener, Stream};
use crate::broker_wire::{encode_header, read_frame, request_from_fetch, send, write_frame, Call, Hello, Op, Outcome, Reply, Welcome};
use crate::storage::StorageKind;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Notify};

/// Lo que tiene un renderer recién conectado para presentarse.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct BrokerServer {
    local: LocalBroker,
    /// Token sin usar → renderer al que corresponde.
    tokens: Mutex<HashMap<String, String>>,
    /// Renderer conectado → aviso para cortarle el canal.
    live: Mutex<HashMap<String, Arc<Notify>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl BrokerServer {
    pub fn new(local: LocalBroker) -> Arc<Self> {
        Arc::new(Self { local, tokens: Mutex::default(), live: Mutex::default() })
    }

    /// Da de alta un renderer y devuelve su token. Un token anterior del
    /// mismo renderer que no se llegó a usar deja de valer.
    pub fn register(&self, renderer: &str) -> io::Result<String> {
        let token = random_hex(32)?;
        let mut tokens = lock(&self.tokens);
        tokens.retain(|_, owner| owner != renderer);
        tokens.insert(token.clone(), renderer.to_string());
        Ok(token)
    }

    /// Retira un renderer: su token sin usar deja de valer y, si está
    /// conectado, se le corta el canal.
    pub fn revoke(&self, renderer: &str) {
        lock(&self.tokens).retain(|_, owner| owner != renderer);
        if let Some(notify) = lock(&self.live).remove(renderer) {
            notify.notify_one();
        }
    }

    /// Gasta un token. Se recorren todos con una comparación de tiempo
    /// constante en vez de buscar en el `HashMap`, que compara con `==`.
    fn redeem(&self, token: &str) -> Option<String> {
        let mut tokens = lock(&self.tokens);
        let found = tokens.keys().find(|known| constant_time_eq(known, token)).cloned()?;
        tokens.remove(&found)
    }

    /// Acepta renderers hasta que se suelta la tarea.
    pub async fn serve(self: Arc<Self>, mut listener: Listener) -> io::Result<()> {
        loop {
            let stream = listener.accept().await?;
            tokio::spawn(self.clone().serve_connection(stream));
        }
    }

    async fn serve_connection(self: Arc<Self>, stream: Stream) {
        let (mut reader, mut writer) = tokio::io::split(stream);

        let hello = match tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader)).await {
            Ok(Ok(Some(frame))) => frame.parse::<Hello>().ok(),
            _ => None,
        };
        let Some(hello) = hello else {
            tracing::warn!("[broker] conexión sin saludo válido, se cierra");
            return;
        };
        let Some(renderer) = self.redeem(&hello.token) else {
            tracing::warn!("[broker] token desconocido o ya usado, se rechaza la conexión");
            let _ = send(&mut writer, &Welcome::Rejected { reason: "token desconocido o ya usado".to_string() }, &[]).await;
            return;
        };
        if send(&mut writer, &Welcome::Accepted { renderer: renderer.clone() }, &[]).await.is_err() {
            return;
        }

        let revoked = Arc::new(Notify::new());
        lock(&self.live).insert(renderer.clone(), revoked.clone());

        // Las respuestas salen por una cola: una descarga que termina tarde
        // no puede escribir a la vez que una respuesta de `localStorage`.
        let (outgoing, mut queue) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();
        let writer_task = tokio::spawn(async move {
            while let Some((header, body)) = queue.recv().await {
                if write_frame(&mut writer, &header, &body).await.is_err() {
                    break;
                }
            }
        });

        loop {
            let frame = tokio::select! {
                frame = read_frame(&mut reader) => frame,
                _ = revoked.notified() => {
                    tracing::info!("[broker] {renderer} retirado por el supervisor");
                    break;
                }
            };
            let frame = match frame {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("[broker] canal de {renderer} roto: {e}");
                    break;
                }
            };
            let call: Call = match frame.parse() {
                Ok(call) => call,
                Err(e) => {
                    // Quien manda algo que no es el protocolo no es un
                    // renderer sano: se le corta, no se le adivina.
                    tracing::warn!("[broker] llamada ilegible de {renderer}, se corta el canal: {e}");
                    break;
                }
            };

            match call.op {
                Op::Fetch { url, method, headers, has_body, origin, include_credentials } => {
                    let body = has_body.then_some(frame.body);
                    let server = self.clone();
                    let outgoing = outgoing.clone();
                    tokio::spawn(async move {
                        let (outcome, body) = match request_from_fetch(&url, &method, headers, body, origin, include_credentials) {
                            Ok(request) => Outcome::from_fetch(server.local.network.fetch(&request).await),
                            Err(reason) => (Outcome::Invalid { reason }, Vec::new()),
                        };
                        reply(&outgoing, call.id, outcome, body);
                    });
                }
                // Cookies y almacenamiento son instantáneos y se atienden en
                // orden, en esta misma tarea.
                op => reply(&outgoing, call.id, self.handle(&renderer, op), Vec::new()),
            }
        }

        // Si el supervisor ya registró otra conexión para este renderer, esa
        // entrada no es la nuestra y no se toca.
        {
            let mut live = lock(&self.live);
            if live.get(&renderer).is_some_and(|notify| Arc::ptr_eq(notify, &revoked)) {
                live.remove(&renderer);
            }
        }
        drop(outgoing);
        let _ = writer_task.await;
    }

    /// Las operaciones instantáneas.
    fn handle(&self, renderer: &str, op: Op) -> Outcome {
        match op {
            Op::Fetch { .. } => Outcome::Invalid { reason: "fetch no se atiende aquí".to_string() },
            Op::CookieGet { page_url } => Outcome::Text { value: self.local.network.cookie_header_for_js(&page_url) },
            Op::CookieSet { raw, page_url } => {
                self.local.network.set_cookie_from_js(&raw, &page_url);
                Outcome::Done
            }
            Op::StorageGet { kind, origin, key } => Outcome::MaybeText { value: self.local.storage().get_item(kind, &area(renderer, kind, &origin), &key) },
            Op::StorageSet { kind, origin, key, value } => match self.local.storage().set_item(kind, &area(renderer, kind, &origin), &key, &value) {
                Ok(()) => Outcome::Done,
                Err(_) => Outcome::QuotaExceeded,
            },
            Op::StorageRemove { kind, origin, key } => {
                self.local.storage().remove_item(kind, &area(renderer, kind, &origin), &key);
                Outcome::Done
            }
            Op::StorageClear { kind, origin } => {
                self.local.storage().clear(kind, &area(renderer, kind, &origin));
                Outcome::Done
            }
            Op::StorageLength { kind, origin } => Outcome::Count { value: self.local.storage().length(kind, &area(renderer, kind, &origin)) },
            Op::StorageKey { kind, origin, index } => Outcome::MaybeText { value: self.local.storage().key_at(kind, &area(renderer, kind, &origin), index) },
        }
    }
}

/// La clave del área de almacenamiento. `localStorage` es del origen, lo
/// vea quien lo vea; `sessionStorage` es además de la pestaña, y aquí cada
/// renderer es una. Nunca se persiste, así que el prefijo no llega a disco.
fn area(renderer: &str, kind: StorageKind, origin: &str) -> String {
    match kind {
        StorageKind::Local => origin.to_string(),
        StorageKind::Session => format!("{renderer}\n{origin}"),
    }
}

fn reply(outgoing: &mpsc::UnboundedSender<(Vec<u8>, Vec<u8>)>, id: u64, outcome: Outcome, body: Vec<u8>) {
    match encode_header(&Reply { id, outcome }) {
        Ok(header) => {
            let _ = outgoing.send((header, body));
        }
        Err(e) => {
            tracing::warn!("[broker] respuesta {id} imposible de enviar: {e}");
            if let Ok(header) = encode_header(&Reply { id, outcome: Outcome::Invalid { reason: e.to_string() } }) {
                let _ = outgoing.send((header, Vec::new()));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Canal de control
// ---------------------------------------------------------------------------

/// Atiende una línea del supervisor. Devuelve la respuesta y si hay que
/// terminar.
pub fn handle_control(server: &BrokerServer, line: &str) -> (Value, bool) {
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => return (json!({ "id": null, "type": "error", "message": format!("invalid_request: {e}") }), false),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let renderer = request.get("renderer").and_then(Value::as_str).filter(|name| !name.is_empty());
    match (request.get("type").and_then(Value::as_str), renderer) {
        (Some("register"), Some(renderer)) => match server.register(renderer) {
            Ok(token) => (json!({ "id": id, "type": "registered", "renderer": renderer, "token": token }), false),
            Err(e) => (json!({ "id": id, "type": "error", "message": e.to_string() }), false),
        },
        (Some("revoke"), Some(renderer)) => {
            server.revoke(renderer);
            (json!({ "id": id, "type": "revoked", "renderer": renderer }), false)
        }
        (Some("ping"), _) => (json!({ "id": id, "type": "pong" }), false),
        (Some("shutdown"), _) => (json!({ "id": id, "type": "bye" }), true),
        (Some("register" | "revoke"), None) => (json!({ "id": id, "type": "error", "message": "falta `renderer`" }), false),
        _ => (json!({ "id": id, "type": "error", "message": "tipo de mensaje desconocido" }), false),
    }
}

/// El proceso broker entero: abre el canal de renderers, anuncia dónde está
/// y atiende al supervisor hasta que este cierra stdin o pide `shutdown`.
pub async fn run_stdio(local: LocalBroker) -> io::Result<()> {
    let listener = Listener::bind()?;
    let endpoint = listener.endpoint().to_string();
    let server = BrokerServer::new(local);
    let serving = tokio::spawn(server.clone().serve(listener));

    let mut stdout = tokio::io::stdout();
    write_line(&mut stdout, &json!({ "id": null, "type": "ready", "endpoint": endpoint })).await?;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let (response, shutdown) = handle_control(&server, &line);
        write_line(&mut stdout, &response).await?;
        if shutdown {
            break;
        }
    }
    serving.abort();
    Ok(())
}

async fn write_line(stdout: &mut tokio::io::Stdout, value: &Value) -> io::Result<()> {
    let mut line = value.to_string();
    line.push('\n');
    stdout.write_all(line.as_bytes()).await?;
    stdout.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::ResourceBroker;
    use crate::broker_remote::RemoteBroker;

    /// Un broker en memoria sirviendo en su canal real, dentro del test.
    fn arrancar() -> (Arc<BrokerServer>, String) {
        let listener = Listener::bind().unwrap();
        let endpoint = listener.endpoint().to_string();
        let server = BrokerServer::new(LocalBroker::in_memory());
        tokio::spawn(server.clone().serve(listener));
        (server, endpoint)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_renderers_share_local_storage_and_cookies_but_not_session_storage() {
        let (server, endpoint) = arrancar();
        let a = RemoteBroker::connect(&endpoint, &server.register("tab-a").unwrap()).await.unwrap();
        let b = RemoteBroker::connect(&endpoint, &server.register("tab-b").unwrap()).await.unwrap();
        assert_eq!(a.renderer(), "tab-a");

        // Las llamadas síncronas bloquean el hilo: fuera del runtime.
        let (a, b) = tokio::task::spawn_blocking(move || {
            a.storage_set(StorageKind::Local, "https://a.test", "k", "v").unwrap();
            a.storage_set(StorageKind::Session, "https://a.test", "s", "1").unwrap();
            a.set_cookie_from_js("sesion=1; Path=/", "https://a.test/");
            assert_eq!(b.storage_get(StorageKind::Local, "https://a.test", "k").as_deref(), Some("v"));
            assert_eq!(b.cookie_header_for_js("https://a.test/otra"), "sesion=1");
            assert_eq!(b.storage_get(StorageKind::Session, "https://a.test", "s"), None, "sessionStorage es de cada pestaña");
            assert_eq!(a.storage_get(StorageKind::Session, "https://a.test", "s").as_deref(), Some("1"));
            assert_eq!(a.storage_length(StorageKind::Local, "https://a.test"), 1);
            assert_eq!(a.storage_key(StorageKind::Local, "https://a.test", 0).as_deref(), Some("k"));
            (a, b)
        })
        .await
        .unwrap();
        drop((a, b));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_token_opens_the_channel_once() {
        let (server, endpoint) = arrancar();
        let token = server.register("tab-a").unwrap();
        let _primero = RemoteBroker::connect(&endpoint, &token).await.unwrap();
        let error = RemoteBroker::connect(&endpoint, &token).await.err().expect("un token gastado no puede volver a abrir");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let error = RemoteBroker::connect(&endpoint, "inventado").await.err().expect("un token inventado tampoco");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_revoked_renderer_fails_fast_instead_of_hanging() {
        let (server, endpoint) = arrancar();
        let a = RemoteBroker::connect(&endpoint, &server.register("tab-a").unwrap()).await.unwrap();
        server.revoke("tab-a");
        let respuesta = tokio::task::spawn_blocking(move || {
            // Hasta que el lector ve el cierre puede pasar un instante; lo que
            // no puede pasar es que la llamada espere el plazo entero.
            let inicio = std::time::Instant::now();
            loop {
                let resultado = a.storage_set(StorageKind::Local, "https://a.test", "k", "v");
                if resultado.is_err() || inicio.elapsed() > Duration::from_secs(5) {
                    return (resultado, inicio.elapsed());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .await
        .unwrap();
        assert!(respuesta.0.is_err(), "tras retirarlo, el renderer ya no puede escribir");
        assert!(respuesta.1 < Duration::from_secs(5), "falla enseguida: {:?}", respuesta.1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn connecting_from_a_single_threaded_runtime_is_refused() {
        let error = RemoteBroker::connect("no-importa", "x").await.err().unwrap();
        assert!(error.to_string().contains("multihilo"));
    }

    #[test]
    fn the_control_channel_registers_and_rejects_nonsense() {
        let server = BrokerServer::new(LocalBroker::in_memory());
        let (respuesta, fin) = handle_control(&server, r#"{"id":1,"type":"register","renderer":"tab-1"}"#);
        assert_eq!(respuesta["type"], "registered");
        assert_eq!(respuesta["id"], 1);
        assert_eq!(respuesta["token"].as_str().unwrap().len(), 64);
        assert!(!fin);
        assert_eq!(handle_control(&server, r#"{"id":2,"type":"register"}"#).0["type"], "error");
        assert_eq!(handle_control(&server, "no es json").0["type"], "error");
        assert!(handle_control(&server, r#"{"id":3,"type":"shutdown"}"#).1);
    }
}
