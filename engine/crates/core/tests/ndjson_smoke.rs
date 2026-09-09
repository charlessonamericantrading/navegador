//! Test de humo del protocolo NDJSON contra el binario `engine_server` REAL.
//!
//! Por que existe, y por que no basta con los tests de `server.rs`: esos
//! construyen un `EngineServer` en memoria y llaman a `handle()` directamente.
//! Eso prueba la logica, pero se salta justo la capa que el producto usa de
//! verdad — el proceso aparte, su stdin/stdout, el serde de ida y vuelta, y la
//! regla de que NADA que no sea JSON puede salir por stdout. Un `println!` de
//! depuracion mal puesto, un campo renombrado en `EngineResponse`, o un panico
//! al arrancar no los ve ninguno de aquellos tests y rompen la aplicacion
//! entera; este si los ve.
//!
//! Cubre las 17 variantes de `EngineRequest`. Cuando se añada la 18 y no se
//! añada aqui, `todas_las_variantes_del_protocolo_estan_cubiertas` falla a
//! proposito, para que la cobertura no se degrade en silencio.
//!
//! Sin red externa: el HTML de prueba lo sirve un `TcpListener` local que vive
//! y muere dentro del propio test.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Margen para que el motor termine una navegacion completa (descarga, parseo,
/// cascada, layout y una captura PNG). Generoso a proposito: un CI compartido
/// va mucho mas lento que una maquina de desarrollo, y un test que parpadea es
/// peor que no tenerlo.
const TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Servidor HTTP minimo
// ---------------------------------------------------------------------------

/// Sirve el mismo cuerpo a cada peticion que reciba, hasta que se le suelta.
///
/// No usa `hyper` a proposito aunque ya sea dependencia: aqui hace falta lo
/// contrario de un cliente HTTP correcto — un servidor tonto y predecible, de
/// treinta lineas, que no comparta ni una linea de codigo con el que se esta
/// probando. Si el test usara la misma pila que el motor, un fallo en esa pila
/// podria cancelarse consigo mismo y pasar desapercibido.
struct TestServer {
    puerto: u16,
    /// Se conserva para que el hilo siga vivo mientras el test lo use; al
    /// soltar la struct, el listener se cierra y el hilo termina solo.
    _hilo: std::thread::JoinHandle<()>,
}

impl TestServer {
    fn nuevo(cuerpo: &'static str) -> Self {
        // Puerto 0 = que el sistema operativo elija uno libre. Fijar un puerto
        // haria que dos tests en paralelo (que es como corre `cargo test`) se
        // pelearan por el.
        let listener = TcpListener::bind("127.0.0.1:0").expect("no se pudo abrir el puerto");
        let puerto = listener.local_addr().unwrap().port();

        let hilo = std::thread::spawn(move || {
            for flujo in listener.incoming() {
                let Ok(mut flujo) = flujo else { break };
                // Leer la peticion entera no hace falta: basta con vaciar la
                // primera linea para que el cliente no vea un reset.
                let mut buffer = [0u8; 2048];
                let _ = flujo.read(&mut buffer);

                let respuesta = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n\
                     {}",
                    cuerpo.len(),
                    cuerpo
                );
                let _ = flujo.write_all(respuesta.as_bytes());
                let _ = flujo.flush();
            }
        });

        Self {
            puerto,
            _hilo: hilo,
        }
    }

    fn url(&self, ruta: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.puerto, ruta)
    }
}

/// HTML con lo justo para que TODOS los comandos del protocolo tengan algo real
/// sobre lo que actuar: un titulo (`get_state`), un enlace y un boton
/// (`click`), un `<input>` (`type_text`/`press_key`), texto suficiente para
/// que haya scroll vertical (`scroll`), y roles que el AOM pueda nombrar
/// (`get_accessibility_tree`).
const PAGINA: &str = r#"<!doctype html>
<html><head><title>Pagina de humo</title></head>
<body>
  <h1>Titular</h1>
  <a id="enlace" href="/otra">Ir a otra</a>
  <button id="boton">Pulsar</button>
  <input id="campo" type="text" value="">
  <p style="height: 3000px">Texto largo para que haya scroll de verdad.</p>
</body></html>"#;

// ---------------------------------------------------------------------------
// El proceso del motor
// ---------------------------------------------------------------------------

