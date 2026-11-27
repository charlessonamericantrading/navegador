# Genera los cuatro logos requeridos por Microsoft Store en `Assets\*.png` (escala 100 %, sin dependencias externas).
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File engine\packaging\msix\generar-logos.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$destino = Join-Path $PSScriptRoot 'Assets'
New-Item -ItemType Directory -Force -Path $destino | Out-Null

function Dibujar-Isotipo([System.Drawing.Graphics]$g, [float]$cx, [float]$cy, [float]$radio) {
    $azulOscuro = [System.Drawing.Color]::FromArgb(255, 21, 94, 184)
    $azulVivo = [System.Drawing.Color]::FromArgb(255, 37, 132, 245)
    $azulClaro = [System.Drawing.Color]::FromArgb(255, 118, 189, 255)
    $blanco = [System.Drawing.Color]::White

    # Halo / esfera principal con degradado diagonal
    $rectEsfera = New-Object System.Drawing.RectangleF(($cx - $radio), ($cy - $radio), ($radio * 2), ($radio * 2))
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.PointF(($cx - $radio), ($cy - $radio))),
        (New-Object System.Drawing.PointF(($cx + $radio), ($cy + $radio))),
        $azulVivo,
        $azulOscuro
    )
    try {
        $g.FillEllipse($grad, $rectEsfera)
    } finally {
        $grad.Dispose()
    }

    # Arco interior luminoso (órbita / ventana web)
    $penArco = New-Object System.Drawing.Pen($azulClaro, [Math]::Max(1.8, $radio * 0.14))
    $penArco.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $penArco.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    try {
        $r2 = $radio * 0.68
        $g.DrawArc($penArco, $cx - $r2, $cy - $r2, $r2 * 2, $r2 * 2, 200, 145)
    } finally {
        $penArco.Dispose()
    }

    # Aguja de brújula estilizada en diagonal (apunta noreste en blanco, suroeste en azul claro)
    $ang = -45.0 * [Math]::PI / 180.0
    $cos = [float][Math]::Cos($ang)
    $sin = [float][Math]::Sin($ang)
    $largo = $radio * 0.64
    $anchoAla = $radio * 0.22

    $punta = New-Object System.Drawing.PointF(($cx + $largo * $cos), ($cy + $largo * $sin))
    $cola = New-Object System.Drawing.PointF(($cx - $largo * $cos), ($cy - $largo * $sin))
    $ala1 = New-Object System.Drawing.PointF(($cx - $anchoAla * $sin), ($cy + $anchoAla * $cos))
    $ala2 = New-Object System.Drawing.PointF(($cx + $anchoAla * $sin), ($cy - $anchoAla * $cos))

    $brochaBlanca = New-Object System.Drawing.SolidBrush($blanco)
    $brochaSombra = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(230, 198, 228, 255))
    $brochaCentro = New-Object System.Drawing.SolidBrush($azulOscuro)
    try {
        $g.FillPolygon($brochaBlanca, [System.Drawing.PointF[]]@($punta, $ala1, $ala2))
        $g.FillPolygon($brochaSombra, [System.Drawing.PointF[]]@($cola, $ala1, $ala2))
        $rCentro = $radio * 0.09
        $g.FillEllipse($brochaCentro, $cx - $rCentro, $cy - $rCentro, $rCentro * 2, $rCentro * 2)
    } finally {
        $brochaBlanca.Dispose()
        $brochaSombra.Dispose()
        $brochaCentro.Dispose()
    }
}

function Guardar-Logo([int]$ancho, [int]$alto, [string]$nombre) {
    $bmp = New-Object System.Drawing.Bitmap($ancho, $alto, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.Clear([System.Drawing.Color]::White)

        $lado = [float][Math]::Min($ancho, $alto)
        Dibujar-Isotipo $g ($ancho / 2.0) ($alto / 2.0) ($lado * 0.36)

        $ruta = Join-Path $destino $nombre
        $bmp.Save($ruta, [System.Drawing.Imaging.ImageFormat]::Png)
        Write-Output ("{0,-24} {1} x {2}" -f $nombre, $ancho, $alto)
    } finally {
        $g.Dispose()
        $bmp.Dispose()
    }
}

Guardar-Logo 50 50 'StoreLogo.png'
Guardar-Logo 44 44 'Square44x44Logo.png'
Guardar-Logo 150 150 'Square150x150Logo.png'
Guardar-Logo 310 150 'Wide310x150Logo.png'
