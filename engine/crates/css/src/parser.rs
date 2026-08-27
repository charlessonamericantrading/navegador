//! Parsea CSS real usando `cssparser` (el tokenizador que usa Servo/Stylo):
//! respeta bloques anidados, strings con `{`/`}`/`;`/`:` dentro, comentarios
//! y arroba-reglas (`@media`, etc.) correctamente en vez de partir el texto
//! por caracteres literales como hacia el parser anterior.
//!
//! Simplificacion honesta: el *selector* de cada regla se guarda tal cual
//! como texto sin interpretar (`RuleParser::parse_prelude` solo consume y
//! devuelve el slice crudo). El matching real de selectores sigue viviendo
//! en `selector.rs` con su implementacion actual (tag/.clase/#id, sin
//! combinadores) - sustituirla por el crate `selectors` (que exige
//! implementar el trait `Element` con ~26 metodos mas `SelectorImpl`) es
//! tarea aparte, no incluida en este cambio.
//!
//! Arroba-reglas (`@media`, `@font-face`, etc.) se ignoran por completo: al
//! no sobrescribir los metodos de `AtRuleParser`, sus implementaciones por
//! defecto rechazan cualquier arroba-regla, y `StyleSheetParser` salta su
//! preludio y bloque sin corromper el resto de la hoja de estilos.

use crate::stylesheet::{MediaCondition, Rule, StyleSheet};
use cssparser::{
    AtRuleParser, Delimiter, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, StyleSheetParser,
};
use std::collections::{HashMap, HashSet};

struct RuleParser;

/// Sin diagnostico de errores detallado todavia (`Error = ()`): cuando una
/// regla no parsea, se descarta y se sigue con la siguiente, igual que hace
/// un navegador real con CSS invalido - no hay nada que fingir aqui.
impl<'i> AtRuleParser<'i> for RuleParser {
    type Prelude = MediaCondition;
    type AtRule = Vec<Rule>;
    type Error = ();

    /// El preludio de un `@media` es todo lo que va entre el nombre y la
    /// llave (`screen and (max-width: 600px)`). Se consume entero como
    /// texto crudo y se interpreta con `parse_media_condition`.
    ///
    /// Cualquier OTRA arroba-regla (`@font-face`, `@keyframes`,
    /// `@supports`...) se rechaza aqui, y `StyleSheetParser` salta su
    /// bloque entero sin corromper el resto de la hoja - igual que antes
    /// de que `@media` existiera.
    fn parse_prelude<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        if !name.eq_ignore_ascii_case("media") {
            return Err(input.new_custom_error(()));
        }
        let start = input.position();
        while input.next().is_ok() {}
        Ok(parse_media_condition(input.slice_from(start)))
    }

    /// El cuerpo de un `@media` son REGLAS normales, no declaraciones -
    /// asi que se recorre con otro `StyleSheetParser` anidado (el mismo
    /// codigo que la hoja de nivel superior, sin duplicar nada) y a cada
    /// regla resultante se le estampa la condicion.
    ///
    /// Anidar un `@media` dentro de otro funciona por recursion, pero solo
    /// se conserva la condicion MAS INTERNA: combinar ambas exigiria una
    /// condicion compuesta que este modelo minimo no tiene. Es un caso
    /// raro y queda declarado aqui en vez de fingirse.
    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, Self::Error>> {
        let mut nested_parser = RuleParser;
        let mut rules = Vec::new();
        for result in StyleSheetParser::new(input, &mut nested_parser) {
            if let Ok(inner) = result {
                for mut rule in inner {
                    rule.media.get_or_insert_with(|| prelude.clone());
                    rules.push(rule);
                }
            }
        }
        Ok(rules)
    }
}

/// Longitud en pixeles de un valor de media query (`769px`, `0`).
/// Deliberadamente solo `px`: `em`/`rem` dependerian del tamano de letra de
/// la raiz, que en una media query es el del NAVEGADOR y no el de la
/// pagina, y `calc()` habria que evaluarlo entero.
fn parse_media_px(value: &str) -> Option<f32> {
    let v = value.trim();
    if v == "0" {
        return Some(0.0);
    }
    v.strip_suffix("px")?.trim().parse::<f32>().ok()
}

