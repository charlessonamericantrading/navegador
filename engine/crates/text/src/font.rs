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
/// `Clone` COMPARTE los bytes (`Arc`): una fuente son cientos de KB, y cada
/// pagina cargada recibe un clon de las 12 caras. Antes se copiaban enteras en
/// cada pagina y en cada pestaña (varios MB por pestaña); ahora todas
/// apuntan a la misma copia, que vive una vez por proceso.
#[derive(Debug, Clone)]
pub struct SystemFont {
    bytes: std::sync::Arc<[u8]>,
    face_index: u32,
    /// Identidad estable de esta cara, para poder cachear medidas de texto
    /// sin volver a mirar los bytes (ver `shape.rs`). No se puede usar la
    /// direccion de `bytes` como identidad: `Clone` copia los bytes a otra
    /// direccion, y una direccion liberada puede reaparecer luego apuntando
    /// a otra fuente distinta - eso daria medidas de la fuente equivocada.
    /// `Clone` PROPAGA el id a proposito: un clon tiene exactamente los
    /// mismos bytes, asi que mide exactamente igual y debe compartir cache.
    id: u64,
}

/// Contador de identidades de fuente (ver `SystemFont::id`). Empieza en 1
/// para que 0 nunca sea un id valido.
fn next_font_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
            bytes: data.into(),
            face_index,
            id: next_font_id(),
        })
    }

    /// Ver el campo `id`. Publico dentro del crate para que `shape.rs`
    /// pueda usarlo como clave de cache.
    pub(crate) fn cache_id(&self) -> u64 {
        self.id
    }

    pub(crate) fn rustybuzz_face(&self) -> Option<rustybuzz::Face<'_>> {
        rustybuzz::Face::from_slice(&self.bytes, self.face_index)
    }
}

/// La familia de letra que el motor sabe distinguir (tarea 2.7 del plan): las
/// tres genericas de CSS. Un `font-family` real es una lista de nombres
/// (`Georgia, "Times New Roman", serif`); el motor no carga fuentes web ni
/// nombres concretos, asi que cada nombre conocido se reduce a su familia
/// generica y la lista se resuelve por el primero que se reconoce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontFamily {
    #[default]
    SansSerif,
    Serif,
    Monospace,
}

impl FontFamily {
    /// Reduce un valor de `font-family` a una de las tres familias: el primer
    /// nombre de la lista que se reconoce manda; si ninguno se reconoce
    /// (`"Noto Sans JP"`), sans-serif, que es lo que hacia el motor siempre.
    pub fn from_css(value: &str) -> Self {
        for name in value.split(',') {
            let name = name.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_ascii_lowercase();
            if let Some(family) = Self::from_name(&name) {
                return family;
            }
        }
        Self::SansSerif
    }

    fn from_name(name: &str) -> Option<Self> {
        const SERIF: &[&str] = &[
            "serif", "georgia", "times", "times new roman", "garamond", "palatino", "palatino linotype", "book antiqua", "cambria", "baskerville",
            "liberation serif", "dejavu serif", "noto serif", "charter", "didot", "bookman",
        ];
        const MONOSPACE: &[&str] = &[
            "monospace", "courier", "courier new", "consolas", "menlo", "monaco", "lucida console", "ui-monospace", "sfmono-regular", "sf mono",
            "source code pro", "fira code", "fira mono", "jetbrains mono", "dejavu sans mono", "liberation mono", "roboto mono", "cascadia code",
            "cascadia mono", "andale mono", "noto sans mono",
        ];
        const SANS: &[&str] = &[
            "sans-serif", "arial", "helvetica", "helvetica neue", "verdana", "tahoma", "segoe ui", "system-ui", "-apple-system", "blinkmacsystemfont",
            "roboto", "inter", "open sans", "lucida grande", "trebuchet ms", "liberation sans", "dejavu sans", "noto sans", "ubuntu", "calibri",
        ];
        if SERIF.contains(&name) {
            Some(Self::Serif)
        } else if MONOSPACE.contains(&name) {
            Some(Self::Monospace)
        } else if SANS.contains(&name) {
            Some(Self::SansSerif)
        } else {
            None
        }
    }

