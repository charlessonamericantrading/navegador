# plan.md — Plan de ataque completo de Navegador IA

> Fecha de elaboración: 2026-09-09. Rama de partida: `fix/paginas-vacias-y-rendimiento`
> (11 commits por delante de `main`). Estado medido, no leído: `cargo test --workspace`
> ejecutado hoy → **819 tests pasando, 0 fallando** en los 10 crates.
>
> **Progreso: bloques A y B cerrados** (Fase 41), salvo A1 (abrir el PR, requiere
> decisión) y A5 (reevaluada como innecesaria). **Del bloque C: C1, C3 y C4
> cerrados; C5, C6, C7 y C9 parcialmente** (Fases 42 a 45). El bloqueo estructural
> que impedía que cualquier bundle arrancara está resuelto. Superficie de
> plataforma medida: **85/114**, y 60 tests estilo-WPT.
> Ver el [checklist maestro](#15-checklist-maestro).
>
> Este documento sigue la misma doctrina que `engine/ARCHITECTURE.md`: cada tarea
> dice QUÉ falta, DÓNDE está el hueco en el código, CÓMO se verifica que quedó
> cerrado y QUÉ NO se hace a propósito. Cuando una tarea se cierre, se marca aquí
> y se documenta como una Fase nueva en `ARCHITECTURE.md` (que sigue siendo la
> fuente de verdad; si este plan lo contradice, gana `ARCHITECTURE.md`).

---

## Índice

- [0. Cómo usar este plan](#0-cómo-usar-este-plan)
- [1. Diagnóstico: qué hay y qué no](#1-diagnóstico-qué-hay-y-qué-no)
- [2. Bloque A — Higiene inmediata (1-2 días)](#2-bloque-a--higiene-inmediata)
- [3. Bloque B — Integración continua y calidad (2-3 días)](#3-bloque-b--integración-continua-y-calidad)
- [4. Bloque C — Web moderna: que los bundles no mueran (el techo del producto)](#4-bloque-c--web-moderna)
- [5. Bloque D — CSS y layout pendientes](#5-bloque-d--css-y-layout-pendientes)
- [6. Bloque E — Red: caché HTTP, HTTP/2, `@import`](#6-bloque-e--red)
- [7. Bloque F — Seguridad: sandbox de proceso](#7-bloque-f--seguridad-sandbox-de-proceso)
- [8. Bloque G — Plataforma web ausente (iframe, media, workers, sockets, IndexedDB, WebGL)](#8-bloque-g--plataforma-web-ausente)
- [9. Bloque H — Compatibilidad medible: Web Platform Tests reales](#9-bloque-h--compatibilidad-medible-wpt)
- [10. Bloque I — Producto: IA, Electron, backend, distribución](#10-bloque-i--producto)
- [11. Bloque J — Multiplataforma](#11-bloque-j--multiplataforma)
- [12. Bloque K — Rendimiento](#12-bloque-k--rendimiento)
- [13. Orden recomendado y dependencias entre bloques](#13-orden-recomendado-y-dependencias)
- [14. Lo que NO se va a hacer (y por qué)](#14-lo-que-no-se-va-a-hacer)
- [15. Checklist maestro](#15-checklist-maestro)

---

## 0. Cómo usar este plan

1. Cada tarea tiene un identificador (`A1`, `C4`...), una **prioridad** (P0 = bloquea
   el producto, P1 = importante, P2 = deseable, P3 = a largo plazo), una **estimación**
   orientativa en días de trabajo de una persona, los **ficheros** implicados, los
   **criterios de aceptación** y el **comando de verificación**.
2. Se ataca en el orden del apartado 13, no en el orden de aparición.
3. Regla heredada de `ARCHITECTURE.md` y que este plan mantiene: **si una función no
   está implementada, no existe.** Nada de stubs que devuelvan éxito. Un stub de
   `IntersectionObserver` que nunca dispara es peor que un `TypeError` claro.
4. Cada tarea cerrada:
   - añade tests en el crate afectado,
   - se verifica en vivo contra `engine_server.exe` (release) o contra una web real,
   - se documenta como Fase N+1 en `engine/ARCHITECTURE.md` con sus simplificaciones,
   - actualiza las cifras del README si cambian.
5. Los commits siguen el formato ya usado en el repo:
   `feat(crate): descripción (Fase N)` / `fix(crate): ...` / `docs: ...`.

---

## 1. Diagnóstico: qué hay y qué no

### 1.1 Lo que funciona y está probado (no tocar salvo regresión)

| Área | Estado real |
|---|---|
| Red | HTTP/1.1 + HTTPS (`hyper` + `rustls`), redirecciones 301-308, gzip/deflate/brotli, cookies RFC 6265 con `HttpOnly`/`SameSite`, CORS con preflight, CSP (7 directivas), esquemas seguros |
| HTML | `html5ever`, DOM mutable, `<template>` (simplificado), `<noscript>` correcto |
| CSS | Cascada con especificidad, `!important`, origen agente-de-usuario, `:hover` y pseudo-clases básicas, `@media` (con `em`/`rem`/rangos), `@supports`, `var()`, shorthands, `rem`/`em`/`%`, `hsl()`, `box-sizing` |
| Layout | Bloque, inline, inline-block, flexbox (`taffy`) con intrinsic sizing, grid (columnas, gap, áreas), tablas, `float`, `position` (relative/absolute/fixed/sticky), `overflow: hidden`, scroll real |
| JS | `boa` (ES moderno completo), DOM bindings, eventos con captura/burbujeo, `fetch`/`XHR`, timers, rAF, `localStorage`/`sessionStorage` persistentes, `location`, `history` (SPA), `MutationObserver`, `getComputedStyle`, `getBoundingClientRect`, CSSOM básico, Canvas 2D, `document.cookie` |
| Pintado | `tiny-skia`, SVG (`resvg`), fuentes reales, `border-radius`, sombras, `text-decoration`, `text-align`, `background-image` (mosaico natural), value/placeholder de inputs |
| IA | Crate `engine-ai` con AOM y `to_llm_representation` |
| Producto | Electron ↔ `engine_server` por NDJSON/IPC directo; pestañas, historial, formularios GET/POST, login por cookies; aviso `requires_javascript` |

### 1.2 Lo que falta (resumen; el detalle está en cada bloque)

| # | Hueco | Prioridad | Bloque |
|---|---|---|---|
| 1 | Las webs construidas con JS en cliente se ven vacías. ~~Causa raíz: no hay `<script type="module">`~~ **resuelto** (Fase 43). Lo que queda es superficie de plataforma: APIs del DOM que un framework toca al arrancar | **P0** | C3, C6 |
| 2 | Sin sandbox de proceso | P1 | F |
| 3 | ~~Sin CI~~ **hecho** (Fase 41): `engine.yml` y `app.yml` | ~~P0~~ | B |
| 4 | ~~README con datos falsos y `huecos_sin_resolver.md` inexistente~~ **hecho** (Fase 41). Queda solo la rama sin fusionar en `main` | P1 | A |
| 5 | Sin `<iframe>`, `<video>`, `<audio>`, WebGL, IndexedDB, WebSockets, Web Workers, Service Workers | P2/P3 | G |
| 6 | Sin caché HTTP ni HTTP/2 ni `@import` | P1 | E |
| 7 | Sin `::before`/`::after`, `linear-gradient()`, `background-size`/`position`/`repeat` variantes, `calc()` real, `transition`/`animation`, `transform` | P1 | D |
| 8 | Solo Windows; nunca compilado en macOS/Linux | P2 | J |
| 9 | Sin métrica de compatibilidad (4 ficheros WPT a mano, no la suite oficial) | P1 | H |
| 10 | IA solo con Gemini y clave del usuario en `localStorage`; agente duplicado en TS y Python; sin opción de modelo local (Ollama en PCCOM) | P1 | I |
| 11 | Backend Python huérfano que se sigue empaquetando | P1 | I |
| 12 | Instalador sin firmar; auto-update apuntando a un repo sin releases | P2 | I |
| 13 | JS interpretado sin JIT; sin reflow incremental; cada navegación reconstruye todo | P2 | K |

### 1.3 APIs de JavaScript ausentes (comprobado con `grep` en `engine/crates/js/src`, 2026-09-09)

Ninguna de estas existe hoy. Cada una es una línea potencial que mata un bundle entero:

```
matchMedia            IntersectionObserver   ResizeObserver        URLSearchParams
AbortController       TextEncoder/Decoder    customElements        performance.now
console (como global) CustomEvent            DOMParser             structuredClone
atob / btoa           document.currentScript document.write        innerText
dataset               closest()              insertAdjacentHTML    URL (constructor)
```

`document.readyState` y `Event` existen parcialmente (una referencia cada uno; hay
que verificar si son constructores reales o solo cadenas).

Globales que SÍ están registrados hoy (`register_global_property` en el crate `js`):
`window`, `document`, `location`, `history`, `navigator`, `localStorage`,
`sessionStorage`, `fetch`, `XMLHttpRequest`, `MutationObserver`, `setTimeout`,
`setInterval`, `clearTimeout`, `clearInterval`, `queueMicrotask`,
`requestAnimationFrame`, `cancelAnimationFrame`, `getComputedStyle`, más los del
arnés de tests (`test`, `assert_*`).

---

## 2. Bloque A — Higiene inmediata

Objetivo: que el repo diga la verdad y que el trabajo hecho llegue a `main`.
Sin esto, cualquier bloque posterior se construye sobre documentación que miente.

### A1 — Fusionar `fix/paginas-vacias-y-rendimiento` en `main` — P0 — 0,5 d

- **Qué**: abrir PR de la rama actual contra `main`. 11 commits, 42 ficheros,
  +8.083/-628 líneas.
- **Antes de abrir**: `cargo test --workspace` en verde (hecho hoy: 691/0),
  `cargo clippy --workspace` sin errores nuevos, `npm run build` en `frontend`.
- **Aceptación**: PR fusionado; `main` contiene la Fase 40.
- **Verificación**: `git log --oneline main | head -1` muestra `41ef295` o el merge.

### A2 — Corregir el README — P0 — 0,5 d

Ficheros: `README.md`.

Errores concretos detectados:

1. Sección "Web moderna": dice *"Hoy faltan `location.href` y `MutationObserver`"*.
   Falso desde el commit `8a0ea7f`. Sustituir por la lista real del apartado 1.3.
2. Tabla "Motor": *"703 pasando"*. Hoy son 691 (medido). `ARCHITECTURE.md` dice 805
   en la Fase 40. **Investigar la discrepancia** (ver A4) antes de escribir la cifra.
3. Comando `cargo test --workspace # los 703 tests` → quitar el número o poner el real.
4. Tabla "Seguridad": la fila *Política de mismo origen* está en ✅ pero el texto
   de abajo dice *"Sin política de mismo origen ni sandbox"*. Unificar: SOP existe
   para cookies/almacenamiento/CORS; lo que no existe es sandbox.
5. Sección "Sobre la IA": añadir que la clave de Gemini se guarda en `localStorage`
   del renderer y que no hay modo local todavía (hasta que I2 lo cierre).
6. Añadir enlace a este `plan.md`.

- **Aceptación**: ningún dato del README contradice `ARCHITECTURE.md` ni el código.

### A3 — Crear `engine/huecos_sin_resolver.md` o quitar la referencia — P0 — 0,25 d

`ARCHITECTURE.md` (Fase 40) cita `huecos_sin_resolver.md` dos veces como si existiera.
No está en el repo (`find` sin resultados). Dos opciones; se elige la primera:

- **Opción elegida**: crear `engine/huecos_sin_resolver.md` con la lista viva de
  simplificaciones declaradas que aún no se han cerrado (extraerlas con
  `grep -n "NO implementado\|No implementado\|Deliberadamente NO" engine/ARCHITECTURE.md`).
  Este fichero pasa a ser el backlog técnico del motor y este `plan.md` lo enlaza.
- Alternativa: borrar las dos referencias.
- **Aceptación**: `grep -rn huecos_sin_resolver engine/` solo apunta a ficheros existentes.

### A4 — Reconciliar la cifra de tests — P1 — 0,25 d

- Ejecutar `cargo test --workspace 2>&1 | grep "test result"` y sumar.
- Ejecutar `cargo test --workspace -- --list | wc -l` para contar sin ejecutar.
- Posibles causas de 805 vs 691: tests con `#[ignore]`, tests de integración en
  `engine/tests/` que no se compilan por defecto, doc-tests (hoy 0), o simplemente
  una cifra escrita a mano. Documentar la causa en `ARCHITECTURE.md`.
- **Aceptación**: README, ARCHITECTURE.md y la salida real coinciden.

### A5 — Limpiar el árbol de trabajo — P2 — 0,25 d

- `Navegador IA Setup.exe` (108 MB) vive en la raíz. Está ignorado por `*.exe`,
  correcto, pero conviene moverlo a `desktop/dist/` o borrarlo tras subirlo a un release.
- `backend/app/**/__pycache__/` está ignorado pero presente; borrar.
- `backend/build/`, `backend/dist/` idem.
- `.claude/launch.json`: decidir si se versiona (hoy sí, sin `.lock`).

---

## 3. Bloque B — Integración continua y calidad

Objetivo: que ningún commit rompa los 691 tests sin que alguien se entere.

### B1 — GitHub Actions: tests del motor — P0 — 0,5 d

Fichero nuevo: `.github/workflows/engine.yml`.

```yaml
name: engine
on:
  push: { branches: [main] }
  pull_request:
jobs:
  test:
    runs-on: windows-latest          # única plataforma verificada hoy
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with: { workspaces: engine }
      - run: cargo test --workspace --locked
        working-directory: engine
      - run: cargo clippy --workspace --all-targets -- -D warnings
        working-directory: engine
      - run: cargo fmt --all --check
        working-directory: engine
```

- Nota: `clippy -D warnings` y `fmt --check` probablemente fallen la primera vez.
  Hacer primero una pasada `cargo fmt --all` y `cargo clippy --fix` en un commit
  aparte (`chore(engine): fmt + clippy`) y solo después activar `-D warnings`.
- **Aceptación**: badge verde en el README; un PR con un test roto no se puede fusionar
  (activar *branch protection* en `main` con el check requerido).

### B2 — GitHub Actions: frontend y desktop — P1 — 0,5 d

Fichero: `.github/workflows/app.yml`.

- `npm ci` + `npm run lint` + `npm run build` en `frontend/`.
- `npm ci` en `desktop/` + `electron-builder --dir` (sin publicar) para comprobar que
  el empaquetado no está roto.
- **Aceptación**: ambos jobs en verde.

### B3 — Runner de WPT en CI — P1 — 0,25 d (depende de B1)

- Añadir paso `cargo run -p engine-core --bin wpt_runner -- tests/wpt-style` en
  `engine.yml`. El binario ya devuelve exit code 1 si algo falla.
- **Aceptación**: los 4 ficheros de `engine/tests/wpt-style/` se ejecutan en cada PR.

### B4 — Tests de humo del protocolo NDJSON — P1 — 1 d

- Hoy `engine_server` se prueba con tests unitarios dentro de `server.rs`
  (`Resize`, `Scroll`...). Falta un test de integración que arranque el binario
  real, le mande `ping`, `navigate` a un servidor local, `get_state`, `click`,
  `type_text`, `press_key`, `back`, `new_tab`, `get_accessibility_tree`, `shutdown`
  y compruebe las respuestas JSON.
- Fichero nuevo: `engine/crates/core/tests/ndjson_smoke.rs`.
- **Aceptación**: el test cubre los 17 `EngineRequest` del `match` en `server.rs:313-419`.

### B5 — Matriz multiplataforma en CI — P2 (ver bloque J)

---

## 4. Bloque C — Web moderna

**Este bloque es el producto.** Todo lo demás es soporte. El diagnóstico de la Fase 39
sigue vigente: no falta un motor de JS, faltan piezas sueltas de DOM/BOM y cada una
que falte mata el script completo con un `TypeError`.

Principio de trabajo del bloque: **medir antes de implementar**. No se implementa una
API porque "seguro que hace falta"; se implementa porque una sonda o un bundle real
se murió en ella.

### C1 — Sonda de APIs v2 (60+ comprobaciones) — P0 — 1 d

- La sonda de la Fase 39 medía 28 APIs (26/28). Ampliarla a un fichero versionado:
  `engine/tests/probes/api-probe.html`.
- Cada comprobación es `typeof X === 'function'` / `'object'` o una llamada mínima
  con `try/catch`, y el resultado se vuelca en `document.title` como `"N/M: faltan a,b,c"`.
- Lista inicial (de 1.3 más las de constructores/prototipos que los bundlers
  usan para *feature detection*):
  `Element.prototype`, `HTMLElement.prototype`, `Node.prototype`, `Event`,
  `CustomEvent`, `EventTarget` (constructor), `URL`, `URLSearchParams`, `AbortController`,
  `AbortSignal`, `TextEncoder`, `TextDecoder`, `atob`, `btoa`, `structuredClone`,
  `performance.now`, `performance.mark`, `console.log/warn/error/debug/table/group`,
  `matchMedia`, `IntersectionObserver`, `ResizeObserver`, `customElements.define`,
  `DOMParser`, `document.currentScript`, `document.readyState`, `document.write`,
  `document.documentElement.dataset`, `el.closest`, `el.matches`,
  `el.insertAdjacentHTML/Element/Text`, `el.innerText`, `el.textContent` (setter),
  `el.replaceWith`, `el.before/after/append/prepend`, `el.getAttributeNames`,
  `el.hasAttributes`, `el.toggleAttribute`, `el.scrollIntoView`, `el.focus/blur`,
  `document.activeElement`, `document.hasFocus`, `window.innerWidth/Height`,
  `window.devicePixelRatio`, `window.scrollX/Y`, `window.scrollTo`, `window.open`,
  `window.crypto.getRandomValues`, `crypto.randomUUID`, `Intl.DateTimeFormat`,
  `Intl.NumberFormat`, `queueMicrotask`, `Promise.allSettled`, `WeakRef`,
  `FinalizationRegistry`, `globalThis`, `Symbol.asyncIterator`, `Array.prototype.at`,
  `Object.hasOwn`, `String.prototype.replaceAll`, `import()` dinámico,
  `<script type="module">`, `<script defer>`, `<script async>`.
- **Aceptación**: la sonda corre con `wpt_runner` o con `engine_server` y el número
  queda registrado en `ARCHITECTURE.md`. El número inicial se anota aquí:
  `Sonda v2: __/__ (fecha)`.

### C2 — Corpus de bundles reales — P0 — 1 d

- Crear `engine/tests/bundles/` con 5 páginas mínimas **congeladas en disco**
  (sin red en tests) que usen los frameworks reales compilados en producción:
  1. `react-vite/` (Vite + React 19, `npm run build`, un contador).
  2. `vue-vite/` (Vue 3, un formulario).
  3. `svelte/` (Svelte 5).
  4. `nextjs-static/` (`next export` de una página con hidratación).
  5. `vanilla-esm/` (módulos ES nativos con `import`).
- Un test de integración por bundle: cargar con `build_page_keeping_runtime`,
  comprobar que `requires_javascript` es `false` y que el texto esperado aparece
  en el layout.
- Estos 5 tests **fallarán al crearse**. Es el objetivo: son la definición de
  "hecho" de todo el bloque C. Se marcan `#[ignore]` hasta que cada uno pase, y
  el plan es ir quitando `#[ignore]` uno a uno.
- **Aceptación**: los 5 bundles existen, cada uno tiene su test, y el primer
  fallo de cada uno está anotado (qué API, qué línea).

### C3 — Prototipos reales para los elementos DOM — P0 — 3 d

Ficheros: `engine/crates/js/src/dom_bindings.rs` (2.000+ líneas; valorar partirlo en
`dom_bindings/{element,node,document,event}.rs`).

- Hoy cada nodo se expone como un objeto con propiedades y métodos añadidos
  individualmente. Los bundlers hacen `Element.prototype.matches ||
  Element.prototype.msMatchesSelector`, `Object.getPrototypeOf(el) === HTMLDivElement.prototype`,
  `instanceof HTMLElement`, o parchean `Node.prototype.appendChild`. Nada de eso puede
  funcionar sin una cadena de prototipos real.
- Implementar la jerarquía mínima: `EventTarget` → `Node` → `Element` → `HTMLElement`
  → (`HTMLDivElement`, `HTMLInputElement`, `HTMLAnchorElement`, `HTMLFormElement`,
  `HTMLImageElement`, `HTMLCanvasElement`, `HTMLScriptElement`, `HTMLStyleElement`,
  `HTMLSelectElement`, `HTMLTextAreaElement`, `HTMLButtonElement`); `Document`,
  `Text`, `Comment`, `DocumentFragment`.
- Registrar los constructores como globales (`window.HTMLElement`...) para que
  `instanceof` funcione. Constructores que lanzan `TypeError: Illegal constructor`
  salvo `Event`, `CustomEvent`, `DocumentFragment`, `Text`, `Comment` (que sí son
  construibles en el spec).
- **No se hace**: `HTMLUnknownElement` para cada tag raro (todos caen a `HTMLElement`).
- **Aceptación**: `document.createElement('div') instanceof HTMLDivElement === true`;
  parchear `Element.prototype.foo = ...` afecta a todos los elementos.

### C4 — Constructores de eventos y `EventTarget` construible — P0 — 1 d

Ficheros: `engine/crates/js/src/dom_bindings.rs`, `event_loop.rs`.

- `new Event(type, {bubbles, cancelable, composed})`, `new CustomEvent(type, {detail})`,
  `new EventTarget()`. Sin esto React no puede sintetizar eventos ni ninguna lib de
  estado emitir cambios.
- `event.target`, `currentTarget`, `eventPhase`, `timeStamp`, `isTrusted` (false para
  los sintéticos), `defaultPrevented`, `composedPath()`.
- `KeyboardEvent`, `MouseEvent`, `InputEvent`, `FocusEvent` con sus campos
  (`key`, `code`, `clientX/Y`, `button`, `relatedTarget`, `inputType`, `data`).
  Esto además cierra el hueco declarado en `ARCHITECTURE.md` ("metadatos de tecla
  todavía no están implementados").
- **Aceptación**: `el.dispatchEvent(new CustomEvent('x', {detail: 1}))` llega a un
  listener con `e.detail === 1`.

### C5 — Utilidades de plataforma sin DOM — P0 — 2 d

Fichero nuevo: `engine/crates/js/src/platform.rs`.

- `URL` y `URLSearchParams` → envolver el crate `url` (ya es dependencia de `net`).
- `TextEncoder`/`TextDecoder` (UTF-8 solo; otras codificaciones lanzan `RangeError`
  real, no devuelven basura).
- `atob`/`btoa` → crate `base64`.
- `AbortController`/`AbortSignal` → integrado con `fetch` (cancelar la petición de
  verdad; hoy `fetch` es `hyper`, se puede abortar el future).
- `structuredClone` → serialización recursiva de objetos planos, arrays, `Map`, `Set`,
  `Date`; lanza `DataCloneError` con funciones/símbolos como el spec.
- `crypto.getRandomValues`, `crypto.randomUUID` → crate `rand`/`uuid`.
- `performance.now()` (ms desde la navegación, `f64`), `performance.mark/measure`
  (mínimo: no lanzar, registrar en un `Vec`).
- `console` como objeto global real: `log/info/warn/error/debug/trace/table/group/
  groupEnd/time/timeEnd/assert/count`. Salida a `tracing` (NO a stdout: rompería el
  protocolo NDJSON, ver `engine_server.rs:15`). Hoy existe `printEngineLog` como
  apaño; `console` debe reemplazarlo.
- `queueMicrotask` ya existe; verificar orden respecto a `Promise.then`.
- **Aceptación**: cada API tiene ≥3 tests; la sonda C1 las marca en verde.

### C6 — `document` y `Element`: métodos que faltan — P0 — 2 d

Fichero: `dom_bindings.rs`.

- `document.readyState` (`loading` → `interactive` → `complete`, con eventos
  `readystatechange`), `document.currentScript` (apuntar al `<script>` en ejecución;
  `null` en módulos), `document.write`/`writeln` (solo durante el parseo: como el
  motor parsea todo antes de ejecutar, se implementa como *insertar tras el script
  actual* y se documenta la simplificación), `document.activeElement`, `document.hasFocus`,
  `document.createDocumentFragment`, `document.createComment`, `document.createRange`
  (mínimo), `document.importNode`, `document.adoptNode`, `document.getElementsByClassName`,
  `document.elementFromPoint` (ya hay hit-testing en `server.rs`; exponerlo).
- `Element`: `closest`, `matches` (ya hay matcher real; exponerlo), `insertAdjacentHTML/
  Element/Text`, `innerText` (getter con layout: solo lo visible; setter = `textContent`),
  `outerHTML`, `dataset` (Proxy sobre `data-*`), `getAttributeNames`, `toggleAttribute`,
  `hasAttributes`, `replaceWith`, `before`, `after`, `append`, `prepend`, `replaceChildren`,
  `children` (solo elementos), `firstElementChild`/`lastElementChild`/
  `nextElementSibling`/`previousElementSibling`, `childElementCount`, `scrollIntoView`
  (mover el scroll real del servidor), `focus`/`blur` (con `focusin/focusout/focus/blur`),
  `getClientRects` (ya existe), `scrollTop/Left/Width/Height` (lectura real, escritura
  mueve scroll), `offsetTop/Left/Width/Height/Parent`, `clientWidth/Height`.
- `Node`: `contains`, `isConnected`, `nodeType` (constantes en `Node.ELEMENT_NODE`...),
  `nodeName`, `nodeValue`, `cloneNode(deep)`, `isEqualNode`, `compareDocumentPosition`,
  `getRootNode`, `normalize`.
- `NodeList`/`HTMLCollection` iterables con `forEach`, `length`, índice, `Symbol.iterator`.
- **Aceptación**: los tests de `engine/tests/wpt-style/dom-mutation-and-navigation.html`
  se amplían con cada método; sonda C1 en verde.

### C7 — `window`: propiedades de entorno — P0 — 1 d

Fichero: `engine/crates/js/src/window.rs`.

- `innerWidth/innerHeight/outerWidth/outerHeight` (del viewport real que ya llega
  en `Resize`), `devicePixelRatio` (1.0), `scrollX/scrollY/pageXOffset/pageYOffset`,
  `scrollTo/scrollBy/scroll` (mueven el scroll real: hoy `Scroll` viene del servidor;
  hace falta el camino inverso JS → servidor), `screen.width/height`,
  `matchMedia(query)` → reusar el evaluador de `@media` del crate `css`; devuelve
  `MediaQueryList` con `matches`, `media`, `addEventListener('change')` que dispara
  en `Resize`.
- `window.open` → **no se implementa**; lanza y se documenta (sin ventanas emergentes
  a propósito).
- `window.name`, `window.self/top/parent/frames` (= `window` mientras no haya iframes),
  `window.frameElement = null`.
- `alert/confirm/prompt` → registran en `tracing` y devuelven `undefined/false/null`;
  el servidor NDJSON emite un evento `dialog` para que la UI de Electron lo muestre
  (nuevo mensaje en `protocol.rs`).
- **Aceptación**: un bundle React que hace `window.matchMedia('(prefers-color-scheme: dark)')`
  no muere.

### C8 — Observadores — P1 — 2 d

Fichero nuevo: `engine/crates/js/src/observers.rs` (junto a `mutation_observer.rs`).

- `IntersectionObserver`: calcular intersección real contra el viewport a partir de
  los rectángulos del layout tras cada `Scroll`/`Resize`/re-layout. `observe/unobserve/
  disconnect/takeRecords`, `thresholds`, `rootMargin` (solo px). Callbacks en la cola de
  macrotareas.
- `ResizeObserver`: comparar `contentRect` tras cada re-layout.
- Regla: **si no se puede disparar de verdad, no se registra.** Un observador que
  nunca dispara deja al bundle esperando para siempre (misma razón por la que la
  Fase 39 no puso un stub de `MutationObserver`).
- **Aceptación**: test donde un elemento fuera del viewport entra al hacer scroll y
  el callback recibe `isIntersecting: true`.

### C9 — Módulos ES, `defer`, `async`, `import()` dinámico — P0 — 3 d — **EMPEZAR POR AQUÍ**

> **Reordenado el 2026-09-09.** Al verificar los huecos contra el código (Fase 41)
> quedó claro que esta tarea no es una más del bloque: es la que va **primera**.
> Se creía que el techo eran las APIs del DOM ausentes; es anterior a eso. Todo
> bundle de Vite, Next o Svelte se sirve como `<script type="module">`, que aquí
> no existe, así que **ninguno arranca por muchas APIs que se añadan**. C1 y C2
> (medir) siguen antes, pero de las tareas de implementación esta es la primera.

Ficheros: `engine/crates/core/src/scripting.rs`, `pipeline.rs`, `engine/crates/js/src/runtime.rs`.

- Hoy: los `<script>` se ejecutan **todos seguidos después de parsear** (`scripting.rs:13`).
  Los externos se descargan (`find_external_script_srcs`) pero hay que verificar el orden.
- Implementar el orden real del spec:
  1. scripts clásicos sin atributo → en orden de documento (ya),
  2. `defer` → tras el parseo, en orden, antes de `DOMContentLoaded`,
  3. `async` → en cuanto llegan (aquí: tras los clásicos, orden de descarga),
  4. `type="module"` → `defer` implícito, semántica de módulo (`import`/`export`,
     `this === undefined`, modo estricto). `boa` soporta módulos (`Module::parse`);
     hace falta un *module loader* que resuelva especificadores relativos contra la
     URL del documento y los descargue con `NetworkEngine`.
  5. `import()` dinámico → mismo loader, devuelve `Promise`.
  6. `<script type="importmap">` → P2, solo `imports` sin `scopes`.
  7. `nomodule` → ignorar cuando hay soporte de módulos.
- Todo bundle Vite/Next moderno es `type="module"`. **Sin esta tarea el corpus C2 no
  puede pasar aunque todas las APIs existan.**
- **Aceptación**: `vanilla-esm` del corpus C2 pasa; `defer` se ejecuta antes de
  `DOMContentLoaded` y `async` no bloquea.

### C10 — Ejecución de scripts durante el parseo (streaming) — P1 — 4 d

- Simplificación declarada en `ARCHITECTURE.md` (Fase 8, líneas ~2019-2087): el
  pipeline ejecuta todo el JS **después** de parsear el documento completo. Eso rompe
  `document.write`, `document.currentScript`, scripts que leen `document.body` cuando
  aún no existe (esperan `null`), y cualquier script que dependa de que el DOM "de
  abajo" no exista todavía.
- Solución: `html5ever` soporta pausar el `TreeSink` en `</script>`. Enganchar la
  ejecución del script en ese punto (`html5ever_sink.rs`), con el runtime de `boa`
  vivo durante el parseo.
- Riesgo: cambia la firma de `build_page*` y el orden de `fetch_external_stylesheets`.
  Hacerlo detrás de un flag interno hasta que el corpus C2 pase con ambos modos.
- **Aceptación**: `document.write('<p>x</p>')` inserta en el sitio correcto; un
  script antes de `<body>` ve `document.body === null`.

### C11 — Formularios desde JS — P1 — 1 d

- `form.submit()`, `form.reset()`, `form.elements`, `form.requestSubmit()`,
  evento `submit` cancelable desde JS (hoy el submit lo hace el servidor al hacer
  clic; el evento tiene que pasar por JS primero y respetar `preventDefault`).
- `FormData` (constructor desde `<form>`, `append/get/getAll/has/entries`), aceptado
  por `fetch` como cuerpo (`multipart/form-data` real con boundary).
- `input.value` setter que dispara `input`/`change` **solo** cuando lo hace el
  usuario (no cuando lo asigna JS; React depende de esta distinción).
- `input.checked`, `select.value`/`selectedIndex`/`options`, `textarea.value`.
- `EngineRequest::SubmitForm` en el protocolo (declarado como no implementado en
  `ARCHITECTURE.md`).
- **Aceptación**: `vue-vite` del corpus (formulario) pasa.

### C12 — Selección de texto y portapapeles — P2 — 1,5 d

- `window.getSelection()`, `Selection`/`Range` mínimos, `document.execCommand('copy')`
  (deprecado pero usado), `navigator.clipboard.writeText/readText` (vía Electron).
- Selección visual por arrastre en el servidor (`ARCHITECTURE.md`: "selección de
  texto no implementada").
- **Aceptación**: seleccionar con el ratón resalta y Ctrl+C copia.

### C13 — Cierre del bloque: corpus en verde — P0

- Quitar `#[ignore]` de los 5 tests de C2 uno a uno.
- Verificar en vivo contra 10 webs reales de la lista de la Fase 39
  (`ignislove.com`, una tienda Shopify, un blog Next, una app Vue, docs con Docusaurus,
  Google, Wikipedia, un periódico, GitHub, MDN) y anotar en `ARCHITECTURE.md` cuáles
  se ven y cuáles no, con captura.
- Actualizar el aviso `requires_javascript`: si el JS se ejecutó y sigue sin haber
  texto, el mensaje debe decir "el script se ejecutó pero falló en X" con el
  primer error capturado, no "esta página necesita JavaScript".

---

## 5. Bloque D — CSS y layout pendientes

Todas estas son simplificaciones declaradas en `ARCHITECTURE.md`. Prioridad según
frecuencia en CSS real.

### D1 — `::before` / `::after` con `content` — P1 — 2 d

- Ficheros: `engine/crates/css/src/selector.rs` (`NoPseudoElement` es un enum vacío:
  el parser rechaza `::before` y **descarta la regla entera**, ver línea ~2630 de
  `ARCHITECTURE.md`), `engine/crates/layout/src/tree.rs`.
- Implementar `PseudoElement::{Before, After}`, generar cajas anónimas en el árbol
  de layout con `content: "texto" | attr(x) | counter()` (counters P2), `display`
  por defecto `inline`.
- Impacto: iconos de fuente (Font Awesome), clearfix, viñetas personalizadas,
  comillas. Muchísimo CSS real.
- **Aceptación**: `a::after { content: " →" }` pinta la flecha.

### D2 — `linear-gradient()` / `radial-gradient()` como `background-image` — P1 — 1,5 d

- Ficheros: `engine/crates/css/src/parser.rs` (hoy solo extrae `url()`),
  `engine/crates/gfx/src/display_list.rs` + `image_paint.rs`.
- `tiny-skia` tiene `LinearGradient`/`RadialGradient` nativos: es solo parseo +
  mapeo de ángulo/paradas.
- Declarado como "candidato real siguiente" en la Fase 40.
- **Aceptación**: `background: linear-gradient(90deg, red, blue)` pinta el degradado.

### D3 — `background-size`, `background-position`, `background-repeat` variantes, capas múltiples — P1 — 1,5 d

- Continuación directa de la Fase 40. `cover`/`contain`/`<length>`/`%`,
  `no-repeat`/`repeat-x`/`repeat-y`, `center`/`top left`/`<length>`, y lista separada
  por comas (capas, la primera arriba).
- **Aceptación**: `background: url(x) center / cover no-repeat` se ve como en Chrome.

### D4 — `calc()` real — P1 — 1,5 d

- Hoy `calc()` y `var()` dentro de shorthands se dejan sin expandir y el layout
  resuelve a cero (Fase 39). `var()` ya funciona en longhands (commit `bc20af9`).
- Implementar un evaluador de expresiones con unidades mixtas (`calc(100% - 2rem)`)
  que se resuelva en el momento en que se conoce el *containing block*, es decir,
  en `layout`, no en `css`. Guardar el AST en `computed_style` y evaluar tarde.
- `min()`, `max()`, `clamp()` con el mismo evaluador.
- **Aceptación**: `width: calc(100% - 40px)` en un padre de 400px → 360px.

### D5 — `transform` 2D — P1 — 2 d

- `translate/scale/rotate/skew/matrix`, `transform-origin`. `tiny-skia` pinta con
  `Transform`; el hit-testing del servidor (`click`) tiene que invertir la matriz.
- No afecta al layout (correcto por spec), solo al pintado y al hit-testing.
- **Aceptación**: un botón con `transform: translateX(100px)` se pinta y recibe clics
  en su posición transformada.

### D6 — `transition` y `animation` (`@keyframes`) — P2 — 4 d

- Requiere un bucle de fotogramas real (hoy `requestAnimationFrame` va por la cola
  de `setTimeout(0)`). Diseñar primero el *tick* del servidor: `EngineRequest::Tick`
  o un temporizador interno que re-pinte a 60 Hz solo cuando hay animaciones activas.
- `transition-property/duration/timing-function/delay`, `@keyframes`,
  `animation-*`. Interpolación de longitudes, colores, `transform`, `opacity`.
- **No se hace**: `animation-timeline`, `view-transition`, `will-change`.
- **Aceptación**: `transition: opacity .3s` produce ≥10 capturas intermedias distintas.

### D7 — `opacity`, `visibility`, `z-index` con contextos de apilamiento — P1 — 1,5 d

- Verificar cuáles existen (grep en `gfx`). `z-index` necesita ordenar la display
  list por contexto de apilamiento (spec CSS2 apéndice E), no solo por orden de árbol.
- **Aceptación**: un `position: absolute; z-index: -1` queda detrás de su hermano.

### D8 — Listas: viñetas, numeración, `list-style-*` — P1 — 1 d

- `ARCHITECTURE.md` (hoja de agente de usuario): "sin viñetas ni sangría de listas".
  Commit `8360c3e` habla de "pulido de listas"; verificar qué quedó.
- `list-style-type: disc|circle|square|decimal|none`, `list-style-position`,
  marcadores como cajas `::marker` (reusar D1).
- **Aceptación**: `<ul><li>` pinta el punto y `<ol>` numera.

### D9 — Tipografía — P1 — 2 d

- `@font-face` con descarga de `woff2`/`ttf` (crate `ttf-parser` ya está; `woff2`
  necesita el crate `woff2` o descomprimir con `brotli`, que ya es dependencia).
- `font-family` con *fallback* real por glifo ausente (hoy: verificar en `text/`).
- `line-height` numérico/`normal` correcto, `letter-spacing`, `word-spacing`,
  `text-transform`, `white-space: pre|nowrap|pre-wrap`, `text-overflow: ellipsis`,
  `word-break`, `overflow-wrap`.
- Shaping con `rustybuzz` para ligaduras/árabe/devanagari (hoy `text/src/shape.rs`
  tiene 190 líneas cambiadas en esta rama: revisar qué cubre).
- **Aceptación**: una página con Google Fonts por `@font-face` usa la fuente descargada.

### D10 — Grid completo — P2 — 2 d

- Hoy: `grid-template-columns`, `gap`, `grid-template-areas`. Faltan
  `grid-template-rows`, `grid-auto-flow`, `grid-auto-rows/columns`, `repeat(auto-fill,
  minmax())`, `grid-column/row` con `span`, alineación (`justify-items`, `align-content`).
  `taffy` lo soporta todo con la feature `grid`; es solo el puente en `layout/src/tree.rs`.

### D11 — Flexbox completo — P2 — 1 d

- Verificar `order`, `align-self`, `flex-wrap` multi-línea con `align-content`, `gap`
  en flex, `min-width: auto` correcto. `taffy` lo hace; comprobar el mapeo.

### D12 — Otros CSS frecuentes — P2 — 2 d

- `outline`, `cursor` (enviar al servidor para cambiar el cursor de Electron),
  `pointer-events: none` (afecta al hit-testing), `user-select`, `object-fit`/
  `object-position` en `<img>`, `aspect-ratio`, `inset`, `gap` en flex,
  `filter: blur|grayscale|drop-shadow` (P3), `backdrop-filter` (P3), `clip-path` (P3),
  `mix-blend-mode` (P3), `columns` (P3), `writing-mode` (P3).
- `:focus`, `:focus-visible`, `:active`, `:checked`, `:disabled`, `:first-child`,
  `:last-child`, `:nth-child(an+b)`, `:not()`, `:is()`, `:where()`, `:has()` (P2),
  `:root`, `:empty`, `:target` — verificar cuáles del `EnginePseudoClass` existen
  y añadir el resto. El crate `selectors` los parsea; falta la evaluación en `element.rs`.
- `@container` (P3), `@layer` (P2: afecta a la cascada), `@property` (P3),
  `@font-face` (D9), `@import` (E3), `@page` (nunca).

### D13 — Reflow incremental — P2 — 5 d

- Hoy cualquier mutación del DOM desde JS re-hace layout completo. Para páginas de
  13.000 nodos son ~1,8 s por mutación (cifra de la Fase 39 para la carga inicial).
  Con React re-renderizando en cada tecleo, es inutilizable.
- Diseñar *dirty flags* por subárbol: una mutación marca su ancestro de bloque
  más cercano; el layout solo recalcula desde ahí si el tamaño del contenedor no
  cambia (contención). Empezar por el caso fácil: cambios de texto/atributo dentro
  de un bloque con `width` fijo.
- Cache de estilos calculados por (elemento, hash de reglas aplicables).
- **Aceptación**: cambiar un `textContent` en una página de 10.000 nodos tarda
  <50 ms (medido con `performance.now()` desde el propio JS).

---

## 6. Bloque E — Red

### E1 — Caché HTTP en memoria y disco — P1 — 2 d

- Fichero nuevo: `engine/crates/net/src/cache.rs`.
- RFC 9111 mínimo: `Cache-Control: max-age/no-store/no-cache`, `Expires`, `ETag` +
  `If-None-Match`, `Last-Modified` + `If-Modified-Since`, 304. Clave: método + URL.
  Solo GET/HEAD. `Vary: Accept-Encoding` respetado.
- Disco: mismo directorio que `localStorage` (Fase 25), formato simple
  (cabeceras JSON + cuerpo). Límite 200 MB con LRU.
- Impacto medido esperado: la segunda carga de Wikipedia debería bajar de 4,0 s a
  <1 s (sin descargas).
- **Aceptación**: test con servidor local que cuenta peticiones: segunda navegación
  no vuelve a pedir el CSS/imágenes con `max-age`.

### E2 — HTTP/2 — P2 — 1 d

- `hyper` 1.x soporta h2 con la feature `http2` y ALPN en `hyper-rustls`. Es
  configuración, no código: activar y verificar que `Accept-Encoding`, redirecciones y
  cookies siguen pasando los tests.
- Multiplexación reduce la latencia de subrecursos (hoy paralelizados con
  `futures-util`, pero cada uno abre conexión).
- **Aceptación**: `NetworkResponse` expone `version` y contra `https://www.google.com`
  devuelve `HTTP/2`.

### E3 — `@import` en hojas de estilo — P1 — 0,5 d

- Declarado no implementado en `fetch_external_stylesheets`. Parsear `@import url(...)
  [media]` al principio de cada hoja descargada, descargar recursivamente (límite de
  profundidad 5, detección de ciclos), insertar en orden. Reusar el pool paralelo de
  la Fase 39.
- **Aceptación**: una hoja que importa otra aplica ambas.

### E4 — `<link rel="preload|prefetch|modulepreload|icon">` — P2 — 0,5 d

- `preload`/`modulepreload`: adelantar descarga al pool. `icon`: exponer el favicon
  en `get_state` para que la pestaña de Electron lo muestre.

### E5 — Streaming de respuesta y parseo progresivo — P2 — 3 d

- Hoy se descarga el cuerpo completo antes de parsear. Con C10 (parseo con scripts en
  streaming) tiene sentido alimentar `html5ever` por *chunks*. Mejora el tiempo hasta
  el primer pintado en páginas grandes.

### E6 — DNS, proxy, `Referer`, `Referrer-Policy`, HSTS — P2 — 1,5 d

- `Referer` correcto según `Referrer-Policy` (hoy verificar si se manda).
- HSTS: recordar `Strict-Transport-Security` y forzar https (persistente en disco).
- Proxy del sistema: `hyper-util` puede leer `HTTPS_PROXY`; exponer en ajustes.
- DNS propio: **no** (doctrina de dependencias).

### E7 — Cookies: `Set-Cookie` con `Partitioned`, `__Host-`/`__Secure-` prefijos, límites — P2 — 0,5 d

---

## 7. Bloque F — Seguridad: sandbox de proceso

Es el hueco de seguridad más grave que queda y el único ❌ de la tabla del README.
Rust elimina la corrupción de memoria pero no un fallo lógico en `boa` que permita a
un script leer el disco vía una API mal expuesta, ni un bug en `resvg`/`image` con un
fichero malicioso.

### F1 — Modelo de amenazas escrito — P1 — 0,5 d

- Fichero nuevo: `engine/SECURITY.md`. Qué se protege (disco del usuario, otras
  pestañas, cookies de otros orígenes, la red local), de qué (página maliciosa,
  subrecurso malicioso, servidor comprometido), y qué NO (un atacante con acceso
  local a la máquina).
- Inventario de superficie: cada API de `engine_server` que toca disco
  (`localStorage` en disco, caché E1, sesión persistente del commit `8360c3e`).

### F2 — Aislar el motor en un proceso por pestaña — P1 — 4 d

- Hoy: un solo `engine_server` con N pestañas en el mismo proceso (`server.rs:147`,
  struct de pestaña). Un pánico en una pestaña mata todas.
- Diseño: `engine_server` pasa a ser un **broker** que lanza un `engine_tab` (nuevo
  binario en `core/src/bin/`) por pestaña, reenvía las peticiones NDJSON por su
  `tab_id` y agrega respuestas. El broker es el único que toca disco (cookies,
  `localStorage`, caché): los procesos de pestaña le piden esos datos por IPC y solo
  reciben los de su origen.
- Ventaja inmediata: un pánico/OOM en una pestaña no tumba el navegador;
  `close_tab` mata el proceso y libera memoria de verdad.
- **Aceptación**: `panic!()` inyectado en una pestaña → la pestaña muestra "página
  bloqueada", las demás siguen.

### F3 — Sandbox del SO para los procesos de pestaña — P1 — 3 d (Windows) + 2 d por plataforma

- Ya existe `engine/crates/core/src/sandbox.rs` (Fase 37, "sandboxing avanzado";
  `sandbox.rs:58` habla de plataforma sin soporte). **Leerlo primero** y documentar
  qué hace hoy exactamente antes de ampliarlo.
- Windows: *Job Objects* (límite de memoria/CPU, sin procesos hijos), *restricted
  token* (sin privilegios), *AppContainer* (P2, más complejo pero es lo que usa
  Chromium). Crate `windows` o `windows-sys`.
- Linux: `seccomp-bpf` + *namespaces* (crate `nix`/`seccompiler`). macOS:
  `sandbox_init` con perfil `.sb`.
- El proceso de pestaña **no debe** poder abrir ficheros ni sockets: toda la red pasa
  por el broker. Esto obliga a mover `NetworkEngine` al broker (o a un tercer proceso
  de red, como Chromium). Hacerlo por fases: primero red en pestaña con sandbox
  parcial, después red en broker.
- **Aceptación**: desde JS, un `fetch('file:///C:/Windows/win.ini')` falla en el
  esquema (ya), y un intento de `std::fs::read` inyectado en el proceso de pestaña
  falla por el SO, no por el código.

### F4 — Aislamiento de sitios (site isolation) — P3

- Un proceso por **sitio**, no por pestaña, para que un `<iframe>` de otro origen no
  comparta memoria. Depende de G1 (iframes). Solo tiene sentido después de F2/F3.

### F5 — Endurecimiento del propio protocolo NDJSON — P2 — 1 d

- Límite de tamaño por línea (hoy `main.js`/backend hablan de buffer de 64 MB),
  validación estricta de JSON con `deny_unknown_fields`, `tab_id` no adivinable
  (UUID en vez de posición: `server.rs:222` documenta que hoy es por posición).
- Fuzzing del parser de peticiones con `cargo-fuzz` (P2).

### F6 — Fuzzing de decodificadores — P2 — 1 d

- `cargo-fuzz` sobre `engine-image::decode_image` (PNG/JPEG/SVG/WebP), el parser de
  CSS y el parser de cookies. Los crates externos ya están fuzzeados, pero el
  *pegamento* propio no.

### F7 — Auditoría de dependencias en CI — P1 — 0,25 d

- `cargo audit` y `cargo deny` (licencias + advisories) en `engine.yml`;
  `npm audit --audit-level=high` en `app.yml`.

---

## 8. Bloque G — Plataforma web ausente

Ordenado por lo que más páginas rompe.

### G1 — `<iframe>` — P1 — 4 d

- Necesario para: vídeos de YouTube embebidos, widgets de pago (Stripe), Google
  Maps, botones sociales, y toda la publicidad. Sin iframe muchas páginas "se ven"
  pero tienen agujeros.
- Diseño: un iframe es un **documento anidado** con su propio DOM, hoja de estilos,
  runtime JS y `location`, cuyo `<html>` se coloca como caja reemplazada en el
  layout del padre. `window.parent`/`frames`/`postMessage` para comunicación
  entre ambos, con comprobación de origen. `sandbox` attribute con sus tokens.
  `srcdoc`. `about:blank`.
- Dependencia fuerte de F2 si se quiere aislar por origen; sin F2, mismo proceso
  (aceptable como primera fase, igual que Firefox durante años).
- **Aceptación**: una página con un iframe de otro origen pinta el contenido del
  iframe en su caja y `postMessage` cruza en ambas direcciones.

### G2 — `<video>` y `<audio>` — P2 — 5 d

- Decodificación: **no se escribe a mano** (doctrina). Opciones: crate `ffmpeg-next`
  (enlaza libav, licencia LGPL, pesado), `symphonia` (audio puro Rust: MP3/AAC/FLAC/
  Vorbis/Opus) + `rav1d`/`dav1d` para AV1, o delegar el vídeo a Electron
  (`<video>` de Chromium en una capa superpuesta posicionada con el rectángulo del
  layout). La delegación a Electron es la más barata y honesta: el motor calcula la
  caja, Electron reproduce. Elegir esta como primera fase; documentar que el
  vídeo NO lo pinta el motor.
- API JS: `HTMLMediaElement` (`play/pause/currentTime/duration/volume/muted/
  readyState`, eventos `play/pause/timeupdate/ended/loadedmetadata`), controles nativos.
- Audio puro con `symphonia` + `cpal` sí puede vivir en el motor (P3).
- **Aceptación**: un `<video src="x.mp4" controls>` se reproduce en su sitio y
  `video.play()` desde JS funciona.

### G3 — WebSockets — P2 — 1,5 d

- Crate `tokio-tungstenite` (ya está `tokio`). API `WebSocket` completa: `send`,
  `close`, `onopen/onmessage/onerror/onclose`, `binaryType`, `readyState`. CSP
  `connect-src` ya existe; aplicarla a `ws://`/`wss://`. Origen en el handshake.
- Necesario para: chats, dashboards en vivo, HMR de Vite en desarrollo (útil para
  el propio corpus C2).
- **Aceptación**: eco contra un servidor local `tungstenite`.

### G4 — IndexedDB — P2 — 4 d

- Crate `sled` o `redb` (Rust puro) como almacén clave-valor por origen. API:
  `indexedDB.open` con `onupgradeneeded`, `IDBDatabase`, `IDBObjectStore`
  (`add/put/get/delete/getAll/count/openCursor`), `IDBTransaction` con `readonly/
  readwrite`, índices con `createIndex`, `IDBKeyRange`. Todo asíncrono por la cola
  de macrotareas; claves y valores con `structuredClone` (C5).
- Necesario para: Firebase, muchas PWAs, Excalidraw, editores offline.
- **Aceptación**: los tests estilo WPT `idbfactory_open` básicos portados a mano.

### G5 — Web Workers — P2 — 3 d

- Un `Worker` = un `boa::Context` en un hilo de `tokio` con su propia cola de
  macrotareas, sin DOM, con `postMessage`/`onmessage` (mensajes por `structuredClone`),
  `importScripts`, `fetch`, timers, `self`. `SharedWorker` no. `SharedArrayBuffer`
  no (requiere COOP/COEP).
- **Aceptación**: `new Worker('w.js')` calcula algo y devuelve el resultado.

### G6 — Service Workers — P3 — 6 d

- Depende de E1 (caché), G5 (workers) y de un registro persistente por origen.
  `register`, ciclo `install/activate/fetch`, `Cache` API, `clients`. Es lo que hace
  que las PWAs funcionen offline. Muy invasivo: intercepta toda la red de su alcance.
  Dejarlo para el final del bloque.

### G7 — WebGL / WebGPU — P3 — 10+ d

- `wgpu` ya es dependencia de `gfx` (para la ventana). Exponer WebGL 1 sobre
  `wgpu` es un traductor GLSL→WGSL (`naga` lo hace) más ~300 funciones de API.
  WebGPU sería más directo (`wgpu` ES WebGPU) pero casi ninguna web lo usa aún.
- Pragmático: `canvas.getContext('webgl')` devuelve `null` de forma honesta (la
  web cae a su fallback 2D si lo tiene). Ya es así hoy por ausencia; documentarlo.

### G8 — APIs de dispositivo — P3

- `Notification`, `Geolocation`, `getUserMedia`, `Bluetooth`, `USB`, `Battery`,
  `Vibration`, `Gamepad`: todas devuelven denegado/no soportado de forma explícita
  y documentada. Sin stubs que finjan éxito.

### G9 — `<dialog>`, `<details>/<summary>`, `<progress>`, `<meter>`, `<input type=date|color|range|file>` — P2 — 2 d

- Controles nativos que faltan en `layout` (verificar cada uno). `<input type=file>`
  necesita diálogo de Electron y `File`/`FileList`/`FileReader`/`Blob` en JS (C5-bis).

### G10 — `Blob`, `File`, `FileReader`, `URL.createObjectURL`, `ReadableStream` — P2 — 2 d

- `fetch(...).body` como `ReadableStream`, `response.blob()`, `arrayBuffer()`,
  `formData()`. Descargas: `<a download>` → diálogo de guardado de Electron.

### G11 — Impresión, `window.print()`, PDF — P3

### G12 — MathML, namespaces SVG correctos en el DOM — P3

- Simplificación declarada en `html5ever_sink.rs`. Afecta a `document.createElementNS`
  y a `el.namespaceURI`, que D3.js y algunas libs de gráficos consultan.

---

## 9. Bloque H — Compatibilidad medible (WPT)

Sin este bloque, "compatibilidad" sigue siendo una impresión. `ARCHITECTURE.md` lo
dice en su sección "Métrica de progreso" y explica qué falta del arnés.

### H1 — Completar `testharness.js` — P1 — 2 d

- Fichero: `engine/crates/js/src/test_harness.rs`.
- Hoy: `test`, `assert_equals`, `assert_true`, `assert_false`. Faltan (declarados):
  `async_test` (con `step`, `step_func`, `done`), `promise_test`, `assert_throws_js`,
  `assert_throws_dom`, `assert_array_equals`, `assert_not_equals`, `assert_in_array`,
  `assert_class_string`, `assert_own_property`, `assert_unreached`, `assert_approx_equals`,
  `setup({explicit_done, timeout})`, `done()`, `add_completion_callback`,
  `step_timeout`, `format_value`.
- Alternativa más honesta: **cargar el `testharness.js` real de WPT** en vez de
  reimplementarlo en Rust. Si el motor ya ejecuta ES moderno, el fichero oficial
  (~4.000 líneas de JS) debería correr. Probarlo primero; si corre, H1 se reduce a
  0,5 d y la métrica es mucho más creíble. Solo si falla, reimplementar.
- **Aceptación**: `testharness.js` oficial carga y `add_completion_callback` entrega
  los resultados a Rust.

### H2 — Vendorizar un subconjunto de WPT — P1 — 1 d

- Script `scripts/sync-wpt.sh` que clona `web-platform-tests/wpt` a `engine/tests/wpt/`
  (ignorado en git, o *sparse checkout* de los directorios elegidos) fijado a un commit.
- Directorios iniciales con sentido para lo que el motor soporta:
  `dom/nodes`, `dom/events`, `html/dom`, `html/semantics/forms` (parcial),
  `css/CSS2/box-display`, `css/css-flexbox`, `css/css-grid`, `css/selectors`,
  `cssom`, `fetch/api/basic`, `xhr`, `url`, `encoding`, `html/webappapis/timers`,
  `html/browsers/history`, `storage`, `cookies`.
- `wpt_runner` gana: `--expectations expectations.json` (lista de tests que se sabe
  que fallan, para que CI solo falle en **regresiones**, no en lo que nunca pasó),
  `--json` para exportar resultados, timeout por test (hoy no hay: un test con bucle
  infinito cuelga el runner), y soporte de `<script src="/resources/testharness.js">`
  con rutas absolutas del corpus.
- Los reftests (comparación de capturas contra referencia) son P2: el runner
  necesita pintar ambos y comparar píxeles con tolerancia.
- **Aceptación**: `wpt_runner tests/wpt/dom/nodes --json > results.json` produce
  un número. Ese número va al README con fecha y **sustituye** cualquier afirmación
  cualitativa de compatibilidad.

### H3 — Panel de compatibilidad — P2 — 0,5 d

- `scripts/wpt-report.js` genera una tabla Markdown por directorio (pasan/total/%)
  a partir de `results.json`; se pega en `ARCHITECTURE.md` en cada fase.

### H4 — Tests de captura (visual regression) propios — P2 — 1 d

- Para cada web del corpus C13, guardar la captura PNG de referencia y comparar en
  CI con tolerancia (crate `image-compare` o `dssim`). Detecta regresiones de pintado
  que ningún test unitario ve.

---

## 10. Bloque I — Producto

### I1 — Decidir el destino del backend Python — P1 — 0,5 d de decisión + 1 d de ejecución

Hechos:
- `backend/app/core/main.py` expone un WebSocket `/ws` que envuelve al motor y
  ejecuta un agente Gemini (`backend/app/domains/agent/agent.py`).
- `frontend/src/domains/agent/AgentOrchestrator.ts` hace **lo mismo** en TypeScript
  contra Gemini directamente desde el renderer.
- Electron ya habla con `engine_server` por IPC (`desktop/main.js:417`) sin pasar por Python.
- `desktop/package.json` sigue empaquetando `build-resources/backend-server` (PyInstaller)
  en el instalador: son decenas de MB y un proceso más que arrancar.

Opción recomendada: **eliminar el backend Python** y quedarse con el orquestador
en TypeScript, moviendo las llamadas al LLM del renderer al **proceso principal de
Electron** (por seguridad: la clave no debe vivir en el renderer ni en `localStorage`).
Si más adelante hace falta Python (por ejemplo, para hablar con Faster-Whisper en
PCCOM), se llama por HTTP a un servicio externo, no se empaqueta.

Tareas si se elimina:
- Borrar `backend/`, `scripts/install-backend.js`, `backend-server.spec`, la entrada
  `extraResources` de `desktop/package.json`, `instalar.bat/.sh` (o dejarlos solo
  para `npm install`), y las referencias en README/ARCHITECTURE.md ("Integración con
  el producto" menciona a Python como cliente del puente).
- **Aceptación**: `npm run build:app` produce un instalador sin carpeta
  `backend-server` y el Copiloto sigue funcionando.

### I2 — Proveedor de IA configurable: Gemini, Anthropic, OpenAI-compatible, Ollama local — P1 — 2 d

Ficheros: `frontend/src/domains/agent/AgentOrchestrator.ts`, `AgentSidebar.tsx`,
`desktop/main.js`, `desktop/preload.js`.

- Abstraer `runGeminiStep` en una interfaz `LlmProvider { complete(prompt, tools) }`
  con implementaciones:
  - `GeminiProvider` (la actual, `gemini-2.0-flash`; actualizar a la versión vigente),
  - `AnthropicProvider` (Messages API con *tool use*; el AOM como herramienta),
  - `OpenAiCompatibleProvider` (sirve para OpenAI, Groq, Mistral, LM Studio),
  - `OllamaProvider` apuntando por defecto a `http://192.168.1.47:11434/api/generate`
    (el 7B `Qwen2.5-Coder` de PCCOM según el CLAUDE.md global) con la URL editable.
- Las llamadas HTTP salen del **proceso principal** de Electron vía `ipcRenderer.invoke('llm:complete')`;
  la clave se guarda con `safeStorage` de Electron (cifrado del SO), no en `localStorage`.
- Selector de proveedor y modelo en el panel de ajustes; modo "simulación" se conserva.
- **Aceptación**: el agente completa una tarea ("busca X en Wikipedia y dime el primer
  párrafo") con Ollama local sin clave de ningún tipo.

### I3 — El agente usa el AOM, no el texto plano — P1 — 1,5 d

- Hoy `AgentOrchestrator.ts` manda `domText` (texto) al modelo. El crate `engine-ai`
  ya genera `to_llm_representation` con roles y coordenadas, y `EngineRequest::
  GetAccessibilityTree` existe. Conectarlos: el prompt del agente lleva el AOM y las
  acciones devuelven `{action: 'click', target_id}` en vez de coordenadas adivinadas.
- Añadir al AOM: estado (`checked`, `disabled`, `expanded`), `value` de inputs,
  `href` de enlaces, y un `id` estable por nodo para que el modelo lo referencie.
- Herramientas del agente: `navigate`, `click(id)`, `type(id, text)`, `press(key)`,
  `scroll(dy)`, `back`, `read(id)`, `done(answer)`. Bucle con límite de pasos y
  confirmación del usuario antes de `submit` de formularios o navegación a un
  dominio nuevo (seguridad del agente).
- **Aceptación**: tarea "inicia sesión en X con estas credenciales" se completa
  usando ids del AOM y pide confirmación antes de enviar el formulario.

### I4 — Transcripción de voz (feature `feat/voice-transcription` de GlowApp reutilizable) — P2 — 1 d

- Botón de micrófono en el panel del Copiloto → `MediaRecorder` en el renderer de
  Electron (Chromium, sí lo tiene) → POST al endpoint de Faster-Whisper en PCCOM →
  texto en el cuadro. URL configurable; desactivado si no responde.

### I5 — UI de Electron: lo que falta para un navegador usable — P1 — 3 d

Ficheros: `frontend/src/core/App.tsx`, `domains/browser/components/BrowserViewport.tsx`.

- Favicon en pestañas (E4), indicador de carga real (progreso por subrecursos:
  nuevo evento `progress` en el protocolo), botón de recargar y detener,
  menú contextual (abrir en pestaña nueva, copiar enlace, inspeccionar AOM),
  atajos (Ctrl+T/W/L/R/Tab, Alt+←/→, F5, Ctrl+F para buscar en página),
  zoom (Ctrl +/-: `Resize` con factor de escala en el servidor), marcadores
  (JSON en `userData`), historial navegable con búsqueda, descargas (G10),
  gestor de cookies/almacenamiento por sitio ("borrar datos de este sitio"),
  candado TLS con detalles del certificado (`rustls` los expone), página de error
  con detalle real (`NetworkError` ya tiene variantes), `about:blank`/`about:settings`
  internas, modo oscuro de la UI, ventana con estado recordado.
- Panel de desarrollador mínimo: árbol DOM, estilos calculados, AOM, consola
  (los `console.*` de C5 viajan por un nuevo mensaje `console` del protocolo), red
  (lista de peticiones con estado/tiempo).
- **Aceptación**: cada elemento tiene un test de humo en Playwright **contra la UI de
  Electron** (no contra el motor; no viola la regla de "sin Playwright en el
  instalador", es dev-only).

### I6 — Distribución: firma, releases, auto-update — P2 — 1 d + decisión de compra

Fichero: `desktop/DISTRIBUCION.md` ya lo explica; falta ejecutarlo.

- Publicar el primer *release* en GitHub (`charlessonamericantrading/navegador`,
  ya configurado en `publish`) con `electron-builder --publish always` desde CI
  (`GH_TOKEN` como secreto). Hasta entonces el auto-update falla en silencio.
- Comprar certificado de firma (OV ~70-300 USD/año) o, gratis, instrucciones de
  SmartScreen en la página de descarga. Decisión del propietario.
- `desktop/package.json`: `version` sigue en `1.0.0` mientras el motor es `0.1.0` y
  el README dice "no apto para uso general". Alinear a `0.x` con *semver* y un
  `CHANGELOG.md`.
- **Aceptación**: `latest.yml` publicado; la app instalada detecta la siguiente versión.

### I7 — Telemetría de fallos opt-in — P3 — 1 d

- Cuando una pestaña muere (F2) o `requires_javascript` salta con un error de JS,
  ofrecer enviar la URL + primer error a un endpoint propio. Estrictamente opt-in.
  Es la forma más rápida de saber qué API falta en el mundo real (bloque C).

### I8 — Documentación de usuario y contribución — P2 — 1 d

- `CONTRIBUTING.md` (cómo añadir una fase, la doctrina de "no existe si no está
  implementado", cómo correr `wpt_runner`), `docs/protocolo-ndjson.md` (los 17
  mensajes con ejemplo JSON de petición y respuesta), plantilla de PR con checklist
  (tests, ARCHITECTURE.md, README).

---

## 11. Bloque J — Multiplataforma

### J1 — Compilar en Linux y macOS en CI — P2 — 1 d

- Añadir `ubuntu-latest` y `macos-latest` a la matriz de `engine.yml`, solo
  `cargo build` + `cargo test` (sin empaquetar). Los fallos esperables: rutas de
  `localStorage`/sesión con `\`, `sandbox.rs` (solo Windows), enlaces a fuentes del
  sistema en `text/`.
- **Aceptación**: los tres SO en verde.

### J2 — Fuentes del sistema por plataforma — P2 — 1 d

- Verificar cómo `text/` localiza fuentes. Si está a mano, usar `font-kit` o
  `fontdb` (ya llega con `usvg`) para descubrir fuentes en Windows/macOS/Linux/
  fontconfig.

### J3 — Empaquetado macOS (`dmg`) y Linux (`AppImage`/`deb`) — P2 — 1 d

- `electron-builder` ya tiene targets `mac` en `package.json`; añadir `linux`.
  El motor se compila por plataforma en CI y se copia a `build-resources/engine/`.
- Notarización de macOS: requiere cuenta de desarrollador de Apple (99 USD/año).
  Decisión del propietario; sin ella Gatekeeper bloquea.

### J4 — Móvil — no planificado

- Electron no existe en móvil. Sería una app nativa con el motor como biblioteca
  (`cdylib` + JNI/Swift). Fuera de alcance; se documenta como decisión.

---

## 12. Bloque K — Rendimiento

Cifras de partida (Fase 39, 2026-08-27): página sintética 13.000 nodos / 2.000
reglas → 1,8 s; Wikipedia "España" → 4,0 s CPU; Google → 457 ms.

### K1 — Banco de pruebas de rendimiento reproducible — P1 — 1 d

- `engine/benches/` con `criterion`: parseo, cascada, layout, pintado por separado
  sobre 3 páginas congeladas (pequeña/media/grande). CI guarda los resultados y avisa
  si una regresión supera el 20 %.
- **Aceptación**: `cargo bench` produce cifras y hay un fichero `BENCHMARKS.md` con
  la evolución por fase.

### K2 — Perfilado de la ruta caliente actual — P1 — 1 d

- `cargo flamegraph` sobre Wikipedia. Sospechosos según `ARCHITECTURE.md`: clonado
  de `Arc<RwLock<Node>>` en cada consulta, medición de texto sin caché por
  (fuente, tamaño, cadena), `resolve_style` por nodo sin caché de reglas compartidas
  (Bloom filter de ancestros como Stylo).

### K3 — Caché de medición de texto y de estilos — P1 — 2 d

### K4 — Pintado incremental y por capas — P2 — 3 d

- Hoy cada `get_state` genera una captura PNG completa en Base64 (coste de codificar
  + transferir por IPC en cada scroll). Alternativas: memoria compartida entre
  `engine_server` y Electron, o solo enviar la región sucia. Con D6 (animaciones)
  esto es obligatorio.

### K5 — JIT — P3 / no planificado

- `boa` no tiene JIT. Cambiar a V8 (`rusty_v8`) rompería la doctrina (traer un motor
  de 2 M de líneas con su propio sandbox) y la identidad del proyecto. Se acepta que
  el JS sea más lento. Se revisará si `boa` añade JIT.

### K6 — Memoria — P2 — 1 d

- Medir RSS por pestaña con 10 pestañas abiertas. Liberar el árbol de layout y la
  captura de las pestañas en segundo plano (hoy se conservan: `server.rs:789`
  "repintar conservando todo su estado").

---

## 13. Orden recomendado y dependencias

```
Semana 1   A1 A2 A3 A4 A5 ──► B1 B2 B3 F7          (repo veraz y protegido)
Semana 2   C1 C2 ──► C3 C4 C5                       (medir; base de prototipos)
Semana 3   C6 C7 C9 ──► primer bundle en verde       (vanilla-esm, react-vite)
Semana 4   C8 C11 K1 K2 ──► vue-vite en verde
Semana 5   D1 D2 D3 D4 E3 ──► svelte/next en verde; C13
Semana 6   I1 I2 I3 ──► Copiloto con Ollama y AOM
Semana 7   E1 E2 D5 D7 D8 D9
Semana 8   H1 H2 H3 ──► primera cifra WPT en el README
Semana 9   F1 F2 F5 ──► proceso por pestaña
Semana 10  F3 (Windows) G1 (iframe)
Semana 11  I5 I6 J1 J2
Semana 12  C10 D13 K3 K4
Después    G3 G4 G5 G9 G10 D6 D10 D11 D12 E4-E7 F3(otros SO) F6 H4 I4 I7 I8 J3 K6
Largo      G2 G6 G7 F4 D12(P3) G8 G11 G12
```

Dependencias duras:
- **B1 antes que todo lo demás**: sin CI, cada bloque puede romper el anterior.
- **C3 antes que C6/C8**: los métodos nuevos deben colgar de prototipos reales o
  habrá que reescribirlos.
- **C5 (`structuredClone`) antes que G4/G5** (IndexedDB y Workers lo usan para pasar datos).
- **C9 (módulos) antes que C13**: ningún bundle moderno pasa sin módulos.
- **F2 antes que F3 y G1 (si se quiere aislado)**.
- **E1 antes que G6**.
- **H1 antes que H2**.
- **I1 antes que I2** (no tiene sentido abstraer proveedores en dos sitios).

---

## 14. Lo que NO se va a hacer

Decisiones explícitas, para que nadie las reabra sin motivo:

| No se hace | Por qué |
|---|---|
| Sustituir `boa` por V8 / JIT propio | Rompe la doctrina de dependencias y la identidad del proyecto; el coste de compatibilidad no está en el JIT |
| Fallback a Chromium/WebView cuando el motor falla | El producto es el motor. Si falla, se ve que falla |
| Stubs de APIs que "no rompen" (observadores que nunca disparan, `play()` que devuelve `Promise` resuelta sin reproducir) | Peor que un `TypeError`: el bundle espera para siempre |
| Imitar el `User-Agent` de Chrome | Las páginas usarían APIs que no existen y fallarían más lejos y peor |
| DNS propio, TLS propio, decodificadores propios de imagen/vídeo | Doctrina: lo resuelto no se reescribe |
| WebGL antes de que el resto del bloque C esté cerrado | 10+ días para algo que casi ninguna web necesita para verse |
| Versión móvil | Sin Electron; sería otro producto |
| `window.open` / ventanas emergentes | Fuente de abusos; se lanza error y se documenta |
| Guardar claves de API en `localStorage` del renderer | Se migra a `safeStorage` (I2) |

---

## 15. Checklist maestro

Marcar aquí al cerrar. Cada ✅ debe tener su Fase en `ARCHITECTURE.md`.

### A — Higiene
- [ ] A1 PR de `fix/paginas-vacias-y-rendimiento` fusionado en `main` — **pendiente de tu visto bueno** (requiere `git push`, acción externa)
- [x] A2 README sin datos falsos (location/MutationObserver, cifra de tests, SOP)
- [x] A3 `engine/huecos_sin_resolver.md` creado y enlazado, con cada entrada verificada por `grep` contra el código
- [x] A4 Cifra de tests reconciliada: eran **805**, no 703; hoy **819** con los nuevos
- [~] A5 Árbol limpio — **reevaluada como innecesaria**: todo lo señalado (`.exe`, `__pycache__`, `backend/build`, `backend/dist`, `.venv`) ya está en `.gitignore`, así que no ensucia el repo. Borrar el `.venv` de 281 MB obligaría a reinstalar sin ganancia, y el destino del backend lo decide I1

### B — CI
- [x] B1 `engine.yml` (build + tests + clippy `-D warnings` + WPT + `cargo audit`). **`fmt` NO se exige**, ver la cabecera del workflow. *Branch protection* queda por activar en GitHub (ajuste del repo, no del código)
- [x] B2 `app.yml` (tipos + build de la interfaz + `electron-builder --dir` + lint). **Lint arreglado y bloqueante** (Fase 46): TypeScript bajado a 5.9 para recuperar `typescript-eslint`, y trinquete que falla si los hallazgos suben (hoy 20)
- [x] B3 `wpt_runner` en CI — los 24 tests estilo-WPT pasan
- [x] B4 Test de humo NDJSON: 14 tests contra el binario real, cubren las 16 variantes más entrada inválida, con guardia anti-regresión de cobertura

### C — Web moderna
- [x] C1 Sonda v2 versionada con **114 comprobaciones**; cifra inicial **48/114**, hoy **56/114**. Test `api_probe` impide que baje y que se borren comprobaciones
- [ ] C2 Corpus de 5 bundles con tests `#[ignore]`
- [x] C3 Cadena de prototipos DOM real + constructores globales (Fase 44). `instanceof` y polyfills funcionan. **Salvedad declarada**: los métodos siguen en la instancia, así que un envoltorio sobre un método que el motor ya tiene no llega a ejecutarse
- [x] C4 `Event`, `CustomEvent`, `KeyboardEvent`, `MouseEvent`, `InputEvent`, `FocusEvent` con sus campos (Fase 45). Falta `EventTarget` construible y que el teclado/ratón reales rellenen los metadatos
- [~] C5 **hecho**: `console`, `URL`, `URLSearchParams`, `performance.now`, `atob`/`btoa`, `TextEncoder`/`TextDecoder` (Fase 42). **Pendientes a propósito**: `AbortController` (hasta que cancele el `fetch` de verdad), `crypto` (necesita aleatoriedad real), `structuredClone`
- [~] C6 **parcialmente** (Fases 44-45): `matches`, `closest`, `contains`, `remove`, `append`, `prepend`, `cloneNode`, `dataset`, `innerHTML` real, `outerHTML`, `isConnected`, `id`, `className`, `readyState`, `activeElement`, `currentScript`, `getElementsByClassName`, `createDocumentFragment`, `createComment`, `hasFocus`. **Pendientes**: `write`, `insertAdjacentHTML`, `innerText`, `focus`, `scrollIntoView`, `offset*`, `client*`
- [~] C7 **mayoritariamente** (Fase 45): `innerWidth`/`innerHeight`, `devicePixelRatio`, `scrollX`/`scrollY`, `matchMedia` con el evaluador real de `@media`. **Pendientes**: `scrollTo` que mueva de verdad, diálogos vía protocolo
- [ ] C8 `IntersectionObserver` / `ResizeObserver` reales
- [~] C9 **mayoritariamente hecho** (Fase 43): `type="module"` en línea y externo, `import`/`export` reales, `defer`/`async` con el orden del spec, `nomodule` omitido, `<link rel="modulepreload">` descargado. **Pendientes**: `import()` dinámico, import maps, y resolución relativa al módulo importador (hoy se resuelve contra la página)
- [ ] C10 Scripts ejecutados durante el parseo (streaming)
- [ ] C11 Formularios desde JS + `FormData` + `SubmitForm` en protocolo
- [ ] C12 Selección de texto y portapapeles
- [ ] C13 Los 5 bundles en verde; 10 webs reales verificadas con captura

### D — CSS/layout
- [ ] D1 `::before`/`::after` + `content`
- [ ] D2 `linear-gradient`/`radial-gradient`
- [ ] D3 `background-size/position/repeat` + capas
- [ ] D4 `calc()`/`min()`/`max()`/`clamp()` evaluados en layout
- [ ] D5 `transform` 2D + hit-testing
- [ ] D6 `transition`/`animation` + bucle de fotogramas
- [ ] D7 `opacity`/`visibility`/`z-index` con contextos de apilamiento
- [ ] D8 Listas con marcadores
- [ ] D9 `@font-face` (woff2), `white-space`, `text-overflow`, `line-height`, shaping
- [ ] D10 Grid completo (rows, auto-flow, repeat/minmax, span, alineación)
- [ ] D11 Flexbox completo (order, align-self, wrap multilínea, gap)
- [ ] D12 Pseudo-clases restantes, `outline`, `cursor`, `pointer-events`, `object-fit`, `aspect-ratio`, `@layer`
- [ ] D13 Reflow incremental (<50 ms por mutación en 10k nodos)

### E — Red
- [ ] E1 Caché HTTP (memoria + disco, validación condicional)
- [ ] E2 HTTP/2 por ALPN
- [ ] E3 `@import`
- [ ] E4 `preload`/`modulepreload`/favicon
- [ ] E5 Parseo progresivo por chunks
- [ ] E6 `Referrer-Policy`, HSTS, proxy del sistema
- [ ] E7 Prefijos y `Partitioned` en cookies

### F — Seguridad
- [ ] F1 `engine/SECURITY.md` con modelo de amenazas
- [ ] F2 Broker + proceso por pestaña
- [ ] F3 Sandbox del SO (Windows; después Linux/macOS)
- [ ] F4 Site isolation (largo plazo)
- [ ] F5 Endurecer protocolo NDJSON (límites, `tab_id` no adivinable)
- [ ] F6 Fuzzing de decodificadores y parsers propios
- [x] F7 `cargo audit` en CI (job `audit` de `engine.yml`). `cargo deny` y `npm audit` siguen pendientes

### G — Plataforma
- [ ] G1 `<iframe>` + `postMessage` + `sandbox`
- [ ] G2 `<video>`/`<audio>` (delegado a Electron en fase 1)
- [ ] G3 WebSockets
- [ ] G4 IndexedDB
- [ ] G5 Web Workers
- [ ] G6 Service Workers
- [ ] G7 WebGL (`null` honesto hasta entonces)
- [ ] G8 APIs de dispositivo denegadas explícitamente
- [ ] G9 `<dialog>`, `<details>`, `<progress>`, inputs date/color/range/file
- [ ] G10 `Blob`/`File`/`FileReader`/`ReadableStream`/descargas
- [ ] G11 Impresión/PDF
- [ ] G12 Namespaces SVG/MathML en el DOM

### H — WPT
- [ ] H1 `testharness.js` oficial cargando (o arnés completo en Rust)
- [ ] H2 Corpus WPT vendorizado + `--expectations` + timeout + JSON; primera cifra: ___/___
- [ ] H3 Informe por directorio en `ARCHITECTURE.md`
- [ ] H4 Tests de captura visual

### I — Producto
- [ ] I1 Backend Python eliminado (o justificado y reducido)
- [ ] I2 Proveedores LLM: Gemini, Anthropic, OpenAI-compatible, Ollama (PCCOM); clave en `safeStorage`
- [ ] I3 Agente sobre AOM con ids estables y confirmación antes de submit/dominio nuevo
- [ ] I4 Voz → Faster-Whisper (PCCOM)
- [ ] I5 UI de navegador completa (favicon, progreso, atajos, zoom, marcadores, descargas, candado TLS, devtools mínimo)
- [ ] I6 Release en GitHub + auto-update funcionando + versión `0.x` + CHANGELOG (+ firma, decisión de compra)
- [ ] I7 Telemetría opt-in de fallos
- [ ] I8 CONTRIBUTING, protocolo documentado, plantilla de PR

### J — Multiplataforma
- [ ] J1 Linux y macOS en la matriz de CI
- [ ] J2 Descubrimiento de fuentes por plataforma
- [ ] J3 Empaquetado dmg/AppImage/deb (+ notarización, decisión de compra)

### K — Rendimiento
- [ ] K1 `cargo bench` con `criterion` + `BENCHMARKS.md`
- [ ] K2 Flamegraph y lista de puntos calientes
- [ ] K3 Cachés de medición de texto y estilos
- [ ] K4 Pintado incremental / memoria compartida con Electron
- [ ] K6 Liberar memoria de pestañas en segundo plano

---

*Última revisión: 2026-09-09. Al cerrar cualquier tarea, actualizar la fecha y la
sección 1.2 de este fichero.*
