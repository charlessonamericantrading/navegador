use boa_engine::{Context, Source};
use thiserror::Error;
use engine_dom::Node;
use engine_net::NetworkEngine;
use std::sync::{Arc, RwLock};
use crate::cssom::LayoutSnapshot;
use crate::dom_bindings::{DocumentBindings, DomBindings};
use crate::event_loop::AsyncEventLoop;
use crate::fetch;

#[derive(Error, Debug)]
pub enum JsError {
    #[error("JS execution error: {0}")]
    Execution(String),
}

pub struct JsRuntime {
    pub context: Context,
    /// `Some` una vez que `bind_dom` haya corrido - lo que permite
    /// `dispatch_event` mas abajo. `None` en un `JsRuntime` que nunca
    /// enlazo un DOM (p.ej. `test_harness.rs` lo usa sin DOM alguno).
    document_bindings: Option<DocumentBindings>,
    /// `Some` una vez que `register_window` haya corrido (Fase 6.4) - la
    /// cola donde `window.open(url)` deja las URLs que pide abrir, para
    /// que `core::server` las drene y abra pestañas de verdad. `None` en
    /// un runtime sin `window` registrado, donde `window` ni siquiera
    /// existe como global.
    pending_window_opens: Option<crate::window::PendingWindowOpens>,
    /// Igual que `pending_window_opens` pero para `history.pushState`/
    /// `history.replaceState` (Fase 7) - ver `crate::history`.
    pending_history_ops: Option<crate::history::PendingHistoryOps>,
    /// `Some` una vez que `register_timers` haya corrido (Fase 14) - la
    /// cola de `setTimeout`/`setInterval` pendientes. `None` en un runtime
    /// sin temporizadores registrados, donde `setTimeout` ni siquiera
    /// existe como global.
    timers: Option<crate::timers::TimerQueue>,
}

impl JsRuntime {
    pub fn new() -> Self {
        let mut context = Context::default();
        let _ = AsyncEventLoop::register_microtasks(&mut context);
        Self { context, document_bindings: None, pending_window_opens: None, pending_history_ops: None, timers: None }
    }

    pub fn bind_dom(&mut self, dom_root: Arc<RwLock<Node>>) -> Result<(), JsError> {
        let registry = DomBindings::register(&mut self.context, dom_root).map_err(|e| JsError::Execution(e.to_string()))?;
        self.document_bindings = Some(registry);
        Ok(())
    }

    /// Registra el global `fetch` (Fase 4.3), respaldado por peticiones
    /// HTTP reales sobre `network` - ver `fetch::register_fetch` para el
    /// diseño completo y las simplificaciones declaradas (sobre todo: el
    /// bloqueo del hilo mientras la peticion esta en vuelo). Separado de
    /// `bind_dom` porque no todo `JsRuntime` tiene acceso a red disponible
    /// (p.ej. `core::main`, que no descarga recursos externos por diseño -
    /// ver `core::pipeline::build_page_keeping_runtime`) - sin llamar a
    /// esto, `fetch(...)` en JS lanza `ReferenceError: fetch is not
    /// defined`, la respuesta honesta cuando de verdad no hay red
    /// disponible, en vez de fingir un `fetch` que nunca conecta a nada.
    /// `page_origin` (Fase 20) activa la politica de mismo origen sobre
    /// las peticiones que haga este `fetch`. `None` en un documento sin
    /// URL propia, donde no hay origen contra el que comparar nada.
    pub fn register_fetch(&mut self, network: Arc<NetworkEngine>, page_url: Option<String>) -> Result<(), JsError> {
        fetch::register_fetch(&mut self.context, network, page_url).map_err(|e| JsError::Execution(e.to_string()))
    }

