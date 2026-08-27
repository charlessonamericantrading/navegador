//! `MutationObserver`: avisar a JS de que el DOM ha cambiado.
//!
//! Lo que hace especial a esta API frente al resto de bindings es CUANDO
//! entrega: no en el momento de la mutacion, sino al terminar la tarea
//! actual, con todas las mutaciones agrupadas. Eso es exactamente lo que el
//! spec llama el "microtask checkpoint", y es lo que permite que un bucle
//! que añade cien nodos produzca UNA llamada al callback y no cien.
//!
//! Aqui se implementa con las dos piezas que ya existian:
//!
//! - `DocumentBindings` gana un REGISTRO DE MUTACIONES que cada funcion que
//!   toca el DOM (`setAttribute`, `appendChild`, `textContent`...) rellena.
//! - `JsRuntime`, que ya vaciaba la cola de trabajos de Boa despues de cada
//!   evaluacion y cada evento, entrega ademas ese registro.
//!
//! **Lo que NO cubre**: `attributeOldValue`/`characterDataOldValue` (habria
//! que guardar el valor anterior en cada mutacion) y `attributeFilter`. Un
//! observador que los pida recibe igualmente sus registros, solo que sin el
//! valor viejo - se declara aqui en vez de fingirlo.

use crate::dom_bindings::{element_to_js_object, DocumentBindings};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsObject, JsResult, JsValue, NativeFunction};
use engine_dom::Node;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Una mutacion ya ocurrida, esperando a que se entregue.
#[derive(Clone)]
pub struct PendingMutation {
    /// `"attributes"`, `"childList"` o `"characterData"` - los mismos
    /// nombres que expone `MutationRecord.type`.
    pub kind: &'static str,
    pub target: Arc<RwLock<Node>>,
    pub attribute_name: Option<String>,
    pub added: Vec<Arc<RwLock<Node>>>,
    pub removed: Vec<Arc<RwLock<Node>>>,
}

pub type MutationLog = Arc<Mutex<Vec<PendingMutation>>>;

/// Un observador registrado, con lo que pidio observar.
pub struct Observer {
    id: u64,
    callback: JsObject,
    observer_object: JsObject,
    targets: Vec<ObservedTarget>,
    /// Registros que le corresponden y todavia no se han entregado - los que
    /// devuelve `takeRecords()`.
    queued: Vec<PendingMutation>,
}

struct ObservedTarget {
    node: Arc<RwLock<Node>>,
    subtree: bool,
    attributes: bool,
    child_list: bool,
    character_data: bool,
}

pub type ObserverRegistry = Arc<Mutex<Vec<Observer>>>;

fn next_observer_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct RegistryCapture(ObserverRegistry, u64);

unsafe impl boa_gc::Trace for RegistryCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for RegistryCapture {}

/// `true` si `candidate` es el propio `root` o desciende de el.
///
/// Se sube por los enlaces al padre en vez de bajar recorriendo el subarbol:
/// la profundidad de un DOM es de decenas, su tamaño de decenas de miles.
fn is_self_or_descendant(candidate: &Arc<RwLock<Node>>, root: &Arc<RwLock<Node>>) -> bool {
    let mut actual = Some(candidate.clone());
    // Tope de profundidad por si un arbol quedara con un ciclo de padres:
    // colgarse aqui seria peor que perder un registro.
    for _ in 0..512 {
        let Some(nodo) = actual else { return false };
        if Arc::ptr_eq(&nodo, root) {
            return true;
        }
        actual = nodo.read().unwrap().parent.as_ref().and_then(|p| p.upgrade());
    }
    false
}

fn observa(target: &ObservedTarget, mutation: &PendingMutation) -> bool {
    let interesa = match mutation.kind {
        "attributes" => target.attributes,
        "childList" => target.child_list,
        "characterData" => target.character_data,
        _ => false,
    };
    if !interesa {
        return false;
    }
    if Arc::ptr_eq(&target.node, &mutation.target) {
        return true;
    }
    target.subtree && is_self_or_descendant(&mutation.target, &target.node)
}

