//! `fetch()` real (Fase 4.3): peticion HTTP DE VERDAD via `engine-net` (no
//! un mock), resuelta como una `Promise` real utilizable con `.then()`/
//! `await` desde JS - la version minima honesta de la Web API real.
//!
//! **Simplificacion de concurrencia declarada, no un bug escondido**: el
//! motor de scripts (`Context::eval`, en `runtime.rs`) es siempre SINCRONO
//! de punta a punta - nunca hay un `.await` de Rust corriendo DENTRO de la
//! pila de llamadas de un script JS (confirmado leyendo la cadena real:
//! `core::server::navigate`/`click` hacen TODO el trabajo async en Rust
//! ANTES de invocar `runtime.eval`, nunca durante). La cola de trabajos por
//! defecto de Boa (`SimpleJobQueue`, la unica que usa `JsRuntime::new` via
//! `Context::default()`) resuelve `enqueue_future_job` bloqueando el hilo
//! actual hasta que el future termine (via `pollster::block_on` - ver el
//! codigo fuente de `boa_engine::job::SimpleJobQueue`) - asi que `fetch()`,
//! aunque devuelve una `Promise` real con forma de API correcta
//! (`await fetch(url)` y `.then()` funcionan tal cual en JS), en la
//! practica BLOQUEA el hilo que esta evaluando el script hasta que la
//! peticion HTTP real termine, no lo libera para hacer otra cosa mientras
//! tanto. Un fetch NO bloqueante de verdad exigiria reestructurar
//! `execute_inline_scripts_keeping_runtime`/`Context::eval` para
//! intercalarse con jobs de tokio pendientes DURANTE la ejecucion del
//! script, no solo antes o despues - fuera del alcance de esta tarea.
//! Aceptable para el uso real de este motor hoy: `core::server` procesa un
//! comando NDJSON a la vez, sin trabajo concurrente que este bloqueo
//! pudiera interferir.
//!
//! `fetch(url, options)` **SI soporta `options`** desde la Fase 27
//! (`method`/`headers`/`body`/`credentials`) - la doc de aqui abajo
//! afirmaba lo contrario porque, cuando se escribio esto (Fase 4.3),
//! `engine-net` de verdad no enviaba cuerpo de peticion en ninguna forma;
//! eso se arreglo en la Fase 16 (`Full<Bytes>` en vez de `Empty`) pero
//! nadie volvio a conectar `fetch()` con ello - encontrado auditando el
//! motor: `fetch(url, {method:'POST', body:...})` seguia haciendo SIEMPRE
//! un GET sin cuerpo, el patron mas comun de AJAX moderno. `method` acepta
//! cualquiera de los siete verbos que `engine-net` modela (no reconocido
//! cae a GET, igual que `xhr.rs`); `headers` es un objeto plano
//! nombre->valor (SIN la clase `Headers` real - ver mas abajo, aplica
//! igual aqui); `body` se convierte a cadena UTF-8 (`ToString` de JS, asi
//! que un objeto pasado como cuerpo da `"[object Object]"`, igual que hace
//! `fetch` real sin `JSON.stringify` explicito) y, si no hay `Content-
//! Type` ya puesto en `headers`, se añade `text/plain;charset=UTF-8` -
//! mismo valor por defecto que el spec real para un cuerpo de cadena.
//! Un `body` con metodo `GET`/`HEAD` rechaza la promise con un
//! `TypeError` SIN tocar la red, igual que el spec (esos dos metodos no
//! pueden llevar cuerpo). `credentials: 'include'` activa el envio de
//! cookies a un origen cruzado (ver `NetworkRequest::include_credentials`,
//! Fase 20); cualquier otro valor (o ausente) se queda en el default real
//! del spec (`'same-origin'` - las cookies SI viajan al mismo origen sin
//! pedirlo, NUNCA a otro sin `'include'`).
//!
//! Sin la clase `Headers` real (`response.headers` es un objeto plano
//! nombre-minuscula -> valor, no `Headers` con `.get()`/`.has()`/
//! iteracion) - simetrico entre lo que `fetch()` ENVIA y lo que expone al
//! LEER una respuesta. `response.json()` reusa el `JSON.parse` REAL de Boa
//! (invocado como si fuera JS) en vez de reinventar un parser JSON propio.

