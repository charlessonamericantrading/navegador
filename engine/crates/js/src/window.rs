//! El global `window` (Fase 6.4) - minimo a proposito: hoy solo lleva
//! `window.open(url)`, porque es lo unico que hay al otro lado capaz de
//! hacer algo real (`core::server::EngineServer::open_new_tab`, que abre
//! una pestaña de verdad desde la Fase 4.5).
//!
//! **Lo que este `window` NO es**: en un navegador real `window` ES el
//! objeto global (`window.foo` y `foo` son lo mismo, `window === this` en
//! el ambito global). Aqui es un objeto NORMAL registrado como una
//! propiedad global mas, asi que `var x = 1; window.x` da `undefined`.
//! Hacerlo de verdad exige que el objeto global de Boa sea un proxy con
//! semantica de `WindowProxy`, bastante mas trabajo, y ninguna de las
//! capacidades que este motor tiene hoy lo necesita - documentarlo como
//! limitacion es mas honesto que fingir un `window` completo.
//!
//! **Como llega una llamada de JS a abrir una pestaña de verdad**: el
//! runtime JS vive DENTRO de una pagina cargada (`LoadedPage::runtime`),
//! y abrir una pestaña es una operacion del SERVIDOR entero
//! (`EngineServer`, que es quien tiene la lista de pestañas) - una capa
//! por encima, inalcanzable desde dentro del `Context` de Boa. El puente
//! es esta cola compartida: `window.open(url)` solo APUNTA la URL aqui, y
//! `core::server` la drena despues de procesar el clic y abre las
//! pestañas de verdad. Ver `PendingWindowOpens`.

use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use std::sync::{Arc, Mutex};

/// URLs que `window.open(...)` ha pedido abrir y que todavia nadie ha
/// atendido. Compartida entre el `Context` de Boa (que solo escribe) y
/// `core::server` (que solo drena) - de ahi `Arc<Mutex<...>>` y no un
/// simple `Vec`.
///
/// Es una COLA, no un solo hueco: un mismo handler puede llamar a
/// `window.open` varias veces, y perder todas menos la ultima seria un
/// bug silencioso.
pub type PendingWindowOpens = Arc<Mutex<Vec<String>>>;

/// Envoltorio para poder capturar la cola dentro de un `NativeFunction` de
/// Boa: `from_copy_closure_with_captures` exige que lo capturado
/// implemente `boa_gc::Trace`, y `Arc<Mutex<Vec<String>>>` no lo hace (no
/// contiene NADA gestionado por el recolector de Boa, que es justo lo que
/// `empty_trace!` declara).
#[derive(Clone)]
struct PendingCapture(PendingWindowOpens);

unsafe impl boa_gc::Trace for PendingCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for PendingCapture {}

