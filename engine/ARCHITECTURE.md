# Arquitectura del motor — doctrina y estado real

Este documento existe porque la version anterior de este codigo no lo tenia,
y sin el, la tentacion de reescribir `rustls` a mano siempre vuelve. Tambien
documenta el estado REAL del motor a fecha de este commit, sin adornos.

## Estado real (no aspiracional)

A fecha de esta limpieza, el motor:

- **Descarga** paginas reales por HTTPS (`hyper` + `hyper-rustls`, TLS real
  via `rustls`) — verificado en vivo contra `info.cern.ch`. Sin DNS propio
  (usa el resolutor del SO via `hyper-util`); sin HTTP/2 todavia.
- **Sigue redirecciones** 301/302/303/307/308 de verdad (`NetworkEngine::
  fetch` en `net/src/http_client.rs`), hasta 20 saltos (limite del fetch
  spec de WHATWG) antes de rendirse con `NetworkError::TooManyRedirects`.
  301/302/303 con un metodo distinto de GET/HEAD degradan a GET sin cuerpo
  antes de repetir la peticion (comportamiento de facto de los navegadores,
  no el RFC original); 307/308 preservan metodo y cuerpo exactos, que es su
  unica razon de existir frente a 301/302. `NetworkResponse::url` expone la
  URL que efectivamente respondio (la final, no la pedida originalmente) -
  `core/server.rs::navigate` ya la usa para que la barra de direcciones
  muestre la pagina real en la que se aterrizo, y las Fases 1.3/1.4
  (recursos externos) la necesitaran para resolver rutas relativas contra
  la pagina correcta. Verificado en vivo con un servidor local que
  encadena 302 → 307 → 200. NO implementado: reintento automatico con
  cache de redirecciones, ni deteccion de bucles mas alla del limite fijo
  de 20 saltos (si A→B→A, se cuenta como 2 saltos "gastados" como
  cualquier otro, no se detecta el ciclo antes de tiempo).
- **Descomprime** el cuerpo de la respuesta segun `Content-Encoding`
  (`decompress_body` en `net/src/http_client.rs`): gzip y deflate/zlib con
  `flate2` (backend `miniz_oxide`, Rust puro, sin dependencia de C),
  brotli con el crate `brotli` (tambien Rust puro). Cada peticion manda
  `Accept-Encoding: gzip, deflate, br` por defecto. `identity` o cualquier
  codificacion no reconocida devuelve el cuerpo tal cual (no es un error,
  es "sin comprimir", igual que un navegador real con una codificacion que
  no soporta); un cuerpo que dice ser gzip/deflate/br y no lo es SI es un
  error real (`NetworkError::Decompress`), no se pasa en silencio. "deflate"
  se interpreta como zlib (RFC 1950, cabecera de 2 bytes), que es lo que
  mandan los servidores reales pese al nombre - no como deflate crudo (RFC
  1951). Verificado en vivo contra un servidor local que sirve HTML
  gzipeado de verdad. NO implementado: zstd (aun no es un `Content-Encoding`
  de uso generalizado en la web).
- **Parsea HTML** con `html5ever`: el algoritmo de construccion de arbol del
  spec real, con su recuperacion de errores para HTML mal formado. Verificado
  en vivo y con el adaptador `TreeSink` propio (`html5ever_sink.rs`).
  Simplificaciones documentadas ahi: namespaces foraneos (SVG/MathML) no
  tienen namespace correcto, `<template>` no usa un DocumentFragment inerte
  separado, doctype se ignora (sin representacion visual).
- **Parsea CSS** con `cssparser` (el tokenizador real de Servo/Stylo): maneja
  bloques anidados, `@media` y otras arroba-reglas (se saltan sin corromper
  el resto de la hoja), strings con `{`/`}`/`;`/`:` dentro, y comentarios.
- **Descarga `<link rel="stylesheet">`** (externo), no solo `<style>`
  inline: `core/server.rs::fetch_external_stylesheets` descubre cada
  `<link>` con `pipeline::find_external_stylesheet_hrefs` (pura, sin red -
  solo mira `rel`/`href`, tolera `rel="preload stylesheet"` con varios
  tokens), resuelve su `href` contra la URL final de la pagina (tras
  redirecciones) y descarga cada hoja con el mismo `NetworkEngine` que la
  pagina - por tanto con los mismos redirects/gzip/brotli que cualquier
  otra peticion. Las hojas descargadas se concatenan, en orden de
  documento, DESPUES de los `<style>` inline (mismo criterio de "lo que
  viene despues gana a igual especificidad" que ya regia entre `<style>` y
  el parametro `css` de test). Una hoja que falla (404, red caida, texto
  invalido) se omite con un aviso, sin abortar la carga de la pagina entera
  - igual que un navegador real. Verificado en vivo contra un servidor
  local que sirve HTML y CSS en rutas separadas. NO implementado: cache de
  hojas repetidas entre navegaciones, `@import` dentro de una hoja externa,
  precarga especulativa antes de que el HTML termine de parsear.
- **Matchea selectores de verdad** con el crate `selectors` (el mismo motor
  que usa Firefox/Stylo): combinadores (` `, `>`), selectores compuestos
  (`div.foo`), selectores de atributo (`[href]`, `[type="text"]` con los 6
  operadores del spec) y especificidad real ordenada como tripleta
  (id, clase, tipo) — todo esto NO existia en el matcher anterior, que solo
  comparaba un tag/`.clase`/`#id` suelto como cadena opaca. El adaptador
  (`element.rs`, trait `Element` con sus ~26 metodos) vive sobre un newtype
  `ElementRef` porque ni el trait ni `Arc<RwLock<Node>>` son locales al
  crate `css` (regla de huerfanos de Rust). Pseudo-clases/pseudo-elementos
  (`:hover`, `::before`) no estan soportados: el tipo que los representa es
  un enum vacio, asi que el parser los rechaza como error de sintaxis en vez
  de fingir que existen. Shadow DOM, partes y estados personalizados: `false`/
  `None` honestos, no hay nada de eso implementado. Verificado con tests que
  prueban exactamente lo que el matcher anterior no podia hacer.
- **Extrae el CSS real de la pagina** (`core/pipeline.rs::build_page`): todos
  los `<style>` del documento parseado se concatenan en orden (mismo patron
  que `<script>` — `Node::find_all_by_tag` + `text_content`) y se parsean de
  verdad — antes, `build_page` solo aplicaba el CSS que quien llamaba le
  pasara aparte, y `main.rs` pasaba una hoja de ejemplo hardcodeada sin
  ninguna relacion con lo que la pagina real descargada declarase; se quito
  en cuanto hubo una fuente real, para no mezclar CSS inventado con CSS real
  sin ninguna etiqueta que distinguiera cual era cual. El parametro `css` de
  `build_page` sigue existiendo pero como CSS ADICIONAL (util para tests o
  una futura hoja de usuario), no como sustituto — probado que ambas fuentes
  se combinan y que varios `<style>` se concatenan en orden. `<link
  rel="stylesheet">` (externo) SI se descarga ahora - ver mas abajo, "Sigue
  redirecciones"/"Descomprime"/"Descarga `<link rel=stylesheet>`" - lo que
  sigue sin ocurrir es que se busque durante el PARSEO en si (se descubre
  parseando el documento una vez completo, no en streaming).
- **Aplica la cascada CSS al layout**: `LayoutTreeBuilder::resolve_style`
  filtra las reglas del stylesheet por `SelectorMatcher::matches`, ordena por
  especificidad (`SelectorMatcher::calculate_specificity`) y fusiona
  declaraciones — probado con tests de cascada (especificidad, no-match).
  El atributo `style="..."` del propio elemento (si tiene uno) se fusiona
  DESPUES de todas las reglas del stylesheet, sin importar su especificidad
  — igual que el spec real, donde un estilo en linea gana incluso a un
  selector de id (`!important` aparte, que no esta modelado) — parseado con
  el mismo tokenizador `cssparser` que las hojas de estilo normales
  (`CssParser::parse_inline_style`, en `css/src/parser.rs`), no un split
  manual. Antes de esto el atributo `style` se ignoraba por completo: un
  `<div style="color: red">` se pintaba igual que sin el atributo — probado
  explicitamente, incluido que gana sobre una regla de id.
- **Hoja de estilos de agente de usuario** (`css/src/user_agent_stylesheet.rs`):
  margenes reales de `body`/`h1`-`h6`/`p`/`ul`/`ol`, tamaños de fuente de
  titulares, color+subrayado de `a`, negrita de `b`/`strong`, cursiva de
  `i`/`em` - los valores por defecto que cualquier navegador real aplica
  ANTES del CSS del autor. Sin esto, una pagina sin su propio CSS se veia
  como texto plano de 16px sin estructura alguna (verificado en vivo antes
  y despues del cambio: el mismo HTML sin CSS pasa de texto plano a tener
  titular grande, margenes reales entre parrafos y enlaces azules). Se
  resuelve como un ORIGEN aparte en `resolve_style`
  (`apply_matching_rules` se llama dos veces: agente de usuario primero,
  autor despues) - una regla de autor SIEMPRE gana a una de agente de
  usuario para la misma propiedad, sin comparar especificidad entre ambas,
  igual que el orden de origenes real del cascade spec; la especificidad
  solo desempata DENTRO de cada origen. Probado explicitamente que un
  autor con la MISMA especificidad nominal que la regla de agente de
  usuario (ambas un simple selector de tag) sigue ganando - si se hubieran
  mezclado ambos origenes en una sola lista ordenada por especificidad, el
  resultado habria dependido del orden de insercion en vez de ser siempre
  "autor gana". La hoja se parsea una sola vez (`OnceLock`), no en cada
  `resolve_style` (que corre por cada nodo del DOM). Simplificaciones
  honestas: `margin` de estos elementos solo tiene el valor VERTICAL real
  en el spec (arriba/abajo, horizontal en cero) pero aqui el motor de
  layout solo soporta un unico valor a los 4 lados (`resolve_margin`), asi
  que tambien empuja los lados; `font-weight`/`font-style`/
  `text-decoration` quedan en el `computed_style` (cascada correcta) y
  `engine-gfx` los lee los tres al pintar desde la Fase 2.4 (negrita/
  cursiva) y la Fase 29 (subrayado, ver su entrada mas abajo - en el
  momento de ESTA fase todavia no se veian); sin viñetas ni sangria de
  listas.
  - **Bug real encontrado al añadir esto, no solo teorico**: dar a `<body>`
    un `margin` real por primera vez expuso que `flow_block_children`
    calculaba la altura de un contenedor sumando solo `dimensions.height`
    de cada hijo, SIN su margin-top/margin-bottom - un contenedor con un
    hijo marginado quedaba mas bajo de lo que su contenido realmente
    ocupaba, y `<html>` dejaba de contener a `<body>` una vez desplazado
    por su propio margin. Sintoma real: el hit-test sobre un `<p>` fallaba
    porque un ancestro (`<html>`) ya no envolvia geometricamente al punto
    pedido. Arreglado sumando tambien el margin de cada nieto al calcular
    `content_height` - probado con un test de regresion dedicado
    (`container_height_accounts_for_a_childs_own_margin_not_just_its_box`).
    Este bug era preexistente desde que `margin` se hizo real (tarea
    anterior); simplemente ningun test previo tenia un contenedor cuyo
    hijo marginado desbordara la altura calculada del contenedor lo
    suficiente como para romper una comprobacion geometrica de un ancestro.
  `color` y `font-size` (`INHERITABLE_PROPERTIES` en `tree.rs`) se propagan
  de verdad a las cajas de texto descendientes, atravesando varios niveles y
  respetando que un ancestro mas cercano pise a uno mas lejano — probado con
  4 tests de herencia. El resto de propiedades heredables del spec
  (`font-family`, `font-weight`, `line-height`...) no se propagan todavia.
  Esto es solo el lado CSS/layout: el objeto JS `element.style`
  (`.getPropertyValue`/`.setProperty`/`.cssText`) para MUTAR el atributo
  desde un script es un paso deliberadamente separado (ver mas abajo,
  ahora real).
- **Calcula un layout de bloque real** (Fase 1, item 6 del plan): las cajas
  se apilan verticalmente, con `width`/`height`/`max-width`/`min-width`
  reales (`resolve_block_width` en `tree.rs`) — antes, TODA caja de bloque
  llenaba el ancho disponible sin importar que dijera su CSS; ahora un
  `width: 300px` (o `max-width`/`min-width`) se respeta de verdad, y sin
  ninguna de las tres se sigue llenando el espacio disponible (mismo
  comportamiento auto de siempre). `width`/`max-width`/`min-width` son
  CONTENT-box (el valor inicial real de `box-sizing`): se convierten a
  border-box sumando el propio padding+border antes de aplicarse, asi que
  un `width: 200px` con padding da un border-box MAS ANCHO que 200px, no
  mas estrecho — `box-sizing: border-box` no esta soportado. `max-width` se
  aplica antes que `min-width` (si entran en conflicto, `min-width` gana,
  igual que `clamp(min, tentative, max)` del spec real) — probado
  explicitamente. `height` (si esta puesta) sustituye el alto
  auto-calculado del contenido por el valor del autor, SIN el minimo
  heuristico que si aplica al alto auto (`LINE_HEIGHT_FALLBACK`, un
  invento propio del motor, no una regla real) — un `height` pequeño se
  respeta aunque el contenido desborde visualmente (sin recorte:
  `overflow` no esta implementado). Sin `max-height`/`min-height`
  todavia. Verificado en vivo: dos cajas con `background-color` distinto,
  una con `width: 300px; padding: 10px` midiendo 320px reales, otra con
  `width: 500px; max-width: 200px` quedando en 200px — ambas visiblemente
  mas estrechas que el viewport de 800px, donde antes las dos lo habrian
  llenado por completo sin importar su CSS. Sin floats ni grid/flex
  todavia. Las cajas de texto SI miden con las metricas
  reales de la fuente que de verdad se va a pintar (`engine_text::measure_text`,
  misma fuente cargada una vez en `core/main.rs` y compartida con
  `engine-gfx`): el alto de linea usa ascenso+descenso+salto de linea reales
  de la fuente. El quiebre de linea YA rompe por palabra de verdad
  (`engine_text::wrap_text`, sin hifenacion — una palabra sola mas ancha que
  el contenedor se desborda en vez de partirse a mitad) — antes, `font-size`
  se ignoraba por completo (constante fija de 22px de alto y 8px/caracter) y
  el "envuelto" solo contaba cuantas lineas de ese ancho cabrian en total,
  sin romper realmente el texto. `layout` y `engine-gfx` llaman a la misma
  `wrap_text` con los mismos argumentos (mismo font_size, mismo ancho
  disponible), asi que el numero de lineas reservado y el pintado coinciden
  por construccion — probado con tests que comprueban que ninguna palabra se
  parte, se pierde ni se reordena al envolver. Sin fuente de sistema
  disponible, cae a la aproximacion anterior por caracteres — mismo tipo de
  respaldo honesto que ya usaba `engine-gfx` al pintar.
- **Layout INLINE real** (Fase 2.3): `<b>`/`<i>`/`<a>`/`<span>` y texto
  suelto fluyen horizontalmente en lineas reales, en vez de que cada uno
  cayera en su propia linea vertical (el bug mas visible detectado en el
  analisis honesto que motivo el plan de 5 fases). `flow_block_children`
  (`tree.rs`) agrupa cada racha de hijos consecutivos `BoxType::Text`/
  `BoxType::Inline` y la delega a `flow_inline_run`, que coloca cada hoja
  de texto ENTERA (granularidad atomica por nodo, no palabra a palabra
  entre hermanos) en la linea actual si cabe, salta de linea si no, y
  envuelve internamente con el mismo `wrap_text` de siempre si ni siquiera
  cabe sola en una linea vacia — el siguiente hermano entonces empieza en
  linea nueva (simplificacion declarada: el spec real permitiria
  continuar en la ultima linea parcial de un vecino que envolvio varias
  lineas). Un elemento inline anidado (`<b>` con hijos propios) se
  recorre recursivamente con el MISMO cursor compartido; su propia caja
  se dimensiona como el rectangulo delimitador de lo que contuvo —
  impreciso si sus hijos terminan repartidos en mas de una linea (un
  inline partido en dos lineas es, en el spec real, dos fragmentos, no
  uno), simplificacion declarada suficiente para pintar/hit-testear el
  caso comun. `flow_block_children` cambio de sumar `dimensions.height`
  de cada hijo a devolver el `cursor_y` final (menos su propio
  content-top): con hijos de bloque nunca se solapan verticalmente y da
  el mismo resultado exacto que antes (verificado matematicamente y con
  tests), pero con una racha inline VARIOS hermanos comparten la misma
  `y` — sumar sus alturas por separado, como se hacia antes, habria
  multiplicado la altura del contenedor por cada fragmento en la misma
  linea.
  - **Dos bugs reales encontrados al construir esto, no solo teoricos**:
    (1) `build_node` recortaba cada nodo de texto con `.trim()` completo,
    incluido un espacio SIGNIFICATIVO al final que separaba palabras de
    un hermano siguiente ("Text " antes de un `<b>bold</b>` se quedaba en
    "Text", pegandose a "bold": "Textbold") — invisible mientras cada
    nodo tenia su propia linea, real ahora que el flujo los junta.
    Arreglado con `collapse_whitespace` (colapsa CUALQUIER racha de
    espacios en blanco a uno solo, el comportamiento real de
    `white-space: normal`, sin recortar los bordes por completo). (2)
    `engine_text::wrap_text` reconstruye lineas via `split_whitespace()` +
    union de palabras, lo que descarta ESTRUCTURALMENTE cualquier espacio
    inicial/final del texto original (los tokens de palabra no llevan
    bordes) — mismo sintoma, un fragmento de texto perdia su espacio de
    separacion al PINTARSE (gfx re-deriva las lineas llamando a
    `wrap_text` de nuevo). Arreglado detectando si el texto original
    empezaba/terminaba en espacio y reinsertandolo en la primera/ultima
    linea del resultado. Ambos verificados en vivo (un parrafo con texto
    + `<b>`/`<i>`/`<a>` mezclados, antes y despues: de palabras pegadas
    entre si a espaciado correcto) y con tests dedicados en
    `engine-text`/`engine-layout` (`wrap_text` no tenia NINGUN test
    directo hasta ahora, pese a ser central para layout y pintado).
  NO implementado (en el momento de ESTA fase; `text-align` cerrado
  despues, ver Fase 31): `text-align`, `vertical-align`, alineacion por
  baseline real (todo el texto de una racha usa el mismo `line_height`,
  calculado una vez con el font-size de su primera hoja — el spec real
  usaria el maximo por linea cuando el tamaño varia dentro de ella).
  `flex`/`grid` NO
  existen: habia un `flexbox.rs` con `FlexLayoutEngine` que nunca se llamaba
  desde `LayoutTreeBuilder` (ningun stylesheet real podia activarlo) y cuya
  logica interna tampoco implementaba el spec de verdad (`justify-content`
  solo distinguia `Center` del resto, `align-items` no se usaba en fila,
  altura fija de 40px, sin `flex-grow`/`shrink`/`basis`/`wrap`, sin recibir
  siquiera el `computed_style` para poder leer CSS real) — exactamente el
  patron de "codigo que miente" de la seccion de abajo, asi que se borro en
  vez de dejarlo fingiendo. Flexbox real, conectado a `display: flex` y
  probado contra casos concretos del spec, sigue pendiente (Fase 2).
- **Flexbox real via `taffy`** (Fase 3.2): la tarea original pedia evaluar
  `taffy` antes de escribir flexbox a mano, dado el `FlexLayoutEngine`
  fingido que se borro arriba (que ni el propio `LayoutTreeBuilder` llamaba
  nunca). Verificado que `taffy` (v0.13, resuelto contra crates.io en este
  mismo entorno) es real y madura: implementa Flexbox Y CSS Grid completos,
  la usan proyectos en produccion real (Bevy, Dioxus), y expone
  `TaffyTree::compute_layout_with_measure` con funciones de medida propias -
  exactamente el patron que hace falta para reusar el `flow_block_children`/
  `measure_text`/`resolve_image_dimensions` YA existentes en vez de que
  taffy tenga que reinventar su propio medidor de texto/imagenes.
  - **Decision, con la razon exacta**: SI usar `taffy` para el ALGORITMO de
    reparto de espacio de flex (eje principal/cruzado, `flex-grow`/`shrink`/
    `basis`, alineacion) - es del mismo orden de complejidad que el arbol
    de HTML5 (~800 estados, ya justifica `html5ever` en esta misma tabla) y
    ya esta resuelto por un crate maduro. Esto es una EXCEPCION explicita a
    la entrada "layout: ..., flex, grid, ..." de la lista "se escribe a
    mano" de mas abajo - un cambio real de la doctrina previa, no una
    entrada mas (aprobado explicitamente antes de integrar). El puente SI
    se escribe a mano - eso sigue siendo "como se comporta la pagina" real.
  - **La integracion real, no solo la evaluacion**: nuevo modulo dentro de
    `engine-layout::tree` con 3 piezas. (1) `flex_container_style`/
    `flex_item_style` traducen `computed_style` ya resuelto (`display: flex`
    detectado en `flow_block_children`, que desvia el contenedor entero a
    `flow_flex_children`) a `taffy::Style`: `flex-direction`/
    `justify-content`/`align-items` para el contenedor, `flex-grow`/
    `flex-shrink`/`flex-basis`/`width`/`height` para cada item - CADA hijo
    DIRECTO es un item flex, sin distincion de `BoxType` (un `<img>` es un
    item tan valido como un `<div>`). (2) `measure_flex_item`, la funcion de
    medida que `taffy` invoca (varias veces, con distintos `available_space`
    especulativos) para saber cuanto necesita cada item: para imagenes
    llama a `resolve_image_dimensions`, para bloque/inline ejecuta
    `flow_block_children` en una pasada especulativa a un ancho candidato.
    (3) Tras `compute_layout_with_measure`, `finalize_flex_item_children`
    hace la pasada FINAL y autoritativa: copia `taffy::Layout` (x/y/ancho/
    alto ya resueltos) a `LayoutBox::dimensions` y posiciona a los NIETOS
    (hijos de cada item) con el mismo `flow_block_children` de siempre, a
    su ancho/alto YA definitivos. El pintado (`display_list.rs`) no
    necesito ningun cambio: solo lee `LayoutBox::dimensions`, sin saber ni
    importarle si vinieron del flujo de bloque o de `taffy`.
  - **Bug real encontrado y arreglado al verificar en vivo, no teorico**:
    el PROPIO tamaño del contenedor flex nunca se paso a `taffy::Style.size`
    - solo se pasaba como `available_space` a `compute_layout_with_measure`,
    que taffy trata como un TECHO para sizing intrinseco/shrink-to-fit, NO
    como el ancho ya resuelto. Resultado: un contenedor `display: flex;
    width: 500px` con un item `flex-grow: 1` sin ancho propio se encogia a
    la suma de los flex-basis de sus items (100px) en vez de ocupar los
    500px reales - `flex-grow` no tenia espacio sobrante que repartir
    porque el contenedor nunca llego a medir 500px. Diagnosticado leyendo
    el propio codigo fuente de `taffy` (`determine_container_main_size` en
    `compute/flexbox.rs`: sin `Style.size` propio, un contenedor de una
    sola linea usa `longest_line_length` -shrink-to-fit-, ignorando
    `available_space` como techo real solo cuando hay mas de una linea).
    Arreglado pasando el `inner_width`/alto explicito YA resueltos por
    `resolve_block_width` (via el flujo de bloque normal, ANTES de llegar
    aqui) directamente en `Style.size` del nodo raiz. Verificado con test
    dedicado (`flex_grow_distributes_the_remaining_space`) y en vivo (fila
    con 3 items reales, `flex-grow`/`justify-content: space-between`/
    `align-items: center`, mas una columna separada con `flex-direction:
    column` - captura de pantalla con colores solidos, screenshot revisado).
  - Tests reales: layout fila/columna, `flex-grow` repartiendo espacio
    sobrante, `justify-content: center`, una `<img>` como item flex midiendo
    su tamaño natural real via `measure_flex_item`.
  - NO implementado: `flex-wrap` (una sola linea siempre), medicion real de
    min-content/max-content (un item sin ancho explicito mide su contenido
    al ancho DISPONIBLE completo al pedir el tamaño de contenido, no al
    ancho minimo que evitaria partir palabras - declarado en el propio
    `measure_flex_item`), `align-content`, `align-self` por item,
    `row-gap`/`column-gap`, `order`, texto suelto como item flex anonimo
    (el spec real lo envolveria; aqui simplemente no se maneja - caso raro).
    CSS Grid NO esta conectado (la feature `grid` de `taffy` esta
    deliberadamente desactivada en `Cargo.toml` - esta tarea era solo
    flexbox); reactivarla cuando llegue esa fase es sumar la feature y un
    puente equivalente, `taffy` ya la trae.
- **`position: relative`/`absolute`/`fixed` + `z-index` reales** (Fase 3.3):
  layout en DOS pasadas. La primera (`flow_block_children`/
  `flow_inline_run`/`flow_flex_children`, ya existentes) trata `relative`
  como flujo normal (`is_out_of_flow` es `false`) y aplica el
  desplazamiento visual (`apply_relative_offset`, `top`/`right`/`bottom`/
  `left`) justo ANTES de recursar en los hijos de la caja - asi todo su
  subarbol hereda el desplazamiento sin tener que recorrerlo aparte, ya que
  sus propias coordenadas se calculan a partir de las de su padre, ya
  desplazadas. `absolute`/`fixed` (`is_out_of_flow` es `true`) se SACAN del
  flujo por completo en esa misma pasada - no reservan espacio, los
  hermanos actuan como si no existieran (mismo criterio en las 3 funciones
  de flujo, y en `flow_flex_children` ni siquiera se crea su nodo hoja de
  taffy) - dejando sus `dimensions` sin resolver a proposito.
  La segunda pasada (`resolve_positioned_boxes`, nueva, se ejecuta despues
  de que la primera termine el arbol ENTERO) recorre todo el arbol
  buscando esas cajas sin resolver y las posiciona contra su "containing
  block" real: la padding-box del ancestro mas cercano con `position`
  distinto de `static` (asi es el spec real - un `relative` sin `top`/
  `left` puestos YA establece containing block para descendientes
  absolutos), o el viewport si no hay ninguno; `fixed` SIEMPRE usa el
  viewport, ignorando cualquier ancestro posicionado. El alto de contenido
  (cuando no hay `height` explicita) se mide con el mismo
  `flow_block_children` de siempre, DESPUES de fijar ancho/x - si solo hay
  `bottom` (sin `top`), la Y final solo se conoce tras esa medicion, y los
  hijos que se posicionaron con la Y provisional durante esa misma llamada
  se corrigen con `shift_subtree_y` (desplaza el subarbol entero, mas
  barato que re-layoutear desde cero).
  `z-index` se resuelve en el PINTADO (`engine-gfx::display_list`), no en
  el layout: `DisplayList::build_items` pinta en orden de documento como
  siempre, EXCEPTO que al descender a un hijo `position != static` con
  `z-index` numerico (sin `position`, `z-index` no tiene ningun efecto,
  igual que el spec real), su subarbol entero se acumula en una capa
  aparte en vez de en la lista principal; al terminar, esas capas se
  ordenan por z-index ascendente y se anexan al final - un elemento
  posicionado con z-index alto pinta encima de todo lo demas sin importar
  su orden de documento. Simplificacion declarada: sin contextos de
  apilamiento anidados de verdad (un z-index dentro de otro se aplana al
  mismo nivel que todos los demas), cubre el caso real mas comun (un
  modal/tooltip/dropdown por encima de todo) no el spec completo.
  Verificado en vivo: un contenedor `relative` con dos hijos `absolute`
  anclados a sus esquinas (no al viewport), un `relative` con `top`
  negativo desplazado sin afectar al parrafo siguiente, y un `absolute`
  con `z-index` alto sin ancestro posicionado anclado al viewport -
  captura de pantalla revisada, todo en su sitio esperado.
  Tests reales: `relative` no mueve al hermano siguiente, un hijo hereda
  el desplazamiento de su padre `relative`, `absolute` se saca del flujo
  por completo, containing block correcto (ancestro `relative` vs
  viewport), `fixed` ignora ancestros posicionados, `bottom` sin `top`
  ancla al borde inferior, y el orden de pintado por z-index.
  NO implementado: `position: sticky`, `inset` (shorthand), `%` en
  `top`/`right`/`bottom`/`left` (solo `px`, mismo criterio que el resto del
  motor), shrink-to-fit real para `width: auto` en elementos fuera de
  flujo (usa el mismo criterio "llenar el containing block" que el flujo
  normal - simplificacion declarada), contextos de apilamiento anidados de
  verdad para `z-index`.
- **Layout de tablas real** (Fase 3.4): `display: table`/`table-row`/
  `table-cell` (nuevas reglas en la hoja de agente de usuario para
  `table`/`tr`/`td`/`th`) desvian el contenedor entero a
  `LayoutTreeBuilder::flow_table_children`, igual que `display: flex` ya
  desviaba a `flow_flex_children` (Fase 3.2) - misma tecnica de dispatch,
  algoritmo distinto: a diferencia de flex/grid, el layout de tablas NO se
  delega a `taffy` (ver la entrada de la doctrina de dependencias mas
  abajo, actualizada) - es codigo propio de principio a fin.
  - **Filas transparentes a traves de `thead`/`tbody`/`tfoot`**:
    `collect_table_rows` recoge las cajas `display: table-row` a CUALQUIER
    profundidad bajo la tabla (no solo hijos directos), atravesando
    cualquier contenedor que no sea la propia `table`/una fila/una celda -
    el motor no genera cajas anonimas de "grupo de filas" para darle a
    `thead`/`tbody`/`tfoot` un rol propio en el algoritmo (siguen
    existiendo como cajas de bloque normales, simplemente invisibles para
    el layout de tabla); sin este atajo, la inmensa mayoria de tablas
    reales (que SI envuelven sus filas en `tbody`) no habrian funcionado.
  - **Columnas de ancho IGUAL, simplificacion declarada**: numero de
    columnas = el maximo de celdas de cualquier fila; cada columna mide
    `ancho_interior / numero_columnas`. El spec real (`auto` table layout)
    reparte el ancho segun el contenido MIN/MAX de cada columna - este
    motor no mide min-content/max-content todavia para NINGUN contexto
    (mismo hueco ya declarado en `flow_flex_children` para items flex sin
    ancho propio), asi que columnas iguales es la aproximacion mas honesta
    disponible sin inventar un medidor de contenido nuevo.
  - Alto de fila = el maximo de sus celdas (cada celda se layoutea con el
    `flow_block_children` de siempre, la celda pasa a ser "container" de
    sus propios hijos); todas las celdas de la fila se ESTIRAN a ese alto
    en una segunda pasada corta - el comportamiento visible por defecto de
    cualquier tabla real.
  - **Bug real encontrado y arreglado al verificar en vivo, no teorico,
    fuera de `engine-layout`**: una columna de ancho fraccionario (500px /
    3 = 166.66...px) con un `border: 1px solid` hacia panic la aplicacion
    ENTERA (no solo el layout) - `debug_assert!(false)` real DENTRO de
    tiny-skia (`scan::hairline_aa::fill_dot8`, tiny-skia 0.11.4) al pintar
    un rectangulo de borde "hairline" (mas fino que 1px tras redondeo de
    punto fijo) en una posicion no alineada a pixel; un limite conocido de
    su rasterizador de rectangulos finos, no una entrada invalida por
    nuestra parte. Bloques con `border` de antes de esta tarea nunca lo
    disparaban porque sus coordenadas eran siempre numeros enteros (flujo
    de bloque simple, sin division fraccionaria en ningun sitio); las
    columnas de tabla son el primer caso real con bordes en coordenadas
    fraccionarias. Arreglado en LAS DOS copias de `border_strip_rects`
    (`engine-gfx::raster`, la captura headless, y `engine-gfx::window`, la
    ventana nativa en vivo - duplicadas desde antes de esta tarea, mismo
    bug en ambas) redondeando `x`/`y` y el borde OPUESTO
    (`x+width`/`y+height`) cada uno por separado a pixel entero antes de
    construir las franjas ("pixel snapping" - la tecnica estandar para que
    cajas vecinas sigan encajando sin huecos). Diagnosticado leyendo el
    backtrace completo (`RUST_BACKTRACE=full`) hasta el `debug_assert!`
    exacto dentro de la dependencia. Verificado con test dedicado
    (`renders_without_panicking_when_a_bordered_box_sits_at_a_fractional_x`,
    que SI hacia panic antes del arreglo) y en vivo (tabla real de 3
    columnas con `thead`/`tbody`, bordes y `background-color` en las
    celdas de cabecera - captura de pantalla revisada, sin panic, columnas
    iguales, fila con texto largo estirando ambas celdas de su fila).
  - Tests reales: celdas lado a lado en columnas iguales, filas apiladas
    verticalmente, una celda corta estirandose al alto de la mas alta de
    su fila, filas dentro de `thead`/`tbody` encontradas correctamente, y
    el numero de columnas fijado por la fila con MAS celdas.
  - NO implementado: `colspan`/`rowspan`, `border-collapse`/
    `border-spacing` (cada celda pinta su propio border por separado, sin
    fusionar bordes adyacentes), medicion de contenido real para el ancho
    de columnas (ver arriba), celdas fuera de flujo (un `position:
    absolute` en una celda participa en el reparto de columnas igual que
    cualquier otra, en vez de sacarse del algoritmo), `text-align: center`
    de `th` (la propiedad todavia no se PINTA en ningun contexto).
