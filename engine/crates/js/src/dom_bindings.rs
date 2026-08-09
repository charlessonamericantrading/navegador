//! Bindings DOM reales para el runtime JS (ver `runtime.rs`, usado desde
//! `core/src/scripting.rs`).
//!
//! `document.getElementById`/`document.querySelector`/`document.querySelectorAll`/
//! `document.documentElement`/`document.body` devuelven un objeto JS por
//! elemento encontrado, con partes vivas y partes foto (snapshot) - la
//! distincion importa, asi que va explicita aqui:
//!
//! - `getAttribute`/`setAttribute`, `textContent` (accessor, con getter Y
//!   setter reales), `appendChild` y `removeChild` son VIVOS: leen y
//!   escriben de verdad sobre el `Arc<RwLock<Node>>` del arbol real
//!   (`ElementCapture` mas abajo, tambien adjunta como datos nativos del
//!   objeto JS - ver su propio aviso). Mutar y leer despues - incluso desde
//!   un objeto JS DISTINTO obtenido con otra llamada a `getElementById`
//!   sobre el mismo id - ve el cambio, porque ambos apuntan al mismo nodo.
//!   Asignar `el.textContent = valor` reemplaza TODOS los hijos existentes
//!   por un unico nodo de texto nuevo (semantica real, no un append) -
//!   `null` se trata como cadena vacia (`[LegacyNullToEmptyString]` del
//!   spec real, no `ToString(null)` = "null"), cualquier otro valor se
//!   convierte a cadena con la conversion normal de JS.
//!   `document.createElement(tag)` crea un nodo nuevo y desconectado
//!   (mismo tipo de objeto JS que devuelven `getElementById`/
//!   `querySelector`); `padre.appendChild(hijo)`/`padre.removeChild(hijo)`
//!   lo conectan/desconectan de verdad del arbol - ambos usan el mismo
//!   mecanismo (`JsObject::downcast_ref::<ElementCapture>()`) para
//!   recuperar el nodo real de `hijo`, asi que si no es un objeto elemento
//!   nuestro (una cadena, un numero, un objeto JS cualquiera sin esos datos
//!   nativos...) no hacen nada, en vez de fingir que se añadio/quito algo -
//!   igual que `removeChild` sobre un nodo que NO es hijo de `padre`: el
//!   DOM real lanzaria `NotFoundError`, aqui es un no-op explicito que
//!   devuelve `null`. `padre.insertBefore(nuevo, referencia)` (inserta
//!   antes de `referencia`, o al final si `referencia` es `null`/ausente,
//!   igual que el spec real) y `padre.replaceChild(nuevo, viejo)`
//!   (sustituye en la misma posicion, devuelve `viejo`) completan las
//!   cuatro mutaciones fundamentales de `Node`; ambos VALIDAN la posicion
//!   ANTES de tocar nada (igual que el algoritmo real del spec), asi que
//!   una referencia/viejo que no es hijo real dejan todo intacto en vez de
//!   una mutacion a medias. Los tres (`appendChild`/`insertBefore`/
//!   `replaceChild`) desconectan primero al nodo nuevo de su padre
//!   ANTERIOR si ya tenia uno (`detach_from_parent`, mas abajo) - un nodo
//!   solo puede tener un padre a la vez en el DOM real, y antes de esto
//!   `appendChild` no lo hacia: mover un nodo ya conectado lo dejaba
//!   fantasma en la lista `children` de su padre viejo. `classList` (getter, `contains`/`add`/`remove`/
//!   `toggle`) lee y escribe de verdad el atributo `class`. `parentElement`
//!   (getter) sube por `Node::parent` (un `Weak`) y devuelve `null` si no
//!   hay padre O si el padre no es un `Element` (la raiz del documento es
//!   un `NodeType::Document`, no un elemento - igual que en el DOM real,
//!   `document.documentElement.parentElement === null`). `children`
//!   (getter) devuelve un `Array` real solo con los hijos `Element`
//!   (los nodos de texto no cuentan, igual que `ParentNode.children` real),
//!   reconstruido - por tanto vivo - en cada lectura, a diferencia de
//!   `querySelectorAll` (ver mas abajo, que congela la LISTA en el momento
//!   de la llamada): aqui cada `.children` vuelve a mirar el arbol real, asi
//!   que ve altas/bajas hechas despues de la ultima lectura.
//!   `firstElementChild`/`lastElementChild`/`nextElementSibling`/
//!   `previousElementSibling` completan la navegacion (real DOM spec,
//!   `ParentNode`/`ElementTraversal`), todos saltando nodos de texto -
//!   deliberadamente Element-only, a diferencia de `firstChild`/
//!   `nextSibling` de `Node` (que SI pueden devolver texto): esos
//!   exigirian envolver un nodo de texto como objeto JS, que este motor no
//!   hace todavia (solo los `Element` se envuelven). `style`
//!   (getter) devuelve un objeto con `getPropertyValue`/`setProperty`/
//!   `removeProperty` reales sobre el atributo `style` (parseado con
//!   `engine_css::CssParser::parse_inline_style`, el mismo tokenizador que
//!   usa una hoja de estilos normal - ver su aviso en `css/src/parser.rs`),
//!   que es exactamente lo que aplica la cascada real
//!   (`layout::resolve_style`) al calcular el layout - mutar `el.style` es
//!   mutar de verdad ese atributo, no una copia paralela. `getPropertyValue`
//!   devuelve `""` (cadena vacia) si la propiedad no esta puesta, NUNCA
//!   `null` - asi es el spec real, a diferencia de `getAttribute`, que si
//!   devuelve `null`. `setProperty(nombre, "")` QUITA la propiedad en vez
//!   de guardar un valor vacio (tambien el spec real). `removeProperty`
//!   devuelve el valor quitado (`""` si no existia). `cssText` (getter que
//!   serializa TODAS las declaraciones, setter que reemplaza el bloque
//!   entero via `CssParser::parse_inline_style`) y tres accessors por
//!   nombre camelCase (`color`, `backgroundColor`, `fontSize`) tambien son
//!   reales, sobre la MISMA fuente que `getPropertyValue`/`setProperty` -
//!   deliberadamente solo esas tres, no las ~cientos del spec real: son
//!   las UNICAS que `layout`/`gfx` leen de verdad hoy
//!   (`computed_style.get("color"/"background-color"/"font-size")` en
//!   `layout/src/tree.rs` y `gfx/src/display_list.rs` - verificado por
//!   grep, no asumido). Cualquier otro nombre camelCase (`el.style.margin`,
//!   `el.style.display`...) NO tiene accessor: se convierte en una
//!   propiedad JS normal, sin relacion con el estilo real - que es, de
//!   hecho, el mismo comportamiento que un navegador real para una
//!   propiedad camelCase no reconocida por `CSSStyleDeclaration`
//!   (`setProperty` en cambio SI sigue siendo generico para cualquier
//!   nombre, como el spec real).
//!   `addEventListener(tipo, listener)`/`removeEventListener(tipo,
//!   listener)`/`dispatchEvent(event)` son reales: un `EventRegistry`
//!   COMPARTIDO por todo el documento (no por elemento ni por objeto JS
//!   envoltorio - ver `EventRegistry` mas abajo) guarda los listeners
//!   indexados por el puntero del nodo real, asi que registrar desde una
//!   consulta y disparar desde OTRA consulta al mismo elemento se ven el
//!   uno al otro - probado explicitamente. `removeEventListener` compara
//!   por identidad real (`JsObject`/`Gc::ptr_eq`), no por contenido: dos
//!   funciones con el mismo codigo fuente declaradas por separado no
//!   matchean. `dispatchEvent` hace BUBBLING real
//!   (`dispatch_event_with_bubbling`, mas abajo): llama primero a los
//!   listeners del propio target, luego sube por sus ancestros llamando a
//!   los suyos, hasta la raiz del documento o hasta que algun listener
//!   llame a `event.stopPropagation()` (comprobado despues de cada nodo) -
//!   probado explicitamente, incluido que un listener en un ANCESTRO se
//!   entera de un evento disparado sobre un descendiente, y que
//!   `stopPropagation` corta la subida antes de tiempo. Dentro de cada
//!   listener, `this` es el elemento en el que ESE listener esta
//!   registrado (como `currentTarget` real - cambia por nivel), mientras
//!   que `event.target` es siempre el nodo ORIGINAL sobre el que se llamo
//!   `dispatchEvent`, fijo en todos los niveles - ambos probados por
//!   separado. `addEventListener(tipo, listener, opciones?)` SI acepta
//!   fase de captura de verdad: `opciones` puede ser un booleano (forma
//!   legado `useCapture`) o un objeto con `.capture` (forma moderna
//!   `{capture: true}`), `false` por defecto en ambas formas -
//!   `event_listener_options_capture` (mas abajo) interpreta las dos.
//!   `removeEventListener` exige que capture coincida ademas de tipo e
//!   identidad: el MISMO listener registrado una vez con captura y otra
//!   sin ella son dos entradas distintas, igual que el spec real. El
//!   despacho tiene las tres fases reales, en orden (ver
//!   `dispatch_event_with_bubbling`): captura (raiz -> padre del target,
//!   solo listeners `{capture: true}`), target (el target mismo, TODOS
//!   sus listeners sin importar captura) y burbujeo (padre del target ->
//!   raiz, solo listeners sin captura, solo si `.bubbles`). `new
//!   Event(tipo, opciones?)` crea un objeto con `.type`, `.target`
//!   (`null` hasta que `dispatchEvent` lo pone), `.bubbles`/`.cancelable`
//!   (de `opciones.bubbles`/`opciones.cancelable` si se pasan, `false`
//!   por defecto en ambos - igual que el spec real), `.defaultPrevented`/
//!   `.propagationStopped` (`false` iniciales) y los metodos reales
//!   `preventDefault()`/`stopPropagation()` - ambos mutan el propio objeto
//!   (`this`) via `JsObject::set`, asi que funcionan igual para un evento
//!   creado en JS que para el que construye `DomBindings::dispatch_event`
//!   internamente (que fija `bubbles`/`cancelable` a `true` siempre - hoy
//!   el unico evento real que pasa por ahi es "click", que en el spec real
//!   siempre burbujea y es cancelable). `preventDefault()` respeta
//!   `.cancelable`: si es `false`, es un no-op honesto. `dispatchEvent`
//!   devuelve `false` si algun listener consiguio marcar
//!   `defaultPrevented`, `true` si no. El clic
//!   izquierdo real del sistema operativo YA
//!   dispara esto de punta a punta (`gfx/src/window.rs` captura
//!   `MouseInput`/`CursorMoved` de winit, hit-testea con
//!   `LayoutBox::hit_test` y llama a `JsRuntime::dispatch_event` desde
//!   `core/main.rs` - ver ARCHITECTURE.md, "Clic real del SO cableado de
//!   punta a punta", para el detalle completo incluida la limitacion
//!   honesta de que ese camino especifico via winit no tiene una prueba
//!   automatizada propia, solo sus piezas por separado). `scroll`/teclado
//!   siguen sin ninguna fuente real - Fase 3 solo esta completa para
//!   clics. `DomBindings::dispatch_event`/`JsRuntime::dispatch_event`
//!   (`runtime.rs`) son lo que hace esto posible: disparan un evento sobre
//!   un nodo desde codigo Rust, SIN pasar por texto JS - necesario porque
//!   antes de esto el `JsRuntime` entero (y con el, el `EventRegistry` con
//!   los listeners) se destruia justo despues de la carga inicial de la
//!   pagina, antes de que la ventana siquiera se abriera.
//!   `execute_inline_scripts_keeping_runtime`/`pipeline::
//!   build_page_keeping_runtime` (`core/scripting.rs`/`core/pipeline.rs`)
//!   devuelven el runtime en vez de dropearlo.
//!   `document.documentElement`/`document.body` son GETTERS reales (se
//!   leen sin parentesis, no son metodos): `documentElement` es el UNICO
//!   hijo `Element` directo de la raiz del documento (normalmente
//!   `<html>`, via `Node::document_element` en `dom/node.rs` - busca solo
//!   entre los hijos DIRECTOS, no en todo el subarbol, igual que el spec
//!   real); `body` es el primer `<body>` real (reusa `Node::
//!   find_all_by_tag`), `null` si no hay ninguno - aunque en la practica
//!   `html5ever` SIEMPRE sintetiza un `<body>` incluso para `<html></html>`
//!   vacio, igual que un navegador real, asi que `null` es dificil de
//!   observar con HTML parseado de verdad (probado explicitamente, un
//!   hallazgo real: la primera version de este binding asumia lo
//!   contrario). Misma caveat de identidad que `classList`/`style`/
//!   `children`/`parentElement`: cada lectura construye un objeto JS
//!   nuevo, asi que `document.body === document.body` da `false` aqui.
//! - `tagName` sigue siendo una FOTO: se calcula una vez al construir el
//!   objeto JS y no se actualiza si el DOM cambia despues - igual que
//!   `Node::find_all_by_tag` (ver `dom/node.rs`). No es observable hoy (no
//!   hay ningun binding que cambie la etiqueta de un elemento, y tampoco lo
//!   hay en el DOM real: `tagName` es de solo lectura incluso alli).
//! - `querySelectorAll` devuelve un `Array` real de JS (no un `NodeList` -
//!   un `Array` ya trae `.forEach`/`.map`/etc. que un `NodeList` no trae de
//!   fabrica, asi que en la practica es mas capaz, no menos), congelado
//!   (la LISTA de elementos, no cada elemento) en el momento de la llamada:
//!   si el DOM gana o pierde elementos que matchearian despues, ese array
//!   ya construido no se entera - hay que volver a llamar a
//!   `querySelectorAll` para verlo.
//!
//! Como `scripting.rs` ejecuta los `<script>` inline ANTES de construir el
//! layout (`core/pipeline.rs::build_page`: parseo -> JS -> cascada ->
//! layout), una mutacion real hecha durante la ejecucion inicial de un
//! script YA se refleja en el layout resultante sin necesitar recalculo
//! tras interaccion (eso sigue siendo Fase 3 - clic/scroll/teclado
//! disparando un nuevo layout en caliente, que no existe).
//!
//! Cualquier otro miembro real del DOM (`insertBefore`/`replaceChild` en
//! `Document`...) no esta
//! implementado: un script que lo use vera `undefined`, como en JS normal
//! al leer una propiedad que no existe - no es un error fingido, pero
//! tampoco es soporte real.

use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::{FunctionObjectBuilder, JsData, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsObject, JsResult, JsValue, NativeFunction};
use boa_gc::{Finalize, Trace};
use engine_css::{CssParser, SelectorMatcher};
use engine_dom::{Node, NodeType};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

pub struct DomBindings;

