//! La app entera con el motor de verdad, sin ventana y sin ningún navegador
//! externo: se escribe en la barra, se pulsan enlaces, se rellena un
//! formulario, se usan las pestañas y se ve la página de error.

use navegador::app::{Entrada, Modificadores, Tecla};
use navegador::barra::ALTO_BARRA;
use navegador::sin_ventana::Sesion;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

const LIMITE: Duration = Duration::from_secs(60);

/// Servidor HTTP mínimo con tres páginas.
fn servidor() -> u16 {
    let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
    let puerto = escucha.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conexion in escucha.incoming() {
            let Ok(mut conexion) = conexion else { continue };
            std::thread::spawn(move || {
                let mut lector = BufReader::new(conexion.try_clone().unwrap());
                let mut primera = String::new();
                let _ = lector.read_line(&mut primera);
                loop {
                    let mut linea = String::new();
                    if lector.read_line(&mut linea).unwrap_or(0) == 0 || linea.trim().is_empty() {
                        break;
                    }
                }
                let ruta = primera.split_whitespace().nth(1).unwrap_or("/").to_string();
                let cuerpo = if ruta.starts_with("/buscar") {
                    let q = ruta.split("q=").nth(1).unwrap_or("").to_string();
                    format!("<html><head><title>Buscar: {q}</title></head><body><p>Resultados de {q}</p></body></html>")
                } else if ruta == "/dos" {
                    "<html><head><title>Dos</title></head><body style=\"margin:0\"><div style=\"width:300px;height:60px;background:#00ff00\"></div><p>Página dos</p></body></html>".to_string()
                } else {
                    "<html><head><title>Inicio</title></head><body style=\"margin:0\">\
                     <a href=\"/dos\" style=\"display:block;width:200px;height:40px;background:#cccccc\">Ir a dos</a>\
                     <form action=\"/buscar\"><input name=\"q\" style=\"width:300px;height:30px\"></form>\
                     </body></html>"
                        .to_string()
                };
                let respuesta = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cuerpo}",
                    cuerpo.len()
                );
                let _ = conexion.write_all(respuesta.as_bytes());
            });
        }
    });
    puerto
}

fn teclas(sesion: &mut Sesion, texto: &str) {
    for c in texto.chars() {
        sesion.entrada(Entrada::Tecla { tecla: Tecla::Caracter(c.to_string()), mods: Modificadores::default() }).unwrap();
    }
}

fn tecla(sesion: &mut Sesion, tecla: Tecla, mods: Modificadores) {
    sesion.entrada(Entrada::Tecla { tecla, mods }).unwrap();
}

fn ctrl() -> Modificadores {
    Modificadores { ctrl: true, ..Default::default() }
}

fn esperar(sesion: &mut Sesion) {
    assert!(sesion.esperar_reposo(LIMITE), "el motor no contestó a tiempo");
}

/// Pulsa en el centro del primer elemento con esa etiqueta.
fn pulsar(sesion: &mut Sesion, etiqueta: &str) {
    let app = sesion.app();
    let e = app.elementos().iter().find(|e| e.etiqueta == etiqueta).unwrap_or_else(|| panic!("no hay <{etiqueta}>: {:?}", app.elementos())).clone();
    let (x, y) = (e.x + e.ancho / 2.0, e.y + e.alto / 2.0 - app.scroll_y() + ALTO_BARRA as f32);
    sesion.entrada(Entrada::Clic { x, y }).unwrap();
}

#[test]
fn navegar_escribir_pestanas_y_errores_con_el_motor_real() {
    let puerto = servidor();
    let mut sesion = Sesion::nueva(Path::new(env!("CARGO_BIN_EXE_navegador")), 900, 600, None).unwrap();
    esperar(&mut sesion);
    assert_eq!(sesion.app().editando_direccion(), Some(""), "arranca con la barra lista para escribir");

    // 1. Escribir la dirección y Enter.
    teclas(&mut sesion, &format!("127.0.0.1:{puerto}/"));
    tecla(&mut sesion, Tecla::Enter, Modificadores::default());
    esperar(&mut sesion);
    assert_eq!(sesion.app().titulo(), "Inicio");
    assert!(sesion.app().url().ends_with(&format!("{puerto}/")), "url: {}", sesion.app().url());

    // La página se ve debajo de la barra: el enlace es gris.
    let marco = sesion.componer();
    let p = marco.pixel(20, ALTO_BARRA + 20).unwrap();
    assert_eq!((p.red(), p.green(), p.blue()), (0xcc, 0xcc, 0xcc));

    // 2. Pulsar el enlace.
    pulsar(&mut sesion, "a");
    esperar(&mut sesion);
    assert_eq!(sesion.app().titulo(), "Dos");
    let marco = sesion.componer();
    let p = marco.pixel(20, ALTO_BARRA + 20).unwrap();
    assert_eq!((p.red(), p.green(), p.blue()), (0, 255, 0));

    // 3. Atrás con Alt+Izquierda.
    tecla(&mut sesion, Tecla::Izquierda, Modificadores { alt: true, ..Default::default() });
    esperar(&mut sesion);
    assert_eq!(sesion.app().titulo(), "Inicio");

    // 4. Escribir en el campo de la página y enviar el formulario con Enter.
    pulsar(&mut sesion, "input");
    esperar(&mut sesion);
    assert!(sesion.app().escribiendo_en_pagina());
    teclas(&mut sesion, "hola");
    tecla(&mut sesion, Tecla::Enter, Modificadores::default());
    esperar(&mut sesion);
    assert_eq!(sesion.app().titulo(), "Buscar: hola");

    // 5. Pestaña nueva, otra página y cerrar.
    tecla(&mut sesion, Tecla::Caracter("t".into()), ctrl());
    esperar(&mut sesion);
    assert_eq!(sesion.app().pestanas().len(), 2);
    teclas(&mut sesion, &format!("127.0.0.1:{puerto}/dos"));
    tecla(&mut sesion, Tecla::Enter, Modificadores::default());
    esperar(&mut sesion);
    assert_eq!(sesion.app().titulo(), "Dos");
    tecla(&mut sesion, Tecla::Caracter("w".into()), ctrl());
    esperar(&mut sesion);
    assert_eq!(sesion.app().pestanas().len(), 1);
    assert_eq!(sesion.app().titulo(), "Buscar: hola", "vuelve a la primera pestaña");

    // 6. Una dirección que no contesta enseña la página de error.
    let cerrado = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    tecla(&mut sesion, Tecla::Caracter("l".into()), ctrl());
    teclas(&mut sesion, &format!("127.0.0.1:{cerrado}/"));
    tecla(&mut sesion, Tecla::Enter, Modificadores::default());
    esperar(&mut sesion);
    assert!(sesion.app().error_visible().is_some(), "debía verse la página de error");
    sesion.componer();

    sesion.cerrar();
}
