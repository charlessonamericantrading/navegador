//! El motor en su propio proceso, hablando NDJSON (ver
//! `engine_core::protocol`). La aplicación se lanza a sí misma con `--motor`:
//! si una página tumba el motor, la ventana sigue viva.
//!
//! La red, las cookies y el almacenamiento van en un tercer proceso, el broker
//! (`--broker`, ADR 0001): el motor, que interpreta contenido hostil, no abre
//! ni la red ni el perfil; se los pide al broker con un token de un solo uso.
//!
//! Un hilo lee las respuestas, decodifica la captura (base64 + PNG) fuera del
//! hilo de la ventana y las entrega con `al_recibir`.

use base64::Engine as _;
use engine_core::sandbox::{spawn_isolated_process, SandboxChild};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use tiny_skia::{IntSize, Pixmap};

/// Argumento con el que el ejecutable arranca como motor.
pub const ARGUMENTO_MOTOR: &str = "--motor";
/// Argumento con el que el ejecutable arranca como broker.
pub const ARGUMENTO_BROKER: &str = "--broker";
/// Cuánto se espera a que el broker conteste al arrancar.
const ESPERA_BROKER: std::time::Duration = std::time::Duration::from_secs(15);

/// Un elemento interactivo de la página (coordenadas de documento).
#[derive(Debug, Clone, PartialEq)]
pub struct Elemento {
    pub id: u32,
    pub etiqueta: String,
    pub tipo: Option<String>,
    pub x: f32,
    pub y: f32,
    pub ancho: f32,
    pub alto: f32,
}

impl Elemento {
    /// ¿Recibe el teclado? Un `<textarea>` y los `<input>` de texto, búsqueda,
    /// correo, contraseña, número...; no las casillas, botones ni ficheros.
    pub fn es_campo_de_texto(&self) -> bool {
        match self.etiqueta.as_str() {
            "textarea" => true,
            "input" => {
                let tipo = self.tipo.as_deref().unwrap_or("text").to_ascii_lowercase();
                !["checkbox", "radio", "button", "submit", "reset", "file", "image", "range", "color", "hidden"].contains(&tipo.as_str())
            }
            _ => false,
        }
    }

    pub fn contiene(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.ancho && y >= self.y && y < self.y + self.alto
    }
}