struct Motor {
    hijo: Child,
    entrada: ChildStdin,
    salida: BufReader<ChildStdout>,
}

impl Motor {
    fn arrancar() -> Self {
        // Cargo exporta esta variable para los tests de integracion del mismo
        // paquete: apunta al binario recien compilado, no a uno del PATH que
        // podria ser de otra rama.
        let mut hijo = Command::new(env!("CARGO_BIN_EXE_engine_server"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr se hereda: los `tracing::warn!` y el resumen de
            // mitigaciones van ahi y son utiles cuando un test falla. Lo que
            // NO puede aparecer ahi es JSON, y lo que no puede aparecer en
            // stdout es cualquier otra cosa - eso es justo lo que se prueba.
            .stderr(Stdio::inherit())
            .spawn()
            .expect("no se pudo arrancar engine_server");

        let entrada = hijo.stdin.take().expect("sin stdin");
        let salida = BufReader::new(hijo.stdout.take().expect("sin stdout"));

        let mut motor = Self {
            hijo,
            entrada,
            salida,
        };

        // El servidor saluda solo, antes de que nadie le pida nada.
        let saludo = motor.leer();
        assert_eq!(
            saludo["type"], "ready",
            "el servidor debe saludar con `ready` nada mas arrancar, no con {saludo}"
        );
        assert_eq!(
            saludo["protocol_version"], 1,
            "version de protocolo inesperada en el saludo"
        );

        motor
    }

    /// Manda una peticion y devuelve la respuesta ya parseada.
    fn pedir(&mut self, peticion: Value) -> Value {
        let linea = serde_json::to_string(&peticion).unwrap();
        writeln!(self.entrada, "{linea}").expect("no se pudo escribir la peticion");
        self.entrada.flush().expect("no se pudo vaciar stdin");
        self.leer()
    }

    /// Lee UNA linea de stdout y exige que sea JSON.
    ///
    /// Esta funcion es la mitad del valor de este fichero: si alguien mete un
    /// `println!` en el pipeline, la linea que llega aqui no parsea y el test
    /// dice exactamente cual era.
    fn leer(&mut self) -> Value {
        let inicio = Instant::now();
        let mut linea = String::new();

        // `read_line` bloquea; el reloj sirve para que el mensaje de error
        // distinga "tardo demasiado" de "murio el proceso".
        let leidos = self
            .salida
            .read_line(&mut linea)
            .expect("error leyendo stdout del motor");

        assert!(
            leidos > 0,
            "el motor cerro stdout sin responder (¿panico al arrancar? mira stderr). \
             Tiempo esperando: {:?}",
            inicio.elapsed()
        );
        assert!(
            inicio.elapsed() < TIMEOUT,
            "el motor tardo mas de {TIMEOUT:?} en responder"
        );

        serde_json::from_str(&linea).unwrap_or_else(|e| {
            panic!(
                "stdout del motor NO era JSON, que rompe el protocolo NDJSON.\n\
                 Error: {e}\n\
                 Linea recibida: {linea:?}\n\
                 Causa tipica: un `println!`/`dbg!` de depuracion. Todo diagnostico \
                 va a stderr (ver la cabecera de `bin/engine_server.rs`)."
            )
        })
    }
}

impl Drop for Motor {
    fn drop(&mut self) {
        // Que un test falle no debe dejar un proceso huerfano ocupando el
        // puerto ni memoria en la maquina de CI.
        let _ = self.hijo.kill();
        let _ = self.hijo.wait();
    }
}

/// Comprueba lo que toda respuesta debe cumplir, sea del tipo que sea.
fn assert_correlacionada(respuesta: &Value, id_esperado: &str, tipo_esperado: &str) {
    assert_eq!(
        respuesta["type"], tipo_esperado,
        "tipo de respuesta inesperado: {respuesta}"
    );
    // La correlacion es lo que permite al cliente casar respuesta con peticion
    // cuando hay varias en vuelo. Perderla no rompe un test ingenuo (la
    // respuesta llega igual) pero si rompe al cliente real.
    assert_eq!(
        respuesta["id"], id_esperado,
        "la respuesta no devolvio el `id` de correlacion de su peticion: {respuesta}"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn ping_responde_pong_con_la_version_del_protocolo() {
    let mut motor = Motor::arrancar();
    let r = motor.pedir(json!({"type": "ping", "id": "p1"}));

    assert_correlacionada(&r, "p1", "pong");
    assert_eq!(r["protocol_version"], 1);
    assert_eq!(
        r["renderer_status"], "ready",
        "con el proceso vivo el renderer se declara listo"
    );
}

#[test]
fn una_linea_que_no_es_json_devuelve_error_y_no_mata_el_proceso() {
    let mut motor = Motor::arrancar();

    writeln!(motor.entrada, "esto no es json").unwrap();
    motor.entrada.flush().unwrap();
    let r = motor.leer();

    assert_eq!(r["type"], "error", "una peticion invalida debe dar error");
    assert!(
        r["message"].as_str().unwrap().contains("invalid_request"),
        "el error debe decir que la peticion era invalida: {r}"
    );

    // Lo que de verdad importa: el servidor sigue atendiendo. Un parser que se
    // lleva por delante el proceso ante una linea basura convertiria cualquier
    // fallo del cliente en una caida del navegador.
    let r = motor.pedir(json!({"type": "ping", "id": "sigue-vivo"}));
    assert_correlacionada(&r, "sigue-vivo", "pong");
}

#[test]
fn una_linea_vacia_se_ignora_sin_responder() {
    let mut motor = Motor::arrancar();

    writeln!(motor.entrada).unwrap();
    motor.entrada.flush().unwrap();

    // Si la linea en blanco generase respuesta, la siguiente lectura devolveria
    // esa y no el `pong`, y esta comprobacion lo cazaria.
    let r = motor.pedir(json!({"type": "ping", "id": "tras-vacia"}));
    assert_correlacionada(&r, "tras-vacia", "pong");
}

#[test]
fn navigate_y_get_state_devuelven_la_pagina_real() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    let r = motor.pedir(json!({
        "type": "navigate", "id": "n1", "url": servidor.url("/")
    }));
    assert_correlacionada(&r, "n1", "state");
    assert_eq!(
        r["title"], "Pagina de humo",
        "el titulo debe salir del <title> real de la pagina descargada"
    );
    assert!(
        r["url"].as_str().unwrap().contains("127.0.0.1"),
        "la URL devuelta debe ser la que realmente respondio: {r}"
    );

    // La captura viaja en Base64 y debe ser PNG. Se comprueba la firma real
    // del formato porque la Fase 39 encontro justo lo contrario: se declaraba
    // JPEG siendo PNG, y nadie se dio cuenta porque Chromium lo adivinaba.
    let captura = r["screenshot"].as_str().unwrap();
    assert!(!captura.is_empty(), "la captura no puede venir vacia");
    let bytes = decodificar_base64(captura);
    assert_eq!(
        &bytes[..4],
        &[0x89, b'P', b'N', b'G'],
        "la captura debe ser un PNG de verdad, no otro formato declarado como tal"
    );

    // Una pagina con texto visible NO debe marcarse como dependiente de JS,
    // aunque el aviso exista (Fase 39): marcarla seria un falso positivo que
    // ensuciaria la interfaz en paginas que se ven perfectamente.
    assert_eq!(
        r["requires_javascript"], false,
        "esta pagina tiene texto visible: no depende de JavaScript"
    );

    assert!(
        r["elements"].as_array().unwrap().len() >= 3,
        "deben salir al menos el enlace, el boton y el input: {}",
        r["elements"]
    );

    // `get_state` por separado debe devolver lo mismo sin volver a navegar.
    let s = motor.pedir(json!({"type": "get_state", "id": "s1"}));
    assert_correlacionada(&s, "s1", "state");
    assert_eq!(s["title"], "Pagina de humo");
}

#[test]
fn navigate_a_un_esquema_no_permitido_da_error_y_no_navega() {
    let mut motor = Motor::arrancar();

    let r = motor.pedir(json!({
        "type": "navigate", "id": "malo", "url": "file:///C:/Windows/win.ini"
    }));

    assert_eq!(
        r["type"], "error",
        "solo http/https estan permitidos (Fase 37); `file:` debe rechazarse: {r}"
    );
}

#[test]
fn resize_cambia_el_viewport_y_get_state_lo_refleja() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/")}));
    let r = motor.pedir(json!({
        "type": "resize", "id": "r1", "width": 800, "height": 600
    }));

    assert_correlacionada(&r, "r1", "state");
    // El efecto observable de un resize es que la pagina se re-maqueta; se
    // comprueba que sigue respondiendo con la pagina correcta y no un estado
    // vacio, que es como fallaba antes de la Fase 4.4.
    assert_eq!(r["title"], "Pagina de humo");
}

#[test]
fn scroll_mueve_el_desplazamiento_vertical_y_lo_reporta() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/")}));
    let inicial = motor.pedir(json!({"type": "get_state", "id": "s0"}));
    assert_eq!(
        inicial["scroll_offset_y"], 0.0,
        "una pagina recien cargada empieza arriba del todo"
    );

    let r = motor.pedir(json!({"type": "scroll", "id": "sc", "dx": 0, "dy": 200}));
    assert_correlacionada(&r, "sc", "state");
    assert!(
        r["scroll_offset_y"].as_f64().unwrap() > 0.0,
        "la pagina mide 3000px: un scroll de 200 tiene sitio de sobra para moverse, \
         pero el desplazamiento sigue en {}",
        r["scroll_offset_y"]
    );
}

#[test]
fn click_type_text_y_press_key_son_aceptados_sobre_la_pagina() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/")}));

