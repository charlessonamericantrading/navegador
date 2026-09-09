//! La jerarquía de clases del DOM: `EventTarget` → `Node` → `Element` →
//! `HTMLElement` → `HTMLDivElement`… (tarea C3 del `plan.md`).
//!
//! **Qué gana una página con esto, medido y no supuesto.** Antes, cada objeto
//! de elemento era un objeto suelto sin ninguna cadena de prototipos, así que
//! dos patrones que un bundle real ejecuta al arrancar fallaban en silencio:
//!
//! 1. `el instanceof HTMLElement` era `false`. Es la comprobación con la que
//!    media web decide si algo es un nodo o un objeto de configuración, y una
//!    respuesta equivocada manda al código por la rama que no es.
//! 2. `Element.prototype.matches = ...` no hacía nada. Ese es literalmente
//!    cómo se instala un polyfill: se añade al prototipo y se espera que lo
//!    vean los elementos ya creados. Sin cadena, el polyfill se instalaba
//!    «bien» y el método seguía sin existir.
//!
//! **La simplificación que esto NO cierra, dicha aquí y no escondida.** Los
//! métodos de un elemento siguen viviendo en la propia instancia, no en el
//! prototipo: `build_element_object` construye cada uno con un *closure* que
//! captura su nodo. Consecuencias exactas:
//!
//! * Un método que el motor YA tiene (`appendChild`, `setAttribute`…) está en
//!   la instancia y **tapa** al del prototipo. Un envoltorio del estilo
//!   `const orig = Element.prototype.appendChild; Element.prototype.appendChild = ...`
//!   se instala pero nunca llega a ejecutarse.
//! * Un método que el motor NO tiene sí se hereda del prototipo, que es el
//!   caso de todo polyfill y el que más importa.
//!
//! Cerrarlo del todo exige que cada método del prototipo recupere su nodo
//! desde `this` en vez de desde una captura, es decir reescribir las ~675
//! líneas de `build_element_object`. Anotado en `huecos_sin_resolver.md`.

use std::collections::HashMap;

use boa_engine::object::{FunctionObjectBuilder, JsObject, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsNativeError, JsResult, JsValue, NativeFunction};

/// Los objetos prototipo del documento, ya enlazados entre sí.
#[derive(Clone)]
pub struct DomPrototypes {
    /// Prototipo por etiqueta HTML (`div` → `HTMLDivElement.prototype`).
    por_etiqueta: HashMap<String, JsObject>,
    /// Para cualquier etiqueta sin clase propia. En el spec real eso es
    /// `HTMLUnknownElement` solo para etiquetas inválidas; aquí `HTMLElement`
    /// cubre las dos cosas, que es la aproximación que menos código rompe:
    /// `<section>` es un `HTMLElement` de verdad, y una etiqueta inventada
    /// también se comporta como tal.
    html_element: JsObject,
    /// Prototipos de nodos que no son elementos.
    text: JsObject,
    comment: JsObject,
    document: JsObject,
}

impl DomPrototypes {
    /// El prototipo que corresponde a un elemento con esa etiqueta.
    pub fn para_etiqueta(&self, etiqueta: &str) -> JsObject {
        self.por_etiqueta
            .get(&etiqueta.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| self.html_element.clone())
    }

    pub fn texto(&self) -> JsObject {
        self.text.clone()
    }

    pub fn comentario(&self) -> JsObject {
        self.comment.clone()
    }

    pub fn documento(&self) -> JsObject {
        self.document.clone()
    }
}

/// Construye una clase: su prototipo y su constructor global, enlazados en los
/// dos sentidos (`Clase.prototype` y `prototipo.constructor`).
///
/// El constructor LANZA si se llama con `new`. No es una limitación: el spec
/// dice que `new HTMLDivElement()` es un `TypeError: Illegal constructor`, y
/// los elementos se crean con `document.createElement`. Lanzar es lo correcto,
/// y además evita el peor resultado posible, que sería devolver un objeto que
/// parece un elemento y no está en ningún documento.
fn crear_clase(
    nombre: &str,
    padre: Option<&JsObject>,
    context: &mut Context,
) -> JsResult<JsObject> {
    let prototipo = ObjectInitializer::new(context).build();

    if let Some(padre) = padre {
        prototipo.set_prototype(Some(padre.clone()));
    }

    let nombre_clase = nombre.to_string();
    let constructor_fn = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], nombre: &NombreClase, _context| {
            Err(JsNativeError::typ()
                .with_message(format!(
                    "Illegal constructor: {} no se construye con `new`; los nodos se \
                     crean con document.createElement/createTextNode",
                    nombre.0
                ))
                .into())
        },
        NombreClase(nombre_clase),
    );

    let constructor = FunctionObjectBuilder::new(context.realm(), constructor_fn)
        .name(js_string!(nombre.to_string()))
        .length(0)
        // `constructor(true)` para que `new X()` llegue a la función y lance el
        // `TypeError` del spec, en vez del «X is not a constructor» de Boa, que
        // diría algo distinto de lo que pasa.
        .constructor(true)
        .build();

    // `instanceof` lee `Clase.prototype` y lo busca en la cadena del objeto.
    // Sin esta línea, toda la jerarquía existiría y `instanceof` seguiría
    // dando `false`, que es justo lo que se viene a arreglar.
    constructor.set(
        js_string!("prototype"),
        prototipo.clone(),
        false,
        context,
    )?;
    prototipo.set(
        js_string!("constructor"),
        constructor.clone(),
        false,
        context,
    )?;

    context.register_global_property(
        js_string!(nombre.to_string()),
        constructor,
        Attribute::WRITABLE | Attribute::CONFIGURABLE,
    )?;

    Ok(prototipo)
}

