//! Devolver al sistema operativo la memoria que el motor ya no usa.
//!
//! Liberar un `Vec` o descartar una pestaña entera no baja el consumo que ve el
//! usuario en el Administrador de tareas: el asignador de memoria se queda con
//! las páginas libres "por si acaso". Tras descartar una pestaña se le pide que
//! las devuelva de verdad.

/// Pide al sistema que recupere la memoria libre del proceso. Barato y sin
/// efectos si no hay nada que devolver; no falla nunca.
pub fn liberar_al_sistema() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: `malloc_trim` no tiene precondiciones: solo pide a glibc que
        // devuelva al sistema las páginas libres de su montón.
        unsafe {
            malloc_trim(0);
        }
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn SetProcessWorkingSetSize(process: *mut c_void, minimum: usize, maximum: usize) -> i32;
        }
        // SAFETY: con `usize::MAX` en los dos límites Windows recorta el conjunto
        // de trabajo del proceso (devuelve a la RAM libre lo que no se usa); el
        // pseudo-manejador de `GetCurrentProcess` es siempre válido. Si falla
        // (por ejemplo bajo el aislamiento) no pasa nada: es solo una ayuda.
        unsafe {
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn liberar_memoria_no_falla_ni_se_cuelga() {
        let grande = vec![1u8; 32 * 1024 * 1024];
        assert_eq!(grande.iter().map(|b| u64::from(*b)).sum::<u64>(), 32 * 1024 * 1024);
        drop(grande);
        super::liberar_al_sistema();
        super::liberar_al_sistema();
    }
}