    // Se apunta al centro del elemento que reporta el PROPIO motor, buscado por
    // su etiqueta y no por su posicion en la lista: asi el test no se rompe
    // cuando cambie una fuente, un margen o el orden del recorrido del DOM.
    let (bx, by) = centro_de(&estado, "button");
    let (ix, iy) = centro_de(&estado, "input");

    let r = motor.pedir(json!({"type": "click", "id": "c1", "x": bx, "y": by}));
    assert_correlacionada(&r, "c1", "state");

    // `type_text` exige un control de texto bajo las coordenadas y responde
    // `error` si no lo hay - por eso apunta al <input> y no al boton.
    let r = motor.pedir(json!({
        "type": "type_text", "id": "t1",
        "x": ix, "y": iy, "text": "hola", "press_enter": false
    }));
    assert_correlacionada(&r, "t1", "state");

    let escrito = motor.pedir(json!({"type": "get_state", "id": "s1"}));
    let campo = elemento_por_etiqueta(&escrito, "input");
    assert_eq!(
        campo["attributes"]["value"], "hola",
        "escribir debe dejar el texto DENTRO del input, no solo aceptar el comando: {campo}"
    );

    let r = motor.pedir(json!({"type": "press_key", "id": "k1", "key": "Tab"}));
    assert_correlacionada(&r, "k1", "state");
}

