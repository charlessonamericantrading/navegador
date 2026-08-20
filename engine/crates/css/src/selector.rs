//! Matching de selectores real via el crate `selectors` (Servo/Stylo - el
//! mismo que usa Firefox). Sustituye al matcher anterior, que solo entendia
//! un tag/`.clase`/`#id` suelto sin combinadores ni selectores compuestos.
//!
//! La API publica (`SelectorMatcher::matches`, `::calculate_specificity`,
//! `Specificity`) no cambia de forma - lo que cambia es que ahora hace el
//! trabajo de verdad por dentro. `tree.rs` (crate `layout`) no necesita
//! tocarse.
//!
//! El adaptador de DOM (`ElementRef` + `impl Element`) vive en `element.rs`.
//! Este archivo define el "vocabulario de tipos" que `selectors` exige via
//! el trait `SelectorImpl` (que tipo de dato es un id, una clase, un
//! namespace...) y el parser que decide que sintaxis se acepta.

use cssparser::{Parser as CssParser, ParserInput, ToCss};
use engine_dom::{Node, NodeType};
use selectors::context::{MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches};
use selectors::matching::matches_selector_list;
use selectors::parser::{NonTSPseudoClass, ParseRelative, PseudoElement, SelectorImpl, SelectorList};
use std::borrow::Borrow;
use std::sync::{Arc, RwLock};

use crate::element::ElementRef;

/// Cadena de texto propia usada para varios tipos asociados de
/// `SelectorImpl` (id, clase, nombre de etiqueta, prefijo de namespace).
/// `selectors` exige que implementen `ToCss`/`PrecomputedHash`, que `String`
/// no trae de fabrica - envolverla es mas simple que tirar de un crate de
/// interning que no necesitamos con paginas de este tamaño.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CssString(String);

impl CssString {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'a> From<&'a str> for CssString {
    fn from(value: &'a str) -> Self {
        CssString(value.to_string())
    }
}

impl Borrow<str> for CssString {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl ToCss for CssString {
    fn to_css<W: std::fmt::Write>(&self, dest: &mut W) -> std::fmt::Result {
        dest.write_str(&self.0)
    }
}

impl precomputed_hash::PrecomputedHash for CssString {
    fn precomputed_hash(&self) -> u32 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut hasher);
        hasher.finish() as u32
    }
}

/// Marcador de "el unico namespace que existe" (HTML implicito). Sin
/// soporte real de namespaces (ver html5ever_sink.rs) - un tipo con un solo
/// valor posible es la forma honesta de decir "no distinguimos namespaces",
/// no un intento de fingir que si lo hacemos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct NoNamespace;

impl precomputed_hash::PrecomputedHash for NoNamespace {
    fn precomputed_hash(&self) -> u32 {
        0
    }
}

/// Pseudo-clases NO estructurales (Fase 18). Las estructurales
/// (`:first-child`, `:nth-child`, `:not`, `:empty`, `:root`...) NO estan
/// aqui: las resuelve el propio crate `selectors` recorriendo el arbol con
/// los metodos que `ElementRef` ya implementa (`prev_sibling_element`,
/// `is_empty`, `is_root`...), asi que funcionan sin que este enum exista.
///
/// Antes esto era un enum VACIO, lo que hacia que el parser rechazara
/// cualquier pseudo-clase como error de sintaxis - y como una regla con
/// selector invalido se descarta ENTERA, un `input:checked + label` (el
/// "checkbox hack", que es como media web sin JavaScript hace menus
/// desplegables y acordeones) desaparecia por completo.
///
/// Se dividen en dos grupos, y la diferencia importa:
/// - **Derivables del DOM**: `:checked`, `:disabled`/`:enabled`,
///   `:required`/`:optional`, `:read-only`/`:read-write`, `:link`. Se
///   resuelven de verdad leyendo atributos - ver
///   `ElementRef::match_non_ts_pseudo_class`.
/// - **Estados de interaccion**: `:hover`, `:focus`, `:active`,
///   `:focus-visible`, `:focus-within`, `:visited`. Se PARSEAN pero nunca
///   coinciden, porque este motor no recalcula la cascada al mover el raton
///   ni al enfocar. Parsearlas igualmente es lo correcto: asi una regla
///   como `.btn, .btn:hover { color: rojo }` conserva su primera mitad en
///   vez de perderse entera, que es lo que pasaba antes.
///   (`:visited` ademas nunca deberia coincidir por privacidad: los
///   navegadores reales restringen severamente que se puede estilar ahi
///   para no filtrar el historial de navegacion.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnginePseudoClass {
    Checked,
    Disabled,
    Enabled,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    Link,
    AnyLink,
    /// Estados de interaccion - se guarda cual era para poder reescribir el
    /// selector con `to_css` de forma fiel, aunque nunca coincidan.
    Hover,
    Focus,
    FocusVisible,
    FocusWithin,
    Active,
    Visited,
}