/// Lo que el motor muestra de la pestaña activa.
#[derive(Debug, Clone)]
pub struct Estado {
    pub id: Option<String>,
    pub pestana: u32,
    pub url: String,
    pub titulo: String,
    pub scroll_y: f32,
    pub captura: Option<Pixmap>,
    pub elementos: Option<Vec<Elemento>>,
    pub puede_atras: bool,
    pub puede_adelante: bool,
    pub enfocado_texto: Option<bool>,
    pub seleccion: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MensajeMotor {
    Listo,
    Estado(Box<Estado>),
    Pestanas { id: Option<String>, lista: Vec<(u32, String, String)>, activa: u32 },
    Error { id: Option<String>, mensaje: String },
    Hecho { id: Option<String> },
    /// El proceso del motor terminó (o su salida se cerró).
    Terminado,
    Otro,
}

impl MensajeMotor {
    /// El `id` de la petición a la que contesta (si contesta a alguna).
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Estado(estado) => estado.id.as_deref(),
            Self::Pestanas { id, .. } | Self::Error { id, .. } | Self::Hecho { id } => id.as_deref(),
            Self::Listo | Self::Terminado | Self::Otro => None,
        }
    }

    /// Interpreta una línea NDJSON del motor.
    pub fn desde_linea(linea: &str) -> Self {
        let Ok(valor) = serde_json::from_str::<Value>(linea) else { return Self::Otro };
        let texto = |clave: &str| valor.get(clave).and_then(Value::as_str).unwrap_or_default().to_string();
        let id = valor.get("id").and_then(Value::as_str).map(str::to_string);
        match valor.get("type").and_then(Value::as_str) {
            Some("ready") => Self::Listo,
            Some("state") => Self::Estado(Box::new(Estado {
                id,
                pestana: valor.get("tab_id").and_then(Value::as_u64).unwrap_or(0) as u32,
                url: texto("url"),
                titulo: texto("title"),
                scroll_y: valor.get("scroll_offset_y").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                captura: decodificar_captura(&texto("screenshot")),
                elementos: valor.get("elements").and_then(Value::as_array).map(|lista| lista.iter().filter_map(leer_elemento).collect()),
                puede_atras: valor.get("can_go_back").and_then(Value::as_bool).unwrap_or(false),
                puede_adelante: valor.get("can_go_forward").and_then(Value::as_bool).unwrap_or(false),
                enfocado_texto: valor.get("focused_is_text").and_then(Value::as_bool),
                seleccion: valor.get("selected_text").and_then(Value::as_str).map(str::to_string),
            })),
            Some("tabs") => Self::Pestanas {
                id,
                lista: valor
                    .get("tabs")
                    .and_then(Value::as_array)
                    .map(|lista| {
                        lista
                            .iter()
                            .map(|p| {
                                let campo = |c: &str| p.get(c).and_then(Value::as_str).unwrap_or_default().to_string();
                                (p.get("id").and_then(Value::as_u64).unwrap_or(0) as u32, campo("title"), campo("url"))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                activa: valor.get("active_tab_id").and_then(Value::as_u64).unwrap_or(0) as u32,
            },
            Some("error") => Self::Error { id, mensaje: texto("message") },
            Some("ok") => Self::Hecho { id },
            _ => Self::Otro,
        }
    }
}

fn leer_elemento(valor: &Value) -> Option<Elemento> {
    let rect = valor.get("rect")?;
    let num = |c: &str| rect.get(c).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    Some(Elemento {
        id: valor.get("id")?.as_u64()? as u32,
        etiqueta: valor.get("tag_name")?.as_str()?.to_ascii_lowercase(),
        tipo: valor.get("attributes").and_then(|a| a.get("element_type")).and_then(Value::as_str).map(str::to_string),
        x: num("x"),
        y: num("y"),
        ancho: num("width"),
        alto: num("height"),
    })
}

/// La captura del motor (PNG en base64) como píxeles listos para pintar.
pub fn decodificar_captura(base64: &str) -> Option<Pixmap> {
    if base64.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).ok()?;
    let imagen = engine_image::decode_image(&bytes)?;
    // La captura del motor es opaca (el lienzo se rellena entero), así que
    // RGBA y RGBA premultiplicado coinciden.
    Pixmap::from_vec(imagen.rgba.clone(), IntSize::from_wh(imagen.width, imagen.height)?)
}

/// El proceso broker y su canal de control.
pub struct Broker {
    hijo: Child,
    entrada: ChildStdin,
    lineas: std::sync::mpsc::Receiver<String>,
    endpoint: String,
}

impl Broker {
    /// Lanza `ejecutable --broker` y espera a que anuncie su canal.
    pub fn lanzar(ejecutable: &Path) -> std::io::Result<Self> {
        let mut hijo = Command::new(ejecutable).arg(ARGUMENTO_BROKER).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn()?;
        let entrada = hijo.stdin.take().ok_or_else(|| std::io::Error::other("el broker no tiene entrada"))?;
        let salida = hijo.stdout.take().ok_or_else(|| std::io::Error::other("el broker no tiene salida"))?;
        let (enviar, lineas) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("lector-broker".to_string()).spawn(move || {
            for linea in BufReader::new(salida).lines() {
                let Ok(linea) = linea else { break };
                if enviar.send(linea).is_err() {
                    break;
                }
            }
        })?;
        let mut broker = Self { hijo, entrada, lineas, endpoint: String::new() };
        let listo = broker.siguiente()?;
        broker.endpoint = listo
            .get("endpoint")
            .and_then(Value::as_str)
            .filter(|_| listo.get("type").and_then(Value::as_str) == Some("ready"))
            .ok_or_else(|| std::io::Error::other(format!("el broker no anunció su canal: {listo}")))?
            .to_string();
        Ok(broker)
    }

    /// Da de alta un proceso del motor y devuelve su token de un solo uso.
    pub fn registrar(&mut self, nombre: &str) -> std::io::Result<String> {
        let peticion = serde_json::json!({ "id": nombre, "type": "register", "renderer": nombre });
        writeln!(self.entrada, "{peticion}")?;
        self.entrada.flush()?;
        let respuesta = self.siguiente()?;
        respuesta
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| std::io::Error::other(format!("el broker no dio token: {respuesta}")))
    }

    fn siguiente(&self) -> std::io::Result<Value> {
        let linea = self.lineas.recv_timeout(ESPERA_BROKER).map_err(|_| std::io::Error::other("el broker no contestó"))?;
        serde_json::from_str(&linea).map_err(|e| std::io::Error::other(format!("respuesta del broker no válida: {e}")))
    }

    fn cerrar(&mut self) {
        let _ = writeln!(self.entrada, "{}", serde_json::json!({ "id": "cierre", "type": "shutdown" }));
        let _ = self.entrada.flush();
        for _ in 0..20 {
            if matches!(self.hijo.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.hijo.kill();
        let _ = self.hijo.wait();
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        if matches!(self.hijo.try_wait(), Ok(None)) {
            let _ = self.hijo.kill();
            let _ = self.hijo.wait();
        }
    }
}

/// El proceso del motor (y su broker). En Windows `--motor` corre con token
/// restringido a integridad baja y dentro de un *Job Object* (`SandboxChild`).
pub struct Motor {
    hijo: SandboxChild,
    entrada: File,
    broker: Option<Broker>,
}

impl Motor {
    /// Arranca el broker y el motor con `ejecutable` (`--broker` y `--motor`)
    /// y entrega cada mensaje del motor a `al_recibir` desde un hilo propio.
    pub fn lanzar(ejecutable: &Path, al_recibir: impl FnMut(MensajeMotor) + Send + 'static) -> std::io::Result<Self> {
        let mut broker = Broker::lanzar(ejecutable)?;
        let token = broker.registrar("motor-1")?;
        let entorno = [
            (engine_core::process::BROKER_ENDPOINT_ENV, broker.endpoint.clone()),
            (engine_core::process::BROKER_TOKEN_ENV, token),
        ];
        let mut motor = Self::lanzar_solo(ejecutable, &entorno, al_recibir)?;
        motor.broker = Some(broker);
        Ok(motor)
    }

    /// Solo el motor, con las variables de entorno dadas, aislado con `SandboxChild`.
    fn lanzar_solo(ejecutable: &Path, entorno: &[(&str, String)], mut al_recibir: impl FnMut(MensajeMotor) + Send + 'static) -> std::io::Result<Self> {
        let pares: Vec<(&str, &str)> = entorno.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut hijo = spawn_isolated_process(ejecutable, &[ARGUMENTO_MOTOR], &pares)?;
        let entrada = hijo.take_stdin().ok_or_else(|| std::io::Error::other("el motor no tiene entrada"))?;
        let salida = hijo.take_stdout().ok_or_else(|| std::io::Error::other("el motor no tiene salida"))?;
        std::thread::Builder::new().name("lector-motor".to_string()).spawn(move || {
            for linea in BufReader::new(salida).lines() {
                let Ok(linea) = linea else { break };
                if linea.trim().is_empty() {
                    continue;
                }
                al_recibir(MensajeMotor::desde_linea(&linea));
            }
            al_recibir(MensajeMotor::Terminado);
        })?;
        Ok(Self { hijo, entrada, broker: None })
    }

    /// Lanza el motor con este mismo ejecutable.
    pub fn lanzar_este(al_recibir: impl FnMut(MensajeMotor) + Send + 'static) -> std::io::Result<Self> {
        Self::lanzar(&std::env::current_exe()?, al_recibir)
    }

    pub fn enviar(&mut self, peticion: &Value) -> std::io::Result<()> {
        let mut linea = peticion.to_string();
        linea.push('\n');
        self.entrada.write_all(linea.as_bytes())?;
        self.entrada.flush()
    }

    /// Pide al motor (y luego al broker) que terminen y espera un poco; si
    /// no, los cierra.
    pub fn cerrar(&mut self) {
        let _ = self.enviar(&serde_json::json!({ "type": "shutdown", "id": "cierre" }));
        let mut terminado = false;
        for _ in 0..20 {
            if matches!(self.hijo.try_wait(), Ok(Some(_))) {
                terminado = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !terminado {
            let _ = self.hijo.kill();
            let _ = self.hijo.wait();
        }
        if let Some(broker) = &mut self.broker {
            broker.cerrar();
        }
    }
}

impl Drop for Motor {
    fn drop(&mut self) {
        if matches!(self.hijo.try_wait(), Ok(None)) {
            let _ = self.hijo.kill();
            let _ = self.hijo.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_un_estado_del_motor() {
        let linea = r#"{"type":"state","id":"a-1","renderer_status":"ready","tab_id":2,"scroll_offset_y":40.0,"url":"https://e.com/","title":"E","screenshot":"","elements":[{"id":3,"tag_name":"INPUT","text":"","rect":{"x":1.0,"y":2.0,"width":30.0,"height":10.0},"selector":"input","attributes":{"element_type":"search","checked":false}}],"can_go_back":true,"can_go_forward":false}"#;
        let MensajeMotor::Estado(estado) = MensajeMotor::desde_linea(linea) else { panic!("debía ser un estado") };
        assert_eq!((estado.id.as_deref(), estado.pestana, estado.url.as_str(), estado.scroll_y), (Some("a-1"), 2, "https://e.com/", 40.0));
        assert!(estado.captura.is_none());
        let elementos = estado.elementos.unwrap();
        assert_eq!(elementos.len(), 1);
        assert!(elementos[0].es_campo_de_texto());
        assert!(elementos[0].contiene(10.0, 5.0));
        assert!(estado.puede_atras && !estado.puede_adelante);
    }

    #[test]
    fn lee_pestanas_y_errores() {
        let MensajeMotor::Pestanas { lista, activa, .. } =
            MensajeMotor::desde_linea(r#"{"type":"tabs","id":null,"tabs":[{"id":0,"title":"A","url":"u"},{"id":4,"title":"","url":""}],"active_tab_id":4}"#)
        else {
            panic!("debían ser pestañas")
        };
        assert_eq!(lista, vec![(0, "A".to_string(), "u".to_string()), (4, String::new(), String::new())]);
        assert_eq!(activa, 4);
        assert!(matches!(MensajeMotor::desde_linea(r#"{"type":"error","id":"x","message":"m"}"#), MensajeMotor::Error { id: Some(_), .. }));
        assert!(matches!(MensajeMotor::desde_linea("no es json"), MensajeMotor::Otro));
    }

    #[test]
    fn decodifica_una_captura_png() {
        let mut pixmap = Pixmap::new(3, 2).unwrap();
        pixmap.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
        let png = pixmap.encode_png().unwrap();
        let captura = decodificar_captura(&base64::engine::general_purpose::STANDARD.encode(png)).unwrap();
        assert_eq!((captura.width(), captura.height()), (3, 2));
        let p = captura.pixel(1, 1).unwrap();
        assert_eq!((p.red(), p.green(), p.blue()), (10, 20, 30));
    }

    #[test]
    fn que_recibe_el_teclado() {
        let campo = |etiqueta: &str, tipo: Option<&str>| Elemento { id: 0, etiqueta: etiqueta.into(), tipo: tipo.map(Into::into), x: 0.0, y: 0.0, ancho: 1.0, alto: 1.0 };
        assert!(campo("input", None).es_campo_de_texto());
        assert!(campo("input", Some("password")).es_campo_de_texto());
        assert!(campo("textarea", None).es_campo_de_texto());
        assert!(!campo("input", Some("checkbox")).es_campo_de_texto());
        assert!(!campo("a", None).es_campo_de_texto());
    }
}
