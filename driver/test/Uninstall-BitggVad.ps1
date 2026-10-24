# Removes the bitggvad device(s) and driver package inside the test VM (elevated).
#Requires -RunAsAdministrator
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot\BitggVad.psm1" -Force

foreach ($d in Get-BitggDevice) {
    pnputil /remove-device $d.InstanceId | Out-Host
}
foreach ($oem in Get-BitggDriverPackage) {
    pnputil /delete-driver $oem /uninstall /force | Out-Host
}
if (Get-BitggDevice) { throw 'A BitGG device is still present.' }
if (Get-Service bitggvad -ErrorAction SilentlyContinue) { Write-Warning 'Service entry still present (removed after reboot).' }
Write-Host 'Removed.'
