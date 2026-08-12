use crate::box_model::EdgeSizes;
use crate::layout_box::{LayoutBox, BoxType, Rect};
use engine_dom::{Node, NodeType};
use engine_css::StyleSheet;
use engine_image::DecodedImage;
use engine_text::FontSet;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// `src` CRUDO (tal como aparece en `<img src="...">`, sin resolver contra
/// la URL de la pagina) -> imagen ya decodificada - mismo criterio que
/// `external_scripts: &HashMap<String, String>` en `core/pipeline.rs` para
/// `<script src>`: quien orquesta la red (`core/server.rs`) descubre los
/// `src`, los descarga y decodifica, y este mapa ya resuelto es lo unico
/// que `LayoutTreeBuilder` necesita - sin saber nada de red. Una imagen
/// ausente del mapa (`src` vacio, descarga fallida, formato no soportado)
/// simplemente no aporta dimension natural - ver `resolve_image_dimensions`.
pub type ImageMap = HashMap<String, Arc<DecodedImage>>;

/// Propiedades que SI se propagan de un elemento a sus descendientes cuando
/// estos no las redefinen (herencia CSS real). Ampliada en la Fase 2.5 a la
/// lista real de propiedades heredables del spec que tienen sentido para
/// este motor hoy - se excluyen a proposito las que son especificas de
/// tablas (`border-collapse`, `border-spacing`, `caption-side`,
/// `empty-cells` - el motor no tiene layout de tablas, Fase 3.4 pendiente)
/// y las de paginacion impresa (`orphans`/`widows` - un renderer de
/// pantalla sin paginacion no tiene "pagina" que romper).
///
/// Igual que ya pasaba con `font-weight`/`font-style` (Fase 2.4) antes de
/// que `engine-gfx` las pintara: que una propiedad este aqui significa que
/// la herencia CSS es correcta para ella (cascada real, verificable en
/// `computed_style`), NO que algo la lea todavia para layout/pintado -
/// varias de las nuevas (`text-align`, `list-style-type`, `letter-spacing`,
/// `white-space` mas alla de collapse de espacios, `visibility`...) no
/// tienen efecto visual todavia (Fase 3+). Documentado asi a proposito en
/// vez de fingir que ya se ven en pantalla.
///
/// Sin resolucion de unidades relativas para ninguna de las nuevas (a
/// diferencia de `font-size`, que SI convierte `em`/`%` via
/// `resolve_font_size` porque algo -el layout- ya consume ese valor
/// resuelto) - se propagan como el string crudo que declaro el autor, igual
/// que `color` siempre ha hecho.
const INHERITABLE_PROPERTIES: &[&str] = &[
    "color",
    "font-size",
    "font-weight",
    "font-style",
    "font-family",
    "font-variant",
    "line-height",
    "text-align",
    "text-indent",
    "text-transform",
    "letter-spacing",
    "word-spacing",
    "white-space",
    "visibility",
    "cursor",
    "direction",
    "list-style-type",
    "list-style-position",
    "list-style-image",
    "quotes",
];

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

/// Igual que `parse_css_length`, pero SI acepta negativos - a diferencia de
/// padding/border/width (que nunca son negativos en el spec real),
/// `top`/`right`/`bottom`/`left` (Fase 3.3, `position: relative/absolute/
/// fixed`) legitimamente pueden serlo (`top: -10px` es una forma comun de
/// desplazar un elemento hacia arriba de donde caeria en el flujo normal).
fn parse_css_offset(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok()
}

/// `position: absolute`/`fixed` (Fase 3.3) saca al elemento del flujo
/// normal por completo - ni reserva espacio ni afecta donde caen sus
/// hermanos, como si no estuviera ahi para efectos de layout de bloque/
/// inline/flex (se posiciona aparte, ver `resolve_positioned_boxes`).
/// `relative` NO cuenta como fuera de flujo (sigue reservando su espacio
/// normal, solo se desplaza visualmente despues - ver
/// `apply_relative_offset`); `static` (el valor inicial real) tampoco.
fn is_out_of_flow(computed_style: &HashMap<String, String>) -> bool {
    matches!(computed_style.get("position").map(String::as_str), Some("absolute") | Some("fixed"))
}

/// Desplaza `node.dimensions.x`/`.y` segun `top`/`right`/`bottom`/`left`
/// (Fase 3.3, `position: relative`) - se llama DESPUES de que `node` ya
/// ocupo su lugar normal en el flujo (sigue reservando su espacio de
/// siempre, esto es SOLO un desplazamiento visual, no afecta a los
/// hermanos) pero ANTES de posicionar a los HIJOS de `node`, para que todo
/// su subarbol herede el desplazamiento automaticamente - sus propias
/// coordenadas se calculan a partir de `node.dimensions.x`/`.y` YA
/// desplazadas, sin necesitar recorrer el subarbol por separado. `left`
/// gana sobre `right` si ambos estan puestos (`right` se ignora), mismo
/// criterio para `top`/`bottom` - asi resuelve un navegador real un caso
/// sobre-especificado. No-op si `position` no es `relative`.
fn apply_relative_offset(node: &mut LayoutBox) {
    if node.computed_style.get("position").map(String::as_str) != Some("relative") {
        return;
    }
    let dx = match (node.computed_style.get("left").and_then(|v| parse_css_offset(v)), node.computed_style.get("right").and_then(|v| parse_css_offset(v))) {
        (Some(l), _) => l,
        (None, Some(r)) => -r,
        (None, None) => 0.0,
    };
    let dy = match (node.computed_style.get("top").and_then(|v| parse_css_offset(v)), node.computed_style.get("bottom").and_then(|v| parse_css_offset(v))) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        (None, None) => 0.0,
    };
    node.dimensions.x += dx;
    node.dimensions.y += dy;
}

fn is_table_cell(b: &LayoutBox) -> bool {
    b.computed_style.get("display").map(String::as_str) == Some("table-cell")
}

/// Recoge, EN ORDEN DE DOCUMENTO, todas las cajas `display: table-row`
/// dentro de una `display: table` (Fase 3.4) - a CUALQUIER profundidad, no
/// solo hijos directos: una tabla real casi siempre envuelve sus filas en
/// `<thead>`/`<tbody>`/`<tfoot>` (o incluso un `<div>` mal formado), y este
/// motor no genera cajas anonimas de "grupo de filas" (`table-row-group`)
/// para darles un rol propio en el layout - en vez de eso, cualquier
/// contenedor que NO sea el propio `table`/una `table-row`/una `table-cell`
/// es transparente: se atraviesa buscando filas mas abajo, como si no
/// existiera para efectos de layout de tabla (sigue existiendo como caja de
/// bloque normal, solo no participa en el algoritmo de columnas).
///
/// La recursion se DETIENE en una `table-cell` (el contenido de una celda
/// no son filas de ESTA tabla) y en una `table` anidada (sus filas son de
/// ESA tabla, se resuelven aparte cuando `flow_block_children` recurse en
/// ella como cualquier otro hijo de bloque normal) - sin este corte, una
/// tabla dentro de una celda aplanaria sus filas con las de la tabla
/// exterior.
fn collect_table_rows(node: &mut LayoutBox) -> Vec<&mut LayoutBox> {
    let mut rows = Vec::new();
    for child in &mut node.children {
        let display = child.computed_style.get("display").map(String::as_str);
        if display == Some("table-row") {
            rows.push(child);
        } else if display != Some("table") && display != Some("table-cell") {
            rows.extend(collect_table_rows(child));
        }
    }
    rows
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

/// `font-weight` computado -> negrita si/no. Simplificacion binaria
/// deliberada: el spec real admite cualquier numero 1-1000 (con caras
/// intermedias reales en fuentes variables), pero `FontSet` (ver
/// `engine-text::font`) solo carga 4 combinaciones fijas por pagina, no una
/// por cada peso posible - negrita/normal es la distincion que de verdad
/// importa para el 99% de la web (`<b>`/`<strong>`/`font-weight: bold` o
/// numeros >= 600, que es donde los navegadores reales empiezan a preferir
/// una cara "bold" sobre la variante regular al hacer matching de fuente).
/// Sin la propiedad, o con un valor que no es ni palabra clave ni numero
/// valido, cae a "no negrita" (el valor inicial real de `font-weight` es
/// `normal`/400).
fn resolve_font_weight_is_bold(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("font-weight") else { return false };
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("bold") || trimmed.eq_ignore_ascii_case("bolder") {
        return true;
    }
    trimmed.parse::<u16>().map(|weight| weight >= 600).unwrap_or(false)
}

/// `font-style` computado -> cursiva si/no. `oblique` (una inclinacion
/// sintetica de la cara regular, distinta de `italic` en el spec real, que
/// tiene sus propios glifos dibujados a mano) se trata igual que `italic`
/// aqui a proposito: `FontSet` solo distingue cursiva-si/cursiva-no, no
/// tiene una tercera variante "inclinada sinteticamente" - la misma
/// simplificacion binaria que `resolve_font_weight_is_bold`, por la misma
/// razon.
fn resolve_font_style_is_italic(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("font-style") else { return false };
    let trimmed = raw.trim();
    trimmed.eq_ignore_ascii_case("italic") || trimmed.eq_ignore_ascii_case("oblique")
}

/// Los atributos HTML `width`/`height` de un `<img>` (numeros sin unidad,
/// pixeles - `<img width="200" height="100">`, con mucho el caso mas comun
/// en la web real, mas comun que `style="width: ..."`) SI participan en el
/// tamaño final, pero con MENOR prioridad que CSS - son un "hint" de
/// presentacion, no una regla de la cascada (asi es el spec real: son el
/// equivalente a una regla de agente de usuario de la especificidad mas
/// baja posible). Por eso solo se insertan en `computed_style` si CSS
/// (`resolve_style`, ya aplicado antes de llamar aqui) no puso `width`/
/// `height` por su cuenta - insertar sin mirar pisaria un
/// `style="width: 300px"` real con el atributo, al reves de como debe ser.
/// Un atributo ausente, vacio o que no parsea como numero simplemente no
/// aporta nada (sin fallback inventado).
fn apply_image_size_attributes(computed_style: &mut HashMap<String, String>, attributes: &HashMap<String, String>) {
    for (attr, prop) in [("width", "width"), ("height", "height")] {
        if computed_style.contains_key(prop) {
            continue;
        }
        if let Some(px) = attributes.get(attr).and_then(|v| v.trim().parse::<f32>().ok()).filter(|n| *n > 0.0) {
            computed_style.insert(prop.to_string(), format!("{px}px"));
        }
    }
}

/// Tamaño final de un `<img>`: si el autor puso AMBOS `width`/`height`
/// (CSS o el atributo HTML, ya fusionados en `computed_style` por
/// `apply_image_size_attributes`), se usan tal cual. Si solo puso UNO de
/// los dos y la imagen decodifico con exito, el otro se escala para
/// mantener la proporcion real de la imagen (igual que el spec real - un
/// `<img width="200">` de una foto 800x400 deberia medir 200x100, no
/// 200x400). Sin dimension natural (imagen sin decodificar - `src` vacio,
/// descarga fallida, formato no soportado), el resultado es SIEMPRE 0x0 sin
/// importar lo que diga `width`/`height`: sin icono de "imagen rota" ni el
/// tamaño de respaldo 300x150 que el spec real exige para un reemplazado
/// sin tamaño intrinseco - simplificacion declarada (Fase 3.1), ninguna
/// caja visible en vez de fingir un tamaño para contenido que no existe.
fn resolve_image_dimensions(explicit_width: Option<f32>, explicit_height: Option<f32>, natural: Option<(f32, f32)>) -> (f32, f32) {
    let Some((natural_width, natural_height)) = natural else { return (0.0, 0.0) };
    if natural_width <= 0.0 || natural_height <= 0.0 {
        return (0.0, 0.0);
    }
    match (explicit_width, explicit_height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * natural_height / natural_width),
        (None, Some(h)) => (h * natural_width / natural_height, h),
        (None, None) => (natural_width, natural_height),
    }
}

