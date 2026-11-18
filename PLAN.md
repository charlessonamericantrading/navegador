# Plan para que el navegador funcione bien

Revisión hecha el 18-nov-2026 sobre el commit `5de0863`. Todo lo que dice este
documento se comprobó **ejecutando** el motor y la app Electron, no leyendo
código. Los números son del build `--release`.

---

## 1. Resumen (para decidir)

**Lo que sí funciona hoy**

- El motor compila y sus 685 tests pasan.
- La app Electron arranca, habla con el motor y navega páginas simples
  (example.com, info.cern.ch) en menos de 1 segundo. Enlaces, atrás/adelante
  y pestañas funcionan.

**Por qué "no funciona bien" (5 causas, por orden de daño)**

1. **No se puede instalar desde cero.** `npm install` falla en `frontend/`
   y ningún script compila el motor Rust.
2. **El motor se cuelga con páginas reales.** Wikipedia tarda **50 s** en
   cargar y **49 s en cada clic**. La interfaz se rinde a los 12 s.
3. **El último commit rompió el layout.** En Wikipedia el texto ya no hace
   salto de línea. El commit anterior lo pintaba bien.
4. **Falta CSS básico** que usa toda web: centrar con `margin: auto`,
   `var()`, `%`, `em`, espacios entre palabras en negrita o enlaces.
5. **El agente IA y la escritura fallan tras hacer scroll**, y el modelo
   Gemini configurado está, casi seguro, retirado.

**Lo que hay que saber antes de invertir más**

Un motor propio **no va a mostrar bien la web moderna** (YouTube, Gmail,
bancos, redes sociales) en meses. Eso lleva años a equipos grandes. Lo que
este plan sí consigue: que **instale, no se cuelgue nunca, y pinte bien y
rápido las webs de documento** (Wikipedia, noticias, blogs, buscadores
simples), con el agente IA funcionando sobre ellas.

**Dos decisiones tuyas**

| Decisión | Mi recomendación |
|---|---|
| ¿Añadir un "modo compatibilidad" que use el Chromium que Electron ya trae, para las webs que el motor propio no pueda mostrar? (Fase 5) | **Sí.** Es la única forma de tener un navegador usable a corto plazo. El motor propio sigue siendo el modo por defecto. |
| ¿Quitar el backend Python? Ya no se usa, pero el instalador lo sigue exigiendo. | **Sí.** Menos piezas, menos fallos. |

**Orden y tamaño** (estimación, no promesa)

| Fase | Qué consigue | Sesiones de Sonnet |
|---|---|---|
| 0 | Instala, arranca y se puede medir | 1 |
| 1 | Nunca se cuelga. Rápido | 3–4 |
| 2 | Las páginas se ven bien | 6–8 |
| 3 | Escribir, scroll y agente IA fiables | 3–4 |
| 4 | JavaScript mínimo de páginas reales | 5–7 |
| 5 | Modo compatibilidad (si lo apruebas) | 2–3 |
| 6 | Documentación honesta y CI | 1 |

Con las fases 0 a 3 terminadas el producto ya es usable y demostrable.

---

## 2. Medidas de partida

Build release, ventana 1280×800. Repetir con
`python3 scripts/dev/engine_driver.py <url>`.

| Página | Cargar | Scroll | Clic en zona vacía | Redimensionar |
|---|---|---|---|---|
| example.com | 0,4 s | 0,01 s | 0,01 s | — |
| info.cern.ch | 0,9 s | 0,03 s | 0,03 s | — |
| Hacker News | 1,7 s | 0,08 s | 0,15 s | — |
| Google (portada) | 0,8 s | 0,01 s | 0,26 s | — |
| DuckDuckGo HTML (resultados) | 2,5 s | 0,05 s | 0,29 s | — |
| **Wikipedia (artículo)** | **50 s** | 0,15 s | **49 s** | **33 s** |

Con el build *debug*: Hacker News 6,7 s y Wikipedia más de 180 s.