use boa_engine::{
    job::NativeJob,
    js_string,
    object::{builtins::JsPromise, ObjectInitializer},
    property::{Attribute, PropertyKey},
    Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction,
};
use boa_gc::{Finalize, Trace};
use engine_net::request::Method;
use engine_net::{NetworkEngine, NetworkRequest, NetworkResponse};
use std::sync::Arc;

/// El metodo HTTP que pidio `options.method`, traducido al enum de
/// `engine-net` - mismo criterio que `xhr::parse_method` (un metodo que
/// ese enum no contempla cae a GET con un aviso, en vez de fallar en
/// silencio o inventarse un verbo). Duplicado a proposito en vez de
/// compartido: es una funcion de ~10 lineas, y `fetch.rs`/`xhr.rs` no
/// tenian ninguna dependencia entre si que valiera la pena crear solo para
/// esto.
fn parse_method(raw: &str) -> Method {
    match raw.to_ascii_uppercase().as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "HEAD" => Method::Head,
        "OPTIONS" => Method::Options,
        "PATCH" => Method::Patch,
        other => {
            tracing::warn!("[fetch] metodo HTTP no soportado por engine-net: {other}, se usara GET");
            Method::Get
        }
    }
}

/// Lo que `options` (segundo argumento de `fetch(url, options)`) le pide a
/// la peticion - ver el aviso del modulo para el diseño completo.
struct FetchOptions {
    method: Method,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    include_credentials: bool,
}

/// Lee `options` de verdad, sin asumir que trae ninguna de las claves -
/// `fetch(url)` sin segundo argumento (o con `undefined`/algo que no es un
/// objeto) da exactamente el mismo resultado que antes de esta fase: GET,
/// sin cuerpo, sin cabeceras extra.
fn read_fetch_options(options: &JsValue, context: &mut Context) -> JsResult<FetchOptions> {
    let mut result = FetchOptions { method: Method::Get, headers: Vec::new(), body: None, include_credentials: false };
    let Some(obj) = options.as_object() else { return Ok(result) };

    let method_value = obj.get(js_string!("method"), context)?;
    if !method_value.is_undefined() {
        result.method = parse_method(&method_value.to_string(context)?.to_std_string_escaped());
    }

    let body_value = obj.get(js_string!("body"), context)?;
    if !body_value.is_undefined() && !body_value.is_null() {
        result.body = Some(body_value.to_string(context)?.to_std_string_escaped().into_bytes());
    }

    let headers_value = obj.get(js_string!("headers"), context)?;
    if let Some(headers_obj) = headers_value.as_object() {
        for key in headers_obj.own_property_keys(context)? {
            let PropertyKey::String(name) = &key else { continue };
            let value = headers_obj.get(key.clone(), context)?;
            result.headers.push((name.to_std_string_escaped(), value.to_string(context)?.to_std_string_escaped()));
        }
    }

    let credentials_value = obj.get(js_string!("credentials"), context)?;
    if !credentials_value.is_undefined() {
        result.include_credentials = credentials_value.to_string(context)?.to_std_string_escaped() == "include";
    }

    Ok(result)
}

/// Vuelca `options` YA LEIDAS sobre una `NetworkRequest` recien construida
/// - logica PURA (sin `Context` de Boa ni red), separada a proposito de
/// `read_fetch_options` (que si necesita `Context` para leer el objeto JS)
/// para poder probarla directamente, mismo criterio que `redirect_decision`
/// en `engine-net::http_client`.
fn apply_fetch_options(request: &mut NetworkRequest, options: FetchOptions) {
    request.method = options.method;
    request.include_credentials = options.include_credentials;
    let has_content_type = options.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    for (k, v) in options.headers {
        request.headers.insert(k, v);
    }
    request.body = options.body;
    // Mismo default que el spec real para un cuerpo de cadena SIN
    // `Content-Type` explicito en `headers`.
    if request.body.is_some() && !has_content_type {
        request.headers.insert("Content-Type".to_string(), "text/plain;charset=UTF-8".to_string());
    }
}

