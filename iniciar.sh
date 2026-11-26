#!/usr/bin/env bash
# Abre el navegador. Antes: ./instalar.sh
set -euo pipefail
cd "$(dirname "$0")"
if [ -x "$HOME/.local/bin/navegador" ]; then
  exec "$HOME/.local/bin/navegador" "$@"
fi
binario=engine/target/release/navegador
if [ ! -x "$binario" ]; then
  echo "Primero ejecuta ./instalar.sh" >&2
  exit 1
fi
exec "$binario" "$@"