    /// Registra el constructor global `XMLHttpRequest` (Fase 9),
    /// respaldado por el MISMO `NetworkEngine` que `register_fetch` - ver
    /// `crate::xhr` para el diseño y, sobre todo, para por que es sincrono
    /// siempre. Mismo criterio de separacion que `register_fetch`: sin
    /// llamar a esto, `new XMLHttpRequest()` lanza `ReferenceError`, que es
    /// la respuesta honesta donde no hay red disponible.
    /// Mismo `page_origin` que `register_fetch`, por la misma razon:
    /// `XMLHttpRequest` esta sujeto a CORS igual que `fetch`.
    pub fn register_xhr(&mut self, network: Arc<NetworkEngine>, page_url: Option<String>) -> Result<(), JsError> {
        crate::xhr::register_xhr(&mut self.context, network, page_url).map_err(|e| JsError::Execution(e.to_string()))
    }

    /// Registra el accessor `document.cookie` (Fase 24, ver `crate::cookie`
    /// para el diseño completo: `HttpOnly` real, misma gramatica de
    /// atributos que un `Set-Cookie` de servidor). Mismo `NetworkEngine` y
    /// mismo `page_url` que `register_fetch`/`register_xhr` - las tres
    /// comparten el mismo `CookieStore` de sesion, asi que una cookie
    /// puesta por `document.cookie` viaja despues en el `Cookie:` de un
    /// `fetch()` posterior, igual que en un navegador real.
    ///
    /// Requiere que `bind_dom` ya haya corrido - sin `document` en el
    /// `Context`, es un no-op honesto (ver `cookie::register_cookie`), no
    /// un error.
    pub fn register_cookie(&mut self, network: Arc<NetworkEngine>, page_url: Option<String>) -> Result<(), JsError> {
        crate::cookie::register_cookie(&mut self.context, network, page_url).map_err(|e| JsError::Execution(e.to_string()))
    }

    /// Registra el global `window` con `open(url)` real (Fase 6.4). Igual
    /// criterio que `register_fetch`: separado del resto porque solo tiene
    /// sentido donde hay alguien capaz de ATENDER lo que se encole -
    /// `core::server`, el unico que tiene pestañas. Sin llamar a esto,
    /// `window` no existe (`typeof window === "undefined"`), que es la
    /// respuesta honesta ahi donde no hay ninguna pestaña que abrir.
    pub fn register_window(&mut self) -> Result<(), JsError> {
        let pending = crate::window::register_window(&mut self.context).map_err(|e| JsError::Execution(e.to_string()))?;
        self.pending_window_opens = Some(pending);
        Ok(())
    }

    /// Saca (y VACIA) las URLs que `window.open(...)` haya pedido abrir
    /// desde la ultima vez (Fase 6.4). Vaciar es parte del contrato: si no,
    /// cada clic reabriria tambien las pestañas pedidas por los clics
    /// anteriores. Lista vacia si `register_window` nunca corrio o si nadie
    /// llamo a `window.open` - los dos casos son "no hay nada que abrir",
    /// no un error.
    pub fn take_pending_window_opens(&mut self) -> Vec<String> {
        let Some(pending) = &self.pending_window_opens else { return Vec::new() };
        let Ok(mut queue) = pending.lock() else { return Vec::new() };
        std::mem::take(&mut *queue)
    }

    /// Registra `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval`
    /// (Fase 14). Mismo criterio de separacion que `register_fetch`/
    /// `register_window`: sin llamar a esto los globales no existen
    /// (`typeof setTimeout === "undefined"`), que es la respuesta honesta
    /// donde nadie va a llamar a `run_due_timers` para hacerlos avanzar.
    ///
    /// Conviene llamarlo DESPUES de `register_window` para que ademas
    /// queden colgados de `window.*` (muchisimo codigo real escribe
    /// `window.setTimeout`); si `window` no existe todavia, se registran
    /// igual como globales sueltos.
    pub fn register_timers(&mut self) -> Result<(), JsError> {
        let queue = crate::timers::register_timers(&mut self.context).map_err(|e| JsError::Execution(e.to_string()))?;
        self.timers = Some(queue);
        Ok(())
    }