/// Envoltorio `Trace`-able sobre `Arc<NetworkEngine>` - las "captures" de
/// `NativeFunction::from_copy_closure_with_captures` deben implementar
/// `Trace` (el recolector de basura de Boa necesita saber que recorrer),
/// pero `NetworkEngine` no contiene NINGUN valor de Boa (`JsValue`/
/// `JsObject`/`Gc<T>`) - es un cliente HTTP puro de `hyper`, ajeno por
/// completo al heap de Boa. `empty_trace!()` declara "nada que recorrer
/// aqui", que es la verdad, no un atajo inseguro.
#[derive(Clone)]
struct NetworkCapture(Arc<NetworkEngine>, Option<String>);

impl Finalize for NetworkCapture {}
unsafe impl Trace for NetworkCapture {
    boa_gc::empty_trace!();
}

/// Mismo criterio que `NetworkCapture`, para el cuerpo YA DESCARGADO que
/// `response.text()`/`response.json()` necesitan (ver `build_response_object`)
/// - una vez que `fetch()` ya termino, leer el texto que ya esta en memoria
/// no necesita ninguna `Promise` genuinamente asincrona, solo la FORMA de
/// una (la Web API real tambien devuelve una promise aqui, aunque en la
/// practica resuelva al instante).
#[derive(Clone)]
struct BodyCapture(Result<String, String>);

impl Finalize for BodyCapture {}
unsafe impl Trace for BodyCapture {
    boa_gc::empty_trace!();
}

/// Registra el global `fetch` sobre `context`, respaldado por `network`
/// (el mismo `NetworkEngine` que usa el resto del motor - reusa su cliente
/// HTTP/pool de conexiones ya construido, no crea uno nuevo). Ver el
/// doc-comment del modulo para las simplificaciones declaradas.
pub fn register_fetch(context: &mut Context, network: Arc<NetworkEngine>, page_origin: Option<String>) -> JsResult<()> {
    let capture = NetworkCapture(network, page_origin);
    let fetch_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture, context| {
            let url = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
            let (promise, resolvers) = JsPromise::new_pending(context);

            // Se resuelve contra la URL de la pagina (Fase 20.1), asi que
            // `fetch('/api/datos')` funciona igual que en un navegador
            // real. De paso sale el origen, que activa la politica de
            // mismo origen (Fase 20): con el, una respuesta de otro
            // dominio solo se puede leer si trae permiso CORS.
            let options = read_fetch_options(args.get_or_undefined(1), context)?;
            // GET/HEAD con cuerpo es un `TypeError` SINCRONO del spec real
            // (`Request constructor: HEAD or GET Request cannot have a
            // body`) - se rechaza aqui, antes de resolver siquiera la URL,
            // sin tocar la red en absoluto.
            if options.body.is_some() && matches!(options.method, Method::Get | Method::Head) {
                let js_error: JsError = JsNativeError::typ().with_message("Failed to execute 'fetch': Request with GET/HEAD method cannot have body").into();
                let opaque = js_error.to_opaque(context);
                resolvers.reject.call(&JsValue::undefined(), &[opaque], context)?;
                return Ok(promise.into());
            }

            let resolved = engine_net::request::resolve_against_page(&url, capture.1.as_deref());
            let request = match resolved.map(|(absolute, origin)| {
                NetworkRequest::new(absolute.as_str()).map(|mut r| {
                    r.origin = origin;
                    apply_fetch_options(&mut r, options);
                    r
                })
            }) {
                Some(Ok(request)) => request,
                _ => {
                    let js_error: JsError = JsNativeError::typ().with_message(format!("Failed to fetch '{url}': URL invalida")).into();
                    let opaque = js_error.to_opaque(context);
                    resolvers.reject.call(&JsValue::undefined(), &[opaque], context)?;
                    return Ok(promise.into());
                }
            };

            let network = capture.0.clone();
            // El future en si SOLO hace trabajo Rust puro (peticion HTTP
            // real) - construir el objeto `Response` de verdad necesita
            // `Context`, que no puede vivir dentro de un future generico
            // (no es `Send`/`'static` de esa forma) - por eso se difiere a
            // el `NativeJob` que este mismo future produce como resultado,
            // ejecutado DESPUES con acceso real a `context` (mismo patron
            // que usa `JsPromise::from_future` internamente, adaptado a
            // mano porque necesitamos construir un objeto rico, no solo un
            // valor primitivo).
            let future = async move {
                let result = network.fetch(&request).await;
                NativeJob::new(move |context| match result {
                    Ok(response) => match build_response_object(&response, context) {
                        Ok(js_response) => resolvers.resolve.call(&JsValue::undefined(), &[js_response], context),
                        Err(error) => {
                            let opaque = error.to_opaque(context);
                            resolvers.reject.call(&JsValue::undefined(), &[opaque], context)
                        }
                    },
                    Err(error) => {
                        let js_error: JsError = JsNativeError::typ().with_message(format!("Failed to fetch '{url}': {error}")).into();
                        let opaque = js_error.to_opaque(context);
                        resolvers.reject.call(&JsValue::undefined(), &[opaque], context)
                    }
                })
            };
            context.job_queue().enqueue_future_job(Box::pin(future), context);

            Ok(promise.into())
        },
        capture,
    );

    context.register_global_builtin_callable(js_string!("fetch"), 1, fetch_fn)?;
    Ok(())
}

