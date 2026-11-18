# Plan para que el navegador funcione bien

Revisión del 18-nov-2026. Todo lo que dice este documento se comprobó
**ejecutando** el motor y la app Electron, no leyendo documentación. Los
números son del build `--release`.

Se revisaron las dos líneas de trabajo que existen:

- `main` (commit `5de0863`, 22-ago).
- La rama `fix/paginas-vacias-y-rendimiento` (commit `81094d3`, 23-sep),
  que es el **PR #1, abierto y sin fusionar**.

---

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
"superar a Chrome". Buena parte de las últimas fases se fue a broker de
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
| ¿Añadir un "modo compatibilidad" con el Chromium que Electron ya trae, para las webs que el motor propio no pueda mostrar? (Fase 5) | **Sí.** Es la única forma de tener un navegador usable a corto plazo. El motor propio sigue por defecto. |

### Orden y tamaño (estimación, no promesa)

| Fase | Qué consigue | Sesiones de Sonnet |
|---|---|---|
| 0 | Una sola versión del código, y medible | 1 |
| 1 | Nunca se cuelga. Rápido | 3–4 |
| 2 | Las páginas se ven bien | 5–6 |
| 3 | Escribir, scroll y agente IA fiables | 3–4 |
| 4 | JavaScript que falta | 1–2 |
| 5 | Modo compatibilidad (si lo apruebas) | 2–3 |
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

### [ ] 0.4 Banco de páginas visibles

- **Hacer:** script `scripts/dev/bench-all` que pase `engine_driver.py`
  por los dos fixtures y por cuatro páginas reales (Wikipedia, Hacker
  News, DuckDuckGo HTML, Google) e imprima una tabla como la de la
  sección 2. Debe salir con error si alguna operación supera su límite.
  Las páginas de terceros **no se suben al repo**: se descargan a una
  carpeta ignorada por git.
- **Verificar:** la tabla reproduce los números de la columna "PR #1".

### [ ] 0.5 Un test que falla fuera de Windows

`desktop/tests/engine-protocol.test.js`, caso "lo que el parser de URL no
normaliza se rechaza": en Linux `app://./..%5C..%5Csecreto` no se
rechaza. En el CI pasa. Decidir si es un fallo real de la validación o
un test que solo vale para Windows, y dejarlo explícito.

---

## Fase 1 — Que nunca se cuelgue y sea rápido

### [ ] 1.1 Límites al JavaScript

- **Comprobado:** una página con `<script>while(true){}</script>` cuelga
  el motor para siempre. No responde a nada más.
- **Hacer:** en `engine/crates/js/src/runtime.rs`, activar los
  `RuntimeLimits` de Boa (iteraciones de bucle y recursión). Añadir un
  tope de tiempo total de scripts por página.
- **Verificar:** test nuevo con ese bucle: el motor responde en menos de
  5 s y pinta el resto de la página.

### [ ] 1.2 Red con límites

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

### [ ] 1.3 Interfaz que no se desincroniza

- **Comprobado en `main`:** si una página tarda más de 12 s sale el aviso
  "tardando demasiado", y después la barra muestra la URL nueva con el
  contenido de la página anterior. Las respuestas atrasadas llegan luego
  y pisan la pantalla. El límite de 12 s sigue en el `App.tsx` del PR #1.
- **Hacer:** numerar cada navegación e ignorar cualquier `state` que no
  sea de la última. Mostrar una página de error dentro del visor cuando
  falle la carga, no solo un aviso flotante. Botón "Detener".
- **Verificar:** prueba Playwright contra Electron: navegar a una página
  lenta y después a una rápida. Al final, barra y contenido coinciden.

### [ ] 1.4 Rendimiento que falta

- **Medido:** Wikipedia 6,8 s al cargar y **0,58 s por cada paso de
  scroll**. DuckDuckGo 4,7 s (en `main` eran 2,5 s: ha empeorado).
- **Hacer:** perfilar primero (en `main` el 100 % del tiempo estaba en
  `engine_text::shape::measure_text` bajo `measure_flex_item`). Averiguar
  por qué el scroll tarda tanto: debería ser solo repintar.