/// Registro de listeners COMPARTIDO por todo el runtime (no por elemento ni
/// por objeto JS envoltorio, que se reconstruye nuevo en cada consulta -
/// ver el aviso de `ElementCapture`): clave = puntero identificador del
/// `Arc<RwLock<Node>>` real (`Arc::as_ptr`, estable entre clones del mismo
/// `Arc`), valor = lista de `(tipo, listener)` en orden de registro. Asi
/// que `addEventListener` registrado desde una consulta y `dispatchEvent`
/// invocado desde OTRA consulta al MISMO elemento operan sobre el mismo
/// registro real - ver `dispatch_event`/`add_event_listener` en
/// `element_to_js_object`.
///
/// Guarda `JsObject` (valores JS reales, con punteros `Gc<_>` internos) en
/// un `std::sync::Mutex` marcado `#[unsafe_ignore_trace]`. A diferencia de
/// `DomRootCapture`/`ElementCapture` de aqui abajo (que NO contienen
/// punteros `Gc` y por eso el ignore es trivialmente seguro), esto SI los
/// contiene - pero sigue siendo seguro, por una razon distinta, verificada
/// contra el codigo fuente de `boa_gc` (no asumida): `Gc<T>` no es un
/// colector puramente por trazado - lleva un contador de referencias
/// normal (`ref_count`, vivo mientras exista algun `Clone` en cualquier
/// sitio) ADEMAS del recorrido de trazado (`non_root_count`, solo cuenta
/// clones DESCUBIERTOS trazando campos `Trace`). Un objeto se trata como
/// RAIZ (`is_rooted() = ref_count > non_root_count`) cuando tiene mas
/// referencias de las que el trazado puede explicar - exactamente lo que
/// pasa aqui: el `JsObject` guardado en este Mutex invisible al trazador
/// sigue contando para `ref_count` (via `Clone`/`Drop` normales) pero
/// nunca para `non_root_count`, asi que el colector lo mantiene vivo como
/// raiz en vez de liberarlo bajo los pies mientras siga en el mapa. El
/// unico riesgo real es el mundano: olvidar quitar una entrada del mapa
/// (lo que `removeEventListener` hace), no un use-after-free.
///
/// `pub` (a diferencia de `DomRootCapture`/`ElementCapture`, que son
/// puramente internos): `JsRuntime::bind_dom` necesita guardar el que
/// `DomBindings::register` crea, para poder pasarlo mas tarde a
/// `DomBindings::dispatch_event` desde fuera de este archivo. El campo
/// interno sigue siendo privado - un llamador externo solo puede pasar el
/// handle de un lado a otro, no fabricar uno vacio ni mirar dentro.
#[derive(Trace, Finalize, Clone)]
pub struct EventRegistry(#[unsafe_ignore_trace] Arc<Mutex<HashMap<usize, Vec<(String, JsObject, bool)>>>>);

/// Envoltorio para poder capturar `Arc<RwLock<Node>>` en un closure nativo
/// de Boa (`NativeFunction::from_copy_closure_with_captures` exige que las
/// capturas implementen `Trace`, para que el recolector pueda rastrear
/// posibles punteros `Gc<_>` propios). `Arc<RwLock<Node>>` no contiene
/// ninguno - es memoria propia del motor, fuera del heap que rastrea Boa -
/// asi que `#[unsafe_ignore_trace]` es correcto aqui, no un atajo que
/// esconda un bug: es el mismo patron que usa Boa internamente para
/// envolver closures de Rust (struct `Closure<F, T>` en su propio
/// `native_function.rs`). El segundo campo (`EventRegistry`) viaja junto
/// al documento entero - todas las consultas (`getElementById`,
/// `querySelector`...) comparten el MISMO registro de listeners.
#[derive(Trace, Finalize, Clone)]
struct DomRootCapture(#[unsafe_ignore_trace] Arc<RwLock<Node>>, EventRegistry);

/// Igual que `DomRootCapture` pero para UN elemento concreto, no todo el
/// documento - lo que hace que `getAttribute`/`setAttribute` sean vivos de
/// verdad en vez de leer/escribir una copia congelada (ver el aviso al
/// principio del archivo). El segundo campo (`EventRegistry`) es el MISMO
/// registro compartido de `DomRootCapture` - `addEventListener`/
/// `removeEventListener`/`dispatchEvent` lo usan indexado por el puntero
/// del nodo real (`.0`), no por este objeto JS envoltorio.
///
/// Ademas de capturarse en los closures de sus propios metodos, se adjunta
/// como DATOS NATIVOS del objeto JS del elemento (`ObjectInitializer::
/// with_native_data`, ver `element_to_js_object`) - eso permite que
/// `appendChild` recupere el `Arc<RwLock<Node>>` real de un objeto JS
/// arbitrario que reciba como argumento (via `JsObject::downcast_ref`), no
/// solo del que ya tiene capturado por cierre.
#[derive(Trace, Finalize, Clone)]
struct ElementCapture(#[unsafe_ignore_trace] Arc<RwLock<Node>>, EventRegistry);

/// Cuerpo vacio le basta: el metodo por defecto ya sirve, `NativeObject` se
/// consigue gratis via su impl generica (`impl<T: Any + Trace + JsData>
/// NativeObject for T`).
impl JsData for ElementCapture {}

impl DomBindings {
    /// Devuelve el `EventRegistry` que este `register` acaba de crear y
    /// enganchar a `document.*` - quien llama (`JsRuntime::bind_dom`) lo
    /// guarda para poder disparar eventos MAS TARDE desde codigo Rust (via
    /// `DomBindings::dispatch_event`), sin pasar por texto JS ni depender
    /// de que el registro siga vivo solo porque un closure lo capturo.
    pub fn register(context: &mut Context, dom_root: Arc<RwLock<Node>>) -> JsResult<EventRegistry> {
        let print_fn = NativeFunction::from_fn_ptr(|_this, args, _context| {
            if let Some(msg) = args.first() {
                tracing::info!("[JS Engine Console Log]: {}", msg.display());
            }
            Ok(JsValue::undefined())
        });

        context.register_global_builtin_callable(js_string!("printEngineLog"), 1, print_fn)?;

        let event_registry = EventRegistry(Arc::new(Mutex::new(HashMap::new())));
        let capture = DomRootCapture(dom_root, event_registry.clone());

        let get_element_by_id = NativeFunction::from_copy_closure_with_captures(
            |_this, args, capture: &DomRootCapture, context| {
                let Some(arg) = args.first() else { return Ok(JsValue::null()) };
                let id = arg.to_string(context)?.to_std_string_escaped();
                Ok(match Node::find_by_id(&capture.0, &id) {
                    Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                    None => JsValue::null(),
                })
            },
            capture.clone(),
        );

        let query_selector = NativeFunction::from_copy_closure_with_captures(
            |_this, args, capture: &DomRootCapture, context| {
                let Some(arg) = args.first() else { return Ok(JsValue::null()) };
                let selector = arg.to_string(context)?.to_std_string_escaped();
                Ok(match SelectorMatcher::query_first(&selector, &capture.0) {
                    Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                    None => JsValue::null(),
                })
            },
            capture.clone(),
        );

        let query_selector_all = NativeFunction::from_copy_closure_with_captures(
            |_this, args, capture: &DomRootCapture, context| {
                let Some(arg) = args.first() else {
                    return Ok(JsArray::from_iter(Vec::new(), context).into());
                };
                let selector = arg.to_string(context)?.to_std_string_escaped();
                let nodes = SelectorMatcher::query_all(&selector, &capture.0);

                let mut elements: Vec<JsValue> = Vec::with_capacity(nodes.len());
                for node in &nodes {
                    elements.push(element_to_js_object(node, &capture.1, context).into());
                }
                Ok(JsArray::from_iter(elements, context).into())
            },
            capture.clone(),
        );

        // No necesita el DOM para BUSCAR (crea un nodo nuevo y
        // desconectado), pero SI necesita `EventRegistry` para poder pasarlo
        // a `element_to_js_object` - por eso captura `EventRegistry` sola,
        // no todo `DomRootCapture`.
        let create_element = NativeFunction::from_copy_closure_with_captures(
            |_this, args, registry: &EventRegistry, context| {
                let Some(arg) = args.first() else { return Ok(JsValue::undefined()) };
                let tag_name = arg.to_string(context)?.to_std_string_escaped();
                let node = Node::new(NodeType::Element { tag_name, attributes: HashMap::new() });
                Ok(element_to_js_object(&node, registry, context).into())
            },
            event_registry.clone(),
        );

        // `documentElement`/`body` son GETTERS (se leen sin parentesis,
        // igual que en un navegador real - `.function` los haria metodos,
        // que no es lo que es `document.documentElement`), no funciones.
        // Misma caveat de identidad que `classList`/`style`/`children`/
        // `parentElement`: cada lectura construye un objeto JS nuevo (via
        // `element_to_js_object`), asi que `document.body === document.body`
        // da `false` aqui - no afecta a que ambas lecturas operen sobre el
        // mismo `Arc<RwLock<Node>>` real por debajo.
        let document_element_getter = NativeFunction::from_copy_closure_with_captures(
            |_this, _args, capture: &DomRootCapture, context| {
                Ok(match Node::document_element(&capture.0) {
                    Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                    None => JsValue::null(),
                })
            },
            capture.clone(),
        );
        let document_element_getter_fn = FunctionObjectBuilder::new(context.realm(), document_element_getter)
            .name(js_string!("get documentElement"))
            .length(0)
            .constructor(false)
            .build();

        // `document.body`: el primer `<body>` real del documento (no una
        // busqueda de "el primer elemento", especificamente por tag - igual
        // que `HTMLDocument.body` real, aunque el spec real tambien acepta
        // `<frameset>`, que este motor no distingue de un elemento normal
        // de todas formas). `null` si la pagina no tiene `<body>`.
        let body_getter = NativeFunction::from_copy_closure_with_captures(
            |_this, _args, capture: &DomRootCapture, context| {
                Ok(match Node::find_all_by_tag(&capture.0, "body").into_iter().next() {
                    Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                    None => JsValue::null(),
                })
            },
            capture,
        );
        let body_getter_fn = FunctionObjectBuilder::new(context.realm(), body_getter)
            .name(js_string!("get body"))
            .length(0)
            .constructor(false)
            .build();

        let document = ObjectInitializer::new(context)
            .function(get_element_by_id, js_string!("getElementById"), 1)
            .function(query_selector, js_string!("querySelector"), 1)
            .function(query_selector_all, js_string!("querySelectorAll"), 1)
            .function(create_element, js_string!("createElement"), 1)
            .accessor(js_string!("documentElement"), Some(document_element_getter_fn), None, Attribute::all())
            .accessor(js_string!("body"), Some(body_getter_fn), None, Attribute::all())
            .build();

        context.register_global_property(js_string!("document"), document, Attribute::all())?;

        // `new Event(tipo, opciones?)` - construye el mismo objeto que
        // produce `build_event_object` (mas abajo), el mismo que usa el
        // propio motor al despachar un evento real: `.type`, `.target`
        // (null hasta que algo lo despache de verdad), `.bubbles`/
        // `.cancelable` (de `opciones.bubbles`/`opciones.cancelable` si se
        // pasan, `false` por defecto en ambos - igual que el spec real),
        // `.defaultPrevented`/`.propagationStopped` y `preventDefault()`/
        // `stopPropagation()` reales que mutan esos flags -
        // `preventDefault()` es un no-op si `.cancelable` es `false`.
        // Necesita `.constructor(true)` de verdad para que `new`
        // no lance "not a constructor" - `register_global_builtin_callable`
        // (usado arriba para `printEngineLog`) fuerza `.constructor(false)`
        // a proposito, asi que aqui hace falta el registro CONSTRUIBLE
        // (`register_global_callable`), no ese.
        let event_constructor = NativeFunction::from_fn_ptr(|_this, args, context| {
            let event_type = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => "undefined".to_string(),
            };
            let (bubbles, cancelable) = match args.get(1).and_then(|v| v.as_object()) {
                Some(opts) => (
                    opts.get(js_string!("bubbles"), context)?.to_boolean(),
                    opts.get(js_string!("cancelable"), context)?.to_boolean(),
                ),
                None => (false, false),
            };
            Ok(build_event_object(&event_type, bubbles, cancelable, context))
        });
        context.register_global_callable(js_string!("Event"), 1, event_constructor)?;

        Ok(event_registry)
    }

    /// Dispara `event_type` sobre `node` de verdad, CON bubbling real
    /// (`dispatch_event_with_bubbling`, mas abajo) - SIN pasar por texto
    /// JS: construye el mismo objeto `Event` que `new Event(...)`
    /// (`build_event_object`, con `bubbles`/`cancelable` fijos a `true` -
    /// ver el comentario junto a esa llamada, mas abajo), le pone `target`
    /// = el objeto elemento de `node` (igual que pondria `dispatchEvent`
    /// desde JS), y sube por los ancestros llamando a sus listeners
    /// reales. Pensado para invocarse desde Rust cuando el motor tenga una
    /// fuente de eventos real (clic/teclado del SO) que traducir a un
    /// nodo - el clic ya existe (ver ARCHITECTURE.md), teclado sigue
    /// pendiente. No-op honesto (no un panic) si `registry` no viene de un
    /// `DomBindings::register` real sobre este mismo `context`.
    /// Devuelve si algun listener llamo `event.preventDefault()` (Fase
    /// 4.2) - quien dispara el evento desde Rust (p.ej.
    /// `core::server::click` antes de navegar por un `<a href>`) necesita
    /// saber esto para decidir si la ACCION POR DEFECTO (seguir el
    /// enlace) debe cancelarse, igual que un navegador real. `Ok(false)`
    /// (nunca `Err`) si `registry` no viene de un `DomBindings::register`
    /// real - "nadie llamo preventDefault porque nadie llego a ver el
    /// evento" es la respuesta honesta para ese no-op.
    pub fn dispatch_event(registry: &EventRegistry, node: &Arc<RwLock<Node>>, event_type: &str, context: &mut Context) -> JsResult<bool> {
        Self::dispatch_event_impl(registry, node, event_type, None, context)
    }

    /// Igual que `dispatch_event`, pero el objeto `Event` construido
    /// ademas lleva `.key` real puesto (Fase 4.1) - antes de esta tarea,
    /// CUALQUIER evento disparado desde Rust (incluidos los de teclado
    /// sinteticos de `core::server::press_key`) llegaba a los listeners de
    /// `addEventListener('keydown', ...)` con un `Event` generico SIN
    /// ninguna propiedad de tecla, asi que un listener real que mirara
    /// `event.key` (el caso de uso mas comun de `keydown`) no podia
    /// funcionar. Sin variante de `KeyboardEvent` completa (`.code`/
    /// `.shiftKey`/`.ctrlKey`/... - fuera del alcance de esta tarea, ver
    /// ARCHITECTURE.md), solo `.key`.
    pub fn dispatch_keyboard_event(registry: &EventRegistry, node: &Arc<RwLock<Node>>, event_type: &str, key: &str, context: &mut Context) -> JsResult<bool> {
        Self::dispatch_event_impl(registry, node, event_type, Some(key), context)
    }

    fn dispatch_event_impl(registry: &EventRegistry, node: &Arc<RwLock<Node>>, event_type: &str, key: Option<&str>, context: &mut Context) -> JsResult<bool> {
        let target: JsValue = element_to_js_object(node, registry, context).into();
        // `bubbles`/`cancelable` a `true` a fuego: los eventos reales que
        // pasan por aqui (clic, foco, teclado - ver gfx::window +
        // core::main/server) burbujean y son cancelables en el spec real.
        // Si algun dia se cablea un tipo de evento con semantica distinta,
        // este es el sitio a parametrizar - no antes.
        let event_value = build_event_object(event_type, true, true, context);
        if let Some(event_obj) = event_value.as_object() {
            event_obj.set(js_string!("target"), target.clone(), true, context)?;
            if let Some(k) = key {
                event_obj.set(js_string!("key"), js_string!(k), true, context)?;
            }
        }
        dispatch_event_with_bubbling(registry, node, &target, event_type, &event_value, context)?;
        event_default_prevented(&event_value, context)
    }
}

fn event_default_prevented(event_value: &JsValue, context: &mut Context) -> JsResult<bool> {
    match event_value.as_object() {
        Some(event_obj) => Ok(event_obj.get(js_string!("defaultPrevented"), context)?.to_boolean()),
        None => Ok(false),
    }
}

/// `type`/`target`/`bubbles`/`cancelable` son datos; `defaultPrevented`/
/// `propagationStopped` empiezan en `false` y son propiedades NORMALES
/// escribibles (no hay setter dedicado que las proteja de una asignacion
/// directa tipo `event.defaultPrevented = true` - simplificacion honesta,
/// el efecto observable de todas formas termina siendo el mismo que
/// llamar al metodo real). `preventDefault`/`stopPropagation` son las
/// unicas formas PENSADAS de mutarlas: ambas usan `JsObject::set` sobre
/// `this` (no sobre una captura Rust aparte), asi que funcionan igual
/// para un objeto creado por `new Event(...)` en JS como para el que
/// construye `DomBindings::dispatch_event` internamente - una sola
/// implementacion, dos caminos. `preventDefault` respeta `cancelable`: si
/// es `false`, es un no-op honesto (como el spec real), `defaultPrevented`
/// se queda en `false`. `target` se deja en `null` aqui: lo pone quien
/// dispare el evento (`dispatchEvent` en `element_to_js_object`, o
/// `DomBindings::dispatch_event`), porque `build_event_object` no sabe
/// todavia sobre que nodo se va a disparar.
fn build_event_object(event_type: &str, bubbles: bool, cancelable: bool, context: &mut Context) -> JsValue {
    let prevent_default = NativeFunction::from_fn_ptr(|this, _args, context| {
        if let Some(event_obj) = this.as_object() {
            let cancelable = event_obj.get(js_string!("cancelable"), context)?.to_boolean();
            if cancelable {
                event_obj.set(js_string!("defaultPrevented"), true, true, context)?;
            }
        }
        Ok(JsValue::undefined())
    });
    let stop_propagation = NativeFunction::from_fn_ptr(|this, _args, context| {
        if let Some(event_obj) = this.as_object() {
            event_obj.set(js_string!("propagationStopped"), true, true, context)?;
        }
        Ok(JsValue::undefined())
    });

    let event_obj = ObjectInitializer::new(context)
        .property(js_string!("type"), js_string!(event_type), Attribute::all())
        .property(js_string!("target"), JsValue::null(), Attribute::all())
        .property(js_string!("bubbles"), bubbles, Attribute::all())
        .property(js_string!("cancelable"), cancelable, Attribute::all())
        .property(js_string!("defaultPrevented"), false, Attribute::all())
        .property(js_string!("propagationStopped"), false, Attribute::all())
        .function(prevent_default, js_string!("preventDefault"), 0)
        .function(stop_propagation, js_string!("stopPropagation"), 0)
        .build();
    JsValue::from(event_obj)
}

/// Busca los listeners registrados para `node` (por su puntero real) con
/// tipo `event_type` y los llama, con `this` = `this_value` y como unico
/// argumento `event_value` - compartido entre el `dispatchEvent` expuesto a
/// JS (mas abajo, en `element_to_js_object`) y `DomBindings::dispatch_event`
/// (invocable desde Rust), para no duplicar la logica de busqueda/llamada.
/// Un solo nodo, SIN subir/bajar a otros - `dispatch_event_with_bubbling`
/// (mas abajo) es quien la llama repetidamente, una vez por nodo y fase,
/// para implementar las tres fases reales. `phase_capture` filtra CUALES
/// listeners de `node` se llaman: `Some(true)` solo los de captura,
/// `Some(false)` solo los de burbujeo (sin captura), `None` todos (la
/// fase de target, donde ambos tipos se llaman juntos, en orden de
/// registro, igual que el spec real).
fn dispatch_event_to_listeners(
    registry: &EventRegistry,
    node: &Arc<RwLock<Node>>,
    event_type: &str,
    phase_capture: Option<bool>,
    this_value: &JsValue,
    event_value: &JsValue,
    context: &mut Context,
) -> JsResult<()> {
    let key = Arc::as_ptr(node) as usize;
    let matching: Vec<JsObject> = registry
        .0
        .lock()
        .unwrap()
        .get(&key)
        .map(|listeners| {
            listeners
                .iter()
                .filter(|(t, _, c)| t == event_type && phase_capture.map_or(true, |want| *c == want))
                .map(|(_, l, _)| l.clone())
                .collect()
        })
        .unwrap_or_default();

    for listener in matching {
        if let Some(func) = JsFunction::from_object(listener) {
            func.call(this_value, &[event_value.clone()], context)?;
        }
    }
    Ok(())
}

/// Las tres fases reales del spec, en orden: CAPTURA (raiz -> padre
/// inmediato del target, solo listeners con `{capture: true}`), TARGET
/// (el target mismo, TODOS sus listeners sin importar `capture`, en orden
/// de registro) y BURBUJEO (padre inmediato del target -> raiz, solo
/// listeners SIN captura, y solo si `event_value.bubbles` es `true`).
/// `ancestors` se recolecta subiendo por `.parent` (el unico sentido
/// barato de recorrer el arbol) y se invierte para tener el orden
/// raiz-primero que exige la fase de captura. En el target se usa
/// `target_this` TAL CUAL - la misma referencia de JS que ya tenia quien
/// llamo (`el` en `el.dispatchEvent(...)`, o lo que ya construyera
/// `DomBindings::dispatch_event`) - preservando la identidad ya probada
/// (`this === el` dentro de un listener puesto en el propio target, el
/// caso mas comun con diferencia). En cada ANCESTRO (captura o burbujeo)
/// no hay ninguna referencia previa que reusar, asi que ahi si se
/// construye una envoltura nueva por nodo - misma caveat de identidad que
/// `classList`/`style`/etc, documentada donde corresponde.
/// `propagationStopped` (puesto por `stopPropagation()`) se comprueba
/// ANTES de cada grupo de listeners, en las tres fases - un listener de
/// captura puede impedir que el propio target o el burbujeo se enteren,
/// igual que uno de burbujeo puede impedir que el siguiente ancestro se
/// entere.
fn dispatch_event_with_bubbling(
    registry: &EventRegistry,
    target: &Arc<RwLock<Node>>,
    target_this: &JsValue,
    event_type: &str,
    event_value: &JsValue,
    context: &mut Context,
) -> JsResult<()> {
    let mut ancestors = Vec::new();
    {
        let mut current = {
            let n = target.read().unwrap();
            n.parent.as_ref().and_then(Weak::upgrade)
        };
        while let Some(node) = current {
            current = {
                let n = node.read().unwrap();
                n.parent.as_ref().and_then(Weak::upgrade)
            };
            ancestors.push(node);
        }
    }
    ancestors.reverse(); // raiz -> ... -> padre inmediato del target

    // Fase de captura: el target NO se incluye aqui - sus listeners, de
    // captura o no, se llaman juntos en la fase de target, mas abajo.
    for node in &ancestors {
        if event_propagation_stopped(event_value, context)? {
            return Ok(());
        }
        let this_value: JsValue = element_to_js_object(node, registry, context).into();
        dispatch_event_to_listeners(registry, node, event_type, Some(true), &this_value, event_value, context)?;
    }
    if event_propagation_stopped(event_value, context)? {
        return Ok(());
    }

    // Fase de target: todos los listeners del target, de captura o no.
    dispatch_event_to_listeners(registry, target, event_type, None, target_this, event_value, context)?;

    if !event_bubbles(event_value, context)? {
        return Ok(());
    }

    // Fase de burbujeo: orden inverso a la captura, solo listeners SIN
    // captura.
    for node in ancestors.iter().rev() {
        if event_propagation_stopped(event_value, context)? {
            return Ok(());
        }
        let this_value: JsValue = element_to_js_object(node, registry, context).into();
        dispatch_event_to_listeners(registry, node, event_type, Some(false), &this_value, event_value, context)?;
    }
    Ok(())
}

fn event_bubbles(event_value: &JsValue, context: &mut Context) -> JsResult<bool> {
    match event_value.as_object() {
        Some(event_obj) => Ok(event_obj.get(js_string!("bubbles"), context)?.to_boolean()),
        None => Ok(false),
    }
}

fn event_propagation_stopped(event_value: &JsValue, context: &mut Context) -> JsResult<bool> {
    match event_value.as_object() {
        Some(event_obj) => Ok(event_obj.get(js_string!("propagationStopped"), context)?.to_boolean()),
        None => Ok(false),
    }
}

/// Interpreta el tercer argumento de `addEventListener`/
/// `removeEventListener`. Ausente o cualquier cosa que no matchee ninguna
/// forma de abajo: `false` (sin captura, el default real). Booleano puro
/// (`true`/`false`): forma legado del spec (`useCapture`), donde el
/// booleano ES directamente el valor de captura. Objeto con `.capture`:
/// forma moderna (`{capture: true}`), coercionado con `to_boolean` (asi
/// que `{capture: 1}` tambien cuenta, igual que el spec real via
/// `ToBoolean`).
fn event_listener_options_capture(arg: Option<&JsValue>, context: &mut Context) -> JsResult<bool> {
    match arg {
        Some(v) => match v.as_boolean() {
            Some(b) => Ok(b),
            None => match v.as_object() {
                Some(opts) => Ok(opts.get(js_string!("capture"), context)?.to_boolean()),
                None => Ok(false),
            },
        },
        None => Ok(false),
    }
}

/// Construye el objeto JS de un elemento - `tagName` es foto,
/// `getAttribute`/`setAttribute`/`textContent`/`appendChild` son vivos; ver
/// el aviso al principio del archivo para la distincion completa. `registry`
/// es el `EventRegistry` COMPARTIDO por todo el documento (ver su aviso) -
/// se pasa explicito en vez de crearse aqui para que `addEventListener`
/// registrado desde una consulta y `dispatchEvent` desde otra sigan viendo
/// el mismo registro.
fn element_to_js_object(node: &Arc<RwLock<Node>>, registry: &EventRegistry, context: &mut Context) -> JsObject {
    let tag_name = {
        let n = node.read().unwrap();
        match &n.node_type {
            NodeType::Element { tag_name, .. } => tag_name.clone(),
            _ => String::new(),
        }
    };
    let capture = ElementCapture(node.clone(), registry.clone());

    let get_attribute = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::null()) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            let n = capture.0.read().unwrap();
            Ok(match &n.node_type {
                NodeType::Element { attributes, .. } => match attributes.get(&name) {
                    Some(value) => JsValue::from(js_string!(value.clone())),
                    None => JsValue::null(),
                },
                _ => JsValue::null(),
            })
        },
        capture.clone(),
    );

    let set_attribute = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(name_arg) = args.first() else { return Ok(JsValue::undefined()) };
            let name = name_arg.to_string(context)?.to_std_string_escaped();
            // Igual que JS real: si falta el segundo argumento, es
            // `undefined`, y `ToString(undefined)` es la cadena literal
            // "undefined" - no una cadena vacia ni un error.
            let value = match args.get(1) {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => "undefined".to_string(),
            };
            let mut n = capture.0.write().unwrap();
            if let NodeType::Element { attributes, .. } = &mut n.node_type {
                attributes.insert(name, value);
            }
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let text_content_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, _context| Ok(JsValue::from(js_string!(Node::text_content(&capture.0)))),
        capture.clone(),
    );
    let text_content_getter_fn = FunctionObjectBuilder::new(context.realm(), text_content_getter)
        .name(js_string!("get textContent"))
        .length(0)
        .constructor(false)
        .build();

    let text_content_setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let value = match args.first() {
                None | Some(JsValue::Undefined) => "undefined".to_string(),
                // [LegacyNullToEmptyString] del spec real: `textContent = null`
                // limpia el texto en vez de escribir la cadena "null" (que es
                // lo que haria ToString(null) en cualquier otra propiedad).
                Some(JsValue::Null) => String::new(),
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
            };
            // Reemplaza TODOS los hijos por un unico nodo de texto - la
            // semantica real de `textContent`, no un append.
            let mut n = capture.0.write().unwrap();
            n.children.clear();
            let text_node = Node::new(NodeType::Text(value));
            text_node.write().unwrap().parent = Some(Arc::downgrade(&capture.0));
            n.children.push(text_node);
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );
    let text_content_setter_fn = FunctionObjectBuilder::new(context.realm(), text_content_setter)
        .name(js_string!("set textContent"))
        .length(1)
        .constructor(false)
        .build();

    // Recibe el hijo como `JsValue` (lo que sea que pase el script) y
    // recupera su `Arc<RwLock<Node>>` real via los datos nativos del objeto
    // (ver el aviso en `ElementCapture`) - si no es un objeto nuestro (una
    // cadena, un numero, un objeto JS cualquiera...), no hay nodo que
    // conectar y no se hace nada, en vez de fingir que se añadio algo.
    // `detach_from_parent` (mas abajo) quita primero al hijo de la lista de
    // children de su padre ANTERIOR si ya tenia uno (de este arbol o de
    // otro) - un nodo solo puede tener un padre a la vez en el DOM real; sin
    // esto, un appendChild sobre un nodo ya conectado lo dejaria fantasma en
    // dos listas de children a la vez (o duplicado, si el padre viejo y el
    // nuevo son el mismo elemento).
    let append_child = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, _context| {
            let Some(child_value) = args.first() else { return Ok(JsValue::undefined()) };
            let Some(child_object) = child_value.as_object() else { return Ok(JsValue::undefined()) };
            let Some(child_capture) = child_object.downcast_ref::<ElementCapture>() else {
                return Ok(JsValue::undefined());
            };
            let child_node = child_capture.0.clone();
            drop(child_capture);

            detach_from_parent(&child_node);
            child_node.write().unwrap().parent = Some(Arc::downgrade(&capture.0));
            capture.0.write().unwrap().children.push(child_node);
            Ok(child_value.clone())
        },
        capture.clone(),
    );

    // Mismo mecanismo que appendChild para recuperar el nodo real de
    // `hijo`. El DOM real lanza `NotFoundError` si `hijo` no es hijo de
    // este elemento; aqui, igual que el resto de simplificaciones honestas
    // de este archivo, es un no-op explicito (devuelve `null`) en vez de
    // esa excepcion exacta.
    let remove_child = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, _context| {
            let Some(child_value) = args.first() else { return Ok(JsValue::null()) };
            let Some(child_object) = child_value.as_object() else { return Ok(JsValue::null()) };
            let Some(child_capture) = child_object.downcast_ref::<ElementCapture>() else {
                return Ok(JsValue::null());
            };
            let child_node = child_capture.0.clone();
            drop(child_capture);

            let mut parent = capture.0.write().unwrap();
            let before = parent.children.len();
            parent.children.retain(|c| !Arc::ptr_eq(c, &child_node));
            let removed = parent.children.len() < before;
            drop(parent);

            if !removed {
                return Ok(JsValue::null());
            }
            child_node.write().unwrap().parent = None;
            Ok(child_value.clone())
        },
        capture.clone(),
    );

    // insertBefore(nuevo, referencia): inserta `nuevo` justo antes de
    // `referencia` dentro de este elemento. `referencia` null/ausente
    // inserta al final (igual que el spec real, equivalente a appendChild).
    // Se VALIDA la posicion de `referencia` ANTES de tocar a `nuevo` -
    // igual que el algoritmo real del spec, que comprueba que `referencia`
    // sea hijo de verdad antes de mover nada - asi que si `referencia` no
    // es hijo real de este elemento, es un no-op honesto (`null`, el DOM
    // real lanzaria `NotFoundError`) que deja a `nuevo` exactamente donde
    // estaba, no a medio mover.
    let insert_before = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, _context| {
            let Some(new_value) = args.first() else { return Ok(JsValue::undefined()) };
            let Some(new_object) = new_value.as_object() else { return Ok(JsValue::undefined()) };
            let Some(new_capture) = new_object.downcast_ref::<ElementCapture>() else {
                return Ok(JsValue::undefined());
            };
            let new_node = new_capture.0.clone();
            drop(new_capture);

            let reference_node = match args.get(1) {
                None | Some(JsValue::Null) => None,
                Some(v) => {
                    let Some(object) = v.as_object() else { return Ok(JsValue::undefined()) };
                    let Some(reference_capture) = object.downcast_ref::<ElementCapture>() else {
                        return Ok(JsValue::undefined());
                    };
                    Some(reference_capture.0.clone())
                }
            };

            let insert_at = match &reference_node {
                None => None,
                Some(reference_node) => {
                    let parent = capture.0.read().unwrap();
                    let position = parent.children.iter().position(|c| Arc::ptr_eq(c, reference_node));
                    drop(parent);
                    match position {
                        Some(index) => Some(index),
                        None => return Ok(JsValue::null()),
                    }
                }
            };

            detach_from_parent(&new_node);
            new_node.write().unwrap().parent = Some(Arc::downgrade(&capture.0));

            let mut parent = capture.0.write().unwrap();
            match insert_at {
                Some(index) => parent.children.insert(index, new_node),
                None => parent.children.push(new_node),
            }
            drop(parent);
            Ok(new_value.clone())
        },
        capture.clone(),
    );

    // replaceChild(nuevo, viejo): sustituye a `viejo` por `nuevo` EN SU
    // MISMA POSICION dentro de este elemento y devuelve `viejo` (igual que
    // el spec real). Se valida que `viejo` sea hijo real ANTES de tocar
    // `nuevo`, mismo criterio que insertBefore.
    let replace_child = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, _context| {
            let Some(new_value) = args.first() else { return Ok(JsValue::undefined()) };
            let Some(new_object) = new_value.as_object() else { return Ok(JsValue::undefined()) };
            let Some(new_capture) = new_object.downcast_ref::<ElementCapture>() else {
                return Ok(JsValue::undefined());
            };
            let new_node = new_capture.0.clone();
            drop(new_capture);

            let Some(old_value) = args.get(1) else { return Ok(JsValue::null()) };
            let Some(old_object) = old_value.as_object() else { return Ok(JsValue::null()) };
            let Some(old_capture) = old_object.downcast_ref::<ElementCapture>() else {
                return Ok(JsValue::null());
            };
            let old_node = old_capture.0.clone();
            drop(old_capture);

            let is_real_child = capture.0.read().unwrap().children.iter().any(|c| Arc::ptr_eq(c, &old_node));
            if !is_real_child {
                return Ok(JsValue::null());
            }

            detach_from_parent(&new_node);
            new_node.write().unwrap().parent = Some(Arc::downgrade(&capture.0));

            let mut parent = capture.0.write().unwrap();
            // Se vuelve a buscar la posicion de `viejo` DESPUES del
            // detach de arriba (no se reusa la comprobacion anterior): si
            // `nuevo` ya era hijo de este mismo elemento en otra posicion,
            // ese detach desplaza indices dentro de esta misma lista.
            let index = parent.children.iter().position(|c| Arc::ptr_eq(c, &old_node));
            let Some(index) = index else {
                drop(parent);
                return Ok(JsValue::null());
            };
            parent.children[index] = new_node;
            drop(parent);

            old_node.write().unwrap().parent = None;
            Ok(old_value.clone())
        },
        capture.clone(),
    );

    // `classList` es un getter sin setter (igual que el DOM real) que
    // construye un objeto `classList` nuevo (ver `class_list_to_js_object`)
    // cada vez que se lee `.classList` - todos envuelven el mismo
    // `ElementCapture` vivo, asi que leer/mutar clases funciona igual que
    // en un navegador real, aunque `el.classList === el.classList` de aqui
    // daria `false` en vez de `true` (nadie en este motor depende de esa
    // identidad todavia).
    let class_list_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| Ok(class_list_to_js_object(capture.clone(), context).into()),
        capture.clone(),
    );
    let class_list_getter_fn = FunctionObjectBuilder::new(context.realm(), class_list_getter)
        .name(js_string!("get classList"))
        .length(0)
        .constructor(false)
        .build();

    // Sube por `Node::parent` (un `Weak`, ver su aviso en `dom/node.rs`) y
    // solo devuelve un objeto si el padre sigue vivo Y es un `Element` -
    // la raiz real del arbol es un `NodeType::Document` (ver
    // `html5ever_sink.rs`), no un elemento, asi que `<html>.parentElement`
    // da `null` aqui igual que en un navegador real
    // (`document.documentElement.parentElement === null`). Un nodo
    // desconectado (recien creado con `createElement`, o ya quitado con
    // `removeChild`) tambien da `null`: su `parent` es `None`.
    let parent_element_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| {
            let parent_node = {
                let n = capture.0.read().unwrap();
                n.parent.as_ref().and_then(Weak::upgrade)
            };
            Ok(match parent_node {
                Some(p) if matches!(p.read().unwrap().node_type, NodeType::Element { .. }) => {
                    element_to_js_object(&p, &capture.1, context).into()
                }
                _ => JsValue::null(),
            })
        },
        capture.clone(),
    );
    let parent_element_getter_fn = FunctionObjectBuilder::new(context.realm(), parent_element_getter)
        .name(js_string!("get parentElement"))
        .length(0)
        .constructor(false)
        .build();

    // Igual criterio que `querySelectorAll`: un `Array` real de JS, no un
    // `HTMLCollection` fingido. A diferencia de `querySelectorAll`, aqui SI
    // interesa que sea vivo (cada lectura vuelve a mirar `capture.0.
    // children`), porque `.children` normalmente se lee justo despues de
    // mutar el arbol propio (`appendChild`/`removeChild`) y una foto
    // congelada en el momento equivocado seria una fuente de bugs sutiles.
    // Filtra a solo `Element` (los nodos de texto no cuentan, igual que
    // `ParentNode.children` real) leyendo cada hijo con su propio lock -
    // son `Arc<RwLock<Node>>` DISTINTOS del padre que ya tenemos leido, asi
    // que no hay reentrada sobre el mismo RwLock.
    let children_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| {
            let element_children: Vec<Arc<RwLock<Node>>> = {
                let n = capture.0.read().unwrap();
                n.children
                    .iter()
                    .filter(|c| matches!(c.read().unwrap().node_type, NodeType::Element { .. }))
                    .cloned()
                    .collect()
            };
            let mut elements: Vec<JsValue> = Vec::with_capacity(element_children.len());
            for child in &element_children {
                elements.push(element_to_js_object(child, &capture.1, context).into());
            }
            Ok(JsArray::from_iter(elements, context).into())
        },
        capture.clone(),
    );
    let children_getter_fn = FunctionObjectBuilder::new(context.realm(), children_getter)
        .name(js_string!("get children"))
        .length(0)
        .constructor(false)
        .build();

    // `firstElementChild`/`lastElementChild`: el primer/ultimo hijo
    // DIRECTO que sea un `Element`, saltando nodos de texto - real DOM
    // spec (`ParentNode`), deliberadamente distinto de `firstChild`/
    // `lastChild` de `Node` (que SI pueden dar un nodo de texto): esos
    // exigirian poder envolver un nodo de texto como objeto JS, que este
    // motor no hace todavia (solo los `Element` se envuelven, ver
    // `element_to_js_object`) - `firstElementChild` evita ese problema
    // por diseño del spec real, no por una simplificacion propia.
    let first_element_child_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| {
            Ok(match first_element_child(&capture.0) {
                Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );
    let first_element_child_getter_fn = FunctionObjectBuilder::new(context.realm(), first_element_child_getter)
        .name(js_string!("get firstElementChild"))
        .length(0)
        .constructor(false)
        .build();

    let last_element_child_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| {
            Ok(match last_element_child(&capture.0) {
                Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );
    let last_element_child_getter_fn = FunctionObjectBuilder::new(context.realm(), last_element_child_getter)
        .name(js_string!("get lastElementChild"))
        .length(0)
        .constructor(false)
        .build();

    // `nextElementSibling`/`previousElementSibling`: suben al padre (igual
    // mecanismo que `parentElement`) y escanean sus hijos desde la
    // posicion de este nodo hacia adelante/atras buscando el siguiente
    // `Element`, saltando nodos de texto entre medias. `null` si no hay
    // padre (nodo desconectado), si no hay tal elemento, o si este nodo ni
    // siquiera aparece en `children` del padre (no deberia pasar en un
    // arbol consistente, pero no-op honesto en vez de panic si pasara).
    let next_element_sibling_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| {
            Ok(match next_element_sibling(&capture.0) {
                Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );
    let next_element_sibling_getter_fn = FunctionObjectBuilder::new(context.realm(), next_element_sibling_getter)
        .name(js_string!("get nextElementSibling"))
        .length(0)
        .constructor(false)
        .build();

    let previous_element_sibling_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| {
            Ok(match previous_element_sibling(&capture.0) {
                Some(node) => element_to_js_object(&node, &capture.1, context).into(),
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );
    let previous_element_sibling_getter_fn = FunctionObjectBuilder::new(context.realm(), previous_element_sibling_getter)
        .name(js_string!("get previousElementSibling"))
        .length(0)
        .constructor(false)
        .build();

    // `style` es un getter que construye un objeto nuevo cada vez (mismo
    // patron y misma caveat de identidad que `classList`, ver su aviso
    // arriba), respaldado por el atributo `style` real via
    // `style_read`/`style_write` (mas abajo) - la MISMA fuente que
    // `layout::resolve_style` lee para la cascada, asi que mutar aqui es
    // mutar de verdad lo que se pintaria en el siguiente layout.
    let style_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, context| Ok(style_to_js_object(capture.clone(), context).into()),
        capture.clone(),
    );
    let style_getter_fn = FunctionObjectBuilder::new(context.realm(), style_getter)
        .name(js_string!("get style"))
        .length(0)
        .constructor(false)
        .build();

    // addEventListener(tipo, listener): valida que `listener` sea invocable
    // (`JsValue::as_callable`, mismo mecanismo que ya usa `test_harness.rs`
    // para `test(fn, name)`) - si no, no-op honesto, nada que registrar. Lo
    // guarda en el `EventRegistry` COMPARTIDO (ver su aviso), no en este
    // objeto JS envoltorio, bajo la clave del nodo REAL
    // (`Arc::as_ptr(&capture.0)`) - asi que un `addEventListener` desde una
    // consulta y un `dispatchEvent` desde OTRA consulta al mismo elemento
    // se ven el uno al otro. Tercer argumento real: `event_listener_
    // options_capture` (mas abajo) interpreta un booleano suelto
    // (`useCapture`, forma legado) o un objeto `{capture: bool}` (forma
    // moderna), `false` en ambos casos si esta ausente.
    let add_event_listener = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(type_arg) = args.first() else { return Ok(JsValue::undefined()) };
            let event_type = type_arg.to_string(context)?.to_std_string_escaped();
            let Some(listener) = args.get(1).and_then(JsValue::as_callable).cloned() else {
                return Ok(JsValue::undefined());
            };
            let use_capture = event_listener_options_capture(args.get(2), context)?;
            let key = Arc::as_ptr(&capture.0) as usize;
            capture.1 .0.lock().unwrap().entry(key).or_default().push((event_type, listener, use_capture));
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    // removeEventListener(tipo, listener, opciones?): quita SOLO la
    // entrada que matchee tipo, identidad del listener Y captura -
    // `JsObject` (a diferencia de `JsFunction`, que no implementa
    // `PartialEq`) tiene igualdad real por identidad de puntero
    // (`Gc::ptr_eq`), asi que dos funciones JS "distintas" con el mismo
    // codigo fuente NO matchean, igual que el spec real. El MISMO listener
    // registrado una vez con `{capture: true}` y otra sin ella son DOS
    // entradas distintas - quitar una no toca la otra, tambien igual que
    // el spec real.
    let remove_event_listener = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(type_arg) = args.first() else { return Ok(JsValue::undefined()) };
            let event_type = type_arg.to_string(context)?.to_std_string_escaped();
            let Some(listener) = args.get(1).and_then(JsValue::as_callable) else {
                return Ok(JsValue::undefined());
            };
            let use_capture = event_listener_options_capture(args.get(2), context)?;
            let key = Arc::as_ptr(&capture.0) as usize;
            if let Some(listeners) = capture.1 .0.lock().unwrap().get_mut(&key) {
                listeners.retain(|(t, l, c)| !(t == &event_type && l == listener && *c == use_capture));
            }
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    // dispatchEvent(event): lee `event.type`, le pone `event.target` = el
    // elemento sobre el que se llamo dispatchEvent, y recorre las tres
    // fases reales - captura, target, burbujeo (`dispatch_event_with_
    // bubbling`, mas abajo), COMPARTIDO con `DomBindings::dispatch_event`
    // (invocable desde Rust, sin pasar por texto JS - lo que ya usa un
    // clic real, ver ARCHITECTURE.md). Devuelve `false` si algun listener
    // consiguio marcar `defaultPrevented` (via `preventDefault()`, que a
    // su vez respeta `.cancelable`), `true` si no.
    let dispatch_event = NativeFunction::from_copy_closure_with_captures(
        |this, args, capture: &ElementCapture, context| {
            let Some(event_value) = args.first() else { return Ok(JsValue::from(true)) };
            let Some(event_object) = event_value.as_object() else { return Ok(JsValue::from(true)) };
            let event_type = event_object.get(js_string!("type"), context)?.to_string(context)?.to_std_string_escaped();
            event_object.set(js_string!("target"), this.clone(), true, context)?;
            dispatch_event_with_bubbling(&capture.1, &capture.0, this, &event_type, event_value, context)?;
            let default_prevented = event_object.get(js_string!("defaultPrevented"), context)?.to_boolean();
            Ok(JsValue::from(!default_prevented))
        },
        capture.clone(),
    );

    ObjectInitializer::with_native_data(capture.clone(), context)
        .property(js_string!("tagName"), js_string!(tag_name.to_uppercase()), Attribute::all())
        .accessor(js_string!("textContent"), Some(text_content_getter_fn), Some(text_content_setter_fn), Attribute::all())
        .accessor(js_string!("classList"), Some(class_list_getter_fn), None, Attribute::all())
        .accessor(js_string!("parentElement"), Some(parent_element_getter_fn), None, Attribute::all())
        .accessor(js_string!("children"), Some(children_getter_fn), None, Attribute::all())
        .accessor(js_string!("firstElementChild"), Some(first_element_child_getter_fn), None, Attribute::all())
        .accessor(js_string!("lastElementChild"), Some(last_element_child_getter_fn), None, Attribute::all())
        .accessor(js_string!("nextElementSibling"), Some(next_element_sibling_getter_fn), None, Attribute::all())
        .accessor(js_string!("previousElementSibling"), Some(previous_element_sibling_getter_fn), None, Attribute::all())
        .accessor(js_string!("style"), Some(style_getter_fn), None, Attribute::all())
        .function(get_attribute, js_string!("getAttribute"), 1)
        .function(set_attribute, js_string!("setAttribute"), 2)
        .function(append_child, js_string!("appendChild"), 1)
        .function(remove_child, js_string!("removeChild"), 1)
        .function(insert_before, js_string!("insertBefore"), 2)
        .function(replace_child, js_string!("replaceChild"), 2)
        .function(add_event_listener, js_string!("addEventListener"), 2)
        .function(remove_event_listener, js_string!("removeEventListener"), 2)
        .function(dispatch_event, js_string!("dispatchEvent"), 1)
        .build()
}

/// Si `node` ya tiene un padre (de este arbol o de otro), lo desconecta de
/// verdad de la lista `children` de ESE padre - un nodo solo puede tener un
/// padre a la vez en el DOM real. Comparten esto `appendChild`,
/// `insertBefore` y `replaceChild` (los tres pueden recibir un nodo que ya
/// estaba conectado en otro sitio) - `removeChild` NO lo usa porque su
/// contrato es distinto: solo debe quitar al hijo de ESTE elemento
/// concreto, no de "donde sea que su puntero `.parent` diga que esta".
fn detach_from_parent(node: &Arc<RwLock<Node>>) {
    let old_parent = {
        let n = node.read().unwrap();
        n.parent.as_ref().and_then(Weak::upgrade)
    };
    if let Some(old_parent) = old_parent {
        old_parent.write().unwrap().children.retain(|c| !Arc::ptr_eq(c, node));
    }
}

fn first_element_child(node: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
    let n = node.read().unwrap();
    n.children.iter().find(|c| matches!(c.read().unwrap().node_type, NodeType::Element { .. })).cloned()
}

fn last_element_child(node: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
    let n = node.read().unwrap();
    n.children.iter().rev().find(|c| matches!(c.read().unwrap().node_type, NodeType::Element { .. })).cloned()
}

/// Comparten esto `next_element_sibling`/`previous_element_sibling`: subir
/// al padre y localizar en que posicion de `children` esta `node` de
/// verdad (no asumida) - hace falta para saber desde donde escanear hacia
/// adelante/atras.
fn position_among_siblings(node: &Arc<RwLock<Node>>) -> Option<(Arc<RwLock<Node>>, usize)> {
    let parent = {
        let n = node.read().unwrap();
        n.parent.as_ref().and_then(Weak::upgrade)
    }?;
    let index = {
        let parent_n = parent.read().unwrap();
        parent_n.children.iter().position(|c| Arc::ptr_eq(c, node))
    }?;
    Some((parent, index))
}

fn next_element_sibling(node: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
    let (parent, index) = position_among_siblings(node)?;
    let parent_n = parent.read().unwrap();
    parent_n.children[index + 1..].iter().find(|c| matches!(c.read().unwrap().node_type, NodeType::Element { .. })).cloned()
}

fn previous_element_sibling(node: &Arc<RwLock<Node>>) -> Option<Arc<RwLock<Node>>> {
    let (parent, index) = position_among_siblings(node)?;
    let parent_n = parent.read().unwrap();
    parent_n.children[..index].iter().rev().find(|c| matches!(c.read().unwrap().node_type, NodeType::Element { .. })).cloned()
}

/// `el.classList.add(...)`/`.remove(...)`/`.contains(...)`/`.toggle(...)` -
/// viven sobre el mismo atributo `class` (una cadena separada por
/// espacios) que `getAttribute('class')` ya expone; mutan de verdad sobre
/// `ElementCapture`, igual que `setAttribute`.
fn class_list_to_js_object(capture: ElementCapture, context: &mut Context) -> JsObject {
    let contains = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::from(false)) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            Ok(JsValue::from(class_list_read(&capture.0).iter().any(|c| c == &name)))
        },
        capture.clone(),
    );

    let add = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::undefined()) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            let mut classes = class_list_read(&capture.0);
            if !classes.iter().any(|c| c == &name) {
                classes.push(name);
            }
            class_list_write(&capture.0, &classes);
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let remove = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::undefined()) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            let mut classes = class_list_read(&capture.0);
            classes.retain(|c| c != &name);
            class_list_write(&capture.0, &classes);
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let toggle = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::from(false)) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            let mut classes = class_list_read(&capture.0);
            let currently_present = classes.iter().any(|c| c == &name);
            // Segundo argumento opcional `force`: si se pasa, decide de
            // verdad (fuerza añadir con true, quitar con false) en vez de
            // alternar - coercion generica de JS (`ToBoolean`), no
            // identidad estricta, porque asi es como el DOM real trata
            // `force` (a diferencia de `assert_true` del arnes de tests,
            // que si exige identidad estricta con `true`/`false` - son dos
            // APIs distintas con reglas distintas, no una inconsistencia).
            let target_present = match args.get(1) {
                Some(v) => v.to_boolean(),
                None => !currently_present,
            };
            if target_present && !currently_present {
                classes.push(name);
            } else if !target_present {
                classes.retain(|c| c != &name);
            }
            class_list_write(&capture.0, &classes);
            Ok(JsValue::from(target_present))
        },
        capture,
    );

    ObjectInitializer::new(context)
        .function(contains, js_string!("contains"), 1)
        .function(add, js_string!("add"), 1)
        .function(remove, js_string!("remove"), 1)
        .function(toggle, js_string!("toggle"), 2)
        .build()
}

