<#
.SYNOPSIS
    Builds and signs Ferroshell's identity package into packaging\identity\out\.

.DESCRIPTION
    Run as your normal user (no admin needed). It:
      1. creates, the first time, a self-signed code-signing certificate
         "CN=Ferroshell Local Identity" in your personal certificate store
         (Cert:\CurrentUser\My). Nothing trusts it until you run install-identity.ps1;
      2. packs AppxManifest.xml and Assets\ into Ferroshell.Identity.msix with the
         Windows SDK's makeappx;
      3. signs it with signtool and exports the certificate's public part to
         Ferroshell.Identity.cer for install-identity.ps1.

    Re-run it after changing AppxManifest.xml (bump its Version too).
#>
param(
    # Must equal the Publisher in AppxManifest.xml and fsh-shell.manifest.
    [string]$Subject = 'CN=Ferroshell Local Identity'
)

$ErrorActionPreference = 'Stop'
$out = Join-Path $PSScriptRoot 'out'
$staging = Join-Path $out 'staging'
$msix = Join-Path $out 'Ferroshell.Identity.msix'
$cer = Join-Path $out 'Ferroshell.Identity.cer'

function Find-SdkTool([string]$name) {
    $root = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $tool = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue |
        Where-Object Name -match '^\d+\.\d+\.\d+\.\d+$' |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$name" } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1
    if (-not $tool) { throw "$name not found. Install the Windows SDK (it provides makeappx and signtool)." }
    $tool
}

$makeappx = Find-SdkTool 'makeappx.exe'
$signtool = Find-SdkTool 'signtool.exe'

# 1. Signing certificate (reused while valid).
$cert = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Subject -eq $Subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date).AddDays(30) } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1
if ($cert) {
    Write-Host "Using signing certificate $($cert.Thumbprint) (expires $($cert.NotAfter.ToShortDateString()))"
} else {
    Write-Host "Creating signing certificate '$Subject' in Cert:\CurrentUser\My"
    $cert = New-SelfSignedCertificate -Type Custom -Subject $Subject -KeyUsage DigitalSignature `
        -FriendlyName 'Ferroshell identity package signing' -CertStoreLocation Cert:\CurrentUser\My `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}') `
        -NotAfter (Get-Date).AddYears(5)
}

# 2. Pack.
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Force $staging | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'AppxManifest.xml') $staging
Copy-Item (Join-Path $PSScriptRoot 'Assets') $staging -Recurse
& $makeappx pack /o /nv /d $staging /p $msix
if ($LASTEXITCODE -ne 0) { throw "makeappx failed ($LASTEXITCODE)" }

# 3. Sign, and export the public certificate for install-identity.ps1.
& $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /s My $msix
if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }
Export-Certificate -Cert $cert -FilePath $cer -Force | Out-Null

Write-Host ""
Write-Host "Built $msix"
Write-Host "Next: run install-identity.ps1 as administrator."
