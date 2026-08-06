use std::collections::HashMap;

/// Mapa clave-valor en memoria, sin ninguna semantica real de cookie HTTP:
/// no parsea la sintaxis de la cabecera `Set-Cookie` (`Domain=`/`Path=`/
/// `Expires=`/`Max-Age=`/`Secure`/`HttpOnly`/`SameSite`), no aplica scoping
/// por dominio/ruta, y no expira nada. Nada lo instancia todavia: no esta
/// conectado a `NetworkEngine::fetch` (`http_client.rs`), asi que ninguna
/// peticion real envia ni recibe cookies via este tipo por ahora.
#[derive(Debug, Clone, Default)]
pub struct CookieStore {
    cookies: HashMap<String, String>,
}

impl CookieStore {
    pub fn new() -> Self {
        Self {
            cookies: HashMap::new(),
        }
    }

    pub fn set_cookie(&mut self, name: String, value: String) {
        self.cookies.insert(name, value);
    }

    pub fn get_cookie(&self, name: &str) -> Option<&String> {
        self.cookies.get(name)
    }
}
