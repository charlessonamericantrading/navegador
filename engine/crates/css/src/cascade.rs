//! Cascada CSS minima: dado un nodo del DOM y una hoja de estilos, resuelve
//! que declaraciones le aplican de verdad - reglas que matchean (via
//! `SelectorMatcher`, combinadores/compuestos incluidos) ordenadas por
//! especificidad ascendente y fusionadas, mas el atributo `style="..."` del
//! propio elemento al final, sin importar su especificidad (asi es el spec
//! real: un estilo en linea gana a cualquier selector posible, `!important`
//! aparte, que no esta modelado aqui) - sin cadena de herencia ni el resto
//! de reglas del spec (orden de aparicion como desempate).
//!
//! Origen SI se modela, aunque solo para dos: la hoja de agente de usuario
//! (`user_agent_stylesheet`) se resuelve PRIMERO y sus declaraciones se
//! sobrescriben con cualquier regla del autor que matchee la misma
//! propiedad, SIN comparar especificidad entre ambas - asi es el orden real
//! de origenes del cascade spec (user-agent normal pierde frente a author
//! normal SIEMPRE, incluso si la regla de agente de usuario tuviera mayor
//! especificidad nominal). Dentro de cada origen por separado, la
//! especificidad si desempata como es debido.
//!
//! Vivia originalmente como funcion privada dentro de `engine-layout::tree`
//! (`LayoutTreeBuilder::resolve_style`); se traslado aqui, publica, SIN
//! cambiar su logica ni una linea, porque no necesita nada de geometria o
//! layout - es pura resolucion CSS - y porque `engine-js` tambien la
//! necesita (para `getComputedStyle`, en construccion) sin tener que
//! depender de `layout` solo para esto. `engine-js` ya dependia de
//! `engine-css` desde antes (`CssParser::parse_inline_style`, usado por
//! `element.style` desde la tarea de esa funcionalidad), asi que este
//! traslado no añade ninguna dependencia nueva en ningun Cargo.toml.
//! `layout::tree::build_node` sigue siendo el unico sitio que la llama
//! durante la construccion normal del layout (recorrido top-down de todo
//! el arbol); `getComputedStyle` la llamara de otra forma, caminando desde
//! la raiz hasta UN nodo cualquiera (sin recorrer el resto del arbol),
//! reusando esta misma funcion para resolver cada ancestro por el camino.

