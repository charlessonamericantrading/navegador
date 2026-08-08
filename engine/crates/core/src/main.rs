use engine_core::pipeline::build_page_keeping_runtime;
use engine_core::platform::CrossPlatformTarget;

use engine_net::{NetworkEngine, NetworkRequest};
use engine_gfx::NativeEngineWindow;
use engine_layout::LayoutTreeBuilder;
use engine_text::FontSet;
use tracing::{info, warn};

const VIEWPORT_WIDTH: f32 = 1280.0;
const VIEWPORT_HEIGHT: f32 = 720.0;

/// Pagina local usada solo cuando la red falla. Se etiqueta explicitamente
/// como local para que nunca se confunda con una carga real: la version
/// anterior de este cliente fabricaba una respuesta de "exito" falsa cuando
/// la conexion fallaba, lo cual ocultaba el fallo real.
const OFFLINE_FALLBACK_HTML: &str = r#"<html>
<head><title>Sin conexion</title></head>
<body>
<h1>No se pudo conectar</h1>
<p>Esta es una pagina local de prueba, no contenido descargado de la red.</p>
<p>Esto aparece cuando la peticion de red falla (DNS, conexion rechazada, timeout...); el motor si soporta TLS (hyper+rustls, ver ARCHITECTURE.md).</p>
</body>
</html>"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    CrossPlatformTarget::print_target_info();

    // https://example.com no resuelve en DNS en este entorno de desarrollo
    // (verificado: info.cern.ch y google.com si resuelven, example.com no;
    // particularidad de red local, no del motor). Se usa info.cern.ch de
    // nuevo pero ahora por https, para probar TLS real con un host que
    // sabemos alcanzable.
    let target_url = "https://info.cern.ch/hypertext/WWW/TheProject.html";
    let net = NetworkEngine::new();

    let html = match NetworkRequest::new(target_url) {
        Ok(req) => match net.fetch(&req).await {
            Ok(res) if res.is_success() => {
                info!("Descarga real completada: {} ({} bytes)", target_url, res.body.len());
                res.text().unwrap_or_else(|_| OFFLINE_FALLBACK_HTML.to_string())
            }
            Ok(res) => {
                warn!("{} respondio {} (no 2xx); usando pagina local de prueba", target_url, res.status_code);
                OFFLINE_FALLBACK_HTML.to_string()
            }
            Err(e) => {
                warn!("Fallo la descarga real de {}: {e}. Usando pagina local de prueba.", target_url);
                OFFLINE_FALLBACK_HTML.to_string()
            }
        },
        Err(e) => {
            warn!("URL invalida: {e}. Usando pagina local de prueba.");
            OFFLINE_FALLBACK_HTML.to_string()
        }
    };

    // Se carga una sola vez aqui y se pasa tanto al pipeline (para medir
    // texto con metricas reales, ver LayoutTreeBuilder::build) como a la
    // ventana (para pintarlo) - antes solo la cargaba la ventana, y el
    // layout medía el texto con una aproximacion fija que ignoraba el
    // font-size real.
    let font_set = FontSet::load_default_sans_serif();
    match font_set.pick(false, false) {
        Some(_) => info!("Fuente de sistema cargada para medir y pintar texto real"),
        None => warn!("Sin fuente de sistema disponible: el texto se medira/pintara como aproximacion/relleno"),
    }

    // Pipeline real (parseo -> JS inline -> cascada -> layout), extraido a
    // core/pipeline.rs para poder invocarse tambien sin ventana (ver el
    // aviso ahi sobre por que: primer paso hacia poder medir el motor con
    // datos objetivos en vez de solo verificarlo a ojo). build_page ya
    // extrae y aplica el CSS real que la propia pagina declara en sus
    // <style> - antes de esto, aqui se pasaba un CSS de ejemplo hardcodeado
    // (background-color/color/font-size inventados) que no tenia nada que
    // ver con lo que info.cern.ch realmente declara; se quito porque seguir
    // mezclandolo con el CSS real de la pagina, sin ninguna etiqueta que
    // dijera cual es cual, era enganoso. No se inyecta nada extra aqui: si
    // la pagina no tiene <style>, se ve sin estilo, honestamente.
    // `build_page_keeping_runtime` (en vez de `build_page`) devuelve
    // TAMBIEN el `JsRuntime` vivo en vez de dropearlo - lo necesita
    // `on_click`, mas abajo, para poder disparar un "click" real sobre
    // listeners que un script haya registrado durante la carga inicial.
    // Sin `<link>`/`<script src>` externos aqui a proposito: este binario
    // es el arnes manual de pruebas contra una ventana nativa (ver
    // ARCHITECTURE.md), no el camino real del producto - `server.rs` (el
    // proceso NDJSON que si habla con el backend) es quien descarga
    // recursos externos de verdad.
    let (page, mut runtime) = build_page_keeping_runtime(&html, "", VIEWPORT_WIDTH, VIEWPORT_HEIGHT, Some(&font_set), &std::collections::HashMap::new(), &engine_layout::ImageMap::new());

    for result in &page.script_results {
        match result {
            Ok(value) => info!("[js] script ejecutado, resultado: {value}"),
            Err(e) => warn!("[js] error ejecutando script: {e}"),
        }
    }

    // `relayout` reconstruye el layout de verdad cuando la ventana cambia
    // de tamaño (ver el aviso en NativeEngineWindow::run) - captura clones
    // de dom_root/stylesheet/font en vez de moverlos, porque el layout
    // inicial (`page.layout_root`, abajo) y la propia ventana todavia
    // necesitan sus propias copias.
    let dom_root_for_relayout = page.dom_root.clone();
    let stylesheet_for_relayout = page.stylesheet.clone();
    let font_set_for_relayout = font_set.clone();
    let relayout = move |width: f32, height: f32| {
        LayoutTreeBuilder::build(&dom_root_for_relayout, &stylesheet_for_relayout, width, height, Some(&font_set_for_relayout), &engine_layout::ImageMap::new())
    };

    // `on_click` es el ultimo eslabon de la cadena clic-real -> evento-real
    // (ver ARCHITECTURE.md): `NativeEngineWindow::run` ya hace hit-testing
    // sobre el layout actual y le pasa el nodo encontrado; aqui se dispara
    // el "click" de verdad (`JsRuntime::dispatch_event`, invocable desde
    // Rust sin pasar por texto JS - ver `runtime.rs`) y se reconstruye el
    // layout por si el listener mutó el DOM, igual que `relayout` arriba
    // pero al mismo tamaño que el layout actual (`current_layout.
    // dimensions`), no uno nuevo. Captura su propio `runtime` (movido
    // aqui) y sus propios clones de dom_root/stylesheet/font - cada
    // closure necesita los suyos porque las tres (esta, `relayout`, y el
    // layout inicial de abajo) coexisten.
    let dom_root_for_click = page.dom_root.clone();
    let stylesheet_for_click = page.stylesheet.clone();
    let font_set_for_click = font_set.clone();
    let on_click = move |current_layout: &engine_layout::LayoutBox, x: f32, y: f32| {
        let node = current_layout.hit_test(x, y)?;
        match runtime.dispatch_event(&node, "click") {
            Ok(()) => info!("[click] evento 'click' disparado sobre el nodo bajo ({x}, {y})"),
            Err(e) => warn!("[click] fallo al disparar 'click' sobre el nodo bajo ({x}, {y}): {e}"),
        }
        Some(LayoutTreeBuilder::build(
            &dom_root_for_click,
            &stylesheet_for_click,
            current_layout.dimensions.width,
            current_layout.dimensions.height,
            Some(&font_set_for_click),
            &engine_layout::ImageMap::new(),
        ))
    };

    info!("Abriendo ventana nativa...");
    NativeEngineWindow::run("Navegador IA - Motor Nativo (Fase 1, en progreso)", page.layout_root, Some(font_set), engine_layout::ImageMap::new(), relayout, on_click);

    Ok(())
}
