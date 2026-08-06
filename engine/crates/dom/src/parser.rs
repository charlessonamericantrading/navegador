use crate::html5ever_sink::NodeSink;
use crate::node::Node;
use html5ever::driver::ParseOpts;
use html5ever::tendril::TendrilSink;
use std::sync::{Arc, RwLock};

pub struct HtmlParser;

impl HtmlParser {
    /// Parsea HTML real usando html5ever: implementa el algoritmo de
    /// construccion de arbol del spec, con la misma recuperacion de errores
    /// que un navegador real (etiquetas sin cerrar, anidamiento invalido,
    /// atributos duplicados...). Sustituye al tokenizador artesanal anterior,
    /// que no manejaba ninguno de esos casos.
    pub fn parse(html_str: &str) -> Arc<RwLock<Node>> {
        let sink = NodeSink::new();
        let opts = ParseOpts::default();
        html5ever::parse_document(sink, opts).one(html_str.to_string())
    }
}
