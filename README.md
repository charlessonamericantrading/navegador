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
| **Motor** | 20.719 líneas de Rust, 9 crates |
| **Tests** | 558 pasando, 0 fallando |
| **Red** | HTTP/1.1 + HTTPS real (`hyper` + `rustls`), redirecciones, gzip/deflate/brotli, cookies RFC 6265 |
| **HTML** | Parseo con `html5ever` (el de Servo), DOM mutable |
| **CSS** | Cascada con especificidad real, selectores con combinadores (`selectors`, el de Firefox), pseudo-clases, `@media` |
| **Layout** | Bloque, inline, flexbox, CSS Grid, tablas, `float`, `position` + `z-index` |
| **JavaScript** | Motor `boa`, DOM bindings, eventos con burbujeo, `fetch`, `XMLHttpRequest`, `setTimeout`, `localStorage` |
| **Pintado** | Rasterizado con `tiny-skia`, texto con glifos reales, imágenes, `border-radius`, sombras, `overflow: hidden` |

**Lo que se puede hacer con él ahora mismo:** cargar una página real por
HTTPS, navegar por enlaces, usar el historial, abrir pestañas, rellenar y
**enviar formularios** (GET y POST), e **iniciar sesión** en un sitio con
autenticación por cookies.

---

## Qué falta

Esto es la parte importante de este README, y está aquí arriba a propósito.

### Seguridad — antes de distribuirlo a nadie

| Pieza | Estado |
|---|---|
| Validación TLS de certificados | ✅ Real (`webpki-roots`) |
| Seguridad de memoria | ✅ Rust elimina de raíz ~70% de los CVE críticos de un navegador |
| **Política de mismo origen** | ❌ No existe |
| **Sandbox de proceso** | ❌ No existe |
| **CORS** | ❌ Es un stub que devuelve `true` |
| **CSP** | ❌ No existe |

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

> **Nota sobre el backend de Python:** hoy la aplicación arranca *dos*
> instancias del motor —una desde Electron y otra desde Python— y la
> interfaz solo usa la primera. Es deuda técnica conocida, pendiente de
> resolver: hay que elegir una de las dos rutas y borrar la otra.

`engine/ARCHITECTURE.md` documenta el estado real de cada capacidad, con
sus simplificaciones declaradas una por una. Es la fuente de verdad de este
proyecto; si algo de este README lo contradice, gana `ARCHITECTURE.md`.

---

## Compilar y ejecutar

### Requisitos
* **Rust** 1.75+ (`cargo`)
* **Node.js** 18+ y `npm`
* **Python** 3.11+ (solo si usas el backend)

### Desarrollo

```bash
npm install
npm run start          # frontend (Vite) + aplicación Electron
```

### Solo el motor

```bash
cd engine
cargo test --workspace          # los 558 tests
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

El proyecto se llama Navegador IA porque su objetivo es un agente que
navegue por ti. **Ese agente todavía no está conectado a la interfaz**:
existe el código de orquestación (en TypeScript y en Python) pero no hay
forma de usarlo desde la aplicación de escritorio. Es el siguiente trabajo
de producto pendiente.

---

## Licencia

MIT — ver [LICENSE](LICENSE).
