use std::collections::HashMap;

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