**Objetivo al acabar la Fase 1:** Wikipedia carga en menos de 3 s, clic en
menos de 0,3 s, redimensionar en menos de 1 s. Ninguna operación supera
nunca 10 s.

---

## 3. Reglas de trabajo para Sonnet

Estas reglas existen porque el historial muestra el patrón contrario:
commits enormes, afirmaciones sin comprobar y 685 tests en verde con el
producto roto.

1. **Una tarea por sesión.** En el orden de este documento. No empieces una
   tarea si la anterior no está verificada.
2. **Mide antes y después.** Ejecuta el comando de "Verificar" antes de
   tocar nada y otra vez al acabar. Pega los dos resultados en el mensaje
   del commit.
3. **No digas "hecho" sin la salida del comando.** Si la verificación
   falla, dilo tal cual.
4. **Tiempos siempre con `--release`.**
5. **Commits pequeños.** Un arreglo, un commit. Nada de mezclar
   funcionalidades nuevas.
6. **Cada arreglo lleva su test** en el crate que toca. Además
   `cargo test --workspace` debe seguir en verde.
7. **No añadas funcionalidades fuera de este plan.** Nada de WebGL, vídeo,
   GPU, ni fases nuevas del motor.
8. **No toques el README** hasta la Fase 6.
9. **No leas `engine/ARCHITECTURE.md` entero** (3.771 líneas). Busca con
   grep la parte que necesites.
10. **Al terminar una tarea**, marca su casilla `[x]` en este archivo.

**Aviso de entorno.** En una sesión en la nube el motor no tiene salida
directa a internet: cualquier `https://` falla con `client error (Connect)`.
Es el entorno, no un bug. Usa los fixtures locales o
`scripts/dev/site_mirror.py`. En el PC de Windows el motor sí llega a
internet.

**Herramientas ya incluidas**

```bash
# Compilar el motor
cargo build --release --manifest-path engine/Cargo.toml -p engine-core --bin engine_server

# Medir una página (tiempos + captura PNG)
python3 scripts/dev/engine_driver.py --fixture css-basics-fixture.html --out css.png
python3 scripts/dev/engine_driver.py --fixture js-basics-fixture.html --texts

# Solo en la nube: espejo local de un sitio real
python3 scripts/dev/site_mirror.py 9001=es.wikipedia.org 9003=news.ycombinator.com &
python3 scripts/dev/engine_driver.py http://127.0.0.1:9001/wiki/Navegador_web --out wiki.png
```

---

## Fase 0 — Que instale, arranque y se pueda medir

### [ ] 0.1 Arreglar `npm install` del frontend

- **Problema:** `frontend/package.json` pide `typescript ^7.0.2`, pero
  `typescript-eslint` exige `<6.1.0`. `npm install` y `npm ci` fallan con
  `ERESOLVE`. `npm run lint` revienta.
- **Hacer:** fijar `typescript` a `~6.0.3`. Regenerar `package-lock.json`.
  Si algún flag de `tsconfig.*.json` deja de ser válido, corregirlo.
- **Verificar:** en `frontend/`, sin `--force` ni `--legacy-peer-deps`:
  `rm -rf node_modules && npm ci && npm run build && npm run lint`. Los
  tres deben terminar sin error.

### [ ] 0.2 Compilar el motor dentro del flujo normal

- **Problema:** ni `instalar.*` ni `npm start` compilan el motor. Sin
  binario, la app muestra "No se ha podido iniciar el motor… reinstálala".
  Si solo existe el build *debug*, la app lo usa y va 5–10 veces más lenta.
- **Hacer:**
  - En `package.json` raíz: script `build:engine` con el comando de cargo
    de arriba, y `prestart` que lo ejecute.
  - `instalar.bat` e `instalar.sh`: comprobar que existe `cargo` y llamar
    a `npm run build:engine`.
  - `desktop/main.js` (`getNativeEnginePath`, `startNativeEngine`): si
    falta el binario, mensaje claro: "Falta compilar el motor: ejecuta
    `npm run build:engine`". Si se usa el build debug, avisar por consola.
