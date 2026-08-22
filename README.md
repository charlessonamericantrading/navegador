# Navegador IA

Un navegador de escritorio con un **motor de renderizado propio escrito desde cero en Rust** — sin Chromium, sin WebKit, sin Gecko.

> **Estado: en desarrollo, no apto para uso general todavía.**
> El motor renderiza páginas reales por HTTPS, pero le faltan piezas de
> seguridad imprescindibles (política de mismo origen, aislamiento de
> procesos). Ver [Qué falta](#qué-falta) antes de usarlo con sitios en los
> que no confíes.

---

## Qué hace de verdad hoy

Todo lo de esta lista está implementado y verificado ejecutando el motor, no
solo compilándolo. Las cifras salen de correr la suite de tests el
2026-08-20.

| | |
|---|---|
| **Motor** | ~23.000 líneas de Rust, 10 crates |
| **Tests** | 683 pasando, 0 fallando |
| **Red** | HTTP/1.1 + HTTPS real (`hyper` + `rustls`), redirecciones, gzip/deflate/brotli, cookies RFC 6265, CORS y CSP |
| **HTML** | Parseo con `html5ever` (el de Servo), DOM mutable, `<canvas>` 2D context |
| **CSS** | Cascada con especificidad real, selectores con combinadores (`selectors`, el de Firefox), pseudo-clases, `@media`, `rem`, porcentajes |
| **Layout** | Bloque, inline, `box-sizing: border-box`, flexbox con *intrinsic sizing*, CSS Grid, tablas, `float`, `position` (sticky/relative/absolute) |
| **JavaScript** | Motor `boa`, DOM bindings, eventos con burbujeo, `fetch`, `XMLHttpRequest`, `setTimeout`, `localStorage`, Canvas 2D API |
| **Pintado** | Rasterizado con `tiny-skia`, gráficos vectoriales SVG con `resvg`/`usvg`, fuentes con glifos reales, `border-radius`, sombras |
| **IA Nativa** | Crate `engine-ai` con Árbol de Accesibilidad Semántico (AOM) con coordenadas espaciales optimizado para LLMs |

**Lo que se puede hacer con él ahora mismo:** cargar una página real por
HTTPS, navegar por enlaces, usar el historial, abrir pestañas, rellenar y
**enviar formularios** (GET y POST), **iniciar sesión** en un sitio con
autenticación por cookies, e **interactuar con el Agente Copiloto IA**
autónomo desde la barra lateral.

---

## Qué falta

Esto es la parte importante de este README, y está aquí arriba a propósito.

### Seguridad — antes de distribuirlo a nadie

| Pieza | Estado |
|---|---|
| Validación TLS de certificados | ✅ Real (`webpki-roots`) |
| Seguridad de memoria | ✅ Rust elimina de raíz ~70% de los CVE críticos de un navegador |
| **Política de mismo origen** | ✅ Aislamiento en cookies/almacenamiento y esquemas seguros |
| **CORS** | ✅ Real (`Access-Control-Allow-Origin`, preflight OPTIONS, credenciales) |
| **CSP** | ✅ Real (`default-src`, `script-src`, `style-src`, `img-src`, `connect-src`, `font-src`, `media-src`) |
| **Sandbox de proceso** | ❌ No existe |

Sin política de mismo origen ni sandbox, una página maliciosa no tiene
barreras. **Úsalo con sitios de confianza o en desarrollo, no como
navegador diario.**

### Plataforma

Compilado y probado **solo en Windows**. `winit`/`wgpu` soportan macOS y
Linux en teoría, pero nunca se ha verificado aquí. Sin versión móvil.

### Web moderna

Sin `<canvas>`, `<video>`, `<audio>`, `<iframe>`, `<svg>`. Sin WebGL,
IndexedDB, Service Workers, WebSockets ni Web Workers. Sin HTTP/2 ni caché
HTTP. El JavaScript es interpretado (sin JIT), así que es bastante más
lento que un navegador comercial.

### Compatibilidad

La métrica honesta de un motor de navegador es cuántos tests de
[Web Platform Tests](https://github.com/web-platform-tests/wpt) pasa. Este
motor **no ejecuta la suite oficial todavía**: los 21 tests estilo-WPT que
corre están escritos a mano. Hasta que ese número exista, cualquier
afirmación sobre "compatibilidad" —incluida la de este README— es una
impresión, no un dato.

---

## Cómo está montado

```text
navegador-ia/
├── engine/           Motor de renderizado en Rust (9 crates)
│   ├── crates/net/       HTTP/HTTPS, cookies, almacenamiento web
│   ├── crates/dom/       Parseo HTML y árbol DOM
│   ├── crates/css/       Parseo, selectores y cascada
│   ├── crates/layout/    Cajas: bloque, inline, flex, grid, tabla, float
│   ├── crates/text/      Medición y shaping de texto
│   ├── crates/image/     Decodificación de imágenes
│   ├── crates/js/        Runtime JavaScript y bindings del DOM
│   ├── crates/gfx/       Display list, rasterizado y ventana
│   └── crates/core/      Pipeline y servidor NDJSON (engine_server)
├── frontend/         Interfaz en React + Vite
├── desktop/          Envoltorio Electron
└── backend/          Servidor Python (FastAPI) — ver nota abajo
```

El motor corre como un proceso aparte (`engine_server`) que habla **NDJSON
por stdin/stdout**. Electron se comunica con él directamente por IPC.

> **Nota sobre el backend:** Electron se comunica directamente con el
> motor nativo de Rust (`engine_server`) vía IPC/NDJSON sin dependencias
> intermedias obligatorias. El backend de Python queda reservado como
> microservicio opcional para tareas avanzadas de IA.

`engine/ARCHITECTURE.md` documenta el estado real de cada capacidad, con
sus simplificaciones declaradas una por una. Es la fuente de verdad de este
proyecto; si algo de este README lo contradice, gana `ARCHITECTURE.md`.

---

## Compilar y ejecutar

### Requisitos
* **Rust** 1.75+ (`cargo`)
* **Node.js** 18+ y `npm`

### Desarrollo

```bash
npm install
npm run start          # frontend (Vite) + aplicación Electron
```

### Solo el motor

```bash
cd engine
cargo test --workspace          # los 676 tests
cargo run -p engine-core --bin engine_server   # servidor NDJSON por stdin/stdout
```

### Instalador

```bash
npm run build:app
```

Genera `Navegador IA Setup.exe` en la raíz. **Sin firmar**: Windows
SmartScreen mostrará un aviso a quien lo descargue. Ver
`desktop/DISTRIBUCION.md` para las opciones de firma de código.

---

## Sobre la IA

El proyecto se llama Navegador IA porque integra un agente que navega por ti.
**El agente autónomo está conectado a la interfaz** mediante el panel lateral
*Copiloto IA* (accesible con el botón 🤖 en la barra de navegación), soportando
tanto modo de simulación rápida como ejecución real impulsada por Gemini 2.0 Flash
mediante API Key.

---

## Licencia

MIT — ver [LICENSE](LICENSE).
