//! `location` (y `window.location`): la URL de la pagina, legible por
//! partes, y navegable.
//!
//! Es de las cosas que TODA pagina real da por sentadas. Hasta ahora no
//! existia, y eso no fallaba solo en el sitio donde se usaba: un
//! `location.href` sobre un global inexistente lanza `ReferenceError`, que
//! aborta el script ENTERO. Un solo `if (location.hash)` en una linea
//! cualquiera se llevaba por delante toda la inicializacion de la pagina.
//!
//! **Navegar** (`location.href = "..."`, `assign`, `replace`, `reload`) no
//! se puede hacer desde dentro del `Context` de Boa: cargar un documento es
//! una operacion del SERVIDOR, una capa por encima. Se usa el mismo puente
//! que `window.open` y `history.pushState`: una cola compartida que el
//! runtime solo ESCRIBE y `core::server` drena y ejecuta de verdad. Ver
//! `PendingNavigations`.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};
use std::sync::{Arc, Mutex};

/// Una navegacion que un script ha pedido y que todavia nadie ha atendido.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingNavigation {
    /// La URL tal cual la escribio el script, SIN resolver: quien la
    /// resuelve es el servidor, que es el unico que conoce con certeza la
    /// URL final de la pagina (tras redirecciones).
    pub raw_url: String,
    /// `location.replace(...)` sustituye la entrada actual del historial en
    /// vez de añadir una nueva - la diferencia se nota al pulsar "atras".
    pub replace_current_entry: bool,
}

pub type PendingNavigations = Arc<Mutex<Vec<PendingNavigation>>>;

#[derive(Clone)]
struct NavigationCapture(PendingNavigations, bool);

unsafe impl boa_gc::Trace for NavigationCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for NavigationCapture {}

#[derive(Clone)]
struct UrlCapture(String);

unsafe impl boa_gc::Trace for UrlCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for UrlCapture {}

/// Las partes de una URL que `location` expone, ya troceadas.
///
/// Se calculan UNA vez al registrar el objeto, no en cada acceso: la URL de
/// la pagina no cambia mientras el documento vive (un `pushState` la
/// reescribe, pero eso crea un documento nuevo desde el punto de vista de
/// este registro).
struct LocationParts {
    href: String,
    protocol: String,
    host: String,
    hostname: String,
    port: String,
    pathname: String,
    search: String,
    hash: String,
    origin: String,
}

fn split_url(url: &str) -> LocationParts {
    match url::Url::parse(url) {
        Ok(u) => {
            let hostname = u.host_str().unwrap_or_default().to_string();
            let port = u.port().map(|p| p.to_string()).unwrap_or_default();
            let host = if port.is_empty() { hostname.clone() } else { format!("{hostname}:{port}") };
            LocationParts {
                // `protocol` incluye los dos puntos (`"https:"`), asi es en
                // el spec real - un detalle que rompe comparaciones si se
                // omite.
                protocol: format!("{}:", u.scheme()),
                host,
                hostname,
                port,
                pathname: u.path().to_string(),
                // `search` y `hash` incluyen su signo inicial, y son cadena
                // VACIA (no "?" ni "#") cuando no hay nada - tambien como el
                // spec.
                search: u.query().map(|q| format!("?{q}")).unwrap_or_default(),
                hash: u.fragment().map(|f| format!("#{f}")).unwrap_or_default(),
                origin: engine_net::storage::origin_of(&u),
                href: u.to_string(),
            }
        }
        // Sin URL parseable (un documento construido en memoria, sin
        // origen) se expone `about:blank`, que es lo que un navegador real
        // reporta en ese caso, en vez de inventarse partes.
        Err(_) => LocationParts {
            href: "about:blank".to_string(),
            protocol: "about:".to_string(),
            host: String::new(),
            hostname: String::new(),
            port: String::new(),
            pathname: "blank".to_string(),
            search: String::new(),
            hash: String::new(),
            origin: "null".to_string(),
        },
    }
}

/// Que parte de la URL toca un setter distinto de `href`.
#[derive(Debug, Clone, Copy)]
enum UrlPart {
    Pathname,
    Search,
    Hash,
    Protocol,
    Host,
    Hostname,
    Port,
}

