//! El global `window` (Fase 6.4) - minimo a proposito: hoy solo lleva
//! `window.open(url)`, porque es lo unico que hay al otro lado capaz de
//! hacer algo real (`core::server::EngineServer::open_new_tab`, que abre
//! una pestaña de verdad desde la Fase 4.5).
//!
//! **Lo que este `window` NO es**: en un navegador real `window` ES el
//! objeto global (`window.foo` y `foo` son lo mismo, `window === this` en
//! el ambito global). Aqui es un objeto NORMAL registrado como una
//! propiedad global mas, asi que `var x = 1; window.x` da `undefined`.
//! Hacerlo de verdad exige que el objeto global de Boa sea un proxy con
//! semantica de `WindowProxy`, bastante mas trabajo, y ninguna de las
//! capacidades que este motor tiene hoy lo necesita - documentarlo como
//! limitacion es mas honesto que fingir un `window` completo.
//!
//! **Como llega una llamada de JS a abrir una pestaña de verdad**: el
//! runtime JS vive DENTRO de una pagina cargada (`LoadedPage::runtime`),
//! y abrir una pestaña es una operacion del SERVIDOR entero
//! (`EngineServer`, que es quien tiene la lista de pestañas) - una capa
//! por encima, inalcanzable desde dentro del `Context` de Boa. El puente
//! es esta cola compartida: `window.open(url)` solo APUNTA la URL aqui, y
//! `core::server` la drena despues de procesar el clic y abre las
//! pestañas de verdad. Ver `PendingWindowOpens`.

use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use std::sync::{Arc, Mutex};

/// URLs que `window.open(...)` ha pedido abrir y que todavia nadie ha
/// atendido. Compartida entre el `Context` de Boa (que solo escribe) y
/// `core::server` (que solo drena) - de ahi `Arc<Mutex<...>>` y no un
/// simple `Vec`.
///
/// Es una COLA, no un solo hueco: un mismo handler puede llamar a
/// `window.open` varias veces, y perder todas menos la ultima seria un
/// bug silencioso.
pub type PendingWindowOpens = Arc<Mutex<Vec<String>>>;

/// Envoltorio para poder capturar la cola dentro de un `NativeFunction` de
/// Boa: `from_copy_closure_with_captures` exige que lo capturado
/// implemente `boa_gc::Trace`, y `Arc<Mutex<Vec<String>>>` no lo hace (no
/// contiene NADA gestionado por el recolector de Boa, que es justo lo que
/// `empty_trace!` declara).
#[derive(Clone)]
struct PendingCapture(PendingWindowOpens);

unsafe impl boa_gc::Trace for PendingCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for PendingCapture {}

