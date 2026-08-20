//! El pipeline real (parseo -> JS inline -> cascada -> layout) extraido de
//! `main()` a una funcion que no abre ninguna ventana ni bloquea - la unica
//! forma de correr el motor de punta a punta sin cabeza (headless) que
//! existe hasta ahora. Primer paso real hacia poder medir el motor con
//! datos objetivos (Web Platform Tests u otro corredor propio, ver
//! ARCHITECTURE.md "Metrica de progreso") en vez de solo verificarlo a ojo
//! contra una ventana. Esto todavia NO es un corredor de WPT - es solo la
//! plomeria que cualquier corredor necesitaria para invocar el motor sin
//! interfaz grafica; interpretar tests reales de WPT (su arnes
//! `testharness.js`, sus assertions, vendorizar los archivos de test) sigue
//! sin empezar.
//!
//! Deliberadamente NO incluye la descarga por red (eso se queda en
//! `main.rs`): recibe HTML ya en memoria, para poder probarse con cadenas
//! literales sin depender de la red - igual que el resto de tests del
//! workspace.

use engine_css::{CssParser, StyleSheet};
use engine_dom::{HtmlParser, Node, NodeType};
use engine_js::{JsRuntime, TestResult};
use engine_layout::{ImageMap, LayoutBox, LayoutTreeBuilder};
use engine_net::NetworkEngine;
use engine_text::FontSet;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::scripting;

pub struct PageResult {
    /// `dom_root` y `stylesheet` los usa `pipeline.rs` en sus propios tests
    /// para comprobar la estructura del DOM/CSS, y `main.rs` los necesita
    /// de verdad para reconstruir el layout a un tamaño nuevo cuando la
    /// ventana se redimensiona (ver `gfx/src/window.rs`) sin tener que
    /// volver a parsear nada - por eso ambos son `pub` a proposito, parte
    /// real de la forma del resultado.
    pub dom_root: Arc<RwLock<Node>>,
    pub stylesheet: StyleSheet,
    pub layout_root: LayoutBox,
    /// Resultado de cada `<script>` inline ejecutado, en orden de
    /// documento - ver `scripting::execute_inline_scripts`.
    pub script_results: Vec<Result<String, String>>,
}

