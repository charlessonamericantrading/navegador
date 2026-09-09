//! Los globales `localStorage` y `sessionStorage` (Web Storage API).
//!
//! El almacen de datos en si vive en `engine_net::storage::WebStorage`,
//! compartido por toda la sesion del navegador y con alcance por ORIGEN
//! (ver alli); este modulo solo lo expone a JavaScript. La separacion es la
//! misma que con las cookies: el estado del navegador no puede vivir en la
//! pagina, porque tiene que sobrevivir a navegar a otra.
//!
//! Cada `Context` recibe el origen de SU pagina al registrarse, asi que un
//! script no puede pedir el almacenamiento de otro origen aunque lo
//! intente: no hay ningun parametro con el que hacerlo.
//!
//! ## Acceso por propiedad
//!
//! `localStorage.tema` y `localStorage.getItem("tema")` son equivalentes -
//! el objeto `Storage` es "exotico" (`Proxy` real de Boa, ver
//! `boa_engine::object::builtins::JsProxy`) y atrapa cualquier acceso a
//! propiedad que NO sea uno de los metodos/`length` ya definidos en el
//! objeto base: `get`/`set`/`has`/`deleteProperty` comprueban primero si
//! el TARGET (el objeto con `getItem`/`setItem`/`removeItem`/`clear`/
//! `key`/`length`) ya tiene esa propiedad - si la tiene, se comporta
//! exactamente igual que antes; si no, la trampa trata el nombre de
//! propiedad como una CLAVE de `WebStorage`. `set` es la excepcion: SIEMPRE
//! escribe via `setItem`, incluso si el nombre coincide con un metodo
//! (`localStorage.getItem = 'x'` guarda un dato llamado "getItem", no
//! sobrescribe el metodo) - es el comportamiento real del spec, y ademas
//! mas simple de implementar que comprobar la colision primero.
//!
//! Las trampas son punteros a funcion SIN estado propio (`NativeFunctionPointer`,
//! el tipo que exige `JsProxyBuilder` - a diferencia de los metodos de
//! abajo, no pueden capturar un closure), asi que el `StorageCapture` de
//! CADA `Storage` (local/session) se cuelga como DATOS NATIVOS del target
//! (`ObjectInitializer::with_native_data`) y las trampas lo recuperan del
//! primer argumento que ya les pasa Boa (`target`, ver `capture_from_target`)
//! - mismo patron que `ElementCapture` en `dom_bindings.rs`.
//!
//! Los simbolos (`Symbol.toPrimitive`, `Symbol.iterator`...) nunca son
//! claves de almacenamiento - las trampas los delegan al comportamiento
//! normal del target en vez de intentar convertirlos a cadena (que
//! lanzaria `TypeError`, igual que en JS puro).

use boa_engine::object::builtins::JsProxy;
use boa_engine::object::{JsData, ObjectInitializer};
use boa_engine::property::{Attribute, PropertyKey};
use boa_engine::{js_string, Context, JsError, JsNativeError, JsObject, JsResult, JsValue, NativeFunction};
use engine_net::storage::{StorageKind, WebStorage};
use std::sync::{Arc, Mutex};

/// El almacen compartido de toda la sesion - lo crea y conserva
/// `core::server`, y se lo presta a cada pagina que carga.
pub type SharedWebStorage = Arc<Mutex<WebStorage>>;

/// Lo que cada funcion nativa necesita capturar: el almacen compartido,
/// que area de las dos es, y el origen de ESTA pagina.
#[derive(Clone)]
struct StorageCapture {
    storage: SharedWebStorage,
    kind: StorageKind,
    origin: String,
}

unsafe impl boa_gc::Trace for StorageCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for StorageCapture {}

/// Cuerpo vacio le basta, igual que `ElementCapture` en `dom_bindings.rs` -
/// `NativeObject` se consigue gratis via su impl generica.
impl JsData for StorageCapture {}

/// Recupera el `StorageCapture` que `build_storage_object` colgo del
/// TARGET (no del Proxy en si) como datos nativos - `None` si `target` no
/// es un objeto `Storage` de este motor. Los campos se clonan de uno en
/// uno (no la guarda entera) para no arriesgar clonar el `Ref` en vez del
/// `StorageCapture` que envuelve.
fn capture_from_target(target: &JsObject) -> Option<StorageCapture> {
    let guard = target.downcast_ref::<StorageCapture>()?;
    Some(StorageCapture { storage: guard.storage.clone(), kind: guard.kind, origin: guard.origin.clone() })
}

