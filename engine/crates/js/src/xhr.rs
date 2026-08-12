//! `XMLHttpRequest` real (Fase 9): peticiones HTTP DE VERDAD via
//! `engine-net`, el mismo `NetworkEngine` que ya usan `fetch()` (Fase 4.3) y
//! el resto del motor - no un mock, y sin cliente HTTP nuevo.
//!
//! # Por que XHR ademas de `fetch`
//!
//! No es redundancia: una parte enorme de la web real -incluidas versiones
//! de jQuery que siguen sirviendose hoy- nunca migro a `fetch`. Un motor
//! que solo tiene `fetch` deja esas paginas sin red, aunque el transporte
//! subyacente sea exactamente el mismo.
//!
//! # SINCRONO SIEMPRE, y por que eso es lo honesto aqui
//!
//! El tercer argumento de `open(metodo, url, async)` se acepta y se
//! **ignora**: `send()` hace la peticion y llama a los manejadores antes de
//! devolver el control, tanto si se pidio `true` como `false`. Es la
//! semantica exacta de `open(..., false)` del spec, aplicada tambien al
//! caso `true`.
//!
//! No es una simplificacion arbitraria: es la unica que puede cumplirse.
//! Un XHR asincrono de verdad tiene que devolver el control al script y
//! disparar `onload` MAS TARDE, lo que exige poder suspender y reanudar la
//! ejecucion de JS. `Context::eval` de Boa es sincrono de punta a punta y
//! este motor no tiene forma de intercalarse a mitad de un script (ver el
//! doc-comment de `fetch.rs`, que declara la misma limitacion desde el otro
//! lado). Fingir asincronia -devolver el control y disparar `onload` en
//! algun momento posterior indeterminado- daria un XHR que a veces no
//! dispara nada, que es peor que uno sincrono declarado.
//!
//! Consecuencia practica, que conviene tener presente: el codigo
//! `xhr.onload = ...; xhr.send();` funciona igual que en un navegador
//! (`onload` se llama, y con los datos correctos), pero el codigo que
//! escribe algo DESPUES de `send()` esperando que corra "mientras" la
//! peticion viaja, aqui corre despues de que ya haya terminado. El orden
//! observable cambia; los datos no.
//!
//! # Diferencias declaradas con el spec
//!
//! - Sin `responseType` (`response` es siempre la misma cadena que
//!   `responseText`; no hay `arraybuffer`/`blob`/`document`, que exigirian
//!   tipos que este motor no expone a JS).
//! - Sin `abort()`, `timeout` ni `withCredentials`: los tres solo tienen
//!   sentido sobre una peticion en vuelo, y aqui nunca hay una (ver arriba).
//! - Sin eventos `progress`/`loadstart`/`loadend` ni `upload`.
//! - `send(body)` ignora el cuerpo: `engine-net` todavia no envia cuerpo de
//!   peticion (misma limitacion ya declarada en `fetch.rs`), asi que un
//!   `POST` viaja sin el. `setRequestHeader` SI se aplica de verdad.
//! - `addEventListener('load', ...)` sobre el XHR no existe - solo las
//!   propiedades `on*`. El registro de eventos de este motor esta indexado
//!   por nodo del DOM (ver `dom_bindings::DocumentBindings`) y un XHR no es
//!   un nodo.

use boa_engine::object::{FunctionObjectBuilder, JsData, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsArgs, JsNativeError, JsObject, JsResult, JsValue, NativeFunction};
use boa_gc::{Finalize, Trace};
use engine_net::request::Method;
use engine_net::{NetworkEngine, NetworkRequest};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Los cinco valores de `readyState` del spec. Este motor recorre los cinco
/// de verdad (en `send`), aunque sin pausa entre ellos - ver el aviso de
/// sincronia del modulo.
const UNSENT: u8 = 0;
const OPENED: u8 = 1;
const HEADERS_RECEIVED: u8 = 2;
const LOADING: u8 = 3;
const DONE: u8 = 4;

