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
//! La mayoria de tests WPT reales fallarian en cascada de todas formas: les
//! falta `fetch`/`XMLHttpRequest`, eventos, la mayor parte de CSSOM... asi
//! que vendorizar la corpus real sin soporte para eso serviria de poco.

use engine_core::pipeline::build_page_with_harness;
use std::path::{Path, PathBuf};

fn main() {
    tracing_subscriber::fmt::init();

    let target = match std::env::args().nth(1) {
        Some(arg) => arg,
        None => {
            eprintln!("Uso: wpt_runner <archivo.html o directorio>");
            std::process::exit(2);
        }
    };

    let path = PathBuf::from(&target);
    let files = match collect_html_files(&path) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            eprintln!("No se encontraron archivos .html en {target}");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("No se pudo leer {target}: {e}");
            std::process::exit(2);
        }
    };

    let mut total_passed = 0usize;
    let mut total_failed = 0usize;

    for file in &files {
        let html = match std::fs::read_to_string(file) {
            Ok(contents) => contents,
            Err(e) => {
                eprintln!("No se pudo leer {}: {e}", file.display());
                continue;
            }
        };

        // Sin scripts/hojas externos aqui a proposito: los fixtures de
        // `engine/tests/wpt-style/` son autocontenidos (inline), y este
        // runner no tiene acceso a red - ver el doc-comment de este archivo.
        let (_, test_results) = build_page_with_harness(&html, "", 800.0, 600.0, None, &std::collections::HashMap::new());
        println!("\n{}", file.display());
        for result in &test_results {
            if result.passed {
                total_passed += 1;
                println!("  OK   {}", result.name);
            } else {
                total_failed += 1;
                let message = result.failure_message.as_deref().unwrap_or("(sin mensaje)");
                println!("  FAIL {} - {message}", result.name);
            }
        }
        if test_results.is_empty() {
            println!("  (sin llamadas a test(...) en este archivo)");
        }
    }

    println!("\n{total_passed} pasaron, {total_failed} fallaron, {} en total", total_passed + total_failed);
    if total_failed > 0 {
        std::process::exit(1);
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
