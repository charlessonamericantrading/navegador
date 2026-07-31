use winit::{
    event::{Event, WindowEvent},
    event_loop::EventLoop,
    window::WindowBuilder,
};
use tiny_skia::{Pixmap, Paint, Rect, Transform, Color};
use engine_layout::LayoutBox;
use crate::display_list::{DisplayList, DisplayItem};

pub struct NativeEngineWindow;

impl NativeEngineWindow {
    pub fn run(title: &str, layout_root: LayoutBox) {
        let event_loop = EventLoop::new().unwrap();
        let window = WindowBuilder::new()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
            .build(&event_loop)
            .unwrap();

        tracing::info!("Native OS Window initialized: '{}'", title);

        let _ = event_loop.run(move |event, elwt| {
            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => elwt.exit(),
                Event::WindowEvent {
                    event: WindowEvent::RedrawRequested,
                    ..
                } => {
                    let size = window.inner_size();
                    if let Some(mut pixmap) = Pixmap::new(size.width.max(1), size.height.max(1)) {
                        pixmap.fill(Color::from_rgba8(245, 245, 245, 255));
                        let display_list = DisplayList::build(&layout_root);

                        for item in display_list.items {
                            match item {
                                DisplayItem::SolidRect { rect, color } => {
                                    let mut rect_paint = Paint::default();
                                    rect_paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
                                    if let Some(sk_rect) = Rect::from_xywh(rect.x, rect.y, rect.width.max(10.0), rect.height.max(10.0)) {
                                        pixmap.fill_rect(sk_rect, &rect_paint, Transform::identity(), None);
                                    }
                                }
                                DisplayItem::Text { rect, .. } => {
                                    let mut text_paint = Paint::default();
                                    text_paint.set_color_rgba8(20, 20, 20, 255);
                                    if let Some(sk_rect) = Rect::from_xywh(rect.x, rect.y, rect.width.max(100.0), 20.0) {
                                        pixmap.fill_rect(sk_rect, &text_paint, Transform::identity(), None);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
        });
    }
}
