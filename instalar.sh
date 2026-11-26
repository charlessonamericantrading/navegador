#!/usr/bin/env bash
# Compila e instala el navegador (100 % Rust). Necesita Rust: https://rustup.rs/
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v cargo >/dev/null 2>&1; then
  echo "[ERROR] Rust no está instalado (falta cargo). Instálalo desde https://rustup.rs/" >&2
  exit 1
fi
echo "Compilando el navegador y el instalador (la primera vez tarda unos minutos)..."
cargo build --release --locked --manifest-path engine/Cargo.toml --bin navegador --bin instalador
engine/target/release/instalador --origen engine/target/release/navegador --silencioso
echo "Listo. Ábrelo desde el menú de aplicaciones o con ./iniciar.sh"
