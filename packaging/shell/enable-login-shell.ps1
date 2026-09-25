<#
.SYNOPSIS
    Makes Ferroshell your login shell (instead of Explorer) from your next sign-in. Only
    affects your user account; no admin needed. Undo with disable-login-shell.ps1.

.DESCRIPTION
    Sets HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell to the installed
    fsh-session.exe (see install.ps1) with --replace. Windows starts that instead of
    Explorer when you sign in; fsh-session then starts the panels and your startup apps,
    and falls back to Explorer if Ferroshell can't run.
#>
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Ferroshell')
)

$ErrorActionPreference = 'Stop'
$session = Join-Path $InstallDir 'fsh-session.exe'
if (-not (Test-Path $session)) { throw "$session not found. Run install.ps1 first." }

$key = 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Winlogon'
if (-not (Test-Path $key)) { New-Item -Path $key -Force | Out-Null }
$value = "`"$session`" --replace"
Set-ItemProperty -Path $key -Name 'Shell' -Value $value
Write-Host "Your login shell is now: $value"
Write-Host ""
Write-Host "Sign out and back in to use it. If anything goes wrong at sign-in:"
Write-Host "  * Ferroshell starts Explorer itself if it can't run or keeps crashing."
Write-Host "  * Black screen? Press Ctrl+Alt+Del > Task Manager > Run new task > explorer.exe,"
Write-Host "    then run packaging\shell\disable-login-shell.ps1."
Write-Host "  * The emergency hotkey (see 'fsh-ctl status') stops/starts Ferroshell's panels."