/// Registra el constructor global `MutationObserver` y devuelve el registro
/// de observadores, que `JsRuntime` necesita para entregar.
pub fn register_mutation_observer(context: &mut Context, bindings: DocumentBindings) -> JsResult<ObserverRegistry> {
    let registry: ObserverRegistry = Arc::new(Mutex::new(Vec::new()));

    let constructor_registry = registry.clone();
    let constructor_bindings = bindings.clone();
    let constructor = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, capture: &ConstructorCapture, context| {
            let Some(callback) = args.first().and_then(JsValue::as_callable).cloned() else {
                return Err(boa_engine::JsNativeError::typ()
                    .with_message("MutationObserver necesita una funcion de callback")
                    .into());
            };
            let id = next_observer_id();

            let observe = NativeFunction::from_copy_closure_with_captures(
                |_this, args, capture: &ObserveCapture, context| {
                    let Some(target_value) = args.first() else { return Ok(JsValue::undefined()) };
                    let Some(node) = crate::dom_bindings::node_from_js_value(target_value) else {
                        return Ok(JsValue::undefined());
                    };
                    let opciones = args.get(1).and_then(|v| v.as_object().cloned());
                    let bandera = |nombre: &str, context: &mut Context| -> bool {
                        opciones
                            .as_ref()
                            .and_then(|o| o.get(js_string!(nombre.to_string()), context).ok())
                            .map(|v| v.to_boolean())
                            .unwrap_or(false)
                    };
                    let child_list = bandera("childList", context);
                    let attributes = bandera("attributes", context);
                    let character_data = bandera("characterData", context);
                    let subtree = bandera("subtree", context);
                    // Sin ninguna bandera util el spec lanza TypeError; aqui
                    // se ignora en silencio, que es lo mismo que hacer un
                    // observador que nunca dispara pero sin romper la pagina.
                    let mut registro = capture.0.lock().unwrap();
                    if let Some(obs) = registro.iter_mut().find(|o| o.id == capture.1) {
                        obs.targets.push(ObservedTarget { node, subtree, attributes, child_list, character_data });
                    }
                    Ok(JsValue::undefined())
                },
                RegistryCapture(capture.0.clone(), id),
            );

            let disconnect = NativeFunction::from_copy_closure_with_captures(
                |_this, _args, capture: &ObserveCapture, _context| {
                    let mut registro = capture.0.lock().unwrap();
                    if let Some(obs) = registro.iter_mut().find(|o| o.id == capture.1) {
                        obs.targets.clear();
                        obs.queued.clear();
                    }
                    Ok(JsValue::undefined())
                },
                RegistryCapture(capture.0.clone(), id),
            );

            let take_records = NativeFunction::from_copy_closure_with_captures(
                |_this, _args, capture: &TakeCapture, context| {
                    let pendientes = {
                        let mut registro = capture.0.lock().unwrap();
                        match registro.iter_mut().find(|o| o.id == capture.1) {
                            Some(obs) => std::mem::take(&mut obs.queued),
                            None => Vec::new(),
                        }
                    };
                    let arreglo = build_records_array(&pendientes, &capture.2, context)?;
                    Ok(arreglo.into())
                },
                TakeCapture(capture.0.clone(), id, capture.1.clone()),
            );

            let observer_object = ObjectInitializer::new(context)
                .function(observe, js_string!("observe"), 2)
                .function(disconnect, js_string!("disconnect"), 0)
                .function(take_records, js_string!("takeRecords"), 0)
                .build();

            capture.0.lock().unwrap().push(Observer {
                id,
                callback: callback.into(),
                observer_object: observer_object.clone(),
                targets: Vec::new(),
                queued: Vec::new(),
            });

            Ok(observer_object.into())
        },
        ConstructorCapture(constructor_registry, constructor_bindings),
    );

    let constructor_fn = FunctionObjectBuilder::new(context.realm(), constructor)
        .name(js_string!("MutationObserver"))
        .length(1)
        .constructor(true)
        .build();
    context.register_global_property(js_string!("MutationObserver"), constructor_fn, Attribute::all())?;

    Ok(registry)
}

