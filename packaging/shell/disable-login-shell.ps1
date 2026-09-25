<#
.SYNOPSIS
    Makes Explorer your login shell again from your next sign-in (undoes
    enable-login-shell.ps1). Only affects your user account; no admin needed.
#>
$ErrorActionPreference = 'Stop'
$key = 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Winlogon'
if ((Get-ItemProperty -Path $key -ErrorAction SilentlyContinue).Shell) {
    Remove-ItemProperty -Path $key -Name 'Shell'
    Write-Host "Removed your per-user login shell; Explorer starts at your next sign-in."
} else {
    Write-Host "No per-user login shell was set; Explorer is already your login shell."
}
Write-Host "Ferroshell can still run alongside Explorer: start fsh-session.exe without --replace."
