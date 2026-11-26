//! Instalador y desinstalador 100 % Rust de `navegador` (Fase 5.5).
//!
//! Instala el binario propio en el perfil del usuario (sin exigir privilegios
//! de administrador), crea accesos directos en el menú de aplicaciones y
//! Escritorio, y registra el desinstalador nativo (`desinstalar.exe` en Windows).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const NOMBRE_APP: &str = "Navegador";
const NOMBRE_PAQUETE_WIN: &str = "NavegadorIA";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const AYUDA: &str = "Instalador de Navegador (100 % Rust)

Uso:
  instalador [--origen <ruta>] [--ruta <directorio>] [--sin-escritorio] [--silencioso]
  instalador --desinstalar [--ruta <directorio>] [--silencioso]
  instalador --ayuda

Opciones:
  --origen <ruta>     Ejecutable navegador origen (si no se indica, se busca junto al instalador o en target/release)
  --ruta <dir>        Carpeta de instalación personalizada
  --escritorio        Crea acceso directo en el Escritorio (activo por defecto en Windows)
  --sin-escritorio    No crea acceso directo en el Escritorio
  --sin-sistema       Solo copia archivos locales sin tocar registro ni menús del sistema
  --silencioso        Modo consola sin ventanas ni confirmación";

#[derive(Debug)]
struct Opciones {
    desinstalar: bool,
    origen: Option<PathBuf>,
    destino: Option<PathBuf>,
    escritorio: bool,
    sin_sistema: bool,
    silencioso: bool,
    ayuda: bool,
}

impl Default for Opciones {
    fn default() -> Self {
        Self {
            desinstalar: false,
            origen: None,
            destino: None,
            escritorio: cfg!(windows),
            sin_sistema: false,
            silencioso: false,
            ayuda: false,
        }
    }
}

fn parsear_args<I, S>(args: I) -> Result<Opciones, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut opts = Opciones::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let s = arg.as_ref();
        match s {
            "--desinstalar" => opts.desinstalar = true,
            "--escritorio" => opts.escritorio = true,
            "--sin-escritorio" => opts.escritorio = false,
            "--sin-sistema" => opts.sin_sistema = true,
            "--silencioso" => opts.silencioso = true,
            "--ayuda" | "-h" | "--help" => opts.ayuda = true,
            "--origen" => {
                let valor = it.next().ok_or_else(|| "falta la ruta tras --origen".to_string())?;
                opts.origen = Some(PathBuf::from(valor.as_ref()));
            }
            "--ruta" => {
                let valor = it.next().ok_or_else(|| "falta el directorio tras --ruta".to_string())?;
                opts.destino = Some(PathBuf::from(valor.as_ref()));
            }
            otra => return Err(format!("opción desconocida: {otra}")),
        }
    }
    Ok(opts)
}

fn nombre_binario() -> &'static str {
    if cfg!(windows) { "navegador.exe" } else { "navegador" }
}

fn nombre_desinstalador() -> &'static str {
    if cfg!(windows) { "desinstalar.exe" } else { "desinstalar-navegador" }
}

fn es_nombre_desinstalador(exe: &Path) -> bool {
    exe.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().starts_with("desinstalar"))
}

fn resolver_destino(
    destino_explicito: Option<&Path>,
    desinstalar: bool,
    propio_exe: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(d) = destino_explicito {
        return Ok(d.to_path_buf());
    }
    if desinstalar {
        if let Some(dir) = propio_exe.and_then(|p| p.parent()) {
            if dir.join("navegador-instalacion.json").is_file() {
                return Ok(dir.to_path_buf());
            }
        }
    }
    directorio_instalacion_por_defecto()
}

fn directorio_instalacion_por_defecto() -> Result<PathBuf, String> {
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(local).join("Programs").join(NOMBRE_PAQUETE_WIN));
        }
        return Err("no se encontró LOCALAPPDATA en el entorno".to_string());
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "no se encontró HOME en el entorno".to_string())?;
    Ok(PathBuf::from(home).join(".local").join("bin"))
}