fn class_list_read(node: &Arc<RwLock<Node>>) -> Vec<String> {
    let n = node.read().unwrap();
    match &n.node_type {
        NodeType::Element { attributes, .. } => attributes
            .get("class")
            .map(|c| c.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn class_list_write(node: &Arc<RwLock<Node>>, classes: &[String]) {
    let mut n = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut n.node_type {
        attributes.insert("class".to_string(), classes.join(" "));
    }
}

/// `el.style.getPropertyValue(...)`/`.setProperty(...)`/`.removeProperty(...)`/
/// `.cssText` y tres accessors por nombre (`color`/`backgroundColor`/
/// `fontSize`) viven todos sobre el mismo atributo `style` (parseado con
/// `CssParser::parse_inline_style`, ver `style_read`/`style_write` mas
/// abajo) que la cascada real ya aplica en el layout
/// (`layout::resolve_style`); mutan de verdad, igual que `classList` sobre
/// `class`. Los accessors por nombre son deliberadamente solo esos tres -
/// ver el aviso al principio del archivo para el porque exacto (son las
/// unicas que `layout`/`gfx` leen de verdad hoy). `style_property_get`/
/// `style_property_set` (mas abajo) comparten la logica entre
/// `getPropertyValue`/`setProperty` y los tres accessors, para no
/// duplicarla cuatro veces.
fn style_to_js_object(capture: ElementCapture, context: &mut Context) -> JsObject {
    let get_property_value = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::from(js_string!(""))) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            Ok(JsValue::from(js_string!(style_property_get(&capture.0, &name))))
        },
        capture.clone(),
    );

    let set_property = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(name_arg) = args.first() else { return Ok(JsValue::undefined()) };
            let name = name_arg.to_string(context)?.to_std_string_escaped();
            // Mismo criterio que `setAttribute` para un segundo argumento
            // ausente: `ToString(undefined)` = la cadena literal "undefined".
            let value = match args.get(1) {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => "undefined".to_string(),
            };
            style_property_set(&capture.0, &name, value);
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let remove_property = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let Some(arg) = args.first() else { return Ok(JsValue::from(js_string!(""))) };
            let name = arg.to_string(context)?.to_std_string_escaped();
            let mut declarations = style_read(&capture.0);
            // Spec real: removeProperty DEVUELVE el valor quitado (o "" si
            // la propiedad no estaba puesta), no undefined.
            let removed = declarations.remove(&name).unwrap_or_default();
            style_write(&capture.0, &declarations);
            Ok(JsValue::from(js_string!(removed)))
        },
        capture.clone(),
    );

    let css_text_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, _context| {
            Ok(JsValue::from(js_string!(serialize_style(&style_read(&capture.0)))))
        },
        capture.clone(),
    );
    let css_text_getter_fn = FunctionObjectBuilder::new(context.realm(), css_text_getter)
        .name(js_string!("get cssText"))
        .length(0)
        .constructor(false)
        .build();
    let css_text_setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let value = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => "undefined".to_string(),
            };
            style_write(&capture.0, &CssParser::parse_inline_style(&value));
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );
    let css_text_setter_fn = FunctionObjectBuilder::new(context.realm(), css_text_setter)
        .name(js_string!("set cssText"))
        .length(1)
        .constructor(false)
        .build();

    let color_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, _context| Ok(JsValue::from(js_string!(style_property_get(&capture.0, "color")))),
        capture.clone(),
    );
    let color_getter_fn = FunctionObjectBuilder::new(context.realm(), color_getter).name(js_string!("get color")).length(0).constructor(false).build();
    let color_setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let value = match args.first() { Some(v) => v.to_string(context)?.to_std_string_escaped(), None => "undefined".to_string() };
            style_property_set(&capture.0, "color", value);
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );
    let color_setter_fn = FunctionObjectBuilder::new(context.realm(), color_setter).name(js_string!("set color")).length(1).constructor(false).build();

    let background_color_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, _context| Ok(JsValue::from(js_string!(style_property_get(&capture.0, "background-color")))),
        capture.clone(),
    );
    let background_color_getter_fn = FunctionObjectBuilder::new(context.realm(), background_color_getter).name(js_string!("get backgroundColor")).length(0).constructor(false).build();
    let background_color_setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let value = match args.first() { Some(v) => v.to_string(context)?.to_std_string_escaped(), None => "undefined".to_string() };
            style_property_set(&capture.0, "background-color", value);
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );
    let background_color_setter_fn = FunctionObjectBuilder::new(context.realm(), background_color_setter).name(js_string!("set backgroundColor")).length(1).constructor(false).build();

    let font_size_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &ElementCapture, _context| Ok(JsValue::from(js_string!(style_property_get(&capture.0, "font-size")))),
        capture.clone(),
    );
    let font_size_getter_fn = FunctionObjectBuilder::new(context.realm(), font_size_getter).name(js_string!("get fontSize")).length(0).constructor(false).build();
    let font_size_setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &ElementCapture, context| {
            let value = match args.first() { Some(v) => v.to_string(context)?.to_std_string_escaped(), None => "undefined".to_string() };
            style_property_set(&capture.0, "font-size", value);
            Ok(JsValue::undefined())
        },
        capture,
    );
    let font_size_setter_fn = FunctionObjectBuilder::new(context.realm(), font_size_setter).name(js_string!("set fontSize")).length(1).constructor(false).build();

    ObjectInitializer::new(context)
        .function(get_property_value, js_string!("getPropertyValue"), 1)
        .function(set_property, js_string!("setProperty"), 2)
        .function(remove_property, js_string!("removeProperty"), 1)
        .accessor(js_string!("cssText"), Some(css_text_getter_fn), Some(css_text_setter_fn), Attribute::all())
        .accessor(js_string!("color"), Some(color_getter_fn), Some(color_setter_fn), Attribute::all())
        .accessor(js_string!("backgroundColor"), Some(background_color_getter_fn), Some(background_color_setter_fn), Attribute::all())
        .accessor(js_string!("fontSize"), Some(font_size_getter_fn), Some(font_size_setter_fn), Attribute::all())
        .build()
}

