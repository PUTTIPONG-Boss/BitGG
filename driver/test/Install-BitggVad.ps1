# Installs the test-signed bitggvad driver package inside the test VM (elevated).
# -Package: folder with bitggvad.sys/.inf/.cat/.cer (from Make-TestPackage.ps1).
#Requires -RunAsAdministrator
param([string]$Package = "$PSScriptRoot\..\driver")
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot\BitggVad.psm1" -Force

if (-not (Test-TestSigning)) { throw 'Test signing is off. Run Prepare-Guest.ps1 first.' }
$inf = (Resolve-Path (Join-Path $Package 'bitggvad.inf')).Path
$cer = Join-Path $Package 'bitggvad.cer'

if (Get-BitggDevice) { throw 'BitGG Virtual Audio is already installed. Run Uninstall-BitggVad.ps1 first.' }

# Trust the build machine's test certificate (VM only).
Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null

pnputil /add-driver $inf | Out-Host
if ($LASTEXITCODE -ne 0) { throw "pnputil /add-driver failed ($LASTEXITCODE)" }

$reboot = [BitggSetupApi]::CreateRootDevice($HardwareId, $inf)
$dev = Get-BitggDevice
$dev | Format-Table Status, Class, FriendlyName, InstanceId -AutoSize | Out-Host
if (-not $dev -or $dev.Status -ne 'OK') { throw "Device did not start (status: $($dev.Status)). Check Device Manager / setupapi.dev.log." }
if ($reboot) { Write-Warning 'Windows asked for a reboot.' }
Write-Host 'Installed. Next: Run-DriverTests.ps1'
