//! La app sin pantalla: el mismo `App` y el mismo motor, sin ventana. Sirve
//! para sacar capturas (`navegador --captura`) y para las pruebas de extremo
//! a extremo, sin ningún navegador externo.

use crate::app::{App, Entrada};
use crate::motor::{MensajeMotor, Motor};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;

pub struct Sesion {
    app: App,
    motor: Motor,
    mensajes: Receiver<MensajeMotor>,
    /// Si es verdadero, los errores del motor se apuntan en el registro.
    registrar: bool,
}

impl Sesion {
    /// Arranca el motor (`ejecutable --motor`) y la app.
    pub fn nueva(ejecutable: &Path, ancho: u32, alto: u32, inicial: Option<String>) -> std::io::Result<Self> {
        let (enviar, mensajes) = channel();
        let mut motor = Motor::lanzar(ejecutable, move |mensaje| {
            let _ = enviar.send(mensaje);
        })?;
        let (app, peticiones) = App::nueva(ancho, alto, inicial);
        for peticion in &peticiones {
            motor.enviar(peticion)?;
        }
        Ok(Self { app, motor, mensajes, registrar: false })
    }

    /// Apunta en el registro los errores del motor (las pruebas no lo piden para no ensuciar la carpeta de datos).
    #[must_use]
    pub fn con_registro(mut self) -> Self {
        self.registrar = true;
        self
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    /// Pasa una entrada a la app y manda al motor lo que pida.
    pub fn entrada(&mut self, entrada: Entrada) -> std::io::Result<()> {
        let salida = self.app.manejar(entrada);
        for peticion in &salida.peticiones {
            self.motor.enviar(peticion)?;
        }
        Ok(())
    }

    /// Atiende los mensajes del motor hasta que se cumpla `condicion` o pase
    /// `limite`. Devuelve si se cumplió.
    pub fn esperar(&mut self, limite: Duration, condicion: impl Fn(&App) -> bool) -> bool {
        let fin = Instant::now() + limite;
        loop {
            if condicion(&self.app) {
                return true;
            }
            let resto = fin.saturating_duration_since(Instant::now());
            if resto.is_zero() {
                return false;
            }
            match self.mensajes.recv_timeout(resto) {
                Ok(mensaje) => {
                    if self.registrar {
                        crate::registro::mensaje_del_motor(&mensaje, self.app.destino_en_curso().or(Some(self.app.url())).filter(|u| !u.is_empty()));
                    }
                    if self.entrada(Entrada::Motor(mensaje)).is_err() {
                        return condicion(&self.app);
                    }
                }
                Err(_) => return condicion(&self.app),
            }
        }
    }

    /// Espera a que el motor haya contestado todo lo pedido.
    pub fn esperar_reposo(&mut self, limite: Duration) -> bool {
        self.esperar(limite, |app| !app.ocupado())
    }

    pub fn componer(&mut self) -> Pixmap {
        self.app.componer()
    }

    pub fn cerrar(mut self) {
        self.motor.cerrar();
    }
}

/// `navegador --captura <png> [url]`: abre la dirección, espera a que cargue y
/// guarda la ventana entera (barra incluida) en un PNG.
pub fn capturar(ruta: &Path, url: Option<String>, ancho: u32, alto: u32, limite: Duration) -> Result<String, String> {
    let ejecutable = std::env::current_exe().map_err(|e| e.to_string())?;
    let inicio = Instant::now();
    let mut sesion = Sesion::nueva(&ejecutable, ancho, alto, url).map_err(|e| format!("no se pudo arrancar el motor: {e}"))?.con_registro();
    let listo = sesion.esperar_reposo(limite);
    let marco = sesion.componer();
    marco.save_png(ruta).map_err(|e| format!("no se pudo guardar {}: {e}", ruta.display()))?;
    let app = sesion.app();
    let resumen = format!(
        "{} | {} | {:.2} s{}{}",
        if app.url().is_empty() { "(sin página)" } else { app.url() },
        app.titulo(),
        inicio.elapsed().as_secs_f32(),
        if listo { "" } else { " | el motor no terminó a tiempo" },
        app.error_visible().map(|(_, m)| format!(" | error: {m}")).unwrap_or_default(),
    );
    sesion.cerrar();
    Ok(resumen)
}
