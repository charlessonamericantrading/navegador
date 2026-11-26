//! Mitigaciones de proceso para el motor (Fase 23).
//!
//! # Lo que esto ES y lo que NO ES
//!
//! **NO es un sandbox.** Un sandbox de verdad (el de los navegadores grandes) separa el
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
    pub untrusted_fonts_disabled: bool,
    pub system32_images_preferred: bool,
    pub strict_handle_checks_enabled: bool,
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
            self.untrusted_fonts_disabled.then_some("fuentes-no-confiables"),
            self.system32_images_preferred.then_some("imagenes-system32"),
            self.strict_handle_checks_enabled.then_some("comprobacion-handles"),
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
    const PROCESS_FONT_DISABLE_POLICY: i32 = 9;
    const PROCESS_IMAGE_LOAD_POLICY: i32 = 10;
    const PROCESS_STRICT_HANDLE_CHECK_POLICY: i32 = 11;
    const PROCESS_CHILD_PROCESS_POLICY: i32 = 13;

    // Cada estructura de politica es, en la practica, un unico DWORD de
    // banderas: se pasa como `u32` en vez de declarar las tres structs.
    const PROHIBIT_DYNAMIC_CODE: u32 = 0x1;
    const DISABLE_EXTENSION_POINTS: u32 = 0x1;
    const DISABLE_UNTRUSTED_FONTS: u32 = 0x1;
    const PREFER_SYSTEM32_IMAGES: u32 = 0x1;
    const RAISE_EXCEPTION_ON_INVALID_HANDLE: u32 = 0x1;
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
            untrusted_fonts_disabled: apply(PROCESS_FONT_DISABLE_POLICY, DISABLE_UNTRUSTED_FONTS),
            system32_images_preferred: apply(PROCESS_IMAGE_LOAD_POLICY, PREFER_SYSTEM32_IMAGES),
            strict_handle_checks_enabled: apply(PROCESS_STRICT_HANDLE_CHECK_POLICY, RAISE_EXCEPTION_ON_INVALID_HANDLE),
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

/// Proceso hijo del motor supervisado: en Windows se lanza con token
/// restringido a nivel de integridad baja (`S-1-16-4096`) y encerrado en un
/// *Job Object* (`KILL_ON_JOB_CLOSE`, tope de memoria, un solo proceso y
/// restricciones de UI).
pub struct SandboxChild {
    inner: child_imp::Inner,
}

impl SandboxChild {
    pub fn take_stdin(&mut self) -> Option<std::fs::File> {
        self.inner.take_stdin()
    }

    pub fn take_stdout(&mut self) -> Option<std::fs::File> {
        self.inner.take_stdout()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        self.inner.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.inner.kill()
    }

    pub fn wait(&mut self) -> std::io::Result<i32> {
        self.inner.wait()
    }

    pub fn is_isolated(&self) -> bool {
        self.inner.is_isolated()
    }
}

/// Lanza un subproceso del motor con el aislamiento disponible en el sistema
/// (en Windows: token restringido a integridad baja, escritorio aislado
/// dedicado y Job Object; falla cerrado si no puede aplicarlo).
pub fn spawn_isolated_process(
    executable: &std::path::Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> std::io::Result<SandboxChild> {
    child_imp::spawn_isolated_process(executable, args, envs).map(|inner| SandboxChild { inner })
}

/// Protege el directorio de perfil frente a procesos de integridad baja
/// (`S:(ML;OICI;NWNR;;;ME)`: sin lectura ni escritura desde integridad baja).
pub fn harden_profile_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    child_imp::harden_profile_dir(dir)
}

#[cfg(windows)]
mod child_imp {
    use core::ffi::c_void;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    type Handle = *mut c_void;

