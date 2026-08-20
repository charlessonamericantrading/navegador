//! Punto de entrada del proceso Rust que consumira el backend de la app.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Mitigaciones de proceso (Fase 23) - LO PRIMERO, antes de tocar la
    // red o parsear nada: este es el proceso que interpreta contenido
    // hostil, y las politicas de Windows solo pueden endurecerse, nunca
    // aflojarse, asi que aplicarlas cuanto antes es estrictamente mejor.
    //
    // El informe va a STDERR, nunca a stdout: stdout es el canal NDJSON y
    // cualquier linea que no sea JSON romperia el protocolo.
    let mitigations = engine_core::sandbox::apply_process_mitigations();
    eprintln!("[engine] {}", mitigations.summary());

    engine_core::server::run_stdio().await?;
    Ok(())
}
