# Runs the bitggvad checks inside the test VM (elevated) and writes a log.
#   Run-DriverTests.ps1                 functional checks + disable/enable cycles
#   Run-DriverTests.ps1 -SoakMinutes 30 long run (do this with Driver Verifier on)
#Requires -RunAsAdministrator
param(
    [string]$Bitgg = "$PSScriptRoot\..\bitgg.exe",
    [string]$OutDir = "$PSScriptRoot\..\results",
    [int]$SoakMinutes = 0,
    [int]$Cycles = 3
)
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot\BitggVad.psm1" -Force

New-Item -ItemType Directory -Force $OutDir | Out-Null
$log = Join-Path $OutDir ("drivertest-{0:yyyyMMdd-HHmmss}.log" -f (Get-Date))
$start = Get-Date
$fails = 0
function Log([string]$s) { $s | Tee-Object -FilePath $log -Append | Out-Host }
function Step([string]$name, [scriptblock]$body) {
    Log "== $name"
    $out = & $body 2>&1 | Out-String
    Log $out.TrimEnd()
}
function RunBitgg([string[]]$a) {
    $out = & $Bitgg --drivertest @a 2>&1 | Out-String
    Log $out.TrimEnd()
    $script:fails += $LASTEXITCODE
}

Log "bitggvad test run $(Get-Date -Format s) on $([Environment]::OSVersion.VersionString)"
Step 'test signing' { if (Test-TestSigning) { 'on' } else { 'OFF' } }
Step 'verifier' { verifier /query }
$dev = Get-BitggDevice
Step 'device' { $dev | Format-Table Status, FriendlyName, InstanceId -AutoSize }
if (-not $dev -or $dev.Status -ne 'OK') { Log 'FAIL device not running'; exit 1 }
Step 'driver file' { Get-Item "$env:SystemRoot\System32\DriverStore\FileRepository\bitggvad.inf_*\bitggvad.sys" | ForEach-Object { "$($_.FullName) $($_.Length) bytes, $((Get-AuthenticodeSignature $_.FullName).SignerCertificate.Subject)" } }
Step 'endpoints' { Get-PnpDevice -Class AudioEndpoint -PresentOnly | Where-Object FriendlyName -like 'BitGG*' | Format-Table Status, FriendlyName -AutoSize }

if ($SoakMinutes -gt 0) {
    Log "== soak $SoakMinutes min"
    RunBitgg @('soak', "$SoakMinutes")
} else {
    Log '== functional'
    RunBitgg @()
    for ($i = 1; $i -le $Cycles; $i++) {
        Log "== disable/enable cycle $i"
        pnputil /disable-device $dev.InstanceId | Out-Null
        Start-Sleep 2
        pnputil /enable-device $dev.InstanceId | Out-Null
        Start-Sleep 5   # let the audio service rebuild the endpoints
        RunBitgg @()
    }
}

# Any bugcheck since the run started (or a dump left by a crash before it)?
Step 'crash dumps' {
    Get-ChildItem "$env:SystemRoot\Minidump", "$env:SystemRoot\MEMORY.DMP" -ErrorAction SilentlyContinue | Format-Table FullName, LastWriteTime -AutoSize
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; Id = 1001; ProviderName = 'Microsoft-Windows-WER-SystemErrorReporting' } -MaxEvents 5 -ErrorAction SilentlyContinue | Format-List TimeCreated, Message
}
Log "== result: $fails failure(s), $([int]((Get-Date) - $start).TotalMinutes) min. Log: $log"
exit $fails