/// El estado mutable de UN objeto `XMLHttpRequest`. Va en un `Arc<Mutex<>>`
/// porque lo comparten todos los metodos/getters del mismo objeto JS (cada
/// uno es un closure nativo distinto con su propia captura), y todos tienen
/// que ver las mismas escrituras: `open()` guarda aqui lo que `send()`
/// leera, y `send()` guarda lo que los getters devolveran.
#[derive(Debug, Default)]
struct XhrState {
    ready_state: u8,
    method: String,
    url: String,
    request_headers: Vec<(String, String)>,
    status: u16,
    status_text: String,
    response_text: String,
    response_url: String,
    response_headers: Vec<(String, String)>,
    /// `true` si la peticion fallo antes de llegar a haber respuesta (URL
    /// invalida, host inalcanzable...). Distinto de un 404: un 404 es una
    /// peticion que SI funciono y hay que reportarla como `load` con
    /// `status = 404`, no como `error`. Confundir las dos cosas es el fallo
    /// clasico de un XHR mal implementado.
    network_error: bool,
}

/// Envoltorio `Trace`-able del estado + la red, para poder capturarlo en los
/// closures nativos de Boa. Mismo motivo y mismo patron que
/// `fetch::NetworkCapture` y `window::PendingCapture`: nada de lo que hay
/// dentro es memoria del recolector de Boa (es estado propio del motor y un
/// cliente HTTP de `hyper`), que es exactamente lo que declara
/// `empty_trace!`.
#[derive(Clone)]
struct XhrCapture {
    state: Arc<Mutex<XhrState>>,
    network: Arc<NetworkEngine>,
    /// Los manejadores `on*`. Estos SI son valores de Boa, asi que van
    /// aparte del resto y con el mismo razonamiento ya verificado para el
    /// registro de listeners del DOM (ver el aviso largo de
    /// `dom_bindings::DocumentBindings`): un `JsObject` guardado en un
    /// `Mutex` invisible al trazador sigue contando para su `ref_count`
    /// pero nunca para `non_root_count`, asi que el colector lo trata como
    /// raiz y lo mantiene vivo en vez de liberarlo bajo los pies.
    handlers: Arc<Mutex<HashMap<String, JsObject>>>,
}

impl Finalize for XhrCapture {}
unsafe impl Trace for XhrCapture {
    boa_gc::empty_trace!();
}

impl JsData for XhrCapture {}

/// Registra el constructor global `XMLHttpRequest`, respaldado por
/// `network` (el mismo `NetworkEngine` del resto del motor - reusa su
/// cliente HTTP y su pool de conexiones, no crea otro).
///
/// Separado de `bind_dom` por el mismo criterio que `register_fetch`: no
/// todo `JsRuntime` tiene red disponible. Sin llamar a esto, `new
/// XMLHttpRequest()` lanza `ReferenceError`, que es la respuesta honesta
/// donde de verdad no hay red, en vez de un objeto que nunca conecta.
pub fn register_xhr(context: &mut Context, network: Arc<NetworkEngine>) -> JsResult<()> {
    let constructor = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, captured: &NetworkOnlyCapture, context| Ok(build_xhr_object(captured.0.clone(), context).into()),
        NetworkOnlyCapture(network),
    );

    // `.constructor(true)` es lo que hace que `new XMLHttpRequest()`
    // funcione y no lance "not a constructor". El objeto lo construye la
    // propia funcion y se devuelve como valor: Boa usa el objeto devuelto
    // en vez del `this` recien creado, que es la semantica normal de
    // `new` cuando el constructor devuelve un objeto.
    let constructor_fn = FunctionObjectBuilder::new(context.realm(), constructor)
        .name(js_string!("XMLHttpRequest"))
        .length(0)
        .constructor(true)
        .build();

    context.register_global_property(js_string!("XMLHttpRequest"), constructor_fn, Attribute::all())?;
    Ok(())
}

/// El metodo HTTP que pidio `open()`, traducido al enum de `engine-net`.
/// Un metodo que ese enum no contempla cae a GET con un aviso, en vez de
/// fallar en silencio o inventarse un verbo: `engine-net` solo sabe enviar
/// los siete que modela.
fn parse_method(raw: &str) -> Method {
    match raw {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "HEAD" => Method::Head,
        "OPTIONS" => Method::Options,
        "PATCH" => Method::Patch,
        other => {
            tracing::warn!("[xhr] metodo HTTP no soportado por engine-net: {other}, se usara GET");
            Method::Get
        }
    }
}

