# Navegador IA

> **Reglas del proyecto: [`AGENTS.md`](AGENTS.md).** La aplicación es 100 %
> Rust. Las carpetas `desktop/`, `frontend/` y `backend/` son de una etapa
> anterior que incumple esas reglas y se retiran (Fase 5 de `PLAN_SONNET.md`);
> lo que este README cuenta de ellas es historia, no el camino a seguir.

Un navegador de escritorio con un **motor de renderizado propio escrito desde cero en Rust** — sin Chromium, sin WebKit, sin Gecko.

> **Estado: en desarrollo, no apto para uso general todavía.**
> El motor renderiza páginas reales por HTTPS, pero no tiene aislamiento de
> procesos: todas las pestañas viven en el mismo proceso, sin sandbox del
> sistema operativo. Ver [Qué falta](#qué-falta) antes de usarlo con sitios
> en los que no confíes.
>
> **Para saber qué funciona hoy y cómo medirlo, empieza por
> [`engine/ESTADO.md`](engine/ESTADO.md)** (corto y verificado). El plan de
> trabajo está en [`plan.md`](plan.md) y [`PLAN_SONNET.md`](PLAN_SONNET.md); el
> backlog técnico, en [`engine/huecos_sin_resolver.md`](engine/huecos_sin_resolver.md).

---

## Qué hace de verdad hoy

Todo lo de esta lista está implementado y verificado ejecutando el motor, no
solo compilándolo. Las cifras salen de correr la suite de tests el
2026-11-23 (Linux).

| | |
|---|---|
| **Motor** | ~25.800 líneas de Rust, 10 crates |
| **Tests** | 1.052 pasando, 0 fallando (medido el 2026-11-23), más 60 tests estilo-WPT |
| **Red** | HTTP/1.1 + HTTPS real (`hyper` + `rustls`), redirecciones, gzip/deflate/brotli, cookies RFC 6265, CORS y CSP |
| **HTML** | Parseo con `html5ever` (el de Servo), DOM mutable, `<canvas>` 2D context |
| **CSS** | Cascada con especificidad real y orden de documento, selectores con combinadores (`selectors`, el de Firefox), pseudo-clases, `@media`, variables, `calc()`, `rem`/`em`/`vw`, porcentajes, shorthands, degradados `linear-gradient`, atributos HTML clásicos (`bgcolor`, `width`, `cellpadding`…) |
| **Layout** | Bloque, texto en línea que se parte por palabras entre elementos, `inline-block` con tamaño, `margin: auto`, `box-sizing: border-box`, flexbox con *intrinsic sizing*, CSS Grid, tablas, `float`, `position` (sticky/relative/absolute) |
| **JavaScript** | Motor `boa` (sintaxis ES moderna completa: `async`/`await`, clases, spread, `Map`/`Set`, `Promise`), DOM bindings, eventos con burbujeo, `fetch`, `XMLHttpRequest`, `setTimeout`, `requestAnimationFrame`, `localStorage`, `navigator`, Canvas 2D API |
| **Pintado** | Rasterizado con `tiny-skia`, gráficos vectoriales SVG con `resvg`/`usvg`, fuentes del sistema con glifos reales (sans-serif, serifa y monoespaciada), `border-radius`, sombras, degradados, lienzo blanco |
| **IA Nativa** | Crate `engine-ai` con Árbol de Accesibilidad Semántico (AOM) con coordenadas espaciales optimizado para LLMs |

**Lo que se puede hacer con él ahora mismo:** cargar una página real por
HTTPS, navegar por enlaces, usar el historial, abrir pestañas, **escribir con
el teclado en los campos** (con anillo de foco y cursor) y **enviar
formularios** (GET y POST), **iniciar sesión** en un sitio con autenticación
por cookies, detener una carga que se cuelga, e **interactuar con el Agente
Copiloto IA** autónomo desde la barra lateral. Hacker News y Wikipedia se ven
como en un navegador (barra naranja, ancho centrado, párrafos con enlaces en
su sitio).

**Y lo que NO, dicho aquí y no enterrado abajo:** las webs que construyen su
contenido con JavaScript en el cliente (React, Next, Vue, Shopify) siguen sin
verse en general, pero el motivo ya no es el que era. Hasta la Fase 43 el
bloqueo era estructural: un `<script type="module">` ni siquiera parseaba, así
que **ningún** bundle podía arrancar. Eso está resuelto y hay un test que
carga la estructura que emite un empaquetador y comprueba que se ejecuta y
pinta.

Lo que queda ahora es superficie de plataforma: faltan APIs del DOM que un
framework toca al arrancar, y la ausencia de una sola lanza un `TypeError` que
mata el script entero. Es trabajo incremental y medible, no un muro. Cuando la
página se queda vacía, el navegador lo **dice con un aviso claro** en vez de
dejar la pantalla en blanco. Las webs que envían su contenido ya hecho en el
HTML (Google, Wikipedia, prensa, documentación) se ven desde hace tiempo.

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
| **Dependencias sin avisos** | ❌ Dos fallos reales en el parser de números de `boa` |

Las defensas de origen (cookies aisladas, CORS, CSP, esquemas seguros) son
reales y están probadas. Lo que no existe es la capa de abajo: **el motor
corre en un único proceso sin sandbox del sistema operativo**, y todas las
pestañas lo comparten, así que un pánico en una tumba el navegador entero.
Rust elimina de raíz la corrupción de memoria, pero no un fallo lógico en el
intérprete de JavaScript ni en un decodificador de imágenes ante un fichero
malicioso.

La auditoría de dependencias encuentra además **dos fallos reales**, no de
mantenimiento, en `fast-float`: un fallo de segmentación por falta de
comprobación de límites y varios problemas de *soundness*. Llegan a través de
`boa`, el motor de JavaScript, así que lo que los alcanza es código de
cualquier página. Cerrarlos exige subir `boa` de 0.19 a 0.22, una migración de
API que está planificada. El CI los tiene listados como conocidos y bloquea
cualquier aviso nuevo.

**Úsalo con sitios de confianza o en desarrollo, no como navegador diario.**

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

* **`window` no es el objeto global.** En un navegador `window === globalThis`,
  así que `addEventListener(...)` a secas funciona. Aquí no: lanza
  `ReferenceError` y mata el script.
* **APIs ausentes:** `AbortController`, `structuredClone`, `crypto`,
  `IntersectionObserver`, `ResizeObserver`, `customElements`, `DOMParser`,
  `innerText`, `insertAdjacentHTML`, `FormData`, `Blob`, `WebSocket`,
  `indexedDB`.

**Los módulos ES ya funcionan desde la Fase 43**, que era el bloqueo anterior a
todo lo demás: `<script type="module">` con `import`/`export` reales,
`defer`/`async` en su orden, y los fragmentos de `<link rel="modulepreload">`
disponibles para los `import`. Un bundle con la estructura que emite un
empaquetador se ejecuta y pinta su contenido, verificado de punta a punta en
`engine/crates/core/tests/bundle_modulos.rs`. Lo que sigue faltando de ahí es
`import()` dinámico y los *import maps*.

**La jerarquía de clases del DOM funciona desde la Fase 44**: `instanceof
HTMLElement` responde bien y un polyfill instalado en `Element.prototype` lo
ven los elementos ya creados, que son los dos patrones que un framework ejecuta
al arrancar. Con ella llegaron `matches`, `closest`, `contains`, `remove`,
`append`, `prepend`, `cloneNode`, `dataset`, `id`, `className`, `outerHTML` y
un `innerHTML` de verdad. La Fase 49 añadió la interfaz `Node` (`parentNode`,
`childNodes`, `firstChild`, `nextSibling`, `nodeType`...), que faltaba entera.

Hay una sonda que mide lo demás, no es una impresión: **86 de 114** el
2026-09-23.
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

Medido el 2026-11-23 en Linux (motor en release, 1280×800). **Que nada se
cuelgue** es lo que está garantizado: un bucle infinito de JavaScript responde
en 0,2 s, un servidor que no contesta se abandona (10 s en total para los
subrecursos, 4 s para las imágenes) y una carga se puede detener o sustituir.

| Página | Carga | Scroll | Clic |
|---|---|---|---|
| Hacker News | 1,0–1,3 s | 0,05 s | 0,05 s |
| Google | 1,3–1,9 s | 0,02 s | 0,01 s |
| Wikipedia (el motor, sin red) | ~0,55 s | 0,03–0,05 s | 0,04 s |

La carga completa de Wikipedia **no se puede medir desde la nube** (las
imágenes de otros dominios no salen a internet); en un PC con internet, el
objetivo es menos de 3 s: `python3 scripts/dev/bench_all.py --strict`. Sigue sin
haber caché HTTP ni HTTP/2, así que cada navegación vuelve a descargar todos los
subrecursos. Redimensionar la ventana en una página grande tarda 0,4–0,6 s
(rehace la cascada entera).

### Compatibilidad

La métrica honesta de un motor de navegador es cuántos tests de
[Web Platform Tests](https://github.com/web-platform-tests/wpt) pasa. Este
motor **no ejecuta la suite oficial todavía**: los 60 tests estilo-WPT que
corre están escritos a mano y pasan los 60. Hasta que ese número exista,
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
por stdin/stdout**. Electron se comunica con él directamente por IPC. La red,
las cookies y el almacenamiento los tiene otro proceso, `engine_broker`, que
Electron arranca primero; el motor se los pide por un canal local (ADR 0001,
etapa 2). Sin broker, la aplicación no arranca el motor.

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
* **Rust** (`cargo`). Probado con 1.98; la versión mínima aún no está verificada.
* **Node.js** 22.12+ y `npm` (Electron 44 lo exige). Probado con Node 24.
* **Python no hace falta.** Solo lo usa el backend FastAPI opcional
  (`npm run install:backend`).

### Desarrollo

```bash
npm install
npm run install:all    # interfaz, Electron y compilación del motor (release)
npm run start          # frontend (Vite) + aplicación Electron
```

`install:all` compila el motor porque la aplicación lo busca en
`engine/target/release` (o `debug`) al arrancar: sin él, la ventana abre pero
no hay navegador. Para recompilarlo tras un cambio: `npm run build:engine`.
En Windows, `instalar.bat` hace estos pasos y comprueba los requisitos.

### Solo el motor

```bash
cd engine
cargo test --workspace          # los 874 tests
cargo run -p engine-core --bin engine_server   # servidor NDJSON por stdin/stdout (red propia)
cargo run -p engine-core --bin engine_broker   # broker: anuncia su canal y reparte tokens
```

### Instalador

```bash
npm run build:app          # interfaz + motor + Electron
npm run build:app:python   # además, el backend FastAPI opcional (necesita su .venv)
```

Genera `Navegador IA Setup <versión>.exe` en la raíz (y en `desktop/dist`).
**Sin firmar**: Windows
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

* La clave de Gemini la guarda el proceso principal de Electron cifrada con
  `safeStorage`, y las llamadas al modelo salen de ahí: la página no vuelve a
  verla (Fase 53). Si el sistema no ofrece cifrado real, dura solo la sesión.
  Por eso el modo Gemini solo funciona en la aplicación de escritorio.
* Solo Gemini. No hay opción de modelo local ni de otros proveedores. La prueba
  de punta a punta con la IA real no se ha hecho (necesita una clave).
* El agente recibe la lista de elementos interactivos (con los mismos números
  que usan `click_element`/`type_into_element`, así que actúa sobre lo que ve
  aunque la página esté desplazada) y el **texto legible** de la página
  (`get_page_text`, que prefiere `<main>` a los menús). Puede hacer clic,
  escribir, pulsar teclas, hacer scroll y volver atrás. El árbol de
  accesibilidad (AOM) sigue sin usarse en el prompt: numeraba aparte.
* El modelo de Gemini es un **ajuste** (variable `NAVEGADOR_GEMINI_MODEL` o el
  campo del panel del agente), porque Google retira modelos con el tiempo:
  `gemini-2.0-flash` ya no existe. El valor por defecto (`gemini-3.5-flash`) hay
  que confirmarlo en la [página oficial](https://ai.google.dev/gemini-api/docs/deprecations).
* Hay dos implementaciones del agente, una en TypeScript y otra en Python, que
  hacen lo mismo.

Las pendientes están planificadas en [`plan.md`](plan.md) (F05 y F34).

---

## Licencia

MIT — ver [LICENSE](LICENSE).
