//! Adaptador TreeSink: conecta el algoritmo de construccion de arbol de
//! html5ever (que implementa el spec HTML5 real, incluyendo recuperacion de
//! errores en HTML mal formado) con nuestros tipos `Node`/`NodeType`.
//!
//! Simplificaciones honestas, documentadas aqui para no repetirlas en cada
//! metodo:
//! - Namespaces: todo se trata como si fuera HTML (`ns!(html)`). Contenido
//!   foraneo bien formado (SVG/MathML embebido) no tendra namespace correcto.
//! - Doctype: se ignora (append_doctype_to_document no hace nada) - no hay
//!   representacion visual de un doctype.
//! - `<template>`: su contenido se trata como hijos normales del propio
//!   elemento en vez de quedar en un DocumentFragment inerte separado hasta
//!   que se clone via JS. No hay soporte de clonado via JS todavia de todas
//!   formas.
//! - Processing instructions (`<?xml ...?>`): representadas como comentarios,
//!   son irrelevantes para renderizado HTML normal.

use crate::node::{Node, NodeType};
use html5ever::interface::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::StrTendril;
use html5ever::{Attribute, LocalName, Namespace, QualName};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// `ElemName` exige devolver referencias (`&Namespace`, `&LocalName`) con la
/// misma vida que `&self`, lo que encaja mal con reconstruir un `QualName`
/// nuevo en cada llamada a `elem_name` (que produciria un temporal). Un tipo
/// propio con los campos ya resueltos evita el problema: los metodos del
/// trait simplemente devuelven referencias a sus propios campos.
#[derive(Debug)]
pub struct OwnedElemName {
    ns: Namespace,
    local: LocalName,
}

impl ElemName for OwnedElemName {
    fn ns(&self) -> &Namespace {
        &self.ns
    }
    fn local_name(&self) -> &LocalName {
        &self.local
    }
}

pub struct NodeSink {
    document: Arc<RwLock<Node>>,
}

impl NodeSink {
    pub fn new() -> Self {
        Self {
            document: Node::new(NodeType::Document),
        }
    }
}

impl TreeSink for NodeSink {
    type Handle = Arc<RwLock<Node>>;
    type Output = Arc<RwLock<Node>>;
    type ElemName<'a> = OwnedElemName;

    fn finish(self) -> Self::Output {
        self.document
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        // HTML5 esta deliberadamente diseñado para que el 99% de las paginas
        // reales disparen parse errors (spec de recuperacion de errores, no
        // de rechazo) - por eso es trace y no warn, para no ensuciar logs en
        // uso normal.
        tracing::trace!("[html5ever parse_error] {msg}");
    }

    fn get_document(&self) -> Self::Handle {
        self.document.clone()
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        let t = target.read().unwrap();
        let local = match &t.node_type {
            NodeType::Element { tag_name, .. } => tag_name.clone(),
            _ => String::new(),
        };
        OwnedElemName {
            ns: Namespace::from("http://www.w3.org/1999/xhtml"),
            local: LocalName::from(local),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, _flags: ElementFlags) -> Self::Handle {
        let mut attributes = HashMap::new();
        for attr in attrs {
            attributes.insert(attr.name.local.to_string(), attr.value.to_string());
        }
        Node::new(NodeType::Element {
            tag_name: name.local.to_string(),
            attributes,
        })
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        Node::new(NodeType::Comment(text.to_string()))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        Node::new(NodeType::Comment(format!("[pi {target}] {data}")))
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node) => {
                Node::append_child(parent, node);
            }
            NodeOrText::AppendText(text) => {
                append_text(parent, &text);
            }
        }
    }

    fn append_based_on_parent_node(&self, element: &Self::Handle, prev_element: &Self::Handle, child: NodeOrText<Self::Handle>) {
        let has_parent = element
            .read()
            .unwrap()
            .parent
            .as_ref()
            .and_then(|w| w.upgrade())
            .is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _name: StrTendril, _public_id: StrTendril, _system_id: StrTendril) {
        // Sin representacion visual, se ignora a proposito.
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        // Ver nota de modulo: el propio <template> hace de contenedor de su
        // contenido en vez de un DocumentFragment separado.
        target.clone()
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        Arc::ptr_eq(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        tracing::trace!("[html5ever] modo detectado: {mode:?} (no afecta al layout todavia)");
    }

    fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
        let parent = match sibling.read().unwrap().parent.as_ref().and_then(|w| w.upgrade()) {
            Some(p) => p,
            None => return,
        };

        let node_to_insert = match new_node {
            NodeOrText::AppendNode(n) => n,
            NodeOrText::AppendText(text) => Node::new(NodeType::Text(text.to_string())),
        };
        node_to_insert.write().unwrap().parent = Some(Arc::downgrade(&parent));

        let mut p = parent.write().unwrap();
        let idx = p.children.iter().position(|c| Arc::ptr_eq(c, sibling));
        match idx {
            Some(i) => p.children.insert(i, node_to_insert),
            None => p.children.push(node_to_insert),
        }
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let mut t = target.write().unwrap();
        if let NodeType::Element { attributes, .. } = &mut t.node_type {
            for attr in attrs {
                attributes.entry(attr.name.local.to_string()).or_insert_with(|| attr.value.to_string());
            }
        }
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        let parent = target.read().unwrap().parent.as_ref().and_then(|w| w.upgrade());
        if let Some(parent) = parent {
            parent.write().unwrap().children.retain(|c| !Arc::ptr_eq(c, target));
        }
        target.write().unwrap().parent = None;
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        let moved: Vec<_> = std::mem::take(&mut node.write().unwrap().children);
        for child in &moved {
            child.write().unwrap().parent = Some(Arc::downgrade(new_parent));
        }
        new_parent.write().unwrap().children.extend(moved);
    }
}

fn append_text(parent: &Arc<RwLock<Node>>, text: &str) {
    let merged = {
        let p = parent.read().unwrap();
        if let Some(last) = p.children.last() {
            let mut last_w = last.write().unwrap();
            if let NodeType::Text(existing) = &mut last_w.node_type {
                existing.push_str(text);
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if !merged {
        let text_node = Node::new(NodeType::Text(text.to_string()));
        Node::append_child(parent, text_node);
    }
}