- **Verificar:** borrar `engine/target`, ejecutar `npm start` y comprobar
  que la app abre y carga `http://info.cern.ch`.

### [ ] 0.3 Ver los avisos del motor

- **Problema:** `engine_server` nunca inicializa `tracing`. Todos los
  `tracing::warn!` del motor se pierden. Se depura a ciegas.
- **Hacer:** en `engine/crates/core/src/bin/engine_server.rs`, iniciar
  `tracing_subscriber` escribiendo a **stderr** (nunca a stdout, que es el
  canal NDJSON). Nivel por variable `ENGINE_LOG`, por defecto `warn`.
- **Verificar:** cargar una página con un `<img>` roto y ver el aviso en
  stderr. `stdout` sigue conteniendo solo JSON.

### [ ] 0.4 Sacar Python del camino (si se aprueba la decisión)

- **Hacer:** quitar la comprobación de Python de `instalar.*`, el paso
  PyInstaller de `scripts/build-app.js`, `backend-server` de
  `extraResources` en `desktop/package.json`, `startPythonBackend` de
  `desktop/main.js` y el modo WebSocket de `frontend/src/core/App.tsx`.
  Borrar `backend/` y `scripts/install-backend.js`.
- **Verificar:** `npm run install:all && npm start` funciona en una
  máquina sin Python.

### [ ] 0.5 Banco de páginas de referencia

- **Hacer:** script `scripts/dev/fetch-pages` que descargue a una carpeta
  ignorada por git (`engine/tests/pages-cache/`) el HTML y el CSS de:
  Wikipedia (`es.wikipedia.org/wiki/Navegador_web`), Hacker News,
  DuckDuckGo HTML (`html.duckduckgo.com/html/?q=rust`) y Google. No subir
  esas páginas al repo (son de terceros).
- Añadir script `scripts/dev/bench-all` que pase `engine_driver.py` por
  las cuatro páginas y los dos fixtures, e imprima una tabla como la de la
  sección 2. Debe salir con error si alguna operación supera su límite.
- **Verificar:** la tabla reproduce los números de la sección 2.

---

## Fase 1 — Que nunca se cuelgue y sea rápido

El perfil de Wikipedia es claro: **el 100 % del tiempo** está en
`engine_text::shape::measure_text`, llamado desde `measure_flex_item`
anidado 5–7 niveles.

### [ ] 1.1 No re-parsear la fuente en cada medición

- **Problema:** `SystemFont::rustybuzz_face()`
  (`engine/crates/text/src/font.rs`) crea un `rustybuzz::Face` nuevo en
  cada `measure_text`, `shape_text` y `baseline_offset`. Eso re-lee las
  tablas de la fuente miles de veces por página.
- **Hacer:** construir el `Face` **una sola vez** por fuente. El `FontSet`
  ya es global (`OnceLock`): guarda los bytes como `&'static [u8]`
  (`Box::leak`, son 4 fuentes para toda la vida del proceso) y junto a
  ellos un `rustybuzz::Face<'static>` ya construido. `SystemFont` pasa a
  ser un `Arc` barato de clonar.
- **Verificar:** `bench-all`. Anotar el nuevo tiempo de Wikipedia.

### [ ] 1.2 Caché de medidas de texto

- **Hacer:** caché `(fuente, tamaño, texto) → TextMetrics` en
  `engine-text`, con límite de entradas.
- **Verificar:** `bench-all` mejora o iguala; tests en verde.

### [ ] 1.3 Cortar el relayout exponencial en flex y grid

- **Problema:** `measure_flex_item` (`engine/crates/layout/src/tree.rs`)
  ejecuta `flow_block_children` completo cada vez que taffy pregunta. Con
  contenedores flex anidados, el trabajo se multiplica en cada nivel.