    /// Nombres concretos a probar, por orden, antes de caer en la generica de
    /// `fontdb` (que en Linux casi nunca existe). Los de Windows y macOS
    /// primero porque es donde se usa el producto.
    fn preferred_names(self) -> &'static [&'static str] {
        match self {
            Self::SansSerif => &[],
            Self::Serif => &["Georgia", "Times New Roman", "Liberation Serif", "DejaVu Serif", "FreeSerif", "Noto Serif"],
            Self::Monospace => &["Consolas", "Courier New", "Menlo", "Liberation Mono", "DejaVu Sans Mono", "FreeMono", "Noto Sans Mono"],
        }
    }

    fn generic(self) -> fontdb::Family<'static> {
        match self {
            Self::SansSerif => fontdb::Family::SansSerif,
            Self::Serif => fontdb::Family::Serif,
            Self::Monospace => fontdb::Family::Monospace,
        }
    }
}

/// Las cuatro variantes de peso/estilo de UNA familia.
#[derive(Debug, Clone, Default)]
pub struct FontFaces {
    pub regular: Option<SystemFont>,
    pub bold: Option<SystemFont>,
    pub italic: Option<SystemFont>,
    pub bold_italic: Option<SystemFont>,
}

impl FontFaces {
    pub fn pick(&self, bold: bool, italic: bool) -> Option<&SystemFont> {
        match (bold, italic) {
            (true, true) => self.bold_italic.as_ref(),
            (true, false) => self.bold.as_ref(),
            (false, true) => self.italic.as_ref(),
            (false, false) => self.regular.as_ref(),
        }
    }