- **`border-radius`, `box-shadow`, `overflow: hidden`** (Fase 3.5): las tres
  son propiedades puramente de PINTADO - ningun cambio en `engine-layout`,
  todo vive en `engine-gfx` (`display_list.rs` genera los `DisplayItem`
  nuevos, `paint.rs`, nuevo, los pinta).
  - **Refactor previo, no opcional**: antes de añadir nada nuevo, se
    extrajo el bucle de pintado (antes duplicado ENTERO en `raster.rs` y
    `window.rs`, cada uno con su propia copia de `border_strip_rects`) a un
    modulo compartido, `paint.rs`, con una unica funcion
    `paint_display_list` que ambos consumidores llaman. Motivo real, no
    limpieza porque si: el bug de tiny-skia con bordes fraccionarios (Fase
    3.4) se encontro y arreglo DOS VECES, una por copia, exactamente
    porque el codigo estaba duplicado - añadir 3 propiedades nuevas de
    pintado a DOS copias habria repetido ese riesgo por partida triple.
    `raster.rs` y `window.rs` quedaron mucho mas cortos (solo preparan la
    superficie/pixmap y llaman a `paint_display_list`); `image_paint::
    paint_image` gano un parametro `mask: Option<&Mask>` para poder
    participar en el recorte de `overflow: hidden` igual que el resto.
  - **`border-radius`**: un unico valor en `px` para las 4 esquinas (misma
    simplificacion "un solo numero" que `padding`/`margin`/`border-width`).
    tiny-skia no trae un `push_round_rect`, asi que `paint::
    rounded_rect_path` construye el contorno a mano con curvas cuadraticas
    (`quad_to`, control point en la esquina exacta) por esquina -
    visualmente indistinguible de un arco real a los radios tipicos de una
    UI, no matematicamente perfecto (eso pediria curvas cubicas con la
    constante magica ~0.5522847498). El FONDO redondeado se pinta con
    `fill_path` sobre ese contorno; el BORDER redondeado NO reusa las 4
    franjas rectangulares de siempre (no encajarian en las esquinas) - se
    pinta como un `stroke_path` sobre el mismo contorno, con el rectangulo
    INSET la mitad del grosor del border para que el trazo caiga DENTRO
    del border-box (tiny-skia centra un stroke sobre su path, mitad hacia
    afuera/mitad hacia adentro por defecto - sin el inset, la mitad
    exterior del trazo se saldria de `dimensions`).
  - **`box-shadow`**: `<offset-x> <offset-y> [<blur>] <color>` - el
    `blur-radius` opcional SI se parsea (para no romper el resto de
    tokens, ej. tomar el color por el blur) pero se DESCARTA a proposito:
    sombra "dura", sin difuminado gaussiano real (simplificacion
    declarada, un blur de verdad es su propio problema matematico). Se
    pinta como un `DisplayItem::Shadow` ANTES que `SolidRect`/`Border` de
    la misma caja (orden real del spec: la sombra queda DETRAS del fondo/
    border), con el offset YA aplicado al rectangulo por
    `DisplayList::build_items` - quien pinta la sombra no sabe que existio
    un desplazamiento por separado, solo rellena un rectangulo.
  - **`overflow: hidden`**: `DisplayList::build_items` envuelve TODO el
    subarbol de hijos de una caja con `overflow: hidden` entre un
    `DisplayItem::PushClip { rect }` (la caja, `dimensions` completo) y su
    `PopClip` correspondiente - mismo anidamiento que el arbol de cajas.
    `paint_display_list` mantiene una PILA de rectangulos de recorte
    activos y solo reconstruye la `tiny_skia::Mask` real cuando la pila
    CAMBIA (Push/Pop), no en cada item individual pintado dentro. La
    mascara se construye con la INTERSECCION GEOMETRICA de TODOS los
    recortes activos (no solo el mas cercano) - varios `overflow: hidden`
    anidados recortan correctamente al mas pequeño de todos; si la
    interseccion resulta vacia (recortes que no se solapan en absoluto),
    la mascara queda toda a cero (todo oculto), no `None` (que pintaria
    sin recortar, el bug contrario). Solo `overflow: hidden` esta
    reconocido - `scroll`/`auto` recortarian igual en un motor con scroll
    interno POR ELEMENTO, que este no tiene (solo el scroll de pagina
    completa de `window.rs`); `visible`, el valor inicial real, no recorta
    nada, igual que si la propiedad no estuviera puesta.
  - **Bug residual cerrado de paso, no teorico**: al escribir
    `paint_display_list` se noto que `scroll_offset_y` (acumulado por
    rueda del raton, puede llegar fraccionario desde un
    `MouseScrollDelta::PixelDelta` de trackpad) se restaba de cada
    coordenada Y DESPUES de que `border_strip_rects` ya hubiera redondeado
    esa misma coordenada a pixel entero (el arreglo de la Fase 3.4) -
    reintroduciendo una Y fraccionaria por la puerta de atras, mismo tipo
    de entrada que dispara el `debug_assert!` de tiny-skia. Cerrado
    redondeando `scroll_offset_y` una sola vez, al principio de
    `paint_display_list`, antes de usarse en NINGUN calculo - protege a
    TODOS los tipos de item (no solo `Border`), no solo el caso que ya
    tenia un test dedicado.
  - Tests reales: los 3 parsers CSS nuevos (`border-radius`, `box-shadow`
    con offsets/color en cualquier orden, blur ignorado, offsets
    negativos), orden sombra-antes-que-fondo con el offset ya aplicado,
    `overflow: hidden` envolviendo a los hijos en `PushClip`/`PopClip` con
    el rectangulo del PADRE, `overflow: visible` sin emitir ningun recorte,
    interseccion de rectangulos (solapados y disjuntos), `rounded_rect_path`
    clampando el radio a la mitad del lado corto, y una prueba de
    integracion (`render_layout_to_png`) con las 3 propiedades combinadas
    que NO hace panic. Verificado en vivo: una tarjeta con esquinas
    redondeadas + sombra dura, y un contenedor `overflow: hidden` con un
    hijo de 400x400 desbordandolo a proposito, recortado EXACTAMENTE al
    borde del contenedor - captura de pantalla revisada (con zoom sobre el
    borde de recorte para confirmar el pixel exacto de corte).
  - NO implementado: 4 radios independientes por esquina (`border-radius:
    <tl> <tr> <br> <bl>`), blur gaussiano real en `box-shadow`, `inset`
    box-shadow, multiples sombras (`box-shadow` con comas), `overflow:
    scroll`/`auto` (sin scroll interno por elemento), recorte de
    `DisplayItem::Image` por `border-radius` (una imagen con esquinas
    redondeadas via `border-radius` en su propia caja no se recorta a esas
    esquinas todavia - `overflow: hidden` SI la recorta, via la mascara
    generica), y el hueco ya declarado en `z-index` (Fase 3.3) que ahora
    tambien aplica a `overflow: hidden`: un descendiente `position`+
    `z-index` numerico dentro de un `overflow: hidden` se desvia a
    `z_layers` ANTES de que el `PushClip` que lo envuelve llegue a la
    lista principal, asi que NO queda recortado.
- **Teclado real + foco + `checked` de checkbox/radio** (Fase 4.1): a
  diferencia de Fases anteriores, el punto de partida NO era cero -
  `core::server` (el puente NDJSON real que usa el producto - Electron +
  `BrowserManager`, no la ventana winit de `core::main`, que sigue sin
  ninguna fuente de teclado) ya tenia `focused_node`, `type_text` (escribe
  texto real en `value`) y `press_key` (disparaba `keydown`/`keyup`) desde
  una tarea muy anterior. Lo que faltaba, y era honestamente falso hasta
  ahora, es lo que se completa aqui:
  - **`event.key` real**: `press_key`/`type_text` disparaban `keydown`/
    `keyup` con un `Event` generico SIN ninguna propiedad de tecla - un
    listener JS real (`addEventListener('keydown', e => e.key)`, el caso
    de uso mas comun de este evento) no podia funcionar en absoluto.
    Arreglado con `DomBindings::dispatch_keyboard_event`/`JsRuntime::
    dispatch_keyboard_event` (nuevas, `dispatch_event` normal SIN cambios
    de comportamiento para clic/foco - ver su doc-comment) que ademas
    dejan `.key` puesto en el objeto `Event`. Sin `KeyboardEvent` completo
    (`.code`/`.shiftKey`/`.ctrlKey`/`.altKey`/`.metaKey` - fuera del
    alcance de esta tarea).
  - **`Backspace`/`Delete` mutan `value` de verdad**: antes de esta tarea,
    `press_key` disparaba `keydown`/`keyup` para CUALQUIER tecla sin tocar
    el texto - pulsar `Backspace` sobre un campo enfocado literalmente no
    hacia nada al contenido, solo emitia el evento. `backspace_control_value`
    quita el ULTIMO caracter de `value` (misma simplificacion "sin cursor,
    siempre al final" ya declarada en `append_control_value`/`type_text`),
    dispara `"input"` despues (orden real: primero cambia el contenido,
    despues se notifica) y fuerza un relayout (el nuevo `value` mas corto
    puede cambiar cuanto envuelve el texto, aunque el motor todavia no lo
    PINTE - ver mas abajo). Cualquier otra tecla suelta (`Enter`, `Tab`,
    una letra) no muta nada por su cuenta - escribir texto de verdad sigue
    siendo trabajo de `type_text`.
  - **`checked` real de checkbox/radio**: clicar un `input[type=checkbox]`/
    `input[type=radio]` antes disparaba `"click"` sin cambiar ningun
    estado - visualmente y semanticamente, marcar la casilla no existia.
    `toggle_checked` conmuta el atributo booleano `checked` (presencia =
    marcado, ausencia = sin marcar, semantica HTML real) ANTES de disparar
    `"click"` (asi es el orden real: la accion por defecto ya ocurrio
    cuando el listener ve el evento) y dispara `"change"` despues. El
    nuevo campo `ElementAttributes::checked` (protocolo NDJSON,
    `InteractiveElement.attributes.checked`) lo expone al puente para que
    el frontend (que overlaya inputs HTML reales posicionados sobre la
    captura del motor - ver mas abajo) pueda reflejarlo. Simplificacion
    declarada: SIN comportamiento de grupo para `radio` (marcar uno NO
    desmarca a los demas `input[type=radio]` del mismo `name` - el spec
    real si lo hace; exigiria recorrer el DOM entero buscando
    coincidencias por `name`, fuera del alcance de esta tarea).
  - **Por que el PINTADO del `value`/`checked` sigue sin tocarse**: el
    `value` de un `<input>`/`<textarea>` (y su `checked`) YA viajan de
    vuelta al frontend via `InteractiveElement` en cada `state_response` -
    la hipotesis de trabajo (no verificada leyendo el codigo del frontend
    en esta tarea, pero consistente con que `collect_interactive_elements`
    exista y reporte posicion+valor+marcado de cada control) es que el
    frontend renderiza controles de formulario reales superpuestos
    POSICIONALMENTE sobre la captura PNG del motor, en vez de esperar que
    el motor los pinte el mismo - de ahi que `engine-layout`/`engine-gfx`
    nunca hayan necesitado un `BoxType`/`DisplayItem` para "contenido de
    input" (a diferencia de `<img>`, Fase 3.1, que si lo tiene: una imagen
    no tiene otra via de renderizado). Si esa hipotesis resultara
    incorrecta, pintar `value`/un cursor de verdad seguiria pendiente.
  - **`core::main`/`gfx::window` (la ventana winit nativa) siguen sin
    ninguna fuente de teclado** - cero cambios en esta tarea. Es un camino
    secundario de desarrollo/pruebas del motor, no el que usa el producto
    real (`engine_server.exe` vía NDJSON/WebSocket, que si gano todo lo de
    arriba) - queda como hueco conocido, no honestamente resuelto todavia.
  - Tests reales: `is_checkable_input`/`is_text_control` clasificando cada
    combinacion de tag/type, `toggle_checked` conmutando en ambas
    direcciones (incluido un checkbox que ya venia marcado en el HTML),
    `backspace_control_value` quitando exactamente un caracter (y no
    haciendo nada sobre un valor ya vacio), y `dispatch_keyboard_event`/
    `dispatch_event` verificados AMBOS por separado (uno deja `.key`
    poblado, el otro lo deja `undefined`, sin regresion cruzada). Verificado
    en vivo end-to-end contra `engine_server.exe` real (protocolo NDJSON,
    proceso hijo real, sin mocks): marcar/desmarcar un checkbox dos veces
    seguidas, escribir "XYZ" en un campo con valor inicial, borrar un
    caracter con `Backspace`, y un listener JS real de `keydown` leyendo
    `event.key` y escribiendolo en otro elemento - las 4 verificaciones
    coincidieron exactamente con lo esperado.
- **Navegacion real por clic en `<a href>`** (Fase 4.2): clicar un enlace
  (o cualquier DESCENDIENTE suyo - el texto, un `<b>` decorativo dentro,
  el caso real mas comun, ver mas abajo) en `core::server::click` ahora
  dispara una navegacion de verdad: el mismo `EngineServer::navigate` que
  ya usaba el comando NDJSON `navigate` (fetch HTTP real, reparseo,
  reconstruccion completa de DOM/CSS/JS/layout), no una simulacion aparte.
  - **`find_link_href`**: sube por `Node::parent` (un `Weak`, ver su
    doc-comment en `dom/src/node.rs`) desde el nodo clicado hasta encontrar
    el `<a>` mas cercano - EMPEZANDO por el propio nodo, porque un clic
    real casi nunca aterriza en el `<a>` mismo sino en su contenido (texto
    suelto, un `<b>`/`<span>` decorativo). Se detiene en el PRIMER `<a>`
    que encuentra, tenga `href` navegable o no - no sigue subiendo mas
    alla buscando uno exterior (un `<a>` anidado dentro de otro es HTML
    invalido de todas formas; el propio parser real - `html5ever`,
    verificado en vivo leyendo el arbol resultante - ya los separa en
    hermanos en vez de dejarlos anidados, via el algoritmo de adopcion del
    spec HTML5, asi que ese caso ni siquiera es alcanzable en la practica).
    Lo unico que no produce ninguna accion es un `href` vacio/ausente (un
    `<a>` sin `href` no es ni siquiera un hyperlink real). Las anclas
    internas (`href="#seccion"`) y `javascript:`, que en el momento de esta
    fase se descartaban como "no navegables", tienen su propia accion real
    desde la Fase 6 (ver mas abajo): esta funcion pasó de devolver un
    `Option<String>` a un `Option<LinkAction>` con las tres posibilidades.
  - **Resolucion de URL relativa**: el `href` (posiblemente relativo) se
    resuelve contra la URL de la pagina ACTUAL (`page.url`, re-parseada con
    `url::Url::parse` + `.join(href)`) - mismo patron ya establecido para
    `<link href>`/`<script src>`/`<img src>` en `fetch_external_stylesheets`/
    `fetch_external_scripts`/`fetch_images`.
  - **Respeta `preventDefault()` real**: `DomBindings::dispatch_event`/
    `dispatch_keyboard_event` (y `JsRuntime::dispatch_event`/
    `dispatch_keyboard_event`) cambiaron su tipo de retorno de
    `JsResult<()>`/`Result<(), JsError>` a `JsResult<bool>`/
    `Result<bool, JsError>` - el `bool` es si algun listener llamo
    `event.preventDefault()`, leido directamente del objeto `Event` tras el
    bubbling (`event_default_prevented`, nueva). Cambio NO disruptivo para
    los mas de 10 sitios que ya llamaban a estas funciones (todos
    ignoraban el valor `Ok`, `.expect()`/`if let Err(...)`/`.is_ok()` -
    ninguno necesito cambios de logica, solo `main.rs` tuvo que cambiar un
    `Ok(())` literal por `Ok(_)` en un `match`). `click` en `server.rs`
    ahora SI usa el valor: si el listener de `"click"` llamo
    `preventDefault()`, la navegacion por `<a href>` se CANCELA - igual
    que un navegador real (un `<a>` con un handler de JS que hace
    `e.preventDefault()` para manejar la navegacion el mismo, p.ej. una
    SPA, no debe recargar la pagina).
  - **`click` paso a ser `async`**: antes era sincrona; ahora puede
    `.await` una navegacion real. La deteccion/resolucion del enlace
    ocurre DENTRO del prestamo de `self.current_page` (necesita `page.url`
    para resolver el `href`), pero la llamada a `self.navigate(...).await`
    en si ocurre DESPUES de que ese prestamo termine (en un bloque `{ }`
    separado) - `navigate` reasigna `self.current_page` entero, no puede
    convivir con un prestamo activo de la pagina actual.
  - **Bug real encontrado y arreglado en la propia infraestructura de
    tests, no en el codigo de produccion**: el helper de test `find(html,
    id)` (usado por decenas de tests ya existentes en `server.rs`) solo
    devolvia el nodo buscado, dejando que la UNICA referencia FUERTE al
    arbol DOM completo (`dom`, local a la funcion) se liberara al
    terminar - como `Node::parent` es un `Weak` (a proposito, para no
    crear un ciclo de referencias con `children`), cualquier test que
    necesitara subir a un ANCESTRO del nodo devuelto encontraba
    `parent.upgrade()` devolviendo `None` sistematicamente, aunque el
    arbol se hubiera construido bien. Invisible hasta esta tarea porque
    ningun test anterior necesitaba subir mas alla del propio nodo
    devuelto. Arreglado con `std::mem::forget(dom)` dentro del helper
    (inofensivo en un proceso de test de corta vida) - en produccion esto
    nunca ocurre, `LoadedPage::page::dom_root` mantiene el arbol entero
    vivo mientras la pagina este cargada.
  - Tests reales: `find_link_href` sobre el propio `<a>`, subiendo desde un
    descendiente, sin ancestro `<a>` en absoluto, y las 4 formas de
    `href` no navegable (ausente/vacio/ancla/`javascript:`); `dispatch_event`
    devolviendo `true`/`false` segun si un listener llamo `preventDefault()`.
    Verificado en vivo end-to-end contra `engine_server.exe` real (dos
    paginas HTTP reales servidas localmente): clicar un `<b>` DENTRO de un
    `<a href="pagina2.html">` navego de verdad (la URL y el `<title>` del
    estado devuelto cambiaron a los de la pagina 2), y clicar un enlace
    cuyo listener de `"click"` llama `preventDefault()` NO navego (la URL
    se quedo en la pagina original).
  - NO implementado (en el momento de esta fase, TODOS resueltos despues):
    anclas internas (`href="#id"`, deberia hacer scroll - Fase 6.1),
    `javascript:` (deberia ejecutar el script - Fase 6.2),
    `target="_blank"` (Fase 4.5), historial atras/adelante tras la
    navegacion (Fase 4.4,
    todavia no existe ningun historial en absoluto en este punto), y lo
    mismo que el resto de Fase 4.1: `core::main`/`gfx::window` (la ventana
    winit nativa) NO ganaron esta capacidad - solo el camino NDJSON real
    (`core::server`) la tiene.
