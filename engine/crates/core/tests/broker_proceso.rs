//! El broker como proceso aparte (ADR 0001, etapa 2), contra los binarios
//! reales: `engine_broker` con la red y el perfil, y dos `engine_server` que
//! solo tienen el canal hacia él.

mod common;

use common::{BrokerProceso, Motor, TestServer};
use serde_json::json;
use std::process::Stdio;
use std::time::{Duration, Instant};

const ESCRIBE: &str = r#"<!doctype html><html><head><title>sin script</title></head><body>
<script>
  localStorage.setItem('clave', 'compartida');
  sessionStorage.setItem('pestana', 'a');
  document.cookie = 'galleta=1; path=/';
  document.title = 'escrito';
</script></body></html>"#;

const LEE: &str = r#"<!doctype html><html><head><title>sin script</title></head><body>
<script>
  document.title = [localStorage.getItem('clave'), String(sessionStorage.getItem('pestana')), document.cookie].join('|');
</script></body></html>"#;

fn navegar(motor: &mut Motor, id: &str, url: &str) -> serde_json::Value {
    motor.pedir(json!({ "id": id, "type": "navigate", "url": url }))
}

/// El criterio de la etapa 2: dos renderers, un solo dueño del estado. Lo que
/// una pestaña guarda lo ve la otra (antes cada proceso tenía su copia y se
/// pisaban el fichero), `sessionStorage` sigue siendo de cada pestaña, y
/// ningún renderer abre el perfil en disco.
#[test]
fn dos_motores_comparten_el_estado_del_broker_y_ninguno_toca_el_disco() {
    let servidor = TestServer::con_rutas(vec![("/escribe", ESCRIBE), ("/lee", LEE)]);
    let mut broker = BrokerProceso::arrancar();
    let token_a = broker.registrar("tab-a");
    let token_b = broker.registrar("tab-b");

    let mut a = Motor::arrancar_con_broker(&broker.endpoint, &token_a);
    let mut b = Motor::arrancar_con_broker(&broker.endpoint, &token_b);
    // `arrancar` ya exige el `ready`; aquí, además, que sea con broker remoto.
    let pong = a.pedir(json!({ "id": "p", "type": "ping" }));
    assert_eq!((pong["type"].as_str(), pong["broker"].as_str()), (Some("pong"), Some("remote")), "{pong}");

    let escrito = navegar(&mut a, "1", &servidor.url("/escribe"));
    assert_eq!(escrito["type"], "state", "{escrito}");
    assert_eq!(escrito["title"], "escrito");

    let leido = navegar(&mut b, "2", &servidor.url("/lee"));
    assert_eq!(leido["type"], "state", "{leido}");
    assert_eq!(leido["title"], "compartida|null|galleta=1", "B ve el localStorage y la cookie de A, no su sessionStorage");

    assert!(!a.perfil().exists(), "el renderer A no debe crear su perfil: {:?}", a.perfil());
    assert!(!b.perfil().exists(), "el renderer B no debe crear su perfil: {:?}", b.perfil());
    assert!(broker.perfil().join("local_storage.json").exists(), "el localStorage lo persiste el broker");
}

#[test]
fn el_saludo_dice_que_broker_usa_el_motor() {
    let mut broker = BrokerProceso::arrancar();
    let token = broker.registrar("tab-a");
    let perfil = common::perfil_temporal();
    for (entorno, esperado) in [(vec![], "local"), (vec![("NAVEGADOR_IA_BROKER", broker.endpoint.as_str()), ("NAVEGADOR_IA_BROKER_TOKEN", token.as_str())], "remote")] {
        let mut hijo = Motor::comando(&perfil, &entorno).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut salida = std::io::BufReader::new(hijo.stdout.take().unwrap());
        let mut linea = String::new();
        std::io::BufRead::read_line(&mut salida, &mut linea).unwrap();
        let saludo: serde_json::Value = serde_json::from_str(&linea).unwrap();
        let _ = hijo.kill();
        let _ = hijo.wait();
        assert_eq!(saludo["broker"], esperado, "{saludo}");
    }
    let _ = std::fs::remove_dir_all(&perfil);
}

/// Principio 7 del plan: pedir broker y no tenerlo no es "seguir con la red
/// propia", es no arrancar.
#[test]
fn sin_broker_o_con_token_ajeno_el_motor_no_arranca() {
    let mut broker = BrokerProceso::arrancar();
    let token = broker.registrar("tab-a");
    let perfil = common::perfil_temporal();
    let casos: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("canal inexistente", vec![("NAVEGADOR_IA_BROKER", if cfg!(windows) { r"\\.\pipe\navegador-ia-no-existe" } else { "/tmp/nia-no-existe.sock" }), ("NAVEGADOR_IA_BROKER_TOKEN", "x")]),
        ("token inventado", vec![("NAVEGADOR_IA_BROKER", broker.endpoint.as_str()), ("NAVEGADOR_IA_BROKER_TOKEN", "inventado")]),
        ("canal sin token", vec![("NAVEGADOR_IA_BROKER", broker.endpoint.as_str())]),
        ("token ya usado", vec![("NAVEGADOR_IA_BROKER", broker.endpoint.as_str()), ("NAVEGADOR_IA_BROKER_TOKEN", token.as_str())]),
    ];
    // El último caso necesita que alguien haya gastado ya el token.
    let _primero = Motor::arrancar_con_broker(&broker.endpoint, &token);

    for (caso, entorno) in casos {
        let salida = Motor::comando(&perfil, &entorno).stdin(Stdio::null()).stderr(Stdio::null()).output().unwrap();
        assert!(!salida.status.success(), "{caso}: el motor arrancó igual");
        assert!(!String::from_utf8_lossy(&salida.stdout).contains("\"ready\""), "{caso}: el motor saludó con `ready` sin broker");
    }
    assert!(!perfil.exists(), "ningún intento fallido debe acabar abriendo el perfil propio");
}

/// Si el broker muere, la navegación falla enseguida con un error de red
/// (nada de esperar un plazo), y el motor sigue vivo para contestar.
#[test]
fn si_el_broker_cae_el_motor_falla_rapido_y_sigue_vivo() {
    let servidor = TestServer::nuevo(common::PAGINA);
    let mut broker = BrokerProceso::arrancar();
    let token = broker.registrar("tab-a");
    let mut motor = Motor::arrancar_con_broker(&broker.endpoint, &token);
    assert_eq!(navegar(&mut motor, "1", &servidor.url("/"))["type"], "state");

    broker.hijo.kill().unwrap();
    broker.hijo.wait().unwrap();

    let inicio = Instant::now();
    let respuesta = navegar(&mut motor, "2", &servidor.url("/"));
    assert_eq!(respuesta["type"], "error", "{respuesta}");
    assert!(respuesta["message"].as_str().unwrap().contains("broker"), "el error dice que falta el broker: {respuesta}");
    assert!(inicio.elapsed() < Duration::from_secs(5), "tardó {:?}", inicio.elapsed());
    assert_eq!(motor.pedir(json!({ "id": "3", "type": "ping" }))["type"], "pong");
}