use crate::parser::CssParser;
use crate::selector::SelectorMatcher;
use crate::stylesheet::{Rule, StyleSheet};
use crate::user_agent_stylesheet::user_agent_stylesheet;
use engine_dom::{Node, NodeType};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Aplica las reglas de UN origen (agente de usuario o autor, segun quien
/// llame) que matcheen `dom_node`, ordenadas por especificidad ascendente,
/// escribiendo/sobrescribiendo directamente en `computed` - separado de
/// `resolve_style` para poder invocarse dos veces con el mismo criterio de
/// orden interno (especificidad) pero en dos momentos distintos (UA antes,
/// autor despues), sin duplicar la logica de matching+orden entre ambas.
fn apply_matching_rules(
    computed: &mut HashMap<String, String>,
    stylesheet: &StyleSheet,
    dom_node: &Arc<RwLock<Node>>,
    tag_name: &str,
    id_attr: Option<&str>,
    classes: &[&str],
    viewport_width: f32,
) {
    let mut matched = if let Some(index) = stylesheet.index_if_current() {
        let mut candidate_indices = Vec::new();
        index.collect_candidates(tag_name, id_attr, classes, &mut candidate_indices);
        let candidates: Vec<&Rule> = candidate_indices
            .iter()
            .map(|&idx| &stylesheet.rules[idx])
            .filter(|rule| rule.media.as_ref().is_none_or(|m| m.matches(viewport_width)))
            .collect();
        let mut matched = SelectorMatcher::filter_matching_rules(&candidates, dom_node);
        matched.sort_by_key(|rule| rule.specificity);
        matched
    } else {
        // Respaldo conservador para hojas construidas a mano sin `build_index()`.
        let candidates: Vec<&Rule> = stylesheet
            .rules
            .iter()
            .filter(|rule| rule.media.as_ref().is_none_or(|m| m.matches(viewport_width)))
            .filter(|rule| rule.key.could_match(tag_name, id_attr, classes))
            .collect();
        let mut matched = SelectorMatcher::filter_matching_rules(&candidates, dom_node);
        matched.sort_by_key(|rule| SelectorMatcher::calculate_specificity(&rule.selector));
        matched
    };

    if matched.is_empty() {
        return;
    }

    // DOS pasadas (Fase 22): primero las declaraciones normales por
    // especificidad, y luego las `!important` - tambien por especificidad
    // entre ellas. Asi una declaracion importante gana a CUALQUIER normal
    // sin importar su selector, que es justo lo que `!important` significa
    // y lo unico que no se puede expresar con un solo orden.
    let mut any_important = false;
    for rule in &mut matched {
        if !rule.important.is_empty() {
            any_important = true;
        }
        for (prop, val) in &rule.declarations {
            if !rule.important.contains(prop) {
                computed.insert(prop.clone(), val.clone());
            }
        }
    }
    if any_important {
        for rule in &matched {
            if rule.important.is_empty() {
                continue;
            }
            for (prop, val) in &rule.declarations {
                if rule.important.contains(prop) {
                    computed.insert(prop.clone(), val.clone());
                }
            }
        }
    }
}

/// `viewport_width` decide que bloques `@media` aplican (Fase 18). Lo
/// pasa `engine-layout`, que es quien conoce el tamaño real de la ventana;
/// como se evalua aqui y no al parsear, redimensionar reevalua las
/// consultas sin necesidad de volver a parsear la hoja.
pub fn resolve_style(dom_node: &Arc<RwLock<Node>>, stylesheet: &StyleSheet, viewport_width: f32) -> HashMap<String, String> {
    let (is_element, tag_name, id_attr, class_attr, inline_style) = {
        let n = dom_node.read().unwrap();
        match &n.node_type {
            NodeType::Element { tag_name, attributes } => (
                true,
                if tag_name.bytes().any(|b| b.is_ascii_uppercase()) {
                    tag_name.to_ascii_lowercase()
                } else {
                    tag_name.clone()
                },
                attributes.get("id").cloned(),
                attributes.get("class").cloned(),
                attributes.get("style").cloned(),
            ),
            _ => (false, String::new(), None, None, None),
        }
    };
    if !is_element {
        return HashMap::new();
    }
    let classes: Vec<&str> = class_attr.as_deref().map(|c| c.split_whitespace().collect()).unwrap_or_default();

    let mut computed = HashMap::new();

    // Origen 1: agente de usuario - SIEMPRE pierde frente a lo que venga
    // despues, sin importar especificidad (ver doc-comment del modulo).
    apply_matching_rules(&mut computed, user_agent_stylesheet(), dom_node, &tag_name, id_attr.as_deref(), &classes, viewport_width);

    // Atributos de presentacion (`bgcolor`, `width`, `cellpadding`...): CSS de
    // especificidad cero que cae entre el agente de usuario y el autor (tarea
    // 2.8). Cualquier regla de la pagina los vence.
    for (property, value) in crate::presentational::presentational_hints(dom_node) {
        computed.insert(property, value);
    }

    // Origen 2: autor de la pagina - sobrescribe cualquier declaracion de
    // agente de usuario para la misma propiedad.
    apply_matching_rules(&mut computed, stylesheet, dom_node, &tag_name, id_attr.as_deref(), &classes, viewport_width);

    if let Some(style_attr) = inline_style {
        for (prop, val) in CssParser::parse_inline_style(&style_attr) {
            computed.insert(prop, val);
        }
    }

    merge_border_longhands(&mut computed);
    computed
}

const BORDER_STYLES: &[&str] = &["none", "hidden", "solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset"];

/// Si un token es un ancho de borde (`1px`, `0`, `thin`...) y no un color o un estilo.
fn is_border_width(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    matches!(lower.as_str(), "thin" | "medium" | "thick") || lower.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.')
}

/// `border-width`, `border-style` y `border-color` por separado se funden en el
/// `border` que entienden el layout y el pintado (que solo leen esa propiedad).
/// Es lo habitual en CSS real: `border: 1px solid #a2a9b1; border-color:
/// transparent` (los botones "silenciosos" de Wikipedia) o `border-style: none`.
/// Sin esto el motor dibujaba el borde del shorthand y se saltaba el resto.
///
/// Los `border-*` por lado (`border-bottom`...) siguen sin soportarse. Un estilo
/// distinto de `solid` (`dashed`, `dotted`) se dibuja como solido.
fn merge_border_longhands(computed: &mut HashMap<String, String>) {
    let (width_longhand, style_longhand, color_longhand) = (computed.get("border-width").cloned(), computed.get("border-style").cloned(), computed.get("border-color").cloned());
    if width_longhand.is_none() && style_longhand.is_none() && color_longhand.is_none() {
        return;
    }
    let (mut width, mut style, mut color): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    if let Some(shorthand) = computed.get("border") {
        for token in crate::parser::split_css_tokens(shorthand) {
            let lower = token.to_ascii_lowercase();
            if BORDER_STYLES.contains(&lower.as_str()) {
                style = Some(lower);
            } else if is_border_width(token) {
                width = Some(token.to_string());
            } else {
                color = Some(token.to_string());
            }
        }
    }
    let first = |value: &str| crate::parser::split_css_tokens(value).into_iter().next().map(str::to_string);
    if let Some(value) = width_longhand.as_deref().and_then(first) {
        width = Some(value);
    }
    if let Some(value) = style_longhand.as_deref().and_then(first) {
        style = Some(value.to_ascii_lowercase());
    }
    if let Some(value) = color_longhand.as_deref().and_then(first) {
        color = Some(value);
    }
    if !style.as_deref().is_some_and(|s| BORDER_STYLES.contains(&s) && s != "none" && s != "hidden") {
        computed.remove("border");
        return;
    }
    let width = match width.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("thin") => "1px".to_string(),
        Some("medium") | None => "3px".to_string(),
        Some("thick") => "5px".to_string(),
        Some(_) => width.unwrap_or_default(),
    };
    let mut value = format!("{width} solid");
    if let Some(color) = color {
        value.push(' ');
        value.push_str(&color);
    }
    computed.insert("border".to_string(), value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::CssParser as Parser;
    use engine_dom::HtmlParser;

    /// Localiza el primer `Element` del arbol cuyo tag coincida - suficiente
    /// para estas pruebas de aislamiento, que solo necesitan UN nodo
    /// concreto, no la busqueda por id que exige el DOM completo montado
    /// (estas pruebas no pasan por `layout`, asi que no hay `LayoutBox` que
    /// consultar - se opera directo sobre el DOM).
    fn find_first_element(root: &Arc<RwLock<Node>>, tag: &str) -> Arc<RwLock<Node>> {
        Node::find_all_by_tag(root, tag).into_iter().next().expect("deberia existir el elemento buscado")
    }

    #[test]
    fn resolve_style_applies_a_matching_rule() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = Parser::parse("p { color: #ff0000; }");
        let p = find_first_element(&dom, "p");

        let computed = resolve_style(&p, &stylesheet, 1280.0);
        assert_eq!(computed.get("color").map(String::as_str), Some("#ff0000"));
    }

    fn style_of(html: &str, tag: &str, css: &str) -> HashMap<String, String> {
        let dom = HtmlParser::parse(html);
        let stylesheet = Parser::parse(css);
        resolve_style(&find_first_element(&dom, tag), &stylesheet, 1280.0)
    }

    /// Tarea 2.8: los atributos HTML de presentacion cuentan como CSS.
    #[test]
    fn presentational_attributes_become_css() {
        let table = style_of(
            r##"<html><body><table width="85%" bgcolor="#f6f6ef" border="2" cellspacing="3"><tr><td>x</td></tr></table></body></html>"##,
            "table",
            "",
        );
        assert_eq!(table.get("width").map(String::as_str), Some("85%"));
        assert_eq!(table.get("background-color").map(String::as_str), Some("#f6f6ef"));
        assert_eq!(table.get("border").map(String::as_str), Some("2px solid #808080"));
        assert_eq!(table.get("border-spacing").map(String::as_str), Some("3px"));

        let td = style_of(r##"<html><body><table cellpadding="5" border="1"><tr><td bgcolor="ff6600" width="40">x</td></tr></table></body></html>"##, "td", "");
        assert_eq!(td.get("background-color").map(String::as_str), Some("#ff6600"), "bgcolor sin almohadilla");
        assert_eq!(td.get("width").map(String::as_str), Some("40px"), "un numero suelto son pixeles");
        assert_eq!(td.get("padding-left").map(String::as_str), Some("5px"), "cellpadding de la tabla, en cada lado");
        assert_eq!(td.get("padding-top").map(String::as_str), Some("5px"));
        assert_eq!(td.get("border").map(String::as_str), Some("1px solid #808080"), "las celdas heredan el borde de la tabla");
    }

    /// El CSS de la pagina siempre gana a un atributo de presentacion.
    #[test]
    fn author_css_beats_presentational_attributes() {
        let table = style_of(
            r##"<html><body><table width="85%" bgcolor="#f6f6ef"><tr><td>x</td></tr></table></body></html>"##,
            "table",
            "table { width: 300px; background-color: #000000; }",
        );
        assert_eq!(table.get("width").map(String::as_str), Some("300px"));
        assert_eq!(table.get("background-color").map(String::as_str), Some("#000000"));
    }

    #[test]
    fn presentational_attributes_beat_the_user_agent_sheet() {
        let td = style_of(r##"<html><body><table cellpadding="0"><tr><td>x</td></tr></table></body></html>"##, "td", "");
        assert_eq!(td.get("padding-left").map(String::as_str), Some("0px"), "cellpadding=0 gana a la regla de padding de td del agente de usuario");
    }

    #[test]
    fn align_and_center_and_font_attributes() {
        let p = style_of(r##"<html><body><p align="center">x</p></body></html>"##, "p", "");
        assert_eq!(p.get("text-align").map(String::as_str), Some("center"));

        let table = style_of(r##"<html><body><center><table><tr><td>x</td></tr></table></center></body></html>"##, "table", "");
        assert_eq!(table.get("margin-left").map(String::as_str), Some("auto"), "una tabla dentro de <center> se centra");
        assert_eq!(table.get("margin-right").map(String::as_str), Some("auto"));

        let font = style_of(r##"<html><body><font color="#828282" size="1">x</font></body></html>"##, "font", "");
        assert_eq!(font.get("color").map(String::as_str), Some("#828282"));
        assert_eq!(font.get("font-size").map(String::as_str), Some("10px"));
    }

    #[test]
    fn nonsense_attribute_values_are_ignored() {
        let table = style_of(r##"<html><body><table width="abc" bgcolor="zz!!" border="x"><tr><td>x</td></tr></table></body></html>"##, "table", "");
        assert_eq!(table.get("width"), None);
        assert_eq!(table.get("background-color"), None);
        assert_eq!(table.get("border"), None);
    }

    /// `border-color: transparent` y compañia se funden con el shorthand: el
    /// layout y el pintado solo leen `border`.
    #[test]
    fn border_longhands_are_merged_into_the_border_the_engine_reads() {
        let border = |css: &str| style_of("<html><body><a>x</a></body></html>", "a", css).get("border").cloned();

        assert_eq!(border("a { border: 1px solid #a2a9b1; border-color: transparent; }").as_deref(), Some("1px solid transparent"), "el color de la longhand gana");
        assert_eq!(border("a { border: 1px solid red; border-style: none; }"), None, "border-style: none anula el borde");
        assert_eq!(border("a { border: 1px solid red; border-width: 0; }").as_deref(), Some("0 solid red"), "ancho 0 se conserva (no ocupa nada)");
        assert_eq!(border("a { border: 1px solid red; border-width: 4px; }").as_deref(), Some("4px solid red"));
        assert_eq!(border("a { border-width: 2px; border-style: solid; border-color: rgba(0, 0, 0, .5); }").as_deref(), Some("2px solid rgba(0, 0, 0, .5)"), "solo con longhands");
        assert_eq!(border("a { border-width: 2px; border-color: red; }"), None, "sin estilo no hay borde (el valor inicial es none)");
        assert_eq!(border("a { border-style: dashed; }").as_deref(), Some("3px solid"), "ancho medium por defecto; dashed se dibuja solido");
        assert_eq!(border("a { border: thin solid blue; }").as_deref(), Some("thin solid blue"), "sin longhands no se toca el shorthand");
    }

    #[test]
    fn resolve_style_ignores_non_matching_rules() {
        // `<span>` a proposito, no `<p>`: `<p>` SI tiene una regla propia en
        // la hoja de agente de usuario (`margin`), asi que usar `<p>` aqui
        // haria que el resultado no estuviera vacio por una razon ajena a
        // lo que este test comprueba.
        let dom = HtmlParser::parse("<html><body><span>hola</span></body></html>");
        let stylesheet = Parser::parse("h1 { color: #ff0000; }");
        let span = find_first_element(&dom, "span");

        assert!(resolve_style(&span, &stylesheet, 1280.0).is_empty(), "una regla para h1 no deberia aplicarse a un <span>, ni tiene la hoja de agente de usuario nada para el");
    }

    #[test]
    fn resolve_style_lets_higher_specificity_win() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main">hola</p></body></html>"#);
        let stylesheet = Parser::parse("#main { color: #00ff00; } p { color: #ff0000; }");
        let p = find_first_element(&dom, "p");

        let computed = resolve_style(&p, &stylesheet, 1280.0);
        assert_eq!(computed.get("color").map(String::as_str), Some("#00ff00"), "el selector de mayor especificidad (#main) deberia ganar pese a aparecer antes en la hoja");
    }

    #[test]
    fn resolve_style_lets_inline_style_win_over_every_stylesheet_rule() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main" style="color: #0000ff">hola</p></body></html>"#);
        let stylesheet = Parser::parse("#main { color: #00ff00; }");
        let p = find_first_element(&dom, "p");

        let computed = resolve_style(&p, &stylesheet, 1280.0);
        assert_eq!(computed.get("color").map(String::as_str), Some("#0000ff"), "el atributo style en linea deberia ganar incluso sobre un selector de id");
    }

    /// El punto real de esta tarea: un `<h1>` SIN ningun CSS de autor
    /// deberia seguir viendose con estructura (tamaño de titular, margen),
    /// tal como pasa en cualquier navegador real - antes de esto, sin CSS
    /// de la pagina, todo se veia como texto plano de 16px sin margenes.
    #[test]
    fn resolve_style_applies_the_user_agent_default_when_the_author_has_no_rule_for_it() {
        let dom = HtmlParser::parse("<html><body><h1>titulo</h1></body></html>");
        let stylesheet = Parser::parse("");
        let h1 = find_first_element(&dom, "h1");

        let computed = resolve_style(&h1, &stylesheet, 1280.0);
        assert_eq!(computed.get("font-size").map(String::as_str), Some("32px"), "un <h1> sin CSS de autor deberia seguir teniendo el tamaño por defecto de un titular");
        assert_eq!(computed.get("margin").map(String::as_str), Some("21px"));
    }

    /// La razon real de que el origen se modele aparte de la especificidad:
    /// una regla de autor con la MISMA especificidad nominal que la regla
    /// de agente de usuario (ambas son un simple selector de tag) debe
    /// ganar igualmente - el origen desempata ANTES que la especificidad,
    /// no despues. Si se hubiera implementado mezclando ambos origenes en
    /// una sola lista ordenada solo por especificidad, el resultado
    /// dependeria del orden de insercion en vez de ser siempre "autor gana".
    #[test]
    fn author_rule_overrides_user_agent_default_even_at_equal_specificity() {
        let dom = HtmlParser::parse("<html><body><h1>titulo</h1></body></html>");
        let stylesheet = Parser::parse("h1 { font-size: 10px; }");
        let h1 = find_first_element(&dom, "h1");

        let computed = resolve_style(&h1, &stylesheet, 1280.0);
        assert_eq!(computed.get("font-size").map(String::as_str), Some("10px"), "la regla del autor deberia ganar a la de agente de usuario pese a tener la misma especificidad nominal");
    }

    /// El punto de la Fase 22: `!important` gana a una regla de MAYOR
    /// especificidad, que es lo unico que no se puede expresar con un
    /// solo orden de aplicacion.
    #[test]
    fn an_important_declaration_beats_a_more_specific_normal_one() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main">hola</p></body></html>"#);
        let stylesheet = Parser::parse("#main { color: #00ff00; } p { color: #ff0000 !important; }");
        let p = find_first_element(&dom, "p");

        let computed = resolve_style(&p, &stylesheet, 1280.0);
        assert_eq!(
            computed.get("color").map(String::as_str),
            Some("#ff0000"),
            "el !important de `p` deberia ganar al `#main` normal, pese a tener MENOS especificidad"
        );
    }

    /// Y el valor llega LIMPIO, sin el sufijo pegado - que era el bug de
    /// fondo: antes se guardaba `"#ff0000 !important"`, ganaba la cascada
    /// y luego no parseaba como color, asi que anulaba a la regla que
    /// habria ganado sin el.
    #[test]
    fn the_important_suffix_is_stripped_from_the_value() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = Parser::parse("p { color: #ff0000 !important; }");
        let p = find_first_element(&dom, "p");

        assert_eq!(resolve_style(&p, &stylesheet, 1280.0).get("color").map(String::as_str), Some("#ff0000"));
    }

    /// Entre DOS declaraciones importantes vuelve a mandar la
    /// especificidad, como en el spec.
    #[test]
    fn specificity_still_decides_between_two_important_declarations() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main">hola</p></body></html>"#);
        let stylesheet = Parser::parse("p { color: #ff0000 !important; } #main { color: #00ff00 !important; }");
        let p = find_first_element(&dom, "p");

        assert_eq!(resolve_style(&p, &stylesheet, 1280.0).get("color").map(String::as_str), Some("#00ff00"));
    }

    /// Sin `!important`, la especificidad manda como siempre - fija que la
    /// segunda pasada no altera el comportamiento normal.
    #[test]
    fn normal_declarations_are_unaffected_by_the_second_pass() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main">hola</p></body></html>"#);
        let stylesheet = Parser::parse("p { color: #ff0000; } #main { color: #00ff00; }");
        let p = find_first_element(&dom, "p");

        assert_eq!(resolve_style(&p, &stylesheet, 1280.0).get("color").map(String::as_str), Some("#00ff00"));
    }

    /// Un shorthand importante hace importante tambien al longhand que se
    /// deriva de el: `background: red !important` debe ganar igual.
    #[test]
    fn an_important_shorthand_makes_its_expanded_longhand_important_too() {
        let dom = HtmlParser::parse(r#"<html><body><div id="caja">x</div></body></html>"#);
        let stylesheet = Parser::parse("#caja { background-color: #00ff00; } div { background: #ff0000 !important; }");
        let div = find_first_element(&dom, "div");

        assert_eq!(
            resolve_style(&div, &stylesheet, 1280.0).get("background-color").map(String::as_str),
            Some("#ff0000"),
            "la importancia del shorthand deberia heredarla el longhand derivado"
        );
    }

    /// La razon real de expandir `background` a `background-color` en el
    /// PARSER (`insert_declaration`) en vez de al pintar: aqui, la
    /// cascada nunca sabe que hubo un shorthand de por medio, asi que una
    /// regla de mayor especificidad que solo declara el longhand gana
    /// exactamente igual que con cualquier otro par de reglas normales -
    /// cero logica especial en `apply_matching_rules`.
    #[test]
    fn a_higher_specificity_background_color_rule_wins_over_a_lower_specificity_background_shorthand() {
        let dom = HtmlParser::parse(r#"<html><body><div id="caja">contenido</div></body></html>"#);
        let stylesheet = Parser::parse("div { background: #ff0000; } #caja { background-color: #00ff00; }");
        let div = find_first_element(&dom, "div");

        let computed = resolve_style(&div, &stylesheet, 1280.0);
        assert_eq!(
            computed.get("background-color").map(String::as_str),
            Some("#00ff00"),
            "el longhand mas especifico deberia ganar sobre el shorthand menos especifico, aunque sean propiedades 'distintas' a nivel de string"
        );
    }

    #[test]
    fn indexed_cascade_preserves_document_order_without_duplicates_for_selector_lists() {
        let dom = HtmlParser::parse(r#"<html><body><a id="cta" class="btn primary" href="/go">ir</a></body></html>"#);
        let stylesheet = Parser::parse(
            "a, .btn, #cta { color: #111111; } \
             a[href]:not(.disabled) { background-color: #222222; } \
             a.primary, .btn { color: #333333; } \
             a::before { color: #ff0000; }",
        );
        let a = find_first_element(&dom, "a");
        let computed = resolve_style(&a, &stylesheet, 1280.0);
        // `#cta` en la primera regla tiene especificidad de ID (1,0,0) > (0,2,0) de la tercera,
        // pero si hubiese empate debe ganar la posterior en documento; aqui `#cta` gana por ID.
        assert_eq!(computed.get("color").map(String::as_str), Some("#111111"));
        assert_eq!(computed.get("background-color").map(String::as_str), Some("#222222"));

        // Empate de especificidad entre dos reglas que entran por ramas distintas (`a` y `.btn`):
        let empate = Parser::parse("a, #otro { color: #111111; } .btn, #ninguno { color: #444444; }");
        let comp2 = resolve_style(&a, &empate, 1280.0);
        assert_eq!(comp2.get("color").map(String::as_str), Some("#444444"));
    }

    #[test]
    fn dynamic_hover_and_focus_pseudo_classes_follow_node_interaction_state() {
        let dom = HtmlParser::parse(r#"<html><body><div id="wrap"><a id="link" href="/x">ir</a><input id="txt" type="text"></div></body></html>"#);
        let stylesheet = Parser::parse(
            "a { color: #111111; } a:hover { color: #222222; } \
             #wrap:hover { background-color: #fafafa; } \
             input:focus { border: 2px solid #1a73e8; } \
             #wrap:focus-within { background-color: #eef5ff; }",
        );
        assert!(stylesheet.has_hover_rules());
        assert!(stylesheet.has_focus_rules());

        let wrap = Node::find_by_id(&dom, "wrap").unwrap();
        let link = Node::find_by_id(&dom, "link").unwrap();
        let txt = Node::find_by_id(&dom, "txt").unwrap();

        assert_eq!(resolve_style(&link, &stylesheet, 1280.0).get("color").map(String::as_str), Some("#111111"));
        assert_eq!(resolve_style(&wrap, &stylesheet, 1280.0).get("background-color"), None);

        for ancestor in Node::ancestors(&link) {
            ancestor.write().unwrap().hovered = true;
        }
        assert_eq!(resolve_style(&link, &stylesheet, 1280.0).get("color").map(String::as_str), Some("#222222"));
        assert_eq!(resolve_style(&wrap, &stylesheet, 1280.0).get("background-color").map(String::as_str), Some("#fafafa"));

        txt.write().unwrap().focused = true;
        assert_eq!(resolve_style(&txt, &stylesheet, 1280.0).get("border").map(String::as_str), Some("2px solid #1a73e8"));
        assert_eq!(resolve_style(&wrap, &stylesheet, 1280.0).get("background-color").map(String::as_str), Some("#eef5ff"));
    }
}