/// Registra `window` con `open(url)` real. Devuelve la cola compartida, que
/// el llamador (`JsRuntime::register_window`) guarda para poder drenarla.
pub fn register_window(context: &mut Context) -> JsResult<PendingWindowOpens> {
    let pending: PendingWindowOpens = Arc::new(Mutex::new(Vec::new()));

    let open_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured, context| {
            let Some(url) = args.first() else {
                // `window.open()` sin argumentos abre "about:blank" en un
                // navegador real. Aqui no hay ninguna pagina en blanco
                // navegable, asi que no se encola nada - mas honesto que
                // encolar una URL inventada que fallaria al pedirse.
                return Ok(JsValue::null());
            };
            let url = url.to_string(context)?.to_std_string_escaped();
            if url.trim().is_empty() {
                return Ok(JsValue::null());
            }
            if let Ok(mut queue) = captured.0.lock() {
                queue.push(url);
            }
            // `null`, no un objeto Window: este motor no tiene ningun
            // `WindowProxy` que devolver, y un objeto vacio fingiria una
            // referencia a la pestaña nueva que no permitiria hacer nada.
            // `null` es ademas lo que devuelve un navegador real cuando el
            // bloqueador de ventanas emergentes impide la apertura, asi que
            // el codigo de paginas reales que comprueba el resultado ya
            // sabe tratarlo.
            Ok(JsValue::null())
        },
        PendingCapture(pending.clone()),
    );

    let window = ObjectInitializer::new(context)
        .function(open_fn, js_string!("open"), 1)
        .build();

    // `window.getComputedStyle` es LA MISMA funcion que el global
    // `getComputedStyle` (ver `dom_bindings`), no una copia: codigo real usa
    // las dos formas indistintamente, y tener solo una hacia fallar a la
    // otra con "not a callable function".
    if let Ok(gcs) = context.global_object().get(js_string!("getComputedStyle"), context) {
        if !gcs.is_undefined() {
            window.set(js_string!("getComputedStyle"), gcs, false, context)?;
        }
    }

    // `window.dispatchEvent` como DELEGACION a `document.documentElement`
    // (Fase 42) - MISMO nodo al que ya delega `window.addEventListener`/
    // `removeEventListener` (el shim de `register_history`, ver su
    // aviso): sin esto, un `window.dispatchEvent(new Event('x'))` manual
    // fallaria con "not a function" aunque `window.addEventListener('x',
    // ...)` ya funcionara. `addEventListener`/`removeEventListener` NO se
    // tocan aqui - `register_history` (que corre DESPUES, ver
    // `core::scripting`) ya los engancha, y hacerlo tambien aqui solo
    // se pisaria a si mismo. Guardado tras comprobar que `document` y
    // `documentElement` existen: sin DOM (el arnes de tests, `window` sin
    // `bind_dom` previo) no hay nada a lo que delegar.
    if let Ok(document) = context.global_object().get(js_string!("document"), context) {
        if let Some(document_obj) = document.as_object() {
            if let Ok(document_element) = document_obj.get(js_string!("documentElement"), context) {
                if let Some(document_element_obj) = document_element.as_object() {
                    if let Ok(dispatch) = document_element_obj.get(js_string!("dispatchEvent"), context) {
                        if !dispatch.is_undefined() {
                            window.set(js_string!("dispatchEvent"), dispatch, false, context)?;
                        }
                    }
                }
            }
        }
    }

    context.register_global_property(js_string!("window"), window, Attribute::all())?;

    // `navigator` (Fase 39). Una cantidad enorme de codigo real lee
    // `navigator.userAgent` en su primera linea para decidir rutas de
    // compatibilidad; sin el objeto, eso era un TypeError que se llevaba
    // por delante el script entero.
    //
    // El User-Agent declara lo que este motor ES, no imita a Chrome. Eso
    // significa que el sniffing de navegador de algunas paginas no nos
    // reconocera - preferible a mentir: una pagina que crea estar hablando
    // con Chrome usaria APIs que aqui no existen y fallaria mas adelante y
    // de forma mas confusa.
    let navigator = ObjectInitializer::new(context)
        .property(
            js_string!("userAgent"),
            js_string!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) NavegadorIA/0.1 (motor propio en Rust)"),
            Attribute::all(),
        )
        .property(js_string!("language"), js_string!("es-ES"), Attribute::all())
        .property(js_string!("platform"), js_string!("Win32"), Attribute::all())
        // `onLine` en `true`: si el motor esta cargando la pagina, hay red.
        .property(js_string!("onLine"), JsValue::from(true), Attribute::all())
        .build();
    context.register_global_property(js_string!("navigator"), navigator, Attribute::all())?;

    // `getComputedStyle` es del spec un metodo de `window`, pero se
    // registra como GLOBAL en `DomBindings::register` (Fase 8), que es
    // donde nace el snapshot de layout que consulta. Aqui se cuelga
    // ademas de `window` para que la forma canonica
    // (`window.getComputedStyle(el)`, que es como lo escribe casi todo el
    // codigo real) funcione igual. Guardado porque el orden de registro no
    // esta garantizado: un `JsRuntime` puede tener `window` sin haber
    // enlazado ningun DOM (`register_window` sin `bind_dom`), y entonces
    // `getComputedStyle` no existe y no hay nada que colgar.
    context.eval(boa_engine::Source::from_bytes(
        b"if (typeof getComputedStyle !== 'undefined') { window.getComputedStyle = getComputedStyle; }" as &[u8],
    ))?;

    Ok(pending)
}

#[cfg(test)]
mod tests {
    use crate::runtime::JsRuntime;