/// Interpreta una caracteristica escrita con la SINTAXIS DE RANGOS de Media
/// Queries nivel 4 - `(width > 769px)`, `(width <= 1044px)`, y tambien las
/// formas con el valor a la izquierda (`(769px < width)`) y las de doble
/// extremo (`(400px <= width <= 900px)`).
///
/// No es una comodidad moderna prescindible: es como escribe hoy sus puntos
/// de ruptura una parte grande de la web (MDN, por ejemplo, mete AHI toda
/// la maquetacion de su cabecera). Sin entenderla, el bloque entero se
/// marcaba como no evaluable y se descartaba - la pagina se veia con la
/// disposicion de movil en una ventana de escritorio.
///
/// Devuelve `true` si reconocio la caracteristica (y ya escribio los
/// limites en `condition`), `false` si no es una comparacion de `width`.
fn parse_media_range(body: &str, condition: &mut MediaCondition) -> bool {
    if !body.contains('<') && !body.contains('>') {
        return false;
    }
    // Se normaliza a una lista [operando, operador, operando, ...].
    let mut partes: Vec<String> = Vec::new();
    let mut actual = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' || c == '>' {
            partes.push(actual.trim().to_string());
            actual.clear();
            let mut op = c.to_string();
            if chars.peek() == Some(&'=') {
                chars.next();
                op.push('=');
            }
            partes.push(op);
        } else {
            actual.push(c);
        }
    }
    partes.push(actual.trim().to_string());

    // `a OP b` o `a OP b OP c`. Cualquier otra forma no se sabe leer.
    let comparaciones: Vec<(&str, &str, &str)> = match partes.len() {
        3 => vec![(partes[0].as_str(), partes[1].as_str(), partes[2].as_str())],
        5 => vec![
            (partes[0].as_str(), partes[1].as_str(), partes[2].as_str()),
            (partes[2].as_str(), partes[3].as_str(), partes[4].as_str()),
        ],
        _ => return false,
    };

    for (izq, op, der) in comparaciones {
        // Se reescribe siempre como `width OP valor`, dando la vuelta al
        // operador cuando el valor viene primero (`769px < width`).
        let (op, valor) = if izq == "width" {
            (op.to_string(), der)
        } else if der == "width" {
            let volteado = match op {
                "<" => ">",
                "<=" => ">=",
                ">" => "<",
                ">=" => "<=",
                _ => return false,
            };
            (volteado.to_string(), izq)
        } else {
            return false;
        };
        let Some(px) = parse_media_px(valor) else { return false };
        match op.as_str() {
            ">" => condition.min_width_exclusive = Some(px),
            ">=" => condition.min_width = Some(px),
            "<" => condition.max_width_exclusive = Some(px),
            "<=" => condition.max_width = Some(px),
            _ => return false,
        }
    }
    true
}

/// Interpreta el preludio de un `@media` (`screen and (max-width: 600px)`).
///
/// Entiende `min-width`/`max-width` en pixeles, la sintaxis de rangos de
/// nivel 4 (`width > 600px`, ver `parse_media_range`) y el tipo de medio;
/// ver `MediaCondition` para por que ese subconjunto. Lo que no sepa
/// interpretar se marca `never_matches`, de modo que sus reglas se
/// CONSERVAN pero no se aplican - aplicarlas siempre seria peor (meteria
/// estilos de impresion o de movil en una ventana de escritorio).
fn parse_media_condition(prelude: &str) -> MediaCondition {
    let text = prelude.trim().to_ascii_lowercase();
    let mut condition = MediaCondition::default();

    // Un `not` o un medio que no somos invalidan la consulta entera.
    if text.starts_with("not ") || text.contains("print") || text.contains("speech") {
        condition.never_matches = true;
        return condition;
    }

    let mut saw_supported_feature = false;
    for feature in text.split('(').skip(1) {
        let Some(body) = feature.split(')').next() else { continue };
        // Sintaxis de rangos de nivel 4 antes que nada: `(width > 769px)`
        // no lleva dos puntos, asi que sin esto caeria en "caracteristica
        // sin valor" y anularia la consulta entera.
        if parse_media_range(body, &mut condition) {
            saw_supported_feature = true;
            continue;
        }
        let Some((name, value)) = body.split_once(':') else {
            // Una caracteristica sin valor (`(hover)`, `(color)`) no se
            // sabe evaluar.
            condition.never_matches = true;
            continue;
        };
        let px = value.trim().strip_suffix("px").and_then(|n| n.trim().parse::<f32>().ok());
        match (name.trim(), px) {
            ("min-width", Some(v)) => {
                condition.min_width = Some(v);
                saw_supported_feature = true;
            }
            ("max-width", Some(v)) => {
                condition.max_width = Some(v);
                saw_supported_feature = true;
            }
            // `min-width: 40em` o `(orientation: landscape)`: reconocida la
            // forma pero no el valor/caracteristica - no se puede evaluar.
            _ => condition.never_matches = true,
        }
    }

    // `@media screen { }` sin ninguna caracteristica SI aplica: pide
    // pantalla, que es lo que somos.
    if !saw_supported_feature && !text.contains('(') && (text.contains("screen") || text.contains("all") || text.trim().is_empty()) {
        condition.never_matches = false;
    }

    condition
}

impl<'i> QualifiedRuleParser<'i> for RuleParser {
    type Prelude = String;
    /// `Vec` y no `Rule` porque `StyleSheetParser` exige el MISMO tipo de
    /// salida para reglas normales y arroba-reglas, y un `@media` produce
    /// varias (ver `AtRuleParser::parse_block`). Una regla normal devuelve
    /// simplemente un vector de un elemento.
    type QualifiedRule = Vec<Rule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(input.slice_from(start).trim().to_string())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, Self::Error>> {
        let (declarations, important) = parse_declaration_list(input);
        Ok(vec![Rule {
            key: crate::stylesheet::RuleKey::from_selector(&prelude),
            selector: prelude,
            declarations,
            important,
            media: None,
        }])
    }
}