/// Procesa `html` ya en memoria contra un viewport dado: parsea, ejecuta
/// los `<script>` inline (con bindings DOM reales aunque minimos - ver
/// `js/dom_bindings.rs` para el detalle exacto de que existe y que no
/// todavia), extrae y aplica el CSS real que la propia pagina declara en
/// sus `<style>` (mas `css`, ver abajo) y calcula el layout. No pinta nada
/// ni abre ninguna ventana. El `JsRuntime` que ejecuta los scripts se
/// dropea al terminar - para mantenerlo vivo (necesario para poder
/// disparar eventos MAS TARDE, ver `JsRuntime::dispatch_event`), usar
/// `build_page_keeping_runtime`.
///
/// `css` es CSS ADICIONAL inyectado por quien llama (util para tests, o en
/// el futuro una hoja de usuario) - NO sustituye al CSS real de la pagina.
/// Los `<style>` del documento se concatenan primero, en orden de
/// documento, y `css` se añade despues; a igualdad de especificidad, lo que
/// venga despues en la cascada gana, asi que `css` puede pisar reglas de la
/// pagina si hace falta para un test.
///
/// `<link rel="stylesheet">` (externo) sigue sin descargarse AQUI - esta
/// funcion sigue sin tocar la red a proposito (ver el doc-comment del
/// modulo). Quien SI puede tocar la red (`core/server.rs::navigate`)
/// descubre esos `<link>` con `find_external_stylesheet_hrefs`, descarga
/// cada hoja, y las concatena dentro del propio parametro `css` antes de
/// llamar aqui - por eso ese parametro ya bastaba para conectar el caso
/// real sin cambiar esta firma.
///
/// `<script src="...">` (externo) SI necesita un parametro nuevo -
/// `external_scripts` - en vez de reusar `css`: a diferencia de una hoja de
/// estilos (donde solo importa el conjunto final de declaraciones, no el
/// orden relativo a nada mas), un script comparte estado real con los
/// scripts inline vecinos (`var x` declarada en uno debe verse en el
/// siguiente), asi que su ORDEN DE DOCUMENTO relativo a los `<script>`
/// inline importa. `external_scripts` mapea el `src` CRUDO tal como
/// aparece en el HTML (sin resolver) a su contenido ya descargado -
/// `find_external_script_srcs` descubre que `src` hacen falta, quien llama
/// los descarga y resuelve, y `scripting::run_scripts` los sustituye en su
/// sitio exacto en la lista de `<script>` del documento, sin alterar el
/// orden. Un `src` ausente del mapa (no se pudo descargar, o quien llama no
/// tiene red - como los tests de este archivo) se omite, igual que antes.
pub fn build_page(html: &str, css: &str, viewport_width: f32, viewport_height: f32, font_set: Option<&FontSet>, external_scripts: &HashMap<String, String>, images: &ImageMap) -> PageResult {
    let dom_root = HtmlParser::parse(html);
    let script_results = scripting::execute_inline_scripts(&dom_root, external_scripts);

    let mut combined_css = String::new();
    for style_tag in &Node::find_all_by_tag(&dom_root, "style") {
        combined_css.push_str(&Node::text_content(style_tag));
        combined_css.push('\n');
    }
    combined_css.push_str(css);

    let stylesheet = CssParser::parse(&combined_css);
    let layout_root = LayoutTreeBuilder::build(&dom_root, &stylesheet, viewport_width, viewport_height, font_set, images);

    PageResult { dom_root, stylesheet, layout_root, script_results }
}

/// Igual que `build_page`, pero ejecuta los `<script>` inline con
/// `scripting::execute_inline_scripts_with_harness` en vez de
/// `execute_inline_scripts` - `document.*` real Y `test`/`assert_*` del
/// arnes minimo (`engine_js::TestHarness`) disponibles en el mismo script,
/// para poder escribir tests estilo WPT que manipulan el DOM de verdad. Ver
/// `bin/wpt_runner.rs`, el unico llamador real de esto - `build_page`
/// normal se queda intacta a proposito: ninguna pagina real deberia ver
/// `test`/`assert_equals` como globales.
pub fn build_page_with_harness(html: &str, css: &str, viewport_width: f32, viewport_height: f32, font_set: Option<&FontSet>, external_scripts: &HashMap<String, String>, images: &ImageMap) -> (PageResult, Vec<TestResult>) {
    let dom_root = HtmlParser::parse(html);
    let (script_results, test_results) = scripting::execute_inline_scripts_with_harness(&dom_root, external_scripts);

    let mut combined_css = String::new();
    for style_tag in &Node::find_all_by_tag(&dom_root, "style") {
        combined_css.push_str(&Node::text_content(style_tag));
        combined_css.push('\n');
    }
    combined_css.push_str(css);

    let stylesheet = CssParser::parse(&combined_css);
    let layout_root = LayoutTreeBuilder::build(&dom_root, &stylesheet, viewport_width, viewport_height, font_set, images);

    (PageResult { dom_root, stylesheet, layout_root, script_results }, test_results)
}

