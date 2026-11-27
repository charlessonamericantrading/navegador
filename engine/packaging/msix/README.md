# Glowapp Browser en Microsoft Store (MSIX)

Empaquetado MSIX de **Glowapp Browser** (`navegador.exe`, aplicación 100 % Rust de `engine/crates/app`, sin WebView ni motores externos) para **Microsoft Store (Store ID `9P251CZGDJ92`, borrador)**.

**Sin firmar:** Microsoft Store firma el paquete automáticamente al publicarlo. En este repositorio no hay claves ni certificados privados.

| Fichero | Función |
|---|---|
| `AppxManifest.xml` | Manifiesto MSIX (`Windows.Desktop` >= 10.0.17763.0, x64, `runFullTrust`, `es-ES` y `en-US`). **Único sitio donde cambiar `Identity Name`**. |
| `Assets/` | Los 4 logos requeridos (`StoreLogo.png` 50×50, `Square44x44Logo.png` 44×44, `Square150x150Logo.png` 150×150, `Wide310x150Logo.png` 310×150). |
| `generar-logos.ps1` | Regenera `Assets\*.png` usando solo PowerShell (`System.Drawing`). |
| `empaquetar.ps1` | Valida manifiesto, versión (`engine/Cargo.toml` + `.0`), cabecera PE/DLLs de `navegador.exe` y genera el `.msix` sin firmar + `SHA256SUMS.txt`. |
| `capturas/` | Capturas reales de ventana limpia sin datos personales (`--captura`) listas para la ficha. |

---

## Identidad configurada (Partner Center)

| Campo | Valor |
|---|---|
| **Identity Name** | `PENDIENTE.GlowappBrowser` *(único campo pendiente: sustituir en `AppxManifest.xml` cuando aparezca en Partner Center > Product identity)* |
| **Publisher** | `CN=92865E93-3F3F-4EEF-9C20-8F49C41D64EC` |
| **PublisherDisplayName** | `Charlesson` |
| **DisplayName** | `Glowapp Browser` |
| **Store ID** | `9P251CZGDJ92` |
| **Versión** | `engine/Cargo.toml` `[workspace.package].version` + `.0` (hoy `0.1.0.0`; el 4.º número siempre `0`) |

---

## Cómo generar el `.msix`

### En GitHub Actions (job `msix`)

El flujo `.github/workflows/engine.yml` incluye el job **`msix`** (`windows-latest`):
1. Compila `navegador.exe` en release con runtime CRT estático (`+crt-static`) y verifica `navegador.exe --version`.
2. Ejecuta `engine/packaging/msix/empaquetar.ps1` para generar `GlowappBrowser_<versión>.0_x64.msix` sin firmar y `SHA256SUMS.txt`.
3. Sube el artefacto `glowapp-browser-msix-<run_number>` (retención 30 días).

### En local (Windows, con o sin Windows SDK instalado)

```powershell
# 1. Compilar en release con CRT estático (evita depender de VCRUNTIME140.dll)
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "-C target-feature=+crt-static"
cd engine
cargo build --release --locked -p navegador --bin navegador
cd ..

# 2. Empaquetar MSIX sin firmar
powershell -NoProfile -ExecutionPolicy Bypass -File engine\packaging\msix\empaquetar.ps1
```

Salida en `engine\target\msix\GlowappBrowser_0.1.0.0_x64.msix` y `SHA256SUMS.txt`. Si el PC no tiene instalado `Windows Kits\10`, `empaquetar.ps1` descarga y verifica (SHA-512 + firma `.signature.p7s` + Authenticode de Microsoft Corporation) `Microsoft.Windows.SDK.BuildTools` en `%LOCALAPPDATA%\NavegadorBuild\sdk-buildtools`.

*(Opcional para sustituir el Identity Name al vuelo sin editar archivos: añade `-IdentityName "TuNombreDePartnerCenter"`).*

---

## Cómo probar el paquete dentro del contenedor MSIX en Windows 11

Requiere activar **Configuración › Sistema › Para programadores › Modo de desarrollador** y ejecutar en PowerShell:

```powershell
# Registrar y abrir desde la identidad del contenedor MSIX
Add-AppxPackage -Register engine\target\msix\paquete\AppxManifest.xml
$pkg = Get-AppxPackage PENDIENTE.GlowappBrowser
start "shell:AppsFolder\$($pkg.PackageFamilyName)!GlowappBrowser"

# Desinstalar al terminar la prueba
Get-AppxPackage PENDIENTE.GlowappBrowser | Remove-AppxPackage
```

---

## Datos preparados para rellenar la ficha en Partner Center

- **Política de privacidad pública (obligatoria):** *pendiente de publicar.* El texto bilingüe está preparado fuera del repositorio (lleva un correo de contacto y aún no se ha decidido dónde publicarlo). Cuando se publique, poner su URL pública en la ficha (por ejemplo en tu web, o en este repositorio si se decide subirlo).
- **Justificación de la capacidad restringida `runFullTrust`:**
  > *Glowapp Browser es un navegador web nativo escrito 100 % en Rust (Win32) que necesita `runFullTrust` para abrir su ventana nativa y aislar sus subprocesos locales de motor de renderizado (`--motor`) y gestor de red (`--broker`) en el sistema.*
- **Idiomas de la ficha:** rellenar **Español (es-ES)** e **Inglés (en-US)** (los dos únicos declarados en `<Resources>`).
- **Capturas de pantalla:** ver `engine/packaging/msix/capturas/` (1366 × 768, ventana real sin datos personales).
