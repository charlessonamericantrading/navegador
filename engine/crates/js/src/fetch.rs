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
//! Sin `options` (metodo/headers/body de la peticion - `engine-net` mismo
//! todavia no envia cuerpo de peticion en ninguna forma): solo
//! `fetch(url)`, siempre GET. Sin la clase `Headers` real
//! (`response.headers` es un objeto plano nombre-minuscula -> valor, no
//! `Headers` con `.get()`/`.has()`/iteracion). `response.json()` reusa el
//! `JSON.parse` REAL de Boa (invocado como si fuera JS) en vez de
//! reinventar un parser JSON propio.

use boa_engine::{
    job::NativeJob,
    js_string,
    object::{builtins::JsPromise, ObjectInitializer},
    property::Attribute,
    Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction,
};
use boa_gc::{Finalize, Trace};
use engine_net::{NetworkEngine, NetworkRequest, NetworkResponse};
use std::sync::Arc;

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
            let resolved = engine_net::request::resolve_against_page(&url, capture.1.as_deref());
            let request = match resolved.map(|(absolute, origin)| {
                NetworkRequest::new(absolute.as_str()).map(|mut r| {
                    r.origin = origin;
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
}