    #[test]
    fn window_open_queues_the_url_instead_of_opening_anything_by_itself() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime.eval("window.open('https://ejemplo.test/nueva')").expect("window.open no deberia lanzar");
        assert_eq!(runtime.take_pending_window_opens(), vec!["https://ejemplo.test/nueva".to_string()]);
    }

    #[test]
    fn taking_the_queue_empties_it_so_the_same_url_no_se_abre_dos_veces() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime.eval("window.open('https://ejemplo.test/una')").expect("no deberia lanzar");
        assert_eq!(runtime.take_pending_window_opens().len(), 1);
        assert!(
            runtime.take_pending_window_opens().is_empty(),
            "drenar la cola deberia vaciarla - si no, cada clic reabriria todas las pestañas de los clics anteriores"
        );
    }

    #[test]
    fn several_window_open_calls_are_all_queued_in_order() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime
            .eval("window.open('https://ejemplo.test/1'); window.open('https://ejemplo.test/2')")
            .expect("no deberia lanzar");
        assert_eq!(
            runtime.take_pending_window_opens(),
            vec!["https://ejemplo.test/1".to_string(), "https://ejemplo.test/2".to_string()]
        );
    }

    #[test]
    fn window_open_without_a_usable_url_queues_nothing() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        runtime.eval("window.open(); window.open('   ')").expect("no deberia lanzar");
        assert!(
            runtime.take_pending_window_opens().is_empty(),
            "sin URL utilizable no deberia encolarse nada, en vez de inventarse una"
        );
    }

    #[test]
    fn window_open_returns_null_not_a_fake_window_object() {
        let mut runtime = JsRuntime::new();
        runtime.register_window().expect("window deberia registrarse");
        let result = runtime.eval("window.open('https://ejemplo.test/x') === null").expect("no deberia lanzar");
        assert_eq!(result, "true", "deberia devolver null, no un objeto Window fingido");
    }

    /// Sin `register_window`, `window` no existe en absoluto - la respuesta
    /// honesta cuando no hay nada al otro lado capaz de abrir una pestaña
    /// (mismo criterio que `fetch` sin red, ver `JsRuntime::register_fetch`).
    #[test]
    fn window_is_not_defined_at_all_unless_it_was_registered() {
        let mut runtime = JsRuntime::new();
        assert!(
            runtime.eval("typeof window").is_ok_and(|t| t == "\"undefined\""),
            "sin registrar, window no deberia existir"
        );
    }

    /// `window.dispatchEvent` delega en `document.documentElement`, el
    /// MISMO nodo al que ya delega `window.addEventListener` (el shim de
    /// `register_history`) - un listener puesto por una via tiene que
    /// enterarse de un evento disparado por la otra.
    #[test]
    fn window_dispatch_event_reaches_a_listener_added_via_window_add_event_listener() {
        use engine_dom::HtmlParser;
        let dom = HtmlParser::parse("<html><body></body></html>");
        let mut runtime = JsRuntime::new();
        runtime.bind_dom(dom).expect("bind_dom no deberia fallar");
        runtime.register_window().expect("window deberia registrarse");
        runtime.register_history().expect("history deberia registrarse (ahi vive el shim)");

        runtime.eval("var visto = false; window.addEventListener('miEvento', function() { visto = true; });").expect("registrar el listener deberia ser JS valido");
        runtime.eval("window.dispatchEvent(new Event('miEvento'))").expect("dispatchEvent no deberia lanzar");
        assert_eq!(runtime.eval("visto").unwrap(), "true", "el listener puesto via window.addEventListener deberia dispararse con window.dispatchEvent");
    }
}

// ---------------------------------------------------------------------------
// El entorno de `window` (Fase 45, tarea C7 del plan)
// ---------------------------------------------------------------------------

/// Envoltorio para capturar el buzon de layout dentro de un `NativeFunction`
/// de Boa. Mismo patron y mismo motivo que `PendingCapture`.
#[derive(Clone)]
struct EntornoCapture(crate::cssom::LayoutSnapshot);
impl boa_gc::Finalize for EntornoCapture {}
unsafe impl boa_gc::Trace for EntornoCapture {
    boa_gc::empty_trace!();
}