#[derive(Clone)]
struct ConstructorCapture(ObserverRegistry, DocumentBindings);

unsafe impl boa_gc::Trace for ConstructorCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for ConstructorCapture {}

type ObserveCapture = RegistryCapture;

#[derive(Clone)]
struct TakeCapture(ObserverRegistry, u64, DocumentBindings);

unsafe impl boa_gc::Trace for TakeCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for TakeCapture {}

/// Construye el array de `MutationRecord` que recibe el callback.
fn build_records_array(records: &[PendingMutation], bindings: &DocumentBindings, context: &mut Context) -> JsResult<JsObject> {
    let arreglo = boa_engine::object::builtins::JsArray::new(context);
    for record in records {
        let nodos = |lista: &[Arc<RwLock<Node>>], context: &mut Context| -> JsResult<JsObject> {
            let a = boa_engine::object::builtins::JsArray::new(context);
            for n in lista {
                let obj = element_to_js_object(n, bindings, context);
                a.push(JsValue::from(obj), context)?;
            }
            Ok(a.into())
        };
        let added = nodos(&record.added, context)?;
        let removed = nodos(&record.removed, context)?;
        let target = element_to_js_object(&record.target, bindings, context);
        let attribute_name = match &record.attribute_name {
            Some(nombre) => JsValue::from(js_string!(nombre.as_str())),
            None => JsValue::null(),
        };
        let objeto = ObjectInitializer::new(context)
            .property(js_string!("type"), js_string!(record.kind), Attribute::all())
            .property(js_string!("target"), target, Attribute::all())
            .property(js_string!("attributeName"), attribute_name, Attribute::all())
            .property(js_string!("addedNodes"), added, Attribute::all())
            .property(js_string!("removedNodes"), removed, Attribute::all())
            .build();
        arreglo.push(JsValue::from(objeto), context)?;
    }
    Ok(arreglo.into())
}

/// Reparte las mutaciones acumuladas entre los observadores interesados y
/// llama a sus callbacks. Se invoca en el "microtask checkpoint": al
/// terminar cada evaluacion y cada evento, no en cada mutacion.
///
/// Devuelve cuantos callbacks se llegaron a invocar.
pub fn deliver_mutations(registry: &ObserverRegistry, log: &MutationLog, bindings: &DocumentBindings, context: &mut Context) -> usize {
    let mutaciones: Vec<PendingMutation> = std::mem::take(&mut *log.lock().unwrap());
    if mutaciones.is_empty() {
        return 0;
    }

    // Se decide FUERA del lock a quien le toca que: llamar a un callback JS
    // puede a su vez mutar el DOM (y volver a tocar este registro), asi que
    // mantener el lock cogido durante la llamada seria un bloqueo seguro.
    let mut reparto: Vec<(u64, JsObject, JsObject, Vec<PendingMutation>)> = Vec::new();
    {
        let mut registro = registry.lock().unwrap();
        for obs in registro.iter_mut() {
            let suyas: Vec<PendingMutation> =
                mutaciones.iter().filter(|m| obs.targets.iter().any(|t| observa(t, m))).cloned().collect();
            if suyas.is_empty() {
                continue;
            }
            obs.queued.extend(suyas.iter().cloned());
            reparto.push((obs.id, obs.callback.clone(), obs.observer_object.clone(), suyas));
        }
    }

    let mut llamados = 0;
    for (id, callback, observer_object, suyas) in reparto {
        // Entregar tambien vacia la cola: `takeRecords()` solo debe devolver
        // lo que AUN no se ha entregado.
        {
            let mut registro = registry.lock().unwrap();
            if let Some(obs) = registro.iter_mut().find(|o| o.id == id) {
                obs.queued.clear();
            }
        }
        let Ok(arreglo) = build_records_array(&suyas, bindings, context) else { continue };
        if !callback.is_callable() {
            continue;
        }
        if let Err(e) = callback.call(&JsValue::undefined(), &[arreglo.into(), observer_object.into()], context) {
            tracing::warn!("[js] el callback de un MutationObserver fallo: {e}");
        }
        llamados += 1;
    }
    llamados
}
