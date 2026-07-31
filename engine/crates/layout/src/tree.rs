use crate::layout_box::{LayoutBox, BoxType, Rect};
use engine_dom::{Node, NodeType};
use engine_css::StyleSheet;
use std::sync::{Arc, RwLock};

pub struct LayoutTreeBuilder;

impl LayoutTreeBuilder {
    pub fn build(dom_root: &Arc<RwLock<Node>>, _stylesheet: &StyleSheet, viewport_width: f32, viewport_height: f32) -> LayoutBox {
        let mut root_box = LayoutBox::new(BoxType::Block);
        root_box.dimensions = Rect {
            x: 0.0,
            y: 0.0,
            width: viewport_width,
            height: viewport_height,
        };

        Self::build_node(dom_root, &mut root_box);
        root_box
    }

    fn build_node(dom_node: &Arc<RwLock<Node>>, parent_layout_box: &mut LayoutBox) {
        let r = dom_node.read().unwrap();
        match &r.node_type {
            NodeType::Document => {
                for child in &r.children {
                    Self::build_node(child, parent_layout_box);
                }
            }
            NodeType::Element { tag_name, .. } => {
                let box_type = match tag_name.as_str() {
                    "span" | "a" | "b" | "i" => BoxType::Inline,
                    _ => BoxType::Block,
                };
                let mut current_box = LayoutBox::new(box_type);
                for child in &r.children {
                    Self::build_node(child, &mut current_box);
                }
                parent_layout_box.children.push(current_box);
            }
            NodeType::Text(content) => {
                let text_box = LayoutBox::new(BoxType::Text(content.clone()));
                parent_layout_box.children.push(text_box);
            }
            _ => {}
        }
    }
}
