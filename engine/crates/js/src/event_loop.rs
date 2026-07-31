use boa_engine::{Context, JsResult, JsValue, NativeFunction};

pub struct AsyncEventLoop;

impl AsyncEventLoop {
    pub fn register_microtasks(context: &mut Context) -> JsResult<()> {
        let microtask_fn = NativeFunction::from_fn_ptr(|_this, args, context| {
            if let Some(callback) = args.first() {
                if let Some(func) = callback.as_callable() {
                    func.call(&JsValue::undefined(), &[], context)?;
                }
            }
            Ok(JsValue::undefined())
        });

        context.register_global_builtin_callable(
            boa_engine::js_string!("queueMicrotask"),
            1,
            microtask_fn,
        )?;

        Ok(())
    }
}
