use engine_image::DecodedImage;
use engine_layout::{ImageMap, LayoutBox, BoxType, Rect};
use std::collections::HashMap;
use std::sync::Arc;

/// Valores iniciales CSS reales para las dos propiedades heredables que
/// aplicamos (ver `INHERITABLE_PROPERTIES` en engine-layout): un navegador
/// real tampoco pinta texto sin color ni tamaño, usa estos mismos por
/// defecto cuando nada en la cascada los redefine.
const INITIAL_COLOR: [u8; 4] = [0, 0, 0, 255];
pub const INITIAL_FONT_SIZE: f32 = 16.0;

#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// `radius` (Fase 3.5, `border-radius`) - 0.0 es el caso de siempre
    /// (esquinas cuadradas, `fill_rect` normal); mayor que cero pinta un
    /// rectangulo redondeado de verdad (`paint::rounded_rect_path`). Un
    /// unico valor para las 4 esquinas - misma simplificacion "un solo
    /// numero" ya establecida para `padding`/`margin`/`border` (ver sus
    /// doc-comments en `engine-layout::tree`), no las 4 esquinas
    /// independientes del spec real.
    SolidRect { rect: Rect, color: [u8; 4], radius: f32 },
    Text { rect: Rect, text: String, color: [u8; 4], font_size: f32, bold: bool, italic: bool },
    /// `rect` es el border-box COMPLETO (`layout_box.dimensions`, que ya
    /// incluye el propio border - ver `engine_layout::tree::
    /// flow_block_children`); quien pinta esto (`engine-gfx/src/paint.rs`)
    /// dibuja un marco de `width` de grosor hacia adentro desde el borde de
    /// `rect`, no un rectangulo aparte fuera de `dimensions`. `radius`
    /// mismo criterio que en `SolidRect`.
    Border { rect: Rect, width: f32, color: [u8; 4], radius: f32 },
    /// `image` ya viene RESUELTA (el `Arc<DecodedImage>` real, no el `src`
    /// crudo) desde `DisplayList::build` - quien pinta esto (`raster.rs`/
    /// `window.rs`) no necesita saber nada de `ImageMap` ni de red, solo
    /// escalar+volcar los pixeles ya decodificados sobre `rect` (que ya es
    /// el tamaño FINAL resuelto por el layout - ver `resolve_image_dimensions`
    /// en `engine-layout::tree` - no necesariamente el tamaño natural de la
    /// imagen).
    Image { rect: Rect, image: Arc<DecodedImage> },
    /// `box-shadow` (Fase 3.5) - `rect` YA lleva el desplazamiento
    /// (`offset-x`/`offset-y`) aplicado (ver `parse_css_box_shadow`), asi
    /// que quien pinta esto solo rellena `rect` con `color`, ni sabe que
    /// existio un desplazamiento por separado. Se pinta ANTES que
    /// `SolidRect`/`Border` de la misma caja (orden real del spec: la
    /// sombra queda DETRAS del fondo/border). Simplificacion declarada:
    /// sombra "dura" sin difuminado - el tercer valor de `box-shadow`
    /// (blur radius) se PARSEA (para no romper el resto de tokens) pero se
    /// descarta, un blur gaussiano real no esta implementado.
    Shadow { rect: Rect, color: [u8; 4], radius: f32 },
    /// `overflow: hidden` (Fase 3.5) - todo lo que se pinte entre un
    /// `PushClip` y su `PopClip` correspondiente (mismo anidamiento que el
    /// arbol de cajas: `build_items` los emite envolviendo la recursion en
    /// los hijos) debe recortarse a `rect` (la padding-box de la caja con
    /// `overflow: hidden`). Quien pinta esto (`engine-gfx/src/paint.rs`)
    /// mantiene una pila de rectangulos de recorte y construye una mascara
    /// con la INTERSECCION de todos los activos - varios `overflow: hidden`
    /// anidados recortan correctamente al mas pequeño de todos, no solo al
    /// mas cercano. Simplificacion declarada: una caja `position: relative/
    /// absolute/fixed` con `z-index` numerico DENTRO de un `overflow:
    /// hidden` no queda recortada - su subarbol se desvia a `z_layers`
    /// (ver `DisplayList::build`) ANTES de que el `PushClip` que la
    /// envuelve llegue a la lista principal, mismo hueco ya declarado para
    /// contextos de apilamiento anidados.
    PushClip { rect: Rect },
    PopClip,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    /// `z_layers` acumula el contenido de cada subarbol posicionado
    /// (`position: relative`/`absolute`/`fixed`, ver `engine-layout::tree`)
    /// que ademas trae un `z-index` numerico - se pintan DESPUES de todo el
    /// contenido normal, ordenados por z-index ascendente (mayor z-index
    /// pinta encima), en vez de en su orden de documento original. Un
    /// elemento posicionado SIN `z-index` (el caso mas comun -
    /// `position: relative` sin mas) sigue pintandose en su sitio normal,
    /// en orden de documento, exactamente como el resto del contenido -
    /// solo `z-index` cambia el orden de pintado, no `position` por si
    /// sola. Simplificacion declarada: no hay contextos de apilamiento
    /// anidados de verdad (un z-index dentro de otro z-index se aplana al
    /// mismo nivel que todos los demas, en vez de resolverse DENTRO de su
    /// propio contenedor primero) - cubre el caso real mas comun (un modal/
    /// tooltip/dropdown con z-index alto pintando por encima de todo lo
    /// demas), no el spec completo de contextos de apilamiento.
    pub fn build(layout_root: &LayoutBox, images: &ImageMap) -> Self {
        let mut list = Self::default();
        let mut z_layers: Vec<(i32, Vec<DisplayItem>)> = Vec::new();
        Self::build_items(layout_root, &mut list.items, images, &mut z_layers);
        z_layers.sort_by_key(|(z, _)| *z);
        for (_, items) in z_layers {
            list.items.extend(items);
        }
        list
    }

    fn build_items(layout_box: &LayoutBox, target: &mut Vec<DisplayItem>, images: &ImageMap, z_layers: &mut Vec<(i32, Vec<DisplayItem>)>) {
        // `visibility: hidden` (a diferencia de `display: none`, ver
        // `engine-layout::tree::build_node`) SI genera caja - sigue
        // ocupando su espacio en el layout, solo no se pinta. Es heredable
        // (`INHERITABLE_PROPERTIES` en engine-layout), y la cascada real ya
        // resuelve por caja: `computed_style` de ESTE `layout_box` refleja
        // tanto la herencia como una posible redeclaracion propia
        // (`visibility: visible` en un descendiente reactiva su pintado,
        // exactamente como el spec real), asi que basta comprobar el valor
        // ya resuelto de esta caja - no hace falta propagar un flag aparte
        // ni recorrer el arbol dos veces. Solo se salta la emision de ESTA
        // caja (fondo/borde/sombra/texto/imagen); overflow-clip y la
        // recursion en los hijos siguen igual, porque cada hijo decide su
        // propio pintado con su propio `computed_style`.
        let hidden = layout_box.computed_style.get("visibility").map(String::as_str) == Some("hidden");
        if hidden {
            return Self::build_clipped_children(layout_box, target, images, z_layers);
        }
        match &layout_box.box_type {
            // `BoxType::Replaced` (Fase 11: controles de formulario) se
            // pinta EXACTAMENTE igual que `Block`/`Inline` - fondo/borde/
            // sombra resueltos de la misma cascada, misma rama. Lo unico
            // que lo distingue de un `<div>` cualquiera es que
            // `engine-layout` nunca recursa en sus hijos de verdad (ver
            // `place_inline_node::BoxType::Replaced`), asi que aqui no hay
            // ningun contenido de texto/imagen que pintar dentro - solo la
            // caja del widget en si.
            BoxType::Block | BoxType::Inline | BoxType::Replaced => {
                let radius = parse_css_border_radius(&layout_box.computed_style).unwrap_or(0.0);
                // `box-shadow` se pinta ANTES que fondo/border (orden real
                // del spec - ver el doc-comment de `DisplayItem::Shadow`).
                if let Some((dx, dy, color)) = parse_css_box_shadow(&layout_box.computed_style) {
                    let mut rect = layout_box.dimensions.clone();
                    rect.x += dx;
                    rect.y += dy;
                    target.push(DisplayItem::Shadow { rect, color, radius });
                }
                // Sin background-color explicito en la cascada, las cajas de
                // bloque no pintan fondo propio (transparente = se ve el
                // fondo de la ventana), en vez del blanco solido fijo de
                // antes que ocultaba cualquier fondo heredado del padre.
                // background-color pinta sobre TODO `dimensions` (el
                // border-box completo) a proposito: asi es el valor inicial
                // real de `background-clip` (`border-box`) - el border, si
                // lo hay, se pinta DESPUES y encima, tapando esa franja.
                if let Some(color) = layout_box.computed_style.get("background-color").and_then(|v| parse_css_color(v)) {
                    target.push(DisplayItem::SolidRect { rect: layout_box.dimensions.clone(), color, radius });
                }
                if let Some((width, color)) = parse_css_border(&layout_box.computed_style) {
                    target.push(DisplayItem::Border { rect: layout_box.dimensions.clone(), width, color, radius });
                }
            }
            BoxType::Text(content) => {
                let color = layout_box.computed_style.get("color").and_then(|v| parse_css_color(v)).unwrap_or(INITIAL_COLOR);
                let font_size = layout_box.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
                target.push(DisplayItem::Text {
                    rect: layout_box.dimensions.clone(),
                    text: content.clone(),
                    color,
                    font_size,
                    bold: resolve_font_weight_is_bold(&layout_box.computed_style),
                    italic: resolve_font_style_is_italic(&layout_box.computed_style),
                });
            }
            BoxType::Image(src) => {
                // Sin imagen resuelta (`src` vacio, descarga fallida,
                // formato no soportado) el layout ya dejo `dimensions` en
                // 0x0 (ver `resolve_image_dimensions` en
                // `engine-layout::tree`) - `images.get(src)` sera `None` en
                // ese mismo caso (misma fuente de verdad), asi que no hace
                // falta comprobar el tamaño por separado: sin imagen, no se
                // pinta nada, en vez de un rectangulo de relleno inventado.
                if let Some(image) = images.get(src) {
                    target.push(DisplayItem::Image { rect: layout_box.dimensions.clone(), image: image.clone() });
                }
            }
        }

        Self::build_clipped_children(layout_box, target, images, z_layers);
    }

    /// `overflow: hidden` (Fase 3.5) envuelve TODO el subarbol de hijos
    /// en un `PushClip`/`PopClip` - ver el doc-comment de
    /// `DisplayItem::PushClip`. Solo `hidden` esta reconocido (`scroll`/
    /// `auto` recortarian igual en un motor con scroll interno por
    /// elemento, que este motor no tiene - solo el scroll de pagina
    /// completa de `window.rs`; `visible`, el valor inicial real, no
    /// recorta nada, que es tambien lo que pasa si la propiedad
    /// simplemente no esta puesta).
    ///
    /// Compartido entre el camino normal de `build_items` y su atajo para
    /// `visibility: hidden`: una caja oculta no pinta nada PROPIO, pero
    /// sigue recortando y recursando en sus hijos igual que si fuera
    /// visible (un hijo puede reactivar su propio pintado con `visibility:
    /// visible`, ver el doc-comment de esa rama).
    fn build_clipped_children(layout_box: &LayoutBox, target: &mut Vec<DisplayItem>, images: &ImageMap, z_layers: &mut Vec<(i32, Vec<DisplayItem>)>) {
        let clips = layout_box.computed_style.get("overflow").map(String::as_str) == Some("hidden");
        if clips {
            target.push(DisplayItem::PushClip { rect: layout_box.dimensions.clone() });
        }
        for child in &layout_box.children {
            match z_index_for_stacking(&child.computed_style) {
                Some(z) => {
                    let mut layer_items = Vec::new();
                    Self::build_items(child, &mut layer_items, images, z_layers);
                    z_layers.push((z, layer_items));
                }
                None => Self::build_items(child, target, images, z_layers),
            }
        }
        if clips {
            target.push(DisplayItem::PopClip);
        }
    }
}

