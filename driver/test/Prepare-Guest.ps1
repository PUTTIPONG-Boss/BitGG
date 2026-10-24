# Run ONCE inside the fresh test VM (elevated). Turns on test signing and full
# kernel crash dumps, then reboots. Never run this on your own PC.
#Requires -RunAsAdministrator
param([switch]$NoReboot)
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot\BitggVad.psm1" -Force

$sb = $false
try { $sb = Confirm-SecureBootUEFI } catch { }
if ($sb) { throw 'Secure Boot is ON in this VM. Turn it off in the VM settings (test signing needs it off).' }

bcdedit /set testsigning on | Out-Null
# Kernel memory dump on bugcheck, so a driver crash can be analysed later.
Set-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CrashControl' -Name CrashDumpEnabled -Value 2
Set-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CrashControl' -Name AutoReboot -Value 1
Write-Host 'Test signing on, kernel dumps on.'
if (-not $NoReboot) { Write-Host 'Rebooting in 5 s...'; Start-Sleep 5; Restart-Computer -Force }
