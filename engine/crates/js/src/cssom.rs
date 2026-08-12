//! CSSOM minimo (Fase 8): `getComputedStyle(el)` y `el.getBoundingClientRect()`.
//!
//! # El problema que resuelve este modulo, y por que es un SNAPSHOT
//!
//! Las dos APIs de esta fase no leen el DOM: leen el ARBOL DE LAYOUT (el
//! estilo YA resuelto por la cascada, y la geometria YA calculada). Y el
//! runtime JS de una pagina se construye ANTES de que ese arbol exista -
//! `core::pipeline::build_page_keeping_runtime` ejecuta los `<script>`
//! primero y llama a `LayoutTreeBuilder::build` despues, igual que un
//! navegador real, donde los scripts corren durante el parseo.
//!
//! Un navegador real resuelve esa inversion forzando un *reflow sincrono*:
//! `getBoundingClientRect()` PARA el mundo, rehace el layout ahi mismo y
//! devuelve el resultado fresco. Este motor no puede hacer eso: construir
//! un layout necesita la hoja de estilos, el `FontSet` y el mapa de
//! imagenes, que viven una capa por encima del `Context` de Boa (en
//! `core::server`), inalcanzables desde dentro de un closure nativo.
//!
//! Asi que el puente es el mismo patron que ya usan `window.open` (Fase
//! 6.4) e `history.pushState` (Fase 7), pero en direccion contraria:
//! aquellos son colas donde JS ESCRIBE y `core::server` drena; este es un
//! buzon donde `core::server` PUBLICA (tras cada layout) y JS lee. Ver
//! `LayoutSnapshot`.
//!
//! # Consecuencias honestas de que sea un snapshot y no un reflow
//!
//! - **Durante la carga de la pagina el snapshot esta VACIO**: todavia no
//!   ha corrido ningun layout. Un `<script>` que llame a
//!   `getBoundingClientRect()` en ese momento recibe un rect de ceros, y
//!   `getComputedStyle(el)` un objeto sin ninguna propiedad. No es un
//!   invento: es exactamente lo que devuelve un navegador real para un
//!   elemento que no esta en el arbol de render (`display: none`, o
//!   desconectado del documento). Donde estas APIs se usan de verdad -
//!   dentro de un listener de `click`/`input`/`popstate` - el snapshot ya
//!   esta publicado y los valores son reales.
//! - **Mutar el DOM no actualiza el snapshot al instante.** Si un listener
//!   cambia `el.style.width` y acto seguido lee `getBoundingClientRect()`,
//!   ve la geometria de ANTES del cambio; el navegador real veria la de
//!   despues (por el reflow sincrono). El siguiente layout - que
//!   `core::server` hace al terminar de procesar ese mismo clic - lo pone
//!   al dia. Es la limitacion real de esta fase y no se disimula.
//!
//! # Por que datos planos y no el `LayoutBox` de verdad
//!
//! `engine-js` NO depende de `engine-layout` (ver "Doctrina de
//! dependencias" en ARCHITECTURE.md), y no deberia: el motor JS no tiene
//! por que saber que es una caja de bloque. Lo que cruza la frontera son
//! numeros y pares clave/valor ya resueltos (`BoxMetrics`), copiados por
//! quien SI conoce las dos capas (`core::server`). El coste es una copia
//! por layout, del orden de un `HashMap` pequeño por caja, despreciable al
//! lado del propio layout (que hace shaping de texto real).

use boa_engine::object::builtins::JsArray;
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsObject, JsResult, JsValue, NativeFunction};
use engine_dom::Node;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Geometria + estilo resuelto de UNA caja de layout, ya copiados fuera del
/// arbol de layout (ver el aviso del modulo sobre por que se copia).
///
/// `x`/`y`/`width`/`height` son la CAJA DE BORDE en coordenadas de
/// DOCUMENTO - que es justo lo que `LayoutBox::dimensions` contiene (su
/// `box_dimensions.border_box()` la reconstruye identica, ver
/// `layout::tree`), y justo lo que un `DOMRect` real describe. La
/// conversion a coordenadas de VIEWPORT (restar el scroll) la hace
/// `bounding_client_rect_to_js_object`, no esto.
#[derive(Debug, Clone, Default)]
pub struct BoxMetrics {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Las declaraciones que la cascada resolvio de verdad para esta caja -
    /// NO las ~340 propiedades con valor inicial que un
    /// `getComputedStyle` real expone. Ver `computed_style_to_js_object`.
    pub computed_style: HashMap<String, String>,
}

