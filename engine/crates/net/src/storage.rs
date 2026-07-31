use std::collections::HashMap;

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
        tracing::info!("[WebStorage Persist] Set item: '{}' = '{}'", key, value);
    }

    pub fn get_item(&self, key: &str) -> Option<&String> {
        self.local_storage.get(key)
    }
}
