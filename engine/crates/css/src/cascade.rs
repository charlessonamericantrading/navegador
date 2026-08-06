//! Cascada CSS minima: dado un nodo del DOM y una hoja de estilos, resuelve
//! que declaraciones le aplican de verdad - reglas que matchean (via
//! `SelectorMatcher`, combinadores/compuestos incluidos) ordenadas por
//! especificidad ascendente y fusionadas, mas el atributo `style="..."` del
//! propio elemento al final, sin importar su especificidad (asi es el spec
//! real: un estilo en linea gana a cualquier selector posible, `!important`
//! aparte, que no esta modelado aqui) - sin cadena de herencia ni el resto
//! de reglas del spec (orden de aparicion como desempate, origen del
//! stylesheet).
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
use engine_dom::{Node, NodeType};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub fn resolve_style(dom_node: &Arc<RwLock<Node>>, stylesheet: &StyleSheet) -> HashMap<String, String> {
    let mut matched: Vec<&Rule> = stylesheet
        .rules
        .iter()
        .filter(|rule| SelectorMatcher::matches(&rule.selector, dom_node))
        .collect();
    matched.sort_by_key(|rule| SelectorMatcher::calculate_specificity(&rule.selector));

    let mut computed = HashMap::new();
    for rule in matched {
        for (prop, val) in &rule.declarations {
            computed.insert(prop.clone(), val.clone());
        }
    }

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

        let computed = resolve_style(&p, &stylesheet);
        assert_eq!(computed.get("color").map(String::as_str), Some("#ff0000"));
    }

    #[test]
    fn resolve_style_ignores_non_matching_rules() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = Parser::parse("h1 { color: #ff0000; }");
        let p = find_first_element(&dom, "p");

        assert!(resolve_style(&p, &stylesheet).is_empty(), "una regla para h1 no deberia aplicarse a un <p>");
    }

    #[test]
    fn resolve_style_lets_higher_specificity_win() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main">hola</p></body></html>"#);
        let stylesheet = Parser::parse("#main { color: #00ff00; } p { color: #ff0000; }");
        let p = find_first_element(&dom, "p");

        let computed = resolve_style(&p, &stylesheet);
        assert_eq!(computed.get("color").map(String::as_str), Some("#00ff00"), "el selector de mayor especificidad (#main) deberia ganar pese a aparecer antes en la hoja");
    }

    #[test]
    fn resolve_style_lets_inline_style_win_over_every_stylesheet_rule() {
        let dom = HtmlParser::parse(r#"<html><body><p id="main" style="color: #0000ff">hola</p></body></html>"#);
        let stylesheet = Parser::parse("#main { color: #00ff00; }");
        let p = find_first_element(&dom, "p");

        let computed = resolve_style(&p, &stylesheet);
        assert_eq!(computed.get("color").map(String::as_str), Some("#0000ff"), "el atributo style en linea deberia ganar incluso sobre un selector de id");
    }
}
