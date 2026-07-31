use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Rule {
    pub selector: String,
    pub declarations: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct StyleSheet {
    pub rules: Vec<Rule>,
}

impl StyleSheet {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }
}
