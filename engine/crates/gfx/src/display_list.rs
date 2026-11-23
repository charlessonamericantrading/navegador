use engine_image::DecodedImage;
use engine_layout::{ImageMap, LayoutBox, BoxType, Rect};
use engine_text::FontFamily;
use std::collections::HashMap;
use std::sync::Arc;

/// Valores iniciales CSS reales para las dos propiedades heredables que
/// aplicamos (ver `INHERITABLE_PROPERTIES` en engine-layout): un navegador
/// real tampoco pinta texto sin color ni tamaño, usa estos mismos por
/// defecto cuando nada en la cascada los redefine.
const INITIAL_COLOR: [u8; 4] = [0, 0, 0, 255];
pub const INITIAL_FONT_SIZE: f32 = 16.0;
/// Gris real de un `::placeholder` sin estilo de autor (Fase 34) - el mismo
/// tono que usan Chrome/Firefox por defecto, para que un placeholder se
/// distinga visualmente de un `value` real sin depender del `color` propio
/// del control (que un placeholder, a diferencia de un valor, no hereda).
const PLACEHOLDER_COLOR: [u8; 4] = [117, 117, 117, 255];

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
    /// `underline` (Fase 29, `text-decoration: underline`) - la cascada ya
    /// resolvia esta propiedad desde que existe (`text-decoration` no esta
    /// en `BACKGROUND_NON_COLOR_KEYWORDS` ni necesita estarlo, es una
    /// propiedad normal), pero nada la PINTABA hasta esta fase. El valor
    /// por defecto real de `<a>` (hoja de agente de usuario, `a {
    /// text-decoration: underline; }`) es el caso que mas se nota.
    /// `text_align` (Fase 31) solo tiene efecto cuando esta caja contiene
    /// VARIAS lineas envueltas internamente (ver el aviso de
    /// `resolve_text_align` mas abajo) - el resto de `text-align` ya lo
    /// resolvio `engine-layout` desplazando la caja entera antes de
    /// llegar aqui.
    /// `line_height` (tarea 2.6): la altura de linea en pixeles que el layout
    /// resolvio desde `line-height`; `None` usa la altura natural de la fuente.
    Text { rect: Rect, text: String, color: [u8; 4], font_size: f32, bold: bool, italic: bool, underline: bool, text_align: TextAlign, line_height: Option<f32>, font_family: FontFamily, first_line_indent: f32 },
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
    /// `background-image`/`background: url(...)` (Fase 40) - a diferencia
    /// de `Image` (un `<img>`, que ESCALA la imagen al tamaño resuelto por
    /// el layout, ver su doc-comment), esto pinta la imagen en su TAMAÑO
    /// NATURAL repetida en mosaico hasta cubrir `rect` entero
    /// (`background-repeat: repeat` y `background-size: auto`, los dos
    /// valores iniciales reales del spec - no se leen otros valores de
    /// ninguna de las dos propiedades todavia). `rect` es el border-box
    /// completo de la caja (misma superficie que ya pinta
    /// `background-color`, mismo criterio de `background-clip: border-box`
    /// por defecto documentado alli), y el mosaico empieza en su esquina
    /// superior-izquierda (aproxima `background-position: 0% 0%` sobre el
    /// padding-box, el origen real por defecto - no distingue border-box
    /// de padding-box). Se pinta DESPUES de `background-color` y ANTES de
    /// `Border`, igual que el spec real (una imagen de fondo tapa el color
    /// solido que haya debajo).
    BackgroundImage { rect: Rect, image: Arc<DecodedImage> },
    /// `background-image: linear-gradient(...)` (tarea 2.7 del plan, caso 7 del
    /// fixture). Se pinta donde iria el color de fondo, despues de el y antes del
    /// borde. `radius` mismo criterio que en `SolidRect`.
    Gradient { rect: Rect, gradient: Gradient, radius: f32 },
    /// `box-shadow` (Fase 3.5) - `rect` YA lleva el desplazamiento
    /// (`offset-x`/`offset-y`) aplicado (ver `parse_css_box_shadow`), asi
    /// que quien pinta esto solo rellena `rect` con `color`, ni sabe que
    /// existio un desplazamiento por separado. Se pinta ANTES que
    /// `SolidRect`/`Border` de la misma caja (orden real del spec: la
    /// sombra queda DETRAS del fondo/border). `blur` (el tercer valor de
    /// `box-shadow`, en px) SI se difumina de verdad (ver `paint_blurred_shadow`
    /// en `engine-gfx::paint`: se rellena en un lienzo aparte y se le
    /// aplican 3 pasadas de blur de caja horizontal+vertical, la misma
    /// tecnica de "cajas repetidas" que aproxima un gaussiano real en la
    /// mayoria de motores de render) - `0.0` (sin blur declarado) pinta la
    /// sombra "dura" de siempre, sin lienzo aparte. El spread-radius (4o
    /// valor) sigue sin soportarse.
    Shadow { rect: Rect, color: [u8; 4], radius: f32, blur: f32 },
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
        Self::build_items(layout_root, &mut list.items, images, &mut z_layers, &Vec::new());
        z_layers.sort_by_key(|(z, _)| *z);
        for (_, items) in z_layers {
            list.items.extend(items);
        }
        list
    }

    /// Envoltorio de `build_items_inner` que aplica `opacity` (tarea 2.6 del
    /// plan): lo que emitio la caja y todo su subarbol queda con el alfa
    /// multiplicado por la opacidad. Al envolver cada nivel, las opacidades
    /// anidadas se multiplican solas (0.5 dentro de 0.5 = 0.25).
    ///
    /// Aproximacion declarada: es la opacidad de CADA elemento sumada, no la
    /// del grupo compuesto (un hijo opaco encima de un padre translucido deja
    /// ver el fondo del padre a traves, a diferencia del spec). Las imagenes no
    /// se atenuan (sus items no llevan alfa) y el subarbol con `z-index`, que
    /// se pinta aparte, tampoco.
    fn build_items(
        layout_box: &LayoutBox,
        target: &mut Vec<DisplayItem>,
        images: &ImageMap,
        z_layers: &mut Vec<(i32, Vec<DisplayItem>)>,
        active_clips: &[Rect],
    ) {
        let opacity = parse_css_opacity(&layout_box.computed_style);
        if opacity >= 1.0 {
            return Self::build_items_inner(layout_box, target, images, z_layers, active_clips);
        }
        let start = target.len();
        Self::build_items_inner(layout_box, target, images, z_layers, active_clips);
        for item in &mut target[start..] {
            scale_item_alpha(item, opacity);
        }
    }

    fn build_items_inner(
        layout_box: &LayoutBox,
        target: &mut Vec<DisplayItem>,
        images: &ImageMap,
        z_layers: &mut Vec<(i32, Vec<DisplayItem>)>,
        active_clips: &[Rect],
    ) {
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
            return Self::build_clipped_children(layout_box, target, images, z_layers, active_clips);
        }
        match &layout_box.box_type {
            BoxType::Block | BoxType::Inline => {
                let radius = parse_css_border_radius(&layout_box.computed_style).unwrap_or(0.0);
                // `box-shadow` se pinta ANTES que fondo/border (orden real
                // del spec - ver el doc-comment de `DisplayItem::Shadow`).
                if let Some((dx, dy, color, blur)) = parse_css_box_shadow(&layout_box.computed_style) {
                    let mut rect = layout_box.dimensions.clone();
                    rect.x += dx;
                    rect.y += dy;
                    target.push(DisplayItem::Shadow { rect, color, radius, blur });
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
                push_background_image(layout_box, images, radius, target);
                if let Some((width, color)) = parse_css_border(&layout_box.computed_style) {
                    target.push(DisplayItem::Border { rect: layout_box.dimensions.clone(), width, color, radius });
                }
            }
            BoxType::Replaced => {
                let radius = parse_css_border_radius(&layout_box.computed_style).unwrap_or(0.0);
                if let Some((dx, dy, color, blur)) = parse_css_box_shadow(&layout_box.computed_style) {
                    let mut rect = layout_box.dimensions.clone();
                    rect.x += dx;
                    rect.y += dy;
                    target.push(DisplayItem::Shadow { rect, color, radius, blur });
                }
                if let Some(color) = layout_box.computed_style.get("background-color").and_then(|v| parse_css_color(v)) {
                    target.push(DisplayItem::SolidRect { rect: layout_box.dimensions.clone(), color, radius });
                }
                push_background_image(layout_box, images, radius, target);
                let border_width = if let Some((width, color)) = parse_css_border(&layout_box.computed_style) {
                    target.push(DisplayItem::Border { rect: layout_box.dimensions.clone(), width, color, radius });
                    width
                } else {
                    0.0
                };

                // Fase 34: `value`/`placeholder` de un input/textarea, o la
                // etiqueta de un boton - resuelto en `engine-layout::tree`
                // (`resolve_replaced_text`), aqui solo se pinta. `inset`
                // aproxima el `padding` por defecto real de un control
                // nativo (esta caja nunca tiene `box_dimensions.content`
                // poblado - se posiciona por el camino de layout EN LINEA,
                // no por `flow_block_children`, unico sitio que escribe
                // `box_dimensions` - asi que no hay area de contenido real
                // de la que partir).
                if let Some(replaced) = &layout_box.replaced_text {
                    let font_size = layout_box.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
                    let inset = border_width + 3.0;
                    let rect = Rect {
                        x: layout_box.dimensions.x + inset,
                        y: layout_box.dimensions.y + ((layout_box.dimensions.height - font_size * 1.2) / 2.0).max(0.0),
                        width: (layout_box.dimensions.width - inset * 2.0).max(0.0),
                        height: layout_box.dimensions.height,
                    };
                    let color = if replaced.is_placeholder {
                        PLACEHOLDER_COLOR
                    } else {
                        layout_box.computed_style.get("color").and_then(|v| parse_css_color(v)).unwrap_or(INITIAL_COLOR)
                    };
                    target.push(DisplayItem::Text {
                        rect,
                        text: replaced.text.clone(),
                        color,
                        font_size,
                        bold: false,
                        italic: false,
                        underline: false,
                        text_align: if replaced.centered { TextAlign::Center } else { TextAlign::Left },
                        line_height: None,
                        font_family: FontFamily::SansSerif,
                        first_line_indent: 0.0,
                    });
                }

                // A diferencia de `Block`/`Inline`, NO se recursa en los
                // hijos DOM reales: `engine-layout` los construye (un
                // `<select>` SI tiene una caja por cada `<option>`) pero
                // nunca los posiciona (`Rect::default()`, ver
                // `place_inline_node::BoxType::Replaced` en tree.rs) - sin
                // este corte, `build_clipped_children` los pintaba de
                // todas formas EN (0,0), superpuestos con lo que hubiera en
                // la esquina superior izquierda de la pagina. Encontrado en
                // vivo contra la pantalla de consentimiento real de
                // google.com (texto ilegible solapado ahi arriba).
                return;
            }
            BoxType::Text(content) => {
                let color = layout_box.computed_style.get("color").and_then(|v| parse_css_color(v)).unwrap_or(INITIAL_COLOR);
                let font_size = layout_box.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
                target.push(DisplayItem::Text {
                    rect: layout_box.dimensions.clone(),
                    // El texto sin el espacio que quedo al principio de una linea, si
                    // el layout lo quito (ver `LayoutBox::trimmed_text`).
                    text: layout_box.trimmed_text.clone().unwrap_or_else(|| content.clone()),
                    color,
                    font_size,
                    bold: resolve_font_weight_is_bold(&layout_box.computed_style),
                    italic: resolve_font_style_is_italic(&layout_box.computed_style),
                    underline: resolve_text_decoration_is_underline(&layout_box.computed_style),
                    text_align: resolve_text_align(&layout_box.computed_style),
                    // El layout deja en `line-height-px` el valor ya resuelto
                    // cuando el autor declaro `line-height`; sin el, altura
                    // natural de la fuente.
                    line_height: layout_box.computed_style.get("line-height-px").and_then(|v| parse_css_length(v)),
                    font_family: layout_box.computed_style.get("font-family").map(|v| FontFamily::from_css(v)).unwrap_or_default(),
                    first_line_indent: layout_box.first_line_indent,
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

        Self::build_clipped_children(layout_box, target, images, z_layers, active_clips);
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
    fn build_clipped_children(
        layout_box: &LayoutBox,
        target: &mut Vec<DisplayItem>,
        images: &ImageMap,
        z_layers: &mut Vec<(i32, Vec<DisplayItem>)>,
        active_clips: &[Rect],
    ) {
        let clips = establishes_clip(&layout_box.computed_style);
        if clips {
            target.push(DisplayItem::PushClip { rect: layout_box.dimensions.clone() });
        }
        // Recorte(s) activos que un descendiente diverted a `z_layers`
        // deberia seguir respetando - ver mas abajo. Solo se clona cuando
        // hace falta (esta caja SI recorta): en el caso comun, sin ningun
        // `overflow` recortando por el camino, es un slice vacio prestado
        // sin ninguna asignacion.
        let clips_para_hijos: Vec<Rect>;
        let clips_para_hijos: &[Rect] = if clips {
            clips_para_hijos = active_clips.iter().cloned().chain(std::iter::once(layout_box.dimensions.clone())).collect();
            &clips_para_hijos
        } else {
            active_clips
        };
        for child in &layout_box.children {
            match z_index_for_stacking(&child.computed_style) {
                Some(z) => {
                    // El sitio de ESTA capa se reserva ANTES de recursar.
                    //
                    // Si se empujara al terminar (que es lo que se hacia),
                    // cualquier capa creada por un DESCENDIENTE durante esa
                    // recursion quedaria ANTES en la lista, y como el orden
                    // se decide con una ordenacion estable por z-index, a
                    // igual z el ancestro acabaria pintandose ENCIMA de su
                    // propio contenido. El fondo de un padre va siempre por
                    // debajo de lo que contiene, y este motor lo estaba
                    // invirtiendo: en la Wikipedia real, `.mw-page-container`
                    // pintaba un rectangulo blanco de 1280x62707 sobre el
                    // articulo entero y la pagina se veia en blanco de la
                    // cabecera para abajo.
                    //
                    // Una capa z-index se pinta AL FINAL de la lista entera
                    // (ver `build`), fuera de cualquier `PushClip`/`PopClip`
                    // que un ANCESTRO haya emitido en el camino normal - asi
                    // que aqui se envuelve ella misma con los recortes que
                    // heredaria si no se hubiera desviado. Sin esto, un
                    // `overflow: hidden/auto/scroll` con un descendiente
                    // posicionado y con z-index se derramaba sin recortar -
                    // hueco que se hizo mucho mas frecuente al ampliar
                    // `establishes_clip` a `auto`/`scroll` (antes solo
                    // `overflow: hidden` exacto lo disparaba).
                    let slot = z_layers.len();
                    z_layers.push((z, Vec::new()));
                    let mut layer_items = Vec::new();
                    for rect in clips_para_hijos {
                        layer_items.push(DisplayItem::PushClip { rect: rect.clone() });
                    }
                    Self::build_items(child, &mut layer_items, images, z_layers, clips_para_hijos);
                    for _ in clips_para_hijos {
                        layer_items.push(DisplayItem::PopClip);
                    }
                    z_layers[slot].1 = layer_items;
                }
                None => Self::build_items(child, target, images, z_layers, clips_para_hijos),
            }
        }
        if clips {
            target.push(DisplayItem::PopClip);
        }
    }
}

/// Si esta caja recorta su contenido desbordado.
///
/// Antes se comparaba SOLO contra el literal `"hidden"`, asi que
/// `overflow: auto` (igual de comun), `overflow: scroll`, la forma de dos
/// valores `overflow: hidden auto` y las variantes por eje `overflow-x`/
/// `overflow-y` no recortaban nada y su contenido se derramaba por encima
/// del resto de la pagina.
///
/// `auto` y `scroll` recortan igual que `hidden` porque este motor todavia
/// no puede desplazar el interior de una caja: lo que en un navegador real
/// quedaria oculto tras una barra de scroll, aqui queda simplemente oculto.
/// Es una simplificacion declarada, pero mucho mas cercana a la verdad que
/// dejarlo salir.
fn establishes_clip(computed_style: &HashMap<String, String>) -> bool {
    let recorta = |valor: &str| matches!(valor.trim(), "hidden" | "clip" | "scroll" | "auto");
    for prop in ["overflow", "overflow-x", "overflow-y"] {
        let Some(valor) = computed_style.get(prop) else { continue };
        // `overflow: hidden auto` son DOS valores (eje x y eje y): basta con
        // que uno de los dos recorte.
        if valor.split_whitespace().any(recorta) {
            return true;
        }
    }
    false
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
/// `hwb()` (blancura/negrura, misma familia que HSL - `hwb_to_rgb` mas
/// abajo) y `oklch()` (el espacio de color perceptualmente uniforme de CSS
/// Color 4 - `oklch_to_rgb` mas abajo, la formula de referencia de Björn
/// Ottosson, sin aproximacion) tambien se parsean de verdad.
///
/// NO implementado: `lab()`/`lch()`/`oklab()` y el resto de espacios de
/// color modernos - devuelven `None` y la caja se queda sin pintar, en vez
/// de fingir una conversion. Tampoco los ~90 nombres extendidos que faltan
/// en la tabla.
pub(crate) fn parse_css_color(value: &str) -> Option<[u8; 4]> {
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
        let normalised = inner.replace([',', '/'], " ");
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
        let normalised = inner.replace([',', '/'], " ");
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

    if let Some(rest) = lower.strip_prefix("hwb(") {
        let inner = rest.strip_suffix(')')?;
        let normalised = inner.replace([',', '/'], " ");
        let parts: Vec<&str> = normalised.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }
        let hue = parse_hue_degrees(parts[0])?;
        let whiteness = parse_percentage_0_1(parts[1])?;
        let blackness = parse_percentage_0_1(parts[2])?;
        let (r, g, b) = hwb_to_rgb(hue, whiteness, blackness);
        let a = match parts.get(3) {
            Some(token) => parse_alpha_0_1_or_percent(token)?,
            None => 255,
        };
        return Some([r, g, b, a]);
    }

    if let Some(rest) = lower.strip_prefix("oklch(") {
        let inner = rest.strip_suffix(')')?;
        let normalised = inner.replace('/', " ");
        let parts: Vec<&str> = normalised.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }
        let lightness = parse_oklch_lightness(parts[0])?;
        let chroma = parts[1].trim().parse::<f32>().ok()?.max(0.0);
        let hue = parse_hue_degrees(parts[2])?;
        let (r, g, b) = oklch_to_rgb(lightness, chroma, hue);
        let a = match parts.get(3) {
            Some(token) => parse_alpha_0_1_or_percent(token)?,
            None => 255,
        };
        return Some([r, g, b, a]);
    }

    NAMED_COLORS.iter().find(|(name, _)| *name == lower).map(|(_, rgba)| *rgba)
}

/// Componente de alfa de `hwb()`/`oklch()` - mismo parseo (numero 0.0-1.0 o
/// porcentaje) que ya usan `rgb()`/`hsl()` mas arriba, extraido aqui porque
/// dos consumidores mas lo necesitan tal cual.
fn parse_alpha_0_1_or_percent(token: &str) -> Option<u8> {
    let t = token.trim();
    let alpha = match t.strip_suffix('%') {
        Some(percent) => percent.trim().parse::<f32>().ok()? / 100.0,
        None => t.parse::<f32>().ok()?,
    };
    Some((alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// `hwb(hue whiteness blackness)` -> RGB, la formula estandar de CSS Color
/// 4 §7.1: si blancura+negrura cubren el 100% o mas, el resultado es un
/// gris puro (proporcional a cuanta blancura hay respecto al total);
/// si no, se parte del color puro de ese matiz (`hsl_to_rgb` con
/// saturacion 100%/luminosidad 50%, el mismo helper que ya usa `hsl()`) y
/// se mezcla hacia blanco/negro segun las dos proporciones.
fn hwb_to_rgb(hue_deg: f32, whiteness: f32, blackness: f32) -> (u8, u8, u8) {
    let w = whiteness.clamp(0.0, 1.0);
    let b = blackness.clamp(0.0, 1.0);
    if w + b >= 1.0 {
        let gray = (w / (w + b) * 255.0).round() as u8;
        return (gray, gray, gray);
    }
    let (r, g, bl) = hsl_to_rgb(hue_deg, 1.0, 0.5);
    let mix = |c: u8| -> u8 {
        let c = c as f32 / 255.0;
        ((c * (1.0 - w - b) + w) * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (mix(r), mix(g), mix(bl))
}

/// Luminosidad (`L`) de `oklch()`: `0.0..1.0` de verdad (mas comun como
/// porcentaje, `70%`) o el numero directo que tambien acepta CSS Color 4 -
/// acotado igual que el resto del parseador.
fn parse_oklch_lightness(token: &str) -> Option<f32> {
    let token = token.trim();
    if let Some(percent) = token.strip_suffix('%') {
        return Some((percent.trim().parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0));
    }
    token.parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0))
}

/// `oklch(L C H)` -> RGB: la formula de referencia de Björn Ottosson
/// (creador de OKLab) sin aproximacion - LCH cilindrico a OKLab
/// rectangular (`a = C*cos(H)`, `b = C*sin(H)`), OKLab a sRGB LINEAL via
/// las matrices estandar del espacio, y de ahi a sRGB con la curva gamma
/// real del spec (tramo lineal bajo `0.0031308`, potencia `1/2.4` el
/// resto). Un `L`/`C`/`H` fuera de gama (el color pedido no cabe en sRGB)
/// se ACOTA al canal 0..1 mas cercano tras la curva gamma en vez de
/// devolver `None` o un color inventado - un navegador real hace lo mismo
/// (gamut mapping por recorte).
fn oklch_to_rgb(lightness: f32, chroma: f32, hue_deg: f32) -> (u8, u8, u8) {
    let hue = hue_deg.to_radians();
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();

    let l_ = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = lightness - 0.089_484_18 * a - 1.291_485_5 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let r_lin = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g_lin = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b_lin = -0.0041960863 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;

    let gamma_encode = |c: f32| -> u8 {
        let c = c.clamp(0.0, 1.0);
        let encoded = if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    (gamma_encode(r_lin), gamma_encode(g_lin), gamma_encode(b_lin))
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

/// `text-decoration: underline` (Fase 29) - decide si `paint_text` dibuja
/// la franja de subrayado. Busca el token `underline` entre los valores
/// separados por espacio (el shorthand real admite ademas estilo/color,
/// `text-decoration: underline dotted red`) en vez de comparar el valor
/// entero, para no fallar ante esa forma compuesta. `line-through`/
/// `overline` se PARSEAN igual (no se descartan como candidatos), pero
/// solo `underline` esta conectado a algo que pinte - los otros dos
/// siguen sin implementarse, declarado.
fn resolve_text_decoration_is_underline(computed_style: &HashMap<String, String>) -> bool {
    let Some(raw) = computed_style.get("text-decoration") else { return false };
    raw.split_whitespace().any(|token| token.eq_ignore_ascii_case("underline"))
}

/// Copia deliberada de `TextAlign`/`resolve_text_align` en
/// `engine-layout::tree` (Fase 31, misma razon que las copias de arriba:
/// dos crates que no deben depender entre si) - decide como `paint_text`
/// centra/alinea cada LINEA de un texto que envuelve DENTRO de una sola
/// caja (`BoxType::Text` con `width == inner_width`, la rama "ni siquiera
/// cabe sola" de `place_inline_node`), el UNICO caso de `text-align` que
/// `engine-layout` deja sin resolver a proposito - ver su aviso en
/// `apply_text_align`. Fuera de esa rama (una linea que cabe entera en su
/// caja, la caja YA mide justo lo que su contenido necesita) esta funcion
/// no tiene nada que desplazar: `rect.width` coincide con el ancho real
/// del texto, y el offset que calcularia seria cero - la alineacion de
/// ESE caso ya la resolvio `engine-layout::tree::apply_text_align`
/// desplazando la caja entera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

fn resolve_text_align(computed_style: &HashMap<String, String>) -> TextAlign {
    match computed_style.get("text-align").map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "center" => TextAlign::Center,
        Some(v) if v == "right" || v == "end" => TextAlign::Right,
        _ => TextAlign::Left,
    }
}

/// Igual que `parse_css_font_size`, pero SI acepta cero (un ancho de
/// border de 0 es valido) - copia deliberada de `parse_css_length` en
/// `engine-layout/src/tree.rs`, misma razon que `INITIAL_FONT_SIZE`/
/// `parse_css_font_size` de arriba: dos crates que no deben depender
/// entre si, y unas pocas lineas no justifican enredar la dependencia.
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

    for token in engine_layout::split_css_tokens(raw) {
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

/// Color del anillo de foco de un control (azul de navegador).
const FOCUS_RING_COLOR: [u8; 4] = [26, 115, 232, 255];

/// Lo que se pinta ENCIMA de la pagina para el control con el foco (tarea 3.2):
/// un anillo azul alrededor de su caja y, en un campo de texto de una linea, el
/// cursor al final de lo escrito. No es parte de la pagina (no cambia su layout
/// ni su DOM): se añade a la captura y se tira.
///
/// La geometria del cursor repite la del texto del campo (`inset = borde + 3`,
/// centrado en vertical), que es donde `build_items_inner` lo pinta.
pub fn focus_overlay(focused: &LayoutBox, font_set: Option<&engine_text::FontSet>) -> Vec<DisplayItem> {
    let mut items = vec![DisplayItem::Border { rect: focused.dimensions.clone(), width: 2.0, color: FOCUS_RING_COLOR, radius: parse_css_border_radius(&focused.computed_style).unwrap_or(0.0) }];

    let is_text_field = matches!(focused.box_type, BoxType::Replaced) && !focused.replaced_text.as_ref().is_some_and(|r| r.centered);
    let multiline = focused.dimensions.height > 40.0;
    if is_text_field && !multiline {
        let font_size = focused.computed_style.get("font-size").and_then(|v| parse_css_font_size(v)).unwrap_or(INITIAL_FONT_SIZE);
        let border_width = parse_css_border(&focused.computed_style).map_or(0.0, |(width, _)| width);
        let inset = border_width + 3.0;
        // Un campo vacio (con o sin placeholder) lleva el cursor al principio; con
        // texto, tras la ultima letra.
        let typed_width = match (&focused.replaced_text, font_set.and_then(|set| set.pick_family(FontFamily::SansSerif, false, false))) {
            (Some(replaced), Some(font)) if !replaced.is_placeholder => engine_text::text_width(font, &replaced.text, font_size),
            _ => 0.0,
        };
        let line = font_size * 1.2;
        let max_x = focused.dimensions.x + (focused.dimensions.width - inset).max(0.0);
        items.push(DisplayItem::SolidRect {
            rect: Rect {
                x: (focused.dimensions.x + inset + typed_width).min(max_x),
                y: focused.dimensions.y + ((focused.dimensions.height - line) / 2.0).max(0.0),
                width: 1.0,
                height: line.min(focused.dimensions.height),
            },
            color: focused.computed_style.get("color").and_then(|v| parse_css_color(v)).unwrap_or(INITIAL_COLOR),
            radius: 0.0,
        });
    }
    items
}

/// Hacia donde apunta un degradado lineal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientDirection {
    /// Angulo CSS: 0deg apunta arriba, 90deg a la derecha (en el sentido del reloj).
    Degrees(f32),
    /// `to bottom right`...: hacia una esquina. `sx`/`sy` valen +1 (derecha/abajo)
    /// o -1 (izquierda/arriba). El angulo depende de la forma de la caja, asi que
    /// se resuelve al pintar.
    Corner { sx: f32, sy: f32 },
}

/// Donde cae una parada de color en la linea del degradado.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopPosition {
    /// Fraccion de 0 a 1 de la linea.
    Fraction(f32),
    Pixels(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub color: [u8; 4],
    pub position: StopPosition,
}

/// `linear-gradient(...)` ya interpretado: paradas con posicion resuelta (las
/// que el autor no fijo se reparten por igual) y siempre ordenadas.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub direction: GradientDirection,
    pub stops: Vec<GradientStop>,
}

/// El fondo de imagen de una caja: un degradado se pinta tal cual; un `url(...)`
/// solo si su imagen ya esta descargada.
fn push_background_image(layout_box: &LayoutBox, images: &ImageMap, radius: f32, target: &mut Vec<DisplayItem>) {
    if let Some(gradient) = layout_box.computed_style.get("background-image").and_then(|v| parse_css_linear_gradient(v)) {
        target.push(DisplayItem::Gradient { rect: layout_box.dimensions.clone(), gradient, radius });
        return;
    }
    if let Some(image) = parse_css_background_image(&layout_box.computed_style).and_then(|url| images.get(&url)) {
        target.push(DisplayItem::BackgroundImage { rect: layout_box.dimensions.clone(), image: image.clone() });
    }
}

/// `linear-gradient([<angulo> | to <lado>,] <color> [<posicion>], ...)` (tarea
/// 2.7). `None` si no es un degradado lineal, si tiene menos de dos colores o si
/// algun color no se entiende: un degradado a medias seria peor que ninguno.
/// Sin `repeating-` ni parada doble de dos posiciones.
pub(crate) fn parse_css_linear_gradient(value: &str) -> Option<Gradient> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let inner = lower.strip_prefix("linear-gradient(").and_then(|rest| rest.strip_suffix(')'))?;
    // Los colores y posiciones se leen del texto original (los nombres de color
    // no dependen de mayusculas, pero las URLs si; aqui solo hay colores).
    let inner = &value[value.len() - inner.len() - 1..value.len() - 1];
    let parts = engine_layout::split_css_commas(inner);
    let mut parts = parts.into_iter().map(str::trim).collect::<Vec<_>>();

    let mut direction = GradientDirection::Degrees(180.0);
    if let Some(first) = parts.first().copied() {
        let first_lower = first.to_ascii_lowercase();
        if let Some(sides) = first_lower.strip_prefix("to ") {
            let (mut sx, mut sy) = (0.0_f32, 0.0_f32);
            for side in sides.split_whitespace() {
                match side {
                    "right" => sx = 1.0,
                    "left" => sx = -1.0,
                    "bottom" => sy = 1.0,
                    "top" => sy = -1.0,
                    _ => return None,
                }
            }
            direction = match (sx, sy) {
                (0.0, -1.0) => GradientDirection::Degrees(0.0),
                (1.0, 0.0) => GradientDirection::Degrees(90.0),
                (0.0, 1.0) => GradientDirection::Degrees(180.0),
                (-1.0, 0.0) => GradientDirection::Degrees(270.0),
                (sx, sy) if sx != 0.0 && sy != 0.0 => GradientDirection::Corner { sx, sy },
                _ => return None,
            };
            parts.remove(0);
        } else if let Some(degrees) = parse_gradient_angle(&first_lower) {
            direction = GradientDirection::Degrees(degrees);
            parts.remove(0);
        }
    }

    let mut raw: Vec<([u8; 4], Option<StopPosition>)> = Vec::new();
    for part in parts {
        let tokens = engine_layout::split_css_tokens(part);
        let color_token = tokens.first()?;
        // Una "pista de color" (`50%` suelto, sin color) no es una parada: se salta.
        if parse_gradient_position(color_token).is_some() && tokens.len() == 1 {
            continue;
        }
        let color = parse_css_color(color_token)?;
        let positions: Vec<StopPosition> = tokens[1..].iter().filter_map(|token| parse_gradient_position(token)).collect();
        if positions.is_empty() {
            raw.push((color, None));
        } else {
            // `red 10% 30%` son dos paradas del mismo color.
            for position in positions.into_iter().take(2) {
                raw.push((color, Some(position)));
            }
        }
    }
    if raw.len() < 2 {
        return None;
    }

    // Posiciones que faltan: la primera 0, la ultima 1 y el resto se reparte por
    // igual entre las paradas con posicion que la rodean. Solo se reparte entre
    // fracciones; si hay pixeles se tratan aparte al pintar, y aqui las
    // intermedias sin posicion cuentan desde el extremo anterior conocido.
    let last_index = raw.len() - 1;
    if raw[0].1.is_none() {
        raw[0].1 = Some(StopPosition::Fraction(0.0));
    }
    if raw[last_index].1.is_none() {
        raw[last_index].1 = Some(StopPosition::Fraction(1.0));
    }
    let mut index = 0;
    while index < raw.len() {
        if raw[index].1.is_some() {
            index += 1;
            continue;
        }
        let run_start = index - 1;
        let mut run_end = index;
        while raw[run_end].1.is_none() {
            run_end += 1;
        }
        let (StopPosition::Fraction(from), StopPosition::Fraction(to)) = (raw[run_start].1?, raw[run_end].1?) else {
            // Mezcla de pixeles y repartos automaticos: no se interpreta.
            return None;
        };
        let missing = (run_end - run_start) as f32;
        for (step, slot) in raw[run_start + 1..run_end].iter_mut().enumerate() {
            slot.1 = Some(StopPosition::Fraction(from + (to - from) * (step as f32 + 1.0) / missing));
        }
        index = run_end + 1;
    }

    // Las posiciones no retroceden: una parada nunca queda antes que la anterior.
    let mut stops = Vec::with_capacity(raw.len());
    let mut floor = f32::NEG_INFINITY;
    for (color, position) in raw {
        let position = match position? {
            StopPosition::Fraction(f) => StopPosition::Fraction(f.max(floor.min(f32::MAX))),
            pixels => pixels,
        };
        if let StopPosition::Fraction(f) = position {
            floor = floor.max(f);
        }
        stops.push(GradientStop { color, position });
    }
    Some(Gradient { direction, stops })
}

/// `45deg`, `0.25turn`, `1.5708rad`, `100grad` -> grados.
fn parse_gradient_angle(token: &str) -> Option<f32> {
    let token = token.trim();
    for (suffix, to_degrees) in [("deg", 1.0), ("turn", 360.0), ("rad", 180.0 / std::f32::consts::PI), ("grad", 0.9)] {
        if let Some(number) = token.strip_suffix(suffix) {
            return number.trim().parse::<f32>().ok().map(|n| n * to_degrees);
        }
    }
    None
}

/// `30%` o `40px` -> posicion; cualquier otra cosa (un color) no lo es.
fn parse_gradient_position(token: &str) -> Option<StopPosition> {
    let token = token.trim();
    if let Some(percent) = token.strip_suffix('%') {
        return percent.trim().parse::<f32>().ok().map(|n| StopPosition::Fraction(n / 100.0));
    }
    token.strip_suffix("px").and_then(|px| px.trim().parse::<f32>().ok()).map(StopPosition::Pixels)
}

/// `background-image` (Fase 40) - extrae la URL "desnuda" (sin
/// `url(...)`/comillas) de la declaracion ya expandida por la cascada
/// (`engine_css::parser::insert_declaration` ya deja aqui el `url(...)`
/// tanto si el autor escribio el longhand como el shorthand `background:
/// ... url(...) ...`). Mismo formato sin resolver que usa `ImageMap`/
/// `BoxType::Image` para `<img src>` (ver su doc-comment en
/// `engine-layout::tree`) - asi `images.get(...)` encuentra la misma
/// entrada sin importar de cual de las dos fuentes vino. No distingue
/// `url(...)` de otra funcion de imagen (`linear-gradient(...)`, sin
/// soportar) - un valor sin `url(` simplemente no encuentra nada, la
/// misma degradacion honesta de siempre.
pub(crate) fn parse_css_background_image(computed_style: &HashMap<String, String>) -> Option<String> {
    let raw = computed_style.get("background-image")?;
    let lower = raw.to_ascii_lowercase();
    let start = lower.find("url(")?;
    let after = &raw[start + 4..];
    let end = after.find(')')?;
    let inner = after[..end].trim();
    let inner = inner
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(inner);
    (!inner.is_empty()).then(|| inner.to_string())
}

/// `border-radius` (Fase 3.5) - un unico valor en `px`, aplicado a las 4
/// esquinas por igual (misma simplificacion "un solo numero" que
/// `padding`/`margin`/`border-width`). `None`/cero/negativo resuelve a
/// "sin redondeo" (esquinas cuadradas, el valor inicial real).
fn parse_css_border_radius(computed_style: &HashMap<String, String>) -> Option<f32> {
    let raw = computed_style.get("border-radius")?;
    if let Some(r) = parse_css_length(raw) {
        return (r > 0.0).then_some(r);
    }
    for token in raw.split_whitespace() {
        if let Some(r) = parse_css_length(token) {
            if r > 0.0 {
                return Some(r);
            }
        }
    }
    None
}

/// `opacity` (tarea 2.6 del plan): un numero de 0 a 1 o un porcentaje, acotado
/// a ese rango. Ausente o invalido: 1 (opaco, el valor inicial).
fn parse_css_opacity(computed_style: &HashMap<String, String>) -> f32 {
    let Some(raw) = computed_style.get("opacity") else { return 1.0 };
    let raw = raw.trim();
    let value = match raw.strip_suffix('%') {
        Some(percent) => percent.trim().parse::<f32>().ok().map(|n| n / 100.0),
        None => raw.parse::<f32>().ok(),
    };
    value.map(|v| v.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// Multiplica el alfa del color de un item por `factor`. Los items sin color
/// (imagenes, recortes) no se tocan.
fn scale_item_alpha(item: &mut DisplayItem, factor: f32) {
    let scale = |color: &mut [u8; 4]| color[3] = (color[3] as f32 * factor).round() as u8;
    match item {
        DisplayItem::SolidRect { color, .. } | DisplayItem::Text { color, .. } | DisplayItem::Border { color, .. } | DisplayItem::Shadow { color, .. } => scale(color),
        DisplayItem::Gradient { gradient, .. } => gradient.stops.iter_mut().for_each(|stop| scale(&mut stop.color)),
        _ => {}
    }
}

/// `box-shadow: <offset-x> <offset-y> [<blur-radius>] <color>` (Fase 3.5,
/// blur real desde entonces) - `offset-x`/`offset-y` aceptan negativos (a
/// diferencia de `parse_css_length`, que rechaza negativos porque un
/// padding/border negativo no tiene sentido - un offset de sombra si) via
/// `parse_css_offset` local, deliberadamente NO compartida con la copia de
/// `engine-layout::tree` (misma razon de siempre: crates que no deben
/// depender entre si). El 3er numero (si esta) es el blur-radius, acotado a
/// no-negativo igual que el resto del motor acota en vez de rechazar - un
/// 4o numero (spread-radius) sigue sin soportarse (se ignora, no rompe el
/// resto del parseo). `None` si faltan offset-x/offset-y o el color, o si la
/// propiedad no esta puesta - sin sombra por defecto, el valor inicial real
/// de la propiedad.
fn parse_css_box_shadow(computed_style: &HashMap<String, String>) -> Option<(f32, f32, [u8; 4], f32)> {
    fn parse_offset(value: &str) -> Option<f32> {
        let value = value.trim();
        // `0` sin unidad es valido en CSS y es la forma habitual de escribir
        // el desplazamiento nulo (`0 2px 8px ...`). Sin esto se saltaba y todos
        // los numeros siguientes se corrian una posicion: dx/dy/blur
        // equivocados.
        if value == "0" {
            return Some(0.0);
        }
        let px = value.strip_suffix("px")?;
        px.trim().parse::<f32>().ok()
    }

    let raw = computed_style.get("box-shadow")?;
    let mut offsets: Vec<f32> = Vec::new();
    let mut color: Option<[u8; 4]> = None;

    // Con varias sombras (`a, b`) se usa la primera EXTERIOR: el motor pinta
    // una sola, y las `inset` (interiores) no se dibujan, porque pintarlas
    // como exteriores ensuciaria el borde de la caja con una sombra que el
    // autor puso dentro.
    let layer = engine_layout::split_css_commas(raw).into_iter().find(|layer| !engine_layout::split_css_tokens(layer).iter().any(|t| t.eq_ignore_ascii_case("inset")))?;

    for token in engine_layout::split_css_tokens(layer) {
        if let Some(c) = parse_css_color(token) {
            color = Some(c);
        } else if let Some(n) = parse_offset(token) {
            offsets.push(n);
        }
    }

    let dx = *offsets.first()?;
    let dy = *offsets.get(1)?;
    let blur = offsets.get(2).copied().unwrap_or(0.0).max(0.0);
    Some((dx, dy, color.unwrap_or(INITIAL_COLOR), blur))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tarea 2.6 del plan: `opacity` multiplica el alfa de la caja y de todo su
    /// subarbol, y las opacidades anidadas se multiplican.
    #[test]
    fn opacity_scales_the_alpha_of_the_box_and_its_descendants() {
        let construir = |opacidad_padre: Option<&str>, opacidad_hijo: Option<&str>| {
            let mut padre = LayoutBox::new(BoxType::Block);
            padre.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
            padre.computed_style.insert("background-color".to_string(), "#000000".to_string());
            if let Some(o) = opacidad_padre {
                padre.computed_style.insert("opacity".to_string(), o.to_string());
            }
            let mut hijo = LayoutBox::new(BoxType::Block);
            hijo.dimensions = Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 };
            hijo.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
            if let Some(o) = opacidad_hijo {
                hijo.computed_style.insert("opacity".to_string(), o.to_string());
            }
            padre.children.push(hijo);
            DisplayList::build(&padre, &ImageMap::new())
                .items
                .iter()
                .filter_map(|item| match item {
                    DisplayItem::SolidRect { color, .. } => Some(color[3]),
                    _ => None,
                })
                .collect::<Vec<u8>>()
        };

        assert_eq!(construir(None, None), vec![255, 255], "sin opacity no cambia nada");
        assert_eq!(construir(Some("0.5"), None), vec![128, 128], "el padre translucido atenua tambien a su hijo");
        assert_eq!(construir(Some("0.5"), Some("0.5")), vec![128, 64], "las opacidades anidadas se multiplican");
        assert_eq!(construir(Some("30%"), None), vec![77, 77], "tambien en porcentaje");
        assert_eq!(construir(Some("0"), None), vec![0, 0], "opacity 0: invisible");
        assert_eq!(construir(Some("7"), None), vec![255, 255], "fuera de rango se acota a 1");
        assert_eq!(construir(Some("nada"), None), vec![255, 255], "un valor invalido es opaco");
    }

    /// Tarea 2.6 del plan: el `line-height` que el layout dejo resuelto en la
    /// hoja de texto llega al item de pintado; sin el, es `None` (altura
    /// natural de la fuente).
    #[test]
    fn a_text_leaf_carries_its_resolved_line_height_into_the_display_list() {
        let altura = |estilo: Option<&str>| {
            let mut root = LayoutBox::new(BoxType::Block);
            root.dimensions.width = 200.0;
            root.dimensions.height = 100.0;
            let mut hoja = LayoutBox::new(BoxType::Text("hola".to_string()));
            hoja.dimensions = Rect { x: 0.0, y: 0.0, width: 40.0, height: 40.0 };
            if let Some(valor) = estilo {
                hoja.computed_style.insert("line-height-px".to_string(), valor.to_string());
            }
            root.children.push(hoja);
            let lista = DisplayList::build(&root, &ImageMap::new());
            lista.items.iter().find_map(|item| match item {
                DisplayItem::Text { line_height, .. } => Some(*line_height),
                _ => None,
            })
        };

        assert_eq!(altura(Some("40px")), Some(Some(40.0)));
        assert_eq!(altura(None), Some(None), "sin line-height: altura natural");
        assert_eq!(altura(Some("1.5")), Some(None), "un valor que no es una longitud: altura natural");
    }

    /// Tarea 2.5 del plan: un `rgba(...)` con espacios dentro de `box-shadow`
    /// se reconoce como color. Antes el valor se partia por esos espacios, el
    /// color no se encontraba y la sombra se pintaba en NEGRO OPACO (la caja
    /// negra de la portada de Google).
    #[test]
    fn a_box_shadow_with_a_spaced_rgba_color_keeps_that_color() {
        let sombra = parse_css_box_shadow(&style_with("box-shadow", "0px 3px 10px 0px rgba(31, 31, 31, 0.08)"));
        assert_eq!(sombra, Some((0.0, 3.0, [31, 31, 31, 20], 10.0)), "color con alfa 0.08, no negro opaco");
    }

    #[test]
    fn with_several_box_shadows_the_first_outer_one_is_used_and_inset_ones_are_skipped() {
        let varias = parse_css_box_shadow(&style_with("box-shadow", "0 1px 2px #ff0000, 0 4px 8px #00ff00"));
        assert_eq!(varias, Some((0.0, 1.0, [255, 0, 0, 255], 2.0)));

        let inset_primero = parse_css_box_shadow(&style_with("box-shadow", "inset 0 1px 0 #ffffff, 0 2px 4px #0000ff"));
        assert_eq!(inset_primero, Some((0.0, 2.0, [0, 0, 255, 255], 4.0)), "la sombra inset no se pinta como exterior");

        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "inset 0 1px 0 #ffffff")), None, "solo inset: no hay sombra exterior que pintar");
    }

    /// `0` sin unidad es un desplazamiento valido: `0 2px 8px #000` es dx=0,
    /// dy=2, blur=8 (antes salia dx=2, dy=8, blur=0).
    #[test]
    fn a_unitless_zero_offset_in_a_box_shadow_is_read() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "0 2px 8px #000000")), Some((0.0, 2.0, [0, 0, 0, 255], 8.0)));
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "0 0 0 1px #ff0000")), Some((0.0, 0.0, [255, 0, 0, 255], 0.0)));
    }

    #[test]
    fn a_border_with_a_spaced_rgba_color_keeps_that_color() {
        let borde = parse_css_border(&style_with("border", "1px solid rgba(0, 0, 0, 0.3)"));
        assert_eq!(borde, Some((1.0, [0, 0, 0, 77])), "borde translucido, no negro solido");
    }

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
        assert_eq!(parse_css_color("lab(50% 40 60)"), None, "lab() no esta implementado");
        assert_eq!(parse_css_color("oklab(0.5 0.1 0.1)"), None, "oklab() no esta implementado");
        assert_eq!(parse_css_color("currentColor"), None, "currentColor exige el computed_style completo");
        assert_eq!(parse_css_color("basura"), None);
        assert_eq!(parse_css_color(""), None);
    }

    /// `hwb(0 0% 0%)` es rojo puro (matiz 0, sin blancura ni negrura que lo
    /// desature) - mismo punto de referencia facil de verificar que ya usan
    /// las pruebas de `hsl()`.
    #[test]
    fn hwb_with_no_whiteness_or_blackness_is_the_pure_hue() {
        assert_eq!(parse_css_color("hwb(0 0% 0%)"), Some([255, 0, 0, 255]));
    }

    /// Blancura+negrura sumando 100% o mas colapsa a un gris puro
    /// (proporcional a cuanta blancura hay) - el matiz deja de importar.
    #[test]
    fn hwb_with_whiteness_plus_blackness_over_100_percent_is_a_pure_gray() {
        assert_eq!(parse_css_color("hwb(210 60% 60%)"), Some([128, 128, 128, 255]));
    }

    #[test]
    fn hwb_reads_the_optional_alpha_after_a_slash() {
        assert_eq!(parse_css_color("hwb(0 0% 0% / 0.5)"), Some([255, 0, 0, 128]));
    }

    /// `oklch(1 0 0)` es blanco puro (luminosidad maxima, sin croma) y
    /// `oklch(0 0 0)` es negro puro (luminosidad minima) - los dos puntos
    /// de referencia que cualquier implementacion de OKLCH tiene que dar
    /// exactos, sin importar la formula interna.
    #[test]
    fn oklch_at_the_lightness_extremes_is_pure_white_or_black() {
        assert_eq!(parse_css_color("oklch(1 0 0)"), Some([255, 255, 255, 255]));
        assert_eq!(parse_css_color("oklch(0 0 0)"), Some([0, 0, 0, 255]));
    }

    /// `oklch(0.63 0.26 29)` es el rojo de referencia de CSS Color 4
    /// (equivale casi exacto a `red`/`#ff0000`) - el punto de referencia
    /// que documenta el propio spec para verificar una implementacion de
    /// OKLCH contra un color RGB conocido.
    #[test]
    fn oklch_of_a_known_red_matches_srgb_red_closely() {
        let Some([r, g, b, a]) = parse_css_color("oklch(0.627955 0.257683 29.2339)") else { panic!("deberia parsear") };
        assert_eq!(a, 255);
        assert!(r >= 250, "canal rojo deberia estar cerca de 255, salio {r}");
        assert!(g <= 20, "canal verde deberia estar cerca de 0, salio {g}");
        assert!(b <= 20, "canal azul deberia estar cerca de 0, salio {b}");
    }

    #[test]
    fn oklch_lightness_accepts_a_percentage_or_a_bare_number() {
        assert_eq!(parse_css_color("oklch(100% 0 0)"), parse_css_color("oklch(1 0 0)"));
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

    /// Fase 34: el punto real de esta fase - un `BoxType::Replaced` con
    /// `replaced_text` resuelto deberia emitir un `DisplayItem::Text`, no
    /// quedarse en silencio como antes (el bug real: "Google Search" como
    /// recuadro gris vacio).
    #[test]
    fn a_replaced_box_with_resolved_text_emits_a_text_display_item() {
        let mut input = LayoutBox::new(BoxType::Replaced);
        input.dimensions = Rect { x: 10.0, y: 10.0, width: 170.0, height: 21.0 };
        input.replaced_text = Some(engine_layout::ReplacedText { text: "hola".to_string(), is_placeholder: false, centered: false });

        let list = DisplayList::build(&input, &ImageMap::new());

        assert!(
            list.items.iter().any(|item| matches!(item, DisplayItem::Text { text, .. } if text == "hola")),
            "deberia pintar el value resuelto como texto real"
        );
    }

    /// Tarea 3.2: el foco de un campo de texto dibuja un anillo y un cursor que se
    /// desplaza con lo escrito; un boton solo lleva anillo.
    #[test]
    fn focus_overlay_draws_a_ring_and_a_caret_that_follows_the_text() {
        let font_set = engine_text::FontSet::load_default_sans_serif();
        if font_set.pick(false, false).is_none() {
            return;
        }
        let field = |text: &str, placeholder: bool| {
            let mut b = LayoutBox::new(BoxType::Replaced);
            b.dimensions = Rect { x: 10.0, y: 10.0, width: 170.0, height: 21.0 };
            b.replaced_text = Some(engine_layout::ReplacedText { text: text.to_string(), is_placeholder: placeholder, centered: false });
            b.computed_style.insert("border".to_string(), "1px solid #767676".to_string());
            b
        };
        let caret_x = |items: &[DisplayItem]| items.iter().find_map(|i| match i { DisplayItem::SolidRect { rect, .. } => Some(rect.x), _ => None }).expect("un cursor");

        let empty = focus_overlay(&field("", true), Some(&font_set));
        assert!(matches!(empty[0], DisplayItem::Border { .. }), "el anillo");
        let at_start = caret_x(&empty);
        assert!((at_start - 14.0).abs() < 0.5, "sin texto, el cursor al principio ({at_start})");

        let typed = focus_overlay(&field("hola", false), Some(&font_set));
        assert!(caret_x(&typed) > at_start + 15.0, "con texto, tras la ultima letra ({})", caret_x(&typed));

        let mut button = field("Enviar", false);
        button.replaced_text.as_mut().unwrap().centered = true;
        assert_eq!(focus_overlay(&button, Some(&font_set)).len(), 1, "un boton solo lleva anillo");
    }

    fn gradient(value: &str) -> Gradient {
        parse_css_linear_gradient(value).unwrap_or_else(|| panic!("no se interpreto: {value}"))
    }

    /// Tarea 2.7 (caso 7 del fixture): lo que se entiende de un `linear-gradient`.
    #[test]
    fn linear_gradient_directions_stops_and_positions_are_parsed() {
        let g = gradient("linear-gradient(#000000, #ffffff)");
        assert_eq!(g.direction, GradientDirection::Degrees(180.0), "por defecto, hacia abajo");
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0], GradientStop { color: [0, 0, 0, 255], position: StopPosition::Fraction(0.0) });
        assert_eq!(g.stops[1], GradientStop { color: [255, 255, 255, 255], position: StopPosition::Fraction(1.0) });

        assert_eq!(gradient("linear-gradient(to right, red, blue)").direction, GradientDirection::Degrees(90.0));
        assert_eq!(gradient("linear-gradient(to top, red, blue)").direction, GradientDirection::Degrees(0.0));
        assert_eq!(gradient("linear-gradient(to bottom right, red, blue)").direction, GradientDirection::Corner { sx: 1.0, sy: 1.0 });
        assert_eq!(gradient("linear-gradient(45deg, red, blue)").direction, GradientDirection::Degrees(45.0));
        assert_eq!(gradient("linear-gradient(0.25turn, red, blue)").direction, GradientDirection::Degrees(90.0));
    }

    #[test]
    fn linear_gradient_stop_positions_are_distributed_and_kept_in_order() {
        // Sin posiciones: reparto por igual.
        let g = gradient("linear-gradient(#000, #888, #fff)");
        let fractions: Vec<f32> = g.stops.iter().map(|s| match s.position { StopPosition::Fraction(f) => f, _ => panic!() }).collect();
        assert_eq!(fractions, vec![0.0, 0.5, 1.0]);

        // Con posiciones explicitas, colores con funcion y pixeles.
        let g = gradient("linear-gradient(to right, rgba(0, 0, 0, 0.5) 20%, #fff 80%)");
        assert_eq!(g.stops[0].color, [0, 0, 0, 128]);
        assert_eq!(g.stops[0].position, StopPosition::Fraction(0.2));
        assert_eq!(g.stops[1].position, StopPosition::Fraction(0.8));
        assert_eq!(gradient("linear-gradient(red 10px, blue)").stops[0].position, StopPosition::Pixels(10.0));

        // Una parada no puede quedar antes que la anterior.
        let g = gradient("linear-gradient(red 60%, blue 20%)");
        assert_eq!(g.stops[1].position, StopPosition::Fraction(0.6));
        // `red 10% 30%` son dos paradas.
        assert_eq!(gradient("linear-gradient(red 10% 30%, blue)").stops.len(), 3);
    }

    #[test]
    fn linear_gradient_that_cannot_be_drawn_is_none_rather_than_half_drawn() {
        for malo in ["linear-gradient(red)", "linear-gradient(to sideways, red, blue)", "linear-gradient(red, currentColor)", "radial-gradient(red, blue)", "url(x.png)", "linear-gradient(#000, #fff"] {
            assert!(parse_css_linear_gradient(malo).is_none(), "{malo}");
        }
    }

    /// El fondo se emite entre el color de fondo y el borde, y solo si hay degradado.
    #[test]
    fn a_box_with_a_gradient_emits_it_between_its_background_color_and_its_border() {
        let mut caja = LayoutBox::new(BoxType::Block);
        caja.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 40.0 };
        caja.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
        caja.computed_style.insert("background-image".to_string(), "linear-gradient(#000, #fff)".to_string());
        caja.computed_style.insert("border".to_string(), "1px solid #00ff00".to_string());
        let list = DisplayList::build(&caja, &ImageMap::new());
        let kinds: Vec<&str> = list.items.iter().map(|i| match i {
            DisplayItem::SolidRect { .. } => "color",
            DisplayItem::Gradient { .. } => "degradado",
            DisplayItem::Border { .. } => "borde",
            _ => "otro",
        }).filter(|k| *k != "otro").collect();
        assert_eq!(kinds, vec!["color", "degradado", "borde"]);
    }

    /// Tarea 2.7: el item de texto lleva la familia de letra que pidio la
    /// pagina, para pintar con la misma cara con la que el layout midio.
    #[test]
    fn a_text_item_carries_the_font_family_of_its_box() {
        let family_of = |font_family: Option<&str>| {
            let mut text = LayoutBox::new(BoxType::Text("hola".to_string()));
            text.dimensions = Rect { x: 0.0, y: 0.0, width: 40.0, height: 20.0 };
            if let Some(value) = font_family {
                text.computed_style.insert("font-family".to_string(), value.to_string());
            }
            let list = DisplayList::build(&text, &ImageMap::new());
            list.items
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Text { font_family, .. } => Some(*font_family),
                    _ => None,
                })
                .expect("un item de texto")
        };
        assert_eq!(family_of(Some("Georgia, 'Times New Roman', serif")), FontFamily::Serif);
        assert_eq!(family_of(Some("ui-monospace, Menlo, monospace")), FontFamily::Monospace);
        assert_eq!(family_of(Some("Arial, sans-serif")), FontFamily::SansSerif);
        assert_eq!(family_of(None), FontFamily::SansSerif, "sin font-family, la de siempre");
    }

    /// Fase 40, el punto real de `background-image`: una caja con la
    /// propiedad declarada Y la imagen ya presente en el `ImageMap` (mismo
    /// mapa que sirve `<img src>`) deberia emitir `BackgroundImage`, en
    /// tamaño natural (no escalado como `Image`) y ANTES de `Border` -
    /// mismo orden real del spec que ya usa `background-color`.
    #[test]
    fn a_box_with_a_resolved_background_image_emits_a_background_image_item() {
        let mut input = LayoutBox::new(BoxType::Block);
        input.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        input.computed_style.insert("background-image".to_string(), "url(tile.png)".to_string());
        input.computed_style.insert("border".to_string(), "2px solid black".to_string());

        let mut images = ImageMap::new();
        images.insert("tile.png".to_string(), Arc::new(DecodedImage { width: 4, height: 4, rgba: vec![255; 4 * 4 * 4] }));

        let list = DisplayList::build(&input, &images);

        let bg_index = list.items.iter().position(|item| matches!(item, DisplayItem::BackgroundImage { rect, .. } if rect.width == 100.0));
        let border_index = list.items.iter().position(|item| matches!(item, DisplayItem::Border { .. }));
        assert!(bg_index.is_some(), "deberia emitir BackgroundImage cuando la imagen esta resuelta");
        assert!(bg_index.unwrap() < border_index.unwrap(), "el fondo se pinta ANTES que el border, igual que background-color");
    }

    /// Sin la imagen resuelta en el `ImageMap` (fallo de descarga/
    /// decodificacion, `src` invalido) no deberia inventarse ningun
    /// rectangulo - mismo criterio que `BoxType::Image` ya aplica para
    /// `<img>`.
    #[test]
    fn a_background_image_url_missing_from_the_map_paints_nothing() {
        let mut input = LayoutBox::new(BoxType::Block);
        input.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        input.computed_style.insert("background-image".to_string(), "url(no-existe.png)".to_string());

        let list = DisplayList::build(&input, &ImageMap::new());

        assert!(!list.items.iter().any(|item| matches!(item, DisplayItem::BackgroundImage { .. })));
    }

    /// Un placeholder se pinta en gris (`PLACEHOLDER_COLOR`), NO con el
    /// `color` normal de la cascada - para que se distinga de un valor real
    /// ya escrito, igual que en un navegador real.
    #[test]
    fn a_placeholder_paints_in_the_dim_placeholder_color_not_the_cascades_own_color() {
        let mut input = LayoutBox::new(BoxType::Replaced);
        input.dimensions = Rect { x: 0.0, y: 0.0, width: 170.0, height: 21.0 };
        input.computed_style.insert("color".to_string(), "#ff0000".to_string());
        input.replaced_text = Some(engine_layout::ReplacedText { text: "Buscar...".to_string(), is_placeholder: true, centered: false });

        let list = DisplayList::build(&input, &ImageMap::new());

        let text_item = list.items.iter().find(|item| matches!(item, DisplayItem::Text { .. })).expect("deberia emitir texto");
        assert!(matches!(text_item, DisplayItem::Text { color, .. } if *color == PLACEHOLDER_COLOR));
    }

    /// El bug real encontrado en vivo contra la pantalla de consentimiento
    /// de google.com: los hijos DOM de un `BoxType::Replaced` (las
    /// `<option>` de un `<select>`, el texto de un `<textarea>`) nunca se
    /// posicionan (se quedan en `Rect::default()`), asi que si se dejan
    /// recursar se pintan superpuestos en (0,0) - texto ilegible en la
    /// esquina superior izquierda de la pagina entera. Un `Replaced` no
    /// deberia pintar NADA de sus hijos DOM reales.
    #[test]
    fn a_replaced_box_never_paints_its_unlaidout_dom_children() {
        let mut ghost_child = LayoutBox::new(BoxType::Text("opcion fantasma".to_string()));
        ghost_child.dimensions = Rect::default(); // nunca posicionado, como en la realidad

        let mut select = LayoutBox::new(BoxType::Replaced);
        select.dimensions = Rect { x: 5.0, y: 5.0, width: 170.0, height: 21.0 };
        select.children.push(ghost_child);

        let list = DisplayList::build(&select, &ImageMap::new());

        assert!(
            !list.items.iter().any(|item| matches!(item, DisplayItem::Text { text, .. } if text == "opcion fantasma")),
            "el hijo DOM sin posicionar nunca deberia pintarse, aunque siga en el arbol"
        );
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

    /// `overflow` recorta con cualquiera de sus valores de recorte, no solo
    /// con el literal `hidden`: `auto` y `scroll` son igual de comunes, y la
    /// forma de dos valores y las variantes por eje tambien. Comparando solo
    /// contra `"hidden"`, todo eso se derramaba por encima del resto de la
    /// pagina.
    #[test]
    fn overflow_recorta_con_auto_scroll_y_las_formas_por_eje() {
        let con = |prop: &str, valor: &str| {
            let mut m = HashMap::new();
            m.insert(prop.to_string(), valor.to_string());
            m
        };
        assert!(establishes_clip(&con("overflow", "hidden")));
        assert!(establishes_clip(&con("overflow", "auto")), "`auto` recorta: este motor no puede desplazar el interior de una caja");
        assert!(establishes_clip(&con("overflow", "scroll")));
        assert!(establishes_clip(&con("overflow", "hidden auto")), "la forma de dos ejes recorta si cualquiera de los dos lo hace");
        assert!(establishes_clip(&con("overflow-y", "auto")));
        assert!(!establishes_clip(&con("overflow", "visible")), "`visible` es el valor inicial: no recorta nada");
        assert!(!establishes_clip(&HashMap::new()), "sin `overflow` declarado tampoco");
    }

    /// El fondo de un ANCESTRO va siempre por debajo del contenido de sus
    /// descendientes, aunque los dos creen capa propia con el MISMO
    /// z-index.
    ///
    /// Se aplanan las capas en una sola lista, y el ancestro terminaba de
    /// recursar DESPUES de sus hijos, asi que acababa el ultimo de la lista
    /// y se pintaba encima de su propio contenido. En la Wikipedia real eso
    /// tapaba el articulo entero con un rectangulo blanco de pagina
    /// completa.
    #[test]
    fn el_fondo_de_un_ancestro_se_pinta_antes_que_el_contenido_de_sus_hijos() {
        let mut hijo = LayoutBox::new(BoxType::Block);
        hijo.computed_style.insert("position".to_string(), "relative".to_string());
        hijo.computed_style.insert("z-index".to_string(), "0".to_string());
        hijo.computed_style.insert("background-color".to_string(), "#00ff00".to_string());
        hijo.dimensions = Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };

        let mut ancestro = LayoutBox::new(BoxType::Block);
        ancestro.computed_style.insert("position".to_string(), "relative".to_string());
        ancestro.computed_style.insert("z-index".to_string(), "0".to_string());
        ancestro.computed_style.insert("background-color".to_string(), "#ffffff".to_string());
        ancestro.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        ancestro.children.push(hijo);

        let mut root = LayoutBox::new(BoxType::Block);
        root.children.push(ancestro);

        let list = DisplayList::build(&root, &ImageMap::new());
        let blanco = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [255, 255, 255, 255]))
            .expect("deberia existir el fondo blanco del ancestro");
        let verde = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [0, 255, 0, 255]))
            .expect("deberia existir el fondo verde del hijo");

        assert!(
            blanco < verde,
            "el fondo del ancestro (indice {blanco}) deberia pintarse ANTES que el del hijo (indice {verde}), no taparlo"
        );
    }

    /// Un descendiente `position: relative` con `z-index` numerico se pinta
    /// en una capa aparte, spliceada al FINAL de la lista entera - fuera de
    /// cualquier `PushClip`/`PopClip` que un ANCESTRO con `overflow`
    /// hubiera emitido en el camino normal. Sin envolver esa capa con los
    /// mismos recortes que heredaria de no haberse desviado, un
    /// `overflow: hidden` con un hijo asi por dentro no lo recorta.
    #[test]
    fn una_capa_z_index_hereda_el_recorte_de_su_ancestro_con_overflow() {
        let mut hijo_posicionado = LayoutBox::new(BoxType::Block);
        hijo_posicionado.computed_style.insert("position".to_string(), "relative".to_string());
        hijo_posicionado.computed_style.insert("z-index".to_string(), "1".to_string());
        hijo_posicionado.computed_style.insert("background-color".to_string(), "#ff0000".to_string());
        // Deliberadamente mas grande que el contenedor, para que un recorte
        // ausente sea observable con solo mirar el rectangulo de PushClip.
        hijo_posicionado.dimensions = Rect { x: 0.0, y: 0.0, width: 500.0, height: 500.0 };

        let mut contenedor = LayoutBox::new(BoxType::Block);
        contenedor.computed_style.insert("overflow".to_string(), "hidden".to_string());
        contenedor.dimensions = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        contenedor.children.push(hijo_posicionado);

        let mut root = LayoutBox::new(BoxType::Block);
        root.children.push(contenedor);

        let list = DisplayList::build(&root, &ImageMap::new());
        let rojo = list
            .items
            .iter()
            .position(|item| matches!(item, DisplayItem::SolidRect { color, .. } if *color == [255, 0, 0, 255]))
            .expect("deberia existir el fondo rojo del hijo posicionado");

        let recorte_antes = list.items[..rojo].iter().rev().find_map(|item| match item {
            DisplayItem::PushClip { rect } => Some(rect.clone()),
            _ => None,
        });
        let recorte = recorte_antes.expect("deberia haber un PushClip inmediatamente antes del contenido de la capa z-index");
        assert_eq!(
            (recorte.width, recorte.height),
            (100.0, 100.0),
            "el recorte deberia ser el rectangulo del contenedor con overflow:hidden (100x100), no quedarse sin recortar"
        );
        assert!(
            list.items[rojo..].iter().any(|item| matches!(item, DisplayItem::PopClip)),
            "el PushClip de la capa deberia cerrarse con su PopClip"
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
    fn resolve_text_decoration_is_underline_recognizes_the_token_alone_or_in_the_compound_shorthand() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("text-decoration".to_string(), value.to_string()); m };

        assert!(resolve_text_decoration_is_underline(&style("underline")));
        assert!(resolve_text_decoration_is_underline(&style("underline dotted red")), "el shorthand compuesto (estilo/color) tambien deberia reconocerse");
        assert!(resolve_text_decoration_is_underline(&style("UNDERLINE")), "sin distinguir mayusculas/minusculas, igual que el resto de palabras clave CSS");
        assert!(!resolve_text_decoration_is_underline(&style("none")));
        assert!(!resolve_text_decoration_is_underline(&style("line-through")), "line-through no deberia activar el subrayado");
        assert!(!resolve_text_decoration_is_underline(&HashMap::new()), "sin la propiedad puesta, ningun subrayado");
    }

    #[test]
    fn resolve_text_align_recognizes_center_right_end_and_defaults_to_left() {
        let style = |value: &str| { let mut m = HashMap::new(); m.insert("text-align".to_string(), value.to_string()); m };

        assert_eq!(resolve_text_align(&style("center")), TextAlign::Center);
        assert_eq!(resolve_text_align(&style("right")), TextAlign::Right);
        assert_eq!(resolve_text_align(&style("end")), TextAlign::Right, "el valor logico 'end' deberia comportarse como 'right' en LTR");
        assert_eq!(resolve_text_align(&style("left")), TextAlign::Left);
        assert_eq!(resolve_text_align(&style("justify")), TextAlign::Left, "justify se parsea pero se pinta como left, declarado");
        assert_eq!(resolve_text_align(&HashMap::new()), TextAlign::Left, "el valor inicial real de text-align es left");
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
    fn parse_css_background_image_strips_the_url_wrapper_and_quotes() {
        assert_eq!(parse_css_background_image(&style_with("background-image", "url(x.png)")), Some("x.png".to_string()));
        assert_eq!(parse_css_background_image(&style_with("background-image", "url(\"x.png\")")), Some("x.png".to_string()));
        assert_eq!(parse_css_background_image(&style_with("background-image", "url('x.png')")), Some("x.png".to_string()));
    }

    #[test]
    fn parse_css_background_image_is_none_without_a_url_function() {
        assert_eq!(parse_css_background_image(&style_with("background-image", "none")), None);
        assert_eq!(parse_css_background_image(&style_with("background-image", "linear-gradient(red, blue)")), None, "solo url() esta soportado, no funciones de degradado");
        assert_eq!(parse_css_background_image(&HashMap::new()), None);
    }

    #[test]
    fn parse_css_border_radius_reads_a_positive_px_value() {
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "8px")), Some(8.0));
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "8px 12px")), Some(8.0));
    }

    #[test]
    fn parse_css_border_radius_rejects_zero_negative_and_missing() {
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "0px")), None);
        assert_eq!(parse_css_border_radius(&style_with("border-radius", "-3px")), None);
        assert_eq!(parse_css_border_radius(&HashMap::new()), None);
    }

    #[test]
    fn parse_css_box_shadow_reads_offsets_and_color_in_any_order() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "4px 6px #ff0000")), Some((4.0, 6.0, [255, 0, 0, 255], 0.0)));
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "#ff0000 4px 6px")), Some((4.0, 6.0, [255, 0, 0, 255], 0.0)), "el orden deberia ser libre, igual que border");
    }

    #[test]
    fn parse_css_box_shadow_reads_the_optional_blur_radius_token() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "4px 6px 10px #ff0000")), Some((4.0, 6.0, [255, 0, 0, 255], 10.0)));
    }

    #[test]
    fn parse_css_box_shadow_accepts_negative_offsets() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "-4px -6px #000000")), Some((-4.0, -6.0, [0, 0, 0, 255], 0.0)));
    }

    #[test]
    fn parse_css_box_shadow_defaults_color_to_black_when_missing() {
        assert_eq!(parse_css_box_shadow(&style_with("box-shadow", "4px 6px")), Some((4.0, 6.0, [0, 0, 0, 255], 0.0)));
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
