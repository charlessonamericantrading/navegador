use crate::box_model::Dimensions;
use engine_dom::Node;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// Punto-en-rectangulo simple para hit-testing (`LayoutBox::hit_test`) -
    /// los bordes cuentan como dentro (`>=`/`<=`), asi que un punto justo en
    /// el borde derecho/inferior de una caja SI cuenta como un click sobre
    /// ella, no sobre lo que venga despues.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone)]
pub enum BoxType {
    Block,
    Inline,
    Text(String),
    /// `<img>` - guarda el `src` CRUDO (sin resolver contra la URL de la
    /// pagina), igual que `external_scripts`/`find_external_script_srcs`
    /// ya hacen para `<script src>` (ver `core/pipeline.rs`) - es la clave
    /// con la que se busca la imagen ya decodificada en el mapa que
    /// `LayoutTreeBuilder::build` recibe (Fase 3.1, ver ARCHITECTURE.md).
    /// Elemento inline-level por defecto (como en el spec real, "inline
    /// replaced element" - ver `is_inline_level` en tree.rs), a diferencia
    /// de todo lo demas que no sea `span`/`a`/`b`/`i`/`strong`/`em`, que
    /// cae a `Block`.
    Image(String),
    /// `<input>`/`<select>`/`<textarea>` (Fase 11: controles de
    /// formulario) - un "elemento reemplazado" generico como `Image`
    /// (mismo termino del spec real: caja atomica, sin contenido visible
    /// propio que el motor sepa componer a partir de sus hijos DOM), pero
    /// SIN ningun bitmap que pintar - el `value`/`placeholder` (Fase 34,
    /// ver `ReplacedText` abajo) es TEXTO resuelto aparte, no un bitmap.
    /// Su tamaño viene SIEMPRE de CSS (`width`/`height` resueltos, con
    /// valores por defecto reales de `user_agent_stylesheet.rs` para cada
    /// tipo de control - nunca un "tamaño natural" descubierto como el de
    /// una imagen decodificada). Comparte el mismo tratamiento
    /// inline-level atomico que `Image` (`is_inline_level`,
    /// `place_inline_node`) pero pinta fondo/borde con su propia rama en
    /// `engine-gfx::display_list` (no la de `Block`/`Inline` - necesita
    /// ademas cortar la recursion en sus hijos DOM reales, que nunca se
    /// posicionan - ver `ReplacedText`), no como una imagen. `<button>` NO
    /// usa esta variante - a diferencia de `input`/`select`/`textarea`,
    /// tiene contenido DOM real (su etiqueta es texto hijo, no un
    /// atributo) y se beneficia de encogerse a su contenido siendo
    /// `BoxType::Inline` en vez de un tamaño fijo - ver `build_node` en
    /// tree.rs.
    Replaced,
}

/// Fase 34: texto a pintar DENTRO de una caja `BoxType::Replaced` en vez de
/// su contenido DOM real, que la propia caja nunca expone como cajas hijas
/// posicionadas (ver el doc-comment de `BoxType::Replaced` arriba y de
/// `place_inline_node` en tree.rs - los hijos DOM de un `<select>`/
/// `<textarea>` se construyen pero se quedan en `Rect::default()`, nunca se
/// pintan en su sitio). Resuelto en `LayoutTreeBuilder::build_node`, donde
/// `attributes`/`dom_node` estan disponibles de forma nativa, para que
/// `engine-gfx` (que no conoce el DOM) solo tenga que pintar una cadena ya
/// decidida, igual que ya hace con `BoxType::Image(src)` y una imagen ya
/// resuelta.
#[derive(Debug, Clone)]
pub struct ReplacedText {
    /// El `value` real del control, o su `placeholder` si no hay `value`
    /// (`is_placeholder` distingue cual de los dos es, para pintarlo con el
    /// gris tenue real de un placeholder en vez del `color` normal de la
    /// cascada).
    pub text: String,
    pub is_placeholder: bool,
    /// `<input type="submit/button/reset">` centra su etiqueta (asi es el
    /// aspecto nativo real de un boton); un campo de texto normal la alinea
    /// a la izquierda.
    pub centered: bool,
}

