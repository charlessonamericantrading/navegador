# Estado del motor

Resumen corto y verificado de lo que el motor hace y no hace. Cada afirmación
sale de ejecutarlo, no de leer el código. `ARCHITECTURE.md` queda como
histórico: cuenta cómo se llegó aquí, no cómo está hoy.

Última comprobación: 2026-11-23, Linux (nube), motor en release, ventana de
1280×800. **Windows y macOS no se han vuelto a probar a mano**; en Windows solo
corre lo que cubre el CI (tests, clippy y la prueba de humo del instalador).

## Cómo medirlo (todo en un comando cada uno)

```bash
cd engine && cargo test --workspace --locked          # 1.052 tests
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p engine-core --bin wpt_runner --locked -- tests/wpt-style   # 60 de 60
cargo build --release -p engine-core --bin engine_server --bin engine_broker
python3 scripts/dev/bench_all.py                      # tiempos; --strict = objetivos de la Fase 1
python3 scripts/dev/bench_all.py --mirror             # sin internet directo (nube)
python3 scripts/dev/engine_driver.py --fixture css-basics-fixture.html --out css.png
```

Los fixtures (`engine/tests/css-basics-fixture.html` y `js-basics-fixture.html`)
se miran con el motor y con Chrome: tienen 17 y 12 casos numerados.

## Números

| Qué | Resultado |
|---|---|
| Fixture CSS | 17 de 17 casos se ven (el degradado del 7 incluido) |
| Fixture JS | 12 de 12 en OK |
| Hacker News | carga 1,0–1,3 s; scroll y clic 0,05 s; 229 elementos interactivos |
| Google | carga 1,3–1,9 s; 13 elementos |
| Wikipedia (artículo largo) | motor propio ~0,55 s; clic 0,04 s; scroll 0,03–0,05 s; redimensionar 0,4–0,6 s |
| Wikipedia, carga completa | **no medible en la nube**: las imágenes de otros dominios no salen a internet y agotan su tope de 4 s. Medir en un PC con internet; el objetivo es < 3 s |
| Página con `while(true){}` | responde en 0,18 s (antes: se colgaba) |
| Página con recursos que nunca contestan | termina en 10 s con lo que llegó (antes: > 45 s) |

## Qué funciona

- **Red:** HTTP/1.1 y HTTPS (`hyper` + `rustls`), redirecciones, gzip/deflate/
  brotli, cookies, CORS y CSP. Tiempos límite: conexión 10 s, petición 20 s,
  8 s por subrecurso, 10 s en total para subrecursos (las imágenes, 4 s).
  Páginas 404/500 con HTML se muestran. Codificación según `charset`/`<meta>`.
- **HTML/CSS:** cascada con especificidad, `@media`, variables (`var()`, con
  mayúsculas), `calc()` con %, `em`/`rem`/`vw`, `margin: auto`, `line-height`,
  `opacity`, `box-shadow`, `border-radius`, **degradados `linear-gradient`**,
  `font-family` serifa y monoespaciada, y los atributos clásicos de HTML
  (`bgcolor`, `width`, `border`, `cellpadding`, `align`, `<font>`). Los estilos
  se aplican en orden de documento.
- **Layout:** bloque, texto en línea con partición por palabras entre elementos
  (un enlace en mitad de un párrafo no rompe la línea), `inline-block` con
  tamaño, flex, grid, tablas, `float`, `position`.
- **Pintado:** lienzo blanco, imágenes (PNG, JPEG, GIF, WebP, SVG), anillo de
  foco y cursor en el campo con el teclado.
- **JavaScript (Boa):** `getElementsByTagName/ClassName`, eventos `DOMContentLoaded`
  y `load`, `fetch`, XHR, `localStorage`. Límite de 10 millones de vueltas por
  llamada y 5 s de scripts por página.
- **Interacción:** clic, scroll, escritura con teclas (letras, Retroceso, Enter),
  `click_element` y `type_into_element` por id (llevan el elemento a la vista),
  envío de formularios GET y POST, historial, pestañas.
- **Protocolo (NDJSON):** `navigate`, `stop`, `get_state`, `click`, `scroll`,
  `type_text`, `press_key`, `click_element`, `type_into_element`, `resize`,
  `get_accessibility_tree`, `get_page_text`, pestañas, `ping`. Una navegación en
  curso se abandona con `stop` o con un `navigate` nuevo.
- **Respuestas ligeras:** `scroll` no reenvía `elements`; `elements` solo trae
  lo interactivo y visible (tope 500, texto de 120 caracteres).

## Qué no funciona (conocido y medido)

- **JavaScript de aplicaciones modernas:** una web que construye su contenido
  en el cliente suele verse vacía (el motor lo avisa). La sonda de APIs
  (`engine/tests/probes/api-probe.html`) mide lo que falta.
- **Texto:** no hay selección, mover el cursor con flechas ni Tab entre campos;
  el cursor va siempre al final; un `<textarea>` guarda los saltos de línea pero
  los pinta en una sola línea; no hay copiar/pegar con el teclado.
- **Inline:** `inline-block` se alinea por arriba, no por la línea de base;
  `box-sizing` se trata como `content-box` ahí; `text-align` con un texto
  partido en varias líneas solo es exacto alineado a la izquierda.
- **Degradados:** solo `linear-gradient` (sin `radial`, `conic` ni `repeating`),
  una sola capa de fondo, y sin `background-size`/`position`/`repeat`.
- **Fuentes:** tres familias (sans, serifa, monoespaciada) con las fuentes del
  sistema; sin fuentes web. Si el sistema no tiene la familia, sale en sans.
- **Red:** sin HTTP/2 ni caché; el `User-Agent` es `NextGenWebEngine/0.1.0` y no
  se ha comprobado si cambiarlo mejora algo (tarea 1.2b).
- **Plataforma:** sin `<video>`, `<audio>`, `<iframe>`, WebGL, Workers, WebSockets.
- **Seguridad:** no hay sandbox del sistema operativo. Úsalo con sitios de confianza.
- **Cancelar una navegación** abandona la carga de red, pero un script que ya está
  corriendo no se interrumpe hasta su tope de 5 s.

## Dónde mirar si algo falla

| Síntoma | Primer sitio |
|---|---|
| Texto en lugares raros | `layout/src/tree.rs`, `place_inline_node` (rama `BoxType::Text`) |
| Algo no se ve | `gfx/src/display_list.rs` (qué se emite) y `gfx/src/paint.rs` (cómo se pinta) |
| Un estilo no se aplica | `css/src/cascade.rs` y `css/src/presentational.rs` |
| La página tarda | `RUST_LOG=info` imprime el tiempo de cada fase de `navigate` |
| Una petición del protocolo | `core/src/protocol.rs` y `core/src/server.rs`; el test de humo exige una prueba por petición |
| Una API de JS | `js/src/dom_bindings.rs` y la sonda de APIs |

## Reglas para tocar el motor

1. Un test no puede depender de la velocidad ni de las métricas de la fuente de
   la máquina: el CI corre en Windows y los anchos y alturas cambian. Medir con
   la fuente en el propio test.
2. Antes de subir: `cargo test --workspace --locked`, `cargo clippy ... -D warnings`
   y el `wpt_runner`. Si se toca la interfaz: `npm test`, `npm run lint` y
   `npx tsc -b` en `frontend/`; `npm test` en `desktop/`.
3. Un fichero nuevo del proceso principal de Electron va en `build.files` de
   `desktop/package.json` (un test lo comprueba).
