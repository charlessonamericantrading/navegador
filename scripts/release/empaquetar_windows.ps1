# Empaqueta una distribución portable + instalador 100 % Rust para Windows.
param(
    [string]$Salida = ""
)

$ErrorActionPreference = "Stop"
$Raiz = (Resolve-Path "$PSScriptRoot\..\..").Path
$Engine = Join-Path $Raiz "engine"
if ([string]::IsNullOrWhiteSpace($Salida)) {
    $Salida = Join-Path $Engine "target\dist"
}

Write-Host "Compilando navegador e instalador en release..."
cargo build --release --locked --manifest-path (Join-Path $Engine "Cargo.toml") --bin navegador --bin instalador
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Release = Join-Path $Engine "target\release"
$Paquete = Join-Path $Salida "NavegadorIA-windows"
if (Test-Path $Paquete) { Remove-Item -Recurse -Force $Paquete }
New-Item -ItemType Directory -Force $Paquete | Out-Null

Copy-Item (Join-Path $Release "navegador.exe") $Paquete
Copy-Item (Join-Path $Release "instalador.exe") $Paquete
Copy-Item (Join-Path $Raiz "LICENSE") $Paquete
Copy-Item (Join-Path $Raiz "README.md") $Paquete
Copy-Item (Join-Path $Raiz "SECURITY.md") $Paquete

$Zip = Join-Path $Salida "NavegadorIA-windows.zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
Compress-Archive -Path "$Paquete\*" -DestinationPath $Zip
Remove-Item -Recurse -Force $Paquete

Write-Host "Archivo comprimido listo: $Zip"
