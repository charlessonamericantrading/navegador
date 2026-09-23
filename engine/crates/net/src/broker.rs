//! La frontera de capacidades del renderer (ADR 0001, etapa 2).
//!
//! Todo lo que el proceso que interpreta la página necesita del exterior —red,
//! cookies desde JavaScript y almacenamiento web— pasa por `ResourceBroker`.
//! Son nueve operaciones y nada más: el renderer no abre sockets ni ficheros
//! por su cuenta, pide capacidades concretas.
//!
//! Hoy la única implementación es `LocalBroker`, que hace el trabajo en el
//! mismo proceso con el `NetworkEngine` y el `WebStorage` de siempre: el
//! comportamiento no cambia. Lo que cambia es que el motor ya no depende de
//! esas piezas, sino de esta interfaz, y un broker en otro proceso (el paso
//! siguiente de la ADR) se conecta implementándola, sin tocar el motor. Solo
//! con la red y el disco fuera del renderer se puede restringir su token.
//!
//! Qué NO es todavía: una frontera de seguridad. Con `LocalBroker` todo sigue
//! en el mismo proceso.

use crate::http_client::{NetworkEngine, NetworkError};
use crate::request::NetworkRequest;
use crate::response::NetworkResponse;
use crate::storage::{QuotaExceeded, StorageKind, WebStorage};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// Futuro de una operación del broker. Sin `Send`: el renderer ejecuta su
/// JavaScript en un solo hilo y espera las respuestas en ese mismo hilo.
pub type BrokerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Lo que el renderer puede pedir. Ver el doc-comment del módulo.
pub trait ResourceBroker: Send + Sync {
    /// Petición HTTP(S) completa, con cookies, CORS y redirecciones aplicadas
    /// por quien la ejecuta.
    fn fetch<'a>(&'a self, request: &'a NetworkRequest) -> BrokerFuture<'a, Result<NetworkResponse, NetworkError>>;

    /// `document.cookie` (lectura): solo cookies visibles para JS.
    fn cookie_header_for_js(&self, page_url: &str) -> String;
    /// `document.cookie = ...`: nunca puede crear cookies `HttpOnly`.
    fn set_cookie_from_js(&self, raw: &str, page_url: &str);

    // Web Storage, acotado siempre a un origen.
    fn storage_get(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String>;
    fn storage_set(&self, kind: StorageKind, origin: &str, key: &str, value: &str) -> Result<(), QuotaExceeded>;
    fn storage_remove(&self, kind: StorageKind, origin: &str, key: &str);
    fn storage_clear(&self, kind: StorageKind, origin: &str);
    fn storage_length(&self, kind: StorageKind, origin: &str) -> usize;
    fn storage_key(&self, kind: StorageKind, origin: &str, index: usize) -> Option<String>;
}

/// Broker compartido por todo lo que corre en un renderer.
pub type SharedBroker = Arc<dyn ResourceBroker>;

/// Broker en el mismo proceso: la red y el disco de siempre, detrás de la
/// interfaz nueva.
pub struct LocalBroker {
    pub(crate) network: NetworkEngine,
    storage: Mutex<WebStorage>,
}

impl LocalBroker {
    /// Sin persistencia: cookies y almacenamiento solo en memoria. Es lo que
    /// usan los tests.
    pub fn in_memory() -> Self {
        Self { network: NetworkEngine::new(), storage: Mutex::new(WebStorage::new()) }
    }

    /// El del producto: cookies y `localStorage` del perfil en disco.
    pub fn persistent() -> Self {
        Self { network: NetworkEngine::with_persistent_cookies(), storage: Mutex::new(WebStorage::load_from_disk()) }
    }

    /// Envuelto en `Arc<dyn ResourceBroker>`, que es como lo recibe el motor.
    pub fn shared(self) -> SharedBroker {
        Arc::new(self)
    }

    pub(crate) fn storage(&self) -> std::sync::MutexGuard<'_, WebStorage> {
        // Un pánico con el candado cogido deja el almacén envenenado; los
        // datos siguen siendo válidos (cada operación es atómica), así que se
        // recupera en vez de propagar el pánico a cada lectura posterior.
        self.storage.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ResourceBroker for LocalBroker {
    fn fetch<'a>(&'a self, request: &'a NetworkRequest) -> BrokerFuture<'a, Result<NetworkResponse, NetworkError>> {
        Box::pin(self.network.fetch(request))
    }

    fn cookie_header_for_js(&self, page_url: &str) -> String {
        self.network.cookie_header_for_js(page_url)
    }

    fn set_cookie_from_js(&self, raw: &str, page_url: &str) {
        self.network.set_cookie_from_js(raw, page_url);
    }

    fn storage_get(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String> {
        self.storage().get_item(kind, origin, key)
    }

    fn storage_set(&self, kind: StorageKind, origin: &str, key: &str, value: &str) -> Result<(), QuotaExceeded> {
        self.storage().set_item(kind, origin, key, value)
    }

    fn storage_remove(&self, kind: StorageKind, origin: &str, key: &str) {
        self.storage().remove_item(kind, origin, key);
    }

    fn storage_clear(&self, kind: StorageKind, origin: &str) {
        self.storage().clear(kind, origin);
    }

    fn storage_length(&self, kind: StorageKind, origin: &str) -> usize {
        self.storage().length(kind, origin)
    }

    fn storage_key(&self, kind: StorageKind, origin: &str, index: usize) -> Option<String> {
        self.storage().key_at(kind, origin, index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La interfaz respeta el aislamiento por origen del almacén de debajo.
    #[test]
    fn storage_through_the_broker_stays_per_origin() {
        let broker = LocalBroker::in_memory().shared();
        broker.storage_set(StorageKind::Local, "https://a.test", "k", "v").unwrap();
        assert_eq!(broker.storage_get(StorageKind::Local, "https://a.test", "k").as_deref(), Some("v"));
        assert_eq!(broker.storage_get(StorageKind::Local, "https://b.test", "k"), None);
        assert_eq!(broker.storage_length(StorageKind::Local, "https://a.test"), 1);
        assert_eq!(broker.storage_key(StorageKind::Local, "https://a.test", 0).as_deref(), Some("k"));
        broker.storage_remove(StorageKind::Local, "https://a.test", "k");
        assert_eq!(broker.storage_length(StorageKind::Local, "https://a.test"), 0);
    }

    #[test]
    fn js_cookies_through_the_broker_round_trip() {
        let broker = LocalBroker::in_memory().shared();
        broker.set_cookie_from_js("sesion=1; Path=/", "https://a.test/pagina");
        assert_eq!(broker.cookie_header_for_js("https://a.test/otra"), "sesion=1");
        assert_eq!(broker.cookie_header_for_js("https://b.test/"), "");
    }
}
