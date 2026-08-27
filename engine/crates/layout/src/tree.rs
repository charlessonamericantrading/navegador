use crate::box_model::EdgeSizes;
use crate::layout_box::{AvailableAxis, BoxType, LayoutBox, MeasureKey, Rect, ReplacedText};
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
/// Sustituye cada `var(--nombre, respaldo)` de `value` por el valor de esa
/// propiedad personalizada, o por el respaldo si no esta definida.
///
/// Las propiedades personalizadas (`--color-base: #fff`) son la forma en
/// que se escribe CSS hoy: MDN usa `var()` 547 veces y Wikipedia declara
/// hasta sus colores de fondo asi. Sin resolverlas, el valor que llega al
/// layout es la cadena literal "var(--algo)", que no parsea como longitud
/// ni como color, y la declaracion entera se pierde - la pagina se maqueta
/// como si su hoja de estilos no existiera.
///
/// Se resuelve de dentro hacia fuera y de forma recursiva (una variable
/// puede valer otra `var()`), con un tope de profundidad que corta las
/// referencias circulares: `--a: var(--b); --b: var(--a)` es CSS valido de
/// escribir y no debe colgar el motor.
fn substitute_css_vars(value: &str, vars: &HashMap<String, String>, depth: u8) -> String {
    const MAX_DEPTH: u8 = 8;
    if depth >= MAX_DEPTH || !value.contains("var(") {
        return value.to_string();
    }

    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if !value[i..].starts_with("var(") {
            let ch_len = value[i..].chars().next().map(char::len_utf8).unwrap_or(1);
            out.push_str(&value[i..i + ch_len]);
            i += ch_len;
            continue;
        }
        // Buscar el parentesis que cierra ESTE `var(`, contando los que se
        // abran por dentro (el respaldo puede llevar otro `var()` o un
        // `calc()`).
        let open = i + 3;
        let mut level = 0usize;
        let mut close = None;
        for (offset, ch) in value[open..].char_indices() {
            match ch {
                '(' => level += 1,
                ')' => {
                    level -= 1;
                    if level == 0 {
                        close = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            // `var(` sin cerrar: se copia tal cual y se deja de interpretar.
            out.push_str(&value[i..]);
            break;
        };

        let inner = &value[open + 1..close];
        let (name, fallback) = match inner.find(',') {
            Some(comma) => (inner[..comma].trim(), Some(inner[comma + 1..].trim())),
            None => (inner.trim(), None),
        };

        let replacement = match vars.get(name) {
            Some(found) => substitute_css_vars(found, vars, depth + 1),
            None => match fallback {
                Some(f) => substitute_css_vars(f, vars, depth + 1),
                // Sin valor ni respaldo, el spec declara la propiedad
                // invalida; dejarla vacia es lo mas parecido que puede hacer
                // este motor sin un concepto de "valor no valido".
                None => String::new(),
            },
        };
        out.push_str(&replacement);
        i = close + 1;
    }
    out
}

/// Una propiedad personalizada es cualquiera cuyo nombre empieza por `--`.
/// Heredan SIEMPRE (por eso no estan ni pueden estar en
/// `INHERITABLE_PROPERTIES`, que es una lista cerrada).
fn is_custom_property(name: &str) -> bool {
    name.starts_with("--")
}

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
    "list-style",
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
/// Convierte a pixeles una longitud escrita en cualquier unidad que dependa
/// SOLO del elemento y del viewport - `em`, `rem`, `vw`, `vh`, `vmin`,
/// `vmax`, `pt` - y evalua `calc()` cuando todos sus operandos son de ese
/// tipo. Los PORCENTAJES no: dependen del bloque contenedor, que en este
/// punto todavia no existe, asi que se dejan intactos para que los resuelva
/// la maquetacion (ver `resolve_block_width`, `resolve_box_edges`).
///
/// `None` si no es una longitud de esta familia (un color, `auto`, un
/// porcentaje suelto...), en cuyo caso quien llama deja el valor como esta.
fn absolute_length_to_px(token: &str, font_px: f32, viewport: (f32, f32)) -> Option<f32> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    if t == "0" {
        return Some(0.0);
    }
    let (vw, vh) = viewport;
    // El orden importa: `rem` termina en "em", asi que se prueba antes.
    for (sufijo, escala) in [
        ("rem", 16.0),
        ("em", font_px),
        ("vmin", vw.min(vh) / 100.0),
        ("vmax", vw.max(vh) / 100.0),
        ("vw", vw / 100.0),
        ("vh", vh / 100.0),
        ("pt", 4.0 / 3.0),
        ("px", 1.0),
    ] {
        if let Some(numero) = t.strip_suffix(sufijo) {
            return numero.trim().parse::<f32>().ok().map(|n| n * escala);
        }
    }
    None
}

/// Evalua un `calc()` cuyos operandos sean longitudes absolutas o numeros
/// puros. Soporta `+ - * /` con la precedencia normal y parentesis
/// anidados.
///
/// `None` si aparece algo que no se puede resolver aqui (un porcentaje, una
/// `var()` sin sustituir, una funcion desconocida): entonces la declaracion
/// se deja tal cual, que es mejor que inventarse un numero.
fn eval_calc(expr: &str, font_px: f32, viewport: (f32, f32)) -> Option<f32> {
    // Tokenizado: numeros-con-unidad, operadores y parentesis. Un `-` solo
    // es operador si va rodeado de espacios o sigue a otro operador; en CSS
    // `10px -5px` son dos valores y `10px-5px` no es valido, asi que exigir
    // el espacio es lo correcto y ademas evita partir "e-5" de un numero.
    let mut tokens: Vec<String> = Vec::new();
    let mut actual = String::new();
    let mut anterior_es_valor = false;
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' | ')' => {
                if !actual.trim().is_empty() {
                    tokens.push(actual.trim().to_string());
                }
                actual.clear();
                tokens.push(c.to_string());
                anterior_es_valor = c == ')';
            }
            '+' | '*' | '/' => {
                if !actual.trim().is_empty() {
                    tokens.push(actual.trim().to_string());
                }
                actual.clear();
                tokens.push(c.to_string());
                anterior_es_valor = false;
            }
            '-' if anterior_es_valor && actual.trim().is_empty() => {
                tokens.push("-".to_string());
                anterior_es_valor = false;
            }
            c if c.is_whitespace() => {
                if !actual.trim().is_empty() {
                    tokens.push(actual.trim().to_string());
                    anterior_es_valor = true;
                }
                actual.clear();
            }
            c => actual.push(c),
        }
    }
    if !actual.trim().is_empty() {
        tokens.push(actual.trim().to_string());
    }

    let mut pos = 0usize;
    let valor = eval_suma(&tokens, &mut pos, font_px, viewport)?;
    if pos != tokens.len() {
        return None;
    }
    Some(valor)
}

fn eval_suma(tokens: &[String], pos: &mut usize, font_px: f32, viewport: (f32, f32)) -> Option<f32> {
    let mut acc = eval_producto(tokens, pos, font_px, viewport)?;
    while *pos < tokens.len() && (tokens[*pos] == "+" || tokens[*pos] == "-") {
        let op = tokens[*pos].clone();
        *pos += 1;
        let derecha = eval_producto(tokens, pos, font_px, viewport)?;
        acc = if op == "+" { acc + derecha } else { acc - derecha };
    }
    Some(acc)
}

fn eval_producto(tokens: &[String], pos: &mut usize, font_px: f32, viewport: (f32, f32)) -> Option<f32> {
    let mut acc = eval_atomo(tokens, pos, font_px, viewport)?;
    while *pos < tokens.len() && (tokens[*pos] == "*" || tokens[*pos] == "/") {
        let op = tokens[*pos].clone();
        *pos += 1;
        let derecha = eval_atomo(tokens, pos, font_px, viewport)?;
        if op == "*" {
            acc *= derecha;
        } else {
            if derecha == 0.0 {
                return None;
            }
            acc /= derecha;
        }
    }
    Some(acc)
}

fn eval_atomo(tokens: &[String], pos: &mut usize, font_px: f32, viewport: (f32, f32)) -> Option<f32> {
    let token = tokens.get(*pos)?.clone();
    if token == "(" {
        *pos += 1;
        let dentro = eval_suma(tokens, pos, font_px, viewport)?;
        if tokens.get(*pos)? != ")" {
            return None;
        }
        *pos += 1;
        return Some(dentro);
    }
    *pos += 1;
    if let Some(px) = absolute_length_to_px(&token, font_px, viewport) {
        return Some(px);
    }
    // Un numero puro (el factor de un `* 2`).
    token.parse::<f32>().ok()
}

/// Reescribe un valor de propiedad dejando en PIXELES todo lo que se pueda
/// resolver sin conocer el bloque contenedor. Respeta los valores de varios
/// tokens (`padding: 1em 2em`) convirtiendo cada uno por separado.
///
/// Devuelve `None` si no habia nada que convertir, para no reescribir el
/// mapa de estilos sin motivo.
fn resolve_relative_units(value: &str, font_px: f32, viewport: (f32, f32)) -> Option<String> {
    if !necesita_resolver_unidades(value) {
        return None;
    }
    let mut salida: Vec<String> = Vec::new();
    let mut cambiado = false;
    for token in dividir_en_tokens(value) {
        let bajo = token.to_ascii_lowercase();
        if let Some(interior) = bajo.strip_prefix("calc(").and_then(|r| r.strip_suffix(')')) {
            match eval_calc(interior, font_px, viewport) {
                Some(px) => {
                    salida.push(format!("{px}px"));
                    cambiado = true;
                }
                None => salida.push(token.to_string()),
            }
            continue;
        }
        // `px` no se toca: ya esta en la unidad final y reescribirlo solo
        // introduciria ruido de coma flotante.
        if bajo.ends_with("px") {
            salida.push(token.to_string());
            continue;
        }
        match absolute_length_to_px(&bajo, font_px, viewport) {
            Some(px) => {
                salida.push(format!("{px}px"));
                cambiado = true;
            }
            None => salida.push(token.to_string()),
        }
    }
    if cambiado {
        Some(salida.join(" "))
    } else {
        None
    }
}

/// Filtro barato para no tokenizar cada valor de cada caja: solo los que
/// pueden contener una unidad relativa o un `calc()`.
fn necesita_resolver_unidades(value: &str) -> bool {
    let v = value.as_bytes();
    // "em" cubre tambien "rem"; "v" cubre vw/vh/vmin/vmax.
    value.contains("em") || value.contains("calc(") || value.contains("pt") || value.contains("vw") || value.contains("vh") || value.contains("vmin") || value.contains("vmax") || v.is_empty()
}

/// Parte un valor en tokens separados por espacios, SIN romper el interior
/// de una funcion (`calc(1em + 2px)` es un solo token pese a sus espacios).
fn dividir_en_tokens(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut actual = String::new();
    let mut nivel = 0usize;
    for c in value.chars() {
        match c {
            '(' => {
                nivel += 1;
                actual.push(c);
            }
            ')' => {
                nivel = nivel.saturating_sub(1);
                actual.push(c);
            }
            c if c.is_whitespace() && nivel == 0 => {
                if !actual.is_empty() {
                    out.push(std::mem::take(&mut actual));
                }
            }
            c => actual.push(c),
        }
    }
    if !actual.is_empty() {
        out.push(actual);
    }
    out
}

fn parse_css_font_size(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if let Some(px) = trimmed.strip_suffix("px") {
        px.trim().parse::<f32>().ok().filter(|size| *size > 0.0)
    } else if let Some(rem) = trimmed.strip_suffix("rem") {
        rem.trim().parse::<f32>().ok().filter(|n| *n > 0.0).map(|n| n * 16.0)
    } else {
        None
    }
}

fn parse_css_length(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if trimmed == "0" {
        return Some(0.0);
    }
    if let Some(px) = trimmed.strip_suffix("px") {
        px.trim().parse::<f32>().ok().filter(|n| *n >= 0.0)
    } else if let Some(rem) = trimmed.strip_suffix("rem") {
        rem.trim().parse::<f32>().ok().filter(|n| *n >= 0.0).map(|n| n * 16.0)
    } else {
        None
    }
}

fn parse_css_offset(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if trimmed == "0" {
        return Some(0.0);
    }
    if let Some(px) = trimmed.strip_suffix("px") {
        px.trim().parse::<f32>().ok()
    } else if let Some(rem) = trimmed.strip_suffix("rem") {
        rem.trim().parse::<f32>().ok().map(|n| n * 16.0)
    } else {
        None
    }
}

/// Parsea un offset (`top`, `bottom`, `left`, `right`) soportando tanto
/// unidades absolutas (`px`, `rem`) como porcentajes (`%`) calculados
/// contra `reference_size`.
fn parse_css_offset_relative(value: &str, reference_size: f32) -> Option<f32> {
    let trimmed = value.trim();
    if trimmed == "0" {
        return Some(0.0);
    }
    if let Some(pct) = trimmed.strip_suffix('%') {
        return pct.trim().parse::<f32>().ok().map(|p| p / 100.0 * reference_size);
    }
    parse_css_offset(trimmed)
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

/// `float: left`/`right` (Fase 12) - `None` para `none` (el valor inicial
/// real) o cualquier otro valor no reconocido. A diferencia de
/// `is_out_of_flow`, un float SI reserva espacio (horizontal, para el
/// contenido que fluye a su lado - ver `flow_block_children`), asi que no
/// comparte la misma comprobacion ni el mismo camino de codigo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatSide {
    Left,
    Right,
}

fn float_side(computed_style: &HashMap<String, String>) -> Option<FloatSide> {
    match computed_style.get("float").map(|v| v.trim()) {
        Some("left") => Some(FloatSide::Left),
        Some("right") => Some(FloatSide::Right),
        _ => None,
    }
}

/// `display: list-item` (Fase 40, hoja UA: `li { display: list-item; }`) -
/// el `<li>` sigue siendo `BoxType::Block` de siempre
/// (`override_box_type_from_display` no reconoce este valor, cae al
/// `default.clone()` tag-based), esto solo decide si `flow_normal_block_children`
/// le antepone una vineta.
fn is_list_item(computed_style: &HashMap<String, String>) -> bool {
    computed_style.get("display").map(String::as_str) == Some("list-item")
}

/// `list-style-type: none` o el shorthand `list-style: none` (ambos ya
/// heredables, ver `INHERITABLE_PROPERTIES`) - la forma real, MUY comun,
/// con la que una pagina real quita las vinetas por defecto de una lista
/// entera desde el `<ul>`/`<ol>` (encontrado en vivo contra la Wikipedia
/// real: su tabla de contenidos es un `<ul>` con `list-style: none` que
/// pinta su propia numeracion via `<span>`, no la del navegador - sin este
/// chequeo, `place_list_marker` le anteponia una vineta encima de la suya
/// propia). El shorthand no se expande a longhand en ningun sitio de este
/// motor (ver el aviso de `user_agent_stylesheet.rs`), asi que se busca
/// "none" como TOKEN suelto del valor crudo, no con una comparacion
/// exacta (cubre `list-style: none outside` igual que el `list-style:
/// none` simple).
fn list_style_is_none(computed_style: &HashMap<String, String>) -> bool {
    if computed_style.get("list-style-type").is_some_and(|v| v.trim().eq_ignore_ascii_case("none")) {
        return true;
    }
    computed_style.get("list-style").is_some_and(|v| v.split_whitespace().any(|token| token.eq_ignore_ascii_case("none")))
}

/// Etiqueta HTML del elemento que produjo esta caja, o `None` para una
/// caja de texto/sintetica sin `dom_node` (ver su doc-comment en
/// `layout_box.rs`) - lo unico que necesita `place_list_marker` para
/// distinguir `<ol>` (numerado) de cualquier otro contenedor de lista
/// (con vineta).
fn tag_name_of(node: &LayoutBox) -> Option<String> {
    let dom_node = node.dom_node.as_ref()?;
    match &dom_node.read().unwrap().node_type {
        NodeType::Element { tag_name, .. } => Some(tag_name.clone()),
        _ => None,
    }
}

/// Antepone una vineta real a `child` (un `<li>`, ya con sus `dimensions`
/// finales resueltas por el flujo de bloque normal - por eso se llama
/// DESPUES de `flow_block_children(child, ...)`, no antes: necesita saber
/// donde empieza `child` de verdad). `•` para cualquier lista salvo
/// `<ol>` (numerada, `ordinal.` con el mismo criterio 1-based que un
/// navegador real), via `ordinal` (posicion 1-based entre sus hermanos
/// `list-item`, llevada por quien llama - `place_list_marker` en si no
/// sabe nada de hermanos).
///
/// El marcador se posiciona FUERA de la caja de contenido de `child`, en
/// el hueco que le da su propio `margin-left` (ver la hoja UA) -
/// equivalente honesto-minimo de `list-style-position: outside`, el valor
/// inicial real de la propiedad: no reserva su propio ancho en el flujo
/// (no lo necesita, vive en un margen que ya nadie mas ocupa) y no cambia
/// nada de la posicion/ancho de `child`. Se inserta como el PRIMER hijo de
/// `child` (una caja de texto sintetica, sin `dom_node`) para que pinte
/// por el mismo camino que cualquier otro texto, sin tocar `engine-gfx`.
fn place_list_marker(child: &mut LayoutBox, ordinal: u32, ordered: bool, font_set: Option<&FontSet>) {
    let text = if ordered { format!("{ordinal}.") } else { "\u{2022}".to_string() };
    let font_size = child.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
    let natural_width = match font_set.and_then(|set| set.pick(false, false)) {
        Some(f) => engine_text::wrapped_line_width(f, &text, font_size),
        None => text.chars().count() as f32 * 8.0,
    };
    const GAP_BEFORE_CONTENT: f32 = 6.0;
    let mut marker = LayoutBox::new(BoxType::Text(text));
    marker.computed_style = child.computed_style.clone();
    marker.dimensions = Rect {
        x: child.dimensions.x - natural_width - GAP_BEFORE_CONTENT,
        y: child.dimensions.y,
        width: natural_width,
        height: font_size * 1.2,
    };
    child.children.insert(0, marker);
}

/// Un float TODAVIA activo en la posicion vertical actual del flujo
/// (Fase 12) - `flow_block_children` mantiene como mucho UNO por lado
/// (simplificacion declarada, ver `LayoutTreeBuilder::place_float_child`)
/// y lo usa para estrechar el `origin_x`/ancho disponible de cualquier
/// contenido normal que caiga dentro de `[.., bottom_y)`, y para saber
/// contra que borde ancla un float NUEVO del mismo lado.
///
/// `edge` es la coordenada X (absoluta, mismo sistema que
/// `LayoutBox::dimensions`) desde la que el contenido normal debe
/// empezar (para un float IZQUIERDO, el borde DERECHO del float, margen
/// incluido) o terminar (para uno DERECHO, su borde IZQUIERDO) - un solo
/// campo basta para los dos lados porque cada `ActiveFloat` vive en su
/// propio `Option` (`float_left`/`float_right`), nunca mezclados.
struct ActiveFloat {
    edge: f32,
    bottom_y: f32,
}

/// Ancho de respaldo para un float SIN `width` explicito (Fase 12) - el
/// spec real le daria shrink-to-fit (encogerse a su contenido), que este
/// motor no mide en NINGUN sitio todavia (misma limitacion ya declarada
/// para items flex y cajas fuera de flujo, ver `measure_flex_item`/
/// `resolve_positioned_boxes`) - en la practica, un float sin `width`
/// propio es raro: casi cualquier uso real de `float` ya trae su propio
/// ancho (una imagen, una barra lateral de tamaño fijo).
const DEFAULT_FLOAT_WIDTH: f32 = 200.0;

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
/// sobre-especificado. No-op si `position` no es `relative` o `sticky`.
fn apply_relative_offset(node: &mut LayoutBox) {
    let pos = node.computed_style.get("position").map(String::as_str);
    if pos != Some("relative") && pos != Some("sticky") {
        return;
    }
    let dx = match (
        node.computed_style.get("left").and_then(|v| parse_css_offset_relative(v, node.dimensions.width)),
        node.computed_style.get("right").and_then(|v| parse_css_offset_relative(v, node.dimensions.width)),
    ) {
        (Some(l), _) => l,
        (None, Some(r)) => -r,
        (None, None) => 0.0,
    };
    let ref_h = node
        .computed_style
        .get("height")
        .and_then(|v| parse_css_length(v))
        .unwrap_or(node.dimensions.height);
    let dy = match (
        node.computed_style.get("top").and_then(|v| parse_css_offset_relative(v, ref_h)),
        node.computed_style.get("bottom").and_then(|v| parse_css_offset_relative(v, ref_h)),
    ) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        (None, None) => 0.0,
    };
    node.dimensions.x += dx;
    node.dimensions.y += dy;
}

