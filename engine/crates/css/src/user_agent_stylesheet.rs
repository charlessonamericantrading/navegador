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
//!   `computed_style` resultante Y SI se pintan de verdad (negrita/cursiva
//!   reales via `FontSet`, ver `engine-text::font` y la Fase 2.4 en
//!   ARCHITECTURE.md; subrayado real desde la Fase 29, ver
//!   `DisplayItem::Text::underline` en `engine-gfx::display_list`) - el
//!   subrayado por defecto de `<a>` es el caso que mas se nota.
//! - `<li>` declara `display: list-item` (Fase 40) - una vineta real se
//!   antepone a su contenido, ver `place_list_marker`/`list_marker_text`
//!   en `engine-layout::tree`. `list-style-type` (heredable, propio o
//!   del `<ul>`/`<ol>` ancestro) SI se lee: `disc`/`circle`/`square`
//!   (balas), `decimal`/`decimal-leading-zero`/`lower-roman`/
//!   `upper-roman`/`lower-alpha`/`upper-alpha` (numerados, con la formula
//!   real de sustraccion para romanos y numeracion bijective real para
//!   alfa - no una tabla de casos sueltos). `margin-left: 24px` le da
//!   hueco donde "colgar" (el marcador se pinta FUERA de su propia caja
//!   de contenido, en ese margen - equivalente honesto-minimo de
//!   `list-style-position: outside`, el valor inicial real). Sin
//!   `list-style-image`.
//! - `table`/`tr`/`td`/`th` (Fase 3.4) SI tienen su `display` real
//!   (`table`/`table-row`/`table-cell` - ver `flow_table_children` en
//!   `engine-layout::tree`), pero sin `thead`/`tbody`/`tfoot` con rol propio
//!   (son transparentes para el layout de tabla, ver
//!   `collect_table_rows`), sin `border-collapse`/`border-spacing`. `th`
//!   declara `font-weight: bold` y `text-align: center` (ambos se pintan,
//!   igual que `b`/`strong`), como un navegador real.
//! - `input`/`select`/`textarea` (Fase 11: controles de formulario, ver
//!   `BoxType::Replaced` en `engine-layout`) reciben aqui un TAMAÑO FIJO,
//!   no shrink-to-fit real (este motor no mide min/max-content en ningun
//!   sitio todavia, misma limitacion ya declarada para items flex/cajas
//!   fuera de flujo) - una aproximacion honesta al ancho por defecto real
//!   de un `<input>` sin CSS de autor (`size=20` del spec HTML), no el
//!   tamaño exacto que daria un navegador real. `input[type=hidden]` usa
//!   `display: none` (Fase 10.5, ya real) en vez de cualquier tamaño -
//!   asi es el comportamiento verdadero de un campo oculto. Sin
//!   `text-align`/aspecto nativo por plataforma (flechas de `<select>`,
//!   radios circulares reales - el `border-radius` de abajo es una
//!   aproximacion visual, no una forma geometrica distinta).
//! - `button` (y solo `button` - `input[type=submit/button/...]` sigue
//!   siendo `BoxType::Replaced`, con tamaño fijo, NO `Inline`) se trata
//!   como `span`/`a`/etc: se encoge a su contenido real en vez de un
//!   tamaño fijo, Y `padding`/`border` de elementos inline SI se
//!   resuelven en el layout (ver `place_inline_node`, caso
//!   `BoxType::Inline`) - un boton con `padding` se ve relleno de
//!   verdad, no con el texto pegado al borde. `margin` de un inline
//!   sigue sin resolverse (horizontal-only en el spec real, caso raro
//!   fuera de `button`).

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
li { display: list-item; margin-left: 24px; }
a { color: #0000ee; text-decoration: underline; }
b { font-weight: bold; }
strong { font-weight: bold; }
i { font-style: italic; }
em { font-style: italic; }
table { display: table; }
tr { display: table-row; }
td { display: table-cell; padding: 1px; }
th { display: table-cell; padding: 1px; font-weight: bold; text-align: center; }
input { width: 170px; height: 21px; border: 1px solid #767676; background-color: #ffffff; }
input[type="checkbox"], input[type="radio"] { width: 13px; height: 13px; border: 1px solid #767676; background-color: #ffffff; }
input[type="radio"] { border-radius: 7px; }
input[type="submit"], input[type="button"], input[type="reset"], input[type="image"] { background-color: #efefef; }
input[type="hidden"] { display: none; }
select { width: 170px; height: 21px; border: 1px solid #767676; background-color: #ffffff; }
textarea { width: 200px; height: 60px; border: 1px solid #767676; background-color: #ffffff; }
button { border: 1px solid #767676; background-color: #efefef; padding: 1px 6px; }
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
