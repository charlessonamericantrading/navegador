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
        true
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub selector: String,
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
