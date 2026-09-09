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

**Última verificación contra el código: 2026-09-09** (actualizado tras la Fase 42). Cada entrada de este fichero
se comprobó con `grep` sobre `engine/crates/*/src` en esa fecha, no se copió de
`ARCHITECTURE.md`. Estado de la suite: **874 tests pasando, 0 fallando**, más 60 tests estilo-WPT.

---

## 1. JavaScript: APIs ausentes

Ninguna de estas existe en `engine/crates/js/src`. Cada ausencia lanza un
`TypeError` que mata el script entero, así que el coste no es proporcional a lo
usada que sea la API. Verificado con `grep` una por una.

### 1.1 Utilidades de plataforma `[plan C5]` — parcialmente cerrado (Fase 42)

**Ya implementadas** en `js/src/platform.rs`: `console` (familia completa, salida
a `tracing`), `URL`, `URLSearchParams`, `performance.now`, `atob`/`btoa`,
`TextEncoder`/`TextDecoder`.

Lo que sigue faltando:

| API | Notas |
|---|---|
| `AbortController` / `AbortSignal` | **No se pone hasta que cancele el `fetch` de verdad.** Uno que solo marque una bandera es el stub que la doctrina prohíbe: el código cree haber cancelado y la petición sigue viva |
| `structuredClone` | Bloquea IndexedDB y Workers, que lo necesitan para pasar datos |
| `crypto.getRandomValues` / `crypto.randomUUID` | Necesita una fuente de aleatoriedad real (`getrandom`). Rellenarlo con números no criptográficos sería peor que la ausencia |
| `performance.getEntries` | `mark`/`measure` existen pero no registran. `getEntries` NO se registra a propósito: devolver una lista vacía fingiría que se midió |
| `Intl` | `boa` lo trae detrás de una *feature* que está desactivada |

Simplificación declarada de la Fase 42: `TextEncoder.encode` devuelve un **Array
normal, no un `Uint8Array`**. Se indexa y se recorre igual, que es lo que hace
casi todo el código; lo que no funcionará es pasárselo a algo que exija un
TypedArray de verdad.

### 1.2 DOM: métodos y propiedades `[plan C6]` — parcialmente cerrado (Fase 44)

**Ya funcionan**: `matches`, `closest`, `contains`, `remove`, `append`,
`prepend`, `cloneNode`, `dataset` (solo lectura), `innerHTML` (getter y setter,
con parseo real), `outerHTML` (solo getter), `isConnected`, `id`, `className`.

`document`: faltan `readyState`, `currentScript`, `write`/`writeln`,
`activeElement`, `hasFocus`, `createDocumentFragment`, `createComment`,
`createRange`, `importNode`, `adoptNode`, `getElementsByClassName`,
`elementFromPoint` (el hit-testing ya existe en `server.rs`, solo falta
exponerlo a JS).

`Element`: faltan `insertAdjacentHTML`/`Element`/`Text`, `innerText`,
`getAttributeNames`, `toggleAttribute`, `hasAttributes`, `replaceWith`,
`before`, `after`, `replaceChildren`, `childElementCount`, `scrollIntoView`,
`focus`/`blur`, `offsetTop`/`Left`/`Width`/`Height`/`Parent`,
`clientWidth`/`Height`.

`Node`: faltan `nodeName`, `nodeValue`, `isEqualNode`,
`compareDocumentPosition`, `getRootNode`, `normalize`.

`NodeList`/`HTMLCollection` iterables (`forEach`, `Symbol.iterator`).

Simplificaciones declaradas de la Fase 44:

- **`dataset` es una foto, no un proxy vivo.** Escribir en él no cambia el
  atributo. Leerlo, que es el uso mayoritario, funciona.
- **`outerHTML` no tiene setter.** Reemplazar el nodo dentro de su padre exige
  parseo de HTML *en contexto* (un `<td>` suelto se parsea distinto fuera de una
  tabla), que este motor todavía no hace desde JS. Un setter a medias produciría
  un árbol equivocado en silencio.
- **El serializador de `innerHTML`/`outerHTML` está escrito a mano**, no lo hace
  `html5ever`: el adaptador `TreeSink` no guarda lo necesario para una
  reserialización fiel (orden original de atributos, comillas, mayúsculas del
  fuente). Produce HTML equivalente, no un calco.

### 1.3 Cadena de prototipos `[plan C3]` — cerrada con una salvedad (Fase 44)

**Ya funciona**: `EventTarget` → `Node` → `Element` → `HTMLElement` → subclases
concretas, más `Document`, `Text`, `Comment`, `DocumentFragment`, registrados
como globales. `el instanceof HTMLElement` responde bien, `Node.ELEMENT_NODE`
existe, y `new HTMLDivElement()` lanza `Illegal constructor` como el spec.

**Lo que NO cierra**: los métodos de un elemento siguen viviendo en la propia
instancia, no en el prototipo, porque `build_element_object` los construye con
un *closure* que captura su nodo. Consecuencia exacta:

- Un método que el motor **ya tiene** (`appendChild`, `setAttribute`…) tapa al
  del prototipo. Un envoltorio del estilo `const orig = Element.prototype.appendChild;
  Element.prototype.appendChild = ...` se instala pero nunca se ejecuta.
- Un método que el motor **no tiene** sí se hereda, que es el caso de todo
  polyfill y el que más importa.