/// Espacio disponible que el algoritmo flex ofrece en un eje al MEDIR un
/// item, en terminos propios de este crate (no los de `taffy`): mantener
/// `layout_box.rs` libre de tipos de `taffy` es lo que permite que la
/// cache de medidas viva en la caja sin arrastrar esa dependencia hasta
/// aqui. `tree.rs` convierte desde/hacia los tipos de taffy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AvailableAxis {
    Definite(f32),
    MinContent,
    MaxContent,
}

/// Clave de `LayoutBox::measure_cache`: identifica QUE medida es. Dos
/// medidas con la misma clave sobre la misma caja tienen por fuerza el
/// mismo resultado, porque el subarbol no cambia durante una maquetacion.
///
/// Es un enum y no una sola estructura de campos porque hay tres medidas
/// distintas que no deben mezclarse jamas: la que pide el algoritmo flex
/// (con sus propias entradas) y las dos anchuras intrinsecas, que son
/// preguntas independientes sobre la misma caja.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasureKey {
    /// Una medida pedida por el algoritmo flex, con las entradas de las que
    /// depende su resultado.
    Flex {
        known_width: Option<f32>,
        known_height: Option<f32>,
        available_width: AvailableAxis,
        available_height: AvailableAxis,
    },
    /// Anchura MINIMA a la que la caja puede reducirse sin desbordar su
    /// contenido: para un texto, el ancho de su palabra mas larga.
    MinContentWidth,
    /// Anchura que la caja ocuparia sin ningun corte de linea: para un
    /// texto, la frase entera en una sola linea.
    MaxContentWidth,
}

