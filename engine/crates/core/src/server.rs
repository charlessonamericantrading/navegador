//! Servidor NDJSON del motor nativo.
//!
//! Este proceso mantiene una sesión de página Rust viva. `navigate` usa el
//! cliente HTTP nativo y el pipeline DOM/CSS/JS/layout; `get_state` rasteriza
//! el layout con tiny-skia y devuelve la captura PNG en Base64. La salida
//! estándar contiene exclusivamente JSON; los logs van a stderr.

use crate::pipeline::{build_page_keeping_runtime, PageResult};
use crate::protocol::{
    ElementAttributes, ElementRect, EngineRequest, EngineResponse, InteractiveElement,
    PROTOCOL_VERSION,
};
use base64::Engine as _;
use engine_dom::{Node, NodeType};
use engine_gfx::render_layout_to_png;
use engine_js::JsRuntime;
use engine_layout::LayoutTreeBuilder;
use engine_net::{NetworkEngine, NetworkRequest};
use engine_text::SystemFont;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct LoadedPage {
    url: String,
    title: String,
    page: PageResult,
    runtime: JsRuntime,
    font: Option<SystemFont>,
    focused_node: Option<std::sync::Arc<std::sync::RwLock<Node>>>,
}

struct EngineServer {
    width: u32,
    height: u32,
    scroll_offset_y: f32,
    network: NetworkEngine,
    current_page: Option<LoadedPage>,
}

impl EngineServer {
    fn new() -> Self {
        Self {
            width: 1280,
            height: 720,
            scroll_offset_y: 0.0,
            network: NetworkEngine::new(),
            current_page: None,
        }
    }

    fn ready_response(&self, id: Option<String>) -> EngineResponse {
        EngineResponse::Ready {
            id,
            protocol_version: PROTOCOL_VERSION,
            renderer: "rust-native",
            renderer_status: "ready",
            width: self.width,
            height: self.height,
        }
    }

    async fn handle(&mut self, request: EngineRequest) -> (EngineResponse, bool) {
        let id = request.id().map(str::to_owned);

        match request {
            EngineRequest::Navigate { url, .. } => (self.navigate(id, url).await, false),
            EngineRequest::Ping { .. } => (
                EngineResponse::Pong {
                    id,
                    protocol_version: PROTOCOL_VERSION,
                    renderer_status: "ready",
                },
                false,
            ),
            EngineRequest::Resize { width, height, .. } => {
                self.width = width.clamp(200, 4000);
                self.height = height.clamp(200, 4000);
                if let Some(page) = &mut self.current_page {
                    page.page.layout_root = LayoutTreeBuilder::build(
                        &page.page.dom_root,
                        &page.page.stylesheet,
                        self.width as f32,
                        self.height as f32,
                        page.font.as_ref(),
                    );
                    self.scroll_offset_y = clamp_scroll_offset(
                        self.scroll_offset_y,
                        page.page.layout_root.content_extent(),
                        self.height as f32,
                    );
                }
                (self.state_response(id), false)
            }
            EngineRequest::GetState { .. } => (self.state_response(id), false),
            EngineRequest::Click { x, y, .. } => (self.click(id, x, y), false),
            EngineRequest::Scroll { dy, .. } => {
                if let Some(page) = &self.current_page {
                    self.scroll_offset_y = clamp_scroll_offset(
                        self.scroll_offset_y + dy as f32,
                        page.page.layout_root.content_extent(),
                        self.height as f32,
                    );
                }
                (self.state_response(id), false)
            }
            EngineRequest::TypeText {
                id,
                x,
                y,
                text,
                press_enter,
            } => (self.type_text(id, x, y, text, press_enter), false),
            EngineRequest::PressKey { id, key } => (self.press_key(id, key), false),
            EngineRequest::Shutdown { .. } => (
                EngineResponse::Ok {
                    id,
                    message: "engine_shutdown",
                },
                true,
            ),
        }
    }