/// `z-index` SOLO participa en el orden de pintado si el elemento tambien
/// esta posicionado (`position: relative`/`absolute`/`fixed`) - en un
/// elemento `static` (el valor inicial real), `z-index` no tiene ningun
/// efecto (asi es el spec real: `z-index` sin `position` se ignora por
/// completo), asi que ese caso deliberadamente devuelve `None` en vez de
/// crear una capa de apilamiento que el spec real nunca crearia.
fn z_index_for_stacking(computed_style: &HashMap<String, String>) -> Option<i32> {
    let is_positioned = matches!(computed_style.get("position").map(String::as_str), Some("relative") | Some("absolute") | Some("fixed"));
    if !is_positioned {
        return None;
    }
    computed_style.get("z-index").and_then(|v| v.trim().parse::<i32>().ok())
}

/// Los 16 colores con nombre de CSS1 mas los que de verdad aparecen en
/// paginas reales. **No es la lista completa** de los ~148 nombres
/// extendidos de CSS: se eligieron los que cubren la practica totalidad
/// del uso real, y un nombre fuera de esta tabla resuelve a `None` (la
/// caja se queda sin pintar) en vez de fingir un color inventado.
/// Ampliarla es añadir filas, sin ningun cambio de logica.
const NAMED_COLORS: &[(&str, [u8; 4])] = &[
    // CSS1: los 16 originales, obligatorios en cualquier motor.
    ("black", [0, 0, 0, 255]),
    ("silver", [192, 192, 192, 255]),
    ("gray", [128, 128, 128, 255]),
    ("white", [255, 255, 255, 255]),
    ("maroon", [128, 0, 0, 255]),
    ("red", [255, 0, 0, 255]),
    ("purple", [128, 0, 128, 255]),
    ("fuchsia", [255, 0, 255, 255]),
    ("green", [0, 128, 0, 255]),
    ("lime", [0, 255, 0, 255]),
    ("olive", [128, 128, 0, 255]),
    ("yellow", [255, 255, 0, 255]),
    ("navy", [0, 0, 128, 255]),
    ("blue", [0, 0, 255, 255]),
    ("teal", [0, 128, 128, 255]),
    ("aqua", [0, 255, 255, 255]),
    // Extendidos de uso comun en paginas reales.
    ("orange", [255, 165, 0, 255]),
    ("pink", [255, 192, 203, 255]),
    ("brown", [165, 42, 42, 255]),
    ("cyan", [0, 255, 255, 255]),
    ("magenta", [255, 0, 255, 255]),
    ("gold", [255, 215, 0, 255]),
    ("indigo", [75, 0, 130, 255]),
    ("violet", [238, 130, 238, 255]),
    ("beige", [245, 245, 220, 255]),
    ("ivory", [255, 255, 240, 255]),
    ("khaki", [240, 230, 140, 255]),
    ("salmon", [250, 128, 114, 255]),
    ("crimson", [220, 20, 60, 255]),
    ("coral", [255, 127, 80, 255]),
    ("tomato", [255, 99, 71, 255]),
    ("orchid", [218, 112, 214, 255]),
    ("plum", [221, 160, 221, 255]),
    ("tan", [210, 180, 140, 255]),
    ("turquoise", [64, 224, 208, 255]),
    ("lavender", [230, 230, 250, 255]),
    ("darkgray", [169, 169, 169, 255]),
    ("darkgrey", [169, 169, 169, 255]),
    ("lightgray", [211, 211, 211, 255]),
    ("lightgrey", [211, 211, 211, 255]),
    ("grey", [128, 128, 128, 255]),
    ("dimgray", [105, 105, 105, 255]),
    ("dimgrey", [105, 105, 105, 255]),
    ("darkred", [139, 0, 0, 255]),
    ("darkgreen", [0, 100, 0, 255]),
    ("darkblue", [0, 0, 139, 255]),
    ("lightblue", [173, 216, 230, 255]),
    ("lightgreen", [144, 238, 144, 255]),
    ("skyblue", [135, 206, 235, 255]),
    ("steelblue", [70, 130, 180, 255]),
    ("royalblue", [65, 105, 225, 255]),
    ("midnightblue", [25, 25, 112, 255]),
    ("whitesmoke", [245, 245, 245, 255]),
    ("gainsboro", [220, 220, 220, 255]),
    ("linen", [250, 240, 230, 255]),
    ("snow", [255, 250, 250, 255]),
];

