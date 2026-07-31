use boa_engine::{Context, Source};
use thiserror::Error;
use engine_dom::Node;
use std::sync::{Arc, RwLock};
use crate::dom_bindings::DomBindings;
use crate::event_loop::AsyncEventLoop;

#[derive(Error, Debug)]
pub enum JsError {
    #[error("JS execution error: {0}")]
    Execution(String),
}

pub struct JsRuntime {
    pub context: Context,
}

impl JsRuntime {
    pub fn new() -> Self {
        let mut context = Context::default();
        let _ = AsyncEventLoop::register_microtasks(&mut context);
        Self { context }
    }

    pub fn bind_dom(&mut self, dom_root: Arc<RwLock<Node>>) -> Result<(), JsError> {
        DomBindings::register(&mut self.context, dom_root)
            .map_err(|e| JsError::Execution(e.to_string()))
    }

    pub fn eval(&mut self, script: &str) -> Result<String, JsError> {
        let source = Source::from_bytes(script.as_bytes());
        match self.context.eval(source) {
            Ok(value) => Ok(value.display().to_string()),
            Err(err) => Err(JsError::Execution(err.to_string())),
        }
    }
}