#[derive(Clone)]
struct NetworkOnlyCapture(Arc<NetworkEngine>);

impl Finalize for NetworkOnlyCapture {}
unsafe impl Trace for NetworkOnlyCapture {
    boa_gc::empty_trace!();
}

/// Construye UNA instancia de `XMLHttpRequest` con su propio estado.
fn build_xhr_object(network: Arc<NetworkEngine>, context: &mut Context) -> JsObject {
    let capture = XhrCapture {
        state: Arc::new(Mutex::new(XhrState::default())),
        network,
        handlers: Arc::new(Mutex::new(HashMap::new())),
    };

    // open(metodo, url, async?): solo prepara. El tercer argumento se
    // acepta y se ignora - ver el aviso de sincronia del modulo. Reabrir un
    // XHR ya usado lo devuelve a un estado limpio, igual que el spec.
    let open = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &XhrCapture, context| {
            let method = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
            let url = args.get_or_undefined(1).to_string(context)?.to_std_string_escaped();
            let Ok(mut state) = captured.state.lock() else { return Ok(JsValue::undefined()) };
            *state = XhrState {
                ready_state: OPENED,
                method: method.to_uppercase(),
                url,
                ..XhrState::default()
            };
            drop(state);
            fire_ready_state_change(captured, context)?;
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    // setRequestHeader(nombre, valor): se aplica de VERDAD a la peticion
    // (ver `send`). El spec acumula valores repetidos del mismo nombre
    // separados por coma; aqui se guardan como entradas independientes y es
    // `NetworkRequest` quien decide (su `headers` es un mapa, asi que la
    // ultima gana). Diferencia real solo para el caso raro de repetir
    // cabecera, y se declara en vez de disimularse.
    let set_request_header = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &XhrCapture, context| {
            let name = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
            let value = args.get_or_undefined(1).to_string(context)?.to_std_string_escaped();
            if let Ok(mut state) = captured.state.lock() {
                // El spec exige `open()` antes: llamar a esto sobre un XHR
                // sin abrir lanza `InvalidStateError`. Aqui se ignora en
                // silencio - una cabecera que no se aplicaria a ninguna
                // peticion no rompe nada, y este motor no fabrica
                // excepciones DOM tipadas en ningun otro sitio.
                if state.ready_state != UNSENT {
                    state.request_headers.push((name, value));
                }
            }
            Ok(JsValue::undefined())
        },
        capture.clone(),
    );

    let send = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, captured: &XhrCapture, context| send_impl(captured, context),
        capture.clone(),
    );

    let get_response_header = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &XhrCapture, context| {
            let name = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped().to_lowercase();
            let Ok(state) = captured.state.lock() else { return Ok(JsValue::null()) };
            // `null` (no `""`) cuando la cabecera no viene: es lo que
            // distingue "no esta" de "esta pero vacia", y el spec lo
            // diferencia igual.
            Ok(match state.response_headers.iter().find(|(header, _)| header == &name) {
                Some((_, value)) => JsValue::from(js_string!(value.clone())),
                None => JsValue::null(),
            })
        },
        capture.clone(),
    );

    // getAllResponseHeaders(): una cadena con `nombre: valor` por linea
    // terminada en CRLF, con los nombres en minuscula y ordenados - el
    // formato exacto del spec.
    let get_all_response_headers = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, captured: &XhrCapture, _context| {
            let Ok(state) = captured.state.lock() else { return Ok(JsValue::from(js_string!(""))) };
            let mut lines: Vec<String> = state.response_headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect();
            lines.sort();
            Ok(JsValue::from(js_string!(lines.concat())))
        },
        capture.clone(),
    );

    let mut initializer = ObjectInitializer::with_native_data(capture.clone(), context);
    initializer
        .function(open, js_string!("open"), 3)
        .function(set_request_header, js_string!("setRequestHeader"), 2)
        .function(send, js_string!("send"), 1)
        .function(get_response_header, js_string!("getResponseHeader"), 1)
        .function(get_all_response_headers, js_string!("getAllResponseHeaders"), 0);

    // Las constantes de estado (`XMLHttpRequest.DONE` y `xhr.DONE`): el
    // codigo real las usa constantemente para comparar contra
    // `readyState`.
    for (name, value) in [("UNSENT", UNSENT), ("OPENED", OPENED), ("HEADERS_RECEIVED", HEADERS_RECEIVED), ("LOADING", LOADING), ("DONE", DONE)] {
        initializer.property(js_string!(name), value as i32, Attribute::all());
    }

    let object = initializer.build();
    define_state_accessors(&object, &capture, context);
    define_handler_accessors(&object, &capture, context);
    object
}

