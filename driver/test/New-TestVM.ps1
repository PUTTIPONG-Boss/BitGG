# Run on the HOST. Creates a VirtualBox VM for driver testing and starts an
# unattended Windows 11 install. Secure Boot stays off (test signing needs that);
# the host's own security settings are not touched.
#   New-TestVM.ps1 -Iso D:\ISO\Win11_English_x64.iso -Package $env:USERPROFILE\bitggvad-test
# The test account's password is generated and saved to <VMDir>\<Name>-login.txt.
param(
    [Parameter(Mandatory)][string]$Iso,
    [string]$Package = "$env:USERPROFILE\bitggvad-test",
    [string]$Name = 'BitGG-DriverTest',
    [string]$VMDir = 'D:\VMs',
    [int]$MemoryMB = 4096,
    [int]$Cpus = 4,
    [int]$DiskGB = 64,
    [int]$ImageIndex = 0      # 0 = pick "Windows 11 Pro" from the ISO
)
$ErrorActionPreference = 'Stop'
$vbox = "$env:ProgramFiles\Oracle\VirtualBox\VBoxManage.exe"
if (-not (Test-Path $vbox)) { throw 'VirtualBox is not installed (winget install Oracle.VirtualBox).' }
function VB { & $vbox @args; if ($LASTEXITCODE -ne 0) { throw "VBoxManage $($args[0]) failed ($LASTEXITCODE)" } }

if ((& $vbox list vms) -match "`"$Name`"") { throw "VM '$Name' already exists." }
if (-not (Test-Path $Iso)) { throw "ISO not found: $Iso" }
New-Item -ItemType Directory -Force $VMDir, $Package | Out-Null

if ($ImageIndex -eq 0) {
    $detect = & $vbox unattended detect --iso="$Iso" | Out-String
    if ($detect -match "(?m)Image\s*#?\s*(\d+)\s*[=:].*Windows 11 Pro\b(?! N)") { $ImageIndex = [int]$Matches[1] }
    else { Write-Host $detect; throw 'Could not find "Windows 11 Pro" in the ISO. Pass -ImageIndex.' }
}

# Generated test-only credentials, kept next to the VM (never typed into chat).
$user = 'tester'
$pw = -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 16 | ForEach-Object { [char]$_ })
$login = Join-Path $VMDir "$Name-login.txt"
"VM: $Name`r`nuser: $user`r`npassword: $pw" | Set-Content -Encoding ASCII $login

VB createvm --name $Name --ostype Windows11_64 --basefolder $VMDir --register
VB modifyvm $Name --firmware efi --tpm-type 2.0 --memory $MemoryMB --cpus $Cpus `
    --graphicscontroller vboxsvga --vram 128 --nic1 nat --audio-enabled off --clipboard-mode bidirectional
$disk = Join-Path $VMDir "$Name\$Name.vdi"
VB createmedium disk --filename $disk --size ($DiskGB * 1024) --format VDI
VB storagectl $Name --name SATA --add sata --controller IntelAhci --portcount 2
VB storageattach $Name --storagectl SATA --port 0 --device 0 --type hdd --medium $disk
VB sharedfolder add $Name --name bitggvad-test --hostpath (Resolve-Path $Package).Path --automount --auto-mount-point 'B:'

VB unattended install $Name --iso="$Iso" --image-index=$ImageIndex --user=$user --password=$pw `
    --full-user-name='BitGG Tester' --locale=en_US --country=US --time-zone=UTC --install-additions --start-vm=gui

Write-Host "VM '$Name' is installing Windows (takes 20-40 min, it reboots by itself)."
Write-Host "Login saved to $login. The test package appears in the VM as drive B:."
Write-Host "When Windows is ready: snapshot it, then follow docs/driver/TESTPLAN.md."