impl EnginePseudoClass {
    /// El nombre tal cual se escribe en CSS - usado por `ToCss` (que el
    /// crate necesita para poder reserializar un selector) y por el parser.
    fn name(&self) -> &'static str {
        match self {
            Self::Checked => "checked",
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
            Self::Required => "required",
            Self::Optional => "optional",
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
            Self::Link => "link",
            Self::AnyLink => "any-link",
            Self::Hover => "hover",
            Self::Focus => "focus",
            Self::FocusVisible => "focus-visible",
            Self::FocusWithin => "focus-within",
            Self::Active => "active",
            Self::Visited => "visited",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "checked" => Self::Checked,
            "disabled" => Self::Disabled,
            "enabled" => Self::Enabled,
            "required" => Self::Required,
            "optional" => Self::Optional,
            "read-only" => Self::ReadOnly,
            "read-write" => Self::ReadWrite,
            "link" => Self::Link,
            "any-link" => Self::AnyLink,
            "hover" => Self::Hover,
            "focus" => Self::Focus,
            "focus-visible" => Self::FocusVisible,
            "focus-within" => Self::FocusWithin,
            "active" => Self::Active,
            "visited" => Self::Visited,
            _ => return None,
        })
    }
}

impl ToCss for EnginePseudoClass {
    fn to_css<W: std::fmt::Write>(&self, dest: &mut W) -> std::fmt::Result {
        write!(dest, ":{}", self.name())
    }
}

impl NonTSPseudoClass for EnginePseudoClass {
    type Impl = EngineSelectorImpl;
    fn is_active_or_hover(&self) -> bool {
        matches!(self, Self::Active | Self::Hover)
    }
    fn is_user_action_state(&self) -> bool {
        matches!(self, Self::Active | Self::Hover | Self::Focus | Self::FocusVisible | Self::FocusWithin)
    }
}

/// Igual que `NoPseudoClass` pero para pseudo-elementos (`::before`, `::after`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoPseudoElement {}

impl ToCss for NoPseudoElement {
    fn to_css<W: std::fmt::Write>(&self, _dest: &mut W) -> std::fmt::Result {
        match *self {}
    }
}

impl PseudoElement for NoPseudoElement {
    type Impl = EngineSelectorImpl;
}

#[derive(Debug, Clone)]
pub(crate) struct EngineSelectorImpl;

impl SelectorImpl for EngineSelectorImpl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssString;
    type Identifier = CssString;
    type LocalName = CssString;
    type NamespaceUrl = NoNamespace;
    type NamespacePrefix = CssString;
    type BorrowedNamespaceUrl = NoNamespace;
    type BorrowedLocalName = str;
    type NonTSPseudoClass = EnginePseudoClass;
    type PseudoElement = NoPseudoElement;
}

struct EngineSelectorParser;

impl<'i> selectors::parser::Parser<'i> for EngineSelectorParser {
    type Impl = EngineSelectorImpl;
    type Error = selectors::parser::SelectorParseErrorKind<'i>;

    /// Acepta las pseudo-clases de `EnginePseudoClass` (ver su
    /// doc-comment) y rechaza el resto como error de sintaxis, lo que
    /// descarta esa regla - mismo criterio honesto de siempre: mejor no
    /// aplicar una regla que aplicarla mal.
    fn parse_non_ts_pseudo_class(
        &self,
        location: cssparser::SourceLocation,
        name: cssparser::CowRcStr<'i>,
    ) -> Result<EnginePseudoClass, cssparser::ParseError<'i, Self::Error>> {
        EnginePseudoClass::from_name(&name).ok_or_else(|| {
            location.new_custom_error(selectors::parser::SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name))
        })
    }

    // Lo demas sigue usando el default: pseudo-elementos (`::before`),
    // ::part()/::slotted() y prefijos de namespace se rechazan.
}

