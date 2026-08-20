//! Mitigaciones de proceso para el motor (Fase 23).
//!
//! # Lo que esto ES y lo que NO ES
//!
//! **NO es un sandbox.** Un sandbox de verdad (el de Chromium) separa el
//! proceso que interpreta contenido hostil del que tiene permisos: el
//! renderizador corre con un token restringido que no puede leer los
//! ficheros del usuario, y un proceso "broker" hace por el las operaciones
//! privilegiadas (red, disco). Aqui `engine_server` hace su propia red y
//! su propio disco, asi que restringir su token lo romperia; separarlos es
//! un refactor arquitectonico grande, no una bandera que activar.
//!
//! **Lo que SI es**: un conjunto de mitigaciones de explotacion que se
//! pueden aplicar al propio proceso, sin broker y sin cambiar la
//! arquitectura, y que rompen las cadenas de explotacion mas comunes. No
//! sustituyen al sandbox - lo complementan - y estan aqui porque son
//! defensa real y verificable HOY, no porque cierren el hueco.
//!
//! Decirlo asi de claro importa: una mitigacion presentada como sandbox da
//! una falsa sensacion de seguridad, que es peor que no tener nada.
//!
//! # Las mitigaciones
//!
//! - **Prohibir codigo dinamico** (`ProcessDynamicCodePolicy`): impide que
//!   el proceso cree o modifique paginas de memoria ejecutables. Bloquea
//!   de raiz la tecnica clasica de "escribir shellcode y saltar a el".
//!   **Este motor puede permitirselo y un navegador comercial no**: su
//!   JavaScript corre en `boa`, un INTERPRETE, sin JIT - y un JIT necesita
//!   exactamente eso que aqui se prohibe. Es una ventaja real que sale de
//!   una decision de diseño previa, no un extra.
//! - **Prohibir procesos hijo** (`ProcessChildProcessPolicy`): casi toda
//!   cadena de explotacion termina lanzando `cmd.exe`/`powershell.exe`.
//!   `engine_server` no lanza procesos nunca, asi que prohibirlo no le
//!   cuesta nada y corta ese final.
//! - **Desactivar puntos de extension**
//!   (`ProcessExtensionPointDisablePolicy`): bloquea mecanismos heredados
//!   de inyeccion de DLL (AppInit_DLLs, ganchos SetWindowsHookEx, capas
//!   de proveedor Winsock).
//!
//! Deliberadamente NO se aplica `ProcessSignaturePolicy`
//! (solo-DLL-firmadas-por-Microsoft): romperia la carga de controladores
//! graficos de terceros, y aqui seria adivinar en vez de saber.
//!
//! En plataformas que no son Windows esto es un no-op honesto que lo
//! declara en su informe, en vez de fingir que aplico algo.

/// Que se llego a aplicar de verdad. Se devuelve (en vez de solo registrar
/// en el log) para que quien arranca el proceso pueda informarlo, y para
/// que los tests puedan comprobarlo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MitigationReport {
    pub dynamic_code_blocked: bool,
    pub child_processes_blocked: bool,
    pub extension_points_disabled: bool,
    /// Plataforma sin soporte (todo lo de arriba en `false` y esto en
    /// `true`) - se distingue de "se intento y fallo" a proposito.
    pub unsupported_platform: bool,
}

