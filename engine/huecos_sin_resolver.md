# Huecos sin resolver

Backlog técnico vivo del motor. `ARCHITECTURE.md` lo referencia desde la Fase 40.

**Qué es esto.** `ARCHITECTURE.md` declara cada simplificación en el momento de la
fase que la introdujo. Muchas de esas declaraciones ya no son ciertas: la fase
siguiente las cerró y el texto histórico se quedó ahí (correctamente: es un
registro, no un estado). Este fichero es lo contrario — el estado **actual**, con
lo que sigue abierto y solo eso.

**Cómo se mantiene.** Al cerrar un hueco: se borra de aquí y se documenta la fase
en `ARCHITECTURE.md`. Al declarar una simplificación nueva en una fase: se añade
aquí. Los identificadores entre corchetes remiten a las tareas de `../plan.md`.

**Última verificación contra el código: 2026-09-09.** Cada entrada de este fichero
se comprobó con `grep` sobre `engine/crates/*/src` en esa fecha, no se copió de
`ARCHITECTURE.md`. Estado de la suite ese día: **805 tests pasando, 0 fallando**.

---

## 1. JavaScript: APIs ausentes

Ninguna de estas existe en `engine/crates/js/src`. Cada ausencia lanza un
`TypeError` que mata el script entero, así que el coste no es proporcional a lo
usada que sea la API. Verificado con `grep` una por una.

### 1.1 Utilidades de plataforma `[plan C5]`

| API | Notas |
|---|---|
| `URL` (constructor) | El crate `url` ya es dependencia de `net` |
| `URLSearchParams` | Idem |
| `TextEncoder` / `TextDecoder` | Solo UTF-8; otras codificaciones deben lanzar `RangeError` real |
| `atob` / `btoa` | Crate `base64` |
| `AbortController` / `AbortSignal` | Debe cancelar el `fetch` de verdad, no solo marcar |
| `structuredClone` | Bloquea IndexedDB y Workers, que lo necesitan para pasar datos |
| `crypto.getRandomValues` / `crypto.randomUUID` | |
| `performance.now` / `mark` / `measure` | |
| `console` como objeto global | Hoy existe `printEngineLog` como apaño. La salida debe ir a `tracing`, **nunca a stdout**: rompería el protocolo NDJSON (`engine_server.rs:15`) |

### 1.2 DOM: métodos y propiedades `[plan C6]`

`document`: `readyState`, `currentScript`, `write`/`writeln`, `activeElement`,
`hasFocus`, `createDocumentFragment`, `createComment`, `createRange`, `importNode`,
`adoptNode`, `getElementsByClassName`, `elementFromPoint` (el hit-testing ya existe
en `server.rs`, solo falta exponerlo a JS).

`Element`: `closest`, `matches` (el matcher real ya existe en el crate `css`, falta
el puente), `insertAdjacentHTML`/`Element`/`Text`, `innerText`, `outerHTML`,
`dataset`, `getAttributeNames`, `toggleAttribute`, `hasAttributes`, `replaceWith`,
`before`, `after`, `append`, `prepend`, `replaceChildren`, `children`,
`firstElementChild`/`lastElementChild`/`nextElementSibling`/`previousElementSibling`,
`childElementCount`, `scrollIntoView`, `focus`/`blur`, `scrollTop`/`Left`/`Width`/
`Height`, `offsetTop`/`Left`/`Width`/`Height`/`Parent`, `clientWidth`/`Height`.

`Node`: `contains`, `isConnected`, constantes `nodeType`, `nodeName`, `nodeValue`,
`cloneNode`, `isEqualNode`, `compareDocumentPosition`, `getRootNode`, `normalize`.

`NodeList`/`HTMLCollection` iterables (`forEach`, `Symbol.iterator`).

### 1.3 Cadena de prototipos `[plan C3]`

No existe. Cada nodo se expone como un objeto suelto con propiedades añadidas a
mano. Los bundles hacen `instanceof HTMLElement`, `Object.getPrototypeOf(el)` y
parchean `Element.prototype.foo`; nada de eso puede funcionar hoy.

Falta: `EventTarget` → `Node` → `Element` → `HTMLElement` → subclases concretas, más
`Document`, `Text`, `Comment`, `DocumentFragment`, registrados como globales para
que `instanceof` funcione.

### 1.4 Eventos `[plan C4]`

`new Event(tipo, {bubbles, cancelable})` **sí** existe (`dom_bindings.rs:663`,
probado en `tests/wpt-style/events-and-microtasks.html`). Faltan:

- `CustomEvent` (con `detail`) y `EventTarget` como constructor
- `KeyboardEvent`, `MouseEvent`, `InputEvent`, `FocusEvent` con sus campos (`key`,
  `code`, `clientX/Y`, `button`, `relatedTarget`, `inputType`). Es el mismo hueco
  que `ARCHITECTURE.md` declara en «Integración con el producto» como «metadatos de
  tecla todavía no están implementados».