    const DISABLE_MAX_PRIVILEGE: u32 = 0x1;
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_DUPLICATE: u32 = 0x0002;
    const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
    const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
    const TOKEN_USER: u32 = 1;
    const TOKEN_INTEGRITY_LEVEL: u32 = 25;
    const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
    const GENERIC_ALL: u32 = 0x1000_0000;

    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const STILL_ACTIVE: u32 = 259;
    const INFINITE: u32 = 0xFFFF_FFFF;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_BASIC_UI_RESTRICTIONS: i32 = 4;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;
    const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x0000_0400;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    // Bloquea acceso directo al portapapeles y apagado del sistema desde el renderer
    // (sin `UILIMIT_DESKTOP`/`UILIMIT_GLOBALATOMS`, que rompen la carga inicial de DLLs de sistema).
    const UI_RESTRICTIONS_MASK: u32 = 0x0000_0086;

    const SDDL_REVISION_1: u32 = 1;
    const SE_FILE_OBJECT: u32 = 1;
    const LABEL_SECURITY_INFORMATION: u32 = 0x0000_0010;
    const DEFAULT_RENDERER_MEM_MB: usize = 2048;

    #[repr(C)]
    struct SECURITY_ATTRIBUTES {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: i32,
    }

    #[repr(C)]
    struct SID_AND_ATTRIBUTES {
        sid: *mut c_void,
        attributes: u32,
    }

    #[repr(C)]
    struct TOKEN_MANDATORY_LABEL {
        label: SID_AND_ATTRIBUTES,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        lp_reserved: *mut u16,
        lp_desktop: *mut u16,
        lp_title: *mut u16,
        dw_x: u32,
        dw_y: u32,
        dw_x_size: u32,
        dw_y_size: u32,
        dw_x_count_chars: u32,
        dw_y_count_chars: u32,
        dw_fill_attribute: u32,
        dw_flags: u32,
        w_show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *mut u8,
        h_std_input: Handle,
        h_std_output: Handle,
        h_std_error: Handle,
    }

    #[repr(C)]
    struct PROCESS_INFORMATION {
        h_process: Handle,
        h_thread: Handle,
        dw_process_id: u32,
        dw_thread_id: u32,
    }

    #[repr(C)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IO_COUNTERS {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION_DATA {
        basic_limit_information: JOBOBJECT_BASIC_LIMIT_INFORMATION,
        io_info: IO_COUNTERS,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn CreateRestrictedToken(
            existing_token: Handle,
            flags: u32,
            disable_sid_count: u32,
            sids_to_disable: *const c_void,
            delete_privilege_count: u32,
            privileges_to_delete: *const c_void,
            restricted_sid_count: u32,
            sids_to_restrict: *const c_void,
            new_token: *mut Handle,
        ) -> i32;
        fn GetTokenInformation(
            token: Handle,
            class: u32,
            info: *mut c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
        fn SetTokenInformation(token: Handle, class: u32, info: *const c_void, len: u32) -> i32;
        fn ConvertSidToStringSidW(sid: *mut c_void, string_sid: *mut *mut u16) -> i32;
        fn ConvertStringSidToSidW(string_sid: *const u16, sid: *mut *mut c_void) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_sd: *const u16,
            revision: u32,
            sd: *mut *mut c_void,
            sd_size: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorSacl(
            sd: *mut c_void,
            lpb_sacl_present: *mut i32,
            p_sacl: *mut *mut c_void,
            lpb_sacl_defaulted: *mut i32,
        ) -> i32;
        fn SetNamedSecurityInfoW(
            object_name: *mut u16,
            object_type: u32,
            security_info: u32,
            psid_owner: *mut c_void,
            psid_group: *mut c_void,
            p_dacl: *mut c_void,
            p_sacl: *mut c_void,
        ) -> u32;
        fn CreateProcessAsUserW(
            token: Handle,
            app_name: *const u16,
            command_line: *mut u16,
            process_attrs: *const SECURITY_ATTRIBUTES,
            thread_attrs: *const SECURITY_ATTRIBUTES,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *const c_void,
            current_dir: *const u16,
            startup_info: *const StartupInfoW,
            process_info: *mut PROCESS_INFORMATION,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
        fn CreatePipe(read: *mut Handle, write: *mut Handle, attrs: *const SECURITY_ATTRIBUTES, size: u32) -> i32;
        fn SetHandleInformation(object: Handle, mask: u32, flags: u32) -> i32;
        fn DuplicateHandle(
            src_proc: Handle,
            src_handle: Handle,
            dst_proc: Handle,
            dst_handle: *mut Handle,
            desired_access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn GetStdHandle(which: u32) -> Handle;
        fn CreateJobObjectW(attrs: *const SECURITY_ATTRIBUTES, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn ResumeThread(thread: Handle) -> u32;
        fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn WaitForSingleObject(handle: Handle, millis: u32) -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn CreateDesktopW(
            desktop: *const u16,
            device: *const u16,
            devmode: *const c_void,
            flags: u32,
            desired_access: u32,
            sa: *const SECURITY_ATTRIBUTES,
        ) -> Handle;
        fn CloseDesktop(desktop: Handle) -> i32;
    }

    struct OwnedHandle(Handle);
    unsafe impl Send for OwnedHandle {}
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct OwnedDesktop(Handle);
    unsafe impl Send for OwnedDesktop {}
    impl Drop for OwnedDesktop {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseDesktop(self.0) };
            }
        }
    }