/// El buzon compartido entre el `Context` de Boa (que solo lee) y
/// `core::server` (que solo publica). `Arc<RwLock<...>>` y no un simple
/// campo porque los dos lados viven en capas distintas y ninguno es dueño
/// del otro - mismo motivo que `PendingWindowOpens`/`PendingHistoryOps`,
/// aunque el sentido del flujo sea el contrario.
pub type LayoutSnapshot = Arc<RwLock<LayoutSnapshotData>>;

#[derive(Debug, Default)]
pub struct LayoutSnapshotData {
    /// Una entrada por caja de layout CON nodo del DOM detras, en orden de
    /// documento. Las cajas de texto y la caja raiz sintetica no aparecen
    /// (no tienen `dom_node`), igual que no aparecen en `hit_test`.
    ///
    /// Lista y no `HashMap` porque la clave seria la IDENTIDAD del `Arc`
    /// (`Arc::ptr_eq`), no su contenido: dos `<li>` vacios son iguales
    /// campo a campo y aun asi son cajas distintas. Un `HashMap` por
    /// direccion del puntero funcionaria pero ata la correccion a que
    /// ningun `Arc` se libere y su direccion se reutilice; recorrer la
    /// lista es O(n) en el numero de cajas, el mismo orden que
    /// `LayoutBox::find_box_for_node` ya paga al recorrer el arbol.
    pub boxes: Vec<(Arc<RwLock<Node>>, BoxMetrics)>,
    /// Desplazamiento vertical actual de la pestaña. Se guarda aparte de
    /// las cajas a proposito: hacer scroll NO cambia la geometria de
    /// nadie, solo la relacion entre documento y viewport, asi que
    /// `core::server` puede actualizar solo este campo sin volver a
    /// recorrer el arbol entero.
    pub scroll_offset_y: f32,
}

impl LayoutSnapshotData {
    /// La caja de `node`, o `None` si ese nodo no produjo ninguna (todavia
    /// no ha corrido ningun layout, el elemento esta desconectado del
    /// documento, o su `display` no genera caja).
    pub fn metrics_for(&self, node: &Arc<RwLock<Node>>) -> Option<&BoxMetrics> {
        self.boxes.iter().find(|(candidate, _)| Arc::ptr_eq(candidate, node)).map(|(_, metrics)| metrics)
    }
}

pub fn new_layout_snapshot() -> LayoutSnapshot {
    Arc::new(RwLock::new(LayoutSnapshotData::default()))
}

/// Envoltorio para capturar el estilo resuelto dentro de un
/// `NativeFunction` de Boa - mismo motivo y mismo patron que
/// `window::PendingCapture`: `from_copy_closure_with_captures` exige
/// `boa_gc::Trace`, y aqui no hay nada que el recolector de Boa tenga que
/// rastrear (son `String`s propios del motor), que es exactamente lo que
/// `empty_trace!` declara.
#[derive(Clone)]
struct ComputedStyleCapture {
    declarations: Arc<HashMap<String, String>>,
    /// Los mismos nombres, ordenados - `item(i)` necesita un orden estable
    /// y el de un `HashMap` no lo es.
    names: Arc<Vec<String>>,
}

unsafe impl boa_gc::Trace for ComputedStyleCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for ComputedStyleCapture {}

