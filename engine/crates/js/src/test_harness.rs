//! Arnes minimo compatible con un subconjunto SINCRONO de `testharness.js`
//! (el arnes real de Web Platform Tests). Primer paso real hacia poder
//! ejecutar tests reales de WPT contra el motor (ver ARCHITECTURE.md,
//! "Metrica de progreso") sobre el pipeline headless que ya existe
//! (`core/src/pipeline.rs::build_page`).
//!
//! Esto NO es testharness.js interpretado de verdad: las paginas de test de
//! WPT lo cargan via `<script src="/resources/testharness.js">`, y este
//! motor todavia no descarga scripts externos (ver `scripting.rs`) - es una
//! reimplementacion nativa minima del subconjunto de la API que hace falta
//! para los tests puramente sincronos (`test(fn, name)` con `assert_*`
//! dentro). Deliberadamente NO soportado todavia: `async_test`,
//! `promise_test`, `assert_throws_js`/`assert_throws_dom`,
//! `assert_array_equals`, `setup()`, el resumen final que un runner de WPT
//! real produce (`add_completion_callback`)... Llamar a cualquiera de esos
//! desde un test fallara con un error real (no estan registrados como
//! globales), no en silencio.
//!
//! Se registra por separado de `DomBindings::register` a proposito: ninguna
//! pagina web real tiene `test`/`assert_equals` como globales - son
//! exclusivos del arnes de pruebas, no de la plataforma web.

use boa_engine::{js_string, Context, JsNativeError, JsResult, JsValue, NativeFunction};
use boa_gc::{Finalize, Trace};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    /// Mensaje del error lanzado por el assert que fallo (o por cualquier
    /// otra excepcion dentro de la funcion de test) - `None` si paso.
    pub failure_message: Option<String>,
}

