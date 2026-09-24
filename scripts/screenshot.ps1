# Capture the screen (or a region of it) to a PNG. Used to check panel rendering.
# Usage: scripts\screenshot.ps1 -Out shot.png [-Bottom 120]   (Bottom = only the bottom N pixels)
param([Parameter(Mandatory)][string]$Out, [int]$Bottom = 0, [int]$Top = 0)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -Namespace W -Name Dpi -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();'
[W.Dpi]::SetProcessDPIAware() | Out-Null
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$y = $b.Y; $h = $b.Height
if ($Bottom -gt 0) { $y = $b.Bottom - $Bottom; $h = $Bottom }
elseif ($Top -gt 0) { $h = $Top }
$bmp = New-Object System.Drawing.Bitmap $b.Width, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.X, $y, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
