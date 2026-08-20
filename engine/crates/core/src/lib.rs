//! Expone `pipeline`/`scripting`/`platform` como biblioteca ademas de como
//! modulos internos de `main.rs`, para que un SEGUNDO binario dentro de este
//! mismo crate (`src/bin/wpt_runner.rs`) pueda reusarlos sin duplicar nada -
//! sin este `lib.rs`, cada `[[bin]]` de Cargo compila como una raiz de
//! crate independiente y no puede ver los modulos de otra.

pub mod pipeline;
pub mod scripting;
pub mod platform;
pub mod sandbox;
pub mod protocol;
pub mod server;
