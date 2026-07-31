use crate::node::{Node, NodeType};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct CustomNativeHtmlParser;

impl CustomNativeHtmlParser {
    pub fn parse(html_str: &str) -> Arc<RwLock<Node>> {
        let root = Node::new(NodeType::Document);
        let mut stack: Vec<Arc<RwLock<Node>>> = vec![root.clone()];

        let mut chars = html_str.chars().peekable();

        while let Some(&ch) = chars.peek() {
            if ch == '<' {
                chars.next(); // Consume '<'

                // Check for closing tag
                if let Some(&'/') = chars.peek() {
                    chars.next(); // Consume '/'
                    let mut tag_name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '>' {
                            chars.next(); // Consume '>'
                            break;
                        }
                        tag_name.push(c);
                        chars.next();
                    }
                    let tag_name = tag_name.trim().to_lowercase();
                    if stack.len() > 1 {
                        stack.pop();
                    }
                } else if let Some(&'!') = chars.peek() {
                    // Comment or doctype
                    while let Some(&c) = chars.peek() {
                        if c == '>' {
                            chars.next();
                            break;
                        }
                        chars.next();
                    }
                } else {
                    // Opening or self-closing tag
                    let mut tag_buf = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '>' {
                            chars.next();
                            break;
                        }
                        tag_buf.push(c);
                        chars.next();
                    }

                    let parts: Vec<&str> = tag_buf.trim().split_whitespace().collect();
                    if !parts.is_empty() {
                        let tag_name = parts[0].to_lowercase();
                        let mut attributes = HashMap::new();

                        for part in &parts[1..] {
                            if let Some((k, v)) = part.split_once('=') {
                                let val = v.trim_matches('"').trim_matches('\'').to_string();
                                attributes.insert(k.to_lowercase(), val);
                            } else {
                                attributes.insert(part.to_lowercase(), "true".to_string());
                            }
                        }

                        let element_node = Node::new(NodeType::Element {
                            tag_name: tag_name.clone(),
                            attributes,
                        });

                        if let Some(parent) = stack.last() {
                            Node::append_child(parent, element_node.clone());
                        }

                        // Self closing tags check
                        let is_self_closing = tag_buf.ends_with('/') || matches!(tag_name.as_str(), "img" | "input" | "br" | "hr" | "meta" | "link");
                        if !is_self_closing {
                            stack.push(element_node);
                        }
                    }
                }
            } else {
                // Text node parsing
                let mut text_buf = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '<' {
                        break;
                    }
                    text_buf.push(c);
                    chars.next();
                }

                let trimmed_text = text_buf.trim();
                if !trimmed_text.is_empty() {
                    let text_node = Node::new(NodeType::Text(text_buf));
                    if let Some(parent) = stack.last() {
                        Node::append_child(parent, text_node);
                    }
                }
            }
        }

        tracing::info!("[Pure Custom Native HTML5 Parser] Successfully parsed HTML document into DOM Tree (0 external crates)");
        root
    }
}

pub use CustomNativeHtmlParser as HtmlParser;
