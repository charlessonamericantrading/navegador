use std::collections::HashMap;

/// La condicion de un bloque `@media` (Fase 18), ya interpretada.
///
/// Deliberadamente minima: solo `min-width`/`max-width` en pixeles y el
/// tipo de medio. Es lo que usa la practica totalidad del CSS responsive
/// real ("si la pantalla es mas estrecha que X, apila las columnas"), y
/// las demas caracteristicas (`orientation`, `prefers-color-scheme`,
/// `hover`, `resolution`...) exigirian que el motor tuviera nociones que
/// hoy no tiene.
///
/// Una condicion que el parser NO sepa interpretar se guarda como
/// `never_matches`, de forma que sus reglas se conservan pero no se
/// aplican - mas honesto que aplicarlas siempre (mostraria estilos de
/// movil en escritorio) o que descartar el bloque entero.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaCondition {
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    /// Limites ESTRICTOS de la sintaxis de rangos de Media Queries nivel 4
    /// (`@media (width > 769px)`): a diferencia de `min-width`/`max-width`,
    /// que son inclusivos, `>` y `<` excluyen el propio punto de ruptura.
    /// La diferencia solo se nota exactamente en ese pixel, pero es
    /// precisamente donde un `(width > 769px)` y un `(width <= 769px)`
    /// vecinos tienen que repartirse el mundo sin solaparse ni dejar hueco.
    pub min_width_exclusive: Option<f32>,
    pub max_width_exclusive: Option<f32>,
    /// `true` cuando la consulta pide un medio que no somos (`print`,
    /// `speech`) o usa una caracteristica no soportada.
    pub never_matches: bool,
}

impl MediaCondition {
    /// Si esta condicion se cumple con el ancho de viewport dado.
    ///
    /// `min-width` es inclusivo y `max-width` tambien (asi lo define el
    /// spec: `max-width: 600px` SI aplica exactamente a 600px), un detalle
    /// que decide el comportamiento justo en el punto de ruptura.
    pub fn matches(&self, viewport_width: f32) -> bool {
        if self.never_matches {
            return false;
        }
        if self.min_width.is_some_and(|min| viewport_width < min) {
            return false;
        }
        if self.max_width.is_some_and(|max| viewport_width > max) {
            return false;
        }
        if self.min_width_exclusive.is_some_and(|min| viewport_width <= min) {
            return false;
        }
        if self.max_width_exclusive.is_some_and(|max| viewport_width >= max) {
            return false;
        }
        true
    }
}

/// El simple-selector "clave" de una rama: el mas a la derecha, que es el
/// unico que se compara contra el elemento en si (todo lo que hay a su
/// izquierda habla de sus ANCESTROS/hermanos). Es el mismo truco que usan
/// Chromium y Firefox para no probar cada regla contra cada elemento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPart {
    Id(String),
    Class(String),
    /// Siempre en minusculas: `html5ever` normaliza los tags del DOM.
    Tag(String),
}

/// Filtro BARATO y CONSERVADOR previo al matcher real.
///
/// Existe porque la cascada compara cada nodo contra cada regla: con 13.000
/// nodos y miles de reglas eso son decenas de millones de invocaciones del
/// matcher completo del crate `selectors`, que resuelve combinadores
/// subiendo por el arbol. La inmensa mayoria de esas comparaciones son
/// triviales de descartar - un elemento `<p>` nunca puede matchear
/// `.infobox td a`, basta con mirar que su tag no es `a`.
///
/// La regla de oro es que este filtro NUNCA debe descartar algo que el
/// matcher real habria aceptado: ante cualquier duda devuelve `Any` y se
/// prueba igual. Por eso los selectores con `[`, `(`, comillas o `*` se
/// marcan `Any` en bloque en vez de intentar parsearlos a mano - el parseo
/// de verdad ya lo hace el crate `selectors`, aqui solo se busca un descarte
/// rapido, no una segunda implementacion del spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKey {
    /// No se pudo determinar una clave segura: probar SIEMPRE esta regla.
    Any,
    /// La regla solo puede matchear un elemento que cumpla AL MENOS una de
    /// estas (una por rama de la lista `a, b, c`).
    OneOf(Vec<KeyPart>),
}

