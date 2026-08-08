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
    /// instalada - no hay fuente inventada de respaldo. Equivale a
    /// `load_default_sans_serif_variant(false, false)` - la variante
    /// regular/normal, sin negrita ni cursiva.
    pub fn load_default_sans_serif() -> Option<Self> {
        Self::load_default_sans_serif_variant(false, false)
    }

    /// Igual que `load_default_sans_serif`, pero pidiendo una variante
    /// concreta de peso/estilo - la base real de `FontSet` (ver mas abajo),
    /// que carga las 4 combinaciones una sola vez por pagina para que
    /// `font-weight: bold`/`font-style: italic` (Fase 2.4) se puedan pintar
    /// con una cara de fuente de verdad en vez de solo quedarse en la
    /// cascada sin efecto visible. `fontdb::Query` hace matching por "mejor
    /// candidato", no exacto: si el sistema no tiene una cara en negrita o
    /// cursiva de verdad para esta familia, devuelve la cara mas cercana
    /// que SI tenga (normalmente la regular) en vez de fallar - mismo
    /// comportamiento honesto que un navegador real sin esa cara instalada
    /// (sin negrita sintetica/oblicua artificial), no un caso de error.
    pub fn load_default_sans_serif_variant(bold: bool, italic: bool) -> Option<Self> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();

        let query = fontdb::Query {
            families: &[fontdb::Family::SansSerif],
            weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
            ..fontdb::Query::default()
        };

        let id = db.query(&query).or_else(|| {
            tracing::warn!("[engine-text] Ninguna fuente sans-serif encontrada (bold={bold}, italic={italic}), probando la primera fuente del sistema disponible");
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

/// Las 4 combinaciones reales de peso/estilo que el motor distingue hoy
/// (negrita binaria, cursiva binaria - ver `resolve_font_weight_is_bold`/
/// `resolve_font_style_is_italic` en `engine-layout::tree`, que deciden que
/// combinacion pedir a partir de `font-weight`/`font-style` de la cascada),
/// cargadas UNA sola vez por pagina y compartidas entre quien MIDE el texto
/// (`engine-layout`, para el layout/wrap) y quien lo PINTA (`engine-gfx`) -
/// misma razon por la que antes ya se cargaba una unica `SystemFont` una
/// vez y se pasaba a ambos en vez de que cada uno cargara la suya (ver el
/// doc-comment de `NativeEngineWindow::run` en `engine-gfx/src/window.rs`):
/// medir con una fuente y pintar con otra podria desalinear cajas y glifos.
#[derive(Debug, Clone)]
pub struct FontSet {
    pub regular: Option<SystemFont>,
    pub bold: Option<SystemFont>,
    pub italic: Option<SystemFont>,
    pub bold_italic: Option<SystemFont>,
}

impl FontSet {
    pub fn load_default_sans_serif() -> Self {
        Self {
            regular: SystemFont::load_default_sans_serif_variant(false, false),
            bold: SystemFont::load_default_sans_serif_variant(true, false),
            italic: SystemFont::load_default_sans_serif_variant(false, true),
            bold_italic: SystemFont::load_default_sans_serif_variant(true, true),
        }
    }

    /// Elige la variante que corresponde a `bold`/`italic` - las 4
    /// combinaciones ya cubren todo lo que el motor distingue, sin mezcla
    /// parcial. `None` solo si NINGUNA fuente de sistema se pudo cargar en
    /// absoluto (mismo caso raiz que `SystemFont::load_default_sans_serif()
    /// -> None`, no un caso nuevo de esta funcion).
    pub fn pick(&self, bold: bool, italic: bool) -> Option<&SystemFont> {
        match (bold, italic) {
            (true, true) => self.bold_italic.as_ref(),
            (true, false) => self.bold.as_ref(),
            (false, true) => self.italic.as_ref(),
            (false, false) => self.regular.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_default_sans_serif_variant_can_load_the_plain_variant() {
        // Igual que el resto de tests de este workspace que dependen de una
        // fuente real de sistema: si esta maquina no tiene ninguna
        // instalada, no hay nada que probar aqui - no es un fallo del
        // motor.
        let Some(_) = SystemFont::load_default_sans_serif_variant(false, false) else {
            return;
        };
    }

    #[test]
    fn font_set_pick_returns_the_requested_variant_when_loaded() {
        let Some(regular) = SystemFont::load_default_sans_serif() else { return };
        let set = FontSet {
            regular: Some(regular),
            bold: SystemFont::load_default_sans_serif_variant(true, false),
            italic: SystemFont::load_default_sans_serif_variant(false, true),
            bold_italic: SystemFont::load_default_sans_serif_variant(true, true),
        };
        assert!(set.pick(false, false).is_some(), "la variante regular deberia estar disponible, se acaba de cargar");
    }

    #[test]
    fn font_set_pick_is_none_for_every_combination_when_nothing_was_loaded() {
        let empty = FontSet { regular: None, bold: None, italic: None, bold_italic: None };
        assert!(empty.pick(false, false).is_none());
        assert!(empty.pick(true, false).is_none());
        assert!(empty.pick(false, true).is_none());
        assert!(empty.pick(true, true).is_none());
    }
}
