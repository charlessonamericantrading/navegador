# Navegador

> **Reglas del proyecto: [`AGENTS.md`](AGENTS.md).** La aplicación es 100 %
> Rust y el motor es propio, escrito desde cero.

Un navegador de escritorio con **motor propio en Rust**: red, HTML, CSS,
layout, pintado y JavaScript. La ventana, la barra y las pestañas también son
Rust, y las pinta el mismo motor que pinta las páginas.

> **Estado: en desarrollo.** Úsalo con sitios de confianza: el motor no tiene
> todavía sandbox del sistema operativo (ver [`SECURITY.md`](SECURITY.md)).
>
> **Qué funciona hoy y cómo medirlo: [`engine/ESTADO.md`](engine/ESTADO.md)**
> (corto y verificado). Lo que falta, en
> [`engine/huecos_sin_resolver.md`](engine/huecos_sin_resolver.md).

---

## Abrirlo

Hace falta **Rust** ([rustup.rs](https://rustup.rs/)). Nada más.

**Windows:** doble clic en `instalar.bat` (compila, tarda unos minutos la
primera vez) y luego `iniciar.bat`.

**Linux / macOS:** `./instalar.sh` y luego `./iniciar.sh`.

**A mano:**

```bash
cd engine
cargo run --release -p navegador                       # abre la ventana
cargo run --release -p navegador -- es.wikipedia.org   # con una dirección
```

Atajos: Ctrl+L (barra de direcciones), Ctrl+T (pestaña nueva), Ctrl+W
(cerrar), Ctrl+Tab (cambiar), Ctrl+R o F5 (recargar), Alt+← / Alt+→ (atrás /
adelante), Escape (detener).

---

## Qué hace hoy

| | |
|---|---|
| **Aplicación** | Ventana nativa, pestañas, atrás/adelante, recargar/detener, barra de direcciones (dirección o búsqueda), página de inicio y de error |
| **Red** | HTTP/1.1 y HTTPS (`hyper` + `rustls`), redirecciones, gzip/deflate/brotli, cookies, CORS y CSP. La red y las cookies van en un proceso aparte (broker) |
| **HTML / CSS** | Parseo HTML5, cascada con especificidad, `@media`, variables, `calc()`, `em`/`rem`/`vw`, degradados, atributos HTML clásicos |
| **Layout** | Bloque, texto en línea partido por palabras, `inline-block`, flex, grid, tablas, `float`, `position` |
| **JavaScript** | Intérprete `boa`, DOM, eventos, `fetch`, XHR, temporizadores, `localStorage`, módulos ES |
| **Pintado** | `tiny-skia`, SVG, fuentes del sistema (sans, serifa y monoespaciada), sombras, bordes redondeados |
| **IA** | `engine-ai`: árbol de accesibilidad de la página, pensado para un agente |

Se puede: abrir páginas reales por HTTPS, seguir enlaces, usar el historial y
las pestañas, escribir en los campos, enviar formularios e iniciar sesión en
sitios con cookies. Hacker News, Google y Wikipedia se ven bien.

**Lo que no:** las webs que montan todo su contenido con JavaScript en el
cliente suelen verse vacías (faltan APIs del DOM; el motor lo avisa). Sin
`<video>`, `<audio>`, `<iframe>`, WebGL ni Workers. Sin HTTP/2 ni caché.

---

## Cómo está montado

```text
engine/
├── crates/app/       La aplicación (binario `navegador`): ventana, barra, pestañas
├── crates/core/      Pipeline, servidor NDJSON del motor y arranque de procesos
├── crates/net/       HTTP/HTTPS, cookies, almacenamiento y broker
├── crates/dom/       Parseo HTML y árbol DOM
├── crates/css/       Parseo, selectores y cascada
├── crates/layout/    Cajas: bloque, inline, flex, grid, tabla, float
├── crates/text/      Fuentes, medida y partición de texto
├── crates/image/     Decodificación de imágenes y SVG
├── crates/js/        JavaScript y enlaces con el DOM
├── crates/gfx/       Lista de pintado y rasterizado
└── crates/ai/        Árbol de accesibilidad para el agente
```

`navegador` arranca tres procesos del mismo ejecutable: la ventana, el
**motor** (`--motor`, interpreta las páginas) y el **broker** (`--broker`, red,
cookies y almacenamiento). El motor y la ventana hablan NDJSON por
stdin/stdout; el motor pide la red al broker con un token de un solo uso
([ADR 0001](docs/adr/0001-broker-y-aislamiento-de-renderers.md)). Si una página
tumba el motor, la ventana sigue viva.

`engine/ARCHITECTURE.md` es el diario histórico de cómo se llegó aquí.

---

## Pruebas

Sin ningún navegador externo: se prueba con el propio motor.

```bash
cd engine
cargo test --workspace --locked                              # todos los tests
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p engine-core --bin wpt_runner --locked -- tests/wpt-style   # 60 de 60
cargo run --release -p navegador -- --captura v.png news.ycombinator.com   # imagen de la ventana
python3 scripts/dev/bench_all.py                             # tiempos
```

---

## Licencia

MIT — ver [LICENSE](LICENSE).
