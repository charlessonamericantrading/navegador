//! Punto de entrada del proceso Rust que consumira el backend de la app.

/// Pila del hilo que ejecuta el motor, en MiB.
///
/// El JavaScript de la pagina, el parser y el layout corren en el hilo que
/// atiende `run_stdio`. Con `#[tokio::main]` ese era el hilo PRINCIPAL, y en
/// Windows el hilo principal tiene 1 MiB de pila: con Boa 0.22 en build de
/// depuracion, la pagina de la sonda de APIs lo desbordaba en el runner de
/// GitHub (`thread 'main' has overflowed its stack`) aunque pasara en local.
/// Un desbordamiento de pila nativo no es una excepcion de JS: mata el proceso
/// entero. Los limites de recursion de JS los pone Boa; esta pila solo tiene
/// que ser holgada para que salten ellos antes que el sistema. Es reserva de
/// memoria virtual, no memoria usada: se compromete a medida que se toca.
///
/// `NAVEGADOR_IA_ENGINE_STACK_MB` la cambia (medir cuanta hace falta).
const DEFAULT_ENGINE_STACK_MB: usize = 64;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    let stack_mb = std::env::var("NAVEGADOR_IA_ENGINE_STACK_MB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&mb| mb > 0)
        .unwrap_or(DEFAULT_ENGINE_STACK_MB);

    // El motor corre en un hilo propio con pila holgada; el runtime de Tokio
    // es el mismo de antes (multihilo para la E/S), solo que `block_on` ya no
    // ocupa el hilo principal.
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let engine = std::thread::Builder::new()
        .name("engine".to_string())
        .stack_size(stack_mb * 1024 * 1024)
        .spawn(move || runtime.block_on(engine_core::server::run_stdio()))?;

    match engine.join() {
        Ok(result) => Ok(result?),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
