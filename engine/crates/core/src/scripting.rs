//! Conecta `engine-js` (el wrapper de Boa, hasta ahora aislado) con el
//! pipeline principal: encuentra los `<script>` (inline o externo) de la
//! pagina cargada y los ejecuta de verdad, en orden de documento.
//!
//! Simplificaciones honestas:
//! - `<script src="...">` (externo) NO se descarga aqui - este modulo sigue
//!   sin tocar la red a proposito, igual que `pipeline.rs`. `run_scripts`
//!   recibe `external_scripts: &HashMap<String, String>` (src crudo ->
//!   contenido ya descargado) desde quien SI tiene acceso a red
//!   (`core/server.rs`, via `pipeline::find_external_script_srcs` +
//!   `NetworkEngine`); un `src` ausente del mapa se omite, igual que antes
//!   de que este soporte existiera.
//! - Los scripts se ejecutan todos seguidos, de una vez, despues de parsear
//!   el documento completo - un navegador real intercala parseo y ejecucion
//!   (un `<script>` puede hacer `document.write` y modificar lo que queda
//!   por parsear). Aqui el parseo ya termino del todo para cuando el primer
//!   script corre, asi que esa interaccion no puede pasar estructuralmente,
//!   sin relacion con cuantos bindings DOM haya (`dom_bindings.rs` expone
//!   bastante mas que `printEngineLog` a estas alturas: getElementById/
//!   querySelector(All), setAttribute/textContent, createElement/
//!   appendChild/removeChild/insertBefore/replaceChild, classList/style,
//!   addEventListener/dispatchEvent - `document.write` en si mismo no esta
//!   entre ellos).
//! - Sin `async`/eventos: cada script corre hasta el final de forma
//!   sincrona con el mismo `JsRuntime` (mismo `Context` de Boa) que los
//!   anteriores, para que variables/funciones declaradas en un script -
//!   inline o externo, sin distincion - esten disponibles en el siguiente,
//!   como en una pagina real.

use engine_dom::{Node, NodeType};
use engine_js::{JsRuntime, TestHarness, TestResult};
use engine_net::NetworkEngine;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub fn execute_inline_scripts(dom_root: &Arc<RwLock<Node>>, external_scripts: &HashMap<String, String>) -> Vec<Result<String, String>> {
    let scripts = Node::find_all_by_tag(dom_root, "script");
    if scripts.is_empty() {
        return Vec::new();
    }

    let mut runtime = JsRuntime::new();
    if let Err(e) = runtime.bind_dom(dom_root.clone()) {
        tracing::warn!("[js] no se pudo enlazar el DOM al runtime: {e}");
    }

    run_scripts(&mut runtime, &scripts, external_scripts)
}

/// Igual que `execute_inline_scripts`, pero TAMBIEN registra
/// `TestHarness::register` en el mismo `Context` - `document.*` y
/// `test`/`assert_*` conviven sin conflicto (nombres de globales distintos),
/// asi que un script de test puede usar ambos a la vez, igual que un test
/// real de WPT que manipula el DOM. Ninguna pagina real usa esto - solo el
/// runner de tests (`bin/wpt_runner.rs`) - por eso es una funcion aparte y
/// no un flag en `execute_inline_scripts`.
pub fn execute_inline_scripts_with_harness(dom_root: &Arc<RwLock<Node>>, external_scripts: &HashMap<String, String>) -> (Vec<Result<String, String>>, Vec<TestResult>) {
    let scripts = Node::find_all_by_tag(dom_root, "script");
    if scripts.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut runtime = JsRuntime::new();
    if let Err(e) = runtime.bind_dom(dom_root.clone()) {
        tracing::warn!("[js] no se pudo enlazar el DOM al runtime: {e}");
    }
    let test_results = match TestHarness::register(&mut runtime.context) {
        Ok(results) => results,
        Err(e) => {
            tracing::warn!("[js] no se pudo registrar el arnes de tests: {e}");
            return (run_scripts(&mut runtime, &scripts, external_scripts), Vec::new());
        }
    };

    let script_results = run_scripts(&mut runtime, &scripts, external_scripts);
    let test_results = test_results.lock().unwrap().clone();
    (script_results, test_results)
}