/// Ver el mismo patron en `dom_bindings.rs` (`DomRootCapture`/
/// `ElementCapture`) - `Arc<Mutex<_>>` tampoco contiene punteros `Gc<_>`
/// propios de Boa, asi que `#[unsafe_ignore_trace]` es igual de correcto
/// aqui.
#[derive(Trace, Finalize, Clone)]
struct ResultsCapture(#[unsafe_ignore_trace] Arc<Mutex<Vec<TestResult>>>);

pub struct TestHarness;

impl TestHarness {
    /// Registra `test`/`assert_equals`/`assert_true`/`assert_false` como
    /// globales en `context` y devuelve el `Vec` compartido donde se van
    /// acumulando los resultados segun se ejecutan los `test(...)` del
    /// script - quien llame puede inspeccionarlo despues de `eval`.
    pub fn register(context: &mut Context) -> JsResult<Arc<Mutex<Vec<TestResult>>>> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let capture = ResultsCapture(results.clone());

        // `test(fn, name)`: llama a `fn` en el acto (sincrono - sin
        // `async_test`/`promise_test` todavia) y registra si lanzo o no.
        // Un assert que falla lanza una excepcion real (ver mas abajo), asi
        // que "no lanzo" y "paso" son la misma cosa aqui, igual que en el
        // testharness.js real.
        let test_fn = NativeFunction::from_copy_closure_with_captures(
            |_this, args, capture: &ResultsCapture, context| {
                let Some(func) = args.first().and_then(JsValue::as_callable).cloned() else {
                    return Ok(JsValue::undefined());
                };
                let name = match args.get(1) {
                    Some(v) => v.to_string(context)?.to_std_string_escaped(),
                    None => "(sin nombre)".to_string(),
                };
                let (passed, failure_message) = match func.call(&JsValue::undefined(), &[], context) {
                    Ok(_) => (true, None),
                    Err(e) => (false, Some(e.to_string())),
                };
                capture.0.lock().unwrap().push(TestResult { name, passed, failure_message });
                Ok(JsValue::undefined())
            },
            capture,
        );
        context.register_global_builtin_callable(js_string!("test"), 2, test_fn)?;

        let assert_equals = NativeFunction::from_fn_ptr(|_this, args, context| {
            let actual = args.first().cloned().unwrap_or(JsValue::undefined());
            let expected = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if actual.strict_equals(&expected) {
                return Ok(JsValue::undefined());
            }
            let description = match args.get(2) {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            Err(JsNativeError::error()
                .with_message(format!(
                    "assert_equals falló: {description} (esperado {}, obtenido {})",
                    expected.display(),
                    actual.display(),
                ))
                .into())
        });
        context.register_global_builtin_callable(js_string!("assert_equals"), 3, assert_equals)?;

        // Igual que el testharness.js real: identidad estricta con el
        // booleano `true`/`false`, no verdad/falsedad generica de JS -
        // `assert_true(1)` falla aunque `1` sea "truthy", porque `1 !== true`.
        let assert_true = NativeFunction::from_fn_ptr(|_this, args, context| {
            let actual = args.first().cloned().unwrap_or(JsValue::undefined());
            if actual.strict_equals(&JsValue::from(true)) {
                return Ok(JsValue::undefined());
            }
            let description = match args.get(1) {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            Err(JsNativeError::error()
                .with_message(format!("assert_true falló: {description} (obtenido {})", actual.display()))
                .into())
        });
        context.register_global_builtin_callable(js_string!("assert_true"), 2, assert_true)?;

        let assert_false = NativeFunction::from_fn_ptr(|_this, args, context| {
            let actual = args.first().cloned().unwrap_or(JsValue::undefined());
            if actual.strict_equals(&JsValue::from(false)) {
                return Ok(JsValue::undefined());
            }
            let description = match args.get(1) {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            Err(JsNativeError::error()
                .with_message(format!("assert_false falló: {description} (obtenido {})", actual.display()))
                .into())
        });
        context.register_global_builtin_callable(js_string!("assert_false"), 2, assert_false)?;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::JsRuntime;

    fn run(script: &str) -> Vec<TestResult> {
        let mut runtime = JsRuntime::new();
        let results = TestHarness::register(&mut runtime.context).expect("registrar el arnes no deberia fallar");
        runtime.eval(script).expect("el script de test deberia ser JS valido");
        let results = results.lock().unwrap().clone();
        results
    }

    #[test]
    fn test_records_a_pass_when_the_function_does_not_throw() {
        let results = run("test(function() { assert_equals(1 + 1, 2); }, 'suma basica');");
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
        assert_eq!(results[0].name, "suma basica");
        assert!(results[0].failure_message.is_none());
    }

    #[test]
    fn assert_equals_throws_with_a_clear_message_on_mismatch_and_the_test_records_it_as_failed() {
        let results = run("test(function() { assert_equals(1 + 1, 3, 'la suma esta mal'); }, 'suma rota');");
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        let message = results[0].failure_message.as_ref().expect("deberia haber mensaje de fallo");
        assert!(message.contains("la suma esta mal"), "el mensaje deberia incluir la descripcion: {message}");
    }

    #[test]
    fn assert_true_and_assert_false_enforce_strict_boolean_identity_not_just_truthiness() {
        // 1 es "truthy" en JS pero NO es === true - igual que el
        // testharness.js real, assert_true deberia rechazarlo.
        let results = run("test(function() { assert_true(1); }, 'truthy no es true');");
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed, "assert_true(1) deberia fallar: 1 no es estrictamente true");
    }

    #[test]
    fn assert_true_accepts_the_literal_boolean_true() {
        let results = run("test(function() { assert_true(true); }, 'true de verdad');");
        assert!(results[0].passed);
    }

    #[test]
    fn multiple_tests_are_all_recorded_independently_in_order() {
        let results = run(
            "test(function() { assert_true(true); }, 'primero');\
             test(function() { assert_true(false); }, 'segundo');\
             test(function() { assert_equals(2, 2); }, 'tercero');",
        );
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].name, "primero");
        assert!(results[0].passed);
        assert_eq!(results[1].name, "segundo");
        assert!(!results[1].passed);
        assert_eq!(results[2].name, "tercero");
        assert!(results[2].passed);
    }
}
