//! Corredor real de tests estilo WPT: carga un archivo `.html` (o un
//! directorio - todos los `*.html` dentro, no recursivo) y corre cada uno
//! via `pipeline::build_page_with_harness`, imprimiendo OK/FAIL por cada
//! `test(...)` y un resumen final.
//!
//! Uso: `cargo run -p engine-core --bin wpt_runner -- [--timeout-ms N] <archivo.html o directorio>`
//!
//! Un proceso por documento (plan F03). El runner se relanza a si mismo con
//! `--document <archivo>` para cada fixture y lo espera como mucho
//! `--timeout-ms` (por defecto 10 s). Un bucle infinito o un panico dentro
//! del motor quedan contenidos en ese hijo: el documento termina en
//! `TIMEOUT` o `CRASH` y la suite sigue con el siguiente. El hijo devuelve
//! su resultado como una linea JSON marcada por stdout; su stderr se
//! conserva y se muestra cuando el documento no termina bien.
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
//! incorrecto; `3` algun documento quedo incompleto (`HARNESS-ERROR`,
//! `TIMEOUT` o `CRASH`; tiene prioridad sobre `1`: con resultados
//! incompletos, el recuento de fallos no es fiable).

use engine_core::pipeline::build_page_with_harness;
use engine_js::TestResult;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXIT_SUBTEST_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_HARNESS_ERROR: i32 = 3;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Prefijo de la unica linea de stdout del hijo que el padre interpreta. El
/// resto de stdout (un `println!` de depuracion, por ejemplo) se ignora en
/// vez de corromper el resultado.
const RESULT_MARKER: &str = "WPT-RESULT ";
const USAGE: &str = "Uso: wpt_runner [--timeout-ms N] <archivo.html o directorio>";

fn main() {
    // Los logs a stderr: stdout del hijo es el canal del resultado.
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, file] = args.as_slice() {
        if flag == "--document" {
            run_document_in_this_process(Path::new(file));
            return;
        }
    }

    let (timeout, target) = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(EXIT_USAGE);
        }
    };

    let files = match collect_html_files(Path::new(&target)) {
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
        let outcome = run_document_in_child(file, timeout);

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
            println!("  {} {}", error.kind.label(), error.detail);
        }
        summary.add(&outcome);
    }

    println!("\n{}", summary.line());
    let code = summary.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
}

fn parse_args(args: &[String]) -> Result<(Duration, String), String> {
    let mut timeout = DEFAULT_TIMEOUT;
    let mut target = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--timeout-ms" {
            let value = iter.next().ok_or("--timeout-ms necesita un valor")?;
            let ms: u64 = value.parse().map_err(|_| format!("--timeout-ms no es un numero: {value}"))?;
            if ms == 0 {
                return Err("--timeout-ms debe ser mayor que 0".to_string());
            }
            timeout = Duration::from_millis(ms);
        } else if target.is_none() {
            target = Some(arg.clone());
        } else {
            return Err(format!("argumento inesperado: {arg}"));
        }
    }
    Ok((timeout, target.ok_or("falta el archivo o directorio")?))
}

/// Modo hijo: ejecuta UN documento y escribe su resultado como una linea
/// JSON marcada. Sale con 0 aunque haya fallos: el veredicto es del padre.
fn run_document_in_this_process(file: &Path) {
    let outcome = match std::fs::read_to_string(file) {
        // Sin scripts/hojas externos aqui a proposito: los fixtures de
        // `engine/tests/wpt-style/` son autocontenidos (inline), y este
        // runner no tiene acceso a red - ver el doc-comment de este archivo.
        Ok(html) => {
            let (page, test_results) = build_page_with_harness(&html, "", 800.0, 600.0, None, &std::collections::HashMap::new(), &engine_layout::ImageMap::new());
            classify_document(&page.script_results, test_results)
        }
        Err(e) => DocumentOutcome::incomplete(IncompleteKind::HarnessError, format!("no se pudo leer el archivo: {e}")),
    };
    let json = serde_json::to_string(&outcome).expect("DocumentOutcome siempre es serializable");
    println!("{RESULT_MARKER}{json}");
}

