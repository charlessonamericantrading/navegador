use crate::layout_box::LayoutBox;
use rayon::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    Center,
    FlexEnd,
    SpaceBetween,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    FlexStart,
    Center,
    FlexEnd,
    Stretch,
}

pub struct FlexLayoutEngine;

impl FlexLayoutEngine {
    pub fn layout_flex(
        container: &mut LayoutBox,
        direction: FlexDirection,
        justify: JustifyContent,
        align: AlignItems,
    ) {
        let container_width = container.dimensions.width;
        let total_children = container.children.len().max(1) as f32;

        // Compute parallel layout for children using Rayon
        container.children.par_iter_mut().for_each(|child| {
            if child.dimensions.width == 0.0 {
                child.dimensions.width = container_width / total_children;
            }
            if child.dimensions.height == 0.0 {
                child.dimensions.height = 40.0;
            }
        });

        // Calculate positions based on Flex Direction & JustifyContent
        let mut offset_x = container.dimensions.x;
        let mut offset_y = container.dimensions.y;

        for child in &mut container.children {
            child.dimensions.x = offset_x;
            child.dimensions.y = offset_y;

            match direction {
                FlexDirection::Row => {
                    let step = match justify {
                        JustifyContent::Center => child.dimensions.width + 10.0,
                        _ => child.dimensions.width + 5.0,
                    };
                    offset_x += step;
                }
                FlexDirection::Column => {
                    let step = match align {
                        AlignItems::Center => child.dimensions.height + 10.0,
                        _ => child.dimensions.height + 5.0,
                    };
                    offset_y += step;
                }
            }
        }
    }
}