/// Construye el objeto `Response` (spec-shaped, simplificado - ver el
/// doc-comment del modulo) a partir de una `NetworkResponse` YA
/// DESCARGADA por completo.
fn build_response_object(response: &NetworkResponse, context: &mut Context) -> JsResult<JsValue> {
    let mut headers_init = ObjectInitializer::new(context);
    for (name, value) in &response.headers {
        headers_init.property(js_string!(name.clone()), js_string!(value.clone()), Attribute::all());
    }
    let headers_obj = headers_init.build();
    let body = BodyCapture(Ok(response.text()));

    let text_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, body, context| match &body.0 {
            Ok(text) => Ok(JsPromise::resolve(js_string!(text.clone()), context).into()),
            Err(message) => {
                let js_error: JsError = JsNativeError::typ().with_message(message.clone()).into();
                Ok(JsPromise::reject(js_error, context).into())
            }
        },
        body.clone(),
    );

    let json_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, body, context| {
            let text = match &body.0 {
                Ok(text) => text,
                Err(message) => {
                    let js_error: JsError = JsNativeError::typ().with_message(message.clone()).into();
                    return Ok(JsPromise::reject(js_error, context).into());
                }
            };
            // Reusa el `JSON.parse` REAL de Boa (invocandolo como haria
            // cualquier script JS) en vez de reinventar un parser JSON
            // propio - un JSON invalido lanza de forma natural (un
            // `SyntaxError` real de Boa), capturado aqui y convertido en
            // promise rechazada en vez de propagar la excepcion hacia
            // afuera de esta funcion nativa.
            let json_global = context.global_object().get(js_string!("JSON"), context)?;
            let parse_fn = json_global
                .as_object()
                .and_then(|obj| obj.get(js_string!("parse"), context).ok())
                .and_then(|v| v.as_callable().cloned())
                .ok_or_else(|| JsNativeError::typ().with_message("JSON.parse no deberia faltar en un Context real"))?;
            match parse_fn.call(&JsValue::undefined(), &[js_string!(text.clone()).into()], context) {
                Ok(parsed) => Ok(JsPromise::resolve(parsed, context).into()),
                Err(error) => Ok(JsPromise::reject(error, context).into()),
            }
        },
        body,
    );

    let response_obj = ObjectInitializer::new(context)
        .property(js_string!("status"), response.status_code, Attribute::all())
        .property(js_string!("ok"), response.is_success(), Attribute::all())
        .property(js_string!("statusText"), js_string!(response.status_text.clone()), Attribute::all())
        .property(js_string!("url"), js_string!(response.url.to_string()), Attribute::all())
        .property(js_string!("headers"), headers_obj, Attribute::all())
        .function(text_fn, js_string!("text"), 0)
        .function(json_fn, js_string!("json"), 0)
        .build();

    Ok(response_obj.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::builtins::promise::PromiseState;
    use boa_engine::Source;
    use bytes::Bytes;
    use std::collections::HashMap;
    use url::Url;

    /// Sin peticion HTTP real en NINGUNO de estos tests (ver el
    /// doc-comment del modulo) - `build_response_object` en si es pura
    /// respecto a la red (opera sobre una `NetworkResponse` YA
    /// descargada), asi que se construye una a mano aqui, igual que
    /// `engine-net::http_client` prueba su propia logica (redirecciones,
    /// descompresion) sin tocar la red de verdad tampoco.
    fn sample_response(status: u16, body: &str) -> NetworkResponse {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        NetworkResponse {
            url: Url::parse("http://127.0.0.1/test").unwrap(),
            status_code: status,
            status_text: "OK".to_string(),
            headers,
            set_cookie: Vec::new(),
            body: Bytes::from(body.to_string()),
        }
    }

    fn call_promise_method(context: &mut Context, response_value: &JsValue, method: &str) -> JsPromise {
        let obj = response_value.as_object().expect("response deberia ser un objeto");
        let method_fn = obj.get(js_string!(method), context).expect("deberia existir la propiedad").as_callable().cloned().expect("deberia ser invocable");
        let result = method_fn.call(response_value, &[], context).expect("la llamada no deberia fallar");
        let promise = JsPromise::from_object(result.as_object().expect("deberia devolver una promise").clone()).expect("deberia ser una promise real");
        context.run_jobs();
        promise
    }

    #[test]
    fn build_response_object_exposes_status_ok_statustext_url_and_headers() {
        let mut context = Context::default();
        let response = sample_response(200, "hello");
        let js_response = build_response_object(&response, &mut context).expect("no deberia fallar");
        let obj = js_response.as_object().expect("deberia ser un objeto");

        assert_eq!(obj.get(js_string!("status"), &mut context).unwrap(), JsValue::from(200));
        assert_eq!(obj.get(js_string!("ok"), &mut context).unwrap(), JsValue::from(true));
        assert_eq!(
            obj.get(js_string!("statusText"), &mut context).unwrap().to_string(&mut context).unwrap().to_std_string_escaped(),
            "OK"
        );
        assert_eq!(
            obj.get(js_string!("url"), &mut context).unwrap().to_string(&mut context).unwrap().to_std_string_escaped(),
            "http://127.0.0.1/test"
        );

        let headers_obj = obj.get(js_string!("headers"), &mut context).unwrap();
        let headers_obj = headers_obj.as_object().expect("headers deberia ser un objeto");
        assert_eq!(
            headers_obj.get(js_string!("content-type"), &mut context).unwrap().to_string(&mut context).unwrap().to_std_string_escaped(),
            "application/json"
        );
    }

    #[test]
    fn build_response_object_ok_is_false_for_a_non_2xx_status() {
        let mut context = Context::default();
        let response = sample_response(404, "not found");
        let js_response = build_response_object(&response, &mut context).unwrap();
        let obj = js_response.as_object().unwrap();
        assert_eq!(obj.get(js_string!("ok"), &mut context).unwrap(), JsValue::from(false));
    }

    #[test]
    fn response_text_resolves_to_the_real_body() {
        let mut context = Context::default();
        let response = sample_response(200, "hello world");
        let js_response = build_response_object(&response, &mut context).unwrap();

        let promise = call_promise_method(&mut context, &js_response, "text");
        match promise.state() {
            PromiseState::Fulfilled(v) => assert_eq!(v.to_string(&mut context).unwrap().to_std_string_escaped(), "hello world"),
            other => panic!("se esperaba Fulfilled, se obtuvo {other:?}"),
        }
    }

    #[test]
    fn response_json_parses_a_valid_json_body_using_boas_real_json_parse() {
        let mut context = Context::default();
        let response = sample_response(200, r#"{"a":1}"#);
        let js_response = build_response_object(&response, &mut context).unwrap();

        let promise = call_promise_method(&mut context, &js_response, "json");
        match promise.state() {
            PromiseState::Fulfilled(v) => {
                let parsed = v.as_object().expect("deberia ser un objeto parseado");
                assert_eq!(parsed.get(js_string!("a"), &mut context).unwrap(), JsValue::from(1));
            }
            other => panic!("se esperaba Fulfilled, se obtuvo {other:?}"),
        }
    }

    /// El punto real de reusar `JSON.parse` de Boa en vez de un parser
    /// propio: un JSON invalido rechaza la promise (comportamiento real de
    /// `Response.json()`), no hace panic ni resuelve con basura.
    #[test]
    fn response_json_rejects_an_invalid_json_body() {
        let mut context = Context::default();
        let response = sample_response(200, "esto no es JSON");
        let js_response = build_response_object(&response, &mut context).unwrap();

        let promise = call_promise_method(&mut context, &js_response, "json");
        assert!(matches!(promise.state(), PromiseState::Rejected(_)), "un JSON invalido deberia rechazar la promise");
    }

    #[test]
    fn fetch_is_registered_as_a_real_global_function() {
        let mut context = Context::default();
        register_fetch(&mut context, Arc::new(NetworkEngine::new()), None).unwrap();
        let result = context.eval(Source::from_bytes("typeof fetch")).unwrap();
        assert_eq!(result.to_string(&mut context).unwrap().to_std_string_escaped(), "function");
    }

    /// Una URL invalida rechaza SIN tocar la red en absoluto
    /// (`NetworkRequest::new` falla al parsear antes de que exista
    /// siquiera un future que ejecutar) - verificable sin conexion real.
    #[test]
    fn fetch_with_an_invalid_url_rejects_without_touching_the_network() {
        let mut context = Context::default();
        register_fetch(&mut context, Arc::new(NetworkEngine::new()), None).unwrap();
        let result = context.eval(Source::from_bytes("fetch('esto no es una url')")).unwrap();
        let promise = JsPromise::from_object(result.as_object().unwrap().clone()).unwrap();
        context.run_jobs();
        assert!(matches!(promise.state(), PromiseState::Rejected(_)));
    }

    fn eval_options(context: &mut Context, js_expr: &str) -> FetchOptions {
        let value = context.eval(Source::from_bytes(js_expr.as_bytes())).expect("el literal de options deberia evaluar");
        read_fetch_options(&value, context).expect("leer options no deberia fallar")
    }

    #[test]
    fn undefined_options_gives_the_pre_fase_27_defaults() {
        let mut context = Context::default();
        let options = read_fetch_options(&JsValue::undefined(), &mut context).unwrap();
        assert!(matches!(options.method, Method::Get));
        assert!(options.headers.is_empty());
        assert!(options.body.is_none());
        assert!(!options.include_credentials);
    }

    #[test]
    fn options_method_is_parsed_case_insensitively() {
        let mut context = Context::default();
        let options = eval_options(&mut context, "({method: 'post'})");
        assert!(matches!(options.method, Method::Post));
    }

    #[test]
    fn options_body_is_read_as_a_utf8_string() {
        let mut context = Context::default();
        let options = eval_options(&mut context, "({method: 'POST', body: 'hola mundo'})");
        assert_eq!(options.body.as_deref(), Some(b"hola mundo".as_slice()));
    }

    #[test]
    fn options_headers_reads_every_own_key_of_a_plain_object() {
        let mut context = Context::default();
        let options = eval_options(&mut context, "({headers: {'X-Custom': 'valor', 'Content-Type': 'application/json'}})");
        assert!(options.headers.contains(&("X-Custom".to_string(), "valor".to_string())));
        assert!(options.headers.contains(&("Content-Type".to_string(), "application/json".to_string())));
    }

    #[test]
    fn credentials_include_turns_on_include_credentials() {
        let mut context = Context::default();
        let options = eval_options(&mut context, "({credentials: 'include'})");
        assert!(options.include_credentials);
    }

    #[test]
    fn credentials_omitted_or_anything_else_keeps_the_spec_default_of_false() {
        let mut context = Context::default();
        assert!(!eval_options(&mut context, "({})").include_credentials);
        assert!(!eval_options(&mut context, "({credentials: 'same-origin'})").include_credentials);
    }

    #[test]
    fn apply_fetch_options_sets_method_body_and_extra_headers_on_the_request() {
        let mut request = NetworkRequest::new("https://ejemplo.test/api").unwrap();
        apply_fetch_options(
            &mut request,
            FetchOptions { method: Method::Post, headers: vec![("X-Token".to_string(), "abc".to_string())], body: Some(b"{}".to_vec()), include_credentials: true },
        );
        assert!(matches!(request.method, Method::Post));
        assert_eq!(request.body.as_deref(), Some(b"{}".as_slice()));
        assert_eq!(request.headers.get("X-Token").map(String::as_str), Some("abc"));
        assert!(request.include_credentials);
    }

    /// El default real del spec para un cuerpo de cadena sin `Content-Type`
    /// explicito - solo cuando quien llama NO puso uno ya.
    #[test]
    fn apply_fetch_options_defaults_content_type_only_when_absent() {
        let mut sin_content_type = NetworkRequest::new("https://ejemplo.test/").unwrap();
        apply_fetch_options(&mut sin_content_type, FetchOptions { method: Method::Post, headers: Vec::new(), body: Some(b"x".to_vec()), include_credentials: false });
        assert_eq!(sin_content_type.headers.get("Content-Type").map(String::as_str), Some("text/plain;charset=UTF-8"));

        let mut con_content_type = NetworkRequest::new("https://ejemplo.test/").unwrap();
        apply_fetch_options(
            &mut con_content_type,
            FetchOptions { method: Method::Post, headers: vec![("Content-Type".to_string(), "application/json".to_string())], body: Some(b"{}".to_vec()), include_credentials: false },
        );
        assert_eq!(con_content_type.headers.get("Content-Type").map(String::as_str), Some("application/json"), "no deberia pisar un Content-Type que el script ya puso");
    }

    /// El `TypeError` sincrono real del spec: `GET`/`HEAD` no pueden llevar
    /// cuerpo. Se usa una URL VALIDA a proposito (`https://ejemplo.test/`,
    /// nunca resuelta de verdad en el test): si el rechazo viniera de una
    /// URL invalida en vez del cuerpo, esta prueba no distinguiria las dos
    /// causas.
    #[test]
    fn a_get_request_with_a_body_rejects_synchronously_without_touching_the_network() {
        let mut context = Context::default();
        register_fetch(&mut context, Arc::new(NetworkEngine::new()), None).unwrap();
        let result = context.eval(Source::from_bytes("fetch('https://ejemplo.test/', {method: 'GET', body: 'no deberia llevar cuerpo'})")).unwrap();
        let promise = JsPromise::from_object(result.as_object().unwrap().clone()).unwrap();
        context.run_jobs();
        assert!(matches!(promise.state(), PromiseState::Rejected(_)), "GET con body deberia rechazar de inmediato, sin llegar a encolar ninguna peticion de red");
    }
}