impl UrlPart {
    /// Aplica el nuevo valor sobre `url`. Devuelve `false` si el valor no
    /// es valido para esta parte (un puerto no numerico, un esquema que el
    /// crate `url` no permite cambiar) - en ese caso no se navega, igual
    /// que un navegador real ignora una asignacion invalida.
    fn apply(self, url: &mut url::Url, valor: &str) -> bool {
        match self {
            Self::Pathname => {
                url.set_path(valor);
                true
            }
            Self::Search => {
                let q = valor.strip_prefix('?').unwrap_or(valor);
                url.set_query(if q.is_empty() { None } else { Some(q) });
                true
            }
            Self::Hash => {
                let h = valor.strip_prefix('#').unwrap_or(valor);
                url.set_fragment(if h.is_empty() { None } else { Some(h) });
                true
            }
            Self::Protocol => url.set_scheme(valor.trim_end_matches(':')).is_ok(),
            Self::Host => match valor.split_once(':') {
                Some((host, puerto)) => {
                    let puerto = match puerto.parse::<u16>() {
                        Ok(p) => Some(p),
                        Err(_) => return false,
                    };
                    url.set_host(Some(host)).is_ok() && url.set_port(puerto).is_ok()
                }
                None => url.set_host(Some(valor)).is_ok(),
            },
            Self::Hostname => url.set_host(Some(valor)).is_ok(),
            Self::Port => {
                let puerto = if valor.is_empty() {
                    None
                } else {
                    match valor.parse::<u16>() {
                        Ok(p) => Some(p),
                        Err(_) => return false,
                    }
                };
                url.set_port(puerto).is_ok()
            }
        }
    }
}

#[derive(Clone)]
struct PartSetterCapture(PendingNavigations, String, UrlPart);

unsafe impl boa_gc::Trace for PartSetterCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for PartSetterCapture {}

/// Construye el par getter/setter de una parte de `location` que no es
/// `href` (`pathname`, `search`, `hash`, `protocol`, `host`, `hostname`,
/// `port`).
///
/// Antes estas siete eran propiedades de DATOS normales, escribibles pero
/// sin ningun setter detras: `location.hash = 'x'` se limitaba a guardar la
/// cadena en la propiedad, sin navegar y sin que `location.href` reflejara
/// el cambio - peor que un `ReferenceError`, porque nada delataba el fallo.
/// Asignar cualquiera de las siete reconstruye la URL completa (partiendo
/// de la URL real de la pagina) y navega, igual que hace un navegador real.
fn part_accessor(
    context: &mut Context,
    base_href: &str,
    part: UrlPart,
    valor_inicial: &str,
    pending: PendingNavigations,
) -> JsResult<(boa_engine::object::builtins::JsFunction, boa_engine::object::builtins::JsFunction)> {
    let getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &UrlCapture, _context| Ok(JsValue::from(js_string!(capture.0.as_str()))),
        UrlCapture(valor_inicial.to_string()),
    );
    let getter = boa_engine::object::builtins::JsFunction::from_object(
        boa_engine::object::FunctionObjectBuilder::new(context.realm(), getter)
            .name(js_string!("get"))
            .length(0)
            .constructor(false)
            .build()
            .into(),
    )
    .expect("el builder de Boa devuelve siempre un objeto funcion");

    let setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &PartSetterCapture, context| {
            let Some(value) = args.first() else { return Ok(JsValue::undefined()) };
            let valor = value.to_string(context)?.to_std_string_escaped();
            if let Ok(mut url) = url::Url::parse(&capture.1) {
                if capture.2.apply(&mut url, &valor) {
                    capture.0.lock().unwrap().push(PendingNavigation { raw_url: url.to_string(), replace_current_entry: false });
                }
            }
            Ok(JsValue::undefined())
        },
        PartSetterCapture(pending, base_href.to_string(), part),
    );
    let setter = boa_engine::object::builtins::JsFunction::from_object(
        boa_engine::object::FunctionObjectBuilder::new(context.realm(), setter)
            .name(js_string!("set"))
            .length(1)
            .constructor(false)
            .build()
            .into(),
    )
    .expect("el builder de Boa devuelve siempre un objeto funcion");

    Ok((getter, setter))
}

