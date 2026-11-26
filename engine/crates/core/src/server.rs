//! Servidor NDJSON del motor nativo.
//!
//! Este proceso mantiene una sesión de página Rust viva. `navigate` usa el
//! cliente HTTP nativo y el pipeline DOM/CSS/JS/layout; `get_state` rasteriza
//! el layout con tiny-skia y devuelve la captura PNG en Base64. La salida
//! estándar contiene exclusivamente JSON; los logs van a stderr.

use crate::pipeline::{build_page_keeping_runtime, find_background_image_urls, find_external_script_srcs, find_external_stylesheet_hrefs, find_image_srcs, find_inline_style_css, find_module_preloads, PageResult};
use crate::protocol::{
    ElementAttributes, ElementRect, EngineRequest, EngineResponse, InteractiveElement, TabInfo,
    PROTOCOL_VERSION,
};
use base64::Engine as _;
use engine_dom::{Node, NodeType};
use engine_gfx::{focus_overlay_with_state, render_layout_to_png_with_overlay_scaled, selection_overlay_rects};
use engine_image::decode_image;
use engine_js::JsRuntime;
use engine_layout::{BoxType, ImageMap, LayoutBox, LayoutTreeBuilder};
use engine_net::{NetworkRequest, SharedBroker};
use engine_text::FontSet;
use std::collections::HashMap;
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Default)]
struct FieldEditState {
    cursor: usize,
    anchor: Option<usize>,
}

impl FieldEditState {
    fn at(cursor: usize) -> Self {
        Self { cursor, anchor: None }
    }

    fn selection_range(&self, len: usize) -> Option<(usize, usize)> {
        let anchor = self.anchor?.min(len);
        let cursor = self.cursor.min(len);
        (anchor != cursor).then(|| (anchor.min(cursor), anchor.max(cursor)))
    }
}

/// Cuantos nodos tiene un arbol DOM (solo para el registro de memoria).
fn count_dom_nodes(root: &std::sync::Arc<std::sync::RwLock<Node>>) -> usize {
    1 + root.read().map(|node| node.children.iter().map(count_dom_nodes).sum::<usize>()).unwrap_or(0)
}

/// Cuantas propiedades de estilo hay en total, cuantos bytes de texto suman y
/// cuantas cadenas DISTINTAS son (solo para el registro de memoria).
fn style_stats(root: &LayoutBox) -> (usize, usize, usize) {
    fn walk(layout_box: &LayoutBox, entries: &mut usize, bytes: &mut usize, distinct: &mut std::collections::HashSet<String>) {
        for (key, value) in &layout_box.computed_style {
            *entries += 1;
            *bytes += key.len() + value.len();
            distinct.insert(format!("{key}\u{0}{value}"));
        }
        for child in &layout_box.children {
            walk(child, entries, bytes, distinct);
        }
    }
    let (mut entries, mut bytes, mut distinct) = (0, 0, std::collections::HashSet::new());
    walk(root, &mut entries, &mut bytes, &mut distinct);
    (entries, bytes, distinct.len())
}

/// Cuantas cajas tiene un arbol de layout (solo para el registro de memoria).
fn count_layout_boxes(root: &LayoutBox) -> usize {
    1 + root.children.iter().map(count_layout_boxes).sum::<usize>()
}

struct LoadedPage {
    url: String,
    title: String,
    page: PageResult,
    runtime: JsRuntime,
    font_set: Option<FontSet>,
    images: ImageMap,
    focused_node: Option<std::sync::Arc<std::sync::RwLock<Node>>>,
    focus_state: Option<FieldEditState>,
    hovered_chain: Vec<std::sync::Arc<std::sync::RwLock<Node>>>,
    selection_rects: Vec<engine_layout::Rect>,
    selected_text: Option<String>,
    /// El usuario escribio en esta pagina: una pestaña asi no se descarta
    /// (se perderia lo escrito), igual que no lo haria un navegador de verdad.
    edited: bool,
}

impl LoadedPage {
    fn new(
        url: String,
        title: String,
        page: PageResult,
        runtime: JsRuntime,
        font_set: Option<FontSet>,
        images: ImageMap,
    ) -> Self {
        Self {
            url,
            title,
            page,
            runtime,
            font_set,
            images,
            focused_node: None,
            focus_state: None,
            hovered_chain: Vec::new(),
            selection_rects: Vec::new(),
            selected_text: None,
            edited: false,
        }
    }

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
        // Temporizadores vencidos ANTES de medir (Fase 14). Este es el
        // punto donde el tiempo avanza para el JS de la pagina: este motor
        // no tiene un reloj de fondo propio, asi que un `setTimeout` corre
        // cuando el servidor procesa algo (cargar, clic, escribir, tecla,
        // redimensionar) - todas esas operaciones pasan por aqui, que es
        // justo por lo que se engancha en `relayout` y no en cada una por
        // separado.
        //
        // ANTES de reconstruir el arbol, no despues, porque el trabajo
        // tipico de un callback de temporizador es MUTAR el DOM (mostrar
        // un menu, insertar contenido diferido): hacerlo despues dejaria
        // esos cambios sin medir hasta la siguiente operacion, y el
        // usuario veria la pagina un paso por detras.
        self.run_due_timers_before_layout();

        self.page.layout_root = LayoutTreeBuilder::build(
            &self.page.dom_root,
            &self.page.stylesheet,
            width,
            height,
            self.font_set.as_ref(),
            &self.images,
        );
        // `scrollTop`/`scrollLeft` asignados desde JS desde la ultima vez
        // se aplican AQUI, contra el arbol recien construido - no en el
        // momento de la asignacion (ver el aviso en `dom_bindings::
        // scroll_accessor`). `relayout` es el UNICO sitio que reconstruye
        // el arbol tras la carga inicial (ver el aviso de arriba: clic,
        // tecla, redimension, todo pasa por aqui), asi que es el sitio
        // correcto para que el scroll de cualquier contenedor se refleje
        // de verdad tras la siguiente interaccion.
        if let Some(offsets) = self.runtime.scroll_offsets() {
            let copia = offsets.lock().unwrap().clone();
            engine_layout::apply_scroll_offsets(&mut self.page.layout_root, &copia);
        }
        self.publish_layout_snapshot();
    }

    /// Ejecuta los temporizadores vencidos, si los hay. Separado de
    /// `relayout` solo para que ese comentario largo no lo parta por la
    /// mitad; no tiene mas logica que la de registrar cuantos corrieron.
    fn run_due_timers_before_layout(&mut self) {
        let fired = self.runtime.run_due_timers();
        if fired > 0 {
            tracing::debug!("[server] {fired} callback(s) de temporizador ejecutados antes del layout");
        }
    }

    /// El titulo VIVO del documento, leido del `<title>` real en cada
    /// llamada en vez del que se capturo al cargar (Fase 14.1). Hace falta
    /// porque `document.title = "..."` desde JS - patron universal en
    /// aplicaciones web, para marcar mensajes sin leer o reflejar la
    /// seccion actual - muta el DOM despues de la carga, y el valor
    /// congelado en `LoadedPage::title` se quedaba obsoleto para siempre.
    ///
    /// Se conserva `self.title` como respaldo: un documento sin `<title>`
    /// devuelve cadena vacia aqui, y en ese caso es mejor seguir mostrando
    /// lo que hubiera (normalmente tambien vacio) que no cambiar nada.
    fn current_title(&self) -> String {
        let live = Node::find_all_by_tag(&self.page.dom_root, "title")
            .first()
            .map(Node::text_content)
            .unwrap_or_default();
        if live.is_empty() { self.title.clone() } else { live }
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
        crate::pipeline::collect_box_metrics(&self.page.layout_root, &mut data.boxes);
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

// `collect_box_metrics` vive ahora en `pipeline.rs`: `build_page_keeping_
// runtime` tambien necesita publicar el snapshot, no solo `EngineServer`
// (ver su doc-comment alli para el porque).

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
    /// Desde cuando NO es la pestaña activa (`None` si lo es). Con esto y
    /// `EngineServer::discard_after` se decide cuando una pestaña en segundo
    /// plano se descarta.
    inactive_since: Option<std::time::Instant>,
    /// `Some` si la pestaña se descarto: su pagina (DOM, JavaScript, layout,
    /// imagenes) se libero y solo quedan estos datos, mas el historial y el
    /// scroll. Se vuelve a cargar al activarla.
    discarded: Option<DiscardedTab>,
}

/// Lo que sobrevive a descartar una pestaña, para seguir enseñandola en la barra.
#[derive(Debug, Clone)]
struct DiscardedTab {
    url: String,
    title: String,
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
            inactive_since: None,
            discarded: None,
        }
    }
}

struct EngineServer {
    width: u32,
    height: u32,
    scale: f32,
    /// Todo lo que este renderer pide al exterior - red, cookies desde JS
    /// y Web Storage - pasa por aqui (ADR 0001, etapa 2; Fase 64). Hoy es un
    /// `LocalBroker` en el mismo proceso, con el mismo cliente HTTP, las
    /// cookies y el almacen de siempre, compartidos por todas las paginas de
    /// la sesion (su razon de ser es sobrevivir a navegar). Cada pagina
    /// recibe un handle a este mismo broker mas su origen, y solo puede ver
    /// lo suyo. Un broker en otro proceso se conectara sustituyendo esto.
    broker: SharedBroker,
    /// `"local"` o `"remote"`, para el saludo `ready`.
    broker_kind: &'static str,
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
    /// Cuanto se espera a UN subrecurso (hoja, script, imagen) antes de
    /// omitirlo. Por defecto `SUBRESOURCE_TIMEOUT`; los tests lo bajan.
    subresource_timeout: std::time::Duration,
    /// Tiempo TOTAL que se dedica a los subrecursos de una pagina, contado
    /// desde que llega el documento. Por defecto `SUBRESOURCE_BUDGET`.
    subresource_budget: std::time::Duration,
    /// Tiempo maximo que se dedica a las IMAGENES, contado desde que
    /// terminan las hojas y los scripts (tarea 1.4). Por defecto
    /// `IMAGE_BUDGET`.
    image_budget: std::time::Duration,
    /// Cuanto tiempo sin usarse antes de descartar una pestaña en segundo plano
    /// (`None` = nunca). Por defecto `DISCARD_AFTER`; `NAVEGADOR_IA_DESCARTE_SEGUNDOS`
    /// lo cambia (0 lo desactiva).
    discard_after: Option<std::time::Duration>,
    /// Se solto alguna pagina (se navego, se cerro una pestaña) y falta devolver su
    /// memoria: recolectar los objetos de JavaScript y pedir al sistema que la recupere.
    /// Se hace con el motor en reposo (`IDLE_CLEANUP_DELAY`), nunca a mitad de una operacion.
    needs_cleanup: bool,
    /// Cuando llego la ultima peticion (para saber desde cuando esta en reposo).
    last_activity: std::time::Instant,
}

/// Cuanto reposo hace falta para limpiar la memoria de las paginas ya sueltas.
const IDLE_CLEANUP_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// Tiempo sin usarse tras el cual una pestaña en segundo plano se descarta: libera
/// su memoria y pasa a consumir practicamente nada.
const DISCARD_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Variable de entorno que cambia `DISCARD_AFTER` (en segundos; 0 = nunca descartar).
pub const DISCARD_ENV: &str = "NAVEGADOR_IA_DESCARTE_SEGUNDOS";

/// Cada cuanto despierta como MAXIMO el reloj de fondo cuando hay temporizadores
/// de JavaScript pendientes (una animacion con `setInterval(f, 16)` no debe
/// repintar y codificar una captura 60 veces por segundo).
const MIN_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

fn discard_after_from_env() -> Option<std::time::Duration> {
    parse_discard_after(std::env::var(DISCARD_ENV).ok().as_deref())
}

/// `None` (sin variable o ilegible) = el valor por defecto; `0` = nunca descartar.
fn parse_discard_after(value: Option<&str>) -> Option<std::time::Duration> {
    match value.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(0) => None,
        Some(segundos) => Some(std::time::Duration::from_secs(segundos)),
        None => Some(DISCARD_AFTER),
    }
}

impl EngineServer {
    /// Con el broker en el mismo proceso: cookies y `localStorage` del perfil
    /// en disco (Fases 15 y 25), que recuperan la sesion de una carga
    /// anterior del mismo perfil. `sessionStorage` sigue vacio al arrancar.
    #[cfg(test)]
    fn new() -> Self {
        Self::with_broker(engine_net::LocalBroker::persistent().shared(), "local")
    }

    fn with_broker(broker: SharedBroker, broker_kind: &'static str) -> Self {
        Self {
            width: 1280,
            height: 720,
            scale: 1.0,
            broker,
            broker_kind,
            tabs: vec![Tab::new(0)],
            active_tab: 0,
            next_tab_id: 1,
            next_document_id: 1,
            subresource_timeout: Self::SUBRESOURCE_TIMEOUT,
            subresource_budget: Self::SUBRESOURCE_BUDGET,
            image_budget: Self::IMAGE_BUDGET,
            discard_after: discard_after_from_env(),
            needs_cleanup: false,
            last_activity: std::time::Instant::now(),
        }
    }

    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    /// Cambia la pestaña activa apuntando desde cuando la anterior deja de serlo
    /// (de ahi se cuenta el tiempo para descartarla) y poniendo a cero la de la
    /// nueva.
    fn set_active_tab(&mut self, new_index: usize, now: std::time::Instant) {
        if new_index != self.active_tab {
            if let Some(old) = self.tabs.get_mut(self.active_tab) {
                old.inactive_since = Some(now);
            }
        }
        self.active_tab = new_index;
        self.tabs[new_index].inactive_since = None;
    }

    /// ¿Se puede descartar la pestaña `index`? Una sola regla para saber cuando
    /// despertar (`next_deadline`) y que descartar (`discard_idle_tabs`): si
    /// difirieran, el motor despertaria en bucle para no hacer nada.
    ///
    /// No se descarta: la activa, una sin pagina (ya vacia o ya descartada) ni
    /// una donde el usuario escribio (se perderia lo escrito).
    fn discard_deadline(&self, index: usize) -> Option<std::time::Instant> {
        let after = self.discard_after?;
        let tab = self.tabs.get(index)?;
        if index == self.active_tab || tab.discarded.is_some() {
            return None;
        }
        let page = tab.current_page.as_ref()?;
        if page.edited {
            return None;
        }
        Some(tab.inactive_since? + after)
    }