fn style_property_get(node: &Arc<RwLock<Node>>, kebab_name: &str) -> String {
    // `""` si no esta puesta, NUNCA `null` - asi es el spec real de
    // CSSStyleDeclaration.getPropertyValue (y de cada accessor por nombre),
    // a diferencia de `getAttribute`, que si devuelve `null`.
    style_read(node).get(kebab_name).cloned().unwrap_or_default()
}

fn style_property_set(node: &Arc<RwLock<Node>>, kebab_name: &str, value: String) {
    let mut declarations = style_read(node);
    if value.is_empty() {
        // Spec real: asignar "" (via setProperty o via un accessor por
        // nombre) quita la propiedad en vez de guardar un valor vacio.
        declarations.remove(kebab_name);
    } else {
        declarations.insert(kebab_name.to_string(), value);
    }
    style_write(node, &declarations);
}

fn style_read(node: &Arc<RwLock<Node>>) -> HashMap<String, String> {
    let n = node.read().unwrap();
    match &n.node_type {
        NodeType::Element { attributes, .. } => attributes
            .get("style")
            .map(|s| CssParser::parse_inline_style(s))
            .unwrap_or_default(),
        _ => HashMap::new(),
    }
}

fn serialize_style(declarations: &HashMap<String, String>) -> String {
    declarations.iter().map(|(k, v)| format!("{k}: {v};")).collect::<Vec<_>>().join(" ")
}

