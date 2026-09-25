<#
.SYNOPSIS
    Gives fsh-shell.exe package identity: trusts Ferroshell's signing certificate and
    registers the identity package. Run as administrator, after build-package.ps1.

.DESCRIPTION
    This changes system state, which is why you run it rather than Ferroshell:
      1. imports out\Ferroshell.Identity.cer into Local Machine > Trusted People, so
         Windows accepts packages signed with Ferroshell's self-signed certificate;
      2. registers out\Ferroshell.Identity.msix for your user, with -ExternalLocation
         pointing at the folder that contains fsh-shell.exe (default: target\release).

    Then restart the shell (`fsh-ctl restart`) so it starts with identity. Undo everything
    with uninstall-identity.ps1.
#>
#Requires -RunAsAdministrator
param(
    # The folder containing the fsh-shell.exe that should get identity.
    [string]$ExternalLocation = (Join-Path $PSScriptRoot '..\..\target\release')
)

$ErrorActionPreference = 'Stop'
$out = Join-Path $PSScriptRoot 'out'
$msix = Join-Path $out 'Ferroshell.Identity.msix'
$cer = Join-Path $out 'Ferroshell.Identity.cer'
$ExternalLocation = (Resolve-Path $ExternalLocation).Path
$exe = Join-Path $ExternalLocation 'fsh-shell.exe'

if (-not (Test-Path $msix) -or -not (Test-Path $cer)) { throw "Run build-package.ps1 (as your normal user) first." }
if (-not (Test-Path $exe)) { throw "$exe not found. Build Ferroshell (cargo build --release) or pass -ExternalLocation." }

Write-Host "1/2 Trusting the Ferroshell signing certificate (Local Machine > Trusted People)"
Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null

Write-Host "2/2 Registering the identity package for $ExternalLocation"
Add-AppxPackage -Path $msix -ExternalLocation $ExternalLocation -ForceUpdateFromAnyVersion
Get-AppxPackage -Name 'Ferroshell.Shell' | Format-List Name, Version, PackageFullName, Status

# Check: a fresh fsh-shell.exe process should now report its identity (exit code 0).
$report = Join-Path $env:TEMP 'fsh-identity-check.json'
$p = Start-Process -FilePath $exe -ArgumentList '--identity' -Wait -PassThru -WindowStyle Hidden -RedirectStandardOutput $report
$text = if (Test-Path $report) { Get-Content $report -Raw } else { '' }
Remove-Item $report -ErrorAction SilentlyContinue
if ($p.ExitCode -eq 0) {
    Write-Host "OK: fsh-shell.exe has identity: $text"
    Write-Host "Restart the shell to pick it up:  fsh-ctl restart"
} else {
    Write-Warning "fsh-shell.exe still reports no identity: $text"
    Write-Warning "Check that $exe was built with the embedded manifest (cargo build --release)."
}
