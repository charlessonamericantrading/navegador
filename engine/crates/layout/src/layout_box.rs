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
}

#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub box_type: BoxType,
    pub dimensions: Rect,
    /// Geometria de padding/border/margin (`Dimensions::padding_box/
    /// border_box/margin_box` en box_model.rs) - siempre `default()` (todo
    /// cero) hoy: `LayoutTreeBuilder` nunca la puebla con valores reales de
    /// CSS ni nada la lee todavia. No hay box model completo (padding/
    /// border/margin de verdad) hasta Fase 2 (ver ARCHITECTURE.md y el
    /// doc-comment de `LayoutTreeBuilder::build` en tree.rs); este campo
    /// existe para no tener que rehacer la geometria cuando llegue ese
    /// trabajo, no porque este conectado ahora.
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
    pub fn hit_test(&self, x: f32, y: f32) -> Option<Arc<RwLock<Node>>> {
        if !self.dimensions.contains(x, y) {
            return None;
        }
        for child in &self.children {
            if let Some(hit) = child.hit_test(x, y) {
                return Some(hit);
            }
        }
        self.dom_node.clone()
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