    pub struct Inner {
        process: Handle,
        job: Handle,
        desktop: Handle,
        stdin: Option<File>,
        stdout: Option<File>,
        exited: Option<i32>,
    }

    // SAFETY: Los HANDLE de proceso y Job de Windows son seguros para enviar
    // entre hilos cuando pertenecen exclusivamente a `Inner`.
    unsafe impl Send for Inner {}

    impl Inner {
        pub fn take_stdin(&mut self) -> Option<File> {
            self.stdin.take()
        }

        pub fn take_stdout(&mut self) -> Option<File> {
            self.stdout.take()
        }

        pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
            if let Some(code) = self.exited {
                return Ok(Some(code));
            }
            let mut code = 0u32;
            if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if code == STILL_ACTIVE {
                let wait = unsafe { WaitForSingleObject(self.process, 0) };
                if wait != 0 {
                    return Ok(None);
                }
                let _ = unsafe { GetExitCodeProcess(self.process, &mut code) };
            }
            let status = code as i32;
            self.exited = Some(status);
            Ok(Some(status))
        }

        pub fn kill(&mut self) -> io::Result<()> {
            if self.exited.is_some() {
                return Ok(());
            }
            unsafe { TerminateProcess(self.process, 1) };
            Ok(())
        }

        pub fn wait(&mut self) -> io::Result<i32> {
            if let Some(code) = self.exited {
                return Ok(code);
            }
            unsafe { WaitForSingleObject(self.process, INFINITE) };
            let mut code = 1u32;
            if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let status = code as i32;
            self.exited = Some(status);
            Ok(status)
        }