    /// Ejecuta los temporizadores YA vencidos y devuelve cuantos callbacks
    /// se invocaron - cero si no hay ninguno vencido o si
    /// `register_timers` nunca corrio.
    ///
    /// Este motor no tiene un reloj de fondo propio: es esta llamada la
    /// que hace avanzar el tiempo de los temporizadores, y quien la hace
    /// es `core::server` tras cada operacion real (cargar, clic, escribir,
    /// tecla). Ver el doc-comment de `crate::timers` para la consecuencia
    /// exacta de esa simplificacion.
    ///
    /// El valor devuelto le sirve a quien llama para saber si merece la
    /// pena rehacer el layout: si no disparo ningun callback, nada pudo
    /// haber tocado el DOM.
    pub fn run_due_timers(&mut self) -> usize {
        let Some(queue) = self.timers.clone() else { return 0 };
        crate::timers::run_due_timers(&queue, &mut self.context)
    }

    /// Registra `localStorage`/`sessionStorage` para el origen de ESTA
    /// pagina (Fase 15). El almacen en si es de toda la sesion y lo
    /// conserva `core::server`; aqui solo se expone a JS, ya acotado al
    /// origen que se pase - un script no puede pedir el de otro origen
    /// porque no hay ningun parametro con el que hacerlo.
    ///
    /// Mismo criterio de separacion que `register_fetch`: sin llamar a
    /// esto los globales no existen, que es la respuesta honesta donde no
    /// hay ningun almacen que respalde nada.
    pub fn register_storage(&mut self, storage: crate::storage::SharedWebStorage, origin: String) -> Result<(), JsError> {
        crate::storage::register_storage(&mut self.context, storage, origin).map_err(|e| JsError::Execution(e.to_string()))
    }

    /// Registra el global `history` con `pushState`/`replaceState` reales
    /// (Fase 7), y engancha `window.addEventListener` al elemento raiz si
    /// hay `window` y DOM ya registrados - por eso conviene llamarlo
    /// DESPUES de `bind_dom` y `register_window` (ver `crate::history`).
    pub fn register_history(&mut self) -> Result<(), JsError> {
        let pending = crate::history::register_history(&mut self.context).map_err(|e| JsError::Execution(e.to_string()))?;
        self.pending_history_ops = Some(pending);
        Ok(())
    }

    /// Saca (y VACIA) las operaciones de historial que JS haya pedido desde
    /// la ultima vez (Fase 7) - mismo contrato que
    /// `take_pending_window_opens`.
    pub fn take_pending_history_ops(&mut self) -> Vec<crate::history::HistoryOp> {
        let Some(pending) = &self.pending_history_ops else { return Vec::new() };
        let Ok(mut queue) = pending.lock() else { return Vec::new() };
        std::mem::take(&mut *queue)
    }

    /// El buzon donde publicar el resultado del ultimo layout, para que
    /// `getComputedStyle`/`getBoundingClientRect` (Fase 8) devuelvan datos
    /// reales - ver `crate::cssom` para el diseño completo. `None` si
    /// `bind_dom` no ha corrido: sin DOM no hay documento del que publicar
    /// nada, y ese runtime tampoco tiene esas dos funciones registradas.
    ///
    /// Lo llama `core::server`, el unico que tiene a la vez el arbol de
    /// layout y el runtime de la pagina.
    pub fn layout_snapshot(&self) -> Option<LayoutSnapshot> {
        self.document_bindings.as_ref().map(DocumentBindings::layout_snapshot)
    }

