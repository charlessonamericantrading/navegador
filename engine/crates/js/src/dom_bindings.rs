use boa_engine::{Context, JsResult, JsValue, NativeFunction};
use engine_dom::Node;
use std::sync::{Arc, RwLock};

pub struct DomBindings;

impl DomBindings {
    pub fn register(context: &mut Context, _dom_root: Arc<RwLock<Node>>) -> JsResult<()> {
        let print_fn = NativeFunction::from_fn_ptr(|_this, args, _context| {
            if let Some(msg) = args.first() {
                tracing::info!("[JS Engine Console Log]: {}", msg.display());
            }
            Ok(JsValue::undefined())
        });

        context.register_global_builtin_callable(
            boa_engine::js_string!("printEngineLog"),
            1,
            print_fn,
        )?;

        Ok(())
    }
}
