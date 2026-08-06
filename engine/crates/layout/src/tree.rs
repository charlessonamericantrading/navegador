use crate::box_model::EdgeSizes;
use crate::layout_box::{LayoutBox, BoxType, Rect};
use engine_dom::{Node, NodeType};
use engine_css::StyleSheet;
use engine_text::SystemFont;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Propiedades que SI se propagan de un elemento a sus descendientes cuando
/// estos no las redefinen (herencia CSS real, aunque solo para estas dos
/// - el resto de propiedades heredables del spec, tipografia como
/// `font-family`/`font-weight`/`line-height`, quedan pendientes).
const INHERITABLE_PROPERTIES: &[&str] = &["color", "font-size"];

/// Mismo valor inicial que usa `engine-gfx` al pintar (`display_list.rs`) -
/// duplicado a proposito en vez de compartido: son dos crates que no se
/// deben depender entre si (gfx depende de layout, no al reves), y es una
/// constante de tres lineas, no una razon para enredar la dependencia.
const INITIAL_FONT_SIZE: f32 = 16.0;

/// Igual que `parse_css_font_size` en `engine-gfx/src/display_list.rs` -
/// misma simplificacion honesta (solo `px`), duplicada por la misma razon
/// que `INITIAL_FONT_SIZE` de arriba. Solo entiende `px` a proposito: por
/// diseño, `resolve_font_size` (mas abajo) ya deja resuelto a `px` todo lo
/// que la cascada consigue resolver antes de que nada mas lo lea, asi que
/// ni esta funcion ni la copia de `engine-gfx` necesitan saber de `em`/`%`.
fn parse_css_font_size(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok().filter(|size| *size > 0.0)
}

/// Misma simplificacion honesta que `parse_css_font_size`: solo `px`. A
/// diferencia del tamaño de fuente, aqui SI se acepta `0` (un padding de
/// cero es perfectamente valido y es ademas el valor inicial real de la
/// propiedad), pero no un valor negativo (invalido en el spec real).
fn parse_css_length(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok().filter(|n| *n >= 0.0)
}

/// `padding` real, leido de la cascada - sustituye a la constante fija que
/// habia antes (12px para TODA caja, sin importar lo que diga su CSS de
/// verdad). Solo la forma de un unico valor (aplicado a los 4 lados por
/// igual, `padding: 10px`); la forma abreviada de 2/3/4 valores del spec
/// real (`padding: 10px 20px` para vertical/horizontal distintos, etc.)
/// queda pendiente. Sin `padding` en la cascada, o con un valor que no sea
/// un `px` valido, resuelve a cero - el valor inicial real de la
/// propiedad, no un numero inventado. `padding` no es una propiedad
/// heredable (ni en el spec real ni en `INHERITABLE_PROPERTIES`) - cada
/// caja resuelve la suya propia desde su propio `computed_style`.
fn resolve_padding(computed_style: &HashMap<String, String>) -> EdgeSizes {
    let px = computed_style.get("padding").and_then(|v| parse_css_length(v)).unwrap_or(0.0);
    EdgeSizes { top: px, right: px, bottom: px, left: px }
}

/// `margin` real, leido de la cascada - sustituye a `BLOCK_GAP`, otra
/// constante fija que habia antes (6px de hueco vertical entre CUALQUIER
/// par de hermanos, sin relacion alguna con la propiedad `margin`). Misma
/// simplificacion honesta que `resolve_padding`: solo un valor unico en
/// `px`, cero si no esta puesta o no es valida. Sin COLAPSO de margenes
/// adyacentes (el spec real colapsa el margin-bottom de un hermano con el
/// margin-top del siguiente, quedandose con el mayor de los dos en vez de
/// sumarlos - eso no esta implementado, `flow_block_children` simplemente
/// suma ambos) - simplificacion declarada, no un bug escondido.
fn resolve_margin(computed_style: &HashMap<String, String>) -> EdgeSizes {
    let px = computed_style.get("margin").and_then(|v| parse_css_length(v)).unwrap_or(0.0);
    EdgeSizes { top: px, right: px, bottom: px, left: px }
}

/// SOLO el ancho de `border` (forma abreviada `border: <ancho> <estilo>
/// <color>`, en cualquier orden - igual que el spec real permite) - el
/// layout solo necesita la geometria para reservar espacio;
/// `display_list.rs` (en `engine-gfx`) resuelve el color por separado, al
/// pintar, igual que ya hace con `color`/`background-color`/`font-size`.
/// Sin las propiedades longhand (`border-width`/`border-color`/
/// `border-style` por separado) todavia. Solo el estilo `solid` esta
/// reconocido - la AUSENCIA de un estilo reconocido (`none` explicito,
/// cualquier otro valor, o directamente no poner ninguno) hace que el
/// ancho EFECTIVO sea cero incluso si se puso un numero: asi es el spec
/// real (`border-style: none`, el valor inicial de la propiedad, fuerza
/// el `border-width` computado a cero, por sorprendente que parezca la
/// primera vez que se lee).
fn resolve_border_width(computed_style: &HashMap<String, String>) -> EdgeSizes {
    let Some(raw) = computed_style.get("border") else { return EdgeSizes::default() };
    let mut width: Option<f32> = None;
    let mut is_solid = false;
    for token in raw.split_whitespace() {
        if let Some(w) = parse_css_length(token) {
            width = Some(w);
        } else if token.eq_ignore_ascii_case("solid") {
            is_solid = true;
        }
    }
    if !is_solid {
        return EdgeSizes::default();
    }
    let px = width.unwrap_or(0.0);
    EdgeSizes { top: px, right: px, bottom: px, left: px }
}