/// Un componente `0-255` de `rgb()`/`rgba()`: acepta tanto el numero
/// directo (`128`) como el porcentaje (`50%`), las dos formas del spec.
/// Se acota al rango en vez de rechazar, igual que un navegador real ante
/// un `rgb(300, -5, 0)`.
fn parse_rgb_component(token: &str) -> Option<u8> {
    let token = token.trim();
    let value = match token.strip_suffix('%') {
        Some(percent) => percent.trim().parse::<f32>().ok()? * 255.0 / 100.0,
        None => token.parse::<f32>().ok()?,
    };
    Some(value.clamp(0.0, 255.0).round() as u8)
}

/// Parseo de color CSS: hexadecimal (`#rgb`, `#rrggbb`, `#rrggbbaa`),
/// nombres (`red`, `white`... ver `NAMED_COLORS`), `rgb()`/`rgba()`,
/// `hsl()`/`hsla()` (Fase 28) y las palabras clave `transparent`/
/// `currentColor`.
///
/// Antes de esta fase SOLO entendia hexadecimal, asi que un
/// `background: red` - de lo mas comun que hay en CSS real - no pintaba
/// absolutamente nada. Como este es el UNICO parseador de color del motor,
/// arreglarlo aqui arregla `color`, `background-color`, `border` y
/// `box-shadow` a la vez.
///
/// `hsl()`/`hsla()` (Fase 28) acepta la sintaxis clasica con comas
/// (`hsl(210, 50%, 40%)`) y la moderna con espacios/`/` para el alfa
/// (`hsl(210 50% 40% / 0.5)`), igual que ya hacia `rgb()`. El matiz
/// (`hue`) acepta grados sin unidad (lo mas comun en CSS real) y las
/// otras tres unidades de angulo del spec (`deg`/`grad`/`rad`/`turn`) -
/// normalizado a `0..360` con `rem_euclid` antes de convertir, asi que un
/// matiz negativo o mayor de 360 (legal en el spec) da el mismo color que
/// su equivalente normalizado. Saturacion y luminosidad EXIGEN `%` (la
/// sintaxis clasica real, sin la forma moderna sin unidad de CSS Color 4).
/// La conversion HSL->RGB es la formula estandar del spec (`hsl_to_rgb`,
/// mas abajo) - sin aproximacion, resultado identico al de un navegador
/// real para el mismo triplete.
///
/// NO implementado: `hwb()`/`lab()`/`lch()`/`oklab()`/`oklch()` y el resto
/// de espacios de color modernos - devuelven `None` y la caja se queda
/// sin pintar, en vez de fingir una conversion. Tampoco los ~90 nombres
/// extendidos que faltan en la tabla.
fn parse_css_color(value: &str) -> Option<[u8; 4]> {
    let value = value.trim();

    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }

    let lower = value.to_ascii_lowercase();

    // `transparent` es un color de pleno derecho en el spec (negro con
    // alfa 0), no la ausencia de color - y se usa mucho para "quitar" un
    // fondo o un borde heredado.
    if lower == "transparent" {
        return Some([0, 0, 0, 0]);
    }
    // `currentColor` significa "el valor de `color` de este elemento", que
    // esta funcion no conoce: resolverlo exige el `computed_style`
    // completo. Devuelve `None` a proposito - quien pinta un borde ya cae
    // al `color` del elemento por su cuenta (ver `parse_css_border`), que
    // es exactamente el mismo resultado en el caso que mas importa.
    if lower == "currentcolor" {
        return None;
    }

    if let Some(rest) = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb(")) {
        let inner = rest.strip_suffix(')')?;
        // Acepta tanto la sintaxis clasica con comas (`rgb(1, 2, 3)`) como
        // la moderna con espacios (`rgb(1 2 3 / 0.5)`), que ya usan muchas
        // hojas de estilo reales.
        let normalised = inner.replace(',', " ").replace('/', " ");
        let parts: Vec<&str> = normalised.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }
        let r = parse_rgb_component(parts[0])?;
        let g = parse_rgb_component(parts[1])?;
        let b = parse_rgb_component(parts[2])?;
        // El alfa va de 0.0 a 1.0 (o en porcentaje), no de 0 a 255 - es la
        // diferencia que hace que no se pueda reusar `parse_rgb_component`.
        let a = match parts.get(3) {
            Some(token) => {
                let t = token.trim();
                let alpha = match t.strip_suffix('%') {
                    Some(percent) => percent.trim().parse::<f32>().ok()? / 100.0,
                    None => t.parse::<f32>().ok()?,
                };
                (alpha.clamp(0.0, 1.0) * 255.0).round() as u8
            }
            None => 255,
        };
        return Some([r, g, b, a]);
    }

    if let Some(rest) = lower.strip_prefix("hsla(").or_else(|| lower.strip_prefix("hsl(")) {
        let inner = rest.strip_suffix(')')?;
        let normalised = inner.replace(',', " ").replace('/', " ");
        let parts: Vec<&str> = normalised.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }
        let hue = parse_hue_degrees(parts[0])?;
        let saturation = parse_percentage_0_1(parts[1])?;
        let lightness = parse_percentage_0_1(parts[2])?;
        let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
        // Mismo parseo de alfa que `rgb()`/`rgba()` de aqui arriba.
        let a = match parts.get(3) {
            Some(token) => {
                let t = token.trim();
                let alpha = match t.strip_suffix('%') {
                    Some(percent) => percent.trim().parse::<f32>().ok()? / 100.0,
                    None => t.parse::<f32>().ok()?,
                };
                (alpha.clamp(0.0, 1.0) * 255.0).round() as u8
            }
            None => 255,
        };
        return Some([r, g, b, a]);
    }

    NAMED_COLORS.iter().find(|(name, _)| *name == lower).map(|(_, rgba)| *rgba)
}

