/// El nombre "CrossPlatformTarget" describe una aspiracion, no un hecho
/// probado: este motor solo se ha compilado y ejecutado en Windows hasta
/// ahora. `winit`/`wgpu`/`softbuffer` (las dependencias de ventana/gpu/
/// blit) SI soportan macOS y Linux de escritorio en teoria, pero eso nunca
/// se ha verificado aqui - y iOS/Android necesitarian ademas su propio
/// empaquetado de app y entrada tactil, ninguno de los cuales existe en
/// este codigo. `print_target_info` por tanto solo imprime lo que
/// `std::env::consts` reporta del build ACTUAL, sin afirmar soporte de
/// ninguna otra plataforma.
pub struct CrossPlatformTarget;

impl CrossPlatformTarget {
    pub fn print_target_info() {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        tracing::info!("[Build] Target actual: {}-{} (compilado y probado solo en Windows hasta ahora - ver platform.rs)", os, arch);
    }
}