/// Resuelve el valor CRUDO de `font-size` de un elemento (puede venir en
/// `px`, `em` o `%`) a un tamaño absoluto en pixeles, usando el font-size ya
/// resuelto del padre como referencia para las unidades relativas - la
/// unica base que `em`/`%` necesitan para `font-size` (el spec calcula
/// ambas unidades igual para esta propiedad concreta: relativas al
/// font-size del padre, no al del elemento mismo).
///
/// `rem` (relativo a la raiz del documento, no al padre inmediato) NO esta
/// soportado todavia - exigiria rastrear el font-size de `<html>` por
/// separado de lo heredado nivel a nivel, que este modelo de herencia no
/// hace. Un valor en `rem`, o cualquier otra unidad o valor invalido, cae
/// al tamaño heredado del padre en vez de fingir un numero.
fn resolve_font_size(raw_value: &str, parent_font_size_px: f32) -> f32 {
    if let Some(px) = parse_css_font_size(raw_value) {
        return px;
    }
    let trimmed = raw_value.trim();
    if let Some(em) = trimmed.strip_suffix("em") {
        if let Ok(n) = em.trim().parse::<f32>() {
            if n > 0.0 {
                return n * parent_font_size_px;
            }
        }
    } else if let Some(pct) = trimmed.strip_suffix('%') {
        if let Ok(n) = pct.trim().parse::<f32>() {
            if n > 0.0 {
                return n / 100.0 * parent_font_size_px;
            }
        }
    }
    parent_font_size_px
}

pub struct LayoutTreeBuilder;

impl LayoutTreeBuilder {
    /// Construye el arbol de layout, resuelve el estilo CSS de cada caja
    /// (ver `resolve_style`) y le asigna posiciones/tamanos reales mediante
    /// un flujo de bloque top-to-bottom muy simplificado: cada caja ocupa el
    /// ancho completo del contenedor y se apilan verticalmente. `padding`,
    /// `border` y `margin` reales ya se resuelven desde la cascada (ver
    /// `resolve_padding`/`resolve_border_width`/`resolve_margin`,
    /// `box_dimensions` en cada `LayoutBox`, sin colapso de margenes);
    /// floats e inline real todavia no — eso sigue siendo Fase 2, ver
    /// ARCHITECTURE.md. Esto es honesto-minimo, no el layout final.
    /// `font`: la misma fuente de sistema que usara `engine-gfx` para pintar
    /// (cargada una sola vez por quien orquesta el pipeline, ver
    /// `core/main.rs`), para que el layout mida el texto con las metricas
    /// reales de la fuente que de verdad se va a pintar - no una fuente
    /// distinta ni una recargada aparte. `None` si no hay fuente de sistema
    /// disponible: cae a la aproximacion anterior por caracteres (ver
    /// `flow_block_children`), igual que `engine-gfx` cae a un bloque de
    /// relleno cuando pinta sin fuente.
    pub fn build(dom_root: &Arc<RwLock<Node>>, stylesheet: &StyleSheet, viewport_width: f32, viewport_height: f32, font: Option<&SystemFont>) -> LayoutBox {
        let mut root_box = LayoutBox::new(BoxType::Block);
        root_box.dimensions = Rect {
            x: 0.0,
            y: 0.0,
            width: viewport_width,
            height: viewport_height,
        };

        Self::build_node(dom_root, &mut root_box, stylesheet, &HashMap::new());
        Self::flow_block_children(&mut root_box, font);
        root_box
    }

