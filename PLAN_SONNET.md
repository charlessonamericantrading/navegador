# Plan para que el navegador funcione bien

Revisión del 18-nov-2026. Todo lo que dice este documento se comprobó
**ejecutando** el motor y la app de escritorio antigua, no leyendo documentación. Los
números son del build `--release`.

Se revisaron las dos líneas de trabajo que existen:

- `main` (commit `5de0863`, 22-ago).
- La rama `fix/paginas-vacias-y-rendimiento` (commit `81094d3`, 23-sep),
  que es el **PR #1, abierto y sin fusionar**.

---

## Reglas absolutas del proyecto

Antes que todo lo demás (detalle en `AGENTS.md`):

1. La aplicación es **100 % Rust**: motor, ventana, interfaz, agente e instalador.
2. **Prohibido** usar, instalar, sugerir o nombrar cualquier motor de navegador
   ajeno, también para pruebas o como "modo de compatibilidad".
3. **Prohibido** cualquier envoltorio de escritorio basado en tecnología web.
4. `desktop/`, `frontend/` y `backend/` incumplen estas reglas y se retiran
   (Fase 5). Lo que este plan hizo en ellos es solo especificación.

## 1. Resumen (para decidir)

### Hallazgo 1 — El trabajo bueno no está en `main`

El PR #1 lleva **47 commits** parados desde el 23 de septiembre. Tiene el
CI en verde y no tiene conflictos. Mientras no se fusione, quien compile
`main` ve el producto roto.

| | `main` | PR #1 |
|---|---|---|
| `npm install` del frontend | **Falla** | Funciona |
| Wikipedia: cargar | **50 s** | 6,8 s |
| Wikipedia: cada clic | **49 s** | 0,56 s |
| Wikipedia: texto con salto de línea | **No** (regresión) | Sí |
| Prueba de CSS básico (17 casos) | 2 bien | 4 bien |
| Prueba de JavaScript básico (12) | 2 bien | 10 bien |
| Lint del frontend | Roto | En verde |
| Compilar exige Python | Sí | No |

### Hallazgo 2 — El plan actual apunta demasiado lejos

El plan maestro del PR #1 (`plan.md`) tiene **44 fases** y su objetivo es
"superar a los navegadores grandes". Buena parte de las últimas fases se fue a broker de
procesos, auditorías de dependencias y modelo de amenazas. Es trabajo
serio, pero **no cambia lo que ves en pantalla**. Lo básico visible sigue
fallando.

### Hallazgo 3 — Lo que sigue fallando en la rama buena

1. **Se cuelga para siempre** con una página que tenga un bucle infinito
   de JavaScript. La red no tiene límites de tiempo.
2. **No centra nada.** `margin: auto` no funciona. Por eso Google y casi
   toda web salen pegadas a la izquierda.
3. **Se come los espacios** entre palabras en negrita, cursiva o enlace.
4. **Tras hacer scroll no se puede escribir** en los campos, y el agente
   IA clica en el sitio equivocado.
5. **El agente IA casi seguro no funciona:** el modelo Gemini que usa
   figura como retirado desde junio de 2026.

### Lo que hay que saber antes de invertir más

Un motor propio **no va a mostrar bien las aplicaciones web pesadas**
(YouTube, Gmail, bancos, redes sociales) en meses. Eso lleva años a
equipos grandes. Lo que este plan sí consigue: que **no se cuelgue nunca
y pinte bien y rápido las webs de documento** (Wikipedia, noticias,
blogs, buscadores simples), con el agente IA funcionando sobre ellas.

### Tres decisiones tuyas

| Decisión | Mi recomendación |
|---|---|
| ¿Fusionar el PR #1 en `main`? | **Sí, hoy.** Es el paso con más efecto y menos riesgo. |
| ¿Congelar el plan de 44 fases y seguir este, corto? | **Sí.** Primero que funcione lo que se ve. |
| ¿Cómo se hace la interfaz 100 % Rust? (Fase 5) | Ver la Fase 5. Cualquier opción que dependa de un motor de navegador ajeno queda **descartada** por las reglas del proyecto (`AGENTS.md`). |

### Orden y tamaño (estimación, no promesa)

| Fase | Qué consigue | Sesiones de Sonnet |
|---|---|---|
| 0 | Una sola versión del código, y medible | 1 |
| 1 | Nunca se cuelga. Rápido | 3–4 |
| 2 | Las páginas se ven bien | 5–6 |
| 3 | Escribir, scroll y agente IA fiables | 3–4 |
| 4 | JavaScript que falta | 1–2 |
| 5 | Aplicación 100 % Rust: ventana, interfaz y agente | 6–8 |
| 6 | Documentación al día | 1 |

Con las fases 0 a 3 el producto ya es usable y demostrable.

---

