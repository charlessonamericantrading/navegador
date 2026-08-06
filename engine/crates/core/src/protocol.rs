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
        scroll_offset_y: f32,
        url: String,
        title: String,
        screenshot: String,
        elements: Vec<InteractiveElement>,
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
