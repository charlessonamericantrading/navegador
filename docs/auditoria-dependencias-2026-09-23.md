# Auditoría de dependencias — 23-09-2026

Tarea 8 del backlog de `plan.md` (fase F02, hallazgo H02). Medido en esta
fecha; los avisos cambian sin que cambie el código, así que esto es una foto,
no un estado permanente.

**Herramientas:** `npm audit` (npm 11.9.0, Node 24.14.0) y `cargo-audit` 0.22.2
con la base de avisos de RustSec del 22-09-2026 (1.264 avisos).

## Resultado

| Árbol | Antes | Después | Cómo |
|---|---|---|---|
| `frontend` (npm) | 5: 4 altas, 1 moderada | **0** | `npm audit fix` sin `--force`: solo lockfile, dentro de los rangos declarados |
| `desktop` (npm) | 13: 12 altas, 1 crítica | **10: 9 altas, 1 crítica** | `npm audit fix` sin `--force`: `brace-expansion`, `@xmldom/xmldom`, `js-yaml` (parches) |
| `engine` (Cargo) | 2 vulnerabilidades + 4 avisos | **1 vulnerabilidad + 4 avisos** | `cargo update -p rustls`: 0.23.42 → 0.23.45 |

Verificado tras los cambios: `cargo test --workspace` y `clippy` en verde;
`frontend` compila, pasa sus 12 tests y el lint sigue en 14; `desktop` pasa sus
11 tests y `electron-builder --dir` empaqueta la aplicación con los módulos de
la Fase 53 dentro de `app.asar`.

## Clasificación de lo que queda

| Aviso | Dónde | Llega al producto | Cierre |
|---|---|---|---|
| `tar` (crítica) y cadena de `app-builder-lib`, `builder-util`, `builder-util-runtime`, `dmg-builder`, `electron-publish`, `electron-builder-squirrel-windows` | `electron-builder` 24 | **Construcción**: extracción de archivos al empaquetar. `builder-util-runtime` (fuga de credenciales en redirecciones) también lo usa `electron-updater` en **tiempo de ejecución** | `electron-builder` 24 → 26 (mayor) |
| `electron` (ASAR integrity bypass, AppleScript en macOS) y `extract-zip` | Electron 30.5.1 | **Sí**: es el runtime que se distribuye. Electron 30 está fuera de soporte | Electron 30 → 44 (mayor, 14 versiones) |
| RUSTSEC-2025-0003 y RUSTSEC-2024-0379 (`fast-float` 0.2.0) | `boa_engine`/`boa_parser`/`boa_string` 0.19 | **Sí**: lo alcanza el JavaScript de cualquier página | Boa 0.19 → 0.22 (migración de API) |
| RUSTSEC-2026-0192 (`ttf-parser`), RUSTSEC-2026-0206 (`rustybuzz`) | Tipografía del motor y de `usvg` | Sin mantenimiento; no vulnerables | Sin sustituto equivalente hoy; vigilar |
| RUSTSEC-2024-0436 (`paste`) | Macro de `boa_string` | No: solo compilación | Desaparece o no con Boa |

`npm audit` cuenta paquetes afectados, no fallos independientes explotables: los
diez de `desktop` se reducen a dos decisiones (Electron y `electron-builder`).

## Lo que no se hizo, y por qué

- **Ningún `npm audit fix --force`.** Salta versiones mayores sin revisar el
  cambio, que es justo lo que el plan prohíbe.
- **Las tres migraciones mayores** (Electron, `electron-builder`, Boa) quedan
  para decisión explícita: cada una cambia APIs o comportamiento y necesita su
  propia verificación (sandbox y `preload` en Electron; configuración y
  actualizador en `electron-builder`; cargador de módulos y `NativeFunction` en
  Boa).
- **Python** (`backend/requirements.txt`) no se auditó: no hay `pip-audit`
  instalado y el backend es opcional. Queda pendiente.

## Excepciones de CI

`.github/workflows/engine.yml` mantiene las cinco excepciones de Cargo, ahora
con la fecha de esta revisión. RUSTSEC-2026-0285 (`rustls`) no se añadió: se
corrigió. Los dos avisos de `fast-float` bloquean una release.
