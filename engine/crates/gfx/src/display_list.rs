use engine_layout::{LayoutBox, BoxType, Rect};

#[derive(Debug, Clone)]
pub enum DisplayItem {
    SolidRect { rect: Rect, color: [u8; 4] },
    Text { rect: Rect, text: String, color: [u8; 4] },
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    pub fn build(layout_root: &LayoutBox) -> Self {
        let mut list = Self::default();
        Self::build_items(layout_root, &mut list);
        list
    }

    fn build_items(layout_box: &LayoutBox, list: &mut DisplayList) {
        match &layout_box.box_type {
            BoxType::Block | BoxType::Inline => {
                list.items.push(DisplayItem::SolidRect {
                    rect: layout_box.dimensions.clone(),
                    color: [255, 255, 255, 255],
                });
            }
            BoxType::Text(content) => {
                list.items.push(DisplayItem::Text {
                    rect: layout_box.dimensions.clone(),
                    text: content.clone(),
                    color: [0, 0, 0, 255],
                });
            }
        }

        for child in &layout_box.children {
            Self::build_items(child, list);
        }
    }
}