/// Especificidad real (spec CSS): el crate la empaqueta en un `u32` que ya
/// ordena igual que la tripleta (ids, clases, tags) - envolverlo en un
/// newtype en vez de desempaquetarlo evita reinventar esa logica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Specificity(u32);

fn parse_selector_list(selector_str: &str) -> Option<SelectorList<EngineSelectorImpl>> {
    let mut input = ParserInput::new(selector_str);
    let mut parser = CssParser::new(&mut input);
    match SelectorList::parse(&EngineSelectorParser, &mut parser, ParseRelative::No) {
        Ok(list) => Some(list),
        Err(err) => {
            tracing::trace!("[selectors] selector ignorado, no soportado o invalido: '{selector_str}' ({err:?})");
            None
        }
    }
}

pub struct SelectorMatcher;

impl SelectorMatcher {
    pub fn matches(selector_str: &str, node: &Arc<RwLock<Node>>) -> bool {
        let Some(list) = parse_selector_list(selector_str) else { return false };
        let element = ElementRef(node.clone());

        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );

        matches_selector_list(&list, &element, &mut context)
    }

    pub fn calculate_specificity(selector_str: &str) -> Specificity {
        let Some(list) = parse_selector_list(selector_str) else { return Specificity::default() };
        list.slice().iter().map(|selector| Specificity(selector.specificity())).max().unwrap_or_default()
    }

    /// Primer elemento en orden de documento (preorden: el nodo antes que
    /// sus hijos), incluida la raiz, que matchea `selector_str` - base de
    /// `document.querySelector`. `None` si el selector es invalido/no
    /// soportado o si nada matchea.
    pub fn query_first(selector_str: &str, root: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
        let list = parse_selector_list(selector_str)?;
        first_match_in_subtree(&list, root)
    }

    /// Todos los elementos en orden de documento (incluida la raiz) que
    /// matchean `selector_str` - base de `document.querySelectorAll`. Vacio
    /// si el selector es invalido/no soportado o si nada matchea.
    pub fn query_all(selector_str: &str, root: &Arc<RwLock<Node>>) -> Vec<Arc<RwLock<Node>>> {
        let Some(list) = parse_selector_list(selector_str) else { return Vec::new() };
        let mut out = Vec::new();
        collect_matches_in_subtree(&list, root, &mut out);
        out
    }
}

/// No se mantiene el read-lock de `node` mientras se llama a
/// `matches_selector_list` (que vuelve a bloquear ese mismo nodo por dentro,
/// via los metodos de `ElementRef` como `has_local_name`/`has_id`/etc.):
/// `RwLock::read` de la biblioteca estandar no garantiza ser reentrante en
/// el mismo hilo y podria bloquearse a si mismo segun la plataforma. Por
/// eso los dos accesos de abajo (`is_element` y `children`) son
/// expresiones temporales que sueltan el guard antes de la siguiente
/// llamada, en vez de un `let n = node.read().unwrap();` que lo mantendria
/// vivo durante toda la funcion.
fn first_match_in_subtree(list: &SelectorList<EngineSelectorImpl>, node: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
    let is_element = matches!(node.read().unwrap().node_type, NodeType::Element { .. });
    if is_element {
        let element = ElementRef(node.clone());
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        if matches_selector_list(list, &element, &mut context) {
            return Some(node.clone());
        }
    }

    let children = node.read().unwrap().children.clone();
    for child in &children {
        if let Some(found) = first_match_in_subtree(list, child) {
            return Some(found);
        }
    }
    None
}