    async fn navigate(&mut self, id: Option<String>, url: String) -> EngineResponse {
        let request = match NetworkRequest::new(&url) {
            Ok(request) => request,
            Err(error) => return Self::error(id, format!("invalid_url: {error}")),
        };

        let response = match self.network.fetch(&request).await {
            Ok(response) => response,
            Err(error) => return Self::error(id, format!("network_error: {error}")),
        };
        if !response.is_success() {
            return Self::error(
                id,
                format!(
                    "http_error: {} {}",
                    response.status_code, response.status_text
                ),
            );
        }

        let html = match response.text() {
            Ok(html) => html,
            Err(error) => return Self::error(id, format!("invalid_html_encoding: {error}")),
        };
        let font = SystemFont::load_default_sans_serif();
        let (page, runtime) = build_page_keeping_runtime(
            &html,
            "",
            self.width as f32,
            self.height as f32,
            font.as_ref(),
        );
        let title = Node::find_all_by_tag(&page.dom_root, "title")
            .first()
            .map(Node::text_content)
            .unwrap_or_default();

        self.current_page = Some(LoadedPage {
            url,
            title,
            page,
            runtime,
            font,
            focused_node: None,
        });
        self.scroll_offset_y = 0.0;
        self.state_response(id)
    }

    fn click(&mut self, id: Option<String>, x: f32, y: f32) -> EngineResponse {
        let Some(page) = &mut self.current_page else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        if let Some(node) = page
            .page
            .layout_root
            .hit_test(x, y + self.scroll_offset_y)
        {
            if is_text_control(&node) {
                page.focused_node = Some(node.clone());
                if let Err(error) = page.runtime.dispatch_event(&node, "focus") {
                    return Self::error(id, format!("focus_error: {error}"));
                }
            }
            if let Err(error) = page.runtime.dispatch_event(&node, "click") {
                return Self::error(id, format!("click_error: {error}"));
            }
            page.page.layout_root = LayoutTreeBuilder::build(
                &page.page.dom_root,
                &page.page.stylesheet,
                self.width as f32,
                self.height as f32,
                page.font.as_ref(),
            );
        }
        self.state_response(id)
    }

    fn type_text(
        &mut self,
        id: Option<String>,
        x: f32,
        y: f32,
        text: String,
        press_enter: bool,
    ) -> EngineResponse {
        let Some(page) = &mut self.current_page else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        let Some(node) = page
            .page
            .layout_root
            .hit_test(x, y + self.scroll_offset_y)
        else {
            return Self::error(id, "no hay ningún control bajo esas coordenadas".to_string());
        };
        if !is_text_control(&node) {
            return Self::error(
                id,
                "el elemento bajo esas coordenadas no es un control de texto".to_string(),
            );
        }

        page.focused_node = Some(node.clone());
        append_control_value(&node, &text);
        for event_type in ["focus", "input"] {
            if let Err(error) = page.runtime.dispatch_event(&node, event_type) {
                return Self::error(id, format!("{event_type}_error: {error}"));
            }
        }
        if press_enter {
            for event_type in ["keydown", "keyup"] {
                if let Err(error) = page.runtime.dispatch_event(&node, event_type) {
                    return Self::error(id, format!("{event_type}_error: {error}"));
                }
            }
        }
        page.page.layout_root = LayoutTreeBuilder::build(
            &page.page.dom_root,
            &page.page.stylesheet,
            self.width as f32,
            self.height as f32,
            page.font.as_ref(),
        );
        self.state_response(id)
    }

    fn press_key(&mut self, id: Option<String>, key: String) -> EngineResponse {
        let Some(page) = &mut self.current_page else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        let Some(node) = &page.focused_node else {
            return Self::error(id, format!("no hay un control enfocado para la tecla {key}"));
        };
        let node = node.clone();
        for event_type in ["keydown", "keyup"] {
            if let Err(error) = page.runtime.dispatch_event(&node, event_type) {
                return Self::error(id, format!("{event_type}_error: {error}"));
            }
        }
        self.state_response(id)
    }

    fn state_response(&self, id: Option<String>) -> EngineResponse {
        let Some(page) = &self.current_page else {
            return EngineResponse::State {
                id,
                renderer_status: "ready",
                scroll_offset_y: self.scroll_offset_y,
                url: String::new(),
                title: String::new(),
                screenshot: String::new(),
                elements: Vec::new(),
            };
        };

        let screenshot = match render_layout_to_png(
            &page.page.layout_root,
            page.font.as_ref(),
            self.width,
            self.height,
            self.scroll_offset_y,
        ) {
            Ok(bytes) => base64::engine::general_purpose::STANDARD.encode(bytes),
            Err(error) => return Self::error(id, format!("render_error: {error}")),
        };

        EngineResponse::State {
            id,
            renderer_status: "ready",
            scroll_offset_y: self.scroll_offset_y,
            url: page.url.clone(),
            title: page.title.clone(),
            screenshot,
            elements: collect_interactive_elements(&page.page.layout_root),
        }
    }

    fn error(id: Option<String>, message: String) -> EngineResponse {
        EngineResponse::Error { id, message }
    }
}