/// Registra el global `location` (y lo devuelve para que quien llame pueda
/// colgarlo tambien de `window`), mas la cola de navegaciones pendientes.
///
/// `page_url` es la URL FINAL de la pagina (tras redirecciones); `None` en
/// un documento sin origen.
pub fn register_location(context: &mut Context, page_url: Option<String>) -> JsResult<PendingNavigations> {
    let pending: PendingNavigations = Arc::new(Mutex::new(Vec::new()));
    let parts = split_url(page_url.as_deref().unwrap_or(""));

    // Setter de `href`: asignarle una URL NAVEGA, no cambia una cadena.
    // Es la forma mas comun de navegar desde JS con diferencia.
    let href_setter = NativeFunction::from_copy_closure_with_captures(
        |_this, args, capture: &NavigationCapture, context| {
            let Some(value) = args.first() else { return Ok(JsValue::undefined()) };
            let url = value.to_string(context)?.to_std_string_escaped();
            capture.0.lock().unwrap().push(PendingNavigation { raw_url: url, replace_current_entry: capture.1 });
            Ok(JsValue::undefined())
        },
        NavigationCapture(pending.clone(), false),
    );
    let href_setter = boa_engine::object::builtins::JsFunction::from_object(
        boa_engine::object::FunctionObjectBuilder::new(context.realm(), href_setter)
            .name(js_string!("set href"))
            .length(1)
            .constructor(false)
            .build()
            .into(),
    )
    .expect("el builder de Boa devuelve siempre un objeto funcion");

    let href_getter = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &UrlCapture, _context| Ok(JsValue::from(js_string!(capture.0.as_str()))),
        UrlCapture(parts.href.clone()),
    );
    let href_getter = boa_engine::object::builtins::JsFunction::from_object(
        boa_engine::object::FunctionObjectBuilder::new(context.realm(), href_getter)
            .name(js_string!("get href"))
            .length(0)
            .constructor(false)
            .build()
            .into(),
    )
    .expect("el builder de Boa devuelve siempre un objeto funcion");

    let navegar = |replace: bool, cola: PendingNavigations| {
        NativeFunction::from_copy_closure_with_captures(
            |_this, args, capture: &NavigationCapture, context| {
                let Some(value) = args.first() else { return Ok(JsValue::undefined()) };
                let url = value.to_string(context)?.to_std_string_escaped();
                capture.0.lock().unwrap().push(PendingNavigation { raw_url: url, replace_current_entry: capture.1 });
                Ok(JsValue::undefined())
            },
            NavigationCapture(cola, replace),
        )
    };

    // `reload()` es una navegacion a la MISMA URL. Se apunta como tal en vez
    // de como una operacion aparte: el servidor no necesita distinguirla, y
    // asi no hay dos caminos que mantener.
    let reload = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, capture: &UrlCapture, _context| Ok(JsValue::from(js_string!(capture.0.as_str()))),
        UrlCapture(parts.href.clone()),
    );
    let _ = reload;
    let reload_cola = pending.clone();
    let reload_url = parts.href.clone();
    let reload = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, capture: &ReloadCapture, _context| {
            capture.0.lock().unwrap().push(PendingNavigation { raw_url: capture.1.clone(), replace_current_entry: true });
            Ok(JsValue::undefined())
        },
        ReloadCapture(reload_cola, reload_url),
    );

    let (protocol_get, protocol_set) = part_accessor(context, &parts.href, UrlPart::Protocol, &parts.protocol, pending.clone())?;
    let (host_get, host_set) = part_accessor(context, &parts.href, UrlPart::Host, &parts.host, pending.clone())?;
    let (hostname_get, hostname_set) = part_accessor(context, &parts.href, UrlPart::Hostname, &parts.hostname, pending.clone())?;
    let (port_get, port_set) = part_accessor(context, &parts.href, UrlPart::Port, &parts.port, pending.clone())?;
    let (pathname_get, pathname_set) = part_accessor(context, &parts.href, UrlPart::Pathname, &parts.pathname, pending.clone())?;
    let (search_get, search_set) = part_accessor(context, &parts.href, UrlPart::Search, &parts.search, pending.clone())?;
    let (hash_get, hash_set) = part_accessor(context, &parts.href, UrlPart::Hash, &parts.hash, pending.clone())?;

    let location = ObjectInitializer::new(context)
        .accessor(js_string!("href"), Some(href_getter), Some(href_setter), Attribute::all())
        .accessor(js_string!("protocol"), Some(protocol_get), Some(protocol_set), Attribute::all())
        .accessor(js_string!("host"), Some(host_get), Some(host_set), Attribute::all())
        .accessor(js_string!("hostname"), Some(hostname_get), Some(hostname_set), Attribute::all())
        .accessor(js_string!("port"), Some(port_get), Some(port_set), Attribute::all())
        .accessor(js_string!("pathname"), Some(pathname_get), Some(pathname_set), Attribute::all())
        .accessor(js_string!("search"), Some(search_get), Some(search_set), Attribute::all())
        .accessor(js_string!("hash"), Some(hash_get), Some(hash_set), Attribute::all())
        .property(js_string!("origin"), js_string!(parts.origin.as_str()), Attribute::all())
        .function(navegar(false, pending.clone()), js_string!("assign"), 1)
        .function(navegar(true, pending.clone()), js_string!("replace"), 1)
        .function(reload, js_string!("reload"), 0)
        .build();

    context.register_global_property(js_string!("location"), location.clone(), Attribute::all())?;

    // `window.location` es el MISMO objeto, no una copia: comparar
    // `window.location === location` da `true` en un navegador real, y
    // codigo real lo hace.
    if let Ok(window) = context.global_object().get(js_string!("window"), context) {
        if let Some(window) = window.as_object() {
            window.set(js_string!("location"), location, false, context)?;
        }
    }

    Ok(pending)
}

