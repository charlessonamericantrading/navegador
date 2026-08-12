//! El global `history` (Fase 7) - `pushState`/`replaceState` reales, la
//! base de cualquier SPA (una pagina que cambia de "ruta" sin recargar).
//!
//! **Mismo puente que `window.open`** (ver `window.rs`): el runtime JS vive
//! DENTRO de una pagina y el historial es del SERVIDOR, una capa por
//! encima. `pushState` solo APUNTA la operacion en una cola compartida que
//! `core::server` drena y aplica sobre el historial de verdad de la
//! pestaña.
//!
//! **El argumento `state` se acepta y se IGNORA**, y por tanto
//! `event.state` en un `popstate` es siempre `null`. No es pereza: este
//! motor no tiene bfcache (ver la entrada de la Fase 4.4 en
//! ARCHITECTURE.md), asi que volver a un documento DISTINTO siempre lo
//! vuelve a pedir por red y construye un `JsRuntime` nuevo - el objeto
//! `state` original, que vive en el heap de Boa del runtime viejo, no
//! puede sobrevivir a eso de ninguna manera. Guardarlo serializado
//! fingiria una fidelidad que no existe (perderia funciones, referencias
//! ciclicas e identidad de objeto). Devolver `null` es lo unico honesto
//! mientras no haya bfcache.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction, Source};
use std::sync::{Arc, Mutex};

/// Una entrada que JS pidio añadir o sustituir en el historial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryOp {
    /// `history.pushState(...)` - añade una entrada NUEVA en el mismo
    /// documento (sin recargar nada).
    Push(String),
    /// `history.replaceState(...)` - reescribe la URL de la entrada ACTUAL,
    /// sin añadir ninguna.
    Replace(String),
}

pub type PendingHistoryOps = Arc<Mutex<Vec<HistoryOp>>>;

#[derive(Clone)]
struct HistoryCapture(PendingHistoryOps, bool);

unsafe impl boa_gc::Trace for HistoryCapture {
    boa_gc::empty_trace!();
}
impl boa_gc::Finalize for HistoryCapture {}