fn resolver_origen(explicito: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicito {
        return p
            .is_file()
            .then(|| p.to_path_buf())
            .ok_or_else(|| format!("no existe el ejecutable en {}", p.display()));
    }
    let bin = nombre_binario();
    if let Ok(actual) = std::env::current_exe() {
        let mut cursor = actual.parent();
        for _ in 0..6 {
            let Some(dir) = cursor else { break };
            for candidato in [
                dir.join(bin),
                dir.join("engine").join("target").join("release").join(bin),
                dir.join("target").join("release").join(bin),
            ] {
                if candidato.is_file() {
                    return Ok(candidato);
                }
            }
            cursor = dir.parent();
        }
    }
    let en_repo = Path::new("engine").join("target").join("release").join(bin);
    if en_repo.is_file() {
        return Ok(en_repo);
    }
    let en_target = Path::new("target").join("release").join(bin);
    if en_target.is_file() {
        return Ok(en_target);
    }
    Err(format!(
        "no se encontró {bin} junto al instalador ni en engine/target/release. Usa el paquete completo o pásalo con --origen <ruta>."
    ))
}

#[derive(Debug)]
struct ResumenInstalacion {
    binario_instalado: PathBuf,
    accesos: Vec<PathBuf>,
}

fn instalar_en(
    origen: &Path,
    propio_instalador: Option<&Path>,
    destino_dir: &Path,
    escritorio: bool,
    registrar_sistema: bool,
) -> Result<ResumenInstalacion, String> {
    if !origen.is_file() {
        return Err(format!("el ejecutable origen no existe: {}", origen.display()));
    }
    std::fs::create_dir_all(destino_dir)
        .map_err(|e| format!("no se pudo crear {}: {e}", destino_dir.display()))?;

    let bin_dest = destino_dir.join(nombre_binario());
    std::fs::copy(origen, &bin_dest)
        .map_err(|e| format!("no se pudo copiar a {}: {e}", bin_dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&bin_dest, std::fs::Permissions::from_mode(0o755));
    }

    let mut desinst_path: Option<PathBuf> = None;
    if let Some(inst) = propio_instalador.filter(|p| p.is_file()) {
        let dest_uninst = destino_dir.join(nombre_desinstalador());
        if inst != dest_uninst {
            std::fs::copy(inst, &dest_uninst)
                .map_err(|e| format!("no se pudo copiar desinstalador: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest_uninst, std::fs::Permissions::from_mode(0o755));
            }
        }
        desinst_path = Some(dest_uninst);
    }

    let mut accesos = Vec::new();
    if registrar_sistema {
        accesos = plataforma::registrar_sistema(&bin_dest, desinst_path.as_deref(), destino_dir, escritorio)?;
    }

    let manifiesto = destino_dir.join("navegador-instalacion.json");
    let info = json!({
        "version": VERSION,
        "binario": bin_dest.to_string_lossy(),
        "desinstalador": desinst_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "accesos": accesos.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
    });
    std::fs::write(&manifiesto, serde_json::to_string_pretty(&info).unwrap_or_default())
        .map_err(|e| format!("no se pudo guardar manifiesto {}: {e}", manifiesto.display()))?;

    Ok(ResumenInstalacion {
        binario_instalado: bin_dest,
        accesos,
    })
}

fn desinstalar_en(destino_dir: &Path, limpiar_sistema: bool) -> Result<(), String> {
    let manifiesto = destino_dir.join("navegador-instalacion.json");
    if let Ok(texto) = std::fs::read_to_string(&manifiesto) {
        if let Ok(val) = serde_json::from_str::<Value>(&texto) {
            if let Some(lista) = val.get("accesos").and_then(Value::as_array) {
                for item in lista {
                    if let Some(ruta) = item.as_str() {
                        let _ = std::fs::remove_file(ruta);
                    }
                }
            }
        }
    }
    if limpiar_sistema {
        plataforma::limpiar_sistema()?;
    }
    let bin = destino_dir.join(nombre_binario());
    if bin.exists() {
        std::fs::remove_file(&bin).map_err(|e| format!("no se pudo borrar {}: {e}", bin.display()))?;
    }
    if manifiesto.exists() {
        let _ = std::fs::remove_file(&manifiesto);
    }
    let actual = std::env::current_exe().ok();
    let desinst = destino_dir.join(nombre_desinstalador());
    if desinst.exists() {
        if actual.as_deref() == Some(desinst.as_path()) {
            plataforma::programar_borrado_al_reiniciar(&desinst);
            plataforma::programar_borrado_al_reiniciar(destino_dir);
        } else {
            let _ = std::fs::remove_file(&desinst);
        }
    }
    let _ = std::fs::remove_dir(destino_dir);
    Ok(())
}

#[cfg(windows)]
mod plataforma {
    use super::*;
    use core::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const HKEY_CURRENT_USER: isize = -0x7FFF_FFFF;
    const KEY_WRITE: u32 = 0x20006;
    const REG_SZ: u32 = 1;
    const REG_DWORD: u32 = 4;
    const CLSCTX_INPROC_SERVER: u32 = 1;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const CLSID_SHELL_LINK: Guid = Guid { data1: 0x0002_1401, data2: 0x0000, data3: 0x0000, data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46] };
    const IID_ISHELL_LINK_W: Guid = Guid { data1: 0x0002_14F9, data2: 0x0000, data3: 0x0000, data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46] };
    const IID_IPERSIST_FILE: Guid = Guid { data1: 0x0000_010B, data2: 0x0000, data3: 0x0000, data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46] };

    #[repr(C)]
    struct IShellLinkW {
        vtbl: *const IShellLinkWVtbl,
    }

    #[repr(C)]
    struct IShellLinkWVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_path: *const c_void,
        get_id_list: *const c_void,
        set_id_list: *const c_void,
        get_description: *const c_void,
        set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
        get_working_directory: *const c_void,
        set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
        get_arguments: *const c_void,
        set_arguments: *const c_void,
        get_hotkey: *const c_void,
        set_hotkey: *const c_void,
        get_show_cmd: *const c_void,
        set_show_cmd: *const c_void,
        get_icon_location: *const c_void,
        set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32,
        set_relative_path: *const c_void,
        resolve: *const c_void,
        set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    }

    #[repr(C)]
    struct IPersistFile {
        vtbl: *const IPersistFileVtbl,
    }

    #[repr(C)]
    struct IPersistFileVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_class_id: *const c_void,
        is_dirty: *const c_void,
        load: *const c_void,
        save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32,
        save_completed: *const c_void,
        get_cur_file: *const c_void,
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoUninitialize();
        fn CoCreateInstance(
            rclsid: *const Guid,
            outer: *mut c_void,
            cls_context: u32,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            hkey: isize,
            subkey: *const u16,
            reserved: u32,
            class: *mut u16,
            options: u32,
            sam: u32,
            sec: *const c_void,
            result: *mut isize,
            disp: *mut u32,
        ) -> i32;
        fn RegSetValueExW(hkey: isize, name: *const u16, reserved: u32, kind: u32, data: *const u8, cb: u32) -> i32;
        fn RegDeleteTreeW(hkey: isize, subkey: *const u16) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn set_reg_sz(key: isize, name: Option<&str>, value: &str) -> Result<(), String> {
        let name_w = name.map(wide_z);
        let val_w = wide_z(value);
        let rc = unsafe {
            RegSetValueExW(
                key,
                name_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                0,
                REG_SZ,
                val_w.as_ptr().cast(),
                (val_w.len() * 2) as u32,
            )
        };
        if rc != 0 {
            return Err(format!("error de registro ({rc})"));
        }
        Ok(())
    }

    fn set_reg_dword(key: isize, name: &str, value: u32) -> Result<(), String> {
        let name_w = wide_z(name);
        let rc = unsafe {
            RegSetValueExW(
                key,
                name_w.as_ptr(),
                0,
                REG_DWORD,
                (&value as *const u32).cast(),
                4,
            )
        };
        if rc != 0 {
            return Err(format!("error de registro ({rc})"));
        }
        Ok(())
    }

    pub fn crear_acceso_directo(enlace: &Path, destino: &Path, trabajo: &Path, descripcion: &str) -> Result<(), String> {
        if let Some(padre) = enlace.parent() {
            std::fs::create_dir_all(padre).map_err(|e| format!("no se pudo crear {}: {e}", padre.display()))?;
        }
        unsafe {
            let hr = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
            let must_uninit = hr >= 0;
            let res = (|| {
                let mut sl_ptr: *mut c_void = std::ptr::null_mut();
                let hr = CoCreateInstance(&CLSID_SHELL_LINK, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELL_LINK_W, &mut sl_ptr);
                if hr < 0 || sl_ptr.is_null() {
                    return Err(format!("CoCreateInstance IShellLinkW falló: {hr:#x}"));
                }
                let sl = &*(sl_ptr.cast::<IShellLinkW>());
                let vtbl = &*sl.vtbl;
                let dest_w = wide_path(destino);
                let dir_w = wide_path(trabajo);
                let desc_w = wide_z(descripcion);

                let check = |code: i32, paso: &str| -> Result<(), String> {
                    if code < 0 { Err(format!("{paso} falló: {code:#x}")) } else { Ok(()) }
                };
                let op = (|| {
                    check((vtbl.set_path)(sl_ptr, dest_w.as_ptr()), "SetPath")?;
                    check((vtbl.set_working_directory)(sl_ptr, dir_w.as_ptr()), "SetWorkingDirectory")?;
                    check((vtbl.set_description)(sl_ptr, desc_w.as_ptr()), "SetDescription")?;
                    check((vtbl.set_icon_location)(sl_ptr, dest_w.as_ptr(), 0), "SetIconLocation")?;

                    let mut pf_ptr: *mut c_void = std::ptr::null_mut();
                    check((vtbl.query_interface)(sl_ptr, &IID_IPERSIST_FILE, &mut pf_ptr), "QueryInterface IPersistFile")?;
                    let pf = &*(pf_ptr.cast::<IPersistFile>());
                    let pf_vtbl = &*pf.vtbl;
                    let lnk_w = wide_path(enlace);
                    let save_res = check((pf_vtbl.save)(pf_ptr, lnk_w.as_ptr(), 1), "IPersistFile::Save");
                    (pf_vtbl.release)(pf_ptr);
                    save_res
                })();
                (vtbl.release)(sl_ptr);
                op
            })();
            if must_uninit {
                CoUninitialize();
            }
            res
        }
    }

    pub fn registrar_sistema(
        bin_dest: &Path,
        desinstalador: Option<&Path>,
        destino_dir: &Path,
        escritorio: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let mut accesos = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let menu_lnk = PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join(format!("{NOMBRE_APP}.lnk"));
            crear_acceso_directo(&menu_lnk, bin_dest, destino_dir, "Navegador web 100 % Rust")?;
            accesos.push(menu_lnk);
        }
        if escritorio {
            if let Some(profile) = std::env::var_os("USERPROFILE") {
                let desk_lnk = PathBuf::from(profile).join("Desktop").join(format!("{NOMBRE_APP}.lnk"));
                crear_acceso_directo(&desk_lnk, bin_dest, destino_dir, "Navegador web 100 % Rust")?;
                accesos.push(desk_lnk);
            }
        }

        let uninst_key = wide_z(&format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{NOMBRE_PAQUETE_WIN}"));
        let mut hk: isize = 0;
        let rc = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, uninst_key.as_ptr(), 0, std::ptr::null_mut(), 0, KEY_WRITE, std::ptr::null(), &mut hk, std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(format!("no se pudo registrar la desinstalación ({rc})"));
        }
        let uninst_cmd = match desinstalador {
            Some(d) => format!("\"{}\" --desinstalar --ruta \"{}\"", d.display(), destino_dir.display()),
            None => format!("\"{}\" --desinstalar --ruta \"{}\"", bin_dest.display(), destino_dir.display()),
        };
        let res: Result<(), String> = (|| {
            set_reg_sz(hk, Some("DisplayName"), "Navegador (100 % Rust)")?;
            set_reg_sz(hk, Some("DisplayVersion"), VERSION)?;
            set_reg_sz(hk, Some("Publisher"), "Navegador IA")?;
            set_reg_sz(hk, Some("InstallLocation"), &destino_dir.to_string_lossy())?;
            set_reg_sz(hk, Some("DisplayIcon"), &bin_dest.to_string_lossy())?;
            set_reg_sz(hk, Some("UninstallString"), &uninst_cmd)?;
            set_reg_dword(hk, "NoModify", 1)?;
            set_reg_dword(hk, "NoRepair", 1)?;
            Ok(())
        })();
        unsafe { RegCloseKey(hk) };
        res?;

        let app_paths = wide_z(r"Software\Microsoft\Windows\CurrentVersion\App Paths\navegador.exe");
        let mut apk: isize = 0;
        if unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, app_paths.as_ptr(), 0, std::ptr::null_mut(), 0, KEY_WRITE, std::ptr::null(), &mut apk, std::ptr::null_mut()) } == 0 {
            let _ = set_reg_sz(apk, None, &bin_dest.to_string_lossy());
            let _ = set_reg_sz(apk, Some("Path"), &destino_dir.to_string_lossy());
            unsafe { RegCloseKey(apk) };
        }

        Ok(accesos)
    }

    pub fn limpiar_sistema() -> Result<(), String> {
        let uninst_key = wide_z(&format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{NOMBRE_PAQUETE_WIN}"));
        let app_paths = wide_z(r"Software\Microsoft\Windows\CurrentVersion\App Paths\navegador.exe");
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, uninst_key.as_ptr());
            RegDeleteTreeW(HKEY_CURRENT_USER, app_paths.as_ptr());
        }
        Ok(())
    }

    pub fn adjuntar_consola_padre() {
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    pub fn programar_borrado_al_reiniciar(ruta: &Path) {
        const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x4;
        let w = wide_path(ruta);
        unsafe {
            let _ = MoveFileExW(w.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
        }
    }

    pub fn dialogo_si_no(titulo: &str, mensaje: &str, pregunta: bool) -> bool {
        const MB_YESNO: u32 = 0x0000_0004;
        const MB_ICONQUESTION: u32 = 0x0000_0020;
        const MB_ICONINFORMATION: u32 = 0x0000_0040;
        const MB_SETFOREGROUND: u32 = 0x0001_0000;
        const IDYES: i32 = 6;
        let t = wide_z(titulo);
        let m = wide_z(mensaje);
        let icono = if pregunta { MB_ICONQUESTION } else { MB_ICONINFORMATION };
        unsafe { MessageBoxW(0, m.as_ptr(), t.as_ptr(), MB_YESNO | icono | MB_SETFOREGROUND) == IDYES }
    }

    pub fn dialogo_aviso(titulo: &str, mensaje: &str, error: bool) {
        const MB_OK: u32 = 0x0000_0000;
        const MB_ICONERROR: u32 = 0x0000_0010;
        const MB_ICONINFORMATION: u32 = 0x0000_0040;
        const MB_SETFOREGROUND: u32 = 0x0001_0000;
        let t = wide_z(titulo);
        let m = wide_z(mensaje);
        let icono = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
        unsafe {
            let _ = MessageBoxW(0, m.as_ptr(), t.as_ptr(), MB_OK | icono | MB_SETFOREGROUND);
        }
    }
}

#[cfg(not(windows))]
mod plataforma {
    use super::*;

    pub fn registrar_sistema(
        bin_dest: &Path,
        _desinstalador: Option<&Path>,
        _destino_dir: &Path,
        _escritorio: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let mut accesos = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            let desktop = PathBuf::from(home).join(".local").join("share").join("applications").join("navegador-ia.desktop");
            if let Some(padre) = desktop.parent() {
                std::fs::create_dir_all(padre).map_err(|e| e.to_string())?;
            }
            let contenido = format!(
                "[Desktop Entry]\nType=Application\nName=Navegador\nExec=\"{}\" %U\nTerminal=false\nCategories=Network;WebBrowser;\n",
                bin_dest.display()
            );
            std::fs::write(&desktop, contenido).map_err(|e| e.to_string())?;
            accesos.push(desktop);
        }
        Ok(accesos)
    }

    pub fn limpiar_sistema() -> Result<(), String> {
        Ok(())
    }

    pub fn adjuntar_consola_padre() {}

    pub fn programar_borrado_al_reiniciar(_ruta: &Path) {}

    pub fn dialogo_si_no(_titulo: &str, _mensaje: &str, _pregunta: bool) -> bool {
        true
    }

    pub fn dialogo_aviso(_titulo: &str, _mensaje: &str, _error: bool) {}
}