/// El cuerpo de una regla (lo que va entre `{`/`}`) y el valor de un
/// atributo `style="..."` son la MISMA gramatica CSS - una lista plana de
/// `propiedad: valor;` sin selector - asi que ambos casos (`parse_block`
/// arriba y `CssParser::parse_inline_style` mas abajo) reusan este bucle en
/// vez de duplicarlo.
fn parse_declaration_list<'i, 't>(input: &mut Parser<'i, 't>) -> (HashMap<String, String>, HashSet<String>) {
    let mut declarations = HashMap::new();
    let mut important = HashSet::new();

    loop {
        input.skip_whitespace();
        if input.is_exhausted() {
            break;
        }

        let name = match input.expect_ident_cloned() {
            Ok(name) => name.to_string(),
            Err(_) => {
                let _ = input.parse_until_after::<_, (), ()>(Delimiter::Semicolon, |_| Ok(()));
                continue;
            }
        };

        if input.expect_colon().is_err() {
            let _ = input.parse_until_after::<_, (), ()>(Delimiter::Semicolon, |_| Ok(()));
            continue;
        }

        let value = input
            .parse_until_after::<_, String, ()>(Delimiter::Semicolon, |input| {
                let start = input.position();
                while input.next().is_ok() {}
                Ok(input.slice_from(start).trim().to_string())
            })
            .unwrap_or_default();

        // `!important` (Fase 22) se separa del valor AQUI, no en la
        // cascada: asi todo lo que consume un valor CSS (parseo de color,
        // de longitud...) lo recibe limpio, sin tener que saber nada de la
        // cascada. Antes se quedaba pegado y rompia el parseo del valor.
        let name = name.to_lowercase();
        let (value, is_important) = split_important(&value);
        if is_important {
            important.insert(name.clone());
            // El shorthand expandido hereda la importancia del original:
            // `background: red !important` hace importante tambien al
            // `background-color` que se deriva de el.
            if name == "background" {
                important.insert("background-color".to_string());
            }
        }
        insert_declaration(&mut declarations, name, value);
    }

    (declarations, important)
}

/// Separa el sufijo `!important` del valor. Devuelve el valor limpio y si
/// lo llevaba. Tolera espacios y mayusculas (`! IMPORTANT` es valido en el
/// spec).
fn split_important(value: &str) -> (String, bool) {
    let trimmed = value.trim();
    let Some(bang) = trimmed.rfind('!') else { return (trimmed.to_string(), false) };
    if trimmed[bang + 1..].trim().eq_ignore_ascii_case("important") {
        (trimmed[..bang].trim().to_string(), true)
    } else {
        (trimmed.to_string(), false)
    }
}

/// Guarda una declaracion, expandiendo los shorthand que este motor
/// entiende a sus longhand equivalentes ANTES de insertar - asi la cascada
/// (`engine_css::cascade::apply_matching_rules`, que fusiona declaracion a
/// declaracion sin saber nada de shorthand/longhand, un simple `insert`
/// por clave) las trata exactamente igual que si el autor hubiera escrito
/// el longhand directamente. Esto es lo que hace que el orden importe
/// correctamente en los dos sentidos, sin ningun caso especial en la
/// cascada: una regla mas especifica que solo toque el longhand
/// (`background-color: azul` sobre una `background: rojo` menos
/// especifica en otra regla) gana porque se inserta despues sobre la
/// misma clave; y DENTRO de la misma regla, si el shorthand aparece
/// DESPUES del longhand (`background-color: azul; background: rojo;`),
/// el shorthand lo resetea - tambien el orden natural de insercion, sin
/// logica aparte.
///
/// Solo `background` esta expandido hoy, y solo se extrae de el un COLOR
/// (el unico sub-valor que `engine-gfx` pinta - ver
/// `display_list::build_items`, que solo lee `background-color`, nunca
/// `background`): posicion/repeticion/imagen del shorthand se ignoran sin
/// error, igual que cualquier propiedad no soportada.
///
/// **Este modulo no sabe que es un color, y a proposito**: la tabla de
/// nombres y el parseo de `rgb()`/hex viven en `engine-gfx`
/// (`parse_css_color`), que es quien pinta, y `engine-css` no depende de
/// el (son crates hermanos). El reparto es: aqui se IDENTIFICA el token
/// candidato a color, y alli se decide si de verdad lo es. Un candidato
/// que no lo sea simplemente no se pinta, que es la misma degradacion
/// honesta de siempre - no hace falta duplicar la tabla de colores para
/// conseguirlo.
fn insert_declaration(declarations: &mut HashMap<String, String>, name: String, value: String) {
    if name == "background" {
        if let Some(color) = background_color_candidate(&value) {
            declarations.insert("background-color".to_string(), color);
        }
    }
    if name == "padding" || name == "margin" {
        expand_box_shorthand(declarations, &name, &value);
    }
    if name == "flex" {
        expand_flex_shorthand(declarations, &value);
    }
    declarations.insert(name, value);
}

