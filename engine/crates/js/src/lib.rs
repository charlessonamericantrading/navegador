pub mod runtime;

/// Fuerza una recoleccion de basura del motor de JavaScript.
///
/// El recolector de Boa trabaja por umbral de asignaciones: al soltar el
/// `JsRuntime` de una pagina (se cerro o se descarto su pestaña) sus objetos
/// quedan sin liberar hasta que OTRA pagina asigne lo bastante como para
/// disparar una recoleccion, y en un motor en reposo eso no llega nunca. Quien
/// libera paginas llama a esto para devolver la memoria de verdad.
pub fn collect_garbage() {
    boa_gc::force_collect();
}
pub mod cookie;
pub mod cssom;
pub mod dom_bindings;
pub mod dom_classes;
pub mod event_loop;
pub mod fetch;
pub mod history;
pub mod location;
pub mod modules;
pub mod mutation_observer;
pub mod platform;
pub mod storage;
pub mod test_harness;
pub mod timers;
pub mod window;
pub mod xhr;

pub use runtime::{JsRuntime, JsError};
pub use cssom::{BoxMetrics, LayoutSnapshot, LayoutSnapshotData};
pub use dom_bindings::{DocumentBindings, DomBindings};
pub use event_loop::AsyncEventLoop;
pub use test_harness::{TestHarness, TestResult};
