//! Crate `engine-ai` (Fase 5.1).
//! Proporciona capacidades nativas de comprensión semántica y árbol accesible (AOM)
//! para los agentes de Inteligencia Artificial del navegador.

pub mod aom;

pub use aom::{AccessibilityTree, AccessibleNode, AccessibleRole};