    /// Carga las cuatro variantes de `family` con UN solo escaneo de las
    /// fuentes del sistema (`db` ya lo hizo). Una familia que el sistema no
    /// tiene queda vacia: `FontSet::pick_family` cae entonces en sans-serif.
    fn load(db: &fontdb::Database, family: FontFamily) -> Self {
        let load = |bold: bool, italic: bool| -> Option<SystemFont> {
            let mut families: Vec<fontdb::Family<'_>> = family.preferred_names().iter().map(|name| fontdb::Family::Name(name)).collect();
            families.push(family.generic());
            let query = fontdb::Query {
                families: &families,
                weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                ..fontdb::Query::default()
            };
            let id = db.query(&query)?;
            db.with_face_data(id, |data, face_index| SystemFont { bytes: data.into(), face_index, id: next_font_id() })
        };
        Self { regular: load(false, false), bold: load(true, false), italic: load(false, true), bold_italic: load(true, true) }
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
#[derive(Debug, Clone, Default)]
pub struct FontSet {
    pub regular: Option<SystemFont>,
    pub bold: Option<SystemFont>,
    pub italic: Option<SystemFont>,
    pub bold_italic: Option<SystemFont>,
    /// Serifa y monoespaciada (tarea 2.7). Vacias si el sistema no tiene
    /// ninguna: `pick_family` cae entonces en las de arriba (sans-serif).
    pub serif: FontFaces,
    pub monospace: FontFaces,
}

/// Cache de proceso (Fase 5, hallazgo real de rendimiento) de la unica
/// combinacion de familia que este motor sabe pedir hoy (sans-serif del
/// sistema - la eleccion de `font-family` real todavia esta pendiente,
/// Fase 2.4b), asi que cachear UN solo `FontSet` global es correcto: nada
/// varia todavia entre paginas. Ver el doc-comment de
/// `FontSet::load_default_sans_serif` para el numero real medido.
static DEFAULT_SANS_SERIF: std::sync::OnceLock<FontSet> = std::sync::OnceLock::new();

impl FontSet {
    /// Medido en vivo (Fase 5, `examples/bench_font.rs`): ~500ms por
    /// llamada SIN cache - `fontdb::Database::new()` + `load_system_fonts()`
    /// se ejecutan 4 veces (una por combinacion peso/estilo), cada una
    /// reescaneando TODAS las fuentes instaladas en el sistema desde disco,
    /// y esta funcion se llamaba en CADA `navigate()` real
    /// (`core::server::EngineServer::navigate`) - medio segundo perdido en
    /// cada navegacion antes de que empezara ningun trabajo real de la
    /// pagina. Con el cache (`OnceLock`, primera llamada paga el escaneo
    /// real, todas las siguientes son un `clone()` barato - los bytes de
    /// una fuente son cientos de KB, un memcpy de microsegundos comparado
    /// con los ~500ms del escaneo), navegaciones sucesivas ya no lo pagan.
    /// Invalidacion: ninguna - un proceso de este motor no cambia las
    /// fuentes instaladas del sistema operativo mientras esta vivo, asi
    /// que no hay ningun escenario real en el que el valor cacheado quede
    /// obsoleto durante la vida del proceso.
    pub fn load_default_sans_serif() -> Self {
        DEFAULT_SANS_SERIF
            .get_or_init(Self::load_default_sans_serif_uncached)
            .clone()
    }

    fn load_default_sans_serif_uncached() -> Self {
        // UN solo escaneo de las fuentes del sistema para las 12 caras (3
        // familias x 4 variantes): antes eran 4 escaneos solo para sans-serif.
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let sans = FontFaces::load(&db, FontFamily::SansSerif);
        // Sin ninguna sans-serif cae a la primera fuente del sistema, igual
        // que hacia `SystemFont::load_default_sans_serif_variant`.
        let fallback = |bold: bool, italic: bool, found: Option<SystemFont>| found.or_else(|| SystemFont::load_default_sans_serif_variant(bold, italic));
        Self {
            regular: fallback(false, false, sans.regular),
            bold: fallback(true, false, sans.bold),
            italic: fallback(false, true, sans.italic),
            bold_italic: fallback(true, true, sans.bold_italic),
            serif: FontFaces::load(&db, FontFamily::Serif),
            monospace: FontFaces::load(&db, FontFamily::Monospace),
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

    /// Como `pick`, pero de la familia pedida (tarea 2.7). Si el sistema no
    /// tiene esa familia (o esa variante), se usa la sans-serif: es mejor
    /// texto legible en otra letra que ningun texto.
    pub fn pick_family(&self, family: FontFamily, bold: bool, italic: bool) -> Option<&SystemFont> {
        let faces = match family {
            FontFamily::SansSerif => return self.pick(bold, italic),
            FontFamily::Serif => &self.serif,
            FontFamily::Monospace => &self.monospace,
        };
        faces.pick(bold, italic).or_else(|| faces.pick(false, false)).or_else(|| self.pick(bold, italic))
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
            ..FontSet::default()
        };
        assert!(set.pick(false, false).is_some(), "la variante regular deberia estar disponible, se acaba de cargar");
    }

    #[test]
    fn font_set_pick_is_none_for_every_combination_when_nothing_was_loaded() {
        let empty = FontSet::default();
        assert!(empty.pick(false, false).is_none());
        assert!(empty.pick(true, false).is_none());
        assert!(empty.pick(false, true).is_none());
        assert!(empty.pick(true, true).is_none());
    }

    /// Regresion real (Fase 5) del hallazgo de rendimiento documentado en el
    /// doc-comment de `load_default_sans_serif`: SIN cache, una llamada
    /// tarda ~500ms de verdad (escanea TODAS las fuentes del sistema 4
    /// veces) - si alguien quita el `OnceLock` sin darse cuenta, este test
    /// lo detecta. Margen deliberadamente generoso (50ms, dos ordenes de
    /// magnitud por debajo de los ~500ms medidos) para no ser fragil en una
    /// maquina cargada, sin dejar de detectar una regresion real a "vuelve
    /// a escanear cada vez".
    #[test]
    fn load_default_sans_serif_is_cached_after_the_first_call() {
        let _ = FontSet::load_default_sans_serif(); // fuerza la carga real primero, fuera de la medicion
        let start = std::time::Instant::now();
        let _ = FontSet::load_default_sans_serif();
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 50,
            "una segunda llamada deberia ser un clone barato desde el cache, no un rescan de ~500ms (tardo {elapsed:?})"
        );
    }

    /// El cache es un UNICO `FontSet` global, no una copia por llamada - dos
    /// llamadas deberian devolver los mismos bytes de fuente real, no dos
    /// fuentes potencialmente distintas si el sistema tuviera varias
    /// candidatas empatadas.
    #[test]
    fn load_default_sans_serif_returns_the_same_bytes_across_calls() {
        let Some(first) = FontSet::load_default_sans_serif().regular else { return };
        let Some(second) = FontSet::load_default_sans_serif().regular else { return };
        assert_eq!(first.bytes, second.bytes, "el cache deberia devolver siempre la misma fuente");
    }

    #[test]
    fn font_family_reduces_a_css_list_to_the_first_known_name() {
        assert_eq!(FontFamily::from_css("Georgia, serif"), FontFamily::Serif);
        assert_eq!(FontFamily::from_css("\"Times New Roman\", Times, serif"), FontFamily::Serif);
        assert_eq!(FontFamily::from_css("monospace"), FontFamily::Monospace);
        assert_eq!(FontFamily::from_css("'Courier New', monospace"), FontFamily::Monospace);
        assert_eq!(FontFamily::from_css("ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"), FontFamily::Monospace);
        assert_eq!(FontFamily::from_css("-apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, sans-serif"), FontFamily::SansSerif);
        // El primero conocido manda: una serif concreta antes de la generica sans.
        assert_eq!(FontFamily::from_css("Charter, Arial"), FontFamily::Serif);
        // Nombres desconocidos se saltan; si no se reconoce ninguno, sans-serif.
        assert_eq!(FontFamily::from_css("MiFuenteWeb, Georgia"), FontFamily::Serif);
        assert_eq!(FontFamily::from_css("MiFuenteWeb"), FontFamily::SansSerif);
        assert_eq!(FontFamily::from_css(""), FontFamily::SansSerif);
    }

    #[test]
    fn pick_family_falls_back_to_sans_serif_when_the_family_is_missing() {
        let Some(regular) = SystemFont::load_default_sans_serif() else { return };
        let only_sans = FontSet { regular: Some(regular), ..FontSet::default() };
        assert!(only_sans.pick_family(FontFamily::Serif, false, false).is_some(), "sin serifa instalada se usa la sans-serif");
        assert_eq!(
            only_sans.pick_family(FontFamily::Monospace, true, true).is_some(),
            only_sans.pick(true, true).is_some(),
            "una variante que ni la sans-serif tiene sigue sin existir: mismo comportamiento de siempre"
        );
        assert_eq!(
            only_sans.pick_family(FontFamily::Monospace, false, false).map(|f| f.cache_id()),
            only_sans.pick(false, false).map(|f| f.cache_id()),
            "y es exactamente la misma cara"
        );
    }

    /// En una maquina con fuentes serif y monoespaciadas, son caras distintas
    /// de la sans-serif y medir la misma frase da anchos distintos (la
    /// monoespaciada ancha siempre mas en las 'i'). Si la maquina no las tiene,
    /// no hay nada que probar.
    #[test]
    fn serif_and_monospace_are_really_different_faces_when_installed() {
        let set = FontSet::load_default_sans_serif();
        let (Some(sans), Some(mono)) = (set.pick(false, false), set.monospace.regular.as_ref()) else { return };
        assert_ne!(sans.cache_id(), mono.cache_id());
        let sans_width = crate::shape::text_width(sans, "iiiiiiiiii", 24.0);
        let mono_width = crate::shape::text_width(mono, "iiiiiiiiii", 24.0);
        assert!(mono_width > sans_width * 1.3, "monoespaciada ({mono_width}) deberia ser mas ancha que sans ({sans_width}) en una 'i'");
        if let Some(serif) = set.serif.regular.as_ref() {
            assert_ne!(serif.cache_id(), sans.cache_id());
        }
    }
}
