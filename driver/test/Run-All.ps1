# Inside the TEST VM: right-click -> Run with PowerShell is not elevated, so run
#   Start-Process powershell -Verb RunAs -ArgumentList '-ExecutionPolicy Bypass -File B:\test\Run-All.ps1'
# (or open an admin terminal and run it). One UAC approval covers everything:
#   1. Prepare-Guest (test signing + kernel dumps), then reboot
#   2. after the reboot (RunOnce, at the next admin logon): install the driver,
#      run the functional tests and write B:\results\*.log
#Requires -RunAsAdministrator
param([switch]$Stage2)
$ErrorActionPreference = 'Stop'
$results = Join-Path $PSScriptRoot '..\results'
New-Item -ItemType Directory -Force $results | Out-Null
$status = Join-Path $results 'run-all-status.txt'
function Status([string]$s) { "$(Get-Date -Format s) $s" | Add-Content $status; Write-Host $s }

if (-not $Stage2) {
    Status 'stage 1: preparing guest'
    & "$PSScriptRoot\Prepare-Guest.ps1" -NoReboot
    $cmd = "powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Stage2"
    Set-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce' -Name 'BitGGDriverTest' -Value $cmd
    Status 'stage 1 done, rebooting (stage 2 runs at next logon)'
    Restart-Computer -Force
    return
}

try {
    Status 'stage 2: installing driver'
    & "$PSScriptRoot\Install-BitggVad.ps1" *>&1 | Out-File (Join-Path $results 'install.log')
    Status 'stage 2: running tests'
    & "$PSScriptRoot\Run-DriverTests.ps1"
    Status "stage 2 done, failures: $LASTEXITCODE"
} catch {
    Status "stage 2 FAILED: $_"
}