#[derive(Clone)]
struct NombreClase(String);
impl boa_gc::Finalize for NombreClase {}
unsafe impl boa_gc::Trace for NombreClase {
    boa_gc::empty_trace!();
}

/// Registra la jerarquía entera y devuelve los prototipos para que
/// `build_element_object` pueda colgar cada elemento del suyo.
pub fn register_dom_classes(context: &mut Context) -> JsResult<DomPrototypes> {
    let event_target = crear_clase("EventTarget", None, context)?;
    let node = crear_clase("Node", Some(&event_target), context)?;
    let element = crear_clase("Element", Some(&node), context)?;
    let html_element = crear_clase("HTMLElement", Some(&element), context)?;

    let text = crear_clase("Text", Some(&node), context)?;
    let comment = crear_clase("Comment", Some(&node), context)?;
    let document = crear_clase("Document", Some(&node), context)?;
    let _fragment = crear_clase("DocumentFragment", Some(&node), context)?;

    // Las constantes de tipo de nodo van en el PROTOTIPO y en el CONSTRUCTOR,
    // como en el spec: código real usa las dos formas (`Node.ELEMENT_NODE` y
    // `nodo.ELEMENT_NODE`).
    const TIPOS: [(&str, u8); 8] = [
        ("ELEMENT_NODE", 1),
        ("ATTRIBUTE_NODE", 2),
        ("TEXT_NODE", 3),
        ("CDATA_SECTION_NODE", 4),
        ("COMMENT_NODE", 8),
        ("DOCUMENT_NODE", 9),
        ("DOCUMENT_TYPE_NODE", 10),
        ("DOCUMENT_FRAGMENT_NODE", 11),
    ];
    let constructor_node = context
        .global_object()
        .get(js_string!("Node"), context)?
        .as_object()
        .cloned();
    for (nombre, valor) in TIPOS {
        node.set(js_string!(nombre), JsValue::from(valor), false, context)?;
        if let Some(c) = &constructor_node {
            c.set(js_string!(nombre), JsValue::from(valor), false, context)?;
        }
    }

    // Cada entrada es (nombre de la clase, etiquetas que la usan). La lista no
    // pretende ser exhaustiva: cubre las clases que el código real comprueba
    // con `instanceof`. Una etiqueta que no esté aquí cae en `HTMLElement`,
    // que sigue siendo cierto (todo elemento HTML lo es).
    const CLASES: [(&str, &[&str]); 17] = [
        ("HTMLDivElement", &["div"]),
        ("HTMLSpanElement", &["span"]),
        ("HTMLParagraphElement", &["p"]),
        ("HTMLAnchorElement", &["a"]),
        ("HTMLImageElement", &["img"]),
        ("HTMLInputElement", &["input"]),
        ("HTMLButtonElement", &["button"]),
        ("HTMLTextAreaElement", &["textarea"]),
        ("HTMLSelectElement", &["select"]),
        ("HTMLOptionElement", &["option"]),
        ("HTMLFormElement", &["form"]),
        ("HTMLCanvasElement", &["canvas"]),
        ("HTMLScriptElement", &["script"]),
        ("HTMLStyleElement", &["style"]),
        ("HTMLLinkElement", &["link"]),
        ("HTMLUListElement", &["ul"]),
        ("HTMLLIElement", &["li"]),
    ];

    let mut por_etiqueta = HashMap::new();
    for (clase, etiquetas) in CLASES {
        let proto = crear_clase(clase, Some(&html_element), context)?;
        for etiqueta in etiquetas {
            por_etiqueta.insert((*etiqueta).to_string(), proto.clone());
        }
    }

    // `HTMLHeadingElement` cubre los seis niveles de titular, igual que el
    // spec: no hay una clase por nivel.
    let heading = crear_clase("HTMLHeadingElement", Some(&html_element), context)?;
    for etiqueta in ["h1", "h2", "h3", "h4", "h5", "h6"] {
        por_etiqueta.insert(etiqueta.to_string(), heading.clone());
    }

    // `HTMLTableElement` y sus partes comparten la misma idea.
    let tabla = crear_clase("HTMLTableElement", Some(&html_element), context)?;
    por_etiqueta.insert("table".to_string(), tabla);
    let celda = crear_clase("HTMLTableCellElement", Some(&html_element), context)?;
    for etiqueta in ["td", "th"] {
        por_etiqueta.insert(etiqueta.to_string(), celda.clone());
    }
    let fila = crear_clase("HTMLTableRowElement", Some(&html_element), context)?;
    por_etiqueta.insert("tr".to_string(), fila);

    Ok(DomPrototypes {
        por_etiqueta,
        html_element,
        text,
        comment,
        document,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Source;

    fn evaluar(codigo: &str) -> String {
        let mut context = Context::default();
        register_dom_classes(&mut context).expect("no se pudieron registrar las clases");
        match context.eval(Source::from_bytes(codigo)) {
            Ok(v) => v
                .to_string(&mut context)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|_| "<no representable>".to_string()),
            Err(e) => format!("ERROR: {e}"),
        }
    }

    #[test]
    fn las_clases_existen_como_globales() {
        for clase in [
            "EventTarget",
            "Node",
            "Element",
            "HTMLElement",
            "HTMLDivElement",
            "HTMLInputElement",
            "Text",
            "Document",
            "DocumentFragment",
        ] {
            assert_eq!(
                evaluar(&format!("typeof {clase}")),
                "function",
                "falta el global {clase}"
            );
        }
    }

    #[test]
    fn la_cadena_de_herencia_es_la_del_spec() {
        // Que la cadena sea correcta es lo que hace que `instanceof` responda
        // bien en los dos sentidos: un div ES un HTMLElement, y HTMLElement NO
        // es un HTMLDivElement.
        assert_eq!(
            evaluar("Object.getPrototypeOf(HTMLDivElement.prototype) === HTMLElement.prototype"),
            "true"
        );
        assert_eq!(
            evaluar("Object.getPrototypeOf(HTMLElement.prototype) === Element.prototype"),
            "true"
        );
        assert_eq!(
            evaluar("Object.getPrototypeOf(Element.prototype) === Node.prototype"),
            "true"
        );
        assert_eq!(
            evaluar("Object.getPrototypeOf(Node.prototype) === EventTarget.prototype"),
            "true"
        );
    }

    #[test]
    fn el_prototipo_apunta_de_vuelta_a_su_constructor() {
        // `x.constructor.name` es como bastante codigo averigua de que tipo es
        // algo cuando `instanceof` no le sirve.
        assert_eq!(evaluar("Element.prototype.constructor === Element"), "true");
        assert_eq!(evaluar("HTMLDivElement.prototype.constructor.name"), "HTMLDivElement");
    }

    #[test]
    fn las_constantes_de_tipo_de_nodo_estan_en_los_dos_sitios() {
        assert_eq!(evaluar("Node.ELEMENT_NODE"), "1");
        assert_eq!(evaluar("Node.TEXT_NODE"), "3");
        assert_eq!(evaluar("Node.prototype.COMMENT_NODE"), "8");
        assert_eq!(evaluar("Node.DOCUMENT_FRAGMENT_NODE"), "11");
    }

    #[test]
    fn construir_una_clase_del_dom_con_new_lanza_como_el_spec() {
        // El spec dice `TypeError: Illegal constructor`. Devolver un objeto
        // seria peor: pareceria un elemento y no estaria en ningun documento.
        let r = evaluar("try { new HTMLDivElement(); 'no lanzo' } catch (e) { e.message }");
        assert!(
            r.contains("Illegal constructor"),
            "deberia lanzar el error del spec, dijo: {r}"
        );
    }

    #[test]
    fn un_polyfill_sobre_el_prototipo_lo_ven_los_descendientes() {
        // Este es el patron que motiva toda la clase: se añade al prototipo y
        // los objetos que cuelgan de el lo ven.
        assert_eq!(
            evaluar(
                "Element.prototype.miPolyfill = function () { return 'ok'; };\
                 const o = Object.create(HTMLDivElement.prototype);\
                 o.miPolyfill()"
            ),
            "ok"
        );
    }

    #[test]
    fn instanceof_responde_en_los_dos_sentidos() {
        assert_eq!(
            evaluar("Object.create(HTMLDivElement.prototype) instanceof HTMLElement"),
            "true"
        );
        assert_eq!(
            evaluar("Object.create(HTMLDivElement.prototype) instanceof Node"),
            "true"
        );
        // Y lo que NO debe ser cierto, que es la otra mitad de la prueba: si
        // todo diera `true`, el `instanceof` no distinguiria nada.
        assert_eq!(
            evaluar("Object.create(HTMLElement.prototype) instanceof HTMLDivElement"),
            "false"
        );
    }
}
