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

    // Los `tracing::info!/warn!` que el motor lleva sembrados por todo el
    // pipeline no se emitian a ninguna parte: sin suscriptor instalado,
    // `tracing` los descarta en silencio. Eso dejaba invisibles avisos que
    // SI importan ("no se pudo descargar X, se omite") y hacia imposible
    // medir en que fase se va el tiempo de una carga.
    //
    // El destino es STDERR obligatoriamente: stdout es el canal NDJSON y
    // una sola linea que no sea JSON rompe el protocolo con el backend.
    // Por defecto solo `warn` hacia arriba, para no llenar la consola en
    // uso normal; `RUST_LOG=info` activa los tiempos por fase.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    engine_core::server::run_stdio().await?;
    Ok(())
}