/// El nombre de propiedad de una trampa de Proxy como cadena Rust, o
/// `None` si es un simbolo (nunca una clave de almacenamiento).
fn property_as_string(property: &JsValue) -> Option<String> {
    property.as_string().map(|s| s.to_std_string_escaped())
}

/// La clave de propiedad de una trampa de Proxy convertida al tipo que
/// exigen los metodos de `JsObject` (`get`/`set`/`has_own_property`...) -
/// una cadena o un simbolo, nunca otra cosa (asi construye Boa el
/// argumento `property` de cualquier trampa real).
fn property_key(property: &JsValue) -> Option<PropertyKey> {
    if let Some(s) = property.as_string() {
        return Some(PropertyKey::from(s.clone()));
    }
    property.as_symbol().map(|sym| PropertyKey::from(sym.clone()))
}

/// Trampa `get` (ver el aviso del modulo): metodos/`length` del target si
/// ya existen ahi, si no una lectura de `WebStorage` por esa clave -
/// `null` (nunca `undefined`) para una clave sin dato, igual que
/// `getItem`.
fn proxy_get(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(target) = args.first().and_then(JsValue::as_object) else { return Ok(JsValue::undefined()) };
    let property = args.get(1).cloned().unwrap_or_default();

    let Some(key) = property_as_string(&property) else {
        let Some(pk) = property_key(&property) else { return Ok(JsValue::undefined()) };
        return target.get(pk, context);
    };

    if target.has_own_property(js_string!(key.clone()), context)? {
        return target.get(js_string!(key), context);
    }

    // `undefined`, NO `null`: a diferencia de `getItem` (un metodo con
    // contrato propio de "devuelve null si no hay"), esto es un acceso de
    // PROPIEDAD normal - una propiedad que no existe en ningun objeto JS
    // lee como `undefined`, igual que en un navegador real
    // (`localStorage.getItem('x')` es `null`, pero `localStorage.x` es
    // `undefined` si `x` no esta guardado).
    let Some(capture) = capture_from_target(target) else { return Ok(JsValue::undefined()) };
    let Ok(store) = capture.storage.lock() else { return Ok(JsValue::undefined()) };
    Ok(match store.get_item(capture.kind, &capture.origin, &key) {
        Some(value) => js_string!(value).into(),
        None => JsValue::undefined(),
    })
}

/// Trampa `set` (ver el aviso del modulo): SIEMPRE escribe via `WebStorage::
/// set_item`, sin comprobar si el nombre coincide con un metodo - es el
/// comportamiento real del spec (`localStorage.getItem = 'x'` guarda un
/// dato, no sobrescribe el metodo). `QuotaExceeded` se propaga como
/// excepcion real, igual que `setItem`.
fn proxy_set(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(target) = args.first().and_then(JsValue::as_object) else { return Ok(JsValue::from(false)) };
    let property = args.get(1).cloned().unwrap_or_default();
    let value = args.get(2).cloned().unwrap_or_default();

    let Some(key) = property_as_string(&property) else {
        let Some(pk) = property_key(&property) else { return Ok(JsValue::from(false)) };
        return Ok(JsValue::from(target.set(pk, value, false, context)?));
    };

    let Some(capture) = capture_from_target(target) else { return Ok(JsValue::from(false)) };
    let value_str = to_storage_string(Some(&value), context)?;
    let Ok(mut store) = capture.storage.lock() else { return Ok(JsValue::from(false)) };
    match store.set_item(capture.kind, &capture.origin, &key, &value_str) {
        Ok(()) => Ok(JsValue::from(true)),
        Err(_) => Err(JsError::from_native(
            JsNativeError::error().with_message("QuotaExceededError: se supero la cuota de almacenamiento de este origen"),
        )),
    }
}