        pub fn is_isolated(&self) -> bool {
            !self.job.is_null()
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            if !self.process.is_null() {
                unsafe { CloseHandle(self.process) };
                self.process = std::ptr::null_mut();
            }
            if !self.job.is_null() {
                unsafe { CloseHandle(self.job) };
                self.job = std::ptr::null_mut();
            }
            if !self.desktop.is_null() {
                unsafe { CloseDesktop(self.desktop) };
                self.desktop = std::ptr::null_mut();
            }
        }
    }

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn quote_arg(arg: &str, out: &mut String) {
        if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '"')) {
            out.push_str(arg);
            return;
        }
        out.push('"');
        let mut backslashes = 0usize;
        for ch in arg.chars() {
            match ch {
                '\\' => backslashes += 1,
                '"' => {
                    for _ in 0..backslashes * 2 + 1 {
                        out.push('\\');
                    }
                    backslashes = 0;
                    out.push('"');
                }
                _ => {
                    for _ in 0..backslashes {
                        out.push('\\');
                    }
                    backslashes = 0;
                    out.push(ch);
                }
            }
        }
        for _ in 0..backslashes * 2 {
            out.push('\\');
        }
        out.push('"');
    }

    fn build_environment_block(overrides: &[(&str, &str)]) -> Vec<u16> {
        let mut map: BTreeMap<String, (String, String)> = BTreeMap::new();
        for (k, v) in std::env::vars_os() {
            let ks = k.to_string_lossy().into_owned();
            let vs = v.to_string_lossy().into_owned();
            map.insert(ks.to_ascii_uppercase(), (ks, vs));
        }
        for &(k, v) in overrides {
            map.insert(k.to_ascii_uppercase(), (k.to_string(), v.to_string()));
        }
        let mut block = Vec::new();
        for (_, (k, v)) in map {
            block.extend(k.encode_utf16());
            block.push(b'=' as u16);
            block.extend(v.encode_utf16());
            block.push(0);
        }
        block.push(0);
        block
    }

    fn current_user_sid_string(proc_token: Handle) -> io::Result<String> {
        unsafe {
            let mut needed = 0u32;
            let _ = GetTokenInformation(proc_token, TOKEN_USER, std::ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buf = vec![0u8; needed as usize];
            if GetTokenInformation(
                proc_token,
                TOKEN_USER,
                buf.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            // TOKEN_USER empieza con `SID_AND_ATTRIBUTES User`, cuyo primer campo es `PSID`.
            let sid = *(buf.as_ptr().cast::<*mut c_void>());
            let mut str_ptr: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid, &mut str_ptr) == 0 || str_ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0usize;
            while *str_ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(str_ptr, len));
            LocalFree(str_ptr.cast());
            Ok(s)
        }
    }

    fn create_sandbox_desktop(user_sid: &str) -> io::Result<(OwnedDesktop, Vec<u16>)> {
        static DESK_SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = DESK_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("NavegadorSandbox_{}_{}", std::process::id(), seq);
        let name_w = wide_z(&name);
        // Escritorio separado de `WinSta0\Default` con etiqueta obligatoria baja
        // para que `user32.dll` inicialice sin acceso al escritorio interactivo del usuario.
        let sddl = format!("D:(A;;GA;;;{user_sid})(A;;GA;;;SY)S:(ML;;NW;;;LW)");
        let sddl_w = wide_z(&sddl);
        unsafe {
            let mut sd: *mut c_void = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl_w.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            ) == 0
                || sd.is_null()
            {
                return Err(io::Error::last_os_error());
            }
            let sa = SECURITY_ATTRIBUTES {
                n_length: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lp_security_descriptor: sd,
                b_inherit_handle: 0,
            };
            let h_desk = CreateDesktopW(
                name_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                GENERIC_ALL,
                &sa,
            );
            let err = if h_desk.is_null() { Some(io::Error::last_os_error()) } else { None };
            LocalFree(sd);
            if let Some(e) = err {
                return Err(e);
            }
            Ok((OwnedDesktop(h_desk), wide_z(&format!("WinSta0\\{name}"))))
        }
    }

    fn create_restricted_token_and_user_sid() -> io::Result<(OwnedHandle, String)> {
        unsafe {
            let mut proc_token: Handle = std::ptr::null_mut();
            let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT;
            if OpenProcessToken(GetCurrentProcess(), access, &mut proc_token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let _proc_guard = OwnedHandle(proc_token);
            let user_sid = current_user_sid_string(proc_token)?;

            let mut restricted: Handle = std::ptr::null_mut();
            if CreateRestrictedToken(
                proc_token,
                DISABLE_MAX_PRIVILEGE,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut restricted,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let restricted_guard = OwnedHandle(restricted);

            let low_sid_w = wide_z("S-1-16-4096");
            let mut low_sid: *mut c_void = std::ptr::null_mut();
            if ConvertStringSidToSidW(low_sid_w.as_ptr(), &mut low_sid) == 0 || low_sid.is_null() {
                return Err(io::Error::last_os_error());
            }
            let tml = TOKEN_MANDATORY_LABEL {
                label: SID_AND_ATTRIBUTES {
                    sid: low_sid,
                    attributes: SE_GROUP_INTEGRITY,
                },
            };
            let set_ok = SetTokenInformation(
                restricted_guard.0,
                TOKEN_INTEGRITY_LEVEL,
                (&tml as *const TOKEN_MANDATORY_LABEL).cast(),
                std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32,
            );
            LocalFree(low_sid);
            if set_ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((restricted_guard, user_sid))
        }
    }

    fn create_renderer_job(use_ui_limits: bool) -> io::Result<OwnedHandle> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let guard = OwnedHandle(job);

            let mem_mb = std::env::var("NAVEGADOR_IA_RENDERER_MEM_MB")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|&mb| mb >= 128)
                .unwrap_or(DEFAULT_RENDERER_MEM_MB);
            let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION_DATA = std::mem::zeroed();
            ext.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            ext.basic_limit_information.active_process_limit = 1;
            ext.process_memory_limit = mem_mb * 1024 * 1024;
            if SetInformationJobObject(
                guard.0,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&ext as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION_DATA).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION_DATA>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }

            if use_ui_limits {
                let ui_flags = UI_RESTRICTIONS_MASK;
                if SetInformationJobObject(
                    guard.0,
                    JOB_OBJECT_BASIC_UI_RESTRICTIONS,
                    (&ui_flags as *const u32).cast(),
                    std::mem::size_of::<u32>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(guard)
        }
    }

    pub fn spawn_isolated_process(executable: &Path, args: &[&str], envs: &[(&str, &str)]) -> io::Result<Inner> {
        let (token, user_sid) = create_restricted_token_and_user_sid()?;
        let job = create_renderer_job(true)?;
        let (desktop_guard, mut desktop_w) = create_sandbox_desktop(&user_sid)?;

        let mut exe_w: Vec<u16> = executable.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let exe_str = executable.to_string_lossy();
        let mut cmd = String::new();
        quote_arg(&exe_str, &mut cmd);
        for arg in args {
            cmd.push(' ');
            quote_arg(arg, &mut cmd);
        }
        let mut cmd_w = wide_z(&cmd);
        let env_block = build_environment_block(envs);

        unsafe {
            let sa_inherit = SECURITY_ATTRIBUTES {
                n_length: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lp_security_descriptor: std::ptr::null_mut(),
                b_inherit_handle: 1,
            };
            let (mut in_r, mut in_w): (Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut());
            let (mut out_r, mut out_w): (Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut());
            if CreatePipe(&mut in_r, &mut in_w, &sa_inherit, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            let in_r_guard = OwnedHandle(in_r);
            let in_w_guard = OwnedHandle(in_w);
            if SetHandleInformation(in_w_guard.0, HANDLE_FLAG_INHERIT, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            if CreatePipe(&mut out_r, &mut out_w, &sa_inherit, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            let out_r_guard = OwnedHandle(out_r);
            let out_w_guard = OwnedHandle(out_w);
            if SetHandleInformation(out_r_guard.0, HANDLE_FLAG_INHERIT, 0) == 0 {
                return Err(io::Error::last_os_error());
            }

            let mut err_dup: Handle = std::ptr::null_mut();
            let raw_err = GetStdHandle(STD_ERROR_HANDLE);
            let err_guard = if !raw_err.is_null() && raw_err != (-1isize as Handle) {
                if DuplicateHandle(
                    GetCurrentProcess(),
                    raw_err,
                    GetCurrentProcess(),
                    &mut err_dup,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                ) != 0
                {
                    Some(OwnedHandle(err_dup))
                } else {
                    None
                }
            } else {
                None
            };

            let mut si: StartupInfoW = std::mem::zeroed();
            si.cb = std::mem::size_of::<StartupInfoW>() as u32;
            si.lp_desktop = desktop_w.as_mut_ptr();
            si.dw_flags = STARTF_USESTDHANDLES;
            si.h_std_input = in_r_guard.0;
            si.h_std_output = out_w_guard.0;
            si.h_std_error = err_guard.as_ref().map_or(std::ptr::null_mut(), |g| g.0);

            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessAsUserW(
                token.0,
                exe_w.as_mut_ptr(),
                cmd_w.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                env_block.as_ptr().cast(),
                std::ptr::null(),
                &si,
                &mut pi,
            );
            drop(in_r_guard);
            drop(out_w_guard);
            drop(err_guard);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }

            let proc_guard = OwnedHandle(pi.h_process);
            let thread_guard = OwnedHandle(pi.h_thread);
            if AssignProcessToJobObject(job.0, proc_guard.0) == 0 {
                let err = io::Error::last_os_error();
                TerminateProcess(proc_guard.0, 1);
                return Err(err);
            }
            if ResumeThread(thread_guard.0) == u32::MAX {
                let err = io::Error::last_os_error();
                TerminateProcess(proc_guard.0, 1);
                return Err(err);
            }
            drop(thread_guard);

            let stdin_file = File::from_raw_handle(in_w_guard.0);
            std::mem::forget(in_w_guard);
            let stdout_file = File::from_raw_handle(out_r_guard.0);
            std::mem::forget(out_r_guard);
            let process_handle = proc_guard.0;
            std::mem::forget(proc_guard);
            let job_handle = job.0;
            std::mem::forget(job);
            let desktop_handle = desktop_guard.0;
            std::mem::forget(desktop_guard);

            Ok(Inner {
                process: process_handle,
                job: job_handle,
                desktop: desktop_handle,
                stdin: Some(stdin_file),
                stdout: Some(stdout_file),
                exited: None,
            })
        }
    }

    pub fn harden_profile_dir(dir: &Path) -> io::Result<()> {
        let mut path_w: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let sddl_w = wide_z("S:(ML;OICI;NWNR;;;ME)");
        unsafe {
            let mut sd: *mut c_void = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl_w.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            ) == 0
                || sd.is_null()
            {
                return Err(io::Error::last_os_error());
            }
            let mut present = 0i32;
            let mut defaulted = 0i32;
            let mut sacl: *mut c_void = std::ptr::null_mut();
            if GetSecurityDescriptorSacl(sd, &mut present, &mut sacl, &mut defaulted) == 0 || present == 0 {
                let err = io::Error::last_os_error();
                LocalFree(sd);
                return Err(err);
            }
            let rc = SetNamedSecurityInfoW(
                path_w.as_mut_ptr(),
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                sacl,
            );
            LocalFree(sd);
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc as i32));
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod child_imp {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::{FromRawFd, IntoRawFd};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};

    pub struct Inner {
        child: Child,
        stdin: Option<File>,
        stdout: Option<File>,
    }

    impl Inner {
        pub fn take_stdin(&mut self) -> Option<File> {
            self.stdin.take()
        }

        pub fn take_stdout(&mut self) -> Option<File> {
            self.stdout.take()
        }

        pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
            Ok(self.child.try_wait()?.map(|s| s.code().unwrap_or(1)))
        }

        pub fn kill(&mut self) -> io::Result<()> {
            self.child.kill()
        }

        pub fn wait(&mut self) -> io::Result<i32> {
            Ok(self.child.wait()?.code().unwrap_or(1))
        }

        pub fn is_isolated(&self) -> bool {
            false
        }
    }

    pub fn spawn_isolated_process(executable: &Path, args: &[&str], envs: &[(&str, &str)]) -> io::Result<Inner> {
        let mut cmd = Command::new(executable);
        cmd.args(args)
            .envs(envs.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().map(|s| unsafe { File::from_raw_fd(s.into_raw_fd()) });
        let stdout = child.stdout.take().map(|s| unsafe { File::from_raw_fd(s.into_raw_fd()) });
        Ok(Inner { child, stdin, stdout })
    }

    pub fn harden_profile_dir(_dir: &Path) -> io::Result<()> {
        Ok(())
    }
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
            untrusted_fonts_disabled: true,
            system32_images_preferred: true,
            strict_handle_checks_enabled: true,
            unsupported_platform: false,
        };
        let s = todas.summary();
        assert!(s.contains("codigo-dinamico") && s.contains("procesos-hijo") && s.contains("puntos-de-extension") && s.contains("fuentes-no-confiables"));
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
