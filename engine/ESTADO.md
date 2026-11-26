# Estado del motor y de la aplicación

Resumen corto y verificado de lo que el motor hace y no hace. Cada afirmación
sale de ejecutarlo, no de leer el código. `ARCHITECTURE.md` queda como
histórico: cuenta cómo se llegó aquí, no cómo está hoy.

Última comprobación: 2026-11-26, Windows 11 (local, compilación en `--release` con
`--locked`), más suites de test y clippy del workspace en Windows y Linux.

## Cómo medirlo (todo en un comando cada uno)

```bash
cd engine && cargo test --workspace --locked          # 1.107 tests en total
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p engine-core --bin wpt_runner --locked -- tests/wpt-style   # 60 de 60
cargo build --release --locked -p navegador --bin navegador --bin instalador -p engine-core --bin engine_server --bin engine_broker
python scripts/dev/bench_all.py --strict
```

Los fixtures (`engine/tests/css-basics-fixture.html` y `js-basics-fixture.html`)
tienen 17 y 12 casos numerados; cada caso dice en su texto cómo debe verse.

## La aplicación (`navegador`, 100 % Rust)

```bash
cd engine && cargo run --release -p navegador                 # abre la ventana
cargo run --release -p navegador -- es.wikipedia.org          # con una dirección
cargo run --release -p navegador -- --captura v.png news.ycombinator.com   # sin ventana: guarda la imagen
```

- **Ventana** nativa con soporte **HiDPI** (`winit` + `softbuffer`). El motor
  corre en otro proceso (`navegador --motor`): en Windows se lanza aislado con
  token restringido de integridad baja, escritorio dedicado oculto, *Job Object*
  y perfil protegido frente a lectura/escritura desde integridad baja. La red,
  las cookies y el almacenamiento van en un tercero (`navegador --broker`), al
  que el motor se presenta por pipe autenticado con token de un solo uso.
- **La barra la pinta nuestro motor:** pestañas reordenables (arrastrar o
  `Ctrl+Shift+RePág/AvPág`) con desplazamiento `<` / `>` si superan el ancho,
  atrás/adelante, recargar/detener, estrella de marcador (`Ctrl+D`), barra de
  direcciones y desplegable de sugerencias (marcadores + historial persistente +
  accesos iniciales) en HTML/CSS pintado por el motor (`app/src/barra.rs`).
- **Edición, foco y selección:** cursor nativo en barra y campos web (`ArrowLeft`,
  `ArrowRight`, `Home`, `End`, `Delete`, `Backspace`, `Shift+*`), portapapeles
  del sistema (`Ctrl+C`/`X`/`V`/`A`), `Tab`/`Shift+Tab` entre campos, selección
  con ratón en páginas y campos, cursor del ratón dinámico (`Pointer`, `Text`,
  `Wait`) y estados CSS dinámicos (`:hover`, `:focus`, `:focus-visible`,
  `:focus-within`).
- **Atajos:** `Ctrl+L`, `Ctrl+T`, `Ctrl+W`, `Ctrl+Tab`, `Ctrl+Shift+RePág/AvPág`,
  `Ctrl+D`, `Ctrl+R`/`F5`, `Alt+←`/`Alt+→`, `Escape`. Rueda, `Av Pág`/`Re Pág`,
  espacio y flechas desplazan cuando no hay campo de texto activo.
- **Páginas propias e instalador:** inicio (con accesos y marcadores guardados),
  error (con «Reintentar») e instalador/desinstalador 100 % Rust (`instalador`,
  accesos `.lnk` por COM y Registro HKCU en Windows; empaquetado con
  `scripts/release/empaquetar_windows.ps1`).
- **Pruebas:** 39 tests unitarios en `navegador`, 3 en `instalador`, 1 de extremo
  a extremo con el motor real (`engine/crates/app/tests/extremo_a_extremo.rs`) y
  prueba de aislamiento real del sandbox Windows (`tests/sandbox_isolation.rs`).

Falta: el panel del agente IA (`5.4`), firma de binarios y sandbox de red / por
pestaña (AppContainer en Windows; sandbox de procesos en Linux y macOS).

## Números

| Qué | Resultado (Windows 11, release `--strict`) |
|---|---|
| `wpt_runner` (`tests/wpt-style`) | 60 de 60 en OK |
| `fixture-css` | carga 0,26 s; scroll 0,03 s; clic 0,02 s; resize 0,02 s (`ok`) |
| `fixture-js` | 12 de 12 en OK; carga 0,20 s; scroll 0,02 s; clic 0,01 s; resize 0,01 s (`ok`) |
| Hacker News | carga 4,78 s (`> 3 s`); scroll 0,14 s; clic 0,18 s; resize 0,13 s |
| Google | carga 4,21 s (`> 3 s`); scroll 0,03 s; clic 0,03 s; resize 0,02 s |
| Wikipedia (artículo largo) | carga 7,73 s (`> 3 s`); scroll 0,09 s; clic 0,08 s; resize 0,24 s |
| DuckDuckGo HTML | carga 9,32 s (`> 3 s`); scroll 0,08 s; clic 0,08 s; resize 0,10 s |
| Página con `while(true){}` | responde en ~0,18 s (límite de vueltas de script; no se cuelga) |
| Página con recursos que nunca contestan | termina con tope de subrecursos (8 s recurso / 10 s total; imágenes 4 s) |