fn style_write(node: &Arc<RwLock<Node>>, declarations: &HashMap<String, String>) {
    let mut n = node.write().unwrap();
    if let NodeType::Element { attributes, .. } = &mut n.node_type {
        attributes.insert("style".to_string(), serialize_style(declarations));
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::JsRuntime;
    use engine_dom::{HtmlParser, Node};

    fn eval_with_dom(html: &str, script: &str) -> String {
        let dom = HtmlParser::parse(html);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom).expect("bind_dom no deberia fallar en estos tests");
        runtime.eval(script).expect("el script de test deberia ser JS valido")
    }

    /// La prueba real de esta tarea: el `JsRuntime` (y con el, los
    /// listeners registrados) sobreviven a la ejecucion inicial del script
    /// - no se dropean al terminar `eval`. Un listener registrado con
    /// `addEventListener` DESDE JS se dispara MAS TARDE desde Rust puro
    /// (`runtime.dispatch_event`, sin volver a pasar por texto JS), y su
    /// efecto (mutar `output` via `textContent`) es visible en un `eval`
    /// posterior sobre el MISMO runtime.
    #[test]
    fn js_runtime_dispatch_event_invokes_a_listener_registered_via_js_without_more_js_text() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target"></div><div id="output"></div></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom no deberia fallar");
        runtime
            .eval("document.getElementById('target').addEventListener('click', function() { document.getElementById('output').textContent = 'disparado desde rust'; });")
            .expect("registrar el listener deberia ser JS valido");

        let target = Node::find_by_id(&dom, "target").expect("target deberia existir en el DOM parseado");
        runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");

        let result = runtime.eval("document.getElementById('output').textContent").expect("leer el resultado deberia ser JS valido");
        assert_eq!(result, "\"disparado desde rust\"");
    }

    /// El punto real de la Fase 4.1: `dispatch_keyboard_event` (a
    /// diferencia de `dispatch_event`) deja `event.key` con el valor real
    /// de la tecla - antes de esta tarea, un evento disparado desde Rust
    /// (el unico camino real para teclado, ver `core::server::press_key`)
    /// llegaba SIEMPRE con `event.key === undefined`, sin importar que
    /// tecla fuera.
    #[test]
    fn dispatch_keyboard_event_populates_the_real_key_property() {
        let dom = HtmlParser::parse(r#"<html><body><input id="target"></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom no deberia fallar");
        runtime
            .eval("var teclaVista = null; document.getElementById('target').addEventListener('keydown', function(e) { teclaVista = e.key; });")
            .expect("registrar el listener deberia ser JS valido");

        let target = Node::find_by_id(&dom, "target").expect("target deberia existir");
        runtime.dispatch_keyboard_event(&target, "keydown", "Backspace").expect("dispatch_keyboard_event no deberia fallar");

        let result = runtime.eval("teclaVista").expect("leer el resultado deberia ser JS valido");
        assert_eq!(result, "\"Backspace\"");
    }

    /// El punto real de la Fase 4.2: `runtime.dispatch_event` (invocado
    /// desde Rust, no via `el.dispatchEvent(...)` en JS) devuelve si algun
    /// listener llamo `preventDefault()` - `core::server::click` lo usa
    /// para decidir si debe cancelar la navegacion por un `<a href>`.
    #[test]
    fn dispatch_event_returns_true_when_a_listener_calls_prevent_default() {
        let dom = HtmlParser::parse(r#"<html><body><a id="link" href="/otra">texto</a></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom no deberia fallar");
        runtime
            .eval("document.getElementById('link').addEventListener('click', function(e) { e.preventDefault(); });")
            .expect("registrar el listener deberia ser JS valido");

        let target = Node::find_by_id(&dom, "link").expect("link deberia existir");
        let prevented = runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");
        assert!(prevented, "un listener que llama preventDefault() deberia hacer que dispatch_event devuelva true");
    }

    #[test]
    fn dispatch_event_returns_false_when_no_listener_calls_prevent_default() {
        let dom = HtmlParser::parse(r#"<html><body><a id="link" href="/otra">texto</a></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom no deberia fallar");
        runtime
            .eval("document.getElementById('link').addEventListener('click', function() {});")
            .expect("registrar el listener deberia ser JS valido");

        let target = Node::find_by_id(&dom, "link").expect("link deberia existir");
        let prevented = runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");
        assert!(!prevented, "sin ningun preventDefault(), dispatch_event deberia devolver false");
    }

    /// `dispatch_event` normal (sin tecla) sigue dejando `.key` SIN poblar
    /// (`undefined`, no `""`/`null` inventados) - regresion: que
    /// `dispatch_keyboard_event` exista no deberia cambiar el
    /// comportamiento de la funcion original para clic/foco/etc.
    #[test]
    fn dispatch_event_without_a_key_leaves_the_key_property_undefined() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target"></div></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom no deberia fallar");
        runtime
            .eval("var teclaVista = 'sin_tocar'; document.getElementById('target').addEventListener('click', function(e) { teclaVista = e.key; });")
            .expect("registrar el listener deberia ser JS valido");

        let target = Node::find_by_id(&dom, "target").expect("target deberia existir");
        runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");

        let result = runtime.eval("teclaVista").expect("leer el resultado deberia ser JS valido");
        assert_eq!(result, "undefined");
    }

    #[test]
    fn js_runtime_dispatch_event_calls_all_matching_listeners_registered_via_js() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target"></div></body></html>"#);
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom.clone()).expect("bind_dom no deberia fallar");
        runtime
            .eval(
                "var contador = 0; \
                 var el = document.getElementById('target'); \
                 el.addEventListener('click', function() { contador = contador + 1; }); \
                 el.addEventListener('click', function() { contador = contador + 10; });",
            )
            .expect("registrar los listeners deberia ser JS valido");

        let target = Node::find_by_id(&dom, "target").expect("target deberia existir");
        runtime.dispatch_event(&target, "click").expect("dispatch_event no deberia fallar");

        let result = runtime.eval("contador").expect("leer contador deberia ser JS valido");
        assert_eq!(result, "11", "ambos listeners deberian haberse llamado");
    }

    #[test]
    fn js_runtime_dispatch_event_before_bind_dom_is_a_safe_no_op() {
        let dom = HtmlParser::parse(r#"<html><body><div id="target"></div></body></html>"#);
        let target = Node::find_by_id(&dom, "target").expect("target deberia existir");
        let mut runtime = JsRuntime::new();
        assert!(runtime.dispatch_event(&target, "click").is_ok(), "dispatch_event sin bind_dom no deberia fallar, solo no hacer nada");
    }

    #[test]
    fn get_element_by_id_reads_real_text_content() {
        let result = eval_with_dom(
            r#"<html><body><div id="target">hola mundo</div></body></html>"#,
            "document.getElementById('target').textContent",
        );
        assert_eq!(result, "\"hola mundo\"");
    }

    #[test]
    fn get_element_by_id_returns_null_when_nothing_matches() {
        let result = eval_with_dom(
            "<html><body><p>sin ids</p></body></html>",
            "document.getElementById('no-existe') === null",
        );
        assert_eq!(result, "true");
    }

    /// Prueba que querySelector usa de verdad el matching con combinadores
    /// del crate `selectors` y no un lookup simplón por tag: hay dos
    /// <span>, solo el de dentro de ".card" deberia encontrarse.
    #[test]
    fn query_selector_uses_real_selector_matching_not_just_tag_lookup() {
        let result = eval_with_dom(
            r#"<html><body><span>fuera</span><div class="card"><span>dentro</span></div></body></html>"#,
            "document.querySelector('.card span').textContent",
        );
        assert_eq!(result, "\"dentro\"");
    }

    #[test]
    fn query_selector_returns_null_for_no_match_or_unsupported_selector() {
        let no_match = eval_with_dom(
            "<html><body><p>hola</p></body></html>",
            "document.querySelector('.no-existe') === null",
        );
        assert_eq!(no_match, "true");

        let unsupported = eval_with_dom(
            "<html><body><p>hola</p></body></html>",
            "document.querySelector('p:hover') === null",
        );
        assert_eq!(unsupported, "true", "pseudo-clase no soportada: no deberia matchear por accidente");
    }

    #[test]
    fn get_attribute_reads_real_values_and_null_when_absent() {
        let href = eval_with_dom(
            r#"<html><body><a id="link" href="https://example.com">ir</a></body></html>"#,
            "document.getElementById('link').getAttribute('href')",
        );
        assert_eq!(href, "\"https://example.com\"");

        let missing = eval_with_dom(
            r#"<html><body><a id="link" href="https://example.com">ir</a></body></html>"#,
            "document.getElementById('link').getAttribute('title') === null",
        );
        assert_eq!(missing, "true");
    }

    #[test]
    fn tag_name_is_uppercase_per_dom_spec_convention() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "document.getElementById('d').tagName",
        );
        assert_eq!(result, "\"DIV\"");
    }

    #[test]
    fn query_selector_all_returns_a_real_array_with_every_match_in_document_order() {
        let result = eval_with_dom(
            r#"<html><body><p class="item">uno</p><span>fuera</span><p class="item">dos</p><p class="item">tres</p></body></html>"#,
            "Array.isArray(document.querySelectorAll('.item')) + ',' + document.querySelectorAll('.item').map(e => e.textContent).join('|')",
        );
        assert_eq!(result, "\"true,uno|dos|tres\"");
    }

    #[test]
    fn query_selector_all_returns_an_empty_array_when_nothing_matches() {
        let result = eval_with_dom(
            "<html><body><p>hola</p></body></html>",
            "document.querySelectorAll('.no-existe').length",
        );
        assert_eq!(result, "0");
    }

    #[test]
    fn set_attribute_mutates_the_underlying_node_so_get_attribute_sees_it_immediately() {
        let result = eval_with_dom(
            r#"<html><body><a id="link"></a></body></html>"#,
            "var el = document.getElementById('link'); el.setAttribute('href', 'https://example.com'); el.getAttribute('href')",
        );
        assert_eq!(result, "\"https://example.com\"");
    }

    /// Prueba que la mutacion es sobre el nodo real del arbol, no una copia
    /// local del primer objeto JS - dos llamadas SEPARADAS a
    /// getElementById para el mismo id deberian ver el mismo cambio.
    #[test]
    fn set_attribute_is_visible_through_a_separate_later_query_for_the_same_element() {
        let result = eval_with_dom(
            r#"<html><body><a id="link"></a></body></html>"#,
            "document.getElementById('link').setAttribute('href', 'https://example.com'); document.getElementById('link').getAttribute('href')",
        );
        assert_eq!(result, "\"https://example.com\"");
    }

    #[test]
    fn set_attribute_overwrites_an_existing_value() {
        let result = eval_with_dom(
            r#"<html><body><a id="link" href="https://old.example">x</a></body></html>"#,
            "document.getElementById('link').setAttribute('href', 'https://new.example'); document.getElementById('link').getAttribute('href')",
        );
        assert_eq!(result, "\"https://new.example\"");
    }

    #[test]
    fn set_attribute_without_a_value_coerces_undefined_to_its_string_form_like_real_js() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "document.getElementById('d').setAttribute('data-x'); document.getElementById('d').getAttribute('data-x')",
        );
        assert_eq!(result, "\"undefined\"");
    }

    #[test]
    fn text_content_setter_mutates_the_node_and_the_getter_sees_it_immediately() {
        let result = eval_with_dom(
            r#"<html><body><p id="p">original</p></body></html>"#,
            "var el = document.getElementById('p'); el.textContent = 'nuevo'; el.textContent",
        );
        assert_eq!(result, "\"nuevo\"");
    }

    /// `textContent` reemplaza TODOS los hijos, no solo el texto directo -
    /// un <p> con hijos anidados deberia perderlos por completo al asignar.
    #[test]
    fn text_content_setter_replaces_all_existing_children_not_just_appends() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"><span>a</span><span>b</span></div></body></html>"#,
            "var el = document.getElementById('d'); el.textContent = 'reemplazado'; el.textContent",
        );
        assert_eq!(result, "\"reemplazado\"");
    }

    /// Misma prueba de "es el nodo real, no una copia" que ya se hizo para
    /// setAttribute, ahora para textContent.
    #[test]
    fn text_content_setter_is_visible_through_a_separate_later_query_for_the_same_element() {
        let result = eval_with_dom(
            r#"<html><body><p id="p">original</p></body></html>"#,
            "document.getElementById('p').textContent = 'nuevo'; document.getElementById('p').textContent",
        );
        assert_eq!(result, "\"nuevo\"");
    }

    #[test]
    fn text_content_setter_treats_null_as_empty_string_like_real_dom() {
        let result = eval_with_dom(
            r#"<html><body><p id="p">original</p></body></html>"#,
            "document.getElementById('p').textContent = null; document.getElementById('p').textContent",
        );
        assert_eq!(result, "\"\"", "null en textContent limpia el texto (spec real), no escribe la cadena 'null'");
    }

    #[test]
    fn text_content_setter_coerces_non_string_values_to_their_string_form() {
        let result = eval_with_dom(
            r#"<html><body><p id="p">original</p></body></html>"#,
            "document.getElementById('p').textContent = 42; document.getElementById('p').textContent",
        );
        assert_eq!(result, "\"42\"");
    }

    #[test]
    fn create_element_produces_a_detached_element_with_the_given_tag_name_uppercased() {
        let result = eval_with_dom("<html><body></body></html>", "document.createElement('span').tagName");
        assert_eq!(result, "\"SPAN\"");
    }

    /// El test de verdad para appendChild: crea un elemento, lo mueve por
    /// las APIs de mutacion ya probadas por separado (setAttribute,
    /// textContent), lo conecta con appendChild, y comprueba que es
    /// alcanzable desde una busqueda FRESCA por id desde la raiz del
    /// documento - no solo que la variable local de JS lo "recuerde".
    #[test]
    fn append_child_connects_a_created_element_to_the_real_tree() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"></div></body></html>"#,
            "var el = document.createElement('span'); \
             el.setAttribute('id', 'new-el'); \
             el.textContent = 'hola'; \
             document.getElementById('container').appendChild(el); \
             document.getElementById('new-el').textContent",
        );
        assert_eq!(result, "\"hola\"");
    }

    #[test]
    fn append_child_returns_the_appended_child_like_real_dom() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"></div></body></html>"#,
            "var el = document.createElement('p'); var returned = document.getElementById('container').appendChild(el); returned === el",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn append_child_ignores_a_non_element_argument_instead_of_crashing() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"></div></body></html>"#,
            "document.getElementById('container').appendChild('no soy un elemento'); 'no crash'",
        );
        assert_eq!(result, "\"no crash\"");
    }

    #[test]
    fn remove_child_disconnects_the_child_from_the_real_tree() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="hijo">chau</span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var hijo = document.getElementById('hijo'); \
             contenedor.removeChild(hijo); \
             document.getElementById('hijo') === null",
        );
        assert_eq!(result, "true", "tras removeChild, una busqueda fresca por id ya no deberia encontrar al hijo");
    }

    #[test]
    fn remove_child_returns_the_removed_node_like_real_dom() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="hijo"></span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var hijo = document.getElementById('hijo'); \
             var devuelto = contenedor.removeChild(hijo); \
             devuelto === hijo",
        );
        assert_eq!(result, "true");
    }

    /// Quitar un nodo que NO es hijo del elemento sobre el que se llama
    /// removeChild deberia ser un no-op, no romper el arbol ni crashear -
    /// el DOM real lanzaria NotFoundError, aqui es una simplificacion
    /// honesta documentada.
    #[test]
    fn remove_child_on_a_node_that_is_not_actually_a_child_is_a_safe_no_op() {
        let result = eval_with_dom(
            r#"<html><body><div id="a"></div><div id="b"><span id="ajeno"></span></div></body></html>"#,
            "var a = document.getElementById('a'); \
             var ajeno = document.getElementById('ajeno'); \
             var resultado = a.removeChild(ajeno); \
             (resultado === null) + ',' + (document.getElementById('ajeno') !== null)",
        );
        assert_eq!(result, "\"true,true\"", "removeChild deberia devolver null y 'ajeno' deberia seguir en el arbol, sin tocar");
    }

    #[test]
    fn remove_child_ignores_a_non_element_argument_instead_of_crashing() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"></div></body></html>"#,
            "document.getElementById('container').removeChild(42); 'no crash'",
        );
        assert_eq!(result, "\"no crash\"");
    }

    #[test]
    fn class_list_add_appends_to_the_real_class_attribute() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" class="a"></div></body></html>"#,
            "document.getElementById('d').classList.add('b'); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(result, "\"a b\"");
    }

    #[test]
    fn class_list_add_does_not_duplicate_an_already_present_class() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" class="a"></div></body></html>"#,
            "document.getElementById('d').classList.add('a'); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(result, "\"a\"");
    }

    #[test]
    fn class_list_remove_removes_only_the_named_class() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" class="a b c"></div></body></html>"#,
            "document.getElementById('d').classList.remove('b'); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(result, "\"a c\"");
    }

    #[test]
    fn class_list_contains_reflects_the_real_class_attribute() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" class="a b"></div></body></html>"#,
            "document.getElementById('d').classList.contains('a') + ',' + document.getElementById('d').classList.contains('z')",
        );
        assert_eq!(result, "\"true,false\"");
    }

    #[test]
    fn class_list_toggle_without_force_flips_presence_both_ways() {
        let removed = eval_with_dom(
            r#"<html><body><div id="d" class="a"></div></body></html>"#,
            "document.getElementById('d').classList.toggle('a'); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(removed, "\"\"", "toggle sobre una clase presente deberia quitarla");

        let added = eval_with_dom(
            r#"<html><body><div id="d" class="a"></div></body></html>"#,
            "document.getElementById('d').classList.toggle('b'); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(added, "\"a b\"", "toggle sobre una clase ausente deberia añadirla");
    }

    /// `force` usa coercion generica de JS (ToBoolean), no identidad
    /// estricta - `toggle('x', 1)` deberia forzar presencia igual que
    /// `toggle('x', true)`, porque `1` es "truthy".
    #[test]
    fn class_list_toggle_with_force_decides_regardless_of_current_state() {
        let forced_present = eval_with_dom(
            r#"<html><body><div id="d" class=""></div></body></html>"#,
            "document.getElementById('d').classList.toggle('a', 1); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(forced_present, "\"a\"");

        let forced_absent = eval_with_dom(
            r#"<html><body><div id="d" class="a"></div></body></html>"#,
            "document.getElementById('d').classList.toggle('a', false); document.getElementById('d').getAttribute('class')",
        );
        assert_eq!(forced_absent, "\"\"");
    }

    #[test]
    fn class_list_toggle_returns_whether_the_class_ends_up_present() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" class=""></div></body></html>"#,
            "document.getElementById('d').classList.toggle('a')",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn parent_element_returns_the_real_parent_via_a_fresh_query() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent"><span id="child"></span></div></body></html>"#,
            "document.getElementById('child').parentElement.getAttribute('id')",
        );
        assert_eq!(result, "\"parent\"");
    }

    /// `parentElement` da `null` en dos casos distintos, ambos probados
    /// aqui: la raiz del documento (su padre es un `NodeType::Document`,
    /// no un `Element` - igual que en un navegador real) y un nodo recien
    /// creado que todavia no se ha conectado con `appendChild`.
    #[test]
    fn parent_element_is_null_when_there_is_no_element_parent() {
        let document_element = eval_with_dom(
            "<html><body><p>hola</p></body></html>",
            "document.querySelector('html').parentElement === null",
        );
        assert_eq!(document_element, "true", "el padre de <html> es el Document, no un Element");

        let detached = eval_with_dom(
            "<html><body></body></html>",
            "document.createElement('div').parentElement === null",
        );
        assert_eq!(detached, "true", "un elemento recien creado y no conectado no tiene padre");
    }

    #[test]
    fn children_returns_only_element_children_not_text_nodes_in_document_order() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent">texto1<span>a</span>texto2<p>b</p></div></body></html>"#,
            "document.getElementById('parent').children.length + ',' + \
             document.getElementById('parent').children.map(c => c.tagName).join('|')",
        );
        assert_eq!(result, "\"2,SPAN|P\"", "los nodos de texto sueltos no deberian contar como children");
    }

    /// `.children` no es una foto: a diferencia de `querySelectorAll` (que
    /// congela la lista en el momento de la llamada), cada lectura vuelve a
    /// mirar el arbol real, asi que ve altas y bajas hechas justo antes.
    #[test]
    fn children_reflects_mutations_after_append_child_and_remove_child() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var antes = contenedor.children.length; \
             var nuevo = document.createElement('span'); \
             contenedor.appendChild(nuevo); \
             var trasAppend = contenedor.children.length; \
             contenedor.removeChild(nuevo); \
             var trasRemove = contenedor.children.length; \
             antes + ',' + trasAppend + ',' + trasRemove",
        );
        assert_eq!(result, "\"0,1,0\"");
    }

    #[test]
    fn style_get_property_value_of_an_absent_property_is_an_empty_string_not_null() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "document.getElementById('d').style.getPropertyValue('color') === ''",
        );
        assert_eq!(result, "true", "a diferencia de getAttribute, getPropertyValue nunca da null");
    }

    #[test]
    fn style_get_property_value_reads_the_real_inline_style_attribute() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" style="color: blue"></div></body></html>"#,
            "document.getElementById('d').style.getPropertyValue('color')",
        );
        assert_eq!(result, "\"blue\"");
    }

    #[test]
    fn style_set_property_is_visible_through_get_attribute_and_a_fresh_query() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "document.getElementById('d').style.setProperty('color', 'red'); \
             document.getElementById('d').getAttribute('style') + ',' + \
             document.getElementById('d').style.getPropertyValue('color')",
        );
        assert_eq!(result, "\"color: red;,red\"");
    }

    /// Spec real: `setProperty(nombre, '')` quita la propiedad en vez de
    /// dejarla puesta con un valor vacio.
    #[test]
    fn style_set_property_with_an_empty_value_removes_the_property_instead_of_storing_it() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" style="color: red"></div></body></html>"#,
            "document.getElementById('d').style.setProperty('color', ''); \
             document.getElementById('d').style.getPropertyValue('color') + ',' + \
             document.getElementById('d').getAttribute('style')",
        );
        assert_eq!(result, "\",\"", "getPropertyValue deberia dar '' y el atributo style deberia quedar vacio");
    }

    #[test]
    fn style_remove_property_removes_the_declaration_and_returns_its_old_value() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" style="color: red; font-size: 14px"></div></body></html>"#,
            "var removido = document.getElementById('d').style.removeProperty('color'); \
             removido + ',' + \
             document.getElementById('d').style.getPropertyValue('color') + ',' + \
             document.getElementById('d').style.getPropertyValue('font-size')",
        );
        assert_eq!(result, "\"red,,14px\"");
    }

    #[test]
    fn style_remove_property_of_an_absent_property_returns_an_empty_string() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "document.getElementById('d').style.removeProperty('color')",
        );
        assert_eq!(result, "\"\"");
    }

    #[test]
    fn style_css_text_getter_serializes_every_declaration() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" style="color: red"></div></body></html>"#,
            "document.getElementById('d').style.cssText",
        );
        assert_eq!(result, "\"color: red;\"");
    }

    #[test]
    fn style_css_text_setter_replaces_the_entire_declaration_block() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" style="color: red; font-size: 14px"></div></body></html>"#,
            "document.getElementById('d').style.cssText = 'background-color: blue'; \
             document.getElementById('d').style.getPropertyValue('color') + ',' + \
             document.getElementById('d').style.getPropertyValue('font-size') + ',' + \
             document.getElementById('d').style.getPropertyValue('background-color')",
        );
        assert_eq!(result, "\",,blue\"", "cssText deberia REEMPLAZAR todo el bloque, no fusionarlo con lo que ya habia");
    }

    #[test]
    fn style_color_background_color_and_font_size_accessors_read_and_write_the_real_attribute() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "var el = document.getElementById('d'); \
             el.style.color = 'blue'; \
             el.style.backgroundColor = 'yellow'; \
             el.style.fontSize = '20px'; \
             el.style.getPropertyValue('color') + ',' + \
             el.style.getPropertyValue('background-color') + ',' + \
             el.style.getPropertyValue('font-size') + ',' + \
             el.style.color + ',' + el.style.backgroundColor + ',' + el.style.fontSize",
        );
        assert_eq!(result, "\"blue,yellow,20px,blue,yellow,20px\"", "escribir via el accessor camelCase deberia verse via getPropertyValue con el nombre kebab-case - misma fuente, no una copia paralela");
    }

    #[test]
    fn style_named_accessor_set_to_empty_string_removes_the_property() {
        let result = eval_with_dom(
            r#"<html><body><div id="d" style="color: red"></div></body></html>"#,
            "document.getElementById('d').style.color = ''; \
             document.getElementById('d').style.color + ',' + \
             document.getElementById('d').getAttribute('style')",
        );
        assert_eq!(result, "\",\"", "igual que setProperty(nombre, ''), asignar '' via el accessor deberia quitar la propiedad");
    }

    /// Limite deliberado, no un bug: solo `color`/`backgroundColor`/
    /// `fontSize` tienen accessor por nombre (las unicas que `layout`/`gfx`
    /// leen de verdad, ver el aviso al principio del archivo). Cualquier
    /// otro nombre camelCase se convierte en una propiedad JS normal del
    /// objeto style, sin tocar el atributo real - `setProperty` en cambio
    /// SI acepta cualquier nombre, como el spec real. `var s = el.style`
    /// una sola vez, a proposito: cada LECTURA de `el.style` construye un
    /// objeto nuevo (misma caveat de identidad que `classList`/`children`/
    /// etc, ver el aviso al principio del archivo), asi que comparar dos
    /// accesos `el.style.margin` distintos compararia dos objetos DISTINTOS
    /// y daria `undefined` sin probar nada sobre si `margin` esta conectado
    /// o no al atributo real - detectado por este mismo test en su primera
    /// version, que asumia (mal) que `el.style` era una referencia estable.
    #[test]
    fn an_unrecognized_camelcase_style_property_does_not_touch_the_real_attribute() {
        let result = eval_with_dom(
            r#"<html><body><div id="d"></div></body></html>"#,
            "var el = document.getElementById('d'); \
             var s = el.style; \
             s.margin = '10px'; \
             s.margin + ',' + (el.getAttribute('style') === null)",
        );
        assert_eq!(result, "\"10px,true\"", "el.style.margin deberia comportarse como una propiedad JS normal (se lee de vuelta desde la MISMA referencia de objeto style), pero NUNCA deberia tocar el atributo style real");
    }

    #[test]
    fn insert_before_inserts_the_new_node_immediately_before_the_reference_node() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span><span id="b"></span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var nuevo = document.createElement('p'); \
             contenedor.insertBefore(nuevo, document.getElementById('b')); \
             contenedor.children.map(c => c.tagName).join('|')",
        );
        assert_eq!(result, "\"SPAN|P|SPAN\"");
    }

    /// `referencia` null (o el argumento ausente) equivale a appendChild -
    /// inserta al final - igual que el spec real.
    #[test]
    fn insert_before_with_a_null_or_absent_reference_appends_at_the_end() {
        let with_null = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var nuevo = document.createElement('p'); \
             contenedor.insertBefore(nuevo, null); \
             contenedor.children.map(c => c.tagName).join('|')",
        );
        assert_eq!(with_null, "\"SPAN|P\"");

        let with_absent = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var nuevo = document.createElement('p'); \
             contenedor.insertBefore(nuevo); \
             contenedor.children.map(c => c.tagName).join('|')",
        );
        assert_eq!(with_absent, "\"SPAN|P\"");
    }

    /// Un nodo solo puede tener un padre a la vez: moverlo con
    /// insertBefore deberia quitarlo de verdad de la lista children de su
    /// padre anterior, no solo añadirlo al nuevo.
    #[test]
    fn insert_before_moves_a_node_that_already_had_a_different_parent() {
        let result = eval_with_dom(
            r#"<html><body><div id="origen"><span id="movido"></span></div><div id="destino"></div></body></html>"#,
            "var origen = document.getElementById('origen'); \
             var destino = document.getElementById('destino'); \
             destino.insertBefore(document.getElementById('movido'), null); \
             origen.children.length + ',' + destino.children.length",
        );
        assert_eq!(result, "\"0,1\"", "el nodo movido no deberia seguir en la lista de children del padre viejo");
    }

    #[test]
    fn insert_before_is_a_no_op_when_the_reference_node_is_not_actually_a_child() {
        let result = eval_with_dom(
            r#"<html><body><div id="a"></div><div id="b"><span id="ajeno"></span></div></body></html>"#,
            "var a = document.getElementById('a'); \
             var ajeno = document.getElementById('ajeno'); \
             var nuevo = document.createElement('p'); \
             var resultado = a.insertBefore(nuevo, ajeno); \
             (resultado === null) + ',' + a.children.length",
        );
        assert_eq!(result, "\"true,0\"", "insertBefore no deberia insertar nada si la referencia no es hijo real");
    }

    #[test]
    fn replace_child_replaces_the_old_node_with_the_new_one_in_the_same_position() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span><span id="b"></span><span id="c"></span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var nuevo = document.createElement('p'); \
             contenedor.replaceChild(nuevo, document.getElementById('b')); \
             contenedor.children.map(c => c.tagName).join('|')",
        );
        assert_eq!(result, "\"SPAN|P|SPAN\"");
    }

    #[test]
    fn replace_child_returns_the_replaced_node_like_real_dom() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="viejo"></span></div></body></html>"#,
            "var contenedor = document.getElementById('container'); \
             var viejo = document.getElementById('viejo'); \
             var nuevo = document.createElement('p'); \
             var devuelto = contenedor.replaceChild(nuevo, viejo); \
             devuelto === viejo",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn replace_child_is_a_no_op_when_the_old_child_is_not_actually_a_child() {
        let result = eval_with_dom(
            r#"<html><body><div id="a"></div><div id="b"><span id="ajeno"></span></div></body></html>"#,
            "var a = document.getElementById('a'); \
             var ajeno = document.getElementById('ajeno'); \
             var nuevo = document.createElement('p'); \
             var resultado = a.replaceChild(nuevo, ajeno); \
             (resultado === null) + ',' + (document.getElementById('ajeno') !== null)",
        );
        assert_eq!(result, "\"true,true\"", "replaceChild no deberia tocar nada si el viejo no es hijo real de este elemento");
    }

    /// El gap que insertBefore/replaceChild obligaron a arreglar: antes,
    /// appendChild sobre un nodo que ya tenia padre lo dejaba fantasma en
    /// la lista children de su padre viejo ademas de en la del nuevo.
    #[test]
    fn append_child_moves_a_node_removing_it_from_its_old_parents_children() {
        let result = eval_with_dom(
            r#"<html><body><div id="origen"><span id="movido"></span></div><div id="destino"></div></body></html>"#,
            "var origen = document.getElementById('origen'); \
             var destino = document.getElementById('destino'); \
             destino.appendChild(document.getElementById('movido')); \
             origen.children.length + ',' + destino.children.length",
        );
        assert_eq!(result, "\"0,1\"");
    }

    #[test]
    fn new_event_constructor_sets_the_type_property() {
        let result = eval_with_dom("<html><body></body></html>", "new Event('click').type");
        assert_eq!(result, "\"click\"");
    }

    #[test]
    fn add_event_listener_and_dispatch_event_invoke_the_listener_for_real() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var contador = 0; \
             document.getElementById('target').addEventListener('click', function() { contador = contador + 1; }); \
             document.getElementById('target').dispatchEvent(new Event('click')); \
             contador",
        );
        assert_eq!(result, "1");
    }

    #[test]
    fn dispatch_event_with_no_listeners_registered_is_a_safe_no_op() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "document.getElementById('target').dispatchEvent(new Event('click'))",
        );
        assert_eq!(result, "true", "sin listeners registrados, dispatchEvent no deberia reventar y deberia devolver true");
    }

    #[test]
    fn multiple_listeners_of_the_same_type_are_all_called_in_registration_order() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var log = ''; \
             var el = document.getElementById('target'); \
             el.addEventListener('click', function() { log = log + 'a'; }); \
             el.addEventListener('click', function() { log = log + 'b'; }); \
             el.dispatchEvent(new Event('click')); \
             log",
        );
        assert_eq!(result, "\"ab\"");
    }

    /// `JsObject` compara por identidad (`Gc::ptr_eq`), no por contenido:
    /// dos funciones con codigo IDENTICO pero declaradas por separado no
    /// deberian matchear - solo la referencia exacta que se registro.
    #[test]
    fn remove_event_listener_removes_only_the_exact_listener_by_identity() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var log = ''; \
             var el = document.getElementById('target'); \
             function listenerA() { log = log + 'a'; } \
             function listenerB() { log = log + 'b'; } \
             el.addEventListener('click', listenerA); \
             el.addEventListener('click', listenerB); \
             el.removeEventListener('click', listenerA); \
             el.dispatchEvent(new Event('click')); \
             log",
        );
        assert_eq!(result, "\"b\"", "listenerA se quito, listenerB deberia seguir activo");
    }

    #[test]
    fn dispatch_event_passes_the_real_event_object_to_the_listener() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var recibido = null; \
             var el = document.getElementById('target'); \
             el.addEventListener('miTipo', function(evento) { recibido = evento.type; }); \
             el.dispatchEvent(new Event('miTipo')); \
             recibido",
        );
        assert_eq!(result, "\"miTipo\"");
    }

    #[test]
    fn dispatch_event_calls_the_listener_with_this_bound_to_the_target_element() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var mismoElemento = false; \
             var el = document.getElementById('target'); \
             el.addEventListener('click', function() { mismoElemento = (this === el); }); \
             el.dispatchEvent(new Event('click')); \
             mismoElemento",
        );
        assert_eq!(result, "true", "dentro del listener, this deberia ser el mismo elemento sobre el que se llamo dispatchEvent");
    }

    /// La prueba real de que el registro es por NODO real, no por objeto JS
    /// envoltorio (que se reconstruye nuevo en cada consulta, ver el aviso
    /// de `ElementCapture`): addEventListener via una consulta,
    /// dispatchEvent via una consulta SEPARADA al mismo id.
    #[test]
    fn add_event_listener_and_dispatch_event_work_across_separate_get_element_by_id_queries() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div><div id="output"></div></body></html>"#,
            "document.getElementById('target').addEventListener('click', function() { \
                 document.getElementById('output').textContent = 'disparado'; \
             }); \
             document.getElementById('target').dispatchEvent(new Event('click')); \
             document.getElementById('output').textContent",
        );
        assert_eq!(result, "\"disparado\"");
    }

    #[test]
    fn document_document_element_returns_the_real_html_element() {
        let result = eval_with_dom("<html><body><p>hola</p></body></html>", "document.documentElement.tagName");
        assert_eq!(result, "\"HTML\"");
    }

    #[test]
    fn document_body_returns_the_real_body_element() {
        let result = eval_with_dom("<html><body><p>hola</p></body></html>", "document.body.tagName");
        assert_eq!(result, "\"BODY\"");
    }

    /// Hallazgo real (no asumido - la primera version de este test asumia
    /// lo contrario y fallo): `html5ever`, al ser un parser HTML5 real que
    /// sigue el algoritmo de construccion de arbol del spec, INSERTA un
    /// `<body>` automaticamente incluso para `<html></html>` vacio - igual
    /// que hace un navegador real. Asi que `document.body` no deberia dar
    /// `null` para HTML "vacio" real, aunque no se haya escrito la
    /// etiqueta - solo daria `null` para un documento sin ningun `<body>`
    /// en absoluto, algo que este parser en la practica no produce.
    #[test]
    fn document_body_is_auto_inserted_by_html5ever_even_for_empty_html() {
        let result = eval_with_dom("<html></html>", "document.body !== null && document.body.tagName");
        assert_eq!(result, "\"BODY\"", "html5ever deberia sintetizar un <body> real, igual que un navegador real");
    }

    /// Prueba que `document.body` es vivo (el mismo `Arc<RwLock<Node>>` que
    /// cualquier otra consulta), no una foto congelada: mutar via
    /// `document.body` deberia verse a traves de una consulta COMPLETAMENTE
    /// distinta (`querySelector('body')`) al mismo elemento.
    #[test]
    fn document_body_is_live_visible_through_a_separate_query() {
        let result = eval_with_dom(
            "<html><body><p>hola</p></body></html>",
            "document.body.setAttribute('data-x', 'y'); document.querySelector('body').getAttribute('data-x')",
        );
        assert_eq!(result, "\"y\"");
    }

    #[test]
    fn first_element_child_skips_text_nodes_and_returns_the_first_element() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent">texto suelto<span>a</span><p>b</p></div></body></html>"#,
            "document.getElementById('parent').firstElementChild.tagName",
        );
        assert_eq!(result, "\"SPAN\"");
    }

    #[test]
    fn last_element_child_skips_text_nodes_and_returns_the_last_element() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent"><span>a</span><p>b</p>texto suelto</div></body></html>"#,
            "document.getElementById('parent').lastElementChild.tagName",
        );
        assert_eq!(result, "\"P\"");
    }

    #[test]
    fn first_element_child_is_null_when_there_are_no_element_children() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent">solo texto, sin ningun elemento</div></body></html>"#,
            "document.getElementById('parent').firstElementChild === null",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn next_element_sibling_skips_text_nodes_and_returns_the_next_element() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span>texto suelto<p id="b"></p></div></body></html>"#,
            "document.getElementById('a').nextElementSibling.tagName",
        );
        assert_eq!(result, "\"P\"", "deberia saltar el nodo de texto entre a y b");
    }

    #[test]
    fn previous_element_sibling_skips_text_nodes_and_returns_the_previous_element() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span>texto suelto<p id="b"></p></div></body></html>"#,
            "document.getElementById('b').previousElementSibling.tagName",
        );
        assert_eq!(result, "\"SPAN\"");
    }

    #[test]
    fn next_element_sibling_is_null_for_the_last_element_child() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span></div></body></html>"#,
            "document.getElementById('a').nextElementSibling === null",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn previous_element_sibling_is_null_for_the_first_element_child() {
        let result = eval_with_dom(
            r#"<html><body><div id="container"><span id="a"></span></div></body></html>"#,
            "document.getElementById('a').previousElementSibling === null",
        );
        assert_eq!(result, "true");
    }

    /// Un nodo recien creado con `createElement` (todavia sin padre) no
    /// deberia reventar al pedirle un hermano - simplemente no tiene.
    #[test]
    fn next_element_sibling_is_null_for_a_detached_node() {
        let result = eval_with_dom(
            "<html><body></body></html>",
            "document.createElement('div').nextElementSibling === null",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn dispatch_event_without_prevent_default_returns_true() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var el = document.getElementById('target'); \
             el.addEventListener('click', function() {}); \
             el.dispatchEvent(new Event('click'))",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn prevent_default_makes_dispatch_event_return_false_when_cancelable() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var el = document.getElementById('target'); \
             el.addEventListener('click', function(e) { e.preventDefault(); }); \
             el.dispatchEvent(new Event('click', {cancelable: true}))",
        );
        assert_eq!(result, "false");
    }

    #[test]
    fn prevent_default_is_a_no_op_when_the_event_is_not_cancelable() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var el = document.getElementById('target'); \
             el.addEventListener('click', function(e) { e.preventDefault(); }); \
             el.dispatchEvent(new Event('click'))",
        );
        assert_eq!(result, "true", "sin {{cancelable: true}}, preventDefault no deberia tener efecto - igual que el spec real");
    }

    /// La prueba real de que hay bubbling de verdad, no solo disparo sobre
    /// el nodo exacto: un listener registrado en un ANCESTRO se llama
    /// cuando el evento se dispara sobre un descendiente - requiere
    /// `{bubbles: true}` explicito (ver
    /// `new_event_defaults_bubbles_and_cancelable_to_false` para el porque).
    #[test]
    fn bubbling_calls_an_ancestors_listener_when_dispatched_on_a_descendant() {
        let result = eval_with_dom(
            r#"<html><body><div id="grandparent"><div id="parent"><span id="child"></span></div></div></body></html>"#,
            "var fired = false; \
             document.getElementById('grandparent').addEventListener('click', function() { fired = true; }); \
             document.getElementById('child').dispatchEvent(new Event('click', {bubbles: true})); \
             fired",
        );
        assert_eq!(result, "true");
    }

    #[test]
    fn a_non_bubbling_event_never_reaches_an_ancestors_listener() {
        let result = eval_with_dom(
            r#"<html><body><div id="grandparent"><div id="parent"><span id="child"></span></div></div></body></html>"#,
            "var fired = false; \
             document.getElementById('grandparent').addEventListener('click', function() { fired = true; }); \
             document.getElementById('child').dispatchEvent(new Event('click')); \
             fired",
        );
        assert_eq!(result, "false", "sin {{bubbles: true}}, dispatchEvent no deberia burbujear - igual que el spec real");
    }

    #[test]
    fn stop_propagation_stops_bubbling_before_reaching_the_grandparent() {
        let result = eval_with_dom(
            r#"<html><body><div id="grandparent"><div id="parent"><span id="child"></span></div></div></body></html>"#,
            "var grandparentFired = false; \
             document.getElementById('parent').addEventListener('click', function(e) { e.stopPropagation(); }); \
             document.getElementById('grandparent').addEventListener('click', function() { grandparentFired = true; }); \
             document.getElementById('child').dispatchEvent(new Event('click', {bubbles: true})); \
             grandparentFired",
        );
        assert_eq!(result, "false", "stopPropagation en el padre deberia impedir que el abuelo se entere");
    }

    /// `currentTarget`-like: dentro de un listener puesto en un ANCESTRO
    /// (no el target original), `this` deberia ser ese ancestro, no el
    /// elemento sobre el que se llamo dispatchEvent.
    #[test]
    fn this_inside_a_bubbled_listener_is_the_ancestor_it_is_registered_on() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent"><span id="child"></span></div></body></html>"#,
            "var parentIsThis = false; \
             var parent = document.getElementById('parent'); \
             parent.addEventListener('click', function() { parentIsThis = (this.getAttribute('id') === 'parent'); }); \
             document.getElementById('child').dispatchEvent(new Event('click', {bubbles: true})); \
             parentIsThis",
        );
        assert_eq!(result, "true");
    }

    /// A diferencia de `this` (que cambia por nivel, ver el test anterior),
    /// `event.target` deberia seguir siendo el nodo ORIGINAL sobre el que
    /// se llamo dispatchEvent en TODOS los niveles del burbujeo, incluido
    /// dentro de un listener puesto en un ancestro.
    #[test]
    fn event_target_stays_the_original_node_throughout_bubbling() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent"><span id="child"></span></div></body></html>"#,
            "var targetIdSeenByParent = null; \
             document.getElementById('parent').addEventListener('click', function(e) { \
                 targetIdSeenByParent = e.target.getAttribute('id'); \
             }); \
             document.getElementById('child').dispatchEvent(new Event('click', {bubbles: true})); \
             targetIdSeenByParent",
        );
        assert_eq!(result, "\"child\"");
    }

    #[test]
    fn new_event_defaults_bubbles_and_cancelable_to_false() {
        let result = eval_with_dom(
            "<html><body></body></html>",
            "var e = new Event('click'); e.bubbles + ',' + e.cancelable",
        );
        assert_eq!(result, "\"false,false\"", "igual que el spec real: EventInit.bubbles/cancelable son false por defecto");
    }

    #[test]
    fn new_event_reads_bubbles_and_cancelable_from_the_options_object() {
        let result = eval_with_dom(
            "<html><body></body></html>",
            "var e = new Event('click', {bubbles: true, cancelable: true}); e.bubbles + ',' + e.cancelable",
        );
        assert_eq!(result, "\"true,true\"");
    }

    /// La prueba real de que la fase de captura existe: un listener
    /// `{capture: true}` en un ANCESTRO se llama ANTES que el listener
    /// (sin captura) puesto en el propio target - orden capturado en un
    /// log compartido, no solo "ambos se llamaron".
    #[test]
    fn a_capture_listener_on_an_ancestor_fires_before_the_targets_own_listener() {
        let result = eval_with_dom(
            r#"<html><body><div id="grandparent"><div id="parent"><span id="child"></span></div></div></body></html>"#,
            "var log = ''; \
             document.getElementById('grandparent').addEventListener('click', function() { log += 'captura,'; }, {capture: true}); \
             document.getElementById('child').addEventListener('click', function() { log += 'target'; }); \
             document.getElementById('child').dispatchEvent(new Event('click')); \
             log",
        );
        assert_eq!(result, "\"captura,target\"");
    }

    /// Punto del spec facil de pasar por alto: la fase de captura NO
    /// depende de `.bubbles` - solo la fase de burbujeo (la ultima) lo
    /// hace. Un listener de captura en un ancestro se entera de un evento
    /// que NUNCA va a burbujear.
    #[test]
    fn a_capture_listener_fires_even_when_the_event_does_not_bubble() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent"><span id="child"></span></div></body></html>"#,
            "var capturado = false; \
             document.getElementById('parent').addEventListener('click', function() { capturado = true; }, {capture: true}); \
             document.getElementById('child').dispatchEvent(new Event('click')); \
             capturado",
        );
        assert_eq!(result, "true", "sin {{bubbles: true}} el evento no deberia burbujear - pero SI deberia haber pasado por la fase de captura de camino al target");
    }

    /// En el target, TODOS los listeners se llaman sin importar su flag de
    /// captura - la distincion captura/burbujeo solo aplica a ANCESTROS,
    /// nunca al target mismo.
    #[test]
    fn target_phase_calls_both_capture_and_non_capture_listeners_registered_on_the_target() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var log = ''; \
             var el = document.getElementById('target'); \
             el.addEventListener('click', function() { log += 'a'; }, {capture: true}); \
             el.addEventListener('click', function() { log += 'b'; }); \
             el.dispatchEvent(new Event('click')); \
             log",
        );
        assert_eq!(result, "\"ab\"");
    }

    #[test]
    fn stop_propagation_during_capture_prevents_the_target_listener_from_running() {
        let result = eval_with_dom(
            r#"<html><body><div id="parent"><span id="child"></span></div></body></html>"#,
            "var targetFired = false; \
             document.getElementById('parent').addEventListener('click', function(e) { e.stopPropagation(); }, {capture: true}); \
             document.getElementById('child').addEventListener('click', function() { targetFired = true; }); \
             document.getElementById('child').dispatchEvent(new Event('click')); \
             targetFired",
        );
        assert_eq!(result, "false", "stopPropagation durante la fase de captura deberia impedir que la fase de target siquiera corra");
    }

    /// La MISMA funcion registrada dos veces - una con captura, otra sin -
    /// son dos entradas DISTINTAS: quitar la de captura no debe tocar la
    /// que no la tiene.
    #[test]
    fn remove_event_listener_only_removes_the_entry_matching_the_capture_flag() {
        let result = eval_with_dom(
            r#"<html><body><div id="target"></div></body></html>"#,
            "var log = ''; \
             var el = document.getElementById('target'); \
             function listener() { log += 'x'; } \
             el.addEventListener('click', listener, {capture: true}); \
             el.addEventListener('click', listener); \
             el.removeEventListener('click', listener, {capture: true}); \
             el.dispatchEvent(new Event('click')); \
             log",
        );
        assert_eq!(result, "\"x\"", "solo deberia quedar UNA invocacion (la registrada sin captura) tras quitar la de captura");
    }

    #[test]
    fn add_event_listener_accepts_a_legacy_boolean_third_argument_for_capture() {
        let result = eval_with_dom(
            r#"<html><body><div id="grandparent"><div id="parent"><span id="child"></span></div></div></body></html>"#,
            "var log = ''; \
             document.getElementById('grandparent').addEventListener('click', function() { log += 'captura,'; }, true); \
             document.getElementById('child').addEventListener('click', function() { log += 'target'; }); \
             document.getElementById('child').dispatchEvent(new Event('click')); \
             log",
        );
        assert_eq!(result, "\"captura,target\"", "un tercer argumento booleano suelto (forma legado useCapture) deberia comportarse igual que {{capture: true}}");
    }
}