#[test]
fn type_text_sobre_algo_que_no_es_un_campo_da_error_explicito() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    let estado = motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/")}));
    let (x, y) = centro_de(&estado, "button");

    let r = motor.pedir(json!({
        "type": "type_text", "id": "t2",
        "x": x, "y": y, "text": "hola", "press_enter": false
    }));

    // Fallar con un motivo legible es parte del contrato: el cliente necesita
    // distinguir "no se pudo" de "se hizo y no paso nada".
    assert_eq!(
        r["type"], "error",
        "escribir sobre un <button> no es posible y debe decirlo: {r}"
    );
    assert!(
        !r["message"].as_str().unwrap().is_empty(),
        "el error debe traer un motivo, no venir vacio: {r}"
    );
}

#[test]
fn back_y_forward_recorren_el_historial_de_verdad() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    motor.pedir(json!({"type": "navigate", "id": "n1", "url": servidor.url("/uno")}));
    let segunda = motor.pedir(json!({"type": "navigate", "id": "n2", "url": servidor.url("/dos")}));

    assert_eq!(
        segunda["can_go_back"], true,
        "tras dos navegaciones debe poder ir atras"
    );
    assert_eq!(
        segunda["can_go_forward"], false,
        "estando en la ultima entrada no hay nada delante"
    );

    let atras = motor.pedir(json!({"type": "back", "id": "b1"}));
    assert_correlacionada(&atras, "b1", "state");
    assert!(
        atras["url"].as_str().unwrap().ends_with("/uno"),
        "`back` debe volver a la primera URL, no a {}",
        atras["url"]
    );
    assert_eq!(
        atras["can_go_forward"], true,
        "despues de ir atras si hay algo delante"
    );

    let alante = motor.pedir(json!({"type": "forward", "id": "f1"}));
    assert_correlacionada(&alante, "f1", "state");
    assert!(
        alante["url"].as_str().unwrap().ends_with("/dos"),
        "`forward` debe volver a la segunda URL, no a {}",
        alante["url"]
    );
}