/// Igual que `execute_inline_scripts`, pero DEVUELVE el `JsRuntime` en vez
/// de dropearlo al terminar - necesario para poder disparar eventos MAS
/// TARDE (`JsRuntime::dispatch_event`) sobre los listeners que un script
/// registro con `addEventListener` durante la carga inicial; sin esto, el
/// `EventRegistry` entero (y con el, cualquier listener) se destruye antes
/// de que la ventana siquiera se abra. A diferencia de
/// `execute_inline_scripts`/`execute_inline_scripts_with_harness` (que se
/// saltan crear el runtime si no hay ningun `<script>`, porque no habria
/// nada que evaluar), esta SIEMPRE crea y enlaza uno, incluso sin scripts -
/// quien llama quiere un runtime vivo pase lo que pase, no solo cuando hubo
/// algo que ejecutar al principio.
///
/// `network`: `Some` registra `fetch()` real (Fase 4.3, ver
/// `engine_js::fetch`) y `XMLHttpRequest` (Fase 9, ver `engine_js::xhr`)
/// ANTES de correr ningun script - asi el PRIMER
/// `<script>` de la pagina ya lo ve disponible, no solo listeners
/// registrados mas tarde. `None` (p.ej. `core::main`, que no descarga
/// recursos externos por diseño) deja `fetch` sin definir - `fetch(...)`
/// en JS lanza `ReferenceError`, la respuesta honesta cuando de verdad no
/// hay red disponible en ese contexto - y lo mismo para `new
/// XMLHttpRequest()`.
pub fn execute_inline_scripts_keeping_runtime(dom_root: &Arc<RwLock<Node>>, external_scripts: &HashMap<String, String>, network: Option<Arc<NetworkEngine>>) -> (Vec<Result<String, String>>, JsRuntime) {
    let scripts = Node::find_all_by_tag(dom_root, "script");

    let mut runtime = JsRuntime::new();
    if let Err(e) = runtime.bind_dom(dom_root.clone()) {
        tracing::warn!("[js] no se pudo enlazar el DOM al runtime: {e}");
    }
    if let Some(network) = network {
        if let Err(e) = runtime.register_fetch(network.clone()) {
            tracing::warn!("[js] no se pudo registrar fetch: {e}");
        }
        // Fase 9: mismo `NetworkEngine`, misma condicion. Un motor con
        // `fetch` pero sin `XMLHttpRequest` deja sin red a toda la parte
        // de la web (enorme) que nunca migro - ver `engine_js::xhr`.
        if let Err(e) = runtime.register_xhr(network) {
            tracing::warn!("[js] no se pudo registrar XMLHttpRequest: {e}");
        }
    }
    // `window` se registra SIEMPRE (Fase 6.4), a diferencia de `fetch`
    // (condicional). El motivo no es que aqui haya siempre pestañas que
    // abrir - no las hay en `core::main` - sino que `window` es el objeto
    // que TODA pagina real da por sentado: dejarlo sin definir haria que
    // un `window.loQueSea` de deteccion de capacidades, comunisimo en la
    // web real, lanzara `ReferenceError` y rompiera la pagina entera. Que
    // `window.open()` acabe abriendo una pestaña de verdad depende de si
    // quien construyo este runtime drena la cola (`core::server` lo hace
    // al procesar un clic; `core::main` no, porque ese camino no tiene
    // pestañas - ver ARCHITECTURE.md).
    if let Err(e) = runtime.register_window() {
        tracing::warn!("[js] no se pudo registrar window: {e}");
    }
    // DESPUES de `bind_dom` y `register_window` a proposito (Fase 7):
    // `register_history` engancha ademas `window.addEventListener`
    // delegando en `document.documentElement`, asi que necesita que los
    // dos ya existan - ver el shim en `engine_js::history`.
    if let Err(e) = runtime.register_history() {
        tracing::warn!("[js] no se pudo registrar history: {e}");
    }

    let script_results = run_scripts(&mut runtime, &scripts, external_scripts);
    (script_results, runtime)
}