/// Igual que `first_match_in_subtree` pero acumulando todos los matches en
/// vez de devolver el primero - misma nota sobre no mantener el read-lock
/// mientras se llama a `matches_selector_list`.
fn collect_matches_in_subtree(list: &SelectorList<EngineSelectorImpl>, node: &Arc<RwLock<Node>>, out: &mut Vec<Arc<RwLock<Node>>>) {
    let is_element = matches!(node.read().unwrap().node_type, NodeType::Element { .. });
    if is_element {
        let element = ElementRef(node.clone());
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        if matches_selector_list(list, &element, &mut context) {
            out.push(node.clone());
        }
    }

    let children = node.read().unwrap().children.clone();
    for child in &children {
        collect_matches_in_subtree(list, child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_dom::NodeType;
    use std::collections::HashMap;

    fn element(tag: &str, attrs: &[(&str, &str)]) -> Arc<RwLock<Node>> {
        let attributes = attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        Node::new(NodeType::Element { tag_name: tag.to_string(), attributes })
    }

    /// El matcher anterior solo comparaba la cadena completa del selector
    /// contra UN nodo, sin mirar ancestros - "div p" nunca podia distinguir
    /// un <p> dentro de un <div> de un <p> suelto.
    #[test]
    fn descendant_combinator_requires_an_ancestor_not_just_the_tag() {
        let div = element("div", &[]);
        let p_inside = element("p", &[]);
        Node::append_child(&div, p_inside.clone());
        let p_outside = element("p", &[]);

        assert!(SelectorMatcher::matches("div p", &p_inside));
        assert!(!SelectorMatcher::matches("div p", &p_outside));
    }

    #[test]
    fn child_combinator_only_matches_direct_children() {
        let div = element("div", &[]);
        let span = element("span", &[]);
        let p = element("p", &[]);
        Node::append_child(&span, p.clone());
        Node::append_child(&div, span.clone());

        assert!(!SelectorMatcher::matches("div > p", &p), "p es nieto de div, no hijo directo");
        assert!(SelectorMatcher::matches("div > span", &span));
    }

    /// El matcher anterior trataba "div.foo" como una sola cadena opaca: no
    /// coincidia con la comparacion `trimmed == tag_name` (no es "div.foo" el
    /// tag) ni con la de clase (no empieza por '.'), asi que nunca matcheaba
    /// nada en absoluto.
    #[test]
    fn compound_selector_requires_both_tag_and_class_on_the_same_element() {
        let div_bar = element("div", &[("class", "bar")]);
        let div_foo = element("div", &[("class", "foo")]);
        let span_foo = element("span", &[("class", "foo")]);

        assert!(SelectorMatcher::matches("div.foo", &div_foo));
        assert!(!SelectorMatcher::matches("div.foo", &div_bar), "clase distinta no deberia matchear");
        assert!(!SelectorMatcher::matches("div.foo", &span_foo), "tag distinto no deberia matchear aunque la clase coincida");
    }

    /// Selectores de atributo no existian en absoluto en el matcher anterior.
    #[test]
    fn attribute_selectors_match_by_presence_and_by_exact_value() {
        let link = element("a", &[("href", "https://example.com")]);
        let plain_a = element("a", &[]);

        assert!(SelectorMatcher::matches("[href]", &link));
        assert!(!SelectorMatcher::matches("[href]", &plain_a));
        assert!(SelectorMatcher::matches(r#"a[href="https://example.com"]"#, &link));
        assert!(!SelectorMatcher::matches(r#"a[href="https://other.example"]"#, &link));
    }

    /// Especificidad real: se compara como tripleta (ids, clases, tags) en
    /// ese orden - un id gana a cualquier combinacion de clases/tags, y a
    /// igualdad de clases, mas tags en el selector SI suma (compound > simple).
    #[test]
    fn specificity_orders_by_id_then_class_then_type_per_spec() {
        let id = SelectorMatcher::calculate_specificity("#main");
        let compound = SelectorMatcher::calculate_specificity("div.main");
        let class_only = SelectorMatcher::calculate_specificity(".main");
        let tag_only = SelectorMatcher::calculate_specificity("div");

        assert!(id > compound, "un id pesa mas que cualquier combinacion de clases/tags");
        assert!(compound > class_only, "div.main (clase+tag) pesa mas que .main (solo clase)");
        assert!(class_only > tag_only, "una clase pesa mas que un tag");
    }

    /// Los estados de interaccion se PARSEAN (para no descartar la regla
    /// entera) pero nunca coinciden: este motor no recalcula la cascada al
    /// mover el raton ni al enfocar. Es ademas lo que hace un navegador
    /// real con `:hover` en un dispositivo sin puntero.
    #[test]
    fn interaction_pseudo_classes_parse_but_never_match() {
        let div = element("div", &[]);
        assert!(!SelectorMatcher::matches("div:hover", &div));
        assert!(!SelectorMatcher::matches("div:focus", &div));
        assert!(!SelectorMatcher::matches("div:active", &div));
    }

    /// La razon de parsearlas en vez de rechazarlas: una regla con VARIOS
    /// selectores conserva los que si coinciden, en vez de perderse
    /// entera. Antes, el `:hover` invalidaba toda la regla.
    #[test]
    fn a_selector_list_keeps_its_matchable_half_despite_an_interaction_pseudo_class() {
        let div = element("div", &[("class", "btn")]);
        assert!(SelectorMatcher::matches(".btn, .btn:hover", &div), "la mitad sin :hover deberia seguir aplicando");
    }

    /// Una pseudo-clase que el motor NO conoce si invalida la regla -
    /// mejor no aplicarla que aplicarla mal.
    #[test]
    fn a_genuinely_unknown_pseudo_class_still_invalidates_the_rule() {
        let div = element("div", &[]);
        assert!(!SelectorMatcher::matches("div:pseudoinventada", &div));
    }

    /// El "checkbox hack": la razon principal de implementar `:checked`.
    /// Es como media web sin JavaScript hace menus desplegables y
    /// acordeones, y antes la regla entera se descartaba.
    #[test]
    fn checked_matches_a_checked_input_and_powers_the_sibling_combinator() {
        let marcado = element("input", &[("type", "checkbox"), ("checked", "")]);
        let sin_marcar = element("input", &[("type", "checkbox")]);
        assert!(SelectorMatcher::matches("input:checked", &marcado));
        assert!(!SelectorMatcher::matches("input:checked", &sin_marcar));

        // El caso real completo: `#toggle:checked + .menu`.
        let raiz = element("div", &[]);
        let toggle = element("input", &[("type", "checkbox"), ("id", "toggle"), ("checked", "")]);
        let menu = element("div", &[("class", "menu")]);
        Node::append_child(&raiz, toggle);
        Node::append_child(&raiz, menu.clone());
        assert!(SelectorMatcher::matches("#toggle:checked + .menu", &menu), "el checkbox hack completo deberia funcionar");
    }

    /// Semantica de atributo booleano HTML real: cuenta la PRESENCIA del
    /// atributo, no su valor - `checked="false"` sigue estando marcado.
    #[test]
    fn checked_follows_html_boolean_attribute_semantics() {
        let raro = element("input", &[("type", "checkbox"), ("checked", "false")]);
        assert!(SelectorMatcher::matches("input:checked", &raro), "en HTML la presencia es lo que cuenta, no el valor");
    }

    #[test]
    fn disabled_and_enabled_are_complementary_and_only_apply_to_form_controls() {
        let apagado = element("input", &[("disabled", "")]);
        let encendido = element("input", &[]);
        assert!(SelectorMatcher::matches("input:disabled", &apagado));
        assert!(!SelectorMatcher::matches("input:enabled", &apagado));
        assert!(SelectorMatcher::matches("input:enabled", &encendido));

        // Un `<div disabled>` NO esta deshabilitado en el sentido del spec.
        let div = element("div", &[("disabled", "")]);
        assert!(!SelectorMatcher::matches("div:disabled", &div), "solo los controles de formulario pueden estar deshabilitados");
    }

    #[test]
    fn required_and_optional_are_complementary() {
        let obligatorio = element("input", &[("required", "")]);
        let opcional = element("input", &[]);
        assert!(SelectorMatcher::matches("input:required", &obligatorio));
        assert!(SelectorMatcher::matches("input:optional", &opcional));
        assert!(!SelectorMatcher::matches("input:required", &opcional));
    }

    #[test]
    fn link_matches_an_anchor_with_href_but_visited_never_matches() {
        let enlace = element("a", &[("href", "/x")]);
        let ancla_sin_href = element("a", &[]);
        assert!(SelectorMatcher::matches("a:link", &enlace));
        assert!(SelectorMatcher::matches("a:any-link", &enlace));
        assert!(!SelectorMatcher::matches("a:link", &ancla_sin_href));
        assert!(
            !SelectorMatcher::matches("a:visited", &enlace),
            ":visited nunca deberia coincidir - estilarlo filtraria el historial de navegacion"
        );
    }

    /// Las pseudo-clases ESTRUCTURALES las resuelve el propio crate
    /// `selectors` con los metodos de arbol que `ElementRef` ya
    /// implementaba - este test fija que de verdad funcionan, porque nunca
    /// se habian probado.
    #[test]
    fn structural_pseudo_classes_already_work_through_the_selectors_crate() {
        let raiz = element("ul", &[]);
        let primero = element("li", &[]);
        let medio = element("li", &[]);
        let ultimo = element("li", &[]);
        Node::append_child(&raiz, primero.clone());
        Node::append_child(&raiz, medio.clone());
        Node::append_child(&raiz, ultimo.clone());

        assert!(SelectorMatcher::matches("li:first-child", &primero));
        assert!(!SelectorMatcher::matches("li:first-child", &medio));
        assert!(SelectorMatcher::matches("li:last-child", &ultimo));
        assert!(SelectorMatcher::matches("li:nth-child(2)", &medio));
        assert!(SelectorMatcher::matches("li:not(.especial)", &primero));

        let vacio = element("div", &[]);
        assert!(SelectorMatcher::matches("div:empty", &vacio));
        assert!(!SelectorMatcher::matches("ul:empty", &raiz), "un ul con hijos no esta vacio");
    }

    #[test]
    fn query_first_finds_the_first_match_in_document_order() {
        let root = element("div", &[]);
        let first_p = element("p", &[("class", "target")]);
        let second_p = element("p", &[("class", "target")]);
        Node::append_child(&root, first_p.clone());
        Node::append_child(&root, second_p);

        let found = SelectorMatcher::query_first(".target", &root);
        assert!(matches!(&found, Some(n) if Arc::ptr_eq(n, &first_p)), "deberia devolver el primero en orden de documento, no cualquiera");
    }

    #[test]
    fn query_first_searches_nested_descendants_not_just_direct_children() {
        let root = element("div", &[]);
        let wrapper = element("section", &[]);
        let target = element("span", &[("id", "deep")]);
        Node::append_child(&wrapper, target.clone());
        Node::append_child(&root, wrapper);

        let found = SelectorMatcher::query_first("#deep", &root);
        assert!(matches!(&found, Some(n) if Arc::ptr_eq(n, &target)));
    }

    #[test]
    fn query_first_can_match_the_root_node_itself() {
        let root = element("div", &[("id", "root")]);
        let found = SelectorMatcher::query_first("#root", &root);
        assert!(matches!(&found, Some(n) if Arc::ptr_eq(n, &root)), "la raiz tambien deberia poder matchear, no solo sus descendientes");
    }

    #[test]
    fn query_first_returns_none_when_nothing_matches_or_selector_is_unsupported() {
        let root = element("div", &[]);
        assert!(SelectorMatcher::query_first(".no-existe", &root).is_none());
        assert!(SelectorMatcher::query_first("div:hover", &root).is_none(), "pseudo-clase no soportada: no deberia matchear por accidente");
    }

    #[test]
    fn query_all_finds_every_match_in_document_order_not_just_the_first() {
        let root = element("div", &[]);
        let first = element("p", &[("class", "target")]);
        let middle = element("span", &[]);
        let second = element("p", &[("class", "target")]);
        Node::append_child(&root, first.clone());
        Node::append_child(&root, middle);
        Node::append_child(&root, second.clone());

        let found = SelectorMatcher::query_all(".target", &root);
        assert_eq!(found.len(), 2);
        assert!(Arc::ptr_eq(&found[0], &first));
        assert!(Arc::ptr_eq(&found[1], &second));
    }

    #[test]
    fn query_all_returns_empty_when_nothing_matches_or_selector_is_unsupported() {
        let root = element("div", &[]);
        Node::append_child(&root, element("p", &[]));
        assert!(SelectorMatcher::query_all(".no-existe", &root).is_empty());
        assert!(SelectorMatcher::query_all("div:hover", &root).is_empty());
    }
}