    /// Dispara `event_type` sobre `node` de verdad, invocando los
    /// listeners reales registrados via `addEventListener` - SIN pasar
    /// por texto JS (`eval`). Pensada para invocarse desde codigo Rust
    /// cuando el motor tiene una fuente de eventos real que traducir a un
    /// nodo del DOM: el clic del raton ya esta cableado asi de punta a
    /// punta (`gfx::window` reporta `MouseInput`/`CursorMoved`,
    /// `core::main` hace hit-test sobre el `LayoutBox` real y llama aqui,
    /// ver ARCHITECTURE.md "Clic real del SO cableado de punta a punta") -
    /// scroll/teclado todavia no tienen fuente equivalente. No-op honesto
    /// (no un panic, `Ok(false)`) si `bind_dom` no se ha llamado todavia.
    ///
    /// Devuelve si algun listener llamo `event.preventDefault()` (Fase
    /// 4.2) - ver el doc-comment de `DomBindings::dispatch_event`.
    pub fn dispatch_event(&mut self, node: &Arc<RwLock<Node>>, event_type: &str) -> Result<bool, JsError> {
        let Some(registry) = self.document_bindings.clone() else { return Ok(false) };
        let result = DomBindings::dispatch_event(&registry, node, event_type, &mut self.context).map_err(|e| JsError::Execution(e.to_string()));
        self.drain_jobs();
        result
    }

    /// Igual que `dispatch_event`, pero con `.key` real puesto en el
    /// objeto `Event` (Fase 4.1) - usado por `core::server::press_key`,
    /// la unica fuente de eventos de teclado real que existe hoy (ver
    /// ARCHITECTURE.md, "Clic real del SO cableado de punta a punta" para
    /// el equivalente ya cableado de raton).
    pub fn dispatch_keyboard_event(&mut self, node: &Arc<RwLock<Node>>, event_type: &str, key: &str) -> Result<bool, JsError> {
        let Some(registry) = self.document_bindings.clone() else { return Ok(false) };
        let result =
            DomBindings::dispatch_keyboard_event(&registry, node, event_type, key, &mut self.context).map_err(|e| JsError::Execution(e.to_string()));
        self.drain_jobs();
        result
    }

    /// Vacia la cola de trabajos de Boa: microtasks (`queueMicrotask`, y las
    /// reacciones de cualquier `Promise`) y los trabajos diferidos que deja
    /// un `fetch()` con su respuesta ya descargada.
    ///
    /// **Esto arregla un bug real de la Fase 4.3, encontrado al construir la
    /// Fase 9.** Hasta aqui, lo unico que drenaba la cola era `eval` (al
    /// terminar cada `<script>`), asi que `fetch()` solo funcionaba durante
    /// la CARGA de la pagina. Llamado desde un manejador de eventos - que es
    /// como lo usa cualquier pagina real - la peticion HTTP se hacia de
    /// verdad, pero el trabajo que resuelve la `Promise` se quedaba
    /// encolado para siempre: ni `.then(...)` ni `await` llegaban a
    /// ejecutarse nunca, y la pagina se quedaba colgada sin ningun error
    /// visible. Verificado en vivo antes y despues del arreglo.
    ///
    /// Drenar aqui no es un parche: en el spec, disparar un evento por una
    /// accion real del usuario ES una tarea del bucle de eventos, y al
    /// final de CADA tarea se vacia la cola de microtasks. Es la misma
    /// razon por la que `eval` ya lo hacia al terminar cada script.
    ///
    /// No se drena en el `dispatchEvent` que un script invoca desde JS: eso
    /// ocurre DENTRO de una tarea (el script en curso), no al final de una,
    /// y el `eval` que lo envuelve ya se encarga al terminar.
    fn drain_jobs(&mut self) {
        self.context.run_jobs();
    }

    #[cfg(test)]
    fn eval_without_draining(&mut self, script: &str) -> Result<(), JsError> {
        self.context
            .eval(Source::from_bytes(script.as_bytes()))
            .map(|_| ())
            .map_err(|err| JsError::Execution(err.to_string()))
    }