fn clamp_scroll_offset(offset: f32, content_extent: f32, viewport_height: f32) -> f32 {
    let max_offset = (content_extent - viewport_height).max(0.0);
    offset.clamp(0.0, max_offset)
}

fn is_text_control(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> bool {
    let node_guard = node.read().unwrap();
    match &node_guard.node_type {
        NodeType::Element { tag_name, .. } if tag_name == "textarea" => true,
        NodeType::Element {
            tag_name,
            attributes,
        } if tag_name == "input" => !matches!(
            attributes.get("type").map(String::as_str),
            Some("checkbox" | "radio" | "button" | "submit" | "reset" | "file")
        ),
        _ => false,
    }
}

fn append_control_value(node: &std::sync::Arc<std::sync::RwLock<Node>>, text: &str) {
    let mut node_guard = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut node_guard.node_type {
        attributes
            .entry("value".to_string())
            .or_default()
            .push_str(text);
    }
}

pub async fn run_stdio() -> io::Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());
    let mut server = EngineServer::new();

    write_response(&mut stdout, server.ready_response(Some("boot".to_string()))).await?;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<EngineRequest>(&line) {
            Ok(request) => {
                let (response, should_shutdown) = server.handle(request).await;
                write_response(&mut stdout, response).await?;
                if should_shutdown {
                    break;
                }
                continue;
            }
            Err(error) => EngineResponse::Error {
                id: None,
                message: format!("invalid_request: {error}"),
            },
        };

        write_response(&mut stdout, response).await?;
    }

    Ok(())
}

async fn write_response(
    stdout: &mut tokio::io::BufWriter<tokio::io::Stdout>,
    response: EngineResponse,
) -> io::Result<()> {
    let json = serde_json::to_string(&response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    stdout.write_all(json.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await
}

fn collect_interactive_elements(layout_root: &engine_layout::LayoutBox) -> Vec<InteractiveElement> {
    let mut elements = Vec::new();
    collect_elements_recursive(layout_root, &mut elements);
    elements
}

fn collect_elements_recursive(
    layout_box: &engine_layout::LayoutBox,
    elements: &mut Vec<InteractiveElement>,
) {
    if let Some(node) = &layout_box.dom_node {
        let node_guard = node.read().unwrap();
        if let NodeType::Element {
            tag_name,
            attributes,
        } = &node_guard.node_type
        {
            let id = elements.len() as u32;
            let selector = attributes
                .get("id")
                .map(|value| format!("#{value}"))
                .unwrap_or_else(|| tag_name.clone());
            elements.push(InteractiveElement {
                id,
                tag_name: tag_name.clone(),
                text: Node::text_content(node),
                rect: ElementRect {
                    x: layout_box.dimensions.x,
                    y: layout_box.dimensions.y,
                    width: layout_box.dimensions.width,
                    height: layout_box.dimensions.height,
                },
                selector,
                attributes: ElementAttributes {
                    id: attributes.get("id").cloned(),
                    name: attributes.get("name").cloned(),
                    placeholder: attributes.get("placeholder").cloned(),
                    element_type: attributes.get("type").cloned(),
                    role: attributes.get("role").cloned(),
                    href: attributes.get("href").cloned(),
                    value: attributes.get("value").cloned(),
                },
            });
        }
    }
    for child in &layout_box.children {
        collect_elements_recursive(child, elements);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EngineRequest;

    #[tokio::test]
    async fn server_starts_with_a_ready_native_renderer() {
        let server = EngineServer::new();
        let response = server.ready_response(Some("boot".to_string()));
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"renderer_status\":\"ready\""));
    }

    #[tokio::test]
    async fn resize_is_clamped_before_the_empty_state_is_returned() {
        let mut server = EngineServer::new();
        let (response, should_shutdown) = server
            .handle(EngineRequest::Resize {
                id: Some("resize".to_string()),
                width: 10,
                height: 9000,
            })
            .await;

        assert!(!should_shutdown);
        assert_eq!((server.width, server.height), (200, 4000));
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"renderer_status\":\"ready\""));
    }

    #[test]
    fn scroll_offset_is_clamped_to_the_real_content_extent() {
        assert_eq!(clamp_scroll_offset(-100.0, 2000.0, 720.0), 0.0);
        assert_eq!(clamp_scroll_offset(5000.0, 2000.0, 720.0), 1280.0);
        assert_eq!(clamp_scroll_offset(50.0, 400.0, 720.0), 0.0);
    }
}