#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub box_type: BoxType,
    pub dimensions: Rect,
    /// Geometria de padding/border/margin (`Dimensions::padding_box/
    /// border_box/margin_box` en box_model.rs), POBLADA de verdad desde
    /// las Fases 2.2-3.3: `flow_block_children` resuelve padding, border y
    /// margin desde la cascada y escribe aqui el area de contenido, de
    /// modo que `padding_box()`/`border_box()` reconstruyen exactamente
    /// `dimensions` (ver el comentario que lo explica en `tree.rs`). Lo
    /// lee el propio layout para calcular el bloque contenedor de un
    /// `position: relative/absolute/fixed`.
    ///
    /// Relacion con `dimensions`, que importa para no confundirlas:
    /// `dimensions` es la CAJA DE BORDE (lo que se pinta y sobre lo que se
    /// hace hit-test, y lo que devuelve `getBoundingClientRect`);
    /// `box_dimensions.content` es el area interior, sin padding ni
    /// border.
    pub box_dimensions: Dimensions,
    pub children: Vec<LayoutBox>,
    /// Declaraciones CSS resueltas para esta caja (cascada ya aplicada por
    /// especificidad, ver LayoutTreeBuilder::build_node). Las cajas de texto
    /// no tienen reglas propias (un selector nunca apunta a un nodo de
    /// texto) - lo que llevan aqui son las propiedades heredables
    /// (`INHERITABLE_PROPERTIES` en tree.rs) resueltas del ancestro mas
    /// cercano que las definiera.
    pub computed_style: HashMap<String, String>,
    /// El `Node` real del DOM que produjo esta caja - `None` para cajas de
    /// texto (un click real resuelve al ELEMENTO contenedor, nunca a un
    /// nodo de texto - igual que `event.target` en un navegador real jamas
    /// es un `Text`) y para la caja raiz sintetica que envuelve el
    /// viewport entero (ver `LayoutTreeBuilder::build`, no corresponde a
    /// ningun elemento). Poblado en `LayoutTreeBuilder::build_node`, es lo
    /// que permite `hit_test` devolver un nodo real en vez de solo unas
    /// coordenadas.
    pub dom_node: Option<Arc<RwLock<Node>>>,
    /// `None` para todo lo que no sea `BoxType::Replaced`, y tambien para un
    /// `BoxType::Replaced` sin `value` ni `placeholder` que mostrar
    /// (checkbox/radio/hidden/`<select>` - ver `resolve_replaced_text` en
    /// tree.rs).
    pub replaced_text: Option<ReplacedText>,
    /// Medidas ya calculadas para esta caja, por clave de entrada.
    ///
    /// Existe por una razon medida, no por prudencia: `taffy` llama a la
    /// funcion de medida de un item flex varias veces (min-content,
    /// max-content, tamaño definitivo), y medir un item significa MAQUETAR
    /// SU SUBARBOL ENTERO. Con contenedores flex anidados - lo normal en
    /// cualquier web moderna - ese coste se multiplica por nivel de
    /// anidamiento en vez de sumarse: en la Wikipedia real eran 505.765
    /// remaquetados para 35.000 nodos, 84 s de reloj. Recordar el
    /// resultado por clave lo vuelve lineal, que es lo que hace cualquier
    /// motor real.
    ///
    /// Un `Vec` con busqueda lineal, no un `HashMap`: en la practica son
    /// dos o tres entradas por caja, donde recorrerlas gana a construir un
    /// hash, y ademas `f32` no implementa `Hash`.
    ///
    /// Solo es valida mientras el subarbol no cambie. No hace falta
    /// invalidarla nunca porque `LayoutTreeBuilder::build` reconstruye el
    /// arbol entero desde el DOM en cada maquetacion (tambien al
    /// redimensionar), asi que cada caja nace con su cache vacia.
    pub(crate) measure_cache: Vec<(MeasureKey, (f32, f32))>,
    /// Donde habria caido esta caja si NO estuviera fuera de flujo - la
    /// "posicion estatica" del spec, en coordenadas absolutas.
    ///
    /// Solo se rellena para cajas `position: absolute`/`fixed`, y solo
    /// existe porque el spec la exige: un absoluto con `left`/`top` en
    /// `auto` NO va a la esquina de su bloque contenedor, va exactamente
    /// donde el flujo normal lo habria dejado. Mandarlos todos a la esquina
    /// (lo que hacia este motor) amontona en (0,0) cada menu desplegable y
    /// cada tooltip de la pagina, unos encima de otros - en la Wikipedia
    /// real eso era la franja de texto ilegible pegada al borde superior.
    ///
    /// `None` para todo lo demas: una caja en flujo ya tiene su posicion en
    /// `dimensions`, y no hay nada que recordar.
    pub(crate) static_position: Option<(f32, f32)>,
    /// Cuantas columnas ocupa esta celda (`colspan` de `<td>`/`<th>`), 1 si
    /// no lo declara.
    ///
    /// Vive aqui y no en `computed_style` porque `colspan` es un atributo
    /// PRESENTACIONAL de HTML, no una propiedad CSS: no llega por la
    /// cascada, se lee del DOM al construir la caja (igual que el `src` de
    /// una imagen). Ignorarlo no solo ensancha mal una celda - corre TODAS
    /// las celdas siguientes de esa fila una columna a la izquierda, que es
    /// lo que descuadraba las filas de subtitulo de Hacker News.
    pub(crate) colspan: u32,
    /// Cuantas FILAS ocupa esta celda (`rowspan`), 1 si no lo declara.
    /// Mismo motivo que `colspan` para vivir aqui y no en la cascada: es un
    /// atributo presentacional de HTML, no una propiedad CSS.
    pub(crate) rowspan: u32,
    /// Ancho del BLOQUE CONTENEDOR de esta caja, que es la referencia contra
    /// la que el spec resuelve cualquier porcentaje suyo - tambien el de
    /// `padding-top`/`margin-bottom`, no solo el de `width`.
    ///
    /// Se guarda aqui, y no se pasa como parametro, porque quien lo conoce
    /// (el padre, al colocar al hijo) y quien lo necesita (el hijo, al
    /// resolver su propio padding dentro de su propia pasada de flujo) estan
    /// en llamadas distintas. `0.0` significa "todavia sin colocar", y un
    /// porcentaje sobre esa referencia resuelve a cero - que es justo lo que
    /// hace el spec cuando la referencia es indefinida.
    pub(crate) containing_width: f32,
    /// Alto del bloque contenedor, o `0.0` si es INDEFINIDO (lo normal: un
    /// contenedor con `height: auto` crece con su contenido, asi que no hay
    /// numero contra el que medir todavia).
    ///
    /// El spec dice que un `height` en porcentaje sobre una referencia
    /// indefinida se comporta como `auto`, que es justo lo que produce
    /// dejarlo en cero: `resolve_explicit_height` no devuelve nada y la caja
    /// sigue creciendo con su contenido.
    pub(crate) containing_height: f32,
}