/// Igual que `build_page`, pero devuelve el `JsRuntime` en vez de dejar que
/// `scripting::execute_inline_scripts_keeping_runtime` lo dropee al
/// terminar - necesario para `main.rs`, que necesita un runtime vivo
/// despues de que la ventana se abra para poder disparar un "click" real
/// sobre el nodo que un click del raton resuelva via hit-testing
/// (`LayoutBox::hit_test`) y `JsRuntime::dispatch_event`. `build_page`
/// normal (sin runtime) sigue siendo la opcion correcta para cualquier uso
/// headless que no necesite interactividad despues de la carga inicial
/// (`wpt_runner`, los tests de este mismo archivo).
///
/// `network`: un simple PASE-DE-MANO opaco hacia
/// `scripting::execute_inline_scripts_keeping_runtime` (Fase 4.3, `fetch()`
/// real) - `pipeline.rs` en si sigue sin hacer NADA con el (ninguna
/// resolucion de URL, ninguna llamada a `NetworkEngine::fetch`), asi que no
/// contradice el aviso de mas arriba sobre mantener este archivo libre de
/// red: ese aviso es sobre LOGICA de red (resolver/descargar), no sobre
/// reenviar un handle que otra capa mas abajo sabe usar.
///
/// `storage.csp` (Fase 26) tambien decide si los `<style>` EN LINEA del
/// documento se aplican - mismo criterio que ya usaba `script-src` para
/// `<script>` (Fase 24, ver `scripting::execute_inline_scripts_keeping_
/// runtime`): la doc de `net::csp` ya declaraba `style-src` como aplicado
/// de verdad, pero nadie llamaba `allows_inline("style-src")` en ningun
/// sitio - los `<style>` se concatenaban SIEMPRE, con o sin CSP. `None`
/// (sin `StorageContext`, ver el resto de este archivo) se permite, igual
/// que "sin politica" en el spec real.
pub fn build_page_keeping_runtime(html: &str, css: &str, viewport_width: f32, viewport_height: f32, font_set: Option<&FontSet>, external_scripts: &HashMap<String, String>, images: &ImageMap, network: Option<Arc<NetworkEngine>>, storage: Option<crate::scripting::StorageContext>) -> (PageResult, JsRuntime) {
    let dom_root = HtmlParser::parse(html);
    let allow_inline_style = storage.as_ref().is_none_or(|ctx| ctx.csp.allows_inline("style-src"));
    let (script_results, runtime) = scripting::execute_inline_scripts_keeping_runtime(&dom_root, external_scripts, network, storage);

    let mut combined_css = String::new();
    if allow_inline_style {
        for style_tag in &Node::find_all_by_tag(&dom_root, "style") {
            combined_css.push_str(&Node::text_content(style_tag));
            combined_css.push('\n');
        }
    }
    combined_css.push_str(css);

    let stylesheet = CssParser::parse(&combined_css);
    let layout_root = LayoutTreeBuilder::build(&dom_root, &stylesheet, viewport_width, viewport_height, font_set, images);

    (PageResult { dom_root, stylesheet, layout_root, script_results }, runtime)
}

/// Devuelve el valor CRUDO (sin resolver contra ninguna URL base) del
/// atributo `href` de cada `<link rel="stylesheet" href="...">` del
/// documento, en orden de documento. `rel` puede traer varios valores
/// separados por espacios (`rel="preload stylesheet"` es valido) - basta
/// con que "stylesheet" sea uno de ellos, comparado sin distinguir
/// mayusculas/minusculas como exige el spec de atributos con lista de
/// tokens.
///
/// Deliberadamente pura y sin red: resolver cada href relativo contra la
/// URL de la pagina y descargarlo es responsabilidad de quien llama
/// (`core/server.rs::navigate`), que es quien tiene tanto la URL base como
/// acceso a `NetworkEngine` - `pipeline.rs` no depende del crate `url` ni
/// de `engine-net` y no deberia empezar a hacerlo solo por esto.
pub fn find_external_stylesheet_hrefs(dom_root: &Arc<RwLock<Node>>) -> Vec<String> {
    Node::find_all_by_tag(dom_root, "link")
        .iter()
        .filter_map(|link_node| {
            let node = link_node.read().unwrap();
            let NodeType::Element { attributes, .. } = &node.node_type else {
                return None;
            };
            let is_stylesheet = attributes
                .get("rel")
                .is_some_and(|rel| rel.split_whitespace().any(|token| token.eq_ignore_ascii_case("stylesheet")));
            if !is_stylesheet {
                return None;
            }
            attributes.get("href").cloned()
        })
        .collect()
}

