//! Punto de entrada del proceso Rust que consumira el backend de la app.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    engine_core::server::run_stdio().await?;
    Ok(())
}
