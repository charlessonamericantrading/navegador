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
  `text-decoration` quedan en el `computed_style` (cascada correcta) pero
  `engine-gfx` todavia no los lee al pintar (negrita/cursiva/subrayado no
  se ven todavia - Fase 2.4); sin viñetas ni sangria de listas.
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
  NO implementado: `text-align`, `vertical-align`, alineacion por
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
  pero nada pinta todavia); pesos intermedios reales (100/200/300...) mas
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
  `border-spacing`, `caption-side`, `empty-cells` - sin layout de tablas,
  Fase 3.4 pendiente) y las de paginacion impresa (`orphans`/`widows` - un
  renderer de pantalla sin paginacion no tiene pagina que romper). Mismo
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
  mas abajo); el teclado sigue sin ninguna fuente de eventos — sigue
  siendo Fase 3.
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
  fuente que lo dispare); el teclado sigue sin ninguna fuente real en
  absoluto.
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
  `border_strip_rects` en `window.rs`, probado) en vez de un stroke con
  la API de trazado de tiny-skia — mismo resultado visual para un border
  UNIFORME (unica forma que se resuelve hoy), reusando `fill_rect`, ya
  probado, en vez de investigar API nueva sin necesidad. 15 tests nuevos
  entre las tres capas (layout, parseo de color en gfx, geometria de
  pintado).
- **`resolve_style` trasladado de `layout` a `css`** (paso 1, preparatorio,
  hacia `getComputedStyle` - todavia NO implementado, ver mas abajo por
  que). La funcion de cascada real (matching + especificidad + atributo
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
│   ├── net/         HTTP/HTTPS real (hyper+rustls); CookieStore/WebStorage/CorsPolicy son stubs honestos (mapas en memoria / permite-todo) SIN conectar a `NetworkEngine::fetch` ni a bindings JS todavia - ver doc-comments en cookie.rs/storage.rs/cors.rs
│   ├── dom/         Nodos, arbol, adaptador TreeSink para html5ever (los eventos DOM viven en js/dom_bindings.rs - ver ese crate - no aqui: guardar listeners exige poder guardar un JsObject, y este crate no depende de Boa a proposito)
│   ├── css/         Parseo real (cssparser), matching de selectores real (selectors: combinadores, compuestos, atributos), resolucion de cascada real (`cascade::resolve_style` - matching+especificidad+atributo style inline; se traslado aqui desde `layout` para que `js` tambien pueda reusarla, ver "Metrica de progreso")
│   ├── layout/      Cajas con layout de bloque, inline Y flex real (`BoxType::Block`/`Inline`/`Text`/`Image`; `display: flex` via `taffy`, ver `flow_flex_children`) + cascada CSS aplicada (via `engine_css::resolve_style`), texto medido con metricas reales de fuente (negrita/cursiva incluidas, via `FontSet`); box model completo desde CSS - `padding`/`border`/`margin` reales (`LayoutBox::box_dimensions`, `Dimensions::padding_box()`/`border_box()`/`margin_box()` en box_model.rs); floats/grid/position siguen sin existir
│   ├── image/       Decodificacion real de imagenes de trama (`image`: PNG/JPEG/GIF/BMP/WebP/TIFF/ICO) a RGBA8 - crate propio y minimo porque tanto `layout` (dimensiones) como `gfx` (pixeles) lo necesitan sin que `layout` dependa de `gfx`
│   ├── text/         Shaping real (rustybuzz), medida sin construir contornos (measure_text), carga de fuentes del sistema en 4 variantes peso/estilo (`FontSet`, fontdb), contornos de glifo -> tiny-skia
│   ├── gfx/         Display list (incluye `DisplayItem::Image`, pintado real via `image_paint.rs`), ventana real (winit+tiny-skia+softbuffer), texto con glifos reales, adaptador GPU real (wgpu), scroll real de la rueda del raton (offset aplicado en pintado + hit-testing, sin relayout - ver "Scroll real de la rueda del raton" mas abajo)
│   ├── js/          Runtime Boa enganchado al pipeline (via core/scripting.rs), bindings DOM con mutacion real (getElementById/querySelector(All)/setAttribute/textContent/createElement/appendChild/removeChild/insertBefore/replaceChild/classList/style/parentElement/children/firstElementChild.../documentElement/body), eventos reales (addEventListener/removeEventListener/dispatchEvent/Event con preventDefault/stopPropagation/target/bubbling/fase de captura real) - el clic del raton SI esta conectado a input real del SO (ver "Clic real del SO cableado de punta a punta"), scroll/teclado todavia no; microtasks reales (queueMicrotask), arnes minimo tipo testharness.js (test_harness.rs, ya conectado a wpt_runner - ver core/)
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
Los fixtures que corre hoy (`engine/tests/wpt-style/*.html`, 2 archivos, 11
`test(...)` en total) estan escritos A MANO en el mismo estilo
(`test`/`assert_equals`/`assert_true`) para ejercitar capacidad real que el
motor ya tiene — mutacion/navegacion del DOM y `classList`/`style` — no son
la suite oficial. Vendorizar la corpus real sigue sin empezar, y no serviria
de mucho todavia: la inmensa mayoria de esos archivos fallarian en cascada
por falta de `fetch`/`XMLHttpRequest`, eventos, la mayor parte de CSSOM...
antes hay que decidir que categorias de WPT tienen siquiera sentido de
intentar dado lo que el motor soporta hoy (mas que antes gracias a las
mutaciones DOM reales - `setAttribute`/`textContent`/`createElement`/
`appendChild`/`insertBefore`/`replaceChild`/`classList`/`style` - pero
sigue siendo casi ninguna categoria completa).

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