/// Reparte `available` entre las columnas de una tabla a partir de la
/// anchura minima y maxima de contenido de cada una - el algoritmo de
/// "tabla automatica" del spec, simplificado a un solo reparto
/// proporcional (sin `colspan`, sin `table-layout: fixed`).
///
/// Antes cada columna recibia `available / numero_de_columnas`, a partes
/// iguales. Eso descuadra CUALQUIER pagina maquetada con tablas: en Hacker
/// News la columna del numero de orden ("1.", "2.") se llevaba un tercio de
/// la pantalla y empujaba el titular fuera de su sitio.
fn distribute_table_columns(min_widths: &[f32], max_widths: &[f32], available: f32) -> Vec<f32> {
    if min_widths.is_empty() {
        return Vec::new();
    }
    let total_max: f32 = max_widths.iter().sum();
    let total_min: f32 = min_widths.iter().sum();

    // Cabe todo sin cortar nada: cada columna a su anchura maxima, y el
    // espacio sobrante se reparte EN PROPORCION a esa anchura para que la
    // tabla siga ocupando el ancho que se le dio (lo que hace una tabla real
    // al 100%). Proporcional y no a partes iguales, que es lo que dice CSS
    // 2.1 y ademas lo unico razonable: repartiendo por igual, la columna del
    // numero de orden de Hacker News ("1.", "2.") se llevaba los mismos ~200
    // px que la del titular y empujaba todo el contenido a la derecha.
    if total_max <= available {
        let extra = available - total_max;
        if total_max <= 0.0 {
            let each = available / max_widths.len() as f32;
            return vec![each; max_widths.len()];
        }
        return max_widths.iter().map(|w| w + extra * (w / total_max)).collect();
    }
    // Ni con todas las columnas en su minimo cabe: se quedan en el minimo y
    // la tabla desborda, igual que una tabla real demasiado ancha.
    if total_min >= available {
        return min_widths.to_vec();
    }
    // Caso normal: cada columna parte de su minimo y el espacio que sobra se
    // reparte en proporcion a cuanto quiere crecer cada una.
    let slack = available - total_min;
    let total_want: f32 = max_widths.iter().zip(min_widths).map(|(mx, mn)| (mx - mn).max(0.0)).sum();
    if total_want <= 0.0 {
        return min_widths.to_vec();
    }
    min_widths
        .iter()
        .zip(max_widths)
        .map(|(mn, mx)| mn + slack * ((mx - mn).max(0.0) / total_want))
        .collect()
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
/// Si `node` aporta alguna `table-row` a la tabla que lo contiene,
/// siguiendo exactamente las mismas reglas de travesia que
/// `collect_table_rows` (los contenedores intermedios son transparentes; la
/// busqueda se detiene en una celda o en una tabla anidada).
///
/// Sirve para distinguir los hijos de una tabla que SI participan en el
/// algoritmo de filas y columnas de los que no participan en absoluto -
/// tipicamente un `<caption>` (`display: table-caption`). Los segundos no
/// los colocaba nadie: se quedaban en `Rect::default()` y se pintaban todos
/// superpuestos en la esquina (0,0) de la pagina.
fn contributes_table_rows(node: &LayoutBox) -> bool {
    // Comprueba PRIMERO el propio `node`, no solo sus hijos: un `display:
    // table-row` colocado como hijo DIRECTO de la tabla (sin envoltorio -
    // valido en CSS puro con divs, aunque un `<table>` HTML real siempre
    // interpone un `<tbody>` implicito) se perdia, porque la version
    // anterior solo miraba los HIJOS de `node`, nunca `node` mismo. Se
    // enmascaraba en cualquier tabla HTML real (el `<tbody>` de por medio
    // hacia que se llamara sobre el, cuyo hijo SI es el `<tr>`), pero no en
    // una tabla montada con `display: table`/`table-row` sin ese
    // envoltorio - encontrado al escribir un test que evitaba a proposito
    // el "foster parenting" del parser HTML usando divs en vez de
    // `<table>` real.
    match node.computed_style.get("display").map(String::as_str) {
        Some("table-row") => true,
        Some("table") | Some("table-cell") => false,
        _ => node.children.iter().any(contributes_table_rows),
    }
}

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
fn resolve_padding(computed_style: &HashMap<String, String>, containing_width: f32) -> EdgeSizes {
    resolve_box_edges(computed_style, "padding", containing_width)
}

/// Lee los cuatro lados de `padding`/`margin` desde sus LONGHANDS
/// (`padding-top`...), que es donde el parser deja ya expandida cualquier
/// forma abreviada de 1/2/3/4 valores.
///
/// Se sigue mirando la propiedad abreviada como ultimo recurso para el caso
/// que el parser no expande (valores con `calc()`/`var()`), y porque hay
/// tests que construyen el `computed_style` a mano con solo la abreviada.
fn resolve_box_edges(computed_style: &HashMap<String, String>, name: &str, containing_width: f32) -> EdgeSizes {
    // OJO: los cuatro lados resuelven su porcentaje contra el ANCHO del
    // bloque contenedor, tambien arriba y abajo. No es un descuido: asi lo
    // define el spec, para que un `padding: 5%` de un lado a otro de una
    // caja produzca un hueco cuadrado.
    let fallback = computed_style.get(name).and_then(|v| parse_css_length_relative(v, containing_width)).unwrap_or(0.0);
    let side = |suffix: &str| {
        computed_style
            .get(&format!("{name}-{suffix}"))
            .and_then(|v| parse_css_length_relative(v, containing_width))
            .unwrap_or(fallback)
    };
    EdgeSizes { top: side("top"), right: side("right"), bottom: side("bottom"), left: side("left") }
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
fn resolve_margin(computed_style: &HashMap<String, String>, containing_width: f32) -> EdgeSizes {
    resolve_box_edges(computed_style, "margin", containing_width)
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

fn is_border_box(computed_style: &HashMap<String, String>) -> bool {
    computed_style
        .get("box-sizing")
        .map(|v| v.trim().eq_ignore_ascii_case("border-box"))
        .unwrap_or(false)
}

/// Resuelve el ancho BORDER-BOX final de una caja de bloque, a partir de
/// `width`/`max-width`/`min-width` (si estan puestas en la cascada) mas el
/// ancho que tendria por defecto (`auto_width` - "llenar el espacio
/// disponible", el unico comportamiento que existia antes de esta tarea).
///
/// Si `box-sizing: border-box` esta presente, `width`/`max-width`/`min-width`
/// son directamente el ancho border-box. De lo contrario (el default `content-box`),
/// se suman padding + border para obtener el border-box.
/// Longitud que ademas admite PORCENTAJE, resuelto contra `reference`.
///
/// Los porcentajes no se pueden resolver donde se resuelven `em` o `calc()`
/// (ver `resolve_relative_units`): dependen del bloque contenedor, que solo
/// existe ya durante la maquetacion. Por eso viven aqui, en las funciones
/// que SI conocen esa referencia.
fn parse_css_length_relative(value: &str, reference: f32) -> Option<f32> {
    let trimmed = value.trim();
    if let Some(pct) = trimmed.strip_suffix('%') {
        return pct.trim().parse::<f32>().ok().filter(|n| *n >= 0.0).map(|n| n / 100.0 * reference);
    }
    parse_css_length(trimmed)
}

/// `height` explicito de una caja, ya acotado por `min-height`/`max-height`,
/// resolviendo los porcentajes contra `containing_height`.
///
/// `None` cuando no hay `height` declarada, o cuando la hay en porcentaje
/// pero el bloque contenedor no tiene un alto definido - en ese caso el
/// spec dice que se comporta como `auto`, y devolver `None` es exactamente
/// eso: la caja sigue creciendo con su contenido.
fn resolve_explicit_height(computed_style: &HashMap<String, String>, containing_height: f32) -> Option<f32> {
    let porcentaje_resoluble = |v: &String| -> Option<f32> {
        if v.trim().ends_with('%') && containing_height <= 0.0 {
            return None;
        }
        parse_css_length_relative(v, containing_height)
    };

    let mut height = computed_style.get("height").and_then(porcentaje_resoluble);
    // `min`/`max-height` acotan tambien una altura AUTO, no solo una
    // declarada: un `max-height` sobre contenido que crece es justo su caso
    // de uso. Por eso se aplican aunque `height` sea `None`... salvo que
    // entonces no hay nada que acotar todavia y decidirlo aqui seria
    // adivinar el alto del contenido, asi que solo se acota lo declarado.
    if let Some(h) = height.as_mut() {
        if let Some(max_h) = computed_style.get("max-height").and_then(porcentaje_resoluble) {
            *h = h.min(max_h);
        }
        if let Some(min_h) = computed_style.get("min-height").and_then(porcentaje_resoluble) {
            *h = h.max(min_h);
        }
    }
    height
}

/// Acota un alto ya calculado (el del contenido) con `min-height`/
/// `max-height`. Separado de `resolve_explicit_height` porque aqui SI hay un
/// numero que acotar.
///
/// `border_box`/`box_model_extra`: `max-height`/`min-height` declaran un
/// valor de BORDER-BOX cuando `box-sizing: border-box` esta activo (igual
/// que `height`), pero `height` aqui es CONTENIDO puro. Sin convertir el
/// limite, `max-height: 120px` con `box-sizing: border-box` y 40px de
/// padding+border acotaba el CONTENIDO a 120px y el `dimensions.height`
/// final salia en 160px en vez de los 120px que el autor pidio.
fn clamp_height(height: f32, computed_style: &HashMap<String, String>, containing_height: f32, border_box: bool, box_model_extra: f32) -> f32 {
    let porcentaje_resoluble = |v: &String| -> Option<f32> {
        if v.trim().ends_with('%') && containing_height <= 0.0 {
            return None;
        }
        parse_css_length_relative(v, containing_height)
    };
    let a_contenido = |limite: f32| if border_box { (limite - box_model_extra).max(0.0) } else { limite };
    let mut h = height;
    if let Some(max_h) = computed_style.get("max-height").and_then(porcentaje_resoluble) {
        h = h.min(a_contenido(max_h));
    }
    if let Some(min_h) = computed_style.get("min-height").and_then(porcentaje_resoluble) {
        h = h.max(a_contenido(min_h));
    }
    h
}

fn resolve_block_width(computed_style: &HashMap<String, String>, auto_width: f32) -> f32 {
    let padding = resolve_padding(computed_style, auto_width);
    let border = resolve_border_width(computed_style);
    let box_model_extra = padding.left + padding.right + border.left + border.right;
    let border_box = is_border_box(computed_style);

    // `auto_width` es el ancho del bloque contenedor, que es exactamente la
    // referencia contra la que el spec resuelve un `width` en porcentaje.
    let mut width = computed_style
        .get("width")
        .and_then(|v| parse_css_length_relative(v, auto_width))
        .map(|w| if border_box { w } else { w + box_model_extra })
        .unwrap_or(auto_width);

    if let Some(max_w) = computed_style.get("max-width").and_then(|v| parse_css_length_relative(v, auto_width)) {
        let max_limit = if border_box { max_w } else { max_w + box_model_extra };
        width = width.min(max_limit);
    }
    if let Some(min_w) = computed_style.get("min-width").and_then(|v| parse_css_length_relative(v, auto_width)) {
        let min_limit = if border_box { min_w } else { min_w + box_model_extra };
        width = width.max(min_limit);
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

/// `text-align` (Fase 31) - las tres alineaciones que este motor sabe
/// desplazar de verdad. `justify` se PARSEA (no cae al caso "no
/// reconocido") pero se pinta como `Left` a proposito: fingir un
/// justificado real (repartir espacio extra ENTRE palabras) sin
/// implementarlo se veria peor que dejarlo a la izquierda. `start`/`end`
/// (los valores logicos del spec moderno) se tratan como `left`/`right`
/// respectivamente - este motor no modela `direction: rtl`, asi que en
/// LTR (el unico caso real) son identicos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextAlign {
    Left,
    Center,
    Right,
}

/// `text-align` computado del CONTENEDOR de una racha inline (Fase 31) -
/// se lee del bloque, no de cada hijo suelto: es una propiedad heredable
/// (`INHERITABLE_PROPERTIES`) que en la practica se declara una vez en el
/// padre y se aplica a TODA su racha de contenido inline por igual, igual
/// que hace un navegador real.
fn resolve_text_align(computed_style: &HashMap<String, String>) -> TextAlign {
    match computed_style.get("text-align").map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "center" => TextAlign::Center,
        Some(v) if v == "right" || v == "end" => TextAlign::Right,
        _ => TextAlign::Left,
    }
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

/// Tamaño de respaldo de un `BoxType::Replaced` (Fase 11: controles de
/// formulario) cuando NINGUNA CSS lo fija - en la practica esto nunca
/// deberia alcanzarse, porque la hoja de agente de usuario
/// (`user_agent_stylesheet.rs`) siempre declara `width`/`height` para
/// `input`/`select`/`textarea`; es una red de seguridad honesta (un
/// tamaño fijo declarado, no un panic ni una caja invisible) por si
/// algun dia un selector de esa hoja no matcheara un tag nuevo. El valor
/// aproxima el ancho por defecto real de un `<input>` sin CSS de autor en
/// un navegador real (`size=20` del spec HTML, unos 170px en la mayoria).
const DEFAULT_REPLACED_WIDTH: f32 = 170.0;
const DEFAULT_REPLACED_HEIGHT: f32 = 21.0;

/// A diferencia de `resolve_image_dimensions`, no hay ningun bitmap
/// "natural" del que partir - un control de formulario no es una imagen
/// decodificada, su tamaño SIEMPRE sale de CSS.
fn resolve_replaced_dimensions(explicit_width: Option<f32>, explicit_height: Option<f32>) -> (f32, f32) {
    (
        explicit_width.unwrap_or(DEFAULT_REPLACED_WIDTH),
        explicit_height.unwrap_or(DEFAULT_REPLACED_HEIGHT),
    )
}

/// Fase 36: `display: inline-block/inline/block` puede RECLASIFICAR una
/// caja Block<->Inline - encontrado en vivo contra google.com real:
/// `box_type` (arriba, en `build_node`) se decidia SOLO por nombre de
/// etiqueta, sin consultar `display` en absoluto salvo los casos ya
/// especiales de `none`/`flex`. Un `<div style="display:inline-block">`
/// (patron real EXTREMADAMENTE comun - barras de navegacion, insignias,
/// grupos de botones, la mayoria de layouts pre-flexbox) se quedaba
/// `BoxType::Block`: `is_inline_level` lo excluia de cualquier racha
/// inline (`flow_block_children`), asi que en vez de fluir al lado de sus
/// hermanos se apilaba solo, con el ANCHO COMPLETO del contenedor - un bug
/// de layout real, no solo cosmetico (el sintoma exacto que llevo a
/// investigar esta fase: varios `<div>` con esta declaracion colapsando a
/// ancho CERO en un caso real, arrastrados por una cascada de contenedores
/// mal clasificados).
///
/// Solo Block<->Inline se reclasifican - `Image`/`Replaced` (`<img>`/
/// `<input>`/`<select>`/`<textarea>`) quedan SIEMPRE forzados por su
/// etiqueta: son "elementos reemplazados" cuya naturaleza atomica el spec
/// real tampoco cambia con `display` (salvo matices de flujo que este
/// motor no modela) - simplificacion declarada, y mantiene el cambio
/// acotado a lo que la arquitectura ya soporta de sobra:
/// `place_inline_node::BoxType::Inline` ya recursa en hijos
/// `BoxType::Block` sin problema (ver su doc-comment, "un elemento inline
/// puede contener elementos de bloque"), asi que reclasificar no abre
/// ningun camino de codigo NUEVO, solo dirige tags existentes hacia ramas
/// ya probadas por `<button>`/`<a>`/`<span>` de siempre.
fn override_box_type_from_display(default: &BoxType, display: Option<&str>) -> BoxType {
    match (display, default) {
        (Some("inline-block") | Some("inline"), BoxType::Block) => BoxType::Inline,
        (Some("block"), BoxType::Inline) => BoxType::Block,
        _ => default.clone(),
    }
}

/// Fase 34: que texto (si alguno) mostrar DENTRO de una caja
/// `BoxType::Replaced` - ver el doc-comment de `ReplacedText`. Encontrado en
/// vivo contra google.com real: sin esto, "Google Search"/"I'm Feeling
/// Lucky" se pintaban como recuadros grises SIN ninguna etiqueta.
///
/// `checkbox`/`radio`/`hidden`/`file`/`image`/`range`/`color` no llevan
/// texto (su aspecto nativo real no es una cadena dentro de la caja) - un
/// navegador real tampoco pinta el `value` de un checkbox como texto.
/// `<select>` (Fase 35) resuelve la opcion seleccionada via
/// `resolve_select_text`.
fn resolve_replaced_text(tag_name: &str, attributes: &HashMap<String, String>, dom_node: &Arc<RwLock<Node>>) -> Option<ReplacedText> {
    let input_type = attributes.get("type").map(|v| v.to_lowercase()).unwrap_or_default();
    let has_value = |v: &&String| !v.is_empty();

    match tag_name {
        "input" => match input_type.as_str() {
            "checkbox" | "radio" | "hidden" | "file" | "image" | "range" | "color" => None,
            "submit" | "button" | "reset" => {
                let default_label = match input_type.as_str() {
                    "submit" => "Submit Query",
                    "reset" => "Reset",
                    _ => "",
                };
                let text = attributes.get("value").filter(has_value).cloned().unwrap_or_else(|| default_label.to_string());
                if text.is_empty() {
                    None
                } else {
                    Some(ReplacedText { text, is_placeholder: false, centered: true })
                }
            }
            _ => {
                if let Some(value) = attributes.get("value").filter(has_value) {
                    // `password`: el spec real jamas pinta el valor tal
                    // cual, sustituye cada caracter por un punto de mascara
                    // - se cuenta por CARACTERES Unicode (`chars().count()`),
                    // no bytes, para que un valor con acentos/emoji no
                    // pinte de mas ni de menos puntos que letras tiene.
                    let text = if input_type == "password" {
                        "\u{2022}".repeat(value.chars().count())
                    } else {
                        value.clone()
                    };
                    Some(ReplacedText { text, is_placeholder: false, centered: false })
                } else {
                    attributes
                        .get("placeholder")
                        .filter(has_value)
                        .map(|p| ReplacedText { text: p.clone(), is_placeholder: true, centered: false })
                }
            }
        },
        "textarea" => {
            // A diferencia de `input`, el valor inicial de un `<textarea>`
            // es su CONTENIDO DOM (un nodo de texto hijo), no un atributo -
            // por eso hace falta `Node::text_content` en vez de
            // `attributes.get("value")`.
            let content = Node::text_content(dom_node);
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                Some(ReplacedText { text: trimmed.to_string(), is_placeholder: false, centered: false })
            } else {
                attributes
                    .get("placeholder")
                    .filter(has_value)
                    .map(|p| ReplacedText { text: p.clone(), is_placeholder: true, centered: false })
            }
        }
        "select" => resolve_select_text(dom_node),
        _ => None,
    }
}

/// Fase 35: la opcion seleccionada de un `<select>` - cerrando la
/// simplificacion que la Fase 34 dejo declarada a proposito. `<option>` NO
/// tiene por que ser hijo DIRECTO (puede estar anidado dentro de
/// `<optgroup>`), asi que `find_all_by_tag` (que ya busca en TODO el
/// subarbol, no solo hijos directos) es lo correcto aqui, no un simple
/// `dom_node.children`.
///
/// Semantica real del spec: la PRIMERA `<option>` con el atributo booleano
/// `selected` presente gana (igual criterio de "presencia = true" que
/// `checked`, ver `ElementAttributes::checked`); si NINGUNA lo declara, el
/// spec por defecto selecciona la primera opcion de la lista - mismo
/// comportamiento visible en cualquier navegador real con un `<select>`
/// sencillo sin `multiple`. Un `<select>` sin ninguna `<option>` no tiene
/// nada que mostrar (`None`), igual que un campo vacio.
fn resolve_select_text(dom_node: &Arc<RwLock<Node>>) -> Option<ReplacedText> {
    let options = Node::find_all_by_tag(dom_node, "option");
    let is_selected = |opt: &Arc<RwLock<Node>>| -> bool {
        matches!(&opt.read().unwrap().node_type, NodeType::Element { attributes, .. } if attributes.contains_key("selected"))
    };
    let chosen = options.iter().find(|opt| is_selected(opt)).or_else(|| options.first())?;
    let text = Node::text_content(chosen);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(ReplacedText { text: trimmed.to_string(), is_placeholder: false, centered: false })
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
    // `gap` se leia solo en `grid_container_style`, asi que en un contenedor
    // flex se ignoraba por completo y los items salian pegados. Misma
    // propiedad, mismo parseo: es la propiedad unificada del spec, no una
    // exclusiva de grid.
    let gap_val = computed_style.get("gap").and_then(|v| parse_css_length(v)).unwrap_or(0.0);

    // `flex-wrap`: sin el, un contenedor cuyos items no caben los aplasta a
    // todos en una sola linea en vez de pasarlos a la siguiente - que es
    // justo para lo que se declara `wrap`. Lo resuelve taffy, solo hay que
    // traducirle el valor.
    let flex_wrap = match computed_style.get("flex-wrap").map(String::as_str) {
        Some("wrap") => taffy::FlexWrap::Wrap,
        Some("wrap-reverse") => taffy::FlexWrap::WrapReverse,
        _ => taffy::FlexWrap::NoWrap,
    };

    taffy::Style {
        display: taffy::Display::Flex,
        flex_direction,
        flex_wrap,
        justify_content,
        align_items,
        gap: taffy::geometry::Size {
            width: taffy::style_helpers::length(gap_val),
            height: taffy::style_helpers::length(gap_val),
        },
        ..Default::default()
    }
}

/// Parsea la sintaxis basica de grid-template-columns/rows a GridTemplateComponent de Taffy
fn parse_grid_template_tracks(value: &str) -> Vec<taffy::GridTemplateComponent<String>> {
    let mut tracks = Vec::new();
    for token in value.split_whitespace() {
        if let Some(fr_val) = token.strip_suffix("fr").and_then(|s| s.parse::<f32>().ok()) {
            tracks.push(taffy::GridTemplateComponent::Single(taffy::style_helpers::fr(fr_val)));
        } else if let Some(px_val) = token.strip_suffix("px").and_then(|s| s.parse::<f32>().ok()) {
            tracks.push(taffy::GridTemplateComponent::Single(taffy::style_helpers::length(px_val)));
        } else if token == "auto" {
            tracks.push(taffy::GridTemplateComponent::Single(taffy::style_helpers::auto()));
        }
    }
    if tracks.is_empty() {
        tracks.push(taffy::GridTemplateComponent::Single(taffy::style_helpers::fr(1.0)));
    }
    tracks
}

/// Traduce las propiedades CSS de un contenedor GRID (`grid-template-columns`,
/// `grid-template-rows`, `gap`/`grid-gap`) a `taffy::Style`.
/// Rectangulo que ocupa un area con nombre dentro de la rejilla, en LINEAS
/// de grid (base 1, como el spec).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GridArea {
    row_start: u16,
    row_end: u16,
    column_start: u16,
    column_end: u16,
}

/// Interpreta `grid-template-areas` - la rejilla dibujada con nombres, una
/// cadena por fila:
///
/// ```text
/// grid-template-areas: 'siteNotice siteNotice' 'columnStart pageContent'
/// ```
///
/// Devuelve, para cada nombre, el rectangulo de lineas que ocupa. Es como
/// se maqueta hoy una pagina de dos columnas: sin esto, los items caen en
/// posiciones automaticas y el orden visual no tiene nada que ver con el
/// que pidio el autor - en la Wikipedia real, el indice de contenidos se
/// colaba ENTERO por delante del articulo.
fn parse_grid_template_areas(value: &str) -> HashMap<String, GridArea> {
    let mut filas: Vec<Vec<String>> = Vec::new();
    let mut actual = String::new();
    let mut dentro: Option<char> = None;
    for c in value.chars() {
        match dentro {
            Some(comilla) if c == comilla => {
                filas.push(actual.split_whitespace().map(str::to_string).collect());
                actual.clear();
                dentro = None;
            }
            Some(_) => actual.push(c),
            None if c == '\'' || c == '"' => dentro = Some(c),
            None => {}
        }
    }

    let mut areas: HashMap<String, GridArea> = HashMap::new();
    for (indice_fila, fila) in filas.iter().enumerate() {
        for (indice_col, nombre) in fila.iter().enumerate() {
            // `.` es el nombre reservado para "hueco vacio".
            if nombre == "." {
                continue;
            }
            let r = indice_fila as u16 + 1;
            let c = indice_col as u16 + 1;
            areas
                .entry(nombre.clone())
                .and_modify(|a| {
                    // Un mismo nombre repetido en celdas contiguas define un
                    // area RECTANGULAR que las abarca todas.
                    a.row_start = a.row_start.min(r);
                    a.row_end = a.row_end.max(r + 1);
                    a.column_start = a.column_start.min(c);
                    a.column_end = a.column_end.max(c + 1);
                })
                .or_insert(GridArea { row_start: r, row_end: r + 1, column_start: c, column_end: c + 1 });
        }
    }
    areas
}

/// Traduce las propiedades de colocacion de UN item de rejilla
/// (`grid-area`, `grid-column`, `grid-row`) a las lineas que taffy entiende.
///
/// `areas` es el mapa de `grid-template-areas` del contenedor: un
/// `grid-area: pageContent` no significa nada sin el.
fn grid_item_style(computed_style: &HashMap<String, String>, areas: &HashMap<String, GridArea>, margin: EdgeSizes) -> taffy::Style {
    use taffy::style_helpers::line;
    let mut style = flex_item_style(computed_style, margin);

    if let Some(area) = computed_style.get("grid-area").and_then(|nombre| areas.get(nombre.trim())) {
        style.grid_row = taffy::geometry::Line { start: line(area.row_start as i16), end: line(area.row_end as i16) };
        style.grid_column = taffy::geometry::Line { start: line(area.column_start as i16), end: line(area.column_end as i16) };
        return style;
    }

    // Formas numericas: `grid-column: 2`, `grid-column: 1 / 3`,
    // `grid-column: 1 / -1` (hasta el final).
    let colocar = |valor: &str| -> Option<taffy::geometry::Line<taffy::GridPlacement>> {
        let (a, b) = match valor.split_once('/') {
            Some((a, b)) => (a.trim(), Some(b.trim())),
            None => (valor.trim(), None),
        };
        let inicio: i16 = a.parse().ok()?;
        let fin = match b {
            Some(b) => b.parse::<i16>().ok()?,
            None => inicio + 1,
        };
        Some(taffy::geometry::Line { start: line(inicio), end: line(fin) })
    };
    if let Some(l) = computed_style.get("grid-column").and_then(|v| colocar(v)) {
        style.grid_column = l;
    }
    if let Some(l) = computed_style.get("grid-row").and_then(|v| colocar(v)) {
        style.grid_row = l;
    }
    style
}

fn grid_container_style(computed_style: &HashMap<String, String>) -> taffy::Style {
    // `grid-template: <filas> / <columnas>` es la forma abreviada, y es la
    // que usan las paginas reales (Wikipedia entre ellas). Los longhands
    // ganan si estan puestos.
    let (abreviada_filas, abreviada_columnas) = match computed_style.get("grid-template").and_then(|v| v.split_once('/')) {
        Some((filas, columnas)) => (Some(filas.trim().to_string()), Some(columnas.trim().to_string())),
        None => (None, None),
    };

    let grid_template_columns = computed_style
        .get("grid-template-columns")
        .cloned()
        .or(abreviada_columnas)
        .map(|v| parse_grid_template_tracks(&v))
        .unwrap_or_else(|| vec![taffy::GridTemplateComponent::Single(taffy::style_helpers::fr(1.0))]);

    let grid_template_rows = computed_style
        .get("grid-template-rows")
        .cloned()
        .or(abreviada_filas)
        .map(|v| parse_grid_template_tracks(&v))
        .unwrap_or_default();

    let gap_val = computed_style
        .get("gap")
        .or_else(|| computed_style.get("grid-gap"))
        .and_then(|v| parse_css_length(v))
        .unwrap_or(0.0);

    taffy::Style {
        display: taffy::Display::Grid,
        grid_template_columns,
        grid_template_rows,
        gap: taffy::geometry::Size {
            width: taffy::style_helpers::length(gap_val),
            height: taffy::style_helpers::length(gap_val),
        },
        ..Default::default()
    }
}

/// Traduce las propiedades CSS de un ITEM flex (`flex-grow`/`flex-shrink`/
/// `flex-basis`, mas `width`/`height` si estan puestas) al `taffy::Style`
/// del nodo hoja correspondiente. Valores iniciales reales del spec cuando
/// la propiedad no esta puesta: `flex-grow: 0`, `flex-shrink: 1`,
/// `flex-basis: auto`.
/// `margin` va aparte de `computed_style`: ya viene RESUELTO en pixeles (los
/// porcentajes se miden contra el ancho del contenedor, que esta funcion no
/// conoce) por quien llama, con `resolve_margin`.
///
/// Faltaba antes por completo - `..Default::default()` deja el margen de
/// taffy en cero, asi que el ALGORITMO DE FLEX (no solo el pintado)
/// ignoraba cualquier `margin` de un item. Es justo como Wikipedia separa
/// los items de su barra de usuario (`margin: 0 4px` en cada `<li>`, no
/// `gap`): sin esto, taffy los coloca pegados, sin ningun hueco entre
/// "Donaciones" y "Crear una cuenta".
fn flex_item_style(computed_style: &HashMap<String, String>, margin: EdgeSizes) -> taffy::Style {
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
        margin: taffy::geometry::Rect {
            left: taffy::style_helpers::length(margin.left),
            right: taffy::style_helpers::length(margin.right),
            top: taffy::style_helpers::length(margin.top),
            bottom: taffy::style_helpers::length(margin.bottom),
        },
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
/// Mide el ancho intrínseco (min-content / max-content) de un elemento para
/// que taffy no colapse cajas flex a 0 cuando consulta pasadas especulativas.
/// Cual de las dos anchuras intrinsecas del spec se esta preguntando.
///
/// La distincion NO es un matiz: el `min-width: auto` que todo item flex
/// tiene por defecto resuelve precisamente a `MinContent`, y `taffy` (como
/// cualquier motor real) se niega a encoger un item por debajo de ese
/// valor. Cuando este motor devolvia la misma anchura para las dos, todo
/// item flex con texto declaraba un minimo igual a su frase entera en una
/// sola linea, asi que NADA podia encogerse: los contenedores flex de
/// cualquier web moderna se desbordaban por la derecha (medido en la
/// Wikipedia real: cajas de 8.225 px dentro de un viewport de 1.280) y el
/// texto no llegaba nunca a cortar linea porque su caja era enorme.
#[derive(Debug, Clone, Copy, PartialEq)]
enum IntrinsicWidth {
    /// Lo minimo sin desbordar: para un texto, su palabra mas larga.
    Min,
    /// Sin ningun corte de linea: para un texto, la frase entera.
    Max,
}

impl IntrinsicWidth {
    fn cache_key(self) -> MeasureKey {
        match self {
            Self::Min => MeasureKey::MinContentWidth,
            Self::Max => MeasureKey::MaxContentWidth,
        }
    }
}

fn measure_intrinsic_width(child: &mut LayoutBox, mode: IntrinsicWidth, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
    let key = mode.cache_key();
    if let Some(&(_, (w, _))) = child.measure_cache.iter().find(|(k, _)| *k == key) {
        return w;
    }
    INTRINSIC_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let width = measure_intrinsic_width_uncached(child, mode, font_set, images);
    child.measure_cache.push((key, (width, 0.0)));
    width
}

fn measure_intrinsic_width_uncached(child: &mut LayoutBox, mode: IntrinsicWidth, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
    // Al medir el tamano INTRINSECO no hay bloque contenedor contra el que
    // resolver un porcentaje (justo se esta calculando cuanto pide la caja),
    // asi que cuentan como cero - el mismo criterio del spec.
    let padding = resolve_padding(&child.computed_style, 0.0);
    let border = resolve_border_width(&child.computed_style);
    let extra = padding.left + padding.right + border.left + border.right;

    if let Some(w) = child.computed_style.get("width").and_then(|v| parse_css_length(v)) {
        let border_box = is_border_box(&child.computed_style);
        return if border_box { w } else { w + extra };
    }

    match &child.box_type {
        BoxType::Image(src) => {
            let natural_w = images.get(src).map(|img| img.width as f32).unwrap_or(0.0);
            natural_w + extra
        }
        BoxType::Text(text) => {
            let font_size = child.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
            let bold = resolve_font_weight_is_bold(&child.computed_style);
            let italic = resolve_font_style_is_italic(&child.computed_style);
            let font = font_set.and_then(|s| s.pick(bold, italic));
            let text_w = match font {
                // `Min`: la caja mas estrecha en la que este texto cabe sin
                // desbordar es la de su palabra mas larga - por debajo de eso
                // esa palabra sobresaldria, porque el motor no parte palabras
                // por la mitad (igual que un navegador real sin
                // `overflow-wrap: break-word`).
                Some(f) if mode == IntrinsicWidth::Min => text
                    .split_whitespace()
                    .map(|word| engine_text::text_width(f, word, font_size))
                    .fold(0.0_f32, f32::max),
                // Mismo motivo que en `place_inline_node`: la anchura
                // maxima de contenido es la de la linea tal como el pintor
                // la va a componer.
                Some(f) => engine_text::wrapped_line_width(f, text, font_size),
                // Sin fuente de sistema, la misma aproximacion por numero de
                // caracteres que ya usaba el motor, aplicada a la palabra mas
                // larga o a la cadena entera segun el modo.
                None if mode == IntrinsicWidth::Min => {
                    text.split_whitespace().map(|w| w.chars().count()).max().unwrap_or(0) as f32 * font_size * 0.5
                }
                None => text.chars().count() as f32 * font_size * 0.5,
            };
            text_w + extra
        }
        BoxType::Replaced => 150.0 + extra,
        // Un contenedor flex NO apila a sus hijos verticalmente, asi que su
        // anchura intrinseca no es la del hijo mas ancho: en una FILA los
        // items van uno al lado del otro y sus anchuras se SUMAN (mas los
        // `gap`). Medirlo como si fuera flujo de bloque lo dejaba mucho mas
        // estrecho de lo que necesita, y entonces sus propios hijos se
        // salian de el - eso era la cabecera de Wikipedia, con las cajas
        // desbordando a la derecha y pisandose unas a otras.
        //
        // En una COLUMNA el eje principal es el vertical, asi que la anchura
        // sigue siendo la del hijo mas ancho, igual que en flujo normal.
        _ if child.computed_style.get("display").map(String::as_str) == Some("flex") => {
            let column = matches!(
                child.computed_style.get("flex-direction").map(String::as_str),
                Some("column") | Some("column-reverse")
            );
            let gap = child.computed_style.get("gap").and_then(|v| parse_css_length(v)).unwrap_or(0.0);
            let mut total: f32 = 0.0;
            let mut widest: f32 = 0.0;
            let mut items = 0;
            for c in &mut child.children {
                if is_out_of_flow(&c.computed_style) {
                    continue;
                }
                let cw = measure_intrinsic_width(c, mode, font_set, images);
                total += cw;
                widest = widest.max(cw);
                items += 1;
            }
            if column {
                widest + extra
            } else {
                total + gap * (items.max(1) - 1) as f32 + extra
            }
        }
        _ => {
            let mut max_w: f32 = 0.0;
            let mut inline_w: f32 = 0.0;
            for c in &mut child.children {
                let cw = measure_intrinsic_width(c, mode, font_set, images);
                let is_inline = c.computed_style.get("display").map(String::as_str) == Some("inline") || matches!(c.box_type, BoxType::Text(_));
                // Los hermanos inline SUMAN solo para `Max`: van todos en la
                // misma linea si no hay que cortar. Para `Min` cada uno puede
                // caer en su propia linea, asi que el minimo del conjunto es
                // el MAYOR de sus minimos y no su suma - sumarlos era lo que
                // inflaba el minimo de un parrafo entero hasta su ancho
                // completo.
                if is_inline && mode == IntrinsicWidth::Max {
                    inline_w += cw;
                    max_w = max_w.max(inline_w);
                } else {
                    inline_w = 0.0;
                    max_w = max_w.max(cw);
                }
            }
            max_w + extra
        }
    }
}

/// Funcion de medida que `taffy` llama para saber cuanto espacio necesita
/// UN item flex - taffy puede llamarla varias veces con distintos
/// `known_dimensions`/`available_space` mientras resuelve el layout final.
/// Traduce el `AvailableSpace` de taffy al tipo neutro que usa la clave de
/// cache (ver `layout_box::AvailableAxis`).
fn available_axis(space: taffy::AvailableSpace) -> AvailableAxis {
    match space {
        taffy::AvailableSpace::Definite(v) => AvailableAxis::Definite(v),
        taffy::AvailableSpace::MinContent => AvailableAxis::MinContent,
        taffy::AvailableSpace::MaxContent => AvailableAxis::MaxContent,
    }
}

/// `measure_flex_item` con memoria. Medir un item significa MAQUETAR SU
/// SUBARBOL ENTERO (ver el cuerpo de `measure_flex_item_uncached`), y taffy
/// pide la medida del mismo item varias veces mientras resuelve el reparto
/// de espacio. Sin recordar el resultado, cada nivel de anidamiento flex
/// multiplica el trabajo del nivel de abajo en vez de sumarlo - medido en
/// la Wikipedia real: 505.765 remaquetados y 289.419 medidas para 35.000
/// nodos, 84 s de reloj solo en el flujo normal.
///
/// La clave son las entradas de las que el resultado depende de verdad
/// (`known_dimensions` y `available_space`); el subarbol no cambia durante
/// una maquetacion, asi que misma clave implica mismo resultado.
///
/// Devolver un valor cacheado se salta el efecto secundario de colocar los
/// descendientes, y eso es CORRECTO aqui: la posicion definitiva de los
/// nietos no la fija esta medida sino `finalize_flex_item_children`, que
/// corre despues con las dimensiones que taffy acabo eligiendo.
fn measure_flex_item(
    child: &mut LayoutBox,
    known_dimensions: taffy::geometry::Size<Option<f32>>,
    available_space: taffy::geometry::Size<taffy::AvailableSpace>,
    font_set: Option<&FontSet>,
    images: &ImageMap,
) -> taffy::geometry::Size<f32> {
    let key = MeasureKey::Flex {
        known_width: known_dimensions.width,
        known_height: known_dimensions.height,
        available_width: available_axis(available_space.width),
        available_height: available_axis(available_space.height),
    };
    if let Some(&(_, (width, height))) = child.measure_cache.iter().find(|(k, _)| *k == key) {
        return taffy::geometry::Size { width, height };
    }
    let size = measure_flex_item_uncached(child, known_dimensions, available_space, font_set, images);
    child.measure_cache.push((key, (size.width, size.height)));
    size
}

fn measure_flex_item_uncached(
    child: &mut LayoutBox,
    known_dimensions: taffy::geometry::Size<Option<f32>>,
    available_space: taffy::geometry::Size<taffy::AvailableSpace>,
    font_set: Option<&FontSet>,
    images: &ImageMap,
) -> taffy::geometry::Size<f32> {
    MEASURE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    if let BoxType::Replaced = &child.box_type {
        let explicit_width = child.computed_style.get("width").and_then(|v| parse_css_length(v));
        let explicit_height = child.computed_style.get("height").and_then(|v| parse_css_length(v));
        let (width, height) = resolve_replaced_dimensions(explicit_width, explicit_height);
        return taffy::geometry::Size {
            width: known_dimensions.width.unwrap_or(width),
            height: known_dimensions.height.unwrap_or(height),
        };
    }

    let explicit_w = child.computed_style.get("width").and_then(|v| parse_css_length(v));
    let width = known_dimensions.width.unwrap_or(match available_space.width {
        taffy::AvailableSpace::Definite(w) => {
            if let Some(ew) = explicit_w {
                let border_box = is_border_box(&child.computed_style);
                let extra = child.box_dimensions.padding.left + child.box_dimensions.padding.right + child.box_dimensions.border.left + child.box_dimensions.border.right;
                if border_box { ew } else { ew + extra }
            } else {
                w
            }
        }
        taffy::AvailableSpace::MinContent => measure_intrinsic_width(child, IntrinsicWidth::Min, font_set, images),
        taffy::AvailableSpace::MaxContent => measure_intrinsic_width(child, IntrinsicWidth::Max, font_set, images),
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
    if matches!(child.box_type, BoxType::Image(_) | BoxType::Replaced) {
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

/// Contadores de diagnostico (solo `RUST_LOG=info`): cuantas veces se
/// maqueta un subarbol y cuantas se mide un item flex durante UNA carga.
/// Existen porque el coste del flujo normal resulto no ser proporcional al
/// numero de nodos sino al numero de REMAQUETADOS, y eso solo se ve
/// contandolos.
pub(crate) static FLOW_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static MEASURE_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static INTRINSIC_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static FLEX_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static TABLE_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static GRID_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static INLINE_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Aplica el scroll de cada elemento con `overflow: auto/scroll` que lo
/// tenga registrado (`offsets`: puntero de nodo DOM -> (scrollLeft,
/// scrollTop), el mismo registro que llenan `scrollTop`/`scrollLeft` desde
/// JS - ver `engine_js`). Se llama DESPUES de `LayoutTreeBuilder::build`,
/// no dentro: el arbol de layout no sabe nada de JS ni de registros, solo
/// de posiciones.
///
/// La idea es la misma que ya usa `apply_relative_offset` para `position:
/// relative`: el CONTENEDOR se queda donde estaba (su `PushClip` en
/// `engine-gfx` sigue recortando exactamente el mismo rectangulo), pero
/// TODO su contenido se desplaza en bloque - hacia arriba/izquierda el
/// numero de pixeles que se ha scrolleado. Reusar el recorte que
/// `overflow` ya emite es lo que hace que esto sea scroll de verdad
/// (contenido que se sale por arriba desaparece, contenido nuevo entra por
/// abajo) sin tocar `engine-gfx` para nada: pintura y hit-testing leen
/// `dimensions` como siempre, y `dimensions` ya viene desplazada.
///
/// Simplificacion declarada: se desplazan TODOS los descendientes por
/// igual, incluidos los `position: absolute`/`fixed` que en el spec real
/// escapan al scroll de un ancestro que no sea su containing block. Cubre
/// el caso comun (contenido en flujo normal dentro de un contenedor con
/// scroll) sin resolver el caso raro de un absoluto anidado dentro de un
/// contenedor con scroll.
pub fn apply_scroll_offsets(root: &mut LayoutBox, offsets: &HashMap<usize, (f32, f32)>) {
    if offsets.is_empty() {
        return;
    }
    apply_scroll_offsets_rec(root, offsets);
}

fn apply_scroll_offsets_rec(node: &mut LayoutBox, offsets: &HashMap<usize, (f32, f32)>) {
    let propio = node.dom_node.as_ref().and_then(|n| offsets.get(&(Arc::as_ptr(n) as usize)).copied());
    if let Some((scroll_left, scroll_top)) = propio {
        if scroll_left != 0.0 || scroll_top != 0.0 {
            for hijo in &mut node.children {
                LayoutTreeBuilder::shift_subtree_x(hijo, -scroll_left);
                LayoutTreeBuilder::shift_subtree_y(hijo, -scroll_top);
            }
        }
    }
    for hijo in &mut node.children {
        apply_scroll_offsets_rec(hijo, offsets);
    }
}

pub struct LayoutTreeBuilder;

impl LayoutTreeBuilder {
    /// Construye el arbol de layout, resuelve el estilo CSS de cada caja
    /// (ver `resolve_style`) y le asigna posiciones/tamanos reales mediante
    /// un flujo de bloque top-to-bottom para el caso general, mas flex/grid
    /// reales via `taffy`, floats (`float_left`/`float_right`, ver
    /// `place_float_child`) e inline real (`place_inline_node`). `padding`,
    /// `border` y `margin` reales ya se resuelven desde la cascada (ver
    /// `resolve_padding`/`resolve_border_width`/`resolve_margin`,
    /// `box_dimensions` en cada `LayoutBox`, sin colapso de margenes).
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
        // La caja raiz hace de bloque contenedor inicial: cualquier
        // porcentaje de primer nivel se mide contra el viewport.
        root_box.containing_width = viewport_width;
        root_box.containing_height = viewport_height;

        // Cronometro de las tres pasadas (cascada+construccion del arbol,
        // flujo normal, posicionados). Nivel `info`, igual que el resto del
        // pipeline: en uso normal no cuesta nada y con `RUST_LOG=info` dice
        // cual de las tres domina, en vez de dejarlas como un solo numero.
        let t = std::time::Instant::now();
        Self::build_node(dom_root, &mut root_box, stylesheet, &HashMap::new(), (viewport_width, viewport_height));
        tracing::info!("[tiempo]     cascada + construccion del arbol {:?}", t.elapsed());
        let t = std::time::Instant::now();
        Self::flow_block_children(&mut root_box, font_set, images);
        tracing::info!(
            "[tiempo]     flujo normal {:?} ({} remaquetados, {} medidas flex, {} medidas intrinsecas)",
            t.elapsed(),
            FLOW_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed),
            MEASURE_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed),
            INTRINSIC_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed)
        );
        tracing::info!(
            "[tiempo]     desglose: {} flex, {} tabla, {} grid, {} rachas inline",
            FLEX_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed),
            TABLE_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed),
            GRID_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed),
            INLINE_CALLS.swap(0, std::sync::atomic::Ordering::Relaxed)
        );
        // Segunda pasada (Fase 3.3): `flow_block_children`/`flow_inline_run`/
        // `flow_flex_children`, arriba, ya dejaron cada `position: absolute`/
        // `fixed` SIN resolver a proposito (`is_out_of_flow`, ver esas
        // funciones) - se posicionan aparte aqui, ahora que el flujo normal
        // entero ya existe de verdad y hay "containing blocks" reales contra
        // los que resolverlos. Ver el doc-comment de `resolve_positioned_boxes`.
        let viewport = root_box.dimensions.clone();
        let t = std::time::Instant::now();
        Self::resolve_positioned_boxes(&mut root_box, &viewport, &viewport, font_set, images);
        tracing::info!("[tiempo]     posicionados {:?}", t.elapsed());
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
            node.containing_width = reference.width;

            let left = node.computed_style.get("left").and_then(|v| parse_css_offset_relative(v, reference.width));
            let right = node.computed_style.get("right").and_then(|v| parse_css_offset_relative(v, reference.width));
            let top = node.computed_style.get("top").and_then(|v| parse_css_offset_relative(v, reference.height));
            let bottom = node.computed_style.get("bottom").and_then(|v| parse_css_offset_relative(v, reference.height));

            // Ancho: mismo criterio que el flujo normal (`resolve_block_width`,
            // ya existente) usando el ancho del containing block como "auto" -
            // simplificacion declarada: el spec real usaria shrink-to-fit
            // para un `width: auto` fuera de flujo, no "llenar el
            // contenedor"; el motor no mide shrink-to-fit todavia (mismo
            // hueco que `measure_flex_item`, ver su doc-comment).
            let width = resolve_block_width(&node.computed_style, reference.width);
            node.dimensions.width = width;
            // Con `left` y `right` ambos en `auto` manda la posicion
            // estatica (donde el flujo normal habria dejado la caja), no la
            // esquina del bloque contenedor - ver `LayoutBox::
            // static_position`. Sin posicion estatica anotada (una caja
            // fuera de flujo dentro de un contenedor que no la registro
            // todavia) se cae al comportamiento anterior, que al menos la
            // deja dentro del contenedor.
            let static_position = node.static_position;
            node.dimensions.x = match (left, right) {
                (Some(l), _) => reference.x + l,
                (None, Some(r)) => reference.x + reference.width - width - r,
                (None, None) => static_position.map(|(x, _)| x).unwrap_or(reference.x),
            };
            // Y provisional: si `top` esta puesto, ya es el Y final (`bottom`
            // se ignora cuando ambos estan puestos - un caso sobre-
            // especificado que el spec real resuelve igual, descartando
            // `bottom`). Sin `top`, se coloca provisionalmente en el origen
            // del containing block hasta conocer el alto real de contenido
            // (mas abajo) y poder aplicar `bottom` correctamente.
            node.dimensions.y = match top {
                Some(t) => reference.y + t,
                // Igual que en X: sin `top`, la posicion estatica. Sigue
                // siendo PROVISIONAL cuando hay `bottom`, que se aplica mas
                // abajo una vez se conoce el alto real del contenido.
                None => static_position.map(|(_, y)| y).unwrap_or(reference.y),
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
            if matches!(position.as_deref(), Some("relative") | Some("sticky") | Some("absolute") | Some("fixed")) { node.box_dimensions.padding_box() } else { containing_block.clone() };

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
    /// `viewport_width` solo se usa para evaluar los bloques `@media`
    /// (Fase 18, ver `engine_css::resolve_style`) - se pasa a traves de
    /// toda la recursion sin tocarse porque una consulta de medios se
    /// resuelve contra la VENTANA, no contra el contenedor de cada
    /// elemento (eso serian container queries, que son otra cosa y no
    /// estan implementadas).
    fn build_node(dom_node: &Arc<RwLock<Node>>, parent_layout_box: &mut LayoutBox, stylesheet: &StyleSheet, inherited: &HashMap<String, String>, viewport: (f32, f32)) {
        let r = dom_node.read().unwrap();
        match &r.node_type {
            NodeType::Document => {
                for child in &r.children {
                    Self::build_node(child, parent_layout_box, stylesheet, inherited, viewport);
                }
            }
            NodeType::Element { tag_name, attributes } => {
                // "head", "script" y "style" no tienen representacion visual;
                // sin esto, su contenido de texto se pintaria como si fuera
                // parrafo visible.
                //
                // "noscript" (Fase 32) faltaba en esta lista - encontrado en
                // vivo con una pagina real (el fragmento de respaldo de
                // Google Tag Manager, `<noscript><iframe src="...">
                // </iframe></noscript>`, presente en una cantidad enorme de
                // sitios reales). `html5ever` parsea `<noscript>` como
                // RAWTEXT cuando el scripting esta activado
                // (`ParseOpts::default().tree_builder.scripting_enabled ==
                // true`, ver `dom::parser`) - EXACTAMENTE lo que exige el
                // spec real: en un navegador CON JavaScript, el contenido de
                // `<noscript>` nunca debe ejecutarse ni parsearse como
                // markup real, asi que su interior queda como UN SOLO nodo
                // de texto con el HTML crudo tal cual (aqui, literalmente
                // `<iframe src="...">...</iframe>` como cadena). Un
                // navegador real lo esconde con `noscript { display: none }`
                // en su hoja de agente de usuario; este motor, en cambio, no
                // tenia ninguna regla asi NI este atajo - el resultado
                // exacto que aparecio en pantalla: el marcado del iframe de
                // respaldo pintado como texto plano visible. Se resuelve
                // aqui, no con una regla CSS, por la misma razon que
                // "script"/"style" ya se resuelven aqui: es mas simple y
                // mas robusto que depender de que ninguna hoja de autor
                // sobreescriba `display` por accidente.
                if matches!(tag_name.as_str(), "head" | "script" | "style" | "meta" | "link" | "title" | "noscript") {
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
                // `<input>`/`<select>`/`<textarea>` (Fase 11: controles de
                // formulario) son TAMBIEN "elementos reemplazados" del spec
                // real, mismo concepto que `<img>` pero sin bitmap que
                // pintar - `BoxType::Replaced`, ver su doc-comment en
                // layout_box.rs para el porque no comparten variante con
                // `Image`. `<button>` es el UNICO control que se trata como
                // `span`/`a`/etc (`Inline`, no `Replaced`): a diferencia de
                // los otros tres, su etiqueta es contenido DOM real (un
                // nodo de texto hijo, no un atributo `value`/`placeholder`)
                // y se beneficia de encogerse a ese contenido en vez de un
                // tamaño fijo - ver el doc-comment de `BoxType::Replaced`.
                let box_type = match tag_name.as_str() {
                    "span" | "a" | "b" | "i" | "strong" | "em" | "button" => BoxType::Inline,
                    "img" => BoxType::Image(attributes.get("src").cloned().unwrap_or_default()),
                    "input" | "select" | "textarea" => BoxType::Replaced,
                    _ => BoxType::Block,
                };
                let mut current_box = LayoutBox::new(box_type);
                current_box.dom_node = Some(dom_node.clone());
                // `colspan` solo tiene sentido en una celda; un valor de 0 o
                // ilegible cuenta como 1, igual que exige el propio HTML.
                if matches!(tag_name.as_str(), "td" | "th") {
                    current_box.colspan = attributes.get("colspan").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).max(1);
                    current_box.rowspan = attributes.get("rowspan").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).max(1);
                }
                if matches!(&current_box.box_type, BoxType::Replaced) {
                    current_box.replaced_text = resolve_replaced_text(tag_name, attributes, dom_node);
                }
                // La resolucion de cascada en si (matching + especificidad +
                // atributo style inline) vive en `engine_css::resolve_style`
                // desde hace poco, no aqui - se traslado para que `engine-js`
                // (`getComputedStyle`, en construccion) tambien pueda
                // reusarla sin depender de `layout` solo para esto. Misma
                // logica exacta, cero cambio de comportamiento.
                current_box.computed_style = engine_css::resolve_style(dom_node, stylesheet, viewport.0);

                // `display: none` (no confundir con `visibility: hidden`,
                // que SI genera caja - ver `engine-gfx::display_list`): el
                // spec real saca al elemento y a TODO su subarbol del arbol
                // de render por completo, como si no existieran - de ahi
                // que se corte aqui, ANTES de recursar en los hijos, en vez
                // de generar la caja y filtrarla despues. `display` no es
                // heredable (no esta en `INHERITABLE_PROPERTIES`), asi que
                // esto es una comprobacion puramente local al propio
                // elemento, correcto segun el spec real.
                if current_box.computed_style.get("display").map(String::as_str) == Some("none") {
                    return;
                }

                // Fase 36: `display: inline-block/inline/block` puede
                // RECLASIFICAR Block<->Inline - ver el doc-comment de
                // `override_box_type_from_display`.
                current_box.box_type = override_box_type_from_display(&current_box.box_type, current_box.computed_style.get("display").map(String::as_str));

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

                // Las propiedades personalizadas heredan siempre y se
                // propagan tal cual (sin sustituir): su valor puede
                // contener a su vez `var()`, que se resuelve al usarlas,
                // no al declararlas.
                let mut child_inherited = inherited.clone();
                for (prop, value) in current_box.computed_style.iter() {
                    if is_custom_property(prop) {
                        child_inherited.insert(prop.clone(), value.clone());
                    }
                }

                // Con todas las variables visibles (propias mas heredadas),
                // se resuelve `var()` en el resto de propiedades. Va ANTES
                // del bucle de herencia de abajo para que lo que se propague
                // a los hijos sea ya el valor final, no una referencia.
                let variables: HashMap<String, String> = child_inherited
                    .iter()
                    .filter(|(k, _)| is_custom_property(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !variables.is_empty() {
                    let pending: Vec<(String, String)> = current_box
                        .computed_style
                        .iter()
                        .filter(|(k, v)| !is_custom_property(k) && v.contains("var("))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    for (prop, value) in pending {
                        let resolved = substitute_css_vars(&value, &variables, 0);
                        if resolved.trim().is_empty() {
                            current_box.computed_style.remove(&prop);
                        } else {
                            current_box.computed_style.insert(prop, resolved);
                        }
                    }
                }

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

                // Unidades relativas -> pixeles. Va AQUI, despues del bucle
                // de herencia, porque `em` se mide contra el `font-size` YA
                // resuelto de esta caja, y ese bucle es quien lo deja en px.
                //
                // Sin esto, `width: 10em` o `calc(100px + 50px)` no parsean
                // como longitud y la caja cae a "ocupo todo el contenedor":
                // cualquier diseño de varias columnas se renderiza como una
                // sola columna a ancho completo.
                let font_px = current_box
                    .computed_style
                    .get("font-size")
                    .and_then(|v| parse_css_font_size(v))
                    .unwrap_or(INITIAL_FONT_SIZE);
                let por_resolver: Vec<(String, String)> = current_box
                    .computed_style
                    .iter()
                    .filter(|(k, v)| !is_custom_property(k) && k.as_str() != "font-size" && necesita_resolver_unidades(v))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (prop, value) in por_resolver {
                    if let Some(resuelto) = resolve_relative_units(&value, font_px, viewport) {
                        current_box.computed_style.insert(prop, resuelto);
                    }
                }

                for child in &r.children {
                    Self::build_node(child, &mut current_box, stylesheet, &child_inherited, viewport);
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
        FLOW_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // `display: flex` desvia el contenedor entero a `flow_flex_children`
        // (Fase 3.2, via el crate `taffy` - ver ARCHITECTURE.md "Doctrina de
        // dependencias") ANTES de tocar nada del flujo de bloque normal: un
        // contenedor flex no apila a sus hijos verticalmente ni los agrupa
        // en rachas inline, taffy decide su posicion en los ejes principal/
        // cruzado.
        if container.computed_style.get("display").map(String::as_str) == Some("flex") {
            return Self::flow_flex_children(container, font_set, images);
        }
        // `display: grid` (Fase 3) desvia el contenedor al motor CSS Grid de Taffy
        if container.computed_style.get("display").map(String::as_str) == Some("grid") {
            return Self::flow_grid_children(container, font_set, images);
        }
        // `display: table` (Fase 3.4) se desvia igual que `flex` arriba -
        // ver `flow_table_children` para el porque no es "otro flujo de
        // bloque mas".
        if container.computed_style.get("display").map(String::as_str) == Some("table") {
            return Self::flow_table_children(container, font_set, images);
        }

        Self::flow_normal_block_children(container, font_set, images)
    }

    /// El flujo de bloque propiamente dicho, ya sin el despacho por
    /// `display` que hace `flow_block_children`. Separado para que un
    /// contenedor que se desvio a otro algoritmo pueda VOLVER aqui sin
    /// rebotar en ese despacho para siempre - lo necesita
    /// `flow_table_children` cuando una `display: table` no tiene ninguna
    /// fila (ver alli).
    fn flow_normal_block_children(container: &mut LayoutBox, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {

        let padding = resolve_padding(&container.computed_style, container.containing_width);
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

        // Alto de contenido de ESTE contenedor, pero solo si es DEFINIDO -
        // o sea, si el autor lo declaro. Con `height: auto` (lo normal) no
        // hay numero: el contenedor crece con su contenido, asi que un
        // `height: 50%` de un hijo no tiene contra que medirse y el spec lo
        // trata como `auto`. `0.0` codifica ese "indefinido".
        let definite_inner_height = resolve_explicit_height(&container.computed_style, container.containing_height)
            .map(|h| if is_border_box(&container.computed_style) { (h - border.top - border.bottom - padding.top - padding.bottom).max(0.0) } else { h })
            .unwrap_or(0.0);

        // `float: left`/`right` (Fase 12) - a lo sumo UN float activo por
        // lado a la vez (simplificacion declarada, ver el doc-comment de
        // `place_float_child`). Se declaran aqui, no dentro del bucle,
        // porque tienen que sobrevivir entre iteraciones: un float
        // colocado en la iteracion N sigue estrechando el contenido de
        // las iteraciones N+1, N+2... hasta que `cursor_y` supere su
        // borde inferior.
        let mut float_left: Option<ActiveFloat> = None;
        let mut float_right: Option<ActiveFloat> = None;

        // `display: list-item` (Fase 40) - se leen UNA sola vez antes del
        // bucle: `ordered` es una propiedad del CONTENEDOR (`<ol>` vs
        // cualquier otra lista), no de cada hijo, y el ordinal 1-based
        // cuenta solo entre hermanos `list-item` (un nodo de texto suelto
        // entre `<li>` por espacio en blanco del propio HTML no cuenta).
        let ordered_list = tag_name_of(container).as_deref() == Some("ol");
        let mut list_item_ordinal: u32 = 0;

        let mut i = 0;
        while i < container.children.len() {
            // `position: absolute`/`fixed` (Fase 3.3) se saca del flujo por
            // completo aqui: no reserva espacio ni avanza `cursor_y`, como
            // si no estuviera - se posiciona aparte, ver
            // `resolve_positioned_boxes`, despues de que el flujo normal
            // entero ya este resuelto.
            if is_out_of_flow(&container.children[i].computed_style) {
                // No reserva espacio ni avanza `cursor_y` - pero SI se
                // anota donde habria caido, que es lo que el spec usa
                // cuando `left`/`top` son `auto` (ver
                // `LayoutBox::static_position`).
                container.children[i].static_position = Some((origin_x, cursor_y));
                i += 1;
                continue;
            }

            // Un float cuyo borde inferior ya quedo atras deja de
            // estrechar nada - se comprueba con el `cursor_y` de ESTE
            // momento, antes de decidir nada sobre el hijo actual.
            if float_left.as_ref().is_some_and(|f| cursor_y >= f.bottom_y) {
                float_left = None;
            }
            if float_right.as_ref().is_some_and(|f| cursor_y >= f.bottom_y) {
                float_right = None;
            }

            // `clear: left`/`right`/`both` (Fase 12) - salta por debajo de
            // los floats activos del lado indicado ANTES de colocar este
            // hijo, en vez de dejar que se solape con ellos. Es lo que
            // hace funcionar el patron real "clearfix" (un `<div
            // style="clear: both">` vacio para 'cerrar' una seccion
            // flotada) y cualquier contenido que deba volver a ocupar el
            // ancho completo despues de un float.
            if let Some(clear) = container.children[i].computed_style.get("clear").map(|v| v.trim().to_ascii_lowercase()) {
                if matches!(clear.as_str(), "left" | "both") {
                    if let Some(f) = float_left.take() {
                        cursor_y = cursor_y.max(f.bottom_y);
                    }
                }
                if matches!(clear.as_str(), "right" | "both") {
                    if let Some(f) = float_right.take() {
                        cursor_y = cursor_y.max(f.bottom_y);
                    }
                }
            }

            // El float en si (Fase 12) se resuelve ANTES de la
            // agrupacion en rachas inline: `is_inline_level` no distingue
            // floats, y un `<div class="float">` (el caso mas comun) ni
            // siquiera es inline-level, asi que sin este corte caeria
            // directamente en el camino de "hijo de bloque normal" de mas
            // abajo, apilandose verticalmente como si `float` no
            // existiera - exactamente el bug que esta tarea corrige.
            if let Some(side) = float_side(&container.children[i].computed_style) {
                Self::place_float_child(&mut container.children[i], side, origin_x, inner_width, cursor_y, &mut float_left, &mut float_right, font_set, images);
                i += 1;
                continue;
            }

            // Ancho/origen REALES disponibles para contenido NORMAL en
            // esta posicion vertical (Fase 12) - estrechados por
            // cualquier float activo a cada lado. Granularidad de CAJA
            // COMPLETA, no por linea: un hijo que arranca dentro del
            // rango vertical de un float usa este ancho estrechado para
            // TODA su caja, aunque su contenido termine mas abajo del
            // borde inferior del float (el spec real reajustaria linea a
            // linea dentro de un mismo parrafo, ensanchando las lineas
            // que ya quedan por debajo del float - este motor no hace
            // reflow de texto por linea segun obstaculos, simplificacion
            // declarada). El caso real mas comun - un float con una o dos
            // lineas de texto al lado, o un contenedor completo que
            // deliberadamente se queda mas angosto mientras dura el
            // float - se ve correcto; un parrafo mucho mas alto que el
            // float seguira angosto en toda su altura en vez de
            // ensancharse al pasar por debajo.
            let mut eff_origin_x = origin_x;
            let mut eff_inner_width = inner_width;
            if let Some(f) = &float_left {
                let taken = (f.edge - eff_origin_x).max(0.0);
                eff_origin_x += taken;
                eff_inner_width = (eff_inner_width - taken).max(0.0);
            }
            if let Some(f) = &float_right {
                let taken = ((eff_origin_x + eff_inner_width) - f.edge).max(0.0);
                eff_inner_width = (eff_inner_width - taken).max(0.0);
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
                let text_align = resolve_text_align(&container.computed_style);
                cursor_y = Self::flow_inline_run(&mut container.children[i..run_end], eff_origin_x, eff_inner_width, cursor_y, font_set, images, text_align);
                i = run_end;
                continue;
            }

            let child = &mut container.children[i];
            // `margin` no es heredable y las cajas de texto solo llevan
            // propiedades heredadas en su `computed_style` (ver
            // `build_node`) - por construccion, una caja de texto nunca
            // tiene "margin" en su mapa, asi que esto resuelve a cero para
            // texto de forma automatica, sin necesitar un caso aparte.
            child.containing_width = eff_inner_width;
            child.containing_height = definite_inner_height;
            let margin = resolve_margin(&child.computed_style, eff_inner_width);
            child.box_dimensions.margin = margin;

            cursor_y += margin.top;
            child.dimensions.x = eff_origin_x + margin.left;
            child.dimensions.y = cursor_y;
            // `width`/`max-width`/`min-width` (si estan puestas) sustituyen
            // o acotan el ancho "llenar el espacio disponible" que era el
            // unico comportamiento antes de esta tarea - ver
            // `resolve_block_width`.
            let auto_width = (eff_inner_width - margin.left - margin.right).max(0.0);
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
            // autor; `max-height`/`min-height` se aplican despues via
            // `clamp_height`. El layout en si no reduce el contenido para
            // que quepa (igual que un navegador real: `overflow` no cambia
            // el tamano de las cajas) - lo que SI ocurre es el recorte
            // visual en pintura cuando `overflow` lo pide (`hidden`/
            // `auto`/`scroll`), ver `establishes_clip` en
            // `engine-gfx::display_list`.
            //
            // Una caja de bloque con `height: auto` y sin contenido en flujo
            // mide CERO, que es lo que dice el spec. Antes se le aplicaba un
            // suelo de `LINE_HEIGHT_FALLBACK` (un heuristico del motor "para
            // no colapsar una caja vacia") que le daba 22 px fantasma a cada
            // `<div>` estructural vacio - y una pagina real tiene cientos.
            // En la Wikipedia real eso metia mas de 100 px de hueco en
            // blanco entre la cabecera y el titulo del articulo, y separaba
            // entre si todas las secciones.
            let vertical_extra = child_padding.top + child_padding.bottom + child_border.top + child_border.bottom;
            let explicit_height = resolve_explicit_height(&child.computed_style, child.containing_height);
            let border_box = is_border_box(&child.computed_style);
            let resolved_content_height = match explicit_height {
                Some(h) => if border_box { (h - vertical_extra).max(0.0) } else { h },
                None => clamp_height(content_height, &child.computed_style, child.containing_height, border_box, vertical_extra),
            };
            child.dimensions.height = match explicit_height {
                Some(h) => if border_box { h } else { h + vertical_extra },
                None => resolved_content_height + vertical_extra,
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

            // `display: list-item` (Fase 40) - `child.dimensions` ya es
            // FINAL en este punto, que es justo lo que `place_list_marker`
            // necesita para saber donde colgar la vineta a su izquierda.
            if is_list_item(&child.computed_style) {
                list_item_ordinal += 1;
                if !list_style_is_none(&child.computed_style) {
                    place_list_marker(child, list_item_ordinal, ordered_list, font_set);
                }
            }

            cursor_y += child.dimensions.height + margin.bottom;
            i += 1;
        }

        // El alto final INCLUYE cualquier float todavia activo al
        // terminar el bucle (Fase 12) - a diferencia del spec real (donde
        // un contenedor con SOLO floats dentro colapsa a alto CERO salvo
        // que "los contenga" explicitamente, el famoso problema del
        // "clearfix" que tantas paginas reales trabajan para evitar), este
        // motor SI cuenta el alto de los floats en el auto-height de su
        // contenedor. Eleccion deliberada, no un descuido: es el
        // comportamiento que la mayoria de autores esperan/quieren de
        // todas formas (que el contenedor "abrace" a sus floats), y evitar
        // la sorpresa real del spec aqui no le cuesta nada a ninguna
        // pagina que SI dependiera de ella (ese patron exige un
        // `overflow`/clearfix explicito precisamente para conseguir este
        // mismo resultado).
        let floats_bottom = [float_left, float_right]
            .into_iter()
            .flatten()
            .fold(cursor_y, |max_y, f| max_y.max(f.bottom_y));

        (floats_bottom - content_top).max(0.0)
    }

    /// Coloca un hijo `float: left`/`right` (Fase 12) - anclado al borde
    /// del lado indicado, SIN avanzar `cursor_y` (un float no empuja a
    /// sus hermanos hacia abajo por su propia altura, a diferencia de un
    /// hijo de bloque normal - solo reserva espacio HORIZONTAL para el
    /// contenido que venga despues, ver donde se llama esto en
    /// `flow_block_children`).
    ///
    /// Ancho: `resolve_block_width` de siempre (respeta `width`/
    /// `max-width`/`min-width` si estan puestas), pero con
    /// `DEFAULT_FLOAT_WIDTH` como valor "auto" en vez de "llenar el
    /// contenedor" - ver su doc-comment para el porque (sin shrink-to-fit
    /// real en este motor).
    ///
    /// Simplificacion declarada: solo UN float activo por lado a la vez
    /// (`float_left`/`float_right` son `&mut Option<ActiveFloat>`, no una
    /// pila) - dos `float: left` consecutivos, sin que el primero haya
    /// quedado atras verticalmente, se SOLAPAN en vez de colocarse uno al
    /// lado del otro (el spec real los apila horizontalmente hasta que no
    /// caben, y entonces baja a la siguiente "linea de floats"). El caso
    /// mas comun en paginas reales - un float por lado, con texto normal
    /// fluyendo alrededor - funciona correctamente; una galeria de varias
    /// imagenes flotadas seguidas del mismo lado, no.
    fn place_float_child(
        child: &mut LayoutBox,
        side: FloatSide,
        origin_x: f32,
        inner_width: f32,
        cursor_y: f32,
        float_left: &mut Option<ActiveFloat>,
        float_right: &mut Option<ActiveFloat>,
        font_set: Option<&FontSet>,
        images: &ImageMap,
    ) {
        const LINE_HEIGHT_FALLBACK: f32 = 22.0;

        child.containing_width = inner_width;
        let margin = resolve_margin(&child.computed_style, inner_width);
        child.box_dimensions.margin = margin;

        let width = resolve_block_width(&child.computed_style, DEFAULT_FLOAT_WIDTH);
        child.dimensions.x = match side {
            FloatSide::Left => origin_x + margin.left,
            FloatSide::Right => origin_x + inner_width - width - margin.right,
        };
        child.dimensions.y = cursor_y + margin.top;
        child.dimensions.width = width;
        apply_relative_offset(child);

        let content_height = Self::flow_block_children(child, font_set, images);
        let child_padding = child.box_dimensions.padding;
        let child_border = child.box_dimensions.border;
        let explicit_content_height = child.computed_style.get("height").and_then(|v| parse_css_length(v));
        let resolved_content_height = explicit_content_height.unwrap_or(content_height.max(LINE_HEIGHT_FALLBACK));
        child.dimensions.height = resolved_content_height + child_padding.top + child_padding.bottom + child_border.top + child_border.bottom;
        child.box_dimensions.content = Rect {
            x: child.dimensions.x + child_border.left + child_padding.left,
            y: child.dimensions.y + child_border.top + child_padding.top,
            width: width - child_border.left - child_border.right - child_padding.left - child_padding.right,
            height: resolved_content_height,
        };

        let active = ActiveFloat {
            edge: match side {
                FloatSide::Left => child.dimensions.x + child.dimensions.width + margin.right,
                FloatSide::Right => child.dimensions.x - margin.left,
            },
            bottom_y: child.dimensions.y + child.dimensions.height + margin.bottom,
        };
        match side {
            FloatSide::Left => *float_left = Some(active),
            FloatSide::Right => *float_right = Some(active),
        }
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
        FLEX_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let padding = resolve_padding(&container.computed_style, container.containing_width);
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
        for (index, child) in container.children.iter_mut().enumerate() {
            if is_out_of_flow(&child.computed_style) {
                continue;
            }
            // Se fija ANTES de medir, no solo al comprometer la posicion
            // final (mas abajo): `measure_flex_item` recursa en
            // `flow_block_children`, que resuelve el PROPIO padding/height
            // en porcentaje del item contra `containing_width` - con el
            // valor por defecto (0.0) de una caja recien creada, un item con
            // `padding: 5%` mediria con padding cero durante la pasada de
            // medida de taffy, y el tamaño que taffy comete quedaria
            // pequeño de mas aunque la segunda pasada corrija el padding
            // pintado despues.
            child.containing_width = inner_width;
            let margin = resolve_margin(&child.computed_style, inner_width);
            let style = flex_item_style(&child.computed_style, margin);
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
            child.containing_width = inner_width;
            // Ver el mismo comentario en `flow_grid_children`: puebla el
            // margen que taffy ya uso para colocar el item, no solo el
            // `Style` que se le paso.
            child.box_dimensions.margin = resolve_margin(&child.computed_style, inner_width);
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

    /// Layout real de `display: grid` (Fase 3) - delegado al motor CSS Grid de `taffy`.
    fn flow_grid_children(container: &mut LayoutBox, font_set: Option<&FontSet>, images: &ImageMap) -> f32 {
        GRID_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let padding = resolve_padding(&container.computed_style, container.containing_width);
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

        // Las areas con nombre del contenedor: hacen falta para colocar cada
        // item (`grid-area: pageContent`), asi que se leen una vez aqui y no
        // por hijo.
        let areas = container
            .computed_style
            .get("grid-template-areas")
            .map(|v| parse_grid_template_areas(v))
            .unwrap_or_default();

        let mut taffy_tree: taffy::TaffyTree<usize> = taffy::TaffyTree::new();
        let mut child_node_ids: Vec<(usize, taffy::NodeId)> = Vec::with_capacity(container.children.len());
        for (index, child) in container.children.iter_mut().enumerate() {
            if is_out_of_flow(&child.computed_style) {
                continue;
            }
            // Mismo motivo que en `flow_flex_children`: hace falta ANTES de
            // que taffy mida, no solo al comprometer la posicion final.
            child.containing_width = inner_width;
            let margin = resolve_margin(&child.computed_style, inner_width);
            let style = grid_item_style(&child.computed_style, &areas, margin);
            let node_id = taffy_tree
                .new_leaf_with_context(style, index)
                .expect("crear nodo hoja de taffy para grid");
            child_node_ids.push((index, node_id));
        }

        let explicit_container_height = container.computed_style.get("height").and_then(|v| parse_css_length(v));
        let mut root_style = grid_container_style(&container.computed_style);
        root_style.size.width = taffy::style_helpers::length(inner_width);
        if let Some(h) = explicit_container_height {
            root_style.size.height = taffy::style_helpers::length(h);
        }
        let grid_node_ids: Vec<taffy::NodeId> = child_node_ids.iter().map(|(_, id)| *id).collect();
        let root_id = taffy_tree
            .new_with_children(root_style, &grid_node_ids)
            .expect("crear nodo contenedor grid de taffy");

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
            .expect("compute_layout_with_measure para grid");

        let mut max_bottom = origin_y;
        for (index, node_id) in &child_node_ids {
            let layout = *taffy_tree.layout(*node_id).expect("layout deberia existir tras compute_layout_with_measure");
            let child = &mut container.children[*index];
            child.containing_width = inner_width;
            // `box_dimensions.margin` tambien se puebla aqui, no solo el
            // `Style` que taffy ya consumio - es lo que `getBoundingClientRect`/
            // `margin_box()` leen despues, y sin esto un item flex/grid
            // reportaba margen cero aunque taffy ya lo hubiera respetado al
            // colocarlo.
            child.box_dimensions.margin = resolve_margin(&child.computed_style, inner_width);
            child.dimensions.x = origin_x + layout.location.x;
            child.dimensions.y = origin_y + layout.location.y;
            child.dimensions.width = layout.size.width;
            child.dimensions.height = layout.size.height;
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
        TABLE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let padding = resolve_padding(&container.computed_style, container.containing_width);
        let border = resolve_border_width(&container.computed_style);
        container.box_dimensions.padding = padding;
        container.box_dimensions.border = border;

        let inset_left = border.left + padding.left;
        let inset_right = border.right + padding.right;
        let inset_top = border.top + padding.top;

        let origin_x = container.dimensions.x + inset_left;
        let inner_width = (container.dimensions.width - inset_left - inset_right).max(0.0);
        let content_top = container.dimensions.y + inset_top;

        // Hijos directos que no aportan NINGUNA fila (un `<caption>`, o
        // contenido suelto dentro de la tabla) se maquetan como bloque
        // ENCIMA de las filas: es donde un navegador real pinta un caption
        // por defecto (`caption-side: top` es el valor inicial), y para el
        // contenido suelto es la aproximacion mas cercana a la celda
        // anonima que el spec generaria. Antes no se colocaban en ningun
        // sitio.
        let mut caption_height = 0.0;
        for index in 0..container.children.len() {
            if contributes_table_rows(&container.children[index]) {
                continue;
            }
            if is_table_cell(&container.children[index]) {
                continue;
            }
            // Un `position: absolute`/`fixed` NO ocupa espacio de flujo -
            // `resolve_positioned_boxes` lo coloca aparte, DESPUES, igual
            // que hace `flow_normal_block_children`/`flow_flex_children`
            // con cualquier otro hijo fuera de flujo. Sin este corte, su
            // alto se sumaba a `caption_height` y empujaba hacia abajo TODA
            // fila real de la tabla - aunque esa posicion se sobreescribiera
            // despues, el desplazamiento que ya habia provocado en el resto
            // del contenido no se deshacia.
            if is_out_of_flow(&container.children[index].computed_style) {
                // Misma posicion estatica que anota `flow_normal_block_
                // children` para cualquier otro hijo fuera de flujo -
                // `resolve_positioned_boxes` la usa cuando `left`/`top`
                // estan en `auto`. Sin apuntarla, un absoluto sin
                // `left`/`top` dentro de una tabla caia a la esquina del
                // bloque contenedor en vez de a donde el flujo lo habria
                // dejado.
                container.children[index].static_position = Some((origin_x, content_top + caption_height));
                continue;
            }
            let child = &mut container.children[index];
            child.containing_width = inner_width;
            child.dimensions.x = origin_x;
            child.dimensions.y = content_top + caption_height;
            child.dimensions.width = inner_width;
            let content_height = Self::flow_normal_block_children(child, font_set, images);
            let child_padding = child.box_dimensions.padding;
            let child_border = child.box_dimensions.border;
            let explicit_height = child.computed_style.get("height").and_then(|v| parse_css_length(v));
            child.dimensions.height = explicit_height.unwrap_or(content_height)
                + child_padding.top
                + child_padding.bottom
                + child_border.top
                + child_border.bottom;
            caption_height += child.dimensions.height;
        }
        let content_top = content_top + caption_height;

        let rows = collect_table_rows(container);
        if rows.is_empty() {
            // Una caja `display: table` SIN ninguna `table-row` dentro sigue
            // teniendo contenido que hay que colocar. Devolver 0 aqui (lo
            // que se hacia antes) dejaba ese contenido entero sin posicionar
            // en `Rect::default()`, o sea en la esquina (0,0) del viewport,
            // donde ademas se PINTA: en la Wikipedia real eso amontonaba
            // cientos de pies de foto y enlaces unos sobre otros en una
            // franja ilegible pegada al borde superior de la pagina.
            //
            // El caso es de lo mas comun: MediaWiki maqueta cada miniatura
            // con `figure { display: table }` + `figcaption { display:
            // table-caption }`, sin ninguna fila de por medio.
            //
            // Un navegador real envuelve ese contenido en cajas anonimas de
            // fila y celda; el equivalente honesto aqui, sin inventarse
            // cajas nuevas en el arbol, es maquetarlo con el flujo de bloque
            // normal - una celda anonima que ocupa la tabla entera se
            // comporta exactamente asi.
            return Self::flow_normal_block_children(container, font_set, images);
        }

        // El numero de columnas es el maximo de columnas OCUPADAS por una
        // fila, no el de celdas: una celda con `colspan="2"` ocupa dos.
        let column_count = rows
            .iter()
            .map(|row| row.children.iter().filter(|c| is_table_cell(c)).map(|c| c.colspan as usize).sum::<usize>())
            .max()
            .unwrap_or(0)
            .max(1);

        // Anchura minima y maxima de contenido de cada columna: el maximo,
        // columna a columna, de lo que pide cada celda de esa columna. Las
        // medidas van a la cache de la propia celda, asi que recorrer las
        // filas una vez mas aqui no las vuelve a calcular despues.
        let mut min_widths = vec![0.0_f32; column_count];
        let mut max_widths = vec![0.0_f32; column_count];
        let mut rows = rows;
        // Primera pasada: solo las celdas de UNA columna, que son las unicas
        // que atribuyen su anchura a una columna concreta sin ambiguedad.
        // Las que abarcan varias se reparten despues, cuando ya se sabe
        // cuanto piden las columnas por su cuenta.
        let mut spanning: Vec<(usize, u32, f32, f32)> = Vec::new();
        // Misma ocupacion por `rowspan` que la pasada de POSICIONAMIENTO de
        // mas abajo (ver `ocupadas` alli) - hace falta TAMBIEN aqui: sin
        // ella, esta pasada de medida no sabe que una celda con rowspan
        // sigue bloqueando su columna en las filas siguientes, y le
        // atribuye el ancho de la primera celda de esas filas a la columna
        // EQUIVOCADA (la que sigue ocupada, no la que esa celda ocupara de
        // verdad una vez colocada).
        let mut ocupadas_medida: Vec<u32> = vec![0; column_count];
        for row in rows.iter_mut() {
            let mut col = 0usize;
            for cell in row.children.iter_mut().filter(|c| is_table_cell(c)) {
                while col < column_count && ocupadas_medida[col] > 0 {
                    col += 1;
                }
                if col >= column_count {
                    break;
                }
                let span = (cell.colspan as usize).min(column_count - col);
                let min = measure_intrinsic_width(cell, IntrinsicWidth::Min, font_set, images);
                let max = measure_intrinsic_width(cell, IntrinsicWidth::Max, font_set, images);
                if span <= 1 {
                    min_widths[col] = min_widths[col].max(min);
                    max_widths[col] = max_widths[col].max(max);
                } else {
                    spanning.push((col, span as u32, min, max));
                }
                let filas_abarcadas = cell.rowspan.max(1);
                if filas_abarcadas > 1 {
                    for c in col..(col + span).min(column_count) {
                        ocupadas_medida[c] = filas_abarcadas;
                    }
                }
                col += span.max(1);
            }
            for o in ocupadas_medida.iter_mut() {
                *o = o.saturating_sub(1);
            }
        }
        // Segunda pasada: una celda que abarca varias columnas solo obliga a
        // ensanchar si lo que ya piden esas columnas juntas no le basta, y
        // entonces el defecto se reparte a partes iguales entre ellas.
        for (start, span, min, max) in spanning {
            let span = span as usize;
            let range = start..(start + span).min(column_count);
            let current_min: f32 = range.clone().map(|i| min_widths[i]).sum();
            if min > current_min {
                let share = (min - current_min) / span as f32;
                for i in range.clone() {
                    min_widths[i] += share;
                }
            }
            let current_max: f32 = range.clone().map(|i| max_widths[i]).sum();
            if max > current_max {
                let share = (max - current_max) / span as f32;
                for i in range {
                    max_widths[i] += share;
                }
            }
        }
        let column_widths = distribute_table_columns(&min_widths, &max_widths, inner_width);
        // Desplazamiento horizontal acumulado de cada columna.
        let mut column_offsets = Vec::with_capacity(column_widths.len());
        let mut accumulated = 0.0_f32;
        for w in &column_widths {
            column_offsets.push(accumulated);
            accumulated += w;
        }

        // Ocupacion arrastrada por `rowspan`: cuantas filas mas sigue ocupada
        // cada columna por una celda que empezo mas arriba. Sin esto, las
        // celdas de las filas siguientes se corren a la izquierda y toda la
        // tabla queda descuadrada a partir de la primera celda con
        // `rowspan`.
        let mut ocupadas: Vec<u32> = vec![0; column_count];
        // Para la segunda pasada: (fila, indice de celda dentro de la fila,
        // filas que abarca). El alto real de una celda con `rowspan` no se
        // sabe hasta haber medido todas las filas que cruza.
        let mut celdas_que_abarcan: Vec<(usize, usize, u32)> = Vec::new();
        let mut tops_de_fila: Vec<f32> = Vec::with_capacity(rows.len());
        let mut altos_de_fila: Vec<f32> = Vec::with_capacity(rows.len());

        let mut cursor_y = content_top;
        for (indice_fila, row) in rows.iter_mut().enumerate() {
            row.containing_width = inner_width;
            row.dimensions.x = origin_x;
            row.dimensions.y = cursor_y;
            row.dimensions.width = inner_width;

            // Una fila sin ninguna `table-cell` dentro tiene el mismo
            // problema que una tabla sin filas (ver arriba): su contenido se
            // quedaria sin posicionar. Se maqueta como bloque, que es lo que
            // hace la celda anonima que un navegador real generaria.
            if !row.children.iter().any(is_table_cell) {
                let height = Self::flow_normal_block_children(row, font_set, images);
                row.dimensions.height = height;
                // Tambien apunta su top/alto: la segunda pasada indexa estos
                // vectores POR NUMERO DE FILA, y saltarse una los
                // desalinearia con todas las de despues.
                tops_de_fila.push(cursor_y);
                altos_de_fila.push(height);
                cursor_y += height;
                // Esta fila tambien tiene que consumir una fila de
                // ocupacion pendiente de un `rowspan` anterior - sin esto,
                // una fila SIN celdas (poco comun pero valida) desalineaba
                // la cuenta en una fila para todas las de despues.
                for o in ocupadas.iter_mut() {
                    *o = o.saturating_sub(1);
                }
                continue;
            }

            let mut row_height: f32 = 0.0;
            let mut col = 0usize;
            for (indice_celda, cell) in row.children.iter_mut().filter(|c| is_table_cell(c)).enumerate() {
                // Saltar las columnas que sigue ocupando una celda de una
                // fila anterior.
                while col < column_count && ocupadas[col] > 0 {
                    col += 1;
                }
                if col >= column_count {
                    break;
                }
                let span = (cell.colspan as usize).clamp(1, column_count - col);
                let filas_abarcadas = cell.rowspan.max(1);
                if filas_abarcadas > 1 {
                    for c in col..(col + span).min(column_count) {
                        // El contador se decrementa al CERRAR cada fila,
                        // incluida esta, asi que se apunta el numero total de
                        // filas y no una menos: si no, la ocupacion se agota
                        // justo antes de la primera fila que deberia saltar.
                        ocupadas[c] = filas_abarcadas;
                    }
                    celdas_que_abarcan.push((indice_fila, indice_celda, filas_abarcadas));
                }
                // Una celda con `colspan` ocupa el ancho SUMADO de todas las
                // columnas que abarca.
                let column_width: f32 = (col..col + span).map(|i| column_widths.get(i).copied().unwrap_or(0.0)).sum();
                cell.dimensions.x = origin_x + column_offsets.get(col).copied().unwrap_or(0.0);
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
                col += span;
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
            tops_de_fila.push(cursor_y);
            altos_de_fila.push(row_height);
            cursor_y += row_height;

            // Consumir una fila de ocupacion pendiente.
            for o in ocupadas.iter_mut() {
                *o = o.saturating_sub(1);
            }
        }

        // Segunda pasada: una celda con `rowspan` llega hasta el borde
        // inferior de la ultima fila que abarca. Hasta aqui tenia el alto de
        // su propia fila, porque las siguientes todavia no estaban medidas.
        for (indice_fila, indice_celda, filas) in celdas_que_abarcan {
            let ultima = (indice_fila + filas as usize - 1).min(altos_de_fila.len().saturating_sub(1));
            let fondo = tops_de_fila[ultima] + altos_de_fila[ultima];
            let alto = (fondo - tops_de_fila[indice_fila]).max(0.0);
            if let Some(cell) = rows[indice_fila].children.iter_mut().filter(|c| is_table_cell(c)).nth(indice_celda) {
                cell.dimensions.height = alto;
                let p = cell.box_dimensions.padding;
                let b = cell.box_dimensions.border;
                cell.box_dimensions.content.height = (alto - p.top - p.bottom - b.top - b.bottom).max(0.0);
            }
        }

        // `content_top` ya viene desplazado por el alto de los captions, asi
        // que se vuelve a sumar para que el alto devuelto sea el de la
        // tabla COMPLETA (caption incluido) y no solo el de sus filas.
        (cursor_y - content_top).max(0.0) + caption_height
    }

    fn is_inline_level(b: &LayoutBox) -> bool {
        matches!(b.box_type, BoxType::Text(_) | BoxType::Inline | BoxType::Image(_) | BoxType::Replaced)
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
    fn flow_inline_run(nodes: &mut [LayoutBox], origin_x: f32, inner_width: f32, start_y: f32, font_set: Option<&FontSet>, images: &ImageMap, text_align: TextAlign) -> f32 {
        INLINE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        const LINE_HEIGHT_FALLBACK: f32 = 22.0;

        let text_line_height = match font_set {
            Some(set) => {
                let (font_size, bold, italic) = Self::first_leaf_font_info(nodes).unwrap_or((INITIAL_FONT_SIZE, false, false));
                match set.pick(bold, italic) {
                    Some(f) => engine_text::line_height(f, font_size),
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
        if text_align != TextAlign::Left {
            Self::apply_text_align(nodes, origin_x, inner_width, text_align);
        }
        cursor_y + line_extent
    }

    /// Desplaza cada LINEA ya posicionada de `nodes` (Fase 31) segun
    /// `text_align` - agrupa por `dimensions.y` (todos los nodos de una
    /// misma linea comparten el MISMO `cursor_y` de origen, sin deriva de
    /// punto flotante entre hermanos: viene siempre del mismo acumulador),
    /// calcula cuanto ancho REAL ocupo esa linea, y mueve el grupo entero
    /// (recursivamente, para arrastrar tambien a los hijos de un `<b>`/
    /// `<i>` ya posicionados dentro) el hueco que sobra.
    ///
    /// Cubre el caso mas comun de `text-align` (una racha que cabe en una
    /// o varias lineas SIN que ningun nodo individual necesite envolverse
    /// por dentro): un texto tan largo que un SOLO nodo de texto envuelve
    /// varias lineas dentro de su propia caja (`place_inline_node`, rama
    /// "ni siquiera cabe sola") queda con `width == inner_width` siempre
    /// - el desplazamiento que esta funcion calcularia para el ya seria
    /// cero, asi que no hace falta ni excluirlo aparte; ESE caso se
    /// centra/alinea por LINEA en `engine-gfx::paint_text`, no aqui (ver
    /// su aviso). Los nodos fuera de flujo (`position: absolute/fixed`)
    /// se saltan por completo - `place_inline_node` los deja sin
    /// posicionar, y agruparlos por su `Rect::default()` compartido
    /// mezclaria lineas reales entre si.
    fn apply_text_align(nodes: &mut [LayoutBox], origin_x: f32, inner_width: f32, text_align: TextAlign) {
        let mut i = 0;
        while i < nodes.len() {
            if is_out_of_flow(&nodes[i].computed_style) {
                i += 1;
                continue;
            }
            let line_y = nodes[i].dimensions.y;
            let mut j = i + 1;
            while j < nodes.len() && (is_out_of_flow(&nodes[j].computed_style) || nodes[j].dimensions.y == line_y) {
                j += 1;
            }

            let line_right_edge = nodes[i..j]
                .iter()
                .filter(|n| !is_out_of_flow(&n.computed_style))
                .map(|n| n.dimensions.x + n.dimensions.width)
                .fold(f32::MIN, f32::max);
            if line_right_edge > f32::MIN {
                let used_width = line_right_edge - origin_x;
                let offset = match text_align {
                    TextAlign::Center => ((inner_width - used_width) / 2.0).max(0.0),
                    TextAlign::Right => (inner_width - used_width).max(0.0),
                    TextAlign::Left => 0.0,
                };
                if offset > 0.0 {
                    for node in &mut nodes[i..j] {
                        if !is_out_of_flow(&node.computed_style) {
                            Self::shift_subtree_x(node, offset);
                        }
                    }
                }
            }
            i = j;
        }
    }

    /// Mueve `node` (y TODOS sus descendientes ya posicionados) `offset`
    /// pixeles en X - lo que hace que desplazar un `<b>`/`<i>` inline
    /// arrastre tambien el texto que contiene, en vez de mover solo su
    /// rectangulo delimitador y dejar el contenido real donde estaba.
    fn shift_subtree_x(node: &mut LayoutBox, offset: f32) {
        node.dimensions.x += offset;
        for child in &mut node.children {
            Self::shift_subtree_x(child, offset);
        }
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
                BoxType::Block | BoxType::Image(_) | BoxType::Replaced => {}
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
                    // `wrapped_line_width` y no `measure_text`: tiene que ser
                    // la misma aritmetica con la que `engine-gfx` repartira
                    // este texto en lineas (ver su doc-comment).
                    Some(f) => engine_text::wrapped_line_width(f, content, font_size),
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
                // `padding`/`border` de elementos inline (span/a/b/button...)
                // SI se resuelven: el spec real solo les da efecto HORIZONTAL
                // sobre el flujo (empujan donde arrancan/terminan los hijos
                // en la linea; el vertical no reserva mas alto de linea ni
                // desplaza a los hermanos), pero SI se pintan verticalmente
                // (el fondo/borde crece simetrico arriba/abajo desde el
                // texto) - es como un boton real con `padding` se ve
                // "relleno" sin que la linea entera se separe de la
                // siguiente. `margin` de un inline sigue sin resolverse
                // (mismo criterio: horizontal-only en el spec real, caso
                // raro fuera de `button`).
                let padding = resolve_padding(&node.computed_style, inner_width);
                let border = resolve_border_width(&node.computed_style);
                let start_x = *cursor_x;
                let start_y = *cursor_y;
                *cursor_x += padding.left + border.left;
                for child in &mut node.children {
                    Self::place_inline_node(child, origin_x, inner_width, text_line_height, line_extent, cursor_x, cursor_y, font_set, images);
                }
                *cursor_x += padding.right + border.right;
                node.box_dimensions.padding = padding;
                node.box_dimensions.border = border;
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
                    y: start_y - padding.top - border.top,
                    width: (*cursor_x - start_x).max(0.0),
                    height: (*cursor_y - start_y) + *line_extent + padding.top + padding.bottom + border.top + border.bottom,
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
                *line_extent = line_extent.max(height);
            }
            BoxType::Replaced => {
                // `<input>`/`<select>`/`<textarea>` (Fase 11) - mismo
                // criterio ATOMICO que `BoxType::Image` justo arriba (no
                // se parte en trozos mas pequeños), pero el tamaño sale
                // SIEMPRE de CSS (`resolve_replaced_dimensions`, sin
                // ningun bitmap "natural" del que partir). NO recursa en
                // `node.children` a proposito: un `<select>` SI tiene
                // cajas hijas reales para cada `<option>` (`build_node`
                // las crea igual que para cualquier otro elemento, ver
                // tree.rs), pero aqui se dejan sin posicionar (se quedan
                // en `Rect::default()`) - un desplegable no expone sus
                // opciones como contenido normal de la pagina, ni un
                // navegador real las pinta ahi.
                let explicit_width = node.computed_style.get("width").and_then(|v| parse_css_length(v));
                let explicit_height = node.computed_style.get("height").and_then(|v| parse_css_length(v));
                let (width, height) = resolve_replaced_dimensions(explicit_width, explicit_height);

                let remaining = origin_x + inner_width - *cursor_x;
                if width > remaining && *cursor_x > origin_x {
                    *cursor_y += *line_extent;
                    *cursor_x = origin_x;
                    *line_extent = text_line_height;
                }

                node.dimensions = Rect { x: *cursor_x, y: *cursor_y, width, height };
                *cursor_x += width;
                *line_extent = line_extent.max(height);
            }
            BoxType::Block => {
                // En HTML5 moderno (como en Google o Wikipedia), un elemento inline (ej. <a>)
                // puede contener elementos de bloque (ej. <div> o <p>).
                // En vez de crashear el motor con un panic, se salta a una linea nueva y se fluye como bloque.
                if *cursor_x > origin_x {
                    *cursor_y += *line_extent;
                    *cursor_x = origin_x;
                    *line_extent = text_line_height;
                }
                node.dimensions.x = origin_x;
                node.dimensions.y = *cursor_y;
                node.dimensions.width = inner_width;
                node.containing_width = inner_width;
                let block_height = Self::flow_block_children(node, font_set, images);
                let padding = resolve_padding(&node.computed_style, inner_width);
                let border = resolve_border_width(&node.computed_style);
                node.box_dimensions.padding = padding;
                node.box_dimensions.border = border;
                let explicit_height = node.computed_style.get("height").and_then(|v| parse_css_length(v));
                let total_h = explicit_height.unwrap_or(block_height) + padding.top + padding.bottom + border.top + border.bottom;
                node.dimensions.height = total_h;
                *cursor_y += total_h;
                *cursor_x = origin_x;
                *line_extent = text_line_height;
            }
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

    /// Igual que `find_box_for_dom_node` pero prestando la caja de forma
    /// MUTABLE - lo necesitan los tests que llaman a `measure_intrinsic_
    /// width`, que escribe en la cache de medidas de la propia caja.
    fn find_box_for_dom_node_mut<'a>(root: &'a mut LayoutBox, target: &Arc<RwLock<Node>>) -> Option<&'a mut LayoutBox> {
        if let Some(node) = &root.dom_node {
            if Arc::ptr_eq(node, target) {
                return Some(root);
            }
        }
        root.children.iter_mut().find_map(|c| find_box_for_dom_node_mut(c, target))
    }

    /// `em`, `rem`, `vw` y `calc()` tienen que llegar al layout ya en
    /// pixeles. Cuando no se resolvian, la declaracion no parseaba como
    /// longitud y la caja caia a "ocupo todo el contenedor": cualquier
    /// diseño de varias columnas se renderizaba como una sola a ancho
    /// completo.
    #[test]
    fn las_unidades_relativas_y_calc_se_resuelven_a_pixeles() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="em">e</div><div id="vw">v</div><div id="calc">c</div></body></html>"#,
        );
        let stylesheet = CssParser::parse(
            "body { margin: 0px; font-size: 20px; } #em { width: 10em; } #vw { width: 25vw; } #calc { width: calc(100px + 50px); }",
        );
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let ancho = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja").dimensions.width;

        assert_eq!(ancho("em"), 200.0, "10em con font-size 20px son 200px");
        assert_eq!(ancho("vw"), 200.0, "25vw de un viewport de 800px son 200px");
        assert_eq!(ancho("calc"), 150.0, "calc(100px + 50px) son 150px");
    }

    /// Los porcentajes se miden contra el BLOQUE CONTENEDOR, tambien
    /// anidados. No se pueden resolver junto a `em` (ahi todavia no hay
    /// bloque contenedor), de ahi que vivan en la maquetacion.
    #[test]
    fn los_porcentajes_se_miden_contra_el_bloque_contenedor() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="mitad"><div id="cuarto">c</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } #mitad { width: 50%; } #cuarto { width: 50%; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let ancho = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja").dimensions.width;

        assert_eq!(ancho("mitad"), 400.0, "50% de un viewport de 800px");
        assert_eq!(ancho("cuarto"), 200.0, "50% del PADRE (400px), no del viewport");
    }

    /// Un `padding` en porcentaje se mide contra el ANCHO del bloque
    /// contenedor por los cuatro lados, tambien arriba y abajo - asi lo
    /// define el spec, para que un `padding: 5%` produzca un hueco cuadrado.
    #[test]
    fn un_padding_en_porcentaje_se_mide_contra_el_ancho() {
        let dom = HtmlParser::parse(r#"<html><body><div id="caja">c</div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; } #caja { width: 100px; padding: 10%; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let caja = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "caja").unwrap()).expect("caja");

        // 10% de 800 = 80 por lado; el ancho de borde suma los dos.
        assert_eq!(caja.dimensions.width, 260.0, "100px de contenido mas 80px de padding a cada lado");
        assert_eq!(caja.box_dimensions.padding.top, 80.0, "el padding vertical tambien se mide contra el ANCHO");
    }

    /// `height: 50%` necesita que el contenedor tenga un alto DEFINIDO. Si
    /// no lo tiene (`height: auto`, lo normal), el spec dice que se comporta
    /// como `auto` - y devolver eso es mas correcto que inventarse un alto.
    #[test]
    fn un_alto_en_porcentaje_solo_aplica_si_el_contenedor_lo_tiene_definido() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="fijo"><div id="dentro">d</div></div><div id="suelto"><div id="huerfano">h</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse(
            "body { margin: 0px; } div { margin: 0px; } #fijo { height: 400px; } #dentro { height: 50%; } #huerfano { height: 50%; }",
        );
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let alto = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja").dimensions.height;

        assert_eq!(alto("dentro"), 200.0, "50% de un contenedor de 400px");
        assert!(alto("huerfano") < 100.0, "sin alto definido en el padre, el 50% no aplica y la caja crece con su contenido");
    }

    /// `min-height`/`max-height` acotan tanto un alto declarado como el que
    /// sale del contenido.
    #[test]
    fn min_height_y_max_height_acotan_el_alto() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="tope">t</div><div id="suelo">s</div></body></html>"#,
        );
        let stylesheet = CssParser::parse(
            "body { margin: 0px; } div { margin: 0px; } #tope { height: 900px; max-height: 120px; } #suelo { height: 5px; min-height: 60px; }",
        );
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let alto = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja").dimensions.height;

        assert_eq!(alto("tope"), 120.0, "max-height recorta un height mayor");
        assert_eq!(alto("suelo"), 60.0, "min-height eleva un height menor");
    }

    /// Una celda con `rowspan` ocupa su columna tambien en las filas
    /// siguientes: las celdas de esas filas NO deben correrse a su hueco.
    /// Ignorarlo descuadra la tabla entera a partir de la primera celda que
    /// abarque filas.
    #[test]
    fn una_celda_con_rowspan_reserva_su_columna_en_las_filas_siguientes() {
        let dom = HtmlParser::parse(
            r#"<html><body><table>
               <tr><td id="alta" rowspan="2">alta</td><td id="a1">a1</td></tr>
               <tr><td id="b1">b1</td></tr>
               <tr><td id="c1">c1</td><td id="c2">c2</td></tr>
               </table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } table { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let caja = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja");

        let a1 = caja("a1").dimensions.x;
        assert!(a1 > 0.0, "a1 esta en la segunda columna");
        assert_eq!(caja("b1").dimensions.x, a1, "b1 deberia quedarse en la SEGUNDA columna, no correrse al hueco de la celda alta");
        assert_eq!(caja("c1").dimensions.x, 0.0, "pasado el rowspan, la fila vuelve a empezar en la primera columna");
        assert!(
            caja("alta").dimensions.height > caja("a1").dimensions.height * 1.5,
            "la celda con rowspan=2 deberia llegar hasta el fondo de la segunda fila"
        );
    }

    /// `max-height`/`min-height` declaran un valor de BORDER-BOX cuando
    /// `box-sizing: border-box` esta activo, igual que `height` - pero el
    /// alto que se acota en la rama `height: auto` es CONTENIDO puro. Sin
    /// convertir el limite, un `max-height` con padding se quedaba corto:
    /// el `dimensions.height` final superaba el `max-height` declarado en
    /// el padding+border que nadie habia restado.
    #[test]
    fn max_height_border_box_incluye_el_padding_en_el_limite() {
        // Tres bloques de 60px (180px de contenido en total, sin depender
        // de wrap de texto ni de tener una fuente real) desbordan con
        // creces cualquier limite razonable.
        let dom = HtmlParser::parse(
            r#"<html><body><div id="caja"><div style="height: 60px;"></div><div style="height: 60px;"></div><div style="height: 60px;"></div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse(
            "body { margin: 0px; } div { margin: 0px; } #caja { box-sizing: border-box; max-height: 120px; padding: 20px 0px; }",
        );
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let caja = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "caja").unwrap()).expect("caja");

        assert_eq!(
            caja.dimensions.height, 120.0,
            "border-box: 120px de max-height es el alto TOTAL (contenido + 40px de padding), no 120px de contenido mas el padding encima"
        );
    }

    /// El bucle que coloca el contenido "suelto" de una tabla (lo que no es
    /// ni fila ni celda - un `<caption>`, por ejemplo) tenia que saltarse
    /// los hijos FUERA DE FLUJO igual que hace `flow_normal_block_children`
    /// con cualquier otro contenido: un `position: absolute` no ocupa
    /// espacio de flujo, y sumar su alto a `caption_height` empujaba TODA
    /// fila real hacia abajo aunque `resolve_positioned_boxes` fuera a
    /// reposicionarlo despues - el desplazamiento que ya habia provocado en
    /// el resto del contenido no se deshacia.
    ///
    /// Se usa `display: table` sobre `<div>`s normales (no `<table>` real):
    /// el parser HTML reubica ("foster parenting") cualquier `<div>` que no
    /// sea celda/fila puesto directamente dentro de un `<table>` de
    /// verdad, lo que taparia el propio escenario que este test quiere
    /// comprobar.
    #[test]
    fn el_contenido_fuera_de_flujo_de_una_tabla_no_empuja_las_filas() {
        // Sin ningun espacio en blanco entre etiquetas a proposito: un hueco
        // se parsearia como un nodo de TEXTO (whitespace) hijo directo del
        // contenedor, que caeria en el mismo bucle de "contenido suelto"
        // que esta prueba quiere aislar y confundiria el resultado.
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: table; width: 200px;"><div style="position: absolute;"><div style="height: 500px;"></div></div><div style="display: table-row;"><div id="celda" style="display: table-cell;">x</div></div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let celda = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "celda").unwrap()).expect("caja");

        assert!(
            celda.dimensions.y < 10.0,
            "la celda deberia empezar cerca de y=0 - el hermano fuera de flujo (500px de alto) no deberia empujarla hacia abajo; midio y={}",
            celda.dimensions.y
        );
    }

    /// La pasada que MIDE min/max-content de cada columna tiene que
    /// respetar la misma ocupacion por `rowspan` que la pasada que las
    /// COLOCA - si no, atribuye el contenido de una celda a la columna
    /// equivocada (la que sigue ocupada, no la que esa celda ocupara de
    /// verdad una vez colocada).
    #[test]
    fn la_medida_de_columnas_respeta_la_ocupacion_por_rowspan() {
        let dom = HtmlParser::parse(
            r#"<html><body><table>
               <tr><td id="a" rowspan="2">a</td><td id="b0">b0</td></tr>
               <tr><td id="d">contenidoMuyLargoQueDeberiaEstarEnLaColumnaUno</td></tr>
               <tr><td id="e">e0</td><td id="f">f1</td></tr>
               </table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } table { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let caja = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja");

        // `e` ocupa en solitario la columna 0 en la fila 3, ya fuera del
        // rowspan de `a` - su ancho ES el ancho real que la tabla decidio
        // para esa columna. `d` deberia caer en la columna 1 (la 0 sigue
        // ocupada por `a`); si la medida no lo sabe, le atribuye a la
        // columna 0 su contenido largo, y esa columna sale mucho mas ancha
        // de lo que "a"/"e0" (ambos cortos) piden.
        assert!(
            caja("e").dimensions.width < caja("f").dimensions.width,
            "columna 0 (a/e0, cortos: {}) deberia ser mas ESTRECHA que columna 1 (b0 y el contenido largo de d: {})",
            caja("e").dimensions.width,
            caja("f").dimensions.width
        );
    }

    /// Una fila sin ninguna celda (poco comun, pero valida en HTML) tiene
    /// que consumir su turno de ocupacion pendiente de un `rowspan`
    /// anterior igual que cualquier otra fila - saltarselo desalineaba la
    /// cuenta una fila para todo lo que viniera despues.
    #[test]
    fn una_fila_sin_celdas_no_desalinea_el_rowspan_de_las_siguientes() {
        let dom = HtmlParser::parse(
            r#"<html><body><table>
               <tr><td id="alta" rowspan="2">alta</td><td id="a1">a1</td></tr>
               <tr></tr>
               <tr><td id="c1">c1</td><td id="c2">c2</td></tr>
               </table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } table { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let caja = |id: &str| find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja");

        assert_eq!(
            caja("c1").dimensions.x, 0.0,
            "pasado el rowspan (y la fila vacia intermedia), la fila deberia volver a empezar en la primera columna"
        );
    }

    /// Las propiedades personalizadas (`--x`) se resuelven donde se USAN,
    /// heredando desde donde se declararon - casi siempre `:root`. Sin
    /// esto, el valor que llega al layout es la cadena literal
    /// "var(--algo)", que no parsea como longitud, y la declaracion se
    /// pierde: la pagina se maqueta como si su hoja de estilos no
    /// existiera.
    #[test]
    fn las_variables_css_se_resuelven_y_heredan_desde_la_raiz() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="a">a</div><div id="b">b</div><div id="c">c</div></body></html>"#,
        );
        let stylesheet = CssParser::parse(
            ":root { --ancho: 300px; --alias: var(--ancho); } body { margin: 0px; }              #a { width: var(--ancho); } #b { width: var(--noexiste, 150px); } #c { width: var(--alias); }",
        );
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let ancho = |id: &str| {
            find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect("nodo")).expect("caja").dimensions.width
        };

        assert_eq!(ancho("a"), 300.0, "una var() declarada en :root deberia heredarse y resolverse");
        assert_eq!(ancho("b"), 150.0, "una var() sin declarar deberia caer a su valor de respaldo");
        assert_eq!(ancho("c"), 300.0, "una var() cuyo valor es otra var() deberia resolverse en cadena");
    }

    /// Una caja de bloque vacia mide CERO de alto. Antes se le aplicaba un
    /// suelo de 22px "para no colapsarla", que en una pagina real - llena de
    /// `<div>` estructurales sin contenido propio - metia mas de 100px de
    /// hueco en blanco entre secciones.
    #[test]
    fn una_caja_de_bloque_vacia_no_ocupa_alto() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="vacio"></div><div id="siguiente" style="height: 30px;">x</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let vacio = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "vacio").unwrap()).expect("caja");
        let siguiente = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "siguiente").unwrap()).expect("caja");

        assert_eq!(vacio.dimensions.height, 0.0, "un div sin contenido mide cero de alto");
        assert_eq!(siguiente.dimensions.y, 0.0, "y por tanto no empuja hacia abajo a lo que viene despues");
    }

    /// `margin` de un item flex/grid tenia que separar a sus hermanos, no
    /// solo pintarse: `flex_item_style` nunca lo pasaba a taffy, asi que el
    /// ALGORITMO de flex los colocaba pegados sin importar el margen
    /// declarado. Es exactamente como Wikipedia separa los items de su
    /// barra de usuario (`margin: 0 4px`, no `gap`) - sin esto,
    /// "Donaciones" y "Crear una cuenta" quedaban unidos sin espacio.
    #[test]
    fn el_margin_de_un_item_flex_separa_a_sus_hermanos() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: flex; width: 300px;"><div id="a" style="margin: 0px 4px;">Uno</div><div id="b" style="margin: 0px 4px;">Dos</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let a = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "a").unwrap()).expect("caja");
        let b = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "b").unwrap()).expect("caja");

        let hueco = b.dimensions.x - (a.dimensions.x + a.dimensions.width);
        assert!(
            (hueco - 8.0).abs() < 0.5,
            "deberia haber 8px de hueco (4px margin-right de 'a' + 4px margin-left de 'b'), midio {hueco}"
        );
    }

    /// `apply_scroll_offsets` desplaza el CONTENIDO de un contenedor con
    /// scroll (no el contenedor en si), reusando el mismo mecanismo de
    /// desplazar un subarbol que ya usa `position: relative` - es lo que
    /// hace que pintura y hit-testing salgan correctos sin tocar
    /// `engine-gfx` para nada.
    #[test]
    fn apply_scroll_offsets_desplaza_el_contenido_no_el_contenedor() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="caja" style="overflow:auto;width:200px;height:100px;"><div id="uno" style="height:100px;">u</div><div id="dos" style="height:100px;">d</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let caja_node = Node::find_by_id(&dom, "caja").expect("caja");
        let uno_node = Node::find_by_id(&dom, "uno").expect("uno");
        let dos_node = Node::find_by_id(&dom, "dos").expect("dos");
        let caja_ptr = std::sync::Arc::as_ptr(&caja_node) as usize;

        let caja_y_antes = find_box_for_dom_node(&root, &caja_node).unwrap().dimensions.y;
        let uno_y_antes = find_box_for_dom_node(&root, &uno_node).unwrap().dimensions.y;

        let mut root = root;
        let mut offsets = HashMap::new();
        offsets.insert(caja_ptr, (0.0_f32, 100.0_f32));
        apply_scroll_offsets(&mut root, &offsets);

        let caja_y_despues = find_box_for_dom_node(&root, &caja_node).unwrap().dimensions.y;
        let uno_y_despues = find_box_for_dom_node(&root, &uno_node).unwrap().dimensions.y;
        let dos_y_despues = find_box_for_dom_node(&root, &dos_node).unwrap().dimensions.y;

        assert_eq!(caja_y_despues, caja_y_antes, "el CONTENEDOR no se mueve, solo su contenido");
        assert_eq!(uno_y_despues, uno_y_antes - 100.0, "el contenido se desplaza hacia arriba el scroll aplicado");
        assert_eq!(dos_y_despues, uno_y_despues + 100.0, "los hermanos mantienen su posicion relativa entre si");
    }

    /// La anchura intrinseca de un contenedor flex en FILA es la SUMA de
    /// las de sus items (van uno al lado del otro), no la del mas ancho.
    /// Medirla como flujo de bloque lo dejaba demasiado estrecho y sus
    /// propios hijos se salian por la derecha.
    #[test]
    fn la_anchura_intrinseca_de_una_fila_flex_suma_sus_items() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="fila" style="display: flex;"><div>aaaa</div><div>bbbb</div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let mut root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let fila = find_box_for_dom_node_mut(&mut root, &Node::find_by_id(&dom, "fila").unwrap()).expect("caja");
        let suma_esperada: f32 = fila
            .children
            .iter_mut()
            .map(|c| measure_intrinsic_width(c, IntrinsicWidth::Max, None, &ImageMap::new()))
            .sum();
        let medida = measure_intrinsic_width(fila, IntrinsicWidth::Max, None, &ImageMap::new());

        assert!(
            (medida - suma_esperada).abs() < 0.5,
            "la fila deberia medir la suma de sus items ({suma_esperada}), no {medida}"
        );
    }

    /// La anchura MINIMA de contenido de un texto es la de su palabra mas
    /// larga, no la de la frase entera: por debajo de esa palabra el texto
    /// desbordaria, pero por encima siempre puede cortar linea.
    ///
    /// Es la distincion que faltaba y que rompia el renderizado entero (ver
    /// `IntrinsicWidth`): con min == max, ningun item flex podia encogerse.
    #[test]
    fn la_anchura_minima_de_un_texto_es_su_palabra_mas_larga() {
        let dom = HtmlParser::parse(r#"<html><body><div id="t">uno dos tresmuchomaslarga</div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let mut root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let t_node = Node::find_by_id(&dom, "t").expect("t deberia existir");
        let t_box = find_box_for_dom_node_mut(&mut root, &t_node).expect("t deberia tener caja");
        let min = measure_intrinsic_width(t_box, IntrinsicWidth::Min, None, &ImageMap::new());
        let max = measure_intrinsic_width(t_box, IntrinsicWidth::Max, None, &ImageMap::new());

        assert!(min > 0.0, "la anchura minima no puede ser cero: hay una palabra que ocupa sitio");
        assert!(
            min < max,
            "min-content ({min}) deberia ser MENOR que max-content ({max}): la palabra mas larga cabe en mucho menos que la frase entera"
        );
    }

    /// Un item flex cuyo texto no cabe tiene que ENCOGERSE hasta su
    /// contenedor y cortar linea, no desbordarse por la derecha.
    ///
    /// Es el sintoma que se veia en cualquier web moderna: en la Wikipedia
    /// real habia cajas de 8.225 px dentro de un viewport de 1.280.
    #[test]
    fn un_item_flex_con_texto_largo_se_encoge_en_vez_de_desbordar() {
        let texto = "palabra ".repeat(200);
        let dom = HtmlParser::parse(&format!(
            r#"<html><body><div id="c" style="display: flex; width: 400px;"><div id="t">{texto}</div></div></body></html>"#
        ));
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let t_node = Node::find_by_id(&dom, "t").expect("t deberia existir");
        let t_box = find_box_for_dom_node(&root, &t_node).expect("t deberia tener caja");

        assert!(
            t_box.dimensions.width <= 400.0,
            "el item deberia caber en su contenedor de 400px, pero mide {}",
            t_box.dimensions.width
        );
    }

    /// Una caja `display: table` que no contiene ninguna fila sigue
    /// teniendo contenido que colocar. Antes se devolvia 0 sin tocar nada y
    /// ese contenido se quedaba en `Rect::default()` - o sea en (0,0), donde
    /// ademas se PINTA, amontonado sobre todo lo demas.
    ///
    /// El caso es de lo mas comun: MediaWiki maqueta cada miniatura con
    /// `figure { display: table }`, sin filas de por medio.
    #[test]
    fn una_tabla_sin_filas_sigue_colocando_su_contenido() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="height: 40px;">antes</div><figure style="display: table;"><div id="pie">pie de foto</div></figure></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div, figure { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let pie_node = Node::find_by_id(&dom, "pie").expect("pie deberia existir");
        let pie_box = find_box_for_dom_node(&root, &pie_node).expect("pie deberia tener caja");

        assert!(pie_box.dimensions.width > 0.0, "el contenido de la tabla deberia tener anchura real, no quedarse sin colocar");
        assert!(
            pie_box.dimensions.y >= 40.0,
            "el contenido deberia caer debajo del bloque de 40px que lo precede, no en la esquina (0,0); esta en y={}",
            pie_box.dimensions.y
        );
    }

    /// `position: absolute` sin `left` ni `top` se coloca en su POSICION
    /// ESTATICA - donde el flujo normal lo habria dejado - no en la esquina
    /// de su bloque contenedor. Mandarlos todos a la esquina amontonaba en
    /// (0,0) cada menu y cada tooltip de la pagina.
    #[test]
    fn un_absoluto_sin_left_ni_top_usa_su_posicion_estatica() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="height: 50px;">a</div><div style="height: 50px;">b</div><div id="abs" style="position: absolute;">c</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let abs_node = Node::find_by_id(&dom, "abs").expect("abs deberia existir");
        let abs_box = find_box_for_dom_node(&root, &abs_node).expect("abs deberia tener caja");

        assert_eq!(
            abs_box.dimensions.y, 100.0,
            "deberia quedarse donde el flujo lo habria puesto (bajo dos bloques de 50px), no en y=0"
        );
    }

    /// Las columnas de una tabla se reparten segun lo que PIDE cada una, no
    /// a partes iguales. Repartiendo por igual, una columna con un simple
    /// numero de orden se llevaba lo mismo que una con un titular entero.
    #[test]
    fn las_columnas_de_una_tabla_se_reparten_segun_su_contenido() {
        let dom = HtmlParser::parse(
            r#"<html><body><table><tr><td id="corta">1.</td><td id="larga">un titular considerablemente mas largo que el numero de al lado</td></tr></table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } table { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let corta = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "corta").unwrap()).expect("corta deberia tener caja");
        let larga = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "larga").unwrap()).expect("larga deberia tener caja");

        assert!(
            larga.dimensions.width > corta.dimensions.width * 2.0,
            "la columna del titular ({}) deberia ser bastante mas ancha que la del numero ({}), no practicamente igual",
            larga.dimensions.width,
            corta.dimensions.width
        );
        assert!(
            corta.dimensions.width > 0.0,
            "la columna estrecha sigue necesitando anchura suficiente para su contenido"
        );
    }

    /// `colspan` es un atributo presentacional de HTML, no CSS: si no se
    /// lee, la celda ocupa una sola columna y CORRE todas las siguientes de
    /// su fila una columna a la izquierda.
    #[test]
    fn una_celda_con_colspan_ocupa_las_columnas_que_declara() {
        let dom = HtmlParser::parse(
            r#"<html><body><table><tr><td id="a">a</td><td id="b">b</td></tr><tr><td id="ancha" colspan="2">ocupa las dos</td></tr></table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } table { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "a").unwrap()).expect("a deberia tener caja");
        let b = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "b").unwrap()).expect("b deberia tener caja");
        let ancha = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "ancha").unwrap()).expect("ancha deberia tener caja");

        let dos_columnas = a.dimensions.width + b.dimensions.width;
        assert!(
            (ancha.dimensions.width - dos_columnas).abs() < 0.5,
            "la celda con colspan=2 deberia medir lo que suman las dos columnas ({dos_columnas}), pero mide {}",
            ancha.dimensions.width
        );
    }

    /// `containing_width` de un item flex tiene que estar puesto ANTES de
    /// que taffy lo MIDA, no solo al comprometer su posicion final: el item
    /// mide su propio `padding` en porcentaje contra esa referencia, y con
    /// el valor por defecto (0.0) de una caja recien creada, `padding: 10%`
    /// se resolvia a CERO durante la medida - taffy comprometia un item mas
    /// bajo de lo que deberia, y la correccion posterior de
    /// `finalize_flex_item_children` llega demasiado tarde para el alto ya
    /// fijado.
    #[test]
    fn el_padding_en_porcentaje_de_un_item_flex_se_mide_con_la_referencia_correcta() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: flex; width: 400px;"><div id="item"><div style="height: 50px;"></div></div></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; } #item { padding: 10%; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let item = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "item").unwrap()).expect("caja");

        // 50px de contenido + 2 x 40px de padding (10% de los 400px del
        // contenedor) = 130px. Sin la referencia correcta durante la
        // medida, el padding se cuenta como cero y el alto se queda en 50.
        assert!(
            item.dimensions.height > 100.0,
            "el item deberia medir ~130px (contenido + padding en porcentaje), no quedarse en ~50 con el padding a cero: midio {}",
            item.dimensions.height
        );
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

    /// `display: none` no debe generar caja para el elemento en si.
    #[test]
    fn display_none_produces_no_box_for_the_element() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="oculto" style="display: none;">texto</div><div id="visible">otro</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let oculto_node = Node::find_by_id(&dom, "oculto").expect("oculto deberia existir en el DOM");
        let visible_node = Node::find_by_id(&dom, "visible").expect("visible deberia existir en el DOM");

        assert!(find_box_for_dom_node(&root, &oculto_node).is_none(), "un elemento con display:none no deberia tener ninguna caja de layout");
        assert!(find_box_for_dom_node(&root, &visible_node).is_some(), "un hermano sin display:none deberia seguir teniendo su caja normal");
    }

    /// `display: none` debe sacar TAMBIEN a los descendientes del arbol de
    /// render, no solo al elemento que lo declara - asi es el spec real.
    #[test]
    fn display_none_removes_the_whole_subtree_not_just_the_element() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="display: none;"><span id="nieto">deberia desaparecer</span></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let nieto_node = Node::find_by_id(&dom, "nieto").expect("nieto deberia existir en el DOM");
        assert!(find_box_for_dom_node(&root, &nieto_node).is_none(), "un descendiente de display:none no deberia tener caja aunque el no declare display:none el mismo");
    }

    /// A diferencia de `display: none`, un elemento SOLO con `height`
    /// declarado sigue generando caja y ocupando su espacio en el layout -
    /// distingue el corte real de `display:none` de un falso positivo por
    /// "cualquier caja con computed_style no vacio se filtra".
    #[test]
    fn an_element_without_display_none_keeps_occupying_its_layout_space() {
        let dom = HtmlParser::parse(r#"<html><body><div id="normal" style="height: 40px;">contenido</div></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let normal_node = Node::find_by_id(&dom, "normal").expect("normal deberia existir en el DOM");
        let normal_box = find_box_for_dom_node(&root, &normal_node).expect("un elemento sin display:none deberia tener caja");
        assert_eq!(normal_box.dimensions.height, 40.0);
    }

    /// El punto real de la Fase 11: antes de esto, un `<input>` sin CSS de
    /// autor ocupaba el ANCHO COMPLETO del contenedor (800px aqui) - el
    /// mismo bug exacto que la auditoria encontro ejecutando el motor en
    /// vivo. Ahora deberia tener el tamaño fijo que declara la hoja de
    /// agente de usuario, sin importar cuanto mida su contenedor.
    #[test]
    fn a_plain_input_gets_a_fixed_size_box_instead_of_filling_the_container() {
        let dom = HtmlParser::parse(r#"<html><body><input id="q"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let input_node = Node::find_by_id(&dom, "q").expect("deberia existir");
        let input_box = find_box_for_dom_node(&root, &input_node).expect("un input deberia tener caja propia");
        assert_eq!(input_box.dimensions.width, 170.0, "no deberia ocupar el ancho completo del contenedor (800px)");
        assert_eq!(input_box.dimensions.height, 21.0);
    }

    /// Un checkbox/radio es un control mucho mas pequeño que un input de
    /// texto - sin esto, el centro que calcularia un agente de IA para
    /// clicarlo estaria completamente fuera del control real.
    #[test]
    fn a_checkbox_gets_a_small_box_not_the_text_input_size() {
        let dom = HtmlParser::parse(r#"<html><body><input id="c" type="checkbox"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let checkbox_node = Node::find_by_id(&dom, "c").expect("deberia existir");
        let checkbox_box = find_box_for_dom_node(&root, &checkbox_node).expect("deberia tener caja");
        assert_eq!(checkbox_box.dimensions.width, 13.0);
        assert_eq!(checkbox_box.dimensions.height, 13.0);
    }

    /// `input[type=hidden]` usa `display: none` real en la hoja de agente
    /// de usuario (Fase 10.5 ya implementada) - un campo oculto no deberia
    /// tener NINGUNA caja, ni siquiera una de tamaño cero.
    #[test]
    fn a_hidden_input_produces_no_box_at_all() {
        let dom = HtmlParser::parse(r#"<html><body><input id="csrf" type="hidden" value="abc"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let hidden_node = Node::find_by_id(&dom, "csrf").expect("deberia existir en el DOM");
        assert!(find_box_for_dom_node(&root, &hidden_node).is_none(), "un input hidden no deberia generar ninguna caja de layout");
    }

    /// Fase 34: el punto real de esta fase - un `<input>` con `value` deberia
    /// llevar ese texto listo para pintar, no quedarse en `None` (el gap
    /// exacto que dejaba "Google Search" como un recuadro gris vacio).
    #[test]
    fn an_input_with_a_value_resolves_replaced_text_to_that_value() {
        let dom = HtmlParser::parse(r#"<html><body><input id="q" value="hola mundo"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "q").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "hola mundo");
        assert!(!replaced.is_placeholder);
    }

    /// Sin `value`, el `placeholder` es lo que se ve - pero marcado como tal
    /// (`is_placeholder`) para que se pinte en gris, no como si fuera texto
    /// real ya escrito por el usuario.
    #[test]
    fn an_input_without_a_value_falls_back_to_its_placeholder_as_dim_text() {
        let dom = HtmlParser::parse(r#"<html><body><input id="q" placeholder="Buscar..."></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "q").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "Buscar...");
        assert!(replaced.is_placeholder);
    }

    /// `input[type=submit]` sin `value` propio usa la etiqueta por defecto
    /// real del spec ("Submit Query"), no se queda vacio - y se centra
    /// (`centered`), como el aspecto nativo real de un boton.
    #[test]
    fn a_submit_input_without_a_value_gets_the_real_default_label_centered() {
        let dom = HtmlParser::parse(r#"<html><body><input id="btn" type="submit"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "btn").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "Submit Query");
        assert!(replaced.centered);
    }

    /// `input[type=submit]` con `value` propio ("Google Search") usa ESE
    /// texto en vez del por defecto - el caso real que motivo esta fase.
    #[test]
    fn a_submit_input_with_its_own_value_uses_that_label_instead_of_the_default() {
        let dom = HtmlParser::parse(r#"<html><body><input id="btn" type="submit" value="Google Search"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "btn").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "Google Search");
    }

    /// El spec real jamas pinta un `value` de `type=password` tal cual -
    /// cada caracter se sustituye por un punto de mascara, contado por
    /// caracter Unicode, no por byte.
    #[test]
    fn a_password_input_masks_its_value_with_dots() {
        let dom = HtmlParser::parse(r#"<html><body><input id="p" type="password" value="niño"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "p").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text.chars().count(), 4, "4 caracteres Unicode ('niño'), no 5 bytes (la 'ñ' ocupa 2 bytes en UTF-8)");
        assert!(replaced.text.chars().all(|c| c == '\u{2022}'));
    }

    /// Un checkbox/radio/hidden no lleva ningun texto que pintar - su
    /// aspecto nativo real no es una cadena dentro de la caja.
    #[test]
    fn a_checkbox_resolves_no_replaced_text_at_all() {
        let dom = HtmlParser::parse(r#"<html><body><input id="c" type="checkbox" value="on"></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "c").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        assert!(b.replaced_text.is_none());
    }

    /// A diferencia de `input`, el valor inicial de un `<textarea>` es su
    /// CONTENIDO DOM (un nodo de texto hijo), no un atributo `value`.
    #[test]
    fn a_textarea_resolves_replaced_text_from_its_dom_text_content() {
        let dom = HtmlParser::parse(r#"<html><body><textarea id="t">  hola desde el contenido  </textarea></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "t").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "hola desde el contenido");
        assert!(!replaced.is_placeholder);
    }

    /// Fase 35: el punto real de esta fase - un `<select>` con una
    /// `<option selected>` explicita (no necesariamente la primera) deberia
    /// mostrar ESA opcion, no la primera de la lista.
    #[test]
    fn a_select_shows_the_explicitly_selected_option_even_if_its_not_the_first() {
        let dom = HtmlParser::parse(
            r#"<html><body><select id="s"><option>Uno</option><option selected>Dos</option><option>Tres</option></select></body></html>"#,
        );
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "s").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "Dos");
    }

    /// Sin ninguna `<option selected>`, el spec real por defecto selecciona
    /// la PRIMERA opcion - mismo comportamiento que cualquier navegador
    /// real con un `<select>` sencillo.
    #[test]
    fn a_select_without_any_explicit_selected_option_defaults_to_the_first_one() {
        let dom = HtmlParser::parse(r#"<html><body><select id="s"><option>Uno</option><option>Dos</option></select></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "s").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "Uno");
    }

    /// `<option>` puede estar anidada dentro de `<optgroup>` - la busqueda
    /// tiene que alcanzar TODO el subarbol, no solo los hijos directos del
    /// `<select>`.
    #[test]
    fn a_select_finds_the_selected_option_nested_inside_an_optgroup() {
        let dom = HtmlParser::parse(
            r#"<html><body><select id="s"><optgroup label="Grupo"><option>Uno</option><option selected>Dos</option></optgroup></select></body></html>"#,
        );
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "s").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        let replaced = b.replaced_text.as_ref().expect("deberia resolver texto");
        assert_eq!(replaced.text, "Dos");
    }

    /// Un `<select>` sin ninguna `<option>` no tiene nada que mostrar - no
    /// deberia entrar en pánico ni inventarse un texto.
    #[test]
    fn a_select_with_no_options_at_all_resolves_no_replaced_text() {
        let dom = HtmlParser::parse(r#"<html><body><select id="s"></select></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "s").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        assert!(b.replaced_text.is_none());
    }

    /// `<button>` es el UNICO control que se encoge a su contenido en vez
    /// de un tamaño fijo (`BoxType::Inline`, no `Replaced` - ver el
    /// doc-comment en `build_node`) - su etiqueta real ("Enviar") deberia
    /// seguir siendo visible/medible, a diferencia de `input`/`select`.
    #[test]
    fn a_button_shrinks_to_its_text_content_instead_of_a_fixed_size() {
        let dom = HtmlParser::parse(r#"<html><body><button id="btn">Enviar</button></body></html>"#);
        let stylesheet = CssParser::parse("");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let btn_node = Node::find_by_id(&dom, "btn").expect("deberia existir");
        let btn_box = find_box_for_dom_node(&root, &btn_node).expect("deberia tener caja");
        assert!(btn_box.dimensions.width < 100.0, "un boton con 'Enviar' no deberia acercarse al ancho del contenedor (800px), deberia encogerse a su texto");
        assert!(btn_box.dimensions.width > 0.0, "deberia medir ALGO a partir de su texto, no colapsar a cero");
    }

    /// Fase 36: el punto real de esta fase - dos `<div>` con
    /// `display:inline-block` (SIN `display:flex` en el padre, el patron
    /// pre-flexbox real que motivo esta fase) deberian sentarse UNO AL
    /// LADO DEL OTRO, no apilados verticalmente como cualquier `<div>`
    /// normal.
    #[test]
    fn two_inline_block_divs_sit_side_by_side_instead_of_stacking() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="a" style="display:inline-block;">a</div><div id="b" style="display:inline-block;">b</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("deberia tener caja");

        assert_eq!(a_box.dimensions.y, b_box.dimensions.y, "ambos deberian compartir la misma linea (misma y), no apilarse");
        assert!(a_box.dimensions.width < 100.0, "deberia encogerse a su contenido ('a'), no llenar los 800px del contenedor");
    }

    /// El reverso: `display:block` sobre una etiqueta naturalmente inline
    /// (`<span>`) deberia sacarla del flujo inline - ocupando el ANCHO
    /// COMPLETO del contenedor como cualquier bloque normal, en vez de
    /// encogerse a su texto.
    #[test]
    fn a_span_with_display_block_fills_the_container_like_any_block() {
        let dom = HtmlParser::parse(r#"<html><body><span id="s" style="display:block;">hola</span></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let node = Node::find_by_id(&dom, "s").expect("deberia existir");
        let b = find_box_for_dom_node(&root, &node).expect("deberia tener caja");
        assert_eq!(b.dimensions.width, 800.0, "display:block deberia llenar el contenedor, no encogerse como un span normal");
    }

    /// Sin ningun `display` de autor, el comportamiento por defecto de
    /// siempre (etiqueta -> tipo de caja) no deberia cambiar - regresion
    /// directa contra la reclasificacion nueva de esta fase.
    #[test]
    fn without_an_author_display_override_the_tag_based_default_is_unchanged() {
        let dom = HtmlParser::parse(r#"<html><body><div id="d">bloque</div><span id="s">inline</span></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let div_node = Node::find_by_id(&dom, "d").expect("deberia existir");
        let span_node = Node::find_by_id(&dom, "s").expect("deberia existir");
        let div_box = find_box_for_dom_node(&root, &div_node).expect("deberia tener caja");
        let span_box = find_box_for_dom_node(&root, &span_node).expect("deberia tener caja");
        assert_eq!(div_box.dimensions.width, 800.0, "un div sin CSS deberia seguir llenando el contenedor, como siempre");
        assert!(span_box.dimensions.width < 100.0, "un span sin CSS deberia seguir encogiendose a su texto, como siempre");
    }

    /// Patron real muy comun: una barra de busqueda `display: flex` con un
    /// input y un boton - `measure_flex_item`/`finalize_flex_item_children`
    /// necesitan su propio camino para `BoxType::Replaced`, igual que ya
    /// tenian para `Image` (ver su doc-comment), o taffy mediria el input
    /// como si no tuviera contenido intrinseco.
    #[test]
    fn a_replaced_control_inside_a_flex_container_keeps_its_own_size() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="barra" style="display: flex;"><input id="q"><button id="btn">Ir</button></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } #barra input, #barra button { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let input_node = Node::find_by_id(&dom, "q").expect("deberia existir");
        let input_box = find_box_for_dom_node(&root, &input_node).expect("deberia tener caja dentro del flex");
        assert_eq!(input_box.dimensions.width, 170.0, "un input dentro de flex deberia conservar su tamaño de CSS, no llenar el eje principal");
        assert_eq!(input_box.dimensions.height, 21.0);
    }

    /// El punto real de la Fase 12: antes de esto, un `float: left` era
    /// indistinguible de un `<div>` normal - se apilaba verticalmente como
    /// cualquier otro hijo de bloque, empujando a su hermano hacia abajo
    /// por su propia altura. Un float NO deberia avanzar el cursor
    /// vertical del flujo: el hermano siguiente deberia arrancar en la
    /// MISMA `y` que el float, no debajo de el.
    #[test]
    fn a_float_does_not_push_the_next_sibling_down() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="f" style="float: left; width: 100px; height: 80px;"></div><p id="texto">hola</p></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } p { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let texto_node = Node::find_by_id(&dom, "texto").expect("deberia existir");
        let texto_box = find_box_for_dom_node(&root, &texto_node).expect("deberia tener caja");
        assert_eq!(texto_box.dimensions.y, 0.0, "el parrafo deberia arrancar donde arranca el float, no debajo de su altura completa");
    }

    /// El float SI reserva espacio horizontal para lo que venga despues,
    /// mientras siga activo verticalmente - un parrafo que arranca dentro
    /// del rango vertical del float deberia usar un ancho mas angosto,
    /// desplazado desde el borde izquierdo.
    #[test]
    fn content_next_to_an_active_float_left_is_narrower_and_shifted_right() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="f" style="float: left; width: 100px; height: 80px;"></div><p id="texto">hola</p></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } p { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let texto_node = Node::find_by_id(&dom, "texto").expect("deberia existir");
        let texto_box = find_box_for_dom_node(&root, &texto_node).expect("deberia tener caja");
        assert_eq!(texto_box.dimensions.x, 100.0, "deberia empezar justo donde termina el float (100px de ancho)");
        assert_eq!(texto_box.dimensions.width, 700.0, "deberia perder exactamente el ancho que ocupa el float (800 - 100)");
    }

    /// Una vez que el flujo ya paso por debajo del borde inferior del
    /// float, el contenido siguiente vuelve a usar el ancho completo - un
    /// float no deberia estrechar TODA la pagina para siempre.
    #[test]
    fn content_starting_below_the_float_uses_the_full_width_again() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="f" style="float: left; width: 100px; height: 30px;"></div><p id="antes" style="height: 50px;">antes</p><p id="despues">despues</p></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } p { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        // "antes" arranca en y=0 (dentro del rango del float, 0..30) y por
        // tanto SI se estrecha; "despues" arranca en y=50 (el float ya
        // quedo atras, su borde inferior es 30) y deberia usar el ancho
        // completo otra vez.
        let despues_node = Node::find_by_id(&dom, "despues").expect("deberia existir");
        let despues_box = find_box_for_dom_node(&root, &despues_node).expect("deberia tener caja");
        assert_eq!(despues_box.dimensions.x, 0.0);
        assert_eq!(despues_box.dimensions.width, 800.0, "el float ya quedo atras verticalmente, no deberia estrechar nada aqui");
    }

    /// Simetrico a la izquierda: un `float: right` se ancla al borde
    /// DERECHO del contenedor, y el contenido siguiente pierde ancho por
    /// ESE lado.
    #[test]
    fn a_float_right_anchors_to_the_right_edge_and_narrows_from_that_side() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="f" style="float: right; width: 100px; height: 80px;"></div><p id="texto">hola</p></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } p { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let f_node = Node::find_by_id(&dom, "f").expect("deberia existir");
        let f_box = find_box_for_dom_node(&root, &f_node).expect("deberia tener caja");
        assert_eq!(f_box.dimensions.x, 700.0, "un float:right de 100px en un contenedor de 800px deberia empezar en x=700");

        let texto_node = Node::find_by_id(&dom, "texto").expect("deberia existir");
        let texto_box = find_box_for_dom_node(&root, &texto_node).expect("deberia tener caja");
        assert_eq!(texto_box.dimensions.x, 0.0, "el lado izquierdo no deberia verse afectado por un float:right");
        assert_eq!(texto_box.dimensions.width, 700.0);
    }

    /// Dos floats en lados OPUESTOS a la vez - patron real comun (una
    /// imagen a la izquierda, una caja de "relacionados" a la derecha,
    /// texto principal en medio) - ambos deberian estrechar el contenido
    /// simultaneamente, cada uno desde su propio lado.
    #[test]
    fn floats_on_both_sides_at_once_both_narrow_the_content_between_them() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="izq" style="float: left; width: 100px; height: 50px;"></div><div id="der" style="float: right; width: 150px; height: 50px;"></div><p id="texto">hola</p></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } p { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let texto_node = Node::find_by_id(&dom, "texto").expect("deberia existir");
        let texto_box = find_box_for_dom_node(&root, &texto_node).expect("deberia tener caja");
        assert_eq!(texto_box.dimensions.x, 100.0);
        assert_eq!(texto_box.dimensions.width, 550.0, "800 menos 100 (float izquierdo) menos 150 (float derecho)");
    }

    /// El patron real "clearfix": un elemento con `clear: both` deberia
    /// saltar por debajo de CUALQUIER float activo, en vez de solaparse
    /// con el o seguir estrechado por el.
    #[test]
    fn clear_both_drops_below_every_active_float() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="f" style="float: left; width: 100px; height: 80px;"></div><div id="c" style="clear: both;">limpio</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let c_node = Node::find_by_id(&dom, "c").expect("deberia existir");
        let c_box = find_box_for_dom_node(&root, &c_node).expect("deberia tener caja");
        assert_eq!(c_box.dimensions.y, 80.0, "deberia saltar hasta el borde inferior del float (80px), no quedarse en y=0");
        assert_eq!(c_box.dimensions.x, 0.0, "una vez que limpia el float, deberia volver al borde izquierdo real");
        assert_eq!(c_box.dimensions.width, 800.0, "y al ancho completo, ya no estrechado");
    }

    /// Un contenedor cuyo UNICO contenido es un float no deberia colapsar
    /// a alto cero - a diferencia del spec real (donde esto exige un
    /// clearfix explicito), este motor cuenta el float en el auto-height
    /// de su padre a proposito (ver el doc-comment al final de
    /// `flow_block_children`).
    #[test]
    fn a_container_with_only_a_float_child_still_grows_to_contain_it() {
        let dom = HtmlParser::parse(r#"<html><body><div id="contenedor"><div style="float: left; width: 50px; height: 120px;"></div></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let contenedor_node = Node::find_by_id(&dom, "contenedor").expect("deberia existir");
        let contenedor_box = find_box_for_dom_node(&root, &contenedor_node).expect("deberia tener caja");
        assert_eq!(contenedor_box.dimensions.height, 120.0, "el contenedor deberia abrazar la altura completa del float que contiene");
    }

    /// Un float SIN `width` explicito cae al ancho de respaldo
    /// (`DEFAULT_FLOAT_WIDTH`) en vez de llenar el contenedor entero - lo
    /// segundo anularia por completo el proposito de flotar.
    #[test]
    fn a_float_without_an_explicit_width_falls_back_to_the_default_instead_of_filling_the_container() {
        let dom = HtmlParser::parse(r#"<html><body><div id="f" style="float: left; height: 40px;"></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let f_node = Node::find_by_id(&dom, "f").expect("deberia existir");
        let f_box = find_box_for_dom_node(&root, &f_node).expect("deberia tener caja");
        assert_eq!(f_box.dimensions.width, 200.0, "deberia usar DEFAULT_FLOAT_WIDTH, no los 800px del contenedor");
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

    #[test]
    fn position_sticky_offsets_visually_like_relative_in_initial_flow() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="sticky" style="position: sticky; top: 15px; left: 25px; height: 40px;">s</div><div id="b" style="height: 10px;">b</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } div { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let sticky_node = Node::find_by_id(&dom, "sticky").expect("sticky deberia existir");
        let b_node = Node::find_by_id(&dom, "b").expect("b deberia existir");
        let sticky_box = find_box_for_dom_node(&root, &sticky_node).expect("sticky deberia tener caja");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b deberia tener caja");

        assert_eq!(sticky_box.dimensions.x, 25.0, "desplazado 25px por left");
        assert_eq!(sticky_box.dimensions.y, 15.0, "desplazado 15px por top");
        assert_eq!(b_box.dimensions.y, 40.0, "b ocupa su lugar de flujo normal tras sticky (40px)");
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

    /// Sin fuente de sistema (`font_set: None`, ver `LayoutTreeBuilder::
    /// build` mas abajo), el ancho natural de un texto es determinista:
    /// `content.len() as f32 * 8.0` (la misma aproximacion de respaldo
    /// que usa `place_inline_node` en todo el motor) - por eso estas
    /// pruebas de `text-align` no necesitan saltarse en un entorno sin
    /// fuentes, a diferencia de las de `engine-gfx::paint`.
    #[test]
    fn text_align_center_shifts_a_single_short_text_line_to_the_middle() {
        let dom = HtmlParser::parse(r#"<html><body><div style="text-align: center;">hi</div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "hi").expect("deberia existir la caja de texto 'hi'");
        assert_eq!(text_box.dimensions.x, 392.0, "16px de texto (2 caracteres * 8px) centrados en 800px deberian dejar 392px de hueco a cada lado");
    }

    #[test]
    fn text_align_right_pushes_the_line_to_the_right_edge() {
        let dom = HtmlParser::parse(r#"<html><body><div style="text-align: right;">hi</div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "hi").expect("deberia existir la caja de texto 'hi'");
        assert_eq!(text_box.dimensions.x, 784.0, "800 - 16px de texto");
    }

    #[test]
    fn text_align_left_or_unset_never_shifts_anything() {
        let stylesheet = CssParser::parse("body { margin: 0px; }");

        let dom_left = HtmlParser::parse(r#"<html><body><div style="text-align: left;">hi</div></body></html>"#);
        let root_left = LayoutTreeBuilder::build(&dom_left, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        assert_eq!(find_text_box(&root_left, "hi").unwrap().dimensions.x, 0.0);

        let dom_unset = HtmlParser::parse("<html><body><div>hi</div></body></html>");
        let root_unset = LayoutTreeBuilder::build(&dom_unset, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        assert_eq!(find_text_box(&root_unset, "hi").unwrap().dimensions.x, 0.0, "el valor inicial real de text-align es left");
    }

    /// `th` centra su contenido por defecto (hoja de agente de usuario,
    /// `user_agent_stylesheet.rs`), igual que un navegador real - `td` no.
    /// `text-align` ya era heredable e implementado end-to-end antes de
    /// esta prueba; lo que faltaba era la declaracion en la hoja UA.
    #[test]
    fn th_centers_its_text_by_default_but_td_does_not() {
        let dom = HtmlParser::parse(
            r#"<html><body><table style="width:400px"><tr><th id="th" style="width:200px">hi</th><td id="td" style="width:200px">yo</td></tr></table></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; } table { border-spacing: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let th_cell = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "th").expect("th")).expect("caja del th");
        let td_cell = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "td").expect("td")).expect("caja del td");
        let th_text = find_text_box(&root, "hi").expect("deberia existir la caja de texto 'hi' del th");
        let td_text = find_text_box(&root, "yo").expect("deberia existir la caja de texto 'yo' del td");
        let th_offset = th_text.dimensions.x - th_cell.dimensions.x;
        let td_offset = td_text.dimensions.x - td_cell.dimensions.x;
        assert!(th_offset > td_offset + 10.0, "el texto del th deberia estar mucho mas desplazado dentro de su celda que el del td (centrado vs pegado a la izquierda): th_offset={th_offset}, td_offset={td_offset}");
    }

    /// `padding`/`border` de un elemento `BoxType::Inline` (span/a/b/button)
    /// SI se resuelven ahora: el texto arranca desplazado por
    /// `padding.left + border.left`, y la caja del propio contenedor crece
    /// simetrica arriba/abajo (no solo a la derecha) para que un boton se
    /// vea relleno de verdad.
    #[test]
    fn padding_and_border_of_an_inline_box_shift_its_text_and_grow_its_own_rect() {
        let stylesheet = CssParser::parse("body { margin: 0px; }");

        let dom = HtmlParser::parse(
            r#"<html><body><button id="btn" style="padding: 10px; border: 2px solid black;">hi</button></body></html>"#,
        );
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let btn_box = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "btn").expect("btn")).expect("caja del button");
        let text_box = find_text_box(&root, "hi").expect("deberia existir la caja de texto 'hi'");
        assert_eq!(text_box.dimensions.x - btn_box.dimensions.x, 12.0, "10px de padding + 2px de border deberian desplazar el texto 12px del borde izquierdo del boton");
        assert_eq!(btn_box.box_dimensions.padding.top, 10.0);
        assert_eq!(btn_box.box_dimensions.border.top, 2.0);

        let dom_sin_relleno = HtmlParser::parse(r#"<html><body><button id="btn" style="padding: 0px; border: none;">hi</button></body></html>"#);
        let root_sin_relleno = LayoutTreeBuilder::build(&dom_sin_relleno, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let btn_sin_relleno = find_box_for_dom_node(&root_sin_relleno, &Node::find_by_id(&dom_sin_relleno, "btn").expect("btn")).expect("caja del button");
        assert_eq!(btn_box.dimensions.height - btn_sin_relleno.dimensions.height, 24.0, "10+10px de padding vertical + 2+2px de border deberian crecer la caja del boton 24px por encima de la misma sin relleno");
    }

    /// `<ul>` antepone una vineta (`•`) a cada `<li>`, colocada a la
    /// IZQUIERDA de su contenido (en el hueco de su propio `margin-left`,
    /// ver la hoja UA) - nunca dentro, que empujaria el texto real.
    #[test]
    fn ul_prepends_a_bullet_marker_positioned_to_the_left_of_each_li() {
        let dom = HtmlParser::parse(r#"<html><body><ul><li id="a">uno</li><li id="b">dos</li></ul></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let li_a = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "a").expect("a")).expect("caja del li a");
        let li_b = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "b").expect("b")).expect("caja del li b");

        let marker_a = li_a.children.first().expect("el li deberia tener un marcador como primer hijo");
        assert!(matches!(&marker_a.box_type, BoxType::Text(t) if t == "\u{2022}"), "una lista sin ordenar deberia usar una vineta, no un numero");
        assert!(marker_a.dimensions.x < li_a.dimensions.x, "el marcador deberia pintarse a la izquierda del contenido del li, no dentro de el");

        let marker_b = li_b.children.first().expect("el segundo li tambien deberia tener su propio marcador");
        assert!(matches!(&marker_b.box_type, BoxType::Text(t) if t == "\u{2022}"));
    }

    /// `<ol>` numera sus marcadores 1-based y en orden de documento -
    /// solo entre hermanos `list-item` (no cuenta nada mas).
    #[test]
    fn ol_numbers_its_markers_sequentially_starting_at_one() {
        let dom = HtmlParser::parse(r#"<html><body><ol><li id="a">uno</li><li id="b">dos</li><li id="c">tres</li></ol></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let marker_text = |id: &str| {
            let li = find_box_for_dom_node(&root, &Node::find_by_id(&dom, id).expect(id)).expect("caja del li");
            match &li.children.first().expect("deberia tener marcador").box_type {
                BoxType::Text(t) => t.clone(),
                other => panic!("el primer hijo deberia ser el marcador de texto, salio {other:?}"),
            }
        };
        assert_eq!(marker_text("a"), "1.");
        assert_eq!(marker_text("b"), "2.");
        assert_eq!(marker_text("c"), "3.");
    }

    /// `list-style: none` (el shorthand, la forma MUY comun con la que una
    /// pagina real quita las vinetas de una lista entera desde el `<ul>` -
    /// encontrado en vivo contra la Wikipedia real, su tabla de
    /// contenidos) suprime la vineta - heredado desde el `<ul>`, no
    /// declarado en cada `<li>`.
    #[test]
    fn list_style_none_on_the_ul_suppresses_the_marker_of_every_li() {
        let dom = HtmlParser::parse(r#"<html><body><ul style="list-style: none;"><li id="a">uno</li></ul></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let li_a = find_box_for_dom_node(&root, &Node::find_by_id(&dom, "a").expect("a")).expect("caja del li");
        let first_child_is_marker = li_a.children.first().is_some_and(|c| matches!(&c.box_type, BoxType::Text(t) if t == "\u{2022}"));
        assert!(!first_child_is_marker, "list-style: none deberia suprimir la vineta, no anteponerla igual que sin la propiedad");
    }

    /// `justify` se PARSEA (no cae al "no reconocido") pero se pinta como
    /// `left` a proposito - ver el aviso de `resolve_text_align`.
    #[test]
    fn text_align_justify_is_not_shifted() {
        let dom = HtmlParser::parse(r#"<html><body><div style="text-align: justify;">hi</div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        assert_eq!(find_text_box(&root, "hi").unwrap().dimensions.x, 0.0);
    }

    /// El desplazamiento tiene que arrastrar tambien al hijo de texto de
    /// un elemento inline (`<b>`) ya posicionado DENTRO de el - shiftear
    /// solo el rectangulo delimitador del `<b>` y dejar su texto real
    /// donde estaba lo desalinearia del propio contenedor que se movio.
    #[test]
    fn text_align_center_also_shifts_a_nested_inline_elements_text() {
        let dom = HtmlParser::parse(r#"<html><body><div style="text-align: center;"><b id="target">hi</b></div></body></html>"#);
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let target_node = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let b_box = find_box_for_dom_node(&root, &target_node).expect("el <b> deberia tener caja");
        assert_eq!(b_box.dimensions.x, 392.0);

        let text_box = find_text_box(&root, "hi").expect("deberia existir la caja de texto 'hi'");
        assert_eq!(text_box.dimensions.x, 392.0, "el texto real dentro del <b> deberia haberse desplazado junto con el");
    }

    /// El caso que de verdad prueba que el agrupado es POR LINEA y no por
    /// racha entera: dos elementos de bloque SEPARADOS, cada uno con su
    /// propio `text-align: center` y su propio ancho de linea usado -
    /// cada uno deberia centrarse con SU PROPIO hueco sobrante, sin que
    /// el contenido de uno contamine el calculo del otro. (No se prueba
    /// con dos hermanos inline en la MISMA racha que se reparten en dos
    /// lineas - ver la limitacion declarada en el doc-comment de
    /// `apply_text_align` sobre el rectangulo delimitador de un elemento
    /// inline que el mismo cruza un salto de linea.)
    #[test]
    fn text_align_center_computes_an_independent_offset_per_container() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="text-align: center; width: 90px;">aa</div><div style="text-align: center; width: 90px;">bbbbbbbbbb</div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let corto_text = find_text_box(&root, "aa").expect("deberia existir la caja de texto 'aa'");
        let largo_text = find_text_box(&root, "bbbbbbbbbb").expect("deberia existir la caja de texto 'bbbbbbbbbb'");

        assert_eq!(corto_text.dimensions.x, 37.0, "(90 - 16px de 'aa') / 2");
        assert_eq!(largo_text.dimensions.x, 5.0, "(90 - 80px de 'bbbbbbbbbb') / 2 - un hueco DISTINTO, calculado sobre SU PROPIO contenido");
    }

    /// Un hermano fuera de flujo (`position: absolute`) intercalado en la
    /// racha no deberia romper el agrupado por linea de los que si estan
    /// en flujo a cada lado - `place_inline_node` lo deja sin posicionar
    /// (`Rect::default()`), y `apply_text_align` lo salta explicitamente
    /// en vez de tratar su `y == 0.0` como el limite de una linea nueva.
    #[test]
    fn text_align_grouping_skips_an_out_of_flow_sibling_in_the_middle_of_a_line() {
        let dom = HtmlParser::parse(
            r#"<html><body><div style="text-align: center;"><span id="a">hi</span><span id="p" style="position: absolute;">xxxx</span></div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a deberia existir");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a deberia tener caja");
        assert_eq!(a_box.dimensions.x, 392.0, "el hermano en flujo deberia centrarse igual que si el fuera-de-flujo no estuviera");
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
        // 'vw' no esta soportado (unidades de viewport) y 'not-a-size' no
        // es un valor valido en ninguna unidad - ninguno de los dos deberia
        // fingir un numero y deben caer al font-size heredado.
        let stylesheet = CssParser::parse("body { font-size: 20px; } span { font-size: 3vw; }");

        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());
        let text_box = find_text_box(&root, "hola").expect("deberia existir una caja de texto 'hola'");

        assert_eq!(
            text_box.computed_style.get("font-size").map(String::as_str),
            Some("20px"),
            "vw no soportado: deberia caer al font-size heredado del padre (20px), no a 0 ni a un valor inventado"
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

    /// Regresion encontrada en vivo (Fase 32) con una pagina real que
    /// usaba el fragmento de respaldo de Google Tag Manager
    /// (`<noscript><iframe src="...">...</iframe></noscript>`, presente
    /// en una cantidad enorme de sitios reales): `html5ever` parsea el
    /// interior de `<noscript>` como RAWTEXT cuando el scripting esta
    /// activado (el caso real de este motor - `ParseOpts::default()` ya
    /// activa `scripting_enabled`), asi que su DOM real es un SOLO nodo
    /// de texto con el HTML crudo tal cual, no markup anidado real. Sin
    /// excluir el elemento del layout (igual que ya se excluye
    /// `<script>`/`<style>`), ese marcado se pintaba como texto plano
    /// visible - exactamente lo que aparecio en pantalla.
    #[test]
    fn noscript_content_never_gets_a_visual_representation() {
        let dom = HtmlParser::parse(
            r#"<html><body><noscript><iframe src="https://www.googletagmanager.com/ns.html?id=GTM-TEST" height="0" width="0" style="display:none;visibility:hidden"></iframe></noscript><p id="real">contenido real</p></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        fn collect_text(node: &LayoutBox, out: &mut String) {
            if let BoxType::Text(content) = &node.box_type {
                out.push_str(content);
            }
            for child in &node.children {
                collect_text(child, out);
            }
        }
        let mut all_text = String::new();
        collect_text(&root, &mut all_text);

        assert!(!all_text.contains("iframe"), "el marcado crudo de <noscript> nunca deberia aparecer como texto visible: {all_text:?}");
        assert!(all_text.contains("contenido real"), "el contenido normal de la pagina deberia seguir renderizando");
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

    /// `display: grid` con `grid-template-columns: 1fr 1fr` coloca dos elementos lado a lado con 50% de ancho cada uno.
    #[test]
    fn grid_columns_divide_space_equally() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="grid" style="display: grid; grid-template-columns: 1fr 1fr; width: 400px;">
                <div id="col1" style="height: 50px;">Item 1</div>
                <div id="col2" style="height: 50px;">Item 2</div>
            </div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let col1_node = Node::find_by_id(&dom, "col1").expect("col1 node");
        let col2_node = Node::find_by_id(&dom, "col2").expect("col2 node");
        let col1_box = find_box_for_dom_node(&root, &col1_node).expect("col1 box");
        let col2_box = find_box_for_dom_node(&root, &col2_node).expect("col2 box");

        assert_eq!(col1_box.dimensions.x, 0.0);
        assert_eq!(col1_box.dimensions.width, 200.0, "columna 1 deberia ocupar 200px (la mitad de 400px)");
        assert_eq!(col2_box.dimensions.x, 200.0, "columna 2 deberia empezar en 200px");
        assert_eq!(col2_box.dimensions.width, 200.0, "columna 2 deberia ocupar 200px");
    }

    /// `display: grid` con `gap: 20px` añade espacio entre columnas.
    #[test]
    fn grid_respects_column_gap() {
        let dom = HtmlParser::parse(
            r#"<html><body><div id="grid" style="display: grid; grid-template-columns: 100px 100px; gap: 20px; width: 300px;">
                <div id="a" style="height: 40px;">A</div>
                <div id="b" style="height: 40px;">B</div>
            </div></body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let a_node = Node::find_by_id(&dom, "a").expect("a node");
        let b_node = Node::find_by_id(&dom, "b").expect("b node");
        let a_box = find_box_for_dom_node(&root, &a_node).expect("a box");
        let b_box = find_box_for_dom_node(&root, &b_node).expect("b box");

        assert_eq!(a_box.dimensions.x, 0.0);
        assert_eq!(a_box.dimensions.width, 100.0);
        assert_eq!(b_box.dimensions.x, 120.0, "item B deberia empezar en 100px + 20px de gap");
        assert_eq!(b_box.dimensions.width, 100.0);
    }

    #[test]
    fn box_sizing_border_box_keeps_exact_outer_width_and_height() {
        let dom = HtmlParser::parse(
            r#"<html><body>
                <div id="target" style="box-sizing: border-box; width: 200px; height: 100px; padding: 20px; border: 5px solid black;">
                    Contenido
                </div>
            </body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let target_node = Node::find_by_id(&dom, "target").expect("target node");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target box");

        assert_eq!(target_box.dimensions.width, 200.0, "con border-box el ancho total debe ser exactamente 200px");
        assert_eq!(target_box.dimensions.height, 100.0, "con border-box el alto total debe ser exactamente 100px");
        // El contenido interno debe ser 200 - (20+20) - (5+5) = 150px de ancho y 100 - 50 = 50px de alto
        assert_eq!(target_box.box_dimensions.content.width, 150.0);
        assert_eq!(target_box.box_dimensions.content.height, 50.0);
    }

    #[test]
    fn rem_units_scale_with_root_font_size_base() {
        let dom = HtmlParser::parse(
            r#"<html><body>
                <div id="rembox" style="width: 10rem; height: 5rem; padding: 1rem;">
                    Texto
                </div>
            </body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let rem_node = Node::find_by_id(&dom, "rembox").expect("rembox node");
        let rem_box = find_box_for_dom_node(&root, &rem_node).expect("rembox box");

        // 10rem = 160px content-width + 2rem (32px) padding = 192px border-box width
        assert_eq!(rem_box.dimensions.width, 192.0);
        // 5rem = 80px content-height + 2rem (32px) padding = 112px border-box height
        assert_eq!(rem_box.dimensions.height, 112.0);
    }

    #[test]
    fn position_absolute_resolves_percentage_offsets_against_containing_block() {
        let dom = HtmlParser::parse(
            r#"<html><body>
                <div style="position: relative; width: 400px; height: 200px;">
                    <div id="target" style="position: absolute; top: 50%; left: 25%; width: 50px; height: 30px;">Abs</div>
                </div>
            </body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let target_node = Node::find_by_id(&dom, "target").expect("target node");
        let target_box = find_box_for_dom_node(&root, &target_node).expect("target box");

        // left: 25% de 400px = 100px
        assert_eq!(target_box.dimensions.x, 100.0);
        // top: 50% de 200px = 100px
        assert_eq!(target_box.dimensions.y, 100.0);
    }

    #[test]
    fn position_sticky_resolves_percentage_offsets() {
        let dom = HtmlParser::parse(
            r#"<html><body>
                <div id="sticky_elem" style="position: sticky; top: 10%; left: 20%; width: 100px; height: 50px;">Sticky</div>
            </body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let sticky_node = Node::find_by_id(&dom, "sticky_elem").expect("sticky node");
        let sticky_box = find_box_for_dom_node(&root, &sticky_node).expect("sticky box");

        // left: 20% de 100px = 20px
        assert_eq!(sticky_box.dimensions.x, 20.0);
        // top: 10% de 50px = 5px
        assert_eq!(sticky_box.dimensions.y, 5.0);
    }

    #[test]
    fn flex_item_without_explicit_width_measures_intrinsic_content_width() {
        let dom = HtmlParser::parse(
            r#"<html><body>
                <div id="flex" style="display: flex; width: 500px;">
                    <div id="item" style="padding: 10px;">
                        <span style="display: inline;">Contenido</span>
                    </div>
                </div>
            </body></html>"#,
        );
        let stylesheet = CssParser::parse("body { margin: 0px; }");
        let root = LayoutTreeBuilder::build(&dom, &stylesheet, 800.0, 600.0, None, &ImageMap::new());

        let item_node = Node::find_by_id(&dom, "item").expect("item node");
        let item_box = find_box_for_dom_node(&root, &item_node).expect("item box");

        // El ancho no debe colapsar a 0, sino medir el contenido + padding
        assert!(item_box.dimensions.width > 20.0, "el ancho del item flex debe medirse intrínsecamente y superar el padding");
    }
}