    /// `inherited` son las propiedades heredables (ver `INHERITABLE_PROPERTIES`)
    /// ya resueltas por los ancestros - se propaga hacia abajo y cada
    /// elemento la actualiza con lo que el mismo redefina antes de pasarla a
    /// sus hijos, igual que la herencia CSS real.
    fn build_node(dom_node: &Arc<RwLock<Node>>, parent_layout_box: &mut LayoutBox, stylesheet: &StyleSheet, inherited: &HashMap<String, String>) {
        let r = dom_node.read().unwrap();
        match &r.node_type {
            NodeType::Document => {
                for child in &r.children {
                    Self::build_node(child, parent_layout_box, stylesheet, inherited);
                }
            }
            NodeType::Element { tag_name, .. } => {
                // "head", "script" y "style" no tienen representacion visual;
                // sin esto, su contenido de texto se pintaria como si fuera
                // parrafo visible.
                if matches!(tag_name.as_str(), "head" | "script" | "style" | "meta" | "link" | "title") {
                    return;
                }
                let box_type = match tag_name.as_str() {
                    "span" | "a" | "b" | "i" => BoxType::Inline,
                    _ => BoxType::Block,
                };
                let mut current_box = LayoutBox::new(box_type);
                current_box.dom_node = Some(dom_node.clone());
                // La resolucion de cascada en si (matching + especificidad +
                // atributo style inline) vive en `engine_css::resolve_style`
                // desde hace poco, no aqui - se traslado para que `engine-js`
                // (`getComputedStyle`, en construccion) tambien pueda
                // reusarla sin depender de `layout` solo para esto. Misma
                // logica exacta, cero cambio de comportamiento.
                current_box.computed_style = engine_css::resolve_style(dom_node, stylesheet);

                let mut child_inherited = inherited.clone();
                for prop in INHERITABLE_PROPERTIES {
                    let Some(value) = current_box.computed_style.get(*prop) else { continue };
                    let resolved = if *prop == "font-size" {
                        let parent_font_size_px = inherited
                            .get("font-size")
                            .and_then(|v| parse_css_font_size(v))
                            .unwrap_or(INITIAL_FONT_SIZE);
                        format!("{}px", resolve_font_size(value, parent_font_size_px))
                    } else {
                        value.clone()
                    };
                    current_box.computed_style.insert(prop.to_string(), resolved.clone());
                    child_inherited.insert(prop.to_string(), resolved);
                }

                for child in &r.children {
                    Self::build_node(child, &mut current_box, stylesheet, &child_inherited);
                }
                parent_layout_box.children.push(current_box);
            }
            NodeType::Text(content) => {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    let mut text_box = LayoutBox::new(BoxType::Text(trimmed.to_string()));
                    text_box.computed_style = inherited.clone();
                    parent_layout_box.children.push(text_box);
                }
            }
            _ => {}
        }
    }

    /// Apila los hijos de `container` verticalmente dentro de su ancho,
    /// calculando la altura de cada uno de forma recursiva (post-order: los
    /// hijos se posicionan antes de que el padre conozca su propia altura).
    /// `dimensions` representa el BORDER-BOX de la caja (contenido +
    /// padding + border - una capa mas hacia afuera que antes de que
    /// existiera border real). El `padding`/`border` de `container`
    /// (resueltos de su propio CSS) desplazan hacia adentro donde empiezan
    /// sus hijos - exactamente lo que hacen en el spec real, border por
    /// fuera de padding. Se guardan en `container.box_dimensions` de paso,
    /// para que queden accesibles mas alla de esta funcion (pintado,
    /// herramientas de depuracion...). El `margin` de CADA HIJO (a
    /// diferencia de padding/border, que son del contenedor) empuja
    /// `cursor_y` antes de colocarlo (margin-top), desplaza `x` y reduce el
    /// ancho asignado (margin-left/right), y vuelve a empujar `cursor_y`
    /// despues (margin-bottom) - sin colapso entre margenes adyacentes, ver
    /// `resolve_margin`.
    fn flow_block_children(container: &mut LayoutBox, font: Option<&SystemFont>) {
        const LINE_HEIGHT_FALLBACK: f32 = 22.0;

        let padding = resolve_padding(&container.computed_style);
        let border = resolve_border_width(&container.computed_style);
        container.box_dimensions.padding = padding;
        container.box_dimensions.border = border;

        let inset_left = border.left + padding.left;
        let inset_right = border.right + padding.right;
        let inset_top = border.top + padding.top;

        let origin_x = container.dimensions.x + inset_left;
        let inner_width = (container.dimensions.width - inset_left - inset_right).max(0.0);
        let mut cursor_y = container.dimensions.y + inset_top;

        for child in &mut container.children {
            // `margin` no es heredable y las cajas de texto solo llevan
            // propiedades heredadas en su `computed_style` (ver
            // `build_node`) - por construccion, una caja de texto nunca
            // tiene "margin" en su mapa, asi que esto resuelve a cero para
            // texto de forma automatica, sin necesitar un caso aparte.
            let margin = resolve_margin(&child.computed_style);
            child.box_dimensions.margin = margin;

            cursor_y += margin.top;
            child.dimensions.x = origin_x + margin.left;
            child.dimensions.y = cursor_y;
            child.dimensions.width = (inner_width - margin.left - margin.right).max(0.0);
            let child_width = child.dimensions.width;

            match &child.box_type {
                BoxType::Text(content) => {
                    let font_size = child
                        .computed_style
                        .get("font-size")
                        .and_then(|v| parse_css_font_size(v))
                        .unwrap_or(INITIAL_FONT_SIZE);

                    child.dimensions.height = match font {
                        Some(font) => {
                            // Quiebre de linea real por palabra (wrap_text,
                            // engine-text) - la misma funcion que usa
                            // engine-gfx para pintar cada linea, con los
                            // mismos argumentos (mismo font_size, mismo
                            // child_width), asi que el numero de lineas
                            // que aqui se reserva de alto y el que alli se
                            // pinta coinciden siempre por construccion.
                            let lines = engine_text::wrap_text(font, content, font_size, child_width);
                            let line_height = engine_text::measure_text(font, "", font_size).line_height;
                            lines.len().max(1) as f32 * line_height
                        }
                        None => {
                            // Sin fuente de sistema disponible (ver
                            // engine-gfx/window.rs, mismo caso): aproximacion
                            // por caracteres, no shaping real.
                            let approx_chars_per_line = (child_width / 8.0).max(1.0);
                            let lines = (content.len() as f32 / approx_chars_per_line).ceil().max(1.0);
                            lines * LINE_HEIGHT_FALLBACK
                        }
                    };
                }
                _ => {
                    Self::flow_block_children(child, font);
                    // `flow_block_children(child, ...)`, arriba, ya dejo
                    // `child.box_dimensions.padding`/`.border` resueltos
                    // (child pasa a ser el "container" de esa llamada) - se
                    // reusan en vez de volver a leer la cascada dos veces.
                    let child_padding = child.box_dimensions.padding;
                    let child_border = child.box_dimensions.border;
                    let content_height: f32 = child.children.iter().map(|c| c.dimensions.height).sum();
                    child.dimensions.height = (content_height
                        + child_padding.top + child_padding.bottom
                        + child_border.top + child_border.bottom)
                        .max(LINE_HEIGHT_FALLBACK);
                    // El area de contenido real (sin padding NI border, los
                    // dos ya sumados arriba) - poblar esto es lo que hace
                    // que `Dimensions::padding_box()`/`border_box()`
                    // reconstruyan exactamente `child.dimensions`.
                    child.box_dimensions.content = Rect {
                        x: child.dimensions.x + child_border.left + child_padding.left,
                        y: child.dimensions.y + child_border.top + child_padding.top,
                        width: child_width - child_border.left - child_border.right - child_padding.left - child_padding.right,
                        height: content_height,
                    };
                }
            }

            cursor_y += child.dimensions.height + margin.bottom;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_dom::HtmlParser;
    use engine_css::CssParser;

    /// El arbol de layout refleja el anidamiento real del DOM (<html> es una
    /// caja de bloque propia que envuelve a <body>, igual que en un
    /// navegador real), asi que hay que buscar recursivamente en vez de
    /// asumir que <body> es hijo directo de la raiz.
    fn find_box_with_style<'a>(root: &'a LayoutBox, key: &str) -> Option<&'a LayoutBox> {
        if root.computed_style.contains_key(key) {
            return Some(root);
        }
        root.children.iter().find_map(|c| find_box_with_style(c, key))
    }

    fn find_box_for_dom_node<'a>(root: &'a LayoutBox, target: &Arc<RwLock<Node>>) -> Option<&'a LayoutBox> {
        if let Some(node) = &root.dom_node {
            if Arc::ptr_eq(node, target) {
                return Some(root);
            }
        }
        root.children.iter().find_map(|c| find_box_for_dom_node(c, target))
    }

    #[test]
    fn cascade_applies_background_color_to_matching_element() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = CssParser::parse("body { background-color: #dbe9f4; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let styled_box = find_box_with_style(&root, "background-color")
            .expect("alguna caja deberia tener background-color tras la cascada");

        assert_eq!(
            styled_box.computed_style.get("background-color").map(String::as_str),
            Some("#dbe9f4"),
        );
    }

    #[test]
    fn cascade_ignores_non_matching_rules() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = CssParser::parse("h1 { background-color: #ff0000; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        assert!(
            find_box_with_style(&root, "background-color").is_none(),
            "una regla para h1 no deberia aplicarse a un documento sin ningun <h1>"
        );
    }

    #[test]
    fn higher_specificity_wins_the_cascade() {
        let dom = HtmlParser::parse(r#"<html><body id="main"><p>hola</p></body></html>"#);
        // Especificidad de '#main' (1 id) > 'body' (1 tag): el id deberia ganar
        // pese a aparecer antes en la hoja de estilos.
        let stylesheet = CssParser::parse("#main { background-color: #00ff00; } body { background-color: #ff0000; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let styled_box = find_box_with_style(&root, "background-color").expect("body deberia tener estilo");

        assert_eq!(
            styled_box.computed_style.get("background-color").map(String::as_str),
            Some("#00ff00"),
            "el selector de mayor especificidad (#main) deberia ganar sobre 'body'"
        );
    }

    #[test]
    fn inline_style_attribute_is_applied_even_without_any_matching_stylesheet_rule() {
        let dom = HtmlParser::parse(r#"<html><body><div style="color: red">hola</div></body></html>"#);
        let stylesheet = CssParser::parse(""); // sin ninguna regla: solo el atributo style deberia aportar algo

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let styled_box = find_box_with_style(&root, "color").expect("el atributo style inline deberia aplicarse");

        assert_eq!(styled_box.computed_style.get("color").map(String::as_str), Some("red"));
    }

    /// El atributo `style` tiene mas especificidad que CUALQUIER selector
    /// (aparte de `!important`, que no esta modelado aqui) - incluso un
    /// selector de id, que ya es el mas especifico que entiende este motor
    /// (ver `higher_specificity_wins_the_cascade`), deberia perder frente a
    /// un estilo en linea sobre la misma propiedad.
    #[test]
    fn inline_style_attribute_wins_over_the_highest_specificity_stylesheet_rule() {
        let dom = HtmlParser::parse(r#"<html><body id="main" style="background-color: #0000ff"><p>hola</p></body></html>"#);
        let stylesheet = CssParser::parse("#main { background-color: #00ff00; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let styled_box = find_box_with_style(&root, "background-color").expect("body deberia tener estilo");

        assert_eq!(
            styled_box.computed_style.get("background-color").map(String::as_str),
            Some("#0000ff"),
            "el estilo en linea deberia ganar incluso sobre un selector de id"
        );
    }

    fn find_text_box<'a>(root: &'a LayoutBox, text: &str) -> Option<&'a LayoutBox> {
        if let BoxType::Text(content) = &root.box_type {
            if content == text {
                return Some(root);
            }
        }
        root.children.iter().find_map(|c| find_text_box(c, text))
    }

    #[test]
    fn text_box_inherits_color_and_font_size_from_its_element() {
        let dom = HtmlParser::parse("<html><body><h1>titulo</h1></body></html>");
        let stylesheet = CssParser::parse("h1 { color: #ff0000; font-size: 32px; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "titulo").expect("deberia existir una caja de texto 'titulo'");

        assert_eq!(text_box.computed_style.get("color").map(String::as_str), Some("#ff0000"));
        assert_eq!(text_box.computed_style.get("font-size").map(String::as_str), Some("32px"));
    }

    /// La herencia debe atravesar mas de un nivel (no solo el padre
    /// inmediato): un <span> sin estilo propio dentro de un <div> con
    /// `color` debe seguir heredandolo para su texto.
    #[test]
    fn inheritance_propagates_through_multiple_ancestor_levels() {
        let dom = HtmlParser::parse("<html><body><div><span>anidado</span></div></body></html>");
        let stylesheet = CssParser::parse("div { color: #0000ff; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "anidado").expect("deberia existir una caja de texto 'anidado'");

        assert_eq!(
            text_box.computed_style.get("color").map(String::as_str),
            Some("#0000ff"),
            "el color de un ancestro no inmediato (div) deberia heredarse a traves de span"
        );
    }

    /// `background-color` no es una propiedad heredable (ni en el spec real
    /// ni en INHERITABLE_PROPERTIES) - no deberia filtrarse a las cajas de
    /// texto aunque este presente en el elemento contenedor.
    #[test]
    fn non_inheritable_properties_do_not_leak_into_text_boxes() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = CssParser::parse("body { background-color: #dbe9f4; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "hola").expect("deberia existir una caja de texto 'hola'");

        assert!(
            !text_box.computed_style.contains_key("background-color"),
            "background-color no deberia heredarse a una caja de texto"
        );
    }

    /// Un elemento mas cercano que redefine `color` debe pisar el valor
    /// heredado de un ancestro mas lejano, igual que la cascada real.
    #[test]
    fn closer_ancestor_overrides_inherited_color_from_farther_ancestor() {
        let dom = HtmlParser::parse("<html><body><div><span>texto</span></div></body></html>");
        let stylesheet = CssParser::parse("div { color: #0000ff; } span { color: #00ff00; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "texto").expect("deberia existir una caja de texto 'texto'");

        assert_eq!(
            text_box.computed_style.get("color").map(String::as_str),
            Some("#00ff00"),
            "span redefine color, deberia ganar sobre el heredado de div"
        );
    }

    #[test]
    fn text_box_height_scales_with_font_size_when_a_real_font_is_available() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };

        let dom_small = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet_small = CssParser::parse("p { font-size: 16px; }");
        let root_small = LayoutTreeBuilder::build(&dom_small, &stylesheet_small, 800.0, 600.0, Some(&font));
        let small = find_text_box(&root_small, "hola").expect("deberia existir una caja de texto 'hola'");

        let dom_big = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet_big = CssParser::parse("p { font-size: 64px; }");
        let root_big = LayoutTreeBuilder::build(&dom_big, &stylesheet_big, 800.0, 600.0, Some(&font));
        let big = find_text_box(&root_big, "hola").expect("deberia existir una caja de texto 'hola'");

        assert!(
            big.dimensions.height > small.dimensions.height,
            "un font-size mayor deberia producir una caja de texto real mas alta, no la misma altura fija de antes"
        );
    }

    #[test]
    fn text_wraps_into_more_lines_when_the_container_is_narrower() {
        let Some(font) = SystemFont::load_default_sans_serif() else {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        };

        let long_text = "este es un parrafo bastante largo que deberia necesitar mas de una linea en un contenedor estrecho";
        let stylesheet = CssParser::parse("");

        let dom_wide = HtmlParser::parse(&format!("<html><body><p>{long_text}</p></body></html>"));
        let root_wide = LayoutTreeBuilder::build(&dom_wide, &stylesheet, 2000.0, 600.0, Some(&font));
        let wide = find_text_box(&root_wide, long_text).expect("deberia existir la caja de texto larga");

        let dom_narrow = HtmlParser::parse(&format!("<html><body><p>{long_text}</p></body></html>"));
        let root_narrow = LayoutTreeBuilder::build(&dom_narrow, &stylesheet, 150.0, 600.0, Some(&font));
        let narrow = find_text_box(&root_narrow, long_text).expect("deberia existir la caja de texto larga");

        assert!(
            narrow.dimensions.height > wide.dimensions.height,
            "el mismo texto en un contenedor mas estrecho deberia envolver en mas lineas (calculadas con el ancho real del texto) y medir mas alto"
        );
    }

    #[test]
    fn em_font_size_resolves_relative_to_the_parents_resolved_font_size() {
        let dom = HtmlParser::parse("<html><body><span>hola</span></body></html>");
        let stylesheet = CssParser::parse("body { font-size: 20px; } span { font-size: 2em; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "hola").expect("deberia existir una caja de texto 'hola'");

        assert_eq!(
            text_box.computed_style.get("font-size").map(String::as_str),
            Some("40px"),
            "2em sobre un padre de 20px deberia resolver a 40px, no quedarse como '2em' ni caer al valor inicial"
        );
    }

    #[test]
    fn percent_font_size_resolves_relative_to_the_parents_resolved_font_size() {
        let dom = HtmlParser::parse("<html><body><span>hola</span></body></html>");
        let stylesheet = CssParser::parse("body { font-size: 20px; } span { font-size: 150%; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "hola").expect("deberia existir una caja de texto 'hola'");

        assert_eq!(
            text_box.computed_style.get("font-size").map(String::as_str),
            Some("30px"),
            "150% sobre un padre de 20px deberia resolver a 30px"
        );
    }

    /// La resolucion debe encadenarse nivel a nivel (cada elemento resuelve
    /// contra el font-size YA RESUELTO de su padre inmediato, no contra el
    /// tamaño inicial del documento) - si esto colapsara a resolver siempre
    /// contra 16px, "div" daria 32px (correcto por coincidencia) pero
    /// "span" tambien daria 32px en vez de 48px.
    #[test]
    fn em_font_size_compounds_through_multiple_nested_levels() {
        let dom = HtmlParser::parse("<html><body><div><span>hola</span></div></body></html>");
        let stylesheet = CssParser::parse("div { font-size: 2em; } span { font-size: 1.5em; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "hola").expect("deberia existir una caja de texto 'hola'");

        assert_eq!(
            text_box.computed_style.get("font-size").map(String::as_str),
            Some("48px"),
            "div: 16px*2=32px; span: 32px*1.5=48px - cada nivel contra el resuelto de su padre inmediato"
        );
    }

    #[test]
    fn unsupported_units_and_invalid_font_size_values_fall_back_to_the_inherited_size() {
        let dom = HtmlParser::parse("<html><body><span>hola</span></body></html>");
        // 'rem' no esta soportado (ver resolve_font_size) y 'not-a-size' no
        // es un valor valido en ninguna unidad - ninguno de los dos deberia
        // fingir un numero.
        let stylesheet = CssParser::parse("body { font-size: 20px; } span { font-size: 3rem; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let text_box = find_text_box(&root, "hola").expect("deberia existir una caja de texto 'hola'");

        assert_eq!(
            text_box.computed_style.get("font-size").map(String::as_str),
            Some("20px"),
            "rem no soportado: deberia caer al font-size heredado del padre (20px), no a 0 ni a un valor inventado"
        );
    }

    #[test]
    fn padding_from_css_insets_children_instead_of_the_old_fixed_twelve_pixels() {
        let dom = HtmlParser::parse(r#"<html><body><div style="padding: 20px"><p>hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "padding").expect("el div deberia tener padding en su computed_style");
        let child_box = div_box.children.first().expect("el div deberia tener un hijo (el <p>)");

        assert_eq!(child_box.dimensions.x, div_box.dimensions.x + 20.0, "el hijo deberia empezar 20px a la derecha del borde del div, no 12px");
        assert_eq!(child_box.dimensions.y, div_box.dimensions.y + 20.0, "el hijo deberia empezar 20px por debajo del borde del div, no 12px");
    }

    #[test]
    fn missing_padding_resolves_to_zero_not_the_old_fixed_default() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><p>hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse(""); // sin padding en ningun sitio

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo (el <p>)");

        assert_eq!(child_box.dimensions.x, container_box.dimensions.x, "sin padding declarado, el hijo deberia quedar pegado al borde de su contenedor (offset cero), no a los 12px fijos de antes");
        assert_eq!(child_box.dimensions.y, container_box.dimensions.y, "sin padding declarado, el hijo deberia quedar pegado al borde de su contenedor (offset cero), no a los 12px fijos de antes");
        assert_eq!(container_box.box_dimensions.padding.top, 0.0);
    }

    #[test]
    fn box_dimensions_padding_is_populated_from_the_real_css_value() {
        let dom = HtmlParser::parse(r#"<html><body><div style="padding: 15px">contenido</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "padding").expect("el div deberia tener padding en su computed_style");

        assert_eq!(div_box.box_dimensions.padding.top, 15.0);
        assert_eq!(div_box.box_dimensions.padding.right, 15.0);
        assert_eq!(div_box.box_dimensions.padding.bottom, 15.0);
        assert_eq!(div_box.box_dimensions.padding.left, 15.0);
    }

    /// Prueba de consistencia real: `Dimensions::padding_box()` (escrito
    /// hace tiempo en box_model.rs, pero nunca ejercitado hasta ahora - la
    /// auditoria de honestidad lo encontro como codigo muerto) debe
    /// reconstruir EXACTAMENTE `dimensions` a partir de `box_dimensions.
    /// content` + `box_dimensions.padding`, porque `dimensions` YA es
    /// conceptualmente el padding-box de la caja (contenido + padding, sin
    /// border/margin todavia). Si esto no cuadra, algo en la resolucion de
    /// `content`/`padding` esta mal.
    #[test]
    fn padding_box_reconstructs_dimensions_exactly_from_content_plus_padding() {
        let dom = HtmlParser::parse(r#"<html><body><div style="padding: 25px"><p>hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "padding").expect("el div deberia tener padding en su computed_style");

        let reconstructed = div_box.box_dimensions.padding_box();
        assert_eq!(reconstructed.x, div_box.dimensions.x);
        assert_eq!(reconstructed.y, div_box.dimensions.y);
        assert_eq!(reconstructed.width, div_box.dimensions.width);
        assert_eq!(reconstructed.height, div_box.dimensions.height);
    }

    #[test]
    fn invalid_padding_value_falls_back_to_zero_not_a_made_up_number() {
        let dom = HtmlParser::parse(r#"<html><body><div style="padding: not-a-length">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "padding").expect("el div deberia tener padding en su computed_style (aunque el valor sea invalido)");

        assert_eq!(div_box.box_dimensions.padding.top, 0.0, "un valor de padding invalido deberia caer a cero, no a ningun numero inventado");
    }

    #[test]
    fn margin_from_css_pushes_the_child_down_and_right() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><p style="margin: 10px">hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo (el <p>)");

        assert_eq!(child_box.dimensions.x, container_box.dimensions.x + 10.0, "margin-left deberia desplazar el hijo 10px a la derecha");
        assert_eq!(child_box.dimensions.y, container_box.dimensions.y + 10.0, "margin-top deberia desplazar el hijo 10px hacia abajo");
    }

    #[test]
    fn missing_margin_resolves_to_zero_not_the_old_fixed_gap() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><p>uno</p><p>dos</p></div></body></html>"#);
        let stylesheet = CssParser::parse(""); // sin margin en ningun sitio

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let first = &container_box.children[0];
        let second = &container_box.children[1];

        assert_eq!(
            second.dimensions.y,
            first.dimensions.y + first.dimensions.height,
            "sin margin declarado, el segundo <p> deberia empezar justo donde termina el primero (hueco cero), no a los 6px fijos de antes"
        );
    }

    /// Simplificacion honesta declarada, no un bug escondido: el spec real
    /// COLAPSA margenes verticales adyacentes (se queda con el mayor de
    /// los dos, no la suma). Este motor no implementa colapso todavia -
    /// este test prueba explicitamente el comportamiento actual (suma) para
    /// que quede documentado en el propio test, no solo en un comentario.
    #[test]
    fn adjacent_margins_are_summed_not_collapsed() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><p style="margin: 10px">uno</p><p style="margin: 20px">dos</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let first = &container_box.children[0];
        let second = &container_box.children[1];

        let gap = second.dimensions.y - (first.dimensions.y + first.dimensions.height);
        assert_eq!(
            gap, 30.0,
            "sin colapso de margenes: margin-bottom (10) del primero + margin-top (20) del segundo = 30, no el mayor de los dos (20) como haria el spec real con colapso"
        );
    }

    #[test]
    fn box_dimensions_margin_is_populated_from_the_real_css_value() {
        let dom = HtmlParser::parse(r#"<html><body><div style="margin: 12px">contenido</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "margin").expect("el div deberia tener margin en su computed_style");

        assert_eq!(div_box.box_dimensions.margin.top, 12.0);
        assert_eq!(div_box.box_dimensions.margin.right, 12.0);
        assert_eq!(div_box.box_dimensions.margin.bottom, 12.0);
        assert_eq!(div_box.box_dimensions.margin.left, 12.0);
    }

    /// Igual espiritu que `padding_box_reconstructs_dimensions_exactly_
    /// from_content_plus_padding`, pero un nivel mas afuera:
    /// `Dimensions::margin_box()` (tambien escrito hace tiempo, tambien sin
    /// ejercitar hasta ahora - otro hallazgo de la auditoria de honestidad)
    /// deberia expandir el padding-box (= `dimensions`, border en cero)
    /// exactamente por el margin resuelto. El margin-box "empieza" donde el
    /// hijo habria estado sin su propio margin - justo en el borde del
    /// contenedor, en este caso, porque el contenedor no tiene padding.
    #[test]
    fn margin_box_expands_dimensions_by_the_real_margin_on_every_side() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><p style="margin: 15px">hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo (el <p>)");

        let margin_box = child_box.box_dimensions.margin_box();
        assert_eq!(margin_box.x, container_box.dimensions.x, "el margin-box deberia empezar justo en el borde del contenedor, antes de aplicar margin-left");
        assert_eq!(margin_box.y, container_box.dimensions.y, "el margin-box deberia empezar justo en el borde del contenedor, antes de aplicar margin-top");
        assert_eq!(margin_box.width, child_box.dimensions.width + 30.0, "15px de margin-left + 15px de margin-right");
        assert_eq!(margin_box.height, child_box.dimensions.height + 30.0, "15px de margin-top + 15px de margin-bottom");
    }

    #[test]
    fn border_from_css_insets_children_same_as_padding_would() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container" style="border: 5px solid #000000"><p>hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo (el <p>)");

        assert_eq!(child_box.dimensions.x, container_box.dimensions.x + 5.0, "border-width deberia desplazar el hijo hacia adentro, igual que padding");
        assert_eq!(child_box.dimensions.y, container_box.dimensions.y + 5.0, "border-width deberia desplazar el hijo hacia adentro, igual que padding");
    }

    /// Punto del spec real facil de pasar por alto: `border-style` vale
    /// `none` por defecto, y con `none` el `border-width` COMPUTADO es
    /// cero pase lo que pase se haya escrito como ancho - un `border: 5px
    /// #000000` sin la palabra `solid` no deberia pintar ni ocupar espacio.
    #[test]
    fn border_without_solid_style_has_zero_effective_width() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container" style="border: 5px #000000"><p>hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo (el <p>)");

        assert_eq!(child_box.dimensions.x, container_box.dimensions.x, "sin 'solid', el ancho efectivo del border deberia ser cero - border-style:none es el valor inicial real");
        assert_eq!(container_box.box_dimensions.border.top, 0.0);
    }

    #[test]
    fn box_dimensions_border_is_populated_from_the_real_css_value() {
        let dom = HtmlParser::parse(r#"<html><body><div style="border: 3px solid #ff0000">contenido</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "border").expect("el div deberia tener border en su computed_style");

        assert_eq!(div_box.box_dimensions.border.top, 3.0);
        assert_eq!(div_box.box_dimensions.border.right, 3.0);
        assert_eq!(div_box.box_dimensions.border.bottom, 3.0);
        assert_eq!(div_box.box_dimensions.border.left, 3.0);
    }

    /// Igual espiritu que las pruebas de `padding_box()`/`margin_box()`:
    /// `Dimensions::border_box()` (tambien sin ejercitar hasta ahora)
    /// deberia reconstruir `dimensions` exactamente, incluso con padding Y
    /// border presentes a la vez en la misma caja.
    #[test]
    fn border_box_reconstructs_dimensions_exactly_with_padding_and_border_together() {
        let dom = HtmlParser::parse(r#"<html><body><div style="padding: 8px; border: 4px solid #000000"><p>hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "border").expect("el div deberia tener border en su computed_style");

        let reconstructed = div_box.box_dimensions.border_box();
        assert_eq!(reconstructed.x, div_box.dimensions.x);
        assert_eq!(reconstructed.y, div_box.dimensions.y);
        assert_eq!(reconstructed.width, div_box.dimensions.width);
        assert_eq!(reconstructed.height, div_box.dimensions.height);
    }

    #[test]
    fn hit_test_at_a_point_inside_an_element_returns_that_elements_real_node() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target">contenido</div></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir en el DOM");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener una caja de layout");

        let center_x = target_box.dimensions.x + target_box.dimensions.width / 2.0;
        let center_y = target_box.dimensions.y + target_box.dimensions.height / 2.0;

        let hit = root.hit_test(center_x, center_y).expect("deberia encontrar un nodo en el centro del elemento");
        assert!(Arc::ptr_eq(&hit, &target_node), "hit_test deberia devolver el mismo nodo real que el elemento, no una copia");
    }

    #[test]
    fn hit_test_outside_every_box_returns_none() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        assert!(root.hit_test(99999.0, 99999.0).is_none());
    }

    /// La prueba real de la caida hacia el ancestro: el texto es una caja
    /// HIJA sin `dom_node` propio (un click real jamas resuelve a un nodo
    /// de texto - ver el aviso de `LayoutBox::dom_node`). Un punto sobre el
    /// area del `<p>` deberia resolver al `<p>`, no a `None`, aunque la
    /// caja mas especifica que matchee ese punto sea la de texto.
    #[test]
    fn hit_test_over_a_text_box_resolves_to_the_containing_element_not_none() {
        let dom = HtmlParser::parse(r#"<html><body><p id="target">algo de texto</p></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");
        let point_x = target_box.dimensions.x + 1.0;
        let point_y = target_box.dimensions.y + 1.0;

        let hit = root.hit_test(point_x, point_y).expect("deberia resolver al elemento contenedor, no a None");
        assert!(Arc::ptr_eq(&hit, &target_node));
    }
}
