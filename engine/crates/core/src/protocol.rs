//! Contrato NDJSON entre el proceso del motor Rust y el backend de la app.
//!
//! El protocolo es deliberadamente pequeño en esta primera iteracion. No
//! declara que haya una pagina renderizada hasta que el pipeline nativo este
//! conectado de verdad; `renderer_status` distingue el servidor vivo del
//! renderer disponible.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

fn default_scale() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineRequest {
    Navigate {
        id: Option<String>,
        url: String,
    },
    Ping {
        id: Option<String>,
    },
    Resize {
        id: Option<String>,
        width: u32,
        height: u32,
        #[serde(default = "default_scale")]
        scale: f32,
    },
    GetState {
        id: Option<String>,
    },
    Click {
        id: Option<String>,
        x: f32,
        y: f32,
    },
    Scroll {
        id: Option<String>,
        dx: i32,
        dy: i32,
    },
    TypeText {
        id: Option<String>,
        x: f32,
        y: f32,
        text: String,
        press_enter: bool,
    },
    PressKey {
        id: Option<String>,
        key: String,
    },
    /// Clic en un elemento de la lista `elements` por su `id` (tarea 3.1 del
    /// plan). Las coordenadas de `elements[].rect` son del DOCUMENTO, y
    /// `click` espera coordenadas de VIEWPORT: tras un scroll no coinciden.
    /// Con esta peticion el motor desplaza el elemento a la vista y calcula
    /// el punto exacto, que es la unica forma fiable de clicar lo que se ve.
    ClickElement {
        id: Option<String>,
        element_id: u32,
    },
    /// Escribe en un elemento de la lista `elements` por su `id` (tarea 3.1).
    /// `replace` borra antes el valor actual (rellenar un formulario no debe
    /// concatenar al texto que ya hubiera).
    TypeIntoElement {
        id: Option<String>,
        element_id: u32,
        text: String,
        #[serde(default)]
        replace: bool,
        #[serde(default)]
        press_enter: bool,
    },
    /// Detiene la navegacion en curso (tarea 1.3 del plan). Solo hace algo si
    /// hay un `navigate` a medias: el motor lo abandona y contesta a ese
    /// `navigate` con un error `navegacion_cancelada`. Sin nada en curso es un
    /// no-op. Un `navigate` nuevo mientras otro
    /// sigue cargando tambien lo cancela: lo ultimo que pide el usuario manda.
    /// La respuesta a `stop` es siempre `ok` (`stopped`), haya o no algo que
    /// detener.
    Stop {
        id: Option<String>,
    },
    /// Historial atras/adelante (Fase 4.4) - sin parametros propios: la URL
    /// a la que ir ya se conoce del historial que `EngineServer` mantiene
    /// internamente (ver `server::EngineServer::back`/`forward`), igual
    /// que `history.back()`/`history.forward()` reales de JS no reciben
    /// ninguna URL como argumento.
    Back {
        id: Option<String>,
    },
    Forward {
        id: Option<String>,
    },
    /// Pestañas (Fase 4.5). `NewTab` crea una pestaña y la hace ACTIVA de
    /// inmediato (igual que `target="_blank"` real, ver
    /// `server::EngineServer::open_new_tab`); `url` ausente deja la pestaña
    /// en blanco, igual que abrir una pestaña nueva a mano. `CloseTab`/
    /// `SwitchTab` referencian la pestaña por su `tab_id` (el `id` de
    /// `TabInfo`, NO el `id` de correlacion de peticion/respuesta que ya
    /// llevan TODAS las variantes de este enum). `ListTabs` no tiene
    /// parametros propios: la lista completa se deriva del estado interno
    /// del servidor.
    NewTab {
        id: Option<String>,
        url: Option<String>,
    },
    CloseTab {
        id: Option<String>,
        tab_id: u32,
    },
    SwitchTab {
        id: Option<String>,
        tab_id: u32,
    },
    ListTabs {
        id: Option<String>,
    },
    /// Árbol Semántico Accesible (Fase 5.2): extrae la jerarquía AOM optimizada
    /// con coordenadas espaciales para modelos de lenguaje (LLMs).
    GetAccessibilityTree {
        id: Option<String>,
    },
    /// El texto visible de la pagina, en orden de lectura y con saltos de linea
    /// entre bloques (tarea 3.4). `elements` solo trae lo accionable; para
    /// contestar "¿que dice la primera frase?" el agente necesita leer el resto.
    GetPageText {
        id: Option<String>,
    },
    Shutdown {
        id: Option<String>,
    },
}