#[test]
fn el_ciclo_completo_de_pestanas_funciona() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/primera")}));

    let lista = motor.pedir(json!({"type": "list_tabs", "id": "l0"}));
    assert_correlacionada(&lista, "l0", "tabs");
    let iniciales = lista["tabs"].as_array().unwrap().len();
    assert_eq!(iniciales, 1, "al arrancar hay exactamente una pestaña");
    let primera_id = lista["tabs"][0]["id"].as_u64().unwrap();

    // Abrir una pestaña nueva la hace activa de inmediato, igual que un
    // target="_blank" real.
    let nueva = motor.pedir(json!({
        "type": "new_tab", "id": "nt", "url": servidor.url("/segunda")
    }));
    assert_correlacionada(&nueva, "nt", "state");
    let segunda_id = nueva["tab_id"].as_u64().unwrap();
    assert_ne!(
        segunda_id, primera_id,
        "la pestaña nueva no puede reusar el id de la anterior"
    );

    let lista = motor.pedir(json!({"type": "list_tabs", "id": "l1"}));
    assert_eq!(
        lista["tabs"].as_array().unwrap().len(),
        2,
        "ahora deben verse las dos pestañas"
    );

    // Volver a la primera debe recuperar SU estado, no re-navegar.
    let vuelta = motor.pedir(json!({
        "type": "switch_tab", "id": "sw", "tab_id": primera_id
    }));
    assert_correlacionada(&vuelta, "sw", "state");
    assert_eq!(
        vuelta["tab_id"].as_u64().unwrap(),
        primera_id,
        "el estado devuelto debe ser el de la pestaña a la que se cambio"
    );
    assert!(
        vuelta["url"].as_str().unwrap().ends_with("/primera"),
        "cada pestaña conserva su propia URL: {}",
        vuelta["url"]
    );

    let cerrada = motor.pedir(json!({
        "type": "close_tab", "id": "ct", "tab_id": segunda_id
    }));
    assert_correlacionada(&cerrada, "ct", "state");

    let lista = motor.pedir(json!({"type": "list_tabs", "id": "l2"}));
    assert_eq!(
        lista["tabs"].as_array().unwrap().len(),
        1,
        "tras cerrar una deben quedar las de antes menos esa"
    );
}

#[test]
fn get_accessibility_tree_devuelve_el_aom_de_la_pagina() {
    let servidor = TestServer::nuevo(PAGINA);
    let mut motor = Motor::arrancar();

    motor.pedir(json!({"type": "navigate", "id": "n", "url": servidor.url("/")}));
    let r = motor.pedir(json!({"type": "get_accessibility_tree", "id": "a1"}));

    assert_eq!(
        r["id"], "a1",
        "la respuesta del AOM debe correlacionarse con su peticion: {r}"
    );
    // El AOM es la diferenciacion del producto (es lo que ve el agente de IA):
    // que responda vacio seria tan malo como no responder.
    let texto = serde_json::to_string(&r).unwrap();
    assert!(
        texto.contains("Titular") || texto.contains("Pulsar") || texto.contains("Ir a otra"),
        "el arbol debe nombrar el contenido real de la pagina: {texto}"
    );
}

#[test]
fn shutdown_responde_y_despues_cierra_el_proceso() {
    let mut motor = Motor::arrancar();

    let r = motor.pedir(json!({"type": "shutdown", "id": "fin"}));
    assert_eq!(
        r["id"], "fin",
        "shutdown debe responder ANTES de cerrar, no cortar en seco: {r}"
    );

    // Y despues de responder, stdout debe cerrarse: es la senal por la que el
    // proceso padre (Electron) sabe que puede dejar de esperar.
    let mut resto = String::new();
    motor
        .salida
        .read_to_string(&mut resto)
        .expect("stdout deberia poder leerse hasta el final");
    assert!(
        resto.trim().is_empty(),
        "tras `shutdown` no debe salir nada mas por stdout, pero salio: {resto:?}"
    );

    let estado = motor.hijo.wait().expect("el proceso deberia terminar solo");
    assert!(
        estado.success(),
        "`shutdown` es una salida ordenada: el codigo debe ser 0, fue {estado:?}"
    );
}