/// Construye el objeto que devuelve `getComputedStyle(el)`.
///
/// **De solo lectura, a proposito.** El `CSSStyleDeclaration` que devuelve
/// un `getComputedStyle` real tambien lo es (asignarle una propiedad lanza
/// `NoModificationAllowedError`), y ademas aqui escribir no podria
/// funcionar: esto es una copia de lo que la cascada resolvio, no la
/// fuente. Para escribir estilo esta `el.style`, que SI es vivo (ver
/// `dom_bindings::style_to_js_object`).
///
/// **Solo lleva lo que la cascada resolvio de verdad.** Un
/// `getComputedStyle` real expone TODAS las propiedades CSS, con su valor
/// inicial cuando nadie las toco (`getComputedStyle(div).color` da
/// `"rgb(0, 0, 0)"` aunque nadie haya puesto `color`). Este motor solo
/// guarda en `computed_style` lo que alguna regla - de autor, en linea, o
/// de la hoja de usuario-agente - puso de verdad, mas lo heredado
/// (`INHERITABLE_PROPERTIES` en `layout::tree`). Una propiedad que nadie
/// definio devuelve `""`, no su valor inicial. Fingir lo contrario exigiria
/// una tabla completa de valores iniciales del spec que este motor no
/// tiene; devolver `""` dice la verdad ("aqui no se resolvio nada").
///
/// **Valores especificados, no usados.** Se devuelve la cadena tal como la
/// resolvio la cascada (`"2em"`, `"50%"`, `"red"`), no el valor usado en
/// pixeles/`rgb()` que da un navegador real. La conversion existe dentro
/// del layout (`parse_css_length`, `resolve_font_size`...) pero es por
/// propiedad y no esta centralizada; exponer la cadena original es
/// exacto en lugar de aproximado.
pub fn computed_style_to_js_object(declarations: &HashMap<String, String>, context: &mut Context) -> JsObject {
    let mut names: Vec<String> = declarations.keys().cloned().collect();
    names.sort();
    let capture = ComputedStyleCapture { declarations: Arc::new(declarations.clone()), names: Arc::new(names.clone()) };

    let get_property_value = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &ComputedStyleCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::from(js_string!(""))) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            let value = captured.declarations.get(name.trim()).cloned().unwrap_or_default();
            Ok(JsValue::from(js_string!(value)))
        },
        capture.clone(),
    );

    // `item(i)` del spec: el NOMBRE de la propiedad i-esima, "" si el
    // indice se sale. Junto a `length` permite recorrer todo lo que el
    // motor resolvio de verdad, que es la unica forma honesta de
    // enumerarlo (un `for...in` sobre el objeto veria tambien los
    // metodos).
    let item = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &ComputedStyleCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::from(js_string!(""))) };
            let index = arg.to_number(context)?;
            if index < 0.0 || !index.is_finite() {
                return Ok(JsValue::from(js_string!("")));
            }
            let name = captured.names.get(index as usize).cloned().unwrap_or_default();
            Ok(JsValue::from(js_string!(name)))
        },
        capture.clone(),
    );

    let mut initializer = ObjectInitializer::new(context);
    initializer
        .function(get_property_value, js_string!("getPropertyValue"), 1)
        .function(item, js_string!("item"), 1)
        .property(js_string!("length"), names.len() as i32, Attribute::all());

    // Cada propiedad resuelta se expone por sus DOS nombres, igual que un
    // `CSSStyleDeclaration` real: el de CSS (`background-color`, util para
    // `style['background-color']`) y el camelCase de IDL
    // (`backgroundColor`, la forma que usa casi todo el codigo real). Son
    // valores fijos, no accessors, porque el objeto entero es una foto: no
    // hay nada vivo detras que un getter pudiera releer.
    for name in &names {
        let value = declarations.get(name).cloned().unwrap_or_default();
        initializer.property(js_string!(name.clone()), js_string!(value.clone()), Attribute::all());
        let camel = kebab_to_camel(name);
        if &camel != name {
            initializer.property(js_string!(camel), js_string!(value), Attribute::all());
        }
    }

    initializer.build()
}

