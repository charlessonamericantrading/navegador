//! Servidor NDJSON del motor nativo.
//!
//! Este proceso mantiene una sesión de página Rust viva. `navigate` usa el
//! cliente HTTP nativo y el pipeline DOM/CSS/JS/layout; `get_state` rasteriza
//! el layout con tiny-skia y devuelve la captura PNG en Base64. La salida
//! estándar contiene exclusivamente JSON; los logs van a stderr.

use crate::pipeline::{build_page_keeping_runtime, find_external_script_srcs, find_external_stylesheet_hrefs, find_image_srcs, PageResult};
use crate::protocol::{
    ElementAttributes, ElementRect, EngineRequest, EngineResponse, InteractiveElement,
    PROTOCOL_VERSION,
};
use base64::Engine as _;
use engine_dom::{Node, NodeType};
use engine_gfx::render_layout_to_png;
use engine_image::decode_image;
use engine_js::JsRuntime;
use engine_layout::{ImageMap, LayoutTreeBuilder};
use engine_net::{NetworkEngine, NetworkRequest};
use engine_text::FontSet;
use std::collections::HashMap;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct LoadedPage {
    url: String,
    title: String,
    page: PageResult,
    runtime: JsRuntime,
    font_set: Option<FontSet>,
    images: ImageMap,
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
                        page.font_set.as_ref(),
                        &page.images,
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

        // `response.url` es la URL que realmente respondio 200, tras seguir
        // las redirecciones que hiciera falta (ver `NetworkEngine::fetch`) -
        // no el parametro `url` que llego en la peticion, que puede ser una
        // URL intermedia que ya no existe. Tambien es la base correcta para
        // resolver rutas relativas de `<link href>` (si `/viejo` redirigio a
        // `/nuevo/`, un href="foo.css" es `/nuevo/foo.css`, no `/foo.css`).
        let page_url = response.url.clone();
        let final_url = page_url.to_string();
        let html = match response.text() {
            Ok(html) => html,
            Err(error) => return Self::error(id, format!("invalid_html_encoding: {error}")),
        };

        // Se parsea UNA vez aqui solo para descubrir que recursos externos
        // hacen falta (`<link rel=stylesheet>`, `<script src>`) - un DOM de
        // usar y tirar, distinto del DOM real que construye
        // `build_page_keeping_runtime` mas abajo. Duplica el parseo (barato,
        // microsegundos para una pagina normal) a cambio de que
        // `pipeline.rs` siga sin depender de `url`/`engine-net` para nada -
        // ver el doc-comment de `find_external_stylesheet_hrefs`.
        let discovery_dom = engine_dom::HtmlParser::parse(&html);
        let stylesheet_hrefs = find_external_stylesheet_hrefs(&discovery_dom);
        let script_srcs = find_external_script_srcs(&discovery_dom);
        let image_srcs = find_image_srcs(&discovery_dom);

        let external_css = self.fetch_external_stylesheets(stylesheet_hrefs, &page_url).await;
        let external_scripts = self.fetch_external_scripts(script_srcs, &page_url).await;
        let images = self.fetch_images(image_srcs, &page_url).await;

        let font_set = FontSet::load_default_sans_serif();
        let (page, runtime) = build_page_keeping_runtime(
            &html,
            &external_css,
            self.width as f32,
            self.height as f32,
            Some(&font_set),
            &external_scripts,
            &images,
        );
        let title = Node::find_all_by_tag(&page.dom_root, "title")
            .first()
            .map(Node::text_content)
            .unwrap_or_default();

        self.current_page = Some(LoadedPage {
            url: final_url,
            title,
            page,
            runtime,
            font_set: Some(font_set),
            images,
            focused_node: None,
        });
        self.scroll_offset_y = 0.0;
        self.state_response(id)
    }

    /// Descarga cada href de `<link rel="stylesheet">` ya descubierto por
    /// `find_external_stylesheet_hrefs` y concatena su contenido, en orden
    /// de documento - la inmensa mayoria de la web real no lleva su CSS en
    /// `<style>` inline, asi que sin esto casi ninguna pagina real llegaba
    /// a verse estilada.
    ///
    /// Cada href se resuelve contra `page_url` (la URL final tras
    /// redirecciones, no la pedida originalmente). Una hoja que falla al
    /// descargarse (404, red caida, URL invalida) se omite con un aviso en
    /// vez de abortar la carga entera de la pagina - igual que un
    /// navegador real, que sigue mostrando la pagina con el resto de sus
    /// estilos aunque una hoja concreta no cargue.
    async fn fetch_external_stylesheets(&self, hrefs: Vec<String>, page_url: &url::Url) -> String {
        let mut combined = String::new();
        for href in hrefs {
            let Ok(sheet_url) = page_url.join(&href) else {
                tracing::warn!("[server] href de <link rel=stylesheet> invalido, se omite: {href}");
                continue;
            };
            let request = match NetworkRequest::new(sheet_url.as_str()) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!("[server] no se pudo construir la peticion para {sheet_url}: {error}");
                    continue;
                }
            };
            match self.network.fetch(&request).await {
                Ok(response) if response.is_success() => match response.text() {
                    Ok(css) => {
                        combined.push_str(&css);
                        combined.push('\n');
                    }
                    Err(error) => tracing::warn!("[server] {sheet_url} no es texto valido, se omite: {error}"),
                },
                Ok(response) => tracing::warn!(
                    "[server] {sheet_url} respondio {} {}, se omite",
                    response.status_code,
                    response.status_text
                ),
                Err(error) => tracing::warn!("[server] no se pudo descargar {sheet_url}: {error}"),
            }
        }
        combined
    }

    /// Descarga cada src de `<script src>` ya descubierto por
    /// `find_external_script_srcs` y devuelve un mapa `src crudo ->
    /// contenido`, que `scripting::run_scripts` usa para ejecutar cada
    /// script externo en su posicion exacta de documento (ver el
    /// doc-comment de `scripting.rs` sobre por que el orden importa aqui y
    /// no importaba para las hojas de estilo). La clave es el `src` SIN
    /// resolver (tal como aparece en el HTML), porque es lo unico que
    /// `run_scripts` ve al recorrer el DOM - resolverlo pasa AQUI, antes de
    /// insertarlo en el mapa.
    ///
    /// Mismo criterio que las hojas de estilo: un script que falla al
    /// descargarse se omite con un aviso, no aborta la pagina entera.
    async fn fetch_external_scripts(&self, srcs: Vec<String>, page_url: &url::Url) -> HashMap<String, String> {
        let mut fetched = HashMap::new();
        for src in srcs {
            let Ok(script_url) = page_url.join(&src) else {
                tracing::warn!("[server] src de <script> invalido, se omite: {src}");
                continue;
            };
            let request = match NetworkRequest::new(script_url.as_str()) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!("[server] no se pudo construir la peticion para {script_url}: {error}");
                    continue;
                }
            };
            match self.network.fetch(&request).await {
                Ok(response) if response.is_success() => match response.text() {
                    Ok(js) => {
                        fetched.insert(src, js);
                    }
                    Err(error) => tracing::warn!("[server] {script_url} no es texto valido, se omite: {error}"),
                },
                Ok(response) => tracing::warn!(
                    "[server] {script_url} respondio {} {}, se omite",
                    response.status_code,
                    response.status_text
                ),
                Err(error) => tracing::warn!("[server] no se pudo descargar {script_url}: {error}"),
            }
        }
        fetched
    }

    /// Descarga cada `src` de `<img src>` ya descubierto por
    /// `find_image_srcs` y lo decodifica a RGBA8 (`engine_image::
    /// decode_image`) - mismo criterio exacto que `fetch_external_scripts`
    /// (resuelve contra `page_url`, un fallo se omite con un aviso en vez
    /// de abortar la carga entera de la pagina), solo que el resultado son
    /// bytes binarios decodificados en vez de texto. La clave sigue siendo
    /// el `src` SIN resolver - lo unico que `engine-layout`/`engine-gfx`
    /// ven al recorrer las cajas (`BoxType::Image`), igual que
    /// `external_scripts`.
    async fn fetch_images(&self, srcs: Vec<String>, page_url: &url::Url) -> ImageMap {
        let mut fetched = ImageMap::new();
        for src in srcs {
            let Ok(image_url) = page_url.join(&src) else {
                tracing::warn!("[server] src de <img> invalido, se omite: {src}");
                continue;
            };
            let request = match NetworkRequest::new(image_url.as_str()) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!("[server] no se pudo construir la peticion para {image_url}: {error}");
                    continue;
                }
            };
            match self.network.fetch(&request).await {
                Ok(response) if response.is_success() => match decode_image(&response.body) {
                    Some(image) => {
                        fetched.insert(src, image);
                    }
                    None => tracing::warn!("[server] {image_url} no se pudo decodificar como imagen, se omite"),
                },
                Ok(response) => tracing::warn!(
                    "[server] {image_url} respondio {} {}, se omite",
                    response.status_code,
                    response.status_text
                ),
                Err(error) => tracing::warn!("[server] no se pudo descargar {image_url}: {error}"),
            }
        }
        fetched
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
                page.font_set.as_ref(),
                        &page.images,
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
            page.font_set.as_ref(),
                        &page.images,
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
            page.font_set.as_ref(),
                        &page.images,
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