- **Verificar:** Wikipedia carga en menos de 3 s y el scroll baja de
  0,2 s.

### [ ] 1.5 Respuestas más ligeras

- **Medido:** cada respuesta lleva la captura y **todos** los elementos:
  1,4 MB en Wikipedia, también en cada paso de scroll.
- **Hacer:** en `scroll`, no reenviar los elementos.
- **Verificar:** la respuesta de `scroll` en Wikipedia baja de 600 KB.

### [ ] 1.6 Codificación de caracteres

- **Comprobado:** una página en ISO-8859-1 muestra "Espa�a".
- **Hacer:** decodificar según el `charset` de `Content-Type` o de
  `<meta charset>`, con el crate `encoding_rs`.
- **Verificar:** test con una página Latin-1.

---

## Fase 2 — Que las páginas se vean bien

Referencia: abrir `engine/tests/css-basics-fixture.html` en Chrome y
compararlo con la captura del motor. Tiene 17 casos numerados.
**En el PR #1 pasan 4** (el 4, 15, 16 y 17). Los 13 que fallan salen de
9 causas.

### [ ] 2.1 `margin: auto` no centra

- **Casos 1, 2, 5 y 6.** Es el fallo más visible: ninguna web sale
  centrada. Explica buena parte del aspecto de Google y de Hacker News.

### [ ] 2.2 Espacios entre elementos en línea

- **Caso 12.** `Texto <b>negrita</b> <i>cursiva</i> <a>enlace</a>` se
  pinta "negritacursivaenlace".
- Relacionado: salto de línea de más al acabar un elemento en línea. En
  Wikipedia sale "(del inglés *web browser*" y en la línea siguiente
  ") es un software".

### [ ] 2.3 `inline-block` con ancho y alto

- **Caso 13.** Tres cuadrados de 100×30 no se pintan.
- Comprobar de paso: en `main`, un enlace que ocupaba varias líneas
  informaba ancho 0 y no se podía clicar.

### [ ] 2.4 `calc()` con porcentaje

- **Caso 3.** `calc(100% - 200px)` ocupa todo el ancho.

### [ ] 2.5 Un valor no entendido hace desaparecer la caja

- **Caso 7.** Con `background: linear-gradient(...)` la caja desaparece.
  Debe ignorarse esa declaración y quedar el valor anterior (azul).
- Misma familia: la caja de búsqueda de Google sale **negra**.

### [ ] 2.6 `line-height` y `opacity`

- **Casos 8 y 9.** No se aplican.

### [ ] 2.7 Familias de fuente

- **Casos 10 y 11.** Todo sale en la misma fuente sin serifa. Elegir
  serifa, sin serifa o monoespaciada según `font-family`.

### [ ] 2.8 Atributos HTML clásicos

- **Caso 14.** Se ignoran `bgcolor`, `width`, `border`, `align` y
  `cellpadding`. Por eso Hacker News no tiene su barra naranja ni su
  ancho del 85 %.

### [ ] 2.9 Fondo de la ventana

- El lienzo es gris (245) cuando la página no declara fondo. Se ve en
  Google y en Hacker News. Debe ser blanco. Ver
  `engine/crates/gfx/src/raster.rs`.

### Vistos en `main` y sin comprobar en el PR #1

Comprobar antes de trabajar en ellos:

- Un error 404 dejaba la pantalla vacía en vez de mostrar la página de
  error del servidor.
- Los `<style>` se aplicaban todos antes que los CSS externos, sin
  respetar el orden del documento.

---

## Fase 3 — Escribir, hacer scroll y agente IA

### [ ] 3.1 Coordenadas correctas tras hacer scroll

- **Comprobado en la app Electron del PR #1:** bajar al final de Hacker
  News y clicar el campo "Search" no abre el cuadro de escritura.
- **Causa:** `elements[].rect` viene en coordenadas de documento
  (`engine/crates/core/src/server.rs`, `collect_elements_recursive`). El
  frontend y el agente las usan como coordenadas de ventana.
