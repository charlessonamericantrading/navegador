# Navegador IA

Un navegador de escritorio con un **motor de renderizado propio escrito desde cero en Rust** — sin Chromium, sin WebKit, sin Gecko.

> **Estado: en desarrollo, no apto para uso general todavía.**
> El motor renderiza páginas reales por HTTPS, pero no tiene aislamiento de
> procesos: todas las pestañas viven en el mismo proceso, sin sandbox del
> sistema operativo. Ver [Qué falta](#qué-falta) antes de usarlo con sitios
> en los que no confíes.
>
> El plan de trabajo completo, con las tareas pendientes priorizadas, está
> en [`plan.md`](plan.md). El backlog técnico del motor, verificado contra
> el código, está en [`engine/huecos_sin_resolver.md`](engine/huecos_sin_resolver.md).

---

## Qué hace de verdad hoy

Todo lo de esta lista está implementado y verificado ejecutando el motor, no
solo compilándolo. Las cifras salen de correr la suite de tests el
2026-08-27.

| | |
|---|---|
| **Motor** | ~25.800 líneas de Rust, 10 crates |
| **Tests** | 841 pasando, 0 fallando (medido el 2026-09-09) |
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

Las defensas de origen (cookies aisladas, CORS, CSP, esquemas seguros) son
reales y están probadas. Lo que no existe es la capa de abajo: **el motor
corre en un único proceso sin sandbox del sistema operativo**, y todas las
pestañas lo comparten, así que un pánico en una tumba el navegador entero.
Rust elimina de raíz la corrupción de memoria, pero no un fallo lógico en el
intérprete de JavaScript ni en un decodificador de imágenes ante un fichero
malicioso. **Úsalo con sitios de confianza o en desarrollo, no como
navegador diario.**

### Plataforma

Compilado y probado **solo en Windows**. `winit`/`wgpu` soportan macOS y
Linux en teoría, pero nunca se ha verificado aquí. Sin versión móvil.

### Web moderna

**Lo que falta de verdad y define el techo:** no se ejecutan los *bundles*
de las aplicaciones web modernas. No es que falte un motor de JavaScript —
`boa` cubre la sintaxis ES moderna entera — sino que faltan piezas del
DOM/BOM sin las cuales el bundle muere en su primera línea. La ausencia de
*una sola* API lanza un `TypeError` que se lleva por delante el script
completo, así que el coste de una API ausente no es proporcional a lo usada
que sea.

Lo que bloquea hoy, comprobado contra el código el 2026-09-09:

* **Sin módulos ES.** No hay `<script type="module">`, ni `import()`
  dinámico, ni `defer`/`async` con su orden real. Todo bundle de Vite, Next
  o Svelte se sirve como módulo, así que **ninguno puede arrancar** por más
  APIs que se añadan.
* **Sin cadena de prototipos DOM.** Cada nodo es un objeto suelto, así que
  `instanceof HTMLElement` es falso y parchear `Element.prototype` no hace
  nada. Los bundles hacen ambas cosas al arrancar.
* **`window` no es el objeto global.** En un navegador `window === globalThis`,
  así que `addEventListener(...)` a secas funciona. Aquí no: lanza
  `ReferenceError` y mata el script.
* **APIs ausentes:** `AbortController`, `structuredClone`, `crypto`,
  `matchMedia`, `IntersectionObserver`, `ResizeObserver`, `customElements`,
  `CustomEvent`, `DOMParser`, `closest`, `matches`, `dataset`, `innerText`,
  `insertAdjacentHTML`, `document.readyState`, `FormData`, `Blob`.

Hay una sonda que mide esto, no es una impresión: **56 de 114** el 2026-09-09.
Se ejecuta con la suite y un test impide que el número baje.

```bash
cargo test -p engine-core --test api_probe -- --nocapture
```

`location` y `MutationObserver` **sí** existen desde la Fase 39, y `console`,
`URL`, `URLSearchParams`, `performance`, `atob`/`btoa` y `TextEncoder` desde la
Fase 42. La lista completa y actualizada está en
[`engine/huecos_sin_resolver.md`](engine/huecos_sin_resolver.md).

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
corre están escritos a mano y pasan los 24. Hasta que ese número exista,
cualquier afirmación sobre "compatibilidad" —incluida la de este README— es
una impresión, no un dato.

```bash
cargo run -p engine-core --bin wpt_runner -- tests/wpt-style
```

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
cargo test --workspace          # los 841 tests
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
*Copiloto IA* (accesible con el botón 🤖 en la barra de navegación), con un modo
de simulación rápida y un modo real impulsado por Gemini mediante API Key.

El motor incluye además el crate `engine-ai`, que expone un Árbol de
Accesibilidad (AOM) con roles y coordenadas reales, pensado para que un modelo
de lenguaje entienda la página gastando ~80% menos tokens que enviándole el HTML
crudo.

**Limitaciones actuales del agente, dichas aquí y no enterradas:**

* La clave de Gemini se guarda en el `localStorage` del renderer y las llamadas
  al modelo salen desde ahí. Debe moverse al proceso principal de Electron con
  `safeStorage` (cifrado del sistema operativo).
* Solo Gemini. No hay opción de modelo local ni de otros proveedores.
* El agente recibe **texto plano** de la página, no el AOM, pese a que el AOM ya
  existe y el protocolo ya lo expone. Conectarlos es tarea pendiente.
* Hay dos implementaciones del agente, una en TypeScript y otra en Python, que
  hacen lo mismo.

Las cuatro están planificadas en [`plan.md`](plan.md), bloque I.

---

## Licencia

MIT — ver [LICENSE](LICENSE).
