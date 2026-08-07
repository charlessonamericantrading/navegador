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

/// Colapsa cualquier RACHA de espacios en blanco (incluidos saltos de
/// linea/tabulaciones de la indentacion del HTML fuente) a un unico espacio
/// - el comportamiento real de `white-space: normal` (el valor inicial real
/// de esa propiedad), NO un simple recorte de bordes (`str::trim`): un
/// espacio inicial/final SIGNIFICATIVO (el que separa palabras de un
/// elemento vecino) se conserva como UN espacio, no se elimina por
/// completo. Un texto puramente en blanco colapsa a `" "` (no vacio) -
/// quien llama decide si eso cuenta como "sin contenido" con su propio
/// `.trim().is_empty()`.
fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(' ');
            }
            last_was_space = true;
        } else {
            result.push(ch);
            last_was_space = false;
        }
    }
    result
}

/// Resuelve el ancho BORDER-BOX final de una caja de bloque, a partir de
/// `width`/`max-width`/`min-width` (si estan puestas en la cascada) mas el
/// ancho que tendria por defecto (`auto_width` - "llenar el espacio
/// disponible", el unico comportamiento que existia antes de esta tarea).
///
/// `width`/`max-width`/`min-width` son CONTENT-box en el spec real (el
/// valor inicial de `box-sizing`), asi que se convierten a border-box
/// sumando el propio padding+border del elemento antes de aplicarlas - sin
/// esta conversion, un `width: 200px` con padding habria dado un
/// border-box MAS ESTRECHO que 200px, al reves de lo que hace cualquier
/// navegador real por defecto. `box-sizing: border-box` (donde `width` ya
/// seria border-box directamente) no esta soportado.
///
/// `max-width` se aplica ANTES que `min-width` - si ambas entran en
/// conflicto (un `max-width` menor que `min-width`), `min-width` gana,
/// igual que exige el spec real (`clamp(min, tentative, max)`, no al
/// reves).
fn resolve_block_width(computed_style: &HashMap<String, String>, auto_width: f32) -> f32 {
    let padding = resolve_padding(computed_style);
    let border = resolve_border_width(computed_style);
    let box_model_extra = padding.left + padding.right + border.left + border.right;

    let mut width = computed_style
        .get("width")
        .and_then(|v| parse_css_length(v))
        .map(|content_width| content_width + box_model_extra)
        .unwrap_or(auto_width);

    if let Some(max_content_width) = computed_style.get("max-width").and_then(|v| parse_css_length(v)) {
        width = width.min(max_content_width + box_model_extra);
    }
    if let Some(min_content_width) = computed_style.get("min-width").and_then(|v| parse_css_length(v)) {
        width = width.max(min_content_width + box_model_extra);
    }
    width.max(0.0)
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
                // `collapse_whitespace`, NO `content.trim()` a secas (como
                // era antes de esta tarea): un `.trim()` completo tambien
                // quita un espacio SIGNIFICATIVO al final de este nodo si
                // separaba palabras de un hermano siguiente - invisible
                // mientras cada nodo de texto/inline tenia su propia linea
                // (antes de la Fase 2.3), pero un bug real ahora que el
                // flujo inline los junta en la misma linea ("Text " antes
                // de un `<b>bold</b>` se quedaba en "Text", pegandose a
                // "bold" sin espacio: "Textbold"). `white-space: normal`
                // (el valor inicial real de esa propiedad) colapsa
                // cualquier RACHA de espacios en blanco a uno solo, sin
                // quitar los bordes por completo.
                let collapsed = collapse_whitespace(content);
                if !collapsed.trim().is_empty() {
                    let mut text_box = LayoutBox::new(BoxType::Text(collapsed));
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
    /// Devuelve el alto de CONTENIDO real que este contenedor termino
    /// ocupando (`cursor_y` final menos su propio content-top) - quien
    /// llama (el mismo `flow_block_children`, para SU padre) lo usa
    /// directamente en vez de volver a sumar alturas de hijos por su
    /// cuenta. Esto es lo que hace que el calculo sea correcto tanto para
    /// hijos de BLOQUE (que nunca se solapan verticalmente, `cursor_y`
    /// avanza monotono) como para RACHAS INLINE (donde varios hermanos SI
    /// comparten la misma linea/`y` - ver `flow_inline_run`): sumar
    /// `dimensions.height` por hijo, como se hacia antes, contaria la misma
    /// linea varias veces si dos fragmentos inline la comparten. Devolver
    /// el `cursor_y` final ya resuelve eso por construccion, sin necesitar
    /// un caso aparte para "hijos que se solapan".
    fn flow_block_children(container: &mut LayoutBox, font: Option<&SystemFont>) -> f32 {
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
        let content_top = container.dimensions.y + inset_top;
        let mut cursor_y = content_top;

        let mut i = 0;
        while i < container.children.len() {
            if Self::is_inline_level(&container.children[i]) {
                // Racha de hijos inline-level (texto y/o span/a/b/i)
                // consecutivos: fluyen juntos en la(s) misma(s) linea(s) en
                // vez de apilarse uno por uno - ver `flow_inline_run`.
                let run_end = container.children[i..]
                    .iter()
                    .position(|c| !Self::is_inline_level(c))
                    .map(|rel| i + rel)
                    .unwrap_or(container.children.len());
                cursor_y = Self::flow_inline_run(&mut container.children[i..run_end], origin_x, inner_width, cursor_y, font);
                i = run_end;
                continue;
            }

            let child = &mut container.children[i];
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
            // `width`/`max-width`/`min-width` (si estan puestas) sustituyen
            // o acotan el ancho "llenar el espacio disponible" que era el
            // unico comportamiento antes de esta tarea - ver
            // `resolve_block_width`.
            let auto_width = (inner_width - margin.left - margin.right).max(0.0);
            child.dimensions.width = resolve_block_width(&child.computed_style, auto_width);
            let child_width = child.dimensions.width;

            let content_height = Self::flow_block_children(child, font);
            // `flow_block_children(child, ...)`, arriba, ya dejo
            // `child.box_dimensions.padding`/`.border` resueltos (child
            // pasa a ser el "container" de esa llamada) - se reusan en vez
            // de volver a leer la cascada dos veces.
            let child_padding = child.box_dimensions.padding;
            let child_border = child.box_dimensions.border;
            // `height` (si esta puesta) sustituye la altura AUTO (la que
            // acaba de devolver la recursion) por el valor explicito del
            // autor - a diferencia del ancho auto, aqui NO se aplica el
            // minimo `LINE_HEIGHT_FALLBACK`: ese minimo es un heuristico
            // propio del motor para no colapsar una caja vacia a cero, no
            // una regla real del spec, y no deberia pisar un `height` que
            // el autor puso a proposito (aunque sea mas pequeño que el
            // contenido - el contenido simplemente desborda, sin recorte:
            // `overflow` no esta implementado todavia). Sin
            // `max-height`/`min-height` todavia (fuera del alcance de esta
            // tarea).
            let explicit_content_height = child.computed_style.get("height").and_then(|v| parse_css_length(v));
            let resolved_content_height = explicit_content_height.unwrap_or(content_height);
            child.dimensions.height = match explicit_content_height {
                Some(h) => h + child_padding.top + child_padding.bottom + child_border.top + child_border.bottom,
                None => (content_height + child_padding.top + child_padding.bottom + child_border.top + child_border.bottom).max(LINE_HEIGHT_FALLBACK),
            };
            // El area de contenido real (sin padding NI border, los dos ya
            // sumados arriba) - poblar esto es lo que hace que
            // `Dimensions::padding_box()`/`border_box()` reconstruyan
            // exactamente `child.dimensions`. Usa `resolved_content_height`
            // (no `content_height` a secas) para seguir siendo consistente
            // cuando `height` esta puesta explicitamente.
            child.box_dimensions.content = Rect {
                x: child.dimensions.x + child_border.left + child_padding.left,
                y: child.dimensions.y + child_border.top + child_padding.top,
                width: child_width - child_border.left - child_border.right - child_padding.left - child_padding.right,
                height: resolved_content_height,
            };

            cursor_y += child.dimensions.height + margin.bottom;
            i += 1;
        }

        (cursor_y - content_top).max(0.0)
    }

    fn is_inline_level(b: &LayoutBox) -> bool {
        matches!(b.box_type, BoxType::Text(_) | BoxType::Inline)
    }

    /// Coloca una RACHA de hijos inline-level (`BoxType::Text`/
    /// `BoxType::Inline` consecutivos, ya sea texto suelto o elementos como
    /// `span`/`a`/`b`/`i`) fluyendo horizontalmente en lineas reales, en vez
    /// de apilarlos verticalmente uno por uno como hacia este motor antes
    /// de esta tarea (la razon real del bug "cada `<b>`/`<a>` en su propia
    /// linea").
    ///
    /// Granularidad ATOMICA por hoja de texto, no palabra a palabra entre
    /// hermanos: cada hoja de texto se coloca ENTERA en la linea actual si
    /// cabe (una sola medicion via `measure_text`); si no cabe pero la
    /// linea actual ya tiene contenido, salta a una linea nueva; si ni
    /// siquiera cabe sola en una linea vacia, esa hoja consume el ANCHO
    /// COMPLETO del contenedor y envuelve internamente con el mismo
    /// `wrap_text` de siempre (igual que cualquier caja de texto de
    /// bloque) - el siguiente hermano SIEMPRE empieza en una linea nueva
    /// despues de eso, nunca continua a media linea (simplificacion
    /// declarada: el spec real permitiria que el siguiente inline
    /// continuara en la ultima linea parcial de un vecino que envolvio
    /// varias lineas; aqui no - caso raro en paginas reales).
    ///
    /// `line_height` se calcula UNA vez para TODA la racha (con el
    /// font-size de su primera hoja de texto) y se usa para todas sus
    /// lineas - el spec real usaria el maximo real de cada linea cuando el
    /// font-size varia dentro de ella; esta simplificacion asume tamaño
    /// uniforme, cierto para la inmensa mayoria de parrafos reales.
    ///
    /// Devuelve el `cursor_y` final (el tope de una linea nueva lista para
    /// lo que venga despues de la racha).
    fn flow_inline_run(nodes: &mut [LayoutBox], origin_x: f32, inner_width: f32, start_y: f32, font: Option<&SystemFont>) -> f32 {
        const LINE_HEIGHT_FALLBACK: f32 = 22.0;

        let line_height = match font {
            Some(f) => {
                let font_size = Self::first_leaf_font_size(nodes).unwrap_or(INITIAL_FONT_SIZE);
                engine_text::measure_text(f, "", font_size).line_height
            }
            None => LINE_HEIGHT_FALLBACK,
        };

        let mut cursor_x = origin_x;
        let mut cursor_y = start_y;
        for node in nodes.iter_mut() {
            Self::place_inline_node(node, origin_x, inner_width, line_height, &mut cursor_x, &mut cursor_y, font);
        }
        cursor_y + line_height
    }

    /// Busca el `font-size` de la primera hoja de TEXTO de la racha,
    /// atravesando elementos inline anidados (`<b>`, `<i>`...) - la base
    /// para el `line_height` COMPARTIDO de toda la racha (ver
    /// `flow_inline_run`). `None` si la racha no tiene ninguna hoja de
    /// texto real (p.ej. un `<span></span>` vacio suelto).
    fn first_leaf_font_size(nodes: &[LayoutBox]) -> Option<f32> {
        for node in nodes {
            match &node.box_type {
                BoxType::Text(_) => {
                    return Some(
                        node.computed_style
                            .get("font-size")
                            .and_then(|v| parse_css_font_size(v))
                            .unwrap_or(INITIAL_FONT_SIZE),
                    );
                }
                BoxType::Inline => {
                    if let Some(fs) = Self::first_leaf_font_size(&node.children) {
                        return Some(fs);
                    }
                }
                BoxType::Block => {}
            }
        }
        None
    }

    /// Coloca UN nodo inline-level (hoja de texto, o elemento inline cuyos
    /// hijos se recorren recursivamente con el MISMO cursor compartido) -
    /// ver `flow_inline_run` para la logica de ajuste de linea.
    fn place_inline_node(node: &mut LayoutBox, origin_x: f32, inner_width: f32, line_height: f32, cursor_x: &mut f32, cursor_y: &mut f32, font: Option<&SystemFont>) {
        match &node.box_type {
            BoxType::Text(content) => {
                let font_size = node
                    .computed_style
                    .get("font-size")
                    .and_then(|v| parse_css_font_size(v))
                    .unwrap_or(INITIAL_FONT_SIZE);

                let natural_width = match font {
                    Some(f) => engine_text::measure_text(f, content, font_size).width,
                    // Sin fuente de sistema (ver engine-gfx/window.rs, mismo
                    // caso): misma aproximacion por caracteres que el resto
                    // del motor sin fuente real.
                    None => content.len() as f32 * 8.0,
                };

                let mut remaining = origin_x + inner_width - *cursor_x;
                if natural_width > remaining && *cursor_x > origin_x {
                    // No cabe en lo que queda de la linea actual, pero la
                    // linea ya tiene contenido de un hermano anterior: salta
                    // a una linea nueva antes de decidir nada mas.
                    *cursor_y += line_height;
                    *cursor_x = origin_x;
                    remaining = inner_width;
                }

                if natural_width <= remaining {
                    node.dimensions = Rect { x: *cursor_x, y: *cursor_y, width: natural_width, height: line_height };
                    *cursor_x += natural_width;
                } else {
                    // Ni siquiera cabe sola en una linea vacia (cursor_x ==
                    // origin_x aqui siempre, por el salto de arriba):
                    // consume el ancho completo y envuelve internamente,
                    // igual que una caja de texto de bloque de siempre.
                    let lines = match font {
                        Some(f) => engine_text::wrap_text(f, content, font_size, inner_width),
                        None => vec![content.clone()],
                    };
                    let consumed_lines = lines.len().max(1) as f32;
                    node.dimensions = Rect { x: origin_x, y: *cursor_y, width: inner_width, height: consumed_lines * line_height };
                    *cursor_y += consumed_lines * line_height;
                    *cursor_x = origin_x;
                }
            }
            BoxType::Inline => {
                // `margin`/`padding`/`border` de elementos inline no se
                // resuelven todavia (fuera de alcance de esta tarea) - el
                // spec real solo les aplica margen/padding HORIZONTAL de
                // todas formas (el vertical no afecta el alto de linea), y
                // es un caso raro en paginas reales para span/a/b/i.
                let start_x = *cursor_x;
                let start_y = *cursor_y;
                for child in &mut node.children {
                    Self::place_inline_node(child, origin_x, inner_width, line_height, cursor_x, cursor_y, font);
                }
                // Rectangulo delimitador de todo lo que contuvo - honesto
                // solo para el caso comun (contenido que cabe en una sola
                // linea); si sus hijos terminaron repartidos en mas de una
                // linea, esto NO es geometricamente preciso (un elemento
                // inline partido en dos lineas es, en el spec real, DOS
                // fragmentos rectangulares, no uno) - simplificacion
                // declarada, suficiente para pintar/hit-testear el caso
                // comun.
                node.dimensions = Rect {
                    x: start_x,
                    y: start_y,
                    width: (*cursor_x - start_x).max(0.0),
                    height: (*cursor_y - start_y) + line_height,
                };
            }
            BoxType::Block => unreachable!("is_inline_level ya filtro los bloques antes de llegar aqui"),
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
        // `<div>` hijo, no `<p>`: `<p>` tiene su propio `margin` real desde
        // la hoja de agente de usuario, lo que desplazaria al hijo por una
        // razon ajena a lo que este test comprueba (el padding del padre).
        let dom = HtmlParser::parse(r#"<html><body><div style="padding: 20px"><div>hola</div></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let div_box = find_box_with_style(&root, "padding").expect("el div deberia tener padding en su computed_style");
        let child_box = div_box.children.first().expect("el div deberia tener un hijo");

        assert_eq!(child_box.dimensions.x, div_box.dimensions.x + 20.0, "el hijo deberia empezar 20px a la derecha del borde del div, no 12px");
        assert_eq!(child_box.dimensions.y, div_box.dimensions.y + 20.0, "el hijo deberia empezar 20px por debajo del borde del div, no 12px");
    }

    #[test]
    fn missing_padding_resolves_to_zero_not_the_old_fixed_default() {
        // `<div>` hijo, no `<p>`: `<p>` ya tiene margin real por la hoja de
        // agente de usuario, lo que desplazaria al hijo aunque el padding
        // siga en cero - el test quiere aislar solo el padding.
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><div>hola</div></div></body></html>"#);
        let stylesheet = CssParser::parse(""); // sin padding en ningun sitio

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo");

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
        // `<div>` hijos, no `<p>`: `<p>` ya tiene margin real por la hoja de
        // agente de usuario, que introduciria justo el hueco que este test
        // quiere comprobar que NO existe sin margin declarado.
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><div>uno</div><div>dos</div></div></body></html>"#);
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
        // Busqueda por id, no `find_box_with_style(&root, "margin")`: desde
        // la hoja de agente de usuario, `<body>` (un ANCESTRO del div en
        // todo este arbol) tambien tiene su propio `margin` real - la
        // primera caja con esa clave ya no seria necesariamente el div que
        // este test quiere comprobar.
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="margin: 12px">contenido</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let div_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja de layout");

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
        // `<div>` hijo, no `<p>`: `<p>` ya tiene margin real por la hoja de
        // agente de usuario, que desplazaria al hijo ademas del border.
        let dom = HtmlParser::parse(r#"<html><body><div id="container" style="border: 5px solid #000000"><div>hola</div></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo");

        assert_eq!(child_box.dimensions.x, container_box.dimensions.x + 5.0, "border-width deberia desplazar el hijo hacia adentro, igual que padding");
        assert_eq!(child_box.dimensions.y, container_box.dimensions.y + 5.0, "border-width deberia desplazar el hijo hacia adentro, igual que padding");
    }

    /// Punto del spec real facil de pasar por alto: `border-style` vale
    /// `none` por defecto, y con `none` el `border-width` COMPUTADO es
    /// cero pase lo que pase se haya escrito como ancho - un `border: 5px
    /// #000000` sin la palabra `solid` no deberia pintar ni ocupar espacio.
    #[test]
    fn border_without_solid_style_has_zero_effective_width() {
        // `<div>` hijo, no `<p>`: mismo motivo que el test anterior.
        let dom = HtmlParser::parse(r#"<html><body><div id="container" style="border: 5px #000000"><div>hola</div></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir en el DOM");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener una caja de layout");
        let child_box = container_box.children.first().expect("container deberia tener un hijo");

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

    /// Regresion real, encontrada al añadir la hoja de agente de usuario
    /// (`<body>` gana `margin: 8px` por defecto): el calculo de altura de
    /// un contenedor sumaba solo `dimensions.height` de cada hijo, sin su
    /// margin-top/margin-bottom - un contenedor con un hijo marginado se
    /// quedaba mas bajo de lo que su contenido realmente ocupaba, y el hijo
    /// se salia por debajo del propio contenedor sin que este lo supiera.
    /// Prueba directamente el sintoma: la caja de `<html>` debe crecer lo
    /// bastante para seguir conteniendo a `<body>` una vez desplazado por
    /// su propio margin, no solo por la altura "interna" de `<body>`.
    #[test]
    fn container_height_accounts_for_a_childs_own_margin_not_just_its_box() {
        let dom = HtmlParser::parse(r#"<html><body><div id="child" style="margin: 40px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let html_box = root.children.first().expect("root deberia tener un hijo (<html>)");
        let body_box = html_box.children.first().expect("html deberia tener un hijo (<body>)");
        let child_node = Node::find_by_id(&dom, "child").expect("child deberia existir");
        let child_box = find_box_for_dom_node(&root, &child_node).expect("child deberia tener caja");

        let child_bottom = child_box.dimensions.y + child_box.dimensions.height + child_box.box_dimensions.margin.bottom;
        let body_bottom = body_box.dimensions.y + body_box.dimensions.height;
        assert!(body_bottom >= child_bottom, "body (hasta {body_bottom}) deberia seguir conteniendo a su hijo marginado (hasta {child_bottom}), no quedarse corto");

        let html_bottom = html_box.dimensions.y + html_box.dimensions.height;
        assert!(html_bottom >= body_bottom, "html (hasta {html_bottom}) deberia seguir conteniendo a body (hasta {body_bottom})");
    }

    #[test]
    fn explicit_width_sets_the_border_box_including_padding_and_border() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="width: 200px; padding: 10px; border: 5px solid #000000">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        // width:200px es CONTENT-box (comportamiento por defecto real de
        // box-sizing) - el border-box final suma el padding y el border a
        // los dos lados: 200 + 10*2 + 5*2 = 230.
        assert_eq!(target_box.dimensions.width, 230.0, "el border-box deberia incluir el width de contenido mas padding y border a ambos lados");
    }

    #[test]
    fn without_an_explicit_width_the_box_still_fills_the_available_space() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        // 800 (viewport) - 16 (margin: 8px a cada lado de <body>, hoja de
        // agente de usuario) = 784, no 800 - el div llena el espacio
        // disponible DENTRO de body, no el viewport completo.
        assert_eq!(target_box.dimensions.width, 784.0, "sin width declarado, el comportamiento auto de siempre (llenar el ancho disponible del padre) deberia seguir intacto");
    }

    #[test]
    fn max_width_clamps_a_wider_explicit_width() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="width: 500px; max-width: 300px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.width, 300.0, "max-width deberia acotar un width explicito mayor");
    }

    #[test]
    fn max_width_clamps_the_auto_width_too() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="max-width: 250px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.width, 250.0, "max-width deberia acotar tambien el ancho automatico (llenar 800px), no solo un width explicito");
    }

    #[test]
    fn min_width_wins_over_a_smaller_max_width() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="width: 50px; max-width: 100px; min-width: 400px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.width, 400.0, "min-width deberia ganar sobre max-width si entran en conflicto, igual que el spec real (clamp(min, tentative, max))");
    }

    #[test]
    fn explicit_width_shrinks_the_space_available_to_its_own_children() {
        let dom = HtmlParser::parse(r#"<html><body><div id="parent" style="width: 300px"><div id="child">hola</div></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let child_node = Node::find_by_id(&dom, "child").expect("child deberia existir");
        let child_box = find_box_for_dom_node(&root, &child_node).expect("child deberia tener caja");

        assert_eq!(child_box.dimensions.width, 300.0, "el hijo deberia llenar el ancho del padre YA acotado por su width, no el ancho del viewport completo");
    }

    #[test]
    fn explicit_height_overrides_the_auto_computed_content_height() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="height: 400px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.height, 400.0, "height explicito deberia ganar al alto auto-calculado del contenido");
    }

    #[test]
    fn explicit_height_includes_padding_and_border_in_the_border_box() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="height: 100px; padding: 10px; border: 5px solid #000000">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        // height:100px es CONTENT-box, igual que width: 100 + 10*2 + 5*2 = 130.
        assert_eq!(target_box.dimensions.height, 130.0, "el border-box final deberia incluir el height de contenido mas padding y border arriba y abajo");
    }

    #[test]
    fn explicit_height_reconstructs_exactly_via_padding_box() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="height: 50px; padding: 8px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        let reconstructed = target_box.box_dimensions.padding_box();
        assert_eq!(reconstructed.height, target_box.dimensions.height, "box_dimensions.content.height debe seguir siendo consistente con dimensions.height cuando height es explicito, no solo en el caso auto");
    }

    /// El punto real de la Fase 2.3: un `<span>` (u otro elemento inline)
    /// deberia continuar en la MISMA linea que el texto anterior, no
    /// saltar a su propia linea como pasaba antes de esta tarea (cada
    /// hijo, fuera Text o Inline, avanzaba `cursor_y` por su cuenta).
    #[test]
    fn text_and_inline_element_share_the_same_line_and_continue_horizontally() {
        let dom = HtmlParser::parse(r#"<html><body><p>Text <span id="target">bold</span></p></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let text_box = find_text_box(&root, "Text ").expect("deberia existir una caja de texto 'Text '");
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(text_box.dimensions.y, target_box.dimensions.y, "el <span> deberia compartir la misma linea que el texto anterior, no saltar a la suya propia");
        assert_eq!(target_box.dimensions.x, text_box.dimensions.x + text_box.dimensions.width, "el <span> deberia continuar justo donde termina el texto anterior");
    }

    /// Varios elementos inline consecutivos (no solo texto+inline) tambien
    /// deberian compartir linea entre si.
    #[test]
    fn multiple_inline_elements_in_a_row_share_the_same_line() {
        let dom = HtmlParser::parse(r#"<html><body><p><b id="first">uno</b><i id="second">dos</i></p></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let first_node = Node::find_by_id(&dom, "first").expect("first deberia existir");
        let second_node = Node::find_by_id(&dom, "second").expect("second deberia existir");
        let first_box = find_box_for_dom_node(&root, &first_node).expect("first deberia tener caja");
        let second_box = find_box_for_dom_node(&root, &second_node).expect("second deberia tener caja");

        assert_eq!(first_box.dimensions.y, second_box.dimensions.y, "<b> e <i> consecutivos deberian compartir linea");
        assert_eq!(second_box.dimensions.x, first_box.dimensions.x + first_box.dimensions.width, "<i> deberia continuar justo donde termina <b>");
    }

    /// Cuando el contenido inline no cabe en lo que queda de la linea
    /// actual, debe saltar a una linea nueva - no desbordar horizontalmente
    /// mas alla del contenedor.
    #[test]
    fn inline_content_wraps_to_a_new_line_when_it_doesnt_fit() {
        // Sin fuente real (font: None), el ancho es determinista: 8px por
        // caracter (misma aproximacion que usa el resto del motor sin
        // fuente). "primera_palabra" (15 caracteres) = 120px, "segunda" (7
        // caracteres) = 56px - un contenedor de 150px de ancho deja sitio
        // de sobra para la primera pero no para las dos en la misma linea
        // (120+56=176 > 150).
        let dom = HtmlParser::parse(r#"<html><body><div id="container" style="width: 150px"><b id="first">primera_palabra</b><i id="second">segunda</i></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let first_text = find_text_box(&root, "primera_palabra").expect("deberia existir la caja de texto 'primera_palabra'");
        let second_text = find_text_box(&root, "segunda").expect("deberia existir la caja de texto 'segunda'");

        assert!(second_text.dimensions.y > first_text.dimensions.y, "el segundo elemento deberia saltar a una linea nueva al no caber junto al primero");
        assert_eq!(second_text.dimensions.x, first_text.dimensions.x, "tras saltar de linea, deberia volver al borde izquierdo del contenedor, no seguir desplazado");
    }

    /// Un hijo de BLOQUE despues de una racha inline no deberia compartir
    /// linea con ella - el limite de la racha se detecta correctamente al
    /// encontrar el primer hijo que ya no es inline-level.
    #[test]
    fn a_block_sibling_after_an_inline_run_starts_below_it_not_on_the_same_line() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><span id="inline_child">texto</span><div id="block_child">bloque</div></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let inline_node = Node::find_by_id(&dom, "inline_child").expect("inline_child deberia existir");
        let block_node = Node::find_by_id(&dom, "block_child").expect("block_child deberia existir");
        let inline_box = find_box_for_dom_node(&root, &inline_node).expect("inline_child deberia tener caja");
        let block_box = find_box_for_dom_node(&root, &block_node).expect("block_child deberia tener caja");

        assert!(block_box.dimensions.y >= inline_box.dimensions.y + inline_box.dimensions.height, "el hijo de bloque deberia empezar despues de que termine la racha inline, no compartir su linea");
    }

    /// La razon real del cambio de `flow_block_children` de sumar alturas a
    /// devolver el `cursor_y` final: varios fragmentos inline en la MISMA
    /// linea no deberian multiplicar la altura del contenedor - si se
    /// sumaran sus `dimensions.height` (todas iguales, una linea) por
    /// separado, un parrafo con 3 palabras cortas en una sola linea
    /// pareceria 3 lineas de alto.
    #[test]
    fn sibling_fragments_on_the_same_line_dont_inflate_the_containers_height() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><b id="one">a</b><i id="two">b</i><span id="three">c</span></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None);

        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener caja");

        let one_node = Node::find_by_id(&dom, "one").expect("one deberia existir");
        let one_box = find_box_for_dom_node(&root, &one_node).expect("one deberia tener caja");

        // Los 3 fragmentos comparten linea (caben de sobra), asi que el
        // contenedor deberia medir UNA sola linea de alto, no tres.
        assert_eq!(container_box.dimensions.height, one_box.dimensions.height, "3 fragmentos en la misma linea no deberian multiplicar la altura del contenedor");
    }
}
