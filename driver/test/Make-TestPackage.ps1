# Run on the BUILD machine (not the VM). Builds the driver (Debug, test-signed)
# and the BitGG app, and collects everything the VM needs in one folder:
#   <Out>\driver\  bitggvad.sys/.inf/.cat/.cer
#   <Out>\bitgg.exe
#   <Out>\test\    these scripts
# Share <Out> with the VM (New-TestVM.ps1 does that) or copy it in.
param(
    [string]$Out = "$env:USERPROFILE\bitggvad-test",
    [string]$Configuration = 'Debug',
    [string]$BitggPackages = '',   # short WDK NuGet folder if the repo path is very deep
    [string]$BitggOutRoot = ''     # short build output folder, same reason
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$msbuild = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\amd64\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild) { throw 'MSBuild not found (install Visual Studio with the WDK component).' }

$props = @("-p:Configuration=$Configuration", '-p:Platform=x64')
if ($BitggPackages) { $props += "-p:BitggPackages=$BitggPackages" }
if ($BitggOutRoot) { $props += "-p:BitggOutRoot=$BitggOutRoot" }
& $msbuild "$repo\driver\bitggvad\bitggvad.vcxproj" @props -nologo -v:m
if ($LASTEXITCODE -ne 0) { throw 'driver build failed' }
$int = if ($BitggOutRoot) { Join-Path $BitggOutRoot "x64\$Configuration" } else { "$repo\driver\bitggvad\x64\$Configuration" }

Push-Location $repo
# via cmd: PowerShell 5.1 turns cargo's stderr progress into terminating errors
cmd /c "cargo build --release 2>&1" | Out-Host
if ($LASTEXITCODE -ne 0) { Pop-Location; throw 'app build failed' }
$target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
Pop-Location

Remove-Item -Recurse -Force $Out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$Out\driver", "$Out\test", "$Out\results" | Out-Null
Copy-Item "$int\bitggvad\*" "$Out\driver\"
Copy-Item "$int\bitggvad.cer" "$Out\driver\"
Copy-Item "$target\release\bitgg.exe" "$Out\"
Copy-Item "$PSScriptRoot\*.ps1", "$PSScriptRoot\*.psm1" "$Out\test\"
Get-ChildItem -Recurse $Out -File | Select-Object @{n='File';e={$_.FullName.Substring($Out.Length + 1)}}, Length | Format-Table -AutoSize
Write-Host "Test package ready: $Out"
