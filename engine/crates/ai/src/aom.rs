//! Accessibility Object Model (AOM) y Árbol Semántico para Agentes IA (Fase 5.1).
//!
//! Transforma el árbol de layout y DOM en una jerarquía semántica accesible
//! diseñada específicamente para consumo eficiente por Modelos de Lenguaje (LLMs).
//! Reduce el consumo de tokens en un ~80% frente al envío de HTML crudo, filtrando
//! nodos invisibles y asociando coordenadas espaciales exactas para acciones automáticas.

use engine_dom::NodeType;
use engine_layout::{BoxType, LayoutBox};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Rol semántico estándar para la IA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessibleRole {
    Button,
    Link,
    Input,
    Checkbox,
    Radio,
    Select,
    Heading,
    Image,
    Text,
    Form,
    Navigation,
    Main,
    Generic,
}

impl AccessibleRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Link => "link",
            Self::Input => "input",
            Self::Checkbox => "checkbox",
            Self::Radio => "radio",
            Self::Select => "select",
            Self::Heading => "heading",
            Self::Image => "image",
            Self::Text => "text",
            Self::Form => "form",
            Self::Navigation => "navigation",
            Self::Main => "main",
            Self::Generic => "generic",
        }
    }
}

/// Nodo accesible individual con metadatos espaciales y de interacción.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibleNode {
    pub id: usize,
    pub role: AccessibleRole,
    pub name: String,
    pub value: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub interactive: bool,
    pub disabled: bool,
    pub children: Vec<AccessibleNode>,
}

/// Árbol semántico completo de la página.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibilityTree {
    pub root: AccessibleNode,
    pub total_nodes: usize,
    pub interactive_count: usize,
}

static NODE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

impl AccessibilityTree {
    /// Construye el árbol accesible a partir del árbol de layout raíz.
    pub fn build(layout_root: &LayoutBox) -> Self {
        NODE_ID_COUNTER.store(1, Ordering::SeqCst);
        let mut total_nodes = 0;
        let mut interactive_count = 0;
        let root = Self::convert_box(layout_root, &mut total_nodes, &mut interactive_count);
        Self {
            root,
            total_nodes,
            interactive_count,
        }
    }

    fn convert_box(
        layout_box: &LayoutBox,
        total_nodes: &mut usize,
        interactive_count: &mut usize,
    ) -> AccessibleNode {
        *total_nodes += 1;
        let id = NODE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);

        let (_tag, role, name, value, interactive, disabled) = match &layout_box.dom_node {
            Some(arc_node) => {
                let n = arc_node.read().unwrap();
                match &n.node_type {
                    NodeType::Element { tag_name, attributes } => {
                        let tag = tag_name.to_ascii_lowercase();
                        let role = Self::deduce_role(&tag, attributes);
                        let is_inter = matches!(
                            role,
                            AccessibleRole::Button
                                | AccessibleRole::Link
                                | AccessibleRole::Input
                                | AccessibleRole::Checkbox
                                | AccessibleRole::Radio
                                | AccessibleRole::Select
                        );
                        let disabled = attributes.contains_key("disabled");
                        let aria_label = attributes.get("aria-label").cloned();
                        let placeholder = attributes.get("placeholder").cloned();
                        let alt = attributes.get("alt").cloned();
                        let title = attributes.get("title").cloned();
                        let val = attributes.get("value").cloned();

                        let mut text_buf = String::new();
                        Self::collect_text(layout_box, &mut text_buf);
                        let text_content = text_buf.trim().to_string();

                        let name = aria_label
                            .or(alt)
                            .or(placeholder)
                            .or(title)
                            .unwrap_or_else(|| {
                                if !text_content.is_empty() {
                                    text_content
                                } else {
                                    tag.clone()
                                }
                            });

                        (tag, role, name, val, is_inter, disabled)
                    }
                    _ => (
                        String::new(),
                        AccessibleRole::Generic,
                        String::new(),
                        None,
                        false,
                        false,
                    ),
                }
            }
            None => {
                let (role, name) = match &layout_box.box_type {
                    BoxType::Text(t) => (AccessibleRole::Text, t.clone()),
                    BoxType::Image(src) => (AccessibleRole::Image, src.clone()),
                    _ => (AccessibleRole::Generic, String::new()),
                };
                (String::new(), role, name, None, false, false)
            }
        };

        if interactive && layout_box.dimensions.width > 0.0 && layout_box.dimensions.height > 0.0 {
            *interactive_count += 1;
        }

