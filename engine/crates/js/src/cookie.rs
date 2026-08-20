//! `document.cookie` real (Fase 24): accessor con getter Y setter sobre el
//! MISMO `CookieStore` que ya usan las peticiones de red (`engine-net::
//! cookie`, ver su aviso de modulo) - no un mapa paralelo que pudiera
//! desincronizarse de lo que de verdad viaja en la cabecera `Cookie:`.
//!
//! El getter usa `NetworkEngine::cookie_header_for_js`, que filtra toda
//! cookie `HttpOnly` - la proteccion real que esa bandera aporta (RFC 6265
//! §8.6), inexistente hasta ahora porque no habia ningun `document.cookie`
//! del que protegerla. El setter usa `NetworkEngine::set_cookie_from_js`,
//! que parsea con la MISMA gramatica que un `Set-Cookie` de servidor
//! (`Domain`/`Path`/`Max-Age`/`Secure`/`SameSite`) pero un script nunca
//! puede marcar `HttpOnly` - igual que un navegador real.
//!
//! `page_url: None` (documento sin URL propia, p.ej. un runtime construido
//! sin `core::server` detras) deja `document.cookie` como cadena vacia
//! siempre y su setter como no-op silencioso: no hay origen contra el que
//! guardar nada, igual criterio que `fetch`/`XMLHttpRequest` sin red (ver
//! `fetch.rs`/`xhr.rs`).
//!
//! Requiere que `document` ya exista en el `Context` (`bind_dom` ya haya
//! corrido, ver `dom_bindings::DocumentBindings::register`) - sin eso es un
//! no-op honesto, no un panic: un runtime sin DOM no tiene donde colgar el
//! accessor.

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::property::PropertyDescriptor;
use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};
use boa_gc::{Finalize, Trace};
use engine_net::NetworkEngine;
use std::sync::Arc;

#[derive(Clone)]
struct CookieCapture(Arc<NetworkEngine>, Option<String>);

impl Finalize for CookieCapture {}
unsafe impl Trace for CookieCapture {
    boa_gc::empty_trace!();
}

/// Registra el accessor `document.cookie`. Ver el aviso del modulo para el
/// diseño completo y las simplificaciones declaradas.
pub fn register_cookie(context: &mut Context, network: Arc<NetworkEngine>, page_url: Option<String>) -> JsResult<()> {
    let document = context.global_object().get(js_string!("document"), context)?;
    let Some(document_obj) = document.as_object().cloned() else {
        return Ok(());
    };

    let capture = CookieCapture(network, page_url);

    let getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, captured: &CookieCapture, _context| {
            let Some(url) = captured.1.as_deref() else { return Ok(JsValue::from(js_string!(""))) };
            Ok(JsValue::from(js_string!(captured.0.cookie_header_for_js(url))))
        },
        capture.clone(),
    );
    let getter_fn = FunctionObjectBuilder::new(context.realm(), getter).name(js_string!("get cookie")).length(0).constructor(false).build();

    let setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured: &CookieCapture, context| {
            let Some(url) = captured.1.as_deref() else { return Ok(JsValue::undefined()) };
            let raw = args.first().cloned().unwrap_or_else(JsValue::undefined).to_string(context)?.to_std_string_escaped();
            captured.0.set_cookie_from_js(&raw, url);
            Ok(JsValue::undefined())
        },
        capture,
    );
    let setter_fn = FunctionObjectBuilder::new(context.realm(), setter).name(js_string!("set cookie")).length(1).constructor(false).build();

    let descriptor = PropertyDescriptor::builder().get(getter_fn).set(setter_fn).enumerable(true).configurable(true).build();
    document_obj.define_property_or_throw(js_string!("cookie"), descriptor, context)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::JsRuntime;
    use engine_dom::HtmlParser;

    fn runtime_with_network_at(page_url: &str) -> (JsRuntime, Arc<NetworkEngine>) {
        let dom = HtmlParser::parse("<html><body></body></html>");
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom).expect("bind_dom deberia funcionar");
        let network = Arc::new(NetworkEngine::new());
        runtime.register_cookie(network.clone(), Some(page_url.to_string())).expect("registrar document.cookie no deberia fallar");
        (runtime, network)
    }

    #[test]
    fn reading_document_cookie_reflects_a_cookie_the_server_set() {
        let (mut runtime, network) = runtime_with_network_at("https://ejemplo.test/");
        network.set_cookie_from_js("tema=oscuro", "https://ejemplo.test/");
        assert_eq!(runtime.eval("document.cookie").unwrap(), "\"tema=oscuro\"");
    }

    #[test]
    fn writing_document_cookie_from_js_is_visible_to_a_later_network_request() {
        let (mut runtime, network) = runtime_with_network_at("https://ejemplo.test/");
        runtime.eval("document.cookie = 'consentimiento=si; Path=/'").expect("no deberia lanzar");
        assert_eq!(network.cookie_header_for_js("https://ejemplo.test/"), "consentimiento=si");
    }

    /// El propio setter de `document.cookie` no puede esconderse una
    /// cookie a si mismo: `HttpOnly` escrita desde JS se ignora en vez de
    /// aplicarse (ver `engine_net::cookie::CookieStore::set_from_js`), asi
    /// que la cookie sigue siendo legible por el mismo `document.cookie`
    /// que la creo - igual que un navegador real.
    #[test]
    fn writing_http_only_from_document_cookie_is_ignored_not_applied() {
        let (mut runtime, _network) = runtime_with_network_at("https://ejemplo.test/");
        runtime.eval("document.cookie = 'a=1; HttpOnly'").expect("no deberia lanzar");
        assert_eq!(runtime.eval("document.cookie").unwrap(), "\"a=1\"", "un script no deberia poder crear una cookie HttpOnly para si mismo");
    }

    #[test]
    fn document_cookie_without_a_page_url_is_always_empty_and_the_setter_is_a_no_op() {
        let dom = HtmlParser::parse("<html><body></body></html>");
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom).expect("bind_dom deberia funcionar");
        let network = Arc::new(NetworkEngine::new());
        runtime.register_cookie(network, None).expect("registrar sin page_url no deberia fallar");

        assert_eq!(runtime.eval("document.cookie").unwrap(), "\"\"");
        runtime.eval("document.cookie = 'a=1'").expect("no deberia lanzar aunque no guarde nada");
        assert_eq!(runtime.eval("document.cookie").unwrap(), "\"\"", "sin page_url no hay donde guardar la cookie");
    }
}