/// Trampa `has` (`'tema' in localStorage`, y lo que usa `for...in`): SI el
/// target ya la tiene (metodo/`length`), o si hay un dato guardado con esa
/// clave.
fn proxy_has(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(target) = args.first().and_then(JsValue::as_object) else { return Ok(JsValue::from(false)) };
    let property = args.get(1).cloned().unwrap_or_default();

    let Some(key) = property_as_string(&property) else {
        let Some(pk) = property_key(&property) else { return Ok(JsValue::from(false)) };
        return Ok(JsValue::from(target.has_property(pk, context)?));
    };

    if target.has_own_property(js_string!(key.clone()), context)? {
        return Ok(JsValue::from(true));
    }
    let Some(capture) = capture_from_target(target) else { return Ok(JsValue::from(false)) };
    let Ok(store) = capture.storage.lock() else { return Ok(JsValue::from(false)) };
    Ok(JsValue::from(store.get_item(capture.kind, &capture.origin, &key).is_some()))
}

/// Trampa `deleteProperty` (`delete localStorage.tema`): borra la clave de
/// `WebStorage`. Los propios metodos/`length` del target NO se pueden
/// borrar (se ignora, `false`) - igual que un navegador real, donde son
/// propiedades no configurables.
fn proxy_delete_property(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(target) = args.first().and_then(JsValue::as_object) else { return Ok(JsValue::from(true)) };
    let property = args.get(1).cloned().unwrap_or_default();

    let Some(key) = property_as_string(&property) else { return Ok(JsValue::from(true)) };

    if target.has_own_property(js_string!(key.clone()), context)? {
        return Ok(JsValue::from(false));
    }
    let Some(capture) = capture_from_target(target) else { return Ok(JsValue::from(true)) };
    if let Ok(mut store) = capture.storage.lock() {
        store.remove_item(capture.kind, &capture.origin, &key);
    }
    Ok(JsValue::from(true))
}

/// Convierte un argumento cualquiera a la cadena que el spec guarda:
/// `setItem('n', 42)` almacena `"42"`, y `getItem(42)` busca la clave
/// `"42"`. Los valores de Web Storage son SIEMPRE cadenas - una diferencia
/// observable que el codigo real comprueba (`typeof v === 'string'`).
fn to_storage_string(value: Option<&JsValue>, context: &mut Context) -> JsResult<String> {
    Ok(value.cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped())
}

/// Construye el objeto `Storage` de un area y un origen concretos.
fn build_storage_object(context: &mut Context, storage: SharedWebStorage, kind: StorageKind, origin: String) -> JsResult<boa_engine::JsObject> {
    let capture = StorageCapture { storage, kind, origin };

    let get_item = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], cap: &StorageCapture, context| {
            let key = to_storage_string(args.first(), context)?;
            let Ok(store) = cap.storage.lock() else { return Ok(JsValue::null()) };
            Ok(match store.get_item(cap.kind, &cap.origin, &key) {
                Some(value) => js_string!(value).into(),
                // `null`, NO `undefined`: el codigo real comprueba
                // `=== null` para distinguir "no hay" de "hay vacio".
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );

    let set_item = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], cap: &StorageCapture, context| {
            let key = to_storage_string(args.first(), context)?;
            let value = to_storage_string(args.get(1), context)?;
            let Ok(mut store) = cap.storage.lock() else { return Ok(JsValue::undefined()) };
            match store.set_item(cap.kind, &cap.origin, &key, &value) {
                Ok(()) => Ok(JsValue::undefined()),
                // Un `QuotaExceededError` de verdad, que es lo que una
                // pagina real captura para degradar con elegancia cuando
                // el almacenamiento esta lleno.
                Err(_) => Err(JsError::from_native(
                    JsNativeError::error().with_message("QuotaExceededError: se supero la cuota de almacenamiento de este origen"),
                )),
            }
        },
        capture.clone(),
    );

    let remove_item = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], cap: &StorageCapture, context| {
            let key = to_storage_string(args.first(), context)?;
            if let Ok(mut store) = cap.storage.lock() {
                store.remove_item(cap.kind, &cap.origin, &key);
            }
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let clear = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], cap: &StorageCapture, _context| {
            if let Ok(mut store) = cap.storage.lock() {
                store.clear(cap.kind, &cap.origin);
            }
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let key_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], cap: &StorageCapture, context| {
            let index = args.first().cloned().unwrap_or_default().to_number(context)?;
            if !index.is_finite() || index < 0.0 {
                return Ok(JsValue::null());
            }
            let Ok(store) = cap.storage.lock() else { return Ok(JsValue::null()) };
            Ok(match store.key_at(cap.kind, &cap.origin, index as usize) {
                Some(key) => js_string!(key).into(),
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );

    // `length` es una PROPIEDAD de solo lectura, no un metodo - se lee sin
    // parentesis (`localStorage.length`), igual que en un navegador real.
    let length_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], cap: &StorageCapture, _context| {
            let Ok(store) = cap.storage.lock() else { return Ok(JsValue::from(0)) };
            Ok(JsValue::from(store.length(cap.kind, &cap.origin) as u32))
        },
        capture.clone(),
    );
    let length_getter_fn = boa_engine::object::FunctionObjectBuilder::new(context.realm(), length_getter)
        .name(js_string!("get length"))
        .length(0)
        .constructor(false)
        .build();

    // `with_native_data` (no `new`): cuelga `capture` del TARGET en si -
    // las trampas del Proxy de abajo lo recuperan de ahi (ver
    // `capture_from_target`), porque son punteros a funcion sin closure
    // propio.
    let target = ObjectInitializer::with_native_data(capture, context)
        .function(get_item, js_string!("getItem"), 1)
        .function(set_item, js_string!("setItem"), 2)
        .function(remove_item, js_string!("removeItem"), 1)
        .function(clear, js_string!("clear"), 0)
        .function(key_fn, js_string!("key"), 1)
        .accessor(js_string!("length"), Some(length_getter_fn), None, Attribute::all())
        .build();

    // El Proxy (ver el aviso del modulo) es lo que hace que
    // `localStorage.tema` funcione ademas de `localStorage.getItem
    // ('tema')` - sin el, esto seguiria siendo el objeto de metodos de
    // siempre.
    Ok(JsProxy::builder(target)
        .get(proxy_get)
        .set(proxy_set)
        .has(proxy_has)
        .delete_property(proxy_delete_property)
        .build(context)
        .into())
}

