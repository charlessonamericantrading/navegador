use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub enum NodeType {
    Document,
    Element {
        tag_name: String,
        attributes: HashMap<String, String>,
    },
    Text(String),
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub node_type: NodeType,
    pub children: Vec<Arc<RwLock<Node>>>,
}

impl Node {
    pub fn new(node_type: NodeType) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            node_type,
            children: Vec::new(),
        }))
    }

    pub fn append_child(parent: &Arc<RwLock<Self>>, child: Arc<RwLock<Self>>) {
        parent.write().unwrap().children.push(child);
    }
}