/// Registra las propiedades de `window` que describen el ENTORNO:
/// `innerWidth`/`innerHeight`, `devicePixelRatio`, `scrollX`/`scrollY`,
/// `scrollTo`/`scrollBy` y `matchMedia`.
///
/// Por que van aparte de `register_window` y despues: necesitan el buzon de
/// layout, que solo existe una vez que `bind_dom` ha corrido. Registrar aqui un
/// `innerWidth` sin buzon devolveria siempre cero, que es peor que no tenerlo:
/// una pagina que reparte espacio con `innerWidth` produciria un diseño de
/// ancho cero en vez de fallar de forma visible.
///
/// Todas son ACCESSORS y no propiedades fijas: el viewport cambia con cada
/// `resize` y el scroll con cada rueda del raton. Una foto tomada al cargar
/// daria respuestas obsoletas en cuanto el usuario tocara la ventana.
pub fn register_window_environment(
    context: &mut Context,
    snapshot: crate::cssom::LayoutSnapshot,
) -> JsResult<()> {
    let captura = EntornoCapture(snapshot);

    let inner_width = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], c: &EntornoCapture, _context| {
            let ancho = c.0.read().map(|d| d.viewport_width).unwrap_or(0.0);
            Ok(JsValue::from(ancho as f64))
        },
        captura.clone(),
    );
    let inner_height = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], c: &EntornoCapture, _context| {
            let alto = c.0.read().map(|d| d.viewport_height).unwrap_or(0.0);
            Ok(JsValue::from(alto as f64))
        },
        captura.clone(),
    );
    let scroll_y = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], c: &EntornoCapture, _context| {
            let y = c.0.read().map(|d| d.scroll_offset_y).unwrap_or(0.0);
            Ok(JsValue::from(y as f64))
        },
        captura.clone(),
    );

    // `matchMedia` evalua la MISMA condicion que `@media`, con el mismo parser
    // (`engine_css::parse_media_condition`). Tener dos evaluadores seria
    // garantizar que un dia respondan distinto sobre la misma consulta, y ese
    // es justo el fallo que nadie diagnostica.
    let match_media = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], c: &EntornoCapture, context| {
            let consulta = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            let ancho = c.0.read().map(|d| d.viewport_width).unwrap_or(0.0);
            let condicion = engine_css::parse_media_condition(&consulta);
            let coincide = condicion.matches(ancho);

            // `addEventListener`/`addListener` existen y NO hacen nada, y eso
            // es deliberado: un `MediaQueryList` no dispara aqui porque el
            // motor no reevalua consultas al redimensionar. Registrar el
            // metodo evita el `TypeError` que mataria el script; no
            // registrarlo lo mataria. Que ademas no dispare esta declarado en
            // `huecos_sin_resolver.md` - a diferencia de un observador, un
            // listener de media query que no dispara deja a la pagina en su
            // estado inicial, que es un estado valido, no colgada.
            let sin_efecto =
                NativeFunction::from_fn_ptr(|_this, _args, _context| Ok(JsValue::undefined()));

            Ok(ObjectInitializer::new(context)
                .property(js_string!("matches"), JsValue::from(coincide), Attribute::all())
                .property(js_string!("media"), js_string!(consulta), Attribute::all())
                .function(sin_efecto.clone(), js_string!("addEventListener"), 2)
                .function(sin_efecto.clone(), js_string!("removeEventListener"), 2)
                .function(sin_efecto.clone(), js_string!("addListener"), 1)
                .function(sin_efecto, js_string!("removeListener"), 1)
                .build()
                .into())
        },
        captura.clone(),
    );

    // `scrollTo`/`scrollBy` aceptan la llamada sin mover nada: mover el scroll
    // de verdad exige un camino JS -> servidor que todavia no existe (el
    // scroll lo manda hoy el servidor hacia JS, no al reves). Se registran
    // porque su AUSENCIA mata el script, y no mover es un resultado que la
    // pagina puede observar y sobrevivir - al contrario que un observador que
    // nunca dispara, que la deja esperando. Declarado en
    // `huecos_sin_resolver.md`.
    let scroll_to = NativeFunction::from_fn_ptr(|_this, _args, _context| {
        tracing::info!("[js] window.scrollTo/scrollBy aceptado pero sin efecto todavia");
        Ok(JsValue::undefined())
    });

    let global = context.global_object();
    let window = global.get(js_string!("window"), context)?;
    let Some(window) = window.as_object().cloned() else {
        return Ok(());
    };

    let accessors: [(&str, NativeFunction); 3] = [
        ("innerWidth", inner_width),
        ("innerHeight", inner_height),
        ("scrollY", scroll_y),
    ];
    for (nombre, funcion) in accessors {
        let getter = FunctionObjectBuilder::new(context.realm(), funcion)
            .name(js_string!(format!("get {nombre}")))
            .length(0)
            .constructor(false)
            .build();
        let descriptor = boa_engine::property::PropertyDescriptor::builder()
            .get(getter)
            .enumerable(true)
            .configurable(true)
            .build();
        window.define_property_or_throw(js_string!(nombre), descriptor, context)?;
    }

    // `pageYOffset` es el nombre antiguo de `scrollY` y sigue muy usado.
    let page_y = window.get(js_string!("scrollY"), context)?;
    window.set(js_string!("pageYOffset"), page_y, false, context)?;

    // `scrollX` es siempre 0: este motor no tiene scroll horizontal. Es un
    // valor CIERTO, no un relleno.
    window.set(js_string!("scrollX"), JsValue::from(0.0), false, context)?;
    window.set(js_string!("pageXOffset"), JsValue::from(0.0), false, context)?;
    // 1.0 real: el motor rasteriza a 1 pixel fisico por pixel CSS.
    window.set(js_string!("devicePixelRatio"), JsValue::from(1.0), false, context)?;

    let match_media_fn = FunctionObjectBuilder::new(context.realm(), match_media)
        .name(js_string!("matchMedia"))
        .length(1)
        .constructor(false)
        .build();
    window.set(js_string!("matchMedia"), match_media_fn.clone(), false, context)?;
    context.register_global_property(
        js_string!("matchMedia"),
        match_media_fn,
        Attribute::all(),
    )?;

    let scroll_to_fn = FunctionObjectBuilder::new(context.realm(), scroll_to)
        .name(js_string!("scrollTo"))
        .length(2)
        .constructor(false)
        .build();
    window.set(js_string!("scrollTo"), scroll_to_fn.clone(), false, context)?;
    window.set(js_string!("scrollBy"), scroll_to_fn.clone(), false, context)?;
    window.set(js_string!("scroll"), scroll_to_fn, false, context)?;

    Ok(())
}