/// Expande `padding`/`margin` a sus cuatro longhands con las formas de 1, 2,
/// 3 y 4 valores del spec.
///
/// Antes solo existia la forma de UN valor, y no porque estuviera modelada:
/// `engine-layout` leia la propiedad abreviada como si fuera una longitud
/// suelta, asi que `padding: 16px 24px` no fallaba con un aviso - parseaba
/// como invalido y resolvia a CERO en los cuatro lados. Es decir, la forma
/// mas comun de escribir padding en CSS real se ignoraba en silencio.
/// Verificado en vivo: `padding: 20px` se veia y `padding: 20px 60px` no.
///
/// Un valor con parentesis (`calc(...)`, `var(...)`) se deja pasar sin
/// expandir: trocearlo por espacios lo romperia. En ese caso el longhand no
/// se genera y el layout resuelve a cero, igual que antes - no es una
/// regresion, es el mismo limite de siempre acotado a un caso mucho menor.
fn expand_box_shorthand(declarations: &mut HashMap<String, String>, name: &str, value: &str) {
    if value.contains('(') {
        return;
    }
    let parts: Vec<&str> = value.split_whitespace().collect();
    let (top, right, bottom, left) = match parts.as_slice() {
        [all] => (*all, *all, *all, *all),
        [vertical, horizontal] => (*vertical, *horizontal, *vertical, *horizontal),
        [top, horizontal, bottom] => (*top, *horizontal, *bottom, *horizontal),
        [top, right, bottom, left] => (*top, *right, *bottom, *left),
        _ => return,
    };
    for (side, side_value) in [("top", top), ("right", right), ("bottom", bottom), ("left", left)] {
        declarations.insert(format!("{name}-{side}"), side_value.to_string());
    }
}

/// Expande el shorthand `flex` a `flex-grow`/`flex-shrink`/`flex-basis`.
///
/// `flex: 1` es la forma en que se escribe flexbox en la practica, y no
/// estaba soportada: `engine-layout` leia unicamente los longhands, asi que
/// los items no crecian y se quedaban al ancho de su contenido. Verificado
/// en vivo: `flex-grow: 1; flex-basis: 0` funcionaba y `flex: 1` no.
///
/// Los valores iniciales al expandir NO son los mismos que cuando la
/// propiedad no esta puesta, que es la trampa clasica de este shorthand:
/// `flex: 1` significa `1 1 0%`, no `1 1 auto`. Se emite `0px` en vez de
/// `0%` porque es lo que el layout sabe resolver hoy, y para una base de
/// cero ambos significan lo mismo.
fn expand_flex_shorthand(declarations: &mut HashMap<String, String>, value: &str) {
    let trimmed = value.trim();
    let (grow, shrink, basis) = match trimmed.to_ascii_lowercase().as_str() {
        "none" => ("0", "0", "auto"),
        "auto" => ("1", "1", "auto"),
        "initial" => ("0", "1", "auto"),
        _ => {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let is_number = |token: &str| token.parse::<f32>().is_ok();
            match parts.as_slice() {
                // `flex: <grow>` -> 1 de shrink y base CERO (no `auto`).
                [grow] if is_number(grow) => (*grow, "1", "0px"),
                // `flex: <basis>` (una longitud suelta) -> crece y encoge.
                [basis] => ("1", "1", *basis),
                [grow, second] if is_number(grow) && is_number(second) => (*grow, *second, "0px"),
                [grow, basis] if is_number(grow) => (*grow, "1", *basis),
                [grow, shrink, basis] if is_number(grow) && is_number(shrink) => (*grow, *shrink, *basis),
                _ => return,
            }
        }
    };
    declarations.insert("flex-grow".to_string(), grow.to_string());
    declarations.insert("flex-shrink".to_string(), shrink.to_string());
    declarations.insert("flex-basis".to_string(), basis.to_string());
}

/// Palabras clave del shorthand `background` que NUNCA son un color -
/// posicion, repeticion, tamaño, anclaje, area de pintado y los valores
/// globales de CSS. Si tras descartarlas no queda nada, es que el
/// shorthand no traia color.
const BACKGROUND_NON_COLOR_KEYWORDS: &[&str] = &[
    "repeat", "repeat-x", "repeat-y", "no-repeat", "space", "round",
    "scroll", "fixed", "local",
    "left", "right", "top", "bottom", "center",
    "cover", "contain", "auto",
    "border-box", "padding-box", "content-box", "text",
    "none", "inherit", "initial", "unset", "revert",
];

/// El trozo del valor de `background` que puede ser un color.
///
/// El caso dominante con diferencia en CSS real es `background: <color>` a
/// secas, asi que se trata aparte y se devuelve el valor ENTERO sin
/// trocear: eso es lo que hace que `rgb(1, 2, 3)` y `rgba(0, 0, 0, .5)`
/// funcionen, ya que partir por espacios los romperia en pedazos
/// inservibles.
///
/// Solo cuando el valor trae ademas otras partes del shorthand (una
/// imagen, una posicion...) se cae a buscar un token suelto, y ahi si se
/// exige que empiece por `#`: sin la tabla de nombres no hay forma fiable
/// de distinguir un nombre de color de un valor de posicion que tampoco
/// esta en la lista de arriba, y equivocarse pintaria un fondo que el
/// autor no pidio.
fn background_color_candidate(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    let has_image = lower.contains("url(") || lower.contains("gradient(");
    let has_other_keyword = lower
        .split_whitespace()
        .any(|token| BACKGROUND_NON_COLOR_KEYWORDS.contains(&token.trim_matches(',')));

    if !has_image && !has_other_keyword {
        // `background: red`, `background: #ff0000`, `background: rgb(1,2,3)`.
        return Some(trimmed.to_string());
    }

    trimmed.split_whitespace().find(|token| token.starts_with('#')).map(str::to_string)
}

