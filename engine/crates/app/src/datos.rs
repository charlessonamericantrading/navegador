//! Historial de navegación, marcadores y sugerencias de la barra de direcciones.
//! Se persiste como JSON en el directorio del perfil (`profile_dir`).

use crate::paginas::SITIOS;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORIAL: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntradaHistorial {
    pub url: String,
    pub titulo: String,
    pub visitas: u64,
    pub ultimo_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marcador {
    pub url: String,
    pub titulo: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrigenSugerencia {
    Marcador,
    Historial,
    Acceso,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sugerencia {
    pub url: String,
    pub titulo: String,
    pub origen: OrigenSugerencia,
}

pub struct DatosUsuario {
    ruta: Option<PathBuf>,
    historial: Vec<EntradaHistorial>,
    marcadores: Vec<Marcador>,
}

impl DatosUsuario {
    pub fn en_memoria() -> Self {
        Self { ruta: None, historial: Vec::new(), marcadores: Vec::new() }
    }

    pub fn cargar() -> Self {
        match engine_core::process::profile_dir() {
            Some(dir) => Self::en_directorio(dir),
            None => Self::en_memoria(),
        }
    }

    pub fn en_directorio(dir: PathBuf) -> Self {
        let historial = leer_json(&dir.join("historial.json"))
            .and_then(|v| v.as_array().cloned())
            .map(|lista| {
                lista
                    .into_iter()
                    .filter_map(|item| {
                        Some(EntradaHistorial {
                            url: item.get("url")?.as_str()?.to_string(),
                            titulo: item.get("titulo").and_then(Value::as_str).unwrap_or_default().to_string(),
                            visitas: item.get("visitas").and_then(Value::as_u64).unwrap_or(1),
                            ultimo_unix: item.get("ultimo_unix").and_then(Value::as_u64).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let marcadores = leer_json(&dir.join("marcadores.json"))
            .and_then(|v| v.as_array().cloned())
            .map(|lista| {
                lista
                    .into_iter()
                    .filter_map(|item| {
                        Some(Marcador {
                            url: item.get("url")?.as_str()?.to_string(),
                            titulo: item.get("titulo").and_then(Value::as_str).unwrap_or_default().to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self { ruta: Some(dir), historial, marcadores }
    }

    pub fn historial(&self) -> &[EntradaHistorial] {
        &self.historial
    }

    pub fn marcadores(&self) -> &[Marcador] {
        &self.marcadores
    }

    pub fn registrar_visita(&mut self, url: &str, titulo: &str) {
        let url = url.trim();
        if !url_navegable(url) {
            return;
        }
        let ahora = ahora_unix();
        let titulo_limpio = titulo.trim();
        if let Some(pos) = self.historial.iter().position(|e| e.url == url) {
            self.historial[pos].visitas += 1;
            self.historial[pos].ultimo_unix = ahora;
            if !titulo_limpio.is_empty() {
                self.historial[pos].titulo = titulo_limpio.to_string();
            }
            let entrada = self.historial.remove(pos);
            self.historial.insert(0, entrada);
        } else {
            self.historial.insert(
                0,
                EntradaHistorial {
                    url: url.to_string(),
                    titulo: titulo_limpio.to_string(),
                    visitas: 1,
                    ultimo_unix: ahora,
                },
            );
            self.historial.truncate(MAX_HISTORIAL);
        }
        self.guardar_historial();
    }

    pub fn es_marcador(&self, url: &str) -> bool {
        let limpia = url.trim();
        !limpia.is_empty() && self.marcadores.iter().any(|m| misma_url(&m.url, limpia))
    }

    /// Añade o quita un marcador. Devuelve `true` si quedó guardado.
    pub fn alternar_marcador(&mut self, url: &str, titulo: &str) -> bool {
        let url = url.trim();
        if !url_navegable(url) {
            return false;
        }
        if let Some(pos) = self.marcadores.iter().position(|m| misma_url(&m.url, url)) {
            self.marcadores.remove(pos);
            self.guardar_marcadores();
            return false;
        }
        let titulo = if titulo.trim().is_empty() { url } else { titulo.trim() };
        self.marcadores.insert(0, Marcador { url: url.to_string(), titulo: titulo.to_string() });
        self.guardar_marcadores();
        true
    }

    pub fn sugerir(&self, consulta: &str, max: usize) -> Vec<Sugerencia> {
        if max == 0 {
            return Vec::new();
        }
        let q = consulta.trim().to_ascii_lowercase();
        let mut salida = Vec::new();
        let mut vistas: HashSet<String> = HashSet::new();
        let mut agregar = |url: &str, titulo: &str, origen: OrigenSugerencia| {
            if salida.len() >= max {
                return;
            }
            if !q.is_empty() && !url.to_ascii_lowercase().contains(&q) && !titulo.to_ascii_lowercase().contains(&q) {
                return;
            }
            let clave = normalizar_clave(url);
            if vistas.insert(clave) {
                salida.push(Sugerencia { url: url.to_string(), titulo: titulo.to_string(), origen });
            }
        };

        for m in &self.marcadores {
            agregar(&m.url, &m.titulo, OrigenSugerencia::Marcador);
        }
        for h in &self.historial {
            agregar(&h.url, if h.titulo.is_empty() { &h.url } else { &h.titulo }, OrigenSugerencia::Historial);
        }
        if !q.is_empty() {
            for (nombre, url) in SITIOS {
                agregar(url, nombre, OrigenSugerencia::Acceso);
            }
        }
        salida
    }

    fn guardar_historial(&self) {
        let Some(dir) = &self.ruta else { return };
        let datos: Vec<Value> = self
            .historial
            .iter()
            .map(|e| json!({ "url": e.url, "titulo": e.titulo, "visitas": e.visitas, "ultimo_unix": e.ultimo_unix }))
            .collect();
        escribir_json(&dir.join("historial.json"), &Value::Array(datos));
    }

    fn guardar_marcadores(&self) {
        let Some(dir) = &self.ruta else { return };
        let datos: Vec<Value> = self.marcadores.iter().map(|m| json!({ "url": m.url, "titulo": m.titulo })).collect();
        escribir_json(&dir.join("marcadores.json"), &Value::Array(datos));
    }
}

fn url_navegable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file://")
}

fn normalizar_clave(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn misma_url(a: &str, b: &str) -> bool {
    normalizar_clave(a) == normalizar_clave(b)
}

fn ahora_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn leer_json(ruta: &PathBuf) -> Option<Value> {
    let texto = std::fs::read_to_string(ruta).ok()?;
    serde_json::from_str(&texto).ok()
}

fn escribir_json(ruta: &PathBuf, valor: &Value) {
    if let Some(padre) = ruta.parent() {
        let _ = std::fs::create_dir_all(padre);
    }
    if let Ok(texto) = serde_json::to_string_pretty(valor) {
        let _ = std::fs::write(ruta, texto);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_temporal(nombre: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("navegador-datos-{nombre}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn persiste_historial_y_marcadores_y_ordena_sugerencias() {
        let dir = dir_temporal("persist");
        {
            let mut datos = DatosUsuario::en_directorio(dir.clone());
            datos.registrar_visita("https://rust-lang.org/", "Rust Programming Language");
            datos.registrar_visita("https://docs.rs/", "Docs.rs");
            assert!(datos.alternar_marcador("https://docs.rs/", "Docs.rs"));
            assert!(datos.es_marcador("https://docs.rs"));
        }

        let recargado = DatosUsuario::en_directorio(dir.clone());
        assert_eq!(recargado.historial().len(), 2);
        assert_eq!(recargado.marcadores().len(), 1);
        let sugs = recargado.sugerir("rs", 5);
        assert_eq!(sugs[0].origen, OrigenSugerencia::Marcador);
        assert_eq!(sugs[0].url, "https://docs.rs/");
        let wiki = recargado.sugerir("wiki", 5);
        assert_eq!(wiki[0].origen, OrigenSugerencia::Acceso);
        assert_eq!(wiki[0].url, "https://es.wikipedia.org");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
