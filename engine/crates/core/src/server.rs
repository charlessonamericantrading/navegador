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
use engine_js::{BoxMetrics, JsRuntime};
use engine_layout::{ImageMap, LayoutBox, LayoutTreeBuilder};
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

impl LoadedPage {
    /// Rehace el arbol de layout con el tamaño de ventana dado y publica el
    /// resultado donde JS puede verlo. Es el bloque que antes estaba
    /// copiado literalmente en seis sitios (`resize`, `switch_tab`,
    /// `click`, `type_text`, `press_key`, `relayout_active_tab`); tenerlo
    /// en uno solo es lo que garantiza que el snapshot de la Fase 8 se
    /// publique SIEMPRE que el layout cambie, sin depender de acordarse en
    /// cada sitio nuevo.
    ///
    /// NO toca el scroll: rehacer el layout no mueve al usuario. Quien
    /// necesite reajustarlo (porque el contenido encogio) llama despues a
    /// `publish_scroll_offset` con el valor ya acotado.
    fn relayout(&mut self, width: f32, height: f32) {
        self.page.layout_root = LayoutTreeBuilder::build(
            &self.page.dom_root,
            &self.page.stylesheet,
            width,
            height,
            self.font_set.as_ref(),
            &self.images,
        );
        self.publish_layout_snapshot();
    }

    /// Copia la geometria y el estilo resuelto de cada caja al buzon que
    /// leen `getComputedStyle`/`getBoundingClientRect` (Fase 8, ver
    /// `engine_js::cssom`). No-op si este runtime no tiene DOM enlazado -
    /// entonces tampoco tiene esas dos funciones registradas y no hay nadie
    /// a quien publicarle nada.
    ///
    /// Conserva el `scroll_offset_y` que hubiera: un relayout no mueve el
    /// scroll (ver `relayout`).
    fn publish_layout_snapshot(&self) {
        let Some(snapshot) = self.runtime.layout_snapshot() else { return };
        let Ok(mut data) = snapshot.write() else { return };
        data.boxes.clear();
        collect_box_metrics(&self.page.layout_root, &mut data.boxes);
    }

    /// Actualiza SOLO el desplazamiento del snapshot. Separado de
    /// `publish_layout_snapshot` porque hacer scroll no cambia la geometria
    /// de ninguna caja - solo la relacion entre documento y viewport - y
    /// `getBoundingClientRect` devuelve coordenadas de viewport: sin esto,
    /// un rect leido despues de un scroll estaria desplazado justo lo que
    /// el usuario bajo. Recorrer el arbol entero en cada evento de rueda
    /// para actualizar un solo `f32` seria desperdiciarlo.
    fn publish_scroll_offset(&self, scroll_offset_y: f32) {
        let Some(snapshot) = self.runtime.layout_snapshot() else { return };
        let Ok(mut data) = snapshot.write() else { return };
        data.scroll_offset_y = scroll_offset_y;
    }
}

/// Aplana el arbol de layout a la lista de `(nodo, metricas)` que espera el
/// snapshot. Solo entran las cajas CON nodo del DOM detras: las de texto y
/// la raiz sintetica no corresponden a ningun elemento al que JS pueda
/// llegar (misma regla que `LayoutBox::hit_test`).
///
/// Aqui es donde se paga la copia que `engine_js::cssom` declara: un clon
/// del `computed_style` por caja. Se hace en `core` y no en `layout` porque
/// `BoxMetrics` es un tipo de `engine-js`, y es `core` - que depende de los
/// dos - el unico sitio donde las dos capas pueden encontrarse sin crear
/// una dependencia nueva entre ellas.
fn collect_box_metrics(layout: &LayoutBox, out: &mut Vec<(std::sync::Arc<std::sync::RwLock<Node>>, BoxMetrics)>) {
    if let Some(node) = &layout.dom_node {
        out.push((
            node.clone(),
            BoxMetrics {
                x: layout.dimensions.x,
                y: layout.dimensions.y,
                width: layout.dimensions.width,
                height: layout.dimensions.height,
                computed_style: layout.computed_style.clone(),
            },
        ));
    }
    for child in &layout.children {
        collect_box_metrics(child, out);
    }
}

/// Pestaña (Fase 4.5) - agrupa TODO lo que ya era, antes de esta fase,
/// estado directo de `EngineServer` y que en realidad pertenece a una
/// sesion de navegacion concreta, no a la ventana entera: la pagina
/// cargada, su propio historial atras/adelante (Fase 4.4) y su propio
/// desplazamiento vertical. `width`/`height` SI se quedan en
/// `EngineServer` - son el tamaño de la VENTANA, compartido por todas las
/// pestañas (igual que un navegador real: cambiar de pestaña no cambia el
/// tamaño de la ventana).
/// Una entrada del historial (Fase 7 - antes era un `String` suelto con la
/// URL). `document_id` identifica QUE CARGA DE DOCUMENTO creo esta entrada,
/// y es lo que permite distinguir los dos tipos de navegacion del historial
/// que el spec trata de forma completamente distinta:
///
/// - **Entre documentos** (entradas con `document_id` distinto al del
///   documento vivo): volver ahi exige pedir la pagina otra vez y
///   reconstruirlo todo, como hasta la Fase 4.4.
/// - **Dentro del mismo documento** (entradas creadas por
///   `history.pushState`, que heredan el `document_id` del documento
///   vivo): volver ahi NO recarga nada - solo cambia la URL y dispara
///   `popstate` sobre el runtime que ya esta corriendo. Es exactamente lo
///   que hace que una SPA funcione: sin esto, "atras" en una SPA recargaria
///   la pagina entera y perderia todo su estado.
#[derive(Debug, Clone)]
struct HistoryEntry {
    url: String,
    document_id: u64,
}