/// Traduce las propiedades CSS de un CONTENEDOR flex (`flex-direction`,
/// `justify-content`, `align-items`) al `taffy::Style` que taffy necesita -
/// puente honesto: cada valor de la cascada ya resuelto que el motor
/// entiende hoy se traduce 1-a-1; un valor no reconocido o ausente cae al
/// mismo valor inicial real del spec que usaria taffy por su cuenta
/// (`Style::default()` ya trae `flex_direction: Row`, y `justify_content`/
/// `align_items` en `None` significan "sin alinear", que taffy resuelve
/// como `Start` - el valor inicial real de ambas propiedades). Sin
/// `flex-wrap`, `align-content`, `row-gap`/`column-gap` todavia
/// (simplificaciones declaradas, ver el doc-comment de `flow_flex_children`).
fn flex_container_style(computed_style: &HashMap<String, String>) -> taffy::Style {
    let flex_direction = match computed_style.get("flex-direction").map(String::as_str) {
        Some("column") => taffy::FlexDirection::Column,
        Some("column-reverse") => taffy::FlexDirection::ColumnReverse,
        Some("row-reverse") => taffy::FlexDirection::RowReverse,
        _ => taffy::FlexDirection::Row,
    };
    let justify_content = match computed_style.get("justify-content").map(String::as_str) {
        Some("center") => Some(taffy::JustifyContent::CENTER),
        Some("flex-end") | Some("end") => Some(taffy::JustifyContent::FLEX_END),
        Some("space-between") => Some(taffy::JustifyContent::SPACE_BETWEEN),
        Some("space-around") => Some(taffy::JustifyContent::SPACE_AROUND),
        Some("space-evenly") => Some(taffy::JustifyContent::SPACE_EVENLY),
        Some("flex-start") | Some("start") => Some(taffy::JustifyContent::FLEX_START),
        _ => None,
    };
    let align_items = match computed_style.get("align-items").map(String::as_str) {
        Some("center") => Some(taffy::AlignItems::CENTER),
        Some("flex-end") | Some("end") => Some(taffy::AlignItems::FLEX_END),
        Some("flex-start") | Some("start") => Some(taffy::AlignItems::FLEX_START),
        Some("baseline") => Some(taffy::AlignItems::BASELINE),
        Some("stretch") => Some(taffy::AlignItems::STRETCH),
        _ => None,
    };
    taffy::Style {
        display: taffy::Display::Flex,
        flex_direction,
        justify_content,
        align_items,
        ..Default::default()
    }
}

/// Traduce las propiedades CSS de un ITEM flex (`flex-grow`/`flex-shrink`/
/// `flex-basis`, mas `width`/`height` si estan puestas) al `taffy::Style`
/// del nodo hoja correspondiente. Valores iniciales reales del spec cuando
/// la propiedad no esta puesta: `flex-grow: 0`, `flex-shrink: 1`,
/// `flex-basis: auto`.
fn flex_item_style(computed_style: &HashMap<String, String>) -> taffy::Style {
    let flex_grow = computed_style.get("flex-grow").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0);
    let flex_shrink = computed_style.get("flex-shrink").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0);
    let flex_basis: taffy::Dimension = computed_style
        .get("flex-basis")
        .and_then(|v| parse_css_length(v))
        .map(taffy::style_helpers::length)
        .unwrap_or_else(taffy::style_helpers::auto);
    let width: taffy::Dimension =
        computed_style.get("width").and_then(|v| parse_css_length(v)).map(taffy::style_helpers::length).unwrap_or_else(taffy::style_helpers::auto);
    let height: taffy::Dimension =
        computed_style.get("height").and_then(|v| parse_css_length(v)).map(taffy::style_helpers::length).unwrap_or_else(taffy::style_helpers::auto);
    taffy::Style {
        flex_grow,
        flex_shrink,
        flex_basis,
        size: taffy::geometry::Size { width, height },
        ..Default::default()
    }
}

/// Funcion de medida que `taffy` llama para saber cuanto espacio necesita
/// UN item flex - taffy puede llamarla varias veces con distintos
/// `known_dimensions`/`available_space` mientras resuelve el layout final
/// (pasadas ESPECULATIVAS, no la definitiva - esa es
/// `finalize_flex_item_children`, despues). Reusa el motor de medida real
/// que ya existe (`flow_block_children` para bloque/inline,
/// `resolve_image_dimensions` para `<img>`) en vez de que taffy tenga que
/// inventar su propio medidor de texto/imagenes - exactamente el patron
/// que `compute_layout_with_measure` espera.
fn measure_flex_item(
    child: &mut LayoutBox,
    known_dimensions: taffy::geometry::Size<Option<f32>>,
    available_space: taffy::geometry::Size<taffy::AvailableSpace>,
    font_set: Option<&FontSet>,
    images: &ImageMap,
) -> taffy::geometry::Size<f32> {
    if let BoxType::Image(src) = &child.box_type {
        let natural = images.get(src).map(|img| (img.width as f32, img.height as f32));
        let explicit_width = child.computed_style.get("width").and_then(|v| parse_css_length(v));
        let explicit_height = child.computed_style.get("height").and_then(|v| parse_css_length(v));
        let (width, height) = resolve_image_dimensions(explicit_width, explicit_height, natural);
        return taffy::geometry::Size {
            width: known_dimensions.width.unwrap_or(width),
            height: known_dimensions.height.unwrap_or(height),
        };
    }

    let width = known_dimensions.width.unwrap_or(match available_space.width {
        taffy::AvailableSpace::Definite(w) => w,
        // Sin ancho conocido ni disponible definido (min-content/max-content
        // especulativos): el motor no mide min/max-content real todavia
        // (ver el doc-comment de `flow_flex_children`) - cero es honesto
        // (mejor que fingir un ancho arbitrario) y taffy vuelve a preguntar
        // con un ancho definido antes del layout final de todas formas.
        _ => 0.0,
    });

    child.dimensions.x = 0.0;
    child.dimensions.y = 0.0;
    child.dimensions.width = width;
    let content_height = LayoutTreeBuilder::flow_block_children(child, font_set, images);
    let child_padding = child.box_dimensions.padding;
    let child_border = child.box_dimensions.border;
    let explicit_height = child.computed_style.get("height").and_then(|v| parse_css_length(v));
    let height = known_dimensions.height.unwrap_or_else(|| {
        let content_or_explicit = explicit_height.unwrap_or(content_height);
        content_or_explicit + child_padding.top + child_padding.bottom + child_border.top + child_border.bottom
    });

    taffy::geometry::Size { width, height }
}

