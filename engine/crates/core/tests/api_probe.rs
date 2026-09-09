//! Ejecuta la sonda de APIs (`engine/tests/probes/api-probe.html`) y convierte
//! su resultado en un numero vigilado.
//!
//! Por que es un test y no un script suelto: la Fase 39 midio 26/28 con una
//! sonda que no quedo en el repositorio, asi que el numero no se pudo volver a
//! comprobar ni comparar. Un dato que no se puede reproducir es una anecdota.
//!
//! Como funciona la vigilancia: este test NO exige que la sonda pase entera —
//! si lo exigiera, fallaria hoy y seguiria fallando durante meses, y un test
//! rojo permanente deja de leerse. Lo que exige es que **el numero no baje**
//! (`MINIMO`). Añadir APIs lo sube y entonces hay que subir la constante, que
//! es justo el momento de dejar constancia del avance en `ARCHITECTURE.md`.
//!
//! La sonda corre por el camino REAL (proceso `engine_server` + HTTP), no por
//! `pipeline::build_page`, porque `build_page` no registra red ni
//! almacenamiento: mediria menos superficie de la que una pagina de verdad
//! recibe, y el numero saldria pesimista sin motivo.

mod common;

use common::{Motor, TestServer};
use serde_json::json;

/// Minimo que la sonda debe alcanzar. Se sube cada vez que se cierra una tarea
/// del bloque C del plan; nunca se baja para hacer pasar un cambio.
///
/// Historia del numero, para que se vea el avance y no solo el ultimo valor:
///
/// | Fecha      | Resultado | Que cambio                                  |
/// |------------|-----------|---------------------------------------------|
/// | 2026-09-09 | 48/114    | Primera medicion con esta sonda (tarea C1)  |
/// | 2026-09-09 | 56/114    | `console`, `URL`, `URLSearchParams`,        |
/// |            |           | `performance`, `atob`/`btoa`,               |
/// |            |           | `TextEncoder`/`TextDecoder` (tarea C5)      |
/// | 2026-09-09 | 65/114    | Jerarquia de clases del DOM: `instanceof` y |
/// |            |           | polyfills sobre prototipos (tarea C3)       |
/// | 2026-09-09 | 73/114    | `matches`, `closest`, `contains`, `remove`, |
/// |            |           | `append`/`prepend`, `cloneNode`, `dataset`, |
/// |            |           | `innerHTML` real, `outerHTML`, `id`,        |
/// |            |           | `className`, `isConnected` (tarea C6).      |
/// |            |           | Incluye corregir un FALSO POSITIVO: la      |
/// |            |           | sonda daba `innerHTML` por presente sin     |
/// |            |           | estarlo, porque asignar una propiedad       |
/// |            |           | cualquiera a un objeto JS siempre funciona  |
/// | 2026-09-09 | 85/114    | `CustomEvent`, `KeyboardEvent`,             |
/// |            |           | `MouseEvent`, `InputEvent`, entorno de      |
/// |            |           | `window` (`innerWidth`, `matchMedia`...) y  |
/// |            |           | `document.readyState`, `activeElement`,     |
/// |            |           | `getElementsByClassName` (tareas C4, C6, C7)|
const MINIMO: usize = 85;

/// El total de comprobaciones de la sonda. Se vigila aparte del minimo para
/// cazar un caso concreto: que alguien "suba" el porcentaje borrando
/// comprobaciones incomodas en vez de implementando lo que falta.
const TOTAL_ESPERADO: usize = 114;

const SONDA: &str = include_str!("../../../tests/probes/api-probe.html");

struct Resultado {
    pasan: usize,
    total: usize,
    faltan: Vec<String>,
}

fn ejecutar_sonda() -> Resultado {
    let servidor = TestServer::nuevo(SONDA);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({
        "type": "navigate", "id": "sonda", "url": servidor.url("/sonda")
    }));

    let titulo = estado["title"].as_str().unwrap_or("");

    // Si la sonda hubiera muerto a mitad, el `<title>` seguiria siendo el que
    // trae el HTML. Distinguirlo importa: un titulo sin cambiar significa
    // "la sonda no llego al final", no "cero APIs".
    assert!(
        titulo.starts_with("SONDA "),
        "la sonda no llego a publicar su resultado; el titulo sigue siendo {titulo:?}.\n\
         Causa tipica: una de las APIs que mide se uso tambien en la propia \
         infraestructura de la sonda, o `eval` no esta disponible. Mira stderr."
    );

    // Formato: `SONDA 62/96 | faltan: a, b, c`
    let (cuenta, faltan) = titulo
        .trim_start_matches("SONDA ")
        .split_once(" | faltan: ")
        .expect("formato de titulo inesperado");

    let (pasan, total) = cuenta.split_once('/').expect("se esperaba pasan/total");

    Resultado {
        pasan: pasan.parse().expect("`pasan` no era un numero"),
        total: total.parse().expect("`total` no era un numero"),
        faltan: if faltan.trim() == "(ninguna)" {
            Vec::new()
        } else {
            faltan.split(", ").map(|s| s.trim().to_string()).collect()
        },
    }
}

#[test]
fn la_superficie_de_plataforma_no_retrocede() {
    let r = ejecutar_sonda();

    // Se imprime siempre (visible con `--nocapture`, y en el log de CI cuando
    // falla): el valor de este test es el informe, no solo el veredicto.
    println!("\n=== Sonda de APIs: {}/{} ===", r.pasan, r.total);
    if r.faltan.is_empty() {
        println!("No falta ninguna.");
    } else {
        println!("Faltan {}:", r.faltan.len());
        for nombre in &r.faltan {
            println!("  - {nombre}");
        }
    }
    println!();

    assert_eq!(
        r.total, TOTAL_ESPERADO,
        "el numero de comprobaciones de la sonda cambio ({} -> {}). Si se añadieron \
         comprobaciones, sube `TOTAL_ESPERADO`. Si se QUITARON, para y piensa: borrar \
         una comprobacion sube el porcentaje sin implementar nada.",
        TOTAL_ESPERADO, r.total
    );

    assert!(
        r.pasan >= MINIMO,
        "la superficie de plataforma RETROCEDIO: {} de {} (el minimo registrado es {}).\n\
         Algo que antes existia dejo de existir. Faltan ahora: {:?}",
        r.pasan,
        r.total,
        MINIMO,
        r.faltan
    );
}

/// Las APIs que el bloque C del plan da por presentes ya hoy. Si una de estas
/// cae, no es "la sonda baja un punto": es que se rompio algo que webs reales
/// usan y que este motor tenia ganado.
#[test]
fn las_apis_ya_conseguidas_siguen_ahi() {
    let r = ejecutar_sonda();

    let imprescindibles = [
        "sintaxis: async/await",
        "sintaxis: clases",
        "document.body",
        "document.head",
        "document.querySelector",
        "document.createElement",
        "el.classList",
        "el.innerHTML",
        "el.getBoundingClientRect",
        "window.location.href",
        "window.localStorage",
        "window.getComputedStyle",
        "window.requestAnimationFrame",
        "MutationObserver",
        "new Event",
        "addEventListener + dispatchEvent",
        "fetch",
        "XMLHttpRequest",
    ];

    let perdidas: Vec<_> = imprescindibles
        .iter()
        .filter(|n| r.faltan.iter().any(|f| f == *n))
        .collect();

    assert!(
        perdidas.is_empty(),
        "estas APIs estaban conseguidas y han desaparecido: {perdidas:?}"
    );
}