/// Registra `window` con `open(url)` real. Devuelve la cola compartida, que
/// el llamador (`JsRuntime::register_window`) guarda para poder drenarla.
pub fn register_window(context: &mut Context) -> JsResult<PendingWindowOpens> {
    let pending: PendingWindowOpens = Arc::new(Mutex::new(Vec::new()));

    let open_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured, context| {
            let Some(url) = args.first() else {
                // `window.open()` sin argumentos abre "about:blank" en un
                // navegador real. Aqui no hay ninguna pagina en blanco
                // navegable, asi que no se encola nada - mas honesto que
                // encolar una URL inventada que fallaria al pedirse.
                return Ok(JsValue::null());
            };
            let url = url.to_string(context)?.to_std_string_escaped();
            if url.trim().is_empty() {
                return Ok(JsValue::null());
            }
            if let Ok(mut queue) = captured.0.lock() {
                queue.push(url);
            }
            // `null`, no un objeto Window: este motor no tiene ningun
            // `WindowProxy` que devolver, y un objeto vacio fingiria una
            // referencia a la pestaña nueva que no permitiria hacer nada.
            // `null` es ademas lo que devuelve un navegador real cuando el
            // bloqueador de ventanas emergentes impide la apertura, asi que
            // el codigo de paginas reales que comprueba el resultado ya
            // sabe tratarlo.
            Ok(JsValue::null())
        },
        PendingCapture(pending.clone()),
    );

    let window = ObjectInitializer::new(context)
        .function(open_fn, js_string!("open"), 1)
        .build();

    // `window.getComputedStyle` es LA MISMA funcion que el global
    // `getComputedStyle` (ver `dom_bindings`), no una copia: codigo real usa
    // las dos formas indistintamente, y tener solo una hacia fallar a la
    // otra con "not a callable function".
    if let Ok(gcs) = context.global_object().get(js_string!("getComputedStyle"), context) {
        if !gcs.is_undefined() {
            window.set(js_string!("getComputedStyle"), gcs, false, context)?;
        }
    }

    // `window.dispatchEvent` como DELEGACION a `document.documentElement`
    // (Fase 42) - MISMO nodo al que ya delega `window.addEventListener`/
    // `removeEventListener` (el shim de `register_history`, ver su
    // aviso): sin esto, un `window.dispatchEvent(new Event('x'))` manual
    // fallaria con "not a function" aunque `window.addEventListener('x',
    // ...)` ya funcionara. `addEventListener`/`removeEventListener` NO se
    // tocan aqui - `register_history` (que corre DESPUES, ver
    // `core::scripting`) ya los engancha, y hacerlo tambien aqui solo
    // se pisaria a si mismo. Guardado tras comprobar que `document` y
    // `documentElement` existen: sin DOM (el arnes de tests, `window` sin
    // `bind_dom` previo) no hay nada a lo que delegar.
    if let Ok(document) = context.global_object().get(js_string!("document"), context) {
        if let Some(document_obj) = document.as_object() {
            if let Ok(document_element) = document_obj.get(js_string!("documentElement"), context) {
                if let Some(document_element_obj) = document_element.as_object() {
                    if let Ok(dispatch) = document_element_obj.get(js_string!("dispatchEvent"), context) {
                        if !dispatch.is_undefined() {
                            window.set(js_string!("dispatchEvent"), dispatch, false, context)?;
                        }
                    }
                }
            }
        }
    }

    context.register_global_property(js_string!("window"), window, Attribute::all())?;

    // `navigator` (Fase 39). Una cantidad enorme de codigo real lee
    // `navigator.userAgent` en su primera linea para decidir rutas de
    // compatibilidad; sin el objeto, eso era un TypeError que se llevaba
    // por delante el script entero.
    //
    // El User-Agent declara lo que este motor ES, no imita a Chrome. Eso
    // significa que el sniffing de navegador de algunas paginas no nos
    // reconocera - preferible a mentir: una pagina que crea estar hablando
    // con Chrome usaria APIs que aqui no existen y fallaria mas adelante y
    // de forma mas confusa.
    let navigator = ObjectInitializer::new(context)
        .property(
            js_string!("userAgent"),
            js_string!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) NavegadorIA/0.1 (motor propio en Rust)"),
            Attribute::all(),
        )
        .property(js_string!("language"), js_string!("es-ES"), Attribute::all())
        .property(js_string!("platform"), js_string!("Win32"), Attribute::all())
        // `onLine` en `true`: si el motor esta cargando la pagina, hay red.
        .property(js_string!("onLine"), JsValue::from(true), Attribute::all())
        .build();
    context.register_global_property(js_string!("navigator"), navigator, Attribute::all())?;

    // `getComputedStyle` es del spec un metodo de `window`, pero se
    // registra como GLOBAL en `DomBindings::register` (Fase 8), que es
    // donde nace el snapshot de layout que consulta. Aqui se cuelga
    // ademas de `window` para que la forma canonica
    // (`window.getComputedStyle(el)`, que es como lo escribe casi todo el
    // codigo real) funcione igual. Guardado porque el orden de registro no
    // esta garantizado: un `JsRuntime` puede tener `window` sin haber
    // enlazado ningun DOM (`register_window` sin `bind_dom`), y entonces
    // `getComputedStyle` no existe y no hay nada que colgar.
    context.eval(boa_engine::Source::from_bytes(
        b"if (typeof getComputedStyle !== 'undefined') { window.getComputedStyle = getComputedStyle; }" as &[u8],
    ))?;

    Ok(pending)
}

