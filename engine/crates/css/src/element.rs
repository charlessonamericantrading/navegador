//! Adaptador `selectors::Element`: conecta el arbol de matching generico del
//! crate `selectors` (el mismo que usa Firefox/Stylo) con nuestro `Node`.
//!
//! El trait `Element` exige ~26 metodos. Bastantes de los nuestros son `false`
//! / `None` honestos, no fingidos: no hay shadow DOM, ni partes (`::part`),
//! ni estados personalizados, ni pseudo-elementos - decir que no existen es
//! la respuesta correcta, no una simplificacion que esconde algo roto (ver
//! ARCHITECTURE.md, regla "si una funcion no esta implementada, no existe").
//!
//! No se implementa `selectors::Element` directamente sobre `Arc<RwLock<Node>>`
//! porque ni el trait (de `selectors`) ni el tipo (de `engine_dom`, envuelto
//! en `Arc`/`RwLock` de std) son locales a este crate - la regla de huerfanos
//! de Rust lo rechaza. `ElementRef` es un newtype local que sortea eso.

use crate::selector::{CssString, EngineSelectorImpl, NoNamespace, NoPseudoClass, NoPseudoElement};
use engine_dom::{Node, NodeType};
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::MatchingContext;
use selectors::matching::ElementSelectorFlags;
use selectors::{Element, OpaqueElement};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub(crate) struct ElementRef(pub(crate) Arc<RwLock<Node>>);

fn is_element(node: &Arc<RwLock<Node>>) -> bool {
    matches!(node.read().unwrap().node_type, NodeType::Element { .. })
}

impl Element for ElementRef {
    type Impl = EngineSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // Identidad estable por nodo: la direccion de la asignacion
        // compartida del Arc, no la del wrapper local (que cambia en cada
        // clon de ElementRef aunque apunten al mismo Node).
        let ptr = Arc::as_ptr(&self.0) as *mut ();
        let non_null = std::ptr::NonNull::new(ptr).expect("Arc::as_ptr nunca es null");
        OpaqueElement::from_non_null_ptr(non_null)
    }

    fn parent_element(&self) -> Option<Self> {
        let parent = self.0.read().unwrap().parent.as_ref()?.upgrade()?;
        is_element(&parent).then(|| ElementRef(parent))
    }

    // Sin shadow DOM todavia: ni slots, ni shadow root, ni hosts.
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn is_pseudo_element(&self) -> bool {
        // NoPseudoElement es un enum vacio: nunca se construye un ElementRef
        // "pseudo" de verdad.
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let parent = self.0.read().unwrap().parent.as_ref()?.upgrade()?;
        let parent = parent.read().unwrap();
        let idx = parent.children.iter().position(|c| Arc::ptr_eq(c, &self.0))?;
        parent.children[..idx].iter().rev().find(|c| is_element(c)).map(|c| ElementRef(c.clone()))
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let parent = self.0.read().unwrap().parent.as_ref()?.upgrade()?;
        let parent = parent.read().unwrap();
        let idx = parent.children.iter().position(|c| Arc::ptr_eq(c, &self.0))?;
        parent.children[idx + 1..].iter().find(|c| is_element(c)).map(|c| ElementRef(c.clone()))
    }

    fn first_element_child(&self) -> Option<Self> {
        self.0.read().unwrap().children.iter().find(|c| is_element(c)).map(|c| ElementRef(c.clone()))
    }

    fn is_html_element_in_html_document(&self) -> bool {
        // Sin soporte de documentos XML: todo lo que parseamos es HTML.
        true
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        match &self.0.read().unwrap().node_type {
            NodeType::Element { tag_name, .. } => tag_name == local_name,
            _ => false,
        }
    }

    fn has_namespace(&self, _ns: &NoNamespace) -> bool {
        // Un unico namespace implicito (HTML) para todo - ver
        // html5ever_sink.rs. Cualquier elemento "esta" en el.
        true
    }

    fn is_same_type(&self, other: &Self) -> bool {
        match (&self.0.read().unwrap().node_type, &other.0.read().unwrap().node_type) {
            (NodeType::Element { tag_name: a, .. }, NodeType::Element { tag_name: b, .. }) => a == b,
            _ => false,
        }
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&NoNamespace>,
        local_name: &CssString,
        operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        let node = self.0.read().unwrap();
        let NodeType::Element { attributes, .. } = &node.node_type else { return false };
        let Some(actual) = attributes.get(local_name.as_str()) else { return false };
        match operation {
            AttrSelectorOperation::Exists => true,
            // El propio crate implementa la semantica de cada operador
            // (=, ~=, |=, ^=, $=, *=) correctamente - no hay razon para
            // reimplementarla peor a mano.
            AttrSelectorOperation::WithValue { operator, case_sensitivity, value } => {
                operator.eval_str(actual, value.as_str(), *case_sensitivity)
            }
        }
    }

    fn match_non_ts_pseudo_class(&self, pc: &NoPseudoClass, _context: &mut MatchingContext<Self::Impl>) -> bool {
        match *pc {}
    }

    fn match_pseudo_element(&self, pe: &NoPseudoElement, _context: &mut MatchingContext<Self::Impl>) -> bool {
        match *pe {}
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // Flags de optimizacion para invalidacion incremental de estilos -
        // no aplica: no hay recalculo tras interaccion todavia (Fase 3).
    }

    fn is_link(&self) -> bool {
        match &self.0.read().unwrap().node_type {
            NodeType::Element { tag_name, attributes } => {
                (tag_name == "a" || tag_name == "area") && attributes.contains_key("href")
            }
            _ => false,
        }
    }

    fn assigned_slot(&self) -> Option<Self> {
        None
    }

    fn has_id(&self, id: &CssString, case_sensitivity: CaseSensitivity) -> bool {
        match &self.0.read().unwrap().node_type {
            NodeType::Element { attributes, .. } => attributes
                .get("id")
                .map(|actual| case_sensitivity.eq(actual.as_bytes(), id.as_str().as_bytes()))
                .unwrap_or(false),
            _ => false,
        }
    }

    fn has_class(&self, name: &CssString, case_sensitivity: CaseSensitivity) -> bool {
        match &self.0.read().unwrap().node_type {
            NodeType::Element { attributes, .. } => attributes
                .get("class")
                .map(|classes| classes.split_whitespace().any(|c| case_sensitivity.eq(c.as_bytes(), name.as_str().as_bytes())))
                .unwrap_or(false),
            _ => false,
        }
    }

    // Estados personalizados (`:state(...)`) y partes (`::part`/`exportparts`)
    // son API de Custom Elements / shadow DOM, ninguna soportada todavia.
    fn has_custom_state(&self, _name: &CssString) -> bool {
        false
    }
    fn imported_part(&self, _name: &CssString) -> Option<CssString> {
        None
    }
    fn is_part(&self, _name: &CssString) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.0.read().unwrap().children.iter().all(|c| {
            match &c.read().unwrap().node_type {
                NodeType::Element { .. } => false,
                NodeType::Text(text) => text.trim().is_empty(),
                _ => true,
            }
        })
    }

    fn is_root(&self) -> bool {
        // Sin padre-elemento (el padre es el Document o no existe) = es la
        // raiz. `<html>` cuelga del Document, no de otro elemento.
        self.parent_element().is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        // MatchingContext siempre se construye con bloom_filter: None (ver
        // selector.rs) - esto es una aceleracion opcional que no usamos
        // todavia, no una cuestion de correccion.
        false
    }
}