/// El matiz (`hue`) de `hsl()` en cualquiera de las cuatro unidades de
/// angulo del spec, normalizado a grados - `deg` (o sin unidad, la forma
/// mas comun en CSS real) se toma tal cual, las otras tres se convierten.
/// El orden de las comprobaciones importa: `strip_suffix("rad")` tambien
/// coincidiria con un token que termina en `grad` (los ultimos tres
/// caracteres de "grad" son "rad"), asi que `grad` se comprueba ANTES que
/// `rad` a proposito.
fn parse_hue_degrees(token: &str) -> Option<f32> {
    let token = token.trim();
    if let Some(v) = token.strip_suffix("deg") {
        return v.trim().parse::<f32>().ok();
    }
    if let Some(v) = token.strip_suffix("grad") {
        return v.trim().parse::<f32>().ok().map(|g| g * 360.0 / 400.0);
    }
    if let Some(v) = token.strip_suffix("turn") {
        return v.trim().parse::<f32>().ok().map(|t| t * 360.0);
    }
    if let Some(v) = token.strip_suffix("rad") {
        return v.trim().parse::<f32>().ok().map(|r| r.to_degrees());
    }
    token.parse::<f32>().ok()
}

/// Saturacion/luminosidad de `hsl()`: EXIGEN `%` (la sintaxis clasica real
/// - la forma moderna sin unidad de CSS Color 4 no esta soportada, mismo
/// criterio de alcance que el resto de este parser). Acotado a `0.0..1.0`,
/// igual que `parse_rgb_component` acota sus componentes.
fn parse_percentage_0_1(token: &str) -> Option<f32> {
    let percent = token.trim().strip_suffix('%')?;
    Some((percent.trim().parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0))
}

/// HSL -> RGB, la formula estandar del spec (CSS Color 4 §4.2, identica a
/// la de HTML/CSS desde HSL original) - no una aproximacion. `hue_deg`
/// puede venir fuera de `0..360` (el spec lo permite, ver el aviso de
/// `parse_css_color`) y se normaliza aqui con `rem_euclid` ANTES de
/// convertir, no despues: un `hue` negativo debe dar el mismo color que su
/// equivalente positivo, no un canal RGB fuera de rango.
fn hsl_to_rgb(hue_deg: f32, saturation: f32, lightness: f32) -> (u8, u8, u8) {
    let h = hue_deg.rem_euclid(360.0);
    let s = saturation.clamp(0.0, 1.0);
    let l = lightness.clamp(0.0, 1.0);

    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = chroma * (1.0 - (h_prime.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = if h_prime < 1.0 {
        (chroma, x, 0.0)
    } else if h_prime < 2.0 {
        (x, chroma, 0.0)
    } else if h_prime < 3.0 {
        (0.0, chroma, x)
    } else if h_prime < 4.0 {
        (0.0, x, chroma)
    } else if h_prime < 5.0 {
        (x, 0.0, chroma)
    } else {
        (chroma, 0.0, x)
    };
    let m = l - chroma / 2.0;

    let to_channel = |c: f32| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_channel(r1), to_channel(g1), to_channel(b1))
}

/// Hexadecimal en sus tres longitudes reales: `#rgb`, `#rrggbb` y
/// `#rrggbbaa` (esta ultima con canal alfa, cada vez mas comun).
fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    match hex.len() {
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some([r, g, b, a])
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some([r, g, b, 255])
        }
        3 => {
            let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
            let mut chars = hex.chars();
            let r = expand(chars.next()?)?;
            let g = expand(chars.next()?)?;
            let b = expand(chars.next()?)?;
            Some([r, g, b, 255])
        }
        _ => None,
    }
}

