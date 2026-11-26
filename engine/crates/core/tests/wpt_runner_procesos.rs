//! `wpt_runner` contra el binario REAL, con fixtures que rompen el arnes
//! (plan F03). Los tests unitarios del binario prueban la clasificacion; este
//! prueba lo que solo existe con procesos de verdad: que un documento colgado
//! se mata al vencer el plazo y que la suite sigue con el siguiente.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Directorio temporal propio del test, borrado al terminar aunque falle.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(nombre: &str, documentos: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("wpt-runner-{nombre}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (archivo, html) in documentos {
            std::fs::write(dir.join(archivo), html).unwrap();
        }
        Fixtures(dir)
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn correr(dir: &PathBuf, timeout_ms: u64) -> (Option<i32>, String, Duration) {
    let inicio = Instant::now();
    let salida = Command::new(env!("CARGO_BIN_EXE_wpt_runner")).arg("--timeout-ms").arg(timeout_ms.to_string()).arg(dir).output().expect("wpt_runner deberia arrancar");
    (salida.status.code(), String::from_utf8_lossy(&salida.stdout).into_owned(), inicio.elapsed())
}

const PASA: &str = "<html><body><script>test(function() { assert_true(true); }, 'NOMBRE');</script></body></html>";

#[test]
fn un_documento_colgado_termina_en_timeout_y_la_suite_continua() {
    let antes = PASA.replace("NOMBRE", "antes del bucle");
    let despues = PASA.replace("NOMBRE", "despues del bucle");
    let fixtures = Fixtures::new(
        "timeout",
        &[("a.html", &antes), ("b-bucle.html", "<html><body><script>while (true) {}</script></body></html>"), ("c.html", &despues)],
    );

    let (codigo, stdout, duracion) = correr(&fixtures.0, 1500);

    assert_eq!(codigo, Some(3), "un documento incompleto hace fallar la suite:\n{stdout}");
    assert!(stdout.contains("TIMEOUT sin terminar tras 1500 ms"), "{stdout}");
    assert!(stdout.contains("OK   antes del bucle"), "{stdout}");
    assert!(stdout.contains("OK   despues del bucle"), "el documento posterior al bucle tambien corre:\n{stdout}");
    assert!(stdout.contains("2 pasaron, 0 fallaron, 2 en total; documentos incompletos: 0 HARNESS-ERROR, 1 TIMEOUT, 0 CRASH"), "{stdout}");
    assert!(duracion < Duration::from_secs(60), "el plazo tiene que cortar el bucle, tardo {duracion:?}");
}

/// La reproduccion 11.2 del plan, de punta a punta: antes daba `0/0` y
/// salida 0.
#[test]
fn una_excepcion_antes_de_cualquier_test_no_aprueba_la_suite() {
    let fixtures = Fixtures::new("excepcion", &[("excepcion.html", "<html><body><script>noExiste(); test(function() {}, 'nunca llega');</script></body></html>")]);

    let (codigo, stdout, _) = correr(&fixtures.0, 10_000);

    assert_eq!(codigo, Some(3), "{stdout}");
    assert!(stdout.contains("HARNESS-ERROR excepcion no capturada en el script #1"), "{stdout}");
    assert!(stdout.contains("noExiste"), "{stdout}");
}

#[test]
fn una_suite_sana_sale_con_cero() {
    let fixtures = Fixtures::new("sana", &[("a.html", &PASA.replace("NOMBRE", "sano"))]);

    let (codigo, stdout, _) = correr(&fixtures.0, 10_000);

    assert_eq!(codigo, Some(0), "{stdout}");
    assert!(stdout.contains("1 pasaron, 0 fallaron, 1 en total"), "{stdout}");
}