pub struct CssParser;

impl CssParser {
    pub fn parse(css_str: &str) -> StyleSheet {
        let mut parser_input = ParserInput::new(css_str);
        let mut input = Parser::new(&mut parser_input);
        let mut rule_parser = RuleParser;
        let mut stylesheet = StyleSheet::new();

        let rules = StyleSheetParser::new(&mut input, &mut rule_parser);
        for result in rules {
            match result {
                Ok(rules) => stylesheet.rules.extend(rules),
                Err((err, _slice)) => {
                    // CSS invalido/no soportado (p.ej. una arroba-regla) es
                    // el caso comun y esperado, no un fallo del motor.
                    tracing::trace!("[cssparser] regla ignorada: {err:?}");
                }
            }
        }

        tracing::info!("[cssparser] {} reglas parseadas", stylesheet.rules.len());
        stylesheet
    }

    /// Parsea el valor de un atributo HTML `style="..."` - la misma
    /// gramatica que el interior de un bloque `{ }` de una regla normal
    /// (ver `parse_declaration_list`), pero sin selector ni llaves porque
    /// aqui no hacen falta: el elemento dueño del atributo YA es el
    /// "selector" implicito. Quien llama (`layout::resolve_style`) decide
    /// como fusionar esto con las reglas del stylesheet - este parser no
    /// sabe nada de especificidad ni de a que elemento pertenece.
    pub fn parse_inline_style(style_str: &str) -> HashMap<String, String> {
        let mut parser_input = ParserInput::new(style_str);
        let mut input = Parser::new(&mut parser_input);
        // El atributo `style` gana a cualquier selector de todas formas
        // (ver `cascade::resolve_style`), asi que su `!important` solo
        // importaria frente a OTRO `!important` de una hoja - caso raro y
        // sin modelar: aqui se descarta el conjunto y se queda el valor ya
        // limpio, que es lo que arregla el bug de parseo.
        parse_declaration_list(&mut input).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El punto real de la expansion de shorthand: `background: #ff0000`
    /// (nunca pintado antes, ver ARCHITECTURE.md) deberia dejar TAMBIEN
    /// `background-color` en las declaraciones, que es lo unico que
    /// `engine-gfx` lee al pintar.
    #[test]
    fn background_shorthand_expands_a_hex_color_into_background_color() {
        let sheet = CssParser::parse("div { background: #ff0000; }");
        assert_eq!(sheet.rules[0].declarations.get("background-color").map(String::as_str), Some("#ff0000"));
        assert_eq!(sheet.rules[0].declarations.get("background").map(String::as_str), Some("#ff0000"), "el valor crudo del shorthand se conserva ademas del longhand derivado");
    }

    /// Un valor de `background` sin ningun color hex (posicion/repeticion/
    /// una imagen sin color de respaldo) no deberia inventarse un
    /// `background-color` de la nada - simplemente no hay color que
    /// extraer, igual que hoy no hay soporte de `background-image`.
    #[test]
    fn background_shorthand_without_a_hex_color_does_not_fabricate_one() {
        let sheet = CssParser::parse("div { background: no-repeat center; }");
        assert_eq!(sheet.rules[0].declarations.get("background-color"), None);
    }

    /// Desde que `engine-gfx` entiende nombres de color, el shorthand
    /// tambien debe pasarlos - antes se descartaban a proposito porque
    /// nadie rio abajo sabia interpretarlos.
    #[test]
    fn background_shorthand_now_passes_named_colors_through() {
        let sheet = CssParser::parse("div { background: red; }");
        assert_eq!(sheet.rules[0].declarations.get("background-color").map(String::as_str), Some("red"));
    }

    /// `rgb()`/`rgba()` llevan espacios y comas DENTRO, asi que partir el
    /// valor por espacios los romperia en pedazos inservibles - de ahi que
    /// el caso "solo un color" se devuelva entero y sin trocear.
    #[test]
    fn background_shorthand_passes_rgb_functions_whole_instead_of_splitting_them() {
        let sheet = CssParser::parse("div { background: rgb(255, 0, 0); }");
        assert_eq!(sheet.rules[0].declarations.get("background-color").map(String::as_str), Some("rgb(255, 0, 0)"));

        let con_alfa = CssParser::parse("div { background: rgba(0, 0, 0, 0.5); }");
        assert_eq!(con_alfa.rules[0].declarations.get("background-color").map(String::as_str), Some("rgba(0, 0, 0, 0.5)"));
    }

    /// Un shorthand con posicion/repeticion pero SIN color no deberia
    /// inventarse uno tomando una palabra clave por un nombre de color.
    #[test]
    fn background_shorthand_with_only_position_keywords_yields_no_color() {
        let sheet = CssParser::parse("div { background: no-repeat center; }");
        assert_eq!(sheet.rules[0].declarations.get("background-color"), None);
    }

    /// Con imagen Y color a la vez se cae a buscar un token hexadecimal -
    /// sin la tabla de nombres (que vive en `engine-gfx`) no hay forma
    /// fiable de distinguir aqui un nombre de color de una palabra clave
    /// de posicion, y pintar un fondo que el autor no pidio seria peor
    /// que no pintarlo.
    #[test]
    fn background_shorthand_with_an_image_still_finds_a_hex_color() {
        let sheet = CssParser::parse("div { background: #ff0000 url(x.png) no-repeat; }");
        assert_eq!(sheet.rules[0].declarations.get("background-color").map(String::as_str), Some("#ff0000"));
    }

    /// Dentro de la MISMA regla, la propiedad declarada DESPUES gana -
    /// tanto si el shorthand llega despues del longhand (lo resetea) como
    /// al reves (el longhand explicito gana sobre el shorthand anterior).
    #[test]
    fn background_shorthand_and_longhand_within_the_same_rule_respect_declaration_order() {
        let shorthand_last = CssParser::parse("div { background-color: #00ff00; background: #ff0000; }");
        assert_eq!(
            shorthand_last.rules[0].declarations.get("background-color").map(String::as_str),
            Some("#ff0000"),
            "el shorthand declarado despues deberia resetear el longhand anterior"
        );

        let longhand_last = CssParser::parse("div { background: #ff0000; background-color: #00ff00; }");
        assert_eq!(
            longhand_last.rules[0].declarations.get("background-color").map(String::as_str),
            Some("#00ff00"),
            "el longhand explicito declarado despues deberia ganar sobre el shorthand anterior"
        );
    }

    #[test]
    fn background_shorthand_also_expands_inside_an_inline_style_attribute() {
        let declarations = CssParser::parse_inline_style("background: #123456");
        assert_eq!(declarations.get("background-color").map(String::as_str), Some("#123456"), "el mismo shorthand deberia expandirse igual en style=\"...\", misma funcion compartida");
    }

    #[test]
    fn parses_simple_declaration() {
        let sheet = CssParser::parse("body { background-color: #dbe9f4; }");
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector, "body");
        assert_eq!(
            sheet.rules[0].declarations.get("background-color").map(String::as_str),
            Some("#dbe9f4")
        );
    }

