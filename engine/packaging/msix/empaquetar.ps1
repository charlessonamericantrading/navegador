# Empaqueta Glowapp Browser (`navegador.exe`, 100 % Rust) como paquete MSIX sin firmar para Microsoft Store.
# La Store firma el paquete al publicarlo; aquí no se usa ningún certificado privado.
#
# Uso:
#   powershell -NoProfile -ExecutionPolicy Bypass -File engine\packaging\msix\empaquetar.ps1 `
#       [-Exe engine\target\release\navegador.exe] [-Salida engine\target\msix]
[CmdletBinding()]
param(
    [string]$Exe = '',
    [string]$Salida = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

# Identidad de Partner Center > Product identity (Store ID 9P251CZGDJ92).
$EsperadoIdentityName = 'Charlesson.GlowappBrowser'
$EsperadoFamilia = 'Charlesson.GlowappBrowser_t5wqe1epm5p6c'
$EsperadoPublisher = 'CN=92865E93-3F3F-4EEF-9C20-8F49C41D64EC'
$EsperadoPublisherDisplay = 'Charlesson'
$EsperadoDisplayName = 'Glowapp Browser'
$EsperadoArquitectura = 'x64'
$IdiomasEsperados = @('es-ES', 'en-US')

# SDK BuildTools oficial de Microsoft en nuget.org (fallback si no hay Windows SDK en la máquina).
$BuildToolsVersion = '10.0.28000.2957'
$BuildToolsSha512 = 'PhfL6f1d9c2CXCD2fyQkwBBFR6/csgVxWDWDj/9uIY3PU5aUR3vtoAJn7HP1d/cMrHGQhuSH1FXHvaZIjOIajw=='
$BuildToolsUrl = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.buildtools/$BuildToolsVersion/microsoft.windows.sdk.buildtools.$BuildToolsVersion.nupkg"

$RuntimesNoDeWindows = @(
    '^vcruntime\d+(_\d+)?d?\.dll$',
    '^msvcp\d+(_\w+)?d?\.dll$',
    '^concrt\d+d?\.dll$',
    '^vccorlib\d+d?\.dll$',
    '^ucrtbased\.dll$',
    '^libgcc_s_[\w-]+\.dll$',
    '^libstdc\+\+-\d+\.dll$',
    '^libwinpthread-\d+\.dll$'
)

$aqui = $PSScriptRoot
$engineDir = (Resolve-Path (Join-Path $aqui '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $engineDir 'target\release\navegador.exe' }
if (-not $Salida) { $Salida = Join-Path $engineDir 'target\msix' }

function Paso([string]$texto) { Write-Host "`n== $texto ==" }
function Fallo([string]$texto) { throw "empaquetar.ps1 (MSIX): $texto" }

# ── 1. Leer Cargo.toml y AppxManifest.xml ─────────────────────────────────────────────────────────────
Paso 'Manifiesto y versión'
$cargoVersion = $null
$enBloque = $false
foreach ($linea in Get-Content (Join-Path $engineDir 'Cargo.toml')) {
    if ($linea -match '^\s*\[') { $enBloque = ($linea.Trim() -eq '[workspace.package]'); continue }
    if ($enBloque -and $linea -match '^\s*version\s*=\s*"([^"]+)"') { $cargoVersion = $Matches[1]; break }
}
if (-not $cargoVersion) { Fallo 'No se encontró version en [workspace.package] de engine/Cargo.toml.' }
$versionEsperada = "$cargoVersion.0"

$manifiestoRuta = Join-Path $aqui 'AppxManifest.xml'
[xml]$manifiesto = Get-Content -Raw -Encoding UTF8 $manifiestoRuta
$id = $manifiesto.Package.Identity

$idName = $id.GetAttribute('Name')
$idPublisher = $id.GetAttribute('Publisher')
$idVersion = $id.GetAttribute('Version')
$idArq = $id.GetAttribute('ProcessorArchitecture')
$display = [string]$manifiesto.Package.Properties.DisplayName
$pubDisplay = [string]$manifiesto.Package.Properties.PublisherDisplayName

if ($idName -ne $EsperadoIdentityName) { Fallo "Identity Name es «$idName»; debe ser «$EsperadoIdentityName» (Partner Center)." }
if ($idPublisher -ne $EsperadoPublisher) { Fallo "Identity Publisher es «$idPublisher»; debe ser «$EsperadoPublisher»." }

# Package Family Name = Name + "_" + id del Publisher: los 8 primeros bytes del SHA-256 del Publisher
# en UTF-16LE, en base32 de Crockford en minúsculas (13 caracteres). Si no coincide con el de Partner
# Center, la Store rechaza el paquete aunque cada campo parezca correcto.
$sha = [System.Security.Cryptography.SHA256]::Create()
try { $huellaPub = $sha.ComputeHash([System.Text.Encoding]::Unicode.GetBytes($idPublisher)) } finally { $sha.Dispose() }
$bits = (($huellaPub[0..7] | ForEach-Object { [Convert]::ToString($_, 2).PadLeft(8, '0') }) -join '') + '0'
$alfabeto = '0123456789abcdefghjkmnpqrstvwxyz'
$idPub = -join (0..12 | ForEach-Object { $alfabeto[[Convert]::ToInt32($bits.Substring($_ * 5, 5), 2)] })
$familia = "${idName}_$idPub"
if ($familia -ne $EsperadoFamilia) { Fallo "Package Family Name calculado «$familia»; Partner Center espera «$EsperadoFamilia»." }
if ($pubDisplay -ne $EsperadoPublisherDisplay) { Fallo "PublisherDisplayName es «$pubDisplay»; debe ser «$EsperadoPublisherDisplay»." }
if ($display -ne $EsperadoDisplayName) { Fallo "DisplayName es «$display»; debe ser «$EsperadoDisplayName»." }
if ($idArq -ne $EsperadoArquitectura) { Fallo "ProcessorArchitecture es «$idArq»; debe ser «$EsperadoArquitectura»." }
if ($idVersion -ne $versionEsperada) {
    Fallo "Version en AppxManifest.xml es «$idVersion», pero engine/Cargo.toml es $cargoVersion (debe ser $versionEsperada)."
}

$idiomas = @($manifiesto.Package.Resources.Resource | ForEach-Object { $_.GetAttribute('Language') })
if (($idiomas -join ',') -ne ($IdiomasEsperados -join ',')) {
    Fallo "Los idiomas del manifiesto deben ser exactamente $($IdiomasEsperados -join ', '); actual: $($idiomas -join ', ')."
}

$app = $manifiesto.Package.Applications.Application
$ejecutable = $app.GetAttribute('Executable')
if ($app.GetAttribute('EntryPoint') -ne 'Windows.FullTrustApplication') {
    Fallo 'Application EntryPoint debe ser Windows.FullTrustApplication.'
}
Write-Host "Identidad: $familia · $idPublisher · v$idVersion ($idArq) · idiomas=$($idiomas -join ',')"

# ── 2. Validar ejecutable PE x64, DLLs y --version ────────────────────────────────────────────────────
Paso 'Ejecutable y DLLs'
if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { Fallo "No existe el ejecutable «$Exe»." }
$Exe = (Resolve-Path -LiteralPath $Exe).Path

function Leer-Pe([string]$ruta) {
    $fs = [System.IO.File]::OpenRead($ruta)
    try {
        $br = New-Object System.IO.BinaryReader($fs)
        if ($br.ReadUInt16() -ne 0x5A4D) { Fallo "«$ruta» no es un ejecutable Windows válido." }
        $fs.Position = 0x3C
        $pe = $br.ReadInt32()
        $fs.Position = $pe
        if ($br.ReadUInt32() -ne 0x00004550) { Fallo "«$ruta» no tiene cabecera PE." }
        $maquina = $br.ReadUInt16()
        $nSecciones = $br.ReadUInt16()
        $fs.Position = $pe + 20
        $tamOpcional = $br.ReadUInt16()
        $opcional = $pe + 24
        $fs.Position = $opcional
        if ($br.ReadUInt16() -ne 0x20B) { Fallo "«$ruta» no es PE32+ (64 bits)." }
        $fs.Position = $opcional + 112 + 1 * 8
        $rvaImport = $br.ReadUInt32()
        $fs.Position = $opcional + 112 + 13 * 8
        $rvaDiferidas = $br.ReadUInt32()
        $secciones = @()
        for ($i = 0; $i -lt $nSecciones; $i++) {
            $fs.Position = $opcional + $tamOpcional + $i * 40 + 8
            $tamVirtual = $br.ReadUInt32()
            $va = $br.ReadUInt32()
            $tamCrudo = $br.ReadUInt32()
            $crudo = $br.ReadUInt32()
            $secciones += New-Object psobject -Property @{ Va = [long]$va; Tam = [long][Math]::Max($tamVirtual, $tamCrudo); Crudo = [long]$crudo }
        }
        $aDesp = {
            param([long]$rva)
            foreach ($s in $secciones) { if ($rva -ge $s.Va -and $rva -lt $s.Va + $s.Tam) { return $rva - $s.Va + $s.Crudo } }
            Fallo "«$ruta»: RVA fuera de secciones PE."
        }
        $leerNombre = {
            param([long]$pos)
            $fs.Position = $pos
            $bytes = New-Object System.Collections.Generic.List[byte]
            while ($bytes.Count -lt 260) { $b = $fs.ReadByte(); if ($b -le 0) { break }; $bytes.Add([byte]$b) }
            [System.Text.Encoding]::ASCII.GetString($bytes.ToArray())
        }
        $dlls = New-Object System.Collections.Generic.List[string]
        if ($rvaImport -ne 0) {
            $base = & $aDesp $rvaImport
            for ($k = 0; ; $k++) {
                $fs.Position = $base + $k * 20 + 12
                $rvaNom = $br.ReadUInt32()
                $thunk = $br.ReadUInt32()
                if ($rvaNom -eq 0 -and $thunk -eq 0) { break }
                $dlls.Add((& $leerNombre (& $aDesp $rvaNom)))
            }
        }
        if ($rvaDiferidas -ne 0) {
            $base = & $aDesp $rvaDiferidas
            for ($k = 0; ; $k++) {
                $fs.Position = $base + $k * 32 + 4
                $rvaNom = $br.ReadUInt32()
                if ($rvaNom -eq 0) { break }
                $dlls.Add((& $leerNombre (& $aDesp $rvaNom)))
            }
        }
        New-Object psobject -Property @{ Maquina = $maquina; Dlls = @($dlls | Sort-Object -Unique) }
    } finally {
        $fs.Dispose()
    }
}

$pe = Leer-Pe $Exe
if ($pe.Maquina -ne 0x8664) { Fallo ('El ejecutable no es x64 (0x{0:X4}).' -f $pe.Maquina) }

# Comprueba salida `--version` usando Process con stdout redirigido (funciona en windows_subsystem="windows").
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Exe
$psi.Arguments = '--version'
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.CreateNoWindow = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$verSalida = $proc.StandardOutput.ReadToEnd().Trim()
$proc.WaitForExit()
if ($proc.ExitCode -ne 0 -or $verSalida -ne "Glowapp Browser $cargoVersion") {
    Fallo "«$Exe --version» devolvió «$verSalida» (código $($proc.ExitCode)); se esperaba «Glowapp Browser $cargoVersion»."
}
Write-Host ("{0} ({1:N1} MB) -> {2}" -f $Exe, ((Get-Item -LiteralPath $Exe).Length / 1MB), $verSalida)

$carpetaExe = Split-Path -Parent $Exe
$sistema = [Environment]::GetFolderPath('System')
$aCopiar = @()
foreach ($dll in $pe.Dlls) {
    $esRuntime = $false
    foreach ($patron in $RuntimesNoDeWindows) { if ($dll -match $patron) { $esRuntime = $true } }
    $alLado = Join-Path $carpetaExe $dll
    if (-not $esRuntime -and ($dll -match '^(api|ext)-ms-' -or (Test-Path -LiteralPath (Join-Path $sistema $dll)))) { continue }
    if (Test-Path -LiteralPath $alLado -PathType Leaf) {
        $aCopiar += $alLado
        Write-Host "DLL incluida en el paquete: $dll"
    } elseif ($esRuntime) {
        Fallo "El .exe importa $dll (runtime externo) y no está junto al .exe. En MSVC usa RUSTFLAGS='-C target-feature=+crt-static'."
    } else {
        Fallo "El .exe importa $dll, que no forma parte de Windows ni está junto al .exe."
    }
}
Write-Host "Importa $($pe.Dlls.Count) DLLs; $($aCopiar.Count) adicionales copiadas al paquete."

# ── 3. Localizar o descargar makeappx.exe oficial verificado ──────────────────────────────────────────
Paso 'makeappx.exe'

function Firma-De-Microsoft([string]$ruta) {
    $firma = Get-AuthenticodeSignature -LiteralPath $ruta
    ($firma.Status -eq 'Valid') -and ($null -ne $firma.SignerCertificate) -and ($firma.SignerCertificate.Subject -match 'O=Microsoft Corporation')
}

function MakeAppx-Del-Sdk {
    $raices = @()
    foreach ($pf in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if ($pf) { $raices += Join-Path $pf 'Windows Kits\10\bin' }
    }
    $candidatos = @()
    foreach ($r in $raices) {
        if (Test-Path -LiteralPath $r) {
            $candidatos += Get-ChildItem -LiteralPath $r -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' } |
                ForEach-Object { New-Object psobject -Property @{ Version = [version]$_.Name; Ruta = Join-Path $_.FullName 'x64\makeappx.exe' } } |
                Where-Object { Test-Path -LiteralPath $_.Ruta }
        }
    }
    $mejor = $candidatos | Sort-Object Version -Descending | Select-Object -First 1
    if ($mejor) { return $mejor.Ruta }
    foreach ($pf in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if ($pf) {
            $wack = Join-Path $pf 'Windows Kits\10\App Certification Kit\makeappx.exe'
            if (Test-Path -LiteralPath $wack) { return $wack }
        }
    }
    $null
}

function MakeAppx-De-NuGet {
    $cache = Join-Path $env:LOCALAPPDATA 'NavegadorBuild\sdk-buildtools'
    $destino = Join-Path $cache $BuildToolsVersion
    $marca = Join-Path $destino 'verificado.txt'
    $buscar = { Get-ChildItem -LiteralPath $destino -Recurse -Filter 'makeappx.exe' | Where-Object { $_.Directory.Name -eq 'x64' } | Select-Object -First 1 }
    if ((Test-Path -LiteralPath $marca) -and ($hallado = & $buscar)) {
        if (-not (Firma-De-Microsoft $hallado.FullName)) { Fallo "makeappx.exe en caché sin firma válida de Microsoft." }
        Write-Host "Usando caché verificada ($BuildToolsVersion)."
        return $hallado.FullName
    }
    New-Item -ItemType Directory -Force -Path $cache | Out-Null
    $nupkg = Join-Path $cache "microsoft.windows.sdk.buildtools.$BuildToolsVersion.nupkg"
    Write-Host "Sin Windows SDK local: descargando Microsoft.Windows.SDK.BuildTools $BuildToolsVersion de nuget.org..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $antes = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try { Invoke-WebRequest -UseBasicParsing -Uri $BuildToolsUrl -OutFile $nupkg } finally { $ProgressPreference = $antes }

    $sha = [System.Security.Cryptography.SHA512]::Create()
    $fs = [System.IO.File]::OpenRead($nupkg)
    try { $huella = [Convert]::ToBase64String($sha.ComputeHash($fs)) } finally { $fs.Dispose(); $sha.Dispose() }
    if ($huella -ne $BuildToolsSha512) {
        Remove-Item -LiteralPath $nupkg -Force
        Fallo "Huella SHA-512 inesperada en nupkg: $huella"
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Add-Type -AssemblyName System.Security
    $zip = [System.IO.Compression.ZipFile]::OpenRead($nupkg)
    try {
        $entrada = $zip.GetEntry('.signature.p7s')
        if ($null -eq $entrada) { Fallo 'Falta .signature.p7s en nupkg.' }
        $ms = New-Object System.IO.MemoryStream
        $s = $entrada.Open(); try { $s.CopyTo($ms) } finally { $s.Dispose() }
        $nuspec = $zip.Entries | Where-Object { $_.FullName -like '*.nuspec' } | Select-Object -First 1
        $lector = New-Object System.IO.StreamReader($nuspec.Open())
        try { [xml]$especificacion = $lector.ReadToEnd() } finally { $lector.Dispose() }
    } finally {
        $zip.Dispose()
    }
    $cms = New-Object System.Security.Cryptography.Pkcs.SignedCms
    $cms.Decode($ms.ToArray())
    $cms.CheckSignature($true)
    $firmante = $cms.SignerInfos[0].Certificate.Subject
    if ($firmante -notmatch 'O=Microsoft Corporation') { Fallo "Firma inesperada en nupkg: $firmante" }
    if ($especificacion.package.metadata.id -ne 'Microsoft.Windows.SDK.BuildTools' -or $especificacion.package.metadata.authors -ne 'Microsoft') {
        Fallo "Metadatos nupkg inesperados."
    }

    $temporal = "$destino.tmp-$PID"
    if (Test-Path -LiteralPath $temporal) { Remove-Item -LiteralPath $temporal -Recurse -Force }
    [System.IO.Compression.ZipFile]::ExtractToDirectory($nupkg, $temporal)
    $hallado = Get-ChildItem -LiteralPath $temporal -Recurse -Filter 'makeappx.exe' | Where-Object { $_.Directory.Name -eq 'x64' } | Select-Object -First 1
    if (-not $hallado -or -not (Firma-De-Microsoft $hallado.FullName)) {
        Remove-Item -LiteralPath $temporal -Recurse -Force
        Fallo 'makeappx.exe extraído no tiene firma válida de Microsoft.'
    }
    if (Test-Path -LiteralPath $destino) { Remove-Item -LiteralPath $destino -Recurse -Force }
    Move-Item -LiteralPath $temporal -Destination $destino
    Set-Content -LiteralPath $marca -Encoding ASCII -Value "Microsoft.Windows.SDK.BuildTools $BuildToolsVersion SHA512 $BuildToolsSha512 $firmante"
    Remove-Item -LiteralPath $nupkg -Force
    (& $buscar).FullName
}

$makeappx = MakeAppx-Del-Sdk
if (-not $makeappx) { $makeappx = MakeAppx-De-NuGet }
Write-Host "Usando: $makeappx"

# ── 4. Armar paquete y ejecutar makeappx pack ─────────────────────────────────────────────────────────
Paso 'Empaquetar MSIX'
New-Item -ItemType Directory -Force -Path $Salida | Out-Null
$Salida = (Resolve-Path -LiteralPath $Salida).Path
$carpeta = Join-Path $Salida 'paquete'
if (Test-Path -LiteralPath $carpeta) { Remove-Item -LiteralPath $carpeta -Recurse -Force }
New-Item -ItemType Directory -Path (Join-Path $carpeta 'Assets') | Out-Null

Copy-Item -LiteralPath $Exe -Destination (Join-Path $carpeta $ejecutable)
foreach ($dll in $aCopiar) { Copy-Item -LiteralPath $dll -Destination $carpeta }
$manifiesto.Save((Join-Path $carpeta 'AppxManifest.xml'))
foreach ($logo in @('StoreLogo.png', 'Square44x44Logo.png', 'Square150x150Logo.png', 'Wide310x150Logo.png')) {
    $rutaLogo = Join-Path (Join-Path $aqui 'Assets') $logo
    if (-not (Test-Path -LiteralPath $rutaLogo)) { Fallo "Falta Assets\$logo (ejecuta generar-logos.ps1)." }
    Copy-Item -LiteralPath $rutaLogo -Destination (Join-Path $carpeta 'Assets')
}
Get-ChildItem -LiteralPath $carpeta -Recurse -File | ForEach-Object {
    Write-Host ("  {0,-32} {1,12:N0} bytes" -f $_.FullName.Substring($carpeta.Length + 1), $_.Length)
}

$msix = Join-Path $Salida "GlowappBrowser_${idVersion}_${EsperadoArquitectura}.msix"
& $makeappx pack /d $carpeta /p $msix /o
if ($LASTEXITCODE -ne 0) { Fallo "makeappx pack falló con código $LASTEXITCODE." }

$sha256 = (Get-FileHash -LiteralPath $msix -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content -LiteralPath (Join-Path $Salida 'SHA256SUMS.txt') -Encoding ASCII -Value "$sha256  $(Split-Path -Leaf $msix)"

Paso 'Listo'
Write-Host ("Paquete MSIX sin firmar: {0} ({1:N1} MB)" -f $msix, ((Get-Item -LiteralPath $msix).Length / 1MB))
Write-Host "SHA-256: $sha256"