#[cfg(test)]
mod tests {
    use crate::runtime::JsRuntime;

    #[test]
    fn window_open_queues_the_url_instead_of_opening_anything_by_itself() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime.eval("window.open('https://ejemplo.test/nueva')").expect("window.open no deberia lanzar");
        assert_eq!(runtime.take_pending_window_opens(), vec!["https://ejemplo.test/nueva".to_string()]);
    }

    #[test]
    fn taking_the_queue_empties_it_so_the_same_url_no_se_abre_dos_veces() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime.eval("window.open('https://ejemplo.test/una')").expect("no deberia lanzar");
        assert_eq!(runtime.take_pending_window_opens().len(), 1);
        assert!(
            runtime.take_pending_window_opens().is_empty(),
            "drenar la cola deberia vaciarla - si no, cada clic reabriria todas las pestañas de los clics anteriores"
        );
    }

    #[test]
    fn several_window_open_calls_are_all_queued_in_order() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime
            .eval("window.open('https://ejemplo.test/1'); window.open('https://ejemplo.test/2')")
            .expect("no deberia lanzar");
        assert_eq!(
            runtime.take_pending_window_opens(),
            vec!["https://ejemplo.test/1".to_string(), "https://ejemplo.test/2".to_string()]
        );
    }

    #[test]
    fn window_open_without_a_usable_url_queues_nothing() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime.eval("window.open(); window.open('   ')").expect("no deberia lanzar");
        assert!(
            runtime.take_pending_window_opens().is_empty(),
            "sin URL utilizable no deberia encolarse nada, en vez de inventarse una"
        );
    }

    #[test]
    fn window_open_returns_null_not_a_fake_window_object() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        let result = runtime.eval("window.open('https://ejemplo.test/x') === null").expect("no deberia lanzar");
        assert_eq!(result, "true", "deberia devolver null, no un objeto Window fingido");
    }

    /// Sin `register_window`, `window` no existe en absoluto - la respuesta
    /// honesta cuando no hay nada al otro lado capaz de abrir una pestaña
    /// (mismo criterio que `fetch` sin red, ver `JsRuntime::register_fetch`).
    #[test]
    fn window_is_not_defined_at_all_unless_it_was_registered() {
        let mut runtime = JsRuntime::new();
        assert!(
            runtime.eval("typeof window").is_ok_and(|t| t == "\"undefined\""),
            "sin registrar, window no deberia existir"
        );
    }

    /// `window.dispatchEvent` delega en `document.documentElement`, el
    /// MISMO nodo al que ya delega `window.addEventListener` (el shim de
    /// `register_history`) - un listener puesto por una via tiene que
    /// enterarse de un evento disparado por la otra.
    #[test]
    fn window_dispatch_event_reaches_a_listener_added_via_window_add_event_listener() {
        use engine_dom::HtmlParser;
        let dom = HtmlParser::parse("<html><body></body></html>");
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom).expect("bind_dom no deberia fallar");
        runtime.register_window().expect("window deberia registrarse");
        runtime.register_history().expect("history deberia registrarse (ahi vive el shim)");

        runtime.eval("var visto = false; window.addEventListener('miEvento', function() { visto = true; });").expect("registrar el listener deberia ser JS valido");
        runtime.eval("window.dispatchEvent(new Event('miEvento'))").expect("dispatchEvent no deberia lanzar");
        assert_eq!(runtime.eval("visto").unwrap(), "true", "el listener puesto via window.addEventListener deberia dispararse con window.dispatchEvent");
    }
}
