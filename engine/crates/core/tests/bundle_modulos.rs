//! Un bundle moderno, servido como lo sirve un bundler de verdad, se ejecuta y
//! pinta su contenido (tarea C9 del `plan.md`).
//!
//! **Qué prueba esto que no probaba nada más.** Todos los tests anteriores del
//! motor cargan HTML que ya trae su contenido escrito. Este carga lo contrario:
//! un `index.html` con el `<body>` prácticamente vacío, cuyo contenido lo
//! construye JavaScript. Es la forma de la inmensa mayoría de la web actual
//! (React, Vue, Svelte, Next) y es exactamente lo que el README lleva desde el
//! principio diciendo que **no se ve**.
//!
//! La estructura no es inventada, es la que emite `vite build`:
//!
//! ```html
//! <script type="module" crossorigin src="/assets/index-abc.js"></script>
//! <link rel="modulepreload" href="/assets/vendor-def.js">
//! ```
//!
//! El módulo raíz importa sus fragmentos, y el `<link rel="modulepreload">`
//! existe justo para que el navegador los tenga descargados antes de que haga
//! falta. Este motor no va a la red durante la evaluación (ver
//! `engine_js::modules`), así que ese `<link>` no es un detalle de rendimiento
//! aquí: es de dónde salen los módulos.
//!
//! **Por qué el test comprueba el TEXTO del layout y no una variable global.**
//! Que el script corra sin lanzar no significa que la página se vea. Lo que
//! importa es que el contenido que produjo llegue hasta el árbol de layout, que
//! es lo que se pinta. Comprobar una global pasaría aunque el DOM se hubiera
//! quedado sin tocar.

mod common;

use common::{Motor, TestServer};
use serde_json::json;

/// `index.html` como lo emite un bundler: cáscara vacía más el módulo.
const INDICE: &str = r#"<!doctype html>
<html>
<head>
  <title>App</title>
  <script type="module" crossorigin src="/assets/index.js"></script>
  <link rel="modulepreload" href="/assets/vendor.js">
</head>
<body>
  <div id="root"></div>
</body>
</html>"#;

/// El fragmento compartido, importado por el raíz. Exporta como haría una
/// librería empaquetada.
const VENDOR: &str = r#"
export function crearTitulo(texto) {
  const h = document.createElement('h1');
  h.textContent = texto;
  return h;
}
export const VERSION = '1.0';
"#;

/// El módulo raíz: importa y monta, que es lo que hace el punto de entrada de
/// cualquier aplicación con framework.
/// Produce holgadamente mas de 40 caracteres de texto visible a proposito: ese
/// es el umbral por debajo del cual `requires_javascript` da una pagina por
/// vacia (ver `MIN_VISIBLE_TEXT_CHARS` en `core::server`). Con menos, el test
/// de la bandera fallaria por lo escueto del contenido y no por lo que quiere
/// medir. Una aplicacion real siempre pinta bastante mas que esto.
const INDEX: &str = r#"
import { crearTitulo, VERSION } from '/assets/vendor.js';

const raiz = document.getElementById('root');
raiz.appendChild(crearTitulo('Hola desde el bundle'));

const pie = document.createElement('p');
pie.textContent = 'Contenido generado por el modulo, version ' + VERSION +
  ', suficiente para que la pagina cuente como no vacia.';
raiz.appendChild(pie);
"#;

fn servidor_bundle() -> TestServer {
    TestServer::con_rutas(vec![
        ("/", INDICE),
        ("/assets/index.js", INDEX),
        ("/assets/vendor.js", VENDOR),
    ])
}

#[test]
fn un_bundle_con_modulos_se_ejecuta_y_pinta_su_contenido() {
    let servidor = servidor_bundle();
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({
        "type": "navigate", "id": "b1", "url": servidor.url("/")
    }));

    assert_eq!(estado["type"], "state", "la navegacion fallo: {estado}");

    // El texto solo puede estar ahi si: el `<script type="module">` se parseo
    // como modulo (antes era un error de sintaxis en su primer `import`), el
    // `<link rel="modulepreload">` se descargo, el `import` lo encontro, y las
    // mutaciones del DOM llegaron al arbol de layout.
    let texto = serde_json::to_string(&estado).unwrap();
    assert!(
        texto.contains("Hola desde el bundle"),
        "el modulo no llego a pintar su contenido. Estado: {}",
        &texto[..texto.len().min(1200)]
    );
    assert!(
        texto.contains("version 1.0"),
        "el valor importado del fragmento no llego al DOM"
    );
}

#[test]
fn un_bundle_que_funciona_no_se_marca_como_dependiente_de_javascript() {
    // `requires_javascript` (Fase 39) existe para avisar de que la pagina se
    // quedo vacia porque el motor no ejecuto su bundle. Que siga marcandose
    // cuando el bundle SI se ejecuto seria un falso positivo: el usuario veria
    // un aviso de "esta pagina necesita JavaScript" sobre una pagina que se ve
    // perfectamente.
    let servidor = servidor_bundle();
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({
        "type": "navigate", "id": "b2", "url": servidor.url("/")
    }));

    assert_eq!(
        estado["requires_javascript"], false,
        "el bundle se ejecuto y pinto contenido: marcarlo como dependiente de JS \
         seria un falso positivo"
    );
}

