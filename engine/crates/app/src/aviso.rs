//! Un cuadro de aviso para lo que no se puede dejar pasar en silencio.
//!
//! En Windows el navegador no tiene consola: si no arranca o falla algo grave,
//! sin esto el usuario solo ve que "no pasa nada". Fuera de Windows se escribe
//! en la salida de error.

/// Enseña `texto`. Si `error` es verdadero lleva el icono de error. Bloquea
/// hasta que se cierra, así que no se llama desde el hilo de la ventana.
pub fn mostrar(titulo: &str, texto: &str, error: bool) {
    #[cfg(windows)]
    ventana(titulo, texto, error);
    #[cfg(not(windows))]
    {
        let _ = error;
        eprintln!("[{titulo}] {texto}");
    }
}

/// Como `mostrar`, pero en otro hilo: no espera a que el usuario lo cierre.
pub fn mostrar_sin_esperar(titulo: &str, texto: &str, error: bool) {
    let (titulo, texto) = (titulo.to_string(), texto.to_string());
    let _ = std::thread::Builder::new().name("aviso".to_string()).spawn(move || mostrar(&titulo, &texto, error));
}

#[cfg(windows)]
fn ventana(titulo: &str, texto: &str, error: bool) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    const MB_OK: u32 = 0x0000_0000;
    const MB_ICONERROR: u32 = 0x0000_0010;
    const MB_ICONINFORMATION: u32 = 0x0000_0040;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;
    let ancho = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (t, m) = (ancho(titulo), ancho(texto));
    let icono = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: los dos textos acaban en cero y viven hasta que vuelve la llamada.
    unsafe {
        let _ = MessageBoxW(0, m.as_ptr(), t.as_ptr(), MB_OK | icono | MB_SETFOREGROUND);
    }
}
