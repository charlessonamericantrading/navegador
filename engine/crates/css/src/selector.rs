use engine_dom::{Node, NodeType};
use std::sync::{Arc, RwLock};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default)]
pub struct Specificity {
    pub id_count: u32,
    pub class_count: u32,
    pub tag_count: u32,
}

pub struct SelectorMatcher;

impl SelectorMatcher {
    pub fn calculate_specificity(selector_str: &str) -> Specificity {
        let mut spec = Specificity::default();
        for token in selector_str.split_whitespace() {
            if token.starts_with('#') {
                spec.id_count += 1;
            } else if token.starts_with('.') {
                spec.class_count += 1;
            } else if !token.is_empty() && token != "*" {
                spec.tag_count += 1;
            }
        }
        spec
    }

    pub fn matches(selector_str: &str, node: &Arc<RwLock<Node>>) -> bool {
        let r = node.read().unwrap();
        match &r.node_type {
            NodeType::Element { tag_name, attributes } => {
                let trimmed = selector_str.trim();
                if trimmed == "*" {
                    return true;
                }
                if trimmed == tag_name {
                    return true;
                }
                if let Some(class_attr) = attributes.get("class") {
                    if trimmed.starts_with('.') && class_attr.split_whitespace().any(|c| c == &trimmed[1..]) {
                        return true;
                    }
                }
                if let Some(id_attr) = attributes.get("id") {
                    if trimmed.starts_with('#') && &trimmed[1..] == id_attr {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }
}