#[derive(Clone)]
struct ReloadCapture(PendingNavigations, String);

unsafe impl boa_gc::Trace for ReloadCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for ReloadCapture {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn las_partes_de_una_url_se_trocean_como_el_spec() {
        let p = split_url("https://ejemplo.test:8443/ruta/pagina?a=1&b=2#seccion");
        assert_eq!(p.protocol, "https:", "protocol incluye los dos puntos");
        assert_eq!(p.hostname, "ejemplo.test");
        assert_eq!(p.port, "8443");
        assert_eq!(p.host, "ejemplo.test:8443", "host incluye el puerto, hostname no");
        assert_eq!(p.pathname, "/ruta/pagina");
        assert_eq!(p.search, "?a=1&b=2", "search incluye el interrogante");
        assert_eq!(p.hash, "#seccion", "hash incluye la almohadilla");
    }

    #[test]
    fn sin_busqueda_ni_fragmento_esas_partes_son_cadena_vacia() {
        let p = split_url("https://ejemplo.test/ruta");
        assert_eq!(p.search, "", "sin query, `search` es vacio - no \"?\"");
        assert_eq!(p.hash, "", "sin fragmento, `hash` es vacio - no \"#\"");
        assert_eq!(p.port, "", "sin puerto explicito, `port` es vacio");
        assert_eq!(p.host, "ejemplo.test");
    }

    #[test]
    fn una_url_no_parseable_reporta_about_blank() {
        let p = split_url("");
        assert_eq!(p.href, "about:blank");
        assert_eq!(p.origin, "null");
    }

    /// Asignar a `location.hash`/`pathname`/`search` (no solo `href`) tiene
    /// que navegar de verdad. Antes eran propiedades de datos normales sin
    /// ningun setter: la asignacion se perdia en silencio, sin navegar y
    /// sin que `location.href` reflejara nada - peor que un
    /// `ReferenceError`, porque nada delataba el fallo.
    #[test]
    fn asignar_hash_pathname_o_search_navega_de_verdad() {
        let mut context = Context::default();
        let pending = register_location(&mut context, Some("https://ejemplo.test/pagina?a=1#viejo".to_string()))
            .expect("registrar location no deberia fallar");

        context.eval(boa_engine::Source::from_bytes("location.hash = 'nuevo';")).expect("asignar hash deberia ser JS valido");
        let despues_hash = pending.lock().unwrap().last().cloned();
        assert_eq!(
            despues_hash.map(|n| n.raw_url),
            Some("https://ejemplo.test/pagina?a=1#nuevo".to_string()),
            "asignar hash deberia navegar a la MISMA url con el fragmento cambiado"
        );

        context.eval(boa_engine::Source::from_bytes("location.pathname = '/otra';")).expect("asignar pathname deberia ser JS valido");
        let despues_pathname = pending.lock().unwrap().last().cloned();
        assert_eq!(
            despues_pathname.map(|n| n.raw_url),
            Some("https://ejemplo.test/otra?a=1#viejo".to_string()),
            "asignar pathname deberia navegar cambiando solo la ruta"
        );

        context.eval(boa_engine::Source::from_bytes("location.search = 'b=2';")).expect("asignar search deberia ser JS valido");
        let despues_search = pending.lock().unwrap().last().cloned();
        assert_eq!(
            despues_search.map(|n| n.raw_url),
            Some("https://ejemplo.test/pagina?b=2#viejo".to_string()),
            "asignar search deberia navegar cambiando solo la query, aceptando el valor sin el interrogante inicial"
        );
    }
}