- **Hacer:** añadir al protocolo `click_element { element_id }` y
  `type_into_element { element_id, text, replace, press_enter }`. El
  motor hace scroll hasta el elemento y actúa en su centro. El agente usa
  solo estas dos. En `BrowserViewport.tsx` (`handleImageClick`), restar
  `scroll_offset_y` al buscar el campo bajo el clic.
- **Verificar:** prueba Playwright que repita el caso de Hacker News y
  escriba en el campo.

### [ ] 3.2 Escribir de verdad

- **Comprobado en código:** a mano solo se escribe con un cuadro
  emergente que **añade** texto y **siempre** pulsa Enter
  (`App.tsx`, `handleManualType`). No se puede corregir ni borrar.
- **Hacer:** clic en un campo lo enfoca. Las teclas sobre el visor van
  al motor. Dibujar foco y cursor.
- **Verificar:** escribir, borrar con Retroceso y enviar con Enter.

### [ ] 3.3 Lista de elementos útil

- **Medido:** `elements` trae 799 entradas en Hacker News y 1.307 en
  Wikipedia. Incluye `html`, `body`, `div`, `span`, `option`. Es ruido
  para la IA y peso en cada respuesta.
- **Hacer:** solo interactivos y visibles: `a[href]`, `button`, `input`,
  `select`, `textarea`, `[onclick]`, `[role=button]`, `[role=link]`.
- **Verificar:** Hacker News baja de 300 elementos.

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

---

## Fase 4 — JavaScript que falta

Referencia:
`python3 scripts/dev/engine_driver.py --fixture js-basics-fixture.html --texts`.
**En el PR #1 pasan 10 de 12.**

### [ ] 4.1 `document.getElementsByTagName`

No existe. Es de las llamadas más usadas de la web.

### [ ] 4.2 Evento `load` de `window`

No se dispara nunca. Muchas webs arrancan ahí.

### [ ] 4.3 Sonda de APIs

El PR #1 trae su propia sonda (`engine/tests/probes/api-probe.html`,
87 de 115). Subirla solo con lo que rompa páginas del banco de la
tarea 0.4. No perseguir el número por sí mismo.

---

## Fase 5 — Modo compatibilidad (solo si se aprueba)

- Botón en la barra: "Abrir en modo compatibilidad".
- Usa un `WebContentsView` de Electron (el Chromium que la app ya
  incluye) en lugar de la captura del motor propio.
- Aislado: sin Node, con `contextIsolation` y sesión separada.
- El agente IA obtiene los elementos con un script inyectado que
  devuelve el mismo formato que `elements`.
- El README debe decir con claridad que este modo usa Chromium.

---

## Fase 6 — Documentación al día

### [ ] 6.1 README

Reescribirlo con lo que este plan deje verificado. El de `main` se
contradice (dice a la vez que hay y que no hay canvas, SVG y política de
mismo origen).

### [ ] 6.2 Estado corto del motor

Crear `engine/ESTADO.md` (máximo 150 líneas): qué funciona, qué no y
cómo medirlo. `ARCHITECTURE.md` queda como histórico.

### [ ] 6.3 Banco visible en el CI

Añadir `bench-all` (tarea 0.4) con los dos fixtures locales a los
workflows que ya existen en el PR #1.

---

## Qué NO hacer ahora

No aportan a que una página se vea bien hoy:

- Broker de procesos, sandbox y aislamiento (fases 64–67 del PR #1).
- WPT oficial, Test262 y comparativas con Chrome.
- Workers, WebGL, audio, vídeo, extensiones, sincronización y móvil.

Se retoman cuando las fases 0 a 3 estén cerradas.

---

## Cómo lanzar cada sesión de Sonnet

Copiar y pegar, cambiando el número de tarea:

> Lee `PLAN_SONNET.md`. Ejecuta solo la tarea **1.1**. Cumple las "Reglas
> de trabajo". Al terminar: pega la salida de "Verificar", marca la
> casilla en `PLAN_SONNET.md`, haz commit y push.