fn main() -> ExitCode {
    plataforma::adjuntar_consola_padre();
    let mut opts = match parsear_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[instalador] {e}\n{AYUDA}");
            if cfg!(windows) {
                plataforma::dialogo_aviso("Instalador de Navegador", &format!("{e}\n\n{AYUDA}"), true);
            }
            return ExitCode::FAILURE;
        }
    };
    let propio_exe = std::env::current_exe().ok();
    if !opts.desinstalar && propio_exe.as_deref().is_some_and(es_nombre_desinstalador) {
        opts.desinstalar = true;
    }
    if opts.ayuda {
        println!("{AYUDA}");
        if cfg!(windows) && !opts.silencioso {
            plataforma::dialogo_aviso("Ayuda — Instalador de Navegador", AYUDA, false);
        }
        return ExitCode::SUCCESS;
    }

    let interactivo = cfg!(windows) && !opts.silencioso;
    let destino = match resolver_destino(opts.destino.as_deref(), opts.desinstalar, propio_exe.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[instalador] {e}");
            if interactivo {
                plataforma::dialogo_aviso("Instalador de Navegador", &e, true);
            }
            return ExitCode::FAILURE;
        }
    };

    if opts.desinstalar {
        if interactivo {
            let confirmado = plataforma::dialogo_si_no(
                "Desinstalar Navegador",
                &format!("¿Deseas desinstalar Navegador de:\n\n{}?", destino.display()),
                true,
            );
            if !confirmado {
                return ExitCode::SUCCESS;
            }
        }
        return match desinstalar_en(&destino, !opts.sin_sistema) {
            Ok(()) => {
                let msg = format!("Navegador desinstalado de {}", destino.display());
                println!("{msg}");
                if interactivo {
                    plataforma::dialogo_aviso("Desinstalador de Navegador", &msg, false);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("[instalador] {e}");
                if interactivo {
                    plataforma::dialogo_aviso("Error al desinstalar Navegador", &e, true);
                }
                ExitCode::FAILURE
            }
        };
    }

    let origen = match resolver_origen(opts.origen.as_deref()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[instalador] {e}");
            if interactivo {
                plataforma::dialogo_aviso("Error — Instalador de Navegador", &e, true);
            }
            return ExitCode::FAILURE;
        }
    };

    if interactivo {
        let extra = if opts.sin_sistema {
            "Sin accesos directos ni registro (--sin-sistema)."
        } else if opts.escritorio {
            "Se crearán accesos directos en el Menú Inicio y en el Escritorio."
        } else {
            "Se creará acceso directo en el Menú Inicio."
        };
        let confirmado = plataforma::dialogo_si_no(
            "Instalar Navegador (100 % Rust)",
            &format!(
                "Se instalará Navegador v{VERSION} en:\n\n{}\n\n{extra}\n\n¿Continuar con la instalación?",
                destino.display()
            ),
            true,
        );
        if !confirmado {
            return ExitCode::SUCCESS;
        }
    }

    match instalar_en(&origen, propio_exe.as_deref(), &destino, opts.escritorio, !opts.sin_sistema) {
        Ok(resumen) => {
            let msg = format!(
                "Navegador instalado en {} ({} accesos creados)",
                resumen.binario_instalado.display(),
                resumen.accesos.len()
            );
            println!("{msg}");
            if interactivo {
                let abrir = plataforma::dialogo_si_no(
                    "Instalación completada",
                    &format!(
                        "Navegador se instaló correctamente en:\n{}\n\nAccesos creados: {}\n\n¿Quieres abrir Navegador ahora?",
                        destino.display(),
                        resumen.accesos.len()
                    ),
                    false,
                );
                if abrir {
                    let _ = std::process::Command::new(&resumen.binario_instalado).spawn();
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[instalador] {e}");
            if interactivo {
                plataforma::dialogo_aviso("Error al instalar Navegador", &e, true);
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instala_y_desinstala_en_carpeta_sin_tocar_el_sistema() {
        let base = std::env::temp_dir().join(format!("navegador-inst-test-{}", std::process::id()));
        let origen = base.join("origen").join(nombre_binario());
        let fake_inst = base.join("origen").join("inst.exe");
        let destino = base.join("destino");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(origen.parent().unwrap()).unwrap();
        std::fs::write(&origen, b"binario-prueba").unwrap();
        std::fs::write(&fake_inst, b"inst-prueba").unwrap();

        let res = instalar_en(&origen, Some(&fake_inst), &destino, false, false).unwrap();
        assert!(res.binario_instalado.is_file());
        assert!(destino.join("navegador-instalacion.json").is_file());
        assert!(destino.join(nombre_desinstalador()).is_file());

        desinstalar_en(&destino, false).unwrap();
        assert!(!res.binario_instalado.exists());
        assert!(!destino.join(nombre_desinstalador()).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parsea_banderas_del_instalador() {
        let o = parsear_args(["--escritorio", "--ruta", "C:\\temp\\nav", "--silencioso"]).unwrap();
        assert!(o.escritorio && o.silencioso && !o.desinstalar);
        assert_eq!(o.destino, Some(PathBuf::from("C:\\temp\\nav")));
        let sin = parsear_args(["--sin-escritorio"]).unwrap();
        assert!(!sin.escritorio);
        assert!(parsear_args(["--desconocida"]).is_err());
        assert!(es_nombre_desinstalador(Path::new("C:\\App\\desinstalar.exe")));
        assert!(!es_nombre_desinstalador(Path::new("C:\\App\\instalador.exe")));
    }

    #[test]
    fn desinstalar_usa_su_propia_carpeta_si_tiene_manifiesto() {
        let base = std::env::temp_dir().join(format!("navegador-uninst-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("navegador-instalacion.json"), b"{}").unwrap();
        let fake_exe = base.join("desinstalar.exe");
        let resolved = resolver_destino(None, true, Some(&fake_exe)).unwrap();
        assert_eq!(resolved, base);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn crea_un_acceso_directo_lnk_real_en_disco() {
        let base = std::env::temp_dir().join(format!("navegador-lnk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let target = std::env::current_exe().unwrap();
        let lnk = base.join("Prueba.lnk");
        plataforma::crear_acceso_directo(&lnk, &target, &base, "Acceso directo de prueba").unwrap();
        let bytes = std::fs::read(&lnk).unwrap();
        assert!(bytes.len() > 76 && &bytes[..4] == b"L\0\0\0", "cabecera ShellLink esperada");
        let _ = std::fs::remove_dir_all(&base);
    }
}