/// Modo padre: lanza el hijo, lo espera hasta `timeout` y lo mata si no ha
/// terminado. stdout y stderr se leen en hilos aparte para que un hijo que
/// escribe mucho no se bloquee con la tuberia llena mientras se le espera.
fn run_document_in_child(file: &Path, timeout: Duration) -> DocumentOutcome {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return DocumentOutcome::incomplete(IncompleteKind::HarnessError, format!("no se pudo localizar el propio ejecutable: {e}")),
    };
    let mut child = match Command::new(exe).arg("--document").arg(file).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(e) => return DocumentOutcome::incomplete(IncompleteKind::HarnessError, format!("no se pudo lanzar el proceso hijo: {e}")),
    };
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                // El hijo no lanza procesos propios, asi que matarlo cierra
                // el arbol entero.
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                let _ = child.kill();
                return DocumentOutcome::incomplete(IncompleteKind::HarnessError, format!("no se pudo esperar al proceso hijo: {e}"));
            }
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    let Some(status) = status else {
        return DocumentOutcome::incomplete(IncompleteKind::Timeout, with_stderr(format!("sin terminar tras {} ms", timeout.as_millis()), &stderr));
    };
    let parsed = stdout.lines().find_map(|line| line.strip_prefix(RESULT_MARKER)).map(serde_json::from_str::<DocumentOutcome>);
    match (status.success(), parsed) {
        (true, Some(Ok(outcome))) => outcome,
        (true, Some(Err(e))) => DocumentOutcome::incomplete(IncompleteKind::HarnessError, format!("resultado ilegible del proceso hijo: {e}")),
        _ => DocumentOutcome::incomplete(IncompleteKind::Crash, with_stderr(format!("el proceso hijo termino con {status} sin resultado valido"), &stderr)),
    }
}

fn read_in_background<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Anade el final del stderr del hijo al diagnostico: es donde quedan el
/// mensaje de un panico o los ultimos logs antes de colgarse.
fn with_stderr(detail: String, stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return detail;
    }
    let tail = &lines[lines.len().saturating_sub(5)..];
    format!("{detail}\n      stderr: {}", tail.join("\n              "))
}

/// Lo que produjo un documento: sus subtests y los motivos (si los hay) por
/// los que no se pudo completar.
#[derive(Debug, Serialize, Deserialize)]
struct DocumentOutcome {
    harness_errors: Vec<Incomplete>,
    subtests: Vec<Subtest>,
}

impl DocumentOutcome {
    fn incomplete(kind: IncompleteKind, detail: String) -> Self {
        DocumentOutcome { harness_errors: vec![Incomplete { kind, detail }], subtests: Vec::new() }
    }
}

/// Copia serializable de `engine_js::TestResult`, que cruza la frontera
/// entre procesos.
#[derive(Debug, Serialize, Deserialize)]
struct Subtest {
    name: String,
    passed: bool,
    failure_message: Option<String>,
}

impl From<TestResult> for Subtest {
    fn from(r: TestResult) -> Self {
        Subtest { name: r.name, passed: r.passed, failure_message: r.failure_message }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Incomplete {
    kind: IncompleteKind,
    detail: String,
}

/// Por que un documento no llego a un resultado completo. Se distinguen
/// porque piden acciones distintas: un `HARNESS-ERROR` suele ser el fixture
/// o una API ausente; un `TIMEOUT`, un bucle o una espera que no termina;
/// un `CRASH`, un panico del motor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum IncompleteKind {
    HarnessError,
    Timeout,
    Crash,
}

impl IncompleteKind {
    fn label(self) -> &'static str {
        match self {
            IncompleteKind::HarnessError => "HARNESS-ERROR",
            IncompleteKind::Timeout => "TIMEOUT",
            IncompleteKind::Crash => "CRASH",
        }
    }
}

/// Separa el estado del arnes de los subtests. Funcion pura a proposito:
/// la regla de que cuenta como `HARNESS-ERROR` se prueba sin ejecutar JS.
fn classify_document(script_results: &[Result<String, String>], subtests: Vec<TestResult>) -> DocumentOutcome {
    let mut harness_errors: Vec<Incomplete> = script_results
        .iter()
        .enumerate()
        .filter_map(|(index, result)| {
            result.as_ref().err().map(|e| Incomplete { kind: IncompleteKind::HarnessError, detail: format!("excepcion no capturada en el script #{}: {e}", index + 1) })
        })
        .collect();
    if subtests.is_empty() {
        harness_errors.push(Incomplete { kind: IncompleteKind::HarnessError, detail: "el documento no registro ningun test(...)".to_string() });
    }
    DocumentOutcome { harness_errors, subtests: subtests.into_iter().map(Subtest::from).collect() }
}