/// Devuelve el valor CRUDO del atributo `src` de cada `<script src="...">`
/// del documento, en orden de documento - el mismo orden en el que
/// `scripting::run_scripts` recorre `<script>` (inline o externo, sin
/// distincion) para ejecutarlos. Deliberadamente pura y sin red, mismo
/// motivo que `find_external_stylesheet_hrefs`. Un `<script>` SIN `src`
/// (inline) no aparece aqui - eso lo sigue resolviendo `run_scripts` leyendo
/// `text_content` directamente, esta funcion solo existe para que quien
/// llama sepa que URLs descargar de antemano.
pub fn find_external_script_srcs(dom_root: &Arc<RwLock<Node>>) -> Vec<String> {
    Node::find_all_by_tag(dom_root, "script")
        .iter()
        .filter_map(|script_node| {
            let node = script_node.read().unwrap();
            let NodeType::Element { attributes, .. } = &node.node_type else {
                return None;
            };
            attributes.get("src").cloned()
        })
        .collect()
}

/// Devuelve el valor CRUDO del atributo `src` de cada `<img src="...">` del
/// documento, en orden de documento - mismo patron exacto que
/// `find_external_script_srcs`/`find_external_stylesheet_hrefs`: pura, sin
/// red, quien llama (`core/server.rs::navigate`) resuelve cada `src`
/// contra la URL de la pagina, lo descarga y lo decodifica
/// (`engine_image::decode_image`) antes de construir el `ImageMap` que
/// `build_page*`/`LayoutTreeBuilder::build` necesitan. Un `<img>` sin `src`
/// no aparece aqui - `LayoutTreeBuilder` ya lo deja en una caja `src: ""`
/// que simplemente no encontrara nada en el mapa (ver
/// `apply_image_size_attributes`/`BoxType::Image` en `engine-layout::tree`).
pub fn find_image_srcs(dom_root: &Arc<RwLock<Node>>) -> Vec<String> {
    Node::find_all_by_tag(dom_root, "img")
        .iter()
        .filter_map(|img_node| {
            let node = img_node.read().unwrap();
            let NodeType::Element { attributes, .. } = &node.node_type else {
                return None;
            };
            attributes.get("src").cloned()
        })
        .filter(|src| !src.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_page_runs_the_full_pipeline_without_opening_a_window_or_blocking() {
        let page = build_page(
            "<html><body><h1>Titulo</h1><p>hola mundo</p></body></html>",
            "h1 { color: #ff0000; font-size: 32px; }",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );

        assert_eq!(Node::find_all_by_tag(&page.dom_root, "h1").len(), 1);
        assert!(page.script_results.is_empty(), "esta pagina no tiene <script>, no deberia haber resultados");
        assert_eq!(page.layout_root.dimensions.width, 800.0);
    }

    #[test]
    fn build_page_executes_inline_scripts_and_reports_their_results() {
        let page = build_page("<html><body><script>1 + 2</script></body></html>", "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new());
        assert_eq!(page.script_results.len(), 1);
        assert_eq!(page.script_results[0].as_deref(), Ok("3"));
    }

    /// `dom_root` y `script_results` deben venir del MISMO documento: un
    /// script que lee un elemento por id via los bindings DOM reales
    /// deberia ver el mismo arbol que tambien queda expuesto en
    /// `dom_root` - prueba que build_page no reconstruye/duplica el DOM
    /// entre pasos.
    #[test]
    fn build_page_gives_scripts_access_to_the_same_dom_that_gets_returned() {
        let page = build_page(
            r#"<html><body><p id="target">hola</p><script>document.getElementById('target').textContent</script></body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );

        assert_eq!(page.script_results.len(), 1);
        assert_eq!(page.script_results[0].as_deref(), Ok("\"hola\""));
        assert_eq!(Node::find_all_by_tag(&page.dom_root, "p").len(), 1);
    }

    fn find_box_with_style<'a>(root: &'a LayoutBox, key: &str) -> Option<&'a LayoutBox> {
        if root.computed_style.contains_key(key) {
            return Some(root);
        }
        root.children.iter().find_map(|c| find_box_with_style(c, key))
    }

    /// El hallazgo real que motivo esta tarea: antes, el CSS de la pagina
    /// (su propio `<style>`) se ignoraba por completo - solo se aplicaba lo
    /// que quien llamaba a build_page pasara aparte. Una pagina real con su
    /// propio `<style>` deberia verse estilada sin que nadie tenga que
    /// inyectar nada desde fuera.
    #[test]
    fn build_page_extracts_and_applies_style_tags_from_the_page_itself() {
        let page = build_page(
            "<html><head><style>body { background-color: #dbe9f4; }</style></head><body><p>hola</p></body></html>",
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );
        let styled = find_box_with_style(&page.layout_root, "background-color")
            .expect("el <style> de la pagina deberia haber aplicado background-color");
        assert_eq!(styled.computed_style.get("background-color").map(String::as_str), Some("#dbe9f4"));
    }

    fn storage_context_with_csp(csp: engine_net::ContentSecurityPolicy) -> crate::scripting::StorageContext {
        crate::scripting::StorageContext {
            storage: std::sync::Arc::new(std::sync::Mutex::new(engine_net::storage::WebStorage::new())),
            origin: "https://ejemplo.test".to_string(),
            csp,
            url: "https://ejemplo.test/".to_string(),
        }
    }

    /// El fix real de la Fase 26: antes de esto, `build_page_keeping_
    /// runtime` concatenaba TODOS los `<style>` de la pagina sin mirar CSP
    /// en absoluto, aunque `net::csp` ya declarara `style-src` como
    /// aplicado de verdad - un `<style>` inyectado por un atacante (vía
    /// una inyeccion de HTML que la pagina no sanea) se habria aplicado
    /// igual que uno legitimo, exactamente lo que CSP existe para impedir.
    #[test]
    fn a_style_src_none_policy_blocks_the_pages_own_inline_style_tag() {
        let (page, _runtime) = build_page_keeping_runtime(
            "<html><head><style>body { background-color: #ff0000; }</style></head><body><p>hola</p></body></html>",
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
            None,
            Some(storage_context_with_csp(engine_net::ContentSecurityPolicy::parse("style-src 'none'"))),
        );
        assert!(find_box_with_style(&page.layout_root, "background-color").is_none(), "style-src 'none' deberia haber bloqueado el <style> en linea de la propia pagina");
    }

    /// La otra mitad: una politica que SI declara `'unsafe-inline'` para
    /// `style-src` no deberia bloquear nada - CSP restringe solo lo que
    /// prohibe explicitamente.
    #[test]
    fn a_style_src_unsafe_inline_policy_still_allows_the_pages_own_inline_style_tag() {
        let (page, _runtime) = build_page_keeping_runtime(
            "<html><head><style>body { background-color: #ff0000; }</style></head><body><p>hola</p></body></html>",
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
            None,
            Some(storage_context_with_csp(engine_net::ContentSecurityPolicy::parse("style-src 'unsafe-inline'"))),
        );
        let styled = find_box_with_style(&page.layout_root, "background-color").expect("style-src 'unsafe-inline' deberia seguir permitiendo el <style> en linea");
        assert_eq!(styled.computed_style.get("background-color").map(String::as_str), Some("#ff0000"));
    }

    /// Sin `StorageContext` en absoluto (el camino que usa `wpt_runner`/
    /// tests sin red) el `<style>` deberia seguir aplicandose siempre -
    /// "sin CSP" no es lo mismo que "CSP que bloquea todo".
    #[test]
    fn without_a_storage_context_the_inline_style_tag_still_applies() {
        let (page, _runtime) = build_page_keeping_runtime(
            "<html><head><style>body { background-color: #00ff00; }</style></head><body><p>hola</p></body></html>",
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
            None,
            None,
        );
        let styled = find_box_with_style(&page.layout_root, "background-color").expect("sin StorageContext deberia comportarse como sin ninguna politica CSP");
        assert_eq!(styled.computed_style.get("background-color").map(String::as_str), Some("#00ff00"));
    }

    #[test]
    fn build_page_merges_page_style_tags_with_the_injected_css_parameter() {
        let page = build_page(
            "<html><head><style>body { background-color: #dbe9f4; }</style></head><body><h1>titulo</h1></body></html>",
            "h1 { color: #ff0000; }",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );
        assert!(find_box_with_style(&page.layout_root, "background-color").is_some(), "la regla del <style> de la pagina deberia seguir aplicando");
        assert!(find_box_with_style(&page.layout_root, "color").is_some(), "la regla inyectada por el parametro css tambien deberia aplicar");
    }

    #[test]
    fn build_page_concatenates_multiple_style_tags_in_document_order() {
        let page = build_page(
            r#"<html><head>
                <style>body { background-color: #dbe9f4; }</style>
                <style>h1 { color: #ff0000; }</style>
               </head><body><h1>titulo</h1></body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );
        assert!(find_box_with_style(&page.layout_root, "background-color").is_some(), "la regla del primer <style> deberia aplicar");
        assert!(find_box_with_style(&page.layout_root, "color").is_some(), "la regla del segundo <style> tambien deberia aplicar");
    }

    /// `dom_root`/`stylesheet` se exponen en `PageResult` precisamente para
    /// esto: reconstruir el layout a un tamaño de viewport nuevo sin volver
    /// a parsear nada - lo que hace de verdad `gfx::NativeEngineWindow` al
    /// redimensionar la ventana (ver su propio aviso). Prueba que rehacer
    /// el layout con los mismos dom_root+stylesheet a un ancho distinto
    /// produce un arbol con ese ancho, no el original congelado.
    #[test]
    fn dom_root_and_stylesheet_from_a_page_result_can_rebuild_layout_at_a_new_viewport_size() {
        let page = build_page("<html><body><p>hola</p></body></html>", "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new());
        assert_eq!(page.layout_root.dimensions.width, 800.0);

        let relaid_out = LayoutTreeBuilder::build(&page.dom_root, &page.stylesheet, 400.0, 300.0, None, &ImageMap::new());
        assert_eq!(relaid_out.dimensions.width, 400.0, "el layout reconstruido deberia reflejar el nuevo ancho, no seguir en 800");
    }

    #[test]
    fn build_page_with_harness_runs_the_full_pipeline_and_reports_test_results() {
        let (page, test_results) = build_page_with_harness(
            "<html><body><h1>titulo</h1><script>test(function() { assert_equals(1 + 1, 2); }, 'suma');</script></body></html>",
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );
        assert_eq!(test_results.len(), 1);
        assert!(test_results[0].passed);
        // Sigue siendo el mismo pipeline por debajo: layout y script_results
        // normales tambien deberian estar presentes, no solo test_results.
        assert_eq!(page.layout_root.dimensions.width, 800.0);
        assert_eq!(page.script_results.len(), 1);
    }

    /// El punto de tener una funcion _with_harness aparte: `build_page`
    /// normal NO deberia registrar `test`/`assert_equals` como globales -
    /// ninguna pagina real los tiene. Un script que los use sin el arnes
    /// deberia fallar con un ReferenceError real, no en silencio.
    #[test]
    fn build_page_normal_does_not_register_the_test_harness_globals() {
        let page = build_page("<html><body><script>typeof test === 'undefined'</script></body></html>", "", 800.0, 600.0, None, &HashMap::new(), &ImageMap::new());
        assert_eq!(page.script_results[0].as_deref(), Ok("true"), "test no deberia existir como global fuera del arnes");
    }

    fn find_box_for_dom_node<'a>(root: &'a LayoutBox, target: &Arc<RwLock<Node>>) -> Option<&'a LayoutBox> {
        if let Some(node) = &root.dom_node {
            if Arc::ptr_eq(node, target) {
                return Some(root);
            }
        }
        root.children.iter().find_map(|c| find_box_for_dom_node(c, target))
    }

    /// La prueba real de la cadena completa (#70-#72 en las notas de
    /// progreso): compone EXACTAMENTE lo que hace `main.rs::on_click` -
    /// hit-testear el layout, disparar el evento con el `JsRuntime`
    /// devuelto, y reconstruir el layout - sin abrir ninguna ventana ni
    /// depender de winit. Prueba que un click real sobre el elemento
    /// correcto ejecuta el listener registrado por el propio script de la
    /// pagina, y que su mutacion del DOM (aqui, cambiar el texto de otro
    /// elemento) es visible tanto en el DOM real como en un layout
    /// reconstruido despues - justo lo que se repintaria.
    #[test]
    fn hit_test_dispatch_event_and_relayout_together_reflect_a_click_listeners_dom_mutation() {
        let (page, mut runtime) = build_page_keeping_runtime(
            r#"<html><body>
                <div id="target">click aqui</div>
                <div id="output">antes</div>
                <script>
                    document.getElementById('target').addEventListener('click', function() {
                        document.getElementById('output').textContent = 'disparado';
                    });
                </script>
            </body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
            None,
            None,
        );

        let target_node = Node::find_by_id(&page.dom_root, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&page.layout_root, &target_node).expect("target deberia tener caja de layout");
        let click_x = target_box.dimensions.x + target_box.dimensions.width / 2.0;
        let click_y = target_box.dimensions.y + target_box.dimensions.height / 2.0;

        let hit = page.layout_root.hit_test(click_x, click_y).expect("el hit-test deberia encontrar el nodo target en su propio centro");
        assert!(Arc::ptr_eq(&hit, &target_node), "hit_test deberia devolver el nodo real de target, no una copia");

        runtime.dispatch_event(&hit, "click").expect("dispatch_event no deberia fallar");

        let output_node = Node::find_by_id(&page.dom_root, "output").expect("output deberia existir");
        assert_eq!(Node::text_content(&output_node), "disparado", "el listener deberia haber mutado el DOM real via textContent");

        // El punto extra sobre solo comprobar el DOM: la MISMA mutacion
        // debe verse en un layout reconstruido con el DOM ya mutado - es
        // literalmente lo que on_click hace antes de pedir un repintado.
        let new_layout = LayoutTreeBuilder::build(&page.dom_root, &page.stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let new_output_box = find_box_for_dom_node(&new_layout, &output_node).expect("output deberia seguir teniendo caja tras reconstruir el layout");
        let shows_updated_text = new_output_box.children.iter().any(|c| matches!(&c.box_type, engine_layout::BoxType::Text(text) if text == "disparado"));
        assert!(shows_updated_text, "el layout reconstruido deberia pintar el texto ya mutado, no el 'antes' original");
    }

    #[test]
    fn finds_the_href_of_a_stylesheet_link() {
        let dom = HtmlParser::parse(r#"<html><head><link rel="stylesheet" href="/estilos.css"></head><body></body></html>"#);
        assert_eq!(find_external_stylesheet_hrefs(&dom), vec!["/estilos.css".to_string()]);
    }

    #[test]
    fn ignores_link_tags_with_a_different_rel() {
        let dom = HtmlParser::parse(r#"<html><head><link rel="icon" href="/favicon.ico"></head><body></body></html>"#);
        assert!(find_external_stylesheet_hrefs(&dom).is_empty(), "un <link rel=icon> no es una hoja de estilos");
    }

    #[test]
    fn finds_multiple_stylesheets_in_document_order() {
        let dom = HtmlParser::parse(
            r#"<html><head>
                <link rel="stylesheet" href="/a.css">
                <link rel="stylesheet" href="/b.css">
               </head><body></body></html>"#,
        );
        assert_eq!(find_external_stylesheet_hrefs(&dom), vec!["/a.css".to_string(), "/b.css".to_string()]);
    }

    #[test]
    fn matches_stylesheet_as_one_of_several_space_separated_rel_tokens() {
        let dom = HtmlParser::parse(r#"<html><head><link rel="preload stylesheet" href="/c.css"></head><body></body></html>"#);
        assert_eq!(find_external_stylesheet_hrefs(&dom), vec!["/c.css".to_string()]);
    }

    #[test]
    fn link_without_href_is_skipped_instead_of_producing_an_empty_string() {
        let dom = HtmlParser::parse(r#"<html><head><link rel="stylesheet"></head><body></body></html>"#);
        assert!(find_external_stylesheet_hrefs(&dom).is_empty());
    }

    #[test]
    fn finds_the_src_of_an_external_script() {
        let dom = HtmlParser::parse(r#"<html><body><script src="/app.js"></script></body></html>"#);
        assert_eq!(find_external_script_srcs(&dom), vec!["/app.js".to_string()]);
    }

    #[test]
    fn inline_scripts_are_not_returned_by_find_external_script_srcs() {
        let dom = HtmlParser::parse("<html><body><script>1 + 1</script></body></html>");
        assert!(find_external_script_srcs(&dom).is_empty(), "un <script> inline no tiene src, no deberia aparecer aqui");
    }

    #[test]
    fn finds_multiple_script_srcs_in_document_order() {
        let dom = HtmlParser::parse(
            r#"<html><body><script src="/a.js"></script><script src="/b.js"></script></body></html>"#,
        );
        assert_eq!(find_external_script_srcs(&dom), vec!["/a.js".to_string(), "/b.js".to_string()]);
    }

    /// El punto real de `external_scripts`: un `<script src>` cuyo contenido
    /// ya fue descargado por quien llama se ejecuta como si fuera inline -
    /// mismo `JsRuntime`, mismo orden de documento.
    #[test]
    fn build_page_executes_a_prefetched_external_script() {
        let mut external = HashMap::new();
        external.insert("/app.js".to_string(), "1 + 41".to_string());
        let page = build_page(
            r#"<html><body><script src="/app.js"></script></body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &external,
            &ImageMap::new(),
        );
        assert_eq!(page.script_results.len(), 1);
        assert_eq!(page.script_results[0].as_deref(), Ok("42"));
    }

    /// Un `src` que no esta en el mapa (no se pudo descargar, o quien llama
    /// no tiene red) se omite en silencio - igual que el comportamiento
    /// anterior a esta tarea, no un panic ni un error fabricado.
    #[test]
    fn build_page_skips_an_external_script_missing_from_the_map() {
        let page = build_page(
            r#"<html><body><script src="/no-descargado.js"></script></body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &HashMap::new(),
            &ImageMap::new(),
        );
        assert!(page.script_results.is_empty(), "un src sin contenido pre-descargado deberia omitirse, no fallar");
    }

    /// La razon real de que esto necesite un parametro nuevo en vez de
    /// reusar `css`: un script externo comparte ESTADO con los inline
    /// vecinos, en su posicion exacta del documento - no solo su resultado
    /// final importa, sino que una variable que declare debe seguir viva
    /// para el siguiente `<script>`, sea inline o externo.
    #[test]
    fn external_and_inline_scripts_share_state_in_document_order() {
        let mut external = HashMap::new();
        external.insert("/contador.js".to_string(), "var contador = 10;".to_string());
        let page = build_page(
            r#"<html><body>
                <script src="/contador.js"></script>
                <script>contador + 5</script>
            </body></html>"#,
            "",
            800.0,
            600.0,
            None,
            &external,
            &ImageMap::new(),
        );
        assert_eq!(page.script_results.len(), 2);
        assert_eq!(page.script_results[1].as_deref(), Ok("15"), "el script inline deberia ver la variable declarada por el script externo anterior");
    }
}