/// Parseo de `font-size` deliberadamente minimo: solo `<numero>px`. Unidades
/// relativas (`em`, `rem`, `%`) exigirian conocer el tamaño heredado del
/// padre y la raiz del documento, que no rastreamos todavia - se devuelve
/// `None` (cae al tamaño inicial) en vez de fingir una conversion.
fn parse_css_font_size(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok().filter(|size| *size > 0.0)
}

/// Copia deliberada de `resolve_font_weight_is_bold` en
/// `engine-layout::tree` (misma razon de siempre: dos crates que no deben
/// depender entre si, y unas pocas lineas no justifican enredar la
/// dependencia) - decide que variante de `FontSet` pedir al pintar
/// (`engine-gfx/src/raster.rs`, `engine-gfx/src/window.rs`), igual que la
/// copia de layout decide que variante pedir al MEDIR. Misma
/// simplificacion binaria: negrita si/no, no el espacio 1-1000 completo del
/// spec.
fn resolve_font_weight_is_bold(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("font-weight") else { return false };
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("bold") || trimmed.eq_ignore_ascii_case("bolder") {
        return true;
    }
    trimmed.parse::<u16>().map(|weight| weight >= 600).unwrap_or(false)
}

/// Copia deliberada de `resolve_font_style_is_italic` en
/// `engine-layout::tree` - misma razon que `resolve_font_weight_is_bold`
/// arriba.
fn resolve_font_style_is_italic(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("font-style") else { return false };
    let trimmed = raw.trim();
    trimmed.eq_ignore_ascii_case("italic") || trimmed.eq_ignore_ascii_case("oblique")
}

/// Igual que `parse_css_font_size`, pero SI acepta cero (un ancho de
/// border de 0 es valido) - copia deliberada de `parse_css_length` en
/// `engine-layout/src/tree.rs`, misma razon que `INITIAL_FONT_SIZE`/
/// `parse_css_font_size` de arriba: dos crates que no deben depender
/// entre si, y unas pocas lineas no justifican enredar la dependencia.
fn parse_css_length(value: &str) -> Option<f32> {
    let px = value.trim().strip_suffix("px")?;
    px.trim().parse::<f32>().ok().filter(|n| *n >= 0.0)
}

/// Ancho+color de `border` (forma abreviada `border: <ancho> <estilo>
/// <color>`, en cualquier orden). Mismo criterio que
/// `engine_layout::tree::resolve_border_width` (que resuelve solo el
/// ancho, para el layout): sin la palabra `solid`, el border no existe -
/// `None` aqui, no un rectangulo con ancho cero (que tampoco pintaria
/// nada, pero por las razones equivocadas). Color ausente cae al `color`
/// YA RESUELTO de la propia caja - equivalente a `currentColor`, el valor
/// inicial real de `border-color`.
fn parse_css_border(computed_style: &HashMap<String, String>) -> Option<(f32, [u8; 4])> {
    let raw = computed_style.get("border")?;
    let mut width: Option<f32> = None;
    let mut color: Option<[u8; 4]> = None;
    let mut is_solid = false;

    for token in raw.split_whitespace() {
        if let Some(w) = parse_css_length(token) {
            width = Some(w);
        } else if token.eq_ignore_ascii_case("solid") {
            is_solid = true;
        } else if let Some(c) = parse_css_color(token) {
            color = Some(c);
        }
    }

    if !is_solid {
        return None;
    }

    let resolved_width = width.unwrap_or(0.0);
    if resolved_width <= 0.0 {
        return None;
    }
    let resolved_color = color
        .or_else(|| computed_style.get("color").and_then(|v| parse_css_color(v)))
        .unwrap_or(INITIAL_COLOR);
    Some((resolved_width, resolved_color))
}

/// `border-radius` (Fase 3.5) - un unico valor en `px`, aplicado a las 4
/// esquinas por igual (misma simplificacion "un solo numero" que
/// `padding`/`margin`/`border-width`). `None`/cero/negativo resuelve a
/// "sin redondeo" (esquinas cuadradas, el valor inicial real).
fn parse_css_border_radius(computed_style: &HashMap<String, String>) -> Option<f32> {
    computed_style.get("border-radius").and_then(|v| parse_css_length(v)).filter(|r| *r > 0.0)
}