#[test]
fn el_modulo_raiz_puede_manipular_el_dom_como_un_script_clasico() {
    // Un modulo corre en su propio ambito (sus `const` no son globales), pero
    // el `document` que ve es EL MISMO. Si no lo fuera, el modulo se ejecutaria
    // sin errores y no se veria nada, que es el fallo mas dificil de
    // diagnosticar de todos.
    let servidor = TestServer::con_rutas(vec![
        (
            "/",
            r#"<!doctype html><html><head><title>t</title>
               <script type="module" src="/m.js"></script></head>
               <body><div id="destino"></div></body></html>"#,
        ),
        (
            "/m.js",
            "document.getElementById('destino').textContent = 'escrito por el modulo';",
        ),
    ]);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "m", "url": servidor.url("/")}));
    let texto = serde_json::to_string(&estado).unwrap();
    assert!(
        texto.contains("escrito por el modulo"),
        "el modulo no vio el mismo document que el resto de la pagina"
    );
}

#[test]
fn un_modulo_en_linea_tambien_se_ejecuta_como_modulo() {
    // `<script type="module">` sin `src`. Su marca distintiva es que la
    // sintaxis de modulo (aqui `import.meta`) parsea, cosa que en un script
    // clasico es un error.
    let servidor = TestServer::con_rutas(vec![(
        "/",
        r#"<!doctype html><html><head><title>t</title></head><body>
           <div id="d"></div>
           <script type="module">
             const marca = typeof import.meta === 'object' ? 'es modulo' : 'no';
             document.getElementById('d').textContent = marca;
           </script>
           </body></html>"#,
    )]);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "i", "url": servidor.url("/")}));
    let texto = serde_json::to_string(&estado).unwrap();
    assert!(
        texto.contains("es modulo"),
        "un <script type=\"module\"> en linea no se evaluo como modulo"
    );
}

#[test]
fn nomodule_se_omite_cuando_el_motor_ejecuta_modulos() {
    // `nomodule` marca el respaldo para navegadores viejos. Ejecutarlo ADEMAS
    // del modulo montaria la aplicacion dos veces; es un fallo que en una web
    // real se manifiesta como contenido duplicado.
    let servidor = TestServer::con_rutas(vec![(
        "/",
        r#"<!doctype html><html><head><title>t</title></head><body>
           <div id="d"></div>
           <script type="module">
             document.getElementById('d').textContent = 'moderno';
           </script>
           <script nomodule>
             document.getElementById('d').textContent = 'antiguo';
           </script>
           </body></html>"#,
    )]);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/")}));
    let texto = serde_json::to_string(&estado).unwrap();
    assert!(
        texto.contains("moderno"),
        "deberia haber ganado el modulo: {}",
        &texto[..texto.len().min(600)]
    );
    assert!(
        !texto.contains("antiguo"),
        "el <script nomodule> se ejecuto pese a que este motor soporta modulos"
    );
}

#[test]
fn un_script_con_type_de_datos_no_se_ejecuta() {
    // `<script type="application/json">` es un contenedor de datos, comunisimo
    // para datos estructurados y estado inicial de una aplicacion. Antes de
    // esta fase se intentaba ejecutar y producia un error de sintaxis que
    // ensuciaba el diagnostico de la pagina.
    let servidor = TestServer::con_rutas(vec![(
        "/",
        r#"<!doctype html><html><head><title>t</title></head><body>
           <div id="d">intacto</div>
           <script type="application/json">{"esto": "no es codigo"}</script>
           </body></html>"#,
    )]);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "j", "url": servidor.url("/")}));
    assert_eq!(
        estado["type"], "state",
        "un <script type=\"application/json\"> no debe romper la carga: {estado}"
    );
}

#[test]
fn defer_corre_despues_que_un_script_clasico_aunque_venga_antes_en_el_documento() {
    // El orden del spec, que antes de esta fase se ignoraba: `defer` espera a
    // que el documento este parseado, asi que corre DESPUES de los clasicos
    // aunque aparezca primero. Codigo real depende de esto para asumir que el
    // DOM entero existe.
    let servidor = TestServer::con_rutas(vec![
        (
            "/",
            r#"<!doctype html><html><head><title>t</title>
               <script defer src="/tarde.js"></script>
               <script src="/pronto.js"></script>
               </head><body><div id="d"></div></body></html>"#,
        ),
        ("/pronto.js", "globalThis.orden = 'pronto';"),
        (
            "/tarde.js",
            "document.getElementById('d').textContent = 'primero fue ' + globalThis.orden;",
        ),
    ]);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "d", "url": servidor.url("/")}));
    let texto = serde_json::to_string(&estado).unwrap();
    assert!(
        texto.contains("primero fue pronto"),
        "el `defer` no espero al script clasico: {}",
        &texto[..texto.len().min(800)]
    );
}