    #[test]
    fn parses_multiple_rules_and_declarations() {
        let sheet = CssParser::parse("body { background-color: #dbe9f4; } h1 { color: #222; }");
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.rules[1].selector, "h1");
        assert_eq!(sheet.rules[1].declarations.get("color").map(String::as_str), Some("#222"));
    }

    /// El splitter anterior partia por `{`/`}`/`;`/`:` literales, asi que un
    /// caracter especial dentro de un string rompia el parseo del resto de
    /// la hoja. cssparser tokeniza strings como unidad atomica.
    #[test]
    fn string_values_with_special_chars_do_not_break_parsing() {
        let sheet = CssParser::parse(r#"p { content: "a; b: c { d }"; } h1 { color: red; }"#);
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.rules[0].selector, "p");
        assert_eq!(
            sheet.rules[0].declarations.get("content").map(String::as_str),
            Some(r#""a; b: c { d }""#)
        );
        assert_eq!(sheet.rules[1].selector, "h1");
    }

    /// El splitter anterior no tenia ningun concepto de arroba-regla: el
    /// `{` de `@media` y sus reglas anidadas se habrian tratado como un
    /// bloque de declaraciones normal, produciendo basura.
    ///
    /// Este test comprobaba ademas que el bloque `@media` se DESCARTABA
    /// entero (`rules.len() == 1`). Desde la Fase 18 ya no: sus reglas se
    /// conservan con la condicion adjunta - ver
    /// `media_block_rules_are_kept_with_their_condition_attached`. Lo que
    /// sigue midiendo, y por lo que se conserva, es que la regla de
    /// DESPUES del bloque llega intacta.
    #[test]
    fn a_media_block_does_not_corrupt_the_rules_that_follow_it() {
        let sheet = CssParser::parse("@media (max-width: 600px) { body { color: blue; } } h1 { color: red; }");
        let h1 = sheet.rules.iter().find(|r| r.selector == "h1").expect("la regla posterior al bloque deberia parsearse intacta");
        assert_eq!(h1.declarations.get("color").map(String::as_str), Some("red"));
        assert!(h1.media.is_none());
    }

    /// La sintaxis de RANGOS de Media Queries nivel 4 (`(width > 769px)`)
    /// es como escribe hoy sus puntos de ruptura una parte grande de la web
    /// - MDN mete ahi toda la maquetacion de su cabecera. Antes no llevaba
    /// dos puntos, asi que caia en "caracteristica sin valor" y anulaba el
    /// bloque entero: la pagina se veia con la disposicion de movil en una
    /// ventana de escritorio.
    #[test]
    fn la_sintaxis_de_rangos_de_media_queries_se_entiende() {
        let sheet = CssParser::parse("@media (width > 769px) { nav { display: flex; } }");
        let cond = sheet.rules[0].media.as_ref().expect("deberia llevar condicion");
        assert!(!cond.never_matches, "un rango de anchura SI se sabe evaluar");
        assert!(cond.matches(1280.0), "1280px es mayor que 769px");
        assert!(!cond.matches(700.0), "700px no es mayor que 769px");
        assert!(!cond.matches(769.0), "`>` es ESTRICTO: justo en el punto de ruptura no aplica");
    }

    /// Las cuatro comparaciones, el valor a la izquierda, y la forma de
    /// doble extremo.
    #[test]
    fn los_rangos_cubren_las_dos_direcciones_y_el_doble_extremo() {
        let cond = |css: &str| CssParser::parse(css).rules[0].media.clone().expect("condicion");

        let menor_igual = cond("@media (width <= 1044px) { a { color: red; } }");
        assert!(menor_igual.matches(1044.0), "`<=` es inclusivo en su punto de ruptura");
        assert!(!menor_igual.matches(1045.0));

        let volteado = cond("@media (769px < width) { a { color: red; } }");
        assert!(volteado.matches(1280.0), "`769px < width` es lo mismo que `width > 769px`");
        assert!(!volteado.matches(700.0));

        let intervalo = cond("@media (400px <= width <= 900px) { a { color: red; } }");
        assert!(intervalo.matches(600.0));
        assert!(!intervalo.matches(300.0));
        assert!(!intervalo.matches(1000.0));
    }

    /// Antes, TODO bloque `@media` se descartaba entero - las reglas de
    /// dentro no existian para el motor. Ahora se conservan con su
    /// condicion adjunta.
    #[test]
    fn media_block_rules_are_kept_with_their_condition_attached() {
        let sheet = CssParser::parse("@media (max-width: 600px) { body { color: blue; } } h1 { color: red; }");
        assert_eq!(sheet.rules.len(), 2, "la regla de dentro del @media deberia conservarse, no descartarse");

        let dentro = sheet.rules.iter().find(|r| r.selector == "body").expect("deberia existir la regla de body");
        let condicion = dentro.media.as_ref().expect("deberia llevar condicion");
        assert_eq!(condicion.max_width, Some(600.0));

        let fuera = sheet.rules.iter().find(|r| r.selector == "h1").expect("deberia existir la regla de h1");
        assert!(fuera.media.is_none(), "una regla de nivel superior no lleva condicion");
    }

    #[test]
    fn media_condition_matches_only_within_its_width_range() {
        let sheet = CssParser::parse("@media (max-width: 600px) { body { color: blue; } }");
        let c = sheet.rules[0].media.as_ref().unwrap();
        assert!(c.matches(500.0));
        assert!(c.matches(600.0), "max-width es INCLUSIVO en el spec: justo en el punto de ruptura SI aplica");
        assert!(!c.matches(601.0));
    }

    #[test]
    fn min_and_max_width_together_form_a_closed_range() {
        let sheet = CssParser::parse("@media (min-width: 400px) and (max-width: 800px) { p { color: red; } }");
        let c = sheet.rules[0].media.as_ref().unwrap();
        assert!(!c.matches(399.0));
        assert!(c.matches(400.0));
        assert!(c.matches(800.0));
        assert!(!c.matches(801.0));
    }

    /// Una consulta que el motor no sabe evaluar CONSERVA sus reglas pero
    /// no las aplica - aplicarlas siempre seria peor (meteria estilos de
    /// impresion o de otra orientacion en una ventana normal).
    #[test]
    fn an_unsupported_media_feature_keeps_its_rules_but_never_matches() {
        let sheet = CssParser::parse("@media (orientation: landscape) { p { color: red; } }");
        assert_eq!(sheet.rules.len(), 1, "la regla se conserva");
        assert!(!sheet.rules[0].media.as_ref().unwrap().matches(1280.0), "pero no deberia aplicarse nunca");
    }

    #[test]
    fn print_only_styles_never_match_a_screen() {
        let sheet = CssParser::parse("@media print { body { color: black; } }");
        assert!(!sheet.rules[0].media.as_ref().unwrap().matches(1280.0));
    }

    #[test]
    fn a_plain_screen_query_without_features_always_matches() {
        let sheet = CssParser::parse("@media screen { body { color: red; } }");
        assert!(sheet.rules[0].media.as_ref().unwrap().matches(1280.0));
        assert!(sheet.rules[0].media.as_ref().unwrap().matches(320.0));
    }

    /// Las arroba-reglas que NO son `@media` se siguen saltando enteras
    /// sin corromper el resto de la hoja - comportamiento de siempre.
    #[test]
    fn other_at_rules_are_still_skipped_without_breaking_the_sheet() {
        let sheet = CssParser::parse("@font-face { font-family: X; src: url(x.woff); } h1 { color: red; }");
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector, "h1");
    }

    #[test]
    fn comments_are_ignored() {
        let sheet = CssParser::parse("/* h1 { color: green; } */ body { color: black; }");
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector, "body");
    }

    #[test]
    fn parse_inline_style_reads_a_single_declaration() {
        let declarations = CssParser::parse_inline_style("color: red");
        assert_eq!(declarations.get("color").map(String::as_str), Some("red"));
    }

    #[test]
    fn parse_inline_style_reads_multiple_declarations_separated_by_semicolons() {
        let declarations = CssParser::parse_inline_style("color: red; font-size: 14px");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations.get("color").map(String::as_str), Some("red"));
        assert_eq!(declarations.get("font-size").map(String::as_str), Some("14px"));
    }

    /// Un atributo style real de HTML suele terminar en `;` y puede traer
    /// espacios sueltos - ninguno de los dos deberia producir una
    /// declaracion vacia ni romper el parseo de las demas.
    #[test]
    fn parse_inline_style_tolerates_a_trailing_semicolon_and_stray_whitespace() {
        let declarations = CssParser::parse_inline_style("  color : red ;  font-size: 14px ;  ");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations.get("color").map(String::as_str), Some("red"));
        assert_eq!(declarations.get("font-size").map(String::as_str), Some("14px"));
    }

    #[test]
    fn parse_inline_style_of_an_empty_string_is_empty() {
        assert!(CssParser::parse_inline_style("").is_empty());
    }
}