    /// Descarta las pestañas en segundo plano que llevan `discard_after` sin
    /// usarse: libera su pagina entera (DOM, JavaScript, layout, imagenes) y
    /// conserva direccion, titulo, historial y scroll. Devuelve los ids de las
    /// descartadas. Una pestaña descartada no consume CPU ni memoria
    /// apreciable; al activarla se vuelve a cargar (`restore_active_tab`).
    fn discard_idle_tabs(&mut self, now: std::time::Instant) -> Vec<u32> {
        let due: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.discard_deadline(i).is_some_and(|deadline| deadline <= now)).collect();
        let mut discarded = Vec::new();
        for index in due {
            let tab = &mut self.tabs[index];
            if let Some(page) = tab.current_page.take() {
                tab.discarded = Some(DiscardedTab { url: page.url.clone(), title: page.current_title() });
                discarded.push(tab.id);
            }
        }
        if !discarded.is_empty() {
            tracing::info!("[tabs] pestañas descartadas por inactividad: {discarded:?}");
            // Las paginas ya se soltaron; el recolector de JavaScript no libera sus
            // objetos hasta que corre, y despues se devuelve la memoria al sistema.
            self.cleanup_memory();
        }
        discarded
    }

    /// Cuando tiene que despertar el motor: el temporizador de JavaScript mas
    /// proximo de la pestaña activa, o el momento de descartar la primera
    /// pestaña inactiva. `None` = no hay nada que hacer hasta que llegue una
    /// peticion, asi que el motor puede dormir sin gastar CPU.
    fn next_deadline(&self) -> Option<std::time::Instant> {
        let timer = self.active_tab().current_page.as_ref().and_then(|page| page.runtime.next_timer_deadline());
        let discard = (0..self.tabs.len()).filter_map(|i| self.discard_deadline(i)).min();
        let cleanup = self.needs_cleanup.then(|| self.last_activity + IDLE_CLEANUP_DELAY);
        [timer, discard, cleanup].into_iter().flatten().min()
    }

    /// Libera de verdad la memoria de las paginas ya sueltas (ver `needs_cleanup`).
    fn cleanup_memory(&mut self) {
        engine_js::collect_garbage();
        crate::memoria::liberar_al_sistema();
        self.needs_cleanup = false;
    }

    /// Lo que hay que hacer cuando el reloj de fondo despierta: avanzar los
    /// temporizadores de la pestaña activa y descartar las inactivas que toque.
    /// Devuelve lo que hay que publicar sin que nadie lo pida.
    fn tick(&mut self, now: std::time::Instant) -> Vec<EngineResponse> {
        let mut publications = Vec::new();
        if let Some(state) = self.tick_active_tab_timers() {
            publications.push(state);
        }
        if !self.discard_idle_tabs(now).is_empty() {
            publications.push(self.list_tabs(None));
        }
        if self.needs_cleanup && now >= self.last_activity + IDLE_CLEANUP_DELAY {
            self.cleanup_memory();
        }
        publications
    }

    /// Marca la pagina activa como "con cosas escritas" (ver `LoadedPage::edited`).
    fn mark_active_page_edited(&mut self) {
        if let Some(page) = self.active_tab_mut().current_page.as_mut() {
            page.edited = true;
        }
    }

    /// El reloj de fondo real (ver el aviso de `run_stdio`) - se llama
    /// periodicamente desde una tarea de Tokio aparte, no desde ningun
    /// comando NDJSON. Solo la pestaña ACTIVA (no todas): una pestaña en
    /// segundo plano no se esta pintando, tiquearla igual seria trabajo
    /// tirado (mismo criterio que un navegador real, que tambien
    /// despriorizca temporizadores de pestañas en segundo plano) - si el
    /// usuario cambia a ella mas tarde, `switch_tab` ya relayoutea esa
    /// pestaña por su cuenta.
    ///
    /// `run_due_timers()` se llama DIRECTO (no via `relayout`, que lo haria
    /// de nuevo) solo para decidir barato si merece la pena pagar un
    /// relayout completo - la inmensa mayoria de los pulsos de este reloj
    /// no tendran ningun temporizador vencido. Que `relayout()` vuelva a
    /// comprobar temporizadores vencidos justo despues es inofensivo (no
    /// hay ninguno nuevo que disparar dos veces), no una duplicacion real.
    ///
    /// Devuelve el estado nuevo (con `id: None`) si disparo algun
    /// temporizador, para que `run_stdio` lo publique sin que nadie lo pida
    /// (plan H07, Fase 50). Antes el relayout ocurria pero nadie se
    /// enteraba: el cambio solo aparecia con el siguiente comando.
    fn tick_active_tab_timers(&mut self) -> Option<EngineResponse> {
        let (w, h) = (self.width, self.height);
        let tab = self.active_tab_mut();
        let page = tab.current_page.as_mut()?;
        if page.runtime.run_due_timers() == 0 {
            return None;
        }
        page.relayout(w as f32, h as f32);
        let content_extent = page.page.layout_root.content_extent();
        let scrolled = clamp_scroll_offset(tab.scroll_offset_y, content_extent, h as f32);
        page.publish_scroll_offset(scrolled);
        tab.scroll_offset_y = scrolled;
        Some(self.state_response(None))
    }

    fn ready_response(&self, id: Option<String>) -> EngineResponse {
        EngineResponse::Ready {
            id,
            protocol_version: PROTOCOL_VERSION,
            renderer: "rust-native",
            renderer_status: "ready",
            width: self.width,
            height: self.height,
            broker: self.broker_kind,
        }
    }

    async fn handle(&mut self, request: EngineRequest) -> (EngineResponse, bool) {
        let id = request.id().map(str::to_owned);
        self.last_activity = std::time::Instant::now();

        match request {
            EngineRequest::Navigate { url, .. } => (self.navigate(id, url, true).await, false),
            EngineRequest::Ping { .. } => (
                EngineResponse::Pong {
                    id,
                    protocol_version: PROTOCOL_VERSION,
                    renderer_status: "ready",
                    broker: self.broker_kind,
                },
                false,
            ),
            EngineRequest::Resize { width, height, scale, .. } => {
                self.width = width.clamp(200, 4000);
                self.height = height.clamp(200, 4000);
                self.scale = if scale.is_finite() { scale.clamp(0.5, 4.0) } else { 1.0 };
                // Solo la pestaña ACTIVA se relayouta aqui (Fase 4.5) - las
                // demas se ponen al dia de forma perezosa en `switch_tab`
                // cuando (si) el usuario vuelve a ellas, para no pagar el
                // coste de un relayout completo por cada pestaña en
                // segundo plano en cada `resize`.
                let (w, h) = (self.width, self.height);
                let tab = self.active_tab_mut();
                if let Some(page) = &mut tab.current_page {
                    // Evento `resize` real sobre `<html>` (mismo objetivo y
                    // mismo motivo que el `scroll` de arriba: es donde
                    // `window.addEventListener('resize', ...)` delega, ver
                    // el aviso de `EngineRequest::Scroll`) ANTES del
                    // relayout - un listener real de `resize` a menudo
                    // muta el DOM (mostrar/ocultar un menu segun el ancho),
                    // y ese cambio tiene que estar YA en el arbol que
                    // `page.relayout` va a construir, no en el siguiente.
                    if let Some(html) = Node::document_element(&page.page.dom_root) {
                        if let Err(error) = page.runtime.dispatch_event(&html, "resize") {
                            return (Self::error(id, format!("resize_event_error: {error}")), false);
                        }
                    }
                    page.relayout(w as f32, h as f32);
                    let content_extent = page.page.layout_root.content_extent();
                    tab.scroll_offset_y = clamp_scroll_offset(tab.scroll_offset_y, content_extent, h as f32);
                }
                (self.state_response(id), false)
            }
            EngineRequest::GetState { .. } => (self.state_response(id), false),
            EngineRequest::Click { x, y, .. } => (self.click(id, x, y).await, false),
            EngineRequest::Scroll { dy, .. } => {
                let (w, h) = (self.width, self.height);
                let tab = self.active_tab_mut();
                if let Some(page) = &mut tab.current_page {
                    let content_extent = page.page.layout_root.content_extent();
                    let scrolled = clamp_scroll_offset(tab.scroll_offset_y + dy as f32, content_extent, h as f32);
                    // Fase 8: `getBoundingClientRect` devuelve coordenadas
                    // de VIEWPORT, asi que el snapshot necesita saber
                    // cuanto se ha desplazado el documento.
                    page.publish_scroll_offset(scrolled);
                    tab.scroll_offset_y = scrolled;

                    // Solo si ALGUIEN escucha `scroll` (tarea 1.5/1.4 del plan): sin
                    // listeners el evento no puede cambiar nada, y rehacer el
                    // layout entero en cada paso de scroll era lo que tardaba
                    // (0,66 s en Wikipedia). El desplazamiento ya esta
                    // publicado arriba.
                    if page.runtime.has_event_listener("scroll") {
                        // Evento `scroll` real: el desplazamiento de arriba YA
                        // era real (`getBoundingClientRect` ya lo reflejaba),
                        // lo que faltaba era que un listener JS se enterara. Se
                        // dispara sobre `<html>` (`documentElement`), NO sobre
                        // `dom_root` directo - mismo objetivo exacto que ya usa
                        // `fire_popstate`, y no por casualidad: `window.
                        // addEventListener` (el shim de `register_history`, ver
                        // su aviso) delega precisamente en `documentElement`,
                        // asi que dispararlo ahi es lo unico que hace que
                        // `window.addEventListener('scroll', ...)` tambien se
                        // entere. `document.addEventListener('scroll', ...)`
                        // SIGUE funcionando igual: `documentElement.parent` es
                        // `dom_root`, y la fase de burbuja del dispatch camina
                        // por los ancestros reales hasta llegar ahi.
                        // `None` (documento sin ningun elemento raiz - un DOM
                        // malformado que HTML5 parsing en la practica nunca
                        // produce) se salta el evento sin tratarlo como error:
                        // no hay a quien dispararselo, no que la propia
                        // operacion de scroll haya fallado.
                        if let Some(html) = Node::document_element(&page.page.dom_root) {
                            if let Err(error) = page.runtime.dispatch_event(&html, "scroll") {
                                return (Self::error(id, format!("scroll_event_error: {error}")), false);
                            }
                        }
                        // Un listener de scroll puede mutar el DOM (el patron
                        // real de "infinite scroll": cargar mas contenido al
                        // acercarse al final) - mismo criterio que `click`/
                        // `press_key`: relayout despues de dispatchear, y
                        // reajustar/republicar el scroll contra el arbol
                        // fresco (pudo haber crecido o encogido).
                        page.relayout(w as f32, h as f32);
                        let content_extent = page.page.layout_root.content_extent();
                        let scrolled = clamp_scroll_offset(tab.scroll_offset_y, content_extent, h as f32);
                        page.publish_scroll_offset(scrolled);
                        tab.scroll_offset_y = scrolled;
                    }
                }
                (self.scroll_state_response(id), false)
            }
            EngineRequest::TypeText {
                id,
                x,
                y,
                text,
                press_enter,
            } => {
                self.mark_active_page_edited();
                (self.type_text(id, x, y, text, press_enter).await, false)
            }
            EngineRequest::PressKey { id, key } => {
                self.mark_active_page_edited();
                (self.press_key(id, key).await, false)
            }
            EngineRequest::Hover { id, x, y } => (self.hover(id, x, y), false),
            EngineRequest::SelectText { id, from_x, from_y, to_x, to_y } => (self.select_text(id, from_x, from_y, to_x, to_y), false),
            // Llega aqui solo cuando no hay ninguna navegacion en curso: si la
            // hubiera, `run_stdio` la abandona antes de llamar a `handle`
            // (ver `handle_interruptible`).
            EngineRequest::Stop { .. } => (EngineResponse::Ok { id, message: "stopped" }, false),
            EngineRequest::ClickElement { element_id, .. } => match self.locate_element(element_id) {
                Ok((x, y)) => (self.click(id, x, y).await, false),
                Err(message) => (Self::error(id, message), false),
            },
            EngineRequest::TypeIntoElement { element_id, text, replace, press_enter, .. } => {
                match self.locate_element(element_id) {
                    Ok((x, y)) => {
                        self.mark_active_page_edited();
                        if replace {
                            self.clear_control_value_at(x, y);
                        }
                        (self.type_text(id, x, y, text, press_enter).await, false)
                    }
                    Err(message) => (Self::error(id, message), false),
                }
            }
            EngineRequest::Back { .. } => (self.back(id).await, false),
            EngineRequest::Forward { .. } => (self.forward(id).await, false),
            EngineRequest::NewTab { url, .. } => (self.open_new_tab(id, url).await, false),
            EngineRequest::CloseTab { tab_id, .. } => {
                let response = self.close_tab(id.clone(), tab_id);
                // Si la que pasa a ser activa estaba descartada, se vuelve a cargar.
                if !matches!(response, EngineResponse::Error { .. }) && self.active_tab().discarded.is_some() {
                    return (self.restore_active_tab(id).await, false);
                }
                (response, false)
            }
            EngineRequest::SwitchTab { tab_id, .. } => (self.switch_tab(id, tab_id).await, false),
            EngineRequest::ListTabs { .. } => (self.list_tabs(id), false),
            EngineRequest::GetAccessibilityTree { .. } => (self.accessibility_tree_response(id), false),
            EngineRequest::GetPageText { .. } => (self.page_text_response(id), false),
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
        self.navigate_with_body(id, url, record_history, None, 0).await
    }

    /// Navega enviando un cuerpo `application/x-www-form-urlencoded` por
    /// POST (Fase 16) - el envio de un `<form method="post">`. Siempre
    /// registra historial: un POST es una navegacion de pleno derecho.
    ///
    /// Aviso real que este motor NO da todavia: volver ATRAS a una entrada
    /// que se creo con un POST deberia re-enviar el formulario (y un
    /// navegador de verdad pregunta antes de hacerlo). Aqui `back` la
    /// repetira como GET, que es distinto - declarado en ARCHITECTURE.md.
    async fn navigate_post(&mut self, id: Option<String>, url: String, body: Vec<u8>) -> EngineResponse {
        self.navigate_with_body(id, url, true, Some(body), 0).await
    }

    /// El cuerpo comun de las dos: `body` a `None` hace un GET normal,
    /// `Some` hace un POST con ese contenido. Se unifican aqui en vez de
    /// duplicar la funcion entera porque TODO lo que viene despues de la
    /// peticion (seguir redirecciones, descubrir sub-recursos, construir
    /// la pagina, historial, temporizadores de carga) es identico.
    /// Numero maximo de redirecciones ENCADENADAS que un script puede
    /// provocar (`location.href = ...` durante la carga). Un navegador real
    /// tiene un tope equivalente por la misma razon: sin el, una pagina que
    /// se redirige a si misma cuelga el motor.
    const MAX_CLIENT_REDIRECTS: u8 = 5;

    async fn navigate_with_body(&mut self, id: Option<String>, url: String, record_history: bool, body: Option<Vec<u8>>, depth: u8) -> EngineResponse {
        // Cronometro por fases. Sin esto la unica cifra observable era el
        // total de la navegacion, que no distingue "la red va lenta" de
        // "el motor tarda en maquetar" - y sin distinguirlo cualquier
        // intento de optimizar es a ciegas. Se emite a `info`, asi que en
        // uso normal (nivel `warn`) no cuesta nada; `RUST_LOG=info` lo
        // enciende. Ver `bin/engine_server.rs`.
        let nav_start = std::time::Instant::now();
        let request = match NetworkRequest::new(&url) {
            Ok(mut request) => {
                let scheme = request.url.scheme();
                if scheme != "http" && scheme != "https" {
                    return Self::error(id, format!("esquema_no_soportado: solo se admiten peticiones http y https (esquema: {scheme})"));
                }
                request.navigation = true;
                if let Some(body) = body {
                    request.method = engine_net::request::Method::Post;
                    request.headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());
                    request.body = Some(body);
                }
                request
            }
            Err(error) => return Self::error(id, format!("invalid_url: {error}")),
        };

        let response = match self.broker.fetch(&request).await {
            Ok(response) => response,
            Err(error) => return Self::error(id, format!("network_error: {error}")),
        };
        // Un 404 o un 500 con una pagina HTML se MUESTRA, como hace cualquier
        // navegador: la pagina de error del servidor dice mas (y a veces ofrece
        // un buscador o enlaces) que un aviso generico. Solo es un error sin
        // pagina si el cuerpo no es un documento legible.
        if !response.is_success() && !is_renderable_error_page(&response) {
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
        tracing::info!(
            "[tiempo] documento descargado en {:?} ({} bytes)",
            nav_start.elapsed(),
            html.len()
        );

        // Se parsea UNA vez aqui solo para descubrir que recursos externos
        // hacen falta (`<link rel=stylesheet>`, `<script src>`) - un DOM de
        // usar y tirar, distinto del DOM real que construye
        // `build_page_keeping_runtime` mas abajo. Duplica el parseo (barato,
        // microsegundos para una pagina normal) a cambio de que
        // `pipeline.rs` siga sin depender de `url`/`engine-net` para nada -
        // ver el doc-comment de `find_external_stylesheet_hrefs`.
        let discovery_dom = engine_dom::HtmlParser::parse(&html);

        // Content Security Policy (Fase 21). Se toma de la cabecera y del
        // `<meta http-equiv>`, y si vienen las dos se COMBINAN de forma
        // restrictiva: hay que pasar ambas. Combinarlas relajando seria un
        // agujero - quien pudiera inyectar un `<meta>` desactivaria la
        // politica del servidor, justo lo que CSP existe para impedir.
        let mut csp = response
            .headers
            .get("content-security-policy")
            .map(|h| engine_net::ContentSecurityPolicy::parse(h))
            .unwrap_or_default();
        if let Some(meta_policy) = find_meta_csp(&discovery_dom) {
            csp.merge(&engine_net::ContentSecurityPolicy::parse(&meta_policy));
        }
        let page_origin = engine_net::storage::origin_of(&page_url);

        let stylesheet_hrefs = find_external_stylesheet_hrefs(&discovery_dom);
        // Los fragmentos que un bundle declara con `<link rel="modulepreload">`
        // se descargan junto a los `<script src>` y por el MISMO camino
        // (mismo filtro de CSP `script-src`, mismo pool paralelo, mismo mapa):
        // para el motor son codigo JavaScript que un `import` va a pedir, y
        // tratarlos aparte solo abriria la puerta a que un dia uno de los dos
        // caminos aplicara una politica distinta.
        let mut script_srcs = find_external_script_srcs(&discovery_dom);
        script_srcs.extend(find_module_preloads(&discovery_dom));

        // CSP se aplica ANTES de descargar, no despues: el objetivo es no
        // pedirle nada a un origen no autorizado, no descartar lo que ya
        // llego (que ya habria filtrado que la pagina visito ese sitio).
        let stylesheet_hrefs = filter_by_csp(stylesheet_hrefs, "style-src", &csp, &page_url, &page_origin);
        let script_srcs = filter_by_csp(script_srcs, "script-src", &csp, &page_url, &page_origin);

        let (n_css, n_js) = (stylesheet_hrefs.len(), script_srcs.len());

        // Un unico tope compartido por hojas, scripts e imagenes, por ese
        // orden: lo que condiciona el aspecto va primero y, si el tiempo se
        // acaba, son las imagenes lo que se sacrifica.
        let deadline = tokio::time::Instant::now() + self.subresource_budget;

        let t = std::time::Instant::now();
        let external_css = self.fetch_external_stylesheets(stylesheet_hrefs, &page_url, deadline).await;
        tracing::info!("[tiempo] {n_css} hoja(s) de estilo en {:?} ({} bytes de CSS)", t.elapsed(), external_css.len());

        let t = std::time::Instant::now();
        let external_scripts = self.fetch_external_scripts(script_srcs, &page_url, deadline).await;
        tracing::info!("[tiempo] {n_js} script(s) externos en {:?}", t.elapsed());

        // `background-image`/`background: url(...)` (Fase 40) - las URLs se
        // descubren escaneando el CSS YA ENSAMBLADO (el `<style>` en linea
        // del documento + las hojas externas que se acaban de descargar
        // arriba), no desde el DOM como `<img src>` - por eso este
        // descubrimiento no puede ocurrir antes que `external_css`. Se
        // añaden a la MISMA lista que `<img src>` para reusar exactamente
        // el mismo fetch/decode/`ImageMap` - `img-src` de CSP tambien
        // gobierna una imagen de fondo, no solo `<img>`.
        let allow_inline_style = csp.allows_inline("style-src");
        let inline_css = if allow_inline_style { find_inline_style_css(&discovery_dom) } else { String::new() };
        let mut image_srcs = find_image_srcs(&discovery_dom);
        image_srcs.extend(find_background_image_urls(&format!("{inline_css}\n{}", crate::pipeline::strip_sheet_markers(&external_css))));
        let image_srcs = filter_by_csp(image_srcs, "img-src", &csp, &page_url, &page_origin);
        let n_img = image_srcs.len();

        let t = std::time::Instant::now();
        let images = self.fetch_images(image_srcs, &page_url, self.image_deadline(deadline)).await;
        tracing::info!("[tiempo] {n_img} imagen(es) en {:?} ({} decodificadas)", t.elapsed(), images.len());

        let t = std::time::Instant::now();
        let font_set = FontSet::load_default_sans_serif();
        tracing::info!("[tiempo] fuentes cargadas en {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let (page, mut runtime) = build_page_keeping_runtime(
            &html,
            &external_css,
            self.width as f32,
            self.height as f32,
            Some(&font_set),
            &external_scripts,
            &images,
            Some(self.broker.clone()),
            // Fase 15: el origen sale de la URL FINAL (`page_url`, tras
            // seguir redirecciones), no de la pedida - si `http://a.test`
            // redirige a `https://a.test`, el almacenamiento que toca es
            // el del origen donde de verdad se aterrizo, igual que en un
            // navegador real.
            Some(crate::scripting::StorageContext {
                storage: self.broker.clone(),
                origin: page_origin.clone(),
                url: page_url.to_string(),
                csp: csp.clone(),
            }),
        );
        // Fase 6.4: un `window.open()` llamado durante la CARGA de la pagina
        // (no desde un clic real del usuario) se descarta - mismo criterio
        // que el bloqueador de ventanas emergentes de cualquier navegador
        // real, que solo permite abrir ventanas con "activacion del
        // usuario" de por medio. No es una limitacion tecnica disfrazada:
        // honrarlas ademas abriria la puerta a que una pagina que llama
        // `window.open` al cargar se abriera a si misma en bucle, ya que
        // cada pestaña nueva vuelve a pasar por aqui.
        tracing::info!("[tiempo] parseo + JS + cascada + layout en {:?}", t.elapsed());
        let discarded = runtime.take_pending_window_opens();
        if !discarded.is_empty() {
            tracing::info!(
                "[server] {} window.open() durante la carga de la pagina ignorados (hace falta un clic real, igual que un bloqueador de popups)",
                discarded.len()
            );
        }
        if tracing::enabled!(tracing::Level::DEBUG) {
            // Para saber DONDE se va la memoria de una pagina (`RUST_LOG=engine_core=debug`).
            let image_bytes: usize = images.values().map(|image| image.rgba.len()).sum();
            tracing::debug!(
                "[mem] {}: {} imagenes decodificadas = {:.1} MiB; {} nodos DOM; {} cajas de layout",
                final_url,
                images.len(),
                image_bytes as f64 / 1_048_576.0,
                count_dom_nodes(&page.dom_root),
                count_layout_boxes(&page.layout_root)
            );
            let (entries, string_bytes, distinct) = style_stats(&page.layout_root);
            tracing::debug!("[mem] estilos: {entries} propiedades en total, {:.1} MiB de texto, {distinct} valores distintos", string_bytes as f64 / 1_048_576.0);
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

        // La pagina anterior de esta pestaña (si la habia) se suelta aqui: su memoria se
        // devuelve con el motor en reposo, no ahora.
        if self.active_tab().current_page.is_some() {
            self.needs_cleanup = true;
        }
        let tab = self.active_tab_mut();
        tab.current_page = Some(LoadedPage::new(final_url, title, page, runtime, Some(font_set), images));
        tab.discarded = None;
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
        // Temporizadores de la CARGA (Fase 14). El patron mas comun de
        // todos - `setTimeout(inicializar, 0)` para diferir el arranque
        // hasta despues de que el documento este montado - vence de
        // inmediato, asi que tiene que correr aqui: si se dejara para la
        // primera interaccion del usuario, la pagina se mostraria en su
        // estado sin inicializar hasta que alguien la tocara.
        //
        // Va DESPUES de publicar el primer snapshot a proposito: un
        // callback que lea `getBoundingClientRect` durante la
        // inicializacion debe ver geometria real, no el snapshot vacio.
        // Y solo se rehace el layout si algun callback llego a correr -
        // sin temporizadores vencidos no hay nada que pudiera haber
        // cambiado el DOM, y `build_page_keeping_runtime` ya dejo el
        // arbol construido.
        let (width, height) = (self.width, self.height);
        if let Some(page) = self.active_tab_mut().current_page.as_mut() {
            if page.runtime.run_due_timers() > 0 {
                page.relayout(width as f32, height as f32);
            }
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

        // Navegaciones pedidas por un script de CARGA (`location.href =
        // ...`, tipico de una redireccion en cliente). A diferencia de
        // `window.open`, esta SI se honra durante la carga: no abre nada
        // nuevo, sustituye esta misma pagina, que es lo que la pagina esta
        // pidiendo. El limite de profundidad corta el bucle de una pagina
        // que se redirija a si misma; sin el, cada carga volveria a pedir
        // la siguiente sin fin.
        let navegaciones = self
            .active_tab_mut()
            .current_page
            .as_mut()
            .map(|page| page.runtime.take_pending_navigations())
            .unwrap_or_default();
        // `.last()`, no `.first()`: varias asignaciones sincronas seguidas
        // (`location.pathname = 'a'; location.hash = 'b';`) son la ULTIMA
        // ganando, igual que en un navegador real - la primera se estaba
        // honrando y la ultima se descartaba en silencio.
        if let Some(destino) = navegaciones.into_iter().last() {
            if depth < Self::MAX_CLIENT_REDIRECTS {
                if let Some(page) = self.active_tab().current_page.as_ref() {
                    match url::Url::parse(&page.url).and_then(|base| base.join(&destino.raw_url)) {
                        Ok(resuelta) => {
                            let resuelta = resuelta.to_string();
                            if resuelta != page.url {
                                tracing::info!("[server] redireccion desde JS a {resuelta}");
                                return Box::pin(self.navigate_with_body(id, resuelta, !destino.replace_current_entry, None, depth + 1)).await;
                            }
                        }
                        Err(error) => {
                            tracing::warn!("[server] URL de location.href invalida, se ignora la redireccion: {} ({error})", destino.raw_url);
                        }
                    }
                }
            } else {
                tracing::warn!("[server] demasiadas redirecciones desde JS seguidas, se corta el bucle");
            }
        }

        let t = std::time::Instant::now();
        let response = self.state_response(id);
        tracing::info!("[tiempo] captura PNG en {:?}", t.elapsed());
        tracing::info!("[tiempo] TOTAL navegacion {:?}", nav_start.elapsed());
        response
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
        self.set_active_tab(self.tabs.len() - 1, std::time::Instant::now());
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
        if self.tabs.remove(index).current_page.is_some() {
            self.needs_cleanup = true;
        }
        if index == self.active_tab {
            self.active_tab = index.saturating_sub(1);
        } else if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.tabs[self.active_tab].inactive_since = None;
        self.state_response(id)
    }

    /// Cambia la pestaña ACTIVA a `tab_id` (Fase 4.5). Relayout perezoso de
    /// la pestaña recien activada: el tamaño de la ventana pudo cambiar
    /// (`resize`) mientras estaba en segundo plano - `resize` solo
    /// relayouta la pestaña activa EN ESE MOMENTO (ver el handler de
    /// `Resize` en `handle`), asi que hay que ponerla al dia aqui, no ahi.
    async fn switch_tab(&mut self, id: Option<String>, tab_id: u32) -> EngineResponse {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return Self::error(id, format!("no existe ninguna pestaña con id {tab_id}"));
        };
        self.set_active_tab(index, std::time::Instant::now());
        // Una pestaña descartada no tiene pagina: se vuelve a cargar.
        if self.active_tab().discarded.is_some() {
            return self.restore_active_tab(id).await;
        }
        let (w, h) = (self.width, self.height);
        let tab = self.active_tab_mut();
        if let Some(page) = &mut tab.current_page {
            page.relayout(w as f32, h as f32);
            let content_extent = page.page.layout_root.content_extent();
            tab.scroll_offset_y = clamp_scroll_offset(tab.scroll_offset_y, content_extent, h as f32);
        }
        self.state_response(id)
    }

    /// Vuelve a cargar la pestaña ACTIVA, que estaba descartada: pide otra vez la
    /// pagina de la entrada actual del historial (sin añadir otra entrada) y
    /// recoloca el scroll donde estaba. Si la red falla, la pestaña sigue
    /// descartada y el error llega a quien lo pidio, que puede reintentar.
    async fn restore_active_tab(&mut self, id: Option<String>) -> EngineResponse {
        let tab = self.active_tab();
        let Some(discarded) = tab.discarded.clone() else {
            return self.state_response(id);
        };
        let url = tab.history_index.and_then(|index| tab.history.get(index)).map_or(discarded.url, |entry| entry.url.clone());
        let scroll = tab.scroll_offset_y;
        let response = self.navigate(id.clone(), url, false).await;
        if !matches!(response, EngineResponse::State { .. }) {
            return response;
        }
        // `navigate` ya dejo `discarded` en `None`; falta el scroll de antes.
        let height = self.height as f32;
        let tab = self.active_tab_mut();
        if let Some(page) = &mut tab.current_page {
            let scrolled = clamp_scroll_offset(scroll, page.page.layout_root.content_extent(), height);
            page.publish_scroll_offset(scrolled);
            tab.scroll_offset_y = scrolled;
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
                title: tab.current_page.as_ref().map(LoadedPage::current_title).or_else(|| tab.discarded.as_ref().map(|d| d.title.clone())).unwrap_or_default(),
                url: tab.current_page.as_ref().map(|page| page.url.clone()).or_else(|| tab.discarded.as_ref().map(|d| d.url.clone())).unwrap_or_default(),
                discarded: tab.discarded.is_some(),
            })
            .collect();
        EngineResponse::Tabs { id, tabs, active_tab_id }
    }

    /// Cuantos subrecursos (imagenes, hojas, scripts) se descargan A LA VEZ.
    ///
    /// Seis es el limite clasico de conexiones por host de un navegador
    /// real: suficiente para que la latencia deje de dominar, y bajo como
    /// para no parecer un ataque al servidor ni agotar descriptores.
    ///
    /// El numero importa mucho: antes de esto los subrecursos se
    /// descargaban EN SERIE, asi que el tiempo de carga era la SUMA de
    /// todas las latencias. Medido en vivo: el articulo "Espana" de
    /// Wikipedia trae 161 `<img>` de upload.wikimedia.org y tardaba ~38 s
    /// solo en recorrerlas una a una.
    const MAX_CONCURRENT_SUBRESOURCES: usize = 12;

    /// Cuanto se espera a UN subrecurso antes de omitirlo (tarea 1.2 del
    /// plan). Un navegador real tambien se rinde con un recurso lento y
    /// pinta la pagina sin el. Antes no habia limite: una sola imagen cuyo
    /// servidor aceptaba la conexion y no contestaba dejaba la navegacion
    /// esperando para siempre.
    const SUBRESOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

    /// Tiempo TOTAL para todos los subrecursos de una pagina, desde que llega
    /// el documento (tarea 1.2 del plan). Sin esto, 160 imagenes lentas a 8 s
    /// cada una y 6 a la vez sumaban mas de 3 minutos. Pasado el tope se
    /// pinta con lo que haya llegado: lo ya descargado se conserva y a lo que
    /// falta ni siquiera se le pide.
    const SUBRESOURCE_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

    /// Las imagenes son lo ultimo que se descarga y lo menos importante para
    /// poder leer y actuar sobre la pagina: se les da menos tiempo que al
    /// resto (tarea 1.4 del plan). Una pagina con 150 imagenes en una red
    /// lenta tardaba 10 s en aparecer; ahora aparece, sin las que no llegaron,
    /// a los 4 s de empezar con ellas. Las imagenes van en orden de documento,
    /// asi que lo que se pierde es lo de mas abajo.
    const IMAGE_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

    /// Hasta cuando se esperan las imagenes: su propio tope, sin pasar nunca
    /// del compartido de toda la pagina.
    fn image_deadline(&self, shared_deadline: tokio::time::Instant) -> tokio::time::Instant {
        (tokio::time::Instant::now() + self.image_budget).min(shared_deadline)
    }

    /// Descarga en PARALELO (acotado, ver `MAX_CONCURRENT_SUBRESOURCES`)
    /// una lista de referencias crudas (`href`/`src` tal como aparecen en
    /// el HTML), resolviendolas antes contra `page_url` - la URL final tras
    /// redirecciones, no la pedida originalmente.
    ///
    /// Devuelve `(referencia cruda, respuesta)` EN EL ORDEN ORIGINAL del
    /// documento, no en el de llegada: se usa `buffered` y no
    /// `buffer_unordered` justo por eso. El orden es indiferente para
    /// imagenes y scripts (van a un mapa por clave) pero es OBLIGATORIO
    /// para las hojas de estilo, donde "la que viene despues gana a igual
    /// especificidad" - devolverlas segun quien contestara antes haria que
    /// el aspecto de la pagina dependiera de la latencia de la red.
    ///
    /// Lo que falla (URL invalida, peticion no construible, 404, red
    /// caida) se omite con un aviso y NO aborta la pagina, exactamente
    /// igual que antes y que un navegador real.
    async fn fetch_subresources(
        &self,
        refs: Vec<String>,
        page_url: &url::Url,
        kind: &'static str,
        deadline: tokio::time::Instant,
    ) -> Vec<(String, engine_net::NetworkResponse)> {
        use futures_util::stream::StreamExt;

        let resolved: Vec<(String, url::Url)> = refs
            .into_iter()
            .filter_map(|raw| match page_url.join(&raw) {
                Ok(absolute) if matches!(absolute.scheme(), "http" | "https") => Some((raw, absolute)),
                Ok(_) => None,
                Err(_) => {
                    tracing::warn!("[server] {kind} con URL invalida, se omite: {raw}");
                    None
                }
            })
            .collect();

        futures_util::stream::iter(resolved)
            .map(|(raw, absolute)| async move {
                let request = match NetworkRequest::new(absolute.as_str()) {
                    Ok(request) => request,
                    Err(error) => {
                        tracing::warn!("[server] no se pudo construir la peticion para {absolute}: {error}");
                        return None;
                    }
                };
                // Tope de ESTA peticion: su propio tiempo, sin pasar nunca del
                // tope compartido de toda la pagina. Si el tope compartido ya
                // se agoto, ni se le pide: no hace falta abrir una conexion
                // para abandonarla al instante.
                let limit = (tokio::time::Instant::now() + self.subresource_timeout).min(deadline);
                if limit <= tokio::time::Instant::now() {
                    tracing::debug!("[server] tiempo para subrecursos agotado, no se pide {absolute}");
                    return None;
                }
                match tokio::time::timeout_at(limit, self.broker.fetch(&request)).await {
                    Err(_elapsed) => {
                        tracing::warn!("[server] {absolute} no respondio a tiempo, se omite");
                        None
                    }
                    Ok(Ok(response)) if response.is_success() => Some((raw, response)),
                    Ok(Ok(response)) => {
                        tracing::warn!(
                            "[server] {absolute} respondio {} {}, se omite",
                            response.status_code,
                            response.status_text
                        );
                        None
                    }
                    Ok(Err(error)) => {
                        tracing::warn!("[server] no se pudo descargar {absolute}: {error}");
                        None
                    }
                }
            })
            .buffered(Self::MAX_CONCURRENT_SUBRESOURCES)
            .filter_map(|result| async move { result })
            .collect()
            .await
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
    async fn fetch_external_stylesheets(&self, hrefs: Vec<String>, page_url: &url::Url, deadline: tokio::time::Instant) -> String {
        let mut combined = String::new();
        // El orden que devuelve `fetch_subresources` es el del documento,
        // no el de llegada - imprescindible aqui (ver su doc-comment).
        // Cada hoja va marcada con su `href` crudo para que `pipeline::assemble_css`
        // la coloque en el sitio de su `<link>`, no al final de todo.
        for (href, response) in self.fetch_subresources(hrefs, page_url, "<link rel=stylesheet>", deadline).await {
            combined.push_str(&crate::pipeline::wrap_sheet(&href, &response.text()));
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
    async fn fetch_external_scripts(&self, srcs: Vec<String>, page_url: &url::Url, deadline: tokio::time::Instant) -> HashMap<String, String> {
        let mut fetched = HashMap::new();
        for (src, response) in self.fetch_subresources(srcs, page_url, "<script src>", deadline).await {
            fetched.insert(src, response.text());
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
    async fn fetch_images(&self, srcs: Vec<String>, page_url: &url::Url, deadline: tokio::time::Instant) -> ImageMap {
        let mut unique_urls: Vec<String> = Vec::new();
        let mut raws_by_url: HashMap<String, Vec<String>> = HashMap::new();
        for raw in srcs {
            let Ok(absolute) = page_url.join(&raw) else {
                tracing::warn!("[server] <img src> con URL invalida, se omite: {raw}");
                continue;
            };
            if !matches!(absolute.scheme(), "http" | "https") {
                continue;
            }
            let key = absolute.as_str().to_string();
            raws_by_url
                .entry(key.clone())
                .and_modify(|list| {
                    if !list.contains(&raw) {
                        list.push(raw.clone());
                    }
                })
                .or_insert_with(|| {
                    unique_urls.push(key);
                    vec![raw]
                });
        }

        let mut fetched = ImageMap::new();
        for (abs_url, response) in self.fetch_subresources(unique_urls, page_url, "<img src>", deadline).await {
            match decode_image(&response.body) {
                Some(image) => {
                    if let Some(raws) = raws_by_url.get(&abs_url) {
                        for raw in raws {
                            fetched.insert(raw.clone(), std::sync::Arc::clone(&image));
                        }
                    }
                }
                None => tracing::warn!("[server] {abs_url} no se pudo decodificar como imagen, se omite"),
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
        // Envio de formulario (Fase 10): clic en un boton submit dentro de
        // un `<form>` - mismo criterio que `link_target` arriba (se decide
        // AQUI, dentro del prestamo de `page`, pero se ejecuta DESPUES,
        // porque `submit_form` necesita `&mut self` completo).
        let mut submit_form_target: Option<std::sync::Arc<std::sync::RwLock<Node>>> = None;
        let mut submit_control_target: Option<std::sync::Arc<std::sync::RwLock<Node>>> = None;
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
                // Un listener de clic (o de foco/cambio, que este mismo clic
                // dispara) puede tocar el DOM, y clicar un campo o una casilla
                // cambia su estado. Sin nada de eso, no hay nada que rehacer.
                // Tarea 1.4: el layout solo se rehace si el clic pudo cambiar
                // algo. En Wikipedia rehacerlo costaba 0,55 s por clic,
                // incluso en una zona vacia donde no pasa nada.
                let mut may_have_changed_the_page = page.runtime.has_event_listener("click")
                    || is_text_control(&node)
                    || is_checkable_input(&node);
                page.selection_rects.clear();
                page.selected_text = None;
                let prev_focus = page.focused_node.clone();
                if is_text_control(&node) {
                    let focus_changed = !prev_focus.as_ref().is_some_and(|p| std::sync::Arc::ptr_eq(p, &node));
                    if focus_changed && page.page.stylesheet.has_focus_rules() {
                        may_have_changed_the_page = true;
                    }
                    let cursor = page
                        .page
                        .layout_root
                        .find_box_for_node(&node)
                        .map(|b| cursor_index_at_point(b, page.font_set.as_ref(), &control_value(&node), x, y + scroll_offset_y))
                        .unwrap_or_else(|| control_value(&node).chars().count());
                    apply_focus(page, Some(node.clone()), Some(FieldEditState::at(cursor)));
                    if focus_changed {
                        if let Some(prev) = &prev_focus {
                            let _ = page.runtime.dispatch_event(prev, "blur");
                        }
                    }
                    if let Err(error) = page.runtime.dispatch_event(&node, "focus") {
                        return Self::error(id, format!("focus_error: {error}"));
                    }
                } else {
                    if prev_focus.is_some() && page.page.stylesheet.has_focus_rules() {
                        may_have_changed_the_page = true;
                    }
                    apply_focus(page, None, None);
                    if let Some(prev) = &prev_focus {
                        let _ = page.runtime.dispatch_event(prev, "blur");
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
                            may_have_changed_the_page = true;
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
                        // Sin `<a href>` navegable: la OTRA accion por
                        // defecto real de un clic es enviar un `<form>`,
                        // si el nodo (o un ancestro, ver
                        // `find_submit_control`) es un boton submit -
                        // `<button>` sin `type` cuenta, es submit por
                        // defecto en el spec real.
                        None => {
                            if let Some(submit_control) = find_submit_control(&node) {
                                if let Some(form) = find_form_ancestor(&submit_control) {
                                    submit_control_target = Some(submit_control);
                                    submit_form_target = Some(form);
                                }
                            }
                        }
                    }
                }
                if may_have_changed_the_page {
                    page.relayout(width as f32, height as f32);
                }
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
            None => match submit_form_target {
                Some(form) => self.submit_form(id.clone(), form, submit_control_target).await,
                None => self.state_response(id.clone()),
            },
        };
        for url in window_opens {
            response = self.open_new_tab(id.clone(), Some(url)).await;
        }
        response
    }

    async fn type_text(
        &mut self,
        id: Option<String>,
        x: f32,
        y: f32,
        text: String,
        press_enter: bool,
    ) -> EngineResponse {
        let scroll_offset_y = self.active_tab().scroll_offset_y;
        let (width, height) = (self.width, self.height);
        // Envio de formulario (Fase 10): Enter en un input de texto de una
        // sola linea dentro de un <form> lo envia, igual que un navegador
        // real - pero `submit_form` (mas abajo) necesita `&mut self`
        // completo para poder navegar, y `page` (prestado de `self` aqui
        // debajo) todavia esta vivo en ese punto. Mismo patron que
        // `link_target` en `click`: el nodo implicado sale del bloque como
        // valor propio, la navegacion real ocurre DESPUES de que el
        // prestamo de `page` termine.
        let mut submit_form_target: Option<std::sync::Arc<std::sync::RwLock<Node>>> = None;
        {
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

            page.selection_rects.clear();
            page.selected_text = None;
            let cursor = append_control_value_at(&node, &text, None);
            apply_focus(page, Some(node.clone()), Some(FieldEditState::at(cursor)));
            for event_type in ["focus", "input"] {
                if let Err(error) = page.runtime.dispatch_event(&node, event_type) {
                    return Self::error(id, format!("{event_type}_error: {error}"));
                }
            }
            if press_enter {
                let mut enter_prevented = false;
                for event_type in ["keydown", "keyup"] {
                    match page.runtime.dispatch_keyboard_event(&node, event_type, "Enter") {
                        Ok(prevented) if event_type == "keydown" => enter_prevented = prevented,
                        Ok(_) => {}
                        Err(error) => return Self::error(id, format!("{event_type}_error: {error}")),
                    }
                }
                // `<textarea>` tambien es un control de texto
                // (`is_text_control`), pero Enter ahi inserta una linea
                // nueva en vez de enviar - un navegador real nunca envia
                // un formulario por Enter dentro de un textarea.
                if !enter_prevented && !is_textarea(&node) {
                    submit_form_target = find_form_ancestor(&node);
                }
            }
            page.relayout(width as f32, height as f32);
        }

        if let Some(form) = submit_form_target {
            return self.submit_form(id, form, None).await;
        }
        self.state_response(id)
    }

    /// Resuelve un `id` de la lista `elements` al punto donde clicarlo, en
    /// coordenadas de VIEWPORT (las que esperan `click` y `type_text`), y
    /// desplaza la pagina si ese punto queda fuera de la ventana (tarea 3.1).
    ///
    /// El `id` es la posicion en la lista que se calcula AHORA mismo con la
    /// misma funcion que la respuesta de `state`: si la pagina cambio desde
    /// que se pidio el estado, el `id` puede apuntar a otro elemento, igual
    /// que le pasaria a un humano que clica sobre una captura antigua. Por
    /// eso quien actua debe pedir el estado antes de cada acción.
    fn locate_element(&mut self, element_id: u32) -> Result<(f32, f32), String> {
        let viewport_height = self.height as f32;
        let tab = self.active_tab_mut();
        let Some(page) = &tab.current_page else {
            return Err("no hay ninguna página cargada".to_string());
        };
        let targets = collect_interactive_targets(&page.page.layout_root);
        let Some((element, _)) = targets.get(element_id as usize) else {
            return Err(format!(
                "el elemento [{element_id}] no existe: la página tiene {} elementos interactivos; pide el estado de nuevo",
                targets.len()
            ));
        };
        let rect = &element.rect;
        // Un elemento mas alto que la ventana se apunta por su parte
        // superior visible, no por su centro (que podria quedar fuera).
        let point_x = rect.x + rect.width / 2.0;
        let point_y = if rect.height > viewport_height {
            rect.y + viewport_height / 2.0
        } else {
            rect.y + rect.height / 2.0
        };
        let mut offset = tab.scroll_offset_y;
        if point_y < offset || point_y >= offset + viewport_height {
            let content_extent = page.page.layout_root.content_extent();
            offset = clamp_scroll_offset(point_y - viewport_height / 2.0, content_extent, viewport_height);
            page.publish_scroll_offset(offset);
            tab.scroll_offset_y = offset;
        }
        Ok((point_x, point_y - offset))
    }

    /// `replace` de `type_into_element`: vacia el `value` del control de texto
    /// que hay en ese punto del viewport. Si no es un control de texto no
    /// toca nada; `type_text` ya devuelve el error adecuado.
    fn clear_control_value_at(&mut self, x: f32, y: f32) {
        let scroll_offset_y = self.active_tab().scroll_offset_y;
        let Some(page) = &self.active_tab().current_page else { return };
        let Some(node) = page.page.layout_root.hit_test(x, y + scroll_offset_y) else { return };
        if !is_text_control(&node) {
            return;
        }
        let mut node_guard = node.write().unwrap();
        if let NodeType::Element { attributes, .. } = &mut node_guard.node_type {
            attributes.insert("value".to_string(), String::new());
        }
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
    async fn press_key(&mut self, id: Option<String>, key: String) -> EngineResponse {
        let (width, height) = (self.width, self.height);
        let mut submit_form_target: Option<std::sync::Arc<std::sync::RwLock<Node>>> = None;
        let mut scroll_target: Option<f32> = None;
        {
            let Some(page) = &mut self.active_tab_mut().current_page else {
                return Self::error(id, "no hay ninguna página cargada".to_string());
            };
            if matches!(key.as_str(), "Tab" | "Shift+Tab") {
                let backward = key == "Shift+Tab";
                if let Err(error) = move_focus_tab(page, backward, width as f32, height as f32, &mut scroll_target) {
                    return Self::error(id, error);
                }
                if let Some(node) = &page.focused_node {
                    let node = node.clone();
                    for event_type in ["keydown", "keyup"] {
                        if let Err(error) = page.runtime.dispatch_keyboard_event(&node, event_type, "Tab") {
                            return Self::error(id, format!("{event_type}_error: {error}"));
                        }
                    }
                }
            } else {
                let Some(node) = &page.focused_node else {
                    return Self::error(id, format!("no hay un control enfocado para la tecla {key}"));
                };
                let node = node.clone();
                let is_text = is_text_control(&node);
                let op = if is_text { parse_edit_key(&key, is_textarea(&node)) } else { None };
                let mutates_value = matches!(
                    op,
                    Some(EditOp::Insert(_) | EditOp::Backspace | EditOp::Delete | EditOp::Cut)
                );
                if let Some(op) = op {
                    let state = page.focus_state.get_or_insert_with(|| FieldEditState::at(control_value(&node).chars().count()));
                    edit_control(&node, state, op);
                }
                let mut enter_prevented = false;
                let event_key = normalized_keyboard_key(&key);
                for event_type in ["keydown", "keyup"] {
                    match page.runtime.dispatch_keyboard_event(&node, event_type, event_key) {
                        Ok(prevented) if event_type == "keydown" => enter_prevented = prevented,
                        Ok(_) => {}
                        Err(error) => return Self::error(id, format!("{event_type}_error: {error}")),
                    }
                }
                if mutates_value {
                    if let Err(error) = page.runtime.dispatch_event(&node, "input") {
                        return Self::error(id, format!("input_error: {error}"));
                    }
                    let page_may_react = ["input", "keydown", "keyup", "change"].iter().any(|event| page.runtime.has_event_listener(event));
                    if page_may_react {
                        page.relayout(width as f32, height as f32);
                    } else {
                        refresh_control_text(page, &node);
                    }
                }
                if key == "Enter" && !enter_prevented && is_text && !is_textarea(&node) {
                    submit_form_target = find_form_ancestor(&node);
                }
            }
        }

        if let Some(offset) = scroll_target {
            if let Some(page) = &self.active_tab().current_page {
                page.publish_scroll_offset(offset);
            }
            self.active_tab_mut().scroll_offset_y = offset;
        }
        if let Some(form) = submit_form_target {
            return self.submit_form(id, form, None).await;
        }
        self.state_response(id)
    }

    fn hover(&mut self, id: Option<String>, x: f32, y: f32) -> EngineResponse {
        let scroll_offset_y = self.active_tab().scroll_offset_y;
        let (width, height) = (self.width, self.height);
        let needs_repaint = {
            let Some(page) = &mut self.active_tab_mut().current_page else {
                return EngineResponse::Ok { id, message: "hover_no_page" };
            };
            let hit = page.page.layout_root.hit_test(x, y + scroll_offset_y);
            let new_chain = hit.as_ref().map(Node::ancestors).unwrap_or_default();
            let unchanged = new_chain.len() == page.hovered_chain.len()
                && new_chain.iter().zip(page.hovered_chain.iter()).all(|(a, b)| std::sync::Arc::ptr_eq(a, b));
            if unchanged {
                return EngineResponse::Ok { id, message: "hover_unchanged" };
            }
            for old in &page.hovered_chain {
                old.write().unwrap().hovered = false;
            }
            for node in &new_chain {
                node.write().unwrap().hovered = true;
            }
            page.hovered_chain = new_chain;
            if page.page.stylesheet.has_hover_rules() {
                page.relayout(width as f32, height as f32);
                true
            } else {
                false
            }
        };
        if needs_repaint {
            self.state_response(id)
        } else {
            EngineResponse::Ok { id, message: "hover_ok" }
        }
    }

    fn select_text(&mut self, id: Option<String>, from_x: f32, from_y: f32, to_x: f32, to_y: f32) -> EngineResponse {
        let scroll_offset_y = self.active_tab().scroll_offset_y;
        {
            let Some(page) = &mut self.active_tab_mut().current_page else {
                return Self::error(id, "no hay ninguna página cargada".to_string());
            };
            let (doc_fx, doc_fy) = (from_x, from_y + scroll_offset_y);
            let (doc_tx, doc_ty) = (to_x, to_y + scroll_offset_y);
            if let Some(hit) = page.page.layout_root.hit_test(doc_fx, doc_fy) {
                if is_text_control(&hit) {
                    if let Some(box_rect) = page.page.layout_root.find_box_for_node(&hit) {
                        let val = control_value(&hit);
                        let anchor = cursor_index_at_point(box_rect, page.font_set.as_ref(), &val, doc_fx, doc_fy);
                        let cursor = cursor_index_at_point(box_rect, page.font_set.as_ref(), &val, doc_tx, doc_ty);
                        page.selection_rects.clear();
                        page.selected_text = None;
                        apply_focus(page, Some(hit), Some(FieldEditState { cursor, anchor: (anchor != cursor).then_some(anchor) }));
                        return self.state_response(id);
                    }
                }
            }
            let (rects, text) = compute_text_selection(&page.page.layout_root, page.font_set.as_ref(), (doc_fx, doc_fy), (doc_tx, doc_ty));
            page.selection_rects = rects;
            page.selected_text = text;
        }
        self.state_response(id)
    }

    /// Envia el `<form>` ancestro de quien disparo el envio: Enter en un
    /// input de texto (`submit_control: None`, ver `type_text`/
    /// `press_key`) o clic en un boton submit (`submit_control: Some`, ver
    /// `click`/`find_submit_control`). Solo `method="get"` (el valor por
    /// defecto real cuando el atributo falta) esta implementado: la query
    /// string se construye con `url::Url::query_pairs_mut` (el mismo
    /// crate `url` que ya resuelve el resto de URLs del motor - doctrina
    /// de dependencias en ARCHITECTURE.md, nunca a mano) y navega como
    /// cualquier otro enlace.
    ///
    /// `method="post"` (o cualquier otro valor) devuelve un error
    /// explicito en vez de fingir un envio que este motor no hace de
    /// verdad: no hay forma de mandar un cuerpo en una peticion desde
    /// aqui todavia, y tratar un POST como si fuera GET filtraria datos
    /// de formulario (credenciales incluidas) por la URL - un fallo de
    /// seguridad real, no solo una imprecision.
    async fn submit_form(
        &mut self,
        id: Option<String>,
        form: std::sync::Arc<std::sync::RwLock<Node>>,
        submit_control: Option<std::sync::Arc<std::sync::RwLock<Node>>>,
    ) -> EngineResponse {
        let (action_attr, method) = {
            let guard = form.read().unwrap();
            let NodeType::Element { attributes, .. } = &guard.node_type else {
                return Self::error(id, "el envio de formulario no encontro un <form> real".to_string());
            };
            (
                attributes.get("action").cloned(),
                attributes
                    .get("method")
                    .map(|m| m.trim().to_ascii_lowercase())
                    .unwrap_or_else(|| "get".to_string()),
            )
        };

        // Cualquier metodo que no sea `post` se trata como GET, igual que
        // el spec real: `method` solo reconoce `get`, `post` y `dialog`, y
        // un valor invalido cae al valor por defecto (GET) en vez de
        // rechazar el envio.
        let is_post = method == "post";

        let Some(page_url_str) = self.active_tab().current_page.as_ref().map(|p| p.url.clone()) else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        let data = collect_form_data(&form, submit_control.as_ref());

        if is_post {
            // El `action` de un POST se resuelve igual que el de un GET
            // pero SIN tocar su query string: los datos van en el cuerpo,
            // asi que un `action="/buscar?pagina=2"` conserva ese `pagina=2`
            // (a diferencia del GET, donde la query se reemplaza entera).
            let target_url = match resolve_submit_action(&page_url_str, action_attr.as_deref()) {
                Ok(url) => url,
                Err(message) => return Self::error(id, message),
            };
            return self.navigate_post(id, target_url.to_string(), encode_form_body(&data)).await;
        }

        match build_get_submit_url(&page_url_str, action_attr.as_deref(), &data) {
            Ok(target_url) => self.navigate(id, target_url.to_string(), true).await,
            Err(message) => Self::error(id, message),
        }
    }

    fn state_response(&self, id: Option<String>) -> EngineResponse {
        self.state_snapshot(id, true)
    }

    /// El estado tras un `scroll`: igual que `state_response` pero sin los
    /// elementos ni `requires_javascript`, que un desplazamiento no cambia
    /// (tarea 1.5 del plan). Evita reenviar y recalcular lo que mas pesa.
    fn scroll_state_response(&self, id: Option<String>) -> EngineResponse {
        self.state_snapshot(id, false)
    }

    /// `with_page_details`: incluir `elements` y `requires_javascript`.
    fn state_snapshot(&self, id: Option<String>, with_page_details: bool) -> EngineResponse {
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
                elements: with_page_details.then(Vec::new),
                requires_javascript: with_page_details.then_some(false),
                can_go_back,
                can_go_forward,
                focused_is_text: None,
                selected_text: None,
            };
        };

        // Resaltado de selección en página + foco y cursor sobre el control activo.
        let mut overlay = selection_overlay_rects(&page.selection_rects);
        if let Some(node) = &page.focused_node {
            if let Some(focused) = page.page.layout_root.find_box_for_node(node) {
                let len = control_value(node).chars().count();
                let (cursor, selection) = match &page.focus_state {
                    Some(st) => (st.cursor.min(len), st.selection_range(len)),
                    None => (len, None),
                };
                overlay.extend(focus_overlay_with_state(focused, page.font_set.as_ref(), cursor, selection));
            }
        }
        let focused_is_text = Some(page.focused_node.as_ref().is_some_and(is_text_control));
        let selected_text = focused_selected_text(page).or_else(|| page.selected_text.clone());
        let screenshot = match render_layout_to_png_with_overlay_scaled(
            &page.page.layout_root,
            page.font_set.as_ref(),
            &page.images,
            self.width,
            self.height,
            tab.scroll_offset_y,
            &overlay,
            self.scale,
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
            title: page.current_title(),
            screenshot,
            elements: with_page_details.then(|| collect_interactive_elements(&page.page.layout_root)),
            requires_javascript: with_page_details.then(|| page_content_requires_javascript(&page.page.dom_root, &page.page.layout_root)),
            can_go_back,
            can_go_forward,
            focused_is_text,
            selected_text,
        }
    }

    /// Construye y devuelve el árbol semántico accesible (AOM) de la página activa (Fase 5.2).
    fn page_text_response(&self, id: Option<String>) -> EngineResponse {
        let tab = self.active_tab();
        let Some(page) = &tab.current_page else {
            return Self::error(id, "no hay ninguna página cargada".to_string());
        };
        // El contenido principal primero: en Wikipedia el menu lateral por si
        // solo llena el tope antes de llegar al articulo.
        let root = find_main_content(&page.page.layout_root).unwrap_or(&page.page.layout_root);
        let (text, truncated) = collect_readable_text(root, MAX_PAGE_TEXT_CHARS);
        EngineResponse::PageText { id, tab_id: tab.id, url: page.url.clone(), title: page.current_title(), text, truncated }
    }

    fn accessibility_tree_response(&self, id: Option<String>) -> EngineResponse {
        let tab = self.active_tab();
        let Some(page) = &tab.current_page else {
            return EngineResponse::Error {
                id,
                message: "no_page_loaded".to_string(),
            };
        };

        let tree = engine_ai::AccessibilityTree::build(&page.page.layout_root);
        let prompt = tree.to_llm_representation();

        EngineResponse::AccessibilityTree {
            id,
            tab_id: tab.id,
            url: page.url.clone(),
            title: page.current_title(),
            tree,
            prompt,
        }
    }

    fn error(id: Option<String>, message: String) -> EngineResponse {
        EngineResponse::Error { id, message }
    }
}

/// La politica declarada con `<meta http-equiv="Content-Security-Policy"
/// content="...">` (Fase 21). Muchas paginas la ponen asi en vez de por
/// cabecera, sobre todo las servidas desde un sitio estatico donde no se
/// controlan las cabeceras HTTP.
fn find_meta_csp(dom_root: &std::sync::Arc<std::sync::RwLock<Node>>) -> Option<String> {
    Node::find_all_by_tag(dom_root, "meta").into_iter().find_map(|meta| {
        let guard = meta.read().unwrap();
        let NodeType::Element { attributes, .. } = &guard.node_type else { return None };
        let is_csp = attributes
            .get("http-equiv")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("content-security-policy"));
        if is_csp { attributes.get("content").cloned() } else { None }
    })
}

/// Quita de la lista los subrecursos que la politica no permite, ANTES de
/// descargarlos (Fase 21) - pedirle algo a un origen no autorizado ya
/// filtraria que la pagina lo visito, aunque luego se descartara.
///
/// Un `src`/`href` que no resuelve contra la URL de la pagina se deja
/// pasar: no es CSP quien debe decidir sobre una URL invalida, y quien la
/// descargue ya fallara por su cuenta con un aviso.
fn filter_by_csp(
    sources: Vec<String>,
    directive: &str,
    csp: &engine_net::ContentSecurityPolicy,
    page_url: &url::Url,
    page_origin: &str,
) -> Vec<String> {
    if csp.is_empty() {
        return sources;
    }
    sources
        .into_iter()
        .filter(|src| match page_url.join(src) {
            Ok(resolved) => {
                let allowed = csp.allows_url(directive, &resolved, page_origin);
                if !allowed {
                    tracing::warn!("[csp] bloqueado por '{directive}': {resolved}");
                }
                allowed
            }
            Err(_) => true,
        })
        .collect()
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

/// Pone en la caja de `node` el texto que le corresponde segun su `value` actual,
/// sin rehacer el layout (el tamaño de un campo no depende de lo escrito).
fn refresh_control_text(page: &mut LoadedPage, node: &std::sync::Arc<std::sync::RwLock<Node>>) {
    let (tag_name, attributes) = match &node.read().unwrap().node_type {
        NodeType::Element { tag_name, attributes } => (tag_name.clone(), attributes.clone()),
        _ => return,
    };
    let text = engine_layout::resolve_replaced_text(&tag_name, &attributes, node);
    if let Some(layout_box) = page.page.layout_root.find_box_for_node_mut(node) {
        layout_box.replaced_text = text;
    }
}

enum EditOp {
    Insert(String),
    Backspace,
    Delete,
    Cut,
    MoveLeft { select: bool },
    MoveRight { select: bool },
    MoveHome { select: bool },
    MoveEnd { select: bool },
    SelectAll,
}

fn parse_edit_key(key: &str, multiline: bool) -> Option<EditOp> {
    match key {
        "Backspace" => Some(EditOp::Backspace),
        "Delete" => Some(EditOp::Delete),
        "ArrowLeft" => Some(EditOp::MoveLeft { select: false }),
        "ArrowRight" => Some(EditOp::MoveRight { select: false }),
        "Home" => Some(EditOp::MoveHome { select: false }),
        "End" => Some(EditOp::MoveEnd { select: false }),
        "Shift+ArrowLeft" => Some(EditOp::MoveLeft { select: true }),
        "Shift+ArrowRight" => Some(EditOp::MoveRight { select: true }),
        "Shift+Home" => Some(EditOp::MoveHome { select: true }),
        "Shift+End" => Some(EditOp::MoveEnd { select: true }),
        "Ctrl+a" | "Ctrl+A" => Some(EditOp::SelectAll),
        "Ctrl+x" | "Ctrl+X" => Some(EditOp::Cut),
        "Enter" if multiline => Some(EditOp::Insert("\n".to_string())),
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => Some(EditOp::Insert(key.to_string())),
                _ => None,
            }
        }
    }
}

fn normalized_keyboard_key(key: &str) -> &str {
    key.rsplit('+').next().unwrap_or(key)
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

fn control_value(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> String {
    let (tag_name, attr_val) = {
        let guard = node.read().unwrap();
        match &guard.node_type {
            NodeType::Element { tag_name, attributes } => (tag_name.clone(), attributes.get("value").cloned()),
            _ => return String::new(),
        }
    };
    match attr_val {
        Some(v) => v,
        None if tag_name == "textarea" => Node::text_content(node).trim().to_string(),
        None => String::new(),
    }
}

fn set_control_value(node: &std::sync::Arc<std::sync::RwLock<Node>>, value: String) {
    let mut guard = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut guard.node_type {
        attributes.insert("value".to_string(), value);
    }
}

fn edit_control(node: &std::sync::Arc<std::sync::RwLock<Node>>, state: &mut FieldEditState, op: EditOp) {
    let mut value = control_value(node);
    let len = value.chars().count();
    let mut cursor = state.cursor.min(len);
    let sel = state.selection_range(len);

    let remove_range = |v: &mut String, a: usize, b: usize| {
        let ba = char_to_byte(v, a);
        let bb = char_to_byte(v, b);
        v.replace_range(ba..bb, "");
    };

    match op {
        EditOp::Insert(text) => {
            if let Some((a, b)) = sel {
                remove_range(&mut value, a, b);
                cursor = a;
            }
            let bpos = char_to_byte(&value, cursor);
            value.insert_str(bpos, &text);
            state.cursor = cursor + text.chars().count();
            state.anchor = None;
            set_control_value(node, value);
        }
        EditOp::Backspace => {
            if let Some((a, b)) = sel {
                remove_range(&mut value, a, b);
                state.cursor = a;
            } else if cursor > 0 {
                remove_range(&mut value, cursor - 1, cursor);
                state.cursor = cursor - 1;
            }
            state.anchor = None;
            set_control_value(node, value);
        }
        EditOp::Delete => {
            if let Some((a, b)) = sel {
                remove_range(&mut value, a, b);
                state.cursor = a;
            } else if cursor < len {
                remove_range(&mut value, cursor, cursor + 1);
                state.cursor = cursor;
            }
            state.anchor = None;
            set_control_value(node, value);
        }
        EditOp::Cut => {
            if let Some((a, b)) = sel {
                remove_range(&mut value, a, b);
                state.cursor = a;
                state.anchor = None;
                set_control_value(node, value);
            }
        }
        EditOp::MoveLeft { select } => {
            if select && state.anchor.is_none() {
                state.anchor = Some(cursor);
            }
            state.cursor = match (select, sel) {
                (false, Some((a, _))) => a,
                _ => cursor.saturating_sub(1),
            };
            if !select {
                state.anchor = None;
            }
        }
        EditOp::MoveRight { select } => {
            if select && state.anchor.is_none() {
                state.anchor = Some(cursor);
            }
            state.cursor = match (select, sel) {
                (false, Some((_, b))) => b,
                _ => (cursor + 1).min(len),
            };
            if !select {
                state.anchor = None;
            }
        }
        EditOp::MoveHome { select } => {
            if select && state.anchor.is_none() {
                state.anchor = Some(cursor);
            }
            state.cursor = 0;
            if !select {
                state.anchor = None;
            }
        }
        EditOp::MoveEnd { select } => {
            if select && state.anchor.is_none() {
                state.anchor = Some(cursor);
            }
            state.cursor = len;
            if !select {
                state.anchor = None;
            }
        }
        EditOp::SelectAll => {
            state.anchor = (len > 0).then_some(0);
            state.cursor = len;
        }
    }
}

fn focused_selected_text(page: &LoadedPage) -> Option<String> {
    let node = page.focused_node.as_ref()?;
    if !is_text_control(node) {
        return None;
    }
    let state = page.focus_state.as_ref()?;
    let value = control_value(node);
    let len = value.chars().count();
    let (a, b) = state.selection_range(len)?;
    let ba = char_to_byte(&value, a);
    let bb = char_to_byte(&value, b);
    Some(value[ba..bb].to_string())
}

fn apply_focus(
    page: &mut LoadedPage,
    next: Option<std::sync::Arc<std::sync::RwLock<Node>>>,
    edit_state: Option<FieldEditState>,
) {
    if let Some(prev) = page.focused_node.take() {
        prev.write().unwrap().focused = false;
    }
    if let Some(node) = &next {
        node.write().unwrap().focused = true;
    }
    page.focused_node = next;
    page.focus_state = edit_state;
}

fn cursor_index_in_line(line: &str, rel_x: f32, font: Option<&engine_text::SystemFont>, font_size: f32) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut best = (chars.len(), f32::MAX);
    let mut acc = String::new();
    for i in 0..=chars.len() {
        let w = font.map_or(i as f32 * font_size * 0.55, |f| engine_text::text_width(f, &acc, font_size));
        let d = (w - rel_x).abs();
        if d < best.1 {
            best = (i, d);
        }
        if let Some(&ch) = chars.get(i) {
            acc.push(ch);
        }
    }
    best.0
}

fn cursor_index_at_point(
    layout_box: &LayoutBox,
    font_set: Option<&FontSet>,
    value: &str,
    doc_x: f32,
    doc_y: f32,
) -> usize {
    let font_size = layout_box
        .computed_style
        .get("font-size")
        .and_then(|v| v.trim().strip_suffix("px"))
        .and_then(|n| n.trim().parse::<f32>().ok())
        .filter(|s| *s > 0.0)
        .unwrap_or(16.0);
    let inset = layout_box
        .computed_style
        .get("border")
        .and_then(|b| b.split_whitespace().find_map(|t| t.strip_suffix("px")?.parse::<f32>().ok()))
        .unwrap_or(0.0)
        + 3.0;
    let rel_x = (doc_x - (layout_box.dimensions.x + inset)).max(0.0);
    let font = font_set.and_then(|s| s.pick(false, false));
    let multiline = layout_box.dimensions.height > font_size * 2.5;
    if !multiline {
        return cursor_index_in_line(value, rel_x, font, font_size);
    }
    let line_height = font_size * 1.2;
    let rel_y = (doc_y - (layout_box.dimensions.y + inset)).max(0.0);
    let target_line = (rel_y / line_height).floor() as usize;
    let mut offset = 0usize;
    let lines: Vec<&str> = value.split('\n').collect();
    let clamped_line = target_line.min(lines.len().saturating_sub(1));
    for (idx, line) in lines.iter().enumerate() {
        if idx == clamped_line {
            return offset + cursor_index_in_line(line, rel_x, font, font_size);
        }
        offset += line.chars().count() + 1;
    }
    value.chars().count()
}

fn collect_focusable_nodes(layout_root: &LayoutBox) -> Vec<std::sync::Arc<std::sync::RwLock<Node>>> {
    fn walk(
        b: &LayoutBox,
        seen: &mut std::collections::HashSet<usize>,
        out: &mut Vec<(i32, usize, std::sync::Arc<std::sync::RwLock<Node>>)>,
    ) {
        if let Some(node) = &b.dom_node {
            let ptr = std::sync::Arc::as_ptr(node) as usize;
            if seen.insert(ptr) {
                let g = node.read().unwrap();
                if let NodeType::Element { tag_name, attributes } = &g.node_type {
                    let hidden = tag_name == "input"
                        && attributes.get("type").is_some_and(|t| t.eq_ignore_ascii_case("hidden"));
                    let disabled = attributes.contains_key("disabled");
                    let visible = b.dimensions.height > 0.0 && (b.dimensions.width > 0.0 || tag_name == "a");
                    let tabindex = attributes.get("tabindex").and_then(|v| v.trim().parse::<i32>().ok());
                    let focusable = visible && !disabled && !hidden && match tabindex {
                        Some(ti) => ti >= 0,
                        None => (tag_name == "a" && attributes.contains_key("href"))
                            || matches!(tag_name.as_str(), "button" | "input" | "select" | "textarea"),
                    };
                    if focusable {
                        let order = tabindex.filter(|&ti| ti > 0).unwrap_or(i32::MAX);
                        let seq = out.len();
                        out.push((order, seq, node.clone()));
                    }
                }
            }
        }
        for child in &b.children {
            walk(child, seen, out);
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    walk(layout_root, &mut seen, &mut out);
    out.sort_by_key(|(order, seq, _)| (*order, *seq));
    out.into_iter().map(|(_, _, n)| n).collect()
}

fn move_focus_tab(
    page: &mut LoadedPage,
    backward: bool,
    width: f32,
    height: f32,
    scroll_target: &mut Option<f32>,
) -> Result<(), String> {
    let focusable = collect_focusable_nodes(&page.page.layout_root);
    if focusable.is_empty() {
        return Ok(());
    }
    let next = match &page.focused_node {
        Some(current) => match focusable.iter().position(|n| std::sync::Arc::ptr_eq(n, current)) {
            Some(pos) => {
                let len = focusable.len();
                let next_pos = if backward { (pos + len - 1) % len } else { (pos + 1) % len };
                focusable[next_pos].clone()
            }
            None => if backward { focusable.last().unwrap().clone() } else { focusable[0].clone() },
        },
        None => if backward { focusable.last().unwrap().clone() } else { focusable[0].clone() },
    };
    let prev = page.focused_node.clone();
    let edit_state = is_text_control(&next).then(|| FieldEditState::at(control_value(&next).chars().count()));
    apply_focus(page, Some(next.clone()), edit_state);
    if let Some(prev_node) = prev.filter(|p| !std::sync::Arc::ptr_eq(p, &next)) {
        page.runtime.dispatch_event(&prev_node, "blur").map_err(|e| format!("blur_error: {e}"))?;
    }
    page.runtime.dispatch_event(&next, "focus").map_err(|e| format!("focus_error: {e}"))?;
    if page.page.stylesheet.has_focus_rules() || page.runtime.has_event_listener("focus") || page.runtime.has_event_listener("blur") {
        page.relayout(width, height);
    }
    if let Some(target_box) = page.page.layout_root.find_box_for_node(&next) {
        let cy = target_box.dimensions.y + target_box.dimensions.height / 2.0;
        let extent = page.page.layout_root.content_extent();
        let target = clamp_scroll_offset(cy - height / 2.0, extent, height);
        *scroll_target = Some(target);
    }
    Ok(())
}

fn compute_text_selection(
    layout_root: &LayoutBox,
    font_set: Option<&FontSet>,
    from: (f32, f32),
    to: (f32, f32),
) -> (Vec<engine_layout::Rect>, Option<String>) {
    if (from.0 - to.0).abs() < 3.0 && (from.1 - to.1).abs() < 3.0 {
        return (Vec::new(), None);
    }
    let ((sx, sy), (ex, ey)) = if from.1 < to.1 - 6.0 || ((from.1 - to.1).abs() <= 6.0 && from.0 <= to.0) {
        (from, to)
    } else {
        (to, from)
    };
    let mut rects = Vec::new();
    let mut parts = Vec::new();
    let font = font_set.and_then(|s| s.pick(false, false));

    fn collect_boxes<'a>(b: &'a LayoutBox, out: &mut Vec<&'a LayoutBox>) {
        match &b.box_type {
            BoxType::Text(_) => out.push(b),
            BoxType::Replaced => {}
            _ => {
                for c in &b.children {
                    collect_boxes(c, out);
                }
            }
        }
    }
    let mut text_boxes = Vec::new();
    collect_boxes(layout_root, &mut text_boxes);

    for b in text_boxes {
        let BoxType::Text(raw) = &b.box_type else { continue };
        let text = b.trimmed_text.as_deref().unwrap_or(raw.as_str());
        if text.trim().is_empty() || b.dimensions.width <= 0.0 || b.dimensions.height <= 0.0 {
            continue;
        }
        let top = b.dimensions.y;
        let bottom = b.dimensions.y + b.dimensions.height;
        if bottom < sy || top > ey {
            continue;
        }
        let font_size = b
            .computed_style
            .get("font-size")
            .and_then(|v| v.trim().strip_suffix("px"))
            .and_then(|n| n.trim().parse::<f32>().ok())
            .filter(|s| *s > 0.0)
            .unwrap_or(16.0);
        let single_line_drag = (sy - ey).abs() <= b.dimensions.height.max(20.0) && sy >= top - 4.0 && ey <= bottom + 4.0;
        let chars: Vec<char> = text.chars().collect();
        let (a_idx, b_idx) = if single_line_drag {
            let a = cursor_index_in_line(text, (sx - b.dimensions.x).max(0.0), font, font_size);
            let z = cursor_index_in_line(text, (ex - b.dimensions.x).max(0.0), font, font_size);
            (a.min(z), a.max(z))
        } else {
            let a = if sy >= top - 4.0 && sy <= bottom + 4.0 {
                cursor_index_in_line(text, (sx - b.dimensions.x).max(0.0), font, font_size)
            } else {
                0
            };
            let z = if ey >= top - 4.0 && ey <= bottom + 4.0 {
                cursor_index_in_line(text, (ex - b.dimensions.x).max(0.0), font, font_size)
            } else {
                chars.len()
            };
            (a.min(chars.len()), z.min(chars.len()))
        };
        if a_idx < b_idx {
            let ba = char_to_byte(text, a_idx);
            let bb = char_to_byte(text, b_idx);
            let selected = text[ba..bb].trim();
            if !selected.is_empty() {
                parts.push(selected.to_string());
                let x0 = b.dimensions.x + font.map_or(a_idx as f32 * font_size * 0.55, |f| engine_text::text_width(f, &text[..ba], font_size));
                let x1 = b.dimensions.x + font.map_or(b_idx as f32 * font_size * 0.55, |f| engine_text::text_width(f, &text[..bb], font_size));
                rects.push(engine_layout::Rect {
                    x: x0.min(b.dimensions.x + b.dimensions.width),
                    y: b.dimensions.y,
                    width: (x1 - x0).max(2.0).min(b.dimensions.width),
                    height: b.dimensions.height,
                });
            }
        }
    }
    let joined = parts.join(" ");
    (rects, (!joined.is_empty()).then_some(joined))
}

fn append_control_value_at(node: &std::sync::Arc<std::sync::RwLock<Node>>, text: &str, cursor: Option<usize>) -> usize {
    let mut val = control_value(node);
    let len = val.chars().count();
    let idx = cursor.unwrap_or(len).min(len);
    let bpos = char_to_byte(&val, idx);
    val.insert_str(bpos, text);
    set_control_value(node, val);
    idx + text.chars().count()
}

#[cfg(test)]
fn backspace_control_value(node: &std::sync::Arc<std::sync::RwLock<Node>>) {
    let mut state = FieldEditState::at(control_value(node).chars().count());
    edit_control(node, &mut state, EditOp::Backspace);
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
/// El grupo se agrupa por "form owner" (§ del spec real): mismo `name` Y
/// mismo `<form>` ancestro mas cercano (via `find_form_ancestor`) - dos
/// radios sin NINGUN `<form>` ancestro tambien se agrupan entre si (ambos
/// tienen "sin dueño" como form owner, que sigue siendo el MISMO valor).
/// Antes se agrupaba por `name` a secas en el documento ENTERO, asi que
/// dos formularios distintos que reutilizaran el mismo `name` se pisaban
/// entre si - poco comun en paginas reales, pero un formulario real no
/// puede alcanzar ese estado por clics del usuario. Simplificacion que
/// SIGUE declarada: no resuelve el atributo `form="id-de-otro-form"` (un
/// radio puede pertenecer a un `<form>` que no sea su ancestro via ese
/// atributo, caso mas raro todavia) - solo el ancestro mas cercano.
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
        let form_owner = find_form_ancestor(node);
        let same_form_owner = |other: &std::sync::Arc<std::sync::RwLock<Node>>| match (&form_owner, find_form_ancestor(other)) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, &b),
            (None, None) => true,
            _ => false,
        };
        for other in Node::find_all_by_tag(dom_root, "input") {
            if std::sync::Arc::ptr_eq(&other, node) {
                continue;
            }
            if is_radio(&other) && input_name(&other).as_deref() == Some(group.as_str()) && same_form_owner(&other) {
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

/// El `<form>` ancestro mas cercano de `node` (el propio `node` incluido) -
/// mismo patron de recorrido que `find_link_target`, pero buscando `<form>`
/// en vez de `<a>`. Necesario tanto para Enter en un input de texto
/// (`EngineServer::press_key`/`type_text`) como para un clic en un boton
/// submit (`EngineServer::click`/`find_submit_control`) - ninguno de los
/// dos sabe de antemano si esta dentro de un formulario ni cual, solo el
/// propio DOM lo sabe.
fn find_form_ancestor(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> Option<std::sync::Arc<std::sync::RwLock<Node>>> {
    let mut current = Some(node.clone());
    while let Some(n) = current {
        let is_form = {
            let guard = n.read().unwrap();
            matches!(&guard.node_type, NodeType::Element { tag_name, .. } if tag_name == "form")
        };
        if is_form {
            return Some(n);
        }
        let parent = n.read().unwrap().parent.as_ref().and_then(std::sync::Weak::upgrade);
        current = parent;
    }
    None
}

/// El boton submit real mas cercano en los ANCESTROS de `node` (el clic
/// puede aterrizar en un `<span>`/texto anidado dentro del boton, igual
/// que `find_link_target` con un `<b>` dentro de un `<a>`) - `<button>`
/// SIN `type` es submit por defecto en el spec real (solo `type="button"`/
/// `"reset"` lo desactivan), e `<input type="submit"|"image">` tambien
/// dispara un envio.
fn find_submit_control(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> Option<std::sync::Arc<std::sync::RwLock<Node>>> {
    let mut current = Some(node.clone());
    while let Some(n) = current {
        let is_submit = {
            let guard = n.read().unwrap();
            match &guard.node_type {
                NodeType::Element { tag_name, attributes } if tag_name == "button" => !matches!(
                    attributes.get("type").map(|t| t.trim().to_ascii_lowercase()).as_deref(),
                    Some("button" | "reset")
                ),
                NodeType::Element { tag_name, attributes } if tag_name == "input" => matches!(
                    attributes.get("type").map(|t| t.trim().to_ascii_lowercase()).as_deref(),
                    Some("submit" | "image")
                ),
                _ => false,
            }
        };
        if is_submit {
            return Some(n);
        }
        let parent = n.read().unwrap().parent.as_ref().and_then(std::sync::Weak::upgrade);
        current = parent;
    }
    None
}

fn is_textarea(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> bool {
    matches!(&node.read().unwrap().node_type, NodeType::Element { tag_name, .. } if tag_name == "textarea")
}

/// Los pares nombre=valor de TODOS los controles con `name` dentro de
/// `form` (recorrido de TODO su subarbol, no solo hijos directos - un
/// `<input>` casi siempre esta envuelto en `<label>`/`<div>` intermedios),
/// en el mismo orden de documento que un envio real. `submit_control` es
/// el boton que disparo el envio (`None` si fue por Enter en un input de
/// texto, ver `EngineServer::submit_form`) - solo SU par entra si tiene
/// `name`, igual que el spec real: un envio por Enter no incluye ningun
/// boton, y de los varios botones submit que puede tener un formulario
/// solo el clicado cuenta.
///
/// Simplificaciones declaradas: sin asociacion `form="id"` (solo cuenta
/// ser DESCENDIENTE del `<form>`, no el atributo que asocia un control de
/// fuera); `<input type="file">` se omite (no hay datos de fichero que
/// enviar); un `<select>` sin ningun `<option selected>` usa el PRIMER
/// `<option>` (asi es el valor por defecto real); deshabilitados
/// (`disabled` presente) se omiten por completo, igual que el spec real.
fn collect_form_data(
    form: &std::sync::Arc<std::sync::RwLock<Node>>,
    submit_control: Option<&std::sync::Arc<std::sync::RwLock<Node>>>,
) -> Vec<(String, String)> {
    let mut data = Vec::new();
    for tag in ["input", "select", "textarea"] {
        for control in Node::find_all_by_tag(form, tag) {
            let (tag_name, attributes) = {
                let guard = control.read().unwrap();
                let NodeType::Element { tag_name, attributes } = &guard.node_type else {
                    continue;
                };
                (tag_name.clone(), attributes.clone())
            };
            if attributes.contains_key("disabled") {
                continue;
            }
            let Some(name) = attributes.get("name").map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) else {
                continue;
            };

            match tag_name.as_str() {
                "input" => {
                    let input_type = attributes.get("type").map(|t| t.trim().to_ascii_lowercase());
                    match input_type.as_deref() {
                        Some("checkbox" | "radio") => {
                            if attributes.contains_key("checked") {
                                data.push((name, attributes.get("value").cloned().unwrap_or_else(|| "on".to_string())));
                            }
                        }
                        Some("submit" | "button" | "reset" | "image") => {
                            if submit_control.is_some_and(|s| std::sync::Arc::ptr_eq(s, &control)) {
                                data.push((name, attributes.get("value").cloned().unwrap_or_default()));
                            }
                        }
                        Some("file") => {}
                        _ => data.push((name, attributes.get("value").cloned().unwrap_or_default())),
                    }
                }
                "textarea" => {
                    let value = attributes.get("value").cloned().unwrap_or_else(|| Node::text_content(&control));
                    data.push((name, value));
                }
                "select" => {
                    let options = Node::find_all_by_tag(&control, "option");
                    let selected = options
                        .iter()
                        .find(|opt| {
                            let g = opt.read().unwrap();
                            matches!(&g.node_type, NodeType::Element { attributes, .. } if attributes.contains_key("selected"))
                        })
                        .or_else(|| options.first());
                    if let Some(opt) = selected {
                        let value_attr = {
                            let g = opt.read().unwrap();
                            match &g.node_type {
                                NodeType::Element { attributes, .. } => attributes.get("value").cloned(),
                                _ => None,
                            }
                        };
                        let value = value_attr.unwrap_or_else(|| Node::text_content(opt));
                        data.push((name, value));
                    }
                }
                _ => {}
            }
        }
    }
    data
}

/// Construye la URL final de un envio GET - resuelve `action` (o la propia
/// pagina si esta vacio/ausente, ya que el atributo se puede omitir)
/// contra la URL de la pagina actual, y REEMPLAZA su query string con los
/// datos del formulario (nunca la anexa - asi es el spec real). Separada
/// de `EngineServer::submit_form` (que SI navega de verdad) a proposito:
/// es logica pura sobre strings/URLs, sin tocar `self` ni la red, y por
/// tanto se puede probar sin levantar ninguna pagina.
fn build_get_submit_url(page_url_str: &str, action_attr: Option<&str>, data: &[(String, String)]) -> Result<url::Url, String> {
    let mut target_url = resolve_submit_action(page_url_str, action_attr)?;
    target_url
        .query_pairs_mut()
        .clear()
        .extend_pairs(data.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    Ok(target_url)
}

/// Resuelve el `action` de un formulario contra la URL de la pagina SIN
/// tocar su query string - la mitad compartida entre el envio GET (que
/// luego la reemplaza, ver `build_get_submit_url`) y el POST (que la deja
/// como este, porque sus datos van en el cuerpo: un
/// `action="/buscar?pagina=2"` conserva ese `pagina=2`).
///
/// `action` ausente o vacio envia al documento actual, ya que el atributo
/// se puede omitir.
fn resolve_submit_action(page_url_str: &str, action_attr: Option<&str>) -> Result<url::Url, String> {
    let page_url = url::Url::parse(page_url_str).map_err(|_| "la URL de la página actual no es válida".to_string())?;
    let action = action_attr.filter(|a| !a.trim().is_empty()).unwrap_or(page_url_str);
    page_url.join(action).map_err(|_| format!("action de formulario inválida: {action}"))
}

/// Codifica los pares del formulario como
/// `application/x-www-form-urlencoded`, el tipo por defecto real de un
/// `<form method="post">` sin `enctype`.
///
/// Se construye con `url::form_urlencoded` (el mismo crate `url` que ya
/// resuelve el resto de URLs del motor) y NO a mano: el escapado
/// percent-encoding tiene reglas propias en este contexto - el espacio se
/// codifica como `+`, no como `%20` - y equivocarse ahi corrompe
/// silenciosamente cualquier dato con espacios o acentos.
fn encode_form_body(data: &[(String, String)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(data.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .finish()
        .into_bytes()
}

pub async fn run_stdio() -> io::Result<()> {
    let mut lines = BoundedLines::new(BufReader::new(tokio::io::stdin()), MAX_REQUEST_LINE_BYTES);
    let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());
    // Con `NAVEGADOR_IA_BROKER` la red, las cookies y el almacenamiento los
    // pone otro proceso (ADR 0001, etapa 2) y este no abre el perfil en
    // disco. Si se pidio broker y no se puede conectar, el motor NO arranca:
    // seguir con la red y el disco propios en silencio seria justo lo que la
    // ADR prohibe (principio 7 del plan).
    let mut server = match engine_net::broker_remote::from_env().await? {
        Some(remote) => {
            tracing::info!("[engine] broker remoto: soy {}", remote.renderer());
            EngineServer::with_broker(std::sync::Arc::new(remote), "remote")
        }
        None => EngineServer::with_broker(engine_net::LocalBroker::persistent().shared(), "local"),
    };

    write_response(&mut stdout, server.ready_response(Some("boot".to_string()))).await?;

    // Reloj de fondo real (ver el aviso de `EngineServer::
    // tick_active_tab_timers`): antes de esto, un `setTimeout`/
    // `setInterval` solo avanzaba cuando llegaba un comando NDJSON nuevo
    // (clic, tecla, navegar, redimensionar...). NO se reparte en una
    // tarea de Tokio aparte (`tokio::spawn`) - Boa usa `Rc` internamente
    // (su `Context` no es `Send`), asi que `EngineServer` tampoco lo es y
    // ninguna tarea separada podria tocarlo. `tokio::select!` dentro de
    // este MISMO bucle consigue el mismo efecto (reaccionar a "paso el
    // tiempo" sin bloquear la lectura de la siguiente linea) sin
    // necesitar compartir nada entre tareas.
    //
    // NO despierta a intervalos fijos: duerme hasta el proximo trabajo real
    // (`EngineServer::next_deadline`: un temporizador de JavaScript de la
    // pestaña activa, o el momento de descartar una pestaña inactiva). Sin
    // nada pendiente espera solo a que llegue una peticion, asi que un
    // navegador en reposo no gasta CPU. Entre dos despertares se deja al menos
    // `MIN_TICK_INTERVAL` para que una animacion pesada no repinte sin parar.
    let mut last_tick = std::time::Instant::now();

    // Huella del ultimo `State` escrito, sea respuesta o publicacion
    // espontanea: es lo ultimo que el consumidor tiene en pantalla. Un
    // temporizador que dispara sin cambiar nada visible (un `setInterval`
    // que solo consulta algo) no genera otra publicacion identica.
    let mut last_state: Option<u64> = None;

    // Lineas que llegaron mientras una navegacion estaba en curso y esperan
    // su turno, en orden (ver `handle_interruptible`).
    let mut backlog: std::collections::VecDeque<Result<String, RejectedLine>> = std::collections::VecDeque::new();

    loop {
        let next = match backlog.pop_front() {
            Some(queued) => queued,
            None => {
                let wake = server.next_deadline().map(|deadline| deadline.max(last_tick + MIN_TICK_INTERVAL));
                let sleep = async {
                    match wake {
                        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    line = lines.next_line() => match line? {
                        None => break,
                        Some(line) => line,
                    },
                    () = sleep => {
                        last_tick = std::time::Instant::now();
                        for publication in server.tick(last_tick) {
                            let fingerprint = state_fingerprint(&publication);
                            // Los estados se deduplican; lo demas (la lista de pestañas) se publica siempre.
                            if fingerprint.is_none() || fingerprint != last_state {
                                if fingerprint.is_some() {
                                    last_state = fingerprint;
                                }
                                write_response(&mut stdout, publication).await?;
                            }
                        }
                        continue;
                    }
                }
            }
        };
        let line = match next {
            Ok(line) => line,
            // Una linea desmedida o que no es UTF-8 se contesta y se sigue:
            // antes la primera crecia sin limite y la segunda mataba el
            // proceso entero con `InvalidData`.
            Err(rejected) => {
                write_response(&mut stdout, EngineResponse::Error { id: None, message: rejected.message() }).await?;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<EngineRequest>(&line) {
            Ok(request) => {
                let (response, should_shutdown) = handle_interruptible(&mut server, request, &mut lines, &mut backlog).await?;
                if let Some(fingerprint) = state_fingerprint(&response) {
                    last_state = Some(fingerprint);
                }
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

/// Atiende una peticion. Un `navigate` puede tardar decenas de segundos en una
/// red mala, y mientras tanto el motor no leia nada de la entrada: ni el
/// usuario podia detenerlo ni una navegacion nueva lo sustituia hasta que el
/// anterior terminara (tarea 1.3 del plan).
///
/// Mientras un `navigate` esta en curso se sigue leyendo la entrada:
/// - un `stop` o un `navigate` nuevo ABANDONA el en curso (que se contesta con
///   un error `navegacion_cancelada`, para que quien esperaba su respuesta no
///   se quede colgado) y la linea se procesa a continuacion;
/// - cualquier otra linea (un scroll, un `resize`...) espera su turno en
///   `backlog`, en orden, como si nunca se hubiera adelantado.
///
/// Abandonar una navegacion es seguro porque `navigate` no toca el estado del
/// servidor hasta el final (todo lo anterior es red y calculo): si se cancela
/// en un `await`, la pagina anterior sigue intacta.
async fn handle_interruptible<R: AsyncBufRead + Unpin>(
    server: &mut EngineServer,
    request: EngineRequest,
    lines: &mut BoundedLines<R>,
    backlog: &mut std::collections::VecDeque<Result<String, RejectedLine>>,
) -> io::Result<(EngineResponse, bool)> {
    let EngineRequest::Navigate { id: navigate_id, .. } = &request else {
        return Ok(server.handle(request).await);
    };
    let navigate_id = navigate_id.clone();
    let mut in_flight = std::pin::pin!(server.handle(request));
    let mut input_open = true;
    loop {
        if !input_open {
            return Ok(in_flight.await);
        }
        tokio::select! {
            result = &mut in_flight => return Ok(result),
            line = lines.next_line() => match line? {
                None => input_open = false,
                Some(next) => {
                    let interrupts = matches!(
                        &next,
                        Ok(text) if matches!(
                            serde_json::from_str::<EngineRequest>(text),
                            Ok(EngineRequest::Stop { .. } | EngineRequest::Navigate { .. })
                        )
                    );
                    backlog.push_back(next);
                    if interrupts {
                        return Ok((
                            EngineServer::error(navigate_id, "navegacion_cancelada: la carga se detuvo o se pidio otra pagina".to_string()),
                            false,
                        ));
                    }
                }
            },
        }
    }
}

/// Huella de lo que un `State` muestra, sin su `id`: dos estados con la
/// misma huella son indistinguibles para quien los pinta. `None` si la
/// respuesta no es un `State` (un error de render, por ejemplo), que nunca
/// se deduplica.
fn state_fingerprint(response: &EngineResponse) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let EngineResponse::State { tab_id, scroll_offset_y, url, title, screenshot, elements, requires_javascript, can_go_back, can_go_forward, focused_is_text, selected_text, .. } = response else {
        return None;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (tab_id, scroll_offset_y.to_bits(), url, title, screenshot, requires_javascript, can_go_back, can_go_forward, focused_is_text, selected_text).hash(&mut hasher);
    serde_json::to_string(elements).unwrap_or_default().hash(&mut hasher);
    Some(hasher.finish())
}

/// Tope de una linea de peticion (plan H06). Las peticiones reales son
/// pequenas; la mayor es `type_text`, cuyo texto la interfaz limita a 100.000
/// caracteres, que escapados en JSON no llegan a 1 MiB.
const MAX_REQUEST_LINE_BYTES: usize = 1024 * 1024;

/// Por que se rechazo una linea sin llegar a parsearla.
#[derive(Debug, PartialEq, Eq)]
enum RejectedLine {
    TooLong { bytes: usize },
    NotUtf8,
}

impl RejectedLine {
    fn message(&self) -> String {
        match self {
            RejectedLine::TooLong { bytes } => format!("request_too_large: la linea supera {MAX_REQUEST_LINE_BYTES} bytes ({bytes} leidos)"),
            RejectedLine::NotUtf8 => "invalid_request: la linea no es UTF-8".to_string(),
        }
    }
}

/// Lector de lineas con tope, sustituto de `AsyncBufReadExt::lines`.
///
/// Tiene que ser seguro ante cancelacion: `run_stdio` lo usa dentro de un
/// `tokio::select!` con el reloj de temporizadores, que puede ganar a mitad
/// de una linea. Por eso la linea a medias vive en `self.buf` y no en el
/// futuro: entre `fill_buf` (seguro ante cancelacion) y `consume` no hay
/// ningun `await`, asi que no se pierde ni se duplica ningun byte.
struct BoundedLines<R> {
    reader: R,
    max: usize,
    buf: Vec<u8>,
    /// Bytes de la linea en curso, incluidos los descartados.
    seen: usize,
}

impl<R: AsyncBufRead + Unpin> BoundedLines<R> {
    fn new(reader: R, max: usize) -> Self {
        Self { reader, max, buf: Vec::new(), seen: 0 }
    }

    /// `None` al cerrarse la entrada; `Some(Err)` para una linea rechazada,
    /// que se consume entera para que la siguiente empiece limpia.
    async fn next_line(&mut self) -> io::Result<Option<Result<String, RejectedLine>>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                // Fin de la entrada: una ultima linea sin salto tambien cuenta.
                if self.seen == 0 {
                    return Ok(None);
                }
                return Ok(Some(self.finish_line()));
            }
            let (chunk, found_newline) = match available.iter().position(|&b| b == b'\n') {
                Some(i) => (&available[..i], true),
                None => (available, false),
            };
            let chunk_len = chunk.len();
            self.seen += chunk_len;
            if self.seen <= self.max {
                self.buf.extend_from_slice(chunk);
            } else {
                // Sin guardar nada mas: la memoria no crece con la linea.
                self.buf.clear();
            }
            self.reader.consume(chunk_len + usize::from(found_newline));
            if found_newline {
                return Ok(Some(self.finish_line()));
            }
        }
    }

    fn finish_line(&mut self) -> Result<String, RejectedLine> {
        let seen = std::mem::take(&mut self.seen);
        let bytes = std::mem::take(&mut self.buf);
        if seen > self.max {
            return Err(RejectedLine::TooLong { bytes: seen });
        }
        let mut line = String::from_utf8(bytes).map_err(|_| RejectedLine::NotUtf8)?;
        if line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    }
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
    collect_interactive_targets(layout_root).into_iter().map(|(element, _)| element).collect()
}

/// Tope de la lista `elements` (tarea 3.3). Antes se devolvia TODO elemento
/// de la pagina (miles en una web real: 4,8 MB de JSON en Wikipedia) y el
/// agente se ahogaba. Quien necesite mas, puede desplazar la pagina.
const MAX_INTERACTIVE_ELEMENTS: usize = 500;
/// El texto de cada elemento se recorta: un menu entero dentro de un `<nav>`
/// no ayuda a decidir donde clicar.
const MAX_ELEMENT_TEXT_CHARS: usize = 120;

/// Roles ARIA que convierten cualquier etiqueta en algo con lo que se puede
/// interactuar.
const INTERACTIVE_ROLES: &[&str] = &[
    "button", "link", "textbox", "searchbox", "combobox", "checkbox", "radio", "switch", "tab",
    "menuitem", "menuitemcheckbox", "menuitemradio", "option", "slider", "spinbutton", "listbox",
];

/// ¿Se puede interactuar con este elemento? Es la definicion de la lista
/// `elements` (tarea 3.3): enlaces con destino, controles de formulario y
/// lo que la propia pagina declara como accionable (`role`, `onclick`,
/// `tabindex`, `contenteditable`). Un `<div>` o un `<p>` sin nada de eso no
/// es un objetivo: su texto ya viaja en el de su ancestro interactivo y en
/// la captura.
fn is_interactive_element(tag_name: &str, attributes: &HashMap<String, String>) -> bool {
    match tag_name {
        "a" => return attributes.contains_key("href"),
        "button" | "select" | "textarea" | "summary" => return true,
        "input" => return attributes.get("type").map(|kind| !kind.eq_ignore_ascii_case("hidden")).unwrap_or(true),
        _ => {}
    }
    if attributes.get("role").is_some_and(|role| INTERACTIVE_ROLES.contains(&role.trim().to_ascii_lowercase().as_str())) {
        return true;
    }
    if attributes.contains_key("onclick") {
        return true;
    }
    if attributes.get("contenteditable").is_some_and(|value| !value.eq_ignore_ascii_case("false")) {
        return true;
    }
    // `tabindex="-1"` hace el elemento enfocable solo por codigo, no por el
    // usuario: no es un objetivo.
    attributes.get("tabindex").and_then(|value| value.trim().parse::<i32>().ok()).is_some_and(|index| index >= 0)
}

/// Un elemento sin area no se ve ni se puede clicar. Excepcion: un enlace de
/// anchura cero con altura (su contenido es un icono o un elemento vacio que
/// el motor aun no sabe medir) sigue siendo un destino de navegacion util.
fn is_visible_target(tag_name: &str, rect: &ElementRect) -> bool {
    rect.height > 0.0 && (rect.width > 0.0 || tag_name == "a")
}

/// La lista de elementos interactivos y visibles, cada uno con su nodo del
/// DOM. El `id` de cada elemento es su posicion en esta lista: tanto
/// `state` como `click_element`/`type_into_element` la recalculan con esta
/// misma funcion, asi que coinciden mientras la pagina no cambie.
fn collect_interactive_targets(
    layout_root: &engine_layout::LayoutBox,
) -> Vec<(InteractiveElement, std::sync::Arc<std::sync::RwLock<Node>>)> {
    let mut targets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    collect_targets_recursive(layout_root, &mut targets, &mut seen);
    targets
}

/// El texto VISIBLE de un elemento para la lista `elements` del protocolo
/// NDJSON (Fase 32) - camina el arbol de LAYOUT (ya filtrado por
/// `LayoutTreeBuilder::build_node`), no el DOM crudo. Antes de esto, este
/// campo usaba `Node::text_content` (el mismo que implementa el
/// `.textContent` real de JS), que camina el DOM SIN filtrar - correcto
/// para `.textContent` (que si debe incluir literalmente el codigo fuente
/// de un `<script>`/`<style>` como texto, es el comportamiento real del
/// spec) pero equivocado aqui: un `<noscript>` (RAWTEXT cuando el
/// scripting esta activado, ver el aviso de `build_node`) o un `<script>`
/// dejaba su marcado/codigo crudo colandose en el campo `text` de
/// `<body>`, visible para quien consuma este protocolo (la capa de IA/
/// interaccion) aunque nunca se pintara en pantalla. Reusar el arbol de
/// layout en vez de duplicar la lista de tags excluidos aqui: una sola
/// fuente de verdad sobre que cuenta como "visible".
/// Minimo de texto visible para dar una pagina por "con contenido".
/// 40 caracteres es aproximadamente una frase corta: por debajo de eso no
/// hay nada que un humano reconozca como pagina. El numero es un JUICIO,
/// no un valor del spec - se elige bajo a proposito para no acusar de
/// "vacia" a una pagina real y escueta (un 404 de texto plano, una landing
/// de una sola linea): preferimos callar de mas que mentir.
const MIN_VISIBLE_TEXT_CHARS: usize = 40;

/// Fase 39: ¿esta pagina esta en blanco PORQUE su contenido lo genera
/// JavaScript que este motor todavia no ejecuta?
///
/// Se responde con lo unico que se puede observar sin ejecutar nada:
/// 1. el arbol de LAYOUT no tiene practicamente texto visible (se reusa
///    `collect_visible_text`, que ya filtra `<script>`/`<noscript>`, en vez
///    de mirar el DOM crudo - si no, el propio codigo del script contaria
///    como "contenido"), y
/// 2. el documento SI trae `<script>`.
///
/// Las dos condiciones juntas describen exactamente la cascara vacia que
/// sirven React/Next/Vue/Shopify. Verificado en vivo: `ignislove.com`
/// devuelve 0 caracteres de texto visible con 7 `<script>`; Google y
/// Wikipedia mandan su texto en el HTML y no se marcan.
///
/// Lo que esto NO hace, a proposito: no afirma que la pagina FUNCIONARIA
/// con un motor de JS completo (puede fallar por otras razones), no
/// distingue "el script no se ejecuto" de "se ejecuto y no pinto nada", y
/// da un falso positivo en una pagina legitimamente casi vacia que ademas
/// lleve un script (analitica, por ejemplo). Ese falso positivo se acepta:
/// el coste es un aviso de mas en una pagina que igualmente se ve vacia.
fn page_content_requires_javascript(
    dom_root: &std::sync::Arc<std::sync::RwLock<Node>>,
    layout_root: &LayoutBox,
) -> bool {
    let visible = collect_visible_text(layout_root);
    if visible.trim().chars().count() >= MIN_VISIBLE_TEXT_CHARS {
        return false;
    }
    !Node::find_all_by_tag(dom_root, "script").is_empty()
}

/// ¿Es la respuesta de un error HTTP (4xx/5xx) una pagina que merece mostrarse?
/// HTML con algo de texto visible: un cuerpo vacio, un JSON o un binario
/// quedan como `http_error`, que es lo mas util que se puede decir de ellos.
fn is_renderable_error_page(response: &engine_net::NetworkResponse) -> bool {
    if response.status_code < 400 {
        return false;
    }
    let is_html = response
        .headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .is_some_and(|value| value.contains("text/html") || value.contains("application/xhtml"));
    if !is_html {
        return false;
    }
    let html = response.text();
    let dom = engine_dom::HtmlParser::parse(&html);
    // Con algo que leer: sin el codigo de scripts y estilos.
    Node::find_all_by_tag(&dom, "body")
        .first()
        .is_some_and(|body| collect_readable_text_of_dom(body).chars().filter(|c| !c.is_whitespace()).count() >= 10)
}

/// El texto de un nodo del DOM sin descender en `<script>`/`<style>`/`<noscript>`.
fn collect_readable_text_of_dom(node: &std::sync::Arc<std::sync::RwLock<Node>>) -> String {
    let guard = node.read().unwrap();
    match &guard.node_type {
        NodeType::Text(text) => text.clone(),
        NodeType::Element { tag_name, .. } if matches!(tag_name.as_str(), "script" | "style" | "noscript" | "template") => String::new(),
        _ => guard.children.iter().map(collect_readable_text_of_dom).collect::<Vec<_>>().join(" "),
    }
}

/// Tope del texto que devuelve `get_page_text`: lo que cabe holgado en el
/// prompt de un modelo junto a la lista de elementos.
const MAX_PAGE_TEXT_CHARS: usize = 12_000;

/// La caja del contenido principal de la pagina (`<main>` o `role="main"`; en su
/// defecto, el primer `<article>`), si existe y tiene texto de verdad. Lo que
/// queda fuera (menus, cabeceras, pies) es justo lo que no responde a nada.
fn find_main_content(root: &LayoutBox) -> Option<&LayoutBox> {
    fn matches_main(layout_box: &LayoutBox, article: bool) -> bool {
        let Some(node) = &layout_box.dom_node else { return false };
        let guard = node.read().unwrap();
        let NodeType::Element { tag_name, attributes } = &guard.node_type else { return false };
        if article {
            tag_name == "article"
        } else {
            tag_name == "main" || attributes.get("role").is_some_and(|role| role.eq_ignore_ascii_case("main"))
        }
    }
    fn first(layout_box: &LayoutBox, article: bool) -> Option<&LayoutBox> {
        if matches_main(layout_box, article) {
            return Some(layout_box);
        }
        layout_box.children.iter().find_map(|child| first(child, article))
    }
    // Con texto de sobra para ser el contenido y no un `<main>` vacio de cascara.
    const MIN_MAIN_CHARS: usize = 200;
    [false, true]
        .into_iter()
        .filter_map(|article| first(root, article))
        .find(|candidate| collect_visible_text(candidate).trim().chars().count() >= MIN_MAIN_CHARS)
}

/// El texto visible de la pagina para leerlo: espacios colapsados y un salto de
/// linea entre bloques (parrafos, filas, items de lista). `bool`: si se cortó
/// por llegar al tope. A diferencia de `collect_visible_text` (que pega las
/// hojas sin separador, suficiente para el `text` de un elemento), esto tiene
/// que poder leerse.
fn collect_readable_text(layout_box: &LayoutBox, max_chars: usize) -> (String, bool) {
    fn visit(layout_box: &LayoutBox, out: &mut String, max_chars: usize) -> bool {
        if out.chars().count() >= max_chars {
            return true;
        }
        let is_block = matches!(layout_box.box_type, BoxType::Block) && layout_box.dom_node.is_some();
        if is_block && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if let BoxType::Text(content) = &layout_box.box_type {
            let words = content.split_whitespace().collect::<Vec<_>>().join(" ");
            if !words.is_empty() {
                if !out.is_empty() && !out.ends_with('\n') && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(&words);
            }
        }
        for child in &layout_box.children {
            if visit(child, out, max_chars) {
                return true;
            }
        }
        if is_block && !out.ends_with('\n') {
            out.push('\n');
        }
        false
    }
    let mut out = String::new();
    // Un solo nodo de texto enorme se añade entero antes de poder comprobar el
    // tope, de ahi que tambien se mire el tamaño al terminar.
    let truncated = visit(layout_box, &mut out, max_chars) || out.chars().count() > max_chars;
    if truncated {
        out = out.chars().take(max_chars).collect();
    }
    (out.trim().to_string(), truncated)
}

fn collect_visible_text(layout_box: &LayoutBox) -> String {
    let mut text = String::new();
    collect_visible_text_recursive(layout_box, &mut text);
    text
}

fn collect_visible_text_recursive(layout_box: &LayoutBox, out: &mut String) {
    if let BoxType::Text(content) = &layout_box.box_type {
        out.push_str(content);
    }
    for child in &layout_box.children {
        collect_visible_text_recursive(child, out);
    }
}

fn collect_targets_recursive(
    layout_box: &engine_layout::LayoutBox,
    targets: &mut Vec<(InteractiveElement, std::sync::Arc<std::sync::RwLock<Node>>)>,
    seen: &mut std::collections::HashSet<usize>,
) {
    if targets.len() >= MAX_INTERACTIVE_ELEMENTS {
        return;
    }
    if let Some(node) = &layout_box.dom_node {
        let node_guard = node.read().unwrap();
        if let NodeType::Element { tag_name, attributes } = &node_guard.node_type {
            let rect = ElementRect {
                x: layout_box.dimensions.x,
                y: layout_box.dimensions.y,
                width: layout_box.dimensions.width,
                height: layout_box.dimensions.height,
            };
            // Un mismo nodo puede tener varias cajas (anonimas, fragmentos):
            // se lista una sola vez, la primera.
            let is_new = seen.insert(std::sync::Arc::as_ptr(node) as usize);
            if is_new && is_interactive_element(tag_name, attributes) && is_visible_target(tag_name, &rect) {
                let selector = attributes
                    .get("id")
                    .map(|value| format!("#{value}"))
                    .unwrap_or_else(|| tag_name.clone());
                targets.push((
                    InteractiveElement {
                        id: targets.len() as u32,
                        tag_name: tag_name.clone(),
                        text: short_label(&collect_visible_text(layout_box)),
                        rect,
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
                    },
                    node.clone(),
                ));
            }
        }
    }
    for child in &layout_box.children {
        collect_targets_recursive(child, targets, seen);
    }
}

/// Texto de un elemento para la lista: espacios colapsados (el layout
/// conserva los de entre etiquetas) y recortado.
fn short_label(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_ELEMENT_TEXT_CHARS {
        return collapsed;
    }
    let mut shortened: String = collapsed.chars().take(MAX_ELEMENT_TEXT_CHARS).collect();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod interruptible_navigation_tests {
    use super::*;
    use engine_net::broker::BrokerFuture;
    use engine_net::storage::{QuotaExceeded, StorageKind};
    use engine_net::{LocalBroker, NetworkError, NetworkResponse, ResourceBroker};
    use std::collections::VecDeque;
    use std::time::Duration;

    /// Broker cuya red tarda `delay` en contestar... y entonces falla. Lo
    /// justo para tener una navegacion en curso durante un rato controlado,
    /// sin sockets. El resto de operaciones las delega en un broker de verdad.
    struct SlowBroker {
        delay: Duration,
        inner: LocalBroker,
    }

    impl SlowBroker {
        fn shared(delay: Duration) -> SharedBroker {
            std::sync::Arc::new(Self { delay, inner: LocalBroker::in_memory() })
        }
    }

    impl ResourceBroker for SlowBroker {
        fn fetch<'a>(&'a self, request: &'a NetworkRequest) -> BrokerFuture<'a, Result<NetworkResponse, NetworkError>> {
            let (delay, url) = (self.delay, request.url.to_string());
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Err(NetworkError::Http(format!("sin red en el test ({url})")))
            })
        }
        fn cookie_header_for_js(&self, page_url: &str) -> String {
            self.inner.cookie_header_for_js(page_url)
        }
        fn set_cookie_from_js(&self, raw: &str, page_url: &str) {
            self.inner.set_cookie_from_js(raw, page_url);
        }
        fn storage_get(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String> {
            self.inner.storage_get(kind, origin, key)
        }
        fn storage_set(&self, kind: StorageKind, origin: &str, key: &str, value: &str) -> Result<(), QuotaExceeded> {
            self.inner.storage_set(kind, origin, key, value)
        }
        fn storage_remove(&self, kind: StorageKind, origin: &str, key: &str) {
            self.inner.storage_remove(kind, origin, key);
        }
        fn storage_clear(&self, kind: StorageKind, origin: &str) {
            self.inner.storage_clear(kind, origin);
        }
        fn storage_length(&self, kind: StorageKind, origin: &str) -> usize {
            self.inner.storage_length(kind, origin)
        }
        fn storage_key(&self, kind: StorageKind, origin: &str, index: usize) -> Option<String> {
            self.inner.storage_key(kind, origin, index)
        }
    }

    fn entrada(texto: &'static str) -> BoundedLines<BufReader<&'static [u8]>> {
        BoundedLines::new(BufReader::new(texto.as_bytes()), 4096)
    }

    fn navegar(id: &str) -> EngineRequest {
        EngineRequest::Navigate { id: Some(id.to_string()), url: "http://lento.test/".to_string() }
    }

    /// Un `stop` que llega a mitad de la carga la abandona YA: la respuesta a
    /// la navegacion es `navegacion_cancelada` (no se espera al fallo de la
    /// red, que aqui tardaria una hora) y el `stop` queda en cola para
    /// contestarse a continuacion.
    #[tokio::test]
    async fn a_stop_abandons_the_navigation_in_flight() {
        let mut server = EngineServer::with_broker(SlowBroker::shared(Duration::from_secs(3600)), "test");
        let mut lines = entrada("{\"type\":\"stop\",\"id\":\"s1\"}\n");
        let mut backlog = VecDeque::new();

        let (response, shutdown) = handle_interruptible(&mut server, navegar("n1"), &mut lines, &mut backlog).await.unwrap();

        assert!(!shutdown);
        let EngineResponse::Error { id, message } = response else { panic!("debia ser un error: {response:?}") };
        assert_eq!(id.as_deref(), Some("n1"), "se contesta a la navegacion, no al stop");
        assert!(message.starts_with("navegacion_cancelada"), "{message}");
        assert_eq!(backlog.len(), 1, "el stop espera su turno");

        let Ok(EngineRequest::Stop { id }) = serde_json::from_str(backlog[0].as_ref().unwrap()) else { panic!("la linea en cola es el stop") };
        let (ack, _) = server.handle(EngineRequest::Stop { id: id.clone() }).await;
        assert!(matches!(ack, EngineResponse::Ok { .. }), "{ack:?}");
    }

    /// Lo ultimo que pide el usuario manda: un `navigate` nuevo sustituye al
    /// que sigue cargando, y no hay que esperar a que acabe.
    #[tokio::test]
    async fn a_new_navigate_replaces_the_one_in_flight() {
        let mut server = EngineServer::with_broker(SlowBroker::shared(Duration::from_secs(3600)), "test");
        let mut lines = entrada("{\"type\":\"navigate\",\"id\":\"n2\",\"url\":\"http://rapido.test/\"}\n");
        let mut backlog = VecDeque::new();

        let (response, _) = handle_interruptible(&mut server, navegar("n1"), &mut lines, &mut backlog).await.unwrap();

        assert!(matches!(&response, EngineResponse::Error { id, message } if id.as_deref() == Some("n1") && message.starts_with("navegacion_cancelada")), "{response:?}");
        assert_eq!(backlog.len(), 1, "la navegacion nueva queda para atenderse a continuacion");
    }

    /// Una linea que no es `stop` ni `navigate` (un scroll, un resize...) NO
    /// interrumpe: la navegacion termina sola y la linea se atiende despues.
    #[tokio::test]
    async fn other_requests_wait_their_turn_without_interrupting() {
        let mut server = EngineServer::with_broker(SlowBroker::shared(Duration::from_millis(50)), "test");
        let mut lines = entrada("{\"type\":\"scroll\",\"id\":\"sc\",\"dx\":0,\"dy\":10}\n");
        let mut backlog = VecDeque::new();

        let (response, _) = handle_interruptible(&mut server, navegar("n1"), &mut lines, &mut backlog).await.unwrap();

        let EngineResponse::Error { message, .. } = response else { panic!("la navegacion debia terminar con su propio error de red") };
        assert!(message.starts_with("network_error"), "no se cancelo: {message}");
        assert_eq!(backlog.len(), 1, "el scroll espera en cola, en orden");
    }

    /// Cerrarse la entrada a mitad de una carga no la corta ni gira en vacio:
    /// termina la navegacion y el bucle principal vera el final despues.
    #[tokio::test]
    async fn a_closed_input_lets_the_navigation_finish() {
        let mut server = EngineServer::with_broker(SlowBroker::shared(Duration::from_millis(30)), "test");
        let mut lines = entrada("");
        let mut backlog = VecDeque::new();

        let (response, _) = handle_interruptible(&mut server, navegar("n1"), &mut lines, &mut backlog).await.unwrap();

        assert!(matches!(&response, EngineResponse::Error { message, .. } if message.starts_with("network_error")), "{response:?}");
        assert!(backlog.is_empty());
    }

    /// Cualquier peticion que no sea `navigate` se atiende directamente, sin
    /// tocar la entrada.
    #[tokio::test]
    async fn requests_other_than_navigate_are_handled_directly() {
        let mut server = EngineServer::with_broker(SlowBroker::shared(Duration::from_secs(3600)), "test");
        let mut lines = entrada("{\"type\":\"stop\"}\n");
        let mut backlog = VecDeque::new();

        let (response, _) = handle_interruptible(&mut server, EngineRequest::Ping { id: Some("p".to_string()) }, &mut lines, &mut backlog).await.unwrap();

        assert!(matches!(response, EngineResponse::Pong { .. }), "{response:?}");
        assert!(backlog.is_empty(), "no leyo la entrada");
    }
}

#[cfg(test)]
mod bounded_lines_tests {
    use super::*;

    /// Capacidad minima a proposito: obliga a que cada linea llegue en
    /// varios `fill_buf`, que es donde se rompe un lector ingenuo.
    fn lector(entrada: &'static [u8], max: usize) -> BoundedLines<BufReader<&'static [u8]>> {
        BoundedLines::new(BufReader::with_capacity(4, entrada), max)
    }

    #[tokio::test]
    async fn reads_lines_across_small_buffers_strips_crlf_and_keeps_the_last_unterminated_line() {
        let mut lines = lector(b"{\"a\":1}\r\nsegunda\nultima", 100);
        assert_eq!(lines.next_line().await.unwrap(), Some(Ok("{\"a\":1}".to_string())));
        assert_eq!(lines.next_line().await.unwrap(), Some(Ok("segunda".to_string())));
        assert_eq!(lines.next_line().await.unwrap(), Some(Ok("ultima".to_string())));
        assert_eq!(lines.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_line_over_the_limit_is_rejected_whole_and_the_next_one_is_clean() {
        let mut lines = lector(b"0123456789ABCDEF\n{\"ok\":1}\n", 10);
        assert_eq!(lines.next_line().await.unwrap(), Some(Err(RejectedLine::TooLong { bytes: 16 })));
        assert_eq!(lines.next_line().await.unwrap(), Some(Ok("{\"ok\":1}".to_string())));
    }

    #[tokio::test]
    async fn the_rejected_line_is_not_kept_in_memory() {
        let mut lines = BoundedLines::new(BufReader::with_capacity(4, &[b'x'; 10_000][..]), 10);
        let rechazada = lines.next_line().await.unwrap();
        assert_eq!(rechazada, Some(Err(RejectedLine::TooLong { bytes: 10_000 })));
        assert!(lines.buf.capacity() <= 16, "el buffer no deberia haber crecido con la linea: {}", lines.buf.capacity());
    }

    /// Antes, un byte que no era UTF-8 hacia que `lines()` devolviera
    /// `InvalidData` y `run_stdio` terminaba con `?`: el motor moria.
    #[tokio::test]
    async fn invalid_utf8_is_a_rejected_line_not_a_fatal_error() {
        let mut lines = lector(b"\xff\xfe\n{\"ok\":1}\n", 100);
        assert_eq!(lines.next_line().await.unwrap(), Some(Err(RejectedLine::NotUtf8)));
        assert_eq!(lines.next_line().await.unwrap(), Some(Ok("{\"ok\":1}".to_string())));
    }

    /// El caso de `run_stdio`: el reloj gana el `select!` a mitad de una
    /// linea y el futuro de lectura se descarta. Lo ya leido no se pierde.
    #[tokio::test]
    async fn cancelling_mid_line_loses_no_bytes() {
        let (mut escritor, lectura) = tokio::io::duplex(64);
        let mut lines = BoundedLines::new(BufReader::new(lectura), 100);

        escritor.write_all(b"{\"type\":").await.unwrap();
        let cancelada = tokio::time::timeout(std::time::Duration::from_millis(20), lines.next_line()).await;
        assert!(cancelada.is_err(), "sin salto de linea no deberia haber terminado");

        escritor.write_all(b"\"ping\"}\n").await.unwrap();
        assert_eq!(lines.next_line().await.unwrap(), Some(Ok("{\"type\":\"ping\"}".to_string())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EngineRequest;

    /// Regresion encontrada en vivo (Fase 32) junto con el fix de
    /// `noscript` en `engine-layout`: el campo `text` de la lista
    /// `elements` del protocolo NDJSON usaba `Node::text_content` (el
    /// mismo que implementa `.textContent` real de JS, que camina el DOM
    /// SIN filtrar - correcto para JS, pero no aqui), asi que el marcado
    /// crudo de un `<noscript>` (o el codigo fuente de un `<script>`) se
    /// colaba en el texto de `<body>` aunque nunca se pintara. Camina el
    /// arbol de LAYOUT (ya filtrado) en vez del DOM crudo.
    #[test]
    fn collect_visible_text_excludes_noscript_and_script_content() {
        let dom = engine_dom::HtmlParser::parse(
            r#"<html><body><noscript><iframe src="https://example.test/"></iframe></noscript><script>var x = 1;</script><p>contenido real</p></body></html>"#,
        );
        let stylesheet = engine_css::CssParser::parse("body { margin: 0px; }");
        let layout_root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let text = collect_visible_text(&layout_root);
        assert!(!text.contains("iframe"), "el marcado de <noscript> no deberia colarse en el texto visible: {text:?}");
        assert!(!text.contains("var x"), "el codigo fuente de <script> no deberia colarse en el texto visible: {text:?}");
        assert!(text.contains("contenido real"), "el contenido normal de la pagina si deberia aparecer");
    }

    /// Helper de los tests de Fase 39: parsea HTML y construye su layout,
    /// que es exactamente el par que recibe `page_content_requires_javascript`.
    fn dom_and_layout(html: &str) -> (std::sync::Arc<std::sync::RwLock<Node>>, LayoutBox) {
        let dom = engine_dom::HtmlParser::parse(html);
        let stylesheet = engine_css::CssParser::parse("");
        let layout_root = LayoutTreeBuilder::build(&dom, &stylesheet, 1280.0, 720.0, None, &ImageMap::new());
        (dom, layout_root)
    }

    /// El caso que motivo la Fase 39, medido en vivo contra una web real
    /// (`ignislove.com`): el servidor manda una cascara sin una sola letra
    /// visible y 7 `<script>` que construirian la pagina en el cliente.
    /// Antes esto se pintaba como un blanco mudo.
    #[test]
    fn an_empty_shell_with_scripts_is_reported_as_javascript_dependent() {
        let (dom, layout) = dom_and_layout(
            r#"<html><head><title>Tienda</title></head><body><div id="root"></div><script src="/app.js"></script></body></html>"#,
        );
        assert!(
            page_content_requires_javascript(&dom, &layout),
            "una cascara vacia con <script> deberia marcarse como dependiente de JavaScript"
        );
    }

    /// Una pagina que SI trae su texto en el HTML no se marca, aunque lleve
    /// scripts (analitica, por ejemplo) - que es el caso de Wikipedia y
    /// Google, ambos comprobados en vivo.
    #[test]
    fn a_page_that_ships_its_text_is_not_reported_even_with_scripts() {
        let (dom, layout) = dom_and_layout(
            r#"<html><body><h1>Espana</h1><p>Espana es un pais soberano situado en el suroeste de Europa.</p><script src="/analytics.js"></script></body></html>"#,
        );
        assert!(
            !page_content_requires_javascript(&dom, &layout),
            "una pagina con texto real no deberia marcarse aunque tenga scripts"
        );
    }

    /// Sin `<script>` no hay nada que culpar: una pagina vacia y SIN
    /// scripts esta vacia de verdad, y decir "necesita JavaScript" seria
    /// mentir. Es la mitad de la heuristica que evita el falso positivo mas
    /// obvio.
    #[test]
    fn an_empty_page_without_scripts_is_not_blamed_on_javascript() {
        let (dom, layout) = dom_and_layout(r#"<html><body></body></html>"#);
        assert!(
            !page_content_requires_javascript(&dom, &layout),
            "sin scripts, una pagina vacia no deberia atribuirse a JavaScript"
        );
    }

    /// El codigo fuente del propio script no cuenta como contenido: si
    /// contara (mirando el DOM crudo en vez del arbol de layout), un bundle
    /// grande haria que la cascara pareciera llena y el aviso no saltaria
    /// nunca. Es la razon exacta de reusar `collect_visible_text`.
    #[test]
    fn a_long_inline_script_does_not_count_as_visible_content() {
        let long_code = "var configuracionMuyLarga = { clave: 'valor', otra: 'cosa', mas: 12345 };";
        let html = format!(r#"<html><body><div id="app"></div><script>{long_code}</script></body></html>"#);
        let (dom, layout) = dom_and_layout(&html);
        assert!(
            page_content_requires_javascript(&dom, &layout),
            "el codigo del script no deberia contar como contenido visible y tapar el aviso"
        );
    }

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
                scale: 1.0,
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

    fn server_with_scrollable_page_and_script(script: &str) -> EngineServer {
        let html = format!("<html><body style=\"height:4000px\"><script>{script}</script></body></html>");
        let (page, runtime) = build_page_keeping_runtime(&html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new(
            "http://ejemplo.test/".to_string(),
            String::new(),
            page,
            runtime,
            None,
            ImageMap::new(),
        ));
        server
    }

    fn server_from_html(html: &str) -> EngineServer {
        let (page, runtime) = build_page_keeping_runtime(html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new(
            "http://ejemplo.test/".to_string(),
            String::new(),
            page,
            runtime,
            None,
            ImageMap::new(),
        ));
        server
    }

    async fn state_elements(server: &mut EngineServer) -> Vec<InteractiveElement> {
        let (state, _) = server.handle(EngineRequest::GetState { id: None }).await;
        let EngineResponse::State { elements, .. } = state else { panic!("get_state debe devolver un State") };
        elements.expect("get_state trae la lista de elementos")
    }

    const PAGE_WITH_FIELD: &str = "<html><body style=\"margin:0\"><input id=\"campo\" type=\"text\" placeholder=\"escribe\" style=\"width:200px;height:30px\"><button id=\"otro\" style=\"width:80px;height:30px\">Otro</button></body></html>";

    fn field_value(server: &EngineServer) -> String {
        let page = &server.active_tab().current_page.as_ref().expect("pagina").page;
        let campo = Node::find_by_id(&page.dom_root, "campo").expect("campo");
        let guard = campo.read().unwrap();
        match &guard.node_type {
            NodeType::Element { attributes, .. } => attributes.get("value").cloned().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn field_painted_text(server: &EngineServer) -> Option<String> {
        let page = &server.active_tab().current_page.as_ref().expect("pagina").page;
        let campo = Node::find_by_id(&page.dom_root, "campo").expect("campo");
        page.layout_root.find_box_for_node(&campo).and_then(|b| b.replaced_text.as_ref().map(|r| r.text.clone()))
    }

    async fn press(server: &mut EngineServer, key: &str) -> EngineResponse {
        server.handle(EngineRequest::PressKey { id: None, key: key.to_string() }).await.0
    }

    /// Tarea 3.2: las teclas con caracter escriben en el campo enfocado, Retroceso
    /// borra, y lo que se ve en el campo (el texto de su caja) va por delante sin
    /// rehacer el layout.
    #[tokio::test]
    async fn typing_keys_writes_into_the_focused_field_and_backspace_erases() {
        let mut server = server_from_html(PAGE_WITH_FIELD);
        server.handle(EngineRequest::Click { id: None, x: 20.0, y: 15.0 }).await;
        for key in ["h", "o", "l", "a", " ", "ñ"] {
            let r = press(&mut server, key).await;
            assert!(matches!(r, EngineResponse::State { .. }), "{key}: {r:?}");
        }
        assert_eq!(field_value(&server), "hola ñ");
        assert_eq!(field_painted_text(&server).as_deref(), Some("hola ñ"), "el campo pinta lo escrito");

        press(&mut server, "Backspace").await;
        press(&mut server, "Backspace").await;
        assert_eq!(field_value(&server), "hola");
        assert_eq!(field_painted_text(&server).as_deref(), Some("hola"));
    }

    /// Un `<textarea>` muestra lo que se escribe (el valor va en `value`, y el texto
    /// de la caja salia del contenido inicial: no se veia nada) y Enter dentro de el
    /// añade una linea en vez de enviar el formulario.
    #[tokio::test]
    async fn typing_in_a_textarea_shows_the_text_and_enter_adds_a_line() {
        let mut server = server_from_html("<html><body style=\"margin:0\"><form action=\"/envia\"><textarea id=\"campo\" style=\"width:200px;height:60px\"></textarea></form></body></html>");
        server.handle(EngineRequest::Click { id: None, x: 20.0, y: 15.0 }).await;
        for key in ["h", "i"] {
            press(&mut server, key).await;
        }
        assert_eq!(field_value(&server), "hi");
        assert_eq!(field_painted_text(&server).as_deref(), Some("hi"), "el textarea pinta lo escrito");

        let r = press(&mut server, "Enter").await;
        assert!(matches!(r, EngineResponse::State { .. }), "Enter en un textarea no envia ni falla: {r:?}");
        press(&mut server, "x").await;
        assert_eq!(field_value(&server), "hi\nx");
    }

    /// Las teclas que no son un caracter (flechas, Tab, F5...) no escriben nada.
    #[tokio::test]
    async fn named_keys_do_not_write_text() {
        let mut server = server_from_html(PAGE_WITH_FIELD);
        server.handle(EngineRequest::Click { id: None, x: 20.0, y: 15.0 }).await;
        for key in ["ArrowLeft", "Tab", "Shift", "F5", "Escape"] {
            press(&mut server, key).await;
        }
        assert_eq!(field_value(&server), "");
        assert_eq!(field_painted_text(&server).as_deref(), Some("escribe"), "sigue el placeholder");
    }

    /// Clicar fuera del campo le quita el foco: las teclas ya no van a ningun sitio.
    #[tokio::test]
    async fn clicking_elsewhere_blurs_the_field() {
        let mut server = server_from_html(PAGE_WITH_FIELD);
        server.handle(EngineRequest::Click { id: None, x: 20.0, y: 15.0 }).await;
        press(&mut server, "a").await;
        server.handle(EngineRequest::Click { id: None, x: 250.0, y: 15.0 }).await;
        let r = press(&mut server, "b").await;
        assert!(matches!(&r, EngineResponse::Error { message, .. } if message.contains("no hay un control enfocado")), "{r:?}");
        assert_eq!(field_value(&server), "a");
    }

    /// El foco se ve: la captura con el campo enfocado no es la misma que sin foco
    /// (anillo y cursor), y el cursor se mueve al escribir.
    #[tokio::test]
    async fn a_focused_field_is_drawn_with_a_ring_and_a_caret() {
        let screenshot = |r: EngineResponse| match r {
            EngineResponse::State { screenshot, .. } => screenshot,
            other => panic!("{other:?}"),
        };
        let mut server = server_from_html(PAGE_WITH_FIELD);
        let (before, _) = server.handle(EngineRequest::GetState { id: None }).await;
        let before = screenshot(before);
        let focused = screenshot(server.handle(EngineRequest::Click { id: None, x: 20.0, y: 15.0 }).await.0);
        assert_ne!(before, focused, "con el foco, la captura cambia");
        let typed = screenshot(press(&mut server, "m").await);
        assert_ne!(focused, typed, "al escribir cambia el texto y se mueve el cursor");
        // Sin foco otra vez, la captura de antes de escribir no vuelve (el valor sigue).
        let blurred = screenshot(server.handle(EngineRequest::Click { id: None, x: 250.0, y: 15.0 }).await.0);
        assert_ne!(blurred, typed, "sin foco desaparece el anillo");
    }

    /// Con un listener de teclado en la pagina, que pudo cambiar cualquier cosa, si
    /// se rehace el layout (lo mismo que el clic).
    #[tokio::test]
    async fn typing_rebuilds_the_layout_when_the_page_listens_to_keys() {
        let extent_after_key = |html: &'static str| async move {
            let mut server = server_from_html(html);
            server.handle(EngineRequest::Click { id: None, x: 20.0, y: 15.0 }).await;
            let page = server.active_tab_mut().current_page.as_mut().expect("pagina");
            assert!(page.page.layout_root.content_extent() >= 3000.0);
            page.runtime.eval("document.getElementById('relleno').setAttribute('style', 'height:100px')").expect("mutar el DOM");
            press(&mut server, "a").await;
            server.active_tab().current_page.as_ref().expect("pagina").page.layout_root.content_extent()
        };
        let base = "<html><body style=\"margin:0\"><input id=\"campo\" type=\"text\" style=\"width:200px;height:30px\"><div id=\"relleno\" style=\"height:3000px\"></div>";
        assert!(extent_after_key(Box::leak(format!("{base}</body></html>").into_boxed_str())).await >= 3000.0, "sin listeners no se rehace");
        assert!(
            extent_after_key(Box::leak(format!("{base}<script>document.addEventListener('keydown', function() {{}});</script></body></html>").into_boxed_str())).await < 3000.0,
            "con un listener de keydown si"
        );
    }

    /// Tarea 3.4: el texto de la pagina, legible: bloques en lineas distintas,
    /// espacios colapsados, sin el codigo de los scripts.
    #[tokio::test]
    async fn get_page_text_returns_readable_text_with_line_breaks_between_blocks() {
        let mut server = server_from_html(
            "<html><head><style>p { color: red }</style></head><body><h1>Titulo</h1>\
             <p>Primera   frase <b>con negrita</b> y mas.</p><p>Segunda.</p>\
             <ul><li>uno</li><li>dos</li></ul><script>var oculto = 1;</script></body></html>",
        );
        let (response, _) = server.handle(EngineRequest::GetPageText { id: Some("t".to_string()) }).await;
        let EngineResponse::PageText { text, truncated, .. } = response else { panic!("debia ser PageText: {response:?}") };
        assert!(!truncated);
        assert_eq!(text, "Titulo\nPrimera frase con negrita y mas.\nSegunda.\n• uno\n• dos", "los items de lista conservan su viñeta");
    }

    /// Wikipedia: el menu lateral llena el tope antes de llegar al articulo. Con
    /// un `<main>` con texto de verdad, solo se lee ese.
    #[tokio::test]
    async fn get_page_text_prefers_the_main_content_over_menus() {
        let articulo = "Un navegador web es un programa que permite ver la informacion que contiene una pagina web. ".repeat(4);
        let mut server = server_from_html(&format!(
            "<html><body><nav><a href=\"/a\">Menu principal</a><a href=\"/b\">Portada</a></nav><main><h1>Navegador web</h1><p>{articulo}</p></main><footer>Pie de pagina</footer></body></html>"
        ));
        let (response, _) = server.handle(EngineRequest::GetPageText { id: None }).await;
        let EngineResponse::PageText { text, .. } = response else { panic!("debia ser PageText") };
        assert!(text.starts_with("Navegador web\nUn navegador web es un programa"), "{text}");
        assert!(!text.contains("Menu principal") && !text.contains("Pie de pagina"), "sin menus ni pie: {text}");
    }

    /// Un `<main>` casi vacio (cascara de una web que pinta con JS) no sustituye a la pagina.
    #[tokio::test]
    async fn get_page_text_ignores_an_empty_main_shell() {
        let mut server = server_from_html("<html><body><p>Texto fuera del main.</p><main><p>hola</p></main></body></html>");
        let (response, _) = server.handle(EngineRequest::GetPageText { id: None }).await;
        let EngineResponse::PageText { text, .. } = response else { panic!("debia ser PageText") };
        assert!(text.contains("Texto fuera del main."), "{text}");
    }

    #[tokio::test]
    async fn get_page_text_is_capped_and_says_so() {
        let largo = "palabra ".repeat(5000);
        let mut server = server_from_html(&format!("<html><body><p>{largo}</p></body></html>"));
        let (response, _) = server.handle(EngineRequest::GetPageText { id: None }).await;
        let EngineResponse::PageText { text, truncated, .. } = response else { panic!("debia ser PageText") };
        assert!(truncated, "la pagina tiene 40.000 caracteres");
        assert!(text.chars().count() <= MAX_PAGE_TEXT_CHARS);
    }

    /// Tarea 3.3: `elements` solo trae lo accionable y visible. Antes traia
    /// cada etiqueta de la pagina (4,8 MB en Wikipedia).
    #[tokio::test]
    async fn elements_lists_only_visible_interactive_targets() {
        let mut server = server_from_html(
            "<html><body>\
             <p>texto suelto</p><div>otro bloque</div>\
             <a href=\"/a\">enlace</a><a name=\"ancla\">sin destino</a>\
             <button>Pulsar</button><input type=\"hidden\" name=\"h\"><input type=\"text\" name=\"q\">\
             <select><option>uno</option></select><textarea></textarea>\
             <div onclick=\"1\">accionable</div><div role=\"button\">rol</div>\
             <div tabindex=\"0\">enfocable</div><div tabindex=\"-1\">solo por codigo</div>\
             <div style=\"display:none\"><button>oculto</button></div>\
             <div tabindex=\"0\"></div>\
             <a href=\"/vacio\"></a>\
             </body></html>",
        );
        let elements = state_elements(&mut server).await;
        let tags: Vec<_> = elements.iter().map(|e| (e.tag_name.as_str(), e.text.as_str())).collect();
        assert_eq!(
            tags,
            vec![
                ("a", "enlace"),
                ("button", "Pulsar"),
                ("input", ""),
                ("select", "uno"),
                ("textarea", ""),
                ("div", "accionable"),
                ("div", "rol"),
                ("div", "enfocable"),
                // El <div tabindex> vacio (alto 0) no esta; el enlace vacio si:
                // sin ancho pero con alto sigue siendo un destino de navegacion.
                ("a", ""),
            ],
            "solo elementos interactivos y visibles, en orden de documento"
        );
        let ids: Vec<u32> = elements.iter().map(|e| e.id).collect();
        assert_eq!(ids, (0..elements.len() as u32).collect::<Vec<_>>(), "los ids son la posicion en la lista");
    }

    #[tokio::test]
    async fn element_text_is_collapsed_and_truncated() {
        let largo = "palabra ".repeat(60);
        let mut server = server_from_html(&format!("<html><body><a href=\"/x\">  uno \n\n   dos </a><a href=\"/y\">{largo}</a></body></html>"));
        let elements = state_elements(&mut server).await;
        assert_eq!(elements[0].text, "uno dos", "espacios colapsados");
        assert!(elements[1].text.chars().count() <= MAX_ELEMENT_TEXT_CHARS + 1, "recortado: {}", elements[1].text.chars().count());
        assert!(elements[1].text.ends_with('…'));
    }

    #[tokio::test]
    async fn the_element_list_is_capped() {
        let botones = "<button>b</button>".repeat(MAX_INTERACTIVE_ELEMENTS + 50);
        let mut server = server_from_html(&format!("<html><body>{botones}</body></html>"));
        assert_eq!(state_elements(&mut server).await.len(), MAX_INTERACTIVE_ELEMENTS);
    }

    const TALL_PAGE_WITH_FORM: &str = "<html><body><div style=\"height:3000px\">relleno</div>\
        <button id=\"abajo\">Abajo</button>\
        <input id=\"campo\" type=\"text\" value=\"viejo\">\
        <div style=\"height:1000px\">mas relleno</div>\
        <script>var pulsado = false; document.getElementById('abajo').addEventListener('click', function() { pulsado = true; });</script>\
        </body></html>";

    fn element_id(elements: &[InteractiveElement], dom_id: &str) -> u32 {
        elements.iter().find(|e| e.attributes.id.as_deref() == Some(dom_id)).unwrap_or_else(|| panic!("no hay #{dom_id}")).id
    }

    /// Tarea 3.1: antes, clicar por las coordenadas de `elements[].rect`
    /// (del DOCUMENTO) fallaba en cuanto la pagina estaba desplazada, porque
    /// `click` espera coordenadas de VIEWPORT. `click_element` lo resuelve
    /// dentro del motor, que es quien sabe donde esta el scroll.
    #[tokio::test]
    async fn click_element_scrolls_the_target_into_view_and_clicks_it() {
        let mut server = server_from_html(TALL_PAGE_WITH_FORM);
        let elements = state_elements(&mut server).await;
        let boton = element_id(&elements, "abajo");
        assert_eq!(server.active_tab().scroll_offset_y, 0.0);

        let (response, _) = server.handle(EngineRequest::ClickElement { id: Some("c".to_string()), element_id: boton }).await;
        let EngineResponse::State { scroll_offset_y, .. } = response else { panic!("click_element debe devolver un State: {response:?}") };
        assert!(scroll_offset_y > 2000.0, "el boton estaba a 3000 px: la pagina debio desplazarse ({scroll_offset_y})");
        let runtime = &mut server.active_tab_mut().current_page.as_mut().expect("pagina").runtime;
        assert_eq!(runtime.eval("pulsado").unwrap(), "true", "el listener de click del boton se ejecuto");
    }

    #[tokio::test]
    async fn click_element_also_works_when_the_page_is_already_scrolled_somewhere_else() {
        let mut server = server_from_html(TALL_PAGE_WITH_FORM);
        let elements = state_elements(&mut server).await;
        let boton = element_id(&elements, "abajo");
        server.handle(EngineRequest::Scroll { id: None, dx: 0, dy: 1200 }).await;
        server.handle(EngineRequest::ClickElement { id: None, element_id: boton }).await;
        let runtime = &mut server.active_tab_mut().current_page.as_mut().expect("pagina").runtime;
        assert_eq!(runtime.eval("pulsado").unwrap(), "true");
    }

    #[tokio::test]
    async fn type_into_element_appends_by_default_and_replaces_on_request() {
        let mut server = server_from_html(TALL_PAGE_WITH_FORM);
        let elements = state_elements(&mut server).await;
        let campo = element_id(&elements, "campo");
        let valor = |server: &mut EngineServer| {
            let page = &server.active_tab().current_page.as_ref().expect("pagina").page;
            let campo = Node::find_by_id(&page.dom_root, "campo").expect("el campo existe");
            let guard = campo.read().unwrap();
            match &guard.node_type {
                NodeType::Element { attributes, .. } => attributes.get("value").cloned().unwrap_or_default(),
                _ => String::new(),
            }
        };

        let (response, _) = server
            .handle(EngineRequest::TypeIntoElement { id: None, element_id: campo, text: "+nuevo".to_string(), replace: false, press_enter: false })
            .await;
        assert!(matches!(response, EngineResponse::State { .. }), "{response:?}");
        assert_eq!(valor(&mut server), "viejo+nuevo", "sin replace se añade al final");
        assert!(server.active_tab().scroll_offset_y > 2000.0, "y la pagina se desplazo hasta el campo");

        server
            .handle(EngineRequest::TypeIntoElement { id: None, element_id: campo, text: "limpio".to_string(), replace: true, press_enter: false })
            .await;
        assert_eq!(valor(&mut server), "limpio", "con replace se sustituye el valor");
    }

    #[tokio::test]
    async fn type_into_a_non_text_element_is_a_clear_error() {
        let mut server = server_from_html(TALL_PAGE_WITH_FORM);
        let elements = state_elements(&mut server).await;
        let boton = element_id(&elements, "abajo");
        let (response, _) = server
            .handle(EngineRequest::TypeIntoElement { id: None, element_id: boton, text: "x".to_string(), replace: true, press_enter: false })
            .await;
        let EngineResponse::Error { message, .. } = response else { panic!("debe ser un error") };
        assert!(message.contains("control de texto"), "{message}");
    }

    #[tokio::test]
    async fn an_unknown_element_id_is_an_error_not_a_click_somewhere_else() {
        let mut server = server_from_html(TALL_PAGE_WITH_FORM);
        let (response, _) = server.handle(EngineRequest::ClickElement { id: Some("c".to_string()), element_id: 999 }).await;
        let EngineResponse::Error { message, .. } = response else { panic!("debe ser un error") };
        assert!(message.contains("[999]") && message.contains("pide el estado"), "{message}");
        assert_eq!(server.active_tab().scroll_offset_y, 0.0, "y no se movio nada");
    }

    /// El punto real de esta tarea: un `document.addEventListener('scroll',
    /// ...)` real tiene que dispararse cuando llega un comando `Scroll` -
    /// antes de esto, el desplazamiento de viewport YA era real
    /// (`getBoundingClientRect` ya lo reflejaba) pero ningun listener JS se
    /// enteraba nunca.
    #[tokio::test]
    async fn scroll_command_fires_a_real_scroll_event_on_document() {
        let mut server = server_with_scrollable_page_and_script(
            "var vistoScroll = false; document.addEventListener('scroll', function() { vistoScroll = true; });",
        );
        server.handle(EngineRequest::Scroll { id: Some("s1".to_string()), dx: 0, dy: 200 }).await;

        let runtime = &mut server.active_tab_mut().current_page.as_mut().expect("deberia haber pagina").runtime;
        assert_eq!(runtime.eval("vistoScroll").unwrap(), "true", "el listener 'scroll' de document deberia haberse disparado");
    }

    /// Tarea 1.5 del plan: la respuesta a un `scroll` no reenvia `elements` ni
    /// `requires_javascript` (un desplazamiento no los cambia y pesaban tanto
    /// como la captura), pero `get_state` sigue trayendolos.
    #[tokio::test]
    async fn a_scroll_response_omits_the_page_details_but_get_state_keeps_them() {
        let mut server = server_with_scrollable_page_and_script("");

        let (scroll, _) = server.handle(EngineRequest::Scroll { id: Some("s".to_string()), dx: 0, dy: 100 }).await;
        let EngineResponse::State { elements, requires_javascript, scroll_offset_y, screenshot, .. } = &scroll else { panic!("scroll debe devolver un State: {scroll:?}") };
        assert!(elements.is_none(), "el scroll no reenvia los elementos");
        assert!(requires_javascript.is_none(), "ni requires_javascript");
        assert_eq!(*scroll_offset_y, 100.0, "pero si el desplazamiento");
        assert!(!screenshot.is_empty(), "y la captura");
        let json = serde_json::to_string(&scroll).unwrap();
        assert!(!json.contains("\"elements\"") && !json.contains("requires_javascript"), "los campos ni aparecen en el JSON: {}", &json[..json.len().min(200)]);

        let (state, _) = server.handle(EngineRequest::GetState { id: Some("g".to_string()) }).await;
        let EngineResponse::State { elements, requires_javascript, .. } = &state else { panic!("get_state debe devolver un State") };
        assert!(elements.is_some() && requires_javascript.is_some(), "get_state si trae todo");
    }

    /// Tarea 1.5/1.4: sin nadie escuchando `scroll` no se rehace el layout (era
    /// lo que hacia tardar 0,66 s cada paso en Wikipedia); con un listener si,
    /// porque podria haber cambiado el DOM. Se distingue cambiando el DOM
    /// desde fuera justo antes del scroll: el layout solo se entera si hubo
    /// relayout.
    #[tokio::test]
    async fn scroll_rebuilds_the_layout_only_when_someone_listens_for_it() {
        let extent_after_scroll = |con_listener: bool| async move {
            let script = if con_listener { "document.addEventListener('scroll', function() {});" } else { "" };
            let mut server = server_with_scrollable_page_and_script(script);
            let page = server.active_tab_mut().current_page.as_mut().expect("pagina");
            assert!(page.page.layout_root.content_extent() >= 4000.0, "la pagina mide 4000 px");
            page.runtime.eval("document.body.setAttribute('style', 'height:100px')").expect("mutar el DOM");
            server.handle(EngineRequest::Scroll { id: None, dx: 0, dy: 10 }).await;
            server.active_tab().current_page.as_ref().expect("pagina").page.layout_root.content_extent()
        };

        assert!(extent_after_scroll(false).await >= 4000.0, "sin listeners no se rehace el layout: sigue el anterior");
        assert!(extent_after_scroll(true).await < 4000.0, "con un listener de scroll si se rehace");
    }

    /// Tarea 1.4: un clic donde no hay nada que lo escuche ni nada que
    /// cambiar no rehace el layout (0,55 s en Wikipedia); con un listener de
    /// clic, o sobre un campo, si. Se distingue cambiando el DOM desde fuera
    /// justo antes del clic: el layout solo se entera si hubo relayout.
    #[tokio::test]
    async fn a_click_rebuilds_the_layout_only_when_it_could_have_changed_something() {
        let extent_after_click = |html: &'static str| async move {
            let mut server = server_from_html(html);
            let page = server.active_tab_mut().current_page.as_mut().expect("pagina");
            assert!(page.page.layout_root.content_extent() >= 4000.0, "la pagina mide 4000 px");
            page.runtime.eval("document.body.setAttribute('style', 'height:100px')").expect("mutar el DOM");
            server.handle(EngineRequest::Click { id: None, x: 5.0, y: 5.0 }).await;
            server.active_tab().current_page.as_ref().expect("pagina").page.layout_root.content_extent()
        };

        assert!(
            extent_after_click("<html><body style=\"height:4000px\"><p>texto</p></body></html>").await >= 4000.0,
            "sin listeners ni controles no se rehace el layout"
        );
        assert!(
            extent_after_click("<html><body style=\"height:4000px\"><p>texto</p><script>document.addEventListener('click', function() {});</script></body></html>").await < 4000.0,
            "con un listener de clic si"
        );
        assert!(
            extent_after_click("<html><body style=\"height:4000px;margin:0\"><input style=\"width:300px;height:30px\"></body></html>").await < 4000.0,
            "clicar un campo si"
        );
    }

    /// Un listener de `scroll` puede mutar el DOM (el patron real de
    /// "infinite scroll") - ese cambio tiene que verse reflejado, no
    /// perderse hasta la siguiente interaccion.
    #[tokio::test]
    async fn a_scroll_listener_that_mutates_the_dom_is_reflected_after_relayout() {
        let html = "<html><body style=\"height:4000px\"><p id=\"marcador\">antes</p><script>document.addEventListener('scroll', function() { document.getElementById('marcador').textContent = 'despues'; });</script></body></html>";
        let (page, runtime) = build_page_keeping_runtime(html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new("http://ejemplo.test/".to_string(), String::new(), page, runtime, None, ImageMap::new()));

        server.handle(EngineRequest::Scroll { id: Some("s1".to_string()), dx: 0, dy: 200 }).await;

        let page = &server.active_tab().current_page.as_ref().expect("deberia haber pagina").page;
        let marcador = Node::find_by_id(&page.dom_root, "marcador").expect("deberia existir el marcador");
        assert_eq!(Node::text_content(&marcador), "despues", "el listener de scroll deberia haber mutado el DOM, y el relayout deberia reflejarlo");
    }

    /// El punto real de disparar sobre `<html>` en vez de sobre `dom_root`
    /// directo: `window.addEventListener('scroll', ...)` (el shim de
    /// `register_history`, que delega en `document.documentElement`)
    /// tiene que enterarse tambien, no solo `document.addEventListener`.
    #[tokio::test]
    async fn scroll_command_also_fires_on_window_addeventlistener() {
        let mut server = server_with_scrollable_page_and_script(
            "var vistoEnWindow = false; window.addEventListener('scroll', function() { vistoEnWindow = true; });",
        );
        server.handle(EngineRequest::Scroll { id: Some("s1".to_string()), dx: 0, dy: 200 }).await;

        let runtime = &mut server.active_tab_mut().current_page.as_mut().expect("deberia haber pagina").runtime;
        assert_eq!(runtime.eval("vistoEnWindow").unwrap(), "true", "window.addEventListener('scroll', ...) deberia haberse disparado tambien");
    }

    /// Misma cobertura que el scroll, para `resize`: un
    /// `window.addEventListener('resize', ...)` tiene que dispararse
    /// cuando llega un comando `Resize` real.
    #[tokio::test]
    async fn resize_command_fires_a_real_resize_event_on_window() {
        let html = "<html><body><script>var vistoResize = false; window.addEventListener('resize', function() { vistoResize = true; });</script></body></html>";
        let (page, runtime) = build_page_keeping_runtime(html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new("http://ejemplo.test/".to_string(), String::new(), page, runtime, None, ImageMap::new()));

        server.handle(EngineRequest::Resize { id: Some("r1".to_string()), width: 1000, height: 700, scale: 1.0 }).await;

        let runtime = &mut server.active_tab_mut().current_page.as_mut().expect("deberia haber pagina").runtime;
        assert_eq!(runtime.eval("vistoResize").unwrap(), "true", "window.addEventListener('resize', ...) deberia haberse disparado con un comando Resize real");
    }

    /// Un listener de `resize` puede mutar el DOM (p.ej. mostrar/ocultar
    /// un menu segun el ancho) - ese cambio tiene que verse reflejado en
    /// el MISMO relayout que el propio `resize` ya iba a disparar.
    #[tokio::test]
    async fn a_resize_listener_that_mutates_the_dom_is_reflected_after_relayout() {
        let html = "<html><body><p id=\"marcador\">antes</p><script>window.addEventListener('resize', function() { document.getElementById('marcador').textContent = 'despues'; });</script></body></html>";
        let (page, runtime) = build_page_keeping_runtime(html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new("http://ejemplo.test/".to_string(), String::new(), page, runtime, None, ImageMap::new()));

        server.handle(EngineRequest::Resize { id: Some("r1".to_string()), width: 1000, height: 700, scale: 1.0 }).await;

        let page = &server.active_tab().current_page.as_ref().expect("deberia haber pagina").page;
        let marcador = Node::find_by_id(&page.dom_root, "marcador").expect("deberia existir el marcador");
        assert_eq!(Node::text_content(&marcador), "despues", "el listener de resize deberia haber mutado el DOM, y el relayout deberia reflejarlo");
    }

    /// El punto real del reloj de fondo: un `setTimeout` vencido (delay 0,
    /// ya vencido en cuanto se registra) dispara SOLO con el tick, sin
    /// ningun comando NDJSON de por medio - antes de esta tarea, nada
    /// llamaba nunca a `run_due_timers` fuera de un comando real.
    #[test]
    fn tick_active_tab_timers_fires_a_due_timeout_without_any_command() {
        let html = "<html><body><script>var disparo = false; setTimeout(function() { disparo = true; }, 0);</script></body></html>";
        let (page, runtime) = build_page_keeping_runtime(html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new("http://ejemplo.test/".to_string(), String::new(), page, runtime, None, ImageMap::new()));

        let publicado = server.tick_active_tab_timers();

        let runtime = &mut server.active_tab_mut().current_page.as_mut().expect("deberia haber pagina").runtime;
        assert_eq!(runtime.eval("disparo").unwrap(), "true", "el tick deberia haber disparado el setTimeout ya vencido, sin ningun comando NDJSON");
        assert!(matches!(publicado, Some(EngineResponse::State { id: None, .. })), "el tick deberia devolver el estado nuevo, sin id, para publicarlo");
    }

    /// Plan H07 (Fase 50): el cambio que hace un temporizador tiene que
    /// llegar en el estado que devuelve el tick, no esperar a otro comando.
    #[test]
    fn tick_active_tab_timers_returns_the_state_changed_by_the_timer() {
        let html = "<html><head><title>ANTES</title></head><body><script>setTimeout(function() { document.title = 'DESPUES'; }, 0);</script></body></html>";
        let (page, runtime) = build_page_keeping_runtime(html, "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new(), None, None);
        let mut server = EngineServer::new();
        server.active_tab_mut().current_page = Some(LoadedPage::new("http://ejemplo.test/".to_string(), String::new(), page, runtime, None, ImageMap::new()));

        let Some(EngineResponse::State { title, .. }) = server.tick_active_tab_timers() else { panic!("deberia haber estado que publicar") };
        assert_eq!(title, "DESPUES");
        assert!(server.tick_active_tab_timers().is_none(), "sin temporizadores pendientes no hay nada que publicar");
    }

    #[test]
    fn state_fingerprint_ignores_the_id_but_not_what_is_shown() {
        let server = EngineServer::new();
        let respuesta = server.state_response(Some("r1".to_string()));
        let publicacion = server.state_response(None);
        assert_eq!(state_fingerprint(&respuesta), state_fingerprint(&publicacion), "el mismo estado, pedido o espontaneo, no se publica dos veces");

        let EngineResponse::State { mut title, .. } = server.state_response(None) else { unreachable!() };
        title.push('!');
        let mut distinto = server.state_response(None);
        if let EngineResponse::State { title: t, .. } = &mut distinto {
            *t = title;
        }
        assert_ne!(state_fingerprint(&respuesta), state_fingerprint(&distinto));
        assert_eq!(state_fingerprint(&EngineResponse::Ok { id: None, message: "ok" }), None);
    }

    /// Una pestaña SIN pagina cargada (recien abierta) no deberia hacer
    /// panic al recibir un tick - no-op honesto.
    #[test]
    fn tick_active_tab_timers_on_an_empty_tab_is_a_silent_no_op() {
        let mut server = EngineServer::new();
        assert!(server.tick_active_tab_timers().is_none());
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

    #[test]
    fn find_form_ancestor_climbs_from_a_descendant_input_to_the_form() {
        let dom = r#"<html><body><form id="f"><div><input id="q" name="q"></div></form></body></html>"#;
        let form = find_form_ancestor(&find(dom, "q")).expect("deberia encontrar el form ancestro");
        let guard = form.read().unwrap();
        assert!(matches!(&guard.node_type, NodeType::Element { tag_name, .. } if tag_name == "form"));
    }

    #[test]
    fn find_form_ancestor_is_none_outside_any_form() {
        let dom = r#"<html><body><input id="suelto" name="q"></body></html>"#;
        assert!(find_form_ancestor(&find(dom, "suelto")).is_none());
    }

    #[test]
    fn find_submit_control_recognizes_a_button_without_an_explicit_type_as_submit() {
        let dom = r#"<html><body><form><button id="btn">Enviar</button></form></body></html>"#;
        assert!(find_submit_control(&find(dom, "btn")).is_some(), "un <button> sin type es submit por defecto, igual que el spec real");
    }

    #[test]
    fn find_submit_control_excludes_button_and_reset_types() {
        let dom = r#"<html><body>
            <form>
                <button id="normal" type="button">no envia</button>
                <button id="reset" type="reset">resetear</button>
            </form>
        </body></html>"#;
        assert!(find_submit_control(&find(dom, "normal")).is_none());
        assert!(find_submit_control(&find(dom, "reset")).is_none());
    }

    #[test]
    fn find_submit_control_climbs_from_nested_content_up_to_the_button() {
        let dom = r#"<html><body><form><button id="btn"><span id="etiqueta">Enviar</span></button></form></body></html>"#;
        // Mismo arbol para los dos nodos (`root_of` + `find_by_id`, no dos
        // llamadas a `find()`) - `find()` reparsea desde cero cada vez, asi
        // que comparar `Arc::ptr_eq` entre dos llamadas distintas siempre
        // daria falso aunque el contenido sea identico, ver `root_of`.
        let root = root_of(dom);
        let etiqueta = Node::find_by_id(&root, "etiqueta").expect("deberia existir");
        let btn = Node::find_by_id(&root, "btn").expect("deberia existir");
        let found = find_submit_control(&etiqueta).expect("deberia subir hasta el boton");
        assert!(std::sync::Arc::ptr_eq(&found, &btn));
    }

    #[test]
    fn find_submit_control_recognizes_input_type_submit_and_image() {
        let dom = r#"<html><body><form>
            <input id="s" type="submit" value="Enviar">
            <input id="i" type="image" src="go.png">
            <input id="txt" type="text">
        </form></body></html>"#;
        assert!(find_submit_control(&find(dom, "s")).is_some());
        assert!(find_submit_control(&find(dom, "i")).is_some());
        assert!(find_submit_control(&find(dom, "txt")).is_none(), "un input de texto normal no es un boton submit");
    }

    /// El punto real del envio de formularios: recoge nombre=valor de
    /// texto/checkbox marcado/select/textarea, EXCLUYE al checkbox sin
    /// marcar, al input sin `name`, y al boton submit cuando nadie lo
    /// clico (`submit_control: None`, el caso de Enter en un input).
    #[test]
    fn collect_form_data_gathers_named_controls_and_skips_the_rest() {
        let dom = r#"<html><body><form id="f">
            <input name="q" value="rust">
            <input name="sin_valor">
            <input type="hidden" name="csrf" value="abc123">
            <input type="checkbox" name="marcado" checked value="si">
            <input type="checkbox" name="sin_marcar">
            <input type="radio" name="opcion" value="a" checked>
            <input type="radio" name="opcion" value="b">
            <input name="deshabilitado" value="oculto" disabled>
            <input type="file" name="archivo">
            <input type="submit" name="boton" value="Enviar">
            <textarea name="comentario">hola mundo</textarea>
            <select name="pais"><option value="es">España</option><option value="fr" selected>Francia</option></select>
        </form></body></html>"#;
        let form = find_form_ancestor(&find(dom, "f")).expect("el propio <form> deberia ser su propio ancestro");
        let data = collect_form_data(&form, None);

        let get = |key: &str| data.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("q"), Some("rust"));
        assert_eq!(get("sin_valor"), Some(""), "sin value explicito, un input de texto envia cadena vacia, no se omite");
        assert_eq!(get("csrf"), Some("abc123"), "hidden se envia igual que cualquier otro campo de texto");
        assert_eq!(get("marcado"), Some("si"));
        assert_eq!(get("sin_marcar"), None, "un checkbox sin marcar no envia nada, ni vacio");
        assert_eq!(get("opcion"), Some("a"), "solo el radio marcado del grupo entra");
        assert_eq!(get("deshabilitado"), None, "disabled se omite por completo");
        assert_eq!(get("archivo"), None, "sin datos de fichero que enviar, se omite");
        assert_eq!(get("boton"), None, "sin submit_control (envio por Enter), ningun boton entra");
        assert_eq!(get("comentario"), Some("hola mundo"), "un textarea sin edicion usa su contenido de texto inicial");
        assert_eq!(get("pais"), Some("fr"), "el <option selected> gana sobre el primero");
    }

    #[test]
    fn collect_form_data_includes_only_the_clicked_submit_buttons_pair() {
        let dom = r#"<html><body><form id="f">
            <input type="submit" id="s1" name="accion" value="guardar">
            <input type="submit" id="s2" name="accion" value="borrar">
        </form></body></html>"#;
        // Mismo arbol para `form` y `clicked` - ver el mismo comentario en
        // `find_submit_control_climbs_from_nested_content_up_to_the_button`.
        let root = root_of(dom);
        let form = Node::find_by_id(&root, "f").expect("deberia existir");
        let clicked = Node::find_by_id(&root, "s1").expect("deberia existir");
        let data = collect_form_data(&form, Some(&clicked));
        let accion_values: Vec<&str> = data.iter().filter(|(k, _)| k == "accion").map(|(_, v)| v.as_str()).collect();
        assert_eq!(accion_values, vec!["guardar"], "solo el boton clicado (s1) deberia aportar su par, no s2 ni ambos");
    }

    #[test]
    fn collect_form_data_a_select_without_any_selected_option_defaults_to_the_first() {
        let dom = r#"<html><body><form id="f"><select name="talla"><option>S</option><option>M</option></select></form></body></html>"#;
        let form = find_form_ancestor(&find(dom, "f")).unwrap();
        let data = collect_form_data(&form, None);
        assert_eq!(data.iter().find(|(k, _)| k == "talla").map(|(_, v)| v.as_str()), Some("S"), "sin ningun option con selected, el primero es el valor por defecto real");
    }

    /// El punto real de `build_get_submit_url`: reemplaza la query de
    /// `action` (si tenia una) con los datos del formulario, en vez de
    /// anexarla - asi es el spec real de un envio GET.
    #[test]
    fn build_get_submit_url_replaces_any_existing_query_on_the_action() {
        let data = vec![("q".to_string(), "rust lang".to_string())];
        let url = build_get_submit_url("http://ejemplo.test/pagina", Some("/buscar?viejo=1"), &data).expect("deberia construir la URL");
        assert_eq!(url.as_str(), "http://ejemplo.test/buscar?q=rust+lang", "el query viejo de action deberia desaparecer, sustituido por los datos del formulario");
    }

    #[test]
    fn build_get_submit_url_without_an_action_submits_to_the_current_page() {
        let data = vec![("q".to_string(), "x".to_string())];
        let url = build_get_submit_url("http://ejemplo.test/pagina", None, &data).expect("deberia construir la URL");
        assert_eq!(url.as_str(), "http://ejemplo.test/pagina?q=x", "action ausente deberia enviar al documento actual, no fallar");
    }

    #[test]
    fn build_get_submit_url_resolves_a_relative_action_against_the_page_url() {
        let data: Vec<(String, String)> = vec![];
        let url = build_get_submit_url("http://ejemplo.test/carpeta/pagina.html", Some("buscar"), &data).expect("deberia construir la URL");
        assert_eq!(url.as_str(), "http://ejemplo.test/carpeta/buscar?", "una action relativa deberia resolverse contra el directorio de la pagina actual");
    }

    /// `application/x-www-form-urlencoded` tiene una regla propia que es
    /// facil de equivocar escribiendola a mano: el espacio se codifica
    /// como `+`, no como `%20`. Por eso se usa `url::form_urlencoded` y
    /// no un escapado casero.
    #[test]
    fn encode_form_body_uses_form_urlencoded_rules_not_plain_percent_encoding() {
        let data = vec![("q".to_string(), "rust lang".to_string()), ("año".to_string(), "2026".to_string())];
        let body = String::from_utf8(encode_form_body(&data)).expect("deberia ser UTF-8 valido");
        assert!(body.contains("q=rust+lang"), "el espacio deberia ser '+', no '%20': {body}");
        assert!(body.contains("a%C3%B1o=2026"), "los no-ASCII deberian ir percent-encoded en UTF-8: {body}");
        assert!(body.contains('&'), "los pares se separan con '&': {body}");
    }

    #[test]
    fn encode_form_body_of_no_fields_is_empty() {
        assert!(encode_form_body(&[]).is_empty());
    }

    /// A diferencia del GET (que REEMPLAZA la query con los datos), un
    /// POST deja intacta la que traiga el `action` - sus datos van en el
    /// cuerpo, asi que un `action="/buscar?pagina=2"` conserva ese
    /// parametro.
    #[test]
    fn resolve_submit_action_keeps_an_existing_query_unlike_the_get_path() {
        let url = resolve_submit_action("http://ejemplo.test/pagina", Some("/buscar?pagina=2")).expect("deberia resolver");
        assert_eq!(url.as_str(), "http://ejemplo.test/buscar?pagina=2");

        let get_url = build_get_submit_url("http://ejemplo.test/pagina", Some("/buscar?pagina=2"), &[("q".to_string(), "x".to_string())]).unwrap();
        assert_eq!(get_url.as_str(), "http://ejemplo.test/buscar?q=x", "el GET si deberia reemplazarla");
    }

    #[test]
    fn resolve_submit_action_without_an_action_targets_the_current_page() {
        let url = resolve_submit_action("http://ejemplo.test/pagina?ya=1", None).expect("deberia resolver");
        assert_eq!(url.as_str(), "http://ejemplo.test/pagina?ya=1");
    }

    /// Cualquier `method` que no sea `post` cae a GET, igual que el spec
    /// real (que solo reconoce `get`/`post`/`dialog` y trata lo invalido
    /// como el valor por defecto) - en vez de rechazar el envio, que era
    /// el comportamiento anterior para TODO lo que no fuera GET.
    #[tokio::test]
    async fn an_unrecognised_method_falls_back_to_get_instead_of_refusing_to_submit() {
        let mut server = server_with_page("http://ejemplo.test/pagina");
        let dom = r#"<html><body><form id="f" method="basura" action="/destino"><input name="a" value="1"></form></body></html>"#;
        let form = find_form_ancestor(&find(dom, "f")).unwrap();
        // Sin red real la navegacion fallara, pero el error debe ser de RED
        // (llego a intentarlo) y no el de "metodo no implementado" de antes.
        let response = server.submit_form(Some("sf1".to_string()), form, None).await;
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(
            !json.contains("no está implementado"),
            "un method desconocido deberia degradar a GET y ENVIARSE, no rechazarse: {json}"
        );
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

    /// El punto real del "form owner": dos `<form>` DISTINTOS que
    /// reutilicen el mismo `name` NO deberian pisarse entre si - antes se
    /// agrupaba por `name` en el documento entero, asi que marcar un radio
    /// del segundo formulario desmarcaba al del primero.
    #[test]
    fn radios_with_the_same_name_in_different_forms_do_not_share_a_group() {
        let dom = r#"<html><body>
            <form id="f1"><input id="a" type="radio" name="opcion" checked></form>
            <form id="f2"><input id="b" type="radio" name="opcion"></form>
        </body></html>"#;
        let root = root_of(dom);
        let b = Node::find_by_id(&root, "b").expect("deberia existir");
        apply_checkable_click(&root, &b);

        assert!(is_checked(&b), "el clicado queda marcado");
        assert!(
            is_checked(&Node::find_by_id(&root, "a").unwrap()),
            "un radio de OTRO <form> con el mismo name NO deberia desmarcarse - son grupos distintos (form owner distinto)"
        );
    }

    /// Dos radios SIN ningun `<form>` ancestro (mismo `name`, ninguno
    /// dentro de un formulario) siguen agrupandose entre si - "sin dueño"
    /// tambien es un form owner, y es el MISMO para los dos.
    #[test]
    fn radios_with_no_form_ancestor_at_all_still_share_a_group() {
        let dom = r#"<html><body>
            <input id="a" type="radio" name="opcion" checked>
            <input id="b" type="radio" name="opcion">
        </body></html>"#;
        let root = root_of(dom);
        let b = Node::find_by_id(&root, "b").expect("deberia existir");
        apply_checkable_click(&root, &b);

        assert!(is_checked(&b));
        assert!(!is_checked(&Node::find_by_id(&root, "a").unwrap()), "sin ningun <form>, los dos siguen en el mismo grupo (mismo form owner: ninguno)");
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
            None,
        );
        LoadedPage::new(url.to_string(), String::new(), page, runtime, None, ImageMap::new())
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

    #[tokio::test]
    async fn switch_tab_reports_an_error_for_an_unknown_tab_id() {
        let mut server = EngineServer::new();
        let response = server.switch_tab(Some("st1".to_string()), 99999).await;
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"type\":\"error\""));
    }

    #[tokio::test]
    async fn switch_tab_changes_the_active_tab() {
        let mut server = EngineServer::new();
        server.tabs.push(Tab::new(1));
        let response = server.switch_tab(Some("st2".to_string()), 1).await;
        assert_eq!(server.active_tab().id, 1);
        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"tab_id\":1"));
    }

    // ---- Pestañas dormidas: descarte por inactividad ----

    use std::time::Duration;

    /// Tres pestañas con pagina; la 0 y la 1 estan en segundo plano desde `now` y la 2 es la activa.
    fn server_with_three_tabs(discard_after: Option<Duration>) -> (EngineServer, std::time::Instant) {
        let mut server = EngineServer::new();
        server.discard_after = discard_after;
        let now = std::time::Instant::now();
        server.tabs[0].current_page = Some(loaded_page("http://a.test/"));
        for (id, url) in [(1, "http://b.test/"), (2, "http://c.test/")] {
            let mut tab = Tab::new(id);
            tab.current_page = Some(loaded_page(url));
            server.tabs.push(tab);
        }
        server.set_active_tab(1, now);
        server.set_active_tab(2, now);
        (server, now)
    }

    const HALF_HOUR: Duration = Duration::from_secs(30 * 60);

    #[test]
    fn the_discard_time_comes_from_the_environment_with_a_30_minute_default() {
        assert_eq!(parse_discard_after(None), Some(HALF_HOUR));
        assert_eq!(parse_discard_after(Some("basura")), Some(HALF_HOUR));
        assert_eq!(parse_discard_after(Some("90")), Some(Duration::from_secs(90)));
        assert_eq!(parse_discard_after(Some(" 0 ")), None, "0 desactiva el descarte");
    }

    #[test]
    fn an_idle_background_tab_is_discarded_after_the_time_and_keeps_its_address() {
        let (mut server, now) = server_with_three_tabs(Some(HALF_HOUR));

        assert!(server.discard_idle_tabs(now + HALF_HOUR - Duration::from_secs(1)).is_empty(), "antes de los 30 minutos no se toca nada");
        assert!(server.tabs.iter().all(|t| t.current_page.is_some()));

        let discarded = server.discard_idle_tabs(now + HALF_HOUR + Duration::from_secs(1));
        assert_eq!(discarded, vec![0, 1], "las dos de fondo");
        assert!(server.tabs[0].current_page.is_none() && server.tabs[1].current_page.is_none(), "su pagina se libero");
        assert!(server.tabs[2].current_page.is_some(), "la activa no se toca");

        let EngineResponse::Tabs { tabs, active_tab_id, .. } = server.list_tabs(None) else { panic!("debian ser pestañas") };
        assert_eq!(active_tab_id, 2);
        assert_eq!((tabs[0].discarded, tabs[0].url.as_str()), (true, "http://a.test/"), "sigue enseñando su direccion");
        assert_eq!((tabs[1].discarded, tabs[1].url.as_str()), (true, "http://b.test/"));
        assert!(!tabs[2].discarded);
        assert!(server.discard_idle_tabs(now + HALF_HOUR * 4).is_empty(), "no se descarta dos veces");
    }

    #[test]
    fn the_active_tab_is_never_discarded_however_long_it_stays() {
        let (mut server, now) = server_with_three_tabs(Some(HALF_HOUR));
        server.discard_idle_tabs(now + Duration::from_secs(10 * 3600));
        assert!(server.tabs[2].current_page.is_some());
        assert!(server.tabs[2].discarded.is_none());
    }

    #[test]
    fn a_tab_where_the_user_typed_is_not_discarded() {
        let (mut server, now) = server_with_three_tabs(Some(HALF_HOUR));
        server.tabs[0].current_page.as_mut().unwrap().edited = true;
        let discarded = server.discard_idle_tabs(now + HALF_HOUR * 2);
        assert_eq!(discarded, vec![1], "la 0 tiene texto escrito y se conserva");
        assert!(server.tabs[0].current_page.is_some());
    }

    #[test]
    fn with_discarding_disabled_nothing_is_ever_discarded_and_the_engine_never_wakes() {
        let (mut server, now) = server_with_three_tabs(None);
        assert!(server.discard_idle_tabs(now + HALF_HOUR * 100).is_empty());
        assert_eq!(server.next_deadline(), None);
    }

    #[test]
    fn the_engine_sleeps_until_the_next_real_job_and_not_in_between() {
        let (mut server, now) = server_with_three_tabs(Some(HALF_HOUR));
        // Sin temporizadores de JavaScript: solo el momento de descartar la primera inactiva.
        let deadline = server.next_deadline().expect("hay pestañas por descartar");
        assert_eq!(deadline, now + HALF_HOUR, "dormir hasta el descarte, sin despertares intermedios");

        // Un temporizador de la pestaña activa mas proximo manda.
        server.tabs[2].current_page.as_mut().unwrap().runtime.eval("setTimeout(function () {}, 5000);").unwrap();
        let con_timer = server.next_deadline().unwrap();
        assert!(con_timer < now + Duration::from_secs(60), "el temporizador de 5 s va antes que el descarte: {:?}", con_timer.duration_since(now));

        // Descartadas todas las de fondo y sin temporizadores: nada que hacer hasta la siguiente peticion.
        server.tabs[2].current_page.as_mut().unwrap().runtime.run_due_timers();
        server.discard_idle_tabs(now + HALF_HOUR + Duration::from_secs(1));
        let queda = server.next_deadline();
        assert!(queda.is_some_and(|d| d > now + Duration::from_secs(2)), "solo queda el temporizador pendiente de la activa");
    }

    #[test]
    fn the_background_clock_publishes_the_tab_list_when_it_discards() {
        let (mut server, now) = server_with_three_tabs(Some(HALF_HOUR));
        assert!(server.tick(now + Duration::from_secs(60)).is_empty(), "sin nada que hacer, no publica nada");
        let publicaciones = server.tick(now + HALF_HOUR + Duration::from_secs(1));
        let Some(EngineResponse::Tabs { id, tabs, .. }) = publicaciones.into_iter().find(|p| matches!(p, EngineResponse::Tabs { .. })) else {
            panic!("debia publicar la lista de pestañas")
        };
        assert_eq!(id, None, "es una publicacion espontanea, sin id");
        assert_eq!(tabs.iter().filter(|t| t.discarded).count(), 2);
    }

    /// Un servidor HTTP local con una pagina alta en `/p1` y `/p2`.
    fn tall_pages_server() -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = format!("<html><head><title>Pagina {path}</title></head><body style=\"margin:0\"><div style=\"height:3000px;background:#eeeeee\">{path}</div></body></html>");
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            }
        });
        port
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn going_back_to_a_discarded_tab_reloads_it_and_restores_the_scroll() {
        let port = tall_pages_server();
        let mut server = EngineServer::new();
        server.discard_after = Some(HALF_HOUR);
        let first = format!("http://127.0.0.1:{port}/p1");
        assert!(matches!(server.navigate(None, first.clone(), true).await, EngineResponse::State { .. }));
        server.tabs[0].scroll_offset_y = 500.0;
        let tab0_id = server.tabs[0].id;
        server.open_new_tab(None, Some(format!("http://127.0.0.1:{port}/p2"))).await;
        assert_eq!(server.active_tab().id, 1);

        let later = std::time::Instant::now() + HALF_HOUR + Duration::from_secs(5);
        assert_eq!(server.discard_idle_tabs(later), vec![tab0_id]);
        assert!(server.tabs[0].current_page.is_none() && server.tabs[0].discarded.is_some());

        let response = server.switch_tab(Some("vuelta".to_string()), tab0_id).await;
        let EngineResponse::State { tab_id, url, title, scroll_offset_y, .. } = response else { panic!("debia cargar la pagina: {response:?}") };
        assert_eq!(tab_id, tab0_id);
        assert_eq!(url, first, "vuelve a pedir la misma direccion");
        assert!(title.contains("/p1"), "con su titulo: {title}");
        assert_eq!(scroll_offset_y, 500.0, "y el scroll donde estaba");
        assert!(server.tabs[0].discarded.is_none(), "ya no esta descartada");
        assert_eq!(server.tabs[0].history.len(), 1, "recargar no añade otra entrada al historial");
        assert_eq!(server.tabs[0].history_index, Some(0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn if_the_network_fails_when_returning_the_tab_stays_discarded_and_can_be_retried() {
        let cerrado = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut server = EngineServer::new();
        server.discard_after = Some(HALF_HOUR);
        let mut tab = Tab::new(1);
        tab.history = vec![HistoryEntry { url: format!("http://127.0.0.1:{cerrado}/"), document_id: 3 }];
        tab.history_index = Some(0);
        tab.discarded = Some(DiscardedTab { url: format!("http://127.0.0.1:{cerrado}/"), title: "Antes".to_string() });
        server.tabs.push(tab);

        let response = server.switch_tab(Some("x".to_string()), 1).await;
        assert!(matches!(response, EngineResponse::Error { .. }), "{response:?}");
        assert!(server.tabs[1].discarded.is_some(), "sigue descartada: se puede reintentar");
        let EngineResponse::Tabs { tabs, .. } = server.list_tabs(None) else { panic!() };
        assert_eq!((tabs[1].discarded, tabs[1].title.as_str()), (true, "Antes"));
    }

    #[test]
    fn memory_of_released_pages_is_returned_when_the_engine_is_idle_and_not_before() {
        let (mut server, now) = server_with_three_tabs(None);
        server.last_activity = now;
        assert_eq!(server.next_deadline(), None, "sin paginas sueltas no hay nada que limpiar: el motor duerme");

        server.needs_cleanup = true;
        assert_eq!(server.next_deadline(), Some(now + IDLE_CLEANUP_DELAY), "despierta una vez, tras unos segundos de reposo");
        server.tick(now + Duration::from_secs(1));
        assert!(server.needs_cleanup, "aun no: el usuario acaba de hacer algo");
        server.tick(now + IDLE_CLEANUP_DELAY + Duration::from_millis(1));
        assert!(!server.needs_cleanup, "limpiado");
        assert_eq!(server.next_deadline(), None, "y vuelve a dormir sin despertar en bucle");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn navigating_and_closing_a_tab_mark_the_released_page_for_cleanup() {
        let port = tall_pages_server();
        let mut server = EngineServer::new();
        server.discard_after = None;
        assert!(matches!(server.navigate(None, format!("http://127.0.0.1:{port}/a"), true).await, EngineResponse::State { .. }));
        assert!(!server.needs_cleanup, "la primera carga no suelta ninguna pagina");
        assert!(matches!(server.navigate(None, format!("http://127.0.0.1:{port}/b"), true).await, EngineResponse::State { .. }));
        assert!(server.needs_cleanup, "navegar suelta la anterior");
        server.needs_cleanup = false;
        server.open_new_tab(None, Some(format!("http://127.0.0.1:{port}/c"))).await;
        server.close_tab(None, 1);
        assert!(server.needs_cleanup, "cerrar una pestaña con pagina la suelta");
    }

    #[test]
    fn typing_marks_the_page_as_edited_so_it_is_kept() {
        let mut server = server_from_html("<html><body><input id=\"a\"></body></html>");
        assert!(!server.active_tab().current_page.as_ref().unwrap().edited);
        server.mark_active_page_edited();
        assert!(server.active_tab().current_page.as_ref().unwrap().edited);
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

    #[tokio::test]
    async fn get_accessibility_tree_returns_aom_and_compact_prompt() {
        let mut server = EngineServer::new();
        // Sin página cargada debe devolver error
        let resp = server.accessibility_tree_response(Some("a1".to_string()));
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("\"type\":\"error\""));

        // Cargar página mock con HTML y elementos interactivos
        let (page, runtime) = build_page_keeping_runtime(
            r#"<html><body>
                <header><h1>Dashboard</h1></header>
                <main>
                    <button id="btn_submit">Guardar</button>
                    <input type="text" placeholder="Tu nombre" />
                </main>
            </body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
            None,
            None,
        );
        let tab = server.active_tab_mut();
        tab.current_page = Some(LoadedPage::new(
            "https://example.test/app".to_string(),
            "Dashboard".to_string(),
            page,
            runtime,
            None,
            ImageMap::new(),
        ));

        let resp2 = server.accessibility_tree_response(Some("a3".to_string()));
        let json2 = serde_json::to_string(&resp2).expect("serialize");
        assert!(json2.contains("\"type\":\"accessibility_tree\""));
        assert!(json2.contains("(button) \\\"Guardar\\\""));
        assert!(json2.contains("(input) \\\"Tu nombre\\\""));
    }

    // ---- Tarea 1.2 del plan: tiempos limite de los subrecursos -------------

    /// Servidor local: `/` y `/rapido.css` contestan al instante; cualquier
    /// otra ruta acepta la conexion, lee la peticion y no contesta nunca (el
    /// caso que antes dejaba la navegacion esperando para siempre). Devuelve
    /// el origen y un contador de las peticiones que ha recibido.
    fn server_with_slow_subresources() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let counter = counter.clone();
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 4096];
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    counter.fetch_add(1, Ordering::SeqCst);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (body, kind) = match path.as_str() {
                        "/" => ("<html><head><title>rapida</title><link rel=\"stylesheet\" href=\"/rapido.css\"><link rel=\"stylesheet\" href=\"/lento.css\"></head><body><p>hola</p><img src=\"/lento1.png\"><img src=\"/lento2.png\"></body></html>", "text/html"),
                        "/rapido.css" => ("h1 { color: #cc0000; }", "text/css"),
                        _ => {
                            std::thread::sleep(std::time::Duration::from_secs(60));
                            return;
                        }
                    };
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    fn server_with_budget(timeout_ms: u64, budget_ms: u64) -> EngineServer {
        // En memoria: estos tests no deben tocar las cookies ni el
        // almacenamiento del perfil real.
        let mut server = EngineServer::with_broker(engine_net::LocalBroker::in_memory().shared(), "local");
        server.subresource_timeout = std::time::Duration::from_millis(timeout_ms);
        server.subresource_budget = std::time::Duration::from_millis(budget_ms);
        server
    }

    /// Una pagina cuyos recursos secundarios no contestan se carga igual: el
    /// documento llega, los recursos lentos se omiten y se devuelve el estado.
    /// Antes la navegacion esperaba para siempre. Los topes se bajan para no
    /// esperar 8 y 10 s; el margen de 10 s del assert es contra maquinas
    /// lentas, no el valor que se prueba.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_whose_subresources_never_answer_still_loads() {
        let (origin, _hits) = server_with_slow_subresources();
        let mut server = server_with_budget(300, 600);

        let started = std::time::Instant::now();
        let response = server.navigate(None, format!("{origin}/"), true).await;

        assert!(matches!(response, EngineResponse::State { .. }), "debe devolver el estado, no un error: {response:?}");
        assert!(server.active_tab().current_page.is_some(), "la pagina debe quedar cargada");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "tardo {:?}", started.elapsed());
    }

    /// Lo que llego a tiempo se conserva: la hoja rapida entra aunque la
    /// lenta no conteste. El tope por recurso es de 2 s, no de 300 ms: la
    /// hoja rapida tiene que llegar DENTRO de el, y en un build de depuracion
    /// lento (el CI corre en Windows) 300 ms no es un margen fiable. Cuesta
    /// 2 s de test, que es lo que tarda en rendirse la hoja lenta.
    #[tokio::test(flavor = "multi_thread")]
    async fn what_arrived_in_time_is_kept_when_the_rest_times_out() {
        let (origin, _hits) = server_with_slow_subresources();
        let server = server_with_budget(2_000, 10_000);
        let page_url = url::Url::parse(&format!("{origin}/")).unwrap();
        let deadline = tokio::time::Instant::now() + server.subresource_budget;

        let started = std::time::Instant::now();
        let css = server.fetch_external_stylesheets(vec!["/lento.css".to_string(), "/rapido.css".to_string()], &page_url, deadline).await;

        assert!(css.contains("color: #cc0000"), "la hoja rapida debe conservarse: {css:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "tardo {:?}", started.elapsed());
    }

    /// El tope compartido es de toda la pagina: con el tiempo ya agotado no se
    /// hace ni una peticion mas (no se abre una conexion para abandonarla).
    #[tokio::test(flavor = "multi_thread")]
    async fn once_the_shared_budget_is_spent_no_further_requests_are_made() {
        let (origin, hits) = server_with_slow_subresources();
        let server = server_with_budget(300, 0);
        let page_url = url::Url::parse(&format!("{origin}/")).unwrap();
        let deadline = tokio::time::Instant::now() + server.subresource_budget;

        let css = server.fetch_external_stylesheets(vec!["/rapido.css".to_string()], &page_url, deadline).await;

        assert!(css.is_empty(), "con el tiempo agotado no debe descargarse nada: {css:?}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "no debe llegar ninguna peticion al servidor");
    }

    /// Los valores por defecto son los del plan: no cambian por accidente.
    #[test]
    fn the_default_subresource_limits_are_the_ones_in_the_plan() {
        assert_eq!(EngineServer::SUBRESOURCE_TIMEOUT, std::time::Duration::from_secs(8));
        assert_eq!(EngineServer::SUBRESOURCE_BUDGET, std::time::Duration::from_secs(10));
        assert_eq!(EngineServer::IMAGE_BUDGET, std::time::Duration::from_secs(4));
    }

    /// Tarea 1.4: las imagenes tienen su propio tope, mas corto que el de la
    /// pagina, y nunca pasan del compartido.
    #[tokio::test]
    async fn images_get_a_shorter_deadline_than_the_rest_of_the_page_but_never_a_longer_one() {
        let server = server_with_budget(1000, 10_000);
        let now = tokio::time::Instant::now();

        let long_shared = now + std::time::Duration::from_secs(10);
        let image_deadline = server.image_deadline(long_shared);
        assert!(image_deadline < long_shared, "con tiempo de sobra, las imagenes se cortan antes");
        assert!(image_deadline <= now + EngineServer::IMAGE_BUDGET + std::time::Duration::from_millis(50));

        let short_shared = now + std::time::Duration::from_secs(1);
        assert_eq!(server.image_deadline(short_shared), short_shared, "si al resto casi no le queda tiempo, tampoco a las imagenes");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_image_urls_are_fetched_once_and_shared() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#00ff00"/></svg>"##;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: image/svg+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", svg.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(svg);
            }
        });

        let server = server_with_budget(2_000, 10_000);
        let page_url = url::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let deadline = tokio::time::Instant::now() + server.subresource_budget;
        let images = server
            .fetch_images(
                vec!["/logo.svg".to_string(), "/logo.svg".to_string(), "./logo.svg".to_string(), "data:image/svg+xml,<svg/>".to_string()],
                &page_url,
                deadline,
            )
            .await;

        assert_eq!(hits.load(Ordering::SeqCst), 1, "misma URL resuelta solo debe descargarse una vez");
        assert!(images.contains_key("/logo.svg"));
        assert!(images.contains_key("./logo.svg"));
        assert!(std::sync::Arc::ptr_eq(&images["/logo.svg"], &images["./logo.svg"]));
    }

    #[tokio::test]
    async fn editing_with_arrows_selection_and_tab_navigation_works() {
        let mut server = server_from_html(
            "<html><body style=\"margin:0\">\
             <a id=\"lnk\" href=\"/x\" style=\"display:block;width:100px;height:24px\">ir</a>\
             <input id=\"campo\" type=\"text\" value=\"abde\" style=\"width:200px;height:30px\">\
             <button id=\"btn\" style=\"width:80px;height:30px\">Ok</button>\
             </body></html>",
        );
        // Tab desde sin foco enfoca el enlace; otro Tab pasa al input.
        let r1 = press(&mut server, "Tab").await;
        assert!(matches!(r1, EngineResponse::State { focused_is_text: Some(false), .. }));
        let r2 = press(&mut server, "Tab").await;
        assert!(matches!(r2, EngineResponse::State { focused_is_text: Some(true), .. }));

        // Editar en medio del texto con flechas y seleccion.
        press(&mut server, "ArrowLeft").await;
        press(&mut server, "ArrowLeft").await;
        press(&mut server, "c").await;
        assert_eq!(field_value(&server), "abcde");

        press(&mut server, "Shift+ArrowLeft").await;
        let sel = press(&mut server, "Shift+ArrowLeft").await;
        let EngineResponse::State { selected_text, .. } = sel else { panic!() };
        assert_eq!(selected_text.as_deref(), Some("bc"));
        press(&mut server, "Z").await;
        assert_eq!(field_value(&server), "aZde");

        press(&mut server, "Home").await;
        press(&mut server, "Delete").await;
        assert_eq!(field_value(&server), "Zde");

        press(&mut server, "Ctrl+a").await;
        press(&mut server, "Ctrl+x").await;
        assert_eq!(field_value(&server), "");
    }

    #[tokio::test]
    async fn hover_updates_css_hover_state_only_when_stylesheet_uses_hover() {
        let mut server = server_from_html(
            "<html><head><style>#btn { background: #111111; } #btn:hover { background: #ff0000; }</style></head>\
             <body style=\"margin:0\"><a id=\"btn\" href=\"/a\" style=\"display:block;width:120px;height:40px\">Botón</a></body></html>",
        );
        let r = server.handle(EngineRequest::Hover { id: None, x: 20.0, y: 20.0 }).await.0;
        assert!(matches!(r, EngineResponse::State { .. }), "con regla :hover deberia devolver State actualizado");
        let page = &server.active_tab().current_page.as_ref().unwrap().page;
        let btn = Node::find_by_id(&page.dom_root, "btn").unwrap();
        let box_btn = page.layout_root.find_box_for_node(&btn).unwrap();
        assert_eq!(box_btn.computed_style.get("background-color").map(String::as_str), Some("#ff0000"));
    }

    #[tokio::test]
    async fn drag_select_text_highlights_and_reports_selected_text() {
        let mut server = server_from_html(
            "<html><body style=\"margin:0\"><p style=\"margin:0;width:300px;height:30px\">Hola mundo desde Rust</p></body></html>",
        );
        let r = server
            .handle(EngineRequest::SelectText { id: None, from_x: 0.0, from_y: 10.0, to_x: 280.0, to_y: 15.0 })
            .await
            .0;
        let EngineResponse::State { selected_text, .. } = r else { panic!() };
        assert!(selected_text.as_deref().is_some_and(|t| t.contains("Hola")), "{selected_text:?}");
    }
}