impl LayoutBox {
    pub fn new(box_type: BoxType) -> Self {
        Self {
            box_type,
            dimensions: Rect::default(),
            box_dimensions: Dimensions::default(),
            children: Vec::new(),
            computed_style: HashMap::new(),
            dom_node: None,
            replaced_text: None,
            measure_cache: Vec::new(),
            static_position: None,
            colspan: 1,
            rowspan: 1,
            containing_width: 0.0,
            containing_height: 0.0,
        }
    }

    /// Busca la caja mas profunda (en orden de documento - esta no es una
    /// simple pila de bloques con posible solape, asi que "mas profunda"
    /// basta, no hace falta comparar z-index) cuyo rectangulo contenga
    /// `(x, y)` y que tenga un `dom_node` real, devolviendo ESE nodo. Si la
    /// caja mas especifica que contiene el punto es una caja de texto (sin
    /// `dom_node` propio), cae al `dom_node` de su ancestro mas cercano que
    /// si tenga uno - la recursion ya hace esto de forma natural: si
    /// ningun hijo produce un resultado, se usa `self.dom_node`.
    ///
    /// `(x, y)` estan en coordenadas de DOCUMENTO, no de viewport: quien
    /// llama con un clic real del usuario ya le suma el desplazamiento de
    /// scroll actual (ver `core::server::click`).
    ///
    /// **Los hijos se prueban SIEMPRE, aunque el punto caiga fuera de la
    /// caja de este padre** - arreglo de un bug real (encontrado al cablear
    /// las anclas internas de la Fase 6.1): antes esta funcion empezaba con
    /// un `if !self.dimensions.contains(x, y) { return None }`, lo que
    /// parece una poda razonable pero es FALSO en cuanto un hijo se
    /// desborda de su padre. Y siempre hay uno que lo hace: la caja RAIZ
    /// tiene por altura la del VIEWPORT, no la del contenido (ver
    /// `content_extent`, que existe justamente por eso), asi que cualquier
    /// clic por debajo de esa altura - es decir, cualquier clic sobre
    /// contenido que el usuario haya tenido que desplazar para ver - moria
    /// en esa primera comprobacion y no hacia absolutamente nada. Invisible
    /// hasta ahora porque ninguna pagina de prueba anterior combinaba
    /// scroll con un clic mas abajo del primer pantallazo.
    ///
    /// El coste es que un clic que no acierta nada recorre el arbol entero
    /// en vez de salir en la primera comprobacion; irrelevante en la
    /// practica (un clic por accion del usuario, no por fotograma) y a
    /// cambio de que los clics funcionen donde antes se perdian.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<Arc<RwLock<Node>>> {
        for child in &self.children {
            if let Some(hit) = child.hit_test(x, y) {
                return Some(hit);
            }
        }
        if self.dimensions.contains(x, y) {
            return self.dom_node.clone();
        }
        None
    }

    /// La caja que produjo ESTE nodo del DOM concreto (Fase 6.1) - el
    /// camino inverso a `hit_test` (que va de coordenadas a nodo; esto va
    /// de nodo a coordenadas). Compara por IDENTIDAD del `Arc`
    /// (`Arc::ptr_eq`), no por contenido: dos elementos distintos pueden
    /// ser iguales campo a campo (`<li>` vacios, por ejemplo) y aun asi
    /// ser cajas distintas en sitios distintos de la pagina.
    ///
    /// Devuelve la PRIMERA coincidencia en orden de documento. Un mismo
    /// `Node` produce una sola caja en el arbol de layout de hoy (no hay
    /// fragmentacion todavia: un elemento partido entre dos columnas o dos
    /// paginas generaria varias, pero este motor no tiene ni multicolumna
    /// ni paginacion), asi que "la primera" es "la unica" en la practica.
    ///
    /// Usado por las anclas internas (`href="#id"` -> a que `y` hay que
    /// desplazar el scroll, ver `core::server::click`) y por
    /// `getBoundingClientRect` (Fase 8.2).
    pub fn find_box_for_node(&self, node: &Arc<RwLock<Node>>) -> Option<&LayoutBox> {
        if let Some(own) = &self.dom_node {
            if Arc::ptr_eq(own, node) {
                return Some(self);
            }
        }
        self.children.iter().find_map(|child| child.find_box_for_node(node))
    }

    /// El borde inferior real de TODO el contenido, mas alla del viewport
    /// con el que se construyo el arbol: `dimensions.height` de la caja
    /// raiz es siempre el alto del viewport de ENTRADA
    /// (`LayoutTreeBuilder::build`), nunca se actualiza para reflejar
    /// cuanto se desborda el contenido de verdad (`flow_block_children` en
    /// tree.rs solo escribe la altura de cada HIJO, nunca la de vuelta en
    /// el contenedor). Hay que recorrer el arbol entero y quedarse con el
    /// borde inferior (`y + height`) mas bajo de verdad para saber hasta
    /// donde se puede hacer scroll - usado por `gfx::window` para acotar
    /// `scroll_offset_y`.
    pub fn content_extent(&self) -> f32 {
        let own_bottom = self.dimensions.y + self.dimensions.height;
        self.children.iter().map(LayoutBox::content_extent).fold(own_bottom, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn contains_is_true_for_a_point_strictly_inside() {
        assert!(rect(10.0, 10.0, 100.0, 50.0).contains(50.0, 30.0));
    }

    #[test]
    fn contains_is_true_for_a_point_exactly_on_any_edge() {
        let r = rect(10.0, 10.0, 100.0, 50.0);
        assert!(r.contains(10.0, 30.0), "borde izquierdo");
        assert!(r.contains(110.0, 30.0), "borde derecho");
        assert!(r.contains(50.0, 10.0), "borde superior");
        assert!(r.contains(50.0, 60.0), "borde inferior");
    }

    #[test]
    fn contains_is_false_for_a_point_just_outside_any_edge() {
        let r = rect(10.0, 10.0, 100.0, 50.0);
        assert!(!r.contains(9.9, 30.0));
        assert!(!r.contains(110.1, 30.0));
        assert!(!r.contains(50.0, 9.9));
        assert!(!r.contains(50.0, 60.1));
    }

    #[test]
    fn content_extent_of_a_single_box_is_its_own_bottom_edge() {
        let mut b = LayoutBox::new(BoxType::Block);
        b.dimensions = rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(b.content_extent(), 50.0);
    }

    #[test]
    fn content_extent_grows_with_a_child_that_overflows_the_roots_own_height() {
        let mut root = LayoutBox::new(BoxType::Block);
        // El "viewport" de entrada con el que LayoutTreeBuilder::build crea
        // la raiz - 200 de alto, pero el contenido real se desborda mucho mas.
        root.dimensions = rect(0.0, 0.0, 100.0, 200.0);
        let mut child = LayoutBox::new(BoxType::Block);
        child.dimensions = rect(0.0, 150.0, 100.0, 300.0);
        root.children.push(child);
        assert_eq!(root.content_extent(), 450.0, "150 + 300, no los 200 de dimensions.height de la raiz");
    }

    #[test]
    fn content_extent_falls_back_to_the_roots_own_bottom_when_children_dont_overflow() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions = rect(0.0, 0.0, 100.0, 500.0);
        let mut child = LayoutBox::new(BoxType::Block);
        child.dimensions = rect(0.0, 0.0, 100.0, 50.0);
        root.children.push(child);
        assert_eq!(root.content_extent(), 500.0, "el hijo no se desborda, gana el propio borde inferior de la raiz");
    }

    /// Regresion del bug encontrado en la Fase 6.1: la caja RAIZ mide lo que
    /// el VIEWPORT, no lo que el contenido, asi que un hijo mas abajo del
    /// primer pantallazo cae FUERA de ella. Antes, `hit_test` podaba en la
    /// raiz y devolvia `None` - o sea, ningun clic funcionaba sobre nada que
    /// hubiera que desplazar para ver.
    #[test]
    fn hit_test_finds_a_child_that_overflows_below_the_root_viewport_box() {
        use engine_dom::{HtmlParser, Node};
        let dom = HtmlParser::parse(r#"<html><body><div id="abajo"></div></body></html>"#);
        let abajo = Node::find_by_id(&dom, "abajo").expect("deberia existir #abajo");

        let mut root = LayoutBox::new(BoxType::Block);
        // Exactamente lo que hace LayoutTreeBuilder::build: la raiz mide el
        // viewport (300 de alto), no el contenido.
        root.dimensions = rect(0.0, 0.0, 500.0, 300.0);
        let mut hijo = LayoutBox::new(BoxType::Block);
        hijo.dimensions = rect(0.0, 1400.0, 500.0, 20.0);
        hijo.dom_node = Some(abajo.clone());
        root.children.push(hijo);

        let hit = root.hit_test(10.0, 1410.0);
        assert!(
            hit.is_some_and(|n| Arc::ptr_eq(&n, &abajo)),
            "un clic sobre contenido desplazado (y=1410, muy por debajo de los 300 de la raiz) deberia acertar el elemento, no perderse"
        );
    }

    #[test]
    fn hit_test_still_returns_none_when_the_point_hits_nothing_at_all() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions = rect(0.0, 0.0, 500.0, 300.0);
        let mut hijo = LayoutBox::new(BoxType::Block);
        hijo.dimensions = rect(0.0, 1400.0, 500.0, 20.0);
        root.children.push(hijo);
        assert!(
            root.hit_test(10.0, 800.0).is_none(),
            "un punto en tierra de nadie (ni la raiz ni el hijo) deberia seguir dando None"
        );
    }

    /// `find_box_for_node` compara por identidad de `Arc`, no por contenido -
    /// dos nodos con el MISMO contenido (dos `<li>` vacios, por ejemplo) son
    /// cajas distintas y no deben confundirse (Fase 6.1).
    #[test]
    fn find_box_for_node_matches_by_arc_identity_not_by_equal_content() {
        use engine_dom::{HtmlParser, Node};
        let dom = HtmlParser::parse(r#"<html><body><div id="a"></div><div id="b"></div></body></html>"#);
        let node_a = Node::find_by_id(&dom, "a").expect("deberia existir #a");
        let node_b = Node::find_by_id(&dom, "b").expect("deberia existir #b");

        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions = rect(0.0, 0.0, 100.0, 500.0);
        let mut box_a = LayoutBox::new(BoxType::Block);
        box_a.dimensions = rect(0.0, 10.0, 100.0, 20.0);
        box_a.dom_node = Some(node_a.clone());
        let mut box_b = LayoutBox::new(BoxType::Block);
        box_b.dimensions = rect(0.0, 300.0, 100.0, 20.0);
        box_b.dom_node = Some(node_b.clone());
        root.children.push(box_a);
        root.children.push(box_b);

        assert_eq!(root.find_box_for_node(&node_a).map(|b| b.dimensions.y), Some(10.0));
        assert_eq!(root.find_box_for_node(&node_b).map(|b| b.dimensions.y), Some(300.0));
    }

    #[test]
    fn find_box_for_node_finds_a_deeply_nested_box_and_is_none_for_an_unlaid_out_node() {
        use engine_dom::{HtmlParser, Node};
        let dom = HtmlParser::parse(r#"<html><body><div id="dentro"></div><div id="fuera"></div></body></html>"#);
        let dentro = Node::find_by_id(&dom, "dentro").expect("deberia existir #dentro");
        let fuera = Node::find_by_id(&dom, "fuera").expect("deberia existir #fuera");

        let mut root = LayoutBox::new(BoxType::Block);
        let mut nivel1 = LayoutBox::new(BoxType::Block);
        let mut nivel2 = LayoutBox::new(BoxType::Block);
        let mut hoja = LayoutBox::new(BoxType::Block);
        hoja.dimensions = rect(0.0, 777.0, 50.0, 10.0);
        hoja.dom_node = Some(dentro.clone());
        nivel2.children.push(hoja);
        nivel1.children.push(nivel2);
        root.children.push(nivel1);

        assert_eq!(root.find_box_for_node(&dentro).map(|b| b.dimensions.y), Some(777.0));
        assert!(
            root.find_box_for_node(&fuera).is_none(),
            "un nodo que no produjo ninguna caja (display:none, o fuera del arbol de layout) deberia dar None, no una caja cualquiera"
        );
    }

    #[test]
    fn content_extent_recurses_through_multiple_levels_to_find_the_deepest_bottom() {
        let mut root = LayoutBox::new(BoxType::Block);
        root.dimensions = rect(0.0, 0.0, 100.0, 100.0);
        let mut child = LayoutBox::new(BoxType::Block);
        child.dimensions = rect(0.0, 50.0, 100.0, 50.0);
        let mut grandchild = LayoutBox::new(BoxType::Block);
        grandchild.dimensions = rect(0.0, 900.0, 100.0, 100.0);
        child.children.push(grandchild);
        root.children.push(child);
        assert_eq!(root.content_extent(), 1000.0, "900 + 100 del nieto, no el borde del hijo (100) ni el de la raiz (100)");
    }
}

/// El color de fondo que debe pintar el LIENZO entero, propagado desde el
/// elemento raiz segun el spec (CSS Backgrounds 3, "The Canvas Background").
///
/// Es una de esas reglas que parecen un detalle y se notan muchisimo: el
/// fondo de `<html>` (o, si `<html>` no declara ninguno, el de `<body>`) no
/// se pinta solo en la caja de ese elemento, sino en TODO el viewport,
/// incluso por debajo de donde llega el contenido. Sin esto, una pagina con
/// `body { background: #111 }` se veia como una franja oscura del alto del
/// contenido sobre un fondo gris claro - el sintoma clasico de "esto esta
/// roto" en cualquier web con tema oscuro. Verificado en vivo antes de
/// arreglarlo.
///
/// Devuelve el VALOR CSS sin interpretar (`"#2244aa"`, `"red"`): quien
/// pinta (`engine-gfx`) ya tiene su propio parseo de color, y duplicarlo
/// aqui seria una segunda fuente de verdad sobre que es un color valido.
///
/// NO implementado: que `<body>` deje de pintar su propio fondo cuando este
/// se ha propagado (el spec dice que el elemento cede el fondo al lienzo).
/// Como se pinta el mismo color en ambos sitios, el resultado visible es
/// identico; solo se notaria con fondos semitransparentes superpuestos.
pub fn canvas_background(layout_root: &LayoutBox) -> Option<String> {
    let html = find_by_tag(layout_root, "html");
    if let Some(color) = html.and_then(background_color_of) {
        return Some(color);
    }
    find_by_tag(layout_root, "body").and_then(background_color_of)
}

fn background_color_of(layout_box: &LayoutBox) -> Option<String> {
    layout_box.computed_style.get("background-color").cloned()
}

/// Primera caja en preorden cuyo nodo del DOM tiene esta etiqueta. Se busca
/// por el DOM y no por posicion en el arbol porque la caja raiz es
/// sintetica (envuelve el viewport) y no siempre hay un `<html>` explicito
/// en el HTML original - `html5ever` lo inserta, pero la forma del arbol de
/// layout depende ademas de `display`.
fn find_by_tag<'a>(layout_box: &'a LayoutBox, tag: &str) -> Option<&'a LayoutBox> {
    if let Some(node) = &layout_box.dom_node {
        if let Ok(node) = node.read() {
            if let engine_dom::NodeType::Element { tag_name, .. } = &node.node_type {
                if tag_name.eq_ignore_ascii_case(tag) {
                    return Some(layout_box);
                }
            }
        }
    }
    layout_box.children.iter().find_map(|child| find_by_tag(child, tag))
}