#[cfg(test)]
mod shorthand_expansion_tests {
    use super::*;

    fn decls(css: &str) -> HashMap<String, String> {
        CssParser::parse(css).rules[0].declarations.clone()
    }

    /// El caso medido en vivo: `padding: 20px` se veia y `padding: 20px 60px`
    /// no, porque el layout leia la abreviada como una longitud suelta y
    /// "20px 60px" resolvia a CERO en los cuatro lados.
    #[test]
    fn the_two_value_padding_form_expands_to_vertical_and_horizontal() {
        let d = decls("div { padding: 20px 60px; }");
        assert_eq!(d.get("padding-top").map(String::as_str), Some("20px"));
        assert_eq!(d.get("padding-bottom").map(String::as_str), Some("20px"));
        assert_eq!(d.get("padding-left").map(String::as_str), Some("60px"));
        assert_eq!(d.get("padding-right").map(String::as_str), Some("60px"));
    }

    #[test]
    fn the_one_three_and_four_value_forms_follow_the_spec() {
        let one = decls("div { margin: 5px; }");
        assert_eq!(one.get("margin-left").map(String::as_str), Some("5px"));

        let three = decls("div { padding: 1px 2px 3px; }");
        assert_eq!(three.get("padding-top").map(String::as_str), Some("1px"));
        assert_eq!(three.get("padding-right").map(String::as_str), Some("2px"));
        assert_eq!(three.get("padding-bottom").map(String::as_str), Some("3px"));
        assert_eq!(three.get("padding-left").map(String::as_str), Some("2px"), "izquierda copia a derecha en la forma de 3");

        let four = decls("div { padding: 1px 2px 3px 4px; }");
        assert_eq!(four.get("padding-left").map(String::as_str), Some("4px"));
    }

