use std::collections::HashMap;
use std::sync::{Arc, RwLock, Weak};

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
    /// Weak para no crear un ciclo de referencias con `children` (Arc fuerte
    /// en ambas direcciones nunca se liberaria). Necesario para que el
    /// adaptador de html5ever (TreeSink) pueda implementar remove_from_parent,
    /// append_before_sibling y reparent_children sin recorrer el arbol entero
    /// desde la raiz cada vez.
    pub parent: Option<Weak<RwLock<Node>>>,
}

impl Node {
    pub fn new(node_type: NodeType) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            node_type,
            children: Vec::new(),
            parent: None,
        }))
    }

    pub fn append_child(parent: &Arc<RwLock<Self>>, child: Arc<RwLock<Self>>) {
        child.write().unwrap().parent = Some(Arc::downgrade(parent));
        parent.write().unwrap().children.push(child);
    }

    /// Todos los elementos con esta etiqueta en el subarbol de `root`
    /// (incluido `root` mismo), en orden de documento. Equivalente honesto-
    /// minimo de `getElementsByTagName`: sin namespaces, sin live collection.
    pub fn find_all_by_tag(root: &Arc<RwLock<Self>>, tag: &str) -> Vec<Arc<RwLock<Self>>> {
        let mut results = Vec::new();
        Self::collect_by_tag(root, tag, &mut results);
        results
    }

    fn collect_by_tag(node: &Arc<RwLock<Self>>, tag: &str, out: &mut Vec<Arc<RwLock<Self>>>) {
        let n = node.read().unwrap();
        if let NodeType::Element { tag_name, .. } = &n.node_type {
            if tag_name == tag {
                out.push(node.clone());
            }
        }
        for child in &n.children {
            Self::collect_by_tag(child, tag, out);
        }
    }

    /// El primer elemento (en orden de documento, incluida la raiz) cuyo
    /// atributo `id` sea exactamente `id`. Equivalente honesto-minimo de
    /// `getElementById`: no valida que los ids sean unicos en el documento
    /// (el parser tampoco lo exige), simplemente devuelve el primero que
    /// encuentra - igual que hace un navegador real ante HTML con ids
    /// duplicados.
    pub fn find_by_id(root: &Arc<RwLock<Self>>, id: &str) -> Option<Arc<RwLock<Self>>> {
        let n = root.read().unwrap();
        if let NodeType::Element { attributes, .. } = &n.node_type {
            if attributes.get("id").map(String::as_str) == Some(id) {
                return Some(root.clone());
            }
        }
        for child in &n.children {
            if let Some(found) = Self::find_by_id(child, id) {
                return Some(found);
            }
        }
        None
    }

    /// Concatena todo el texto de los nodos de texto descendientes, en orden
    /// de documento - equivalente honesto-minimo de `textContent` (sin
    /// distinguir `<script>`/`<style>` como casos especiales; quien llame
    /// decide si le importa el tag antes de pedir su texto).
    pub fn text_content(node: &Arc<RwLock<Self>>) -> String {
        let n = node.read().unwrap();
        match &n.node_type {
            NodeType::Text(text) => text.clone(),
            _ => n.children.iter().map(Self::text_content).collect::<Vec<_>>().join(""),
        }
    }

    /// El "elemento documento": el UNICO hijo `Element` directo de `root`
    /// (normalmente `<html>`) - equivalente honesto-minimo de
    /// `document.documentElement`. Busca solo entre los hijos DIRECTOS de
    /// `root`, no en todo el subarbol - igual que el spec real, donde
    /// `documentElement` es especificamente el hijo raiz del documento, no
    /// "el primer elemento a cualquier profundidad" (eso ya lo hace
    /// `find_all_by_tag`/`find_by_id` para otros casos).
    pub fn document_element(root: &Arc<RwLock<Self>>) -> Option<Arc<RwLock<Self>>> {
        let n = root.read().unwrap();
        n.children.iter().find(|child| matches!(child.read().unwrap().node_type, NodeType::Element { .. })).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(tag: &str) -> Arc<RwLock<Node>> {
        Node::new(NodeType::Element { tag_name: tag.to_string(), attributes: HashMap::new() })
    }

    fn text(content: &str) -> Arc<RwLock<Node>> {
        Node::new(NodeType::Text(content.to_string()))
    }

    #[test]
    fn find_all_by_tag_finds_nested_matches_in_document_order() {
        let root = element("html");
        let body = element("body");
        let script_a = element("script");
        let div = element("div");
        let script_b = element("script");

        Node::append_child(&div, script_b.clone());
        Node::append_child(&body, script_a.clone());
        Node::append_child(&body, div.clone());
        Node::append_child(&root, body);

        let found = Node::find_all_by_tag(&root, "script");
        assert_eq!(found.len(), 2);
        assert!(Arc::ptr_eq(&found[0], &script_a));
        assert!(Arc::ptr_eq(&found[1], &script_b), "el <script> anidado dentro de <div> tambien deberia encontrarse");
    }

    #[test]
    fn find_all_by_tag_returns_empty_when_nothing_matches() {
        let root = element("html");
        assert!(Node::find_all_by_tag(&root, "script").is_empty());
    }

    #[test]
    fn find_by_id_finds_a_nested_element_by_exact_id_match() {
        let root = element("html");
        let body = element("body");
        let div = element("div");
        let mut attrs = HashMap::new();
        attrs.insert("id".to_string(), "deep".to_string());
        let target = Node::new(NodeType::Element { tag_name: "span".to_string(), attributes: attrs });

        Node::append_child(&div, target.clone());
        Node::append_child(&body, div);
        Node::append_child(&root, body);

        let found = Node::find_by_id(&root, "deep");
        assert!(matches!(&found, Some(n) if Arc::ptr_eq(n, &target)));
    }

    #[test]
    fn find_by_id_returns_none_when_no_element_has_that_id() {
        let root = element("html");
        assert!(Node::find_by_id(&root, "no-existe").is_none());
    }

    #[test]
    fn text_content_concatenates_nested_text_nodes_in_order() {
        let p = element("p");
        Node::append_child(&p, text("hola "));
        let b = element("b");
        Node::append_child(&b, text("mundo"));
        Node::append_child(&p, b);
        Node::append_child(&p, text("!"));

        assert_eq!(Node::text_content(&p), "hola mundo!");
    }

    #[test]
    fn document_element_returns_the_root_element_child() {
        let root = Node::new(NodeType::Document);
        let html = element("html");
        Node::append_child(&root, html.clone());

        let found = Node::document_element(&root).expect("deberia encontrar el elemento raiz");
        assert!(Arc::ptr_eq(&found, &html));
    }

    #[test]
    fn document_element_ignores_text_node_siblings_and_finds_the_element() {
        // Un documento real puede tener texto/whitespace suelto antes del
        // elemento raiz (p.ej. un doctype mal formado) - documentElement
        // deberia seguir encontrando el Element, no confundirse con eso.
        let root = Node::new(NodeType::Document);
        Node::append_child(&root, text("\n"));
        let html = element("html");
        Node::append_child(&root, html.clone());

        let found = Node::document_element(&root).expect("deberia encontrar el elemento pese al texto suelto antes");
        assert!(Arc::ptr_eq(&found, &html));
    }

    #[test]
    fn document_element_is_none_when_there_is_no_element_child() {
        let root = Node::new(NodeType::Document);
        Node::append_child(&root, text("solo texto, sin ningun elemento"));
        assert!(Node::document_element(&root).is_none());
    }
}