impl MitigationReport {
    /// Resumen de una linea para el log de arranque. Nombra explicitamente
    /// que esto no es un sandbox, para que nadie lo lea de mas al verlo.
    pub fn summary(&self) -> String {
        if self.unsupported_platform {
            return "mitigaciones de proceso: no disponibles en esta plataforma (no es un sandbox en ningun caso)".to_string();
        }
        let aplicadas: Vec<&str> = [
            self.dynamic_code_blocked.then_some("codigo-dinamico"),
            self.child_processes_blocked.then_some("procesos-hijo"),
            self.extension_points_disabled.then_some("puntos-de-extension"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if aplicadas.is_empty() {
            "mitigaciones de proceso: ninguna pudo aplicarse".to_string()
        } else {
            format!("mitigaciones de proceso aplicadas: {} (NO es un sandbox - ver core::sandbox)", aplicadas.join(", "))
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::MitigationReport;

    // Valores de `PROCESS_MITIGATION_POLICY` (winnt.h). Se escriben aqui
    // en vez de importarse porque `windows-sys` solo los expone bajo
    // features que este crate no necesita para nada mas, y son constantes
    // estables del SO desde Windows 8.
    const PROCESS_DYNAMIC_CODE_POLICY: i32 = 2;
    const PROCESS_EXTENSION_POINT_DISABLE_POLICY: i32 = 6;
    const PROCESS_CHILD_PROCESS_POLICY: i32 = 13;

    // Cada estructura de politica es, en la practica, un unico DWORD de
    // banderas: se pasa como `u32` en vez de declarar las tres structs.
    const PROHIBIT_DYNAMIC_CODE: u32 = 0x1;
    const DISABLE_EXTENSION_POINTS: u32 = 0x1;
    const NO_CHILD_PROCESS_CREATION: u32 = 0x1;

    extern "system" {
        fn SetProcessMitigationPolicy(policy: i32, buffer: *const core::ffi::c_void, length: usize) -> i32;
    }

    fn apply(policy: i32, flags: u32) -> bool {
        // SAFETY: `SetProcessMitigationPolicy` lee `length` bytes de
        // `buffer`. Se le pasa la direccion de un `u32` vivo en la pila y
        // exactamente `size_of::<u32>()`, que es el tamaño real de las
        // tres estructuras de politica que se usan aqui.
        unsafe { SetProcessMitigationPolicy(policy, (&flags as *const u32).cast(), core::mem::size_of::<u32>()) != 0 }
    }

    pub fn apply_process_mitigations() -> MitigationReport {
        // Cada una se intenta por separado y un fallo no impide las demas:
        // una version de Windows puede no soportar una politica concreta,
        // y perder las otras dos por eso seria absurdo.
        MitigationReport {
            dynamic_code_blocked: apply(PROCESS_DYNAMIC_CODE_POLICY, PROHIBIT_DYNAMIC_CODE),
            child_processes_blocked: apply(PROCESS_CHILD_PROCESS_POLICY, NO_CHILD_PROCESS_CREATION),
            extension_points_disabled: apply(PROCESS_EXTENSION_POINT_DISABLE_POLICY, DISABLE_EXTENSION_POINTS),
            unsupported_platform: false,
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::MitigationReport;

    /// No-op honesto: macOS y Linux tienen equivalentes (`seccomp-bpf`,
    /// Seatbelt) pero este motor nunca se ha compilado ni probado ahi (ver
    /// `core::platform`), asi que fingir que se aplico algo seria
    /// exactamente el tipo de mentira que este proyecto evita.
    pub fn apply_process_mitigations() -> MitigationReport {
        MitigationReport { unsupported_platform: true, ..Default::default() }
    }
}

/// Aplica las mitigaciones al proceso actual. Idempotente en la practica:
/// volver a aplicarlas no falla ni afloja nada (las politicas de Windows
/// no se pueden relajar una vez puestas, solo endurecer, lo cual es
/// precisamente lo que las hace utiles).
pub fn apply_process_mitigations() -> MitigationReport {
    imp::apply_process_mitigations()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_always_says_it_is_not_a_sandbox() {
        let aplicado = MitigationReport { dynamic_code_blocked: true, ..Default::default() };
        assert!(aplicado.summary().contains("NO es un sandbox"), "el resumen nunca debe dar a entender que esto es un sandbox: {}", aplicado.summary());

        let sin_soporte = MitigationReport { unsupported_platform: true, ..Default::default() };
        assert!(sin_soporte.summary().contains("no es un sandbox"));
    }

    #[test]
    fn the_summary_names_each_applied_mitigation() {
        let todas = MitigationReport {
            dynamic_code_blocked: true,
            child_processes_blocked: true,
            extension_points_disabled: true,
            unsupported_platform: false,
        };
        let s = todas.summary();
        assert!(s.contains("codigo-dinamico") && s.contains("procesos-hijo") && s.contains("puntos-de-extension"));
    }

    #[test]
    fn a_report_with_nothing_applied_says_so_instead_of_pretending() {
        assert!(MitigationReport::default().summary().contains("ninguna pudo aplicarse"));
    }

    /// En Windows deberian aplicarse de verdad. Se comprueba el efecto
    /// REAL y no solo el valor devuelto: tras prohibir procesos hijo,
    /// lanzar uno tiene que fallar.
    #[cfg(windows)]
    #[test]
    fn on_windows_the_mitigations_actually_take_effect() {
        let report = apply_process_mitigations();
        assert!(!report.unsupported_platform);
        assert!(report.child_processes_blocked, "prohibir procesos hijo deberia aplicarse en Windows 10+");

        // El efecto observable: ya no se puede lanzar un proceso. Esto es
        // lo que corta la cola de casi toda cadena de explotacion.
        let intento = std::process::Command::new("cmd.exe").args(["/C", "echo", "hola"]).output();
        assert!(intento.is_err(), "tras la mitigacion, lanzar un proceso hijo deberia fallar de verdad, no solo reportarse como aplicada");
    }
}
