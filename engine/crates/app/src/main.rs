//! `navegador`: el navegador entero, 100 % Rust.
//!
//! - `navegador [dirección]` abre la ventana.
//! - `navegador --captura salida.png [--ancho 1280] [--alto 800] [dirección]`
//!   abre la dirección sin ventana y guarda la imagen (barra incluida).
//! - `navegador --motor` es el proceso del motor (lo lanza la propia app).

// En Windows, sin consola detrás de la ventana (en las builds de depuración
// se deja para ver los mensajes).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use navegador::motor::ARGUMENTO_MOTOR;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const AYUDA: &str = "Uso:
  navegador [dirección]
  navegador --captura salida.png [--ancho 1280] [--alto 800] [--limite 60] [dirección]";

fn main() -> ExitCode {
    let argumentos: Vec<String> = std::env::args().skip(1).collect();
    if argumentos.first().map(String::as_str) == Some(ARGUMENTO_MOTOR) {
        return match engine_core::process::run_engine_process() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("[motor] {error}");
                ExitCode::FAILURE
            }
        };
    }

    let mut captura: Option<PathBuf> = None;
    let mut ancho = 1280;
    let mut alto = 800;
    let mut limite = 60;
    let mut direccion: Option<String> = None;
    let mut resto = argumentos.into_iter();
    while let Some(argumento) = resto.next() {
        let numero = |valor: Option<String>| valor.and_then(|v| v.parse::<u32>().ok());
        match argumento.as_str() {
            "--captura" => captura = resto.next().map(PathBuf::from),
            "--ancho" => ancho = numero(resto.next()).unwrap_or(ancho),
            "--alto" => alto = numero(resto.next()).unwrap_or(alto),
            "--limite" => limite = numero(resto.next()).unwrap_or(limite),
            "--ayuda" | "-h" | "--help" => {
                println!("{AYUDA}");
                return ExitCode::SUCCESS;
            }
            otro if otro.starts_with("--") => {
                eprintln!("Opción desconocida: {otro}\n{AYUDA}");
                return ExitCode::FAILURE;
            }
            otro => direccion = Some(otro.to_string()),
        }
    }

    let resultado = match captura {
        Some(ruta) => navegador::sin_ventana::capturar(&ruta, direccion, ancho, alto, Duration::from_secs(u64::from(limite))).map(|resumen| println!("{resumen}")),
        None => navegador::ventana::ejecutar(direccion),
    };
    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[navegador] {error}");
            ExitCode::FAILURE
        }
    }
}