- Sin verificar: `composedPath()`, `eventPhase`, `timeStamp`, `isTrusted`.

### 1.5 Observadores `[plan C8]`

`IntersectionObserver` y `ResizeObserver` no existen. **No se ponen como stub a
propósito**, por la misma razón que la Fase 39 no puso un stub de
`MutationObserver`: un observador que nunca dispara deja al código esperando para
siempre, que es peor que fallar rápido.

### 1.6 Carga de scripts `[plan C9]`

Los `<script src>` externos **sí** se descargan (`find_external_script_srcs` en
`pipeline.rs`). Lo que no existe:

- `type="module"` y la semántica de módulo (`import`/`export`, loader que resuelva
  especificadores contra la URL del documento)
- `import()` dinámico
- `defer` y `async` con su orden real
- `<script type="importmap">`, `nomodule`

**Este es el hueco que impide que cualquier bundle de Vite o Next se ejecute**,
aunque todas las APIs de los apartados anteriores existieran.

### 1.7 Orden de ejecución `[plan C10]`

Los scripts se ejecutan **todos seguidos después de parsear el documento entero**
(`scripting.rs:13`). Rompe `document.write`, `document.currentScript`, y cualquier
script que espere que el DOM «de abajo» aún no exista. Requiere pausar el
`TreeSink` de `html5ever` en `</script>`.

### 1.8 Otros

- `XMLHttpRequest` es **siempre síncrono** (`xhr.rs`), aunque acepte `async: true`.
  Cualquier test o código que dependa del orden asíncrono real falla.
- `responseType` no implementado: `response` es siempre texto.
- `window.open` con nombre de ventana o *features*: no implementado. Decisión: se
  mantiene sin implementar y debe lanzar error explícito `[plan C7]`.
- `Storage` no es un objeto exótico: solo funcionan los métodos (`getItem`/
  `setItem`), no el acceso por propiedad (`localStorage.foo`).
- Sin `bfcache`: volver atrás reconstruye el documento entero.
- Sin reflow síncrono: `getComputedStyle` devuelve valores especificados, no usados,
  y solo de las propiedades que la cascada resolvió de verdad.

---

## 2. CSS y layout

### 2.1 Pseudo-elementos `[plan D1]`

`::before` y `::after` **no existen**. `NoPseudoElement` es un enum vacío, así que
el parser rechaza el selector como error de sintaxis y **descarta la regla entera**.
Afecta a iconos de fuente, clearfix, comillas y viñetas personalizadas: mucho CSS
real. `::marker`, `::placeholder`, `::selection`, `::first-line`, `::first-letter`
tampoco.

### 2.2 Pseudo-clases de interacción que nunca coinciden

`:hover`, `:focus`, `:focus-visible`, `:focus-within`, `:active` y `:visited` se
**parsean** (para no perder la regla entera) pero **nunca coinciden**: el motor no
recalcula la cascada al mover el ratón ni al enfocar. Cerrarlo requiere invalidación
de estilo por evento, no solo añadirlas al matcher.

`:visited` además nunca debe coincidir, por privacidad. Eso no es un hueco.

Las estructurales (`:first-child`, `:last-child`, `:nth-child`, `:empty`, `:root`,
`:not`, `:is`, `:where`) **sí** funcionan: el crate `selectors` las resuelve y el
adaptador `element.rs` implementa los métodos que necesitan. `:has()` sin verificar.

### 2.3 Imágenes de fondo `[plan D2, D3]`

`background-image` solo acepta `url()`. `linear-gradient()` y `radial-gradient()`
devuelven `None` explícitamente (`display_list.rs:1612` lo prueba). Tampoco hay
`background-position`, `background-size` con valores explícitos,
`background-repeat` distinto de `repeat`, ni capas múltiples.

### 2.4 `calc()` `[plan D4]`

Sin evaluar. Un valor con paréntesis se deja pasar sin trocear, así que el longhand
no se genera y **el layout resuelve a cero**. `min()`, `max()`, `clamp()` igual.
Cerrarlo requiere guardar el AST en `computed_style` y evaluarlo en `layout`, cuando
ya se conoce el *containing block* — no en el crate `css`.

### 2.5 Sin implementar `[plan D5, D6, D7, D12]`

`transform` (y su efecto en el hit-testing), `transition`, `@keyframes`/`animation`,
`filter`, `backdrop-filter`, `clip-path`, `mix-blend-mode`, `columns`,
`writing-mode`, `@layer`, `@container`, `@property`.

`z-index` con contextos de apilamiento reales: sin verificar, probable que la
display list se ordene solo por orden de árbol `[plan D7]`.

### 2.6 Color

`hsl()`/`hsla()` sí (Fase 28). Faltan `hwb()`, `lab()`, `lch()`, `oklch()`,
`color()` y `color-mix()`.

