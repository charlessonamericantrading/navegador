//! Registro de la app en un fichero de texto (`registro.txt`, en la carpeta de
//! datos del navegador).
//!
//! En Windows la ventana no tiene consola: si algo falla (el motor no arranca,
//! una página no carga, el motor se cae) no se veía nada y no quedaba rastro.
//! Aquí queda una línea por suceso, con fecha. No guarda lo que se navega con
//! éxito, solo los fallos, y las direcciones van sin lo que sigue a `?` o `#`
//! (suelen llevar búsquedas y datos personales).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Al pasar de este tamaño el registro se aparta a `registro.antiguo.txt`.
const MAX_BYTES: u64 = 256 * 1024;

/// Dónde está el registro (`None` si el sistema no tiene carpeta de datos).
pub fn ruta() -> Option<PathBuf> {
    engine_core::process::profile_dir().map(|dir| dir.join("registro.txt"))
}

/// Apunta una línea. Nunca falla ni entra en pánico: si no se puede escribir,
/// no se escribe.
pub fn escribir(mensaje: &str) {
    if let Some(ruta) = ruta() {
        escribir_en(&ruta, mensaje);
    }
}

/// Como `escribir`, en un fichero concreto (lo usan las pruebas).
pub fn escribir_en(ruta: &Path, mensaje: &str) {
    if let Some(dir) = ruta.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::metadata(ruta).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let antiguo = ruta.with_file_name("registro.antiguo.txt");
        let _ = fs::remove_file(&antiguo);
        let _ = fs::rename(ruta, &antiguo);
    }
    let ahora = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let linea = format!("{} {}\n", fecha_utc(ahora), mensaje.replace(['\r', '\n'], " ⏎ "));
    if let Ok(mut fichero) = OpenOptions::new().create(true).append(true).open(ruta) {
        let _ = fichero.write_all(linea.as_bytes());
    }
}

/// Las últimas `max` líneas del registro, de la más vieja a la más nueva.
pub fn cola(ruta: &Path, max: usize) -> Vec<String> {
    let texto = fs::read_to_string(ruta).unwrap_or_default();
    let lineas: Vec<&str> = texto.lines().collect();
    lineas[lineas.len().saturating_sub(max)..].iter().map(|l| (*l).to_string()).collect()
}

/// Apunta en el registro lo que el motor dice que no va bien (los errores y su
/// caída). `destino` es la dirección a la que se iba, si se sabe.
pub fn mensaje_del_motor(mensaje: &crate::motor::MensajeMotor, destino: Option<&str>) {
    use crate::motor::MensajeMotor;
    match mensaje {
        // Una navegación cancelada es normal (otra página la sustituyó).
        MensajeMotor::Error { mensaje, .. } if mensaje.starts_with("navegacion_cancelada") => {}
        MensajeMotor::Error { id, mensaje } => {
            let donde = destino.map(|d| format!(" [{}]", sin_consulta(d))).unwrap_or_default();
            escribir(&format!("error del motor ({}){donde}: {mensaje}", id.as_deref().unwrap_or("sin id")));
        }
        MensajeMotor::Terminado => escribir("el proceso del motor terminó (se cerró o se cayó)"),
        _ => {}
    }
}

/// Una dirección sin consulta ni fragmento (`https://x.com/a?q=1#b` → `https://x.com/a`).
pub fn sin_consulta(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or("").to_string()
}

/// Segundos desde 1970 → `2026-11-26 16:54:09 UTC`.
pub fn fecha_utc(unix: u64) -> String {
    let dias = (unix / 86_400) as i64;
    let resto = unix % 86_400;
    let (anio, mes, dia) = civil_desde_dias(dias);
    format!("{anio:04}-{mes:02}-{dia:02} {:02}:{:02}:{:02} UTC", resto / 3600, resto % 3600 / 60, resto % 60)
}

/// Día del calendario civil a partir de los días desde 1970 (algoritmo de Howard Hinnant).
fn civil_desde_dias(dias: i64) -> (i64, u32, u32) {
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dia = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(mes <= 2), mes, dia)
}

/// Días desde 1970 de una fecha civil (la inversa de `civil_desde_dias`).
pub fn dias_desde_civil(anio: i64, mes: u32, dia: u32) -> i64 {
    let a = anio - i64::from(mes <= 2);
    let era = a.div_euclid(400);
    let yoe = a.rem_euclid(400);
    let mp = i64::from(if mes > 2 { mes - 3 } else { mes + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(dia) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carpeta(nombre: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("navegador-registro-{nombre}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn la_fecha_sale_bien() {
        assert_eq!(fecha_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(fecha_utc(1_788_023_649), "2026-08-29 17:14:09 UTC");
        // 29 de febrero de un año bisiesto.
        assert_eq!(fecha_utc(1_709_164_800), "2024-02-29 00:00:00 UTC");
    }

    #[test]
    fn dias_y_fecha_son_inversas() {
        for dias in [-1_i64, 0, 59, 60, 365, 11_016, 19_000, 20_600, 47_482] {
            let (a, m, d) = civil_desde_dias(dias);
            assert_eq!(dias_desde_civil(a, m, d), dias, "{a}-{m}-{d}");
        }
    }

    #[test]
    fn escribe_lineas_con_fecha_y_sin_saltos_dentro() {
        let dir = carpeta("lineas");
        let ruta = dir.join("registro.txt");
        escribir_en(&ruta, "primera");
        escribir_en(&ruta, "dos\nlineas");
        let texto = fs::read_to_string(&ruta).unwrap();
        let lineas: Vec<&str> = texto.lines().collect();
        assert_eq!(lineas.len(), 2, "cada suceso es una sola línea: {texto:?}");
        assert!(lineas[0].ends_with(" UTC primera"), "{}", lineas[0]);
        assert!(lineas[1].contains("dos ⏎ lineas"), "{}", lineas[1]);
        assert_eq!(cola(&ruta, 1), vec![lineas[1].to_string()]);
        assert_eq!(cola(&ruta, 10).len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_registro_enorme_se_aparta() {
        let dir = carpeta("rotar");
        fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("registro.txt");
        fs::write(&ruta, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        escribir_en(&ruta, "nuevo");
        assert!(dir.join("registro.antiguo.txt").is_file());
        assert_eq!(fs::read_to_string(&ruta).unwrap().lines().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_falla_si_no_se_puede_escribir() {
        // Una ruta imposible: no debe entrar en pánico.
        escribir_en(Path::new("/proc/no-existe/registro.txt"), "x");
        assert!(cola(Path::new("/proc/no-existe/registro.txt"), 5).is_empty());
    }

    #[test]
    fn quita_la_consulta_de_las_direcciones() {
        assert_eq!(sin_consulta("https://x.com/a?q=secreto#b"), "https://x.com/a");
        assert_eq!(sin_consulta("https://x.com/a"), "https://x.com/a");
    }
}
