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
use engine_dom::{HtmlParser, Node};
use engine_js::{JsRuntime, TestResult};
use engine_layout::{LayoutBox, LayoutTreeBuilder};
use engine_text::SystemFont;
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
/// pagina si hace falta para un test. `<link rel="stylesheet">` (externo)
/// queda fuera de alcance, mismo motivo que `<script src>`: exigiria
/// encolar otra peticion de red en el orden correcto.
pub fn build_page(html: &str, css: &str, viewport_width: f32, viewport_height: f32, font: Option<&SystemFont>) -> PageResult {
    let dom_root = HtmlParser::parse(html);
    let script_results = scripting::execute_inline_scripts(&dom_root);

    let mut combined_css = String::new();
    for style_tag in &Node::find_all_by_tag(&dom_root, "style") {
        combined_css.push_str(&Node::text_content(style_tag));
        combined_css.push('\n');
    }
    combined_css.push_str(css);

    let stylesheet = CssParser::parse(&combined_css);
    let layout_root = LayoutTreeBuilder::build(&dom_root, &stylesheet, viewport_width, viewport_height, font);

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
pub fn build_page_with_harness(html: &str, css: &str, viewport_width: f32, viewport_height: f32, font: Option<&SystemFont>) -> (PageResult, Vec<TestResult>) {
    let dom_root = HtmlParser::parse(html);
    let (script_results, test_results) = scripting::execute_inline_scripts_with_harness(&dom_root);

    let mut combined_css = String::new();
    for style_tag in &Node::find_all_by_tag(&dom_root, "style") {
        combined_css.push_str(&Node::text_content(style_tag));
        combined_css.push('\n');
    }
    combined_css.push_str(css);

    let stylesheet = CssParser::parse(&combined_css);
    let layout_root = LayoutTreeBuilder::build(&dom_root, &stylesheet, viewport_width, viewport_height, font);

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
pub fn build_page_keeping_runtime(html: &str, css: &str, viewport_width: f32, viewport_height: f32, font: Option<&SystemFont>) -> (PageResult, JsRuntime) {
    let dom_root = HtmlParser::parse(html);
    let (script_results, runtime) = scripting::execute_inline_scripts_keeping_runtime(&dom_root);

    let mut combined_css = String::new();
    for style_tag in &Node::find_all_by_tag(&dom_root, "style") {
        combined_css.push_str(&Node::text_content(style_tag));
        combined_css.push('\n');
    }
    combined_css.push_str(css);

    let stylesheet = CssParser::parse(&combined_css);
    let layout_root = LayoutTreeBuilder::build(&dom_root, &stylesheet, viewport_width, viewport_height, font);

    (PageResult { dom_root, stylesheet, layout_root, script_results }, runtime)
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
        );

        assert_eq!(Node::find_all_by_tag(&page.dom_root, "h1").len(), 1);
        assert!(page.script_results.is_empty(), "esta pagina no tiene <script>, no deberia haber resultados");
        assert_eq!(page.layout_root.dimensions.width, 800.0);
    }

    #[test]
    fn build_page_executes_inline_scripts_and_reports_their_results() {
        let page = build_page("<html><body><script>1 + 2</script></body></html>", "", 800.0, 600.0, None);
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
        );
        let styled = find_box_with_style(&page.layout_root, "background-color")
            .expect("el <style> de la pagina deberia haber aplicado background-color");
        assert_eq!(styled.computed_style.get("background-color").map(String::as_str), Some("#dbe9f4"));
    }

    #[test]
    fn build_page_merges_page_style_tags_with_the_injected_css_parameter() {
        let page = build_page(
            "<html><head><style>body { background-color: #dbe9f4; }</style></head><body><h1>titulo</h1></body></html>",
            "h1 { color: #ff0000; }",
            800.0,
            600.0,
            None,
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
        let page = build_page("<html><body><p>hola</p></body></html>", "", 800.0, 600.0, None);
        assert_eq!(page.layout_root.dimensions.width, 800.0);

        let relaid_out = LayoutTreeBuilder::build(&page.dom_root, &page.stylesheet, 400.0, 300.0, None);
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
        let page = build_page("<html><body><script>typeof test === 'undefined'</script></body></html>", "", 800.0, 600.0, None);
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
        let new_layout = LayoutTreeBuilder::build(&page.dom_root, &page.stylesheet, 800.0, 600.0, None);
        let new_output_box = find_box_for_dom_node(&new_layout, &output_node).expect("output deberia seguir teniendo caja tras reconstruir el layout");
        let shows_updated_text = new_output_box.children.iter().any(|c| matches!(&c.box_type, engine_layout::BoxType::Text(text) if text == "disparado"));
        assert!(shows_updated_text, "el layout reconstruido deberia pintar el texto ya mutado, no el 'antes' original");
    }
}
