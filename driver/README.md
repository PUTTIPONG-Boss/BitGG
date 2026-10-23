# BitGG Virtual Audio driver (`bitggvad`)

> **สรุปภาษาไทย:** โฟลเดอร์นี้คือ driver เสียงเสมือนของ BitGG ดัดแปลงจากตัวอย่าง SysVAD ของ Microsoft ตอนนี้ **build ได้แล้วแต่ยังไม่ได้ทดสอบติดตั้งจริง** ห้ามติดตั้งบนเครื่องที่ใช้เล่นเกม ให้ทดสอบใน VM เท่านั้น (ดู [SPEC.md](../docs/driver/SPEC.md) ข้อ 6) ตัวแอป BitGG ยังใช้ Sonar / VB-Cable / DIRECT mode ได้ตามปกติ

**Status: builds, not yet tested.** Do not install it on your gaming PC. Test it in a VM only, following [TESTPLAN.md](../docs/driver/TESTPLAN.md) (scripts in [`test/`](test/), checks in `bitgg --drivertest`). The BitGG app does not use this driver yet.

## What it creates

One root-enumerated device, **BitGG Virtual Audio** (`ROOT\BitGG_VirtualAudio`), with six endpoints:

| Endpoint | Flow | Formats | Notes |
|---|---|---|---|
| BitGG Game | render | 48 kHz 16-bit, stereo or 7.1 | sink, BitGG reads it with WASAPI loopback |
| BitGG Chat | render | 48 kHz 16-bit stereo | sink |
| BitGG Media | render | 48 kHz 16-bit stereo | sink |
| BitGG Aux | render | 48 kHz 16-bit stereo | sink |
| BitGG Mic (in) | render | 48 kHz 16-bit stereo | BitGG writes the processed mic here |
| BitGG Mic | capture | 48 kHz 16-bit stereo | apps (Discord...) record from here; it plays back what was written to Mic (in) |

These are the device formats. Shared-mode apps can use any format, because Windows converts.

## What changed from SysVAD

The first commit on this folder is an unmodified copy of SysVAD, so `git log -p driver/bitggvad` shows every BitGG change. Upstream: [microsoft/Windows-driver-samples](https://github.com/microsoft/Windows-driver-samples) `audio/sysvad`, commit `bb68ed5e770bce04dc5395ec9258dafab034184d`.

- **Endpoints:** SysVAD's sample endpoints were replaced by the six above (`minipairs.h`, `bitggwavtable.h`, `bitggtoptable.h`). Each endpoint has a named topology pin, so Windows shows "BitGG Game" etc.
- **No file writing:** SysVAD could save rendered audio to `.wav` files (`savedata.*`). That code is gone.
- **No test tone:** the sine generator (`ToneGenerator.*`) is gone.
- **Mic loopback:** `bitggloop.cpp` is a ~340 ms ring buffer. Audio rendered to *Mic (in)* is appended. Every *Mic* capture stream reads it from its own position, so several apps can record at once. Underrun gives silence, and a reader more than 100 ms behind skips ahead.
- **No hardware volume nodes on the mic:** Windows applies volume and mute itself, so the Sound panel slider really changes the level.
- **Compiled out:** Bluetooth/USB/A2DP sideband code (those defines are not set). The keyword detector and SysVAD's private property set are still in the source but unreachable. No BitGG filter exposes those pins or properties.
- **INF:** `bitggvad.inx` is written from scratch, with no APO/effects registration.
- **Platform:** x64 only for now.

The driver has no IOCTLs, no symbolic link and no custom properties. The only inputs are standard audio requests from Windows.

## Build

Needs Visual Studio 2022/2026 with the **Windows Driver Kit** component (`Component.Microsoft.Windows.DriverKit`) and `nuget.exe`. The WDK itself comes from NuGet, the same way Microsoft's samples build it:

```powershell
cd driver
nuget restore packages.config -PackagesDirectory packages
msbuild bitggvad\bitggvad.vcxproj -p:Configuration=Release -p:Platform=x64
```

Output goes to `bitggvad\x64\Release\bitggvad\`: `bitggvad.sys`, `bitggvad.inf`, `bitggvad.cat`. The build signs with an auto-generated **test** certificate (`bitggvad.cer`), which only loads on a machine with test signing on.

If the repo lives in a very deep folder and the build fails with path errors, point the WDK and the output somewhere short:

```powershell
msbuild bitggvad\bitggvad.vcxproj -p:Configuration=Release -p:Platform=x64 -p:BitggPackages=C:\wdk\packages\ -p:BitggOutRoot=C:\bitggvad-out\
```

## License

Files carrying Microsoft's copyright header are SysVAD or derived from it (including `minipairs.h`, `bitggwavtable.h` and `bitggtoptable.h`). They are under the **Microsoft Public License** ([`bitggvad/LICENSE.SysVAD`](bitggvad/LICENSE.SysVAD)). Files written from scratch for BitGG (`bitggloop.*`, `bitggvad.inx`, `bitggvad.rc`) are MIT, like the rest of BitGG. When distributing source, keep `LICENSE.SysVAD` with it.
