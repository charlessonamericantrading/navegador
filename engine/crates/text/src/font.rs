//! Carga de fuentes reales del sistema operativo via `fontdb`. Nada de
//! fuentes embebidas todavia (esa es una decision de producto pendiente:
//! empotrar una fuente garantiza el mismo resultado en cualquier maquina,
//! pero pesa el binario).

/// Bytes crudos de una fuente ya cargados en memoria, mas el indice de cara
/// dentro del archivo (las `.ttc` pueden contener varias). Se guardan como
/// `Vec<u8>` propio en vez de intentar quedarse con un `rustybuzz::Face`
/// prestado de `fontdb::Database`: `Database::with_face_data` solo presta
/// los bytes por la duracion del closure, asi que un `Face<'a>` no podria
/// sobrevivir fuera de el. Reconstruir el `Face` en cada `shape_text` es
/// barato comparado con el propio shaping.
/// `Clone` clona los bytes (una fuente suele ser cientos de KB, pero es una
/// operacion ocasional - al redimensionar la ventana, no por frame - no el
/// camino caliente del re-shaping por frame que ya existe).
#[derive(Debug, Clone)]
pub struct SystemFont {
    bytes: Vec<u8>,
    face_index: u32,
}

impl SystemFont {
    /// Busca la fuente sans-serif por defecto del sistema (Segoe UI/Arial en
    /// Windows, Helvetica/Arial en macOS, DejaVu Sans o similar en Linux via
    /// fontconfig). Devuelve `None` si `fontdb` no encuentra ninguna fuente
    /// instalada - no hay fuente inventada de respaldo.
    pub fn load_default_sans_serif() -> Option<Self> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();

        let query = fontdb::Query {
            families: &[fontdb::Family::SansSerif],
            ..fontdb::Query::default()
        };

        let id = db.query(&query).or_else(|| {
            tracing::warn!("[engine-text] Ninguna fuente sans-serif encontrada, probando la primera fuente del sistema disponible");
            db.faces().next().map(|face| face.id)
        })?;

        db.with_face_data(id, |data, face_index| Self {
            bytes: data.to_vec(),
            face_index,
        })
    }

    pub(crate) fn rustybuzz_face(&self) -> Option<rustybuzz::Face<'_>> {
        rustybuzz::Face::from_slice(&self.bytes, self.face_index)
    }
}