impl RuleKey {
    pub fn from_selector(selector: &str) -> RuleKey {
        // Cualquier construccion que complique el troceado por comas o el
        // reconocimiento del compuesto final: no se arriesga.
        if selector.contains('[') || selector.contains('(') || selector.contains('"') || selector.contains('\'') {
            return RuleKey::Any;
        }
        let mut parts = Vec::new();
        for branch in selector.split(',') {
            match key_of_branch(branch) {
                Some(part) => parts.push(part),
                None => return RuleKey::Any,
            }
        }
        if parts.is_empty() {
            RuleKey::Any
        } else {
            RuleKey::OneOf(parts)
        }
    }

    /// `classes` llega ya troceado por el llamante, que lo calcula UNA vez
    /// por elemento y no una vez por regla - que es justo el punto de todo
    /// esto.
    pub fn could_match(&self, tag: &str, id: Option<&str>, classes: &[&str]) -> bool {
        match self {
            RuleKey::Any => true,
            RuleKey::OneOf(parts) => parts.iter().any(|part| match part {
                KeyPart::Tag(t) => t == tag,
                KeyPart::Id(i) => id == Some(i.as_str()),
                KeyPart::Class(c) => classes.contains(&c.as_str()),
            }),
        }
    }
}

/// Clave de UNA rama (sin comas). `None` = no hay clave segura.
fn key_of_branch(branch: &str) -> Option<KeyPart> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    // El compuesto final es lo que hay tras el ultimo combinador.
    let last = branch
        .rsplit(|c: char| c.is_whitespace() || c == '>' || c == '+' || c == '~')
        .find(|piece| !piece.is_empty())?;

    // Las pseudo-clases/elementos (`:hover`, `::before`) no restringen ni
    // el tag ni las clases del elemento, asi que se recortan. Si al hacerlo
    // no queda nada (el compuesto era SOLO una pseudo-clase), no hay clave.
    let compound = last.split(':').next().unwrap_or("");
    if compound.is_empty() || compound == "*" {
        return None;
    }

    if let Some(pos) = compound.find('#') {
        let id = compound[pos + 1..].split(['.', '#']).next().unwrap_or("");
        return if id.is_empty() { None } else { Some(KeyPart::Id(id.to_string())) };
    }
    if let Some(pos) = compound.find('.') {
        let class = compound[pos + 1..].split('.').next().unwrap_or("");
        return if class.is_empty() { None } else { Some(KeyPart::Class(class.to_string())) };
    }
    if compound.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(KeyPart::Tag(compound.to_ascii_lowercase()));
    }
    None
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub selector: String,
    /// Filtro barato precalculado al parsear (ver `RuleKey`). Se guarda
    /// aqui y no se recalcula en cada comparacion porque el punto entero
    /// es que mirarlo sea mas barato que matchear.
    pub key: RuleKey,
    /// Los valores ya SIN el sufijo `!important` - ese dato vive aparte,
    /// en `important`, para que todo lo que consume un valor CSS (parseo
    /// de color, de longitud...) reciba el valor limpio sin tener que
    /// saber nada de la cascada.
    pub declarations: HashMap<String, String>,
    /// Las propiedades de esta regla declaradas `!important` (Fase 22).
    ///
    /// Antes no se modelaba en absoluto, y el efecto era PEOR que
    /// ignorarlo: el sufijo se quedaba pegado al valor
    /// (`"#ff0000 !important"`), asi que la declaracion ganaba la cascada
    /// (sobrescribia a las anteriores) pero luego no parseaba como color y
    /// no pintaba nada - anulando la regla que habria ganado sin ella. Se
    /// descubrio verificando CSP en vivo, con una hoja de prueba que lo
    /// usaba.
    ///
    /// Se guarda como conjunto de NOMBRES y no como un valor por
    /// declaracion para no cambiar el tipo de `declarations`, que consumen
    /// media docena de sitios.
    pub important: std::collections::HashSet<String>,
    /// `Some` para una regla que venia dentro de un `@media` - la cascada
    /// la salta cuando la condicion no se cumple con el viewport actual
    /// (ver `cascade::apply_matching_rules`). `None` para las reglas
    /// normales, que aplican siempre.
    ///
    /// Se guarda POR REGLA en vez de agrupar las reglas dentro de un nodo
    /// "bloque media" a proposito: asi la cascada sigue siendo una lista
    /// plana ordenada por especificidad, sin ninguna estructura nueva que
    /// recorrer, y las reglas de dentro y fuera de un `@media` compiten
    /// entre si exactamente igual que en el spec.
    pub media: Option<MediaCondition>,
}