/// Registra `localStorage` y `sessionStorage` para el origen dado.
///
/// `origin` sale de `engine_net::storage::origin_of` sobre la URL de la
/// pagina - quien registra (`core::scripting`) es el unico que la conoce.
/// Una pagina sin origen util (documento construido en memoria, sin URL)
/// recibe igualmente los globales, apuntando a un origen propio y aislado:
/// asi el codigo que los usa no revienta con `ReferenceError`, que es lo
/// que mas dano hace.
pub fn register_storage(context: &mut Context, storage: SharedWebStorage, origin: String) -> JsResult<()> {
    let local = build_storage_object(context, storage.clone(), StorageKind::Local, origin.clone())?;
    let session = build_storage_object(context, storage, StorageKind::Session, origin)?;

    context.register_global_property(js_string!("localStorage"), local, Attribute::all())?;
    context.register_global_property(js_string!("sessionStorage"), session, Attribute::all())?;

    // Tambien colgados de `window` si ya existe - mucho codigo real
    // escribe `window.localStorage`. Guardado igual que hacen `crate::
    // window` y `crate::timers`: el orden de registro no esta garantizado.
    context.eval(boa_engine::Source::from_bytes(
        b"if (typeof window !== 'undefined') { window.localStorage = localStorage; window.sessionStorage = sessionStorage; }" as &[u8],
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::JsRuntime;

    fn runtime_at(origin: &str) -> JsRuntime {
        let mut runtime = JsRuntime::new();
        runtime
            .register_storage(Arc::new(Mutex::new(WebStorage::new())), origin.to_string())
            .expect("el almacenamiento deberia registrarse");
        runtime
    }

    /// Dos runtimes (dos "paginas") que comparten el MISMO almacen, como
    /// pasa de verdad al navegar de una pagina a otra.
    fn two_runtimes_sharing_storage(origin_a: &str, origin_b: &str) -> (JsRuntime, JsRuntime) {
        let shared: SharedWebStorage = Arc::new(Mutex::new(WebStorage::new()));
        let mut a = JsRuntime::new();
        a.register_storage(shared.clone(), origin_a.to_string()).unwrap();
        let mut b = JsRuntime::new();
        b.register_storage(shared, origin_b.to_string()).unwrap();
        (a, b)
    }

    #[test]
    fn set_and_get_round_trip_from_javascript() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.setItem('tema', 'oscuro')").unwrap();
        assert_eq!(r.eval("localStorage.getItem('tema')").unwrap(), "\"oscuro\"");
    }

    #[test]
    fn a_missing_key_reads_as_null_not_undefined() {
        let mut r = runtime_at("https://a.test");
        assert_eq!(r.eval("localStorage.getItem('noexiste') === null").unwrap(), "true");
    }

    /// Los valores de Web Storage son SIEMPRE cadenas - una pagina real
    /// comprueba esto al deserializar (`JSON.parse(localStorage.getItem(..))`).
    #[test]
    fn values_are_always_coerced_to_strings() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.setItem('n', 42)").unwrap();
        assert_eq!(r.eval("localStorage.getItem('n')").unwrap(), "\"42\"");
        assert_eq!(r.eval("typeof localStorage.getItem('n')").unwrap(), "\"string\"");
    }

    #[test]
    fn length_and_key_expose_the_stored_entries_in_order() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.setItem('a', '1'); localStorage.setItem('b', '2');").unwrap();
        assert_eq!(r.eval("localStorage.length").unwrap(), "2");
        assert_eq!(r.eval("localStorage.key(0)").unwrap(), "\"a\"");
        assert_eq!(r.eval("localStorage.key(1)").unwrap(), "\"b\"");
        assert_eq!(r.eval("localStorage.key(9) === null").unwrap(), "true", "fuera de rango deberia ser null, no una excepcion");
    }

    #[test]
    fn remove_item_and_clear_work_from_javascript() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.setItem('a','1'); localStorage.setItem('b','2'); localStorage.removeItem('a');").unwrap();
        assert_eq!(r.eval("localStorage.getItem('a') === null").unwrap(), "true");
        assert_eq!(r.eval("localStorage.length").unwrap(), "1");
        r.eval("localStorage.clear()").unwrap();
        assert_eq!(r.eval("localStorage.length").unwrap(), "0");
    }

    #[test]
    fn local_and_session_storage_do_not_see_each_other() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.setItem('k','de-local'); sessionStorage.setItem('k','de-sesion');").unwrap();
        assert_eq!(r.eval("localStorage.getItem('k')").unwrap(), "\"de-local\"");
        assert_eq!(r.eval("sessionStorage.getItem('k')").unwrap(), "\"de-sesion\"");
    }

    /// Lo que hace util a `localStorage`: sobrevive a cambiar de pagina
    /// dentro del mismo origen.
    #[test]
    fn storage_survives_navigating_to_another_page_of_the_same_origin() {
        let (mut primera, mut segunda) = two_runtimes_sharing_storage("https://a.test", "https://a.test");
        primera.eval("localStorage.setItem('sesion', 'abc123')").unwrap();
        assert_eq!(
            segunda.eval("localStorage.getItem('sesion')").unwrap(),
            "\"abc123\"",
            "una pagina posterior del mismo origen deberia ver lo que guardo la anterior"
        );
    }

    /// El aislamiento que de verdad importa para la seguridad.
    #[test]
    fn another_origin_cannot_read_this_origins_storage() {
        let (mut a, mut b) = two_runtimes_sharing_storage("https://a.test", "https://b.test");
        a.eval("localStorage.setItem('secreto', '1234')").unwrap();
        assert_eq!(
            b.eval("localStorage.getItem('secreto') === null").unwrap(),
            "true",
            "otro origen NO deberia poder leer este almacenamiento"
        );
    }

    #[test]
    fn exceeding_the_quota_throws_a_catchable_error_instead_of_failing_silently() {
        let mut r = runtime_at("https://a.test");
        let script = r#"
            var enorme = 'x'.repeat(6 * 1024 * 1024);
            var lanzo = false;
            try { localStorage.setItem('k', enorme); } catch (e) { lanzo = true; }
            lanzo
        "#;
        assert_eq!(r.eval(script).unwrap(), "true", "pasarse de cuota deberia lanzar algo capturable, no fallar en silencio");
    }

    /// Sin `register_storage`, los globales no existen - mismo criterio
    /// honesto que `fetch`/`window`/`setTimeout`.
    #[test]
    fn storage_is_not_defined_at_all_unless_it_was_registered() {
        let mut r = JsRuntime::new();
        assert_eq!(r.eval("typeof localStorage").unwrap(), "\"undefined\"");
    }

    /// El punto real del Proxy: `localStorage.tema` y `localStorage.getItem
    /// ('tema')` tienen que ser equivalentes, en las dos direcciones.
    #[test]
    fn property_access_is_equivalent_to_get_item_and_set_item() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.tema = 'oscuro'").unwrap();
        assert_eq!(r.eval("localStorage.getItem('tema')").unwrap(), "\"oscuro\"", "escribir por propiedad deberia verse via getItem");
        assert_eq!(r.eval("localStorage.tema").unwrap(), "\"oscuro\"", "leer por propiedad deberia devolver lo mismo");

        r.eval("localStorage.setItem('otro', 'valor')").unwrap();
        assert_eq!(r.eval("localStorage.otro").unwrap(), "\"valor\"", "escribir por setItem deberia verse por propiedad");
    }

    /// Una clave sin dato leida por propiedad es `undefined` (no `null`) -
    /// asi se distingue de una propiedad de METODO que si existe (`getItem`
    /// sigue siendo una funcion, no una clave de almacenamiento).
    #[test]
    fn a_missing_property_key_is_undefined_and_methods_still_work_as_methods() {
        let mut r = runtime_at("https://a.test");
        assert_eq!(r.eval("localStorage.noexiste").unwrap(), "undefined");
        assert_eq!(r.eval("typeof localStorage.getItem").unwrap(), "\"function\"", "getItem deberia seguir siendo la funcion, no una clave de almacenamiento");
        assert_eq!(r.eval("localStorage.length").unwrap(), "0", "length deberia seguir siendo la propiedad de siempre");
    }

    /// Escribir con un nombre que coincide con un metodo NO deberia
    /// sobrescribirlo - se comporta como cualquier otra clave, que es el
    /// comportamiento real del spec (`Storage` es un "legacy platform
    /// object": el setter SIEMPRE llama a `setItem`, incluso si el nombre
    /// choca con un metodo).
    #[test]
    fn setting_a_property_that_shadows_a_method_name_does_not_overwrite_the_method() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.getItem = 'no soy una funcion'").unwrap();
        assert_eq!(r.eval("typeof localStorage.getItem").unwrap(), "\"function\"", "getItem deberia seguir siendo la funcion real");
    }

    /// `'clave' in localStorage` (el operador `in`, lo que usa `for...in`
    /// por debajo) tiene que ver tanto los datos guardados como los
    /// propios metodos.
    #[test]
    fn the_in_operator_sees_both_stored_keys_and_the_built_in_methods() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.tema = 'oscuro'").unwrap();
        assert_eq!(r.eval("'tema' in localStorage").unwrap(), "true");
        assert_eq!(r.eval("'getItem' in localStorage").unwrap(), "true");
        assert_eq!(r.eval("'noexiste' in localStorage").unwrap(), "false");
    }

    /// `delete localStorage.clave` tiene que borrar el dato de verdad
    /// (equivalente a `removeItem`), sin poder borrar los propios metodos.
    #[test]
    fn delete_removes_a_stored_key_but_not_the_built_in_methods() {
        let mut r = runtime_at("https://a.test");
        r.eval("localStorage.tema = 'oscuro'; delete localStorage.tema;").unwrap();
        assert_eq!(r.eval("localStorage.tema").unwrap(), "undefined", "delete deberia haber borrado el dato");

        r.eval("delete localStorage.getItem").unwrap();
        assert_eq!(r.eval("typeof localStorage.getItem").unwrap(), "\"function\"", "delete no deberia poder quitar un metodo real");
    }

    /// El acceso por propiedad tambien tiene que respetar el aislamiento
    /// por origen y por area (local vs sesion) - no es un camino nuevo de
    /// almacenamiento, es el MISMO `WebStorage` de siempre visto por otra
    /// puerta.
    #[test]
    fn property_access_respects_origin_isolation_and_the_local_session_split() {
        let (mut a, mut b) = two_runtimes_sharing_storage("https://a.test", "https://b.test");
        a.eval("localStorage.secreto = '1234'").unwrap();
        assert_eq!(b.eval("localStorage.secreto").unwrap(), "undefined", "otro origen no deberia ver esto ni por propiedad");

        let mut r = runtime_at("https://c.test");
        r.eval("localStorage.k = 'de-local'; sessionStorage.k = 'de-sesion';").unwrap();
        assert_eq!(r.eval("localStorage.k").unwrap(), "\"de-local\"");
        assert_eq!(r.eval("sessionStorage.k").unwrap(), "\"de-sesion\"");
    }
}
