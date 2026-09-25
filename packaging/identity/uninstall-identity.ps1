<#
.SYNOPSIS
    Undoes install-identity.ps1: unregisters Ferroshell's identity package and removes its
    certificate from Local Machine > Trusted People. Run as administrator.

.DESCRIPTION
    Ferroshell keeps working afterwards, just without package identity (so without the
    notifications applet). Pass -RemoveSigningCertificate to also delete the signing
    certificate build-package.ps1 created in your personal store.
#>
#Requires -RunAsAdministrator
param(
    [string]$Subject = 'CN=Ferroshell Local Identity',
    [switch]$RemoveSigningCertificate
)

$ErrorActionPreference = 'Stop'

$pkg = Get-AppxPackage -Name 'Ferroshell.Shell'
if ($pkg) {
    Write-Host "Unregistering $($pkg.PackageFullName)"
    $pkg | Remove-AppxPackage
} else {
    Write-Host "The identity package isn't registered."
}

$trusted = Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object Subject -eq $Subject
foreach ($c in $trusted) {
    Write-Host "Removing trusted certificate $($c.Thumbprint)"
    Remove-Item $c.PSPath
}

if ($RemoveSigningCertificate) {
    foreach ($c in (Get-ChildItem Cert:\CurrentUser\My | Where-Object Subject -eq $Subject)) {
        Write-Host "Removing signing certificate $($c.Thumbprint)"
        Remove-Item $c.PSPath
    }
}

Write-Host "Done. Restart the shell (fsh-ctl restart) to drop its identity."
