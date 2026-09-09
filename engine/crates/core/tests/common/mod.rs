//! Ayudantes compartidos por los tests de integracion de `engine-core`.
//!
//! Vive en `tests/common/` (subdirectorio) y no en `tests/` a proposito: Cargo
//! trata cada `.rs` suelto de `tests/` como un binario de test independiente,
//! pero un subdirectorio es un modulo normal. Sin esto, este fichero se
//! compilaria como una suite vacia mas.
//!
//! Cada test que lo use declara `mod common;` y toma solo lo que necesita, de
//! ahi el `allow(dead_code)`: lo que un fichero no use es codigo muerto DESDE
//! SU punto de vista aunque otro lo use, porque son binarios distintos.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Margen para que el motor termine una navegacion completa (descarga, parseo,
/// cascada, layout y una captura PNG). Generoso a proposito: un CI compartido
/// va mucho mas lento que una maquina de desarrollo, y un test que parpadea es
/// peor que no tenerlo.
pub const TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Servidor HTTP minimo
// ---------------------------------------------------------------------------

pub struct TestServer {
    pub puerto: u16,
    /// Se conserva para que el hilo siga vivo mientras el test lo use; al
    /// soltar la struct, el listener se cierra y el hilo termina solo.
    _hilo: std::thread::JoinHandle<()>,
}

/// Construye una respuesta HTTP completa.
///
/// Los saltos de linea del protocolo se escriben como `\r\n` de verdad: un
/// servidor que mande solo `\n` funciona con clientes tolerantes y falla con
/// los estrictos, y aqui el cliente es justo el que se esta probando.
fn respuesta_http(estado: &str, tipo: Option<&str>, cuerpo: &str) -> String {
    let mut r = String::new();
    r.push_str("HTTP/1.1 ");
    r.push_str(estado);
    r.push_str("\r\n");
    if let Some(tipo) = tipo {
        r.push_str("Content-Type: ");
        r.push_str(tipo);
        r.push_str("\r\n");
    }
    r.push_str("Content-Length: ");
    r.push_str(&cuerpo.len().to_string());
    r.push_str("\r\nConnection: close\r\n\r\n");
    r.push_str(cuerpo);
    r
}

/// El tipo MIME que corresponde a una ruta. Importa de verdad: ningun
/// navegador ejecuta como modulo algo servido con `text/html`.
fn tipo_de(ruta: &str) -> &'static str {
    if ruta.ends_with(".js") || ruta.ends_with(".mjs") {
        "text/javascript; charset=utf-8"
    } else if ruta.ends_with(".css") {
        "text/css; charset=utf-8"
    } else {
        "text/html; charset=utf-8"
    }
}

/// Lee la ruta de la primera linea de una peticion HTTP (`GET /x HTTP/1.1`).
fn ruta_pedida(peticion: &str) -> String {
    peticion
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string()
}

impl TestServer {
    /// Sirve el mismo cuerpo a cada peticion que reciba, hasta que se le suelta.
    ///
    /// No usa `hyper` a proposito aunque ya sea dependencia: aqui hace falta lo
    /// contrario de un cliente HTTP correcto — un servidor tonto y predecible,
    /// que no comparta ni una linea de codigo con el que se esta probando. Si
    /// el test usara la misma pila que el motor, un fallo en esa pila podria
    /// cancelarse consigo mismo y pasar desapercibido.
    pub fn nuevo(cuerpo: &'static str) -> Self {
        Self::con_rutas_internas(None, cuerpo)
    }

    /// Sirve rutas distintas: `(ruta, cuerpo)`. Una peticion a una ruta que no
    /// este en la lista responde 404, no el cuerpo de otra: servir cualquier
    /// cosa ante una URL equivocada haria pasar tests que deberian fallar.
    ///
    /// Hace falta desde la Fase 43: un bundle real son VARIOS ficheros
    /// (`index.html`, el modulo raiz, sus fragmentos), y probarlo contra un
    /// servidor que devuelve lo mismo para todo no probaria nada.
    pub fn con_rutas(rutas: Vec<(&'static str, &'static str)>) -> Self {
        Self::con_rutas_internas(Some(rutas), "")
    }

    fn con_rutas_internas(rutas: Option<Vec<(&'static str, &'static str)>>, unico: &'static str) -> Self {
        // Puerto 0 = que el sistema operativo elija uno libre. Fijar un puerto
        // haria que dos tests en paralelo (que es como corre `cargo test`) se
        // pelearan por el.
        let listener = TcpListener::bind("127.0.0.1:0").expect("no se pudo abrir el puerto");
        let puerto = listener.local_addr().unwrap().port();

        let hilo = std::thread::spawn(move || {
            for flujo in listener.incoming() {
                let Ok(mut flujo) = flujo else { break };
                let mut buffer = [0u8; 4096];
                let leidos = flujo.read(&mut buffer).unwrap_or(0);
                let peticion = String::from_utf8_lossy(&buffer[..leidos]).to_string();

                let respuesta = match &rutas {
                    Some(rutas) => {
                        let ruta = ruta_pedida(&peticion);
                        match rutas.iter().find(|(r, _)| *r == ruta) {
                            Some((r, cuerpo)) => respuesta_http("200 OK", Some(tipo_de(r)), cuerpo),
                            None => respuesta_http("404 Not Found", None, ""),
                        }
                    }
                    None => respuesta_http("200 OK", Some("text/html; charset=utf-8"), unico),
                };

                let _ = flujo.write_all(respuesta.as_bytes());
                let _ = flujo.flush();
            }
        });

        Self { puerto, _hilo: hilo }
    }

    pub fn url(&self, ruta: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.puerto, ruta)
    }
}

/// HTML con lo justo para que TODOS los comandos del protocolo tengan algo real
/// sobre lo que actuar: un titulo (`get_state`), un enlace y un boton
/// (`click`), un `<input>` (`type_text`/`press_key`), texto suficiente para
/// que haya scroll vertical (`scroll`), y roles que el AOM pueda nombrar
/// (`get_accessibility_tree`).
pub const PAGINA: &str = r#"<!doctype html>
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

pub struct Motor {
    pub hijo: Child,
    pub entrada: ChildStdin,
    pub salida: BufReader<ChildStdout>,
}

impl Motor {
    pub fn arrancar() -> Self {
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
    pub fn pedir(&mut self, peticion: Value) -> Value {
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
    pub fn leer(&mut self) -> Value {
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
pub fn assert_correlacionada(respuesta: &Value, id_esperado: &str, tipo_esperado: &str) {
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
// Utilidades
// ---------------------------------------------------------------------------
/// El primer elemento interactivo del estado con esa etiqueta.
pub fn elemento_por_etiqueta<'a>(estado: &'a Value, etiqueta: &str) -> &'a Value {
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
pub fn centro_de(estado: &Value, etiqueta: &str) -> (f64, f64) {
    let rect = &elemento_por_etiqueta(estado, etiqueta)["rect"];
    (
        rect["x"].as_f64().unwrap() + rect["width"].as_f64().unwrap() / 2.0,
        rect["y"].as_f64().unwrap() + rect["height"].as_f64().unwrap() / 2.0,
    )
}

pub fn a_snake_case(nombre: &str) -> String {
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
pub fn decodificar_base64(entrada: &str) -> Vec<u8> {
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