/// Guardia de cobertura: si mañana se añade una variante a `EngineRequest` y no
/// se prueba aqui, este test falla y dice cual falta.
///
/// Lee el enum del fuente en vez de depender de un `match` exhaustivo porque
/// `EngineRequest` solo implementa `Deserialize`: no hay forma de enumerar sus
/// variantes en tiempo de ejecucion sin construirlas una a una, que es
/// exactamente el trabajo manual que este test quiere evitar que se olvide.
#[test]
fn todas_las_variantes_del_protocolo_estan_cubiertas() {
    let fuente = include_str!("../src/protocol.rs");

    let cuerpo = fuente
        .split_once("pub enum EngineRequest {")
        .expect("no se encontro `EngineRequest` en protocol.rs")
        .1
        .split_once("\n}")
        .expect("no se encontro el final del enum")
        .0;

    // Las variantes son las lineas con 4 espacios de sangria que terminan en
    // `{` — el resto son campos (8 espacios) o comentarios.
    let declaradas: HashSet<String> = cuerpo
        .lines()
        .filter(|l| l.starts_with("    ") && !l.starts_with("     ") && l.trim_end().ends_with('{'))
        .map(|l| a_snake_case(l.trim().trim_end_matches(" {")))
        .collect();

    let cubiertas: HashSet<String> = [
        "navigate",
        "ping",
        "resize",
        "get_state",
        "click",
        "scroll",
        "type_text",
        "press_key",
        "back",
        "forward",
        "new_tab",
        "close_tab",
        "switch_tab",
        "list_tabs",
        "get_accessibility_tree",
        "shutdown",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let sin_cubrir: Vec<_> = declaradas.difference(&cubiertas).collect();
    assert!(
        sin_cubrir.is_empty(),
        "hay variantes de `EngineRequest` sin test de humo: {sin_cubrir:?}.\n\
         Añade un test en este fichero y su nombre a la lista `cubiertas`."
    );

    let inventadas: Vec<_> = cubiertas.difference(&declaradas).collect();
    assert!(
        inventadas.is_empty(),
        "la lista `cubiertas` nombra variantes que ya no existen: {inventadas:?}"
    );
}

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

/// El primer elemento interactivo del estado con esa etiqueta.
fn elemento_por_etiqueta<'a>(estado: &'a Value, etiqueta: &str) -> &'a Value {
    estado["elements"]
        .as_array()
        .expect("`elements` debe ser una lista")
        .iter()
        .find(|e| e["tag_name"] == etiqueta)
        .unwrap_or_else(|| {
            panic!(
                "no se reporto ningun <{etiqueta}> entre los elementos interactivos: {}",
                estado["elements"]
            )
        })
}

/// Centro del rectangulo de ese elemento, que es donde apuntan `click` y
/// `type_text`.
fn centro_de(estado: &Value, etiqueta: &str) -> (f64, f64) {
    let rect = &elemento_por_etiqueta(estado, etiqueta)["rect"];
    (
        rect["x"].as_f64().unwrap() + rect["width"].as_f64().unwrap() / 2.0,
        rect["y"].as_f64().unwrap() + rect["height"].as_f64().unwrap() / 2.0,
    )
}

fn a_snake_case(nombre: &str) -> String {
    let mut salida = String::new();
    for (i, c) in nombre.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                salida.push('_');
            }
            salida.extend(c.to_lowercase());
        } else {
            salida.push(c);
        }
    }
    salida
}

/// Solo lo necesario para leer la firma de la captura. No se usa el crate
/// `base64` (que si es dependencia) para que el test no dependa de la misma
/// version que usa el codigo probado.
fn decodificar_base64(entrada: &str) -> Vec<u8> {
    const ALFABETO: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut acumulador: u32 = 0;
    let mut bits = 0;
    let mut salida = Vec::new();

    for c in entrada.bytes() {
        if c == b'=' {
            break;
        }
        let Some(valor) = ALFABETO.iter().position(|&a| a == c) else {
            continue; // saltos de linea y demas relleno
        };
        acumulador = (acumulador << 6) | valor as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            salida.push((acumulador >> bits) as u8);
        }
        // Con la firma PNG basta; no hay que decodificar megabytes de imagen.
        if salida.len() >= 8 {
            break;
        }
    }
    salida
}
