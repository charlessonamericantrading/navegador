//! Acceso mínimo al portapapeles del sistema (Windows por Win32, macOS por
//! `pbcopy`/`pbpaste`, Linux por `wl-copy`/`xclip`/`xsel`).
//!
//! En pruebas unitarias `App` usa un búfer en memoria propio para no tocar el
//! portapapeles real del usuario ni depender de un servidor gráfico en CI.

#[cfg(test)]
use std::sync::Mutex;

pub(crate) enum Portapapeles {
    Sistema,
    #[cfg(test)]
    Memoria(Mutex<String>),
}

impl Portapapeles {
    pub(crate) fn leer(&self) -> Option<String> {
        match self {
            Self::Sistema => leer_sistema(),
            #[cfg(test)]
            Self::Memoria(texto) => texto.lock().ok().map(|s| s.clone()).filter(|s| !s.is_empty()),
        }
    }

    pub(crate) fn escribir(&self, texto: &str) -> bool {
        match self {
            Self::Sistema => escribir_sistema(texto),
            #[cfg(test)]
            Self::Memoria(guardado) => {
                if let Ok(mut g) = guardado.lock() {
                    *g = texto.to_string();
                    true
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hWndNewOwner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(uFormat: u32) -> *mut c_void;
        fn SetClipboardData(uFormat: u32, hMem: *mut c_void) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut c_void;
        fn GlobalLock(hMem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hMem: *mut c_void) -> i32;
        fn GlobalFree(hMem: *mut c_void) -> *mut c_void;
    }

    struct CierrePortapapeles;
    impl Drop for CierrePortapapeles {
        fn drop(&mut self) {
            // SAFETY: solo se construye tras `OpenClipboard` con éxito.
            unsafe {
                CloseClipboard();
            }
        }
    }

    pub(super) fn leer() -> Option<String> {
        // SAFETY: llamadas estándar de Win32 con puntero nulo (= tarea actual),
        // validando handles no nulos antes de desreferenciar memoria bloqueada.
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return None;
            }
            let _cierre = CierrePortapapeles;
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                return None;
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let texto = String::from_utf16_lossy(slice);
            GlobalUnlock(handle);
            Some(texto)
        }
    }

    pub(super) fn escribir(texto: &str) -> bool {
        let mut utf16: Vec<u16> = texto.encode_utf16().collect();
        utf16.push(0);
        let bytes = utf16.len() * std::mem::size_of::<u16>();

        // SAFETY: asignamos memoria global movible del tamaño exacto del UTF-16
        // terminado en NUL; si `SetClipboardData` tiene éxito, el sistema pasa a
        // ser dueño del bloque; si falla, lo liberamos nosotros.
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return false;
            }
            let _cierre = CierrePortapapeles;
            if EmptyClipboard() == 0 {
                return false;
            }
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if hmem.is_null() {
                return false;
            }
            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                GlobalFree(hmem);
                return false;
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
            GlobalUnlock(hmem);
            if SetClipboardData(CF_UNICODETEXT, hmem).is_null() {
                GlobalFree(hmem);
                return false;
            }
            true
        }
    }
}

#[cfg(windows)]
fn leer_sistema() -> Option<String> {
    win32::leer()
}

#[cfg(windows)]
fn escribir_sistema(texto: &str) -> bool {
    win32::escribir(texto)
}

#[cfg(not(windows))]
fn leer_sistema() -> Option<String> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let candidatos: &[(&str, &[&str])] = &[("pbpaste", &[])];
    #[cfg(not(target_os = "macos"))]
    let candidatos: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    for (bin, args) in candidatos {
        if let Ok(out) = Command::new(bin).args(*args).output() {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn escribir_sistema(texto: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let candidatos: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(not(target_os = "macos"))]
    let candidatos: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (bin, args) in candidatos {
        if let Ok(mut child) = Command::new(bin).args(*args).stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(texto.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return true;
            }
        }
    }
    false
}
