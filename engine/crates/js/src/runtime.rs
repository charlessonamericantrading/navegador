use boa_engine::{Context, Source};
use thiserror::Error;
use engine_dom::Node;
use std::sync::{Arc, RwLock};
use crate::dom_bindings::{DomBindings, EventRegistry};
use crate::event_loop::AsyncEventLoop;

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
    event_registry: Option<EventRegistry>,
}

impl JsRuntime {
    pub fn new() -> Self {
        let mut context = Context::default();
        let _ = AsyncEventLoop::register_microtasks(&mut context);
        Self { context, event_registry: None }
    }

    pub fn bind_dom(&mut self, dom_root: Arc<RwLock<Node>>) -> Result<(), JsError> {
        let registry = DomBindings::register(&mut self.context, dom_root).map_err(|e| JsError::Execution(e.to_string()))?;
        self.event_registry = Some(registry);
        Ok(())
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
    /// (no un panic) si `bind_dom` no se ha llamado todavia.
    pub fn dispatch_event(&mut self, node: &Arc<RwLock<Node>>, event_type: &str) -> Result<(), JsError> {
        let Some(registry) = self.event_registry.clone() else { return Ok(()) };
        DomBindings::dispatch_event(&registry, node, event_type, &mut self.context).map_err(|e| JsError::Execution(e.to_string()))
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
