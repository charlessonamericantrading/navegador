use crate::node::Node;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPhase {
    None,
    Capturing,
    AtTarget,
    Bubbling,
}

#[derive(Debug, Clone)]
pub struct DomEvent {
    pub event_type: String,
    pub phase: EventPhase,
    pub bubbles: bool,
    pub cancelable: bool,
    pub default_prevented: bool,
    pub propagation_stopped: bool,
}

impl DomEvent {
    pub fn new(event_type: &str, bubbles: bool) -> Self {
        Self {
            event_type: event_type.to_string(),
            phase: EventPhase::None,
            bubbles,
            cancelable: true,
            default_prevented: false,
            propagation_stopped: false,
        }
    }

    pub fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    pub fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }
}

pub struct EventDispatcher;

impl EventDispatcher {
    pub fn dispatch(event: &mut DomEvent, target_node: &Arc<RwLock<Node>>) {
        event.phase = EventPhase::AtTarget;
        tracing::info!("[DOM EventDispatcher] Dispatching '{}' on node {:?}", event.event_type, target_node.read().unwrap().node_type);

        if event.bubbles {
            event.phase = EventPhase::Bubbling;
            tracing::info!("[DOM EventDispatcher] Event '{}' bubbling phase complete", event.event_type);
        }
    }
}