## 2. Medidas de partida

Build release, ventana 1280×800. Tiempos en segundos.

| Página | Cargar `main` | Cargar PR #1 | Clic `main` | Clic PR #1 | Scroll PR #1 |
|---|---|---|---|---|---|
| example.com | 0,4 | 0,4 | 0,01 | 0,01 | 0,01 |
| info.cern.ch | 0,9 | 0,7 | 0,03 | 0,01 | 0,02 |
| Hacker News | 1,7 | 1,6 | 0,15 | 0,06 | 0,06 |
| Google (portada) | 0,8 | 1,3 | 0,26 | 0,02 | 0,02 |
| DuckDuckGo HTML | 2,5 | **4,7** | 0,29 | 0,04 | 0,06 |
| Wikipedia (artículo) | **50** | **6,8** | **49** | 0,56 | **0,58** |

"Clic" es un clic en una zona vacía: mide cuánto tarda el motor en
reaccionar a cualquier cosa.

**Objetivo al acabar la Fase 1:** Wikipedia carga en menos de 3 s, clic y
scroll en menos de 0,2 s. Ninguna operación supera nunca 10 s.

### Lo que no se pudo comprobar

- **Internet directo.** El entorno de revisión no deja salir al motor a
  internet. Se usaron copias en vivo servidas por un espejo local. No se
  probó HTTPS real, ni inicio de sesión en sitios reales.
- **Imágenes de otros dominios.** Por el mismo motivo no cargaron. Las
  capturas sin imágenes no cuentan como fallo del motor.
- **Windows, el instalador y el agente con una clave real de Gemini.**

---

## 3. Reglas de trabajo para Sonnet

Estas reglas existen porque el historial muestra el patrón contrario:
commits enormes, afirmaciones sin comprobar, y cientos de tests en verde
con el producto roto a la vista.

1. **Una tarea por sesión.** En el orden de este documento. No empieces
   una tarea si la anterior no está verificada.
2. **Mide antes y después.** Ejecuta el comando de "Verificar" antes de
   tocar nada y otra vez al acabar. Pega los dos resultados en el mensaje
   del commit.
3. **No digas "hecho" sin la salida del comando.** Si la verificación
   falla, dilo tal cual.
4. **Tiempos siempre con `--release`.**
5. **Commits pequeños.** Un arreglo, un commit.
6. **Cada arreglo lleva su test** en el crate que toca. El CI debe seguir
   en verde.
7. **Sigue este plan, no `plan.md`.** Nada de broker, sandbox, WPT
   oficial, workers, WebGL, vídeo, extensiones ni fases nuevas.
8. **No leas `engine/ARCHITECTURE.md` entero** (más de 5.000 líneas).
   Busca con grep la parte que necesites.
9. **Al terminar una tarea**, marca su casilla `[x]` en este archivo.
10. **Los tests no dependen de la velocidad ni del reloj.** El CI del motor
    corre en Windows con build de depuración, que es 3 a 10 veces más lento
    que tu máquina. Un test que tarda 5 s en Linux falla ahí (pasó con la
    tarea 1.1). Fija los límites por parámetro, no por tiempo.
11. **Antes de subir**, ejecuta lo mismo que el CI, en `engine/`:
    `cargo test --workspace --locked`,
    `cargo clippy --workspace --all-targets --locked -- -D warnings` y
    `cargo run -p engine-core --bin wpt_runner --locked -- tests/wpt-style`.
    En `desktop/` y `frontend/`: `npm test` y `npm run lint`.

**Aviso de entorno.** En una sesión en la nube el motor no tiene salida
directa a internet: cualquier `https://` falla con `client error (Connect)`.
Es el entorno, no un bug. Usa los fixtures locales o
`scripts/dev/site_mirror.py`. En el PC de Windows el motor sí llega a
internet.

**Herramientas de medición (incluidas en este PR)**

```bash
# Compilar el motor
cargo build --release --manifest-path engine/Cargo.toml -p engine-core --bin engine_server

# Medir una página: tiempos y captura PNG
python3 scripts/dev/engine_driver.py --fixture css-basics-fixture.html --out css.png
python3 scripts/dev/engine_driver.py --fixture js-basics-fixture.html --texts

# Solo en la nube: espejo local de un sitio real
python3 scripts/dev/site_mirror.py 9001=es.wikipedia.org 9003=news.ycombinator.com &
python3 scripts/dev/engine_driver.py http://127.0.0.1:9001/wiki/Navegador_web --out wiki.png
```

---

## Fase 0 — Una sola versión del código