- **Hacer:** guardar en cada `LayoutBox`, durante una pasada de layout:
  (a) sus anchos intrínsecos mínimo y máximo, y (b) el alto ya calculado
  para cada ancho. Si llega la misma pregunta, devolver lo guardado sin
  recorrer hijos. Aplicar lo mismo en `flow_grid_children`.
- **Verificar:** Wikipedia carga en **menos de 3 s**.

### [ ] 1.4 No rehacer el layout si nada cambió

- **Problema:** cada clic, tecla y cambio de pestaña llama a
  `LoadedPage::relayout` (`engine/crates/core/src/server.rs`), que rehace
  cascada y layout enteros aunque el DOM no haya cambiado.
- **Hacer:** contador de mutaciones del DOM que suban los bindings JS que
  modifican nodos y las funciones del servidor que cambian `value` o
  `checked`. `relayout` solo si el contador cambió o cambió el tamaño.
- **Verificar:** clic en zona vacía de Wikipedia en **menos de 0,3 s**.

### [ ] 1.5 Límites al JavaScript

- **Problema comprobado:** una página con `<script>while(true){}</script>`
  cuelga el motor para siempre.
- **Hacer:** en `engine/crates/js/src/runtime.rs` activar los
  `RuntimeLimits` de Boa (iteraciones de bucle, recursión). Tope de tiempo
  total de scripts por página. En `scripting.rs::run_scripts`, ejecutar
  solo `<script>` sin `type` o con tipo JavaScript: saltar
  `application/json`, `application/ld+json`, `module`, `importmap` y
  plantillas.
- **Verificar:** test nuevo con el bucle infinito: el motor responde en
  menos de 5 s y la página se pinta igualmente.

### [ ] 1.6 Red con límites y en paralelo

- **Problema:** `NetworkEngine::fetch`
  (`engine/crates/net/src/http_client.rs`) no tiene ningún timeout. Los
  CSS, scripts e imágenes se bajan uno detrás de otro.
- **Hacer:** timeout de conexión (10 s) y total (20 s documento, 8 s
  subrecurso). Descargar subrecursos en paralelo (máximo 6 a la vez).
  Límite global de 10 s para toda la navegación: pasado ese tiempo, pintar
  con lo que haya. `User-Agent` de navegador real y cabecera `Accept`
  correcta por tipo de recurso.
- **Verificar:** test con un servidor local que nunca responde a una
  imagen: la página se pinta en menos de 10 s.

### [ ] 1.7 Interfaz que no se desincroniza

- **Problema comprobado:** tras el aviso de "tardando demasiado", la barra
  muestra la URL nueva con el contenido de la página anterior. Las
  respuestas atrasadas llegan después y pisan la pantalla.
- **Hacer** en `frontend/src/core/App.tsx`:
  - Numerar cada navegación. Ignorar cualquier `state` que no sea de la
    última.
  - No aplicar el estado dos veces (hoy llega por `onEngineState` **y**
    por la respuesta de `sendCommand`).
  - Scroll en Electron: agrupar los eventos de rueda mientras hay una
    petición en vuelo (la lógica ya existe para el modo WebSocket).
  - Mostrar una página de error dentro del visor cuando falle la carga,
    no solo un aviso flotante. Botón "Detener".
- **Verificar:** prueba Playwright contra Electron: navegar a una página
  lenta y luego a una rápida; al final la barra y el contenido coinciden.

### [ ] 1.8 Respuestas más ligeras

- **Problema:** cada respuesta `state` lleva la captura PNG y **todos** los
  elementos (1,8 MB en Wikipedia), incluso en cada tick de scroll.
- **Hacer:** en `scroll`, no reenviar `elements`. Etiquetar bien el tipo
  de imagen (el frontend declara `image/jpeg` y recibe PNG).
- **Verificar:** tamaño de la respuesta de `scroll` en Wikipedia por
  debajo de 600 KB.

---

## Fase 2 — Que las páginas se vean bien

Referencia de cada tarea: abrir `engine/tests/css-basics-fixture.html` en
Chrome y comparar con la captura del motor. El fixture numera 17 casos.
**Hoy fallan 15** (solo pasan el 15 y el 16).