/// `box-shadow: <offset-x> <offset-y> [<blur-radius>] <color>` (Fase 3.5) -
/// el `blur-radius` opcional SI se parsea (para no romper el resto de
/// tokens, p.ej. tomar el color por el blur) pero se DESCARTA - ver el
/// doc-comment de `DisplayItem::Shadow` para el porque (sombra "dura", sin
/// difuminado real). `offset-x`/`offset-y` aceptan negativos (a diferencia
/// de `parse_css_length`, que rechaza negativos porque un padding/border
/// negativo no tiene sentido - un offset de sombra si) via `parse_css_offset`
/// local, deliberadamente NO compartida con la copia de `engine-layout::tree`
/// (misma razon de siempre: crates que no deben depender entre si). `None`
/// si faltan offset-x/offset-y o el color, o si la propiedad no esta
/// puesta - sin sombra por defecto, el valor inicial real de la propiedad.
fn parse_css_box_shadow(computed_style: &HashMap<String, String>) -> Option<(f32, f32, [u8; 4])> {
    fn parse_offset(value: &str) -> Option<f32> {
        let px = value.trim().strip_suffix("px")?;
        px.trim().parse::<f32>().ok()
    }

    let raw = computed_style.get("box-shadow")?;
    let mut offsets: Vec<f32> = Vec::new();
    let mut color: Option<[u8; 4]> = None;

    for token in raw.split_whitespace() {
        if let Some(c) = parse_css_color(token) {
            color = Some(c);
        } else if let Some(n) = parse_offset(token) {
            offsets.push(n);
        }
        // Un tercer numero (blur-radius) cae aqui y se ignora a proposito.
    }

    let dx = *offsets.first()?;
    let dy = *offsets.get(1)?;
    Some((dx, dy, color.unwrap_or(INITIAL_COLOR)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_colors_case_insensitively() {
        assert_eq!(parse_css_color("red"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("WHITE"), Some([255, 255, 255, 255]));
        assert_eq!(parse_css_color("  Blue  "), Some([0, 0, 255, 255]));
        assert_eq!(parse_css_color("rebeccapurple"), None, "un nombre fuera de la tabla no deberia inventarse un color");
    }

    /// Las dos grafias de gris del spec (`gray`/`grey`) deben dar lo
    /// mismo - el codigo real usa ambas indistintamente.
    #[test]
    fn both_spellings_of_gray_resolve_to_the_same_color() {
        assert_eq!(parse_css_color("gray"), parse_css_color("grey"));
        assert_eq!(parse_css_color("lightgray"), parse_css_color("lightgrey"));
    }

    #[test]
    fn parses_the_three_hex_lengths_including_alpha() {
        assert_eq!(parse_css_color("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("#ff0000"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("#ff000080"), Some([255, 0, 0, 128]), "#rrggbbaa deberia leer el canal alfa");
    }

    #[test]
    fn parses_rgb_and_rgba_in_both_comma_and_space_syntax() {
        assert_eq!(parse_css_color("rgb(255, 0, 0)"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("rgb(255 0 0)"), Some([255, 0, 0, 255]), "la sintaxis moderna con espacios tambien es valida");
        assert_eq!(parse_css_color("rgba(0, 0, 0, 0.5)"), Some([0, 0, 0, 128]), "el alfa va de 0 a 1, no de 0 a 255");
        assert_eq!(parse_css_color("rgb(0 0 0 / 50%)"), Some([0, 0, 0, 128]), "alfa en porcentaje tras la barra");
    }

    #[test]
    fn rgb_accepts_percentage_components() {
        assert_eq!(parse_css_color("rgb(100%, 0%, 0%)"), Some([255, 0, 0, 255]));
    }

    /// Un navegador real acota en vez de rechazar un componente fuera de
    /// rango.
    #[test]
    fn out_of_range_rgb_components_are_clamped_not_rejected() {
        assert_eq!(parse_css_color("rgb(300, -5, 0)"), Some([255, 0, 0, 255]));
    }

    /// `transparent` es un color de pleno derecho (negro con alfa cero),
    /// no la ausencia de color - se usa mucho para quitar un fondo o
    /// borde heredado.
    #[test]
    fn transparent_is_a_real_color_with_zero_alpha() {
        assert_eq!(parse_css_color("transparent"), Some([0, 0, 0, 0]));
    }

    #[test]
    fn unsupported_color_syntaxes_are_none_instead_of_a_made_up_color() {
        assert_eq!(parse_css_color("hwb(0 0% 0%)"), None, "hwb() no esta implementado");
        assert_eq!(parse_css_color("oklch(0.5 0.2 30)"), None, "oklch() no esta implementado");
        assert_eq!(parse_css_color("currentColor"), None, "currentColor exige el computed_style completo");
        assert_eq!(parse_css_color("basura"), None);
        assert_eq!(parse_css_color(""), None);
    }

    /// Los tres primarios y los limites de luminosidad (0%/100% siempre dan
    /// negro/blanco puro, sin importar matiz o saturacion) - los puntos de
    /// referencia mas faciles de verificar a ojo contra un navegador real.
    #[test]
    fn hsl_resolves_the_primaries_and_the_lightness_extremes() {
        assert_eq!(parse_css_color("hsl(0, 100%, 50%)"), Some([255, 0, 0, 255]), "rojo");
        assert_eq!(parse_css_color("hsl(120, 100%, 50%)"), Some([0, 255, 0, 255]), "verde");
        assert_eq!(parse_css_color("hsl(240, 100%, 50%)"), Some([0, 0, 255, 255]), "azul");
        assert_eq!(parse_css_color("hsl(180, 100%, 50%)"), Some([0, 255, 255, 255]), "cian");
        assert_eq!(parse_css_color("hsl(0, 100%, 0%)"), Some([0, 0, 0, 255]), "luminosidad 0% siempre es negro");
        assert_eq!(parse_css_color("hsl(0, 100%, 100%)"), Some([255, 255, 255, 255]), "luminosidad 100% siempre es blanco");
        assert_eq!(parse_css_color("hsl(0, 0%, 50%)"), Some([128, 128, 128, 255]), "saturacion 0% es gris puro");
    }

    #[test]
    fn hsl_accepts_both_comma_and_space_syntax_with_alpha() {
        assert_eq!(parse_css_color("hsl(0, 100%, 50%)"), parse_css_color("hsl(0 100% 50%)"), "las dos sintaxis deberian dar el mismo color");
        assert_eq!(parse_css_color("hsla(0, 100%, 50%, 0.5)"), Some([255, 0, 0, 128]), "el alfa va de 0 a 1, no de 0 a 255");
        assert_eq!(parse_css_color("hsl(0 100% 50% / 50%)"), Some([255, 0, 0, 128]), "alfa en porcentaje tras la barra, sintaxis moderna");
    }

    /// El matiz es circular (el spec lo permite fuera de `0..360`): -120
    /// deberia dar el mismo color que su equivalente normalizado 240, y
    /// 360 el mismo que 0.
    #[test]
    fn hsl_hue_wraps_for_negative_and_over_360_values() {
        assert_eq!(parse_css_color("hsl(-120, 100%, 50%)"), parse_css_color("hsl(240, 100%, 50%)"));
        assert_eq!(parse_css_color("hsl(360, 100%, 50%)"), parse_css_color("hsl(0, 100%, 50%)"));
    }

    /// Las cuatro unidades de angulo del spec para el matiz deberian dar
    /// el mismo color que su equivalente en grados.
    #[test]
    fn hsl_hue_accepts_all_four_angle_units() {
        let referencia = parse_css_color("hsl(180deg, 100%, 50%)");
        assert_eq!(parse_css_color("hsl(180, 100%, 50%)"), referencia, "sin unidad se asume deg, la forma mas comun en CSS real");
        assert_eq!(parse_css_color("hsl(200grad, 100%, 50%)"), referencia);
        assert_eq!(parse_css_color("hsl(0.5turn, 100%, 50%)"), referencia);
        assert_eq!(parse_css_color("hsl(3.14159265rad, 100%, 50%)"), referencia);
    }

    /// A diferencia de los componentes de `rgb()`, saturacion y
    /// luminosidad de `hsl()` EXIGEN `%` en la sintaxis clasica real - un
    /// numero sin unidad no es valido.
    #[test]
    fn hsl_saturation_and_lightness_require_a_percent_sign() {
        assert_eq!(parse_css_color("hsl(0, 100, 50)"), None, "sin % en saturacion/luminosidad deberia ser invalido");
    }

    #[test]
    fn parses_pixel_font_size() {
        assert_eq!(parse_css_font_size("32px"), Some(32.0));
        assert_eq!(parse_css_font_size("  18px "), Some(18.0));
    }

    #[test]
    fn resolve_font_weight_is_bold_recognizes_keywords_and_heavy_numeric_weights() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("font-weight".to_string(), value.to_string()); m };

        assert!(resolve_font_weight_is_bold(&style("bold")));
        assert!(resolve_font_weight_is_bold(&style("700")));
        assert!(!resolve_font_weight_is_bold(&style("normal")));
        assert!(!resolve_font_weight_is_bold(&HashMap::new()));
    }

    #[test]
    fn z_index_for_stacking_only_applies_to_positioned_elements() {
        let style = |position: Option<&str>, z: &str| {
            let mut m = HashMap::new();
            if let Some(p) = position {
                m.insert("position".to_string(), p.to_string());
            }
            m.insert("z-index".to_string(), z.to_string());
            m
        };
        assert_eq!(z_index_for_stacking(&style(Some("relative"), "5")), Some(5));
        assert_eq!(z_index_for_stacking(&style(Some("static"), "5")), None, "z-index sin position real (static) no deberia tener efecto, igual que el spec real");
        assert_eq!(z_index_for_stacking(&style(None, "5")), None, "sin position en absoluto (static por defecto), z-index se ignora igual");
    }

    /// `visibility: hidden` no debe emitir ningun item de pintado propio
    /// (fondo/borde), a diferencia de `display: none` (que ni siquiera
    /// llega a `DisplayList::build` porque `engine-layout` no le genera
    /// caja - ver `engine-layout::tree::build_node`). Aqui SI hay caja
    /// (ocupa espacio), solo no se pinta.
    #[test]
    fn visibility_hidden_emits_no_display_items_for_the_box_itself() {
        let mut hidden = LayoutBox::new(BoxType::Block);
        hidden.computed_style.insert("visibility".to_string(), "hidden".to_string());
        hidden.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
        hidden.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let list = DisplayList::build(&hidden, &ImageMap::new());

        assert!(list.items.is_empty(), "una caja con visibility:hidden no deberia producir ningun DisplayItem propio");
    }

    /// Un hijo con `visibility: visible` explicito dentro de un ancestro
    /// `hidden` reactiva su propio pintado - la cascada real ya resuelve
    /// esto (la declaracion propia del hijo gana sobre lo heredado, ver
    /// `engine-layout::tree::build_node`), asi que basta con que
    /// `build_items` respete el `computed_style` YA resuelto de cada caja.
    #[test]
    fn a_child_with_explicit_visibility_visible_still_paints_inside_a_hidden_ancestor() {
        let mut visible_child = LayoutBox::new(BoxType::Block);
        visible_child.computed_style.insert("visibility".to_string(), "visible".to_string());
        visible_child.computed_style.insert("background-color".to_string(), "#00ff00".to_string());
        visible_child.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let mut hidden_parent = LayoutBox::new(BoxType::Block);
        hidden_parent.computed_style.insert("visibility".to_string(), "hidden".to_string());
        hidden_parent.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
        hidden_parent.dimensions = Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 };
        hidden_parent.children.push(visible_child);

        let list = DisplayList::build(&hidden_parent, &ImageMap::new());

        assert!(
            list.items.iter().any(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [0, 255, 0, 255])),
            "el hijo con visibility:visible explicito deberia pintarse pese al ancestro oculto"
        );
        assert!(
            !list.items.iter().any(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [255, 0, 0, 255])),
            "el ancestro oculto en si no deberia pintar su propio fondo"
        );
    }

    /// El punto real del pintado por z-index: un elemento posicionado con
    /// `z-index` alto deberia pintarse DESPUES (encima) de contenido sin
    /// z-index, sin importar que venga ANTES en el documento.
    #[test]
    fn a_document_earlier_box_with_a_higher_z_index_paints_after_a_later_one() {
        let mut low_in_document_but_high_z_index = LayoutBox::new(BoxType::Block);
        low_in_document_but_high_z_index.computed_style.insert("position".to_string(), "relative".to_string());
        low_in_document_but_high_z_index.computed_style.insert("z-index".to_string(), "10".to_string());
        low_in_document_but_high_z_index.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
        low_in_document_but_high_z_index.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let mut later_in_document_no_z_index = LayoutBox::new(BoxType::Block);
        later_in_document_no_z_index.computed_style.insert("background-color".to_string(), "#00ff00".to_string());
        later_in_document_no_z_index.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let mut root = LayoutBox::new(BoxType::Block);
        root.children.push(low_in_document_but_high_z_index);
        root.children.push(later_in_document_no_z_index);

        let list = DisplayList::build(&root, &ImageMap::new());

        let red_index = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [255, 0, 0, 255]))
            .expect("deberia existir el rectangulo rojo (z-index alto)");
        let green_index = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [0, 255, 0, 255]))
            .expect("deberia existir el rectangulo verde (sin z-index)");

        assert!(red_index > green_index, "el z-index alto (rojo, primero en el documento) deberia pintarse DESPUES del verde (sin z-index), pese a venir antes en el DOM");
    }

    #[test]
    fn resolve_font_style_is_italic_recognizes_italic_and_oblique() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("font-style".to_string(), value.to_string()); m };

        assert!(resolve_font_style_is_italic(&style("italic")));
        assert!(resolve_font_style_is_italic(&style("oblique")));
        assert!(!resolve_font_style_is_italic(&HashMap::new()));
    }

    #[test]
    fn rejects_unsupported_units_and_garbage() {
        assert_eq!(parse_css_font_size("2em"), None);
        assert_eq!(parse_css_font_size("120%"), None);
        assert_eq!(parse_css_font_size("not-a-size"), None);
        assert_eq!(parse_css_font_size("-5px"), None, "un tamaño negativo no es valido");
        assert_eq!(parse_css_font_size("0px"), None, "un tamaño cero no pintaria nada visible");
    }

    fn style_with_border(value: &str) -> HashMap<String, String> {
        let mut style = HashMap::new();
        style.insert("border".to_string(), value.to_string());
        style
    }

    #[test]
    fn parse_css_border_reads_width_and_color_in_any_order() {
        assert_eq!(parse_css_border(&style_with_border("2px solid #ff0000")), Some((2.0, [255, 0, 0, 255])));
        assert_eq!(parse_css_border(&style_with_border("solid #ff0000 2px")), Some((2.0, [255, 0, 0, 255])), "el orden de ancho/estilo/color deberia ser libre, igual que el spec real");
        assert_eq!(parse_css_border(&style_with_border("#ff0000 2px solid")), Some((2.0, [255, 0, 0, 255])));
    }

    #[test]
    fn parse_css_border_without_solid_keyword_is_none() {
        assert_eq!(parse_css_border(&style_with_border("2px #ff0000")), None, "sin 'solid', border-style es 'none' - el border no deberia existir en absoluto");
        assert_eq!(parse_css_border(&style_with_border("2px none #ff0000")), None);
    }

    #[test]
    fn parse_css_border_falls_back_to_the_boxs_own_color_when_no_color_is_given() {
        let mut style = style_with_border("2px solid");
        style.insert("color".to_string(), "#00ff00".to_string());
        assert_eq!(parse_css_border(&style), Some((2.0, [0, 255, 0, 255])), "sin border-color, deberia caer al color ya resuelto de la caja (currentColor)");
    }

    #[test]
    fn parse_css_border_defaults_to_black_when_neither_border_color_nor_color_are_set() {
        assert_eq!(parse_css_border(&style_with_border("2px solid")), Some((2.0, [0, 0, 0, 255])));
    }

    #[test]
    fn parse_css_border_with_zero_width_is_none() {
        assert_eq!(parse_css_border(&style_with_border("0px solid #ff0000")), None, "un border de ancho cero no pinta nada, aunque el estilo sea solid");
    }

    #[test]
    fn missing_border_property_is_none() {
        assert_eq!(parse_css_border(&HashMap::new()), None);
    }

    fn style_with(key: &str, value: &str) -> HashMap<String, String> {
        let mut style = HashMap::new();
        style.insert(key.to_string(), value.to_string());
        style
    }

    #[test]
    fn parse_css_border_radius_reads_a_positive_px_value() {
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "8px")), Some(8.0));
    }

    #[test]
    fn parse_css_border_radius_rejects_zero_negative_and_missing() {
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "0px")), None);
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "-3px")), None);
        assert_eq!(parse_css_border_radius(&HashMap::new()), None);
    }

    #[test]
    fn parse_css_box_shadow_reads_offsets_and_color_in_any_order() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "4px 6px #ff0000")), Some((4.0, 6.0, [255, 0, 0, 255])));
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "#ff0000 4px 6px")), Some((4.0, 6.0, [255, 0, 0, 255])), "el orden deberia ser libre, igual que border");
    }

    #[test]
    fn parse_css_box_shadow_ignores_the_optional_blur_radius_token() {
        // El tercer numero (blur-radius) se parsea para no romper el color
        // que viene despues, pero se descarta - offsets siguen siendo los
        // dos primeros numeros encontrados.
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "4px 6px 10px #ff0000")), Some((4.0, 6.0, [255, 0, 0, 255])));
    }

    #[test]
    fn parse_css_box_shadow_accepts_negative_offsets() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "-4px -6px #000000")), Some((-4.0, -6.0, [0, 0, 0, 255])));
    }

    #[test]
    fn parse_css_box_shadow_defaults_color_to_black_when_missing() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "4px 6px")), Some((4.0, 6.0, [0, 0, 0, 255])));
    }

    #[test]
    fn missing_box_shadow_property_is_none() {
        assert_eq!(parse_css_box_shadow(&HashMap::new()), None);
    }

    /// El punto real de `box-shadow`: la sombra se pinta ANTES que el
    /// fondo/border de la misma caja (orden real del spec - la sombra
    /// queda DETRAS), y su rectangulo ya lleva el offset aplicado.
    #[test]
    fn a_box_shadow_paints_before_the_boxs_own_background_with_the_offset_applied() {
        let mut node = LayoutBox::new(BoxType::Block);
        node.dimensions = Rect { x: 10.0, y: 10.0, width: 50.0, height: 30.0 };
        node.computed_style.insert("background-color".to_string(), "#ffffff".to_string());
        node.computed_style.insert("box-shadow".to_string(), "4px 6px #000000".to_string());

        let list = DisplayList::build(&node, &ImageMap::new());

        let shadow_index = list.items.iter().position(|item| matches!(item, DisplayItem::Shadow { .. })).expect("deberia existir la sombra");
        let background_index = list.items.iter().position(|item| matches!(item, DisplayItem::SolidRect { .. })).expect("deberia existir el fondo");
        assert!(shadow_index < background_index, "la sombra deberia pintarse ANTES que el fondo");

        let DisplayItem::Shadow { rect, .. } = &list.items[shadow_index] else { unreachable!() };
        assert_eq!((rect.x, rect.y), (14.0, 16.0), "10+4 y 10+6: el rectangulo de la sombra ya lleva el offset aplicado");
    }

    /// El punto real de `overflow: hidden`: los hijos quedan envueltos
    /// entre un `PushClip` y su `PopClip` correspondiente, con el
    /// rectangulo de la propia caja (no del hijo).
    #[test]
    fn overflow_hidden_wraps_children_in_a_matching_push_and_pop_clip() {
        let mut child = LayoutBox::new(BoxType::Block);
        child.dimensions = Rect { x: 0.0, y: 0.0, width: 999.0, height: 999.0 };
        child.computed_style.insert("background-color".to_string(), "#ff0000".to_string());

        let mut parent = LayoutBox::new(BoxType::Block);
        parent.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 50.0 };
        parent.computed_style.insert("overflow".to_string(), "hidden".to_string());
        parent.children.push(child);

        let list = DisplayList::build(&parent, &ImageMap::new());

        let push_index = list.items.iter().position(|item| matches!(item, DisplayItem::PushClip { .. })).expect("deberia existir PushClip");
        let pop_index = list.items.iter().position(|item| matches!(item, DisplayItem::PopClip)).expect("deberia existir PopClip");
        let child_index = list.items.iter().position(|item| matches!(item, DisplayItem::SolidRect { .. })).expect("deberia existir el fondo del hijo");

        assert!(push_index < child_index && child_index < pop_index, "el hijo deberia quedar ENTRE el PushClip y su PopClip");
        let DisplayItem::PushClip { rect } = &list.items[push_index] else { unreachable!() };
        assert_eq!((rect.width, rect.height), (100.0, 50.0), "el recorte usa las dimensiones del PADRE (overflow: hidden), no las del hijo desbordado");
    }

    #[test]
    fn overflow_visible_does_not_emit_any_clip() {
        let mut node = LayoutBox::new(BoxType::Block);
        node.computed_style.insert("overflow".to_string(), "visible".to_string());
        let list = DisplayList::build(&node, &ImageMap::new());
        assert!(!list.items.iter().any(|item| matches!(item, DisplayItem::PushClip { .. } | DisplayItem::PopClip)));
    }
}