/// `background-color` -> `backgroundColor`. Un guion seguido de nada (o al
/// final) simplemente desaparece, igual que hace la conversion de IDL.
fn kebab_to_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for ch in name.chars() {
        if ch == '-' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Construye el `DOMRect` que devuelve `el.getBoundingClientRect()`.
///
/// `metrics` en coordenadas de DOCUMENTO; el resultado en coordenadas de
/// VIEWPORT (`y - scroll_offset_y`), que es lo que define el spec
/// ("relative to the viewport"). No hay scroll horizontal en este motor,
/// asi que `x` pasa tal cual.
///
/// `metrics = None` produce un rect de CEROS (no `null`, no un error):
/// exactamente lo que devuelve un navegador real para un elemento que no
/// genera caja - desconectado del documento, `display: none`, o - propio de
/// este motor - antes de que haya corrido el primer layout (ver el aviso
/// del modulo).
pub fn bounding_client_rect_to_js_object(metrics: Option<&BoxMetrics>, scroll_offset_y: f32, context: &mut Context) -> JsObject {
    let (x, y, width, height) = match metrics {
        Some(m) => (m.x, m.y - scroll_offset_y, m.width, m.height),
        None => (0.0, 0.0, 0.0, 0.0),
    };

    // Un `DOMRect` real deriva `top`/`right`/`bottom`/`left` de
    // `x`/`y`/`width`/`height`, y con dimensiones negativas los normaliza
    // (`top` es el borde MENOR, no `y` a secas). Este motor nunca produce
    // anchos/altos negativos, pero calcularlo con `min`/`max` cuesta lo
    // mismo y evita que la relacion sea falsa si algun dia los produce.
    let top = y.min(y + height);
    let bottom = y.max(y + height);
    let left = x.min(x + width);
    let right = x.max(x + width);

    ObjectInitializer::new(context)
        .property(js_string!("x"), x, Attribute::all())
        .property(js_string!("y"), y, Attribute::all())
        .property(js_string!("width"), width, Attribute::all())
        .property(js_string!("height"), height, Attribute::all())
        .property(js_string!("top"), top, Attribute::all())
        .property(js_string!("right"), right, Attribute::all())
        .property(js_string!("bottom"), bottom, Attribute::all())
        .property(js_string!("left"), left, Attribute::all())
        .build()
}

/// `element.getClientRects()` del spec: la lista de rectangulos que ocupa
/// el elemento. Un elemento de bloque siempre produce exactamente uno (el
/// mismo que `getBoundingClientRect`); solo un elemento INLINE partido en
/// varias lineas produce mas de uno, y este motor no expone esa
/// fragmentacion todavia (`LayoutBox::find_box_for_node` devuelve una sola
/// caja por nodo, ver su doc-comment). Asi que aqui es siempre una lista
/// de uno - o vacia si el elemento no genera caja, tambien como el spec.
pub fn client_rects_to_js_array(metrics: Option<&BoxMetrics>, scroll_offset_y: f32, context: &mut Context) -> JsObject {
    let rects: Vec<JsValue> = match metrics {
        Some(m) => vec![bounding_client_rect_to_js_object(Some(m), scroll_offset_y, context).into()],
        None => Vec::new(),
    };
    JsArray::from_iter(rects, context).into()
}

/// Registra el global `getComputedStyle(el)`. Devuelve el buzon compartido
/// para que `DomBindings::register` lo guarde y lo pueda pasar tambien a
/// los objetos de elemento (que lo necesitan para
/// `getBoundingClientRect`).
///
/// `node_of` es como se recupera el `Arc<RwLock<Node>>` real del argumento
/// - se pasa como funcion en vez de hacerlo aqui porque el tipo que lleva
/// esos datos nativos (`ElementCapture`) es privado de `dom_bindings`.
pub fn register_computed_style(
    context: &mut Context,
    snapshot: LayoutSnapshot,
    node_of: fn(&JsValue) -> Option<Arc<RwLock<Node>>>,
) -> JsResult<()> {
    let capture = SnapshotCapture { snapshot, node_of };

    let get_computed_style = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &SnapshotCapture, context| {
            // Sin argumento, o con algo que no es un elemento nuestro: un
            // objeto vacio. El spec real lanza `TypeError`, pero eso
            // exigiria fabricar el error nativo y el resultado practico
            // aqui es el mismo (todas las propiedades dan ""), sin
            // arriesgar a tumbar una pagina por una llamada rara.
            let Some(node) = args.first().and_then(captured.node_of) else {
                return Ok(computed_style_to_js_object(&HashMap::new(), context).into());
            };
            let declarations = match captured.snapshot.read() {
                Ok(data) => data.metrics_for(&node).map(|m| m.computed_style.clone()).unwrap_or_default(),
                Err(_) => HashMap::new(),
            };
            Ok(computed_style_to_js_object(&declarations, context).into())
        },
        capture,
    );

    // 2 argumentos declarados como el spec (`element`, `pseudoElt`) aunque
    // el segundo se ignore: no hay pseudo-elementos en este motor, y
    // `getComputedStyle(el, '::before')` devolveria el estilo del elemento
    // real - una mentira que se declara aqui en vez de disimularse.
    context.register_global_builtin_callable(js_string!("getComputedStyle"), 2, get_computed_style)?;
    Ok(())
}

