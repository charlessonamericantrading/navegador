//! `queueMicrotask` real: usa la cola de jobs que `Context` ya trae por
//! defecto (`SimpleJobQueue`, ver `job.rs` de `boa_engine`) en vez de llamar
//! al callback en el momento de la llamada. Antes, `queueMicrotask(fn)`
//! ejecutaba `fn` de forma sincrona e inmediata - un bug de orden observable
//! incluso sin ninguna otra primitiva async: en JS real,
//! `queueMicrotask(() => log('a')); log('b');` imprime "b" y luego "a" (el
//! microtask se difiere hasta que termina el script actual); la version
//! anterior de este archivo imprimia "a" y luego "b".
//!
//! El drenado de la cola pasa en `JsRuntime::eval` (`runtime.rs`), justo
//! despues de evaluar cada script - el punto mas parecido que existe
//! todavia a "termino la tarea actual", sin un event loop real (Fase 3).

use boa_engine::job::{Job, PromiseJob};
use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};

pub struct AsyncEventLoop;

impl AsyncEventLoop {
    pub fn register_microtasks(context: &mut Context) -> JsResult<()> {
        let microtask_fn = NativeFunction::from_fn_ptr(|_this, args, context| {
            let Some(callback) = args.first().and_then(JsValue::as_callable) else {
                // No es invocable (o falta el argumento): en JS real esto
                // lanzaria un TypeError; de momento, igual que el resto de
                // simplificaciones de este archivo, no hacer nada es mas
                // honesto que fingir que se encolo algo.
                return Ok(JsValue::undefined());
            };
            // Boa 0.22: la cola de microtareas es la de `PromiseJob`.
            //
            // La excepcion del callback se REPORTA y no se propaga: ante el
            // primer error, el ejecutor de Boa 0.22 vacia la cola entera, asi
            // que un `queueMicrotask` que lanza se llevaria por delante las
            // reacciones de promesas del resto de la pagina. En un navegador
            // se informa del error y las demas microtareas siguen.
            context.enqueue_job(Job::PromiseJob(PromiseJob::new(move |job_context| {
                if let Err(error) = callback.call(&JsValue::undefined(), &[], job_context) {
                    tracing::warn!("[js] excepcion no capturada en queueMicrotask: {error}");
                }
                Ok(JsValue::undefined())
            })));
            Ok(JsValue::undefined())
        });

        context.register_global_builtin_callable(js_string!("queueMicrotask"), 1, microtask_fn)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::JsRuntime;

    #[test]
    fn queue_microtask_defers_until_after_the_current_script_finishes() {
        let mut runtime = JsRuntime::new();
        runtime
            .eval("var order = []; queueMicrotask(() => order.push('microtask')); order.push('sync');")
            .unwrap();
        let result = runtime.eval("order.join(',')").unwrap();
        assert_eq!(result, "\"sync,microtask\"", "el microtask deberia correr despues del codigo sincrono que lo sigue, no antes");
    }

    #[test]
    fn multiple_microtasks_run_in_the_order_they_were_queued() {
        let mut runtime = JsRuntime::new();
        runtime
            .eval("var order = []; queueMicrotask(() => order.push('first')); queueMicrotask(() => order.push('second'));")
            .unwrap();
        let result = runtime.eval("order.join(',')").unwrap();
        assert_eq!(result, "\"first,second\"");
    }

    #[test]
    fn a_microtask_queued_by_another_microtask_still_drains_in_the_same_eval() {
        // La cola real sigue drenando mientras se sigan encolando trabajos
        // (ver SimpleJobQueue::run_jobs en boa_engine), no solo una pasada -
        // asi que un microtask que encola otro microtask tambien deberia
        // verse resuelto al terminar el eval, no quedarse a medias.
        let mut runtime = JsRuntime::new();
        runtime
            .eval("var order = []; queueMicrotask(() => { order.push('first'); queueMicrotask(() => order.push('nested')); });")
            .unwrap();
        let result = runtime.eval("order.join(',')").unwrap();
        assert_eq!(result, "\"first,nested\"");
    }

    /// Boa 0.22 descarta la cola entera ante el primer trabajo que falla:
    /// una microtarea que lanza no puede impedir que corran las siguientes.
    #[test]
    fn a_throwing_microtask_does_not_cancel_the_ones_queued_after_it() {
        let mut runtime = JsRuntime::new();
        runtime
            .eval("var order = []; queueMicrotask(() => { throw new Error('x'); }); queueMicrotask(() => order.push('after')); Promise.resolve().then(() => order.push('promise'));")
            .unwrap();
        let result = runtime.eval("order.join(',')").unwrap();
        assert_eq!(result, "\"after,promise\"");
    }

    #[test]
    fn queue_microtask_with_a_non_callable_argument_does_nothing_instead_of_panicking() {
        let mut runtime = JsRuntime::new();
        let result = runtime.eval("queueMicrotask(42); 'no crash'");
        assert_eq!(result.unwrap(), "\"no crash\"");
    }
}
