//! `navegador --diagnostico` de punta a punta: el programa real, con su broker y
//! su motor, contra un servidor local. No usa internet ni abre ninguna ventana.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

fn servidor() -> u16 {
    let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
    let puerto = escucha.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conexion in escucha.incoming().flatten() {
            let mut c = conexion;
            let mut buf = [0u8; 2048];
            let _ = c.read(&mut buf);
            let cuerpo = "<html><head><title>Pagina de prueba</title></head><body><p>hola</p></body></html>";
            let r = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cuerpo}", cuerpo.len());
            let _ = c.write_all(r.as_bytes());
        }
    });
    puerto
}

fn perfil(nombre: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("navegador-diag-{nombre}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn diagnosticar(urls: &str, perfil: &PathBuf) -> (bool, String, String) {
    let salida = Command::new(env!("CARGO_BIN_EXE_navegador"))
        .arg("--diagnostico")
        .env("NAVEGADOR_DIAGNOSTICO_URLS", urls)
        .env("NAVEGADOR_DIAGNOSTICO_NO_ABRIR", "1")
        .env("NAVEGADOR_IA_PROFILE_DIR", perfil)
        .output()
        .expect("lanzar navegador --diagnostico");
    let informe = std::fs::read_to_string(perfil.join("diagnostico.txt")).unwrap_or_default();
    let registro = std::fs::read_to_string(perfil.join("registro.txt")).unwrap_or_default();
    (salida.status.success(), informe, registro)
}

#[test]
fn con_la_red_bien_el_informe_dice_que_todo_funciona() {
    let puerto = servidor();
    let dir = perfil("bien");
    let (ok, informe, registro) = diagnosticar(&format!("http://127.0.0.1:{puerto}/"), &dir);
    assert!(ok, "el diagnóstico debe terminar bien:\n{informe}");
    for parte in ["1) RED DIRECTA", "2) CON EL MOTOR COMPLETO", "3) PISTAS", "4) ÚLTIMAS LÍNEAS DEL REGISTRO"] {
        assert!(informe.contains(parte), "falta «{parte}»:\n{informe}");
    }
    assert!(informe.contains("página: bien (estado 200"), "la prueba directa debe ir bien:\n{informe}");
    assert!(informe.contains("bien → http://127.0.0.1"), "la prueba del motor completo debe ir bien:\n{informe}");
    assert!(informe.contains("Pagina de prueba"), "el motor cargó la página y su título:\n{informe}");
    assert!(informe.contains("YouTube"), "las pistas avisan de lo que aún no funciona:\n{informe}");
    assert!(!informe.contains("FALLA"), "no debe haber fallos:\n{informe}");
    assert!(registro.contains("diagnóstico: empieza") && registro.contains("diagnóstico: guardado"), "queda rastro en el registro:\n{registro}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn con_un_servidor_caido_el_informe_dice_donde_falla_y_el_registro_lo_apunta() {
    let cerrado = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = perfil("caido");
    let (ok, informe, registro) = diagnosticar(&format!("http://127.0.0.1:{cerrado}/"), &dir);
    assert!(ok, "aunque todo falle, el diagnóstico termina y cuenta qué pasó:\n{informe}");
    assert!(informe.contains("conexión: FALLA"), "la conexión directa falla:\n{informe}");
    assert!(informe.contains("FALLA"), "{informe}");
    assert!(informe.contains("cortafuegos"), "las pistas explican el bloqueo:\n{informe}");
    assert!(registro.contains("error del motor"), "el error del motor queda en el registro:\n{registro}");
    assert!(registro.contains(&format!("127.0.0.1:{cerrado}")), "con la dirección a la que iba:\n{registro}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn el_informe_no_enseña_la_carpeta_personal() {
    let puerto = servidor();
    let dir = perfil("privado");
    let falsa = dir.join("usuario-falso");
    std::fs::create_dir_all(&falsa).unwrap();
    let salida = Command::new(env!("CARGO_BIN_EXE_navegador"))
        .arg("--diagnostico")
        .env("NAVEGADOR_DIAGNOSTICO_URLS", format!("http://127.0.0.1:{puerto}/"))
        .env("NAVEGADOR_DIAGNOSTICO_NO_ABRIR", "1")
        .env("NAVEGADOR_IA_PROFILE_DIR", falsa.join("datos"))
        .env("HOME", &falsa)
        .env("USERPROFILE", &falsa)
        .output()
        .unwrap();
    assert!(salida.status.success());
    let informe = std::fs::read_to_string(falsa.join("datos").join("diagnostico.txt")).unwrap();
    assert!(!informe.contains(falsa.to_str().unwrap()), "la carpeta personal no debe aparecer:\n{informe}");
    assert!(informe.contains("%USUARIO%"), "{informe}");
    let _ = std::fs::remove_dir_all(&dir);
}
