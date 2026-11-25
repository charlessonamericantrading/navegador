//! La aplicación del navegador, 100 % Rust (Fase 5 de `PLAN_SONNET.md`).
//!
//! - El motor corre en un proceso aparte (`navegador --motor`) y habla NDJSON
//!   por stdin/stdout (`motor`). Si una página lo tumba, la ventana sigue.
//! - La barra (pestañas, atrás/adelante, recargar, dirección) y las páginas
//!   propias (inicio y error) se escriben en HTML/CSS y las pinta **nuestro
//!   propio motor** (`lienzo`), igual que una página web.
//! - `app::App` es toda la lógica de la interfaz sin ventana: recibe entradas
//!   (clic, rueda, teclas, mensajes del motor) y devuelve las peticiones para
//!   el motor. Así se prueba sin abrir ninguna ventana.
//! - `ventana` la conecta a una ventana nativa (`winit` + `softbuffer`) y
//!   `sin_ventana` la usa sin pantalla (capturas y pruebas de extremo a extremo).

pub mod app;
pub mod barra;
pub mod direccion;
pub mod lienzo;
pub mod motor;
pub mod paginas;
pub mod sin_ventana;
pub mod ventana;