impl EngineRequest {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Navigate { id, .. }
            | Self::Ping { id }
            | Self::Resize { id, .. }
            | Self::GetState { id }
            | Self::Click { id, .. }
            | Self::Scroll { id, .. }
            | Self::TypeText { id, .. }
            | Self::PressKey { id, .. }
            | Self::Stop { id }
            | Self::ClickElement { id, .. }
            | Self::TypeIntoElement { id, .. }
            | Self::Back { id }
            | Self::Forward { id }
            | Self::NewTab { id, .. }
            | Self::CloseTab { id, .. }
            | Self::SwitchTab { id, .. }
            | Self::ListTabs { id }
            | Self::GetAccessibilityTree { id }
            | Self::GetPageText { id }
            | Self::Shutdown { id } => id.as_deref(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineResponse {
    Ready {
        id: Option<String>,
        protocol_version: u32,
        renderer: &'static str,
        renderer_status: &'static str,
        width: u32,
        height: u32,
        /// Quien hace la red y guarda cookies y almacenamiento: `"local"` (este
        /// mismo proceso) o `"remote"` (un `engine_broker`, ADR 0001 etapa 2).
        /// Lo dice el motor, no quien lo arranco: asi se comprueba que el modo
        /// pedido es el que de verdad corre.
        broker: &'static str,
    },
    Pong {
        id: Option<String>,
        protocol_version: u32,
        renderer_status: &'static str,
        /// Lo mismo que en `Ready`: permite comprobar el modo de un motor ya
        /// arrancado (la prueba de humo del paquete lo hace).
        broker: &'static str,
    },
    /// Con `id` es la respuesta a una peticion. Con `id: null` es una
    /// publicacion espontanea (Fase 50): un temporizador de la pestaña activa
    /// cambio lo que se ve. Un consumidor tiene que correlacionar por `id` y
    /// no asumir que la siguiente linea responde a su ultima peticion.
    State {
        id: Option<String>,
        renderer_status: &'static str,
        /// Pestañas (Fase 4.5) - el `id` de la pestaña ACTIVA a la que
        /// pertenece este estado (screenshot/url/titulo/historial), para
        /// que el frontend pueda contrastarlo con la pestaña que cree tener
        /// seleccionada sin llevar su propia cuenta paralela.
        tab_id: u32,
        scroll_offset_y: f32,
        url: String,
        title: String,
        screenshot: String,
        /// Los elementos interactivos de la pagina. `None` (el campo no se
        /// envia) significa "no han cambiado desde el estado anterior": lo usa
        /// `scroll`, que solo mueve el desplazamiento - las coordenadas de los
        /// elementos son del documento, no de la ventana, asi que no cambian
        /// - y reenviarlos pesaba tanto como la propia captura (tarea 1.5).
        #[serde(skip_serializing_if = "Option::is_none")]
        elements: Option<Vec<InteractiveElement>>,
        /// Fase 39: la pagina se descargo SIN error de red pero no tiene
        /// nada visible que pintar, y trae `<script>` que podrian haberlo
        /// generado. Es el caso de la inmensa mayoria de webs modernas
        /// (React/Next/Vue): el servidor manda una cascara vacia y el
        /// contenido lo construye JavaScript en el cliente. El motor no
        /// ejecuta todavia esos bundles, asi que pintaba un blanco sin un
        /// solo mensaje - indistinguible de "la app esta rota". Este flag
        /// existe para que el frontend pueda DECIRLO en vez de callar.
        /// NO es una promesa de que la pagina funcionaria con mas JS: es
        /// una descripcion de lo que se observo (vacia + con scripts).
        /// `None` (no se envia) = sin cambios, igual que `elements`.
        #[serde(skip_serializing_if = "Option::is_none")]
        requires_javascript: Option<bool>,
        /// Historial atras/adelante (Fase 4.4) - si hay una entrada real a
        /// la que ir con `back`/`forward` en este momento, para que el
        /// frontend pueda habilitar/deshabilitar sus botones sin tener que
        /// llevar su propia copia paralela del historial.
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// Lista de pestañas abiertas (Fase 4.5, respuesta a `list_tabs`) - cada
    /// `TabInfo` lleva su propio titulo/URL (vacios si esa pestaña todavia
    /// no cargo ninguna pagina) para que el frontend pueda pintar una barra
    /// de pestañas real sin tener que pedir el estado completo (con
    /// screenshot) de cada una.
    Tabs {
        id: Option<String>,
        tabs: Vec<TabInfo>,
        active_tab_id: u32,
    },
    /// Respuesta a `GetAccessibilityTree` (Fase 5.2).
    AccessibilityTree {
        id: Option<String>,
        tab_id: u32,
        url: String,
        title: String,
        tree: engine_ai::AccessibilityTree,
        prompt: String,
    },
    /// Respuesta a `GetPageText` (tarea 3.4). `truncated` avisa de que la pagina
    /// tenia mas texto del que cabe.
    PageText {
        id: Option<String>,
        tab_id: u32,
        url: String,
        title: String,
        text: String,
        truncated: bool,
    },
    Ok {
        id: Option<String>,
        message: &'static str,
    },
    Error {
        id: Option<String>,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TabInfo {
    pub id: u32,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InteractiveElement {
    pub id: u32,
    pub tag_name: String,
    pub text: String,
    pub rect: ElementRect,
    pub selector: String,
    pub attributes: ElementAttributes,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElementRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ElementAttributes {
    pub id: Option<String>,
    pub name: Option<String>,
    pub placeholder: Option<String>,
    pub element_type: Option<String>,
    pub role: Option<String>,
    pub href: Option<String>,
    pub value: Option<String>,
    /// `checked` (Fase 4.1) - `bool`, no `Option<bool>`: a diferencia de
    /// `value` (que puede legitimamente no existir), "marcado" siempre
    /// tiene una respuesta real para cualquier elemento (`false` para todo
    /// lo que no sea un checkbox/radio, tambien correcto - un `<div>` no
    /// esta "marcado" ni "sin marcar", simplemente la pregunta no aplica,
    /// que es lo mismo que reportar `false`). Semantica de atributo
    /// booleano HTML real: presencia de `checked` en el DOM = `true`,
    /// ausencia = `false`, el VALOR del atributo (si lo hay) se ignora -
    /// ver `core::server::toggle_checked`.
    pub checked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resize_request_from_ndjson_shape() {
        let request: EngineRequest =
            serde_json::from_str(r#"{"type":"resize","id":"r1","width":900,"height":600}"#)
                .expect("resize request should parse");

        match request {
            EngineRequest::Resize { id, width, height, scale } => {
                assert_eq!(id.as_deref(), Some("r1"));
                assert_eq!((width, height), (900, 600));
                assert_eq!(scale, 1.0);
            }
            other => panic!("unexpected request: {other:?}"),
        }

        let with_scale: EngineRequest =
            serde_json::from_str(r#"{"type":"resize","id":"r2","width":900,"height":600,"scale":1.5}"#)
                .expect("resize request with scale should parse");
        match with_scale {
            EngineRequest::Resize { scale, .. } => assert_eq!(scale, 1.5),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn serializes_a_ready_response_with_native_renderer_readiness() {
        let response = EngineResponse::Ready {
            id: Some("boot".to_string()),
            protocol_version: PROTOCOL_VERSION,
            renderer: "rust-native",
            renderer_status: "ready",
            width: 1280,
            height: 720,
            broker: "local",
        };

        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"renderer_status\":\"ready\""));
        assert!(json.contains("\"protocol_version\":1"));
    }
}
