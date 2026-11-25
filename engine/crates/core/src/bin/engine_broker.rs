//! El broker (ADR 0001, etapa 2): el proceso que tiene la red, las cookies y
//! el almacenamiento, y los sirve a los renderers (`engine_server`) por un
//! canal local. Toda la lógica está en `engine_net::broker_server`; esto solo
//! monta el proceso.
//!
//! Vive en `engine-core` y no en `engine-net` por una razón práctica: así los
//! tests de integración del motor pueden arrancar los dos binarios reales
//! (`CARGO_BIN_EXE_*` solo existe para los del mismo paquete). No enlaza
//! nada del motor: ni JavaScript, ni HTML, ni layout.
//!
//! Protocolo de control (stdin/stdout, NDJSON, solo para el supervisor):
//!
//! - al arrancar: `{"type":"ready","endpoint":"..."}`
//! - `{"id":..,"type":"register","renderer":"tab-1"}` → `registered` con `token`
//! - `{"id":..,"type":"revoke","renderer":"tab-1"}` → `revoked`
//! - `{"id":..,"type":"ping"}` → `pong`; `{"type":"shutdown"}` → `bye` y sale.
//!
//! Cerrar su stdin también lo termina.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    engine_core::process::run_broker_process()
}
