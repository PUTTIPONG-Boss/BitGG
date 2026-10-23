# BitGG Virtual Audio: VM test plan (Phase 2)

> **สรุปภาษาไทย:** ทดสอบ driver ใน VirtualBox เท่านั้น ห้ามติดตั้งบนเครื่องเล่นเกม ขั้นตอนคือ
> 1. ลง VirtualBox + โหลด ISO Windows 11
> 2. สร้าง VM ด้วย `New-TestVM.ps1` (ติดตั้ง Windows อัตโนมัติ)
> 3. ใน VM เปิด test signing แล้วติดตั้ง driver
> 4. รัน `Run-DriverTests.ps1` ซึ่งจะเปิดเสียงทดสอบผ่านทุก endpoint แล้ววัดผลให้อัตโนมัติ (ผ่าน/ไม่ผ่าน)
> 5. เปิด Driver Verifier แล้ว soak 30 นาที
>
> ถ้า VM จอฟ้า ให้ย้อน snapshot แล้วเก็บไฟล์ dump มาวิเคราะห์

Implements [SPEC.md §6](SPEC.md#6-build--test-plan). Everything below happens **inside a VM**. A kernel driver bug crashes the whole machine, and test signing needs Secure Boot off. The host keeps Secure Boot and Memory Integrity on.

## 0. What you need

| Item | Notes |
|---|---|
| Host | Windows 11 with CPU virtualization. This one: i5-12500H, 16 GB RAM, D: 119 GB free. |
| VirtualBox 7.1+ | `winget install Oracle.VirtualBox`. Runs on top of Windows' hypervisor (Memory Integrity stays on; VMs are a bit slower). |
| Windows 11 x64 ISO | From microsoft.com/software-download/windows11 ("Download Windows 11 Disk Image"), ~6-7 GB. The VM can stay unactivated. |
| Build tools | Visual Studio + WDK component, `nuget`, Rust (only to build the test package). |
| Disk | ~40 GB for the VM (64 GB dynamic disk). |

## 1. Build the test package (host)

```powershell
driver\test\Make-TestPackage.ps1            # -> %USERPROFILE%\bitggvad-test
```

It contains `driver\` (test-signed `bitggvad.sys/.inf/.cat` + `bitggvad.cer`), `bitgg.exe`, `test\` (these scripts) and `results\`. In a very deep repo folder, add `-BitggPackages <short>\ -BitggOutRoot <short>\` (see [driver/README.md](../../driver/README.md)).

## 2. Create the VM (host)

```powershell
driver\test\New-TestVM.ps1 -Iso D:\ISO\Win11_x64.iso
```

- **VM:** `BitGG-DriverTest` in `D:\VMs` with EFI, TPM 2.0, Secure Boot **off**, 4 GB RAM, 4 CPUs, NAT and no VM sound card.
- **Install:** unattended Windows 11 Pro, plus Guest Additions.
- **Login:** user `tester`. The generated password is in `D:\VMs\BitGG-DriverTest-login.txt`.
- **Test package:** appears in the VM as drive **B:** (shared folder). Logs are written back to `B:\results`.

When Windows reaches the desktop, shut it down from inside, remove the install ISOs, and take a snapshot **while the VM is powered off**:

```powershell
VBoxManage storageattach BitGG-DriverTest --storagectl SATA --port 1 --device 0 --medium emptydrive   # repeat for each DVD port VBoxManage showvminfo lists
VBoxManage snapshot BitGG-DriverTest take clean-install
```

> **Never take a live snapshot (VM running) on this host.** With Memory Integrity on, VirtualBox runs on top of Hyper-V ("NEM"). A live snapshot failed with `VERR_VM_UNEXPECTED_UNSTABLE_STATE` and left the VM frozen. Only an elevated `taskkill /F /IM VirtualBoxVM.exe` could stop it. If the install ISOs are still attached, the VM then boots Windows Setup again.

## 3. Prepare the guest (VM, elevated PowerShell)

```powershell
Set-ExecutionPolicy -Scope Process Bypass
B:\test\Prepare-Guest.ps1     # test signing on + kernel dumps, reboots
```

After the reboot ("Test Mode" shows on the desktop), shut down and take the snapshot `testsigning` (powered off). This is the restore point after every crash.

## 4. Test cases

**A:** automated by `Run-DriverTests.ps1` / `bitgg --drivertest`. **M:** manual.

| # | Check | How | Pass |
|---|---|---|---|
| T1 | Install | `B:\test\Install-BitggVad.ps1` | Script ends "Installed"; Device Manager → Sound: **BitGG Virtual Audio**, no warning icon |
| T2 | Endpoints exist (A) | `--drivertest` | 5 render + 1 capture endpoint named `BitGG Game/Chat/Media/Aux/Mic (in)` and `BitGG Mic`, all 48000 Hz, 2 ch |
| T3 | Names and icons (M) | Settings → System → Sound | Names as above; render = speaker icon, Mic = microphone icon; Sound panel → Properties → no "Enhancements" tab |
| T4 | Render sinks (A) | 440 Hz tone at 0.5 into each render endpoint, read back with WASAPI loopback | Level within ±0.5 dB, 440 Hz purity > 0.95, no gap ≥ 2 ms |
| T5 | Mic loopback level (A) | Tone into **Mic (in)**, record **Mic** at 100 % volume | Within **±0.1 dB**, purity > 0.95, no gap |
| T6 | Mic silent when idle (A) | Record Mic with nothing playing | Peak < 0.0001 |
| T7 | No stale audio (A) | Play 1 s, stop, wait 0.5 s, record | Peak < 0.0001 |
| T8 | Two readers (A) | Two streams record Mic at once | Both pass T5 |
| T9 | Open/close ×20 (A) | All streams opened and closed 20× | 0 errors |
| T10 | Disable/enable ×3 (A) | `pnputil /disable-device` + `/enable-device`, then T2-T9 again | All pass after each cycle |
| T11 | Volume slider (M) | Sound settings → BitGG Mic → volume 50 %, then 0 %; make BitGG Mic (in) the default output and play music, record Mic with Sound Recorder | 50 % is clearly quieter, 0 % / mute is silent (Windows applies software volume) |
| T12 | 7.1 on Game (M) | Sound panel → BitGG Game → Configure → 7.1, play a channel test | Configure works; stereo default restores |
| T13 | Sleep/resume (M) | `VBoxManage controlvm BitGG-DriverTest acpisleepbutton`, wake, rerun tests | All pass, no crash |
| T14 | Uninstall/reinstall (A+M) | `Uninstall-BitggVad.ps1`, reboot, `Install-BitggVad.ps1` | Endpoints gone after uninstall, back after reinstall; `pnputil /enum-drivers` has no leftover bitggvad |
| T15 | Driver Verifier soak (A) | `Set-Verifier.ps1 -On`, reboot, `Run-DriverTests.ps1 -SoakMinutes 30` | No bugcheck; `verifier /query` shows no violations; soak line PASS |
| T16 | Clean unload under Verifier (A) | With Verifier on: `Uninstall-BitggVad.ps1` | No bugcheck (pool leak = 0xC4 bugcheck on unload) |
| T17 | BitGG end to end (M) | `B:\bitgg.exe` → SETUP: GAME/CHAT/MEDIA/AUX = BitGG Game/Chat/Media/Aux, MIC OUT = BitGG Mic (in); then record "BitGG Mic" in another app | Mixer, EQ, ChatMix and mic gate work; the other app hears the processed mic. Auto-detect comes with SPEC §4.4 |

Run order: T1 → `Run-DriverTests.ps1` (T2, T4-T10) → T3, T11-T13 → T14 → T15-T16 → T17.

## 5. When something fails

| Symptom | Likely cause | What to do |
|---|---|---|
| Device Manager **Code 52** | signature not trusted | test signing off, or `bitggvad.cer` not imported: rerun Prepare-Guest / Install |
| **Code 10** (failed to start) | DriverEntry/StartDevice error | `C:\Windows\INF\setupapi.dev.log`, then WinDbg + DbgView for the DPF output (Debug build) |
| Endpoints missing but device OK | interface/INF mismatch | KSNAME strings in `bitggvad.inx` vs `minipairs.h` |
| Wrong level or purity in T5 | loopback ring | check format (both sides 48 kHz 16-bit stereo) and `bitggloop.cpp` |
| VM blue screen | driver bug | after reboot `Run-DriverTests.ps1` lists dumps. Copy `C:\Windows\MEMORY.DMP` to `B:\results`, then analyse on the host: `windbg -z MEMORY.DMP` → `!analyze -v`. Restore snapshot `testsigning` |

## 6. Results

Every run writes `B:\results\drivertest-<time>.log`, which is the package's `results\` folder on the host. Attach the logs to the PR or issue. Record a summary here:

| Date | Build (commit) | Windows build | Verifier | Result | Notes |
|---|---|---|---|---|---|
| | | | | | |
