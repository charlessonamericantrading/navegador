use crate::stylesheet::{StyleSheet, Rule};
use std::collections::HashMap;

pub struct CustomNativeCssParser;

impl CustomNativeCssParser {
    pub fn parse(css_str: &str) -> StyleSheet {
        let mut stylesheet = StyleSheet::new();

        let clean_css = css_str.replace("/*", "\0/*").replace("*/", "*/\0");
        let sans_comments: String = clean_css.split('\0')
            .filter(|part| !part.starts_with("/*"))
            .collect();

        for block in sans_comments.split('}') {
            let trimmed = block.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some((selector_part, declarations_part)) = trimmed.split_once('{') {
                let selector = selector_part.trim().to_string();
                let mut declarations = HashMap::new();

                for statement in declarations_part.split(';') {
                    let stmt_trimmed = statement.trim();
                    if stmt_trimmed.is_empty() {
                        continue;
                    }

                    if let Some((prop, val)) = stmt_trimmed.split_once(':') {
                        declarations.insert(prop.trim().to_lowercase(), val.trim().to_string());
                    }
                }

                stylesheet.rules.push(Rule {
                    selector,
                    declarations,
                });
            }
        }

        tracing::info!("[Pure Custom Native CSS3 Lexer] Parsed {} CSS rules (0 external parser crates)", stylesheet.rules.len());
        stylesheet
    }
}

pub use CustomNativeCssParser as CssParser;
