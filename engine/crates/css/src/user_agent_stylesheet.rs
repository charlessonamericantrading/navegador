//! Hoja de estilos de agente de usuario: los valores por defecto que un
//! navegador real aplica ANTES de que el CSS del autor de la pagina toque
//! nada - margenes de `<h1>`-`<h6>`/`<p>`/`<body>`, tamaños de fuente de los
//! titulares, color de enlaces... Sin esto, cualquier pagina sin su propio
//! CSS se veia como texto plano sin estructura visual alguna, pese a que
//! NINGUNA pagina real depende de aportar sus propios estilos para estas
//! cosas basicas - se dan por sentadas en todo navegador real.
//!
//! Simplificaciones honestas, heredadas de sus consumidores reales:
//! - `margin` solo soporta un unico valor aplicado a los 4 lados
//!   (`resolve_margin` en `engine-layout`, ver su doc-comment) - el spec
//!   real da a estos elementos solo margen VERTICAL (arriba/abajo) con
//!   horizontal en cero; aqui el mismo valor tambien empuja los lados. Se
//!   declara asi a proposito en vez de fingir una precision que el motor
//!   de layout no tiene todavia.
//! - `font-weight`/`font-style`/`text-decoration` SI quedan en el
//!   `computed_style` resultante (eso es correcto segun el spec: son
//!   declaraciones reales de la cascada), pero `engine-gfx` todavia no las
//!   LEE al pintar (negrita/cursiva/subrayado - Fase 2.4, pendiente) -
//!   estan aqui porque son cascada real, no porque ya se vean en pantalla.
//! - Sin `list-style` (viñetas), sin sangria de listas, sin `display`
//!   propio por tag (`<li>` no es mas que otro bloque generico todavia,
//!   ver "layout inline" pendiente).

use crate::parser::CssParser;
use crate::stylesheet::StyleSheet;
use std::sync::OnceLock;

const USER_AGENT_CSS: &str = r#"
body { margin: 8px; }
h1 { font-size: 32px; margin: 21px; }
h2 { font-size: 24px; margin: 20px; }
h3 { font-size: 19px; margin: 19px; }
h4 { font-size: 16px; margin: 21px; }
h5 { font-size: 13px; margin: 22px; }
h6 { font-size: 11px; margin: 25px; }
p { margin: 16px; }
ul { margin: 16px; }
ol { margin: 16px; }
a { color: #0000ee; text-decoration: underline; }
b { font-weight: bold; }
strong { font-weight: bold; }
i { font-style: italic; }
em { font-style: italic; }
"#;

/// Devuelve la hoja de agente de usuario, parseada UNA sola vez con el
/// mismo `CssParser` que el CSS de cualquier pagina real - construirla de
/// nuevo en cada resolucion de estilo (`resolve_style` corre por cada nodo
/// del DOM, muchas veces por pagina) seria trabajo repetido para un
/// contenido que nunca cambia entre llamadas.
pub fn user_agent_stylesheet() -> &'static StyleSheet {
    static SHEET: OnceLock<StyleSheet> = OnceLock::new();
    SHEET.get_or_init(|| CssParser::parse(USER_AGENT_CSS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_without_errors_and_has_rules() {
        assert!(!user_agent_stylesheet().rules.is_empty());
    }

    #[test]
    fn is_cached_across_calls_instead_of_reparsed() {
        let a = user_agent_stylesheet() as *const StyleSheet;
        let b = user_agent_stylesheet() as *const StyleSheet;
        assert_eq!(a, b, "deberia devolver la misma instancia cacheada, no reparsear cada vez");
    }
}