struct Tab {
    id: u32,
    current_page: Option<LoadedPage>,
    history: Vec<HistoryEntry>,
    history_index: Option<usize>,
    scroll_offset_y: f32,
    /// El `document_id` del documento ACTUALMENTE cargado en esta pestaña -
    /// con que se comparan las entradas del historial para decidir si una
    /// vuelta atras es dentro del mismo documento o entre documentos.
    /// `0` antes de la primera carga (ningun documento real tiene ese id:
    /// `EngineServer::next_document_id` empieza en 1).
    document_id: u64,
}

impl Tab {
    fn new(id: u32) -> Self {
        Self {
            id,
            current_page: None,
            history: Vec::new(),
            history_index: None,
            scroll_offset_y: 0.0,
            document_id: 0,
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
    /// Contador de cargas de documento (Fase 7) - monotono y compartido por
    /// todas las pestañas, para que dos documentos nunca compartan id.
    /// Empieza en 1 para que el `0` de `Tab::document_id` signifique
    /// inequivocamente "todavia no se ha cargado nada aqui".
    next_document_id: u64,
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
            next_document_id: 1,
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
                    page.relayout(w as f32, h as f32);
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
                    let scrolled = clamp_scroll_offset(tab.scroll_offset_y + dy as f32, content_extent, h as f32);
                    // Fase 8: `getBoundingClientRect` devuelve coordenadas
                    // de VIEWPORT, asi que el snapshot necesita saber
                    // cuanto se ha desplazado el documento.
                    page.publish_scroll_offset(scrolled);
                    tab.scroll_offset_y = scrolled;
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
        let html = response.text();

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
        let (page, mut runtime) = build_page_keeping_runtime(
            &html,
            &external_css,
            self.width as f32,
            self.height as f32,
            Some(&font_set),
            &external_scripts,
            &images,
            Some(self.network.clone()),
        );
        // Fase 6.4: un `window.open()` llamado durante la CARGA de la pagina
        // (no desde un clic real del usuario) se descarta - mismo criterio
        // que el bloqueador de ventanas emergentes de cualquier navegador
        // real, que solo permite abrir ventanas con "activacion del
        // usuario" de por medio. No es una limitacion tecnica disfrazada:
        // honrarlas ademas abriria la puerta a que una pagina que llama
        // `window.open` al cargar se abriera a si misma en bucle, ya que
        // cada pestaña nueva vuelve a pasar por aqui.
        let discarded = runtime.take_pending_window_opens();
        if !discarded.is_empty() {
            tracing::info!(
                "[server] {} window.open() durante la carga de la pagina ignorados (hace falta un clic real, igual que un bloqueador de popups)",
                discarded.len()
            );
        }
        let title = Node::find_all_by_tag(&page.dom_root, "title")
            .first()
            .map(Node::text_content)
            .unwrap_or_default();

        // Fase 7: cada carga real de documento estrena identidad. Es lo que
        // luego permite a `back`/`forward` distinguir una entrada de ESTE
        // documento (creada por `pushState`, no hay que recargar nada) de
        // una de otro (hay que volver a pedirla).
        let document_id = self.next_document_id;
        self.next_document_id += 1;

        let tab = self.active_tab_mut();
        tab.document_id = document_id;
        if record_history {
            let next_index = tab.history_index.map(|i| i + 1).unwrap_or(0);
            tab.history.truncate(next_index);
            tab.history.push(HistoryEntry { url: final_url.clone(), document_id });
            tab.history_index = Some(next_index);
        } else if let Some(index) = tab.history_index {
            // Llegamos aqui desde `back`/`forward` ENTRE documentos: la
            // entrada a la que se ha vuelto acaba de ser servida por una
            // carga nueva, asi que su identidad de documento es la nueva.
            // Sin esto, la entrada seguiria apuntando al documento viejo
            // (ya inexistente) y un `pushState` posterior sobre ella se
            // consideraria de otro documento, forzando recargas absurdas.
            if let Some(entry) = tab.history.get_mut(index) {
                entry.document_id = document_id;
            }
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
        // Fase 8: el PRIMER snapshot del documento recien cargado.
        // `build_page_keeping_runtime` ya construyo su arbol de layout, y
        // hasta que esto corra los scripts de la pagina solo han podido ver
        // el snapshot vacio (ver `engine_js::cssom`); a partir de aqui -
        // que es cuando empiezan a llegar clics - los valores son reales.
        if let Some(page) = &tab.current_page {
            page.publish_layout_snapshot();
            page.publish_scroll_offset(0.0);
        }
        // Fase 7: un `history.replaceState` en un script de CARGA (patron
        // habitual en SPAs para normalizar la ruta inicial) se aplica aqui,
        // sobre la entrada que este mismo `navigate` acaba de crear. A
        // diferencia de `window.open`, esto SI se honra en la carga: no
        // abre nada ni navega a ningun sitio, solo reescribe la URL de una
        // entrada que ya existe, asi que no hay riesgo de bucle.
        let load_time_ops = self
            .active_tab_mut()
            .current_page
            .as_mut()
            .map(|page| page.runtime.take_pending_history_ops())
            .unwrap_or_default();
        self.apply_history_ops(load_time_ops);
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
        self.traverse_history(id, previous_index).await
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
        self.traverse_history(id, next_index).await
    }

    /// El nucleo compartido de `back`/`forward` (Fase 7) - decide cual de
    /// los dos tipos de travesia toca, que el spec trata de forma
    /// radicalmente distinta (ver el doc-comment de `HistoryEntry`):
    ///
    /// - **Dentro del MISMO documento** (la entrada destino nacio de un
    ///   `history.pushState` sobre el documento que sigue vivo): NO se
    ///   recarga nada. Solo cambia la URL y se dispara `popstate` sobre el
    ///   runtime que ya esta corriendo, que es lo que permite a una SPA
    ///   repintar su vista conservando todo su estado en memoria. Antes de
    ///   la Fase 7 esto no existia: `back` SIEMPRE volvia a pedir la pagina
    ///   por red, lo que en una SPA equivale a perder la sesion entera.
    /// - **Entre documentos**: como hasta ahora, se vuelve a pedir la
    ///   pagina de verdad (`navigate` con `record_history: false`).
    async fn traverse_history(&mut self, id: Option<String>, target_index: usize) -> EngineResponse {
        let tab = self.active_tab();
        let entry = tab.history[target_index].clone();
        let same_document = entry.document_id == tab.document_id && tab.current_page.is_some();

        if !same_document {
            self.active_tab_mut().history_index = Some(target_index);
            return self.navigate(id, entry.url, false).await;
        }

        let tab = self.active_tab_mut();
        tab.history_index = Some(target_index);
        if let Some(page) = &mut tab.current_page {
            page.url = entry.url;
        }
        if let Err(error) = self.fire_popstate() {
            return Self::error(id, format!("popstate_error: {error}"));
        }
        // Un listener de `popstate` casi siempre repinta: hay que rehacer
        // el layout antes de devolver la captura, o el estado devuelto
        // seria el de ANTES de que la SPA reaccionara.
        self.relayout_active_tab();
        self.state_response(id)
    }

    /// Dispara `popstate` sobre el elemento raiz (Fase 7). El objetivo real
    /// del spec es `window`, que no es un nodo y por tanto no existe en el
    /// registro de eventos de este motor (indexado por nodo del DOM) - se
    /// usa el elemento raiz porque es el ultimo escalon de propagacion
    /// ANTES de `window`, y porque `window.addEventListener` esta enganchado
    /// justo ahi (ver el shim en `engine_js::history`), de modo que un
    /// `window.addEventListener('popstate', ...)` corriente lo recibe.
    ///
    /// `event.state` es siempre `null`: el argumento `state` de `pushState`
    /// no se guarda, y no puede guardarse de forma honesta mientras no haya
    /// bfcache - ver el doc-comment de `engine_js::history`.
    fn fire_popstate(&mut self) -> Result<(), engine_js::JsError> {
        let Some(page) = &mut self.tabs[self.active_tab].current_page else { return Ok(()) };
        let Some(root_element) = Node::find_all_by_tag(&page.page.dom_root, "html").into_iter().next() else {
            return Ok(());
        };
        page.runtime.dispatch_event(&root_element, "popstate").map(|_| ())
    }

    /// Rehace el arbol de layout de la pestaña activa con el tamaño de
    /// ventana actual - el mismo bloque que ya repetian `click`/`type_text`/
    /// `press_key`/`switch_tab`, extraido al hacer falta tambien tras un
    /// `popstate` (Fase 7).
    fn relayout_active_tab(&mut self) {
        let (width, height) = (self.width, self.height);
        let tab = &mut self.tabs[self.active_tab];
        if let Some(page) = &mut tab.current_page {
            page.relayout(width as f32, height as f32);
            let content_extent = page.page.layout_root.content_extent();
            tab.scroll_offset_y = clamp_scroll_offset(tab.scroll_offset_y, content_extent, height as f32);
        }
    }

    /// Aplica las `history.pushState`/`replaceState` que JS haya pedido
    /// (Fase 7). Las URLs se resuelven contra la de la pagina actual (un
    /// `pushState(null, '', '/ruta')` relativo es lo normal en una SPA), y
    /// la entrada resultante hereda el `document_id` VIVO - que es
    /// precisamente lo que la marca como "misma pagina" para que un `back`
    /// posterior no recargue (ver `traverse_history`).
    ///
    /// `pushState` ademas TRUNCA las entradas "adelante", igual que una
    /// navegacion normal: el spec lo exige y sin ello un `forward` posterior
    /// llevaria a una entrada que ya no pertenece a esta linea de historia.
    fn apply_history_ops(&mut self, ops: Vec<engine_js::history::HistoryOp>) {
        if ops.is_empty() {
            return;
        }
        let document_id = self.active_tab().document_id;
        for op in ops {
            let (raw_url, is_push) = match op {
                engine_js::history::HistoryOp::Push(url) => (url, true),
                engine_js::history::HistoryOp::Replace(url) => (url, false),
            };
            let tab = &mut self.tabs[self.active_tab];
            let Some(page) = &mut tab.current_page else { continue };
            let Ok(base) = url::Url::parse(&page.url) else { continue };
            let Ok(resolved) = base.join(&raw_url) else {
                tracing::warn!("[server] URL de history.pushState/replaceState invalida, se ignora: {raw_url}");
                continue;
            };
            let resolved = resolved.to_string();
            page.url = resolved.clone();
            let entry = HistoryEntry { url: resolved, document_id };
            match (is_push, tab.history_index) {
                (true, Some(index)) => {
                    tab.history.truncate(index + 1);
                    tab.history.push(entry);
                    tab.history_index = Some(index + 1);
                }
                (true, None) => {
                    tab.history.push(entry);
                    tab.history_index = Some(0);
                }
                (false, Some(index)) => tab.history[index] = entry,
                // `replaceState` sin ninguna entrada que sustituir: se
                // comporta como la primera entrada, que es lo que habria
                // si la pagina se hubiera cargado normalmente.
                (false, None) => {
                    tab.history.push(entry);
                    tab.history_index = Some(0);
                }
            }
        }
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
            page.relayout(w as f32, h as f32);
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
                Ok(response) if response.is_success() => {
                    let css = response.text();
                    combined.push_str(&css);
                    combined.push('\n');
                }
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
                Ok(response) if response.is_success() => {
                    let js = response.text();
                    fetched.insert(src, js);
                }
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
        // Fase 6.1: `#seccion` se resuelve a un desplazamiento real DESPUES
        // del relayout de mas abajo (las posiciones tienen que estar al dia
        // - un listener de "click", o el propio `javascript:` de la Fase
        // 6.2, pueden haber movido el elemento destino).
        let mut fragment_target: Option<String> = None;
        let mut scroll_target: Option<f32> = None;
        // Fase 6.4: URLs que un `window.open(...)` de este mismo clic pidio
        // abrir. Se recogen dentro del prestamo de la pagina pero se
        // atienden FUERA (abrir una pestaña reasigna la pestaña activa
        // entera), igual que la navegacion por enlace de la Fase 4.2.
        let mut window_opens: Vec<String> = Vec::new();
        // Fase 7: idem para `history.pushState`/`replaceState`.
        let mut history_ops: Vec<engine_js::history::HistoryOp> = Vec::new();
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
                    apply_checkable_click(&page.page.dom_root, &node);
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
                    match find_link_target(&node) {
                        Some(LinkAction::Navigate { href, opens_new_tab: blank }) => {
                            if let Ok(page_url) = url::Url::parse(&page.url) {
                                if let Ok(resolved) = page_url.join(&href) {
                                    link_target = Some(resolved.to_string());
                                    opens_new_tab = blank;
                                }
                            }
                        }
                        // Fase 6.2: se ejecuta AQUI, antes del relayout de
                        // mas abajo, precisamente para que una mutacion del
                        // DOM hecha por este script se vea reflejada en la
                        // captura que devuelve este mismo clic.
                        Some(LinkAction::RunScript(script)) => {
                            if let Err(error) = page.runtime.eval(&script) {
                                // Un error dentro de un `javascript:` NO
                                // aborta el clic entero (el resto del clic
                                // -sus eventos, su relayout- ya ocurrio y es
                                // real): se reporta y se sigue, igual que un
                                // navegador real deja el error en la consola
                                // sin romper la pagina.
                                tracing::warn!("[server] error ejecutando un href=javascript:: {error}");
                            }
                        }
                        Some(LinkAction::ScrollToFragment(fragment)) => {
                            fragment_target = Some(fragment);
                        }
                        None => {}
                    }
                }
                page.relayout(width as f32, height as f32);
                if let Some(fragment) = fragment_target {
                    scroll_target = fragment_scroll_offset(page, &fragment, height as f32);
                }
                // Fase 6.4: aqui SI se honran (a diferencia de la carga de
                // pagina, ver `navigate`) - este es exactamente el caso con
                // "activacion del usuario" real que un navegador permite.
                // Incluye tanto los `window.open` de un listener de "click"
                // como los de un `href="javascript:window.open(...)"`
                // ejecutado justo arriba (Fase 6.2).
                window_opens = page.runtime.take_pending_window_opens();
                // Fase 7: un listener de "click" que llame a
                // `history.pushState` (el caso normal en una SPA: pulsar un
                // enlace de navegacion interna) deja aqui su operacion.
                history_ops = page.runtime.take_pending_history_ops();
            }
        }
        self.apply_history_ops(history_ops);
        // Fase 6.1: fuera del prestamo de `page` (que sale de `tab`), para
        // poder escribir el scroll DEL TAB. `None` (fragmento que no
        // corresponde a ningun `id` real) deja el scroll como estaba, igual
        // que un navegador real ante un ancla rota: no es un error.
        if let Some(offset) = scroll_target {
            self.active_tab_mut().scroll_offset_y = offset;
        }
        // ORDEN (Fase 6.4): primero la navegacion por enlace, que le toca a
        // la pestaña ACTUAL, y solo despues los `window.open`, que crean
        // pestañas nuevas y se llevan el foco. Al reves, la navegacion
        // acabaria aplicandose a la pestaña recien abierta en vez de a la
        // que el usuario clico. El estado final (pestaña original navegada
        // + popup enfocado) es el mismo que produce un navegador real
        // cuando un clic hace las dos cosas a la vez.
        let mut response = match link_target {
            // `target="_blank"` (Fase 4.5, antes NO implementado - ver el
            // doc-comment de `find_link_target`) abre una pestaña nueva en
            // vez de navegar la pestaña actual, igual que un navegador
            // real.
            Some(target_url) if opens_new_tab => self.open_new_tab(id.clone(), Some(target_url)).await,
            Some(target_url) => self.navigate(id.clone(), target_url, true).await,
            None => self.state_response(id.clone()),
        };
        for url in window_opens {
            response = self.open_new_tab(id.clone(), Some(url)).await;
        }
        response
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
        page.relayout(width as f32, height as f32);
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
            page.relayout(width as f32, height as f32);
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
/// por eso se inserta la cadena vacia, no `"true"`/`"checked"`).
///
/// Esto es el comportamiento de un CHECKBOX. Un `radio` NO se conmuta
/// (clicar uno ya marcado lo deja marcado) y ademas desmarca a su grupo -
/// ver `apply_checkable_click`, que es quien decide cual de los dos
/// comportamientos aplica.
fn toggle_checked(node: &std::sync::Arc<std::sync::RwLock<Node>>) {
    let mut node_guard = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut node_guard.node_type {
        if attributes.remove("checked").is_none() {
            attributes.insert("checked".to_string(), String::new());
        }
    }
}

/// Escribe `checked` a un valor CONCRETO (no lo conmuta) - la primitiva
/// que necesita el comportamiento de grupo de los radio (Fase 6.3).
fn set_checked(node: &std::sync::Arc<std::sync::RwLock<Node>>, checked: bool) {
    let mut node_guard = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut node_guard.node_type {
        if checked {
            attributes.insert("checked".to_string(), String::new());
        } else {
            attributes.remove("checked");
        }
    }
}

/// El `name` de un `<input>`, si lo tiene y no esta vacio - la clave que
/// agrupa a los radio entre si (Fase 6.3).
fn input_name(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> Option<String> {
    let guard = node.read().unwrap();
    match &guard.node_type {
        NodeType::Element { attributes, .. } => attributes
            .get("name")
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        _ => None,
    }
}

fn is_radio(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> bool {
    let guard = node.read().unwrap();
    matches!(
        &guard.node_type,
        NodeType::Element { tag_name, attributes } if tag_name == "input"
            && attributes.get("type").map(String::as_str) == Some("radio")
    )
}

/// La accion por defecto real de clicar un checkbox o un radio (Fase 6.3) -
/// antes de esta fase, AMBOS se limitaban a conmutar su propio `checked`
/// (`toggle_checked`), lo que era correcto para un checkbox pero falso
/// para un radio en dos cosas a la vez:
///
/// 1. **Un radio no se conmuta**: clicar uno YA marcado lo deja marcado
///    (en el spec real no hay forma de desmarcar un radio clicandolo; solo
///    marcando a otro de su grupo). Antes, un segundo clic lo desmarcaba,
///    dejando el grupo entero sin ninguna opcion seleccionada - un estado
///    que un formulario real no puede alcanzar por clics.
/// 2. **Un radio desmarca a su grupo**: los demas `input[type=radio]` con
///    el MISMO `name` pierden su `checked`.
///
/// El grupo se busca en el documento ENTERO, no dentro del `<form>` que
/// contenga al radio - simplificacion declarada: el spec real agrupa por
/// "form owner", asi que dos formularios distintos en la misma pagina que
/// reutilicen el mismo `name` se pisarian entre si aqui y no deberian.
/// Poco comun en paginas reales (reutilizar el mismo `name` en dos
/// formularios de la misma pagina es raro y casi siempre un error), y
/// arreglarlo exige un concepto de "form owner" que este motor todavia no
/// tiene.
///
/// Un radio SIN `name` (o con el `name` vacio) no forma grupo con nadie:
/// se marca el solo, sin tocar a ningun otro - igual que el spec real,
/// donde el grupo se define precisamente por ese atributo.
fn apply_checkable_click(
    dom_root: &std::sync::Arc<std::sync::RwLock<Node>>,
    node: &std::sync::Arc<std::sync::RwLock<Node>>,
) {
    if !is_radio(node) {
        toggle_checked(node);
        return;
    }
    if let Some(group) = input_name(node) {
        for other in Node::find_all_by_tag(dom_root, "input") {
            if std::sync::Arc::ptr_eq(&other, node) {
                continue;
            }
            if is_radio(&other) && input_name(&other).as_deref() == Some(group.as_str()) {
                set_checked(&other, false);
            }
        }
    }
    set_checked(node, true);
}

/// A que desplazamiento vertical hay que ir para un `href="#fragmento"`
/// (Fase 6.1). `None` cuando el fragmento no corresponde a ningun elemento
/// real (ancla rota) o cuando ese elemento no produjo ninguna caja de
/// layout (`display: none`): el llamador deja el scroll como estaba, igual
/// que un navegador real, que tampoco lo trata como un error.
///
/// `#` a secas (fragmento vacio) va al principio del documento, como en el
/// spec real - no es un caso degenerado sino un patron comun ("volver
/// arriba").
///
/// El destino se acota con el mismo `clamp_scroll_offset` que la rueda del
/// raton: un ancla cerca del final del documento no puede desplazar mas
/// alla del final real del contenido.
fn fragment_scroll_offset(page: &LoadedPage, fragment: &str, viewport_height: f32) -> Option<f32> {
    if fragment.is_empty() {
        return Some(0.0);
    }
    let node = Node::find_by_id(&page.page.dom_root, fragment)?;
    let target_box = page.page.layout_root.find_box_for_node(&node)?;
    Some(clamp_scroll_offset(
        target_box.dimensions.y,
        page.page.layout_root.content_extent(),
        viewport_height,
    ))
}

/// Que deberia HACER un clic sobre un `<a>` (Fase 6.1/6.2) - hasta la Fase
/// 6 esto era solo `Option<LinkTarget>`, "navegar o no hacer nada", porque
/// las anclas internas y `javascript:` estaban declaradas como NO
/// implementadas y se descartaban en `find_link_target` como si no fueran
/// enlaces navegables. Ahora cada una tiene su accion real propia.
enum LinkAction {
    /// Navegar a `href` (posiblemente relativo, SIN resolver todavia - eso
    /// es trabajo del llamador, que es quien conoce la URL de la pagina
    /// actual). `opens_new_tab` es `target="_blank"` (Fase 4.5).
    Navigate { href: String, opens_new_tab: bool },
    /// Ancla interna `href="#seccion"` (Fase 6.1) - desplazar el scroll
    /// hasta el elemento con ese `id`, sin ninguna peticion de red. El
    /// `String` es el fragmento YA sin la `#`; vacio para `href="#"` a
    /// secas, que en el spec real significa "al principio del documento".
    ScrollToFragment(String),
    /// `href="javascript:..."` (Fase 6.2) - ejecutar el resto como script
    /// en el runtime de la pagina actual, sin navegar a ningun sitio. El
    /// `String` es el codigo YA sin el prefijo `javascript:`.
    RunScript(String),
}

/// Busca el `<a href="...">` NAVEGABLE mas cercano empezando en `node` e
/// INCLUYENDOLO (Fase 4.2) - un clic real casi siempre aterriza en un
/// DESCENDIENTE del enlace (el nodo de texto, un `<b>`/`<span>` decorativo
/// dentro de el), nunca en el propio `<a>`, asi que hay que subir por
/// `parent` hasta encontrar uno. Se DETIENE en el primer `<a>` que
/// encuentra, tenga `href` navegable o no (un `<a>` sin `href`, o anidado
/// dentro de otro `<a>` - HTML invalido de todas formas - no deberia
/// seguir subiendo mas alla buscando un enlace exterior). Lo unico que
/// sigue sin producir NINGUNA accion es un `href` vacio o ausente: un `<a>`
/// sin `href` no es ni siquiera un hyperlink real, el spec lo llama
/// "enlace de marcador de posicion".
///
/// Las tres acciones reales que puede devolver (ver `LinkAction`):
/// - `href="#seccion"` -> `ScrollToFragment` (Fase 6.1, antes se descartaba
///   como "no navegable").
/// - `href="javascript:..."` -> `RunScript` (Fase 6.2, idem).
/// - cualquier otra cosa -> `Navigate`, con `opens_new_tab` = `true` si el
///   MISMO `<a>` lleva `target="_blank"` (Fase 4.5, comparacion insensible
///   a mayusculas como hacen los navegadores reales). Cualquier OTRO valor
///   de `target` (`"_self"`, `"_parent"`, un nombre de frame inventado...)
///   navega en la misma pestaña - simplificacion declarada: un navegador
///   real abriria pestaña nueva para un nombre de frame que no existe,
///   este motor no.
fn find_link_target(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> Option<LinkAction> {
    let mut current = Some(node.clone());
    while let Some(n) = current {
        let guard = n.read().unwrap();
        if let NodeType::Element { tag_name, attributes } = &guard.node_type {
            if tag_name == "a" {
                return attributes.get("href").and_then(|href| {
                    let trimmed = href.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    if let Some(fragment) = trimmed.strip_prefix('#') {
                        return Some(LinkAction::ScrollToFragment(fragment.to_string()));
                    }
                    if trimmed.len() >= "javascript:".len()
                        && trimmed[.."javascript:".len()].eq_ignore_ascii_case("javascript:")
                    {
                        return Some(LinkAction::RunScript(trimmed["javascript:".len()..].to_string()));
                    }
                    Some(LinkAction::Navigate {
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

    /// Ayuda de test: el `href` de una accion `Navigate`, o `None` si la
    /// accion es otra (ancla/script) o no hay enlace - la mayoria de tests
    /// heredados de la Fase 4.2 solo se preguntan "¿a donde navega?".
    fn navigate_href(action: Option<LinkAction>) -> Option<String> {
        match action {
            Some(LinkAction::Navigate { href, .. }) => Some(href),
            _ => None,
        }
    }

    #[test]
    fn find_link_target_matches_the_anchor_itself() {
        let node = find(r#"<html><body><a id="link" href="/pagina">texto</a></body></html>"#, "link");
        match find_link_target(&node) {
            Some(LinkAction::Navigate { href, opens_new_tab }) => {
                assert_eq!(href, "/pagina");
                assert!(!opens_new_tab);
            }
            other => panic!("deberia navegar, no {}", describe(&other)),
        }
    }

    fn describe(action: &Option<LinkAction>) -> &'static str {
        match action {
            Some(LinkAction::Navigate { .. }) => "Navigate",
            Some(LinkAction::ScrollToFragment(_)) => "ScrollToFragment",
            Some(LinkAction::RunScript(_)) => "RunScript",
            None => "ninguna accion",
        }
    }

    /// El punto real de la Fase 4.2: un clic real casi siempre aterriza en
    /// un DESCENDIENTE del `<a>` (el nodo de texto, o un `<b>` decorativo
    /// dentro), nunca en el propio `<a>` - `find_link_target` tiene que subir
    /// por los ancestros para encontrarlo.
    #[test]
    fn find_link_target_climbs_from_a_descendant_up_to_the_ancestor_anchor() {
        let node = find(r#"<html><body><a id="link" href="/pagina"><b id="bold">texto</b></a></body></html>"#, "bold");
        assert_eq!(navigate_href(find_link_target(&node)), Some("/pagina".to_string()));
    }

    #[test]
    fn find_link_target_is_none_when_there_is_no_ancestor_anchor() {
        let node = find(r#"<html><body><div id="plain">texto</div></body></html>"#, "plain");
        assert!(find_link_target(&node).is_none());
    }

    #[test]
    fn find_link_target_is_none_only_for_an_anchor_without_any_href_at_all() {
        let dom = r##"<html><body>
            <a id="nohref">sin href</a>
            <a id="empty" href="">vacio</a>
        </body></html>"##;
        assert!(find_link_target(&find(dom, "nohref")).is_none(), "un <a> sin href no es un hyperlink real");
        assert!(find_link_target(&find(dom, "empty")).is_none(), "un href vacio tampoco");
    }

    /// Fase 6.1: lo que antes se descartaba como "no navegable" ahora tiene
    /// su propia accion real (scroll), no la ausencia de accion.
    #[test]
    fn find_link_target_reports_a_fragment_anchor_as_a_scroll_not_as_nothing() {
        let dom = r##"<html><body>
            <a id="frag" href="#seccion">ancla interna</a>
            <a id="solohash" href="#">volver arriba</a>
        </body></html>"##;
        match find_link_target(&find(dom, "frag")) {
            Some(LinkAction::ScrollToFragment(fragment)) => assert_eq!(fragment, "seccion", "la # no forma parte del id"),
            other => panic!("deberia ser un scroll a fragmento, no {}", describe(&other)),
        }
        match find_link_target(&find(dom, "solohash")) {
            Some(LinkAction::ScrollToFragment(fragment)) => {
                assert!(fragment.is_empty(), "href=\"#\" a secas significa principio del documento");
            }
            other => panic!("deberia ser un scroll a fragmento, no {}", describe(&other)),
        }
    }

    /// Fase 6.2, idem: `javascript:` pasa de descartarse a ser una accion
    /// real. El prefijo se reconoce sin importar mayusculas (igual que un
    /// navegador real) y NO forma parte del codigo devuelto.
    #[test]
    fn find_link_target_reports_a_javascript_url_as_a_script_to_run() {
        let dom = r#"<html><body>
            <a id="js" href="javascript:hazAlgo(1)">pseudo-protocolo</a>
            <a id="mixed" href="JavaScript:hazAlgo(2)">mayusculas mezcladas</a>
        </body></html>"#;
        match find_link_target(&find(dom, "js")) {
            Some(LinkAction::RunScript(code)) => assert_eq!(code, "hazAlgo(1)"),
            other => panic!("deberia ser un script, no {}", describe(&other)),
        }
        match find_link_target(&find(dom, "mixed")) {
            Some(LinkAction::RunScript(code)) => assert_eq!(code, "hazAlgo(2)"),
            other => panic!("deberia ser un script, no {}", describe(&other)),
        }
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
        let blank = |id: &str| match find_link_target(&find(dom, id)) {
            Some(LinkAction::Navigate { opens_new_tab, .. }) => opens_new_tab,
            other => panic!("deberia navegar, no {}", describe(&other)),
        };
        assert!(blank("blank"));
        assert!(blank("upper"));
        assert!(!blank("self"));
        assert!(!blank("notarget"));
    }

    /// Fase 6.3: la diferencia real entre checkbox y radio. Un checkbox se
    /// conmuta; un radio se MARCA (nunca se desmarca clicandolo) y ademas
    /// desmarca a su grupo.
    #[test]
    fn clicking_a_checkbox_toggles_it_both_ways() {
        let dom = r#"<html><body><input id="cb" type="checkbox"></body></html>"#;
        let root = root_of(dom);
        let cb = Node::find_by_id(&root, "cb").expect("deberia existir");
        apply_checkable_click(&root, &cb);
        assert!(is_checked(&cb), "el primer clic lo marca");
        apply_checkable_click(&root, &cb);
        assert!(!is_checked(&cb), "el segundo clic lo desmarca - eso SI es correcto para un checkbox");
    }

    #[test]
    fn clicking_a_radio_never_unchecks_it_unlike_a_checkbox() {
        let dom = r#"<html><body><input id="r" type="radio" name="g"></body></html>"#;
        let root = root_of(dom);
        let r = Node::find_by_id(&root, "r").expect("deberia existir");
        apply_checkable_click(&root, &r);
        assert!(is_checked(&r));
        apply_checkable_click(&root, &r);
        assert!(
            is_checked(&r),
            "clicar un radio ya marcado deberia dejarlo marcado - en el spec real no hay forma de desmarcarlo clicandolo"
        );
    }

    #[test]
    fn clicking_a_radio_unchecks_the_others_of_its_group_only() {
        let dom = r#"<html><body>
            <input id="a" type="radio" name="grupo" checked>
            <input id="b" type="radio" name="grupo">
            <input id="otro" type="radio" name="otrogrupo" checked>
            <input id="caja" type="checkbox" name="grupo" checked>
        </body></html>"#;
        let root = root_of(dom);
        let b = Node::find_by_id(&root, "b").expect("deberia existir");
        apply_checkable_click(&root, &b);

        assert!(is_checked(&b), "el clicado queda marcado");
        assert!(!is_checked(&Node::find_by_id(&root, "a").unwrap()), "su compañero de grupo se desmarca");
        assert!(
            is_checked(&Node::find_by_id(&root, "otro").unwrap()),
            "un radio de OTRO grupo no deberia verse afectado"
        );
        assert!(
            is_checked(&Node::find_by_id(&root, "caja").unwrap()),
            "un checkbox que comparte name con el grupo NO es parte del grupo de radios y no deberia desmarcarse"
        );
    }

    #[test]
    fn a_radio_without_a_name_forms_no_group_and_only_checks_itself() {
        let dom = r#"<html><body>
            <input id="x" type="radio" checked>
            <input id="y" type="radio">
        </body></html>"#;
        let root = root_of(dom);
        let y = Node::find_by_id(&root, "y").expect("deberia existir");
        apply_checkable_click(&root, &y);
        assert!(is_checked(&y));
        assert!(
            is_checked(&Node::find_by_id(&root, "x").unwrap()),
            "sin name no hay grupo, asi que no deberia desmarcar a nadie"
        );
    }

    /// Una `LoadedPage` de verdad SIN tocar la red (Fase 7):
    /// `build_page_keeping_runtime` con `network: None` corre el pipeline
    /// completo (parseo, cascada, layout, scripts) sobre HTML en memoria, y
    /// no descarga nada. Eso permite probar de verdad la logica de
    /// historial de `apply_history_ops`, que necesita una pagina cargada
    /// para resolver URLs relativas.
    fn loaded_page(url: &str) -> LoadedPage {
        let (page, runtime) = build_page_keeping_runtime(
            "<html><body></body></html>",
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
            None,
        );
        LoadedPage {
            url: url.to_string(),
            title: String::new(),
            page,
            runtime,
            font_set: None,
            images: ImageMap::new(),
            focused_node: None,
        }
    }

    /// Un servidor con una pestaña que ya tiene documento y una entrada de
    /// historial, listo para probar `apply_history_ops`.
    fn server_with_page(url: &str) -> EngineServer {
        let mut server = EngineServer::new();
        let document_id = 7;
        let tab = server.active_tab_mut();
        tab.current_page = Some(loaded_page(url));
        tab.document_id = document_id;
        tab.history = vec![HistoryEntry { url: url.to_string(), document_id }];
        tab.history_index = Some(0);
        server
    }

    #[test]
    fn push_state_adds_an_entry_in_the_same_document_and_moves_the_index() {
        let mut server = server_with_page("http://ejemplo.test/app");
        server.apply_history_ops(vec![engine_js::history::HistoryOp::Push("/app/ruta2".to_string())]);

        let tab = server.active_tab();
        assert_eq!(tab.history.len(), 2);
        assert_eq!(tab.history_index, Some(1));
        assert_eq!(tab.history[1].url, "http://ejemplo.test/app/ruta2", "la URL relativa se resuelve contra la de la pagina");
        assert_eq!(
            tab.history[1].document_id, tab.document_id,
            "una entrada de pushState pertenece al documento VIVO - es lo que hace que volver atras no recargue"
        );
        assert_eq!(
            tab.current_page.as_ref().unwrap().url,
            "http://ejemplo.test/app/ruta2",
            "pushState tambien cambia la URL actual de la pagina"
        );
    }

    #[test]
    fn replace_state_overwrites_the_current_entry_without_adding_one() {
        let mut server = server_with_page("http://ejemplo.test/app");
        server.apply_history_ops(vec![engine_js::history::HistoryOp::Replace("/app/normalizada".to_string())]);

        let tab = server.active_tab();
        assert_eq!(tab.history.len(), 1, "replaceState no deberia añadir ninguna entrada");
        assert_eq!(tab.history_index, Some(0));
        assert_eq!(tab.history[0].url, "http://ejemplo.test/app/normalizada");
    }

    /// Igual que una navegacion normal: empujar una entrada nueva desde
    /// mitad del historial descarta lo que hubiera "adelante" (lo exige el
    /// spec; sin ello un `forward` posterior saltaria a una entrada que ya
    /// no pertenece a esta linea de historia).
    #[test]
    fn push_state_from_the_middle_of_the_history_truncates_whatever_was_ahead() {
        let mut server = server_with_page("http://ejemplo.test/app");
        {
            let tab = server.active_tab_mut();
            tab.history.push(HistoryEntry { url: "http://ejemplo.test/b".to_string(), document_id: 7 });
            tab.history.push(HistoryEntry { url: "http://ejemplo.test/c".to_string(), document_id: 7 });
            tab.history_index = Some(0); // como si se hubiera ido "atras" dos veces
        }
        server.apply_history_ops(vec![engine_js::history::HistoryOp::Push("/nueva".to_string())]);

        let tab = server.active_tab();
        assert_eq!(tab.history.len(), 2, "las dos entradas 'adelante' deberian haberse descartado");
        assert_eq!(tab.history_index, Some(1));
        assert_eq!(tab.history[1].url, "http://ejemplo.test/nueva");
    }

    /// Sin pagina cargada no hay URL base contra la que resolver nada, asi
    /// que la operacion se ignora en vez de inventarse una entrada.
    #[test]
    fn history_ops_without_a_loaded_page_are_ignored_instead_of_corrupting_the_history() {
        let mut server = EngineServer::new();
        server.apply_history_ops(vec![engine_js::history::HistoryOp::Push("/x".to_string())]);
        assert!(server.active_tab().history.is_empty());
        assert_eq!(server.active_tab().history_index, None);
    }

    /// Igual que el helper `find`, pero devolviendo la RAIZ - los tests de
    /// grupos de radio necesitan el arbol entero (`apply_checkable_click`
    /// busca a los hermanos desde la raiz), no solo un nodo suelto, asi que
    /// aqui no hace falta el `std::mem::forget` que `find` si necesita.
    fn root_of(html: &str) -> std::sync::Arc<std::sync::RwLock<Node>> {
        engine_dom::HtmlParser::parse(html)
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