/// Pasada FINAL y autoritativa: `child.dimensions` (x/y/width/height) ya
/// vienen resueltos por `taffy` (ver `flow_flex_children`) - aqui solo se
/// posicionan los NIETOS (hijos de este item flex) dentro de esa caja ya
/// fijada, reusando `flow_block_children` de siempre. El alto que esa
/// llamada devuelve se descarta a proposito: el alto de ESTE item ya lo
/// decidio taffy (puede ser distinto del contenido natural por
/// `align-items: stretch` o `flex-grow` en el eje transversal), no se
/// recalcula aqui.
fn finalize_flex_item_children(child: &mut LayoutBox, font_set: Option<&FontSet>, images: &ImageMap) {
    if matches!(child.box_type, BoxType::Image(_)) {
        return;
    }
    LayoutTreeBuilder::flow_block_children(child, font_set, images);
    let child_padding = child.box_dimensions.padding;
    let child_border = child.box_dimensions.border;
    child.box_dimensions.content = Rect {
        x: child.dimensions.x + child_border.left + child_padding.left,
        y: child.dimensions.y + child_border.top + child_padding.top,
        width: (child.dimensions.width - child_border.left - child_border.right - child_padding.left - child_padding.right).max(0.0),
        height: (child.dimensions.height - child_border.top - child_border.bottom - child_padding.top - child_padding.bottom).max(0.0),
    };
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
    /// `font_set`: las 4 variantes de peso/estilo de la MISMA fuente de
    /// sistema que usara `engine-gfx` para pintar (cargadas una sola vez
    /// por quien orquesta el pipeline, ver `core/main.rs`), para que el
    /// layout mida el texto con las metricas reales de la fuente que de
    /// verdad se va a pintar - no una fuente distinta ni una recargada
    /// aparte. `None` si no hay fuente de sistema disponible: cae a la
    /// aproximacion anterior por caracteres (ver `flow_block_children`),
    /// igual que `engine-gfx` cae a un bloque de relleno cuando pinta sin
    /// fuente.
    /// `images`: mapa de `src` crudo -> imagen ya decodificada (ver
    /// `ImageMap` arriba) - `&ImageMap::new()`/`&HashMap::new()` si no hay
    /// ninguna disponible (por ejemplo, `core/main.rs`, que no descarga
    /// recursos externos - ver su doc-comment).
    pub fn build(dom_root: &Arc<RwLock<Node>>, stylesheet: &StyleSheet, viewport_width: f32, viewport_height: f32, font_set: Option<&FontSet>, images: &ImageMap) -> LayoutBox {
        let mut root_box = LayoutBox::new(BoxType::Block);
        root_box.dimensions = Rect {
            x: 0.0,
            y: 0.0,
            width: viewport_width,
            height: viewport_height,
        };

        Self::build_node(dom_root, &mut root_box, stylesheet, &HashMap::new());
        Self::flow_block_children(&mut root_box, font_set, images);
        // Segunda pasada (Fase 3.3): `flow_block_children`/`flow_inline_run`/
        // `flow_flex_children`, arriba, ya dejaron cada `position: absolute`/
        // `fixed` SIN resolver a proposito (`is_out_of_flow`, ver esas
        // funciones) - se posicionan aparte aqui, ahora que el flujo normal
        // entero ya existe de verdad y hay "containing blocks" reales contra
        // los que resolverlos. Ver el doc-comment de `resolve_positioned_boxes`.
        let viewport = root_box.dimensions.clone();
        Self::resolve_positioned_boxes(&mut root_box, &viewport, &viewport, font_set, images);
        root_box
    }

    /// Recorre TODO el arbol (ya construido por `flow_block_children` en la
    /// primera pasada) buscando cajas `position: absolute`/`fixed` que
    /// quedaron sin resolver a proposito, y las posiciona contra su
    /// "containing block" real: la PADDING-BOX del ancestro mas cercano con
    /// `position` distinto de `static` (asi es el spec real), o el
    /// viewport entero si no hay ninguno - `fixed` SIEMPRE usa el viewport,
    /// ignorando cualquier ancestro posicionado (tambien real: `fixed` se
    /// ancla a la ventana, no al documento).
    ///
    /// Recursa por TODO el arbol, no solo los nodos de nivel superior: un
    /// `position: absolute` puede estar anidado a cualquier profundidad
    /// dentro de contenido que la primera pasada ya coloco con normalidad.
    fn resolve_positioned_boxes(node: &mut LayoutBox, containing_block: &Rect, viewport: &Rect, font_set: Option<&FontSet>, images: &ImageMap) {
        // Clonado (no `&str` prestado de `node.computed_style`) a proposito:
        // esta funcion necesita mutar `node` (`flow_block_children`,
        // `shift_subtree_y`) mientras `position` sigue en alcance mas abajo
        // (para decidir el containing block de los hijos) - un prestamo
        // vivo lo impediria (borrow checker real, no capricho de estilo).
        let position = node.computed_style.get("position").cloned();

        if matches!(position.as_deref(), Some("absolute") | Some("fixed")) {
            let reference = if position.as_deref() == Some("fixed") { viewport } else { containing_block };

            let left = node.computed_style.get("left").and_then(|v| parse_css_offset(v));
            let right = node.computed_style.get("right").and_then(|v| parse_css_offset(v));
            let top = node.computed_style.get("top").and_then(|v| parse_css_offset(v));
            let bottom = node.computed_style.get("bottom").and_then(|v| parse_css_offset(v));

            // Ancho: mismo criterio que el flujo normal (`resolve_block_width`,
            // ya existente) usando el ancho del containing block como "auto" -
            // simplificacion declarada: el spec real usaria shrink-to-fit
            // para un `width: auto` fuera de flujo, no "llenar el
            // contenedor"; el motor no mide shrink-to-fit todavia (mismo
            // hueco que `measure_flex_item`, ver su doc-comment).
            let width = resolve_block_width(&node.computed_style, reference.width);
            node.dimensions.width = width;
            node.dimensions.x = match (left, right) {
                (Some(l), _) => reference.x + l,
                (None, Some(r)) => reference.x + reference.width - width - r,
                (None, None) => reference.x,
            };
            // Y provisional: si `top` esta puesto, ya es el Y final (`bottom`
            // se ignora cuando ambos estan puestos - un caso sobre-
            // especificado que el spec real resuelve igual, descartando
            // `bottom`). Sin `top`, se coloca provisionalmente en el origen
            // del containing block hasta conocer el alto real de contenido
            // (mas abajo) y poder aplicar `bottom` correctamente.
            node.dimensions.y = match top {
                Some(t) => reference.y + t,
                None => reference.y,
            };

            let content_height = Self::flow_block_children(node, font_set, images);
            let node_padding = node.box_dimensions.padding;
            let node_border = node.box_dimensions.border;
            let explicit_height = node.computed_style.get("height").and_then(|v| parse_css_length(v));
            let resolved_height = explicit_height.unwrap_or(content_height) + node_padding.top + node_padding.bottom + node_border.top + node_border.bottom;
            node.dimensions.height = resolved_height;

            if top.is_none() {
                if let Some(b) = bottom {
                    let corrected_y = reference.y + reference.height - resolved_height - b;
                    let delta_y = corrected_y - node.dimensions.y;
                    if delta_y != 0.0 {
                        Self::shift_subtree_y(node, delta_y);
                    }
                }
            }
        }

        // El "containing block" para los DESCENDIENTES de `node` es su
        // propia padding-box si `node` mismo es `position: relative`/
        // `absolute`/`fixed` (asi es el spec real - un `relative` SIN
        // moverse ya establece containing block para hijos absolutos, no
        // hace falta que tenga `top`/`left` puestos); si no, se propaga el
        // mismo que ya traiamos.
        let next_containing_block =
            if matches!(position.as_deref(), Some("relative") | Some("absolute") | Some("fixed")) { node.box_dimensions.padding_box() } else { containing_block.clone() };

        for child in &mut node.children {
            Self::resolve_positioned_boxes(child, &next_containing_block, viewport, font_set, images);
        }
    }

    /// Desplaza `node.dimensions.y` Y TODO su subarbol (recursivamente) por
    /// `delta` - necesario cuando un `position: absolute`/`fixed` solo tiene
    /// `bottom` puesto (sin `top`): la Y final solo se conoce DESPUES de
    /// medir el alto real de contenido (ver `resolve_positioned_boxes`), asi
    /// que los hijos, ya posicionados por el `flow_block_children` de esa
    /// misma funcion con la Y provisional, quedan desfasados y hay que
    /// corregirlos - mas barato que volver a layoutear todo el subarbol
    /// desde cero con la Y ya correcta.
    fn shift_subtree_y(node: &mut LayoutBox, delta: f32) {
        node.dimensions.y += delta;
        node.box_dimensions.content.y += delta;
        for child in &mut node.children {
            Self::shift_subtree_y(child, delta);
        }
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
            NodeType::Element { tag_name, attributes } => {
                // "head", "script" y "style" no tienen representacion visual;
                // sin esto, su contenido de texto se pintaria como si fuera
                // parrafo visible.
                if matches!(tag_name.as_str(), "head" | "script" | "style" | "meta" | "link" | "title") {
                    return;
                }
                // `strong`/`em` faltaban aqui desde que existen como reglas
                // de la hoja de agente de usuario (Fase 2.1, `font-weight:
                // bold`/`font-style: italic`) - sin esto caian al `_ =>
                // BoxType::Block` de abajo, asi que un `<strong>`/`<em>`
                // mezclado con texto suelto (p.ej. "Titular <strong>fuerte
                // </strong> normal") rompia la racha inline en dos: el
                // texto de ANTES se quedaba solo en su propia linea, el
                // `<strong>` se apilaba debajo como si fuera un bloque
                // (un parrafo entero), y el texto de DESPUES empezaba una
                // tercera linea - encontrado en vivo al verificar la Fase
                // 2.4 (negrita/cursiva reales) con una pagina que de verdad
                // mezclaba `<strong>` con texto vecino, caso que ningun
                // test anterior de layout inline (Fase 2.3) cubria porque
                // todos usaban `<b>`/`<i>`, no `<strong>`/`<em>`.
                // `<img>` es "inline replaced element" en el spec real - se
                // resuelve aparte porque, a diferencia de span/a/b/i/strong/
                // em (que envuelven MAS marcado), un `<img>` es una hoja sin
                // hijos cuyo `BoxType` lleva su propio `src` (Fase 3.1).
                let box_type = match tag_name.as_str() {
                    "span" | "a" | "b" | "i" | "strong" | "em" => BoxType::Inline,
                    "img" => BoxType::Image(attributes.get("src").cloned().unwrap_or_default()),
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
                if tag_name == "img" {
                    apply_image_size_attributes(&mut current_box.computed_style, attributes);
                }

                // Herencia real EN LA CAJA DEL ELEMENTO (Fase 8). Hasta
                // aqui, `computed_style` de una caja de elemento solo
                // llevaba lo que ESE elemento declaraba; lo heredado
                // viajaba unicamente en `inherited` y solo aterrizaba en
                // las cajas de TEXTO (mas abajo). Bastaba para pintar -el
                // color/tamaño de letra solo hacen falta donde hay texto-
                // pero deja la caja del elemento diciendo una verdad a
                // medias, y `getComputedStyle` (que por definicion
                // devuelve el valor DESPUES de la herencia) no tenia de
                // donde sacar el valor: un `<div>` dentro de un `<body>`
                // con `color` reportaba "" en vez del color heredado.
                //
                // `or_insert_with`, nunca sobrescribir: lo que el elemento
                // declare el mismo gana siempre sobre lo heredado, que es
                // exactamente el orden de la cascada. Y tiene que ir ANTES
                // del bucle de abajo para que un `font-size: 2em` propio
                // siga sin resolver a estas alturas y lo resuelva ese
                // bucle contra el tamaño del padre.
                //
                // `inherited` solo contiene propiedades de
                // `INHERITABLE_PROPERTIES` (es quien lo construye), asi
                // que copiarlo entero no puede colar nada no heredable.
                for (prop, value) in inherited {
                    current_box.computed_style.entry(prop.clone()).or_insert_with(|| value.clone());
                }

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
    fn flow_block_children(container: &mut LayoutBox, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
        // `display: flex` desvia el contenedor entero a `flow_flex_children`
        // (Fase 3.2, via el crate `taffy` - ver ARCHITECTURE.md "Doctrina de
        // dependencias") ANTES de tocar nada del flujo de bloque normal: un
        // contenedor flex no apila a sus hijos verticalmente ni los agrupa
        // en rachas inline, taffy decide su posicion en los ejes principal/
        // cruzado.
        if container.computed_style.get("display").map(String::as_str) == Some("flex") {
            return Self::flow_flex_children(container, font_set, images);
        }
        // `display: table` (Fase 3.4) se desvia igual que `flex` arriba -
        // ver `flow_table_children` para el porque no es "otro flujo de
        // bloque mas".
        if container.computed_style.get("display").map(String::as_str) == Some("table") {
            return Self::flow_table_children(container, font_set, images);
        }

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
            // `position: absolute`/`fixed` (Fase 3.3) se saca del flujo por
            // completo aqui: no reserva espacio ni avanza `cursor_y`, como
            // si no estuviera - se posiciona aparte, ver
            // `resolve_positioned_boxes`, despues de que el flujo normal
            // entero ya este resuelto.
            if is_out_of_flow(&container.children[i].computed_style) {
                i += 1;
                continue;
            }
            if Self::is_inline_level(&container.children[i]) {
                // Racha de hijos inline-level (texto y/o span/a/b/i)
                // consecutivos: fluyen juntos en la(s) misma(s) linea(s) en
                // vez de apilarse uno por uno - ver `flow_inline_run`.
                let run_end = container.children[i..]
                    .iter()
                    .position(|c| !Self::is_inline_level(c))
                    .map(|rel| i + rel)
                    .unwrap_or(container.children.len());
                cursor_y = Self::flow_inline_run(&mut container.children[i..run_end], origin_x, inner_width, cursor_y, font_set, images);
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
            // `position: relative` (Fase 3.3): `child` YA ocupo su lugar
            // normal arriba (sigue reservando su espacio de siempre, esto
            // es solo un desplazamiento visual) - se aplica ANTES de
            // recursar en sus hijos para que hereden el desplazamiento
            // automaticamente.
            apply_relative_offset(child);

            let content_height = Self::flow_block_children(child, font_set, images);
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

    /// Layout real de `display: flex` (Fase 3.2) - EL ALGORITMO en si (como
    /// se reparte el espacio en los ejes principal/cruzado, `flex-grow`/
    /// `shrink`/`basis`, alineacion) lo resuelve `taffy`, no codigo propio -
    /// ver "Doctrina de dependencias" en ARCHITECTURE.md, entrada "Layout
    /// flex/grid", para la razon exacta. Lo que SI es codigo propio: el
    /// puente completo hacia/desde `taffy` en las 3 funciones de abajo
    /// (`flex_container_style`/`flex_item_style` traducen CSS ya resuelto a
    /// `taffy::Style`; `measure_flex_item` conecta el motor de texto/imagen
    /// YA existente como funcion de medida de `taffy` en vez de que taffy
    /// necesite inventar su propio medidor de contenido; el bucle final
    /// vuelca `taffy::Layout` de vuelta en `LayoutBox::dimensions`).
    ///
    /// Cada hijo DIRECTO del contenedor es un item flex (sin distincion de
    /// `BoxType` - un `<img>` o un `<div>` son items igual de validos); sus
    /// propios hijos (nietos de `container`) se posicionan aparte, DESPUES
    /// de que `taffy` decida el tamaño/posicion final de cada item, via
    /// `finalize_flex_item_children` - reutilizando el mismo
    /// `flow_block_children` de siempre, no una copia.
    ///
    /// Simplificaciones declaradas: sin `flex-wrap` (una sola linea
    /// siempre), sin contenido inline/texto suelto como item flex directo
    /// (el caso raro de texto suelto como hijo directo de un contenedor
    /// flex no se envuelve en un item anonimo, como exigiria el spec real -
    /// en la practica set ignora, no aparece), sin medicion real de
    /// min-content/max-content (un item sin ancho explicito mide su
    /// contenido al ancho DISPONIBLE completo, no al ancho minimo que
    /// evitaria partir palabras - aproximacion razonable para la mayoria de
    /// paginas reales, exacta cuando el item tiene su propio `width`).
    fn flow_flex_children(container: &mut LayoutBox, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
        let padding = resolve_padding(&container.computed_style);
        let border = resolve_border_width(&container.computed_style);
        container.box_dimensions.padding = padding;
        container.box_dimensions.border = border;

        let inset_left = border.left + padding.left;
        let inset_right = border.right + padding.right;
        let inset_top = border.top + padding.top;

        let origin_x = container.dimensions.x + inset_left;
        let origin_y = container.dimensions.y + inset_top;
        let inner_width = (container.dimensions.width - inset_left - inset_right).max(0.0);

        if container.children.is_empty() {
            return 0.0;
        }

        let mut taffy_tree: taffy::TaffyTree<usize> = taffy::TaffyTree::new();
        // `(indice ORIGINAL en container.children, NodeId de taffy)` - un
        // item `position: absolute`/`fixed` (Fase 3.3) se saca del algoritmo
        // de flex por completo (ni siquiera se crea su nodo hoja en taffy,
        // igual que el spec real: un item flex fuera de flujo no participa
        // en el reparto de espacio), asi que los indices ya NO son 1-a-1
        // con `container.children` - de ahi la tupla en vez de un `Vec`
        // simple.
        let mut child_node_ids: Vec<(usize, taffy::NodeId)> = Vec::with_capacity(container.children.len());
        for (index, child) in container.children.iter().enumerate() {
            if is_out_of_flow(&child.computed_style) {
                continue;
            }
            let style = flex_item_style(&child.computed_style);
            let node_id = taffy_tree
                .new_leaf_with_context(style, index)
                .expect("crear un nodo hoja de taffy no deberia fallar (sin limite de nodos alcanzado)");
            child_node_ids.push((index, node_id));
        }
        // El PROPIO tamaño del contenedor va en su `Style.size`, no solo en
        // el `available_space` de `compute_layout_with_measure` (que taffy
        // trata como un techo para sizing intrinseco/shrink-to-fit, no como
        // el ancho ya resuelto) - `resolve_block_width` (fuera de esta
        // funcion, en `flow_block_children`) YA dejo el ancho border-box
        // definitivo en `container.dimensions.width` antes de llegar aqui,
        // asi que se pasa tal cual en vez de dejar que taffy lo redescubra
        // encogiendo el contenedor al contenido (bug real encontrado en
        // vivo: sin esto, un `<div style="display:flex; width:500px">`
        // con un item `flex-grow:1` sin ancho propio se encogia a 100px en
        // vez de 500 - taffy sumaba solo el flex-basis de los items,
        // ignorando el ancho real del contenedor, porque nunca se le dijo).
        let explicit_container_height = container.computed_style.get("height").and_then(|v| parse_css_length(v));
        let mut root_style = flex_container_style(&container.computed_style);
        root_style.size.width = taffy::style_helpers::length(inner_width);
        if let Some(h) = explicit_container_height {
            root_style.size.height = taffy::style_helpers::length(h);
        }
        let flex_node_ids: Vec<taffy::NodeId> = child_node_ids.iter().map(|(_, id)| *id).collect();
        let root_id = taffy_tree
            .new_with_children(root_style, &flex_node_ids)
            .expect("crear el nodo contenedor de taffy no deberia fallar");

        // Alto explicito del CONTENEDOR (si lo hay): un contenedor flex sin
        // `height` propia crece para envolver su contenido (MaxContent);
        // uno con `height` fija le da a taffy un alto DEFINIDO, necesario
        // para que `align-items: stretch` (el valor inicial real de la
        // propiedad) tenga contra que estirar a sus items en flex-direction
        // row.
        let available_height = match explicit_container_height {
            Some(h) => taffy::AvailableSpace::Definite(h),
            None => taffy::AvailableSpace::MaxContent,
        };

        let children = &mut container.children;
        taffy_tree
            .compute_layout_with_measure(
                root_id,
                taffy::geometry::Size { width: taffy::AvailableSpace::Definite(inner_width), height: available_height },
                |known_dimensions, available_space, _node_id, node_context, _style| match node_context {
                    Some(&mut index) => measure_flex_item(&mut children[index], known_dimensions, available_space, font_set, images),
                    None => taffy::geometry::Size::ZERO,
                },
            )
            .expect("compute_layout_with_measure no deberia fallar con un arbol bien formado (sin ciclos, todos los nodos creados arriba)");

        // Con el layout ya resuelto por taffy, se copia cada item de vuelta
        // a `LayoutBox::dimensions` (coordenadas ABSOLUTAS, `origin_x`/
        // `origin_y` mas la posicion RELATIVA al contenedor que devuelve
        // taffy) y se posicionan sus propios hijos (los nietos de
        // `container`) en una pasada final autoritativa.
        let mut max_bottom = origin_y;
        for (index, node_id) in &child_node_ids {
            let layout = *taffy_tree.layout(*node_id).expect("layout deberia existir tras compute_layout_with_measure");
            let child = &mut container.children[*index];
            child.dimensions.x = origin_x + layout.location.x;
            child.dimensions.y = origin_y + layout.location.y;
            child.dimensions.width = layout.size.width;
            child.dimensions.height = layout.size.height;
            // `position: relative` (Fase 3.3) tambien aplica a items flex -
            // el item sigue participando en el algoritmo de flex con su
            // tamaño/posicion normal, solo se desplaza visualmente despues,
            // igual que un hijo de bloque normal (ver `flow_block_children`).
            apply_relative_offset(child);
            finalize_flex_item_children(child, font_set, images);
            max_bottom = max_bottom.max(child.dimensions.y + child.dimensions.height);
        }

        (max_bottom - origin_y).max(0.0)
    }

    /// Layout real de `display: table` (Fase 3.4) - a diferencia de flex
    /// (Fase 3.2, delegado a `taffy`), el algoritmo aqui SI es codigo propio:
    /// el layout de tablas no es del mismo orden de complejidad que flexbox/
    /// grid (ver "Doctrina de dependencias" en ARCHITECTURE.md - esa entrada
    /// justifica la excepcion de flex/grid precisamente porque SON
    /// complejos; una tabla de columnas iguales no lo es).
    ///
    /// Algoritmo (simplificado, "auto table layout" honesto-minimo):
    /// 1. Recoge las filas (`collect_table_rows` - atraviesa `thead`/`tbody`/
    ///    `tfoot` de forma transparente, ver su doc-comment).
    /// 2. Numero de columnas = el maximo de celdas (`display: table-cell`)
    ///    que tiene CUALQUIER fila - filas con menos celdas simplemente
    ///    dejan columnas de mas sin ocupar a la derecha.
    /// 3. TODAS las columnas miden lo mismo (`inner_width / column_count`) -
    ///    simplificacion declarada: el spec real (`auto` table layout)
    ///    reparte el ancho segun el contenido de cada columna
    ///    (min-content/max-content por celda); este motor no mide eso
    ///    todavia para NINGUN contexto (mismo hueco ya declarado en
    ///    `flow_flex_children`, un item flex sin `width` propio tampoco mide
    ///    su min-content real) - columnas iguales es la aproximacion mas
    ///    honesta disponible sin inventar un medidor de contenido nuevo.
    /// 4. Cada celda se layoutea (recursion normal via `flow_block_children`,
    ///    la celda pasa a ser "container" de sus propios hijos) al ancho de
    ///    su columna; el alto de la FILA es el maximo de sus celdas, y todas
    ///    las celdas de esa fila se estiran a ese alto (asi es el spec real:
    ///    `vertical-align` inicial es `baseline`, pero el efecto visible por
    ///    defecto es que las celdas de una fila comparten alto).
    ///
    /// Sin `colspan`/`rowspan`, sin `border-collapse`/`border-spacing`
    /// (cada celda pinta su propio `border` via el box model normal, sin
    /// fusionar bordes adyacentes), sin celdas fuera de flujo
    /// (`position: absolute` en una `td` participa en el reparto de
    /// columnas igual que cualquier otra, en vez de sacarse del algoritmo
    /// como hace `flow_block_children`/`flow_flex_children` con
    /// `is_out_of_flow` - caso raro en tablas reales).
    fn flow_table_children(container: &mut LayoutBox, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
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

        let rows = collect_table_rows(container);
        if rows.is_empty() {
            return 0.0;
        }

        let column_count = rows.iter().map(|row| row.children.iter().filter(|c| is_table_cell(c)).count()).max().unwrap_or(0).max(1);
        let column_width = inner_width / column_count as f32;

        let mut cursor_y = content_top;
        for row in rows {
            row.dimensions.x = origin_x;
            row.dimensions.y = cursor_y;
            row.dimensions.width = inner_width;

            let mut row_height: f32 = 0.0;
            for (col, cell) in row.children.iter_mut().filter(|c| is_table_cell(c)).enumerate() {
                cell.dimensions.x = origin_x + column_width * col as f32;
                cell.dimensions.y = cursor_y;
                cell.dimensions.width = column_width;
                apply_relative_offset(cell);

                let content_height = Self::flow_block_children(cell, font_set, images);
                // `height` explicita en la propia celda (mismo criterio que
                // `flow_block_children` ya aplica a cualquier caja de
                // bloque, ver su doc-comment) - sin esto, un `<td
                // style="height: 100px">` vacio se colapsaria a su alto de
                // contenido real en vez de respetar el valor que puso el
                // autor.
                let explicit_content_height = cell.computed_style.get("height").and_then(|v| parse_css_length(v));
                let resolved_content_height = explicit_content_height.unwrap_or(content_height);
                let cell_padding = cell.box_dimensions.padding;
                let cell_border = cell.box_dimensions.border;
                let total_height = resolved_content_height + cell_padding.top + cell_padding.bottom + cell_border.top + cell_border.bottom;
                cell.dimensions.height = total_height.max(0.0);
                cell.box_dimensions.content = Rect {
                    x: cell.dimensions.x + cell_border.left + cell_padding.left,
                    y: cell.dimensions.y + cell_border.top + cell_padding.top,
                    width: column_width - cell_border.left - cell_border.right - cell_padding.left - cell_padding.right,
                    height: resolved_content_height,
                };
                row_height = row_height.max(cell.dimensions.height);
            }

            // Segunda pasada corta: estira cada celda de la fila al alto
            // MAXIMO que acaba de calcularse arriba (no se conocia todavia
            // mientras se colocaba la primera celda) - el comportamiento
            // visible por defecto de cualquier tabla real.
            for cell in row.children.iter_mut().filter(|c| is_table_cell(c)) {
                cell.dimensions.height = row_height;
                let cell_padding = cell.box_dimensions.padding;
                let cell_border = cell.box_dimensions.border;
                cell.box_dimensions.content.height = row_height - cell_padding.top - cell_padding.bottom - cell_border.top - cell_border.bottom;
            }

            row.dimensions.height = row_height;
            cursor_y += row_height;
        }

        (cursor_y - content_top).max(0.0)
    }

    fn is_inline_level(b: &LayoutBox) -> bool {
        matches!(b.box_type, BoxType::Text(_) | BoxType::Inline | BoxType::Image(_))
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
    /// `text_line_height` se calcula UNA vez para TODA la racha (con el
    /// font-size de su primera hoja de texto) y es el alto que recibe cada
    /// caja de TEXTO, sea cual sea la linea en la que caiga - el spec real
    /// usaria el maximo real de cada linea cuando el font-size varia DENTRO
    /// del texto; esta simplificacion asume tamaño uniforme, cierto para la
    /// inmensa mayoria de parrafos reales. Lo que SI varia por linea es
    /// `line_extent` (ver mas abajo): una imagen (`BoxType::Image`) mas alta
    /// que `text_line_height` SI hace crecer el avance vertical de SU
    /// propia linea, para no solapar el contenido siguiente - sin esto, un
    /// `<img>` alto junto a texto pisaba la linea de abajo (encontrado en
    /// vivo al verificar la Fase 3.1 con una imagen real).
    ///
    /// Devuelve el `cursor_y` final (el tope de una linea nueva lista para
    /// lo que venga despues de la racha).
    fn flow_inline_run(nodes: &mut [LayoutBox], origin_x: f32, inner_width: f32, start_y: f32, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
        const LINE_HEIGHT_FALLBACK: f32 = 22.0;

        let text_line_height = match font_set {
            Some(set) => {
                let (font_size, bold, italic) = Self::first_leaf_font_info(nodes).unwrap_or((INITIAL_FONT_SIZE, false, false));
                match set.pick(bold, italic) {
                    Some(f) => engine_text::measure_text(f, "", font_size).line_height,
                    None => LINE_HEIGHT_FALLBACK,
                }
            }
            None => LINE_HEIGHT_FALLBACK,
        };

        let mut cursor_x = origin_x;
        let mut cursor_y = start_y;
        // Alto real de la linea EN CURSO - arranca en `text_line_height`
        // (una linea de solo texto) y crece si algo mas alto se coloca en
        // ella (una imagen); se usa para avanzar `cursor_y` de verdad al
        // saltar de linea o al terminar la racha, en vez de siempre
        // `text_line_height`.
        let mut line_extent = text_line_height;
        for node in nodes.iter_mut() {
            Self::place_inline_node(node, origin_x, inner_width, text_line_height, &mut line_extent, &mut cursor_x, &mut cursor_y, font_set, images);
        }
        cursor_y + line_extent
    }

    /// Busca `font-size`/`font-weight`/`font-style` de la primera hoja de
    /// TEXTO de la racha, atravesando elementos inline anidados (`<b>`,
    /// `<i>`...) - la base para el `line_height` COMPARTIDO de toda la
    /// racha (ver `flow_inline_run`). `None` si la racha no tiene ninguna
    /// hoja de texto real (p.ej. un `<span></span>` vacio suelto).
    fn first_leaf_font_info(nodes: &[LayoutBox]) -> Option<(f32, bool, bool)> {
        for node in nodes {
            match &node.box_type {
                BoxType::Text(_) => {
                    let font_size = node
                        .computed_style
                        .get("font-size")
                        .and_then(|v| parse_css_font_size(v))
                        .unwrap_or(INITIAL_FONT_SIZE);
                    let bold = resolve_font_weight_is_bold(&node.computed_style);
                    let italic = resolve_font_style_is_italic(&node.computed_style);
                    return Some((font_size, bold, italic));
                }
                BoxType::Inline => {
                    if let Some(info) = Self::first_leaf_font_info(&node.children) {
                        return Some(info);
                    }
                }
                BoxType::Block | BoxType::Image(_) => {}
            }
        }
        None
    }

    /// Coloca UN nodo inline-level (hoja de texto, o elemento inline cuyos
    /// hijos se recorren recursivamente con el MISMO cursor compartido) -
    /// ver `flow_inline_run` para la logica de ajuste de linea.
    fn place_inline_node(node: &mut LayoutBox, origin_x: f32, inner_width: f32, text_line_height: f32, line_extent: &mut f32, cursor_x: &mut f32, cursor_y: &mut f32, font_set: Option<&FontSet>, images: &ImageMap) {
        // Mismo criterio que en `flow_block_children`: `position: absolute`/
        // `fixed` (Fase 3.3) no consume espacio ni avanza el cursor de la
        // linea - `resolve_positioned_boxes` lo posiciona aparte despues.
        if is_out_of_flow(&node.computed_style) {
            return;
        }
        match &node.box_type {
            BoxType::Text(content) => {
                let font_size = node
                    .computed_style
                    .get("font-size")
                    .and_then(|v| parse_css_font_size(v))
                    .unwrap_or(INITIAL_FONT_SIZE);
                // Negrita/cursiva de ESTA hoja de texto (heredadas hasta
                // aqui via INHERITABLE_PROPERTIES desde el `<b>`/`<i>` que
                // la contenga, ver el doc-comment de esa constante) eligen
                // la variante real de `font_set` con la que se mide - la
                // misma que `engine-gfx` elegira despues para pintar (ver
                // `DisplayItem::Text::bold`/`.italic` en
                // `engine-gfx/src/display_list.rs`), para que medir y
                // pintar seleccionen exactamente la misma cara de fuente.
                let bold = resolve_font_weight_is_bold(&node.computed_style);
                let italic = resolve_font_style_is_italic(&node.computed_style);
                let font = font_set.and_then(|set| set.pick(bold, italic));

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
                    // a una linea nueva antes de decidir nada mas. Avanza
                    // por `line_extent` (el alto REAL de la linea que se
                    // deja atras, ya sea texto solo o con una imagen mas
                    // alta - ver `flow_inline_run`), no por
                    // `text_line_height` a secas.
                    *cursor_y += *line_extent;
                    *cursor_x = origin_x;
                    *line_extent = text_line_height;
                    remaining = inner_width;
                }

                if natural_width <= remaining {
                    node.dimensions = Rect { x: *cursor_x, y: *cursor_y, width: natural_width, height: text_line_height };
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
                    node.dimensions = Rect { x: origin_x, y: *cursor_y, width: inner_width, height: consumed_lines * text_line_height };
                    *cursor_y += consumed_lines * text_line_height;
                    *cursor_x = origin_x;
                    *line_extent = text_line_height;
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
                    Self::place_inline_node(child, origin_x, inner_width, text_line_height, line_extent, cursor_x, cursor_y, font_set, images);
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
                    height: (*cursor_y - start_y) + *line_extent,
                };
            }
            BoxType::Image(src) => {
                // Mismo criterio de salto de linea que `BoxType::Text`
                // arriba (no cabe en lo que queda de la linea actual, pero
                // la linea ya tiene contenido de un hermano) - sin el
                // "envuelve internamente" de texto, porque una imagen es
                // atomica, no se puede partir en trozos mas pequeños: si
                // ni siquiera cabe sola en una linea vacia, se coloca de
                // todas formas y desborda el contenedor (igual que un
                // navegador real con una imagen mas ancha que su
                // contenedor y sin `max-width`).
                let natural = images.get(src).map(|img| (img.width as f32, img.height as f32));
                let explicit_width = node.computed_style.get("width").and_then(|v| parse_css_length(v));
                let explicit_height = node.computed_style.get("height").and_then(|v| parse_css_length(v));
                let (width, height) = resolve_image_dimensions(explicit_width, explicit_height, natural);

                let remaining = origin_x + inner_width - *cursor_x;
                if width > remaining && *cursor_x > origin_x {
                    *cursor_y += *line_extent;
                    *cursor_x = origin_x;
                    *line_extent = text_line_height;
                }

                node.dimensions = Rect { x: *cursor_x, y: *cursor_y, width, height };
                *cursor_x += width;
                // Una imagen mas alta que el resto de la linea hace crecer
                // el avance vertical de ESTA linea (ver el doc-comment de
                // `flow_inline_run`) - sin esto, el texto/contenido
                // siguiente se solapaba con la parte de abajo de la imagen.
                *line_extent = line_extent.max(height);
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

    /// El punto real de la Fase 3.2: tres items de ancho fijo en un
    /// contenedor `display: flex` (row, el eje principal por defecto) se
    /// colocan uno al lado del otro, no apilados verticalmente como haria
    /// el flujo de bloque normal.
    #[test]
    fn flex_row_places_children_side_by_side() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="c" style="display: flex;"><div id="a" style="width: 50px;">a</div><div id="b" style="width: 60px;">b</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } #c div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(a_box.dimensions.x, 0.0);
        assert_eq!(a_box.dimensions.width, 50.0);
        assert_eq!(b_box.dimensions.x, 50.0, "b deberia empezar justo donde termina a (eje principal), no en su propia linea");
        assert_eq!(a_box.dimensions.y, b_box.dimensions.y, "ambos items deberian compartir la misma coordenada y (una sola fila)");
    }

    /// `flex-direction: column` cambia el eje principal a vertical - los
    /// items se apilan en Y en vez de en X.
    #[test]
    fn flex_column_stacks_children_vertically() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: flex; flex-direction: column;"><div id="a" style="height: 30px;">a</div><div id="b" style="height: 40px;">b</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(a_box.dimensions.y, 0.0);
        assert_eq!(a_box.dimensions.height, 30.0);
        assert_eq!(b_box.dimensions.y, 30.0, "b deberia empezar justo debajo de a (eje principal vertical)");
        assert_eq!(a_box.dimensions.x, b_box.dimensions.x, "ambos items deberian compartir la misma columna");
    }

    /// El punto real de `flex-grow`: reparte el espacio SOBRANTE del
    /// contenedor entre los items proporcionalmente a su valor - un item
    /// con `flex-grow: 1` y otro sin `flex-grow` (0 por defecto) se lleva
    /// TODO el espacio libre, no una parte fija.
    #[test]
    fn flex_grow_distributes_the_remaining_space() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: flex; width: 500px;"><div id="fixed" style="width: 100px;">fijo</div><div id="grow" style="flex-grow: 1;">crece</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let fixed_node = Node::find_by_id(&dom, "fixed").expect("fixed deberia existir");
        let grow_node = Node::find_by_id(&dom, "grow").expect("grow deberia existir");
        let fixed_box = find_box_for_dom_node(&root, &fixed_node).expect("fixed deberia tener caja");
        let grow_box = find_box_for_dom_node(&root, &grow_node).expect("grow deberia tener caja");

        assert_eq!(fixed_box.dimensions.width, 100.0, "el item sin flex-grow deberia quedarse en su ancho fijo");
        assert_eq!(grow_box.dimensions.width, 400.0, "el item con flex-grow: 1 deberia llevarse todo el espacio sobrante (500 - 100)");
    }

    /// `justify-content: center` centra los items en el eje principal
    /// cuando sobra espacio, en vez de dejarlos pegados al borde inicial
    /// (comportamiento por defecto, `flex-start`).
    #[test]
    fn justify_content_center_centers_items_on_the_main_axis() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: flex; justify-content: center; width: 400px;"><div id="item" style="width: 100px;">x</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let item_node = Node::find_by_id(&dom, "item").expect("item deberia existir");
        let item_box = find_box_for_dom_node(&root, &item_node).expect("item deberia tener caja");

        assert_eq!(item_box.dimensions.x, 150.0, "(400 - 100) / 2 = 150, el item deberia quedar centrado en el eje principal");
    }

    /// Un `<img>` como item flex (sin distincion de BoxType, ver el
    /// doc-comment de `flow_flex_children`) deberia medir su tamaño real -
    /// prueba que `measure_flex_item` conecta de verdad
    /// `resolve_image_dimensions`, no solo las cajas de bloque/texto.
    #[test]
    fn an_image_as_a_flex_item_measures_its_real_natural_size() {
        let dom = HtmlParser::parse(r#"<html><body><div style="display: flex;"><img id="photo" src="foto.png"></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let mut images = ImageMap::new();
        images.insert("foto.png".to_string(), Arc::new(engine_image::DecodedImage { width: 80, height: 40, rgba: vec![0u8; 80 * 40 * 4] }));

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &images);

        let photo_node = Node::find_by_id(&dom, "photo").expect("photo deberia existir");
        let photo_box = find_box_for_dom_node(&root, &photo_node).expect("photo deberia tener caja");

        assert_eq!(photo_box.dimensions.width, 80.0);
        assert_eq!(photo_box.dimensions.height, 40.0);
    }

    /// El punto real de `position: relative`: el elemento sigue ocupando
    /// EXACTAMENTE su lugar normal en el flujo (el hermano siguiente no se
    /// mueve ni un pixel), solo se desplaza VISUALMENTE por `top`/`left`.
    #[test]
    fn position_relative_offsets_visually_without_moving_the_next_sibling() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="a" style="position: relative; top: 10px; left: 20px; height: 30px;">a</div><div id="b" style="height: 10px;">b</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(a_box.dimensions.x, 20.0, "desplazado 20px por left");
        assert_eq!(a_box.dimensions.y, 10.0, "desplazado 10px por top (su lugar normal seria y=0)");
        assert_eq!(b_box.dimensions.y, 30.0, "b deberia caer justo donde a habria terminado SIN desplazarse (su alto normal, 30px) - el desplazamiento de a no debe afectarle");
    }

    /// `position: relative` en un descendiente de un elemento relative
    /// desplazado deberia heredar el desplazamiento del padre (su propia
    /// posicion se calcula a partir de `dimensions.x`/`.y` YA desplazados
    /// del padre).
    #[test]
    fn a_child_of_a_relatively_offset_parent_inherits_the_offset() {
        let dom = HtmlParser::parse(r#"<html><body><div style="position: relative; top: 50px;"><p id="child">hijo</p></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; } div, p { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let child_node = Node::find_by_id(&dom, "child").expect("child deberia existir");
        let child_box = find_box_for_dom_node(&root, &child_node).expect("child deberia tener caja");

        assert_eq!(child_box.dimensions.y, 50.0, "el hijo deberia heredar el desplazamiento de 50px de su padre relative");
    }

    /// El punto real de `position: absolute`: se saca del flujo POR
    /// COMPLETO - el hermano siguiente ocupa el espacio como si el
    /// elemento absoluto no existiera, y este se posiciona aparte contra
    /// su containing block (aqui, el viewport - sin ancestro `position`
    /// distinto de `static` de por medio).
    #[test]
    fn position_absolute_is_removed_from_flow_and_positioned_against_the_viewport() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="a" style="position: absolute; top: 100px; left: 200px; width: 50px; height: 50px;">a</div><div id="b" style="height: 10px;">b</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(a_box.dimensions.x, 200.0);
        assert_eq!(a_box.dimensions.y, 100.0);
        assert_eq!(b_box.dimensions.y, 0.0, "b deberia estar en y=0, como si el <div> absoluto no existiera en el flujo en absoluto");
    }

    /// El containing block real de un `position: absolute` es el ancestro
    /// mas cercano con `position` distinto de `static` - NO necesariamente
    /// el viewport si hay un `position: relative` de por medio.
    #[test]
    fn position_absolute_uses_the_nearest_positioned_ancestor_as_containing_block() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="position: relative; width: 300px; height: 300px;"><div id="child" style="position: absolute; top: 10px; left: 10px; width: 20px; height: 20px;">c</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 50px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let child_node = Node::find_by_id(&dom, "child").expect("child deberia existir");
        let child_box = find_box_for_dom_node(&root, &child_node).expect("child deberia tener caja");

        // El padre relative arranca en (50, 50) por el margin del body; el
        // hijo absoluto deberia anclarse a ESE padre, no al viewport (que
        // le daria x=10, y=10 en vez de 60, 60).
        assert_eq!(child_box.dimensions.x, 60.0, "10px del padre (en x=50) + 10px de left");
        assert_eq!(child_box.dimensions.y, 60.0, "10px del padre (en y=50) + 10px de top");
    }

    /// `position: fixed` SIEMPRE se ancla al viewport, incluso con un
    /// ancestro `position: relative` de por medio (a diferencia de
    /// `absolute`, que si lo usaria como containing block).
    #[test]
    fn position_fixed_always_anchors_to_the_viewport_ignoring_positioned_ancestors() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="position: relative; top: 200px; left: 200px;"><div id="child" style="position: fixed; top: 5px; left: 5px; width: 20px; height: 20px;">c</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let child_node = Node::find_by_id(&dom, "child").expect("child deberia existir");
        let child_box = find_box_for_dom_node(&root, &child_node).expect("child deberia tener caja");

        assert_eq!(child_box.dimensions.x, 5.0, "fixed deberia ignorar el padre relative desplazado y anclarse al viewport (x=0) + left");
        assert_eq!(child_box.dimensions.y, 5.0);
    }

    /// `bottom` sin `top`: la Y final solo se conoce tras medir el alto
    /// real de contenido - prueba que `shift_subtree_y` corrige tanto la
    /// caja como a sus propios hijos.
    #[test]
    fn position_absolute_with_only_bottom_anchors_to_the_bottom_edge() {
        let dom = HtmlParser::parse(r#"<html><body><div id="a" style="position: absolute; bottom: 50px; left: 0px; width: 100px; height: 80px;">a</div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");

        // Viewport 600 de alto, altura de la caja 80, bottom 50:
        // y = 600 - 80 - 50 = 470.
        assert_eq!(a_box.dimensions.y, 470.0);
    }

    #[test]
    fn cascade_applies_background_color_to_matching_element() {
        let dom = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet = CssParser::parse("body { background-color: #dbe9f4; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let styled_box = find_box_with_style(&root, "background-color").expect("body deberia tener estilo");

        assert_eq!(
            styled_box.computed_style.get("background-color").map(String::as_str),
            Some("#00ff00"),
            "el selector de mayor especificidad (#main) deberia ganar sobre 'body'"
        );
    }

    /// Regresion de la Fase 8: la caja de un ELEMENTO tiene que llevar
    /// tambien lo heredado, no solo lo que ese elemento declara. Antes de
    /// esa fase, lo heredado solo aterrizaba en las cajas de TEXTO - lo
    /// justo para pintar, pero deja `getComputedStyle` sin nada que
    /// devolver para el caso mas comun de todos (un `color` puesto en
    /// `body` y leido desde un `div`).
    #[test]
    fn an_element_box_carries_inherited_properties_not_just_its_own_declarations() {
        let dom = HtmlParser::parse(r#"<html><body><div id="hijo">texto</div></body></html>"#);
        let stylesheet = CssParser::parse("body { color: rgb(10, 20, 30); font-size: 20px }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let hijo = Node::find_by_id(&dom, "hijo").expect("el div deberia existir");
        let caja = root.find_box_for_node(&hijo).expect("el div deberia tener caja");

        assert_eq!(
            caja.computed_style.get("color").map(String::as_str),
            Some("rgb(10, 20, 30)"),
            "el div no declara color, pero lo hereda de body - su caja deberia decirlo"
        );
        assert_eq!(caja.computed_style.get("font-size").map(String::as_str), Some("20px"));
    }

    /// La otra mitad de la regla: lo propio SIEMPRE gana sobre lo
    /// heredado. Si el `or_insert` de `build_node` llegara a sobrescribir,
    /// la cascada quedaria del reves.
    #[test]
    fn an_element_own_declaration_wins_over_the_inherited_value() {
        let dom = HtmlParser::parse(r#"<html><body><div id="hijo">texto</div></body></html>"#);
        let stylesheet = CssParser::parse("body { color: red } #hijo { color: blue }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let hijo = Node::find_by_id(&dom, "hijo").expect("el div deberia existir");
        let caja = root.find_box_for_node(&hijo).expect("el div deberia tener caja");

        assert_eq!(caja.computed_style.get("color").map(String::as_str), Some("blue"));
    }

    /// Un `font-size` relativo propio tiene que seguir resolviendose
    /// contra el del PADRE - es decir, el valor heredado no puede haberlo
    /// pisado antes de que el bucle de `INHERITABLE_PROPERTIES` lo
    /// resuelva.
    #[test]
    fn an_own_relative_font_size_still_resolves_against_the_inherited_one() {
        let dom = HtmlParser::parse(r#"<html><body><div id="hijo">texto</div></body></html>"#);
        let stylesheet = CssParser::parse("body { font-size: 20px } #hijo { font-size: 2em }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let hijo = Node::find_by_id(&dom, "hijo").expect("el div deberia existir");
        let caja = root.find_box_for_node(&hijo).expect("el div deberia tener caja");

        assert_eq!(
            caja.computed_style.get("font-size").map(String::as_str),
            Some("40px"),
            "2em sobre 20px heredados son 40px; si sale 20px es que lo heredado piso al valor propio antes de resolverlo"
        );
    }

    #[test]
    fn inline_style_attribute_is_applied_even_without_any_matching_stylesheet_rule() {
        let dom = HtmlParser::parse(r#"<html><body><div style="color: red">hola</div></body></html>"#);
        let stylesheet = CssParser::parse(""); // sin ninguna regla: solo el atributo style deberia aportar algo

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "titulo").expect("deberia existir una caja de texto 'titulo'");

        assert_eq!(text_box.computed_style.get("color").map(String::as_str), Some("#ff0000"));
        assert_eq!(text_box.computed_style.get("font-size").map(String::as_str), Some("32px"));
    }

    /// El punto real de la Fase 2.4: `<b>` deja `font-weight: bold` en SU
    /// PROPIO `computed_style` (la cascada, via `user_agent_stylesheet.rs`,
    /// ya lo hacia desde la Fase 2.1) - pero sin `font-weight` en
    /// `INHERITABLE_PROPERTIES`, la caja de TEXTO hija (la que
    /// `place_inline_node` de verdad mide/pinta) nunca lo veia. Mismo caso
    /// para `<i>`/`font-style: italic`.
    #[test]
    fn text_box_inherits_font_weight_and_font_style_from_a_bold_italic_ancestor() {
        let dom = HtmlParser::parse("<html><body><b><i>fuerte</i></b></body></html>");
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "fuerte").expect("deberia existir una caja de texto 'fuerte'");

        assert_eq!(text_box.computed_style.get("font-weight").map(String::as_str), Some("bold"), "deberia heredar font-weight: bold del <b> ancestro");
        assert_eq!(text_box.computed_style.get("font-style").map(String::as_str), Some("italic"), "deberia heredar font-style: italic del <i> ancestro");
    }

    /// El punto real de la Fase 2.5: propiedades heredables del spec mas
    /// alla de las cuatro que ya cubrian color/tipografia basica -
    /// muestreo de un par de la lista nueva (`text-align`, `line-height`),
    /// no las 15 completas, para no duplicar exactamente la misma
    /// aserción quince veces - el mecanismo de propagacion (el bucle sobre
    /// `INHERITABLE_PROPERTIES` en `build_node`) es el mismo para todas.
    #[test]
    fn newly_inheritable_fase_2_5_properties_propagate_to_text_boxes() {
        let dom = HtmlParser::parse("<html><body><div><span>anidado</span></div></body></html>");
        let stylesheet = CssParser::parse("div { text-align: center; line-height: 1.5; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "anidado").expect("deberia existir una caja de texto 'anidado'");

        assert_eq!(text_box.computed_style.get("text-align").map(String::as_str), Some("center"), "text-align deberia heredarse, ahora que esta en INHERITABLE_PROPERTIES");
        assert_eq!(text_box.computed_style.get("line-height").map(String::as_str), Some("1.5"), "line-height deberia heredarse, sin resolver la unidad (se propaga el valor crudo)");
    }

    #[test]
    fn resolve_font_weight_is_bold_recognizes_keywords_and_heavy_numeric_weights() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("font-weight".to_string(), value.to_string()); m };

        assert!(resolve_font_weight_is_bold(&style("bold")));
        assert!(resolve_font_weight_is_bold(&style("bolder")));
        assert!(resolve_font_weight_is_bold(&style("700")));
        assert!(resolve_font_weight_is_bold(&style("600")));
        assert!(!resolve_font_weight_is_bold(&style("normal")));
        assert!(!resolve_font_weight_is_bold(&style("400")));
        assert!(!resolve_font_weight_is_bold(&style("500")));
        assert!(!resolve_font_weight_is_bold(&HashMap::new()), "sin font-weight en absoluto, el valor inicial real (normal/400) no es negrita");
    }

    #[test]
    fn resolve_font_style_is_italic_recognizes_italic_and_oblique() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("font-style".to_string(), value.to_string()); m };

        assert!(resolve_font_style_is_italic(&style("italic")));
        assert!(resolve_font_style_is_italic(&style("oblique")));
        assert!(!resolve_font_style_is_italic(&style("normal")));
        assert!(!resolve_font_style_is_italic(&HashMap::new()));
    }

    /// La herencia debe atravesar mas de un nivel (no solo el padre
    /// inmediato): un <span> sin estilo propio dentro de un <div> con
    /// `color` debe seguir heredandolo para su texto.
    #[test]
    fn inheritance_propagates_through_multiple_ancestor_levels() {
        let dom = HtmlParser::parse("<html><body><div><span>anidado</span></div></body></html>");
        let stylesheet = CssParser::parse("div { color: #0000ff; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "texto").expect("deberia existir una caja de texto 'texto'");

        assert_eq!(
            text_box.computed_style.get("color").map(String::as_str),
            Some("#00ff00"),
            "span redefine color, deberia ganar sobre el heredado de div"
        );
    }

    #[test]
    fn text_box_height_scales_with_font_size_when_a_real_font_is_available() {
        let font_set = FontSet::load_default_sans_serif();
        if font_set.pick(false, false).is_none() {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        }

        let dom_small = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet_small = CssParser::parse("p { font-size: 16px; }");
        let root_small = LayoutTreeBuilder::build(&dom_small, &stylesheet_small, 800.0, 600.0, Some(&font_set), &ImageMap::new());
        let small = find_text_box(&root_small, "hola").expect("deberia existir una caja de texto 'hola'");

        let dom_big = HtmlParser::parse("<html><body><p>hola</p></body></html>");
        let stylesheet_big = CssParser::parse("p { font-size: 64px; }");
        let root_big = LayoutTreeBuilder::build(&dom_big, &stylesheet_big, 800.0, 600.0, Some(&font_set), &ImageMap::new());
        let big = find_text_box(&root_big, "hola").expect("deberia existir una caja de texto 'hola'");

        assert!(
            big.dimensions.height > small.dimensions.height,
            "un font-size mayor deberia producir una caja de texto real mas alta, no la misma altura fija de antes"
        );
    }

    #[test]
    fn text_wraps_into_more_lines_when_the_container_is_narrower() {
        let font_set = FontSet::load_default_sans_serif();
        if font_set.pick(false, false).is_none() {
            eprintln!("sin fuentes de sistema en este entorno, test omitido");
            return;
        }

        let long_text = "este es un parrafo bastante largo que deberia necesitar mas de una linea en un contenedor estrecho";
        let stylesheet = CssParser::parse("");

        let dom_wide = HtmlParser::parse(&format!("<html><body><p>{long_text}</p></body></html>"));
        let root_wide = LayoutTreeBuilder::build(&dom_wide, &stylesheet, 2000.0, 600.0, Some(&font_set), &ImageMap::new());
        let wide = find_text_box(&root_wide, long_text).expect("deberia existir la caja de texto larga");

        let dom_narrow = HtmlParser::parse(&format!("<html><body><p>{long_text}</p></body></html>"));
        let root_narrow = LayoutTreeBuilder::build(&dom_narrow, &stylesheet, 150.0, 600.0, Some(&font_set), &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let div_box = find_box_with_style(&root, "padding").expect("el div deberia tener padding en su computed_style (aunque el valor sea invalido)");

        assert_eq!(div_box.box_dimensions.padding.top, 0.0, "un valor de padding invalido deberia caer a cero, no a ningun numero inventado");
    }

    #[test]
    fn margin_from_css_pushes_the_child_down_and_right() {
        let dom = HtmlParser::parse(r#"<html><body><div id="container"><p style="margin: 10px">hola</p></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.width, 300.0, "max-width deberia acotar un width explicito mayor");
    }

    #[test]
    fn max_width_clamps_the_auto_width_too() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="max-width: 250px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.width, 250.0, "max-width deberia acotar tambien el ancho automatico (llenar 800px), no solo un width explicito");
    }

    #[test]
    fn min_width_wins_over_a_smaller_max_width() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="width: 50px; max-width: 100px; min-width: 400px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.width, 400.0, "min-width deberia ganar sobre max-width si entran en conflicto, igual que el spec real (clamp(min, tentative, max))");
    }

    #[test]
    fn explicit_width_shrinks_the_space_available_to_its_own_children() {
        let dom = HtmlParser::parse(r#"<html><body><div id="parent" style="width: 300px"><div id="child">hola</div></div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let child_node = Node::find_by_id(&dom, "child").expect("child deberia existir");
        let child_box = find_box_for_dom_node(&root, &child_node).expect("child deberia tener caja");

        assert_eq!(child_box.dimensions.width, 300.0, "el hijo deberia llenar el ancho del padre YA acotado por su width, no el ancho del viewport completo");
    }

    #[test]
    fn explicit_height_overrides_the_auto_computed_content_height() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="height: 400px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(target_box.dimensions.height, 400.0, "height explicito deberia ganar al alto auto-calculado del contenido");
    }

    #[test]
    fn explicit_height_includes_padding_and_border_in_the_border_box() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="height: 100px; padding: 10px; border: 5px solid #000000">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        // height:100px es CONTENT-box, igual que width: 100 + 10*2 + 5*2 = 130.
        assert_eq!(target_box.dimensions.height, 130.0, "el border-box final deberia incluir el height de contenido mas padding y border arriba y abajo");
    }

    #[test]
    fn explicit_height_reconstructs_exactly_via_padding_box() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target" style="height: 50px; padding: 8px">hola</div></body></html>"#);
        let stylesheet = CssParser::parse("");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let text_box = find_text_box(&root, "Text ").expect("deberia existir una caja de texto 'Text '");
        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target deberia tener caja");

        assert_eq!(text_box.dimensions.y, target_box.dimensions.y, "el <span> deberia compartir la misma linea que el texto anterior, no saltar a la suya propia");
        assert_eq!(target_box.dimensions.x, text_box.dimensions.x + text_box.dimensions.width, "el <span> deberia continuar justo donde termina el texto anterior");
    }

    /// Regresion encontrada en vivo al verificar la Fase 2.4: `<strong>`/
    /// `<em>` faltaban en la lista de tags inline de `build_node` (solo
    /// tenia `span`/`a`/`b`/`i`), asi que caian a `BoxType::Block` y
    /// rompian la racha inline en dos - el texto antes del `<strong>` se
    /// quedaba solo en su linea, `<strong>` se apilaba debajo como un
    /// bloque entero, y el texto de despues empezaba una tercera linea.
    /// Mismo caso que `text_and_inline_element_share_the_same_line...` de
    /// arriba, pero con `strong` en vez de `span` - el punto exacto que esa
    /// prueba no cubria.
    #[test]
    fn strong_and_em_are_inline_level_like_b_and_i() {
        let dom = HtmlParser::parse(r#"<html><body><p>Texto <strong id="s">fuerte</strong> <em id="e">enfasis</em></p></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let text_box = find_text_box(&root, "Texto ").expect("deberia existir una caja de texto 'Texto '");
        let strong_node = Node::find_by_id(&dom, "s").expect("s deberia existir");
        let em_node = Node::find_by_id(&dom, "e").expect("e deberia existir");
        let strong_box = find_box_for_dom_node(&root, &strong_node).expect("strong deberia tener caja");
        let em_box = find_box_for_dom_node(&root, &em_node).expect("em deberia tener caja");

        assert_eq!(text_box.dimensions.y, strong_box.dimensions.y, "<strong> deberia compartir linea con el texto anterior, no caer a BoxType::Block");
        assert_eq!(strong_box.dimensions.y, em_box.dimensions.y, "<em> deberia seguir en la misma linea que <strong>");
    }

    /// Regresion encontrada en vivo al verificar la Fase 3.1 con una imagen
    /// real: una `<img>` mas alta que la linea de texto en la que cae
    /// (el caso comun - casi cualquier foto es mucho mas alta que una
    /// linea de texto de 16px) empujaba el `<p>` siguiente hacia arriba lo
    /// bastante como para solaparse con la propia imagen, porque
    /// `flow_inline_run` avanzaba `cursor_y` por el alto FIJO del texto,
    /// ignorando que la imagen de esa misma linea era mas alta. Arreglado
    /// con `line_extent` (ver su doc-comment en `flow_inline_run`).
    #[test]
    fn a_tall_image_grows_the_line_so_the_next_block_does_not_overlap_it() {
        let dom = HtmlParser::parse(r#"<html><body><p>foto: <img id="photo" src="tall.png"></p><p id="after">despues</p></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; } p { margin: 0px; }");

        let mut images = ImageMap::new();
        images.insert("tall.png".to_string(), Arc::new(engine_image::DecodedImage { width: 40, height: 300, rgba: vec![255u8; 40 * 300 * 4] }));

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &images);

        let photo_node = Node::find_by_id(&dom, "photo").expect("photo deberia existir");
        let photo_box = find_box_for_dom_node(&root, &photo_node).expect("photo deberia tener caja");
        assert_eq!(photo_box.dimensions.height, 300.0, "la imagen deberia medir su alto natural real (300px), no un valor fijo");

        let after_node = Node::find_by_id(&dom, "after").expect("after deberia existir");
        let after_box = find_box_for_dom_node(&root, &after_node).expect("after deberia tener caja");

        assert!(
            after_box.dimensions.y >= photo_box.dimensions.y + photo_box.dimensions.height,
            "el <p> siguiente (y={}) no deberia solaparse con el borde inferior de la imagen (y={} + alto={} = {})",
            after_box.dimensions.y, photo_box.dimensions.y, photo_box.dimensions.height, photo_box.dimensions.y + photo_box.dimensions.height,
        );
    }

    /// Varios elementos inline consecutivos (no solo texto+inline) tambien
    /// deberian compartir linea entre si.
    #[test]
    fn multiple_inline_elements_in_a_row_share_the_same_line() {
        let dom = HtmlParser::parse(r#"<html><body><p><b id="first">uno</b><i id="second">dos</i></p></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

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
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let container_node = Node::find_by_id(&dom, "container").expect("container deberia existir");
        let container_box = find_box_for_dom_node(&root, &container_node).expect("container deberia tener caja");

        let one_node = Node::find_by_id(&dom, "one").expect("one deberia existir");
        let one_box = find_box_for_dom_node(&root, &one_node).expect("one deberia tener caja");

        // Los 3 fragmentos comparten linea (caben de sobra), asi que el
        // contenedor deberia medir UNA sola linea de alto, no tres.
        assert_eq!(container_box.dimensions.height, one_box.dimensions.height, "3 fragmentos en la misma linea no deberian multiplicar la altura del contenedor");
    }

    /// El punto real de la Fase 3.4: dos `<td>` en la misma `<tr>` se
    /// colocan uno al lado del otro (no apilados como haria el flujo de
    /// bloque normal), cada uno con la mitad exacta del ancho de la tabla -
    /// el algoritmo de columnas iguales declarado en el doc-comment de
    /// `flow_table_children`.
    #[test]
    fn table_lays_out_cells_side_by_side_in_equal_columns() {
        let dom = HtmlParser::parse(r#"<html><body><table id="t" style="width: 400px;"><tr><td id="a">a</td><td id="b">b</td></tr></table></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; } td { padding: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(a_box.dimensions.x, 0.0);
        assert_eq!(a_box.dimensions.width, 200.0, "cada columna deberia medir la mitad del ancho de la tabla");
        assert_eq!(b_box.dimensions.x, 200.0, "la segunda celda deberia empezar donde termina la primera columna");
        assert_eq!(a_box.dimensions.y, b_box.dimensions.y, "las celdas de la misma fila deberian compartir Y");
    }

    /// Dos filas se apilan verticalmente (segunda fila empieza donde termina
    /// la primera), cada una con su propio alto - la parte "de bloque" del
    /// algoritmo de tabla, analoga a `flow_block_children` pero fila a fila.
    #[test]
    fn table_stacks_multiple_rows_vertically() {
        let dom = HtmlParser::parse(
            r#"<html><body><table id="t" style="width: 200px;">
                <tr id="row1"><td id="a" style="height: 30px;">a</td></tr>
                <tr id="row2"><td id="b" style="height: 50px;">b</td></tr>
            </table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } td { padding: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(a_box.dimensions.y, 0.0);
        assert_eq!(a_box.dimensions.height, 30.0);
        assert_eq!(b_box.dimensions.y, 30.0, "la segunda fila deberia empezar justo donde termina la primera");
        assert_eq!(b_box.dimensions.height, 50.0);
    }

    /// Todas las celdas de una fila se estiran al alto de la celda MAS ALTA
    /// de esa fila - el comportamiento visible por defecto de cualquier
    /// tabla real (ver el doc-comment de `flow_table_children`, punto 4).
    #[test]
    fn table_stretches_every_cell_in_a_row_to_the_tallest_cells_height() {
        let dom = HtmlParser::parse(
            r#"<html><body><table id="t" style="width: 200px;"><tr><td id="short" style="height: 10px;">a</td><td id="tall" style="height: 90px;">b</td></tr></table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } td { padding: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let short_node = Node::find_by_id(&dom, "short").expect("short deberia existir");
        let tall_node = Node::find_by_id(&dom, "tall").expect("tall deberia existir");
        let short_box = find_box_for_dom_node(&root, &short_node).expect("short deberia tener caja");
        let tall_box = find_box_for_dom_node(&root, &tall_node).expect("tall deberia tener caja");

        assert_eq!(tall_box.dimensions.height, 90.0);
        assert_eq!(short_box.dimensions.height, 90.0, "la celda mas corta deberia estirarse al alto de la mas alta de su fila");
    }

    /// `collect_table_rows` debe atravesar `<thead>`/`<tbody>` de forma
    /// transparente - el marcado real de casi cualquier tabla real, no solo
    /// `<table><tr>` directo (ver su doc-comment).
    #[test]
    fn table_rows_wrapped_in_thead_and_tbody_are_still_found() {
        let dom = HtmlParser::parse(
            r#"<html><body><table id="t" style="width: 200px;">
                <thead><tr><td id="header">h</td></tr></thead>
                <tbody><tr><td id="body_cell">b</td></tr></tbody>
            </table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } td { padding: 0px; height: 20px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let header_node = Node::find_by_id(&dom, "header").expect("header deberia existir");
        let body_node = Node::find_by_id(&dom, "body_cell").expect("body_cell deberia existir");
        let header_box = find_box_for_dom_node(&root, &header_node).expect("header deberia tener caja");
        let body_box = find_box_for_dom_node(&root, &body_node).expect("body_cell deberia tener caja");

        assert_eq!(header_box.dimensions.y, 0.0, "la fila de thead deberia layoutearse como la primera fila de la tabla");
        assert_eq!(body_box.dimensions.y, 20.0, "la fila de tbody deberia layoutearse justo debajo de la de thead");
    }

    /// El numero de columnas es el MAXIMO de celdas de cualquier fila - una
    /// fila con menos celdas que otra no reduce el numero de columnas de
    /// toda la tabla.
    #[test]
    fn table_column_count_is_the_max_cell_count_of_any_row() {
        let dom = HtmlParser::parse(
            r#"<html><body><table id="t" style="width: 300px;">
                <tr><td id="only">a</td></tr>
                <tr><td id="first">b</td><td id="second">c</td><td id="third">d</td></tr>
            </table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } td { padding: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let only_node = Node::find_by_id(&dom, "only").expect("only deberia existir");
        let third_node = Node::find_by_id(&dom, "third").expect("third deberia existir");
        let only_box = find_box_for_dom_node(&root, &only_node).expect("only deberia tener caja");
        let third_box = find_box_for_dom_node(&root, &third_node).expect("third deberia tener caja");

        // 3 columnas de 100px cada una (300 / 3) - la fila de una sola celda
        // deberia usar el MISMO ancho de columna que la fila de 3 celdas,
        // no ensanchar su unica celda a los 300px completos.
        assert_eq!(only_box.dimensions.width, 100.0, "el numero de columnas lo fija la fila con MAS celdas, no la fila individual");
        assert_eq!(third_box.dimensions.x, 200.0);
    }
}