/// `external_scripts` mapea el `src` CRUDO (tal como aparece en el atributo,
/// sin resolver contra ninguna URL base) al contenido JS ya descargado por
/// quien llama - ver el doc-comment del modulo. Un `<script src>` cuyo
/// valor no esta en el mapa se omite con un aviso, en vez de fallar: puede
/// que la descarga fallara, o que quien llama (los tests de este archivo,
/// `wpt_runner`) no tenga red en absoluto.
fn run_scripts(runtime: &mut JsRuntime, scripts: &[Arc<RwLock<Node>>], external_scripts: &HashMap<String, String>) -> Vec<Result<String, String>> {
    let mut results = Vec::new();
    for script_node in scripts {
        let src = match &script_node.read().unwrap().node_type {
            NodeType::Element { attributes, .. } => attributes.get("src").cloned(),
            _ => None,
        };

        let code = match src {
            Some(src) => match external_scripts.get(&src) {
                Some(fetched) => fetched.clone(),
                None => {
                    tracing::info!("[js] <script src=\"{src}\"> sin contenido pre-descargado, se omite");
                    continue;
                }
            },
            None => {
                let code = Node::text_content(script_node);
                if code.trim().is_empty() {
                    continue;
                }
                code
            }
        };

        results.push(runtime.eval(&code).map_err(|e| e.to_string()));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_dom::HtmlParser;

    #[test]
    fn executes_inline_script_and_returns_its_result() {
        let dom = HtmlParser::parse("<html><body><script>1 + 2</script></body></html>");
        let results = execute_inline_scripts(&dom, &HashMap::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_deref(), Ok("3"));
    }

    /// Ambos scripts corren en el mismo Context de Boa - una variable
    /// declarada en el primero deberia seguir viva para el segundo, igual
    /// que en una pagina real donde varios <script> comparten el mismo
    /// entorno global.
    #[test]
    fn scripts_share_state_across_the_same_document() {
        let dom = HtmlParser::parse("<html><body><script>var contador = 10;</script><script>contador + 5</script></body></html>");
        let results = execute_inline_scripts(&dom, &HashMap::new());
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].as_deref(), Ok("15"), "el segundo script deberia ver la variable del primero");
    }

    #[test]
    fn reports_script_errors_instead_of_silently_swallowing_them() {
        let dom = HtmlParser::parse("<html><body><script>esto no es JS valido (((</script></body></html>");
        let results = execute_inline_scripts(&dom, &HashMap::new());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err(), "un error de sintaxis JS deberia reportarse, no fingir exito");
    }

    #[test]
    fn external_script_missing_from_the_map_is_skipped_not_fetched() {
        let dom = HtmlParser::parse(r#"<html><body><script src="https://example.com/app.js"></script></body></html>"#);
        let results = execute_inline_scripts(&dom, &HashMap::new());
        assert!(results.is_empty(), "un <script src> sin contenido pre-descargado no deberia intentar ejecutarse (este modulo no toca la red)");
    }

    /// El punto real de `external_scripts`: si quien llama YA descargo el
    /// contenido (ver `core/server.rs::fetch_external_scripts`), un
    /// `<script src>` se ejecuta igual que uno inline.
    #[test]
    fn external_script_present_in_the_map_is_executed() {
        let dom = HtmlParser::parse(r#"<html><body><script src="/app.js"></script></body></html>"#);
        let mut external = HashMap::new();
        external.insert("/app.js".to_string(), "10 * 4".to_string());
        let results = execute_inline_scripts(&dom, &external);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_deref(), Ok("40"));
    }

    #[test]
    fn document_with_no_scripts_produces_no_results() {
        let dom = HtmlParser::parse("<html><body><p>sin scripts</p></body></html>");
        assert!(execute_inline_scripts(&dom, &HashMap::new()).is_empty());
    }

    #[test]
    fn execute_inline_scripts_with_harness_records_a_passing_test() {
        let dom = HtmlParser::parse("<html><body><script>test(function() { assert_equals(1 + 1, 2); }, 'suma');</script></body></html>");
        let (_, test_results) = execute_inline_scripts_with_harness(&dom, &HashMap::new());
        assert_eq!(test_results.len(), 1);
        assert!(test_results[0].passed);
        assert_eq!(test_results[0].name, "suma");
    }

    /// Un test que falla se reporta como fallido, con su mensaje - no se
    /// silencia ni se cuenta como exito por accidente.
    #[test]
    fn execute_inline_scripts_with_harness_reports_a_failing_test_instead_of_silencing_it() {
        let dom = HtmlParser::parse("<html><body><script>test(function() { assert_equals(1, 2, 'no deberian ser iguales'); }, 'resta rota');</script></body></html>");
        let (_, test_results) = execute_inline_scripts_with_harness(&dom, &HashMap::new());
        assert_eq!(test_results.len(), 1);
        assert!(!test_results[0].passed);
        let message = test_results[0].failure_message.as_ref().expect("deberia haber mensaje de fallo");
        assert!(message.contains("no deberian ser iguales"));
    }

    /// El punto real de esta funcion: `document.*` Y `test`/`assert_*`
    /// disponibles A LA VEZ en el mismo script, para poder escribir tests
    /// estilo WPT que manipulan el DOM real y lo comprueban con el arnes.
    #[test]
    fn execute_inline_scripts_with_harness_has_real_dom_bindings_available_too() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="target">hola</div><script>
                test(function() {
                    assert_equals(document.getElementById('target').textContent, 'hola');
                }, 'dom real dentro de un test');
            </script></body></html>"#,
        );
        let (_, test_results) = execute_inline_scripts_with_harness(&dom, &HashMap::new());
        assert_eq!(test_results.len(), 1);
        assert!(test_results[0].passed, "el test deberia poder leer el DOM real: {:?}", test_results[0].failure_message);
    }

    /// El punto real de esta funcion, probado a traves del punto de
    /// entrada de verdad que usaria `main.rs`: el runtime que devuelve
    /// sigue vivo y usable DESPUES de que esta funcion retorne - un
    /// listener registrado durante la carga inicial se dispara mas tarde
    /// desde Rust puro, sin volver a evaluar texto JS.
    #[test]
    fn execute_inline_scripts_keeping_runtime_returns_a_runtime_that_still_works_after_the_call() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="target"></div><div id="output"></div><script>
                document.getElementById('target').addEventListener('click', function() {
                    document.getElementById('output').textContent = 'disparado';
                });
            </script></body></html>"#,
        );
        let (script_results, mut runtime) = execute_inline_scripts_keeping_runtime(&dom, &HashMap::new(), None);
        assert_eq!(script_results.len(), 1, "el <script> que registra el listener deberia haberse ejecutado");

        let target = Node::find_by_id(&dom, "target").expect("target deberia existir");
        runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");

        let result = runtime.eval("document.getElementById('output').textContent").expect("leer el resultado deberia ser JS valido");
        assert_eq!(result, "\"disparado\"");
    }

    #[test]
    fn execute_inline_scripts_keeping_runtime_returns_a_bound_runtime_even_with_no_scripts() {
        let dom = HtmlParser::parse("<html><body><p>sin scripts</p></body></html>");
        let (script_results, mut runtime) = execute_inline_scripts_keeping_runtime(&dom, &HashMap::new(), None);
        assert!(script_results.is_empty());
        // Sin scripts no hay forma de que se haya registrado ningun
        // listener, pero el runtime en si deberia seguir siendo usable
        // (bind_dom se llamo) - probarlo con un eval trivial.
        assert_eq!(runtime.eval("1 + 1").expect("deberia poder evaluar JS"), "2");
    }
}
