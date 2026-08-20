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
fn apply_matching_rules(computed: &mut HashMap<String, String>, stylesheet: &StyleSheet, dom_node: &Arc<RwLock<Node>>, viewport_width: f32) {
    let mut matched: Vec<&Rule> = stylesheet
        .rules
        .iter()
        // `@media` (Fase 18): una regla que venia dentro de un bloque solo
        // participa si su condicion se cumple con el viewport ACTUAL. Se
        // comprueba aqui, en cada resolucion de estilo, y no al parsear,
        // precisamente para que redimensionar la ventana vuelva a
        // evaluarla - la hoja se parsea una sola vez por pagina, pero
        // `resolve_style` corre en cada relayout.
        .filter(|rule| rule.media.as_ref().is_none_or(|m| m.matches(viewport_width)))
        .filter(|rule| SelectorMatcher::matches(&rule.selector, dom_node))
        .collect();
    matched.sort_by_key(|rule| SelectorMatcher::calculate_specificity(&rule.selector));

    // DOS pasadas (Fase 22): primero las declaraciones normales por
    // especificidad, y luego las `!important` - tambien por especificidad
    // entre ellas. Asi una declaracion importante gana a CUALQUIER normal
    // sin importar su selector, que es justo lo que `!important` significa
    // y lo unico que no se puede expresar con un solo orden.
    for rule in &matched {
        for (prop, val) in &rule.declarations {
            if !rule.important.contains(prop) {
                computed.insert(prop.clone(), val.clone());
            }
        }
    }
    for rule in &matched {
        for (prop, val) in &rule.declarations {
            if rule.important.contains(prop) {
                computed.insert(prop.clone(), val.clone());
            }
        }
    }
}

/// `viewport_width` decide que bloques `@media` aplican (Fase 18). Lo
/// pasa `engine-layout`, que es quien conoce el tamaño real de la ventana;
/// como se evalua aqui y no al parsear, redimensionar reevalua las
/// consultas sin necesidad de volver a parsear la hoja.
pub fn resolve_style(dom_node: &Arc<RwLock<Node>>, stylesheet: &StyleSheet, viewport_width: f32) -> HashMap<String, String> {
    let mut computed = HashMap::new();

    // Origen 1: agente de usuario - SIEMPRE pierde frente a lo que venga
    // despues, sin importar especificidad (ver doc-comment del modulo).
    apply_matching_rules(&mut computed, user_agent_stylesheet(), dom_node, viewport_width);

    // Origen 2: autor de la pagina - sobrescribe cualquier declaracion de
    // agente de usuario para la misma propiedad.
    apply_matching_rules(&mut computed, stylesheet, dom_node, viewport_width);

    let inline_style = {
        let n = dom_node.read().unwrap();
        match &n.node_type {
            NodeType::Element { attributes, .. } => attributes.get("style").cloned(),
            _ => None,
        }
    };
    if let Some(style_attr) = inline_style {
        for (prop, val) in CssParser::parse_inline_style(&style_attr) {
            computed.insert(prop, val);
        }
    }

    computed
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
}