    /// `flex: 1` es como se escribe flexbox en la practica y no se soportaba:
    /// solo se leian los longhands, asi que los items no crecian.
    #[test]
    fn flex_one_expands_to_grow_one_shrink_one_and_a_zero_basis() {
        let d = decls("div { flex: 1; }");
        assert_eq!(d.get("flex-grow").map(String::as_str), Some("1"));
        assert_eq!(d.get("flex-shrink").map(String::as_str), Some("1"));
        assert_eq!(d.get("flex-basis").map(String::as_str), Some("0px"), "la trampa del shorthand: la base es 0, no auto");
    }

    #[test]
    fn the_flex_keywords_follow_the_spec() {
        assert_eq!(decls("div { flex: none; }").get("flex-grow").map(String::as_str), Some("0"));
        assert_eq!(decls("div { flex: none; }").get("flex-basis").map(String::as_str), Some("auto"));
        assert_eq!(decls("div { flex: auto; }").get("flex-grow").map(String::as_str), Some("1"));
        assert_eq!(decls("div { flex: auto; }").get("flex-basis").map(String::as_str), Some("auto"));
    }

    /// Un longhand explicito declarado DESPUES sigue ganando al shorthand,
    /// que es lo que la cascada espera - la expansion no puede romper eso.
    #[test]
    fn an_explicit_longhand_after_the_shorthand_still_wins() {
        let d = decls("div { padding: 10px; padding-left: 99px; }");
        assert_eq!(d.get("padding-left").map(String::as_str), Some("99px"));
        assert_eq!(d.get("padding-top").map(String::as_str), Some("10px"));
    }

    /// `calc()`/`var()` no se trocean: se deja sin expandir en vez de
    /// partirlos por espacios y generar longhands sin sentido.
    #[test]
    fn values_with_parentheses_are_left_alone_instead_of_being_split() {
        let d = decls("div { padding: calc(10px + 2px) 4px; }");
        assert!(d.get("padding-top").is_none(), "no deberia inventar longhands a partir de un calc()");
    }
}
