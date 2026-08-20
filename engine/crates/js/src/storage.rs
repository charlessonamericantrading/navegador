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
//! ## Lo que NO soporta: el acceso por propiedad
//!
//! En un navegador real, `localStorage.tema` y `localStorage.getItem
//! ("tema")` son equivalentes: el objeto `Storage` es "exotico" y atrapa
//! cualquier acceso a propiedad. Aqui SOLO funcionan los metodos
//! (`getItem`/`setItem`/`removeItem`/`clear`/`key`) y `length`.
//! Implementar la forma con punto exigiria un objeto con manejadores
//! propios de `[[Get]]`/`[[Set]]`/`[[Delete]]`/`[[OwnPropertyKeys]]` en
//! Boa - trabajo aparte y de bastante mas superficie. La forma con metodos
//! es ademas la que recomienda MDN y la que usa la mayoria del codigo
//! real, asi que la perdida es acotada; se declara aqui en vez de fingir
//! una API completa.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction};
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
        capture,
    );
    let length_getter_fn = boa_engine::object::FunctionObjectBuilder::new(context.realm(), length_getter)
        .name(js_string!("get length"))
        .length(0)
        .constructor(false)
        .build();

    Ok(ObjectInitializer::new(context)
        .function(get_item, js_string!("getItem"), 1)
        .function(set_item, js_string!("setItem"), 2)
        .function(remove_item, js_string!("removeItem"), 1)
        .function(clear, js_string!("clear"), 0)
        .function(key_fn, js_string!("key"), 1)
        .accessor(js_string!("length"), Some(length_getter_fn), None, Attribute::all())
        .build())
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
}