#[derive(Debug, Clone, Default)]
pub struct StyleSheet {
    pub rules: Vec<Rule>,
}

impl StyleSheet {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }
}

#[cfg(test)]
mod rule_key_tests {
    use super::*;

    fn key(sel: &str) -> RuleKey {
        RuleKey::from_selector(sel)
    }

    /// La clave es el compuesto MAS A LA DERECHA: lo de la izquierda habla
    /// de ancestros, no del elemento que se esta resolviendo.
    #[test]
    fn the_key_is_the_rightmost_compound() {
        assert_eq!(key("div"), RuleKey::OneOf(vec![KeyPart::Tag("div".into())]));
        assert_eq!(key("#a .b > c"), RuleKey::OneOf(vec![KeyPart::Tag("c".into())]));
        assert_eq!(key("div>p"), RuleKey::OneOf(vec![KeyPart::Tag("p".into())]), "sin espacios alrededor del combinador");
        assert_eq!(key(".foo"), RuleKey::OneOf(vec![KeyPart::Class("foo".into())]));
        assert_eq!(key("#bar"), RuleKey::OneOf(vec![KeyPart::Id("bar".into())]));
    }

    /// Con tag Y clase se prefiere la clase: descarta muchisimo mas.
    #[test]
    fn a_compound_with_a_class_keys_on_the_class() {
        assert_eq!(key("div.foo"), RuleKey::OneOf(vec![KeyPart::Class("foo".into())]));
        assert_eq!(key("div#x.foo"), RuleKey::OneOf(vec![KeyPart::Id("x".into())]));
    }

    /// Las pseudo-clases no restringen tag ni clase, asi que se recortan -
    /// pero un compuesto que SOLO es una pseudo-clase no deja clave alguna.
    #[test]
    fn pseudo_classes_are_trimmed_but_alone_they_give_no_key() {
        assert_eq!(key("a:hover"), RuleKey::OneOf(vec![KeyPart::Tag("a".into())]));
        assert_eq!(key("p::before"), RuleKey::OneOf(vec![KeyPart::Tag("p".into())]));
        assert_eq!(key(":hover"), RuleKey::Any);
    }

    /// Todo lo que no se sabe descartar con seguridad cae en `Any`, que
    /// significa "pruebala igual". Es la propiedad que impide que este
    /// atajo pueda perder un estilo.
    #[test]
    fn anything_uncertain_falls_back_to_any() {
        assert_eq!(key("*"), RuleKey::Any);
        assert_eq!(key("[href]"), RuleKey::Any);
        assert_eq!(key("a[href^=\"http\"]"), RuleKey::Any);
        assert_eq!(key(":not(.x)"), RuleKey::Any);
        assert_eq!(key(""), RuleKey::Any);
    }

    /// Una lista `a, b, c` puede matchear por cualquiera de sus ramas; si
    /// UNA sola rama no es descartable, la regla entera pasa a `Any`.
    #[test]
    fn a_selector_list_keeps_every_branch_and_one_unknown_poisons_it() {
        assert_eq!(
            key("a, .b, #c"),
            RuleKey::OneOf(vec![KeyPart::Tag("a".into()), KeyPart::Class("b".into()), KeyPart::Id("c".into())])
        );
        assert_eq!(key("a, [x]"), RuleKey::Any, "una rama no descartable obliga a probar la regla entera");
    }

    #[test]
    fn could_match_checks_tag_id_and_classes() {
        let k = key("a, .b, #c");
        assert!(k.could_match("a", None, &[]), "por tag");
        assert!(k.could_match("p", None, &["b"]), "por clase");
        assert!(k.could_match("p", Some("c"), &[]), "por id");
        assert!(!k.could_match("p", Some("otro"), &["z"]), "nada coincide");
        assert!(RuleKey::Any.could_match("lo-que-sea", None, &[]), "Any siempre pasa");
    }

    /// Los tags del DOM llegan en minusculas desde html5ever; un selector
    /// escrito `DIV` debe seguir casando.
    #[test]
    fn tag_keys_are_case_insensitive() {
        assert_eq!(key("DIV"), RuleKey::OneOf(vec![KeyPart::Tag("div".into())]));
        assert!(key("DIV").could_match("div", None, &[]));
    }
}