### [ ] 2.0 Deshacer la regresión del último commit (lo primero)

- **Problema:** desde `5de0863`, en Wikipedia los párrafos no hacen salto
  de línea y se salen por la derecha. La caja flotante lateral desaparece.
  Con `fd399b2` se veía bien.
- **Causa:** en `measure_flex_item`, el caso
  `AvailableSpace::MinContent | MaxContent` devuelve lo mismo para los
  dos: el párrafo entero en una línea (`measure_intrinsic_width`). Como
  un item flex no puede encoger por debajo de su *min-content*, se queda
  tan ancho como su párrafo más largo.
- **Hacer:** `MinContent` debe devolver el ancho de la **palabra más
  larga**, no el del texto completo.
- **Verificar:** captura de Wikipedia a 1280 px: ningún texto pasa del
  borde derecho y la caja lateral vuelve a aparecer. Añadir test de
  layout que lo fije.

### [ ] 2.1 Espacios entre elementos en línea

- **Problema:** `Texto <b>negrita</b> <i>cursiva</i> <a>enlace</a>` se
  pinta "negritacursivaenlace". Afecta a casi todo párrafo de la web.
  También hay saltos de línea de más al acabar un `<b>` (visto en
  DuckDuckGo).
- **Verificar:** caso 12 del fixture.

### [ ] 2.2 `margin: auto` y márgenes abreviados

- **Problema:** `margin: 10px auto` no centra y además se pierden los
  10 px. Por eso ninguna web sale centrada.
- **Verificar:** casos 1, 5 y 6.

### [ ] 2.3 Unidades

- **Problema:** `parse_css_length` solo entiende `px` y `rem`.
- **Hacer:** `%`, `em`, `vh`, `vw`, `pt` y `calc()` con suma y resta.
- **Verificar:** casos 3, 4, 5 y 6.

### [ ] 2.4 Variables CSS `var(--x)`

- **Problema:** no existen. Wikipedia, Google y casi toda web moderna
  definen así sus colores: por eso los enlaces salen negros y la caja de
  búsqueda de Google sale negra.
- **Hacer:** resolver `var(--x, respaldo)` en la cascada. Las propiedades
  `--*` se heredan.
- **Verificar:** caso 2. Enlaces de Wikipedia en azul.

### [ ] 2.5 Descartar declaraciones no válidas

- **Problema:** un valor que el motor no entiende acaba pintado como negro
  o hace desaparecer la caja, en vez de ignorarse y dejar el valor
  anterior de la cascada.
- **Verificar:** caso 7: hoy la caja desaparece; debe quedar azul si no
  hay degradados.

### [ ] 2.6 Fondo de la ventana

- **Problema:** el lienzo es gris (245) y el fondo de `<body>` solo cubre
  el alto del contenido.
- **Hacer:** lienzo blanco por defecto. El fondo de `html`/`body` cubre
  toda la ventana. Ver `engine/crates/gfx/src/raster.rs`.
- **Verificar:** caso 17.

### [ ] 2.7 Texto: `line-height`, familias de fuente y `opacity`

- **Hacer:** aplicar `line-height`. Elegir fuente con serifa, sin serifa
  o monoespaciada según `font-family`. Aplicar `opacity`.
- **Verificar:** casos 8, 9, 10 y 11.

### [ ] 2.8 `inline-block` y cajas en línea de varias líneas

- **Problema:** un `inline-block` con ancho y alto no se pinta. Un enlace
  que ocupa varias líneas informa ancho 0, así que no se puede clicar.
- **Verificar:** caso 13. En DuckDuckGo ningún enlace con texto tiene
  ancho 0 en `elements`.

### [ ] 2.9 Tablas y atributos HTML clásicos

- **Problema:** las columnas se reparten a partes iguales en vez de según
  su contenido. Se ignoran `bgcolor`, `width`, `align`, `border`,
  `cellpadding`. Hacker News sale descuadrado y sin su barra naranja.