Cerrarlo exige que cada método recupere su nodo desde `this` en vez de desde una
captura, es decir reescribir las ~675 líneas de `build_element_object`.

### 1.4 Eventos `[plan C4]` — cerrado (Fase 45)

**Ya existen**: `Event`, `CustomEvent` (con `detail`), `KeyboardEvent`,
`MouseEvent`, `InputEvent`, `FocusEvent`, con sus campos y modificadores.

Falta: `EventTarget` como constructor construible, `composedPath()`,
`eventPhase`, `timeStamp`, `isTrusted`.

Simplificación declarada: `MouseEvent.pageX`/`pageY` se igualan a
`clientX`/`clientY`, porque el constructor no tiene acceso al scroll. Correcto
mientras la página no esté desplazada.

El teclado y el ratón REALES siguen sin rellenar estos tipos: el motor despacha
sus eventos sin metadatos. Eso es del lado de `core::server`, no del binding.

### 1.5 Observadores `[plan C8]`

`IntersectionObserver` y `ResizeObserver` no existen. **No se ponen como stub a
propósito**, por la misma razón que la Fase 39 no puso un stub de
`MutationObserver`: un observador que nunca dispara deja al código esperando para
siempre, que es peor que fallar rápido.

### 1.6 Carga de scripts `[plan C9]` — mayoritariamente cerrado (Fase 43)

**Ya funciona**: `<script type="module">` (en línea y externo) con `import`/
`export` reales, `defer`/`async` con el orden del spec, `nomodule` omitido, los
`type` de datos (`application/json`) sin ejecutarse, y los fragmentos declarados
con `<link rel="modulepreload">` descargados y disponibles para los `import`.
Probado de punta a punta con la estructura que emite un bundler
(`tests/bundle_modulos.rs`).

Lo que sigue faltando:

| Pieza | Notas |
|---|---|
| Resolución relativa al módulo importador | Los especificadores se resuelven contra la URL de la **página**, no contra la del módulo que importa. Los bundlers emiten rutas absolutas (`/assets/x.js`), que salen igual; falla un `./vecino.js` entre módulos que no estén en el directorio del documento. Boa 0.19 no expone dónde guardar la URL de cada módulo (`host_defined` es inmutable, `path` es de disco) |
| `import()` dinámico | Sin implementar |
| `<script type="importmap">` | Sin él, un especificador desnudo (`import x from "react"`) se rechaza con su motivo, en vez de inventar una URL |
| Descarga de módulos en caliente | El motor no va a la red durante la evaluación. Un `import` a algo que no se descubrió antes falla con un mensaje que lo dice. Un módulo importado dinámicamente o con una ruta calculada no se descubre |
| `async` real | Se ejecuta al final y en orden de documento, no «en cuanto llega»: todo está descargado antes de evaluar nada, así que no hay un «cuando llegue» que respetar |

### 1.7 Orden de ejecución `[plan C10]`

Los scripts se ejecutan **todos seguidos después de parsear el documento entero**
(`scripting.rs:13`). Rompe `document.write`, `document.currentScript`, y cualquier
script que espere que el DOM «de abajo» aún no exista. Requiere pausar el
`TreeSink` de `html5ever` en `</script>`.

### 1.8 `window` no es el objeto global

Medido con la sonda el 2026-09-09. En un navegador `window === globalThis`, así
que `addEventListener(...)` a secas y `window.addEventListener(...)` son lo
mismo, igual que `innerWidth` y `window.innerWidth`.

Aquí `window` es un objeto normal registrado como una propiedad global más, así
que **la forma corta lanza `ReferenceError`** y se lleva por delante el script
entero. Muchísimo código real la usa.

Consecuencia práctica que ya obliga a duplicar trabajo: cada global que además
deba verse en `window.*` hay que ponerlo en los dos sitios a mano (ver
`colgar_de_window` en `platform.rs`).

Cerrarlo exige que el objeto global de `boa` sea un proxy con semántica de
`WindowProxy`. Está declarado desde la Fase 6.4 en la cabecera de `window.rs`;
lo que aporta esta entrada es la medida de cuánto cuesta.

### 1.9 Otros

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
- **`boa_engine` 0.19 arrastra `fast-float` 0.2.0, con dos avisos de seguridad
  REALES** (no de mantenimiento): `RUSTSEC-2025-0003`, un fallo de segmentación
  por falta de comprobación de límites, y `RUSTSEC-2024-0379`, varios problemas
  de *soundness*. Lo que los alcanza es el parser de números del motor de
  JavaScript, es decir código de cualquier página.

  Cerrarlo exige subir boa de 0.19 a 0.22: tres versiones menores de un crate
  pre-1.0, con cambios de API en el cargador de módulos y en las firmas de
  `NativeFunction`. Es una tarea propia `[plan F8]`, no un cambio de una línea.

  El CI los tiene listados como conocidos y **bloquea cualquier aviso nuevo**;
  no están silenciados sin más.
- Sin mantenimiento, sin vulnerabilidad conocida: `ttf-parser`
  (`RUSTSEC-2026-0192`), `rustybuzz` (`RUSTSEC-2026-0206`) y `paste`
  (`RUSTSEC-2024-0436`). Las dos primeras son las piezas de tipografía del
  motor y no hay sustituto equivalente hoy; `paste` es una macro que no llega
  al binario.
- `cargo deny` y `npm audit` siguen sin estar en CI `[plan F7]`.

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
