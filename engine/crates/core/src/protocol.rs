//! Contrato NDJSON entre el proceso del motor Rust y el backend de la app.
//!
//! El protocolo es deliberadamente pequeño en esta primera iteracion. No
//! declara que haya una pagina renderizada hasta que el pipeline nativo este
//! conectado de verdad; `renderer_status` distingue el servidor vivo del
//! renderer disponible.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

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
            | Self::Back { id }
            | Self::Forward { id }
            | Self::NewTab { id, .. }
            | Self::CloseTab { id, .. }
            | Self::SwitchTab { id, .. }
            | Self::ListTabs { id }
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
    },
    Pong {
        id: Option<String>,
        protocol_version: u32,
        renderer_status: &'static str,
    },
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
        elements: Vec<InteractiveElement>,
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
            EngineRequest::Resize { id, width, height } => {
                assert_eq!(id.as_deref(), Some("r1"));
                assert_eq!((width, height), (900, 600));
            }
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
        };

        let json = serde_json::to_string(&response).expect("response should serialize");
        assert!(json.contains("\"renderer_status\":\"ready\""));
        assert!(json.contains("\"protocol_version\":1"));
    }
}
