//! Prueba de integración real del aislamiento del subproceso del motor en Windows
//! (vive en un binario de test aparte porque `sandbox::tests::on_windows_the_mitigations_actually_take_effect`
//! activa `ProcessChildProcessPolicy` sobre el proceso unitario de `engine_core`).

#[cfg(windows)]
#[test]
fn child_sandbox_probe_helper() {
    let Ok(profile_dir) = std::env::var("NAVEGADOR_SANDBOX_PROBE_PROFILE") else {
        return;
    };
    let Ok(temp_target) = std::env::var("NAVEGADOR_SANDBOX_PROBE_WRITE") else {
        panic!("falta NAVEGADOR_SANDBOX_PROBE_WRITE");
    };
    // 1. No puede escribir en archivos de integridad media.
    let write_attempt = std::fs::write(&temp_target, b"pwned");
    assert!(write_attempt.is_err(), "integridad baja no debe escribir en {temp_target}");

    // 2. No puede leer el perfil endurecido (`S:(ML;OICI;NWNR;;;ME)`).
    let secret = std::path::Path::new(&profile_dir).join("cookies.json");
    let read_attempt = std::fs::read_to_string(&secret);
    assert!(read_attempt.is_err(), "integridad baja no debe leer {}", secret.display());

    // 3. El Job Object (ActiveProcessLimit = 1) impide crear subprocesos.
    let child_attempt = std::process::Command::new("cmd.exe").args(["/C", "echo", "hola"]).output();
    assert!(child_attempt.is_err(), "el Job Object debe impedir lanzar procesos hijo");

    println!("SANDBOX_PROBE_OK");
}

#[cfg(windows)]
#[test]
fn isolated_child_cannot_read_profile_write_user_files_or_spawn() {
    use engine_core::sandbox::{harden_profile_dir, spawn_isolated_process};
    use std::io::Read;

    let base = std::env::temp_dir().join(format!("navegador-sbx-{}", std::process::id()));
    let profile = base.join("perfil");
    let medium_file = base.join("medio.txt");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    harden_profile_dir(&profile).unwrap();
    std::fs::write(profile.join("cookies.json"), b"{\"secret\":true}").unwrap();

    let exe = std::env::current_exe().unwrap();
    let profile_s = profile.to_string_lossy().into_owned();
    let medium_s = medium_file.to_string_lossy().into_owned();
    let mut child = spawn_isolated_process(
        &exe,
        &["--exact", "child_sandbox_probe_helper", "--nocapture"],
        &[
            ("NAVEGADOR_SANDBOX_PROBE_PROFILE", &profile_s),
            ("NAVEGADOR_SANDBOX_PROBE_WRITE", &medium_s),
        ],
    )
    .unwrap();
    assert!(child.is_isolated());
    let mut stdout = child.take_stdout().unwrap();
    let code = child.wait().unwrap();
    let mut salida = String::new();
    let _ = stdout.read_to_string(&mut salida);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(code, 0, "la sonda aislada falló ({code}): {salida}");
    assert!(salida.contains("SANDBOX_PROBE_OK"), "salida inesperada: {salida}");
    assert!(!medium_file.exists(), "el archivo de integridad media no debió crearse");
}
