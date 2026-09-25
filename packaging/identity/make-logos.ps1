# Draws the package's logos (the Ferroshell mark: a hexagon holding a spiral shell) into
# Assets\. Windows shows them in Settings > Apps and permission prompts. Re-run after
# changing the design; the PNGs are committed.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$assets = Join-Path $PSScriptRoot 'Assets'
New-Item -ItemType Directory -Force $assets | Out-Null

function Draw-Logo([int]$size, [string]$file) {
    $bmp = New-Object Drawing.Bitmap $size, $size
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([Drawing.Color]::Transparent)
    $s = $size / 24.0
    # Hexagon, same geometry as the launcher button (24x24 viewbox).
    $hex = @(@(12,1), @(21.5,6.5), @(21.5,17.5), @(12,23), @(2.5,17.5), @(2.5,6.5)) |
        ForEach-Object { New-Object Drawing.PointF ($_[0] * $s), ($_[1] * $s) }
    $g.FillPolygon((New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(0x3d, 0xae, 0xe9))), [Drawing.PointF[]]$hex)
    # Spiral shell: an inward Archimedean spiral around the centre.
    $pts = for ($i = 0; $i -le 120; $i++) {
        $t = $i / 120.0 * 2.6 * [Math]::PI
        $r = (5.8 - 1.9 * $t / [Math]::PI) * $s
        New-Object Drawing.PointF ((12 * $s) + $r * [Math]::Cos($t + [Math]::PI)), ((12 * $s) + $r * [Math]::Sin($t + [Math]::PI))
    }
    $pen = New-Object Drawing.Pen ([Drawing.Color]::White), ([Math]::Max(1.0, 1.8 * $s))
    $pen.StartCap = $pen.EndCap = [Drawing.Drawing2D.LineCap]::Round
    $g.DrawLines($pen, [Drawing.PointF[]]$pts)
    $bmp.Save((Join-Path $assets $file), [Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

Draw-Logo 150 'Square150x150Logo.png'
Draw-Logo 44 'Square44x44Logo.png'
Draw-Logo 50 'StoreLogo.png'
Write-Host "Logos written to $assets"
