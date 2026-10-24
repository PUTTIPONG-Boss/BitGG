# Driver Verifier for bitggvad inside the test VM (elevated). Needs a reboot.
#   Set-Verifier.ps1 -On     standard checks (includes DDI compliance) + pool tracking
#   Set-Verifier.ps1 -Off    clear settings
#   Set-Verifier.ps1         show current state
#Requires -RunAsAdministrator
param([switch]$On, [switch]$Off)
if ($On) {
    verifier /standard /driver bitggvad.sys | Out-Host
    Write-Host 'Reboot the VM to activate Driver Verifier.'
} elseif ($Off) {
    verifier /reset | Out-Host
    Write-Host 'Reboot the VM to deactivate Driver Verifier.'
} else {
    verifier /querysettings | Out-Host
    verifier /query | Out-Host
}
