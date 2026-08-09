//! Servidor NDJSON del motor nativo.
//!
//! Este proceso mantiene una sesión de página Rust viva. `navigate` usa el
//! cliente HTTP nativo y el pipeline DOM/CSS/JS/layout; `get_state` rasteriza
//! el layout con tiny-skia y devuelve la captura PNG en Base64. La salida
//! estándar contiene exclusivamente JSON; los logs van a stderr.

use crate::pipeline::{build_page_keeping_runtime, find_external_script_srcs, find_external_stylesheet_hrefs, find_image_srcs, PageResult};
use crate::protocol::{
    ElementAttributes, ElementRect, EngineRequest, EngineResponse, InteractiveElement, TabInfo,
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

/// Pestaña (Fase 4.5) - agrupa TODO lo que ya era, antes de esta fase,
/// estado directo de `EngineServer` y que en realidad pertenece a una
/// sesion de navegacion concreta, no a la ventana entera: la pagina
/// cargada, su propio historial atras/adelante (Fase 4.4) y su propio
/// desplazamiento vertical. `width`/`height` SI se quedan en
/// `EngineServer` - son el tamaño de la VENTANA, compartido por todas las
/// pestañas (igual que un navegador real: cambiar de pestaña no cambia el
/// tamaño de la ventana).
struct Tab {
    id: u32,
    current_page: Option<LoadedPage>,
    history: Vec<String>,
    history_index: Option<usize>,
    scroll_offset_y: f32,
}

impl Tab {
    fn new(id: u32) -> Self {
        Self {
            id,
            current_page: None,
            history: Vec::new(),
            history_index: None,
            scroll_offset_y: 0.0,
        }
    }
}

struct EngineServer {
    width: u32,
    height: u32,
    // `Arc` (Fase 4.3, no `NetworkEngine` a secas) - `fetch()` real
    // necesita su PROPIA copia del mismo cliente HTTP/pool de conexiones
    // ya construido (`register_fetch`, via `build_page_keeping_runtime`),
    // no uno nuevo, y vive dentro del `JsRuntime` de cada pagina cargada,
    // fuera del `&self`/`&mut self` normal de este struct - de ahi la
    // necesidad de un handle compartido en vez de un prestamo.
    network: std::sync::Arc<NetworkEngine>,
    /// Pestañas (Fase 4.5) - siempre tiene AL MENOS una (invariante
    /// mantenida por `close_tab`, que rechaza cerrar la ultima). `tabs`
    /// nunca se reordena por id, solo se inserta al final (`open_new_tab`)
    /// o se borra por posicion (`close_tab`) - por eso todo acceso por id
    /// busca con `position()` en vez de asumir que el indice == el id.
    tabs: Vec<Tab>,
    /// Indice (NO id) dentro de `tabs` de la pestaña actualmente visible -
    /// la que `state_response`/`click`/`navigate`/etc. leen y mutan por
    /// defecto via `active_tab`/`active_tab_mut`.
    active_tab: usize,
    /// Siguiente id a asignar en `open_new_tab` - monotonamente creciente,
    /// nunca se reutiliza aunque se cierren pestañas (igual que un
    /// navegador real, donde el id interno de una pestaña cerrada no
    /// vuelve a aparecer).
    next_tab_id: u32,
}

impl EngineServer {
    fn new() -> Self {
        Self {
            width: 1280,
            height: 720,
            network: std::sync::Arc::new(NetworkEngine::new()),
            tabs: vec![Tab::new(0)],
            active_tab: 0,
            next_tab_id: 1,
        }
    }

    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
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
            EngineRequest::Navigate { url, .. } => (self.navigate(id, url, true).await, false),
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
                // Solo la pestaña ACTIVA se relayouta aqui (Fase 4.5) - las
                // demas se ponen al dia de forma perezosa en `switch_tab`
                // cuando (si) el usuario vuelve a ellas, para no pagar el
                // coste de un relayout completo por cada pestaña en
                // segundo plano en cada `resize`.
                let (w, h) = (self.width, self.height);
                let tab = self.active_tab_mut();
                if let Some(page) = &mut tab.current_page {
                    page.page.layout_root = LayoutTreeBuilder::build(
                        &page.page.dom_root,
                        &page.page.stylesheet,
                        w as f32,
                        h as f32,
                        page.font_set.as_ref(),
                        &page.images,
                    );
                    let content_extent = page.page.layout_root.content_extent();
                    tab.scroll_offset_y = clamp_scroll_offset(tab.scroll_offset_y, content_extent, h as f32);
                }
                (self.state_response(id), false)
            }
            EngineRequest::GetState { .. } => (self.state_response(id), false),
            EngineRequest::Click { x, y, .. } => (self.click(id, x, y).await, false),
            EngineRequest::Scroll { dy, .. } => {
                let h = self.height;
                let tab = self.active_tab_mut();
                if let Some(page) = &tab.current_page {
                    let content_extent = page.page.layout_root.content_extent();
                    tab.scroll_offset_y = clamp_scroll_offset(tab.scroll_offset_y + dy as f32, content_extent, h as f32);
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
            EngineRequest::Back { .. } => (self.back(id).await, false),
            EngineRequest::Forward { .. } => (self.forward(id).await, false),
            EngineRequest::NewTab { url, .. } => (self.open_new_tab(id, url).await, false),
            EngineRequest::CloseTab { tab_id, .. } => (self.close_tab(id, tab_id), false),
            EngineRequest::SwitchTab { tab_id, .. } => (self.switch_tab(id, tab_id), false),
            EngineRequest::ListTabs { .. } => (self.list_tabs(id), false),
            EngineRequest::Shutdown { .. } => (
                EngineResponse::Ok {
                    id,
                    message: "engine_shutdown",
                },
                true,
            ),
        }
    }

    /// `record_history` (Fase 4.4): `true` para una navegacion NUEVA
    /// (comando NDJSON `navigate`, o un clic en `<a href>` - ver `click`)
    /// - empuja la URL final al historial, descartando cualquier entrada
    /// "adelante" que quedara por delante. `false` para `back`/`forward`
    /// (mas abajo), que ya movieron `history_index` ELLOS MISMOS antes de
    /// llamar aqui - si `navigate` tambien empujara, `back` se
    /// autodestruiria el historial "adelante" al que deberia poder volver
    /// despues.
    async fn navigate(&mut self, id: Option<String>, url: String, record_history: bool) -> EngineResponse {
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
            Some(self.network.clone()),
        );
        let title = Node::find_all_by_tag(&page.dom_root, "title")
            .first()
            .map(Node::text_content)
            .unwrap_or_default();

        if record_history {
            let tab = self.active_tab_mut();
            let next_index = tab.history_index.map(|i| i + 1).unwrap_or(0);
            tab.history.truncate(next_index);
            tab.history.push(final_url.clone());
            tab.history_index = Some(next_index);
        }

        let tab = self.active_tab_mut();
        tab.current_page = Some(LoadedPage {
            url: final_url,
            title,
            page,
            runtime,
            font_set: Some(font_set),
            images,
            focused_node: None,
        });
        tab.scroll_offset_y = 0.0;
        self.state_response(id)
    }

    /// Vuelve a la entrada ANTERIOR del historial (Fase 4.4) - vuelve a
    /// pedir la pagina por red de verdad, no restaura un snapshot (ver el
    /// doc-comment de `history` en el struct). Error honesto (no un
    /// no-op silencioso) si no hay ningun historial todavia o ya se esta
    /// en la primera entrada - igual que `history.back()` real, que
    /// tampoco hace nada observable en ese caso, pero aqui se reporta en
    /// vez de fingir exito.
    async fn back(&mut self, id: Option<String>) -> EngineResponse {
        let tab = self.active_tab();
        let Some(index) = tab.history_index else {
            return Self::error(id, "no hay historial".to_string());
        };
        let Some(previous_index) = index.checked_sub(1) else {
            return Self::error(id, "ya se esta en la primera pagina del historial".to_string());
        };
        let url = tab.history[previous_index].clone();
        self.active_tab_mut().history_index = Some(previous_index);
        self.navigate(id, url, false).await
    }

    /// Simetrico a `back` - avanza a la entrada SIGUIENTE del historial.
    async fn forward(&mut self, id: Option<String>) -> EngineResponse {
        let tab = self.active_tab();
        let Some(index) = tab.history_index else {
            return Self::error(id, "no hay historial".to_string());
        };
        let next_index = index + 1;
        if next_index >= tab.history.len() {
            return Self::error(id, "ya se esta en la ultima pagina del historial".to_string());
        }
        let url = tab.history[next_index].clone();
        self.active_tab_mut().history_index = Some(next_index);
        self.navigate(id, url, false).await
    }

    /// Abre una pestaña nueva (Fase 4.5) y la hace ACTIVA de inmediato -
    /// igual que `target="_blank"` en un navegador real (ver `click` mas
    /// abajo, unico llamador ademas del propio comando NDJSON `new_tab`):
    /// SIEMPRE enfoca la pestaña recien creada, nunca la deja en segundo
    /// plano. Sin `url`, la pestaña queda en blanco (`current_page: None`),
    /// igual que abrir una pestaña nueva a mano antes de teclear nada.
    async fn open_new_tab(&mut self, id: Option<String>, url: Option<String>) -> EngineResponse {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(tab_id));
        self.active_tab = self.tabs.len() - 1;
        match url {
            Some(url) => self.navigate(id, url, true).await,
            None => self.state_response(id),
        }
    }

    /// Cierra la pestaña `tab_id` (Fase 4.5). Error honesto (no un no-op
    /// silencioso) si es la UNICA pestaña abierta - un navegador real
    /// cerraria la ventana entera en ese caso, fuera del alcance de este
    /// servidor, que siempre mantiene al menos una - o si `tab_id` no
    /// existe. Si la pestaña cerrada era la ACTIVA, activa la que quedo a
    /// su IZQUIERDA (o la nueva primera, si cerraba la de mas a la
    /// izquierda) - mismo criterio que la mayoria de navegadores reales.
    fn close_tab(&mut self, id: Option<String>, tab_id: u32) -> EngineResponse {
        if self.tabs.len() <= 1 {
            return Self::error(id, "no se puede cerrar la unica pestaña abierta".to_string());
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return Self::error(id, format!("no existe ninguna pestaña con id {tab_id}"));
        };
        self.tabs.remove(index);
        if index == self.active_tab {
            self.active_tab = index.saturating_sub(1);
        } else if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.state_response(id)
    }

    /// Cambia la pestaña ACTIVA a `tab_id` (Fase 4.5). Relayout perezoso de
    /// la pestaña recien activada: el tamaño de la ventana pudo cambiar
    /// (`resize`) mientras estaba en segundo plano - `resize` solo
    /// relayouta la pestaña activa EN ESE MOMENTO (ver el handler de
    /// `Resize` en `handle`), asi que hay que ponerla al dia aqui, no ahi.
    fn switch_tab(&mut self, id: Option<String>, tab_id: u32) -> EngineResponse {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return Self::error(id, format!("no existe ninguna pestaña con id {tab_id}"));
        };
        self.active_tab = index;
        let (w, h) = (self.width, self.height);
        let tab = self.active_tab_mut();
        if let Some(page) = &mut tab.current_page {
            page.page.layout_root = LayoutTreeBuilder::build(
                &page.page.dom_root,
                &page.page.stylesheet,
                w as f32,
                h as f32,
                page.font_set.as_ref(),
                &page.images,
            );
            let content_extent = page.page.layout_root.content_extent();
            tab.scroll_offset_y = clamp_scroll_offset(tab.scroll_offset_y, content_extent, h as f32);
        }
        self.state_response(id)
    }

    /// Lista las pestañas abiertas (Fase 4.5) - titulo/URL vacios para una
    /// pestaña que todavia no cargo ninguna pagina (recien abierta con
    /// `new_tab` sin `url`), igual que `state_response` hace para la
    /// pestaña activa sin pagina.
    fn list_tabs(&self, id: Option<String>) -> EngineResponse {
        let active_tab_id = self.active_tab().id;
        let tabs = self
            .tabs
            .iter()
            .map(|tab| TabInfo {
                id: tab.id,
                title: tab.current_page.as_ref().map(|page| page.title.clone()).unwrap_or_default(),
                url: tab.current_page.as_ref().map(|page| page.url.clone()).unwrap_or_default(),
            })
            .collect();
        EngineResponse::Tabs { id, tabs, active_tab_id }
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

    /// `async` desde la Fase 4.2 (antes era sincrona): un clic sobre un
    /// `<a href>` navegable dispara una navegacion real
    /// (`EngineServer::navigate`, que hace un fetch HTTP de verdad), igual
    /// que cualquier otro `await` de este servidor. La deteccion/resolucion
    /// del enlace ocurre DENTRO del bloque que presta `self.current_page`
    /// (necesita `page.url` para resolver el `href` relativo) pero la
    /// llamada a `navigate` en si ocurre DESPUES de que ese prestamo
    /// termine - `navigate` reasigna `self.current_page` entero, asi que
    /// no puede convivir con un prestamo activo de la pagina actual.
    async fn click(&mut self, id: Option<String>, x: f32, y: f32) -> EngineResponse {
        let mut link_target: Option<String> = None;
        let mut opens_new_tab = false;
        let scroll_offset_y = self.active_tab().scroll_offset_y;
        let (width, height) = (self.width, self.height);
        {
            let tab = self.active_tab_mut();
            let Some(page) = &mut tab.current_page else {
                return Self::error(id, "no hay ninguna página cargada".to_string());
            };
            if let Some(node) = page
                .page
                .layout_root
                .hit_test(x, y + scroll_offset_y)
            {
                if is_text_control(&node) {
                    page.focused_node = Some(node.clone());
                    if let Err(error) = page.runtime.dispatch_event(&node, "focus") {
                        return Self::error(id, format!("focus_error: {error}"));
                    }
                }
                // `checked` se conmuta ANTES de disparar "click" (Fase 4.1) -
                // asi es el orden real de un navegador de verdad: la accion
                // por defecto de clicar un checkbox/radio (voltear su estado)
                // ya ocurrio para cuando los listeners de "click" ven el
                // evento, asi que `event.target.checked` dentro de un handler
                // real ya refleja el nuevo valor - ver `toggle_checked`.
                if is_checkable_input(&node) {
                    toggle_checked(&node);
                }
                // El `bool` devuelto (Fase 4.2) es si algun listener llamo
                // `event.preventDefault()` - determina mas abajo si la
                // navegacion por `<a href>` (la ACCION POR DEFECTO real de
                // clicar un enlace) debe cancelarse, igual que un navegador
                // real.
                let click_prevented = match page.runtime.dispatch_event(&node, "click") {
                    Ok(prevented) => prevented,
                    Err(error) => return Self::error(id, format!("click_error: {error}")),
                };
                if is_checkable_input(&node) {
                    if let Err(error) = page.runtime.dispatch_event(&node, "change") {
                        return Self::error(id, format!("change_error: {error}"));
                    }
                }
                // Navegacion real por `<a href>` (Fase 4.2): busca el `<a>`
                // navegable mas cercano (el propio `node` o un ANCESTRO -
                // un clic real casi siempre aterriza en un descendiente,
                // como el texto o un `<b>` dentro del enlace, ver
                // `find_link_target`) y resuelve su `href` (posiblemente
                // relativo) contra la URL de la pagina actual. Sin nada que
                // navegar (no hay enlace, el href no es navegable, o algun
                // listener ya cancelo la accion por defecto), sigue igual
                // que antes: solo relayout + captura del estado actual.
                if !click_prevented {
                    if let Some(target) = find_link_target(&node) {
                        if let Ok(page_url) = url::Url::parse(&page.url) {
                            if let Ok(resolved) = page_url.join(&target.href) {
                                link_target = Some(resolved.to_string());
                                opens_new_tab = target.opens_new_tab;
                            }
                        }
                    }
                }
                page.page.layout_root = LayoutTreeBuilder::build(
                    &page.page.dom_root,
                    &page.page.stylesheet,
                    width as f32,
                    height as f32,
                    page.font_set.as_ref(),
                            &page.images,
                );
            }
        }
        if let Some(target_url) = link_target {
            // `target="_blank"` (Fase 4.5, antes NO implementado - ver el
            // doc-comment de `find_link_target`) abre una pestaña nueva en
            // vez de navegar la pestaña actual, igual que un navegador
            // real.
            if opens_new_tab {
                return self.open_new_tab(id, Some(target_url)).await;
            }
            return self.navigate(id, target_url, true).await;
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
        let scroll_offset_y = self.active_tab().scroll_offset_y;
        let (width, height) = (self.width, self.height);
        let Some(page) = &mut self.active_tab_mut().current_page else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        let Some(node) = page
            .page
            .layout_root
            .hit_test(x, y + scroll_offset_y)
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
                if let Err(error) = page.runtime.dispatch_keyboard_event(&node, event_type, "Enter") {
                    return Self::error(id, format!("{event_type}_error: {error}"));
                }
            }
        }
        page.page.layout_root = LayoutTreeBuilder::build(
            &page.page.dom_root,
            &page.page.stylesheet,
            width as f32,
            height as f32,
            page.font_set.as_ref(),
                        &page.images,
        );
        self.state_response(id)
    }

    /// `key` es el mismo string que `KeyboardEvent.key` real (`"Backspace"`,
    /// `"a"`, `"Enter"`...) - se usa TANTO para poblar `event.key` (via
    /// `dispatch_keyboard_event`, Fase 4.1) COMO para decidir si hay una
    /// mutacion real de `value` que hacer antes de disparar los eventos:
    /// `"Backspace"`/`"Delete"` quitan el ULTIMO caracter de `value` (sin
    /// cursor/seleccion todavia - ver `backspace_control_value`, misma
    /// simplificacion ya declarada en `append_control_value`/`type_text`,
    /// que tambien edita siempre al final). Cualquier OTRA tecla
    /// (`"Enter"`, `"Tab"`, una letra suelta...) no muta nada por su
    /// cuenta - escribir texto de verdad sigue siendo trabajo de
    /// `type_text` (la fuente real de "el usuario tecleo estos
    /// caracteres"), `press_key` es para teclas de control sueltas.
    fn press_key(&mut self, id: Option<String>, key: String) -> EngineResponse {
        let (width, height) = (self.width, self.height);
        let Some(page) = &mut self.active_tab_mut().current_page else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        let Some(node) = &page.focused_node else {
            return Self::error(id, format!("no hay un control enfocado para la tecla {key}"));
        };
        let node = node.clone();
        let mutates_value = matches!(key.as_str(), "Backspace" | "Delete") && is_text_control(&node);
        if mutates_value {
            backspace_control_value(&node);
        }
        for event_type in ["keydown", "keyup"] {
            if let Err(error) = page.runtime.dispatch_keyboard_event(&node, event_type, &key) {
                return Self::error(id, format!("{event_type}_error: {error}"));
            }
        }
        if mutates_value {
            if let Err(error) = page.runtime.dispatch_event(&node, "input") {
                return Self::error(id, format!("input_error: {error}"));
            }
            page.page.layout_root = LayoutTreeBuilder::build(
                &page.page.dom_root,
                &page.page.stylesheet,
                width as f32,
                height as f32,
                page.font_set.as_ref(),
                        &page.images,
            );
        }
        self.state_response(id)
    }

    fn state_response(&self, id: Option<String>) -> EngineResponse {
        let tab = self.active_tab();
        // Fase 4.4: `can_go_back`/`can_go_forward` se calculan siempre
        // igual, tanto con pagina cargada como sin ella (`history_index`
        // es independiente de `current_page` - conceptualmente podrian
        // desincronizarse solo si `back`/`forward` fallaran a mitad de
        // camino, ver el doc-comment de `back`).
        let can_go_back = tab.history_index.is_some_and(|index| index > 0);
        let can_go_forward = tab.history_index.is_some_and(|index| index + 1 < tab.history.len());

        let Some(page) = &tab.current_page else {
            return EngineResponse::State {
                id,
                renderer_status: "ready",
                tab_id: tab.id,
                scroll_offset_y: tab.scroll_offset_y,
                url: String::new(),
                title: String::new(),
                screenshot: String::new(),
                elements: Vec::new(),
                can_go_back,
                can_go_forward,
            };
        };

        let screenshot = match render_layout_to_png(
            &page.page.layout_root,
            page.font_set.as_ref(),
                        &page.images,
            self.width,
            self.height,
            tab.scroll_offset_y,
        ) {
            Ok(bytes) => base64::engine::general_purpose::STANDARD.encode(bytes),
            Err(error) => return Self::error(id, format!("render_error: {error}")),
        };

        EngineResponse::State {
            id,
            renderer_status: "ready",
            tab_id: tab.id,
            scroll_offset_y: tab.scroll_offset_y,
            url: page.url.clone(),
            title: page.title.clone(),
            screenshot,
            elements: collect_interactive_elements(&page.page.layout_root),
            can_go_back,
            can_go_forward,
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

/// Quita el ULTIMO caracter de `value` (Fase 4.1, `Backspace`/`Delete` en
/// `press_key`) - misma simplificacion ya declarada en
/// `append_control_value`/`type_text`: sin cursor ni seleccion todavia, la
/// edicion siempre ocurre al final del texto, nunca en medio. No-op
/// honesto (no un panic) sobre un `value` ya vacio.
fn backspace_control_value(node: &std::sync::Arc<std::sync::RwLock<Node>>) {
    let mut node_guard = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut node_guard.node_type {
        if let Some(value) = attributes.get_mut("value") {
            value.pop();
        }
    }
}

/// `<input type="checkbox">`/`<input type="radio">` - los DOS tipos donde
/// un clic real conmuta `checked` en vez de (o ademas de) enfocar un
/// control de texto (`is_text_control` los excluye explicitamente, ver
/// arriba - son mutuamente excluyentes por diseño).
fn is_checkable_input(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> bool {
    let node_guard = node.read().unwrap();
    matches!(
        &node_guard.node_type,
        NodeType::Element { tag_name, attributes } if tag_name == "input"
            && matches!(attributes.get("type").map(String::as_str), Some("checkbox" | "radio"))
    )
}

/// Conmuta el atributo booleano `checked` (Fase 4.1) - semantica de
/// atributo booleano HTML real: PRESENCIA en el mapa de atributos =
/// marcado, AUSENCIA = sin marcar (el valor en si, si lo hay, no importa -
/// por eso se inserta la cadena vacia, no `"true"`/`"checked"`). Sin
/// comportamiento de GRUPO para `radio` (desmarcar automaticamente los
/// demas `input[type=radio]` con el mismo `name`) - simplificacion
/// declarada: cada radio se conmuta de forma independiente, a diferencia
/// del spec real donde marcar uno desmarca a sus hermanos del mismo grupo;
/// eso exigiria recorrer el DOM entero buscando coincidencias por `name`,
/// fuera del alcance de esta tarea.
fn toggle_checked(node: &std::sync::Arc<std::sync::RwLock<Node>>) {
    let mut node_guard = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut node_guard.node_type {
        if attributes.remove("checked").is_none() {
            attributes.insert("checked".to_string(), String::new());
        }
    }
}

/// Resultado de `find_link_target` - el `href` (posiblemente relativo, sin
/// resolver todavia - eso es trabajo del llamador, que conoce la URL de la
/// pagina actual) y si el enlace deberia abrirse en una pestaña NUEVA
/// (Fase 4.5, `target="_blank"`) en vez de navegar la pestaña actual.
struct LinkTarget {
    href: String,
    opens_new_tab: bool,
}

/// Busca el `<a href="...">` NAVEGABLE mas cercano empezando en `node` e
/// INCLUYENDOLO (Fase 4.2) - un clic real casi siempre aterriza en un
/// DESCENDIENTE del enlace (el nodo de texto, un `<b>`/`<span>` decorativo
/// dentro de el), nunca en el propio `<a>`, asi que hay que subir por
/// `parent` hasta encontrar uno. Se DETIENE en el primer `<a>` que
/// encuentra, tenga `href` navegable o no (un `<a>` sin `href`, o anidado
/// dentro de otro `<a>` - HTML invalido de todas formas - no deberia
/// seguir subiendo mas alla buscando un enlace exterior). "Navegable"
/// excluye deliberadamente: `href` vacio o ausente (un `<a>` sin `href` no
/// es ni siquiera un hyperlink real, el spec lo llama "enlace de marcador
/// de posicion"), anclas internas (`href="#seccion"` - deberia hacer scroll
/// a un elemento con ese `id`, no una navegacion de pagina nueva, sin
/// implementar todavia) y `javascript:` (ejecutaria script en vez de
/// navegar, sin implementar todavia - ver ARCHITECTURE.md para ambos
/// huecos declarados).
///
/// `opens_new_tab` (Fase 4.5, antes NO implementado) es `true` cuando el
/// MISMO `<a>` navegable lleva `target="_blank"` (comparacion insensible a
/// mayusculas, como hacen los navegadores reales) - cualquier OTRO valor de
/// `target` (`"_self"`, un nombre de frame inventado, ausente...) se trata
/// como navegacion normal en la misma pestaña, simplificacion declarada:
/// un navegador real tambien abriria pestaña nueva para un nombre de frame
/// que no existe, este motor no.
fn find_link_target(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> Option<LinkTarget> {
    let mut current = Some(node.clone());
    while let Some(n) = current {
        let guard = n.read().unwrap();
        if let NodeType::Element { tag_name, attributes } = &guard.node_type {
            if tag_name == "a" {
                return attributes.get("href").and_then(|href| {
                    let trimmed = href.trim();
                    let is_navigable = !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.to_ascii_lowercase().starts_with("javascript:");
                    is_navigable.then(|| LinkTarget {
                        href: href.clone(),
                        opens_new_tab: attributes
                            .get("target")
                            .is_some_and(|target| target.trim().eq_ignore_ascii_case("_blank")),
                    })
                });
            }
        }
        let parent = guard.parent.as_ref().and_then(std::sync::Weak::upgrade);
        drop(guard);
        current = parent;
    }
    None
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
                    checked: attributes.contains_key("checked"),
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

    /// Ninguno de estos dos toca la red de verdad (Fase 4.4) - `back`/
    /// `forward` sin ningun historial fallan ANTES de llegar a llamar
    /// `navigate` (early return sobre `history_index: None`), igual que
    /// `history.back()` real no hace nada observable sin nada a lo que
    /// volver, salvo que aqui se reporta el motivo en vez de un no-op
    /// silencioso.
    #[tokio::test]
    async fn back_without_any_history_reports_an_honest_error() {
        let mut server = EngineServer::new();
        let response = server.back(Some("b1".to_string())).await;
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("no hay historial"));
    }

    #[tokio::test]
    async fn forward_without_any_history_reports_an_honest_error() {
        let mut server = EngineServer::new();
        let response = server.forward(Some("f1".to_string())).await;
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("no hay historial"));
    }

    #[test]
    fn state_response_reports_no_history_when_nothing_was_ever_loaded() {
        let server = EngineServer::new();
        let response = server.state_response(None);
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"can_go_back\":false"));
        assert!(json.contains("\"can_go_forward\":false"));
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

    /// `std::mem::forget(dom)` es deliberado, no un descuido: `Node::parent`
    /// es un `Weak` (para no crear un ciclo de referencias fuertes con
    /// `children`, ver su doc-comment en `dom/src/node.rs`) - sin nada que
    /// mantenga vivo AL MENOS el nodo raiz devuelto por `HtmlParser::parse`,
    /// el arbol entero se libera en cuanto esta funcion termina (solo el
    /// nodo devuelto sigue vivo por su propio `Arc`, sus ANCESTROS no, ya
    /// que `dom` era la unica referencia FUERTE al arbol completo) y
    /// `parent.upgrade()` en cualquier ancestro devuelve `None` a partir de
    /// ahi - encontrado en vivo escribiendo los tests de `find_link_href`
    /// (Fase 4.2), el primer uso real de este helper que necesita subir a
    /// un ANCESTRO en vez de solo mirar el propio nodo devuelto. En
    /// produccion esto nunca pasa (`LoadedPage::page::dom_root` mantiene el
    /// arbol entero vivo mientras la pagina este cargada) - es un artefacto
    /// puramente de este helper de test minimalista. Filtrar un `Arc` de
    /// unos pocos nodos por llamada de test es inofensivo en un proceso de
    /// test de corta vida.
    fn find(html: &str, id: &str) -> std::sync::Arc<std::sync::RwLock<Node>> {
        let dom = engine_dom::HtmlParser::parse(html);
        let node = Node::find_by_id(&dom, id).expect("el nodo de prueba deberia existir");
        std::mem::forget(dom);
        node
    }

    #[test]
    fn is_checkable_input_is_true_only_for_checkbox_and_radio() {
        let dom = r#"<html><body><input id="cb" type="checkbox"><input id="rd" type="radio"><input id="txt" type="text"><input id="notype"><textarea id="ta"></textarea></body></html>"#;
        assert!(is_checkable_input(&find(dom, "cb")));
        assert!(is_checkable_input(&find(dom, "rd")));
        assert!(!is_checkable_input(&find(dom, "txt")), "un input de texto no es checkable");
        assert!(!is_checkable_input(&find(dom, "notype")), "sin type, un input NO es checkbox/radio");
        assert!(!is_checkable_input(&find(dom, "ta")), "un textarea nunca es checkable");
    }

    #[test]
    fn toggle_checked_flips_the_attributes_presence_each_call() {
        let node = find(r#"<html><body><input id="cb" type="checkbox"></body></html>"#, "cb");
        assert!(!is_checked(&node), "sin el atributo checked de partida, deberia empezar sin marcar");
        toggle_checked(&node);
        assert!(is_checked(&node), "la primera llamada deberia marcarlo");
        toggle_checked(&node);
        assert!(!is_checked(&node), "la segunda llamada deberia desmarcarlo de vuelta");
    }

    #[test]
    fn toggle_checked_starting_pre_checked_unchecks_first() {
        let node = find(r#"<html><body><input id="cb" type="checkbox" checked></body></html>"#, "cb");
        assert!(is_checked(&node), "el HTML ya trae el atributo checked puesto");
        toggle_checked(&node);
        assert!(!is_checked(&node));
    }

    fn is_checked(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> bool {
        let guard = node.read().unwrap();
        matches!(&guard.node_type, NodeType::Element { attributes, .. } if attributes.contains_key("checked"))
    }

    #[test]
    fn backspace_control_value_removes_only_the_last_character() {
        let node = find(r#"<html><body><input id="txt" type="text" value="hola"></body></html>"#, "txt");
        backspace_control_value(&node);
        let guard = node.read().unwrap();
        let NodeType::Element { attributes, .. } = &guard.node_type else { unreachable!() };
        assert_eq!(attributes.get("value").map(String::as_str), Some("hol"));
    }

    #[test]
    fn backspace_control_value_on_an_empty_value_is_a_harmless_no_op() {
        let node = find(r#"<html><body><input id="txt" type="text" value=""></body></html>"#, "txt");
        backspace_control_value(&node);
        let guard = node.read().unwrap();
        let NodeType::Element { attributes, .. } = &guard.node_type else { unreachable!() };
        assert_eq!(attributes.get("value").map(String::as_str), Some(""));
    }

    #[test]
    fn is_text_control_excludes_checkboxes_radios_and_buttons_but_includes_textarea_and_plain_input() {
        let dom = r#"<html><body>
            <input id="txt" type="text">
            <input id="notype">
            <input id="cb" type="checkbox">
            <input id="rd" type="radio">
            <input id="btn" type="button">
            <textarea id="ta"></textarea>
        </body></html>"#;
        assert!(is_text_control(&find(dom, "txt")));
        assert!(is_text_control(&find(dom, "notype")), "sin type, el valor por defecto real de <input> es text");
        assert!(is_text_control(&find(dom, "ta")));
        assert!(!is_text_control(&find(dom, "cb")));
        assert!(!is_text_control(&find(dom, "rd")));
        assert!(!is_text_control(&find(dom, "btn")));
    }

    #[test]
    fn find_link_target_matches_the_anchor_itself() {
        let node = find(r#"<html><body><a id="link" href="/pagina">texto</a></body></html>"#, "link");
        let target = find_link_target(&node).expect("deberia encontrar el enlace");
        assert_eq!(target.href, "/pagina");
        assert!(!target.opens_new_tab);
    }

    /// El punto real de la Fase 4.2: un clic real casi siempre aterriza en
    /// un DESCENDIENTE del `<a>` (el nodo de texto, o un `<b>` decorativo
    /// dentro), nunca en el propio `<a>` - `find_link_target` tiene que subir
    /// por los ancestros para encontrarlo.
    #[test]
    fn find_link_target_climbs_from_a_descendant_up_to_the_ancestor_anchor() {
        let node = find(r#"<html><body><a id="link" href="/pagina"><b id="bold">texto</b></a></body></html>"#, "bold");
        let target = find_link_target(&node).expect("deberia encontrar el enlace");
        assert_eq!(target.href, "/pagina");
    }

    #[test]
    fn find_link_target_is_none_when_there_is_no_ancestor_anchor() {
        let node = find(r#"<html><body><div id="plain">texto</div></body></html>"#, "plain");
        assert!(find_link_target(&node).is_none());
    }

    #[test]
    fn find_link_target_is_none_for_an_anchor_without_a_navigable_href() {
        let dom = r##"<html><body>
            <a id="nohref">sin href</a>
            <a id="empty" href="">vacio</a>
            <a id="fragment" href="#seccion">ancla interna</a>
            <a id="js" href="javascript:alert(1)">pseudo-protocolo</a>
        </body></html>"##;
        assert!(find_link_target(&find(dom, "nohref")).is_none(), "un <a> sin href no es un hyperlink real");
        assert!(find_link_target(&find(dom, "empty")).is_none());
        assert!(find_link_target(&find(dom, "fragment")).is_none(), "un ancla interna deberia hacer scroll, no navegar - sin implementar todavia");
        assert!(find_link_target(&find(dom, "js")).is_none(), "javascript: ejecutaria script, no navegaria - sin implementar todavia");
    }

    /// `target="_blank"` (Fase 4.5) - comparacion insensible a mayusculas,
    /// igual que un navegador real; cualquier otro valor de `target` navega
    /// en la misma pestaña (ver el doc-comment de `find_link_target`).
    #[test]
    fn find_link_target_reports_opens_new_tab_only_for_target_blank() {
        let dom = r#"<html><body>
            <a id="blank" href="/otra" target="_blank">nueva pestaña</a>
            <a id="upper" href="/otra" target="_BLANK">nueva pestaña mayusculas</a>
            <a id="self" href="/otra" target="_self">misma pestaña</a>
            <a id="notarget" href="/otra">sin target</a>
        </body></html>"#;
        assert!(find_link_target(&find(dom, "blank")).unwrap().opens_new_tab);
        assert!(find_link_target(&find(dom, "upper")).unwrap().opens_new_tab);
        assert!(!find_link_target(&find(dom, "self")).unwrap().opens_new_tab);
        assert!(!find_link_target(&find(dom, "notarget")).unwrap().opens_new_tab);
    }

    #[tokio::test]
    async fn open_new_tab_creates_a_second_tab_and_makes_it_active() {
        let mut server = EngineServer::new();
        assert_eq!(server.tabs.len(), 1);
        let first_tab_id = server.tabs[0].id;

        let response = server.open_new_tab(Some("nt1".to_string()), None).await;
        assert_eq!(server.tabs.len(), 2);
        assert_ne!(server.active_tab().id, first_tab_id);

        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains(&format!("\"tab_id\":{}", server.active_tab().id)));
    }

    #[test]
    fn close_tab_refuses_to_close_the_only_tab() {
        let mut server = EngineServer::new();
        let only_id = server.tabs[0].id;
        let response = server.close_tab(Some("ct1".to_string()), only_id);
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"error\""));
        assert_eq!(server.tabs.len(), 1, "la pestaña no deberia haberse cerrado");
    }

    #[test]
    fn close_tab_reports_an_error_for_an_unknown_tab_id() {
        let mut server = EngineServer::new();
        server.tabs.push(Tab::new(99));
        let response = server.close_tab(Some("ct2".to_string()), 12345);
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"error\""));
    }

    #[test]
    fn closing_the_active_tab_activates_the_tab_to_its_left() {
        let mut server = EngineServer::new();
        server.tabs.push(Tab::new(1));
        server.tabs.push(Tab::new(2));
        server.active_tab = 2; // pestaña id=2 activa, la de mas a la derecha

        server.close_tab(Some("ct3".to_string()), 2);

        assert_eq!(server.tabs.len(), 2);
        assert_eq!(server.active_tab().id, 1, "deberia activar la pestaña inmediatamente a la izquierda");
    }

    #[test]
    fn closing_the_leftmost_active_tab_activates_the_new_first_tab() {
        let mut server = EngineServer::new();
        server.tabs.push(Tab::new(1));
        let closing_id = server.tabs[0].id;
        server.active_tab = 0;

        server.close_tab(Some("ct4".to_string()), closing_id);

        assert_eq!(server.tabs.len(), 1);
        assert_eq!(server.active_tab, 0);
        assert_eq!(server.active_tab().id, 1);
    }

    #[test]
    fn closing_a_background_tab_keeps_the_active_tab_selected() {
        let mut server = EngineServer::new();
        let background_id = server.tabs[0].id;
        server.tabs.push(Tab::new(1));
        server.active_tab = 1; // pestaña id=1 activa

        server.close_tab(Some("ct5".to_string()), background_id);

        assert_eq!(server.tabs.len(), 1);
        assert_eq!(server.active_tab().id, 1, "cerrar una pestaña en segundo plano no deberia cambiar la activa");
    }

    #[test]
    fn switch_tab_reports_an_error_for_an_unknown_tab_id() {
        let mut server = EngineServer::new();
        let response = server.switch_tab(Some("st1".to_string()), 99999);
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"error\""));
    }

    #[test]
    fn switch_tab_changes_the_active_tab() {
        let mut server = EngineServer::new();
        server.tabs.push(Tab::new(1));
        let response = server.switch_tab(Some("st2".to_string()), 1);
        assert_eq!(server.active_tab().id, 1);
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"tab_id\":1"));
    }

    #[test]
    fn list_tabs_reports_every_open_tab_with_the_active_one_flagged() {
        let mut server = EngineServer::new();
        server.tabs.push(Tab::new(1));
        let response = server.list_tabs(Some("lt1".to_string()));
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"tabs\""));
        assert!(json.contains(&format!("\"active_tab_id\":{}", server.active_tab().id)));
    }
}
