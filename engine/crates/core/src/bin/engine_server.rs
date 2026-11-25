//! Proceso del motor que habla NDJSON por stdin/stdout (ver `engine_core::process`).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    engine_core::process::run_engine_process()
}