- **Verificar:** caso 14. Hacker News se parece al original.

### [ ] 2.10 Codificación de caracteres

- **Problema comprobado:** una página en ISO-8859-1 muestra "Espa�a".
- **Hacer:** decodificar según el `charset` de `Content-Type` o
  `<meta charset>` con el crate `encoding_rs`.
- **Verificar:** test con una página Latin-1.

### [ ] 2.11 Orden de las hojas de estilo

- **Problema:** `pipeline.rs` pone primero todos los `<style>` y después
  todos los CSS externos, sin respetar el orden del documento. No hay
  `@import`.
- **Verificar:** test donde un `<style>` posterior a un `<link>` gana.

### [ ] 2.12 Páginas de error del servidor

- **Problema:** un 404 devuelve error y pantalla vacía. Un navegador
  muestra la página de error que envía el servidor.
- **Verificar:** un 404 de Wikipedia se pinta.

---

## Fase 3 — Escribir, hacer scroll y agente IA

### [ ] 3.1 Coordenadas correctas tras hacer scroll

- **Problema comprobado:** `elements[].rect` viene en coordenadas de
  documento. El frontend y el agente las usan como coordenadas de
  ventana. Tras cualquier scroll, clicar en un campo no abre el cuadro de
  escritura y el agente clica en el sitio equivocado.
- **Hacer:** añadir al protocolo (`engine/crates/core/src/protocol.rs`)
  `click_element { element_id }` y
  `type_into_element { element_id, text, replace, press_enter }`. El
  motor hace scroll hasta el elemento y actúa en su centro. El agente usa
  solo estas dos. En el frontend, restar `scroll_offset_y` al buscar el
  campo bajo el clic (`BrowserViewport.tsx`, `handleImageClick`).
- **Verificar:** prueba Playwright: bajar al final de Hacker News, clicar
  el campo "Search" y escribir.

### [ ] 3.2 Escribir de verdad

- **Problema:** solo se escribe con un cuadro emergente que **añade**
  texto y **siempre** pulsa Enter. No se puede corregir ni borrar.
- **Hacer:** clic en un campo lo enfoca. Las teclas sobre el visor van al
  motor (`type_text` sin Enter, `press_key`). Dibujar foco y cursor.
- **Verificar:** escribir, borrar con Retroceso y enviar con Enter.

### [ ] 3.3 Lista de elementos útil

- **Problema:** `elements` incluye `html`, `body`, `div`, `option`,
  `span`… (796 en Hacker News, 1.658 en Wikipedia). Es ruido para la IA.
- **Hacer:** solo interactivos y visibles: `a[href]`, `button`, `input`,
  `select`, `textarea`, `[onclick]`, `[role=button]`, `[role=link]`.
- **Verificar:** Hacker News baja a menos de 300 elementos.

### [ ] 3.4 Agente IA

- **Problemas:**
  - El modelo fijado, `gemini-2.0-flash`, se retiró el 1-jun-2026 según
    varias fuentes. Confirmar en la página oficial de Google.
  - El árbol de accesibilidad (`get_accessibility_tree`, crate
    `engine-ai`) no está conectado al frontend. Y sus ids no coinciden
    con los de `elements`.
  - El agente no puede hacer scroll ni volver atrás.
  - La clave de API viaja en la URL y vive en `localStorage`.
- **Hacer:** modelo configurable en Ajustes, con uno vigente por defecto.
  Unificar ids entre árbol de accesibilidad y `elements`, y conectar
  `getAccessibilityPrompt` en `App.tsx`. Añadir acciones `scroll` y
  `back`. Enviar la clave por cabecera `x-goog-api-key` desde el proceso
  principal de Electron. Cortar si repite la misma acción tres veces.
  Etiquetar el modo "simulación" como demostración.
- **Verificar:** tarea real: "busca Navegador web en Wikipedia y dime la
  primera frase". Debe completarse.

---

## Fase 4 — JavaScript mínimo para páginas reales