/// Registra `history` con `pushState`/`replaceState` reales, y ademas
/// engancha `window.addEventListener`/`removeEventListener` si hay un
/// `window` y un `document` ya registrados (ver el comentario del shim mas
/// abajo). Devuelve la cola compartida para que `core::server` la drene.
pub fn register_history(context: &mut Context) -> JsResult<PendingHistoryOps> {
    let pending: PendingHistoryOps = Arc::new(Mutex::new(Vec::new()));

    // `pushState(state, title, url)` y `replaceState(state, title, url)`
    // comparten TODA la logica salvo que entrada nueva / entrada
    // sustituida, de ahi el bool de la captura.
    let make = |is_push: bool, pending: PendingHistoryOps| {
        NativeFunction::from_copy_closure_with_captures(
            |_this, args: &[JsValue], captured, context| {
                // Argumentos reales del spec: (state, unused, url?). El
                // `state` se ignora a proposito (ver el doc del modulo) y
                // el segundo (`title`) lo ignoran TAMBIEN los navegadores
                // reales desde hace años.
                let Some(url) = args.get(2) else {
                    // Sin tercer argumento, el spec dice "no cambies la
                    // URL". Como aqui la entrada solo ES una URL, no hay
                    // nada real que registrar: no se encola nada.
                    return Ok(JsValue::undefined());
                };
                if url.is_null() || url.is_undefined() {
                    return Ok(JsValue::undefined());
                }
                let url = url.to_string(context)?.to_std_string_escaped();
                if url.trim().is_empty() {
                    return Ok(JsValue::undefined());
                }
                if let Ok(mut queue) = captured.0.lock() {
                    queue.push(if captured.1 { HistoryOp::Push(url) } else { HistoryOp::Replace(url) });
                }
                Ok(JsValue::undefined())
            },
            HistoryCapture(pending, is_push),
        )
    };

    let history = ObjectInitializer::new(context)
        .function(make(true, pending.clone()), js_string!("pushState"), 3)
        .function(make(false, pending.clone()), js_string!("replaceState"), 3)
        .build();
    context.register_global_property(js_string!("history"), history, Attribute::all())?;

    // `window.addEventListener` como DELEGACION al elemento raiz
    // (`document.documentElement`), no como registro aparte. Motivo: el
    // registro de eventos de este motor esta indexado por NODO del DOM
    // (ver `dom_bindings::EventRegistry`), y `window` no es un nodo. En vez
    // de montar un segundo registro paralelo solo para el, se aprovecha que
    // el elemento raiz es exactamente el ultimo escalon de propagacion
    // ANTES de `window` en el spec real: un evento que burbujea llega a los
    // dos, asi que un listener puesto en uno se comporta igual que en el
    // otro para todo lo que este motor sabe disparar hoy. `popstate` (que
    // el motor dispara sobre el elemento raiz, ver
    // `core::server::fire_popstate`) llega asi a un
    // `window.addEventListener('popstate', ...)` normal y corriente.
    //
    // Se define en JS y no en Rust porque es literalmente una redireccion
    // de una llamada a otra: hacerlo con un `NativeFunction` exigiria
    // recapturar el registro de eventos y duplicar la logica de
    // normalizacion de argumentos (`useCapture` booleano vs `{capture}`)
    // que `addEventListener` ya tiene resuelta.
    //
    // Guardado tras `typeof`: en un runtime sin DOM ni `window` (p.ej. el
    // arnes de tests, `test_harness.rs`) no hay nada a lo que delegar, y
    // fallar ahi seria un error inventado.
    let shim = r#"
        if (typeof window !== 'undefined' && typeof document !== 'undefined' && document.documentElement) {
            window.addEventListener = function (type, listener, options) {
                return document.documentElement.addEventListener(type, listener, options);
            };
            window.removeEventListener = function (type, listener, options) {
                return document.documentElement.removeEventListener(type, listener, options);
            };
        }
    "#;
    context.eval(Source::from_bytes(shim))?;

    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::JsRuntime;

    #[test]
    fn push_state_queues_the_new_url() {
        let mut runtime = JsRuntime::new();
        runtime.register_history().expect("history deberia registrarse");
        runtime.eval("history.pushState({a:1}, '', '/ruta/nueva')").expect("no deberia lanzar");
        assert_eq!(runtime.take_pending_history_ops(), vec![HistoryOp::Push("/ruta/nueva".to_string())]);
    }

    #[test]
    fn replace_state_queues_a_replace_not_a_push() {
        let mut runtime = JsRuntime::new();
        runtime.register_history().expect("history deberia registrarse");
        runtime.eval("history.replaceState(null, '', '/otra')").expect("no deberia lanzar");
        assert_eq!(runtime.take_pending_history_ops(), vec![HistoryOp::Replace("/otra".to_string())]);
    }

    /// El spec dice que sin tercer argumento la URL no cambia. Como aqui
    /// una entrada de historial ES una URL, eso significa que no hay nada
    /// real que registrar - encolar algo inventado seria peor.
    #[test]
    fn push_state_without_a_url_queues_nothing() {
        let mut runtime = JsRuntime::new();
        runtime.register_history().expect("history deberia registrarse");
        runtime.eval("history.pushState({a:1}, ''); history.pushState({a:1}, '', null)").expect("no deberia lanzar");
        assert!(runtime.take_pending_history_ops().is_empty());
    }

    #[test]
    fn taking_the_queue_empties_it() {
        let mut runtime = JsRuntime::new();
        runtime.register_history().expect("history deberia registrarse");
        runtime.eval("history.pushState(null, '', '/x')").expect("no deberia lanzar");
        assert_eq!(runtime.take_pending_history_ops().len(), 1);
        assert!(runtime.take_pending_history_ops().is_empty(), "drenar deberia vaciar la cola");
    }

    #[test]
    fn several_operations_keep_their_order() {
        let mut runtime = JsRuntime::new();
        runtime.register_history().expect("history deberia registrarse");
        runtime
            .eval("history.pushState(null,'','/a'); history.replaceState(null,'','/b'); history.pushState(null,'','/c')")
            .expect("no deberia lanzar");
        assert_eq!(
            runtime.take_pending_history_ops(),
            vec![
                HistoryOp::Push("/a".to_string()),
                HistoryOp::Replace("/b".to_string()),
                HistoryOp::Push("/c".to_string()),
            ]
        );
    }

    /// Sin DOM ni `window` que enganchar, `register_history` no debe fallar
    /// (el shim esta guardado tras un `typeof`) - solo deja `history`
    /// funcionando y `window.addEventListener` sin definir.
    #[test]
    fn registering_without_a_dom_still_works_and_skips_the_window_shim() {
        let mut runtime = JsRuntime::new();
        runtime.register_history().expect("deberia registrarse aunque no haya DOM");
        runtime.eval("history.pushState(null,'','/ok')").expect("history deberia seguir funcionando");
        assert_eq!(runtime.take_pending_history_ops().len(), 1);
    }
}