*Nota sobre `bench_all.py --strict`: la interacción local (scroll ≤ 0,2 s, clic ≤ 0,3 s, resize ≤ 1 s) cumple en todas las páginas; la carga en vivo (`cargar`) supera el objetivo de 3 s en páginas externas al descargar subrecursos sin caché ni HTTP/2.*

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
- **Pintado:** lienzo blanco, escalado HiDPI nativo, imágenes (PNG, JPEG, GIF,
  WebP, SVG), `<textarea>` multilínea, resaltado de selección, anillo de foco y
  cursor en campos con el teclado.
- **JavaScript (Boa 0.22):** `getElementsByTagName/ClassName`, eventos
  `DOMContentLoaded` y `load`, `fetch`, XHR, `localStorage`. Límite de 10
  millones de vueltas por llamada y 5 s de scripts por página.
- **Interacción:** clic, hover dinámico, selección de texto por arrastre,
  scroll, edición completa con teclas (flechas, Inicio/Fin, Supr, Retroceso,
  `Tab`/`Shift+Tab`, `Ctrl+A/C/X/V`), `click_element` y `type_into_element` por
  id, envío de formularios GET y POST, historial, pestañas.
- **Protocolo (NDJSON):** `navigate`, `stop`, `get_state` (`focused_is_text`,
  `selected_text`), `click`, `hover`, `select_text`, `scroll`, `type_text`,
  `press_key`, `click_element`, `type_into_element`, `resize`,
  `get_accessibility_tree`, `get_page_text`, pestañas, `ping`, `shutdown`.

## Qué no funciona (conocido y medido)

- **JavaScript de aplicaciones modernas:** una web que construye su contenido
  en el cliente suele verse vacía (el motor lo avisa). La sonda de APIs
  (`engine/tests/probes/api-probe.html`) mide lo que falta.
- **Estilos dinámicos parciales:** `:hover`, `:focus`, `:focus-visible` y
  `:focus-within` ya se recalculan en tiempo real; sigue faltando `:active` en
  cascada dinámica. En `<textarea>` sigue sin editarse salto de línea con Enter
  (Enter envía formulario o queda como envío de campo estándar).
- **Inline:** `inline-block` se alinea por arriba, no por la línea de base;
  `box-sizing` se trata como `content-box` ahí; `text-align` con un texto
  partido en varias líneas solo es exacto alineado a la izquierda.
- **Degradados:** solo `linear-gradient` (sin `radial`, `conic` ni `repeating`),
  una sola capa de fondo, y sin `background-size`/`position`/`repeat`.
- **Fuentes:** tres familias (sans, serifa, monoespaciada) con las fuentes del
  sistema; sin fuentes web. Si el sistema no tiene la familia, sale en sans.
- **Red:** sin HTTP/2 ni caché; el `User-Agent` es `NextGenWebEngine/0.1.0`.
- **Plataforma:** sin `<video>`, `<audio>`, `<iframe>`, WebGL, Workers, WebSockets.
- **Seguridad:** en Windows el proceso `--motor` corre aislado (integridad baja,
  escritorio dedicado, *Job Object* y perfil protegido), pero todas las pestañas
  comparten un único proceso `--motor` y falta aislamiento de red en el propio
  renderizador (*AppContainer*); en Linux y macOS sigue sin sandbox de sistema
  operativo. Ver `SECURITY.md`.
- **Cancelar una navegación** abandona la carga de red, pero un script que ya está
  corriendo no se interrumpe hasta su tope de 5 s.

## Dónde mirar si algo falla

| Síntoma | Primer sitio |
|---|---|
| Texto en lugares raros | `layout/src/tree.rs`, `place_inline_node` (rama `BoxType::Text`) |
| Algo no se ve | `gfx/src/display_list.rs` (qué se emite) y `gfx/src/paint.rs` (cómo se pinta) |
| Un estilo no se aplica | `css/src/cascade.rs` y `css/src/presentational.rs` |
| La página tarda | `RUST_LOG=info` imprime el tiempo de cada fase de `navigate` |
| La barra, las pestañas o un atajo | `app/src/barra.rs` (el HTML y los iconos) y `app/src/app.rs` (qué hace cada entrada) |
| Un clic o una tecla que no llega a la página | `app/src/app.rs`, métodos `clic` y `tecla`; la ventana en `app/src/ventana.rs` |
| El motor se cae o no arranca | `app/src/motor.rs` (procesos motor y broker); `RUST_LOG=info` y la salida de error |
| Una petición del protocolo | `core/src/protocol.rs` y `core/src/server.rs`; el test de humo exige una prueba por petición |
| Una API de JS | `js/src/dom_bindings.rs` y la sonda de APIs |

## Reglas para tocar el motor

1. Un test no puede depender de la velocidad ni de las métricas de la fuente de
   la máquina: el CI corre en Windows y los anchos y alturas cambian. Medir con
   la fuente en el propio test.
2. Antes de subir: `cargo test --workspace --locked`, `cargo clippy ... -D warnings`
   y el `wpt_runner`.
3. La aplicación es 100 % Rust (reglas en `AGENTS.md`) y vive en
   `engine/crates/app`.
