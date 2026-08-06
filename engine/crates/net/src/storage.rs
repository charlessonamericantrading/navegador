use std::collections::HashMap;

/// Mapa clave-valor en memoria (perdido al soltar el struct - NO hay
/// persistencia real a disco entre arranques pese al nombre `WebStorage`).
/// Nada lo instancia todavia: no esta conectado a `document`/`window` en
/// `js/src/dom_bindings.rs`, asi que `localStorage`/`sessionStorage` no
/// existen en JS por ahora.
#[derive(Debug, Clone, Default)]
pub struct WebStorage {
    local_storage: HashMap<String, String>,
}

impl WebStorage {
    pub fn new() -> Self {
        Self {
            local_storage: HashMap::new(),
        }
    }

    pub fn set_item(&mut self, key: &str, value: &str) {
        self.local_storage.insert(key.to_string(), value.to_string());
        tracing::info!("[WebStorage] Set item (solo en memoria, no persiste entre arranques): '{}' = '{}'", key, value);
    }

    pub fn get_item(&self, key: &str) -> Option<&String> {
        self.local_storage.get(key)
    }
}
