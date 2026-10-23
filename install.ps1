# BitGG installer: copies the app to %LOCALAPPDATA%\Programs\BitGG and creates
# Desktop + Start Menu shortcuts.  Optional: -Startup also starts BitGG in the
# tray when you log in.   Uninstall: .\install.ps1 -Uninstall
param([switch]$Startup, [switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$dir   = Join-Path $env:LOCALAPPDATA 'Programs\BitGG'
$exe   = Join-Path $dir 'BitGG.exe'
$links = @(
  (Join-Path ([Environment]::GetFolderPath('Desktop'))  'BitGG.lnk'),
  (Join-Path ([Environment]::GetFolderPath('Programs')) 'BitGG.lnk')
)
$boot  = Join-Path ([Environment]::GetFolderPath('Startup')) 'BitGG.lnk'

Get-Process BitGG -ErrorAction SilentlyContinue | ForEach-Object { $_ | Stop-Process -Force; $_.WaitForExit(5000) | Out-Null }
if ($Uninstall) {
  $links + $boot | Where-Object { Test-Path $_ } | Remove-Item -Force
  if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
  'BitGG removed (settings kept in %APPDATA%\BitGG).'; return
}

New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item -Force (Join-Path $PSScriptRoot 'dist\BitGG.exe') $exe

$sh = New-Object -ComObject WScript.Shell
function New-Link($path, $arguments) {
  $l = $sh.CreateShortcut($path)
  $l.TargetPath = $exe; $l.Arguments = $arguments; $l.WorkingDirectory = $dir
  $l.IconLocation = "$exe,0"; $l.Description = 'BitGG - Sound & Clips'
  $l.Save()
}
foreach ($p in $links) { New-Link $p '' }
if ($Startup) { New-Link $boot '--tray' } elseif (Test-Path $boot) { Remove-Item -Force $boot }

"Installed: $exe"
$links | ForEach-Object { "Shortcut:  $_" }
if ($Startup) { "Autostart: $boot (starts in tray)" }