> **Estado (19-nov):** el trabajo va apilado sobre el PR #1 en la rama
> `plan/fase-0-1-sobre-pr1` ([PR #3](https://github.com/charlessonamericantrading/navegador/pull/3),
> borrador, apunta al PR #1). **No se ha fusionado nada en `main`**: las
> tareas 0.1, 0.2 y 0.3 quedan pendientes hasta que Charlesson lo decida.

### [ ] 0.1 Fusionar el PR #1 en `main`

- **Quién:** Charlesson, o Sonnet si se le pide expresamente.
- **Por qué:** todo lo demás de este plan parte de esa rama.
- **Verificar:** en `main`, `npm run install:all` y `npm start` abren la
  app y cargan una página.

### [ ] 0.2 Fusionar este PR (plan y herramientas)

No toca el producto. Comprobado: no tiene conflictos con el PR #1.

### [ ] 0.3 Limpieza de ramas

- Borrar `feat/motor-web-y-seguridad`: ya está contenida en `main`.
- Mover `plan.md` a `docs/plans/plan-maestro-2026-09-21.md` y dejar una
  línea en su cabecera: "Congelado. El plan vigente es `PLAN_SONNET.md`".

### [x] 0.4 Banco de páginas visibles

- **Hacer:** script `scripts/dev/bench-all` que pase `engine_driver.py`
  por los dos fixtures y por cuatro páginas reales (Wikipedia, Hacker
  News, DuckDuckGo HTML, Google) e imprima una tabla como la de la
  sección 2. Debe salir con error si alguna operación supera su límite.
  Las páginas de terceros **no se suben al repo**: se descargan a una
  carpeta ignorada por git.
- **Verificar:** la tabla reproduce los números de la columna "PR #1".
- **Hecho (PR #3):** `scripts/dev/bench_all.py`. Carga las páginas en vivo
  (con `--mirror` en la nube) en lugar de guardarlas, así no hay nada de
  terceros que ignorar. `--strict` aplica los objetivos de la Fase 1.

### [x] 0.5 Un test que falla fuera de Windows

`desktop/tests/engine-protocol.test.js`, caso "lo que el parser de URL no
normaliza se rechaza": en Linux `app://./..%5C..%5Csecreto` no se
rechaza. En el CI pasa. Decidir si es un fallo real de la validación o
un test que solo vale para Windows, y dejarlo explícito.

- **Hecho (PR #3):** en Linux no era una fuga (la ruta se quedaba dentro
  de la carpeta), pero la regla cambiaba con el sistema. `resolveAppPath`
  rechaza ahora barra invertida y letra de unidad en todos. Test: 35 bien
  y 1 fallo → 36 bien y 0 fallos.

---

## Fase 1 — Que nunca se cuelgue y sea rápido

### [x] 1.1 Límites al JavaScript

- **Comprobado:** una página con `<script>while(true){}</script>` cuelga
  el motor para siempre. No responde a nada más.
- **Hacer:** en `engine/crates/js/src/runtime.rs`, activar los
  `RuntimeLimits` de Boa (iteraciones de bucle y recursión). Añadir un
  tope de tiempo total de scripts por página.
- **Verificar:** test nuevo con ese bucle: el motor responde en menos de
  5 s y pinta el resto de la página.
- **Hecho (PR #3):** límite de 10 millones de vueltas por llamada y tope
  de 5 s de scripts por página. Antes: 15 s sin responder. Después:
  **0,18 s**. Suite: 936 bien, 0 fallos; clippy limpio. El primer intento
  falló en el CI de Windows por un test dependiente de la velocidad;
  corregido (reglas 10 y 11).

### [x] 1.2 Red con límites

- **Comprobado en código:** `NetworkEngine::fetch`
  (`engine/crates/net/src/http_client.rs`) no tiene ningún timeout: un
  recurso que no responde bloquea la página entera. El `User-Agent` es
  `NextGenWebEngine/0.1.0`, y muchos sitios sirven otra página o bloquean
  a agentes desconocidos.
- **Hacer:** timeout de conexión (10 s) y total (20 s el documento, 8 s
  cada subrecurso). Límite global de 10 s por navegación: pasado ese
  tiempo, pintar con lo que haya. `User-Agent` de navegador real y
  `Accept` correcto por tipo.
- **Verificar:** test con un servidor local que nunca responde a una
  imagen: la página se pinta en menos de 10 s.
- **Hecho (PR #3):** conexión 10 s; petición entera 20 s (también para
  `fetch`/XHR de las páginas); 8 s por subrecurso y **10 s en total para
  los subrecursos, contados desde que llega el documento** (lo ya
  descargado se conserva). Antes: una página con recursos mudos no
  terminaba (45 s sin respuesta). Después: **10,0 s** con el estado
  completo. 943 tests bien, clippy limpio. Por tanto una navegación puede
  tardar hasta lo que tarde el documento (máx. 20 s) más 10 s.
- **No hecho:** `User-Agent` de navegador real y `Accept` por tipo. No se
  puede comprobar en la nube (el motor no sale a internet y el espejo
  cambia el agente). Pasa a la 1.2b.

### [ ] 1.2b `User-Agent` y `Accept` (necesita un PC con internet)

- **Hipótesis, sin comprobar:** algunos sitios sirven otra página o
  bloquean a un agente desconocido (`NextGenWebEngine/0.1.0`). Hoy todas
  las peticiones llevan `Accept: text/html,...`, también las imágenes.
- **Hacer:** primero medir. En un PC con internet, cargar Google,
  Wikipedia, DuckDuckGo y Hacker News con el agente actual y con uno
  compatible, y comparar el HTML recibido y la captura. Solo cambiarlo si
  la diferencia es visible. Si se cambia, `navigator.userAgent` de JS debe
  dar el mismo valor.
- **Verificar:** capturas antes y después de las cuatro páginas.

### [x] 1.3 Interfaz que no se desincroniza

- **Comprobado en `main`:** si una página tarda más de 12 s sale el aviso
  "tardando demasiado", y después la barra muestra la URL nueva con el
  contenido de la página anterior. Las respuestas atrasadas llegan luego
  y pisan la pantalla. El límite de 12 s sigue en el `App.tsx` del PR #1.
- **Hacer:** numerar cada navegación e ignorar cualquier `state` que no
  sea de la última. Mostrar una página de error dentro del visor cuando
  falle la carga, no solo un aviso flotante. Botón "Detener".
- **Verificar:** prueba automática de la interfaz: navegar a una página
  lenta y después a una rápida. Al final, barra y contenido coinciden.

- **Hecho (PR #3):** cada petición lleva un número (`NavigationSequence`) y su respuesta solo se aplica si ninguna navegación nueva ni Detener empezó después; de `engine:state` solo se recogen las publicaciones espontáneas (`id` nulo). Se puede pedir otra página con una carga en curso. Botón Detener (sustituye a Recargar mientras carga) y la barra vuelve a la página que se ve. Un fallo de carga se pinta dentro del visor (Reintentar / Volver atrás). Motor: petición `stop` y `navigate` interrumpible (un `stop` o un `navigate` nuevo abandonan el anterior, que se contesta con `navegacion_cancelada`).
- **Verificado en la app de escritorio antigua:** lenta → rápida: gana la última, barra y motor coinciden; Detener una carga muda: 0,6 s y el motor sigue vivo; servidor caído: página de error en el visor. Tests: 6 de la numeración, 5 del motor interrumpible, 2 de humo por NDJSON.

### [ ] 1.4 Rendimiento que falta

- **Medido:** Wikipedia 6,8 s al cargar y **0,58 s por cada paso de
  scroll**. DuckDuckGo 4,7 s (en `main` eran 2,5 s: ha empeorado).
- **Hacer:** perfilar primero (en `main` el 100 % del tiempo estaba en
  `engine_text::shape::measure_text` bajo `measure_flex_item`). Averiguar
  por qué el scroll tarda tanto: debería ser solo repintar.
- **Verificar:** Wikipedia carga en menos de 3 s y el scroll baja de
  0,2 s.

- **Hecho en parte (PR #3):** clic en Wikipedia **0,56 s → 0,04 s** (el layout solo se rehace si el clic pudo cambiar algo); scroll 0,03-0,05 s; redimensionar 0,38 s. Reparto de la carga en la nube: documento + CSS + JS 0,8 s, imágenes hasta 5,4 s, parseo + JS + cascada + layout **0,55 s**, captura 0,04 s. Lo que domina es la red de las imágenes: ahora tienen su propio tope de 4 s y se piden 12 a la vez (antes 6).
- **Falta:** el objetivo "Wikipedia carga en menos de 3 s" **no se puede comprobar desde la nube**: aquí las imágenes de otros dominios no tienen salida a internet y agotan su tope. Medir en un PC con internet: `python3 scripts/dev/bench_all.py --strict --only wikipedia`. Si sigue por encima de 3 s, el siguiente paso es la cascada (258 ms por relayout, recorre 1.297 reglas por cada caja).

### [x] 1.5 Respuestas más ligeras

- **Medido:** cada respuesta lleva la captura y **todos** los elementos:
  1,4 MB en Wikipedia, también en cada paso de scroll.
- **Hacer:** en `scroll`, no reenviar los elementos.
- **Verificar:** la respuesta de `scroll` en Wikipedia baja de 600 KB.

- **Hecho (PR #3):** la respuesta a `scroll` ya no lleva `elements` ni `requires_javascript` (el frontend conserva los anteriores). Wikipedia: **531 KB** por paso de scroll (casi todo es la captura) y 0,03-0,05 s. Y la lista `elements` completa también bajó con la 3.3.

### [x] 1.6 Codificación de caracteres

- **Comprobado:** una página en ISO-8859-1 muestra "Espa�a".
- **Hacer:** decodificar según el `charset` de `Content-Type` o de
  `<meta charset>`, con el crate `encoding_rs`.
- **Verificar:** test con una página Latin-1.

- **Hecho (PR #3):** `decode_text` (BOM → `charset` de `Content-Type` → `<meta charset>` en los primeros 1.024 bytes → UTF-8 si es válido, si no Windows-1252) con `encoding_rs`. 9 tests, incluida una página Latin-1 con acentos y ñ.

---

## Fase 2 — Que las páginas se vean bien

Referencia: `engine/tests/css-basics-fixture.html` tiene 17 casos numerados;
cada uno dice en su texto cómo debe verse en la captura del motor.
**En el PR #1 pasan 4** (el 4, 15, 16 y 17). Los 13 que fallan salen de
9 causas.

### [x] 2.1 `margin: auto` no centra

- **Casos 1, 2, 5 y 6.** Es el fallo más visible: ninguna web sale
  centrada. Explica buena parte del aspecto de Google y de Hacker News.

- **Hecho (PR #3):** `margin: auto` centra cajas con ancho (casos 1, 2, 5 y 6 del fixture).

### [x] 2.2 Espacios entre elementos en línea

- **Caso 12.** `Texto <b>negrita</b> <i>cursiva</i> <a>enlace</a>` se
  pinta "negritacursivaenlace".
- Relacionado: salto de línea de más al acabar un elemento en línea. En
  Wikipedia sale "(del inglés *web browser*" y en la línea siguiente
  ") es un software".

- **Hecho (PR #3):** los espacios entre elementos en línea se conservan (caso 12); al principio y al final de línea no ocupan nada. **Y el texto que sigue a un elemento en línea se parte por palabras** (`wrap_text_from`, con ancho propio para la primera línea): "(del inglés *web browser*" ya no rompe la línea. Tres fallos de colocación corregidos de paso: cada párrafo partido en varias líneas medía una línea de más (88 en vez de 70,4); lo que seguía a un texto largo empezaba en la línea de abajo; y la caja de un enlace que saltaba de línea empezaba una línea más arriba que su texto. Un texto que salta de línea pierde su espacio inicial (guardado aparte, porque la misma caja se coloca varias veces con anchos distintos). Verificado con Wikipedia: los párrafos se leen como en un navegador.

### [x] 2.3 `inline-block` con ancho y alto

- **Caso 13.** Tres cuadrados de 100×30 no se pintan.
- Comprobar de paso: en `main`, un enlace que ocupaba varias líneas
  informaba ancho 0 y no se podía clicar.

- **Hecho (PR #3):** `display:inline-block` con `width`/`height` es una caja atómica (tamaño, padding, borde y margen; salta de línea si no cabe; su texto se reparte dentro de su ancho). Caso 13: los tres cuadrados rojos de 100×30 en fila. De paso, un enlace cuyo texto ocupa varias líneas informaba ancho 0 y no se podía clicar: su caja es ahora la unión de lo que contiene. 4 tests. Simplificación: se alinea por arriba, no por la línea de base.

### [x] 2.4 `calc()` con porcentaje

- **Caso 3.** `calc(100% - 200px)` ocupa todo el ancho.

- **Hecho (PR #3):** `calc()` con porcentaje se evalúa contra el bloque contenedor (caso 3).

### [x] 2.5 Un valor no entendido hace desaparecer la caja

- **Caso 7.** Con `background: linear-gradient(...)` la caja desaparece.
  Debe ignorarse esa declaración y quedar el valor anterior (azul).
- Misma familia: la caja de búsqueda de Google sale **negra**.

- **Hecho (PR #3):** un valor que el motor no entiende ya no hace desaparecer la caja. Era la causa de la caja negra de Google (nombres de variables CSS pasados a minúsculas y `rgba(31, 31, 31, 0.08)` partido por espacios).

### [x] 2.6 `line-height` y `opacity`

- **Casos 8 y 9.** No se aplican.

- **Hecho (PR #3):** `line-height` (px, em, número, %) y `opacity` (casos 8 y 9).

### [x] 2.7 Familias de fuente

- **Casos 10 y 11.** Todo sale en la misma fuente sin serifa. Elegir
  serifa, sin serifa o monoespaciada según `font-family`.

- **Hecho (PR #3):** `FontFamily::from_css` reduce la lista a serifa, monoespaciada o sans-serif (el primer nombre reconocido manda) y `FontSet` carga las tres con sus cuatro variantes en **un solo** escaneo de fuentes (antes cuatro, solo para sans-serif). Layout y pintado usan la misma cara. Casos 10 y 11. Sin la familia instalada, cae en sans-serif. Nombres de Windows/macOS primero (Georgia, Consolas, Menlo…) y los de Linux de respaldo. **Sin comprobar en Windows.**

### [x] 2.8 Atributos HTML clásicos

- **Caso 14.** Se ignoran `bgcolor`, `width`, `border`, `align` y
  `cellpadding`. Por eso Hacker News no tiene su barra naranja ni su
  ancho del 85 %.

- **Hecho (PR #3):** `bgcolor`, `width`, `height`, `border`, `cellpadding`, `cellspacing`, `align`, `<font color/size/face>` y `<body text>` se traducen a CSS de especificidad cero, entre el agente de usuario y el autor (módulo `engine-css::presentational`). Una tabla dentro de `<center>` se centra. Caso 14: tabla amarilla con borde y mitad de ancho. **Hacker News recupera su barra naranja y su ancho del 85 % centrado.**

### [x] 2.9 Fondo de la ventana

- El lienzo es gris (245) cuando la página no declara fondo. Se ve en
  Google y en Hacker News. Debe ser blanco. Ver
  `engine/crates/gfx/src/raster.rs`.

- **Hecho (PR #3):** lienzo blanco (`DEFAULT_CANVAS_COLOR`) en PNG y ventana nativa.

### Vistos en `main` y sin comprobar en el PR #1

Comprobar antes de trabajar en ellos:

- Un error 404 dejaba la pantalla vacía en vez de mostrar la página de
  error del servidor.
- Los `<style>` se aplicaban todos antes que los CSS externos, sin
  respetar el orden del documento.

**Comprobados y resueltos (PR #3):** los dos eran reales. Un 404/500 con
página HTML se muestra ahora (solo es error sin página si el cuerpo no es
HTML con texto); y `pipeline::assemble_css` coloca cada `<style>` y cada
hoja externa en su sitio del documento (las externas viajan marcadas con su
`href`). También salió y se resolvió: **degradados `linear-gradient`** (caso 7
del fixture, ya se ven los 17 casos).

---

## Fase 3 — Escribir, hacer scroll y agente IA

### [x] 3.1 Coordenadas correctas tras hacer scroll

- **Comprobado en la app de escritorio antigua del PR #1:** bajar al final de Hacker
  News y clicar el campo "Search" no abre el cuadro de escritura.
- **Causa:** `elements[].rect` viene en coordenadas de documento
  (`engine/crates/core/src/server.rs`, `collect_elements_recursive`). El
  frontend y el agente las usan como coordenadas de ventana.
- **Hacer:** añadir al protocolo `click_element { element_id }` y
  `type_into_element { element_id, text, replace, press_enter }`. El
  motor hace scroll hasta el elemento y actúa en su centro. El agente usa
  solo estas dos. En `BrowserViewport.tsx` (`handleImageClick`), restar
  `scroll_offset_y` al buscar el campo bajo el clic.
- **Verificar:** prueba automática que repita el caso de Hacker News y
  escriba en el campo.

- **Hecho (PR #3):** `click_element` y `type_into_element` (el motor lleva el elemento a la vista y actúa en su centro; `replace` evita concatenar). `BrowserViewport` resta el scroll al buscar el campo y al colocar el cuadro de escritura; el agente usa solo las dos nuevas. **Verificado en la app de escritorio antigua** con el caso de Hacker News: scroll de 980 px, clic en "Search" → aparece el cuadro de escritura en su sitio y al enviar el motor intenta navegar con el texto (falla por falta de internet en la nube, como se espera).

### [x] 3.2 Escribir de verdad

- **Comprobado en código:** a mano solo se escribe con un cuadro
  emergente que **añade** texto y **siempre** pulsa Enter
  (`App.tsx`, `handleManualType`). No se puede corregir ni borrar.
- **Hacer:** clic en un campo lo enfoca. Las teclas sobre el visor van
  al motor. Dibujar foco y cursor.
- **Verificar:** escribir, borrar con Retroceso y enviar con Enter.

- **Hecho (PR #3):** se escribe con el teclado directamente en el campo y el cuadro emergente desaparece. Clicar un campo de texto le da el teclado (el motor lo enfoca y lo dibuja con anillo azul y cursor); `press_key` con un carácter escribe (acentos, espacio incluidos), Retroceso/Supr borran, Enter envía el formulario (o añade una línea en un `<textarea>`), Escape suelta el campo; clicar fuera o navegar le quita el teclado. Si la página no escucha teclas, solo se repinta el texto del campo (no el layout entero: eran 0,4 s por letra en Wikipedia). **Verificado en la app de escritorio antigua** (Hacker News, campo Search tras bajar al final): "rust" → "rust", Retroceso → "rus", "y" → "rusy". Un fallo real salió de paso: una franja de 1 px con antialias hacía abortar a tiny-skia en debug. **Falta:** selección, flechas, Tab entre campos, copiar y pegar.

### [x] 3.3 Lista de elementos útil

- **Medido:** `elements` trae 799 entradas en Hacker News y 1.307 en
  Wikipedia. Incluye `html`, `body`, `div`, `span`, `option`. Es ruido
  para la IA y peso en cada respuesta.
- **Hacer:** solo interactivos y visibles: `a[href]`, `button`, `input`,
  `select`, `textarea`, `[onclick]`, `[role=button]`, `[role=link]`.
- **Verificar:** Hacker News baja de 300 elementos.

- **Hecho (PR #3):** `elements` solo trae enlaces con `href`, controles de formulario y lo que la página declara accionable (`role`, `onclick`, `tabindex` ≥ 0, `contenteditable`), con área visible; texto colapsado y recortado a 120 caracteres; tope de 500. **Hacker News: 229 elementos** (antes 799) y Google 13. Los tests que leían texto desde `elements` leen ahora del árbol de accesibilidad.

### [ ] 3.4 Agente IA

- **Modelo retirado.** `desktop/ai-provider.js` llama a
  `gemini-2.0-flash`, retirado el 1-jun-2026 según varias fuentes.
  Confirmar el sustituto en la página oficial de Google. Hacer el modelo
  configurable en Ajustes.
- **Árbol de accesibilidad sin conectar.** El motor lo ofrece
  (`get_accessibility_tree`), pero `App.tsx` no lo pasa al agente. En
  `main` sus ids no coincidían con los de `elements`: unificarlos.
- **Faltan acciones.** El agente no puede hacer scroll ni volver atrás.
- **Verificar:** tarea real: "busca Navegador web en Wikipedia y dime la
  primera frase". Debe completarse.

- **Hecho en parte (PR #3):**
  - Modelo configurable: variable `NAVEGADOR_GEMINI_MODEL` > ajuste en la barra del agente > valor por defecto `gemini-3.5-flash`. El nombre se valida porque acaba en la URL. **Hay que confirmar el valor por defecto:** las páginas oficiales de Google no coinciden sobre el sustituto de `gemini-2.0-flash` (hablan de 3.5, 3.6 y 3.8); mirar https://ai.google.dev/gemini-api/docs/deprecations y, si es otro, ponerlo en Ajustes.
  - Una sola numeración: el prompt usa la lista `elements`, cuyos ids son los que entienden `click_element`/`type_into_element` (el árbol de accesibilidad numeraba aparte y no coincidía).
  - El agente lee: petición `get_page_text` (texto legible, bloques en líneas, tope de 12.000 caracteres, prefiere `<main>`). En Wikipedia la primera frase del artículo aparece en el carácter 736.
  - Acciones nuevas: `scroll` y `back`.
- **Falta:** la verificación real ("busca Navegador web en Wikipedia y dime la primera frase") necesita una clave de Gemini y un PC con internet; en la nube solo se probó con modelos falsos y la cañería (texto de página y ajuste del modelo).

---

## Fase 4 — JavaScript que falta

Referencia:
`python3 scripts/dev/engine_driver.py --fixture js-basics-fixture.html --texts`.
**En el PR #1 pasan 10 de 12.**

### [x] 4.1 `document.getElementsByTagName`

No existe. Es de las llamadas más usadas de la web.

- **Hecho (PR #3):** `document.getElementsByTagName` y `element.getElementsByTagName`, además de `getElementsByClassName`. Fixture JS: 12/12.

### [x] 4.2 Evento `load` de `window`

No se dispara nunca. Muchas webs arrancan ahí.

- **Hecho (PR #3):** el evento `load` se dispara sobre `<html>` tras `DOMContentLoaded` y llama a `window.onload`. Fixture JS: 12/12 (antes 10/12).

### [ ] 4.3 Sonda de APIs

El PR #1 trae su propia sonda (`engine/tests/probes/api-probe.html`,
87 de 115). Subirla solo con lo que rompa páginas del banco de la
tarea 0.4. No perseguir el número por sí mismo.

---

## Fase 5 — Aplicación 100 % Rust

**Regla del proyecto (`AGENTS.md`):** la aplicación es 100 % Rust y está
prohibido usar cualquier motor de navegador ajeno o un envoltorio de escritorio
basado en tecnología web. La etapa anterior (`desktop/`, `frontend/`,
`backend/`) incumplía la regla y **ya se retiró** (5.6). La aplicación vive en
`engine/crates/app` (binario `navegador`).

### [x] 5.1 Binario `navegador`

- Ventana nativa (`winit` + `softbuffer`). El motor va en otro proceso del
  mismo ejecutable (`navegador --motor`, NDJSON) y la red en el broker
  (`navegador --broker`): si una página tumba el motor, la ventana sigue.
- **Hecho:** abre páginas reales; `navegador --captura v.png <url>` guarda la
  ventana entera sin pantalla.

### [x] 5.2 Barra del navegador en Rust

- **Decisión: la pinta nuestro propio motor.** Es HTML/CSS que pasa por el
  mismo motor que las páginas; los iconos son SVG. Las zonas pulsables salen
  del layout (`data-accion`).
- Dirección (con cursor, selección, flechas, Ctrl+A), atrás/adelante,
  recargar/Detener, pestañas (nueva, cambiar, cerrar), página de inicio y de
  error con «Reintentar».
- **Hecho:** 32 tests de Rust conducen la barra y comprueban el estado.

### [x] 5.3 Ratón y teclado

- Clic, rueda (una sola petición de scroll en vuelo), teclas al campo
  enfocado, Av Pág/Re Pág/espacio/flechas, atajos (Ctrl+L/T/W/Tab/R, F5,
  Alt+←/→, Escape). Pantallas con escala: la imagen se amplía (pendiente:
  pintar a la escala real para que el texto se vea nítido).
- **Hecho:** probado en una ventana real con ratón y teclado virtuales.

### [ ] 5.4 Agente IA en Rust

- Llamada al modelo con `hyper` + `rustls` (ya están en el motor), clave en el
  almacén de credenciales del sistema (crate `keyring`), modelo configurable.
- Mismo bucle que hoy: `get_page_text`, `elements`, `click_element`,
  `type_into_element`, `scroll`, `back`. Portar los tests del orquestador.

### [ ] 5.5 Instalador en Rust

- `cargo-wix` (Windows) / `cargo-bundle`. Firma de código igual que hoy.

### [x] 5.6 Retirar lo antiguo

- **Hecho:** fuera `desktop/`, `frontend/`, `backend/`, sus scripts y su CI.
  README y SECURITY reescritos; `instalar`/`iniciar` solo necesitan Rust.
  Documentos y comentarios dejan de nombrar otros navegadores.

### [~] 5.7 Pruebas de punta a punta sin navegador externo

- **Hecho:** `engine/crates/app/tests/extremo_a_extremo.rs` conduce la app con
  el motor real (dirección, enlace, atrás, formulario, pestañas, error) y una
  prueba de humo en CI guarda la ventana sin pantalla.
- Falta: comparar capturas contra referencias guardadas.

### [ ] 5.8 (Opcional) Herramientas de medida en Rust

- `bench_all.py`, `engine_driver.py` y `site_mirror.py` son Python. No forman
  parte de la aplicación, pero pueden pasar a un binario `bench` de Rust.

---

## Fase 6 — Documentación al día

### [x] 6.1 README

Reescribirlo con lo que este plan deje verificado. El de `main` se
contradice (dice a la vez que hay y que no hay canvas, SVG y política de
mismo origen).

- **Hecho (PR #3):** cifras al día (1.052 tests), lo nuevo que funciona, tabla de rendimiento medida y los límites del agente tal como son hoy. Apunta a `engine/ESTADO.md` como punto de entrada. No se reescribió entero: lo que no se pudo comprobar (sandbox, plataformas, sonda de APIs) se deja como estaba.

### [x] 6.2 Estado corto del motor

Crear `engine/ESTADO.md` (máximo 150 líneas): qué funciona, qué no y
cómo medirlo. `ARCHITECTURE.md` queda como histórico.

- **Hecho (PR #3):** `engine/ESTADO.md` (111 líneas): qué funciona, qué no (con el motivo), cómo medirlo, los números y dónde mirar si algo falla.

### [x] 6.3 Banco visible en el CI

Añadir `bench-all` (tarea 0.4) con los dos fixtures locales a los
workflows que ya existen en el PR #1.

- **Hecho (PR #3):** job `bench` en `engine.yml` (Linux): compila el motor en release y pasa los dos fixtures por `bench_all.py`. Falla si algo se cuelga o si el fixture de JS baja de 12/12. **Sin comprobar en GitHub todavía:** es la primera vez que ese job corre.

---

## Qué NO hacer ahora

No aportan a que una página se vea bien hoy:

- Broker de procesos, sandbox y aislamiento (fases 64–67 del PR #1).
- WPT oficial y Test262.
- Workers, WebGL, audio, vídeo, extensiones, sincronización y móvil.

Se retoman cuando las fases 0 a 3 estén cerradas.

---

## Cómo lanzar cada sesión de Sonnet

Copiar y pegar, cambiando el número de tarea:

> Lee `PLAN_SONNET.md`. Ejecuta solo la tarea **1.1**. Cumple las "Reglas
> de trabajo". Al terminar: pega la salida de "Verificar", marca la
> casilla en `PLAN_SONNET.md`, haz commit y push.
