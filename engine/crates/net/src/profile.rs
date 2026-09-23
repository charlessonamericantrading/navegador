//! Directorio del perfil: donde se guardan cookies y `localStorage`.
//!
//! Por defecto, `<datos del usuario>/navegador-ia` (`%APPDATA%` en Windows).
//! `NAVEGADOR_IA_PROFILE_DIR` lo sustituye entero. Existe para que las pruebas
//! y el benchmark (plan F03, F39) no lean ni escriban el perfil real del
//! usuario, y es el primer paso de los perfiles separados de F31.

use std::path::PathBuf;

/// Variable de entorno que redirige el perfil.
pub const PROFILE_DIR_ENV: &str = "NAVEGADOR_IA_PROFILE_DIR";

/// El directorio del perfil, o `None` si no se puede determinar (ni variable
/// ni directorio de datos del sistema): se trata como «sin persistencia».
pub fn profile_dir() -> Option<PathBuf> {
    resolve(std::env::var_os(PROFILE_DIR_ENV).map(PathBuf::from), dirs::data_dir())
}

/// Regla pura, separada para probarla sin tocar el entorno del proceso. Una
/// variable vacía cuenta como ausente: `NAVEGADOR_IA_PROFILE_DIR=` no debe
/// acabar escribiendo en el directorio de trabajo.
fn resolve(override_dir: Option<PathBuf>, system_data_dir: Option<PathBuf>) -> Option<PathBuf> {
    match override_dir {
        Some(dir) if !dir.as_os_str().is_empty() => Some(dir),
        _ => Some(system_data_dir?.join("navegador-ia")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_replaces_the_whole_directory() {
        assert_eq!(resolve(Some(PathBuf::from("/tmp/perfil")), Some(PathBuf::from("/datos"))), Some(PathBuf::from("/tmp/perfil")));
    }

    #[test]
    fn without_override_it_is_the_system_data_dir_plus_the_app_folder() {
        assert_eq!(resolve(None, Some(PathBuf::from("/datos"))), Some(PathBuf::from("/datos").join("navegador-ia")));
    }

    #[test]
    fn an_empty_override_counts_as_absent() {
        assert_eq!(resolve(Some(PathBuf::new()), Some(PathBuf::from("/datos"))), Some(PathBuf::from("/datos").join("navegador-ia")));
    }

    #[test]
    fn no_directory_at_all_means_no_persistence() {
        assert_eq!(resolve(None, None), None);
    }
}
