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

    run_scripts(&mut runtime, &scripts, external_scripts, true)
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
            return (run_scripts(&mut runtime, &scripts, external_scripts, true), Vec::new());
        }
    };

    let script_results = run_scripts(&mut runtime, &scripts, external_scripts, true);
    let test_results = test_results.lock().unwrap().clone();
    (script_results, test_results)
}

/// El almacen de Web Storage de la sesion mas el ORIGEN de la pagina que
/// se esta construyendo - los dos datos que `localStorage`/
/// `sessionStorage` necesitan y que solo `core::server` conoce a la vez
/// (el almacen es suyo y vive entre navegaciones; el origen sale de la URL
/// que se acaba de cargar). Se pasan juntos porque por separado no sirven
/// de nada: un almacen sin origen no sabria a quien aislar.
pub struct StorageContext {
    pub storage: engine_js::storage::SharedWebStorage,
    pub origin: String,
    /// La politica de seguridad de contenido de la pagina (Fase 21) -
    /// decide si sus `<script>` EN LINEA se ejecutan. Los externos ya se
    /// filtraron antes de descargarse (`core::server::filter_by_csp`), asi
    /// que aqui solo queda la mitad inline, que es justo el vector
    /// principal de XSS.
    pub csp: engine_net::ContentSecurityPolicy,
    /// La URL COMPLETA de la pagina, no solo su origen (Fase 20.1). Hace
    /// falta como base para resolver las URLs RELATIVAS de `fetch()`/
    /// `XMLHttpRequest` (`xhr.open('GET', '/api/datos')`, comunisimo en
    /// codigo real). El origen solo no basta: una ruta relativa SIN barra
    /// inicial (`datos.json`) se resuelve contra el directorio de la
    /// pagina, no contra la raiz del sitio.
    ///
    /// Va junto al origen y no por separado a proposito: los dos salen de
    /// la misma URL, y pasarlos aparte abriria la puerta a que un dia no
    /// coincidieran y el aislamiento de red dejara de corresponderse con
    /// el de almacenamiento.
    pub url: String,
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
///
/// `storage`: `Some` registra `localStorage`/`sessionStorage` acotados al
/// origen que se pase (Fase 15, ver `StorageContext` justo encima).
pub fn execute_inline_scripts_keeping_runtime(
    dom_root: &Arc<RwLock<Node>>,
    external_scripts: &HashMap<String, String>,
    network: Option<Arc<NetworkEngine>>,
    storage: Option<StorageContext>,
) -> (Vec<Result<String, String>>, JsRuntime) {
    let scripts = Node::find_all_by_tag(dom_root, "script");

    // El origen de la pagina (Fase 20) sale del mismo `StorageContext`
    // que ya trae `core::server` - es literalmente el mismo dato, y
    // duplicar el parametro solo habria abierto la puerta a que un dia se
    // pasaran distintos y el aislamiento de red dejara de coincidir con el
    // de almacenamiento.
    let page_url = storage.as_ref().map(|ctx| ctx.url.clone());

    // Modulos ES (Fase 43): el cargador se instala al CONSTRUIR el `Context`
    // de Boa, asi que la decision hay que tomarla aqui y no despues. Sin URL
    // de pagina no hay base contra la que resolver un `import`, asi que ese
    // camino (`core::main`, los tests sin red) se queda con el runtime
    // clasico de siempre: un `<script type="module">` fallara ahi con su
    // motivo en vez de ejecutarse mal.
    let mut runtime = match &page_url {
        Some(url) => JsRuntime::with_modules(url),
        None => JsRuntime::new(),
    };

    // Todo lo que `core::server` descargo queda disponible para los `import`,
    // indexado por su URL ABSOLUTA - que es como el cargador los busca, y no
    // por el `src` crudo con el que vienen aqui. Incluye tanto los
    // `<script src>` como los `<link rel="modulepreload">`, que es como un
    // bundle declara sus fragmentos.
    if runtime.supports_modules() {
        if let Some(base) = &page_url {
            if let Ok(base) = url::Url::parse(base) {
                for (src, codigo) in external_scripts {
                    if let Ok(absoluta) = base.join(src) {
                        runtime.add_module_source(absoluta.as_str(), codigo);
                    }
                }
            }
        }
    }

    if let Err(e) = runtime.bind_dom(dom_root.clone()) {
        tracing::warn!("[js] no se pudo enlazar el DOM al runtime: {e}");
    }
    let storage_csp = storage.as_ref().map(|ctx| ctx.csp.clone());
    if let Some(network) = network {
        if let Err(e) = runtime.register_fetch(network.clone(), page_url.clone()) {
            tracing::warn!("[js] no se pudo registrar fetch: {e}");
        }
        // Fase 24: mismo `NetworkEngine`, mismo `CookieStore` de sesion que
        // `fetch`/`XMLHttpRequest` - ver `engine_js::cookie`. DESPUES de
        // `bind_dom` (ya corrio arriba) a proposito: el accessor se cuelga
        // del `document` que `bind_dom` acaba de crear.
        if let Err(e) = runtime.register_cookie(network.clone(), page_url.clone()) {
            tracing::warn!("[js] no se pudo registrar document.cookie: {e}");
        }
        // Fase 9: mismo `NetworkEngine`, misma condicion. Un motor con
        // `fetch` pero sin `XMLHttpRequest` deja sin red a toda la parte
        // de la web (enorme) que nunca migro - ver `engine_js::xhr`.
        if let Err(e) = runtime.register_xhr(network, page_url.clone()) {
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
    // DESPUES de `register_window` a proposito (Fase 42): `console` y
    // `performance` se cuelgan tambien de `window` si ya existe, porque en
    // este motor `window` no ES el objeto global (ver la cabecera de
    // `engine_js::window`), asi que un global suelto no aparece en `window.*`
    // y hay codigo real que usa las dos formas.
    if let Err(e) = runtime.register_platform() {
        tracing::warn!("[js] no se pudieron registrar las utilidades de plataforma: {e}");
    }
    // DESPUES de `bind_dom` y `register_window` a proposito (Fase 7):
    // `register_history` engancha ademas `window.addEventListener`
    // delegando en `document.documentElement`, asi que necesita que los
    // dos ya existan - ver el shim en `engine_js::history`.
    if let Err(e) = runtime.register_history() {
        tracing::warn!("[js] no se pudo registrar history: {e}");
    }
    // `location` - SIEMPRE, por la misma razon que `window` y los
    // temporizadores: un `location.href` sobre un global inexistente lanza
    // `ReferenceError` y aborta el script ENTERO, asi que no tenerlo no
    // costaba solo la navegacion, costaba toda la pagina. Sin URL (documento
    // sin origen) reporta `about:blank`, que es lo que hace un navegador
    // real, en vez de no existir.
    if let Err(e) = runtime.register_location(page_url.clone()) {
        tracing::warn!("[js] no se pudo registrar location: {e}");
    }
    // Temporizadores (Fase 14) - se registran SIEMPRE, por la misma razon
    // que `window`: `setTimeout` es de lo que TODA pagina real da por
    // sentado, y dejarlo sin definir rompe la pagina entera con un
    // `ReferenceError` en vez de solo perder la funcionalidad diferida.
    // DESPUES de `register_window` a proposito, para que ademas queden
    // colgados de `window.setTimeout` (forma que usa muchisimo codigo
    // real). Que un temporizador vencido llegue a EJECUTARSE depende de
    // si quien construyo este runtime llama a `run_due_timers`
    // (`core::server` lo hace tras cada operacion; `core::main` no).
    if let Err(e) = runtime.register_timers() {
        tracing::warn!("[js] no se pudieron registrar los temporizadores: {e}");
    }
    // `localStorage`/`sessionStorage` (Fase 15) - condicional, a
    // diferencia de `window`/temporizadores: solo tiene sentido donde hay
    // un almacen de sesion REAL detras y una URL de la que sacar el
    // origen, y quien tiene las dos cosas es `core::server`. Sin ellas los
    // globales no existen, que es la respuesta honesta (mismo criterio
    // que `fetch` sin red) - un documento construido en memoria no tiene
    // origen contra el que aislar nada.
    if let Some(ctx) = storage {
        if let Err(e) = runtime.register_storage(ctx.storage, ctx.origin) {
            tracing::warn!("[js] no se pudo registrar el almacenamiento web: {e}");
        }
    }

    // CSP: si la politica no permite `<script>` en linea, no se
    // ejecutan (Fase 21). Los EXTERNOS ya vienen filtrados desde
    // `core::server`, asi que los que sigan en el mapa estan autorizados.
    let allow_inline = storage_csp.as_ref().is_none_or(|csp| csp.allows_inline("script-src"));
    let script_results = run_scripts(&mut runtime, &scripts, external_scripts, allow_inline);

    // `DOMContentLoaded` NO se dispara aqui - se dispara en
    // `pipeline::build_page_keeping_runtime`, DESPUES de construir el
    // arbol de layout y publicar su snapshot para `getComputedStyle`. El
    // documento ya esta parseado entero y los scripts ya corrieron en este
    // punto (que es lo que el spec exige), pero dispararlo AQUI - como se
    // hacia antes - dejaba el listener mas comun de arranque de una pagina
    // real (leer una medida nada mas cargar) viendo un snapshot vacio,
    // porque el layout ni siquiera se habia calculado todavia.
    (script_results, runtime)
}

/// `external_scripts` mapea el `src` CRUDO (tal como aparece en el atributo,
/// sin resolver contra ninguna URL base) al contenido JS ya descargado por
/// quien llama - ver el doc-comment del modulo. Un `<script src>` cuyo
/// valor no esta en el mapa se omite con un aviso, en vez de fallar: puede
/// que la descarga fallara, o que quien llama (los tests de este archivo,
/// `wpt_runner`) no tenga red en absoluto.
/// `allow_inline` a `false` (Fase 21) salta los `<script>` SIN `src`:
/// la politica de seguridad de la pagina no los permite. Los que tienen
/// `src` no se comprueban aqui porque ya vienen filtrados de
/// `core::server::filter_by_csp` - si su contenido esta en el mapa, es que
/// estaba autorizado y se descargo.
fn run_scripts(runtime: &mut JsRuntime, scripts: &[Arc<RwLock<Node>>], external_scripts: &HashMap<String, String>, allow_inline: bool) -> Vec<Result<String, String>> {
    let clasificados: Vec<ScriptClasificado> = scripts
        .iter()
        .filter_map(|nodo| ScriptClasificado::de_nodo(nodo, runtime.supports_modules()))
        .collect();

    // Orden de ejecucion del spec, en tres pasadas (Fase 43). Antes habia una
    // sola: todos en orden de documento, ignorando `defer`/`async`/`type`.
    //
    // 1. Clasicos sin atributo, en orden de documento.
    // 2. `defer` y los MODULOS, en orden de documento. Un modulo es `defer`
    //    por defecto segun el spec, sin necesidad de escribirlo.
    // 3. `async`, que el spec deja correr en cuanto llegan. Aqui van al final
    //    y en orden de documento, porque todo esta ya descargado antes de
    //    ejecutar nada: no hay un "cuando llegue" que respetar. Es una
    //    aproximacion declarada, y la que menos codigo rompe - lo que `async`
    //    promete es "no bloqueo el parseo", no un orden concreto.
    let mut resultados = Vec::new();
    for orden in [Orden::Clasico, Orden::Diferido, Orden::Asincrono] {
        for script in clasificados.iter().filter(|s| s.orden == orden) {
            if let Some(r) = ejecutar_script(runtime, script, external_scripts, allow_inline) {
                resultados.push(r);
            }
        }
    }
    resultados
}

/// Cuando le toca correr a un `<script>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Orden {
    /// Sin `defer` ni `async`: bloquea el parseo, corre en orden.
    Clasico,
    /// `defer`, y tambien todo modulo (que es `defer` implicito).
    Diferido,
    /// `async`.
    Asincrono,
}

/// Un `<script>` ya clasificado: que codigo le corresponde y cuando.
struct ScriptClasificado {
    nodo: Arc<RwLock<Node>>,
    src: Option<String>,
    orden: Orden,
    /// Si se evalua como modulo ES en vez de como script clasico.
    es_modulo: bool,
}

impl ScriptClasificado {
    /// `None` significa "este `<script>` no debe ejecutarse", que NO es lo
    /// mismo que fallar: un `<script type="application/json">` es un
    /// contenedor de datos y ejecutarlo seria el error.
    fn de_nodo(nodo: &Arc<RwLock<Node>>, soporta_modulos: bool) -> Option<Self> {
        let (src, tipo, tiene_defer, tiene_async, tiene_nomodule) = {
            let n = nodo.read().unwrap();
            let NodeType::Element { attributes, .. } = &n.node_type else {
                return None;
            };
            (
                attributes.get("src").cloned(),
                attributes.get("type").cloned().unwrap_or_default(),
                attributes.contains_key("defer"),
                attributes.contains_key("async"),
                attributes.contains_key("nomodule"),
            )
        };

        let tipo_normalizado = tipo.trim().to_ascii_lowercase();
        let es_modulo = tipo_normalizado == "module";

        // `nomodule` marca el script de respaldo para navegadores SIN modulos.
        // Si este motor los soporta, saltarselo es lo correcto: ejecutarlo
        // ADEMAS del modulo duplicaria la aplicacion entera.
        if tiene_nomodule && soporta_modulos {
            tracing::info!("[js] <script nomodule> omitido: este motor ejecuta modulos");
            return None;
        }

        // Un `type` que no sea vacio, "module" o un tipo de JavaScript clasico
        // NO es codigo: `application/json`, `application/ld+json` (datos
        // estructurados, comunisimo) y `text/template` son contenedores que el
        // spec dice explicitamente que no se ejecutan. Antes de esta fase se
        // ejecutaban todos, y un `<script type="application/json">` con datos
        // producia un error de sintaxis que ensuciaba el diagnostico.
        const TIPOS_CLASICOS: [&str; 6] = [
            "",
            "text/javascript",
            "application/javascript",
            "text/ecmascript",
            "application/ecmascript",
            "module",
        ];
        if !TIPOS_CLASICOS.contains(&tipo_normalizado.as_str()) {
            tracing::info!("[js] <script type=\"{tipo}\"> no es codigo ejecutable, se omite");
            return None;
        }

        // Un modulo es `defer` por defecto. `async` sigue ganando si esta.
        let orden = if tiene_async {
            Orden::Asincrono
        } else if tiene_defer || es_modulo {
            Orden::Diferido
        } else {
            Orden::Clasico
        };

        // `defer` y `async` en un script SIN `src` no tienen efecto segun el
        // spec (no hay descarga que diferir), asi que corre como clasico.
        let orden = if src.is_none() && !es_modulo {
            Orden::Clasico
        } else {
            orden
        };

        Some(Self { nodo: nodo.clone(), src, orden, es_modulo })
    }
}

/// Ejecuta un script ya clasificado. `None` si no habia nada que ejecutar.
fn ejecutar_script(
    runtime: &mut JsRuntime,
    script: &ScriptClasificado,
    external_scripts: &HashMap<String, String>,
    allow_inline: bool,
) -> Option<Result<String, String>> {
    let codigo = match &script.src {
        Some(src) => match external_scripts.get(src) {
            Some(fetched) => fetched.clone(),
            None => {
                tracing::info!("[js] <script src=\"{src}\"> sin contenido pre-descargado, se omite");
                return None;
            }
        },
        None => {
            if !allow_inline {
                tracing::warn!("[csp] bloqueado por 'script-src': un <script> en linea (la politica no incluye 'unsafe-inline')");
                return None;
            }
            let codigo = Node::text_content(&script.nodo);
            if codigo.trim().is_empty() {
                return None;
            }
            codigo
        }
    };

    if script.es_modulo {
        // La URL del modulo importa para dos cosas: identificarlo en la cache
        // (dos `<script type="module">` distintos no son el mismo modulo) y
        // servir de base a sus `import`. Un modulo en linea no tiene URL
        // propia, asi que se le da una derivada de su posicion.
        let url = match &script.src {
            Some(src) => runtime.resolve_module_url(src),
            None => format!("about:inline-module-{:p}", Arc::as_ptr(&script.nodo)),
        };
        return Some(runtime.eval_module(&codigo, &url).map_err(|e| e.to_string()));
    }

    Some(runtime.eval(&codigo).map_err(|e| e.to_string()))
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
        let (script_results, mut runtime) = execute_inline_scripts_keeping_runtime(&dom, &HashMap::new(), None, None);
        assert_eq!(script_results.len(), 1, "el <script> que registra el listener deberia haberse ejecutado");

        let target = Node::find_by_id(&dom, "target").expect("target deberia existir");
        runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");

        let result = runtime.eval("document.getElementById('output').textContent").expect("leer el resultado deberia ser JS valido");
        assert_eq!(result, "\"disparado\"");
    }

    #[test]
    fn execute_inline_scripts_keeping_runtime_returns_a_bound_runtime_even_with_no_scripts() {
        let dom = HtmlParser::parse("<html><body><p>sin scripts</p></body></html>");
        let (script_results, mut runtime) = execute_inline_scripts_keeping_runtime(&dom, &HashMap::new(), None, None);
        assert!(script_results.is_empty());
        // Sin scripts no hay forma de que se haya registrado ningun
        // listener, pero el runtime en si deberia seguir siendo usable
        // (bind_dom se llamo) - probarlo con un eval trivial.
        assert_eq!(runtime.eval("1 + 1").expect("deberia poder evaluar JS"), "2");
    }
}