### 2.7 Tipografía `[plan D9]`

`@font-face` con descarga de fuentes: no existe. `font-family` se parsea y llega al
layout, pero conviene verificar si `text/` respeta la familia pedida o pinta siempre
con la misma. Faltan `letter-spacing`, `word-spacing`, `text-transform`,
`white-space` en sus variantes, `text-overflow: ellipsis`, `word-break`,
`overflow-wrap`.

### 2.8 Fondo del lienzo

Cuando `<body>` propaga su fondo al lienzo, sigue pintando el suyo además. Como es
el mismo color el resultado visible es idéntico; solo se notaría con fondos
semitransparentes superpuestos. Cosmético, prioridad baja.

### 2.9 Rendimiento del layout `[plan D13]`

Cualquier mutación del DOM desde JS re-hace el layout completo. Con un framework
re-renderizando en cada pulsación es inutilizable. Falta invalidación por subárbol.

---

## 3. Red

- **Sin caché HTTP** de ningún tipo: cada navegación vuelve a descargar todos los
  subrecursos `[plan E1]`
- **Sin HTTP/2** `[plan E2]`
- **Sin `@import`** dentro de una hoja de estilos `[plan E3]`
- Sin caché de preflight CORS: cada petición no simple repite el `OPTIONS`
- Sin caché de hojas de estilo entre navegaciones, ni precarga especulativa antes de
  que termine el parseo
- Sin `zstd` como `Content-Encoding` (decisión consciente: aún no es de uso general)
- Sin detección de ciclos de redirección antes del límite fijo de 20 saltos
- Sin streaming: el cuerpo se descarga entero antes de parsear `[plan E5]`
- `Referrer-Policy`, HSTS y proxy del sistema sin verificar `[plan E6]`

---

## 4. Seguridad

- **Sin sandbox de proceso** `[plan F2, F3]`. Es el único ❌ de la tabla del README.
  Existe `crates/core/src/sandbox.rs` de la Fase 37, pero hay que leerlo y
  documentar qué hace realmente antes de ampliarlo; `sandbox.rs:58` contempla
  «plataforma sin soporte».
- **Todas las pestañas en un proceso** (`server.rs:147`): un pánico las mata todas.
- `tab_id` adivinable: se asigna por posición (`server.rs:222`) `[plan F5]`.
- Sin aislamiento de sitios; sin fuzzing de los parsers propios.
- Sin `cargo audit`/`cargo deny` en CI `[plan F7]`.

---

## 5. Plataforma web ausente `[plan G]`

`<iframe>`, `<video>`, `<audio>`, WebGL, WebGPU, IndexedDB, WebSockets, Web Workers,
Service Workers, `Blob`/`File`/`FileReader`, `ReadableStream`, descargas,
`<dialog>`, `<details>`, `<progress>`, `<meter>`, `<input type=date|color|range|file>`,
impresión.

Namespaces foráneos (SVG/MathML) sin namespace correcto en el DOM: afecta a
`createElementNS` y `el.namespaceURI`, que algunas librerías de gráficos consultan.
`<template>` no usa un `DocumentFragment` inerte separado. El doctype se ignora.

APIs de dispositivo (`Notification`, `Geolocation`, `getUserMedia`, `Bluetooth`,
`USB`, `Battery`, `Vibration`, `Gamepad`): ausentes. Decisión: deben denegar
explícitamente, nunca fingir éxito.

---

## 6. Producto y protocolo

- `EngineRequest::SubmitForm` no implementado: devuelve error explícito `[plan C11]`
- Selección de texto no implementada `[plan C12]`
- Sin favicon en `get_state` `[plan E4, I5]`
- Sin evento de progreso de carga `[plan I5]`
- Sin diálogos (`alert`/`confirm`/`prompt`) enrutados a la UI `[plan C7]`
- Sin canal de `console` hacia la UI `[plan C5, I5]`
- La captura se regenera entera y viaja en Base64 en cada `get_state`, incluido cada
  scroll `[plan K4]`

---

## 7. Compatibilidad

No se ejecuta la suite oficial de Web Platform Tests `[plan H]`. Los 4 ficheros de
`engine/tests/wpt-style/` están escritos a mano. Del arnés `testharness.js` solo
existen `test`, `assert_equals`, `assert_true` y `assert_false`; faltan `async_test`,
`promise_test`, `assert_throws_js`, `assert_array_equals` y
`add_completion_callback`, entre otros.

`wpt_runner` no tiene timeout por test: un test con bucle infinito cuelga el runner.

Hasta que exista una cifra de WPT, cualquier afirmación sobre compatibilidad —
incluida la del README — es una impresión, no un dato.

---

## 8. Plataforma de compilación `[plan J]`

Compilado y probado **solo en Windows**. `winit`/`wgpu` soportan macOS y Linux en
teoría, pero nunca se ha verificado. Sin versión móvil (decisión: fuera de alcance).