/// El nombre de una propiedad de solo lectura y la funcion que saca su
/// valor del estado. Puntero a funcion (no closure) a proposito: asi cada
/// getter puede capturarlo por copia dentro de su propio `NativeFunction`
/// sin arrastrar nada mas.
type StateReader = (&'static str, fn(&XhrState) -> JsValue);

/// Los getters de solo lectura respaldados por `XhrState`. Son accessors
/// (no propiedades fijas) porque su valor cambia a lo largo de la vida del
/// objeto: `readyState` recorre 0..4 y el resto se puebla al responder.
fn define_state_accessors(object: &JsObject, capture: &XhrCapture, context: &mut Context) {
    // Cada entrada: (nombre en JS, como sacar el valor del estado).
    let readers: [StateReader; 6] = [
        ("readyState", |state| JsValue::from(state.ready_state)),
        ("status", |state| JsValue::from(state.status)),
        ("statusText", |state| JsValue::from(js_string!(state.status_text.clone()))),
        ("responseText", |state| JsValue::from(js_string!(state.response_text.clone()))),
        // Sin `responseType`, `response` y `responseText` son siempre lo
        // mismo - declarado en el aviso del modulo.
        ("response", |state| JsValue::from(js_string!(state.response_text.clone()))),
        ("responseURL", |state| JsValue::from(js_string!(state.response_url.clone()))),
    ];

    for (name, read) in readers {
        let getter = NativeFunction::from_copy_closure_with_captures(
            move |_this, _args, captured: &XhrCapture, _context| {
                let Ok(state) = captured.state.lock() else { return Ok(JsValue::undefined()) };
                Ok(read(&state))
            },
            capture.clone(),
        );
        let getter_fn = FunctionObjectBuilder::new(context.realm(), getter)
            .name(js_string!(format!("get {name}")))
            .length(0)
            .constructor(false)
            .build();
        let descriptor = boa_engine::property::PropertyDescriptor::builder()
            .get(getter_fn)
            .enumerable(true)
            .configurable(true)
            .build();
        object.define_property_or_throw(js_string!(name), descriptor, context).expect("definir un accessor propio no deberia fallar");
    }
}

/// `onload`/`onerror`/`onreadystatechange`: accessors con getter Y setter,
/// porque el codigo real los ASIGNA (`xhr.onload = function () {...}`) y a
/// veces los lee. Se guardan en el mapa compartido, no como propiedad JS
/// suelta, para que `send` pueda invocarlos desde Rust.
fn define_handler_accessors(object: &JsObject, capture: &XhrCapture, context: &mut Context) {
    for name in ["onload", "onerror", "onreadystatechange"] {
        let getter = NativeFunction::from_copy_closure_with_captures(
            move |_this, _args, captured: &XhrCapture, _context| {
                let Ok(handlers) = captured.handlers.lock() else { return Ok(JsValue::null()) };
                Ok(match handlers.get(name) {
                    Some(handler) => JsValue::from(handler.clone()),
                    None => JsValue::null(),
                })
            },
            capture.clone(),
        );
        let getter_fn = FunctionObjectBuilder::new(context.realm(), getter).name(js_string!(format!("get {name}"))).length(0).constructor(false).build();

        let setter = NativeFunction::from_copy_closure_with_captures(
            move |_this, args: &[JsValue], captured: &XhrCapture, _context| {
                let Ok(mut handlers) = captured.handlers.lock() else { return Ok(JsValue::undefined()) };
                match args.first().and_then(JsValue::as_callable) {
                    Some(callable) => {
                        handlers.insert(name.to_string(), callable.clone());
                    }
                    // Asignar `null` (o cualquier cosa no invocable) quita
                    // el manejador, igual que en el DOM real.
                    None => {
                        handlers.remove(name);
                    }
                }
                Ok(JsValue::undefined())
            },
            capture.clone(),
        );
        let setter_fn = FunctionObjectBuilder::new(context.realm(), setter).name(js_string!(format!("set {name}"))).length(1).constructor(false).build();

        let descriptor = boa_engine::property::PropertyDescriptor::builder()
            .get(getter_fn)
            .set(setter_fn)
            .enumerable(true)
            .configurable(true)
            .build();
        object.define_property_or_throw(js_string!(name), descriptor, context).expect("definir un accessor propio no deberia fallar");
    }
}

/// El nucleo: hace la peticion HTTP real y recorre los estados.
///
/// Bloquea el hilo hasta que la respuesta esta entera (`pollster::block_on`,
/// el mismo mecanismo con el que la cola de trabajos por defecto de Boa ya
/// resuelve el `fetch()` de la Fase 4.3 - ver el doc-comment de `fetch.rs`).
/// Se llama directamente en vez de pasar por la cola de trabajos porque un
/// XHR sincrono tiene que tener sus resultados puestos ANTES de que `send()`
/// devuelva el control, que es justo lo que la cola no garantiza.
fn send_impl(captured: &XhrCapture, context: &mut Context) -> JsResult<JsValue> {
    let (method, url, headers) = {
        let Ok(state) = captured.state.lock() else {
            return Err(JsNativeError::typ().with_message("estado de XMLHttpRequest corrupto").into());
        };
        // `send()` sin `open()` previo: el spec lanza `InvalidStateError`.
        // Aqui es un no-op honesto - no hay ninguna URL a la que pedir
        // nada, asi que no se hace ninguna peticion ni se finge respuesta.
        if state.ready_state != OPENED {
            return Ok(JsValue::undefined());
        }
        (state.method.clone(), state.url.clone(), state.request_headers.clone())
    };

    let request = match NetworkRequest::new(&url) {
        Ok(mut request) => {
            request.method = parse_method(&method);
            for (name, value) in headers {
                request.headers.insert(name, value);
            }
            request
        }
        Err(_) => return finish_with_network_error(captured, context),
    };

    let response = match pollster::block_on(captured.network.fetch(&request)) {
        Ok(response) => response,
        Err(_) => return finish_with_network_error(captured, context),
    };

    // Un cuerpo que no es UTF-8 valido no es un error de red: la peticion
    // funciono. Se reporta como respuesta con `responseText` vacio, en vez
    // de fingir un fallo de conexion que no ocurrio.
    let text = response.text().unwrap_or_default();

    // HEADERS_RECEIVED y LOADING se disparan de verdad, aunque sin espera
    // entre ellos: el codigo real que escucha `onreadystatechange` y
    // compara contra `xhr.HEADERS_RECEIVED` (para leer cabeceras antes que
    // el cuerpo) espera verlos pasar, y saltarselos lo dejaria sin
    // ejecutar.
    {
        let Ok(mut state) = captured.state.lock() else { return Ok(JsValue::undefined()) };
        state.status = response.status_code;
        state.status_text = response.status_text.clone();
        state.response_url = response.url.to_string();
        state.response_headers = response.headers.iter().map(|(name, value)| (name.to_lowercase(), value.clone())).collect();
        state.ready_state = HEADERS_RECEIVED;
    }
    fire_ready_state_change(captured, context)?;

    {
        let Ok(mut state) = captured.state.lock() else { return Ok(JsValue::undefined()) };
        state.ready_state = LOADING;
    }
    fire_ready_state_change(captured, context)?;

    {
        let Ok(mut state) = captured.state.lock() else { return Ok(JsValue::undefined()) };
        state.response_text = text;
        state.ready_state = DONE;
    }
    fire_ready_state_change(captured, context)?;
    // `onload` se dispara para CUALQUIER respuesta que haya llegado,
    // incluido un 404 o un 500 - igual que el spec. `onerror` es solo para
    // fallos de red, donde no hay respuesta ninguna. Quien quiera
    // distinguir exito de error mira `status`, que es como se hace de
    // verdad.
    invoke_handler(captured, "onload", context)?;
    Ok(JsValue::undefined())
}

/// Fallo de RED (URL invalida, host inalcanzable, conexion caida): se pasa
/// a `DONE` con `status = 0` - la marca con la que el codigo real reconoce
/// este caso - y se dispara `onerror`, nunca `onload`.
fn finish_with_network_error(captured: &XhrCapture, context: &mut Context) -> JsResult<JsValue> {
    {
        let Ok(mut state) = captured.state.lock() else { return Ok(JsValue::undefined()) };
        state.ready_state = DONE;
        state.status = 0;
        state.network_error = true;
    }
    fire_ready_state_change(captured, context)?;
    invoke_handler(captured, "onerror", context)?;
    Ok(JsValue::undefined())
}

fn fire_ready_state_change(captured: &XhrCapture, context: &mut Context) -> JsResult<()> {
    invoke_handler(captured, "onreadystatechange", context).map(|_| ())
}

/// Invoca el manejador si lo hay. El `this` es `undefined` en vez del
/// propio XHR: los closures nativos de aqui no tienen una referencia al
/// objeto JS que los envuelve (se construye despues que ellos). Casi todo
/// el codigo real captura el XHR por cierre (`var xhr = new
/// XMLHttpRequest(); xhr.onload = function () { xhr.responseText }`), que
/// funciona igual; el que use `this.responseText` dentro del manejador, no.
/// Declarado en vez de disimulado.
fn invoke_handler(captured: &XhrCapture, name: &str, context: &mut Context) -> JsResult<JsValue> {
    let handler = {
        let Ok(handlers) = captured.handlers.lock() else { return Ok(JsValue::undefined()) };
        handlers.get(name).cloned()
    };
    match handler {
        // `as_callable` ya lo valido al asignarlo (el setter solo guarda
        // valores invocables), asi que aqui no puede fallar por no serlo.
        Some(handler) => match JsValue::from(handler).as_callable() {
            Some(callable) => callable.call(&JsValue::undefined(), &[], context),
            None => Ok(JsValue::undefined()),
        },
        None => Ok(JsValue::undefined()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::JsRuntime;

    /// Ninguno de estos tests toca la red DE VERDAD, igual que los de
    /// `fetch.rs`: se comprueba todo lo que se puede comprobar sin
    /// servidor (forma de la API, transiciones de estado, manejo de un
    /// fallo de red) y la peticion real se verifica en vivo contra
    /// `engine_server.exe` con un servidor local, no aqui.
    fn runtime_with_xhr() -> JsRuntime {
        let mut runtime = JsRuntime::new();
        runtime.register_xhr(Arc::new(NetworkEngine::new())).expect("XHR deberia registrarse");
        runtime
    }

    #[test]
    fn xhr_is_not_defined_at_all_unless_it_was_registered() {
        let mut runtime = JsRuntime::new();
        assert_eq!(
            runtime.eval("typeof XMLHttpRequest").expect("typeof no deberia lanzar"),
            "\"undefined\"",
            "sin red disponible, XMLHttpRequest no deberia existir en vez de fingir que conecta"
        );
    }

    #[test]
    fn a_new_xhr_starts_unsent_with_empty_fields() {
        let mut runtime = runtime_with_xhr();
        assert_eq!(runtime.eval("var x = new XMLHttpRequest(); x.readyState").unwrap(), "0");
        assert_eq!(runtime.eval("x.status").unwrap(), "0");
        assert_eq!(runtime.eval("x.responseText").unwrap(), "\"\"");
    }

    #[test]
    fn the_state_constants_are_exposed_with_their_spec_values() {
        let mut runtime = runtime_with_xhr();
        assert_eq!(runtime.eval("var x = new XMLHttpRequest(); [x.UNSENT, x.OPENED, x.HEADERS_RECEIVED, x.LOADING, x.DONE].join()").unwrap(), "\"0,1,2,3,4\"");
    }

    #[test]
    fn open_moves_to_opened_and_fires_readystatechange() {
        let mut runtime = runtime_with_xhr();
        let estados = runtime
            .eval(
                r#"
                var x = new XMLHttpRequest();
                var vistos = [];
                x.onreadystatechange = function () { vistos.push(x.readyState); };
                x.open('GET', 'http://127.0.0.1:1/nada');
                vistos.join() + '|' + x.readyState;
                "#,
            )
            .unwrap();
        assert_eq!(estados, "\"1|1\"", "open deberia pasar a OPENED y notificarlo");
    }

    #[test]
    fn each_xhr_instance_has_its_own_independent_state() {
        let mut runtime = runtime_with_xhr();
        let result = runtime
            .eval(
                r#"
                var a = new XMLHttpRequest();
                var b = new XMLHttpRequest();
                a.open('GET', 'http://127.0.0.1:1/a');
                a.readyState + ',' + b.readyState;
                "#,
            )
            .unwrap();
        assert_eq!(result, "\"1,0\"", "abrir una instancia no deberia mover el estado de la otra");
    }

    #[test]
    fn assigning_a_handler_then_reading_it_back_gives_the_same_function() {
        let mut runtime = runtime_with_xhr();
        let result = runtime
            .eval(
                r#"
                var x = new XMLHttpRequest();
                var f = function () {};
                x.onload = f;
                (x.onload === f) + ',' + (x.onerror === null);
                "#,
            )
            .unwrap();
        assert_eq!(result, "\"true,true\"");
    }

    #[test]
    fn assigning_null_removes_a_previously_set_handler() {
        let mut runtime = runtime_with_xhr();
        let result = runtime.eval("var x = new XMLHttpRequest(); x.onload = function () {}; x.onload = null; x.onload === null").unwrap();
        assert_eq!(result, "true");
    }

    /// `send()` sin `open()` no debe hacer NADA - ni peticion, ni fingir
    /// una respuesta, ni disparar manejadores.
    #[test]
    fn send_without_open_is_a_no_op_that_fires_nothing() {
        let mut runtime = runtime_with_xhr();
        let result = runtime
            .eval(
                r#"
                var x = new XMLHttpRequest();
                var disparos = 0;
                x.onload = function () { disparos++; };
                x.onerror = function () { disparos++; };
                x.send();
                disparos + ',' + x.readyState;
                "#,
            )
            .unwrap();
        assert_eq!(result, "\"0,0\"");
    }

    /// Un fallo de RED (puerto 1 de localhost: nada escuchando) tiene que
    /// llegar a DONE con `status = 0` y disparar `onerror`, NO `onload` -
    /// es lo que distingue "no se pudo conectar" de "el servidor
    /// respondio 404", que es un error clasico de implementacion.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_network_failure_fires_onerror_not_onload_and_leaves_status_zero() {
        let mut runtime = runtime_with_xhr();
        let result = runtime
            .eval(
                r#"
                var x = new XMLHttpRequest();
                var log = [];
                x.onload = function () { log.push('load'); };
                x.onerror = function () { log.push('error'); };
                x.open('GET', 'http://127.0.0.1:1/no-hay-nadie');
                x.send();
                log.join() + '|' + x.readyState + '|' + x.status;
                "#,
            )
            .unwrap();
        assert_eq!(result, "\"error|4|0\"");
    }

    #[test]
    fn get_response_header_is_null_when_there_is_no_response_yet() {
        let mut runtime = runtime_with_xhr();
        let result = runtime.eval("var x = new XMLHttpRequest(); x.getResponseHeader('content-type') === null").unwrap();
        assert_eq!(result, "true", "sin respuesta, una cabecera ausente deberia ser null, no cadena vacia");
    }

    #[test]
    fn get_all_response_headers_is_empty_before_any_response() {
        let mut runtime = runtime_with_xhr();
        assert_eq!(runtime.eval("var x = new XMLHttpRequest(); x.getAllResponseHeaders()").unwrap(), "\"\"");
    }

    /// Reabrir un XHR ya usado tiene que limpiar lo anterior, no dejar la
    /// respuesta vieja visible junto a la peticion nueva.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reopening_resets_the_previous_result() {
        let mut runtime = runtime_with_xhr();
        let result = runtime
            .eval(
                r#"
                var x = new XMLHttpRequest();
                x.open('GET', 'http://127.0.0.1:1/uno');
                x.send();
                var tras_fallo = x.readyState;
                x.open('GET', 'http://127.0.0.1:1/dos');
                tras_fallo + ',' + x.readyState + ',' + x.status;
                "#,
            )
            .unwrap();
        assert_eq!(result, "\"4,1,0\"", "reabrir deberia volver a OPENED y limpiar el estado anterior");
    }
}
