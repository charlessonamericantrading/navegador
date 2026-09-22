//! Corredor real de tests estilo WPT: carga un archivo `.html` (o un
//! directorio - todos los `*.html` dentro, no recursivo) y corre cada uno
//! via `pipeline::build_page_with_harness`, imprimiendo OK/FAIL por cada
//! `test(...)` y un resumen final.
//!
//! Uso: `cargo run -p engine-core --bin wpt_runner -- <archivo.html o directorio>`
//!
//! Importante lo que esto NO es: no descarga ni interpreta la corpus real
//! de Web Platform Tests (ver ARCHITECTURE.md, "Metrica de progreso") - los
//! archivos de `engine/tests/wpt-style/` son fixtures escritas a mano en el
//! MISMO estilo (`test`/`assert_equals`/`assert_true` de `testharness.js`)
//! que ejercitan capacidad real que el motor ya tiene, no la suite oficial.
//!
//! Estado del documento frente a resultado de subtests (plan F03). Igual que
//! `testharness.js`, el runner separa dos cosas que antes mezclaba:
//!
//! - el ESTADO DEL ARNES por documento: si el documento se leyo, si todos
//!   sus `<script>` terminaron sin excepcion y si registro al menos un
//!   `test(...)`. Cualquiera de esas tres cosas que falle es un
//!   `HARNESS-ERROR`: los resultados de ese documento estan incompletos.
//! - el resultado de cada SUBTEST (`OK`/`FAIL`).
//!
//! Antes, un documento cuyo script lanzaba antes de llegar a `test(...)`
//! producia `0/0` y codigo de salida 0 - un falso verde. `0/0` ya nunca
//! cuenta como aprobado.
//!
//! Codigos de salida: `0` todo aprobado; `1` algun subtest fallo; `2` uso
//! incorrecto; `3` algun documento termino en `HARNESS-ERROR` (tiene
//! prioridad sobre `1`: con resultados incompletos, el recuento de fallos no
//! es fiable).

use engine_core::pipeline::build_page_with_harness;
use engine_js::TestResult;
use std::path::{Path, PathBuf};

const EXIT_SUBTEST_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_HARNESS_ERROR: i32 = 3;

fn main() {
    tracing_subscriber::fmt::init();

    let target = match std::env::args().nth(1) {
        Some(arg) => arg,
        None => {
            eprintln!("Uso: wpt_runner <archivo.html o directorio>");
            std::process::exit(EXIT_USAGE);
        }
    };

    let path = PathBuf::from(&target);
    let files = match collect_html_files(&path) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            eprintln!("No se encontraron archivos .html en {target}");
            std::process::exit(EXIT_USAGE);
        }
        Err(e) => {
            eprintln!("No se pudo leer {target}: {e}");
            std::process::exit(EXIT_USAGE);
        }
    };

    let mut summary = Summary::default();
    for file in &files {
        let outcome = match std::fs::read_to_string(file) {
            // Sin scripts/hojas externos aqui a proposito: los fixtures de
            // `engine/tests/wpt-style/` son autocontenidos (inline), y este
            // runner no tiene acceso a red - ver el doc-comment de este archivo.
            Ok(html) => {
                let (page, test_results) = build_page_with_harness(&html, "", 800.0, 600.0, None, &std::collections::HashMap::new(), &engine_layout::ImageMap::new());
                classify_document(&page.script_results, test_results)
            }
            Err(e) => DocumentOutcome { harness_errors: vec![format!("no se pudo leer el archivo: {e}")], subtests: Vec::new() },
        };

        println!("\n{}", file.display());
        for result in &outcome.subtests {
            if result.passed {
                println!("  OK   {}", result.name);
            } else {
                let message = result.failure_message.as_deref().unwrap_or("(sin mensaje)");
                println!("  FAIL {} - {message}", result.name);
            }
        }
        for error in &outcome.harness_errors {
            println!("  HARNESS-ERROR {error}");
        }
        summary.add(&outcome);
    }

    println!("\n{}", summary.line());
    let code = summary.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
}

/// Lo que produjo un documento: sus subtests y los motivos (si los hay) por
/// los que el arnes no pudo completarlo.
#[derive(Debug)]
struct DocumentOutcome {
    harness_errors: Vec<String>,
    subtests: Vec<TestResult>,
}