- **`fetch()` real** (Fase 4.3, `engine-js::fetch`, nuevo): peticion HTTP DE
  VERDAD via `engine-net` (el mismo `NetworkEngine`/cliente HTTP/pool de
  conexiones que ya usa el resto del motor, no uno nuevo), resuelta como
  una `Promise` real de Boa - `await fetch(url)` y `fetch(url).then(...)`
  funcionan tal cual en JS, con encadenamiento de promises real
  (`.then().then()`, incluyendo aplanar una promise devuelta DESDE un
  callback `.then`, verificado en vivo).
  - **Simplificacion de concurrencia declarada, la mas importante de esta
    tarea, no un bug escondido**: el motor de scripts (`Context::eval`)
    es siempre SINCRONO de punta a punta - nunca hay un `.await` de Rust
    corriendo DENTRO de la pila de llamadas de un script JS (`core::
    server::navigate`/`click` hacen TODO el trabajo async en Rust ANTES
    de invocar `runtime.eval`, nunca durante). La cola de trabajos POR
    DEFECTO de Boa (`SimpleJobQueue`, la unica que usa `JsRuntime::new`)
    resuelve `enqueue_future_job` con `pollster::block_on` - bloqueando
    el hilo actual hasta que el future termine, no liberandolo. Asi que
    `fetch()`, aunque tiene la FORMA de API correcta de principio a fin,
    en la practica BLOQUEA el hilo que esta evaluando el script hasta que
    la peticion HTTP real termine. Un fetch NO bloqueante de verdad
    exigiria reestructurar la ejecucion de scripts para intercalarse con
    jobs de tokio pendientes DURANTE el script, no solo antes o despues -
    fuera del alcance de esta tarea. Aceptable para el uso real de este
    motor hoy: `core::server` procesa un comando NDJSON a la vez, sin
    trabajo concurrente que este bloqueo pudiera interferir - y el hilo
    bloqueado es un worker de tokio dentro de un runtime MULTI-hilo
    (`#[tokio::main]`, su modo por defecto), no el unico hilo del
    proceso.
  - **Registrado por pase-de-mano, no una dependencia nueva de
    `pipeline.rs`**: `engine-js` gano su primera dependencia de red
    (`engine-net`, antes solo `engine-dom`/`engine-css`) - pero
    `core::pipeline`/`core::scripting` siguen sin RESOLVER ninguna URL ni
    llamar a `NetworkEngine::fetch` ellos mismos, solo reenvian un
    `Option<Arc<NetworkEngine>>` opaco hacia `JsRuntime::register_fetch`
    (nueva). `Some` (siempre en `core::server`, ANTES de correr el primer
    `<script>` de la pagina, para que lo vea disponible desde la carga
    inicial, no solo listeners tardios) registra `fetch` de verdad;
    `None` (siempre en `core::main`, que no descarga recursos externos
    por diseño) deja `fetch` SIN DEFINIR - `fetch(...)` en JS lanza
    `ReferenceError: fetch is not defined`, la respuesta honesta cuando
    de verdad no hay red disponible en ese contexto, en vez de fingir un
    `fetch` que nunca conecta a nada.
  - **El puente Rust<->Boa a mano**: `JsPromise::new_pending(context)` da
    una promise pendiente + un par `resolve`/`reject` invocable MAS
    TARDE, desde Rust, despues de que el script que llamo a `fetch`
    ya devolvio el control. El future real (la peticion HTTP en si) NO
    puede tocar `Context` (no es `Send`/`'static` de esa forma) - solo
    hace trabajo Rust puro y produce un `NativeJob` como resultado,
    encolado via `context.job_queue().enqueue_future_job(...)`; ESE job,
    ejecutado despues CON `context` real disponible, es quien construye
    el objeto `Response` y llama `resolve`/`reject`. Mismo patron interno
    que usa `JsPromise::from_future` (leido en el codigo fuente de
    `boa_engine` para entenderlo), adaptado a mano porque `from_future`
    exige que el future produzca el `JsValue` el mismo, y construir un
    objeto `Response` RICO (con metodos `.text()`/`.json()`) necesita
    `Context`, que el future no tiene.
  - **Captures `Trace`-ables**: las "captures" de
    `NativeFunction::from_copy_closure_with_captures` (el `Arc<NetworkEngine>`
    del cliente HTTP, el `Result<String, String>` del cuerpo ya
    descargado para `.text()`/`.json()`) deben implementar `Trace` (el
    recolector de basura de Boa necesita saber que recorrer) - se
    declaran con `boa_gc::empty_trace!()`, correcto y no un atajo
    inseguro: ninguna de las dos contiene ningun valor de Boa
    (`JsValue`/`JsObject`/`Gc<T>`), son datos Rust puros ajenos al heap
    de Boa.
  - **`response.json()` reusa el `JSON.parse` REAL de Boa**, invocandolo
    como si fuera JS (buscando `JSON.parse` en el objeto global y
    llamandolo) en vez de reinventar un parser JSON propio - un JSON
    invalido lanza de forma natural (un `SyntaxError` real de Boa),
    capturado y convertido en promise rechazada.
  - Sin `options` (metodo/headers/body de la peticion - `engine-net`
    mismo todavia no envia cuerpo de peticion en ninguna forma, gap
    preexistente sin relacion con esta tarea): solo `fetch(url)`, siempre
    GET. Sin la clase `Headers` real (`response.headers` es un objeto
    plano nombre-minuscula -> valor, no `Headers` con `.get()`/`.has()`/
    iteracion). Sin `XMLHttpRequest` **cuando se escribio esto** - ya no:
    llego en la Fase 9 (ver mas arriba), sobre este mismo
    `NetworkEngine`. El razonamiento de entonces ("`fetch` cubre el caso
    real") resulto ser falso en la practica: hay demasiado codigo real,
    empezando por versiones de jQuery que se siguen sirviendo hoy, que
    nunca migro.
  - Tests reales: forma del objeto `Response` (status/ok/statusText/url/
    headers) para 200 y 404, `.text()` resolviendo al cuerpo real,
    `.json()` parseando JSON valido Y rechazando JSON invalido (sin
    peticion HTTP real en NINGUNO de estos - una `NetworkResponse` de
    prueba se construye a mano, igual que los propios tests de
    `engine-net::http_client` prueban su logica sin tocar la red),
    `fetch` registrado como global real, y una URL invalida rechazando
    SIN tocar la red (falla al parsear antes de que exista un future que
    ejecutar). Verificado en vivo end-to-end contra `engine_server.exe`
    real (servidor HTTP local real sirviendo JSON): `fetch(url).then(r =>
    {...; return r.json()}).then(data => {...})` con status/ok/`.json()`
    correctos, y un `fetch` a una URL que responde 404 resolviendo
    (NO rechazando - asi es el spec real, un error HTTP no es un error de
    red) con `status:404 ok:false`.
- **Historial atras/adelante** (Fase 4.4, `core::server`): `EngineServer`
  mantiene `history: Vec<String>` (URLs finales, post-redireccion) +
  `history_index: Option<usize>` (posicion actual; `None` hasta la primera
  navegacion exitosa). Semantica real de navegador: visitar una pagina
  NUEVA tras haber ido "atras" TRUNCA cualquier entrada "adelante" que
  quedara por delante (`Vec::truncate` antes de `push`) - verificado en
  vivo (navegar a una URL nueva desde mitad del historial deja
  `can_go_forward: false`, e intentar `forward` despues falla).
  - **`back`/`forward` siempre vuelven a pedir la pagina por red de
    verdad** (mismo `navigate` que una visita nueva) - simplificacion
    declarada, NO restauran un snapshot en memoria (`bfcache` de un
    navegador real). Mas honesto que fingir una cache que no existe, a
    cambio de poder fallar si la pagina ya no es alcanzable (mismo riesgo
    que ya acepta cualquier `navigate` normal) y de recargar recursos
    externos/re-ejecutar scripts en cada vuelta (un navegador real con
    bfcache no lo haria).
  - **Mismo `navigate`, con un flag para no auto-destruirse el historial**:
    `navigate` gano un parametro `record_history: bool` - `true` para una
    navegacion nueva (comando NDJSON `navigate`, o clicar un `<a href>` -
    Fase 4.2) empuja al historial; `false` para `back`/`forward` (que YA
    movieron `history_index` ellos mismos antes de llamar) evita que
    `navigate` vuelva a empujar y descarte el historial "adelante" al que
    `back` deberia poder volver despues.
  - **`can_go_back`/`can_go_forward` en cada `State`** (protocolo NDJSON,
    nuevos) - calculados directamente de `history_index`/`history.len()`,
    para que el frontend pueda habilitar/deshabilitar sus botones de
    atras/adelante sin llevar su propia copia paralela del historial.
  - Tests reales: `back`/`forward` sin ningun historial todavia reportan
    un error honesto (en vez de un no-op silencioso) SIN tocar la red -
    fallan en el guard de `history_index: None` antes de llegar a llamar
    `navigate`; `state_response` sin ninguna pagina cargada reporta
    `can_go_back`/`can_go_forward` en `false`. La logica de push/truncate
    en si (necesita `navigate` real, red de por medio) se verifico en vivo
    end-to-end: 3 navegaciones reales a paginas distintas, `back` x2,
    intentar ir mas atras del principio (error honesto), `forward`, y
    navegar a una URL nueva desde mitad del historial truncando
    "adelante" - las 6 verificaciones (URLs, titulos, `can_go_back`/
    `can_go_forward` y los dos errores esperados) coincidieron exactamente
    con lo esperado en cada paso.
  - NO implementado EN ESTA FASE (los tres primeros llegaron en la Fase 7,
    ver mas abajo): `bfcache` (sigue sin existir), `popstate` real hacia JS
    y `history.pushState`/`replaceState`; y lo mismo que el
    resto de Fase 4: `core::main`/`gfx::window` (la ventana winit nativa)
    NO ganaron esta capacidad - solo el camino NDJSON real (`core::server`)
    la tiene.
- **Pestañas reales** (Fase 4.5, `core::server`): antes de esta fase,
  `EngineServer` solo podia tener UNA pagina cargada a la vez (`current_page`
  directamente en el struct, junto con `history`/`history_index`/
  `scroll_offset_y`). Se extrajo todo eso a un nuevo struct `Tab { id,
  current_page, history, history_index, scroll_offset_y }`, y `EngineServer`
  paso a llevar `tabs: Vec<Tab>` (invariante: nunca vacio) + `active_tab:
  usize` (indice, NO id) + `next_tab_id: u32` (monotono, nunca se reutiliza).
  `width`/`height` se QUEDARON en `EngineServer` (son el tamaño de la
  VENTANA, compartido por todas las pestañas - cambiar de pestaña no cambia
  el tamaño de la ventana, igual que un navegador real).
  - **Protocolo NDJSON nuevo**: `new_tab` (con `url` opcional - sin ella la
    pestaña queda en blanco), `close_tab`/`switch_tab` (por `tab_id`, el id
    ESTABLE de la pestaña, no su posicion en la lista), `list_tabs` (sin
    parametros propios, devuelve `EngineResponse::Tabs { tabs: Vec<TabInfo>,
    active_tab_id }` con el titulo/URL de CADA pestaña abierta). `State`
    (la respuesta de `navigate`/`click`/`back`/`forward`/etc.) gano un campo
    `tab_id` - a que pestaña pertenece el estado devuelto.
  - **`target="_blank"` real, antes declarado explicitamente NO
    implementado** (ver la entrada de Fase 4.2 mas arriba): `find_link_href`
    se renombro a `find_link_target` y ahora devuelve tambien
    `opens_new_tab` (si el MISMO `<a>` navegable lleva `target="_blank"`,
    comparacion insensible a mayusculas) - `click` en `core::server` llama a
    `open_new_tab` en vez de `navigate` cuando `opens_new_tab` es `true`.
    Simplificacion declarada: solo se reconoce `target="_blank"`
    exactamente - cualquier OTRO valor de `target` (un nombre de frame
    inventado, `_parent`, `_top`...) navega en la misma pestaña, un
    navegador real tambien abriria pestaña nueva para un nombre de frame
    que no existe, este motor no.
  - **`open_new_tab` SIEMPRE enfoca la pestaña recien creada** (igual que
    `target="_blank"` real) - sin comportamiento de "abrir en segundo
    plano" (p.ej. Ctrl+clic en un navegador real), fuera del alcance de
    esta tarea.
  - **Cerrar la pestaña ACTIVA activa la que queda a su IZQUIERDA** (o la
    nueva primera, si se cerraba la de mas a la izquierda) - mismo criterio
    que la mayoria de navegadores reales. Cerrar una pestaña en SEGUNDO
    PLANO no cambia cual esta activa. Error honesto (no un no-op
    silencioso) al intentar cerrar la UNICA pestaña abierta (un navegador
    real cerraria la ventana entera, fuera del alcance de este servidor,
    que siempre mantiene al menos una) o una `tab_id` que no existe.
  - **Relayout perezoso de pestañas en segundo plano**: `resize` (cambio de
    tamaño de ventana) solo relayouta la pestaña ACTIVA en ese momento, no
    las demas - pagar un relayout completo por cada pestaña en segundo
    plano en cada `resize` seria trabajo desperdiciado si el usuario nunca
    vuelve a esa pestaña. `switch_tab` relayouta (y reclampa el scroll de)
    la pestaña recien activada antes de devolver su estado, para ponerla al
    dia si el tamaño de ventana cambio mientras estaba en segundo plano.
  - Tests reales (15 nuevos): `find_link_target` distingue `target="_blank"`
    (con variante en mayusculas) de `_self`/ausente/cualquier otro valor;
    `open_new_tab` crea una segunda pestaña y la activa; `close_tab` sobre
    la unica pestaña / sobre un id inexistente reportan error; cerrar la
    pestaña activa activa la de la izquierda (o la nueva primera si era la
    de mas a la izquierda); cerrar una pestaña en segundo plano no cambia
    la activa; `switch_tab` sobre un id inexistente reporta error;
    `list_tabs` reporta todas las pestañas con la activa marcada.
    Verificado en vivo end-to-end contra `engine_server.exe` real (tres
    paginas HTTP reales servidas localmente): navegar la pestaña 1, abrir
    una pestaña 2 en blanco a mano y navegarla, volver a la pestaña 1 y
    clicar un enlace `target="_blank"` (abrio una pestaña 3 NUEVA con la
    pagina correcta y la activo), clicar despues un enlace NORMAL en la
    pestaña 1 (navego la MISMA pestaña, sin crear ninguna otra), cerrar la
    pestaña 3 en segundo plano (la activa no cambio), cerrar con un id
    inexistente (error), y cerrar hasta quedar con una sola pestaña e
    intentar cerrarla tambien (error) - las 9 verificaciones coincidieron
    exactamente con lo esperado en cada paso.
  - NO implementado: "abrir en segundo plano" (ver arriba), reordenar
    pestañas arrastrando, `window.open()` desde JS (crearia una pestaña
    nueva sin pasar por un clic real - sin ningun binding hacia
    `open_new_tab` desde `engine-js` todavia), y lo mismo que el resto de
    Fase 4: `core::main`/`gfx::window` (la ventana winit nativa) NO
    ganaron esta capacidad - solo el camino NDJSON real (`core::server`)
    la tiene. El frontend React real (`frontend/`) tampoco consume este
    protocolo todavia en absoluto - habla con un backend Python/Playwright
    completamente distinto por WebSocket en el puerto 8000, sin ninguna
    conexion con este motor todavia. Esta fase implementa pestañas del
    LADO DEL MOTOR (protocolo +
    `core::server`), no una barra de pestañas visual en ningun frontend -
    de ahi que el nombre original de esta tarea en el plan ("pestañas en
    el frontend") sea impreciso: no existe todavia ningun frontend real
    conectado a este motor al que añadirle una.
- **Rendimiento: descarte por viewport al pintar** (Fase 5, continuacion;
  `engine-gfx::paint`). El motor pintaba TODOS los `DisplayItem` de la
  pagina en cada respuesta, incluidos los que caen fuera de la pantalla.
  - **Medido antes de tocar nada**, igual que la cache de fuentes de la
    Fase 5.1: un benchmark temporal separo las tres etapas de
    `render_layout_to_png` sobre una pagina de 200 filas a 1000x800.
    Resultado que contradijo la sospecha inicial (el PNG): construir el
    display list 0.16 ms, **pintar 182.93 ms**, codificar el PNG 3.30 ms.
    Pintar era el 98% del coste. Y con ~4800px de contenido en un viewport
    de 800px, cinco sextos de ese trabajo eran para pixeles que no podian
    verse.
  - El arreglo es una comprobacion por item: si su rectangulo cae entero
    por encima o por debajo del pixmap (con un margen de 64px, porque una
    sombra se difumina mas alla de su caja y los glifos sobresalen del alto
    de linea), se salta. `PushClip`/`PopClip` **nunca** se descartan: no
    pintan, cambian estado, y saltarselos desemparejaria la pila de
    recorte. Solo se comprueba el eje vertical - es donde el contenido se
    desborda de verdad y donde esta todo el ahorro.
  - **Medido despues, en vivo contra `engine_server.exe`** (release, misma
    pagina de 200 filas): `get_state` 157 -> 36 ms (4.3x), `scroll`
    190 -> 46 ms (4.1x), `click` 243 -> 70 ms (3.5x), `navigate`
    341 -> 235 ms (1.45x - ahi el coste dominante es parsear y hacer el
    layout, no pintar). El benchmark temporal se borro despues; los tests
    de regresion se quedan.
  - **La correccion esta PROBADA, no argumentada**: tres tests comparan el
    PNG completo byte a byte - pintar una pagina larga da un resultado
    identico al de pintar solo lo que cae dentro del viewport (hacia
    abajo y, tras hacer scroll, hacia arriba), y el tercero exige lo
    contrario para una caja que asoma por el borde (quitarla SI cambia los
    pixeles), que es lo que detectaria un descarte demasiado agresivo. Mas
    6 tests sobre la geometria del descarte, incluido que `PushClip`/
    `PopClip` no exponen rectangulo con el que descartarlos.
  - **Lo que sigue sin hacerse**, para no dejarlo implicito: el texto se
    re-conforma (`wrap_text` + `shape_text`, rustybuzz) en CADA pintado,
    aunque no haya cambiado - una cache de shaping es el siguiente paso
    obvio y explica buena parte de los 36 ms que quedan. Tampoco hay
    layout incremental (cualquier interaccion reconstruye el arbol
    entero), ni repintado por rectangulos sucios (siempre se rasteriza el
    viewport completo), ni se ha eliminado el doble parseo de HTML en
    `navigate`.
- **`XMLHttpRequest` real** (Fase 9, `engine-js::xhr` nuevo), sobre el MISMO
  `NetworkEngine` que `fetch()` (Fase 4.3) y el resto del motor - sin
  cliente HTTP nuevo. No es redundante con `fetch`: una parte enorme de la
  web real, incluidas versiones de jQuery que se siguen sirviendo hoy, nunca
  migro, y un motor que solo tiene `fetch` deja esas paginas sin red aunque
  el transporte de debajo sea identico.
  - **Sincrono SIEMPRE, declarado**: el tercer argumento de `open(metodo,
    url, async)` se acepta y se ignora - `send()` hace la peticion y llama a
    los manejadores antes de devolver el control. Es la semantica exacta de
    `open(..., false)` del spec aplicada tambien al caso `true`, y es la
    unica que este motor puede cumplir: un XHR asincrono de verdad exige
    devolver el control al script y disparar `onload` mas tarde, es decir
    poder suspender y reanudar JS a mitad de ejecucion, y `Context::eval`
    de Boa es sincrono de punta a punta (la misma limitacion que `fetch.rs`
    ya declaraba desde el otro lado). Fingir asincronia daria un XHR que a
    veces no dispara nada. Consecuencia practica: `xhr.onload = ...;
    xhr.send();` funciona igual que en un navegador; lo que cambia es el
    ORDEN de lo que se escriba despues de `send()`, no los datos.
  - **404 no es un error de red**: `onload` se dispara para cualquier
    respuesta que haya llegado (incluidos 404 y 500) y `onerror` solo
    cuando no hubo respuesta ninguna, dejando `status = 0`. Confundir los
    dos casos es el fallo clasico de un XHR mal hecho; los dos estan
    probados por separado, en unit tests y en vivo.
  - Recorre los cinco `readyState` de verdad (0..4, aunque sin espera entre
    ellos), expone las constantes (`xhr.DONE`...), `getResponseHeader`
    (`null` si falta, no `""`), `getAllResponseHeaders` (formato exacto del
    spec: minusculas, ordenado, CRLF) y `setRequestHeader`, que SI se
    aplica a la peticion real.
  - NO implementado, declarado: `responseType` (`response` es siempre la
    misma cadena que `responseText`), `abort()`/`timeout`/
    `withCredentials` (los tres solo tienen sentido sobre una peticion en
    vuelo, y aqui nunca hay una), eventos `progress`/`loadstart`/`loadend`,
    cuerpo de peticion en `send(body)` (`engine-net` todavia no lo envia -
    misma limitacion que `fetch`), `addEventListener` sobre el XHR (el
    registro de eventos esta indexado por nodo del DOM y un XHR no es un
    nodo), y `this` dentro de los manejadores (el patron normal de capturar
    el xhr por cierre si funciona).
  - 12 tests nuevos + verificacion en vivo contra `engine_server.exe`: un
    GET real devolvio `200` con el JSON del servidor y sus cabeceras, los
    estados pasaron `1-2-3-4`, un 404 disparo `onload` con `status=404`, y
    un puerto sin nadie escuchando disparo `onerror` con `status=0`.
- **Bug real arreglado al construir la Fase 9: la cola de trabajos no se
  vaciaba tras un evento** (`engine-js::runtime::drain_jobs`). Hasta aqui
  lo unico que la drenaba era `eval`, al terminar cada `<script>`. La
  consecuencia era que **`fetch()` solo funcionaba durante la CARGA de la
  pagina**: llamado desde un manejador de eventos - que es como lo usa
  cualquier pagina real - la peticion HTTP se hacia de verdad y la
  respuesta llegaba, pero el trabajo que resuelve la `Promise` se quedaba
  encolado para siempre, asi que ni `.then(...)` ni `await` llegaban a
  ejecutarse nunca y la pagina se quedaba a medias sin ningun error
  visible. Reproducido en vivo antes del arreglo (una pagina cuyo manejador
  de clic hacia `fetch` se quedaba en "llamando..." indefinidamente,
  mientras el mismo `fetch` en la carga si completaba) y verificado
  despues. Drenar ahi no es un parche: en el spec, disparar un evento por
  una accion real del usuario ES una tarea del bucle de eventos, y al final
  de CADA tarea se vacia la cola de microtasks - la misma razon por la que
  `eval` ya lo hacia. 3 tests de regresion que lo miden con
  `queueMicrotask` (sin necesitar red, asi que corren siempre), incluido el
  complementario que comprueba que sin drenar el microtask NO corre.
- **CSSOM: `getComputedStyle` y `getBoundingClientRect` reales** (Fase 8,
  `engine-js::cssom` nuevo + `core::server` + `layout::tree`). Las dos APIs
  con las que cualquier codigo real mide la pagina antes de reaccionar a
  ella (menus que se posicionan, lazy-loading, animaciones, drag & drop).
  - **El problema de fondo, y el diseño que sale de el**: las dos leen el
    ARBOL DE LAYOUT, no el DOM - y el runtime JS se construye ANTES de que
    ese arbol exista (`build_page_keeping_runtime` corre los `<script>` y
    hace el layout despues, igual que un navegador real donde los scripts
    corren durante el parseo). Un navegador resuelve esa inversion
    forzando un *reflow sincrono*: `getBoundingClientRect()` para el
    mundo, rehace el layout ahi mismo y devuelve el resultado fresco. Este
    motor no puede: construir un layout necesita hoja de estilos,
    `FontSet` y mapa de imagenes, que viven en `core::server`, una capa
    por encima e inalcanzable desde dentro de un closure de Boa. Asi que
    el puente es un **snapshot**: `core::server` PUBLICA geometria y
    estilo resuelto tras cada layout, y JS lee. Es el mismo patron de
    `window.open` (6.4) e `history.pushState` (7) pero en direccion
    contraria (alli JS escribe y el servidor drena).
  - **Consecuencias declaradas de que sea snapshot y no reflow**: (a)
    durante la CARGA de la pagina esta vacio, asi que un `<script>` que
    mida en ese momento recibe un rect de ceros y un estilo sin
    propiedades - que es exactamente lo que devuelve un navegador real
    para un elemento fuera del arbol de render, y donde estas APIs se usan
    de verdad (dentro de un listener) el snapshot ya esta publicado; (b)
    mutar el DOM no lo actualiza al instante: cambiar `el.style.width` y
    medir acto seguido da la geometria de ANTES; el layout que
    `core::server` hace al terminar de procesar ese mismo clic lo pone al
    dia. Es la limitacion real de la fase y no se disimula.
  - **`engine-js` NO gano dependencia de `engine-layout`**: lo que cruza
    la frontera son datos planos (`BoxMetrics`: cuatro `f32` y el mapa de
    declaraciones), copiados en `core` - el unico crate que ya conoce las
    dos capas. Coste: un clon de `HashMap` por caja y layout, despreciable
    al lado del propio layout (que hace shaping de texto real).
  - **Bug real encontrado y arreglado: la herencia no llegaba a las cajas
    de ELEMENTO** (`layout::tree::build_node`). Solo se escribia una
    propiedad heredable en la caja de un elemento si ESE elemento la
    declaraba; lo heredado viajaba en el acumulador `inherited` y solo
    aterrizaba en las cajas de TEXTO. Bastaba para pintar (color y tamaño
    de letra solo hacen falta donde hay texto), pero dejaba la caja del
    elemento diciendo media verdad, y `getComputedStyle` - que por
    definicion devuelve el valor DESPUES de la herencia - no tenia de
    donde sacarlo: un `<div>` dentro de un `<body>` con `color` reportaba
    `""`. Arreglado con un `or_insert` (lo propio siempre gana sobre lo
    heredado, que es el orden de la cascada) colocado ANTES del bucle que
    resuelve unidades relativas, para que un `font-size: 2em` propio siga
    resolviendose contra el del padre. 3 tests de regresion.
  - **Simplificaciones honestas de `getComputedStyle`**: (1) solo lleva lo
    que la cascada resolvio de verdad - una propiedad que nadie definio da
    `""`, NO su valor inicial del spec, porque este motor no tiene tabla
    de valores iniciales y fingirla seria inventar; (2) devuelve valores
    ESPECIFICADOS (`"2em"`, `"50%"`), no los usados en px/`rgb()` de un
    navegador real; (3) es de solo lectura (como el real, que lanza
    `NoModificationAllowedError`) - para escribir esta `el.style`, que si
    es vivo; (4) el segundo argumento (`pseudoElt`) se acepta y se ignora:
    no hay pseudo-elementos. Expone `getPropertyValue`, `length`, `item(i)`
    y cada propiedad por sus dos nombres (`background-color` y
    `backgroundColor`), y se cuelga tambien de `window` para que la forma
    canonica del spec funcione.
  - **`getBoundingClientRect` devuelve coordenadas de VIEWPORT** (documento
    menos scroll), que es lo que define el spec - a diferencia de
    `elements[].rect` del protocolo NDJSON, que son de documento. Lee
    `LayoutBox::dimensions`, que es la caja de BORDE (su
    `box_dimensions.border_box()` la reconstruye identica), justo lo que
    describe un `DOMRect`. Incluye los 8 campos
    (`x`/`y`/`width`/`height`/`top`/`right`/`bottom`/`left`) y
    `getClientRects()`, que aqui siempre da 0 o 1 rectangulos porque no
    hay fragmentacion de inlines.
  - **Refactor de paso**: el bloque de relayout estaba copiado literal en
    seis sitios (`resize`, `switch_tab`, `click`, `type_text`,
    `press_key`, `relayout_active_tab`); ahora es `LoadedPage::relayout`,
    que ademas publica el snapshot - asi no depende de acordarse en cada
    sitio nuevo. `EventRegistry` paso a llamarse `DocumentBindings`: al
    entrar el snapshot de layout junto a los listeners, el nombre viejo
    pasaba a ser falso. Y el `downcast` a `ElementCapture`, inlineado en 6
    sitios, es ahora un solo `node_from_js_value`.
  - Tests reales: 12 nuevos en `engine-js::cssom` + 3 de regresion de
    herencia en `layout` (396 en total, de 387). Verificado ademas en vivo
    contra `engine_server.exe` contrastando DOS caminos independientes:
    `getBoundingClientRect()` (JS -> snapshot) devolvio exactamente el
    mismo rectangulo que `elements[].rect` del protocolo (Rust -> arbol de
    layout), y tras un scroll de 260px el `top` de un elemento a 880 del
    documento paso a 620 - si el snapshot no se enterara del scroll, los
    dos caminos discreparian.
  - NO implementado: reflow sincrono (ver arriba), valores usados,
    pseudo-elementos, `offsetTop`/`offsetWidth`/`scrollTop`,
    `element.matches`/`closest`, y el `CSSOM` de hojas de estilo
    (`document.styleSheets`, `insertRule`).
- **`history.pushState`/`replaceState` + `popstate` reales** (Fase 7,
  `engine-js::history` nuevo + `core::server`): lo que convierte a este
  motor en capaz de correr una SPA sin destruirla. Hasta la Fase 4.4,
  `back` SIEMPRE volvia a pedir la pagina por red; en una SPA eso equivale
  a perder la sesion entera cada vez que el usuario pulsa "atras".
  - **Identidad de documento, la pieza central**: una entrada de historial
    dejo de ser un `String` con la URL y paso a ser un `HistoryEntry { url,
    document_id }`. Cada CARGA real de documento estrena `document_id`
    (contador monotono del servidor); las entradas creadas por `pushState`
    heredan el del documento VIVO. Con eso, `back`/`forward`
    (`traverse_history`, nuevo, nucleo compartido de ambos) distinguen los
    dos casos que el spec trata de forma radicalmente distinta:
    - **misma identidad** -> travesia DENTRO del documento: no se pide
      nada por red, solo cambia la URL y se dispara `popstate` sobre el
      runtime que ya esta corriendo (y se rehace el layout despues, porque
      un listener de `popstate` casi siempre repinta).
    - **identidad distinta** -> como hasta ahora: `navigate` de verdad.
    Al volver ENTRE documentos, la entrada destino se sella con la
    identidad del documento recien cargado - sin eso seguiria apuntando a
    un documento ya inexistente y un `pushState` posterior sobre ella
    forzaria recargas absurdas.
  - **`popstate` se dispara sobre el ELEMENTO RAIZ**, no sobre `window`:
    el registro de eventos de este motor esta indexado por nodo del DOM y
    `window` no es un nodo. Se aprovecha que el elemento raiz es el ultimo
    escalon de propagacion ANTES de `window` en el spec real (un evento
    que burbujea llega a los dos), y `window.addEventListener` se engancha
    justo ahi mediante un shim JS de tres lineas que delega en
    `document.documentElement.addEventListener` - de modo que un
    `window.addEventListener('popstate', ...)` corriente y moliente
    funciona sin que haga falta un segundo registro de eventos paralelo.
  - **`event.state` es siempre `null`, declarado**: el argumento `state` de
    `pushState` se acepta y se ignora. No es pereza - sin bfcache, volver a
    un documento distinto siempre lo reconstruye con un `JsRuntime` nuevo,
    asi que el objeto `state` original (que vive en el heap de Boa del
    runtime viejo) no puede sobrevivir de ninguna manera; serializarlo
    fingiria una fidelidad inexistente (perderia funciones, ciclos e
    identidad de objeto).
  - **`pushState` trunca lo que hubiera "adelante"**, igual que una
    navegacion normal, y resuelve URLs relativas contra la de la pagina
    actual (`pushState(null,'','/ruta')` es la forma habitual en una SPA).
    Un `replaceState` en un script de CARGA SI se honra (patron comun para
    normalizar la ruta inicial): a diferencia de `window.open`, no abre ni
    navega a nada, solo reescribe una entrada que ya existe, asi que no hay
    riesgo de bucle.
  - Tests reales (10 nuevos): los 6 de `engine-js::history` (push/replace
    encolan lo correcto, sin URL no encolan nada, drenar vacia, el orden se
    conserva, y registrar sin DOM no falla porque el shim esta guardado
    tras un `typeof`) y 4 de `core::server` sobre `apply_history_ops`
    (push añade con la identidad VIVA y mueve el indice resolviendo la URL
    relativa; replace sobrescribe sin añadir; push desde mitad del
    historial trunca lo de adelante; sin pagina cargada la operacion se
    ignora en vez de corromper el historial). Estos ultimos son posibles
    porque `build_page_keeping_runtime` con `network: None` construye una
    pagina real SIN tocar la red. Verificado ademas en vivo end-to-end
    contra `engine_server.exe` con una SPA de verdad: dos `pushState`
    cambiaron la URL sin recargar, `back` disparo `popstate` **sobre el
    mismo runtime vivo** (un contador que solo existe en memoria JS seguia
    valiendo 2 - si el motor hubiera recargado estaria a 0, que es
    exactamente lo que pasaba antes de esta fase), el DOM mutado siguio
    intacto, `forward` tampoco recargo, y un `pushState` desde mitad del
    historial descarto las entradas de adelante (el `forward` siguiente dio
    error, como debe).
  - NO implementado: `bfcache` (sigue sin existir - volver a otro documento
    lo vuelve a pedir por red), `event.state` (ver arriba), `history.back()`/
    `forward()`/`go()` llamados DESDE JS (solo existen como comandos NDJSON),
    e `history.length`.
- **Anclas internas, `javascript:`, grupos de radio y `window.open`**
  (Fase 6, `core::server` + `engine-js::window`, nuevo): los cuatro huecos
  que las Fases 4.1/4.2/4.5 habian dejado DECLARADOS como no
  implementados. Ya no queda ninguno abierto de aquellas.
  - **`href="#seccion"` desplaza de verdad** (6.1): `find_link_href` (que
    devolvia un `Option<String>`, "navegar o nada") se convirtio en
    `find_link_target -> Option<LinkAction>`, un enum con las tres cosas
    distintas que un `<a>` puede hacer: `Navigate`, `ScrollToFragment` y
    `RunScript`. El destino sale de `LayoutBox::find_box_for_node` (nuevo,
    el camino inverso a `hit_test`: de nodo a coordenadas), se acota con el
    mismo `clamp_scroll_offset` que la rueda del raton, y se resuelve
    DESPUES del relayout del clic para que un listener que haya movido el
    destino no deje el salto obsoleto. `href="#"` a secas va al principio
    del documento, como en el spec real. Un ancla rota (`id` inexistente)
    deja el scroll donde estaba, sin error - igual que un navegador real.
  - **`href="javascript:..."` ejecuta el script** (6.2): sobre el runtime
    real de la pagina (`page.runtime.eval`), antes del relayout, para que
    una mutacion del DOM hecha ahi se vea ya en la captura que devuelve ese
    mismo clic. Un error dentro del script no aborta el clic (sus eventos y
    su relayout ya ocurrieron y son reales): se registra y se sigue, como
    un navegador real que deja el error en la consola sin romper la pagina.
  - **Grupos de radio reales** (6.3): antes, checkbox y radio compartian
    `toggle_checked`, lo que era correcto para el primero y FALSO para el
    segundo en dos cosas: un radio no se desmarca al reclicarlo (en el
    spec no hay forma de desmarcarlo con un clic; antes un segundo clic
    dejaba el grupo entero sin seleccion, un estado inalcanzable en un
    formulario real) y marcar uno DESMARCA a los demas con el mismo `name`.
    Ambas cosas ya son reales (`apply_checkable_click`). Simplificacion
    declarada: el grupo se busca en el documento entero, no dentro del
    `<form>` que lo contenga - el spec agrupa por "form owner", concepto
    que este motor todavia no tiene, asi que dos formularios de la misma
    pagina que reutilicen un `name` se pisarian.
  - **`window.open(url)` abre una pestaña real** (6.4, `engine-js::window`,
    nuevo): el runtime JS vive DENTRO de una pagina y las pestañas son del
    SERVIDOR, una capa por encima, inalcanzable desde el `Context` de Boa -
    el puente es una cola compartida (`PendingWindowOpens`) donde
    `window.open` solo APUNTA la URL, que `core::server` drena tras
    procesar el clic y convierte en `open_new_tab` de verdad.
    - **`window` se registra SIEMPRE** (a diferencia de `fetch`, que es
      condicional) - no porque siempre haya pestañas que abrir, sino
      porque es el objeto que toda pagina real da por sentado: dejarlo sin
      definir haria que un `window.loQueSea` de deteccion de capacidades,
      comunisimo en la web, lanzara `ReferenceError` y rompiera la pagina.
    - **Los `window.open` de la CARGA de la pagina se descartan**, solo se
      honran los que vienen de un clic real. No es una limitacion
      disfrazada: es exactamente lo que hace el bloqueador de ventanas
      emergentes de cualquier navegador (exige "activacion del usuario"), y
      ademas evita que una pagina que llama `window.open` al cargar se
      abriera a si misma en bucle, ya que cada pestaña nueva vuelve a pasar
      por el mismo camino.
    - Devuelve `null`, no un objeto `Window` fingido - que es ademas lo que
      devuelve un navegador real cuando el bloqueador impide la apertura,
      asi que el codigo de paginas reales que comprueba el resultado ya
      sabe tratarlo. Este `window` NO es el objeto global (`var x = 1;
      window.x` da `undefined`): serlo de verdad exige un proxy con
      semantica de `WindowProxy` en Boa, y nada de lo que el motor hace hoy
      lo necesita.
  - **Bug real encontrado y arreglado en `LayoutBox::hit_test`** (no en
    codigo nuevo de esta fase, sino destapado por ella): empezaba con un
    `if !self.dimensions.contains(x, y) { return None }` que parece una
    poda razonable pero es falsa en cuanto un hijo se desborda de su
    padre - y SIEMPRE hay uno que lo hace, porque la caja RAIZ mide lo que
    el VIEWPORT, no lo que el contenido (`content_extent` existe justo por
    eso). Consecuencia: **cualquier clic sobre contenido que el usuario
    hubiera tenido que desplazar para ver no hacia absolutamente nada**.
    Invisible hasta ahora porque ninguna prueba anterior combinaba scroll
    con un clic mas abajo del primer pantallazo. Arreglado probando los
    hijos SIEMPRE y usando la caja propia solo como respaldo; el coste es
    que un clic que no acierta nada recorre el arbol entero en vez de
    salir antes, irrelevante a un clic por accion del usuario.
  - Tests reales (16 nuevos): las tres acciones de `find_link_target` por
    separado (incluido `javascript:` con mayusculas mezcladas y `#` a
    secas); checkbox conmutando en ambos sentidos vs radio que nunca se
    desmarca; un radio desmarcando solo a SU grupo (sin tocar ni a otro
    grupo ni a un checkbox que comparta `name`); radio sin `name` que no
    forma grupo; `find_box_for_node` distinguiendo por identidad de `Arc` y
    no por contenido; `hit_test` acertando un hijo desbordado muy por
    debajo de la raiz (regresion del bug de arriba) y siguiendo devolviendo
    `None` donde no hay nada; y los 5 de `window.open` (encola, vaciar la
    cola no repite, varias llamadas se encolan en orden, sin URL utilizable
    no encola nada, devuelve `null`). Verificado ademas en vivo end-to-end
    contra `engine_server.exe` real: clicar un ancla dejo la seccion
    destino DENTRO del viewport (acotada al final del documento, como debe
    ser), `href="#"` volvio arriba, un `javascript:` mutó el texto de un
    `<div>` de verdad, clicar un radio movio la seleccion dentro de su
    grupo sin tocar el otro grupo, reclicarlo lo dejo marcado, el checkbox
    si conmuto en ambos sentidos, y un `javascript:window.open(...)` abrio
    una SEGUNDA pestaña real con su pagina cargada y enfocada.
  - NO implementado: `window.open` con nombre de ventana o features
    (`window.open(url, "nombre", "width=...")` - los argumentos 2 y 3 se
    ignoran), reutilizar una pestaña por nombre, y `window` como objeto
    global de verdad (ver arriba).
- **Cache de proceso para la carga de fuentes de sistema** (Fase 5.1,
  `engine-text::font::FontSet`) - primer hallazgo real de rendimiento de
  esta fase, medido en vivo, no una optimizacion especulativa.
  `FontSet::load_default_sans_serif()` construia un `fontdb::Database`
  NUEVO y llamaba a `load_system_fonts()` (rescanea TODAS las fuentes
  instaladas en el sistema operativo desde disco) CUATRO veces por
  llamada (una por combinacion peso/estilo) - y se llamaba en CADA
  `navigate()` real (`core::server::EngineServer::navigate`), es decir,
  en cada navegacion de pagina, antes de que empezara ningun trabajo real
  de esa pagina.
  - **Medido con un benchmark desechable** (`cargo run --release --example
    bench_font`, borrado tras confirmar el arreglo - el test de regresion
    de mas abajo es lo que queda): ~500-670ms por llamada SIN cache, en
    release. Con el cache (`static DEFAULT_SANS_SERIF: OnceLock<FontSet>`,
    primera llamada paga el escaneo real, las siguientes son un `clone()`
    de los bytes ya cargados en memoria), las llamadas siguientes bajan a
    ~3ms - un factor de ~150-200x. Verificado tambien en vivo end-to-end
    contra `engine_server.exe` real (build de depuracion, no release):
    la primera `navigate` de la sesion tarda ~1.25s, las siguientes
    ~0.13-0.22s.
  - **Por que cachear UN solo `FontSet` global es correcto y no una
    simplificacion arriesgada**: hoy el motor solo sabe pedir UNA familia
    (sans-serif del sistema, `fontdb::Family::SansSerif`) - la eleccion de
    `font-family` real de la pagina todavia no existe (Fase 2.4b,
    pendiente). Nada varia todavia entre paginas ni entre pestañas, asi
    que cachear por proceso entero (no por pagina, no por pestaña) no
    pierde ninguna distincion real que el motor ya hiciera. El dia que
    llegue la Fase 2.4b, este cache tendra que crecer a un mapa por
    familia solicitada en vez de una sola entrada - documentado aqui para
    que no se olvide al implementarla.
  - **Sin invalidacion, a proposito**: un proceso de este motor no cambia
    las fuentes instaladas del sistema operativo mientras esta vivo (nadie
    instala/desinstala fuentes concurrentemente con una sesion de
    navegacion real), asi que no hay ningun escenario real en el que el
    valor cacheado quede obsoleto durante la vida del proceso - `OnceLock`
    sin ningun mecanismo de expiracion es la eleccion correcta, no una
    simplificacion que corta esquinas.
  - Tests reales (2 nuevos): una segunda llamada tarda menos de 50ms (dos
    ordenes de magnitud por debajo de los ~500ms medidos sin cache - margen
    generoso a proposito para no ser fragil en una maquina cargada, sin
    dejar de detectar una regresion real a "vuelve a escanear cada vez");
    dos llamadas devuelven exactamente los mismos bytes de fuente (prueba
    que el cache es una unica fuente compartida, no una copia
    independiente por llamada que podria variar si el sistema tuviera
    varias candidatas empatadas).
- **Fixture WPT-style de eventos y microtasks** (Fase 5.2, cobertura -
  `tests/wpt-style/events-and-microtasks.html`, nuevo): hallazgo real al
  revisar `tests/wpt-style/` antes de esta fase solo habia DOS fixtures
  (`class-list-and-style.html`, `dom-mutation-and-navigation.html`, 11
  tests en total) - ninguna cubria `addEventListener`/`removeEventListener`/
  `dispatchEvent`, bubbling, fase de captura, `preventDefault`/
  `stopPropagation`/`defaultPrevented`, `new Event(tipo, opciones)`, ni
  `queueMicrotask`, pese a que las seis son funcionalidad real que este
  motor ya tenia implementada desde fases anteriores (tareas #68, #75,
  #77, #79, #44 del plan) y con tests unitarios de Rust, pero sin ningun
  ejercicio end-to-end en el ESTILO testharness.js real que una pagina
  JS de verdad usaria.
  - 10 tests nuevos, TODOS pasando en la primera ejecucion contra
    `wpt_runner` real (sin necesitar ningun arreglo en el motor - esto es
    cobertura de algo que ya funcionaba, no un hallazgo de bug): listener
    invocado con un `Event` real (`.type`/`.target` correctos);
    `removeEventListener` detiene invocaciones futuras sin afectar a la ya
    ocurrida; bubbling real hijo->padre->abuelo; `stopPropagation` corta el
    burbujeo antes del ancestro; la fase de CAPTURA se ejecuta antes que la
    de objetivo/burbuja (verificado con un `addEventListener(tipo, fn,
    true)` en el ancestro); `preventDefault` marca `defaultPrevented` solo
    si `cancelable` es `true` (no-op honesto si es `false`); `new
    Event(tipo, opciones)` respeta `bubbles`/`cancelable` (falso por
    defecto sin opciones); `queueMicrotask` NO ejecuta su callback de forma
    sincrona (verificado dentro del mismo `<script>`) PERO si se ha
    ejecutado ya para cuando empieza el SIGUIENTE `<script>` de la pagina
    (verificado repartiendo el test en dos bloques `<script>` separados,
    apoyandose en que `JsRuntime::eval` drena la cola de microtasks
    despues de cada script por separado, no solo al final de la pagina -
    ver `event_loop.rs`).
  - `tests/wpt-style/` pasa de 11 a 21 tests reales en total (`cargo run -p
    engine-core --bin wpt_runner -- tests/wpt-style`).
  - Sigue habiendo gaps reales de cobertura tras esta fase, deliberadamente
    fuera de alcance de este incremento: nada del camino NDJSON
    (`core::server` - teclado/clics reales, `target="_blank"`, historial,
    pestañas) tiene fixtures WPT-style, porque `wpt_runner` ejercita
    unicamente el pipeline headless DOM+CSS+JS (`build_page_with_harness`),
    sin simulacion de clic/teclado real ni red - esos caminos ya tienen su
    propia cobertura real (tests unitarios de Rust en `core::server` +
    verificacion en vivo end-to-end contra `engine_server.exe`, ver las
    entradas de Fase 4.1-4.5 mas arriba), pero en un estilo distinto, no
    testharness.js.
- **Negrita/cursiva reales** (Fase 2.4): `<b>`/`<strong>`/`font-weight: bold`
  y `<i>`/`<em>`/`font-style: italic` ya se PINTAN con una cara de fuente de
  verdad, no solo se resuelven en la cascada sin efecto visible (que era el
  estado tras la Fase 2.1: la hoja de agente de usuario ya escribia
  `font-weight`/`font-style` en `computed_style`, pero nada los leia al
  pintar). `engine_text::font::FontSet` (nuevo) carga las 4 combinaciones
  reales de peso/estilo (`regular`/`bold`/`italic`/`bold_italic`) UNA sola
  vez por pagina via `fontdb::Query` con `weight`/`style`, compartidas entre
  quien MIDE el texto (`engine-layout`, para el layout/wrap) y quien lo
  PINTA (`engine-gfx`) — misma razon que ya existia para una unica
  `SystemFont` compartida: medir con una variante y pintar con otra
  desalinearia cajas y glifos. `FontSet::pick(bold, italic)` elige la
  variante; sin cara de negrita/cursiva de verdad instalada para esa
  familia, `fontdb` devuelve la cara mas cercana que SI tenga (normalmente
  la regular) — sin negrita sintetica/oblicua artificial, igual que un
  navegador real sin esa cara instalada.
  - **Simplificacion binaria deliberada**: `resolve_font_weight_is_bold`/
    `resolve_font_style_is_italic` (duplicadas en `engine-layout::tree` y
    `engine-gfx::display_list`, mismo criterio de siempre para no enredar
    la dependencia entre crates) colapsan el espacio real del spec
    (`font-weight` 1-1000, `font-style: normal | italic | oblique`) a un
    binario negrita-si/no y cursiva-si/no — `FontSet` solo carga 4
    combinaciones fijas, no una cara por cada peso posible. `bold`/`bolder`
    o cualquier numero >= 600 cuenta como negrita (el umbral real donde los
    navegadores empiezan a preferir una cara "bold" al hacer matching de
    fuente); `oblique` se trata igual que `italic` (misma variante, sin una
    tercera cara "inclinada sinteticamente" aparte).
  - **`font-weight`/`font-style` se sumaron a `INHERITABLE_PROPERTIES`**
    (antes solo `color`/`font-size`): sin esto, un `<b>texto</b>` dejaba
    `font-weight: bold` en el `computed_style` del propio `<b>` (la cascada
    ya lo resolvia desde la Fase 2.1) pero la caja de TEXTO hija — la que de
    verdad se mide/pinta — nunca lo veia. Mismo mecanismo de herencia que ya
    existia para `color`/`font-size`, solo con dos propiedades mas; el resto
    de la lista real de propiedades heredables del spec sigue pendiente
    (Fase 2.5).
  - **Bug real encontrado (no teorico) al verificar esto en vivo**:
    `build_node` (`tree.rs`) solo trataba `span`/`a`/`b`/`i` como
    `BoxType::Inline` — `strong`/`em` faltaban en esa lista pese a tener
    reglas propias en la hoja de agente de usuario desde la Fase 2.1, asi
    que caian al `_ => BoxType::Block` de respaldo. Un parrafo como "Texto
    `<strong>fuerte</strong>` mas texto" se partia en TRES lineas en vez de
    una: el texto antes del `<strong>` se quedaba solo en su propia racha
    inline (el `<strong>` bloque rompia la racha), el `<strong>` se apilaba
    debajo como si fuera un bloque/parrafo entero, y el texto de despues
    empezaba una tercera linea. Invisible en los tests de la Fase 2.3
    porque todos usaban `<b>`/`<i>`, nunca `<strong>`/`<em>` mezclados con
    texto vecino. Arreglado añadiendolos a la lista de tags inline; test de
    regresion (`strong_and_em_are_inline_level_like_b_and_i`) y verificado
    en vivo (antes/despues: tres lineas sueltas -> una sola linea con
    negrita/cursiva reales).
  NO implementado: `font-family` (el motor sigue pintando SIEMPRE con la
  fuente sans-serif por defecto del sistema, sin importar que la pagina
  pida una familia concreta — Fase 2.4 cubrio peso/estilo, no familia,
  porque `font-family` real necesitaria ademas estar en
  `INHERITABLE_PROPERTIES`, ver Fase 2.5, para no quedarse solo en
  elementos con la propiedad puesta directamente); `text-decoration`
  (subrayado de `<a>`, que la cascada ya resuelve igual que negrita/cursiva
  pero en el momento de ESTA fase nada lo pintaba todavia - cerrado en la
  Fase 29, ver su entrada); pesos intermedios reales (100/200/300...) mas
  alla del binario negrita-si/no; `font-weight: bolder/lighter` RELATIVOS
  al peso heredado (se tratan igual que `bold`/`normal` absolutos, sin
  sumar/restar sobre el peso del padre).
- **`INHERITABLE_PROPERTIES` ampliada a la lista real del spec** (Fase 2.5):
  de 4 propiedades (`color`, `font-size`, `font-weight`, `font-style`) a 20,
  sumando `font-family`, `font-variant`, `line-height`, `text-align`,
  `text-indent`, `text-transform`, `letter-spacing`, `word-spacing`,
  `white-space`, `visibility`, `cursor`, `direction`, `list-style-type`,
  `list-style-position`, `list-style-image`, `quotes` - las heredables del
  spec real que tienen sentido dado lo que el motor soporta hoy. Se
  excluyeron a proposito las especificas de tablas (`border-collapse`,
  `border-spacing`, `caption-side`, `empty-cells` - el layout de tablas
  (Fase 3.4) no las necesita: sin `border-collapse` no hay nada que
  colapsar, sin medicion de contenido no hay `caption`/`empty-cells` que
  cambien nada visible todavia) y las de paginacion impresa (`orphans`/
  `widows` - un renderer de pantalla sin paginacion no tiene pagina que
  romper). Mismo
  patron que `font-weight`/`font-style` en la Fase 2.4 antes de que
  `engine-gfx` las pintara: que una propiedad este en esta lista es cascada
  CSS correcta y verificable en `computed_style`, no implica que algo la
  lea todavia para layout o pintado - `text-align`, `list-style-type`,
  `letter-spacing`, `visibility`... siguen sin efecto visual (Fase 3+),
  documentado asi a proposito. Sin resolucion de unidades relativas para
  ninguna de las nuevas (se propaga el string crudo, igual que `color`
  siempre ha hecho) - solo `font-size` tiene esa conversion especial,
  porque solo `font-size` tiene hoy quien consuma el valor ya resuelto.
- **Imagenes de trama reales** (Fase 3.1): `<img src="...">` se descarga,
  decodifica (PNG/JPEG/GIF/BMP/WebP/TIFF/ICO via el crate `image`, nuevo
  crate propio `engine-image` - ver "Doctrina de dependencias") y se PINTA
  con sus pixeles reales, no un rectangulo de relleno. Arquitectura en 3
  capas: `engine-image::decode_image` (bytes -> `DecodedImage` RGBA8, sin
  saber nada de DOM/layout/red), `engine-layout` (`BoxType::Image(src)`,
  nuevo - `<img>` es inline-level por defecto como en el spec real; su caja
  se dimensiona con `resolve_image_dimensions`: si el autor puso AMBOS
  `width`/`height` (CSS o los atributos HTML del mismo nombre - ver
  `apply_image_size_attributes`, que los inserta en `computed_style` SOLO
  si CSS no los puso ya, como un hint de presentacion de la especificidad
  mas baja posible) se usan tal cual; si solo puso UNO, el otro se escala
  para mantener la proporcion real de la imagen decodificada; sin ninguna
  imagen decodificada disponible, 0x0 siempre - sin icono de "imagen rota"
  ni el tamaño de respaldo 300x150 que el spec real exige para un
  reemplazado sin tamaño intrinseco, simplificacion declarada) y
  `engine-gfx` (`DisplayItem::Image` lleva el `Arc<DecodedImage>` YA
  resuelto - `raster.rs`/`window.rs` comparten `image_paint::paint_image`,
  que premultiplica alpha - `tiny_skia::Pixmap` lo exige, `DecodedImage`
  guarda straight alpha - y escala con `Transform` al tamaño final resuelto
  por el layout, que puede diferir del tamaño natural). `ImageMap` (`src`
  crudo -> `Arc<DecodedImage>`) es un `HashMap` normal, no `Option` como
  `FontSet`: una pagina sin imagenes es simplemente un mapa vacio, sin el
  caso especial de "subsistema entero ausente" que si existe para fuentes.
  `core/server.rs::fetch_images` sigue el mismo patron exacto que
  `fetch_external_scripts`/`fetch_external_stylesheets` (resuelve `src`
  contra la URL de la pagina, una descarga o decodificacion fallida se
  omite con un aviso, no aborta la pagina) - `core/main.rs` (el arnes
  manual sin red, ver su doc-comment) pasa siempre un `ImageMap` vacio.
  - **Bug real encontrado en vivo, no teorico**: una imagen mas alta que la
    linea de texto en la que cae (el caso comun - casi cualquier foto es
    mucho mas alta que una linea de 16px) empujaba el bloque SIGUIENTE
    hacia arriba lo bastante como para solaparse con la propia imagen,
    porque `flow_inline_run` avanzaba `cursor_y` por un alto de linea FIJO
    (el de la fuente de texto), ignorando que la imagen de esa misma linea
    era mas alta. Arreglado con `line_extent` (`flow_inline_run`/
    `place_inline_node` en `tree.rs`): el alto real de avance de CADA
    linea, que arranca en el alto de texto y crece si algo mas alto
    (una imagen) se coloca en ella - verificado en vivo con una imagen real
    200x100 (antes/despues: parrafo solapado -> parrafo debajo del todo).
  NO implementado: SVG (`resvg`, mencionado en la doctrina de dependencias,
  sigue sin integrarse - vectorial, no un simple decode-a-RGBA), `srcset`/
  `<picture>` (imagenes responsivas), carga perezosa (`loading="lazy"`),
  `object-fit`/`object-position`, `max-width`/`max-height` sobre imagenes
  (solo `width`/`height` exactos), cache de imagenes repetidas entre
  navegaciones, imagen de "recurso roto" cuando la decodificacion falla.
- **Pinta en una ventana nativa real** (`winit` + `tiny-skia` + `softbuffer`):
  rectangulos solidos para cajas de bloque con `background-color`, y **texto
  con glifos reales** via `engine-text` (`rustybuzz` para shaping +
  `fontdb` para cargar una fuente sans-serif del sistema + `ttf_parser` para
  extraer el contorno de cada glifo) — verificado con tests que comprueban
  que los contornos tienen area no nula y que el avance entre glifos es
  correcto (izquierda a derecha). El color y el tamaño de fuente del texto
  ya vienen de la cascada heredada (negro/16px son solo el valor inicial
  cuando nada los redefine, igual que en un navegador real); si no hay
  ninguna fuente de sistema disponible, cae a un bloque de relleno en vez de
  fingir que hay glifos. `font-size` ya entiende `em` y `%` (relativos al
  font-size YA RESUELTO del padre inmediato — `resolve_font_size` en
  `layout/src/tree.rs` — probado con resolucion de un nivel y encadenada a
  traves de varios); el resultado se deja resuelto a `px` en `computed_style`
  antes de que nada mas lo lea, asi que `parse_css_font_size` (aqui y en
  `engine-gfx`) sigue sin necesitar saber de unidades relativas. `rem`
  (relativo a la raiz del documento, no al padre inmediato) sigue sin
  soportarse — exigiria rastrear el font-size de `<html>` por separado de lo
  heredado nivel a nivel; cae al tamaño heredado del padre, no a un numero
  inventado.
- **Reflow real al redimensionar la ventana, nada mas todavia**: hasta hace
  poco esto afirmaba (incorrectamente) que "el layout se recalcula en cada
  frame" — no era cierto: `layout_root`/`display_list` se calculaban UNA
  vez al arrancar y se reusaban sin cambios en cada `RedrawRequested`;
  redimensionar la ventana solo estiraba el backbuffer de `softbuffer`
  sobre las mismas cajas del tamaño original (contenido cortado o con
  hueco vacio alrededor, sin fluir de verdad). Arreglado: `NativeEngineWindow::run`
  recibe una closure `relayout(ancho, alto) -> LayoutBox` (construida en
  `core/main.rs`, capturando clones de `dom_root`/`stylesheet`/`font` de
  `PageResult`) que se llama de verdad en el handler de `Resized` para
  recalcular el arbol completo al nuevo tamaño — probado que reconstruir
  layout con el mismo `dom_root`+`stylesheet` a un ancho distinto produce
  dimensiones distintas, no las originales congeladas. `gfx` sigue sin
  depender de `engine-css`/`engine-dom`: la closure es la unica que conoce
  esos tipos, `gfx` solo sabe pedir "un arbol de cajas para este tamaño".
  El shaping de texto SI se recalculaba ya en cada redibujado (eso era
  cierto) — sigue siendo un gasto conocido, aceptable por ahora; cache de
  glifos es trabajo futuro si hace falta. El clic izquierdo SI recalcula
  (relayout completo) ahora (ver mas abajo, "Clic real del SO cableado de
  punta a punta"); el scroll de la rueda del raton YA repinta con un
  nuevo desplazamiento vertical (sin relayout - el contenido no cambia de
  forma, solo que porcion se ve, ver "Scroll real de la rueda del raton"
  mas abajo); el teclado en ESTA ventana (winit/`core::main`) sigue sin
  ninguna fuente de eventos - el camino NDJSON real (`core::server`, el
  que usa el producto) si la tiene desde la Fase 4.1, ver mas arriba.
- **Ya NO quema ~100% de un nucleo de CPU en reposo**: medido en vivo antes
  del arreglo, con la ventana abierta y SIN ninguna interaccion —
  ~97% de un nucleo de forma continua (16.85s de CPU en 17.36s de reloj,
  repetido dos veces). Causa real: `Event::AboutToWait` pedia un redraw sin
  condicion alguna en cada vuelta del bucle, lo que encadenaba
  `RedrawRequested -> AboutToWait -> RedrawRequested` para siempre — el
  bucle nunca llegaba a esperar de verdad, aunque `ControlFlow::Wait` YA es
  el valor por defecto de `winit` (el diagnostico inicial de "hace falta
  poner Wait" era incorrecto; el problema no era el `ControlFlow`, era pedir
  un redraw sin necesidad). Arreglado quitando ese redraw incondicional:
  ahora solo se pide un redraw cuando algo cambia de verdad (una vez al
  arrancar, y tras un `Resized` real) — medido en vivo despues del arreglo,
  mismo experimento: 0.0s de CPU adicional en reposo, exactos, dos veces
  seguidas. `ControlFlow::Wait` se fija explicito de todas formas, para que
  quede documentado en vez de depender de un valor por defecto implicito.
- **Ejecuta JavaScript real** sobre los `<script>` de la pagina, inline O
  externo (`core/src/scripting.rs`): todos comparten el mismo `Context` de
  Boa, asi que una variable declarada en un script sigue viva para el
  siguiente - inline o externo, sin distincion - igual que en una pagina
  real. `<script src="...">` SI se descarga ahora:
  `core/server.rs::fetch_external_scripts` descubre cada `src` con
  `pipeline::find_external_script_srcs` (pura, sin red), lo descarga con el
  mismo `NetworkEngine` de la pagina (mismos redirects/gzip/brotli), y
  `scripting::run_scripts` lo ejecuta EN SU POSICION EXACTA del documento -
  no al final ni por separado - para que el orden relativo a los `<script>`
  inline (y por tanto el estado compartido entre ellos) sea el correcto.
  Verificado en vivo con un servidor local: un script externo declara una
  variable, un `<script>` inline justo despues la lee y la usa. Un `src`
  que falla al descargarse se omite con un aviso, no aborta la pagina.
  Los scripts corren todos seguidos tras parsear el
  documento completo, no intercalados con el parseo como en un navegador
  real (`document.write` no podria hacer nada de todas formas: ver mas
  abajo por que). `queueMicrotask` (`event_loop.rs`) ya encola de verdad en
  la cola de jobs que `Context` trae por defecto (`SimpleJobQueue` de
  `boa_engine::job`) en vez de llamar al callback en el acto — antes,
  `queueMicrotask(() => log('a')); log('b');` imprimia "a" y luego "b"
  (orden invertido respecto a un navegador real); ahora imprime "b" y luego
  "a", como debe ser. La cola se drena en `JsRuntime::eval` (`runtime.rs`)
  justo despues de evaluar cada script — el punto mas parecido que hay
  todavia a "termino la tarea actual", sin un event loop real (Fase 3).
  Probado con 4 tests, incluyendo que un microtask que encola otro
  microtask tambien drena en el mismo eval, y que un argumento no invocable
  no revienta.
- **Bindings DOM reales, pero minimos** (`js/src/dom_bindings.rs`):
  `document.getElementById(id)`, `document.querySelector(selector)` y
  `document.querySelectorAll(selector)` (los dos ultimos con matching real
  via `SelectorMatcher::query_first`/`query_all` — combinadores incluidos,
  no un lookup ingenuo por tag) devuelven un objeto JS por elemento
  encontrado, con partes vivas y partes foto: `getAttribute`/`setAttribute`,
  `textContent` (accessor real, con getter Y setter — via
  `FunctionObjectBuilder`/`ObjectInitializer::accessor`), `appendChild` y
  `removeChild` SI son vivos — leen y escriben de verdad sobre el
  `Arc<RwLock<Node>>` del arbol real (`ElementCapture`), asi que mutar y
  leer despues ve el cambio, incluso desde un objeto JS distinto obtenido
  con otra llamada a `getElementById` sobre el mismo id — probado
  explicitamente. Asignar `el.textContent = valor` reemplaza TODOS los
  hijos existentes por un unico nodo de texto nuevo (semantica real, no un
  append); `null` se trata como cadena vacia (`[LegacyNullToEmptyString]`
  del spec real, no la cadena "null" que daria `ToString(null)` en
  cualquier otra propiedad) — probado explicitamente, igual que la
  coercion normal para otros valores (`42` -> `"42"`). `document.
  createElement(tag)` crea un nodo nuevo y desconectado;
  `padre.appendChild(hijo)`/`padre.removeChild(hijo)` lo conectan/
  desconectan de verdad del arbol — probado explicitamente que un elemento
  creado+mutado+añadido es alcanzable despues via una busqueda FRESCA por
  id desde la raiz del documento (no solo que la variable local de JS lo
  recuerde), y que tras `removeChild` esa misma busqueda ya no lo
  encuentra. Ambos recuperan el nodo real de `hijo` via datos nativos
  adjuntos al objeto JS (`JsObject::downcast_ref::<ElementCapture>()`,
  `ObjectInitializer::with_native_data`) — si `hijo` no es un objeto
  elemento nuestro (una cadena, un numero...) no hacen nada, en vez de
  fingir que se añadio/quito algo; `removeChild` sobre un nodo que no es
  hijo de verdad tambien es un no-op honesto (devuelve `null`, el DOM real
  lanzaria `NotFoundError`) — todo esto probado. Solo `tagName` sigue
  siendo foto (no observable: ningun binding cambia la etiqueta de un
  elemento, y tampoco se puede en el DOM real). Como `scripting.rs`
  ejecuta los `<script>` inline ANTES de construir el layout
  (`core/pipeline.rs::build_page`), una mutacion real hecha durante la
  ejecucion inicial de un script ya se refleja en el layout resultante,
  sin necesitar recalculo tras interaccion (Fase 3, que sigue sin
  existir). `querySelectorAll` devuelve un `Array` real de JS
  (`boa_engine::object::builtins::JsArray`) — no un `NodeList` real, pero un
  `Array` ya trae `.forEach`/`.map`/etc. que un `NodeList` no trae de
  fabrica, asi que en la practica es mas capaz, no menos — la LISTA esta
  congelada en el momento de la llamada (hay que volver a llamar para ver
  elementos nuevos), no cada elemento dentro de ella. `el.classList` tambien
  es real: `contains`/`add`/`remove`/`toggle` leen y escriben de verdad el
  atributo `class` (cadena separada por espacios, partida/unida con
  `split_whitespace`/`join`) — no una lista paralela que se desincroniza.
  `toggle(nombre, force)` usa `ToBoolean` generico para `force` (igual que
  el spec real: `0`, `""`, `undefined`... cuentan como false, no solo el
  booleano literal). Una diferencia real con el DOM autentico, no
  escondida: cada lectura de `el.classList` construye un objeto JS nuevo,
  asi que `el.classList === el.classList` da `false` aqui y `true` en un
  navegador real (alli `classList` es un `DOMTokenList` con identidad
  estable) — no afecta a `contains`/`add`/`remove`/`toggle` en si, que
  siempre operan sobre el `class` real del elemento, pero un script que
  compare la identidad del objeto se comportaria distinto.
  `.parentElement` (getter) sube por `Node::parent` (un `Weak`) y da `null`
  si no hay padre o si el padre no es un `Element` — la raiz real del
  arbol es un `NodeType::Document` (`html5ever_sink.rs`), no un elemento,
  asi que `document.querySelector('html').parentElement` da `null` aqui
  igual que en un navegador real. `.children` (getter) devuelve un `Array`
  real solo con los hijos `Element` (los nodos de texto sueltos no
  cuentan) — a diferencia de `querySelectorAll`, aqui SI es vivo: cada
  lectura vuelve a mirar el arbol real, asi que ve altas/bajas de
  `appendChild`/`removeChild` hechas justo antes, no una foto congelada.
  `.style` (getter) devuelve un objeto con `getPropertyValue`/
  `setProperty`/`removeProperty`/`cssText` reales sobre el atributo
  `style` (parseado con `CssParser::parse_inline_style` — el mismo
  tokenizador `cssparser` que una hoja de estilos normal, ver mas arriba)
  — la MISMA fuente que la cascada real aplica al layout, asi que mutar
  `el.style` es mutar de verdad lo que se pintaria en el siguiente
  layout, no una copia paralela. `getPropertyValue` da `""` (nunca
  `null`) si la propiedad no esta puesta; `setProperty(nombre, "")` QUITA
  la propiedad en vez de guardarla vacia; `removeProperty` devuelve el
  valor quitado — las tres, igual que el spec real. `cssText` (getter que
  serializa TODAS las declaraciones, setter que REEMPLAZA el bloque
  entero en vez de fusionarlo — probado explicitamente que una propiedad
  vieja desaparece tras asignar `cssText`) y tres accessors por nombre
  camelCase — `color`/`backgroundColor`/`fontSize` — tambien mutan de
  verdad la misma fuente. Deliberadamente solo esas tres, no las cientos
  del spec real: son las UNICAS que `layout`/`gfx` leen de verdad hoy
  (verificado por grep contra `computed_style.get(...)`, no asumido) —
  `el.style.margin` o `.display` no tienen accessor, se convierten en una
  propiedad JS normal del objeto (sin tocar el atributo real), que es
  exactamente el mismo comportamiento que un navegador real para una
  propiedad `CSSStyleDeclaration` no reconocida. `padre.insertBefore(nuevo, referencia)` (`referencia` null o
  ausente inserta al final, igual que appendChild) y
  `padre.replaceChild(nuevo, viejo)` (devuelve `viejo`) completan las
  cuatro mutaciones fundamentales de `Node` — ambos validan la posicion
  ANTES de mutar nada (una referencia/viejo que no es hijo real deja todo
  intacto, no a medio mover). Los tres metodos que pueden recibir un nodo
  ya conectado (`appendChild`/`insertBefore`/`replaceChild`) lo desconectan
  primero de su padre anterior si tenia uno — encontrado al construir
  `insertBefore`: `appendChild` NO lo hacia, asi que mover un nodo ya
  conectado antes lo dejaba fantasma en la lista de children de su padre
  viejo ademas de en la del nuevo; arreglado a la vez para los tres.
  `addEventListener(tipo, listener)`/`removeEventListener(tipo, listener)`/
  `dispatchEvent(event)` son reales: un `EventRegistry` COMPARTIDO por
  todo el documento (no por elemento ni por objeto JS envoltorio, que se
  reconstruye nuevo en cada consulta) guarda los listeners indexados por
  el puntero del NODO real, asi que registrar desde una consulta y
  disparar desde otra consulta al mismo elemento se ven — probado
  explicitamente, junto con que dos listeners del mismo tipo se llaman
  ambos en orden, y que `removeEventListener` compara por identidad real
  (`JsObject`/`Gc::ptr_eq`, no por contenido — dos funciones con el mismo
  codigo fuente por separado no matchean). `new Event(tipo)` (constructor
  global, solo `.type` al principio) necesito `.constructor(true)` de
  verdad en Boa (`register_global_callable`, no
  `register_global_builtin_callable` que usa `printEngineLog` — verificado
  contra el codigo fuente de Boa, no asumido) para que `new` no lance "not
  a constructor". Guardar los listeners (`JsObject`, con punteros `Gc<_>`
  reales) en un `Mutex` invisible al trazador de Boa (`#[unsafe_ignore_trace]`)
  sigue siendo seguro — verificado contra `boa_gc`: un objeto con mas
  referencias (`ref_count`) de las que el trazado normal puede explicar
  (`non_root_count`) se trata como RAIZ y sobrevive a cada recoleccion, en
  vez de liberarse bajo los pies. Ya SI esta conectado al clic izquierdo
  real del sistema operativo (ver mas abajo, "Clic real del SO cableado
  de punta a punta"). La rueda del raton YA mueve el viewport de verdad
  (ver "Scroll real de la rueda del raton" mas abajo) pero SOLO a nivel
  de pintado/hit-testing en `gfx` — no dispara ningun evento `scroll`
  hacia JS todavia (`addEventListener('scroll', ...)` no tiene ninguna
  fuente que lo dispare); el teclado en esta ventana (winit) sigue sin
  ninguna fuente real - el camino NDJSON real (`core::server`) si la
  tiene, ver Fase 4.1 mas arriba.
- **Bubbling real + `preventDefault`/`stopPropagation`/`event.target`**:
  `dispatchEvent` ya no se queda en el nodo exacto — sube por los
  ancestros (`dispatch_event_with_bubbling`) llamando a sus listeners
  tambien, parando en cuanto algun listener llame a
  `event.stopPropagation()` (comprobado despues de cada nodo, no solo al
  final) — probado que un listener en un ANCESTRO se entera de un evento
  disparado sobre un descendiente, y que `stopPropagation` corta la
  subida antes de tiempo. Dentro de cada listener, `this` es el elemento
  en el que ESE listener esta registrado (`currentTarget`, cambia por
  nivel), mientras que `event.target` es siempre el nodo ORIGINAL, fijo
  en todos los niveles — ambos probados por separado. `event.
  preventDefault()`/`stopPropagation()` mutan el propio objeto evento
  (`this` dentro del metodo, via `JsObject::set` — verificado su firma
  exacta contra el codigo fuente de Boa antes de escribir nada:
  `set<K,V>(key, value, throw, context) -> JsResult<bool>`), asi que
  funcionan igual para un evento creado con `new Event(...)` en JS que
  para el que construye `DomBindings::dispatch_event` internamente.
  `dispatchEvent` ahora devuelve `false` si algun listener llamo a
  `preventDefault()` (antes siempre devolvia `true`). **Bug real
  encontrado por su propio test**: la primera version de bubbling
  reconstruia un objeto JS nuevo hasta para el TARGET, rompiendo la
  garantia ya probada de `this === el` dentro de un listener puesto en el
  propio elemento (identidad, no solo contenido) — el test existente lo
  detecto de inmediato; arreglado para que el target reuse la MISMA
  referencia de `this` que ya tenia quien llamo a `dispatchEvent`, y solo
  los ANCESTROS (sin ninguna referencia previa que reusar) construyan una
  envoltura nueva.
- **`Event.bubbles`/`.cancelable` reales**: antes todo evento burbujeaba
  incondicionalmente y `preventDefault()` siempre funcionaba, sin
  distincion `cancelable` — ya no. `new Event(tipo, opciones?)` lee
  `opciones.bubbles`/`opciones.cancelable`, `false` por defecto en ambos
  (igual que el spec real: un evento no burbujea ni es cancelable a menos
  que se pida explicitamente) — probado. `dispatch_event_with_bubbling`
  comprueba `.bubbles` antes de subir a los ancestros (`event_bubbles`,
  hermana de `event_propagation_stopped`) — si es `false`, se queda en el
  target y no toca ningun ancestro, probado explicitamente.
  `preventDefault()` comprueba `.cancelable` antes de marcar
  `defaultPrevented` — si es `false`, es un no-op honesto, tambien probado
  junto con el caso contrario. `DomBindings::dispatch_event` (el camino
  que usa el clic real del SO) fija ambos a `true` siempre: hoy el unico
  evento real que pasa por ahi es "click", y un click real siempre
  burbujea y es cancelable en el spec — parametrizable el dia que haga
  falta otro tipo de evento con semantica distinta, no antes. Cuatro
  tests de bubbling de la tarea anterior tuvieron que empezar a pedir
  `{bubbles: true}` explicitamente (antes burbujeaban gratis, ahora hay
  que pedirlo, como en un navegador real) — efecto secundario esperado de
  hacer el flag real en vez de asumido. Probado con 92 tests en total
  (`cargo test -p engine-js`, crate completo).
- **Fase de captura real en `addEventListener`/`removeEventListener`/
  `dispatchEvent`**: las tres fases del spec, en orden - CAPTURA (raiz del
  documento -> padre inmediato del target, solo listeners registrados con
  `{capture: true}` o el legado `useCapture=true`), TARGET (el target
  mismo, TODOS sus listeners sin importar captura, en orden de registro)
  y BURBUJEO (padre inmediato del target -> raiz, solo listeners SIN
  captura, y solo si `.bubbles` es `true`) — probado que un listener de
  captura en un ancestro se llama ANTES que el del propio target (orden
  capturado en un log, no solo "ambos se llamaron"), y que la fase de
  captura pasa AUNQUE el evento no burbujee (punto del spec facil de
  pasar por alto: solo la fase de burbujeo depende de `.bubbles`, la de
  captura no). `EventRegistry` paso de `Vec<(String, JsObject)>` a
  `Vec<(String, JsObject, bool)>` (tipo, listener, `use_capture`);
  `dispatch_event_to_listeners` gano un filtro `phase_capture:
  Option<bool>` (`Some(true)`/`Some(false)`/`None` para captura/burbujeo/
  target respectivamente) para no duplicar la logica de busqueda entre
  las tres fases. `removeEventListener` ahora exige que `capture`
  coincida ademas de tipo e identidad — el MISMO listener registrado una
  vez con captura y otra sin ella son dos entradas DISTINTAS, quitar una
  no toca la otra, probado explicitamente; tambien se acepta la forma
  legado de un booleano suelto como tercer argumento (`useCapture`), no
  solo el objeto `{capture: bool}` moderno. Ningun llamador externo de
  `DomBindings::dispatch_event`/`JsRuntime::dispatch_event` cambio de
  firma — la fase de captura es puramente aditiva desde su perspectiva
  (si nadie registra un listener con `{capture: true}`, el bucle de
  captura simplemente no encuentra nada que llamar). Probado con 103
  tests en total (`cargo test -p engine-js`, crate completo).
- **`firstElementChild`/`lastElementChild`/`nextElementSibling`/
  `previousElementSibling` reales**: completan la navegacion de arbol
  empezada por `children`/`parentElement`. Deliberadamente Element-only
  (real DOM spec, `ParentNode`/`ElementTraversal`) — a diferencia de
  `firstChild`/`nextSibling` de `Node`, que SI pueden dar un nodo de
  texto y exigirian poder envolver uno como objeto JS (que este motor no
  hace todavia, solo los `Element` se envuelven). `nextElementSibling`/
  `previousElementSibling` suben al padre (mismo mecanismo que
  `parentElement`), localizan la posicion REAL de este nodo entre los
  hijos del padre y escanean hacia adelante/atras desde ahi — probado que
  saltan nodos de texto sueltos entre medias, y que dan `null` en los
  bordes (ultimo hijo sin next, primero sin previous) y sobre un nodo
  desconectado, sin reventar.
- **`document.documentElement`/`document.body` reales**: gap encontrado
  por grep (solo se mencionaban en comentarios como ejemplo de
  comportamiento real, nunca implementados) — de los accessors mas usados
  en JS real. `Node::document_element` (`dom/node.rs`) busca el UNICO hijo
  `Element` directo de la raiz (normalmente `<html>`), no en todo el
  subarbol — igual que el spec real. `document.body` reusa `find_all_by_tag`
  ya existente. Ambos son getters (se leen sin parentesis) que devuelven
  el mismo tipo de objeto vivo que `getElementById`/`querySelector` —
  probado que mutar via `document.body` se ve a traves de una consulta
  DISTINTA (`querySelector('body')`) al mismo elemento. Hallazgo real
  durante las pruebas: `html5ever` sintetiza un `<body>` real incluso para
  `<html></html>` vacio (igual que un navegador real) — la primera
  version del test para "sin body" asumia lo contrario y fallo; se
  corrigio para probar la sintesis real en vez de un caso que este parser
  no produce en la practica.
- **`JsRuntime` persistente + `dispatch_event` invocable desde Rust**
  (primer paso concreto hacia cablear clics reales, investigado ANTES de
  intentarlo: el bloqueador real no era la falta de hit-testing —
  `LayoutBox.dimensions: Rect` ya existe — sino que el `JsRuntime` entero,
  y con el el `EventRegistry` de la pagina, se destruia justo despues de
  la carga inicial, antes de que la ventana siquiera se abriera).
  `execute_inline_scripts_keeping_runtime` (`core/scripting.rs`) devuelve
  el `JsRuntime` en vez de dropearlo; `JsRuntime::dispatch_event`/
  `DomBindings::dispatch_event` disparan un evento sobre un `Arc<RwLock<
  Node>>` real directamente desde Rust, sin volver a evaluar texto JS —
  probado explicitamente: un script registra un listener con
  `addEventListener`, la funcion devuelve el runtime, y ENTONCES (fuera de
  cualquier `eval`) se dispara el evento desde Rust y el listener se
  ejecuta de verdad.
- **Backref `LayoutBox` -> `Node` + hit-testing por coordenadas** (segundo
  paso hacia clics reales). `LayoutBox.dom_node: Option<Arc<RwLock<Node>>>`
  se rellena solo en cajas de `Element` (`LayoutTreeBuilder::build_node`) —
  las de texto NO llevan uno propio a proposito: un click real siempre
  resuelve al elemento contenedor, nunca a un nodo de texto, igual que
  `event.target` en un navegador real jamas es un `Text`. `Rect::contains`
  + `LayoutBox::hit_test(x, y)` recorren el arbol buscando la caja mas
  profunda que contenga el punto; si la mas especifica es una caja de
  texto (sin `dom_node`), la recursion cae de forma natural al `dom_node`
  del ancestro mas cercano que si tenga uno — probado explicitamente,
  incluido ese caso de caida.
- **Clic real del SO cableado de punta a punta** (tercer y ultimo paso de
  esta cadena). `gfx/src/window.rs::NativeEngineWindow::run` ahora retiene
  su propio `LayoutBox` mutable entre eventos (antes se perdia justo
  despues de construir el `display_list` inicial — sin eso no habia nada
  que hit-testear tras el primer pintado), rastrea `WindowEvent::
  CursorMoved` (`MouseInput` no trae coordenadas propias) y, en
  `MouseInput` con boton izquierdo en `Released` (mas cerca del `click`
  real del spec — que exige un press+release sobre el mismo objetivo — que
  disparar en `Pressed`), llama a un nuevo parametro `on_click: FnMut(&
  LayoutBox, f32, f32) -> Option<LayoutBox>`. `core/main.rs` implementa
  ese closure: `hit_test` sobre el layout actual, `JsRuntime::
  dispatch_event(nodo, "click")` (con el `JsRuntime` que
  `pipeline::build_page_keeping_runtime` — nueva, mismo patron que
  `build_page_with_harness` — mantiene vivo en vez de dropear), y
  reconstruye el layout al mismo tamaño de viewport que el actual para
  que un repintado posterior refleje cualquier mutacion del DOM que el
  listener haya hecho — igual mecanismo que `relayout` tras un resize.
  `gfx` sigue sin saber que es un DOM, un evento o un `JsRuntime`: solo
  hace hit-testing sobre cajas y delega todo lo demas a quien lo llame.
  De paso, se corrigio una afirmacion desactualizada en el doc comment de
  `pipeline::build_page` que todavia decia que `.classList`/`.style`/
  eventos "no existen todavia" — llevaban varias tareas existiendo.
  **Que se verifico y como, con precision** (para no repetir el error del
  antiguo `wpt_runner.rs` de afirmar mas de lo probado): un test a nivel
  Rust (`pipeline::tests::
  hit_test_dispatch_event_and_relayout_together_reflect_a_click_listeners_dom_mutation`)
  compone exactamente lo que hace `on_click` — construye una pagina real
  con un listener real, hit-testea el layout real, dispara el evento real,
  y confirma que tanto el DOM real como un layout reconstruido reflejan la
  mutacion — sin abrir ninguna ventana. La ventana real se verifico en
  vivo solo en el sentido de "arranca, sigue respondiendo, cierra limpio"
  con el codigo de mouse ya compilado - deliberadamente NO se sintetizo un
  click real del SO (`SetCursorPos`+`mouse_event`) para probar el camino
  completo incluido winit: a diferencia de `MoveWindow` (usado antes para
  probar el resize, que solo reposiciona una ventana propia), tomar control
  del cursor real del sistema es una accion mas invasiva - podria
  interferir con lo que el usuario este haciendo en ese momento - y no fue
  autorizada explicitamente. El camino winit -> `on_click` en si (los
  `match` de `MouseInput`/`CursorMoved`) por tanto NO tiene una prueba
  automatizada propia, solo la composicion logica de sus piezas.
- **Scroll real de la rueda del raton**: hasta ahora, contenido mas alto
  que el viewport era literalmente invisible - sin forma de llegar a el,
  ni con el raton ni de ninguna otra manera. `LayoutBox::content_extent()`
  (nuevo, `layout/src/layout_box.rs`) recorre el arbol y se queda con el
  borde inferior (`y + height`) mas bajo de verdad: `dimensions.height` de
  la caja raiz es siempre el alto del viewport de ENTRADA
  (`LayoutTreeBuilder::build` lo fija una vez) y `flow_block_children`
  nunca lo actualiza con el desborde real de los hijos — verificado
  leyendo `tree.rs`, no asumido, antes de escribir nada. `gfx/src/
  window.rs::NativeEngineWindow::run` gana un nuevo estado local,
  `scroll_offset_y`, acotado por `clamp_scroll_offset` (funcion pura,
  probada: nunca negativo, nunca mas alla de `content_extent - alto_
  viewport`, cero si el contenido cabe entero) tras cada `WindowEvent::
  MouseWheel` y tras cada resize (el contenido puede cambiar de alto).
  Deliberadamente SIN relayout por scroll — el contenido no cambia de
  forma, solo que porcion esta visible, igual que un navegador real.
  `display_list` en si NO se reconstruye por esto: sigue siendo geometria
  en content-space (las mismas coordenadas que calculo el layout); la
  transformacion a screen-space pasa una sola vez, en el momento de
  pintar, restando `scroll_offset_y` a cada `rect.y`. El hit-test de clic
  hace la traduccion inversa: `cursor_position` de winit es screen-space,
  asi que se le SUMA `scroll_offset_y` antes de pasarlo a `on_click`, para
  volver a content-space. `MouseScrollDelta::LineDelta` (ratones fisicos,
  en "lineas") se convierte a pixeles con una constante razonable pero no
  medida (`PIXELS_PER_LINE = 40.0`); `PixelDelta` (trackpads) ya viene en
  pixeles. Igual que con el clic real del SO: el signo exacto (que
  direccion de giro debe sumar vs restar) depende de plataforma/driver y
  NO se verifico en vivo — no se sintetizo un scroll real por el mismo
  motivo que no se sintetizo un click real (tomar control del raton/rueda
  del sistema no fue autorizado); si al probarlo el sentido sale
  invertido, el arreglo es cambiar un `-=` por `+=`, nada mas. Lo que SI
  se verifico en vivo: la ventana arranca, pinta, redimensiona y cierra
  limpio con todo este codigo ya compilado (mismo alcance de verificacion
  que el click). `content_extent`/`clamp_scroll_offset` estan probados a
  fondo (7 tests nuevos: 3 de `content_extent` mas 1 de recursion en
  varios niveles, 4 de `clamp_scroll_offset`). NO implementado todavia:
  ningun evento `scroll` llega a JS (`addEventListener('scroll', ...)` no
  tiene fuente que lo dispare - esto es puramente un mecanismo de `gfx`,
  invisible para el DOM/JS por ahora), ni scroll horizontal (el layout de
  bloques actual nunca produce desborde horizontal, asi que no hay nada
  que desplazar en ese eje todavia).
- **`padding` real desde CSS**: hallazgo real al ponerse con esto -
  `BLOCK_PADDING` en `tree.rs` era una constante fija (12px) aplicada a
  TODA caja de bloque sin excepcion, sin importar lo que su CSS de verdad
  dijera - ni siquiera leia la propiedad `padding`, era pura decoracion.
  Sustituida por `resolve_padding` (misma simplificacion honesta que
  `parse_css_font_size`: solo un valor unico en `px`, aplicado a los 4
  lados por igual - la forma abreviada de 2/3/4 valores del spec real
  queda pendiente), que lee `padding` de verdad de `computed_style` y cae
  a CERO (el valor inicial real de la propiedad) si no esta puesta o no es
  un `px` valido - no a los 12px inventados de antes. Efecto secundario
  esperado: una pagina sin `padding` declarado en su CSS ahora renderiza
  con sus hijos pegados al borde del contenedor en vez del hueco fijo de
  siempre - mas correcto (asi es el spec real sin un UA-stylesheet que
  este motor no tiene), aunque visualmente mas apretado por defecto.
  De paso, esta es la primera vez que `LayoutBox::box_dimensions`
  (`Dimensions`, `box_model.rs`) deja de estar siempre en `default()`: se
  puebla `padding` de verdad y `content` se calcula de forma que
  `Dimensions::padding_box()` — escrito hace tiempo, nunca ejercitado
  hasta ahora, la auditoria de honestidad lo encontro como codigo muerto —
  reconstruye EXACTAMENTE `dimensions`, probado explicitamente (`margin` y
  `border` tambien se conectaron de verdad poco despues, ver los dos
  puntos siguientes). `display_list.rs` no necesito ningun cambio para
  `padding` en si: `dimensions` ya representaba el equivalente al
  padding-box antes y despues de este cambio concreto, solo que la
  cantidad de padding ahora viene de CSS en vez de estar fija. 5 tests
  nuevos,
  incluido uno que verifica la reconstruccion via `padding_box()`
  explicitamente.
- **`margin` real desde CSS**: continuacion directa del punto anterior -
  `BLOCK_GAP` en `tree.rs` era otra constante fija (6px de hueco vertical
  entre CUALQUIER par de hermanos), sin relacion alguna con la propiedad
  `margin`. Sustituida por `resolve_margin` (mismo patron que
  `resolve_padding`: un unico valor en `px`, cero si no esta puesto o no
  es valido). A diferencia de `padding` (propiedad del CONTENEDOR, empuja
  a sus hijos hacia adentro), `margin` es propiedad de CADA HIJO: empuja
  `cursor_y` antes de colocarlo (`margin-top`), desplaza `x` y reduce el
  ancho asignado (`margin-left`/`right`), y vuelve a empujar `cursor_y`
  despues (`margin-bottom`) - sustituye a `BLOCK_GAP` por completo, no lo
  complementa. Que una caja de texto nunca resuelva ningun `margin` propio
  no exige ningun caso especial: `margin` no es heredable y las cajas de
  texto solo llevan las propiedades heredadas en su `computed_style` (ver
  `INHERITABLE_PROPERTIES`), asi que "margin" nunca esta en su mapa por
  construccion. SIN colapso de margenes adyacentes (el spec real se queda
  con el mayor de dos margenes verticales contiguos, no con la suma) -
  simplificacion honesta declarada y probada explicitamente (un test
  verifica la suma, no el colapso, como el comportamiento actual real, no
  como un bug camuflado). Mismo efecto secundario que con `padding`: sin
  `margin` declarado, los elementos de bloque quedan pegados entre si en
  vez del hueco de 6px de siempre. `Dimensions::margin_box()` (codigo
  muerto desde que se escribio, igual que `padding_box()`) se ejercita por
  primera vez, probado que expande `dimensions` exactamente por el margin
  real en las 4 direcciones. 5 tests nuevos.
- **`border` real desde CSS + pintado**: completa el trio del box model
  (`padding`+`margin` ya reales, ver los dos puntos anteriores) - y es la
  primera propiedad de esta serie que ademas de afectar el layout se
  pinta de verdad, no solo ocupa espacio. `resolve_border_width` (en
  `engine-layout/src/tree.rs`) SOLO entiende la forma abreviada
  `border: <ancho> <estilo> <color>`, en cualquier orden (sin las
  longhand `border-width`/`border-color`/`border-style` por separado
  todavia); `display_list.rs` (en `engine-gfx`) resuelve el ancho Y el
  color por separado, al pintar, mismo criterio que `color`/
  `background-color`/`font-size` ya usaban. Regla del spec real
  implementada a proposito porque es facil pasarla por alto: SOLO el
  estilo `solid` esta reconocido - sin la palabra `solid` en el valor
  (incluido `none` explicito, o directamente no poner ningun border), el
  ancho EFECTIVO es CERO pase lo que pase se haya escrito como numero -
  `border-style: none`, el valor inicial real de la propiedad, fuerza el
  `border-width` computado a cero. `border-color` ausente cae al `color`
  YA RESUELTO de la propia caja (`currentColor`, tambien el valor inicial
  real). `dimensions` pasa a representar el BORDER-box (una capa mas
  hacia afuera que el padding-box de antes) - reverificada la matematica
  de `padding_box()`/`border_box()`/`margin_box()` con border en la
  ecuacion: las tres siguen reconstruyendo exactamente lo esperado,
  probado explicitamente incluso con padding Y border presentes a la vez
  en la misma caja. `background-color` sigue pintando sobre TODO
  `dimensions` sin cambios: coincide con el valor inicial real de
  `background-clip` (`border-box`), el border se pinta despues y encima.
  Pintado como 4 rectangulos solidos (arriba/derecha/abajo/izquierda,
  `border_strip_rects`, hoy en `engine-gfx::paint` - ver Fase 3.5 mas
  arriba, vivia en `window.rs` cuando se escribio esto) en vez de un
  stroke con la API de trazado de tiny-skia — mismo resultado visual para
  un border UNIFORME sin `border-radius` (con `border-radius` SI se usa
  stroke, ver Fase 3.5), reusando `fill_rect`, ya probado, en vez de
  investigar API nueva sin necesidad. 15 tests nuevos entre las tres capas
  (layout, parseo de color en gfx, geometria de pintado).
- **`resolve_style` trasladado de `layout` a `css`** (paso 1, preparatorio,
  hacia `getComputedStyle` — **ya implementado, Fase 8, ver mas arriba**;
  lo que sigue describe el estado del motor cuando se escribio esta
  entrada, y el ultimo parrafo explica por que la solucion final no fue
  la que aqui se anticipaba). La funcion de cascada real (matching + especificidad + atributo
  `style` inline, antes privada dentro de `layout::tree`) ahora vive en
  `engine_css::cascade::resolve_style`, publica, SIN cambiar su logica ni
  una linea - `layout::tree::build_node` la llama desde alli en vez de
  tener su propia copia. Cero dependencias nuevas en ningun Cargo.toml:
  `engine-js` ya dependia de `engine-css` desde la tarea de `element.style`
  (`CssParser::parse_inline_style`). Verificado que el traslado no cambio
  NADA de comportamiento observable: los tests de cascada que ya existian
  en `layout` (que prueban el resultado via `LayoutTreeBuilder::build`, no
  llaman a `resolve_style` directamente) siguen pasando exactamente
  igual - mismo numero, ni uno mas ni uno menos. Añadidos 4 tests nuevos
  sobre `resolve_style` en aislamiento, directamente en `css`, sin pasar
  por `layout`. Por que hace falta esto para `getComputedStyle`: el
  pipeline actual (`core/pipeline.rs`) ejecuta TODOS los `<script>` antes
  de extraer y parsear el `<style>` de la pagina (parseo -> JS -> cascada
  -> layout) - en el momento en que un script llamaria a
  `getComputedStyle`, el `StyleSheet` ni siquiera existe todavia. Un
  bloqueo arquitectonico real, no una excusa: hace falta ademas (1)
  extraer+parsear el `<style>` de la pagina ANTES de correr los scripts
  (sin quitar la extraccion actual de despues, para no perder soporte a
  `<style>` añadidos dinamicamente por un script) y enhebrar ese
  `StyleSheet` hasta `dom_bindings.rs`, y (2) una funcion que resuelva la
  cascada caminando desde la raiz hasta UN nodo cualquiera (reusando este
  mismo `resolve_style` para cada ancestro por el camino), porque
  `getComputedStyle` puede pedirse sobre cualquier elemento suelto, no
  sobre el arbol entero como hace el recorrido top-down de `build_node`.
  Ninguno de esos dos pasos existe todavia - esta tarea es solo la base.
- **`display: none` y `visibility: hidden` reales**: hasta ahora ninguna
  de las dos tenia efecto - un elemento oculto generaba caja, ocupaba
  espacio y se pintaba igual que uno visible, encontrado auditando el
  motor en vivo (no leyendo el codigo: una pagina de control con 4 divs
  ocultos de formas distintas los mostraba los 4). `display: none` se
  corta en `layout::tree::build_node`, justo despues de resolver
  `computed_style` y ANTES de recursar en los hijos: el elemento no entra
  al arbol de layout en absoluto, ni el ni su subarbol entero (asi es el
  spec real - `display` no esta en `INHERITABLE_PROPERTIES`, la
  comprobacion es local a cada elemento, pero cortar antes de recursar ya
  saca a todos los descendientes sin que ellos necesiten declarar nada).
  `visibility: hidden` es distinto a proposito: SI genera caja (sigue
  ocupando su espacio, un hueco en blanco donde estaria) y SI hereda a
  sus descendientes (ya estaba en `INHERITABLE_PROPERTIES` desde la Fase
  2.5, pero sin consumidor hasta ahora) - solo deja de pintarse. Se
  resuelve en `engine-gfx::display_list::build_items`: si el
  `computed_style` YA resuelto de la caja dice `hidden`, se salta la
  emision de sus propios `DisplayItem` (fondo/borde/sombra/texto/imagen)
  pero se sigue recortando (`overflow: hidden`) y recursando en los hijos
  con normalidad - un hijo con `visibility: visible` declarado el mismo
  reactiva su propio pintado sin ningun caso especial nuevo, porque la
  cascada ya resuelve esa redeclaracion antes de que `display_list` la
  lea (`entry().or_insert_with()` en `build_node`: lo propio del elemento
  siempre gana sobre lo heredado).
  Tests reales: `display:none` no genera caja para el elemento, saca a un
  descendiente aunque el mismo no declare `display:none`, un hermano sin
  la propiedad conserva su caja y su espacio; `visibility:hidden` no
  emite ningun `DisplayItem` propio, un hijo con `visibility:visible`
  explicito pinta pese al ancestro oculto.
  Verificado en vivo con una pagina de control: antes del fix, 4 divs
  ocultos de 4 formas distintas (`display:none` en hoja de estilo,
  `display:none` inline, `visibility:hidden`) se pintaban los 4 igual que
  el contenido visible; despues, los dos `display:none` desaparecen sin
  dejar hueco y el `visibility:hidden` deja su hueco en blanco sin
  pintarse - captura de pantalla revisada, comportamiento correcto.
  Probado tambien contra un articulo real de Wikipedia por HTTPS: el
  PNG resultante salio byte-identico al de antes del fix. Investigado, no
  descartado como sospechoso: la hoja de estilo real de Wikipedia SI trae
  reglas `display:none` con selectores simples de una sola clase
  (`.error`, `.printonly`, `.mw-empty-elt`...) que este motor ya sabria
  aplicar, pero el colapsado real de su barra lateral/tabla de contenidos
  (lo que mas cambiaria visualmente) depende de un patron de "checkbox
  hack" (`:checked` + combinadores de hermanos) y de media queries -
  ninguno de los dos esta implementado en `engine-css::selector` todavia,
  hueco preexistente y separado de esta tarea. El fix es correcto y esta
  verificado; que una pagina real tan compleja como Wikipedia no cambie
  visualmente todavia es honesto de declarar aqui, no un fallo silencioso.
  NO implementado: `display: none` via JS en caliente sin pasar por
  `resolve_style` de nuevo (cualquier cambio de `style`/`class` que
  dispare `display:none` despues de la carga inicial depende de que algo
  ya llame a relayout, mismo criterio que el resto de mutaciones de
  estilo), y ningun otro valor de `visibility` (`collapse`, pensado para
  filas de tabla, no reconocido).
  **Epilogo (Fase 8): ninguno de esos dos pasos hizo falta.** El diagnostico
  del bloqueo era correcto (cuando un script corre, el `StyleSheet` aun no
  existe) pero la salida propuesta era mas cara de lo necesario: en vez de
  re-resolver la cascada a peticion, `getComputedStyle` lee el
  `computed_style` que el arbol de layout YA tiene resuelto, publicado en
  un snapshot despues de cada layout. Eso convierte el orden del pipeline
  de obstaculo en detalle declarado (durante la carga el snapshot esta
  vacio, igual que un elemento sin caja en un navegador real) y ahorra
  duplicar la resolucion de cascada en dos caminos que podrian
  divergir. Este traslado de `resolve_style` sigue siendo util - es lo que
  `layout::tree::build_node` llama - pero `engine-js` acabo sin usarlo.
- **Envio de formularios real**: hasta ahora `press_enter` en `type_text`
  solo disparaba `keydown`/`keyup` con `"Enter"` - ningun formulario se
  enviaba nunca, encontrado auditando el motor en vivo (no era una
  suposicion: buscar en cualquier sitio real, Google incluido, no hacia
  literalmente nada). Dos caminos disparan un envio ahora, los dos reales
  del spec:
  1. **Enter en un input de texto de una sola linea** (`server::type_text`
     con `press_enter: true`, y tambien `server::press_key` cuando el
     Enter llega SUELTO, sin texto nuevo en el mismo golpe) - nunca en un
     `<textarea>` (`is_textarea`), donde Enter inserta una linea nueva en
     un navegador real, no envia nada.
  2. **Clic en un boton submit** (`server::click`, rama `None` de
     `find_link_target` - un boton submit y un `<a href>` nunca coinciden
     en el mismo nodo en la practica, asi que se comprueban en el mismo
     punto sin pisarse). `find_submit_control` sube por los ANCESTROS del
     nodo clicado (mismo patron que `find_link_target` con un `<b>` dentro
     de un `<a>`) buscando `<button>` SIN `type="button"|"reset"` (submit
     por defecto, asi es el spec real) o `<input type="submit"|"image">`.
  Ambos caminos respetan `preventDefault()` en el `keydown`/`click` real
  (mismo criterio que la navegacion por `<a href>` de la Fase 4.2) antes
  de disparar nada.
  `find_form_ancestor` sube por los ANCESTROS hasta el `<form>` mas
  cercano (mismo patron de nuevo). `collect_form_data` recoge nombre=valor
  de TODO el subarbol del `<form>` (`Node::find_all_by_tag`, no solo hijos
  directos - un input casi siempre esta envuelto en `<label>`/`<div>`
  intermedios): texto/hidden con su `value` (cadena vacia si no tiene),
  checkbox/radio marcado con su `value` o `"on"` por defecto (el valor
  real cuando no se declara ninguno), `<select>` con el `<option selected>`
  o el PRIMERO si ninguno lo esta (valor por defecto real), `<textarea>`
  con su `value` si ya se edito o su contenido de texto inicial si no.
  `disabled` se omite por completo. De los botones submit del formulario
  SOLO el que disparo el envio aporta su par (`submit_control: None` para
  el camino de Enter, que no incluye ningun boton - igual que el spec
  real).
  `build_get_submit_url` (funcion libre, sin tocar `self` ni la red a
  proposito - asi se puede probar sin levantar ninguna pagina) resuelve
  `action` (o el documento actual si esta vacio/ausente) contra la URL de
  la pagina, y REEMPLAZA su query string entera con los datos del
  formulario via `url::Url::query_pairs_mut().clear().extend_pairs(...)`
  - el mismo crate `url` que ya resuelve el resto de URLs del motor
  (doctrina de dependencias mas abajo: la codificacion percent-encoding
  de `application/x-www-form-urlencoded` NUNCA se escribe a mano). El
  resultado navega exactamente igual que un clic en un enlace
  (`EngineServer::navigate`).
  **Solo `method="get"` (el valor por defecto real sin el atributo) esta
  implementado.** `method="post"` (o cualquier otro valor) devuelve un
  error explicito en vez de fingir un envio que este motor no hace de
  verdad: no hay forma de mandar un cuerpo en una peticion desde aqui
  todavia, y tratar un POST como si fuera GET filtraria datos de
  formulario (credenciales incluidas) por la URL - un fallo de seguridad
  real, no solo una imprecision. Un formulario de login real (casi
  siempre POST) reporta ese error en vez de navegar en silencio con la
  contraseña en la barra de direcciones.
  14 tests nuevos (deteccion de ancestro `<form>`/boton submit, recogida
  de datos por tipo de control, construccion de la URL, el error
  explicito de POST). Verificado en vivo contra un servidor HTTP local
  con un formulario real (`<input name="q">`, un checkbox, un `<select>`,
  un boton submit): escribir "rust lang" en el campo y pulsar Enter
  navego a `/buscar?q=rust+lang&s=uno` y cargo la pagina de resultados;
  marcar el checkbox y clicar "Enviar" (con las coordenadas exactas que
  el propio motor reporto para cada control) navego a
  `/buscar?q=&c=on&s=uno` - el checkbox marcado aporto `c=on`, el campo
  vacio aporto `q=` (no se omitio), el select sin tocar aporto su primera
  opcion. Los dos caminos de disparo probados por separado, ambos
  correctos.
  Simplificaciones declaradas: sin asociacion `form="id"` (un control
  fuera del `<form>` pero asociado por ese atributo no se recoge - solo
  cuenta ser descendiente); `<input type="file">` se omite (no hay datos
  de fichero que enviar); sin `<button>`/`<input>` con `formaction`/
  `formmethod` (siempre se usa el `action`/`method` del propio `<form>`,
  nunca la sobreescritura por boton).
- **Expansion del shorthand `background`**: hasta ahora `background:
  #ff0000` no pintaba NADA - solo el longhand `background-color`
  funcionaba (`engine-gfx::display_list` solo lee esa clave), encontrado
  auditando el motor en vivo con una pagina de control (dos cajas
  identicas salvo por usar shorthand vs longhand: una se pintaba, la otra
  no). La mayoria de CSS real escribe `background: <color>`, no
  `background-color: <color>` directamente, asi que este hueco afectaba a
  practicamente cualquier pagina con fondo de color.
  Resuelto en `engine-css::parser::insert_declaration` - el UNICO punto
  por el que pasan TANTO las reglas de una hoja de estilos (`parse_block`)
  COMO un atributo `style="..."` inline (`CssParser::parse_inline_style`,
  misma gramatica, mismo bucle `parse_declaration_list`), asi que expandir
  ahi cubre los dos casos con un solo cambio. `background: <valor>` deja
  TAMBIEN `background-color` en las declaraciones si `<valor>` trae un
  token que empiece por `#` (un color hexadecimal) - el resto del
  shorthand (posicion/repeticion/una imagen) se ignora sin error, igual
  que cualquier propiedad no soportada en este motor.
  **Por que en el parser y no al pintar** (la alternativa obvia, y la
  que NO se eligio): la cascada real (`cascade::apply_matching_rules`)
  fusiona declaracion a declaracion con un `insert` plano por clave, sin
  ningun concepto de que `background`/`background-color` esten
  relacionados. Expandir en el parser hace que, para cuando la cascada
  corre, YA NO EXISTE diferencia entre shorthand y longhand - son la
  misma clave `background-color`, y el orden normal de insercion
  (especificidad entre reglas, orden de declaracion dentro de la misma
  regla) decide quien gana SIN NINGUN caso especial nuevo en
  `cascade.rs`. Expandir al pintar en cambio habria exigido enseñarle a
  la cascada (o a `engine-gfx`) a comparar shorthand contra longhand por
  separado, con su propio criterio de desempate - logica que ya viene
  gratis del modelo de datos elegido aqui.
  **Los nombres de color CSS siguen sin soportarse** (`background: red`
  no pinta nada, declarado a proposito, no un descuido): `parse_css_color`
  en `engine-gfx` solo reconoce hex (`#rgb`/`#rrggbb`) en NINGUN sitio del
  motor, shorthand incluido - expandir el shorthand no inventa soporte de
  color que no existe rio abajo, `insert_declaration` simplemente no
  extrae nada si no encuentra un token que empiece por `#`.
  7 tests nuevos en `parser.rs` (extraccion de color, ausencia de color
  no fabrica nada, nombres de color ignorados, orden de declaracion
  dentro de la misma regla en los dos sentidos, tambien en `style="..."`)
  y 1 en `cascade.rs` (una regla mas especifica que solo toca el longhand
  gana sobre una menos especifica que usa el shorthand - el caso real que
  motiva hacerlo en el parser). Verificado en vivo con la misma pagina de
  control de la auditoria: el div que usaba `background: #ff0000` pasa de
  no pintarse en absoluto a un rectangulo rojo solido identico al que ya
  usaba `background-color` - captura de pantalla revisada, antes/despues.
- **Controles de formulario con caja propia** (Fase 11): hasta ahora
  `input`/`select`/`textarea`/`button` no tenian ningun tratamiento
  propio - caian al `_ => BoxType::Block` generico, lo que significaba
  ocupar el ANCHO COMPLETO del contenedor (un `<input>` en una pagina de
  1280px de ancho media 1280px) sin fondo ni borde visible, encontrado
  auditando el motor en vivo. Ademas de verse mal, esto rompia el calculo
  de "centro del elemento" que hace un agente de IA para clicar: el
  centro de una caja de 1280px de ancho no tiene nada que ver con donde
  esta el control real.
  Dos tratamientos distintos, segun si el control tiene contenido DOM
  real que pintar:
  - **`BoxType::Replaced`** (variante nueva en `engine-layout::layout_box`,
    para `input`/`select`/`textarea`): mismo concepto de "elemento
    reemplazado" del spec real que ya tenia `BoxType::Image` (Fase 3.1),
    pero sin ningun bitmap - su tamaño sale SIEMPRE de CSS
    (`resolve_replaced_dimensions`, sin ningun "tamaño natural" del que
    partir, a diferencia de una imagen decodificada). Atomico: NO recursa
    en sus hijos DOM al posicionar (`place_inline_node::BoxType::Replaced`)
    - un `<select>` SI tiene cajas hijas reales para cada `<option>`
      (`build_node` las crea igual que para cualquier otro elemento), pero
      se quedan sin posicionar a proposito, igual que un navegador real no
      pinta las opciones de un desplegable como contenido normal de la
      pagina.
    `is_inline_level` incluye `Replaced` (participa en el flujo inline
    como texto/span/imagen, no fuerza una linea propia), y tanto
    `measure_flex_item` como `finalize_flex_item_children` tienen su
    propio camino para esta variante (mismo criterio ya existente para
    `Image`) - sin esto, un `<input>` dentro de una barra de busqueda
    `display: flex` (patron real MUY comun) mediria como si no tuviera
    contenido intrinseco.
  - **`BoxType::Inline`** (no una variante nueva - `button` se sumo a la
    lista existente de `span`/`a`/`b`/`i`/`strong`/`em`): a diferencia de
    `input`/`select`/`textarea`, la etiqueta de un `<button>` es contenido
    DOM real (un nodo de texto hijo, no un atributo `value`/`placeholder`
    que el motor no pinta), asi que se beneficia de ENCOGERSE a su
    contenido en vez de un tamaño fijo - que es exactamente lo que
    `BoxType::Inline` ya hacia gratis (su caja es el rectangulo delimitador
    de sus hijos, ver `place_inline_node::BoxType::Inline`). Limitacion ya
    declarada que se hereda sin cambios: `padding`/`border` de elementos
    inline no se resuelven en el layout, asi que el fondo/borde de un
    boton queda pegado al texto sin aire alrededor.
  La hoja de agente de usuario (`user_agent_stylesheet.rs`) da el tamaño y
  aspecto por tipo: `input` de texto 170x21 con borde y fondo blanco
  (aproximacion al `size=20` por defecto real de HTML - sin shrink-to-fit
  real, este motor no mide min/max-content en ningun sitio todavia, misma
  limitacion ya declarada para flex/fuera de flujo); `input[type=checkbox]`/
  `[type=radio]` 13x13 (el segundo con `border-radius: 7px`, una
  aproximacion visual a un circulo, no una forma geometrica distinta -
  `border-radius` no soporta porcentaje, solo `px`); `input[type=submit]`/
  `[type=button]`/`[type=reset]`/`[type=image]` con fondo gris de boton;
  **`input[type=hidden]` usa `display: none` real** (Fase 10.5, ya
  implementada) en vez de cualquier tamaño - un campo oculto no genera
  NINGUNA caja, correcto; `select`/`textarea` con su propio tamaño y
  aspecto.
  **Deliberadamente NO implementado en esta tarea**: el `value`/
  `placeholder` de un `input`/`textarea` no se PINTA como texto dentro de
  la caja (solo se reporta en el JSON de `elements` para el frontend/
  agente, igual que antes) - el motor no tiene ninguna infraestructura de
  "materializar el atributo como una caja de texto sintetica" todavia;
  las opciones de un `<select>` no se pueden desplegar/elegir por clic
  (sigue sin haber ninguna interaccion de desplegable); sin aspecto nativo
  por plataforma real (flechas, sombreados).
  6 tests nuevos en `tree.rs` (tamaño fijo de un input, checkbox pequeño,
  hidden sin caja, boton que se encoge a su texto, control reemplazado
  dentro de flex conservando su tamaño). Verificado en vivo con la misma
  pagina de control de la auditoria: `input[type=text]` paso de
  `[0,342,1280,22]` a `[0,342,170,21]`; el checkbox paso (antes del mismo
  tamaño que el input de texto) a `[170,342,13,13]`; el boton "Enviar" de
  `[0,452,1280,22]` a `[170,385,45,21]` - encogido de verdad a su texto,
  no un tamaño fijo inventado - captura de pantalla revisada, antes/
  despues.
- **`float: left`/`right` real** (Fase 12, el ultimo hueco P0 del punch
  list original): hasta ahora `float` no tenia NINGUN tratamiento propio
  (cero referencias en todo el motor) - un `<div style="float:left">` se
  apilaba verticalmente como cualquier otro hijo de bloque, empujando a
  sus hermanos hacia abajo por su propia altura completa en vez de
  quedarse a un lado con el contenido fluyendo junto a el. Cualquier pagina
  anterior a flexbox (foros, prensa, WordPress clasico) colapsaba a una
  sola columna por esto.
  Implementado en `LayoutTreeBuilder::flow_block_children`, que ahora
  mantiene HASTA UN float activo por lado (`float_left`/`float_right:
  Option<ActiveFloat>`, declaradas fuera del bucle principal porque tienen
  que sobrevivir entre iteraciones) mientras recorre los hijos del
  contenedor:
  - Un hijo `float: left`/`right` (`float_side`, nuevo `BoxType`-agnostico -
    aplica igual a un `<div>` de bloque que a cualquier otro elemento) se
    intercepta ANTES de la agrupacion en rachas inline (`is_inline_level`
    no distingue floats) y se resuelve en `place_float_child`: se ancla al
    borde izquierdo/derecho del contenedor, y a diferencia de un hijo de
    bloque normal, NO avanza `cursor_y` - un float no empuja a sus
    hermanos hacia abajo por su propia altura, solo reserva espacio
    HORIZONTAL mientras dura. Su ancho usa `resolve_block_width` de
    siempre pero con un valor de respaldo fijo (`DEFAULT_FLOAT_WIDTH =
    200px`) en vez de "llenar el contenedor" como "auto" - sin esto,
    cualquier `float` sin `width` explicito ocuparia el ancho completo,
    anulando el proposito de flotar (sin shrink-to-fit real, este motor
    no mide min/max-content en ningun sitio todavia, misma limitacion ya
    declarada para items flex/cajas fuera de flujo).
  - CUALQUIER contenido normal (racha inline o hijo de bloque) que
    arranque dentro del rango vertical de un float activo usa un
    `origin_x`/ancho ESTRECHADO (calculado en cada iteracion del bucle,
    antes de decidir si es una racha inline o un hijo de bloque), en vez
    del ancho completo del contenedor - esto es lo que hace que el texto
    "fluya alrededor" del float en vez de solaparse con el o ignorarlo.
    **Granularidad de CAJA COMPLETA, no por linea**: un hijo que arranca
    dentro del rango del float usa el ancho estrechado para TODA su caja,
    aunque su contenido real termine mas abajo del borde inferior del
    float - el spec real reajustaria linea a linea DENTRO de un mismo
    parrafo (las lineas que ya caen por debajo del float se ensancharian
    solas); este motor no hace reflow de texto por linea segun obstaculos,
    simplificacion declarada. El caso mas comun en paginas reales - un
    float con un parrafo corto al lado, o un contenedor completo que se
    queda deliberadamente angosto mientras dura el float - se ve correcto.
  - Un float cuyo borde inferior ya quedo atras (`cursor_y >=
    float.bottom_y`, comprobado al INICIO de cada iteracion) deja de
    estrechar nada automaticamente.
  - **`clear: left`/`right`/`both`** tambien implementado: salta
    `cursor_y` por debajo del borde inferior de los floats activos del
    lado indicado ANTES de colocar ese hijo - es lo que hace funcionar el
    patron real "clearfix" (un `<div style="clear:both">` vacio para
    'cerrar' una seccion flotada).
  - El alto AUTO final del contenedor incluye cualquier float TODAVIA
    activo al terminar el bucle - deliberadamente DISTINTO del spec real
    (donde un contenedor con SOLO floats dentro colapsa a alto CERO salvo
    que los "contenga" explicitamente, el famoso problema que tantas
    paginas reales trabajan para evitar con un clearfix) - eleccion
    consciente: es el comportamiento que la mayoria de autores esperan de
    todas formas, y no le cuesta nada a ninguna pagina que SI dependiera
    de la sorpresa real del spec (ese patron ya exige un
    `overflow`/clearfix explicito para conseguir precisamente este mismo
    resultado).
  Simplificacion declarada mas importante: SOLO un float activo por lado a
  la vez (`Option<ActiveFloat>`, no una pila/cola) - dos `float: left`
  consecutivos sin que el primero haya quedado atras verticalmente se
  SOLAPAN en vez de colocarse uno al lado del otro (el spec real los
  apila horizontalmente hasta que no caben y baja a la siguiente "linea
  de floats"). El caso real mas comun - un float por lado, con contenido
  normal fluyendo entre ellos - funciona correctamente; una galeria de
  varias imagenes flotadas seguidas del mismo lado, no. Los floats
  siempre se anclan contra los bordes VERDADEROS del contenedor (nunca
  anidados dentro de la zona ya estrechada por OTRO float activo del lado
  opuesto) - dos floats en lados opuestos a la vez SI funcionan
  correctamente entre si (verificado con test dedicado), solo la
  combinacion "float + float del MISMO lado ya activo" tiene este hueco.
  8 tests nuevos (el float no avanza el cursor vertical, estrecha el
  contenido siguiente mientras esta activo, deja de estrechar una vez
  atras, `float:right` simetrico, dos floats en lados opuestos a la vez,
  `clear:both`, un contenedor con solo un float dentro abraza su altura,
  ancho de respaldo sin `width` explicito). Verificado en vivo con una
  pagina de control nueva (barra lateral flotada a la izquierda con texto
  propio, caja "Relacionados" flotada a la derecha, un parrafo fluyendo
  entre ambas, un `clear:both` al final) - resultado visual identico al
  patron real de un foro/blog clasico pre-flexbox: el parrafo se angosta
  correctamente entre los dos floats, y el `clear:both` cae limpio debajo
  de ambos sin solaparse - y con la misma pagina de control de la
  auditoria original, donde el div `.float` (antes apilado como fila
  completa) ahora flota de verdad junto al parrafo siguiente.

- **Cookies HTTP reales (RFC 6265)** (Fase 13): hasta ahora
  `net/src/cookie.rs` era un `HashMap<String, String>` sin ninguna
  semantica de cookie que ademas nadie instanciaba - **ninguna peticion
  enviaba ni recibia cookies**. La consecuencia real, encontrada auditando
  el motor, era que **no se podia iniciar sesion en ningun sitio web**:
  toda sesion HTTP se sostiene sobre una cookie. Era el bloqueante mas
  grave del motor, por encima de cualquier fallo visual.
  Implementado de verdad: parseo completo de `Set-Cookie` (`Domain`,
  `Path`, `Expires`, `Max-Age`, `Secure`, `HttpOnly`, `SameSite`), con
  `Max-Age` ganando sobre `Expires` sin importar el orden (§5.2.2) y
  `Max-Age<=0` borrando la cookie - que es exactamente como un servidor
  real cierra una sesion. Alcance por dominio (§5.1.3) con separador de
  etiqueta real: `ejemplo.test` cubre `www.ejemplo.test` pero NUNCA
  `malejemplo.test` (el ataque obvio si se comparase solo el sufijo
  textual); cookies sin `Domain` son host-only. Alcance por ruta
  (§5.1.4) con la ruta por defecto derivada de la URL. Expiracion real
  con purga al consultar. `Secure` solo por `https:`. Identidad
  `(name, domain, path)` (§5.3.11): un `Set-Cookie` repetido SUSTITUYE
  en vez de acumularse.
  **Dos bugs de infraestructura corregidos de paso**, ambos invisibles
  mientras nadie leia cookies: (1) `fetch_once` recogia las cabeceras en
  un `HashMap`, asi que de las VARIAS `Set-Cookie` que una respuesta real
  trae (un login tipico deja la de sesion y alguna de estado en la misma
  respuesta) **solo sobrevivia la ultima** - ahora se recogen aparte en
  un `Vec` (nuevo campo `NetworkResponse::set_cookie`); (2) la cabecera
  `Cookie` se añade en `fetch_once` y no en `fetch`, para que **cada
  salto de una redireccion recalcule sus propias cookies** - un login
  real es POST/GET -> 302 -> GET, y la cookie de sesion la pone la
  respuesta de la redireccion para que viaje ya en el salto siguiente.
  El almacen (`CookieStore`) vive en `NetworkEngine` tras un `Mutex`,
  compartido por todas las peticiones de la sesion.
  Simplificaciones declaradas: **sin lista de sufijos publicos (PSL)** -
  no se rechaza un `Domain=.co.uk`, aunque si se exige que el `Domain`
  declarado cubra al host que lo pone (un sitio no puede poner cookies a
  un dominio ajeno); **`SameSite` se parsea y guarda pero no se aplica**
  (exige distinguir peticion de primera/tercera parte, concepto que este
  motor no tiene sin `<iframe>`) - declarado asi en vez de fingir la
  defensa CSRF que aportaria; **`HttpOnly` idem**, solo tendria efecto
  ante un `document.cookie` que aun no existe; **sin persistencia a
  disco**, las sesiones no sobreviven a cerrar la app.
  19 tests nuevos (parseo de los tres formatos de fecha HTTP, cada
  atributo, rechazo de dominio ajeno, limites de etiqueta y de segmento,
  host-only, `Secure`, borrado por `Max-Age=0`, sustitucion por terna,
  varias `Set-Cookie` en una respuesta, orden por especificidad de ruta).
  **Verificado en vivo con un servidor de sesiones real**: navegar al
  formulario, escribir el usuario y pulsar Enter llevo a `/privado` con
  titulo "Panel de ana" y el texto "Bienvenido, ana. Tema: oscuro" - las
  DOS cookies de la respuesta 302 sobrevivieron; volver a navegar
  mantuvo la sesion; `/salir` (que reenvia la cookie con `Max-Age=0`)
  la cerro de verdad, y `/privado` paso a "Acceso denegado". Ciclo
  completo de autenticacion, de punta a punta.

- **`setTimeout`/`setInterval`/`clearTimeout`/`clearInterval`** (Fase 14):
  antes no existia NINGUN temporizador (cero referencias en todo
  `engine-js`), y practicamente todo JavaScript real los usa - carruseles,
  menus, `debounce`, reintentos, sondeo, y sobre todo el patron
  omnipresente `setTimeout(inicializar, 0)` para diferir el arranque hasta
  despues de que el documento este montado. Sin ellos la mayoria de
  paginas con JS se quedaban a medio inicializar, sin ningun error visible.
  **Como avanza el tiempo aqui, la simplificacion que mas importa**: un
  navegador real tiene un bucle de eventos con reloj propio y dispara un
  temporizador vencido aunque nadie toque nada. Este motor NO tiene ese
  reloj: los vencidos corren cuando alguien llama a
  `JsRuntime::run_due_timers`, y quien lo hace es `core::server` en
  `LoadedPage::relayout` (por donde pasan clic, escritura, tecla y
  redimension) mas una llamada explicita al final de `navigate` para los
  de la CARGA. Consecuencia honesta: un `setTimeout(fn, 100)` puesto al
  cargar SI se ejecuta, pero un reloj que se actualice solo cada segundo
  con la pagina quieta NO avanza hasta que el usuario haga algo. Cubre el
  uso dominante real (diferir inicializacion, reaccionar a una
  interaccion), no la animacion continua.
  Los vencidos corren por vencimiento ascendente y, a igualdad, por orden
  de creacion - el orden real del spec. Entre uno y otro se DRENAN los
  microtasks, porque cada callback es una TAREA del bucle de eventos
  (misma razon por la que `eval`/`dispatch_event` ya drenaban).
  `run_due_timers` ademas BUCLEA: un `setTimeout(fn, 0)` encolado desde
  otro temporizador tambien esta vencido ya y corre en el mismo ciclo.
  **Dos limites que existen por casos reales, no teoricos**: (1) los
  `setInterval` se acotan a un minimo de 4ms (regla de HTML spec 8.6 para
  temporizadores anidados) - sin eso, un `setInterval(fn, 0)` vuelve a
  estar vencido en el instante en que termina y un SOLO drenado lo
  dispararia cientos de veces; se descubrio con un test que fallo al
  escribir esta fase, no razonando. `setTimeout` NO se acota: dispara una
  vez, y un retardo cero ahi es exactamente lo que el autor pidio. (2)
  `MAX_TIMERS_PER_DRAIN = 1000` corta el bucle, porque un temporizador que
  se reencola sin condicion de parada colgaria el motor entero sin error.
  Los datos del temporizador viven en Rust pero la FUNCION de callback
  vive en un objeto JS oculto (`__engineTimerCallbacks`), no en la
  captura: capturar un `JsObject` dentro de un `NativeFunction` obliga a
  implementar `Trace` del recolector de Boa sobre una estructura que
  ademas contiene `Instant`/`Duration` (no rastreables), mientras que un
  objeto JS normal ya lo gestiona el recolector solo. El coste declarado
  es que ese objeto es alcanzable desde la pagina si lo busca por nombre.
  Un error dentro de un callback no aborta a los demas.
  Deliberadamente NO soportado: `setTimeout("codigo como cadena")`, un
  vector de inyeccion clasico que ya casi nadie usa.
  21 tests nuevos. Verificado en vivo con una pagina que difiere su
  inicializacion y encadena cinco `setTimeout`: ambos patrones corrieron
  enteros durante la propia carga.
- **`document.title` con getter Y setter reales** (Fase 14.1): descubierto
  verificando los temporizadores en vivo - la pagina de prueba se
  retitulaba desde un `setTimeout` y el titulo no cambiaba nunca.
  `document.title` no existia en `dom_bindings`, asi que `document.title =
  "x"` creaba una propiedad JS normal en el objeto `document`: la
  asignacion parecia funcionar desde JS (leerla devolvia lo asignado) pero
  el DOM real no se tocaba. Es un patron universal en aplicaciones web
  (marcar mensajes sin leer, reflejar la seccion actual).
  Ahora es un accessor sobre el `<title>` real: el setter escribe en su
  nodo de TEXTO (reutilizandolo si existe, creandolo si el elemento estaba
  vacio). Un documento SIN `<title>` no se inventa el elemento - haria
  falta crear tambien el `<head>` si falta - y la asignacion se ignora sin
  romper nada, mismo criterio honesto que el resto del modulo.
  Ademas, `core::server` dejo de reportar el titulo CONGELADO en la carga:
  `LoadedPage::current_title` lo relee del DOM en cada estado, porque si
  no, cualquier cambio posterior de JS quedaba invisible para la interfaz
  por muy bien que hubiera mutado el DOM. 3 tests nuevos.

- **`localStorage`/`sessionStorage` reales** (Fase 15): antes
  `net/src/storage.rs` era un `HashMap` suelto que nadie instanciaba y que
  no estaba conectado a JS - `localStorage` no existia como global, asi que
  cualquier pagina que lo usara moria con `ReferenceError`. Practicamente
  toda aplicacion web moderna lo usa para estado de sesion, preferencias y
  cache de cliente.
  Reescrito con **alcance por ORIGEN** (`esquema://host:puerto`), que es la
  parte que de verdad importa: `https://a.test` y `https://b.test` no se
  ven el almacenamiento entre si, ni siquiera `http://a.test` y
  `https://a.test` (esquema distinto = origen distinto, igual que el spec).
  El puerto por defecto del esquema NO forma parte del origen, para que
  `https://a.test` y `https://a.test:443` compartan almacenamiento como
  deben. Orden de insercion estable para `key(n)`/`length`; cuota de 5 MiB
  por origen con `QuotaExceededError` capturable de verdad en vez de
  crecer sin limite; valores SIEMPRE cadenas (`setItem('n', 42)` guarda
  `"42"`); `getItem` de una clave inexistente devuelve **`null`, no
  `undefined`** - las tres son diferencias observables que el codigo real
  comprueba.
  **Donde vive**: el almacen es de toda la SESION y lo conserva
  `EngineServer`, no la pagina - misma razon que las cookies, que por eso
  viven en `NetworkEngine`: su proposito entero es sobrevivir a navegar a
  otra pagina. Cada pagina recibe un puntero al mismo almacen mas SU
  origen, sacado de la URL FINAL tras redirecciones (si `http://a.test`
  redirige a `https://a.test`, el almacenamiento que toca es el del origen
  donde de verdad se aterrizo). Un script no puede pedir el de otro origen
  porque no hay ningun parametro con el que hacerlo.
  **Lo que NO soporta, declarado**: el acceso por PROPIEDAD. En un
  navegador real `localStorage.tema` y `localStorage.getItem('tema')` son
  equivalentes porque `Storage` es un objeto "exotico" que atrapa todo
  acceso a propiedad; aqui solo funcionan los metodos (`getItem`/
  `setItem`/`removeItem`/`clear`/`key`) y `length`. Implementar la forma
  con punto exigiria manejadores propios de `[[Get]]`/`[[Set]]`/
  `[[Delete]]`/`[[OwnPropertyKeys]]` en Boa - trabajo aparte y de bastante
  mas superficie; la forma con metodos es la que recomienda MDN y la que
  usa la mayoria del codigo real, asi que la perdida es acotada.
  **Sin persistencia a disco todavia**: `localStorage` deberia sobrevivir
  a cerrar la aplicacion y hoy no lo hace, asi que se comporta como un
  `sessionStorage` de vida larga. La diferencia ENTRE ambas areas SI esta
  modelada (son almacenes distintos que no se ven), que es lo que nota una
  pagina dentro de una misma sesion; lo que falta es solo el volcado a
  disco. Tampoco hay evento `storage` (avisar a otras pestañas del mismo
  origen): este motor no tiene comunicacion entre pestañas.
  23 tests nuevos (13 del almacen, 10 del binding JS). Verificado en vivo
  con dos servidores en puertos distintos: un contador subio de 1 a 2 al
  recargar, una segunda pagina del MISMO origen leyo lo que dejo la
  primera, `localStorage` y `sessionStorage` devolvieron valores distintos
  para la misma clave, una clave inexistente dio `null`, y el otro puerto
  (otro origen) no vio absolutamente nada.

- **Envio de formularios por POST** (Fase 16): la Fase 10 dejo el envio
  GET funcionando y POST devolviendo un error explicito, porque el motor
  no tenia forma de mandar un cuerpo en una peticion. La causa raiz estaba
  en el tipo del cliente HTTP: `Client<..., Empty<Bytes>>` hacia el cuerpo
  vacio POR TIPO, asi que `NetworkRequest::body` existia como campo y se
  ignoraba en silencio. Cambiado a `Full<Bytes>` (que se comporta igual
  que `Empty` cuando los bytes son cero, asi que no altera nada de lo que
  ya funcionaba) mas `Content-Length` explicito.
  Como casi todo formulario de LOGIN real es POST, esto y las cookies
  (Fase 13) son las dos piezas que juntas hacen posible autenticarse.
  El cuerpo se codifica como `application/x-www-form-urlencoded` (el tipo
  por defecto real sin `enctype`) con `url::form_urlencoded`, NO a mano:
  ese formato tiene una regla propia facil de equivocar - el espacio se
  codifica como `+`, no como `%20` - y un `&` sin escapar dentro de un
  valor partiria el campo en dos silenciosamente.
  El `action` de un POST se resuelve SIN tocar su query string
  (`resolve_submit_action`, compartida con el camino GET), a diferencia
  del GET que la reemplaza entera: los datos del POST van en el cuerpo,
  asi que un `action="/buscar?pagina=2"` conserva ese `pagina=2`.
  `navigate` se refactorizo a `navigate_with_body` con el cuerpo opcional
  en vez de duplicar la funcion: todo lo que viene despues de la peticion
  (redirecciones, sub-recursos, construccion de pagina, historial,
  temporizadores de carga) es identico para los dos metodos.
  **Cambio de criterio respecto a la Fase 10**: cualquier `method` que no
  sea `post` ahora degrada a GET y SE ENVIA, en vez de rechazarse. Es lo
  que hace el spec real, que solo reconoce `get`/`post`/`dialog` y trata
  cualquier valor invalido como el valor por defecto.
  **Limitacion declarada**: volver ATRAS a una entrada de historial creada
  por un POST deberia re-enviar el formulario (y un navegador real
  pregunta antes de hacerlo); aqui `back` la repite como GET, que es
  distinto. Sin `enctype="multipart/form-data"` tampoco, que es lo que
  exige subir ficheros - coherente con que `<input type="file">` ya se
  omite al recoger los datos (Fase 10).
  5 tests nuevos (reglas de codificacion con espacio/acentos/separador,
  cuerpo vacio, la query preservada frente al GET que la reemplaza, y el
  method desconocido que ahora degrada a GET). Verificado en vivo contra
  un servidor de login por POST: se rellenaron tres campos con un espacio
  (`ana lopez`), acentos y un `&` (`año & cía`) - las tres trampas
  clasicas de codificacion - y el clic en Enviar mando el POST, el
  servidor respondio 302 con `Set-Cookie`, y el panel privado mostro
  `usuario=ana lopez comentario=año & cía` intacto. Flujo de
  autenticacion real completo: POST + cookie + redireccion.

- **Colores CSS completos: nombres, `rgb()`/`rgba()`, hex con alfa**
  (Fase 17): `parse_css_color` solo entendia hexadecimal, asi que un
  `background: red` - de lo mas comun que existe en CSS real - no pintaba
  absolutamente nada. Como es el UNICO parseador de color del motor,
  arreglarlo aqui arreglo `color`, `background-color`, `border` y
  `box-shadow` a la vez.
  Ahora reconoce: los 16 nombres de CSS1 mas ~40 extendidos de uso comun
  (`NAMED_COLORS`, con las dos grafias de `gray`/`grey`), hex en sus tres
  longitudes reales (`#rgb`, `#rrggbb` y `#rrggbbaa` con canal alfa),
  `rgb()`/`rgba()` tanto en sintaxis clasica con comas como en la moderna
  con espacios y barra (`rgb(0 128 128 / 50%)`), componentes en numero o
  porcentaje, y la palabra clave `transparent` (que es un color de pleno
  derecho - negro con alfa cero - no la ausencia de color). Los
  componentes fuera de rango se ACOTAN en vez de rechazarse, igual que un
  navegador real ante `rgb(300, -5, 0)`.
  **La tabla NO es la lista completa** de los ~148 nombres extendidos: se
  eligieron los que cubren la practica totalidad del uso real, y un nombre
  fuera de ella resuelve a `None` (la caja se queda sin pintar) en vez de
  fingir un color inventado. Ampliarla es añadir filas, sin cambio de
  logica. NO implementado: `hsl()`/`hwb()`/`lab()`/`oklch()` y el resto de
  espacios de color modernos. `currentColor` devuelve `None` a proposito:
  resolverlo exige el `computed_style` completo, que esta funcion no
  recibe - y en el caso que mas importa (un borde) `parse_css_border` ya
  cae al `color` del elemento por su cuenta, que da el mismo resultado.
  **Reparto de responsabilidades entre crates**, que es la parte de diseño
  que importa: la expansion del shorthand `background` vive en
  `engine-css` y la tabla de colores en `engine-gfx`, que son crates
  HERMANOS (gfx no depende de css). En vez de duplicar la tabla, el
  reparto es: `engine-css` IDENTIFICA el token candidato a color
  (`background_color_candidate`) y `engine-gfx` decide si de verdad lo es.
  Un candidato que no sea un color simplemente no se pinta - la misma
  degradacion honesta de siempre, sin duplicar nada.
  Ese identificador trata aparte el caso dominante (`background: <color>` a
  secas) devolviendo el valor ENTERO sin trocear, que es lo que hace que
  `rgb(1, 2, 3)` funcione: partirlo por espacios lo romperia en pedazos
  inservibles. Solo cuando el valor trae ademas imagen o posicion se cae a
  buscar un token hexadecimal suelto - sin la tabla de nombres no hay
  forma fiable de distinguir ahi un nombre de color de una palabra clave
  de posicion, y pintar un fondo que el autor no pidio seria peor que no
  pintarlo.
  12 tests nuevos (8 en gfx sobre cada sintaxis, 4 en css sobre la
  expansion). Verificado en vivo con una pagina que ejercita las diez
  formas a la vez: los cuatro nombres, `rgb()` con comas, `rgba()` con
  alfa, `rgb()` con espacios, hex de 6 y de 8 digitos, y un `border: 3px
  solid crimson` - todos correctos, y la mezcla alfa real funcionando
  (`rgba(0,0,0,0.5)` sale gris sobre blanco, `#0088ff80` sale azul claro).

- **Pseudo-clases y `@media` reales** (Fase 18): dos huecos del motor de
  selectores que compartian una misma consecuencia grave - una regla con
  selector no soportado se descarta ENTERA, asi que un
  `input:checked + label` o cualquier bloque `@media` desaparecian sin
  dejar rastro.
  - **Pseudo-clases**: `NoPseudoClass` era un enum VACIO, lo que hacia que
    el parser rechazara cualquiera como error de sintaxis. Sustituido por
    `EnginePseudoClass`, dividido en dos grupos con criterios distintos:
    las **derivables del DOM** (`:checked`, `:disabled`/`:enabled`,
    `:required`/`:optional`, `:read-only`/`:read-write`, `:link`/
    `:any-link`) se resuelven de verdad leyendo atributos - `:checked` usa
    la misma semantica de atributo booleano HTML (presencia, no valor) que
    ya usaba `core::server::toggle_checked` al conmutarlo con un clic, asi
    que el "checkbox hack" funciona de punta a punta; y los **estados de
    interaccion** (`:hover`, `:focus`, `:active`, `:focus-visible`,
    `:focus-within`, `:visited`) se PARSEAN pero nunca coinciden, porque
    este motor no recalcula la cascada al mover el raton ni al enfocar.
    Parsearlos igualmente es la parte que importa: asi
    `.btn, .btn:hover` conserva su primera mitad en vez de perderse
    entera. `:visited` ademas no deberia coincidir NUNCA por privacidad -
    estilarlo permitiria filtrar el historial de navegacion por CSS, y los
    navegadores reales lo restringen severamente por eso. Una pseudo-clase
    que el motor no conoce SI sigue invalidando la regla: mejor no
    aplicarla que aplicarla mal.
    Descubrimiento de paso: las pseudo-clases ESTRUCTURALES
    (`:first-child`, `:last-child`, `:nth-child`, `:not`, `:empty`,
    `:root`) **ya funcionaban** - las resuelve el propio crate `selectors`
    recorriendo el arbol con metodos que `ElementRef` implementaba desde
    siempre. Nunca se habian probado; ahora hay un test que lo fija.
  - **`@media`**: antes las arroba-reglas se saltaban enteras. Ahora
    `AtRuleParser` parsea el preludio a un `MediaCondition` y recorre el
    bloque con OTRO `StyleSheetParser` anidado (el mismo codigo que la
    hoja de nivel superior, sin duplicar nada), estampando la condicion en
    cada regla de dentro. `StyleSheetParser` exige el mismo tipo de salida
    para reglas normales y arroba-reglas, de ahi que ambos produzcan
    `Vec<Rule>` - una regla normal devuelve un vector de un elemento.
    La condicion se guarda POR REGLA en vez de crear un nodo "bloque
    media": asi la cascada sigue siendo una lista plana ordenada por
    especificidad, y las reglas de dentro y fuera de un `@media` compiten
    entre si exactamente como en el spec.
    **Se evalua en la CASCADA, no al parsear**, y esa es la decision de
    diseño que importa: la hoja se parsea una sola vez por pagina pero
    `resolve_style` corre en cada relayout, asi que redimensionar la
    ventana REEVALUA las consultas sin reparsear nada. Para ello
    `resolve_style` recibe ahora `viewport_width`, enhebrado desde
    `LayoutTreeBuilder::build` (que ya lo tenia) a traves de `build_node`.
    Verificado en vivo: el mismo div pasa de teal a naranja y vuelve a
    teal al redimensionar 1280 -> 500 -> 1000 px.
    Solo se interpretan `min-width`/`max-width` en pixeles y el tipo de
    medio - lo que usa la practica totalidad del CSS responsive real.
    `max-width` es INCLUSIVO (asi lo define el spec: `max-width: 600px` SI
    aplica exactamente a 600px). Una consulta que el motor no sepa evaluar
    (`orientation`, `prefers-color-scheme`, unidades `em`) se marca
    `never_matches`: sus reglas se CONSERVAN pero no se aplican, que es
    mas honesto que aplicarlas siempre (meteria estilos de impresion o de
    movil en una ventana de escritorio) y que descartar el bloque. Las
    demas arroba-reglas (`@font-face`, `@keyframes`, `@supports`) se
    siguen saltando enteras sin corromper la hoja.
  23 tests nuevos. Verificado en vivo con una pagina que ejercita el
  checkbox hack completo (`#toggle:checked + .menu` sobre un menu con
  `display: none`, es decir tres piezas de fases distintas trabajando
  juntas), `:first-child`/`:last-child`, `:disabled`, `:required` y dos
  bloques `@media` complementarios - todo correcto.

- **Historial y pestañas expuestos en la interfaz** (Fase 19): el motor
  soportaba `back`/`forward`/`new_tab`/`close_tab`/`switch_tab`/`list_tabs`
  desde la Fase 4.4-4.5, y su `state` ya reportaba `can_go_back`/
  `can_go_forward`/`tab_id` en cada respuesta - pero la interfaz solo tenia
  UN boton (recargar) y una barra de direcciones, asi que nada de eso era
  alcanzable para el usuario. Era el hueco mas grande entre lo que el
  motor sabe hacer y lo que el producto deja hacer.
  Añadido en `frontend/`: botones atras/adelante que se habilitan con los
  flags REALES del motor (no con una cuenta paralela que podria
  desincronizarse), boton de pestaña nueva, y una barra de pestañas que
  solo aparece cuando hay mas de una - para no gastar espacio vertical
  cuando no aporta nada.
  De paso se corrigio una duplicacion que causaba el problema de fondo:
  el estado del motor se aplicaba en TRES sitios distintos (respuesta
  inicial IPC, evento `engine:state`, respuesta a un comando) y cada copia
  leia un subconjunto distinto de campos - por eso `can_go_back` y
  `tab_id` se perdian por el camino aunque el motor los enviara. Ahora hay
  una sola funcion (`applyEngineState`) que los aplica todos.
  `list_tabs` es la unica peticion que NO devuelve un `state` sino su
  propia respuesta, asi que se pide aparte (`refreshTabs`) tras cualquier
  accion que pueda cambiar el conjunto de pestañas - incluida `navigate`,
  porque el titulo de la pestaña activa cambia con la pagina.
  Verificado contra el motor real, comando a comando: `back`/`forward` con
  sus flags correctos, `new_tab` creando y activando la pestaña 1,
  `list_tabs` mostrando ambas con titulo, `switch_tab` volviendo a la 0
  **con su propio historial intacto** (`can_go_back: true`, mientras que
  la 1 lo tenia en `false` - el historial es por pestaña, como debe ser), y
  `close_tab` dejando solo una. La aplicacion empaquetada arranco con esta
  interfaz y el motor nuevo, y cerro sin dejar procesos huerfanos.

- **Politica de mismo origen y CORS reales** (Fase 20): `net/src/cors.rs`
  era un stub que devolvia `true` siempre y que ademas **nadie llamaba** -
  `NetworkEngine::fetch` no lo invocaba, asi que un `fetch()` desde
  JavaScript podia leer la respuesta de CUALQUIER dominio. Sin politica de
  mismo origen no hay modelo de seguridad web: es la primitiva sobre la que
  descansa todo lo demas.
  **Donde se aplica y donde no**, que es la parte de diseño que importa:
  CORS solo gobierna las peticiones que un SCRIPT inicia y cuya respuesta
  quiere LEER (`fetch`/`XMLHttpRequest`). NO se aplica a la navegacion
  (escribir una URL en la barra no es una peticion de origen cruzado, es
  cambiar de origen) ni a los subrecursos (`<link>`/`<script src>`/`<img>`,
  que van en modo "no-cors": se descargan de otro dominio pero su
  contenido no se expone a JS - y este motor tampoco lo expone). Esa
  frontera coincide EXACTAMENTE con la que ya separaba `engine-js` de
  `core::server`, asi que el modelo se activa simplemente pasando `origin`
  o no en `NetworkRequest`.
  Implementado de verdad: comparacion de origen real, cabecera `Origin` en
  peticiones cruzadas, comprobacion de `Access-Control-Allow-Origin` (`*` o
  coincidencia EXACTA - nada de sufijos ni comodines de subdominio, que es
  justo el agujero que CORS existe para tapar), distincion entre peticion
  simple y con **preflight** (con las listas seguras de metodos y
  cabeceras del spec), y comprobacion de `Access-Control-Allow-Methods` en
  la respuesta al OPTIONS. El preflight se manda ANTES que la peticion
  real, no despues: su proposito es no ejecutar en el servidor algo que
  quiza no estaba autorizado (un DELETE), y comprobar a posteriori no
  evitaria el daño.
  **Credenciales**: las cookies dejan de viajar a otro origen salvo que se
  pidan explicitamente - `credentials: "same-origin"` es el valor por
  defecto real del fetch spec, y mandar la sesion del usuario a un tercero
  sin que nadie lo pida es justo el ataque que esto existe para impedir. Y
  con credenciales, `*` NO vale como `Access-Control-Allow-Origin` y hace
  falta ademas `Access-Control-Allow-Credentials: true`: las dos reglas
  que impiden que un servidor abra su API a todo el mundo por accidente.
  **Dos huecos preexistentes que esta verificacion destapo**, ninguno
  causado por CORS pero los dos invisibles hasta ahora:
  1. `XMLHttpRequest` no LANZABA al fallar: se limitaba a poner
     `status = 0` y disparar `onerror`. Eso es correcto para un XHR
     asincrono, pero este es SIEMPRE sincrono (limitacion declarada del
     modulo) y el spec exige que un `send()` sincrono fallido lance un
     `NetworkError`. Sin ello, un bloqueo por CORS dejaba `responseText`
     vacio sin ninguna señal capturable con `try`/`catch`: la pagina no
     podia distinguir "bloqueado" de "el servidor devolvio vacio". Ahora
     lanza, con el motivo REAL en el mensaje (que cabecera falta), y los
     manejadores se siguen disparando antes.
  2. `fetch`/XHR no resolvian URLs RELATIVAS: `Url::parse` exige una
     absoluta, asi que `fetch('/api/datos')` - comunisimo en codigo real -
     fallaba. Ahora se resuelven contra la URL de la pagina
     (`request::resolve_against_page`), que ademas es de donde sale el
     origen. De ahi que `StorageContext` lleve la URL COMPLETA y no solo
     el origen: los dos salen del mismo sitio, y pasarlos por separado
     abriria la puerta a que un dia no coincidieran y el aislamiento de
     red dejara de corresponderse con el de almacenamiento.
  NO implementado, declarado: cache de preflight
  (`Access-Control-Max-Age`) - cada peticion no simple manda su OPTIONS;
  y `Access-Control-Expose-Headers` - este motor no filtra que cabeceras
  de respuesta ve el script, asi que las expone todas (mas permisivo que
  el spec).
  14 tests nuevos. Verificado en vivo con DOS servidores en origenes
  distintos: mismo origen se lee (con URL relativa), origen cruzado con
  `*` se lee, con origen nombrado se lee, **sin cabecera CORS se BLOQUEA**
  (el secreto de la API nunca llego a la pagina), un DELETE con preflight
  autorizado se ejecuta, y un DELETE sin autorizar se bloquea.

- **Content Security Policy** (Fase 21): CSP es la defensa principal
  contra XSS - deja que un servidor declare de DONDE puede venir el codigo
  y los recursos de su pagina, de modo que un script inyectado por un
  atacante no se ejecute aunque llegue a colarse en el HTML. No existia
  nada de esto.
  Se aplica de punta a punta porque el motor tiene los ganchos exactos:
  `script-src` gobierna la ejecucion de cada `<script>` en
  `core::scripting`; `style-src` y `img-src` filtran los subrecursos en
  `core::server` ANTES de descargarlos (pedirle algo a un origen no
  autorizado ya filtraria que la pagina lo visito, aunque luego se
  descartara); `default-src` es el respaldo de todas.
  La politica se toma de la cabecera `Content-Security-Policy` Y del
  `<meta http-equiv>` (muchas paginas la ponen asi, sobre todo las
  servidas desde sitios estaticos donde no se controlan las cabeceras). Si
  vienen las dos se COMBINAN de forma restrictiva - hay que pasar ambas.
  Combinarlas relajando seria un agujero: quien pudiera inyectar un
  `<meta>` desactivaria la politica del servidor, justo lo que CSP existe
  para impedir.
  **La regla que mas se malinterpreta, implementada tal cual**: CSP solo
  restringe lo que MENCIONA. Sin `script-src` ni `default-src`, los
  scripts se permiten - una politica que solo diga `img-src 'self'` no
  bloquea JavaScript. Es asi en el spec y es lo que hace que añadir CSP a
  un sitio existente no lo rompa entero.
  Fuentes soportadas: `'none'`, `'self'`, `'unsafe-inline'`, `*`, esquema
  (`https:`), host exacto y comodin de subdominio (`*.cdn.test`, que cubre
  subdominios pero NO el dominio desnudo, y exige separador de etiqueta
  real - mismo criterio que las cookies y por la misma razon: sin el,
  `malcdn.test` colaria).
  **Sin nonces ni hashes** (`'nonce-...'`, `'sha256-...'`): son la forma
  moderna de permitir scripts inline concretos. Se PARSEAN (para no
  confundirlos con un host) pero no habilitan nada, asi que una politica
  basada solo en nonce bloqueara sus scripts - el lado seguro del error:
  bloquear de mas, nunca de menos. Sin `report-uri`/`report-to` ni
  `Content-Security-Policy-Report-Only` (esta ultima se ignora entera, que
  es exactamente lo que debe hacer: por definicion no bloquea nada). Sin
  `frame-ancestors`/`form-action`/`base-uri`, que exigen `<iframe>`/
  `<base>`, inexistentes aqui.
  **Hueco declarado**: el `<style>` EN LINEA no se bloquea todavia (solo
  el `<script>` inline y las hojas externas). Bloquearlo exigiria filtrar
  los `<style>` antes de concatenarlos en `pipeline::build_page`, que hoy
  no recibe la politica.
  17 tests nuevos. Verificado en vivo sirviendo LA MISMA pagina con y sin
  cabecera, para que la unica variable fuera la politica: sin CSP, el
  script inline corrio (cambio el titulo y el texto) y la hoja de estilos
  de OTRO origen se aplico (fondo rojo); con `default-src 'self'`, el
  script inline quedo bloqueado (titulo y texto intactos) y la hoja
  externa tambien (el fondo verde del `<style>` propio sobrevivio).
- **`!important` real** (Fase 22): se descubrio verificando CSP, porque
  la primera version del CSS de prueba lo usaba y no pintaba.
  `background: #ff0000 !important` se guardaba con el sufijo PEGADO al
  valor, y `parse_css_color` no lo reconocia - asi que la declaracion
  ganaba la cascada (sobrescribia a las anteriores) pero luego no pintaba
  nada. **Era peor que ignorarlo**: una regla con `!important` no solo no
  se aplicaba, ademas ANULABA la que habria ganado sin ella.
  Arreglado en dos sitios. En el PARSER (`split_important`), el sufijo se
  separa del valor - asi todo lo que consume un valor CSS (parseo de
  color, de longitud...) lo recibe limpio, sin tener que saber nada de la
  cascada; tolera espacios y mayusculas (`! IMPORTANT` es valido). El dato
  se guarda aparte, como un conjunto de NOMBRES de propiedad en
  `Rule::important`, para no cambiar el tipo de `declarations`, que
  consumen media docena de sitios. Un shorthand importante contagia su
  importancia al longhand que se deriva de el (`background: red
  !important` hace importante tambien al `background-color` expandido).
  En la CASCADA (`apply_matching_rules`), DOS pasadas: primero las
  declaraciones normales por especificidad, luego las importantes -
  tambien por especificidad entre ellas. Asi una declaracion importante
  gana a CUALQUIER normal sin importar su selector, que es justo lo que
  `!important` significa y lo unico que no se puede expresar con un solo
  orden de aplicacion.
  NO modelado: `!important` dentro del atributo `style="..."` - ese ya
  gana a cualquier selector de todas formas (ver `resolve_style`), asi que
  solo importaria frente a OTRO `!important` de una hoja, caso raro. Se
  descarta su marca y se queda el valor limpio, que es lo que arregla el
  bug de parseo. Tampoco el orden de origenes completo del spec
  (author-important pierde frente a user-agent-important), irrelevante
  aqui porque la hoja de agente de usuario no usa `!important`.
  5 tests nuevos (importante gana a mayor especificidad, el sufijo se
  quita del valor, entre dos importantes vuelve a mandar la
  especificidad, las normales no cambian de comportamiento, y el
  contagio shorthand -> longhand). Verificado en vivo: un
  `div { background: #ff0000 !important }` ahora se pinta rojo de verdad
  ganando a un `#caja { background: #00aa00 }` mas especifico - antes no
  se pintaba nada.

- **Mitigaciones de proceso** (Fase 23): **esto NO es un sandbox, y decirlo
  claro es parte del cambio.** Un sandbox de verdad separa el proceso que
  interpreta contenido hostil del que tiene permisos: el renderizador corre
  con un token restringido que no puede leer los ficheros del usuario, y un
  proceso "broker" hace por el las operaciones privilegiadas. Aqui
  `engine_server` hace su propia red y su propio disco, asi que restringir
  su token lo romperia; separarlos es un refactor arquitectonico grande, no
  una bandera que activar. Una mitigacion presentada como sandbox da una
  falsa sensacion de seguridad, que es peor que no tener nada.
  Lo que SI se aplica, al arrancar `engine_server` y antes de tocar la red
  o parsear nada (las politicas de Windows solo se pueden endurecer, nunca
  aflojar, asi que cuanto antes mejor):
  - **Prohibir codigo dinamico** (`ProcessDynamicCodePolicy`): impide crear
    o modificar paginas de memoria ejecutables, bloqueando de raiz la
    tecnica clasica de "escribir shellcode y saltar a el". **Este motor
    puede permitirselo y un navegador comercial no**: su JavaScript corre
    en `boa`, un INTERPRETE sin JIT - y un JIT necesita exactamente eso que
    aqui se prohibe. Es una ventaja de seguridad real que sale de una
    decision de diseño previa, no un extra; Chromium no puede activarla
    porque V8 compila en caliente.
  - **Prohibir procesos hijo** (`ProcessChildProcessPolicy`): casi toda
    cadena de explotacion termina lanzando `cmd.exe`/`powershell.exe`.
    `engine_server` no lanza procesos nunca, asi que prohibirlo no le
    cuesta nada y corta ese final.
  - **Desactivar puntos de extension**
    (`ProcessExtensionPointDisablePolicy`): bloquea mecanismos heredados de
    inyeccion de DLL (AppInit_DLLs, ganchos de SetWindowsHookEx, capas de
    proveedor Winsock).
  Cada una se intenta por separado y un fallo no impide las demas: una
  version de Windows puede no soportar una politica concreta, y perder las
  otras dos por eso seria absurdo. Deliberadamente NO se aplica
  `ProcessSignaturePolicy` (solo-DLL-firmadas-por-Microsoft): romperia la
  carga de controladores graficos de terceros, y ahi seria adivinar en vez
  de saber. En plataformas que no son Windows es un no-op que lo DECLARA en
  su informe, en vez de fingir que aplico algo.
  El informe sale por stderr y nunca por stdout, que es el canal NDJSON -
  una linea que no fuera JSON romperia el protocolo.
  4 tests, uno de ellos comprobando el EFECTO real y no el valor
  devuelto: tras aplicar la mitigacion, lanzar un proceso hijo tiene que
  fallar de verdad. Verificado ademas que el motor sigue funcionando
  entero con las tres puestas - carga local con JavaScript ejecutandose
  (`[JS OK]` en el titulo, 34 elementos, PNG de 63KB) y Wikipedia por
  HTTPS real (2.024 elementos): ni el interprete de JS ni TLS ni el
  rasterizado se ven afectados.

- **`document.cookie` real, con `HttpOnly` protegiendo por fin de verdad**
  (Fase 24): antes de esto `HttpOnly` se parseaba y se guardaba (Fase 16)
  pero no protegia nada, por la razon mas simple posible - no habia ningun
  `document.cookie` del que protegerla. Encontrado auditando el motor tras
  cerrar la Fase 23: la doc de `cookie.rs` declaraba la limitacion
  explicitamente, y seguia sin cerrarse.
  `net/src/cookie.rs::CookieStore` gana `header_for_js` (la mitad LECTURA
  de `document.cookie`: MISMO filtrado por dominio/ruta/`Secure` que
  `header_for`, la funcion que ya usan las peticiones de red reales, mas UN
  filtro extra - descarta toda cookie `http_only`) y `set_from_js` (la
  mitad ESCRITURA: reusa `parse_set_cookie`, la misma gramatica de
  atributos que un `Set-Cookie` de servidor, pero fuerza `http_only =
  false` pase lo que el script haya escrito - un script no puede crearse
  una cookie a la que el mismo no pueda acceder despues, igual que hace un
  navegador real, que ignora el atributo en vez de rechazar la cookie
  entera). Las dos comparten el mismo `Vec<Cookie>` que ya usaba
  `header_for`, via una funcion privada `matching_cookies(url,
  include_http_only)` de la que `header_for`/`header_for_js` son ahora dos
  finales distintos - no hay dos almacenes que pudieran desincronizarse.
  `NetworkEngine` (`http_client.rs`) expone `cookie_header_for_js`/
  `set_cookie_from_js` tomando `page_url: &str` (parsea la `Url`
  internamente - `engine-js` no depende de el crate `url` en produccion, a
  proposito, y esto evita añadirlo solo para esto) sobre el MISMO
  `Mutex<CookieStore>` que ya usan `fetch`/las peticiones de red: una
  cookie puesta por `document.cookie = ...` viaja despues en el `Cookie:`
  de un `fetch()` posterior, y una puesta por el servidor aparece en la
  siguiente lectura de `document.cookie` (salvo que sea `HttpOnly`).
  El binding nuevo, `engine-js/src/cookie.rs::register_cookie`, sigue el
  mismo patron que `fetch.rs`/`xhr.rs` (una funcion `register_*` aparte,
  llamada solo donde hay `NetworkEngine` disponible) en vez de meterse
  dentro de `DocumentBindings::register`: cuelga el accessor (getter Y
  setter reales, via `PropertyDescriptor::builder()` + `define_property_or_
  throw` - el mismo mecanismo que ya usa `xhr.rs` para sus propios
  accessors, no el `ObjectInitializer` que usa `document.title` porque ese
  construye un objeto NUEVO y aqui hace falta añadir una propiedad a un
  `document` que YA EXISTE) sobre el `document` que `bind_dom` ya creo -
  por eso `core/scripting.rs` lo registra DESPUES de `bind_dom` (implicito,
  ya habia corrido) y junto a `register_fetch`/`register_xhr`, con el mismo
  criterio de "solo si hay red" que esos dos. `page_url: None` (sin
  `NetworkEngine` o sin URL propia) deja `document.cookie` como cadena
  vacia siempre y el setter como no-op silencioso, mismo criterio que
  `fetch` sin red.
  12 tests nuevos repartidos en las tres capas (5 en `cookie.rs` de
  `engine-net` sobre `CookieStore` directamente, 3 en `http_client.rs`
  sobre `NetworkEngine::cookie_header_for_js`/`set_cookie_from_js`, 4 en
  `engine-js/src/cookie.rs` evaluando JS de verdad contra un `JsRuntime`)
  - incluido el caso que motivo todo esto: una cookie `HttpOnly` puesta por
  el servidor SI viaja en la siguiente peticion de red pero NUNCA aparece
  en `document.cookie`, y un script que intente `document.cookie = "a=1;
  HttpOnly"` para escondersela a si mismo no lo consigue - la cookie queda
  visible por el mismo `document.cookie` que la creo, exactamente el
  comportamiento de un navegador real. 610 tests en total tras esta fase
  (598 + 12).
  **Simplificaciones que siguen igual, declaradas**: `SameSite` sigue sin
  aplicarse (exige distinguir peticion de primera parte de tercera parte,
  que este motor no modela todavia - sin cambios respecto a la Fase 16);
  sin lista de sufijos publicos (PSL); las COOKIES (a diferencia de
  `localStorage` desde esta Fase) siguen sin persistir a disco.

- **`localStorage` persiste a disco entre sesiones** (Fase 25): antes de
  esto `localStorage` sobrevivia solo mientras el proceso `engine_server`
  seguia vivo - cerrar la app lo borraba todo, asi que en la practica se
  comportaba como un `sessionStorage` de vida mas larga. Encontrado
  auditando el motor tras la Fase 24: la doc de `net/src/storage.rs`
  declaraba la limitacion explicitamente.
  `WebStorage` gana un campo `persist_path: Option<PathBuf>` - `None` para
  `WebStorage::new()` (la version en memoria pura, la que siguen usando
  TODOS los tests de este modulo y `wpt_runner`) y `Some(ruta)` solo para
  `WebStorage::load_from_disk()`, la que usa `core::server` en produccion.
  La ruta sale de `dirs::data_dir()` (crate nuevo, minimo: 3 dependencias
  transitivas) - `%APPDATA%\navegador-ia\local_storage.json` en Windows,
  `~/Library/Application Support/navegador-ia/local_storage.json` en
  macOS, `$XDG_DATA_HOME` (o su fallback `~/.local/share`) en Linux; las
  tres convenciones tienen bordes reales (roaming vs local en Windows,
  fallback de XDG) que no valia la pena reimplementar a mano, mismo
  criterio de dependencias que TLS/parseo HTML. `load_from_disk` carga el
  area `local` guardada (JSON invalido o fichero ausente se tratan igual
  que "sin datos previos" - un perfil corrupto no deberia impedir
  arrancar el navegador). Cada `set_item`/`remove_item`/`clear` sobre
  `local` (nunca sobre `session` - persistirla la convertiria en `local`,
  que es justo la diferencia entre las dos areas que exige el spec)
  vuelca el mapa ENTERO de vuelta al mismo fichero de forma SINCRONA, sin
  `debounce` - simplificacion deliberada, mismo criterio que el resto del
  motor (cada mutacion de DOM/cookie ya se aplica de inmediato).
  `serde`/`serde_json` (ya eran dependencias de workspace, usadas por
  `core` para NDJSON) se extendieron a `engine-net` para esto - no entra
  ningun crate nuevo de serializacion en el arbol.
  6 tests nuevos en `storage.rs` sobre `WebStorage::load_from_path`
  (el nucleo interno de `load_from_disk`, parametrizado por ruta para
  poder probarlo sin tocar el `%APPDATA%` REAL del usuario): ida y vuelta
  completa (escribir, releer desde una instancia nueva), que
  `sessionStorage` JAMAS toca disco, fichero ausente y fichero corrupto
  tratados como "sin datos", y que `remove_item` tambien persiste (no solo
  `set_item`). 616 tests en total tras esta fase (610 + 6).
  **Verificado en vivo, no solo con tests**: dos procesos SEPARADOS de
  `engine_server.exe` (no dos pestañas del mismo proceso) contra un
  servidor HTTP local real - el primero, sin datos previos, lee
  `localStorage.getItem('marca')` como `null` y guarda un valor; el
  segundo, un binario arrancado de cero, lo recupera del disco y lo
  refleja en `document.title`. El fichero resultante en disco se inspecciono
  directamente (`{"http://127.0.0.1:8765":{"items":[["marca","valor-
  persistente"]]}}`) antes de limpiarlo.
  **Simplificacion que sigue igual, declarada**: sin evento `storage` entre
  pestañas (este motor no tiene comunicacion entre pestañas, sin cambios
  respecto a la Fase 15).

- **CSP `style-src` gatea de verdad el `<style>` en linea de la pagina**
  (Fase 26): la doc de `net::csp` (Fase 21) ya afirmaba "`style-src`:
  `core::server` decide si aplicar cada `<style>`", pero eso era falso -
  auditado tras la Fase 25: `pipeline::build_page_keeping_runtime`
  concatenaba TODOS los `<style>` del documento en `combined_css` sin
  mirar ninguna politica, a diferencia de `<script>` en linea, que si
  pasaba por `allows_inline("script-src")` desde la Fase 21. Un
  `<style>` colado por una inyeccion de HTML (la pagina no sanea algun
  campo) se aplicaba exactamente igual que uno legitimo - lo que CSP
  existe para impedir.
  El fix es de una linea de logica real: `build_page_keeping_runtime`
  calcula `allow_inline_style = storage.as_ref().is_none_or(|ctx|
  ctx.csp.allows_inline("style-src"))` ANTES de mover `storage` dentro de
  `scripting::execute_inline_scripts_keeping_runtime` (mismo patron ya
  usado ahi para `script-src`), y solo recorre los `<style>` del
  documento si `allow_inline_style` es cierto - `None` (sin
  `StorageContext`, el camino de `wpt_runner`/tests sin red) se sigue
  permitiendo, igual que "sin politica" en el spec real. `build_page`/
  `build_page_with_harness` (las variantes sin red, sin `storage` en su
  firma) se quedan intactas a proposito - mismo criterio que ya aplicaba a
  `<script>`: ninguna pagina de test deberia ver bloqueado su CSS por una
  politica que no existe en ese camino.
  3 tests nuevos en `pipeline.rs`: `style-src 'none'` bloquea el `<style>`
  propio de la pagina (el caso que faltaba), `style-src 'unsafe-inline'`
  lo sigue permitiendo (que el fix no bloquee de mas), y sin
  `StorageContext` en absoluto tambien se sigue aplicando (que "sin CSP"
  no se confunda con "CSP que bloquea todo"). 619 tests en total tras esta
  fase (616 + 3).
  **Efecto lateral de esta auditoria, sin relacion con CSP**: un `sed`
  global de la Fase 24 (`document.cookie`) habia renombrado por error tres
  comentarios PRE-EXISTENTES de `scripting.rs` que documentaban CSP real
  de la Fase 21 (`script-src`), dejandolos decir "Fase 24" - corregido de
  vuelta a "Fase 21" en el mismo commit que esta fase, encontrado
  revisando ese archivo para el fix de arriba.
  **Simplificacion declarada, sin cambios**: `style-src-attr` (el
  atributo `style="..."` de un elemento, distinto del bloque `<style>`)
  sigue sin gatearse - llega al layout por un camino totalmente distinto
  (`cascade::resolve_style` lee el atributo directamente, no pasa por
  `combined_css`), y conectarlo ahi tambien es trabajo aparte.

- **`fetch(url, options)` real: `method`/`headers`/`body`/`credentials`**
  (Fase 27): el hallazgo mas impactante de esta racha de auditorias -
  `fetch()` SIEMPRE hacia GET sin cuerpo, sin importar que `options` se le
  pasara. `engine-net` dejo de tener esa limitacion en la Fase 16
  (`Full<Bytes>` en vez de `Empty` - el mismo fix que arreglo el envio de
  formularios POST), pero nadie volvio a conectar `fetch()` con ella; el
  doc-comment del modulo seguia citando la razon ORIGINAL de la Fase 4.3
  como si siguiera vigente. `fetch(url, {method:'POST', body:...})` es
  el patron mas comun de AJAX moderno - mas que `XMLHttpRequest`, que ya
  tenia soporte de metodo/cabeceras desde la Fase 9 (aunque el propio
  `send(body)` de XHR sigue sin enviar cuerpo, ver su aviso - gap
  simetrico, no cerrado en esta fase).
  `read_fetch_options` (necesita `Context` de Boa para leer el objeto JS)
  parsea `method` (mismo mapeo de 7 verbos que `xhr::parse_method`,
  duplicado a proposito - ~10 lineas, no vale la pena una dependencia
  entre modulos por esto), `headers` (recorre `own_property_keys` del
  objeto plano, sin la clase `Headers` real - mismo criterio de
  simplificacion que ya declaraba `response.headers` al LEER), `body`
  (convertido a UTF-8 con el `ToString` real de JS: un objeto sin
  `JSON.stringify` explicito da `"[object Object]"`, igual que un
  navegador real) y `credentials` (`'include'` activa
  `NetworkRequest::include_credentials`, cualquier otro valor se queda en
  el default real del spec, `'same-origin'`). `apply_fetch_options`
  (logica PURA, sin `Context` ni red, separada a proposito - mismo
  criterio que `redirect_decision` en `http_client.rs`) vuelca eso sobre
  la `NetworkRequest` YA construida, y añade `Content-Type:
  text/plain;charset=UTF-8` solo si hay cuerpo Y `headers` no puso ya uno
  - el default real del spec para un cuerpo de cadena. Un `GET`/`HEAD` con
  `body` rechaza la promise con un `TypeError` SINCRONO, sin resolver la
  URL ni tocar la red - el `Request constructor` real del spec hace
  exactamente eso.
  9 tests nuevos en `fetch.rs`: cada campo de `options` parseado por
  separado (metodo case-insensitive, cuerpo UTF-8, cabeceras multiples,
  `credentials`), `apply_fetch_options` aplicando todo a una
  `NetworkRequest` de prueba SIN red, que no pisa un `Content-Type`
  explicito, y el rechazo sincrono de `GET`+`body`. 628 tests en total
  tras esta fase (619 + 9).
  **Verificado en vivo**: un `<script>` real haciendo `fetch('/echo',
  {method:'POST', headers:{'Content-Type':'application/json','X-Marca':
  'presente'}, body: JSON.stringify({saludo:'hola'})})` contra un
  servidor Python que hace eco de metodo/cabeceras/cuerpo recibidos, con
  el resultado volcado a `document.title` desde el `.then()` - un solo
  `navigate` (sin esperar a un segundo comando) devolvio el eco COMPLETO
  y exacto: `RESP:method=POST;ct=application/json;marca=presente;
  body={"saludo":"hola"}`. Confirma de paso que el bloqueo declarado del
  modulo (`fetch()` bloquea el hilo hasta que la promise resuelve) sigue
  siendo cierto: el `.then()` ya habia corrido para cuando `navigate`
  devolvio la respuesta.
  **`XMLHttpRequest.send(body)` cerrado en la MISMA fase**: el gap
  simetrico que dejaba esta entrada mas arriba no llego a otra fase -
  `send(body)` convierte el valor a UTF-8 con el mismo `ToString` real de
  JS que `fetch()`, y `attach_send_body` (logica PURA, mismo criterio de
  extraccion que `apply_fetch_options`) lo vuelca sobre la
  `NetworkRequest`. La UNICA diferencia real con `fetch()`, y es del spec,
  no de este motor: XHR con `GET`/`HEAD` IGNORA el cuerpo en silencio
  ("if data is not null and method is GET or HEAD, then set data to
  null"), mientras que `fetch` rechaza con `TypeError` - los dos motores
  reales (Chrome/Firefox) tratan esas dos APIs distinto ahi, y este motor
  ahora tambien. 4 tests nuevos en `xhr.rs` (`attach_send_body` con
  POST/GET/HEAD y con/sin `Content-Type` ya puesto, mas un `send('cuerpo')`
  de verdad que completa el ciclo hasta `DONE` sin lanzar). 632 tests en
  total tras esta fase (628 + 4). **Verificado en vivo tambien**: el mismo
  servidor de eco, ahora con un XHR SINCRONO
  (`x.open('POST', '/echo', false)`) - mismo resultado exacto,
  `document.title` reflejando el eco completo en la MISMA linea de
  `navigate`.
  **Simplificacion que sigue igual, declarada**: sin la clase `Headers`
  real (ni para leer ni para escribir, en ninguna de las dos APIs).

- **`hsl()`/`hsla()` real en `parse_css_color`** (Fase 28): `hsl()` es,
  junto a `rgb()`, la forma mas comun de declarar color en CSS real
  moderno (temas/paletas generadas por variables SASS o custom properties
  casi siempre usan HSL porque separar matiz/saturacion/luminosidad hace
  mas facil generar variantes claras/oscuras que con RGB) - antes de esta
  fase, `background: hsl(210, 60%, 50%)` no pintaba absolutamente nada
  (la propia doc de la funcion ya declaraba el hueco). El shorthand
  `background: hsl(...)` YA pasaba sin cambios por
  `background_color_candidate` en `engine-css` (ninguna palabra de
  `BACKGROUND_NON_COLOR_KEYWORDS` aparece dentro de un `hsl(...)`, asi
  que cae en la misma rama que ya usaba `rgb()`) - todo el trabajo real
  fue en `engine-gfx`, el unico crate que sabe pintar.
  `hsl_to_rgb` es la formula estandar del spec (CSS Color 4 §4.2), sin
  aproximacion: matiz normalizado con `rem_euclid` ANTES de convertir (un
  matiz negativo o mayor de 360 - legal en el spec - da el mismo color que
  su equivalente normalizado, nunca un canal fuera de rango).
  `parse_hue_degrees` acepta las cuatro unidades de angulo del spec para
  el matiz (`deg`, sin unidad - la mas comun en CSS real, equivale a
  `deg` -, `grad`, `rad`, `turn`); saturacion/luminosidad EXIGEN `%` (la
  sintaxis clasica real, sin la forma moderna sin unidad de CSS Color 4).
  Acepta la sintaxis clasica con comas y la moderna con espacios/`/` para
  el alfa, igual que ya hacia `rgb()` - mismo parseo de alfa reusado tal
  cual.
  5 tests nuevos (637 en total tras esta fase): los tres primarios mas
  cian, los limites de luminosidad (0%/100% siempre negro/blanco sin
  importar matiz), las dos sintaxis con alfa, el matiz circular
  (`-120` = `240`, `360` = `0`), las cuatro unidades de angulo dando el
  mismo color que su equivalente en grados, y que saturacion/luminosidad
  SIN `%` sea invalido (a diferencia de los componentes de `rgb()`, que
  si aceptan numero puro). Ademas se corrigio un test EXISTENTE
  (`unsupported_color_syntaxes_...`) que afirmaba "hsl() no esta
  implementado" - ya no es cierto, sustituido por `hwb()`/`oklch()`, que
  siguen sin estarlo.
  **Simplificacion que sigue igual, declarada**: `hwb()`/`lab()`/`lch()`/
  `oklab()`/`oklch()` y el resto de espacios de color modernos del spec
  siguen sin implementarse - devuelven `None` y la caja se queda sin
  pintar, en vez de fingir una conversion.

- **`text-decoration: underline` se PINTA de verdad** (Fase 29): la
  cascada ya resolvia esta propiedad desde que existe - no esta en
  `BACKGROUND_NON_COLOR_KEYWORDS` ni necesita estarlo, es una propiedad
  CSS normal que `resolve_style` copia igual que cualquier otra - pero
  nada la pintaba, ni siquiera el caso mas comun de todos: la hoja de
  agente de usuario ya declara `a { text-decoration: underline; }`
  (ver `user_agent_stylesheet.rs`) desde antes de esta fase, asi que
  TODO enlace de TODA pagina real se pintaba sin subrayado.
  `DisplayItem::Text` gana un campo `underline: bool`
  (`resolve_text_decoration_is_underline` en `display_list.rs`, mismo
  patron que `resolve_font_weight_is_bold`/`resolve_font_style_is_italic`
  - busca el token `underline` entre los valores separados por espacio,
  asi que reconoce tanto `text-decoration: underline` como el shorthand
  compuesto `underline dotted red`; `line-through`/`overline` se
  reconocen igual como candidatos pero ninguno de los dos esta conectado
  a nada que pinte, declarado).
  `engine-text` gana dos funciones nuevas (`baseline_offset`,
  `underline_metrics`) que leen las metricas REALES de la propia fuente
  via `ttf_parser::Face` (`ascender()` y `underline_metrics()` - la
  tabla `post`/`OS-2` que cada fuente declara) en vez de una fraccion
  inventada de `font_size` - asi el subrayado queda pegado al texto con
  la misma posicion/grosor que usaria un navegador real para esa MISMA
  fuente, no una aproximacion generica. Con respaldo a una fraccion de
  `font_size` (~8% bajo el baseline, ~6% de grosor) SOLO si la fuente no
  declara esas metricas (fuentes incompletas/sinteticas).
  `paint_text` (en `engine-gfx::paint`) dibuja la franja POR LINEA (una
  caja de texto envuelta en varias lineas por `wrap_text` subraya cada
  una con SU propio ancho real, via `measure_text` de esa linea - no la
  caja entera, que dejaria una franja de mas en la ultima linea mas corta
  de un parrafo), y la salta por completo en una linea en blanco (sin
  glifos que subrayar, ninguna franja flotando sola).
  9 tests nuevos (643 en total tras esta fase): 3 en `engine-text`
  (`baseline_offset` positivo y creciente con `font_size`, metricas de
  subrayado positivas y crecientes, el subrayado cae dentro del alto de
  linea reservado - los tres contra una fuente de sistema REAL, no
  simulada), 1 en `display_list.rs` (el token se reconoce solo/compuesto/
  sin distinguir mayusculas, `line-through` no lo activa), y 2 en
  `paint.rs` a nivel de PIXEL (activar `underline` pinta mas pixeles no
  transparentes que sin el, una linea en blanco no pinta nada) - los tres
  ultimos, igual que los de `engine-text`, corren contra una fuente de
  sistema real cuando hay una disponible, no un mock.
  **Verificado en vivo ademas**: captura PNG real de una pagina con un
  `<a>` (subrayado por la hoja de agente de usuario), un `<span>` con
  `text-decoration: underline` explicito, y un `<span>` normal - los dos
  primeros muestran la franja, el tercero no, exactamente lo esperado a
  ojo.

- **`SameSite` empieza a aplicarse de verdad, para `fetch()`/
  `XMLHttpRequest` de origen cruzado con `credentials: "include"`**
  (Fase 30): la doc de `cookie.rs` declaraba que necesitaba distinguir
  peticion "de primera parte" de "de tercera parte" (concepto ausente,
  sin `<iframe>`) para aplicarse en absoluto - cierto para NAVEGACION
  (sigue sin cerrarse, ver mas abajo), pero resulta que NO para
  `fetch()`/XHR: esos dos YA llevan `origin` (Fase 20) precisamente
  porque son los que activan CORS, y ese mismo dato basta para saber si
  una peticion es de origen cruzado sin necesitar nada nuevo. El hueco
  real, encontrado auditando el motor: `credentials: "include"` (Fase 27,
  la propia opcion que ACTIVA el envio de cookies a otro origen) se
  saltaba `SameSite` por completo - una cookie `Strict`/`Lax` (su valor
  por defecto real) viajaba igual a cualquier sitio que pidiera
  credenciales, exactamente la proteccion CSRF que `SameSite` existe para
  dar, anulada por la propia bandera que la activa.
  `CookieStore::matching_cookies` (el nucleo compartido de
  `header_for`/`header_for_js` desde la Fase 24) gana un tercer filtro,
  `only_same_site_none`, y una funcion nueva que lo activa:
  `header_for_cross_site` - solo cookies `SameSite=None` sobreviven.
  `NetworkEngine::fetch_once` la llama en el UNICO sitio donde una cookie
  podia cruzar de origen: `cross_origin && include_credentials` (antes
  esa combinacion llamaba a `header_for` normal, sin ningun filtro).
  El resto de combinaciones no cambia: mismo origen sigue mandando TODAS
  las cookies que apliquen (`SameSite` no restringe nada ahi), y origen
  cruzado SIN `include_credentials` sigue sin mandar ninguna (ya lo hacia
  antes, via CORS/Fase 20).
  3 tests nuevos en `cookie.rs` (`Strict`/`Lax`/por-defecto NUNCA cruzan
  pero SI viajan en una peticion normal del mismo origen, `SameSite=None`
  SI cruza, una mezcla de ambas se filtra cookie por cookie). 646 tests
  en total tras esta fase (643 + 3).
  **Verificado en vivo**: DOS servidores HTTP locales en puertos
  distintos (origenes distintos de verdad, no simulados) - el primero
  pone las tres cookies (`Strict`/`Lax`/`None`) via `Set-Cookie` real; el
  segundo sirve una pagina que hace `fetch(...,{credentials:'include'})`
  hacia el primero y refleja en `document.title` la cabecera `Cookie:`
  que el servidor RECIBIO de verdad. Resultado exacto: `none=1` unicamente
  - `strict=1`/`lax=1` nunca llegaron a salir del proceso.
  **Simplificaciones que siguen igual, declaradas**: SIN aplicar a
  NAVEGACION (`core::server` construye esas peticiones con `origin: None`
  - sin un origen de pagina que comparar, no hay forma de distinguir
  `Strict` de `Lax`, que es justo la diferencia que solo importa en
  navegacion top-level; cerrar esto exige que `core::server` recuerde el
  origen de la pagina ANTES de navegar, arquitectura aparte); "origen
  cruzado" se aproxima por ORIGEN exacto, no por SITIO/eTLD+1 real (sin
  PSL, mismo criterio que el resto del modulo) - mas estricto de lo
  necesario entre dos subdominios del mismo sitio, nunca menos.

- **`text-align` se aplica de verdad: `center`/`right`/`left`** (Fase 31):
  `text-align` ya era heredable (`INHERITABLE_PROPERTIES`) y llegaba
  intacto a `computed_style` desde antes de esta fase, pero NADA en
  `engine-layout` ni `engine-gfx` lo leia para desplazar nada - es una de
  las propiedades CSS mas comunes de la web real (practicamente todo
  boton/titulo/logo centrado la usa), y hasta ahora se ignoraba en
  silencio. El fix tiene DOS mitades, en dos crates distintos, para las
  dos formas en que este motor coloca texto:
  - **`engine-layout::tree::apply_text_align`** (el caso mas comun:
    varios hermanos inline-level - texto y/o `<b>`/`<i>`/`<span>` - que
    caben en una o varias lineas SIN que ninguno necesite envolverse por
    dentro): tras el posicionado normal de `flow_inline_run`, agrupa los
    nodos YA posicionados por linea (mismo `dimensions.y` - vienen del
    MISMO acumulador `cursor_y`, sin deriva de punto flotante entre
    hermanos), calcula el ancho real que ocupo cada linea, y desplaza el
    grupo entero (recursivamente via `shift_subtree_x`, para arrastrar
    tambien el texto ya posicionado DENTRO de un `<b>`/`<i>`) el hueco
    que sobra. Los nodos fuera de flujo (`position: absolute/fixed`) se
    saltan al agrupar - `place_inline_node` los deja sin posicionar
    (`Rect::default()`), y agruparlos por esa `y` compartida mezclaria
    lineas reales entre si.
  - **`engine-gfx::paint::paint_text`** (el otro caso: un SOLO nodo de
    texto tan largo que envuelve varias lineas DENTRO de su propia caja -
    `place_inline_node`, rama "ni siquiera cabe sola", `width ==
    inner_width` siempre): el desplazamiento que `apply_text_align`
    calcularia para esa caja seria cero (ya usa el ancho completo), asi
    que esa mitad no tiene nada que hacer ahi - se resuelve en el
    PINTADO, linea por linea (mismo bucle que ya mide cada linea para el
    subrayado de la Fase 29, reusando `measure_text`), con el ancho REAL
    de esa linea concreta - igual que un navegador real centra cada
    linea de un parrafo envuelto por separado, no el parrafo entero como
    bloque.
  Las dos mitades son complementarias, no se pisan: una caja con
  `width == inner_width` (candidata a la mitad de `paint.rs`) siempre
  produce desplazamiento CERO en `apply_text_align`, asi que no hace
  falta excluirla aparte en el lado de layout.
  `TextAlign`/`resolve_text_align` existen DUPLICADOS en `engine-layout`
  y `engine-gfx` (mismos tres valores, misma logica) - mismo criterio ya
  establecido para `resolve_font_weight_is_bold`/`resolve_font_style_is_
  italic`: dos crates que no deben depender entre si por unas pocas
  lineas. `justify` se PARSEA (no cae al caso "no reconocido") pero se
  pinta como `left` a proposito - fingir un justificado real (repartir
  espacio EXTRA entre palabras) sin implementarlo se veria peor que
  dejarlo a la izquierda. `start`/`end` (los valores logicos del spec
  moderno) se tratan como `left`/`right` - este motor no modela
  `direction: rtl`, asi que en LTR (el unico caso real) son identicos.
  9 tests nuevos (655 en total tras esta fase: 646 + 7 en `engine-layout`
  + 2 en `engine-gfx`) - en `engine-layout`: centrado/derecha de una linea
  corta, izquierda/sin-declarar/`justify` sin desplazar, un `<b>` anidado
  arrastra tambien a su texto interior, dos contenedores SEPARADOS con
  huecos sobrantes DISTINTOS no se contaminan entre si, y un hermano
  fuera de flujo intercalado no rompe el agrupado por linea; en
  `engine-gfx`: `resolve_text_align` reconoce los cinco valores, y una
  prueba a nivel de PIXEL confirma que los tres alineamientos producen un
  punto de arranque horizontal distinto y en el orden esperado
  (izquierda < centro < derecha) para el MISMO texto.
  **Verificado en vivo**: captura PNG real con tres `<div>` (centro/
  derecha/izquierda) mas un `<p>` de 300px con un parrafo largo - los
  tres primeros se ven exactamente alineados como se pidio, y el parrafo
  envuelve en 6 lineas, cada una centrada por SEPARADO con su propio
  hueco (visible a ojo: las lineas mas cortas dejan mas margen a los
  lados que las mas largas).
  **Limitacion declarada, encontrada auditando el propio fix**: un
  elemento inline (`<b>`/`<i>`/`<span>`) cuyo UNICO hijo de texto termina
  en una linea DISTINTA de donde el propio elemento empezo (el salto de
  linea ocurre DENTRO de la recursion al colocar ese hijo) deja el
  rectangulo delimitador del elemento con una `y` que no refleja su
  posicion real - la misma simplificacion de "fragmentacion inline" que
  este motor ya declaraba (un elemento partido en dos lineas es, en el
  spec real, DOS fragmentos rectangulares, no uno). `apply_text_align`
  hereda esa imprecision: puede agrupar ese elemento con la linea
  ANTERIOR en vez de la suya propia, dando un desplazamiento ligeramente
  distinto al ideal en ese caso concreto - no cerrado en esta fase.

- **`<noscript>` ya no se pinta como texto plano visible** (Fase 32):
  encontrado en vivo probando la app instalada contra un sitio real
  (Ignis Love) - el fragmento de respaldo de Google Tag Manager
  (`<noscript><iframe src="...">...</iframe></noscript>`, presente en una
  cantidad enorme de sitios reales) aparecia como texto crudo visible en
  la pantalla. Causa raiz exacta: `html5ever` parsea `<noscript>` como
  RAWTEXT cuando el scripting esta activado (`ParseOpts::default()` ya
  activa `scripting_enabled`, el valor por defecto real de la libreria) -
  EXACTAMENTE lo que exige el spec: en un navegador CON JavaScript (el
  caso de este motor), el interior de `<noscript>` nunca debe parsearse
  como markup real, asi que su DOM real es un SOLO nodo de texto con el
  HTML crudo tal cual. Un navegador real lo esconde con `noscript {
  display: none }` en su hoja de agente de usuario; `build_node` en
  `engine-layout::tree` en cambio ya tenia una lista de tags SIN
  representacion visual (`head`/`script`/`style`/`meta`/`link`/`title`) y
  `noscript` faltaba en ella - el mismo patron exacto que el bug de
  `strong`/`em` faltando en la lista de tags inline (Fase 2.4). Cerrado
  añadiendo `noscript` a esa lista, mismo mecanismo que ya usan `script`/
  `style` (excluir del layout por completo, no depender de una regla CSS
  que una hoja de autor pudiera sobreescribir).
  **Segundo hallazgo, mismo hilo**: la lista `elements` del protocolo
  NDJSON (la que consume la capa de IA/interaccion) tenia el MISMO
  problema por una via distinta - su campo `text` usaba `Node::
  text_content` (el mismo que implementa el `.textContent` REAL de JS,
  que camina el DOM entero SIN filtrar - correcto ahi, el spec real
  exige que `.textContent` incluya literalmente el codigo fuente de un
  `<script>`/`<style>` como texto) para describir el texto VISIBLE de
  `<body>`, asi que el marcado crudo se colaba igual, por un camino
  paralelo al de pintado. `collect_visible_text` (nueva, en
  `core::server`) camina el arbol de LAYOUT en vez del DOM crudo - ya
  filtrado por el fix de arriba, asi que no hace falta duplicar la lista
  de tags excluidos: una sola fuente de verdad sobre que cuenta como
  "visible".
  2 tests nuevos (657 en total tras esta fase): uno en `engine-layout`
  reproduciendo el fragmento real de GTM y confirmando que su marcado
  nunca aparece como texto en el arbol de layout, y uno en `core::server`
  confirmando que `collect_visible_text` excluye tanto `<noscript>` como
  `<script>`.
  **Verificado en vivo, de punta a punta**: instalador reinstalado sobre
  la instalacion existente y app abierta de verdad en el escritorio -
  ahi fue donde se encontro el bug originalmente (captura de pantalla del
  usuario). Confirmado el arreglo navegando el binario corregido contra
  la MISMA URL real (`https://www.ignislove.com/`): la captura PNG queda
  sin el texto del iframe (la pagina en si queda en blanco, porque el
  sitio depende de un framework JS que este motor no ejecuta completo -
  limitacion ya declarada, sin relacion con este fix) y la lista
  `elements` ya no contiene "iframe" en ningun campo `text`. De paso se
  investigo y se DESCARTO un tercer sintoma que parecia un bug
  (`Ignis Love | Tienda... Bienestar ntimo en Espaa...` con caracteres
  raros en el titulo): los bytes UTF-8 reales son correctos
  (`Íntimo`/`España`/`Envío`) - era solo la consola del script de prueba
  mostrando mal el Unicode, no el motor.

- **`value`/`placeholder` de un `BoxType::Replaced` ya se pintan de
  verdad, y ya no se pintan sus hijos DOM fantasma** (Fase 34): encontrado
  en vivo contra google.com real - "Google Search"/"I'm Feeling Lucky" se
  veian como recuadros grises vacios, sin ninguna etiqueta. Causa raiz: un
  `BoxType::Replaced` (`<input>`/`<select>`/`<textarea>`) solo pintaba
  fondo/borde/sombra (misma cascada que `Block`/`Inline`), un gap ya
  declarado desde la Fase 11 pero nunca cerrado. `resolve_replaced_text`
  (nueva, `engine-layout::tree`) resuelve QUE texto mostrar en
  `build_node`, donde `attributes`/`dom_node` estan disponibles de forma
  nativa - el `value` si lo hay; si no, el `placeholder` (marcado con
  `is_placeholder`, para pintarse en el gris real de un placeholder,
  `PLACEHOLDER_COLOR` en `engine-gfx`, en vez del `color` de la cascada);
  `type=submit/button/reset` usa la etiqueta por defecto real del spec
  ("Submit Query"/"Reset") si no tiene `value` propio, y se centra
  (`centered`) - asi es el aspecto nativo real de un boton, a diferencia
  de un campo de texto normal (alineado a la izquierda); `type=password`
  enmascara cada caracter con un punto, contado por caracter Unicode
  (`chars().count()`), no por byte, para que un valor con acentos no
  pinte de mas ni de menos puntos que letras tiene; `<textarea>` lee su
  CONTENIDO DOM (`Node::text_content`), no un atributo `value` (a
  diferencia de `input`, asi es el spec real); `checkbox`/`radio`/
  `hidden`/`file`/`image`/`range`/`color` no llevan texto (su aspecto
  nativo real no es una cadena dentro de la caja). `<select>` se deja SIN
  resolver a proposito - que opcion esta seleccionada exige inspeccionar
  sus `<option>` hijos, fuera del alcance de esta fase, simplificacion
  declarada.
  **Segundo hallazgo, mismo hilo**: los hijos DOM reales de un `Replaced`
  (las `<option>` de un `<select>`, el nodo de texto de un `<textarea>`)
  se CONSTRUYEN (`build_node` recursa en ellos igual que en cualquier
  otro elemento) pero nunca se POSICIONAN (`place_inline_node` los deja
  en `Rect::default()` a proposito - ver su doc-comment) - `engine-gfx::
  display_list` antes recursaba en ellos de todas formas para pintarlos,
  asi que terminaban dibujandose en `(0, 0)`, superpuestos con lo que
  hubiera en la esquina superior izquierda de la pagina ENTERA. Visible
  en un pantallazo compartido antes de esta fase, contra la pantalla real
  de consentimiento de google.com: texto ilegible solapado ahi arriba, y
  el boton "Sign in" con su etiqueta desbordando fuera de la pildora
  azul. Cerrado sacando `BoxType::Replaced` de la rama que llama a
  `build_clipped_children` - ya no recursa en sus hijos DOM en absoluto,
  solo pinta el `replaced_text` ya resuelto.
  9 tests nuevos (667 en total tras esta fase): 6 en `engine-layout`
  (value real, placeholder marcado como tal, submit sin value usa la
  etiqueta por defecto y centrado, submit CON value usa el suyo,
  password enmascarado por caracter Unicode, checkbox sin texto,
  textarea desde contenido DOM) y 3 en `engine-gfx` (un `Replaced` con
  texto resuelto emite `DisplayItem::Text`, un placeholder pinta con
  `PLACEHOLDER_COLOR` y no con el `color` de la cascada, un hijo DOM sin
  posicionar nunca se pinta).
  **Verificado en vivo**: motor recompilado en release y reinstalado
  sobre la instalacion existente, confirmado contra la MISMA URL real de
  google.com de las dos fases anteriores - captura PNG con "Google
  Search"/"I'm Feeling Lucky" ahora legibles y centrados dentro de sus
  botones, y la pantalla real de consentimiento sin el texto solapado en
  la esquina superior izquierda que se veia antes.

- **`<select>` muestra la opcion seleccionada de verdad** (Fase 35):
  cierra la simplificacion que la propia Fase 34 dejo declarada
  explicitamente ("`<select>` se deja SIN resolver a proposito"). Ya
  existia la infraestructura entera (`ReplacedText`, `resolve_replaced_text`)
  - solo faltaba la rama `"select"`. `resolve_select_text` (nueva,
  `engine-layout::tree`) usa `Node::find_all_by_tag(dom_node, "option")`
  (busca en TODO el subarbol, no solo hijos directos - `<option>` puede
  estar anidada dentro de `<optgroup>`) y aplica la semantica real del
  spec: la PRIMERA `<option>` con el atributo booleano `selected`
  presente gana (misma "presencia = true" que `checked`); sin ninguna,
  la PRIMERA opcion de la lista es la seleccionada por defecto - igual
  que cualquier navegador real con un `<select>` sencillo sin `multiple`.
  Un `<select>` sin ninguna `<option>` no resuelve texto (`None`), igual
  que un campo vacio.
  4 tests nuevos (671 en total tras esta fase): opcion `selected`
  explicita que NO es la primera, sin ninguna `selected` (cae a la
  primera), `<option>` anidada dentro de `<optgroup>`, y un `<select>`
  vacio sin ninguna `<option>`.
  **Verificado en vivo**: motor recompilado en release y reinstalado;
  pagina de prueba local con dos `<select>` (uno con `<option selected>`
  explicita en la posicion 2, otro sin ninguna `selected`) servida por
  HTTP real - la captura PNG confirma "Dos" en el primero (la opcion
  marcada, no la primera) y "Primero" en el segundo (el valor por
  defecto real sin ninguna marcada).

- **`display: inline-block/inline/block` ya reclasifica Block<->Inline**
  (Fase 36): investigando el ancho CERO de varios `<div>` reales en la
  barra superior de google.com (visible desde antes de esta fase, ver
  captura compartida) se encontro que `box_type` (`build_node`) se
  decidia SOLO por nombre de etiqueta, sin consultar `display` en
  absoluto salvo los casos ya especiales de `none`/`flex` - un `<div
  style="display:inline-block">` (patron real EXTREMADAMENTE comun -
  barras de navegacion, insignias, grupos de botones, la mayoria de
  layouts pre-flexbox) se quedaba `BoxType::Block`, excluido de
  cualquier racha inline (`is_inline_level`), apilandose solo con el
  ANCHO COMPLETO del contenedor en vez de fluir junto a sus hermanos.
  `override_box_type_from_display` (nueva, `engine-layout::tree`)
  reclasifica DESPUES de resolver la cascada (necesita `computed_style`
  ya resuelto, a diferencia del `box_type` inicial que solo mira
  `tag_name`): `inline-block`/`inline` fuerzan `Block -> Inline`,
  `block` fuerza `Inline -> Block`. Acotado a proposito a Block<->Inline
  - `Image`/`Replaced` quedan siempre forzados por su etiqueta
  (simplificacion declarada). El cambio no abre ningun camino de codigo
  NUEVO: `place_inline_node::BoxType::Inline` ya recursaba en hijos
  `BoxType::Block` sin problema desde antes (necesario para que
  `<button>`/`<a>` con contenido de bloque dentro no entraran en panico),
  asi que reclasificar solo dirige tags existentes hacia ramas ya
  probadas.
  3 tests nuevos (674 en total tras esta fase): dos `<div
  style="display:inline-block">` sentandose lado a lado en vez de
  apilarse, un `<span style="display:block">` llenando el contenedor
  como cualquier bloque, y una regresion directa confirmando que SIN
  ningun `display` de autor el comportamiento de siempre (etiqueta ->
  tipo de caja) no cambia.
  **Investigado pero NO resuelto, honestamente**: esta fase NO arreglo
  el ancho-cero real de google.com que la motivo - re-verificado en vivo
  tras el fix, los mismos `<div>` siguen en 0px. Inspeccionando el HTML/
  CSS real de google.com se encontro que esos elementos llevan
  `display:-webkit-box;display:-webkit-flex;display:flex` (el mismo
  patron de fallback de compatibilidad que esta fase SI arregla para el
  caso simple), pero ademas hay TRES reglas `.gb_2d{...}` distintas y
  contradictorias en su hoja de estilos real (`display:none`,
  `display:flex`, `display:table-cell`) cuya resolucion de cascada/
  `@media` exacta no se pudo confirmar por inspeccion manual de una hoja
  minificada de miles de caracteres - podria ser un bug de especificidad/
  `@media` en `engine-css`, o de medicion de contenido intrinseco dentro
  de un contenedor flex (`flow_flex_children` ya declara que no mide
  min-content/max-content real), sin diagnosticar aun cual. Queda como
  gap abierto, documentado en vez de dado por cerrado.
  **Verificado en vivo, con comparacion A/B real**: motor recompilado y
  reinstalado; ademas de la confirmacion negativa de arriba contra
  google.com, se comparo la MISMA pagina real (es.wikipedia.org/wiki/
  Python) renderizada con el binario de ANTES de esta fase (via `git
  stash`) y con el de DESPUES - las dos capturas PNG son identicas byte a
  byte, confirmando que el solapamiento de texto que Wikipedia ya
  mostraba (menu de navegacion, selectores CSS complejos aun no
  soportados - gap ya conocido de antes) es preexistente, no una
  regresion de esta fase.

Todo esto es exactamente lo que dice el plan de la Fase 1 — ni mas, ni menos.
Si un archivo de este repo afirma algo distinto (un log que diga "verificado"
o una cifra de rendimiento), es una mentira que hay que borrar, no una
funcionalidad que hay que documentar.

## Por que existia codigo que mentia

Antes de esta limpieza, la mayoria de modulos en `core`, partes de `net`,
`gfx` y `dom`/`js`/`css` no implementaban nada: eran structs con metodos que
solo hacian `tracing::info!("... 100% Passed ...")` o `... 0 vulnerabilities
detected ...` con numeros inventados (`1_850_000` tests WPT, `99.94%` de
aciertos, `200_000_000` usuarios activos, "JIT" que devolvia un string de
texto disfrazado de ensamblador). No eran retrasos ni advertencias — eran
afirmaciones falsas y especificas. Se borraron por completo en vez de
dejarlas como placeholders, porque un placeholder con esa forma (una
afirmacion de exito) es peor que no tener nada: engaña a quien lea los logs
o el codigo pensando que hay algo probado detras.

Regla para todo lo que se añada de aqui en adelante: **si una funcion no
esta implementada, no existe.** No hay metodos que devuelvan `Ok(42)` sin
tocar el argumento, ni structs con campos de estadisticas inventadas, ni
logs que digan "verified"/"operational" sobre algo que no se ha probado.

## Doctrina de dependencias: que se escribe a mano y que no

Ningun motor real — ni Servo ni Ladybird — escribe TLS, un parser HTML5
completo, o shaping de texto Unicode desde cero. Hacerlo no es una virtud;
es un multiplicador de años que no compra nada porque esos problemas ya
estan resueltos por crates maduros y probados en produccion.

**No se escribe a mano — se usa el crate del ecosistema:**

| Pieza | Crate | Por que no escribirla |
|---|---|---|
| TLS | `rustls` | Un fallo de implementacion criptografica es un agujero de seguridad |
| HTTP/1-2 | `hyper` | Protocolo enorme, con casos raros ya resueltos |
| DNS | `hickory-dns` | Idem |
| Parseo HTML5 | `html5ever` (integrado) | El algoritmo de recuperacion de errores del spec tiene ~800 estados |
| Tokenizado CSS | `cssparser` (integrado) | Bloques anidados, arroba-reglas, strings — casos borde ya resueltos |
| Selectores CSS | `selectors` (integrado) | Vienen de Servo, probados en produccion real |
| Shaping de texto | `rustybuzz` (integrado) | Ligaduras, kerning — decadas de trabajo acumulado. Arabe/devanagari sin probar todavia |
| Fuentes | `fontdb` (integrado), `ttf-parser` (via reexport de rustybuzz) | Formatos binarios con casos límite sin fin |
| Bidi / saltos de linea | `unicode-bidi`, `unicode-linebreak` | Es el algoritmo del estandar Unicode, no se mejora a mano |
| Rasterizado 2D | `tiny-skia` (ya en uso) | Anti-aliasing correcto es matematica no trivial |
| Compositacion GPU | `wgpu` (dependencia integrada, `gfx/src/gpu_pipeline.rs::WebGpuPipeline` consulta un adaptador real - pero nada del pipeline real la llama todavia; el rasterizado actual es 100% CPU via tiny-skia) | Abstraccion real sobre Vulkan/Metal/DX12 |
| Imagenes de trama | `image` (integrado - `engine-image`, Fase 3.1: PNG/JPEG/GIF/BMP/WebP/TIFF/ICO) | Cada formato tiene su propio infierno de casos borde |
| Imagenes vectoriales | `resvg` (pendiente de integrar) | SVG es su propio motor de render, no un simple decode-a-RGBA |
| Layout flex (grid pendiente) | `taffy` (integrado - `flow_flex_children`/`measure_flex_item` en `engine-layout::tree`, Fase 3.2: flexbox real; feature `grid` desactivada a proposito, todavia sin puente propio) | El algoritmo de Flexible Box Layout completo (ejes principal/cruzado, `flex-grow`/`shrink`/`basis`, wrap, min/max-content, alineacion) es del mismo orden de complejidad que el arbol de HTML5 - "resuelto ya", no una virtud reescribirlo |
| Ventanas + eventos | `winit` (ya en uso) | Cada sistema operativo tiene el suyo propio |
| Presentacion a pantalla | `softbuffer` (ya en uso) | Blit de pixeles a superficie de ventana, multiplataforma |

**Si se escribe a mano — es donde vive el motor de verdad:**

- El **DOM** y su semantica viva (arbol, eventos, en el futuro colecciones/rangos/observers)
- La **cascada CSS**: fusion de declaraciones por especificidad ya real (via `selectors`), con el atributo `style="..."` del elemento ganando siempre al final (como en el spec real), y herencia real de 20 propiedades (`color`/`font-size` con unidades relativas `em`/`%`, `font-weight`/`font-style` ya pintados como negrita/cursiva real, mas `font-family`/`line-height`/`text-align`/... - ver Fases 2.4/2.5 en "Estado real" - todavia sin efecto visual); `rem` (relativo a `<html>`, no al padre inmediato) sigue sin resolverse
- El **layout de bloque e inline** (`BoxType::Block`/`Inline`/`Text`/`Image`, contextos de formato de bloque/inline reales - `flow_block_children`/`flow_inline_run` en `tree.rs`) y el **puente hacia `taffy`** para flex/grid (ver la fila de la tabla de arriba): el ALGORITMO de Flexible Box Layout/Grid en si NO se reescribe a mano (razon en la tabla), pero SI se escribe a mano todo el puente real - mapear `computed_style` a `taffy::Style`, medir contenido de texto/imagenes via el callback `MeasureFunction` de `taffy` reusando `flow_block_children`/`measure_text` YA existentes, y volcar el resultado de vuelta en `LayoutBox::dimensions` - eso sigue siendo "como se comporta la pagina", solo que el ALGORITMO interno de reparto de espacio en el eje principal/cruzado no se reinventa
- El **layout de tablas** (`flow_table_children` en `tree.rs`, Fase 3.4) SI es codigo propio de punta a punta, sin delegar a ningun crate - a diferencia de flex/grid, repartir filas/columnas en un algoritmo de columnas de ancho igual no es del mismo orden de complejidad que el arbol de HTML5; no hay una virtud real en traer una dependencia para esto
- El **arbol de pintado** (`display_list.rs`) y su recorrido a la superficie
- El **bucle de eventos** del navegador y el hit-testing
- El **puente IA↔DOM** — la diferenciacion real del producto (crate `ai`, pendiente de crear cuando haya un DOM+layout real que exponer; no antes)
- El **shell** del navegador (pestañas, barra de direcciones, historial)

Regla practica: si el codigo define *como se comporta una pagina web segun
el spec*, es del motor. Si define *como se decodifica/transporta un
formato ya estandarizado*, es una dependencia.

## Sobre JavaScript

Tres opciones reales, cada una con una compensacion distinta:

- **Boa** (`boa_engine`, ya integrado en `engine-js`): Rust puro, facil de
  empotrar, pero sin JIT y con cobertura de spec incompleta. No movera con
  soltura una app React real.
- **V8** (crate `v8` de Deno): el motor de Chrome. Rapido y completo.
  Pesado de compilar, añade una dependencia de C++ al build.
- **SpiderMonkey** (`mozjs`): el motor de Servo/Firefox. Compensaciones
  similares a V8.

Decision para este proyecto: **Boa en las fases donde JS no es el foco
(1-3)**, con **migracion planeada a V8 en la Fase 4**. El binding DOM en
`engine-js::dom_bindings` debe quedar detras de un trait propio para que ese
cambio, cuando llegue, no sea una reescritura del resto del motor.

## Arquitectura de crates

```
engine/
├── crates/
│   ├── net/         HTTP/HTTPS real (hyper+rustls); `CookieStore` (RFC 6265 real, con `document.cookie` conectado a JS desde la Fase 24 - ver su entrada), `WebStorage` (origen-aislado, conectado a `localStorage`/`sessionStorage` desde la Fase 15) y `CorsPolicy`/`ContentSecurityPolicy` (aplicados de verdad en `NetworkEngine::fetch` desde las Fases 20/21) - ninguno de los tres es ya un stub, ver doc-comments en cookie.rs/storage.rs/cors.rs/csp.rs para las simplificaciones que SI siguen declaradas
│   ├── dom/         Nodos, arbol, adaptador TreeSink para html5ever (los eventos DOM viven en js/dom_bindings.rs - ver ese crate - no aqui: guardar listeners exige poder guardar un JsObject, y este crate no depende de Boa a proposito)
│   ├── css/         Parseo real (cssparser), matching de selectores real (selectors: combinadores, compuestos, atributos), resolucion de cascada real (`cascade::resolve_style` - matching+especificidad+atributo style inline; se traslado aqui desde `layout` para que `js` tambien pueda reusarla, ver "Metrica de progreso")
│   ├── layout/      Cajas con layout de bloque, inline, flex Y tabla real (`BoxType::Block`/`Inline`/`Text`/`Image`; `display: flex` via `taffy` (`flow_flex_children`), `display: table` a mano (`flow_table_children`, Fase 3.4)) + cascada CSS aplicada (via `engine_css::resolve_style`), texto medido con metricas reales de fuente (negrita/cursiva incluidas, via `FontSet`); box model completo desde CSS - `padding`/`border`/`margin` reales (`LayoutBox::box_dimensions`, `Dimensions::padding_box()`/`border_box()`/`margin_box()` en box_model.rs); `position: relative/absolute/fixed` + `z-index` reales (Fase 3.3, `resolve_positioned_boxes`); floats/grid siguen sin existir
│   ├── image/       Decodificacion real de imagenes de trama (`image`: PNG/JPEG/GIF/BMP/WebP/TIFF/ICO) a RGBA8 - crate propio y minimo porque tanto `layout` (dimensiones) como `gfx` (pixeles) lo necesitan sin que `layout` dependa de `gfx`
│   ├── text/         Shaping real (rustybuzz), medida sin construir contornos (measure_text), carga de fuentes del sistema en 4 variantes peso/estilo (`FontSet`, fontdb), contornos de glifo -> tiny-skia
│   ├── gfx/         Display list (`DisplayItem::Image`/`Shadow`/`PushClip`/`PopClip` incluidos) + pintado real COMPARTIDO en `paint.rs` (border-radius, box-shadow, overflow:hidden via mascara de recorte - Fase 3.5) entre el rasterizado headless (`raster.rs`) y la ventana nativa (`window.rs`, winit+tiny-skia+softbuffer); texto con glifos reales, adaptador GPU real (wgpu), scroll real de la rueda del raton (offset aplicado en pintado + hit-testing, sin relayout - ver "Scroll real de la rueda del raton" mas abajo)
│   ├── js/          Runtime Boa enganchado al pipeline (via core/scripting.rs), bindings DOM con mutacion real (getElementById/querySelector(All)/setAttribute/textContent/createElement/appendChild/removeChild/insertBefore/replaceChild/classList/style/parentElement/children/firstElementChild.../documentElement/body), eventos reales (addEventListener/removeEventListener/dispatchEvent/Event con preventDefault/stopPropagation/target/bubbling/fase de captura real, `.key` real en eventos de teclado - Fase 4.1) - el clic del raton SI esta conectado a input real del SO en la ventana winit (ver "Clic real del SO cableado de punta a punta"), el teclado SI en el camino NDJSON real (`core::server`, Fase 4.1) pero no en la ventana winit; microtasks reales (queueMicrotask), `fetch()` real respaldado por `engine-net` (Fase 4.3, primera dependencia de red de este crate - bloqueante bajo el capó, ver su entrada mas arriba), arnes minimo tipo testharness.js (test_harness.rs, ya conectado a wpt_runner - ver core/)
│   └── core/        Orquestacion: lib.rs (pipeline/scripting/platform, compartido) + main.rs (red -> pipeline.rs -> ventana) + bin/wpt_runner.rs (corredor real de fixtures estilo WPT, sin ventana)
```

Crates planeados, no creados todavia (no crear hasta que haya algo real que
poner dentro — un crate vacio con nombre ambicioso es exactamente el
problema que se acaba de limpiar):

- **`ai`**: extraccion semantica del DOM+layout y API de acciones para el
  agente. Se crea cuando haya un DOM/layout real que exponer (Fase 3-4), no
  antes — la version anterior de este bridge contaba hijos directos de un
  nodo y lo llamaba "IA".
- **`shell`**: ventana de aplicacion, pestañas, barra de direcciones,
  historial — UI del navegador en si, no del motor de renderizado.

(`text` estaba en esta lista y ya se creo — shaping real con rustybuzz, ver
tabla de dependencias arriba.)

## Metrica de progreso

"¿Funciona?" no es una pregunta con respuesta binaria para un motor de
navegador. La metrica honesta es un numero: cuantos tests de
[Web Platform Tests](https://github.com/web-platform-tests/wpt) pasan, por
categoria. Todavia no hay un ejecutor de WPT real integrado (el anterior
`wpt_runner.rs` afirmaba "1000/1000, 100%" sin ejecutar nada; se borro).

Tres piezas reales dadas hacia esa metrica, conectadas entre si, pero
TODAVIA no un ejecutor de la corpus real de WPT (ver la distincion exacta
mas abajo - importa especialmente aqui, dado el `wpt_runner.rs` anterior
mencionado arriba):

1. `core/src/pipeline.rs::build_page` extrae el pipeline completo (parseo ->
   JS inline -> cascada -> layout) de `main()` a una funcion que no abre
   ninguna ventana ni bloquea - hasta ahora, la UNICA forma de correr el
   motor de punta a punta era abrir una ventana nativa y esperar a que un
   humano la cerrara, lo cual hace imposible cualquier automatizacion. Es
   solo la plomeria minima que cualquier corredor necesitaria para invocar
   el motor sin interfaz grafica.
2. `js/src/test_harness.rs::TestHarness` implementa un subconjunto SINCRONO
   minimo de `testharness.js` (el arnes real de WPT): `test(fn, name)`,
   `assert_equals`/`assert_true`/`assert_false`, con los resultados
   acumulados en un `Vec` inspeccionable desde Rust despues de evaluar el
   script — no solo logueados. `assert_true`/`assert_false` exigen
   identidad estricta con el booleano (`1` no pasa `assert_true`, aunque
   sea "truthy" en JS) igual que el arnes real, no una aproximacion mas
   floja — probado explicitamente. Se registra por separado de
   `DomBindings` a proposito: ninguna pagina web real tiene `test`/
   `assert_equals` como globales, son exclusivos del arnes de pruebas.
   `async_test`/`promise_test`/`assert_throws_js`/`assert_array_equals`/
   el resumen final de un runner real (`add_completion_callback`) NO estan
   implementados — un test que los use fallara con un error real (no estan
   registrados como globales), no en silencio.
3. **1 y 2 ya estan conectados**: `pipeline::build_page_with_harness` (usa
   `scripting::execute_inline_scripts_with_harness`, que registra
   `TestHarness::register` EN EL MISMO `Context` que `DomBindings` -
   `document.*` y `test`/`assert_*` conviven, asi que un test puede
   manipular el DOM real y comprobarlo) y `crates/core/src/bin/wpt_runner.rs`
   - un binario real (`cargo run -p engine-core --bin wpt_runner --
   <archivo.html o directorio>`) que carga HTML de disco, lo corre por el
   pipeline con el arnes activo, e imprime OK/FAIL por cada `test(...)` mas
   un resumen (`N pasaron, M fallaron`), con exit code 1 si algo fallo -
   verificado en vivo, no solo con tests: corre limpio sobre los fixtures
   reales (ver abajo) y reporta FAIL con el mensaje correcto sobre un test
   deliberadamente roto usado solo para probarlo.

**Lo que esto NO es, a proposito**: `wpt_runner` no descarga ni vendoriza
la corpus real de [Web Platform Tests](https://github.com/web-platform-tests/wpt).
Los fixtures que corre hoy (`engine/tests/wpt-style/*.html`, 3 archivos, 21
`test(...)` en total tras la Fase 5.2) estan escritos A MANO en el mismo
estilo (`test`/`assert_equals`/`assert_true`) para ejercitar capacidad real
que el motor ya tiene — mutacion/navegacion del DOM, `classList`/`style`, y
(desde la Fase 5.2) eventos reales (`addEventListener`/bubbling/captura/
`preventDefault`/`stopPropagation`) y `queueMicrotask` — no son la suite
oficial. Vendorizar la corpus real sigue sin empezar, y no serviria de mucho
todavia: la inmensa mayoria de esos archivos fallarian en cascada por falta
del CSSOM de hojas de estilo (`document.styleSheets`, `CSSStyleSheet`,
`insertRule`...). La red expuesta a JS ya no es el cuello de botella:
`fetch` es real desde la Fase 4.3 y `XMLHttpRequest` desde la Fase 9,
aunque este ultimo sea sincrono siempre (declarado en su entrada), lo que
haria fallar cualquier test que dependa del ORDEN asincrono en si. El layout SI es
inspeccionable desde JS desde la Fase 8 (`getComputedStyle`,
`getBoundingClientRect`/`getClientRects`), con las limitaciones declaradas
en su entrada (sin reflow sincrono, valores especificados en vez de usados,
y solo las propiedades que la cascada resolvio de verdad) - asi que un test
que lea posiciones/tamaños desde JS ya puede pasar, pero uno que espere el
valor inicial de una propiedad que nadie declaro, no... antes hay que decidir que categorias de WPT
tienen siquiera sentido de intentar dado lo que el motor soporta hoy (mas
que antes gracias a las mutaciones DOM reales y los eventos con
bubbling/captura - pero sigue siendo casi ninguna categoria completa).

## Integracion con el producto

El producto es un navegador nativo y NO incluye Chromium, Playwright ni otro
motor de navegador externo. El renderer real debe venir del motor Rust de
`engine/`; no existe un fallback de navegador externo.

El backend de la interfaz funciona como cliente del puente IPC Rust. Si el
binario no está presente, devuelve un estado vacío y notifica "motor de
navegador no disponible"; no sustituye el motor por un navegador externo.

La integracion se diseña detras de una
interfaz comun (`BrowserBackend` — trait a definir cuando el motor tenga
suficiente superficie para implementarlo: navegacion, arbol semantico para
la IA, click). La interfaz debera quedar conectada unicamente al backend Rust.
No hay un flag para activar Chromium ni una comparacion contra Playwright.
El puente ya cubre estado de página, captura PNG, hit-testing, navegación,
clic, scroll, resize, escritura en `input`/`textarea` y eventos básicos de
teclado. Submit de formularios, selección de texto y metadatos de tecla
todavía no están implementados. No se deben añadir dependencias de
Playwright ni copiar carpetas `ms-playwright` al instalador.

### Protocolo IPC v1

El primer puente se implementa como un proceso Rust que habla **NDJSON por
stdin/stdout** (`crates/core/src/bin/engine_server.rs`). Cada línea recibida
es una petición y cada línea emitida es una respuesta JSON. Los logs de
diagnóstico no deben escribirse en stdout, porque romperían el protocolo.

La versión actual incluye `navigate`, `ping`, `resize`, `get_state`, `click`,
`scroll`, `type_text`, `press_key` y `shutdown`. `navigate` ejecuta
`pipeline::build_page_keeping_runtime`; `get_state` devuelve la URL, el
título, elementos con rectángulos y una captura PNG Base64 generada por
`engine-gfx`. Las acciones aún no conectadas, como submit de formularios,
devuelven un error explícito.
Cuando el servidor está vivo, `renderer_status` es `ready`; si el proceso no
existe, Python mantiene el mensaje de motor no disponible.
