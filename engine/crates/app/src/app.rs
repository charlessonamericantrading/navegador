//! Toda la lógica de la interfaz, sin ventana: recibe entradas (clic, rueda,
//! teclas, tamaño, mensajes del motor), decide qué pedir al motor y compone
//! la imagen de la ventana (barra + página).

use crate::barra::{self, Accion, BarraPintada, Pestana, VistaBarra, ALTO_BARRA};
use crate::datos::{DatosUsuario, Sugerencia};
use crate::lienzo::{self, Lienzo};
use crate::motor::{Elemento, Estado, MensajeMotor};
use crate::portapapeles::Portapapeles;
use crate::{direccion, paginas};
use engine_layout::ImageMap;
use engine_text::FontSet;
use serde_json::{json, Value};
use std::collections::HashMap;
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};

/// Píxeles por línea de la rueda del ratón.
pub const PIXELES_POR_LINEA: f32 = 48.0;
/// Paso de las flechas arriba/abajo.
const PASO_FLECHA: f32 = 40.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tecla {
    /// Un carácter escrito (ya con mayúsculas, acentos...).
    Caracter(String),
    Enter,
    Escape,
    Retroceso,
    Suprimir,
    Izquierda,
    Derecha,
    Arriba,
    Abajo,
    Inicio,
    Fin,
    AvPag,
    RePag,
    Tab,
    F5,
    F6,
    Otra,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modificadores {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorTipo {
    Normal,
    Mano,
    Texto,
}

#[derive(Debug, Clone)]
pub enum Entrada {
    /// Clic izquierdo en coordenadas de la ventana.
    Clic { x: f32, y: f32 },
    /// Arrastre sobre la barra de pestañas para reordenarlas.
    ArrastrarBarra { x: f32, y: f32 },
    SoltarBarra,
    /// Movimiento del puntero en coordenadas de la ventana.
    Raton { x: f32, y: f32 },
    /// Selección con arrastre dentro del área de página.
    SeleccionarPagina { desde_x: f32, desde_y: f32, hasta_x: f32, hasta_y: f32 },
    /// Rueda: píxeles hacia abajo (negativo, hacia arriba).
    Rueda { dy: f32 },
    Tecla { tecla: Tecla, mods: Modificadores },
    Tamano { ancho: u32, alto: u32 },
    Escala(f32),
    Motor(MensajeMotor),
}

/// Lo que hay que hacer tras una entrada.
#[derive(Debug, Default)]
pub struct Salida {
    pub peticiones: Vec<Value>,
    pub redibujar: bool,
}

#[derive(Debug, Clone)]
struct Carga {
    id: String,
    /// Adónde se va ("" si no se sabe: atrás, adelante, un enlace).
    url: String,
}

#[derive(Debug, Clone)]
struct Campo {
    texto: String,
    cursor: usize,
    todo: bool,
}

struct Desplegable {
    sugerencias: Vec<Sugerencia>,
    seleccionada: Option<usize>,
    ancho: u32,
    escala_cent: u32,
    lienzo: Lienzo,
}

pub struct App {
    ancho: u32,
    alto: u32,
    escala: f32,
    pestanas: Vec<Pestana>,
    activa: u32,
    pagina: Option<Pixmap>,
    url: String,
    titulo: String,
    scroll_y: f32,
    elementos: Vec<Elemento>,
    puede_atras: bool,
    puede_adelante: bool,
    carga: Option<Carga>,
    /// Error de carga por pestaña: (url, mensaje).
    errores: HashMap<u32, (String, String)>,
    campo: Option<Campo>,
    /// ¿Las teclas van a un campo de texto de la página?
    escribiendo: bool,
    seleccion_pagina: Option<String>,
    hover_en_vuelo: Option<String>,
    hover_pendiente: Option<(f32, f32)>,
    ultimo_hover: Option<(f32, f32)>,
    scroll_en_vuelo: Option<String>,
    scroll_pendiente: f32,
    tamano_en_vuelo: Option<String>,
    tamano_pendiente: bool,
    siguiente_id: u64,
    /// Peticiones mandadas que el motor aún no ha contestado.
    en_vuelo: std::collections::HashSet<String>,
    fuentes: FontSet,
    iconos: ImageMap,
    barra: Option<(VistaBarra, u32, u32, BarraPintada)>,
    local: Option<(String, u32, u32, u32, Lienzo)>,
    mostrando_local: bool,
    portapapeles: Portapapeles,
    datos: DatosUsuario,
    sugerencias: Vec<Sugerencia>,
    sugerencia_seleccionada: Option<usize>,
    mostrar_sugerencias: bool,
    desplegable: Option<Desplegable>,
    desplazamiento_pestanas: usize,
    orden_pestanas: Vec<u32>,
    arrastrando_pestana: Option<u32>,
}

impl App {
    /// Crea la app a escala 1x y devuelve lo primero que hay que pedir al motor.
    pub fn nueva(ancho: u32, alto: u32, inicial: Option<String>) -> (Self, Vec<Value>) {
        Self::nueva_con_escala(ancho, alto, 1.0, inicial)
    }

    /// Crea la app con la escala física de pantalla (`1.25`, `2.0`...) para
    /// rasterizar textos, iconos y páginas con nitidez nativa.
    pub fn nueva_con_escala(ancho: u32, alto: u32, escala: f32, inicial: Option<String>) -> (Self, Vec<Value>) {
        let escala = normalizar_escala(escala);
        let mut app = Self {
            ancho: ancho.max(1),
            alto: alto.max(1),
            escala,
            pestanas: vec![Pestana { id: 0, titulo: String::new(), url: String::new() }],
            activa: 0,
            pagina: None,
            url: String::new(),
            titulo: String::new(),
            scroll_y: 0.0,
            elementos: Vec::new(),
            puede_atras: false,
            puede_adelante: false,
            carga: None,
            errores: HashMap::new(),
            campo: None,
            escribiendo: false,
            seleccion_pagina: None,
            hover_en_vuelo: None,
            hover_pendiente: None,
            ultimo_hover: None,
            scroll_en_vuelo: None,
            scroll_pendiente: 0.0,
            tamano_en_vuelo: None,
            tamano_pendiente: false,
            siguiente_id: 0,
            en_vuelo: std::collections::HashSet::new(),
            fuentes: FontSet::load_default_sans_serif(),
            iconos: barra::iconos_con_escala(escala),
            barra: None,
            local: None,
            mostrando_local: false,
            portapapeles: Portapapeles::Sistema,
            #[cfg(test)]
            datos: DatosUsuario::en_memoria(),
            #[cfg(not(test))]
            datos: DatosUsuario::cargar(),
            sugerencias: Vec::new(),
            sugerencia_seleccionada: None,
            mostrar_sugerencias: false,
            desplegable: None,
            desplazamiento_pestanas: 0,
            orden_pestanas: vec![0],
            arrastrando_pestana: None,
        };
        let mut salida = Salida::default();
        app.pedir_tamano(&mut salida);
        let id = app.nuevo_id("app");
        salida.peticiones.push(json!({ "type": "list_tabs", "id": id }));
        match inicial.as_deref().and_then(direccion::resolver) {
            Some(url) => app.navegar(url, &mut salida),
            None => app.empezar_a_editar(String::new()),
        }
        app.apuntar(&salida);
        (app, salida.peticiones)
    }

    /// Apunta las peticiones que salen para saber cuándo están todas contestadas.
    fn apuntar(&mut self, salida: &Salida) {
        for peticion in &salida.peticiones {
            if let Some(id) = peticion.get("id").and_then(Value::as_str) {
                self.en_vuelo.insert(id.to_string());
            }
        }
    }

    /// ¿Queda alguna petición sin respuesta del motor?
    pub fn ocupado(&self) -> bool {
        !self.en_vuelo.is_empty()
    }

    // --- Consultas (para la ventana y las pruebas) ---

    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn titulo(&self) -> &str {
        &self.titulo
    }
    pub fn cargando(&self) -> bool {
        self.carga.is_some()
    }
    pub fn editando_direccion(&self) -> Option<&str> {
        self.campo.as_ref().map(|c| c.texto.as_str())
    }
    pub fn escribiendo_en_pagina(&self) -> bool {
        self.escribiendo
    }
    pub fn pestanas(&self) -> &[Pestana] {
        &self.pestanas
    }
    pub fn pestana_activa(&self) -> u32 {
        self.activa
    }
    pub fn error_visible(&self) -> Option<&(String, String)> {
        self.errores.get(&self.activa)
    }
    pub fn tiene_pagina(&self) -> bool {
        self.pagina.is_some() && !self.url.is_empty()
    }
    pub fn elementos(&self) -> &[Elemento] {
        &self.elementos
    }
    pub fn scroll_y(&self) -> f32 {
        self.scroll_y
    }
    pub fn escala(&self) -> f32 {
        self.escala
    }
    pub fn datos(&self) -> &DatosUsuario {
        &self.datos
    }
    pub fn datos_mut(&mut self) -> &mut DatosUsuario {
        &mut self.datos
    }
    pub fn sugerencias(&self) -> &[Sugerencia] {
        &self.sugerencias
    }
    pub fn cursor_en(&self, x: f32, y: f32) -> CursorTipo {
        if let Some((ox, oy, _, l)) = self.desplegable_en_pantalla() {
            if x >= ox && y >= oy && x < ox + l.layout.dimensions.width && y < oy + l.layout.dimensions.height {
                return if lienzo::accion_en(&l.regiones, x - ox, y - oy).is_some() {
                    CursorTipo::Mano
                } else {
                    CursorTipo::Normal
                };
            }
        }
        if y < ALTO_BARRA as f32 {
            return match self.barra.as_ref().and_then(|(_, _, _, b)| b.accion_en(x, y)) {
                Some(Accion::Direccion) => CursorTipo::Texto,
                Some(Accion::Atras) => if self.puede_atras { CursorTipo::Mano } else { CursorTipo::Normal },
                Some(Accion::Adelante) => if self.puede_adelante { CursorTipo::Mano } else { CursorTipo::Normal },
                Some(Accion::Marcador) => if !self.url.is_empty() { CursorTipo::Mano } else { CursorTipo::Normal },
                Some(_) => CursorTipo::Mano,
                None => CursorTipo::Normal,
            };
        }
        let (cx, cy) = (x, y - ALTO_BARRA as f32);
        if self.mostrando_local {
            return if self.local.as_ref().and_then(|(_, _, _, _, l)| lienzo::accion_en(&l.regiones, cx, cy)).is_some() {
                CursorTipo::Mano
            } else {
                CursorTipo::Normal
            };
        }
        if self.carga.is_some() || !self.tiene_pagina() {
            return CursorTipo::Normal;
        }
        match self.elementos.iter().rev().find(|e| e.contiene(cx, cy + self.scroll_y)) {
            Some(el) if el.es_campo_de_texto() => CursorTipo::Texto,
            Some(_) => CursorTipo::Mano,
            None => CursorTipo::Normal,
        }
    }
    pub fn titulo_ventana(&self) -> String {
        if self.titulo.trim().is_empty() {
            "Navegador".to_string()
        } else {
            format!("{} — Navegador", self.titulo.trim())
        }
    }

    fn alto_contenido(&self) -> u32 {
        self.alto.saturating_sub(ALTO_BARRA).max(1)
    }

    fn nuevo_id(&mut self, prefijo: &str) -> String {
        self.siguiente_id += 1;
        format!("{prefijo}-{}", self.siguiente_id)
    }

    fn desplegable_en_pantalla(&self) -> Option<(f32, f32, u32, &Lienzo)> {
        if self.campo.is_none() || !self.mostrar_sugerencias || self.sugerencias.is_empty() {
            return None;
        }
        let d = self.desplegable.as_ref()?;
        let (ox, oy, _) = self.caja_desplegable_css()?;
        Some((ox, oy, 0, &d.lienzo))
    }

    fn caja_desplegable_css(&self) -> Option<(f32, f32, u32)> {
        let dir = self
            .barra
            .as_ref()
            .and_then(|(_, _, _, b)| b.lienzo.regiones.iter().find(|r| r.accion == "direccion"))
            .map(|r| r.rect.clone());
        let (x, ancho) = match dir {
            Some(r) => (r.x, (r.width.round() as u32).max(200)),
            None => (120.0, self.ancho.saturating_sub(170).max(200)),
        };
        Some((x, ALTO_BARRA as f32 - 2.0, ancho))
    }

    fn cerrar_campo(&mut self) {
        self.campo = None;
        self.sugerencias.clear();
        self.sugerencia_seleccionada = None;
        self.mostrar_sugerencias = false;
        self.desplegable = None;
    }

    fn actualizar_sugerencias(&mut self, mostrar_vacio: bool) {
        let consulta = self.campo.as_ref().map(|c| c.texto.clone()).unwrap_or_default();
        if consulta.trim().is_empty() && !mostrar_vacio {
            self.sugerencias.clear();
            self.sugerencia_seleccionada = None;
            self.mostrar_sugerencias = false;
            return;
        }
        self.sugerencias = self.datos.sugerir(&consulta, 6);
        self.sugerencia_seleccionada = None;
        self.mostrar_sugerencias = !self.sugerencias.is_empty();
    }

    fn asegurar_pestana_visible(&mut self) {
        let Some(pos) = self.pestanas.iter().position(|p| p.id == self.activa) else { return };
        let (ini, fin, _, _) = barra::ventana_pestanas(self.ancho, self.pestanas.len(), self.desplazamiento_pestanas);
        if pos < ini {
            self.desplazamiento_pestanas = pos;
        } else if pos >= fin {
            let mostradas = fin.saturating_sub(ini).max(1);
            self.desplazamiento_pestanas = pos + 1 - mostradas;
        }
    }

    fn sincronizar_pestanas(&mut self, lista: Vec<(u32, String, String)>) {
        self.orden_pestanas.retain(|id| lista.iter().any(|(lid, _, _)| lid == id));
        for &(id, _, _) in &lista {
            if !self.orden_pestanas.contains(&id) {
                self.orden_pestanas.push(id);
            }
        }
        let mut ordenadas = Vec::with_capacity(lista.len());
        for &id in &self.orden_pestanas {
            if let Some((_, titulo, url)) = lista.iter().find(|(lid, _, _)| *lid == id) {
                ordenadas.push(Pestana { id, titulo: titulo.clone(), url: url.clone() });
            }
        }
        if ordenadas.is_empty() {
            ordenadas.push(Pestana { id: self.activa, titulo: String::new(), url: String::new() });
        }
        self.pestanas = ordenadas;
        self.asegurar_pestana_visible();
    }

    // --- Entradas ---

    pub fn manejar(&mut self, entrada: Entrada) -> Salida {
        let mut salida = Salida::default();
        match entrada {
            Entrada::Clic { x, y } => self.clic(x, y, &mut salida),
            Entrada::ArrastrarBarra { x, y } => self.arrastrar_barra(x, y, &mut salida),
            Entrada::SoltarBarra => self.arrastrando_pestana = None,
            Entrada::Raton { x, y } => self.mover_raton(x, y, &mut salida),
            Entrada::SeleccionarPagina { desde_x, desde_y, hasta_x, hasta_y } => {
                self.seleccionar_pagina(desde_x, desde_y, hasta_x, hasta_y, &mut salida)
            }
            Entrada::Rueda { dy } => self.desplazar(dy, &mut salida),
            Entrada::Tecla { tecla, mods } => self.tecla(tecla, mods, &mut salida),
            Entrada::Tamano { ancho, alto } => {
                let (ancho, alto) = (ancho.max(1), alto.max(1));
                if (ancho, alto) != (self.ancho, self.alto) {
                    let antes = (self.ancho, self.alto_contenido());
                    self.ancho = ancho;
                    self.alto = alto;
                    if antes != (self.ancho, self.alto_contenido()) {
                        self.pedir_tamano(&mut salida);
                    }
                    salida.redibujar = true;
                }
            }
            Entrada::Escala(nueva) => {
                let nueva = normalizar_escala(nueva);
                if (nueva - self.escala).abs() > 0.001 {
                    self.escala = nueva;
                    self.iconos = barra::iconos_con_escala(self.escala);
                    self.barra = None;
                    self.local = None;
                    self.pedir_tamano(&mut salida);
                    salida.redibujar = true;
                }
            }
            Entrada::Motor(mensaje) => {
                if let Some(id) = mensaje.id() {
                    self.en_vuelo.remove(id);
                }
                if matches!(mensaje, MensajeMotor::Terminado) {
                    self.en_vuelo.clear();
                }
                self.mensaje(mensaje, &mut salida)
            }
        }
        self.apuntar(&salida);
        salida
    }

    fn clic(&mut self, x: f32, y: f32, salida: &mut Salida) {
        salida.redibujar = true;
        if let Some((ox, oy, _, l)) = self.desplegable_en_pantalla() {
            let in_box = x >= ox && y >= oy && x < ox + l.layout.dimensions.width && y < oy + l.layout.dimensions.height;
            if in_box {
                if let Some(Accion::Sugerencia(idx)) = lienzo::accion_en(&l.regiones, x - ox, y - oy).and_then(Accion::desde) {
                    if let Some(sug) = self.sugerencias.get(idx).cloned() {
                        self.cerrar_campo();
                        self.navegar(sug.url, salida);
                        return;
                    }
                }
                return;
            }
        }
        if y < ALTO_BARRA as f32 {
            let accion = self.barra.as_ref().and_then(|(_, _, _, b)| b.accion_en(x, y));
            match accion {
                Some(Accion::Direccion) => {
                    let pintado = self.barra.as_ref().and_then(|(_, _, _, b)| b.campo.clone());
                    match (&mut self.campo, pintado) {
                        (Some(campo), Some(pintado)) => {
                            campo.cursor = barra::cursor_en_x(&pintado, x, &self.fuentes).min(campo.texto.chars().count());
                            campo.todo = false;
                        }
                        _ => self.empezar_a_editar(self.url_mostrada()),
                    }
                }
                Some(Accion::Pestana(id)) => {
                    self.cerrar_campo();
                    self.arrastrando_pestana = Some(id);
                    self.ejecutar(Accion::Pestana(id), salida);
                }
                Some(accion) => {
                    self.cerrar_campo();
                    self.ejecutar(accion, salida);
                }
                None => self.cerrar_campo(),
            }
            return;
        }

        self.cerrar_campo();
        let (cx, cy) = (x, y - ALTO_BARRA as f32);
        if self.mostrando_local {
            let accion = self.local.as_ref().and_then(|(_, _, _, _, l)| lienzo::accion_en(&l.regiones, cx, cy)).map(str::to_string);
            match accion.as_deref() {
                Some("reintentar") => self.ejecutar(Accion::Recargar, salida),
                Some(destino) if destino.starts_with("ir:") => self.navegar(destino[3..].to_string(), salida),
                _ => {}
            }
            return;
        }
        // Mientras carga, el motor está ocupado: un clic esperaría y caería
        // sobre la página nueva, en un sitio que el usuario no ha visto.
        if self.carga.is_some() || !self.tiene_pagina() {
            return;
        }
        let elemento = self.elementos.iter().rev().find(|e| e.contiene(cx, cy + self.scroll_y)).cloned();
        self.escribiendo = elemento.as_ref().is_some_and(Elemento::es_campo_de_texto);
        let id = if elemento.as_ref().is_some_and(puede_navegar) {
            let id = self.nuevo_id("nav");
            self.carga = Some(Carga { id: id.clone(), url: String::new() });
            id
        } else {
            self.nuevo_id("app")
        };
        salida.peticiones.push(json!({ "type": "click", "id": id, "x": cx, "y": cy }));
    }

    fn arrastrar_barra(&mut self, x: f32, _y: f32, salida: &mut Salida) {
        let Some(id_origen) = self.arrastrando_pestana else { return };
        let Some(Accion::Pestana(id_destino)) = self.barra.as_ref().and_then(|(_, _, _, b)| b.accion_en(x, 15.0)) else {
            return;
        };
        if id_origen == id_destino {
            return;
        }
        let (Some(i), Some(j)) = (
            self.pestanas.iter().position(|p| p.id == id_origen),
            self.pestanas.iter().position(|p| p.id == id_destino),
        ) else {
            return;
        };
        let movida = self.pestanas.remove(i);
        self.pestanas.insert(j, movida);
        self.orden_pestanas = self.pestanas.iter().map(|p| p.id).collect();
        self.asegurar_pestana_visible();
        salida.redibujar = true;
    }

    fn mover_raton(&mut self, x: f32, y: f32, salida: &mut Salida) {
        if y < ALTO_BARRA as f32 || self.mostrando_local || self.carga.is_some() || !self.tiene_pagina() {
            return;
        }
        let (cx, cy) = (x, y - ALTO_BARRA as f32);
        if self.hover_en_vuelo.is_some() {
            self.hover_pendiente = Some((cx, cy));
            return;
        }
        if let Some((lx, ly)) = self.ultimo_hover {
            if (lx - cx).abs() < 4.0 && (ly - cy).abs() < 4.0 {
                return;
            }
        }
        self.ultimo_hover = Some((cx, cy));
        let id = self.nuevo_id("hover");
        self.hover_en_vuelo = Some(id.clone());
        salida.peticiones.push(json!({ "type": "hover", "id": id, "x": cx, "y": cy }));
    }

    fn seleccionar_pagina(&mut self, desde_x: f32, desde_y: f32, hasta_x: f32, hasta_y: f32, salida: &mut Salida) {
        if self.mostrando_local || self.carga.is_some() || !self.tiene_pagina() {
            return;
        }
        self.campo = None;
        let fy = (desde_y - ALTO_BARRA as f32).max(0.0);
        let ty = (hasta_y - ALTO_BARRA as f32).max(0.0);
        let id = self.nuevo_id("sel");
        salida.peticiones.push(json!({
            "type": "select_text",
            "id": id,
            "from_x": desde_x,
            "from_y": fy,
            "to_x": hasta_x,
            "to_y": ty
        }));
    }

    fn desplazar(&mut self, dy: f32, salida: &mut Salida) {
        if dy == 0.0 || self.carga.is_some() || !self.tiene_pagina() || self.mostrando_local {
            return;
        }
        // Una sola petición de scroll en vuelo: lo que llega mientras tanto se
        // suma y sale junto en cuanto el motor contesta.
        if self.scroll_en_vuelo.is_some() {
            self.scroll_pendiente += dy;
            return;
        }
        let id = self.nuevo_id("scroll");
        self.scroll_en_vuelo = Some(id.clone());
        salida.peticiones.push(json!({ "type": "scroll", "id": id, "dx": 0, "dy": dy.round() as i32 }));
    }

    fn tecla(&mut self, tecla: Tecla, mods: Modificadores, salida: &mut Salida) {
        salida.redibujar = true;
        let letra = |c: &str| matches!(&tecla, Tecla::Caracter(t) if t.eq_ignore_ascii_case(c));

        // Atajos que valen siempre.
        if (mods.ctrl && letra("l")) || tecla == Tecla::F6 || (mods.alt && letra("d")) {
            self.empezar_a_editar(self.url_mostrada());
            return;
        }
        if mods.ctrl && letra("t") {
            return self.ejecutar(Accion::NuevaPestana, salida);
        }
        if mods.ctrl && letra("w") {
            return self.ejecutar(Accion::CerrarPestana(self.activa), salida);
        }
        if mods.ctrl && letra("d") {
            return self.ejecutar(Accion::Marcador, salida);
        }
        if mods.ctrl && mods.shift && matches!(tecla, Tecla::RePag | Tecla::AvPag) {
            if let Some(pos) = self.pestanas.iter().position(|p| p.id == self.activa) {
                let n = self.pestanas.len();
                if n > 1 {
                    let destino = if tecla == Tecla::RePag { pos.saturating_sub(1) } else { (pos + 1).min(n - 1) };
                    if destino != pos {
                        let movida = self.pestanas.remove(pos);
                        self.pestanas.insert(destino, movida);
                        self.orden_pestanas = self.pestanas.iter().map(|p| p.id).collect();
                        self.asegurar_pestana_visible();
                    }
                }
            }
            return;
        }
        if (mods.ctrl && letra("r")) || tecla == Tecla::F5 {
            return self.ejecutar(Accion::Recargar, salida);
        }
        if mods.alt && tecla == Tecla::Izquierda {
            return self.ejecutar(Accion::Atras, salida);
        }
        if mods.alt && tecla == Tecla::Derecha {
            return self.ejecutar(Accion::Adelante, salida);
        }
        if mods.ctrl && tecla == Tecla::Tab {
            if let Some(posicion) = self.pestanas.iter().position(|p| p.id == self.activa) {
                let n = self.pestanas.len();
                let otra = if mods.shift { (posicion + n - 1) % n } else { (posicion + 1) % n };
                let id = self.pestanas[otra].id;
                self.ejecutar(Accion::Pestana(id), salida);
            }
            return;
        }

        if self.campo.is_some() {
            return self.editar(tecla, mods, salida);
        }
        if tecla == Tecla::Escape && self.carga.is_some() {
            return self.ejecutar(Accion::Detener, salida);
        }
        if mods.ctrl && !mods.alt && self.tiene_pagina() && !self.mostrando_local {
            if letra("c") {
                if let Some(texto) = self.seleccion_pagina.as_deref() {
                    self.portapapeles.escribir(texto);
                }
                return;
            }
            if self.escribiendo && letra("x") {
                if let Some(texto) = self.seleccion_pagina.as_deref() {
                    self.portapapeles.escribir(texto);
                }
                let id = self.nuevo_id("app");
                salida.peticiones.push(json!({ "type": "press_key", "id": id, "key": "Ctrl+x" }));
                return;
            }
            if self.escribiendo && letra("a") {
                let id = self.nuevo_id("app");
                salida.peticiones.push(json!({ "type": "press_key", "id": id, "key": "Ctrl+a" }));
                return;
            }
            if self.escribiendo && letra("v") {
                if let Some(texto) = self.portapapeles.leer() {
                    for ch in texto.replace("\r\n", "\n").replace('\r', "\n").chars().take(2000) {
                        if ch == '\n' {
                            let id = self.nuevo_id("app");
                            salida.peticiones.push(json!({ "type": "press_key", "id": id, "key": "Enter" }));
                        } else if !ch.is_control() {
                            let id = self.nuevo_id("app");
                            salida.peticiones.push(json!({ "type": "press_key", "id": id, "key": ch.to_string() }));
                        }
                    }
                }
                return;
            }
        }
        if mods.ctrl || mods.alt || !self.tiene_pagina() || self.mostrando_local {
            return;
        }

        if tecla == Tecla::Tab {
            let clave = if mods.shift { "Shift+Tab" } else { "Tab" };
            let id = self.nuevo_id("app");
            salida.peticiones.push(json!({ "type": "press_key", "id": id, "key": clave }));
            return;
        }

        if self.escribiendo {
            let prefijo = if mods.shift { "Shift+" } else { "" };
            let clave = match &tecla {
                Tecla::Caracter(c) => c.clone(),
                Tecla::Retroceso => "Backspace".to_string(),
                Tecla::Suprimir => "Delete".to_string(),
                Tecla::Izquierda => format!("{prefijo}ArrowLeft"),
                Tecla::Derecha => format!("{prefijo}ArrowRight"),
                Tecla::Inicio => format!("{prefijo}Home"),
                Tecla::Fin => format!("{prefijo}End"),
                Tecla::Enter => "Enter".to_string(),
                Tecla::Escape => {
                    self.escribiendo = false;
                    return;
                }
                _ => return,
            };
            // Enter puede enviar un formulario: cuenta como carga.
            let id = if tecla == Tecla::Enter {
                let id = self.nuevo_id("nav");
                self.carga = Some(Carga { id: id.clone(), url: String::new() });
                id
            } else {
                self.nuevo_id("app")
            };
            salida.peticiones.push(json!({ "type": "press_key", "id": id, "key": clave }));
            return;
        }

        let pagina = self.alto_contenido() as f32 * 0.9;
        let dy = match &tecla {
            Tecla::Caracter(c) if c == " " => {
                if mods.shift {
                    -pagina
                } else {
                    pagina
                }
            }
            Tecla::AvPag => pagina,
            Tecla::RePag => -pagina,
            Tecla::Abajo => PASO_FLECHA,
            Tecla::Arriba => -PASO_FLECHA,
            Tecla::Inicio => -self.scroll_y - 1.0,
            Tecla::Fin => 1.0e7,
            _ => return,
        };
        self.desplazar(dy, salida);
    }

    fn editar(&mut self, tecla: Tecla, mods: Modificadores, salida: &mut Salida) {
        let mut editar_sugerencias = false;
        let mut navegar_destino: Option<String> = None;
        let mut cerrar = false;
        {
            let Some(campo) = &mut self.campo else { return };
            let largo = campo.texto.chars().count();
            let quitar_todo = |campo: &mut Campo| {
                campo.texto.clear();
                campo.cursor = 0;
                campo.todo = false;
            };
            match tecla {
                Tecla::Caracter(c) if mods.ctrl && c.eq_ignore_ascii_case("a") => {
                    campo.todo = true;
                    campo.cursor = largo;
                }
                Tecla::Caracter(c) if mods.ctrl && c.eq_ignore_ascii_case("c") => {
                    if campo.todo && !campo.texto.is_empty() {
                        self.portapapeles.escribir(&campo.texto);
                    }
                }
                Tecla::Caracter(c) if mods.ctrl && c.eq_ignore_ascii_case("x") => {
                    if campo.todo && !campo.texto.is_empty() {
                        self.portapapeles.escribir(&campo.texto);
                        quitar_todo(campo);
                        editar_sugerencias = true;
                    }
                }
                Tecla::Caracter(c) if mods.ctrl && c.eq_ignore_ascii_case("v") => {
                    if let Some(pegado) = self.portapapeles.leer() {
                        let limpio: String = pegado.chars().map(|ch| if ch == '\r' || ch == '\n' { ' ' } else { ch }).filter(|ch| !ch.is_control()).collect();
                        if !limpio.is_empty() {
                            if campo.todo {
                                quitar_todo(campo);
                            }
                            let posicion = byte_de(&campo.texto, campo.cursor);
                            campo.texto.insert_str(posicion, &limpio);
                            campo.cursor += limpio.chars().count();
                            editar_sugerencias = true;
                        }
                    }
                }
                Tecla::Caracter(c) if !mods.ctrl && !mods.alt => {
                    if campo.todo {
                        quitar_todo(campo);
                    }
                    let posicion = byte_de(&campo.texto, campo.cursor);
                    campo.texto.insert_str(posicion, &c);
                    campo.cursor += c.chars().count();
                    editar_sugerencias = true;
                }
                Tecla::Retroceso => {
                    if campo.todo {
                        quitar_todo(campo);
                    } else if campo.cursor > 0 {
                        let inicio = byte_de(&campo.texto, campo.cursor - 1);
                        let fin = byte_de(&campo.texto, campo.cursor);
                        campo.texto.replace_range(inicio..fin, "");
                        campo.cursor -= 1;
                    }
                    editar_sugerencias = true;
                }
                Tecla::Suprimir => {
                    if campo.todo {
                        quitar_todo(campo);
                    } else if campo.cursor < largo {
                        let inicio = byte_de(&campo.texto, campo.cursor);
                        let fin = byte_de(&campo.texto, campo.cursor + 1);
                        campo.texto.replace_range(inicio..fin, "");
                    }
                    editar_sugerencias = true;
                }
                Tecla::Izquierda => {
                    campo.cursor = if campo.todo { 0 } else { campo.cursor.saturating_sub(1) };
                    campo.todo = false;
                }
                Tecla::Derecha => {
                    campo.cursor = if campo.todo { largo } else { (campo.cursor + 1).min(largo) };
                    campo.todo = false;
                }
                Tecla::Inicio => {
                    campo.cursor = 0;
                    campo.todo = false;
                }
                Tecla::Fin => {
                    campo.cursor = largo;
                    campo.todo = false;
                }
                Tecla::Abajo | Tecla::Arriba => {
                    if !self.mostrar_sugerencias {
                        self.actualizar_sugerencias(true);
                    }
                    if !self.sugerencias.is_empty() {
                        let n = self.sugerencias.len();
                        self.sugerencia_seleccionada = Some(match self.sugerencia_seleccionada {
                            Some(i) if tecla == Tecla::Abajo => (i + 1) % n,
                            Some(i) => (i + n - 1) % n,
                            None if tecla == Tecla::Abajo => 0,
                            None => n - 1,
                        });
                    }
                }
                Tecla::Enter => {
                    if let Some(sug) = self.sugerencia_seleccionada.and_then(|i| self.sugerencias.get(i)) {
                        navegar_destino = Some(sug.url.clone());
                    } else if let Some(url) = direccion::resolver(&campo.texto) {
                        navegar_destino = Some(url);
                    }
                }
                Tecla::Escape | Tecla::Tab => cerrar = true,
                _ => {}
            }
        }
        if editar_sugerencias {
            self.actualizar_sugerencias(false);
        }
        if let Some(url) = navegar_destino {
            self.cerrar_campo();
            self.navegar(url, salida);
        } else if cerrar {
            self.cerrar_campo();
        }
    }

    fn ejecutar(&mut self, accion: Accion, salida: &mut Salida) {
        match accion {
            Accion::Atras | Accion::Adelante => {
                let (puede, tipo) = if accion == Accion::Atras { (self.puede_atras, "back") } else { (self.puede_adelante, "forward") };
                if puede && self.carga.is_none() {
                    let id = self.nuevo_id("nav");
                    self.carga = Some(Carga { id: id.clone(), url: String::new() });
                    self.escribiendo = false;
                    salida.peticiones.push(json!({ "type": tipo, "id": id }));
                }
            }
            Accion::Recargar => {
                let destino = self.errores.get(&self.activa).map(|(url, _)| url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| self.url.clone());
                if !destino.is_empty() {
                    self.navegar(destino, salida);
                }
            }
            Accion::Detener => {
                if self.carga.take().is_some() {
                    let id = self.nuevo_id("app");
                    salida.peticiones.push(json!({ "type": "stop", "id": id }));
                }
            }
            Accion::Direccion => self.empezar_a_editar(self.url_mostrada()),
            Accion::Marcador => {
                let url = self.url.clone();
                if !url.is_empty() {
                    self.datos.alternar_marcador(&url, &self.titulo);
                    self.local = None;
                    salida.redibujar = true;
                }
            }
            Accion::NuevaPestana => {
                let id = self.nuevo_id("app");
                salida.peticiones.push(json!({ "type": "new_tab", "id": id, "url": null }));
                self.escribiendo = false;
                self.empezar_a_editar(String::new());
            }
            Accion::PestanasAnt => {
                self.desplazamiento_pestanas = self.desplazamiento_pestanas.saturating_sub(1);
                salida.redibujar = true;
            }
            Accion::PestanasSig => {
                self.desplazamiento_pestanas += 1;
                let (ini, _, _, _) = barra::ventana_pestanas(self.ancho, self.pestanas.len(), self.desplazamiento_pestanas);
                self.desplazamiento_pestanas = ini;
                salida.redibujar = true;
            }
            Accion::Pestana(id) => {
                if id != self.activa {
                    let peticion = self.nuevo_id("app");
                    salida.peticiones.push(json!({ "type": "switch_tab", "id": peticion, "tab_id": id }));
                    self.escribiendo = false;
                }
            }
            Accion::CerrarPestana(id) => {
                if self.pestanas.len() > 1 {
                    let peticion = self.nuevo_id("app");
                    salida.peticiones.push(json!({ "type": "close_tab", "id": peticion, "tab_id": id }));
                    self.pestanas.retain(|p| p.id != id);
                    self.orden_pestanas.retain(|p| *p != id);
                    self.errores.remove(&id);
                    self.asegurar_pestana_visible();
                    let lista = self.nuevo_id("app");
                    salida.peticiones.push(json!({ "type": "list_tabs", "id": lista }));
                }
            }
            Accion::Sugerencia(idx) => {
                if let Some(sug) = self.sugerencias.get(idx).cloned() {
                    self.cerrar_campo();
                    self.navegar(sug.url, salida);
                }
            }
        }
    }

    fn navegar(&mut self, url: String, salida: &mut Salida) {
        let id = self.nuevo_id("nav");
        salida.peticiones.push(json!({ "type": "navigate", "id": id, "url": url }));
        self.errores.remove(&self.activa);
        self.carga = Some(Carga { id, url });
        self.escribiendo = false;
        salida.redibujar = true;
    }

    fn pedir_tamano(&mut self, salida: &mut Salida) {
        if self.tamano_en_vuelo.is_some() {
            self.tamano_pendiente = true;
            return;
        }
        let id = self.nuevo_id("tam");
        self.tamano_en_vuelo = Some(id.clone());
        self.tamano_pendiente = false;
        salida.peticiones.push(json!({ "type": "resize", "id": id, "width": self.ancho, "height": self.alto_contenido(), "scale": self.escala }));
    }

    fn empezar_a_editar(&mut self, texto: String) {
        let largo = texto.chars().count();
        self.campo = Some(Campo { todo: largo > 0, cursor: largo, texto });
        self.sugerencias.clear();
        self.sugerencia_seleccionada = None;
        self.mostrar_sugerencias = false;
    }

    /// Lo que la barra enseña cuando no se edita: adónde se va mientras
    /// carga; si no, la URL de la página.
    fn url_mostrada(&self) -> String {
        match &self.carga {
            Some(carga) if !carga.url.is_empty() => carga.url.clone(),
            _ => self.errores.get(&self.activa).map(|(url, _)| url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| self.url.clone()),
        }
    }

    // --- Mensajes del motor ---

    fn mensaje(&mut self, mensaje: MensajeMotor, salida: &mut Salida) {
        match mensaje {
            MensajeMotor::Estado(estado) => self.estado(*estado, salida),
            MensajeMotor::Error { id, mensaje } => {
                let id = id.unwrap_or_default();
                if self.carga.as_ref().is_some_and(|c| c.id == id) {
                    let carga = self.carga.take().unwrap_or(Carga { id: id.clone(), url: String::new() });
                    if !mensaje.starts_with("navegacion_cancelada") {
                        let url = if carga.url.is_empty() { self.url.clone() } else { carga.url };
                        self.errores.insert(self.activa, (url, mensaje.clone()));
                    }
                }
                if self.scroll_en_vuelo.as_deref() == Some(&id) {
                    self.scroll_en_vuelo = None;
                    self.scroll_pendiente = 0.0;
                }
                if self.tamano_en_vuelo.as_deref() == Some(&id) {
                    self.tamano_en_vuelo = None;
                    if self.tamano_pendiente {
                        self.pedir_tamano(salida);
                    }
                }
                if self.hover_en_vuelo.as_deref() == Some(&id) {
                    self.hover_en_vuelo = None;
                }
                if mensaje.contains("no hay un control enfocado") {
                    self.escribiendo = false;
                }
                salida.redibujar = true;
            }
            MensajeMotor::Pestanas { lista, activa, .. } => {
                self.activa = activa;
                self.sincronizar_pestanas(lista);
                salida.redibujar = true;
            }
            MensajeMotor::Terminado => {
                self.carga = None;
                self.hover_en_vuelo = None;
                self.errores.insert(self.activa, (self.url.clone(), "El motor se cerró de forma inesperada. Cierra y vuelve a abrir el navegador.".to_string()));
                salida.redibujar = true;
            }
            MensajeMotor::Hecho { id } => {
                if let Some(id) = id {
                    if self.hover_en_vuelo.as_deref() == Some(&id) {
                        self.hover_en_vuelo = None;
                        if let Some((px, py)) = self.hover_pendiente.take() {
                            self.mover_raton(px, py + ALTO_BARRA as f32, salida);
                        }
                    }
                }
            }
            MensajeMotor::Listo | MensajeMotor::Otro => {}
        }
    }

    fn estado(&mut self, estado: Estado, salida: &mut Salida) {
        let id = estado.id.clone().unwrap_or_default();
        if self.carga.as_ref().is_some_and(|c| c.id == id) {
            self.carga = None;
            self.errores.remove(&estado.pestana);
        }
        if self.scroll_en_vuelo.as_deref() == Some(&id) {
            self.scroll_en_vuelo = None;
            let pendiente = std::mem::take(&mut self.scroll_pendiente);
            if pendiente != 0.0 {
                self.desplazar(pendiente, salida);
            }
        }
        if self.tamano_en_vuelo.as_deref() == Some(&id) {
            self.tamano_en_vuelo = None;
            if self.tamano_pendiente {
                self.pedir_tamano(salida);
            }
        }
        if self.hover_en_vuelo.as_deref() == Some(&id) {
            self.hover_en_vuelo = None;
            if let Some((px, py)) = self.hover_pendiente.take() {
                self.mover_raton(px, py + ALTO_BARRA as f32, salida);
            }
        }
        if estado.pestana != self.activa || estado.url != self.url {
            self.escribiendo = false;
        }
        if let Some(enfocado_texto) = estado.enfocado_texto {
            self.escribiendo = enfocado_texto;
        }
        self.seleccion_pagina = estado.seleccion;
        let url_cambio = estado.url != self.url;
        let titulo_cambio = estado.titulo != self.titulo;
        self.activa = estado.pestana;
        if !self.orden_pestanas.contains(&estado.pestana) {
            self.orden_pestanas.push(estado.pestana);
        }
        match self.pestanas.iter_mut().find(|p| p.id == estado.pestana) {
            Some(p) => {
                p.titulo = estado.titulo.clone();
                p.url = estado.url.clone();
            }
            None => self.pestanas.push(Pestana { id: estado.pestana, titulo: estado.titulo.clone(), url: estado.url.clone() }),
        }
        self.asegurar_pestana_visible();
        self.url = estado.url;
        self.titulo = estado.titulo;
        if !self.url.is_empty() && (url_cambio || (titulo_cambio && !self.titulo.is_empty())) {
            self.datos.registrar_visita(&self.url, &self.titulo);
        }
        self.scroll_y = estado.scroll_y;
        self.puede_atras = estado.puede_atras;
        self.puede_adelante = estado.puede_adelante;
        if self.url.is_empty() {
            self.pagina = None;
            self.elementos.clear();
        } else if let Some(captura) = estado.captura {
            self.pagina = Some(captura);
        }
        if let Some(elementos) = estado.elementos {
            self.elementos = elementos;
        }
        salida.redibujar = true;
    }

    // --- Pintado ---

    fn vista(&self) -> VistaBarra {
        let (direccion, editando, cursor, todo) = match &self.campo {
            Some(c) => (c.texto.clone(), true, c.cursor, c.todo),
            None => (self.url_mostrada(), false, 0, false),
        };
        VistaBarra {
            pestanas: self.pestanas.clone(),
            activa: self.activa,
            desplazamiento_pestanas: self.desplazamiento_pestanas,
            direccion,
            editando,
            cursor,
            todo_seleccionado: todo,
            cargando: self.carga.is_some(),
            puede_atras: self.puede_atras,
            puede_adelante: self.puede_adelante,
            es_marcador: self.datos.es_marcador(&self.url),
            marcador_habilitado: !self.url.is_empty(),
        }
    }

    /// La imagen entera de la ventana en píxeles físicos: la barra arriba y
    /// debajo la página (o la página de inicio o de error).
    pub fn componer(&mut self) -> Pixmap {
        let escala_cent = (self.escala * 100.0).round() as u32;
        let alto_contenido = self.alto_contenido();
        let ancho_fis = fisico(self.ancho, self.escala);
        let alto_barra_fis = fisico(ALTO_BARRA, self.escala);
        let alto_contenido_fis = fisico(alto_contenido, self.escala);
        let alto_fis = alto_barra_fis + alto_contenido_fis;
        let mut marco = Pixmap::new(ancho_fis, alto_fis).unwrap_or_else(|| Pixmap::new(1, 1).expect("1x1"));
        marco.fill(Color::WHITE);

        let vista = self.vista();
        let repintar_barra = !matches!(&self.barra, Some((v, ancho, esc, _)) if *v == vista && *ancho == self.ancho && *esc == escala_cent);
        if repintar_barra {
            match barra::pintar_con_escala(&vista, self.ancho, self.escala, &self.fuentes, &self.iconos) {
                Ok(pintada) => self.barra = Some((vista, self.ancho, escala_cent, pintada)),
                Err(error) => eprintln!("[navegador] no se pudo pintar la barra: {error}"),
            }
        }
        if let Some((_, _, _, pintada)) = &self.barra {
            marco.draw_pixmap(0, 0, pintada.lienzo.pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }

        let html_local = match self.errores.get(&self.activa) {
            Some((url, mensaje)) => Some(paginas::error(url, mensaje)),
            None if !self.tiene_pagina() => Some(paginas::inicio_con_marcadores(self.datos.marcadores())),
            None => None,
        };
        self.mostrando_local = html_local.is_some();
        match html_local {
            Some(html) => {
                let repintar = !matches!(&self.local, Some((h, a, b, esc, _)) if *h == html && *a == self.ancho && *b == alto_contenido && *esc == escala_cent);
                if repintar {
                    match lienzo::pintar_html_escalado(&html, self.ancho, alto_contenido, self.escala, &self.fuentes, &ImageMap::new(), |_| Vec::new()) {
                        Ok(pintado) => self.local = Some((html, self.ancho, alto_contenido, escala_cent, pintado)),
                        Err(error) => eprintln!("[navegador] no se pudo pintar la página propia: {error}"),
                    }
                }
                if let Some((_, _, _, _, pintado)) = &self.local {
                    marco.draw_pixmap(0, alto_barra_fis as i32, pintado.pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
                }
            }
            None => {
                if let Some(pagina) = &self.pagina {
                    if (pagina.width(), pagina.height()) != (ancho_fis, alto_contenido_fis)
                        && (self.escala - 1.0).abs() > 0.001
                        && (pagina.width(), pagina.height()) == (self.ancho, alto_contenido)
                    {
                        let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                        let transform = Transform::from_row(
                            ancho_fis as f32 / pagina.width() as f32,
                            0.0,
                            0.0,
                            alto_contenido_fis as f32 / pagina.height() as f32,
                            0.0,
                            alto_barra_fis as f32,
                        );
                        marco.draw_pixmap(0, 0, pagina.as_ref(), &paint, transform, None);
                    } else {
                        marco.draw_pixmap(0, alto_barra_fis as i32, pagina.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
                    }
                }
            }
        }
        if self.campo.is_some() && self.mostrar_sugerencias && !self.sugerencias.is_empty() {
            if let Some((ox, oy, aw)) = self.caja_desplegable_css() {
                let repintar_sug = !matches!(
                    &self.desplegable,
                    Some(d) if d.sugerencias == self.sugerencias && d.seleccionada == self.sugerencia_seleccionada && d.ancho == aw && d.escala_cent == escala_cent
                );
                if repintar_sug {
                    if let Ok((lienzo, _)) = barra::pintar_sugerencias_con_escala(
                        &self.sugerencias,
                        self.sugerencia_seleccionada,
                        aw,
                        self.escala,
                        &self.fuentes,
                        &self.iconos,
                    ) {
                        self.desplegable = Some(Desplegable {
                            sugerencias: self.sugerencias.clone(),
                            seleccionada: self.sugerencia_seleccionada,
                            ancho: aw,
                            escala_cent,
                            lienzo,
                        });
                    }
                }
                if let Some(d) = &self.desplegable {
                    marco.draw_pixmap(
                        (ox * self.escala).round() as i32,
                        (oy * self.escala).round() as i32,
                        d.lienzo.pixmap.as_ref(),
                        &PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                }
            }
        } else {
            self.desplegable = None;
        }
        marco
    }
}

fn normalizar_escala(escala: f32) -> f32 {
    if escala.is_finite() && escala > 0.0 {
        escala.clamp(0.5, 4.0)
    } else {
        1.0
    }
}

fn fisico(css: u32, escala: f32) -> u32 {
    ((css as f32 * escala).round() as u32).max(1)
}

/// ¿Un clic aquí puede abrir otra página? (enlaces y botones de formulario)
fn puede_navegar(elemento: &Elemento) -> bool {
    match elemento.etiqueta.as_str() {
        "a" | "button" => true,
        "input" => matches!(elemento.tipo.as_deref().map(str::to_ascii_lowercase).as_deref(), Some("submit" | "image")),
        _ => false,
    }
}

/// Posición en bytes del carácter número `indice`.
fn byte_de(texto: &str, indice: usize) -> usize {
    texto.char_indices().nth(indice).map_or(texto.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tecla(t: Tecla) -> Entrada {
        Entrada::Tecla { tecla: t, mods: Modificadores::default() }
    }

    fn escribir(app: &mut App, texto: &str) -> Vec<Value> {
        texto.chars().flat_map(|c| app.manejar(tecla(Tecla::Caracter(c.to_string()))).peticiones).collect()
    }

    fn estado(id: &str, url: &str) -> Entrada {
        let mut captura = Pixmap::new(10, 10).unwrap();
        captura.fill(Color::from_rgba8(200, 0, 0, 255));
        Entrada::Motor(MensajeMotor::Estado(Box::new(Estado {
            id: Some(id.to_string()),
            pestana: 0,
            url: url.to_string(),
            titulo: "Título".to_string(),
            scroll_y: 0.0,
            captura: Some(captura),
            elementos: Some(vec![
                Elemento { id: 1, etiqueta: "input".into(), tipo: Some("text".into()), x: 10.0, y: 10.0, ancho: 200.0, alto: 30.0 },
                Elemento { id: 2, etiqueta: "a".into(), tipo: None, x: 10.0, y: 100.0, ancho: 80.0, alto: 20.0 },
            ]),
            puede_atras: true,
            puede_adelante: false,
            enfocado_texto: None,
            seleccion: None,
        })))
    }

    fn tipo(peticion: &Value) -> &str {
        peticion["type"].as_str().unwrap_or_default()
    }

    /// Una app con una página ya cargada en "https://e.com/".
    fn con_pagina() -> App {
        let (mut app, inicio) = App::nueva(800, 600, Some("e.com".to_string()));
        let navegar = inicio.iter().find(|p| tipo(p) == "navigate").unwrap();
        assert_eq!(navegar["url"], "https://e.com");
        let id = navegar["id"].as_str().unwrap().to_string();
        app.manejar(estado(&id, "https://e.com/"));
        app.componer();
        assert!(app.tiene_pagina() && !app.cargando());
        app
    }

    #[test]
    fn arranca_pidiendo_tamano_y_pestanas() {
        let (app, peticiones) = App::nueva(800, 600, None);
        assert_eq!(tipo(&peticiones[0]), "resize");
        assert_eq!(peticiones[0]["height"], 600 - ALTO_BARRA);
        assert_eq!(tipo(&peticiones[1]), "list_tabs");
        assert_eq!(peticiones.len(), 2, "sin dirección inicial no se navega");
        assert_eq!(app.editando_direccion(), Some(""), "la barra queda lista para escribir");
    }

    #[test]
    fn escribir_en_la_barra_y_enter_navega() {
        let (mut app, _) = App::nueva(800, 600, None);
        app.componer();
        assert!(escribir(&mut app, "clima en madrid").is_empty());
        assert_eq!(app.editando_direccion(), Some("clima en madrid"));
        app.manejar(tecla(Tecla::Retroceso));
        let salida = app.manejar(tecla(Tecla::Enter));
        assert_eq!(tipo(&salida.peticiones[0]), "navigate");
        assert_eq!(salida.peticiones[0]["url"], "https://www.google.com/search?q=clima+en+madri");
        assert!(app.cargando());
        assert_eq!(app.editando_direccion(), None);
    }

    #[test]
    fn pulsar_la_barra_selecciona_todo_y_escribir_lo_sustituye() {
        let mut app = con_pagina();
        let barra = app.barra.as_ref().unwrap();
        let region = barra.3.lienzo.regiones.iter().find(|r| r.accion == "direccion").unwrap().rect.clone();
        app.manejar(Entrada::Clic { x: region.x + 40.0, y: region.y + 10.0 });
        assert_eq!(app.editando_direccion(), Some("https://e.com/"));
        escribir(&mut app, "x.org");
        assert_eq!(app.editando_direccion(), Some("x.org"));
        app.manejar(tecla(Tecla::Escape));
        assert_eq!(app.editando_direccion(), None);
        assert_eq!(app.url(), "https://e.com/");
    }

    #[test]
    fn editar_con_flechas_inicio_y_suprimir() {
        let (mut app, _) = App::nueva(800, 600, None);
        escribir(&mut app, "abd");
        app.manejar(tecla(Tecla::Izquierda));
        escribir(&mut app, "c");
        assert_eq!(app.editando_direccion(), Some("abcd"));
        app.manejar(tecla(Tecla::Inicio));
        app.manejar(tecla(Tecla::Suprimir));
        assert_eq!(app.editando_direccion(), Some("bcd"));
        app.manejar(tecla(Tecla::Fin));
        escribir(&mut app, "ñ");
        app.manejar(tecla(Tecla::Retroceso));
        app.manejar(tecla(Tecla::Retroceso));
        assert_eq!(app.editando_direccion(), Some("bc"));
        app.manejar(Entrada::Tecla { tecla: Tecla::Caracter("a".into()), mods: Modificadores { ctrl: true, ..Default::default() } });
        escribir(&mut app, "z");
        assert_eq!(app.editando_direccion(), Some("z"));
    }

    #[test]
    fn clic_en_un_campo_de_la_pagina_manda_las_teclas_a_la_pagina() {
        let mut app = con_pagina();
        let salida = app.manejar(Entrada::Clic { x: 50.0, y: ALTO_BARRA as f32 + 20.0 });
        assert_eq!(tipo(&salida.peticiones[0]), "click");
        assert_eq!(salida.peticiones[0]["y"], 20.0, "la y es la de la página, sin la barra");
        assert!(app.escribiendo_en_pagina());
        let teclas = escribir(&mut app, "hé");
        assert_eq!(teclas.iter().map(|p| p["key"].as_str().unwrap()).collect::<Vec<_>>(), ["h", "é"]);
        let enter = app.manejar(tecla(Tecla::Enter));
        assert_eq!(enter.peticiones[0]["key"], "Enter");
        assert!(app.cargando(), "Enter puede enviar el formulario");
    }

    #[test]
    fn sin_campo_las_teclas_desplazan() {
        let mut app = con_pagina();
        app.manejar(Entrada::Clic { x: 500.0, y: 500.0 });
        assert!(!app.escribiendo_en_pagina());
        let salida = app.manejar(tecla(Tecla::AvPag));
        assert_eq!(tipo(&salida.peticiones[0]), "scroll");
        assert!(salida.peticiones[0]["dy"].as_i64().unwrap() > 400);
    }

    #[test]
    fn la_rueda_no_amontona_peticiones() {
        let mut app = con_pagina();
        let primera = app.manejar(Entrada::Rueda { dy: 48.0 });
        assert_eq!(primera.peticiones.len(), 1);
        let id = primera.peticiones[0]["id"].as_str().unwrap().to_string();
        assert!(app.manejar(Entrada::Rueda { dy: 48.0 }).peticiones.is_empty());
        assert!(app.manejar(Entrada::Rueda { dy: 48.0 }).peticiones.is_empty());
        // Cuando el motor contesta, sale lo acumulado en una sola petición.
        let salida = app.manejar(estado(&id, "https://e.com/"));
        assert_eq!(salida.peticiones.len(), 1);
        assert_eq!(salida.peticiones[0]["dy"], 96);
    }

    #[test]
    fn un_enlace_cuenta_como_carga_y_se_puede_detener() {
        let mut app = con_pagina();
        let salida = app.manejar(Entrada::Clic { x: 20.0, y: ALTO_BARRA as f32 + 105.0 });
        assert_eq!(tipo(&salida.peticiones[0]), "click");
        assert!(app.cargando());
        // Mientras carga, otro clic en la página no se manda.
        assert!(app.manejar(Entrada::Clic { x: 20.0, y: ALTO_BARRA as f32 + 105.0 }).peticiones.is_empty());
        let salida = app.manejar(tecla(Tecla::Escape));
        assert_eq!(tipo(&salida.peticiones[0]), "stop");
        assert!(!app.cargando());
    }

    #[test]
    fn un_error_de_carga_muestra_la_pagina_de_error_y_reintentar_vuelve_a_pedirla() {
        let (mut app, inicio) = App::nueva(800, 600, Some("noexiste.test".to_string()));
        let id = inicio.iter().find(|p| tipo(p) == "navigate").unwrap()["id"].as_str().unwrap().to_string();
        app.manejar(Entrada::Motor(MensajeMotor::Error { id: Some(id), mensaje: "network_error: connection refused".into() }));
        assert!(!app.cargando());
        assert_eq!(app.error_visible().map(|(u, _)| u.as_str()), Some("https://noexiste.test"));
        app.componer();
        assert!(app.mostrando_local);
        let boton = app.local.as_ref().unwrap().4.regiones.iter().find(|r| r.accion == "reintentar").unwrap().rect.clone();
        let salida = app.manejar(Entrada::Clic { x: boton.x + 10.0, y: boton.y + ALTO_BARRA as f32 + 10.0 });
        assert_eq!(salida.peticiones[0]["url"], "https://noexiste.test");
        assert!(app.error_visible().is_none());
    }

    #[test]
    fn una_navegacion_cancelada_no_es_un_error() {
        let (mut app, inicio) = App::nueva(800, 600, Some("e.com".to_string()));
        let id = inicio.iter().find(|p| tipo(p) == "navigate").unwrap()["id"].as_str().unwrap().to_string();
        app.manejar(Entrada::Motor(MensajeMotor::Error { id: Some(id), mensaje: "navegacion_cancelada: x".into() }));
        assert!(app.error_visible().is_none());
        assert!(!app.cargando());
    }

    #[test]
    fn la_pagina_de_inicio_abre_sus_sitios() {
        let (mut app, _) = App::nueva(1000, 700, None);
        app.componer();
        let sitio = app.local.as_ref().unwrap().4.regiones.iter().find(|r| r.accion.starts_with("ir:")).unwrap().clone();
        let salida = app.manejar(Entrada::Clic { x: sitio.rect.x + 5.0, y: sitio.rect.y + ALTO_BARRA as f32 + 5.0 });
        assert_eq!(tipo(&salida.peticiones[0]), "navigate");
        assert_eq!(salida.peticiones[0]["url"], &sitio.accion[3..]);
    }

    #[test]
    fn pestanas_nueva_cambiar_y_cerrar() {
        let mut app = con_pagina();
        let atajo = |c: &str| Entrada::Tecla { tecla: Tecla::Caracter(c.into()), mods: Modificadores { ctrl: true, ..Default::default() } };
        let salida = app.manejar(atajo("t"));
        assert_eq!(tipo(&salida.peticiones[0]), "new_tab");
        assert_eq!(app.editando_direccion(), Some(""));
        app.manejar(Entrada::Motor(MensajeMotor::Pestanas { id: None, lista: vec![(0, "A".into(), "https://e.com/".into()), (1, String::new(), String::new())], activa: 1 }));
        assert_eq!(app.pestanas().len(), 2);
        let salida = app.manejar(Entrada::Tecla { tecla: Tecla::Tab, mods: Modificadores { ctrl: true, ..Default::default() } });
        assert_eq!(salida.peticiones[0]["type"], "switch_tab");
        assert_eq!(salida.peticiones[0]["tab_id"], 0);
        let salida = app.manejar(atajo("w"));
        assert_eq!(tipo(&salida.peticiones[0]), "close_tab");
        assert_eq!(app.pestanas().len(), 1);
        // La última pestaña no se cierra.
        assert!(app.manejar(atajo("w")).peticiones.is_empty());
    }

    #[test]
    fn redimensionar_pide_un_tamano_cada_vez() {
        let (mut app, inicio) = App::nueva(800, 600, None);
        let id = inicio[0]["id"].as_str().unwrap().to_string();
        assert!(app.manejar(Entrada::Tamano { ancho: 900, alto: 600 }).peticiones.is_empty(), "espera a que el motor conteste el anterior");
        assert!(app.manejar(Entrada::Tamano { ancho: 1000, alto: 700 }).peticiones.is_empty());
        let salida = app.manejar(estado(&id, ""));
        assert_eq!(tipo(&salida.peticiones[0]), "resize");
        assert_eq!((salida.peticiones[0]["width"].as_u64(), salida.peticiones[0]["height"].as_u64()), (Some(1000), Some(700 - ALTO_BARRA as u64)));
    }

    #[test]
    fn compone_la_barra_encima_y_la_pagina_debajo() {
        let mut app = con_pagina();
        let marco = app.componer();
        assert_eq!((marco.width(), marco.height()), (800, 600));
        let pagina = marco.pixel(5, ALTO_BARRA + 5).unwrap();
        assert_eq!((pagina.red(), pagina.green(), pagina.blue()), (200, 0, 0), "la captura del motor va justo debajo de la barra");
        let barra = marco.pixel(400, 2).unwrap();
        assert_ne!((barra.red(), barra.green(), barra.blue()), (200, 0, 0));
        assert_eq!(app.titulo_ventana(), "Título — Navegador");
    }

    #[test]
    fn hidpi_compone_a_escala_fisica_y_pide_scale_al_motor() {
        let (mut app, peticiones) = App::nueva_con_escala(800, 600, 1.5, None);
        assert_eq!(peticiones[0]["scale"], 1.5);
        let id = peticiones[0]["id"].as_str().unwrap().to_string();
        app.manejar(estado(&id, ""));
        let marco = app.componer();
        assert_eq!((marco.width(), marco.height()), (1200, 900));
        let salida = app.manejar(Entrada::Escala(2.0));
        assert!(salida.redibujar);
        let resize = salida.peticiones.iter().find(|p| tipo(p) == "resize").unwrap();
        assert_eq!(resize["scale"], 2.0);
        let marco_2x = app.componer();
        assert_eq!((marco_2x.width(), marco_2x.height()), (1600, 1200));
    }

    #[test]
    fn copiar_cortar_y_pegar_en_barra_y_pagina() {
        let mut app = con_pagina();
        app.portapapeles = Portapapeles::Memoria(std::sync::Mutex::new(String::new()));
        let ctrl_c = Entrada::Tecla { tecla: Tecla::Caracter("c".into()), mods: Modificadores { ctrl: true, ..Default::default() } };
        let ctrl_x = Entrada::Tecla { tecla: Tecla::Caracter("x".into()), mods: Modificadores { ctrl: true, ..Default::default() } };
        let ctrl_v = Entrada::Tecla { tecla: Tecla::Caracter("v".into()), mods: Modificadores { ctrl: true, ..Default::default() } };
        let ctrl_l = Entrada::Tecla { tecla: Tecla::Caracter("l".into()), mods: Modificadores { ctrl: true, ..Default::default() } };

        app.manejar(ctrl_l);
        assert_eq!(app.editando_direccion(), Some("https://e.com/"));
        app.manejar(ctrl_c);
        assert_eq!(app.portapapeles.leer().as_deref(), Some("https://e.com/"));
        app.manejar(ctrl_x);
        assert_eq!(app.editando_direccion(), Some(""));
        app.portapapeles.escribir("rust-lang.org\n");
        app.manejar(ctrl_v.clone());
        assert_eq!(app.editando_direccion(), Some("rust-lang.org "));
        app.manejar(tecla(Tecla::Escape));

        // Pegar también llega al campo enfocado de la página.
        app.manejar(Entrada::Clic { x: 50.0, y: ALTO_BARRA as f32 + 20.0 });
        assert!(app.escribiendo_en_pagina());
        app.portapapeles.escribir("ok");
        let salida = app.manejar(ctrl_v);
        let teclas: Vec<_> = salida.peticiones.iter().map(|p| p["key"].as_str().unwrap()).collect();
        assert_eq!(teclas, ["o", "k"]);
    }

    #[test]
    fn cursor_cambia_segun_elemento_y_zona() {
        let app = con_pagina();
        let barra = app.barra.as_ref().unwrap();
        let dir = barra.3.lienzo.regiones.iter().find(|r| r.accion == "direccion").unwrap().rect.clone();
        let rec = barra.3.lienzo.regiones.iter().find(|r| r.accion == "recargar").unwrap().rect.clone();
        assert_eq!(app.cursor_en(dir.x + 10.0, dir.y + 10.0), CursorTipo::Texto);
        assert_eq!(app.cursor_en(rec.x + 10.0, rec.y + 10.0), CursorTipo::Mano);
        assert_eq!(app.cursor_en(50.0, ALTO_BARRA as f32 + 20.0), CursorTipo::Texto);
        assert_eq!(app.cursor_en(30.0, ALTO_BARRA as f32 + 105.0), CursorTipo::Mano);
        assert_eq!(app.cursor_en(500.0, ALTO_BARRA as f32 + 500.0), CursorTipo::Normal);
    }

    #[test]
    fn flechas_tab_hover_y_seleccion_llegan_al_motor() {
        let mut app = con_pagina();
        app.portapapeles = Portapapeles::Memoria(std::sync::Mutex::new(String::new()));

        let hover = app.manejar(Entrada::Raton { x: 20.0, y: ALTO_BARRA as f32 + 105.0 });
        assert_eq!(tipo(&hover.peticiones[0]), "hover");
        let hover_id = hover.peticiones[0]["id"].as_str().unwrap().to_string();
        app.manejar(Entrada::Motor(MensajeMotor::Hecho { id: Some(hover_id) }));

        let sel = app.manejar(Entrada::SeleccionarPagina { desde_x: 10.0, desde_y: ALTO_BARRA as f32 + 10.0, hasta_x: 150.0, hasta_y: ALTO_BARRA as f32 + 20.0 });
        assert_eq!(tipo(&sel.peticiones[0]), "select_text");

        app.manejar(Entrada::Clic { x: 50.0, y: ALTO_BARRA as f32 + 20.0 });
        let izq = app.manejar(Entrada::Tecla { tecla: Tecla::Izquierda, mods: Modificadores { shift: true, ..Default::default() } });
        assert_eq!(izq.peticiones[0]["key"], "Shift+ArrowLeft");
        let tab = app.manejar(Entrada::Tecla { tecla: Tecla::Tab, mods: Modificadores::default() });
        assert_eq!(tab.peticiones[0]["key"], "Tab");

        let tab_id = tab.peticiones[0]["id"].as_str().unwrap().to_string();
        app.manejar(Entrada::Motor(MensajeMotor::Estado(Box::new(Estado {
            id: Some(tab_id),
            pestana: 0,
            url: "https://e.com/".into(),
            titulo: "Título".into(),
            scroll_y: 0.0,
            captura: None,
            elementos: None,
            puede_atras: true,
            puede_adelante: false,
            enfocado_texto: Some(false),
            seleccion: Some("copiable".into()),
        }))));
        assert!(!app.escribiendo_en_pagina());
        app.manejar(Entrada::Tecla { tecla: Tecla::Caracter("c".into()), mods: Modificadores { ctrl: true, ..Default::default() } });
        assert_eq!(app.portapapeles.leer().as_deref(), Some("copiable"));
    }

    #[test]
    fn historial_marcadores_sugerencias_y_reorden_de_pestanas() {
        let mut app = con_pagina();
        assert_eq!(app.datos().historial().len(), 1);
        assert!(!app.datos().es_marcador("https://e.com/"));

        // Ctrl+D guarda marcador y vuelve a quitarlo si se pulsa de nuevo.
        let ctrl_d = Entrada::Tecla { tecla: Tecla::Caracter("d".into()), mods: Modificadores { ctrl: true, ..Default::default() } };
        app.manejar(ctrl_d.clone());
        assert!(app.datos().es_marcador("https://e.com/"));

        // Abrir nueva pestaña, escribir parte del dominio y elegir sugerencia con flecha abajo + Enter.
        app.manejar(Entrada::Tecla { tecla: Tecla::Caracter("t".into()), mods: Modificadores { ctrl: true, ..Default::default() } });
        app.manejar(Entrada::Motor(MensajeMotor::Pestanas {
            id: None,
            lista: vec![(0, "Título".into(), "https://e.com/".into()), (1, "Docs".into(), "https://docs.rs/".into())],
            activa: 1,
        }));
        escribir(&mut app, "e.com");
        assert!(!app.sugerencias().is_empty());
        assert_eq!(app.sugerencias()[0].url, "https://e.com/");
        app.componer();
        app.manejar(tecla(Tecla::Abajo));
        let nav = app.manejar(tecla(Tecla::Enter));
        assert_eq!(nav.peticiones[0]["url"], "https://e.com/");

        // Reordenar pestañas arrastrando la primera sobre la segunda.
        app.componer();
        let barras = app.barra.as_ref().unwrap();
        let p0 = barras.3.lienzo.regiones.iter().find(|r| r.accion == "pestana:0").unwrap().rect.clone();
        let p1 = barras.3.lienzo.regiones.iter().find(|r| r.accion == "pestana:1").unwrap().rect.clone();
        app.manejar(Entrada::Clic { x: p0.x + 15.0, y: p0.y + 10.0 });
        app.manejar(Entrada::ArrastrarBarra { x: p1.x + 15.0, y: p1.y + 10.0 });
        app.manejar(Entrada::SoltarBarra);
        let ids: Vec<u32> = app.pestanas().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![1, 0]);
    }
}