    pub fn eval(&mut self, script: &str) -> Result<String, JsError> {
        let source = Source::from_bytes(script.as_bytes());
        let result = match self.context.eval(source) {
            Ok(value) => Ok(value.display().to_string()),
            Err(err) => Err(JsError::Execution(err.to_string())),
        };
        // Drenar la cola de microtasks despues de cada script, no solo al
        // final de todos - asi cada `<script>` ve los microtasks que el
        // mismo (o uno anterior) encolo, igual que "termina la tarea actual"
        // en un navegador real. Se drena tanto si el script tuvo éxito como
        // si no: lo que ya se encolo antes de un error a mitad de script
        // deberia seguir corriendo, igual que el spec.
        self.context.run_jobs();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_dom::HtmlParser;

    /// Regresion del bug de bucle de eventos encontrado al construir la
    /// Fase 9 (ver `drain_jobs`): disparar un evento desde Rust - lo que
    /// hace un clic real del usuario - tiene que VACIAR la cola de
    /// trabajos al terminar, igual que hace `eval` al final de cada
    /// script.
    ///
    /// Se prueba con `queueMicrotask` en vez de con `fetch` a proposito:
    /// mide exactamente lo mismo (si la cola se drena o no) sin necesitar
    /// ni red ni servidor, asi que este test corre siempre. El caso de
    /// `fetch` real - el sintoma que destapo el bug, donde un
    /// `.then(...)` dentro de un manejador de clic no se ejecutaba jamas -
    /// esta verificado en vivo contra `engine_server.exe`.
    #[test]
    fn dispatching_an_event_from_rust_drains_the_job_queue_afterwards() {
        let dom = HtmlParser::parse(r#"<html><body><div id="objetivo"></div><div id="salida">sin tocar</div></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom deberia funcionar");
        runtime
            .eval(
                r#"
                document.getElementById('objetivo').addEventListener('click', function () {
                    queueMicrotask(function () {
                        document.getElementById('salida').textContent = 'el microtask corrio';
                    });
                });
                "#,
            )
            .expect("registrar el listener deberia funcionar");

        let objetivo = Node::find_by_id(&dom, "objetivo").expect("objetivo deberia existir");
        runtime.dispatch_event(&objetivo, "click").expect("dispatch_event no deberia fallar");

        // Leido SIN pasar por `eval` (que drenaria la cola por su cuenta y
        // haria pasar el test aunque `dispatch_event` no drenase nada).
        let salida = Node::find_by_id(&dom, "salida").expect("salida deberia existir");
        assert_eq!(
            Node::text_content(&salida),
            "el microtask corrio",
            "el microtask encolado por un listener deberia haber corrido antes de que dispatch_event devuelva: \
             disparar un evento por una accion del usuario es una TAREA del bucle de eventos, y al final de cada \
             tarea se vacia la cola"
        );
    }

    /// La mitad complementaria: sin drenar, el microtask NO corre. Fija
    /// que el test de arriba mide de verdad el drenado y no algo que Boa
    /// hiciera solo.
    #[test]
    fn a_microtask_stays_queued_while_nobody_drains_the_queue() {
        let dom = HtmlParser::parse(r#"<html><body><div id="salida">sin tocar</div></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom deberia funcionar");
        runtime
            .eval_without_draining("queueMicrotask(function () { document.getElementById('salida').textContent = 'corrio'; });")
            .expect("encolar deberia funcionar");

        let salida = Node::find_by_id(&dom, "salida").expect("salida deberia existir");
        assert_eq!(Node::text_content(&salida), "sin tocar", "sin drenar la cola, el microtask no deberia haber corrido todavia");
    }

    #[test]
    fn dispatching_a_keyboard_event_from_rust_also_drains_the_queue() {
        let dom = HtmlParser::parse(r#"<html><body><div id="objetivo"></div><div id="salida">sin tocar</div></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom deberia funcionar");
        runtime
            .eval(
                r#"
                document.getElementById('objetivo').addEventListener('keydown', function (e) {
                    var tecla = e.key;
                    queueMicrotask(function () {
                        document.getElementById('salida').textContent = 'tecla ' + tecla;
                    });
                });
                "#,
            )
            .expect("registrar el listener deberia funcionar");

        let objetivo = Node::find_by_id(&dom, "objetivo").expect("objetivo deberia existir");
        runtime.dispatch_keyboard_event(&objetivo, "keydown", "Enter").expect("dispatch no deberia fallar");

        let salida = Node::find_by_id(&dom, "salida").expect("salida deberia existir");
        assert_eq!(Node::text_content(&salida), "tecla Enter");
    }
}
