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

use crate::stylesheet::{Rule, StyleSheet};
use cssparser::{
    AtRuleParser, Delimiter, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, StyleSheetParser,
};
use std::collections::HashMap;

struct RuleParser;

/// Sin diagnostico de errores detallado todavia (`Error = ()`): cuando una
/// regla no parsea, se descarta y se sigue con la siguiente, igual que hace
/// un navegador real con CSS invalido - no hay nada que fingir aqui.
impl<'i> AtRuleParser<'i> for RuleParser {
    type Prelude = ();
    type AtRule = Rule;
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for RuleParser {
    type Prelude = String;
    type QualifiedRule = Rule;
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
        Ok(Rule { selector: prelude, declarations: parse_declaration_list(input) })
    }
}

/// El cuerpo de una regla (lo que va entre `{`/`}`) y el valor de un
/// atributo `style="..."` son la MISMA gramatica CSS - una lista plana de
/// `propiedad: valor;` sin selector - asi que ambos casos (`parse_block`
/// arriba y `CssParser::parse_inline_style` mas abajo) reusan este bucle en
/// vez de duplicarlo.
fn parse_declaration_list<'i, 't>(input: &mut Parser<'i, 't>) -> HashMap<String, String> {
    let mut declarations = HashMap::new();

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

        declarations.insert(name.to_lowercase(), value);
    }

    declarations
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
                Ok(rule) => stylesheet.rules.push(rule),
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
        parse_declaration_list(&mut input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[test]
    fn at_rules_are_skipped_without_corrupting_following_rules() {
        let sheet = CssParser::parse("@media (max-width: 600px) { body { color: blue; } } h1 { color: red; }");
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector, "h1");
        assert_eq!(sheet.rules[0].declarations.get("color").map(String::as_str), Some("red"));
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
