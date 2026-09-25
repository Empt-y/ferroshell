<#
.SYNOPSIS
    Installs the release build of Ferroshell to %LOCALAPPDATA%\Programs\Ferroshell, the copy
    the login shell runs from. Run as your normal user (no admin).

.DESCRIPTION
    Rebuilding in target\release never touches the installed copy, so a broken or cleaned
    build can't break sign-in; run this again to update it. The previous version is kept in
    `previous\` next to it.

    If Ferroshell is running it is stopped for the copy and started again afterwards (as the
    login shell if it was one).

    Notifications need the package identity to point at the installed folder: after the
    first install, run (as administrator)
        packaging\identity\install-identity.ps1 -ExternalLocation "$env:LOCALAPPDATA\Programs\Ferroshell"
#>
param(
    [string]$Source = (Join-Path $PSScriptRoot '..\..\target\release'),
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\Ferroshell')
)

$ErrorActionPreference = 'Stop'
$exes = 'fsh-session', 'fsh-shell', 'fsh-ctl', 'fsh-settings', 'fsh-sysmon'
$Source = (Resolve-Path $Source).Path

foreach ($e in $exes) {
    if (-not (Test-Path (Join-Path $Source "$e.exe"))) { throw "$e.exe not found in $Source. Build first: cargo build --release" }
}

# Stop a running Ferroshell (wherever it runs from), remembering whether it was the login shell.
$ctl = @((Join-Path $Destination 'fsh-ctl.exe'), (Join-Path $Source 'fsh-ctl.exe')) | Where-Object { Test-Path $_ } | Select-Object -First 1
$wasRunning = [bool](Get-Process fsh-session -ErrorAction SilentlyContinue)
$wasReplace = $false
if ($wasRunning -and $ctl) {
    try { $wasReplace = [bool]((& $ctl status | ConvertFrom-Json).session.replace_mode) } catch { }
    Write-Host "Stopping Ferroshell for the update"
    & $ctl quit | Out-Null
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Process fsh-session, fsh-shell -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    if ($wasReplace -and -not (Get-Process explorer -ErrorAction SilentlyContinue)) {
        # Stopping the login shell leaves no desktop for a moment; the restart below brings it back.
        Write-Host "(Ferroshell was your login shell; it restarts in a moment)"
    }
}

New-Item -ItemType Directory -Force $Destination | Out-Null
$previous = Join-Path $Destination 'previous'
if (Test-Path (Join-Path $Destination 'fsh-session.exe')) {
    New-Item -ItemType Directory -Force $previous | Out-Null
    foreach ($e in $exes) {
        foreach ($ext in 'exe', 'pdb') {
            $f = Join-Path $Destination "$e.$ext"
            if (Test-Path $f) { Copy-Item $f $previous -Force }
        }
    }
}
foreach ($e in $exes) {
    Copy-Item (Join-Path $Source "$e.exe") $Destination -Force
    $pdb = Join-Path $Source "$e.pdb"
    if (Test-Path $pdb) { Copy-Item $pdb $Destination -Force }
}
Write-Host "Installed Ferroshell to $Destination"

if ($wasRunning) {
    $sessionExe = Join-Path $Destination 'fsh-session.exe'
    if ($wasReplace) { Start-Process -FilePath $sessionExe -ArgumentList '--replace' } else { Start-Process -FilePath $sessionExe }
    Write-Host "Started Ferroshell again"
}

$pkg = Get-AppxPackage -Name 'Ferroshell.Shell' -ErrorAction SilentlyContinue
if ($pkg) {
    Write-Host ""
    Write-Host "Notifications: if you haven't yet, point the identity package at this folder (as administrator):"
    Write-Host "  packaging\identity\install-identity.ps1 -ExternalLocation `"$Destination`""
}
