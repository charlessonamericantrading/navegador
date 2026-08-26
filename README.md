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
2026-08-27.

| | |
|---|---|
| **Motor** | ~25.800 líneas de Rust, 10 crates |
| **Tests** | 703 pasando, 0 fallando |
| **Red** | HTTP/1.1 + HTTPS real (`hyper` + `rustls`), redirecciones, gzip/deflate/brotli, cookies RFC 6265, CORS y CSP |
| **HTML** | Parseo con `html5ever` (el de Servo), DOM mutable, `<canvas>` 2D context |
| **CSS** | Cascada con especificidad real, selectores con combinadores (`selectors`, el de Firefox), pseudo-clases, `@media`, `rem`, porcentajes, shorthands (`padding`/`margin` de 1-4 valores, `flex`) |
| **Layout** | Bloque, inline, `box-sizing: border-box`, flexbox con *intrinsic sizing*, CSS Grid, tablas, `float`, `position` (sticky/relative/absolute) |
| **JavaScript** | Motor `boa` (sintaxis ES moderna completa: `async`/`await`, clases, spread, `Map`/`Set`, `Promise`), DOM bindings, eventos con burbujeo, `fetch`, `XMLHttpRequest`, `setTimeout`, `requestAnimationFrame`, `localStorage`, `navigator`, Canvas 2D API |
| **Pintado** | Rasterizado con `tiny-skia`, gráficos vectoriales SVG con `resvg`/`usvg`, fuentes con glifos reales, `border-radius`, sombras |
| **IA Nativa** | Crate `engine-ai` con Árbol de Accesibilidad Semántico (AOM) con coordenadas espaciales optimizado para LLMs |

**Lo que se puede hacer con él ahora mismo:** cargar una página real por
HTTPS, navegar por enlaces, usar el historial, abrir pestañas, rellenar y
**enviar formularios** (GET y POST), **iniciar sesión** en un sitio con
autenticación por cookies, e **interactuar con el Agente Copiloto IA**
autónomo desde la barra lateral.

**Y lo que NO, dicho aquí y no enterrado abajo:** las webs que construyen su
contenido con JavaScript en el cliente (React, Next, Vue, Shopify — es decir,
la mayoría de tiendas y aplicaciones web de hoy) **se ven vacías**. El motor
descarga la página correctamente, pero esa página no trae contenido: lo
genera un bundle que este motor todavía no ejecuta. Desde 2026-08-27 el
navegador lo **dice con un aviso claro** en vez de dejar la pantalla en
blanco, pero decirlo no es arreglarlo. Las webs que envían su contenido ya
hecho en el HTML (Google, Wikipedia, prensa, documentación) sí se ven.

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

**Lo que falta de verdad y define el techo:** no se ejecutan los *bundles*
de las aplicaciones web modernas. No es que falte un motor de JavaScript —
`boa` cubre la sintaxis ES moderna entera, medido con una sonda de 28 APIs
que el motor pasa 26 — sino que faltan piezas del DOM/BOM sin las cuales el
bundle muere en su primera línea. Hoy faltan `location.href` y
`MutationObserver`; la ausencia de *una sola* API lanza un `TypeError` que
se lleva por delante el script completo.

Sin `<video>`, `<audio>`, `<iframe>`. Sin WebGL, IndexedDB, Service Workers,
WebSockets ni Web Workers. Sin HTTP/2 ni caché HTTP. El JavaScript es
interpretado (sin JIT), así que es bastante más lento que un navegador
comercial.

`<canvas>` 2D y `<svg>` **sí** están soportados desde las Fases 37-38 (ver
`engine/ARCHITECTURE.md`).

### Rendimiento

Medido el 2026-08-27 sobre páginas reales y sintéticas. Una página grande
con mucho CSS (13.000 nodos contra 2.000 reglas) tardaba **69,7 s** y ahora
tarda **1,8 s**; el artículo "España" de Wikipedia costaba **38,9 s** de CPU
y ahora **4,0 s**. Las tres causas —selectores reparseados en cada
comparación, ausencia de prefiltro por selector clave, y subrecursos
descargados en serie— están documentadas en `ARCHITECTURE.md` (Fase 39).

Sigue sin haber caché HTTP ni HTTP/2, así que cada navegación vuelve a
descargar todos los subrecursos.

### Compatibilidad

La métrica honesta de un motor de navegador es cuántos tests de
[Web Platform Tests](https://github.com/web-platform-tests/wpt) pasa. Este
motor **no ejecuta la suite oficial todavía**: los 24 tests estilo-WPT que
corre están escritos a mano. Hasta que ese número exista, cualquier
afirmación sobre "compatibilidad" —incluida la de este README— es una
impresión, no un dato.

---

## Cómo está montado

```text
navegador-ia/
├── engine/           Motor de renderizado en Rust (10 crates)
│   ├── crates/net/       HTTP/HTTPS, cookies, almacenamiento web
│   ├── crates/dom/       Parseo HTML y árbol DOM
│   ├── crates/css/       Parseo, selectores y cascada
│   ├── crates/layout/    Cajas: bloque, inline, flex, grid, tabla, float
│   ├── crates/text/      Medición y shaping de texto
│   ├── crates/image/     Decodificación de imágenes
│   ├── crates/js/        Runtime JavaScript y bindings del DOM
│   ├── crates/gfx/       Display list, rasterizado y ventana
│   ├── crates/ai/        Árbol de accesibilidad (AOM) para el agente IA
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
cargo test --workspace          # los 703 tests
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