Referencia: `python3 scripts/dev/engine_driver.py --fixture js-basics-fixture.html --texts`.
**Hoy 10 de 12 comprobaciones fallan.** Meta: las 12 en "OK".

### [ ] 4.1 Globales que todo script da por hechos

`console` (a stderr), `navigator`, `location` (leer `href` y navegar al
asignarlo), `window.innerWidth/innerHeight`, `requestAnimationFrame`,
`matchMedia`. Hoy faltan todos: casi cualquier script real revienta en
su primera línea.

### [ ] 4.2 Eventos de carga

`document.addEventListener`, `DOMContentLoaded`, `load` y
`document.readyState`. Hoy no se disparan nunca, y la mayoría de webs
arrancan ahí.

### [ ] 4.3 DOM que falta

`getElementsByTagName`, `getElementsByClassName`, `parentNode`,
`childNodes`, `firstChild`, `nextSibling`, `createTextNode`, `cloneNode`,
`innerHTML` real (lectura y escritura), y las propiedades `id`,
`className`, `value`, `href`, `src`, `checked`, `dataset`.

### [ ] 4.4 Observadores

`MutationObserver`, `IntersectionObserver` y `ResizeObserver`, aunque
sean versiones mínimas, para que los scripts no fallen al crearlos.

### [ ] 4.5 Tiempo en segundo plano

- **Problema:** los temporizadores solo avanzan cuando llega una petición
  del usuario. Una página que se actualiza sola no cambia en pantalla.
- **Hacer:** tic periódico en `run_stdio`. Si un temporizador cambió el
  DOM, enviar un `state` sin `id` (Electron ya reenvía esos eventos).

**Expectativa honesta:** esto hace funcionar menús, pestañas y webs con
poco JavaScript. **No** hace funcionar aplicaciones web completas.

---

## Fase 5 — Modo compatibilidad (solo si se aprueba)

- Botón en la barra: "Abrir en modo compatibilidad".
- Usa un `WebContentsView` de Electron (el Chromium que la app ya incluye)
  en lugar de la captura del motor propio.
- Aislado: sin Node, `contextIsolation`, sesión separada.
- El agente IA obtiene los elementos con un script inyectado que devuelve
  el mismo formato que `elements`.
- El README debe decir con claridad que este modo usa Chromium.

---

## Fase 6 — Documentación honesta y CI

### [ ] 6.1 README

Hoy se contradice: dice a la vez que hay y que no hay política de mismo
origen, canvas y SVG; "9 crates" y "10 crates"; "683 tests" y "676".
Reescribirlo con lo que este plan haya dejado verificado.

### [ ] 6.2 Estado corto del motor

Crear `engine/ESTADO.md` (máximo 150 líneas): qué funciona, qué no, y
cómo medirlo. `ARCHITECTURE.md` queda como histórico.

### [ ] 6.3 CI

GitHub Action en cada PR: `cargo test --workspace`, build y lint del
frontend, y `bench-all` con los fixtures locales.

---

## Otros hallazgos (menores)

- `README` afirma "Compilado y probado solo en Windows": el motor compila
  y pasa sus tests también en Linux.
- La página de inicio carga fuentes de Google Fonts: sin internet la
  interfaz cambia de tipografía. Empaquetarlas.
- `desktop/main.js` espera 3 s fijos antes de cargar Vite. Mejor
  reintentar hasta que responda.
- `npm audit` avisa de 17 vulnerabilidades en dependencias del frontend.
- No hay sandbox de proceso ni límite de memoria. El motor interpreta
  contenido de internet: mantener el aviso de "solo sitios de confianza".

---

## Cómo lanzar cada sesión de Sonnet

Copiar y pegar, cambiando el número de tarea:

> Lee `PLAN.md`. Ejecuta solo la tarea **0.1**. Cumple las "Reglas de
> trabajo". Al terminar: pega la salida de "Verificar", marca la casilla
> en `PLAN.md`, haz commit y push.