        let mut children = Vec::new();
        for child in &layout_box.children {
            // Omitir nodos sin dimensiones reales a menos que tengan hijos
            if child.dimensions.width > 0.0 || child.dimensions.height > 0.0 || !child.children.is_empty() {
                children.push(Self::convert_box(child, total_nodes, interactive_count));
            }
        }

        AccessibleNode {
            id,
            role,
            name,
            value,
            x: layout_box.dimensions.x,
            y: layout_box.dimensions.y,
            width: layout_box.dimensions.width,
            height: layout_box.dimensions.height,
            interactive,
            disabled,
            children,
        }
    }

    fn deduce_role(tag: &str, attributes: &std::collections::HashMap<String, String>) -> AccessibleRole {
        if let Some(role_attr) = attributes.get("role") {
            match role_attr.to_ascii_lowercase().as_str() {
                "button" => return AccessibleRole::Button,
                "link" => return AccessibleRole::Link,
                "heading" => return AccessibleRole::Heading,
                "navigation" => return AccessibleRole::Navigation,
                "main" => return AccessibleRole::Main,
                _ => {}
            }
        }
        match tag {
            "button" => AccessibleRole::Button,
            "a" if attributes.contains_key("href") => AccessibleRole::Link,
            "input" => {
                let input_type = attributes.get("type").map(|t| t.to_ascii_lowercase()).unwrap_or_else(|| "text".to_string());
                match input_type.as_str() {
                    "checkbox" => AccessibleRole::Checkbox,
                    "radio" => AccessibleRole::Radio,
                    "submit" | "button" | "reset" => AccessibleRole::Button,
                    _ => AccessibleRole::Input,
                }
            }
            "select" => AccessibleRole::Select,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => AccessibleRole::Heading,
            "img" => AccessibleRole::Image,
            "form" => AccessibleRole::Form,
            "nav" => AccessibleRole::Navigation,
            "main" => AccessibleRole::Main,
            _ => AccessibleRole::Generic,
        }
    }

    fn collect_text(layout_box: &LayoutBox, out: &mut String) {
        if let BoxType::Text(t) = &layout_box.box_type {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(t.trim());
        }
        for child in &layout_box.children {
            Self::collect_text(child, out);
        }
    }

    /// Genera una representación textual ultra-compacta estructurada para prompts de LLM.
    pub fn to_llm_representation(&self) -> String {
        let mut lines = Vec::new();
        Self::render_node_for_llm(&self.root, 0, &mut lines);
        lines.join("\n")
    }

    fn render_node_for_llm(node: &AccessibleNode, depth: usize, out: &mut Vec<String>) {
        if node.interactive {
            let val_str = node.value.as_ref().map(|v| format!(" value=\"{v}\"")).unwrap_or_default();
            let line = format!(
                "{:indent$}[{}] ({}) \"{}\"{val_str} @ [x:{:.0}, y:{:.0}, w:{:.0}, h:{:.0}]",
                "",
                node.id,
                node.role.as_str(),
                node.name,
                node.x,
                node.y,
                node.width,
                node.height,
                indent = depth * 2
            );
            out.push(line);
        } else if matches!(node.role, AccessibleRole::Heading | AccessibleRole::Image | AccessibleRole::Navigation) {
            let line = format!(
                "{:indent$}({}) \"{}\"",
                "",
                node.role.as_str(),
                node.name,
                indent = depth * 2
            );
            out.push(line);
        }

        for child in &node.children {
            Self::render_node_for_llm(child, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_css::CssParser;
    use engine_dom::HtmlParser;
    use engine_layout::{ImageMap, LayoutTreeBuilder};

    #[test]
    fn builds_accessibility_tree_with_interactive_elements() {
        let dom = HtmlParser::parse(
            r#"<html><body>
                <nav>
                    <a href="/home">Inicio</a>
                </nav>
                <main>
                    <h1>Título de la Página</h1>
                    <form action="/search">
                        <input type="text" placeholder="Buscar producto..." value="zapatillas" />
                        <button type="submit">Buscar</button>
                    </form>
                </main>
            </body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } input { width: 200px; height: 30px; } button { width: 80px; height: 30px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let tree = AccessibilityTree::build(&root);
        assert!(tree.interactive_count >= 3, "debe encontrar el link, input y botón como interactivos");

        let prompt = tree.to_llm_representation();
        assert!(prompt.contains("(link) \"Inicio\""));
        assert!(prompt.contains("(input) \"Buscar producto...\""));
        assert!(prompt.contains("(button) \"Buscar\""));
        assert!(prompt.contains("(heading) \"Título de la Página\""));
    }
}