/// Separa el estado del arnes de los subtests. Funcion pura a proposito:
/// la regla de que cuenta como `HARNESS-ERROR` se prueba sin ejecutar JS.
fn classify_document(script_results: &[Result<String, String>], subtests: Vec<TestResult>) -> DocumentOutcome {
    let mut harness_errors: Vec<String> = script_results
        .iter()
        .enumerate()
        .filter_map(|(index, result)| result.as_ref().err().map(|e| format!("excepcion no capturada en el script #{}: {e}", index + 1)))
        .collect();
    if subtests.is_empty() {
        harness_errors.push("el documento no registro ningun test(...)".to_string());
    }
    DocumentOutcome { harness_errors, subtests }
}

#[derive(Debug, Default)]
struct Summary {
    passed: usize,
    failed: usize,
    harness_errors: usize,
}

impl Summary {
    fn add(&mut self, outcome: &DocumentOutcome) {
        self.passed += outcome.subtests.iter().filter(|t| t.passed).count();
        self.failed += outcome.subtests.iter().filter(|t| !t.passed).count();
        if !outcome.harness_errors.is_empty() {
            self.harness_errors += 1;
        }
    }

    fn line(&self) -> String {
        format!(
            "{} pasaron, {} fallaron, {} en total; {} documento(s) con HARNESS-ERROR",
            self.passed,
            self.failed,
            self.passed + self.failed,
            self.harness_errors
        )
    }

    fn exit_code(&self) -> i32 {
        if self.harness_errors > 0 {
            EXIT_HARNESS_ERROR
        } else if self.failed > 0 {
            EXIT_SUBTEST_FAILED
        } else {
            0
        }
    }
}

fn collect_html_files(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("html"))
            .collect();
        files.sort();
        Ok(files)
    } else {
        Ok(vec![path.to_path_buf()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subtest(name: &str, passed: bool) -> TestResult {
        TestResult { name: name.to_string(), passed, failure_message: (!passed).then(|| "fallo".to_string()) }
    }

    fn summarize(outcomes: &[DocumentOutcome]) -> Summary {
        let mut summary = Summary::default();
        for outcome in outcomes {
            summary.add(outcome);
        }
        summary
    }

    /// La reproduccion del plan (11.2): un script que lanza antes de
    /// registrar ningun test daba `0/0` y salida 0.
    #[test]
    fn a_script_that_throws_before_any_test_is_a_harness_error_not_a_pass() {
        let outcome = classify_document(&[Err("ReferenceError: noExiste is not defined".to_string())], Vec::new());
        assert_eq!(outcome.harness_errors.len(), 2, "excepcion y ausencia de tests: {:?}", outcome.harness_errors);
        assert_eq!(summarize(&[outcome]).exit_code(), EXIT_HARNESS_ERROR);
    }

    #[test]
    fn a_document_without_tests_is_a_harness_error_even_if_its_scripts_ran() {
        let outcome = classify_document(&[Ok("undefined".to_string())], Vec::new());
        assert_eq!(outcome.harness_errors, vec!["el documento no registro ningun test(...)".to_string()]);
    }

    /// Un script que lanza DESPUES de tests que pasaron deja el documento
    /// incompleto: los subtests se conservan, pero no bastan para aprobar.
    #[test]
    fn an_exception_after_passing_tests_keeps_the_subtests_but_fails_the_run() {
        let outcome = classify_document(&[Ok("undefined".to_string()), Err("TypeError".to_string())], vec![subtest("a", true)]);
        assert_eq!(outcome.subtests.len(), 1);
        assert!(outcome.harness_errors[0].contains("script #2"));
        assert_eq!(summarize(&[outcome]).exit_code(), EXIT_HARNESS_ERROR);
    }

    #[test]
    fn exit_code_distinguishes_clean_run_failed_subtests_and_harness_errors() {
        let clean = classify_document(&[Ok(String::new())], vec![subtest("a", true)]);
        assert_eq!(summarize(&[clean]).exit_code(), 0);

        let failing = classify_document(&[Ok(String::new())], vec![subtest("a", true), subtest("b", false)]);
        assert_eq!(summarize(&[failing]).exit_code(), EXIT_SUBTEST_FAILED);

        let failing = classify_document(&[Ok(String::new())], vec![subtest("b", false)]);
        let broken = classify_document(&[], Vec::new());
        assert_eq!(summarize(&[failing, broken]).exit_code(), EXIT_HARNESS_ERROR, "el error de arnes tiene prioridad");
    }
}