/// Igual que `ComputedStyleCapture` pero para el buzon entero - lo capturan
/// las funciones que tienen que consultarlo en el momento de la llamada
/// (`getComputedStyle`, `getBoundingClientRect`), no las que ya recibieron
/// una foto.
#[derive(Clone)]
pub(crate) struct SnapshotCapture {
    pub(crate) snapshot: LayoutSnapshot,
    pub(crate) node_of: fn(&JsValue) -> Option<Arc<RwLock<Node>>>,
}

unsafe impl boa_gc::Trace for SnapshotCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for SnapshotCapture {}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Source;

    fn eval(context: &mut Context, code: &str) -> String {
        context.eval(Source::from_bytes(code.as_bytes())).expect("JS valido").display().to_string()
    }

    fn style(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn kebab_to_camel_converts_css_names_to_idl_names() {
        assert_eq!(kebab_to_camel("background-color"), "backgroundColor");
        assert_eq!(kebab_to_camel("color"), "color");
        assert_eq!(kebab_to_camel("border-top-left-radius"), "borderTopLeftRadius");
    }

    #[test]
    fn computed_style_exposes_values_by_css_name_and_by_idl_name() {
        let mut context = Context::default();
        let object = computed_style_to_js_object(&style(&[("background-color", "red"), ("color", "blue")]), &mut context);
        context.register_global_property(js_string!("cs"), object, Attribute::all()).unwrap();

        assert_eq!(eval(&mut context, "cs.getPropertyValue('background-color')"), "\"red\"");
        assert_eq!(eval(&mut context, "cs.backgroundColor"), "\"red\"");
        assert_eq!(eval(&mut context, "cs['background-color']"), "\"red\"");
        assert_eq!(eval(&mut context, "cs.color"), "\"blue\"");
    }

    /// La simplificacion declarada en el doc-comment, probada: una
    /// propiedad que nadie resolvio da "" y NO su valor inicial del spec.
    #[test]
    fn computed_style_returns_empty_string_for_a_property_nobody_resolved() {
        let mut context = Context::default();
        let object = computed_style_to_js_object(&style(&[("color", "blue")]), &mut context);
        context.register_global_property(js_string!("cs"), object, Attribute::all()).unwrap();

        assert_eq!(eval(&mut context, "cs.getPropertyValue('margin-top')"), "\"\"");
        assert_eq!(eval(&mut context, "typeof cs.marginTop"), "\"undefined\"");
    }

    #[test]
    fn computed_style_length_and_item_enumerate_what_was_resolved_in_stable_order() {
        let mut context = Context::default();
        let object = computed_style_to_js_object(&style(&[("color", "blue"), ("background-color", "red")]), &mut context);
        context.register_global_property(js_string!("cs"), object, Attribute::all()).unwrap();

        assert_eq!(eval(&mut context, "cs.length"), "2");
        assert_eq!(eval(&mut context, "cs.item(0)"), "\"background-color\"", "deberia estar ordenado alfabeticamente");
        assert_eq!(eval(&mut context, "cs.item(1)"), "\"color\"");
        assert_eq!(eval(&mut context, "cs.item(99)"), "\"\"", "un indice fuera de rango da \"\", no un error");
    }

    #[test]
    fn bounding_client_rect_derives_the_four_edges_from_position_and_size() {
        let mut context = Context::default();
        let metrics = BoxMetrics { x: 10.0, y: 20.0, width: 100.0, height: 50.0, computed_style: HashMap::new() };
        let object = bounding_client_rect_to_js_object(Some(&metrics), 0.0, &mut context);
        context.register_global_property(js_string!("r"), object, Attribute::all()).unwrap();

        assert_eq!(eval(&mut context, "r.left"), "10");
        assert_eq!(eval(&mut context, "r.top"), "20");
        assert_eq!(eval(&mut context, "r.right"), "110");
        assert_eq!(eval(&mut context, "r.bottom"), "70");
        assert_eq!(eval(&mut context, "r.width"), "100");
        assert_eq!(eval(&mut context, "r.height"), "50");
    }

    /// El punto de la fase: el rect es RELATIVO AL VIEWPORT, asi que el
    /// scroll tiene que restarse. Sin esto, un elemento al que el usuario
    /// ha bajado quedaria reportado como si siguiera fuera de pantalla.
    #[test]
    fn bounding_client_rect_is_viewport_relative_so_scroll_shifts_it_up() {
        let mut context = Context::default();
        let metrics = BoxMetrics { x: 0.0, y: 500.0, width: 100.0, height: 50.0, computed_style: HashMap::new() };
        let object = bounding_client_rect_to_js_object(Some(&metrics), 400.0, &mut context);
        context.register_global_property(js_string!("r"), object, Attribute::all()).unwrap();

        assert_eq!(eval(&mut context, "r.top"), "100", "500 en el documento con 400 de scroll son 100 en el viewport");
        assert_eq!(eval(&mut context, "r.bottom"), "150");
        assert_eq!(eval(&mut context, "r.x"), "0", "no hay scroll horizontal: x no cambia");
    }

    #[test]
    fn bounding_client_rect_of_an_element_with_no_box_is_all_zeros_like_a_real_browser() {
        let mut context = Context::default();
        let object = bounding_client_rect_to_js_object(None, 400.0, &mut context);
        context.register_global_property(js_string!("r"), object, Attribute::all()).unwrap();

        assert_eq!(eval(&mut context, "r.x + r.y + r.width + r.height + r.top + r.left"), "0");
    }

    #[test]
    fn client_rects_is_a_one_element_list_for_a_box_and_empty_without_one() {
        let mut context = Context::default();
        let metrics = BoxMetrics { x: 1.0, y: 2.0, width: 3.0, height: 4.0, computed_style: HashMap::new() };
        let with_box = client_rects_to_js_array(Some(&metrics), 0.0, &mut context);
        context.register_global_property(js_string!("a"), with_box, Attribute::all()).unwrap();
        assert_eq!(eval(&mut context, "a.length"), "1");
        assert_eq!(eval(&mut context, "a[0].width"), "3");

        let without_box = client_rects_to_js_array(None, 0.0, &mut context);
        context.register_global_property(js_string!("b"), without_box, Attribute::all()).unwrap();
        assert_eq!(eval(&mut context, "b.length"), "0");
    }

    #[test]
    fn metrics_for_matches_by_arc_identity_not_by_node_contents() {
        use engine_dom::NodeType;
        let one = Node::new(NodeType::Element { tag_name: "li".to_string(), attributes: HashMap::new() });
        // Campo a campo identico al anterior, pero es OTRO nodo.
        let two = Node::new(NodeType::Element { tag_name: "li".to_string(), attributes: HashMap::new() });

        let data = LayoutSnapshotData {
            boxes: vec![(one.clone(), BoxMetrics { x: 7.0, ..Default::default() })],
            scroll_offset_y: 0.0,
        };

        assert_eq!(data.metrics_for(&one).map(|m| m.x), Some(7.0));
        assert!(data.metrics_for(&two).is_none(), "un nodo distinto con el mismo contenido no deberia heredar la caja del otro");
    }
}