#[derive(Debug, Default)]
struct Summary {
    passed: usize,
    failed: usize,
    /// Documentos incompletos, contados por el PRIMER motivo de cada uno.
    harness_errors: usize,
    timeouts: usize,
    crashes: usize,
}

impl Summary {
    fn add(&mut self, outcome: &DocumentOutcome) {
        self.passed += outcome.subtests.iter().filter(|t| t.passed).count();
        self.failed += outcome.subtests.iter().filter(|t| !t.passed).count();
        match outcome.harness_errors.first().map(|e| e.kind) {
            Some(IncompleteKind::HarnessError) => self.harness_errors += 1,
            Some(IncompleteKind::Timeout) => self.timeouts += 1,
            Some(IncompleteKind::Crash) => self.crashes += 1,
            None => {}
        }
    }

    fn line(&self) -> String {
        format!(
            "{} pasaron, {} fallaron, {} en total; documentos incompletos: {} HARNESS-ERROR, {} TIMEOUT, {} CRASH",
            self.passed,
            self.failed,
            self.passed + self.failed,
            self.harness_errors,
            self.timeouts,
            self.crashes
        )
    }

    fn exit_code(&self) -> i32 {
        if self.harness_errors + self.timeouts + self.crashes > 0 {
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
        assert_eq!(outcome.harness_errors.len(), 1);
        assert_eq!(outcome.harness_errors[0].detail, "el documento no registro ningun test(...)");
    }

    /// Un script que lanza DESPUES de tests que pasaron deja el documento
    /// incompleto: los subtests se conservan, pero no bastan para aprobar.
    #[test]
    fn an_exception_after_passing_tests_keeps_the_subtests_but_fails_the_run() {
        let outcome = classify_document(&[Ok("undefined".to_string()), Err("TypeError".to_string())], vec![subtest("a", true)]);
        assert_eq!(outcome.subtests.len(), 1);
        assert!(outcome.harness_errors[0].detail.contains("script #2"));
        assert_eq!(summarize(&[outcome]).exit_code(), EXIT_HARNESS_ERROR);
    }

    #[test]
    fn exit_code_distinguishes_clean_run_failed_subtests_and_incomplete_documents() {
        let clean = classify_document(&[Ok(String::new())], vec![subtest("a", true)]);
        assert_eq!(summarize(&[clean]).exit_code(), 0);

        let failing = classify_document(&[Ok(String::new())], vec![subtest("a", true), subtest("b", false)]);
        assert_eq!(summarize(&[failing]).exit_code(), EXIT_SUBTEST_FAILED);

        let failing = classify_document(&[Ok(String::new())], vec![subtest("b", false)]);
        let hung = DocumentOutcome::incomplete(IncompleteKind::Timeout, "sin terminar".to_string());
        let summary = summarize(&[failing, hung]);
        assert_eq!(summary.timeouts, 1);
        assert_eq!(summary.exit_code(), EXIT_HARNESS_ERROR, "un documento incompleto tiene prioridad");
    }

    /// El resultado cruza la frontera entre procesos como JSON: lo que sale
    /// del hijo tiene que volver identico en el padre.
    #[test]
    fn document_outcome_survives_the_json_round_trip() {
        let outcome = classify_document(&[Err("boom".to_string())], vec![subtest("con\nsalto y \"comillas\"", false)]);
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains('\n'), "el resultado debe caber en una sola linea de stdout");
        let back: DocumentOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back.subtests[0].name, "con\nsalto y \"comillas\"");
        assert_eq!(back.harness_errors[0].kind, IncompleteKind::HarnessError);
    }

    #[test]
    fn parse_args_reads_the_timeout_and_rejects_nonsense() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&args(&["dir"])).unwrap(), (DEFAULT_TIMEOUT, "dir".to_string()));
        assert_eq!(parse_args(&args(&["--timeout-ms", "250", "dir"])).unwrap(), (Duration::from_millis(250), "dir".to_string()));
        assert!(parse_args(&args(&["--timeout-ms", "0", "dir"])).is_err());
        assert!(parse_args(&args(&["--timeout-ms", "x", "dir"])).is_err());
        assert!(parse_args(&args(&["a", "b"])).is_err());
        assert!(parse_args(&args(&[])).is_err());
    }
}
