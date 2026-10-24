# BitGG Virtual Audio driver: specification (Phase 1)

> **สรุปภาษาไทย:** เอกสารนี้คือสเปกของ driver เสียงเสมือนของ BitGG เอง เพื่อให้คนที่ไม่มี SteelSeries ใช้ EQ และแยกช่องเสียงได้ด้วย เราสร้างจากตัวอย่าง **SysVAD ของ Microsoft** แบบ clean-room (ดูพฤติกรรมของ Sonar จากภายนอกเท่านั้น ไม่ใช้โค้ดของ SteelSeries) driver ทำหน้าที่แค่ "สร้างอุปกรณ์เสียงเสมือน" ส่วนการแต่งเสียงทั้งหมด (EQ, mix, gate) ยังทำในแอป BitGG เหมือนเดิม driver ไม่มีช่องทางคุยกับแอปเลย เพื่อให้ปลอดภัยที่สุด

Status: draft · Owner: BitGG · Target: Windows 10 21H2+ / Windows 11, x64 (ARM64 later)

---

## 1. Goal

Ship BitGG's own virtual audio endpoints so BitGG's DSP mode (routing, per-channel EQ, ChatMix, mic gate) works on any PC, not only PCs where SteelSeries GG installed its Sonar driver.

### Non-goals

- **No DSP in the driver.** No APO, no EQ, no effects. All processing stays in the BitGG app (user mode), where a bug can't crash the PC.
- **No private user-mode interface.** No IOCTLs, no custom properties, no device interface for BitGG to talk to. BitGG uses only standard Windows audio APIs (WASAPI, MMDevice, IPolicyConfig). Less attack surface, nothing for anti-cheat to inspect.
- Not a replacement for SteelSeries hardware features (GameSense, headset firmware, ChatMix dial).

## 2. Clean-room rules

| Allowed sources | Not allowed |
|---|---|
| Microsoft's **SysVAD** sample (Windows-driver-samples, MS-PL) | Decompiled or disassembled code of any SteelSeries binary |
| Microsoft Learn: PortCls, WaveRT, INF, audio endpoint docs | Copying SteelSeries INF text, strings, GUIDs or resource data |
| **Externally observable behaviour** of the Sonar endpoints through public Windows APIs (what any app can see) | Matching SteelSeries device names, hardware IDs or branding |

The Phase-0 REA pass was limited to identifying the *architecture family* (it is a PortCls/WaveRT driver structured like SysVAD). All design decisions below come from Microsoft's sample and docs plus the observations in §3.

## 3. Reference behaviour we match (observed via public APIs)

Measured on the developer's PC with BitGG's own tools (`bitgg --vads`, `--vadtest`, `--apotest`, `--staletest`, `--nantest`):

| Endpoint (Sonar) | Flow | Mix format | Role in BitGG |
|---|---|---|---|
| Gaming | render | 8 ch, 96 kHz | GAME channel source |
| Chat | render | 2 ch, 48 kHz | CHAT channel source |
| Media | render | 8 ch, 96 kHz | MEDIA channel source |
| Aux | render | 8 ch, 96 kHz | AUX channel source |
| Microphone | render + capture pair | 2 ch, 48 kHz | processed mic: BitGG writes to render, apps read capture |
| Stream | render + capture pair | 2 ch, 48 kHz | (unused by BitGG today) |

Behaviour that matters:

1. **Render endpoints are sinks.** Audio written to them goes nowhere by itself. BitGG reads it with WASAPI **loopback**, which the Windows audio engine provides for every render endpoint. The driver does no work for this.
2. **The mic pair is a loopback:** samples rendered to the mic's render side come out of its capture side, which apps such as Discord select as "the microphone". Measured: a 0.5 tone in → 0.315 out (−4 dB). The capture endpoint's Windows volume was 76.8 % at the time, which likely explains it (not verified at 100 %). Our test plan checks for unity gain at 100 %.
3. **Endpoints can be disabled/enabled** with `IPolicyConfig::SetEndpointVisibility` (the Sound panel's Disable/Enable). No driver support is needed.
4. **Sonar ships an APO** (effects module) on these endpoints that forwards audio to the headset by itself, which doubled the audio until BitGG bypassed it. **Our driver ships no APO**, so this class of problem disappears.
5. 8-channel endpoints made Windows' automatic 7.1→2.0 downmix attenuate stereo content. BitGG already folds channels itself. **We default to stereo** and expose 7.1 only where it's useful (§4.2).

## 4. Design

### 4.1 Device

| Item | Value |
|---|---|
| Driver | `bitggvad.sys`, PortCls miniport driver derived from SysVAD |
| Enumeration | Root-enumerated software device, hardware ID `ROOT\BitGG_VirtualAudio` |
| INF | Our own `bitggvad.inf`, class `MEDIA`, `PnpLockdown=1`, no co-installers, no APO registration |
| Device name | "BitGG Virtual Audio" |
| Install | `pnputil /add-driver bitggvad.inf /install`, then create the root device (`devcon install`-equivalent via SetupAPI in a small installer). Needs admin **once**. |

### 4.2 Endpoints

Each endpoint is a SysVAD-style **wave + topology miniport pair** (WaveRT). Names are what users see in Sound settings.

| Endpoint | Flow | Formats offered | Default | Notes |
|---|---|---|---|---|
| **BitGG Game** | render | 48 kHz 16-bit, stereo **and** 7.1 | stereo | 7.1 for games with surround mixing; BitGG folds it |
| **BitGG Chat** | render | 48 kHz 16-bit stereo | stereo | |
| **BitGG Media** | render | 48 kHz 16-bit stereo | stereo | |
| **BitGG Aux** | render | 48 kHz 16-bit stereo | stereo | |
| **BitGG Mic (in)** | render | 48 kHz 16-bit stereo | stereo | BitGG writes the processed mic here |
| **BitGG Mic** | capture | 48 kHz 16-bit stereo | stereo | what Discord etc. select; reads from "BitGG Mic (in)" |

Phase 1 implements 16-bit PCM only, the path SysVAD itself uses and tests. The Windows audio engine mixes in float and converts, so shared-mode apps are unaffected. Exclusive-mode apps must use 48 kHz 16-bit. 24-bit/float device formats are a later improvement. Mic (in) and Mic share one format, so the loopback copies bytes with no conversion.

Why 48 kHz everywhere: BitGG's engine runs at 48 kHz, so no resampling happens in Windows or BitGG. 44.1 kHz can be added later if a real app needs it (Windows' mixer converts anyway).

Endpoint properties set by the INF:

- `PKEY_AudioEndpoint_FormFactor`: Speakers for render, Microphone for capture.
- `PKEY_AudioEndpoint_Association`: `KSNODETYPE_ANY`.
- **No** `PKEY_AudioEndpoint_Disable_SysFx`-style tricks and **no** effect GUIDs: no APO at all.
- Event-driven mode supported (`PKEY_AudioEndpoint_Supports_EventDriven_Mode = 1`).

### 4.3 Streaming

- **Render pins (Game/Chat/Media/Aux):** WaveRT buffer allocated by the miniport. A high-resolution timer (`ExAllocateTimer`, 1–10 ms period) advances the play position at the format's byte rate and signals notification events. Data is **not read**: these endpoints are sinks (§3.1). This is SysVAD's simulated-hardware path with the sine/file stages removed.
- **Mic loopback (Mic (in) → Mic), `bitggloop.cpp`:**
  - One static 64 KB ring (~340 ms at 48 kHz 16-bit stereo) behind a spin lock.
  - When the render stream's position advances, the newly "played" bytes are appended to the ring.
  - Each capture stream keeps its own read position, so Discord and OBS can both record the mic.
  - A capture stream starts reading at "now", so it never gets stale audio. If it underruns, it writes silence. If it falls more than 100 ms behind, it skips ahead to 20 ms behind.
  - Never block, never allocate. Same format on both sides, so no conversion.
- Clock: all streams run off the system timer (no hardware), the same as SysVAD's virtual mode.

### 4.4 What BitGG changes (app side)

- Auto-detect order: **BitGG Virtual Audio** → SteelSeries Sonar → VB-Cable / Voicemeeter → DIRECT mode.
- With BitGG's own driver there's no APO to bypass, so `apo_bypass()` becomes a no-op for these endpoints.
- SETUP gains "INSTALL BITGG AUDIO DRIVER" (runs the signed installer elevated, once) and "REMOVE".
- Everything else (engine, default-device routing, guard on exit) is unchanged; it already works with any virtual render endpoint.

## 5. Safety requirements

- No user-mode interface (no IOCTL, no symbolic link). The only inputs are standard KS/PortCls requests from the Windows audio stack.
- Static analysis: CodeQL for drivers (Microsoft's ruleset) + `/analyze` clean.
- **Driver Verifier** (standard flags + DDI compliance) clean during the whole test plan.
- No paged code at DISPATCH_LEVEL; all DPC buffers pre-allocated at stream start.
- Unload must be clean (`DriverUnload`, `pnputil /remove-device`) with no leaked pool (verified with Verifier pool tracking).
- Anti-cheat stance: a signed PortCls audio driver of the same kind as Sonar, VB-Cable, Voicemeeter and Nahimic. No game interaction, no hooks, no IOCTLs.

## 6. Build & test plan

**Build (Phase 1, done):** Visual Studio with the WDK component, plus WDK/SDK 10.0.28000 from NuGet, as in Microsoft's samples. The code is in `driver/bitggvad/`, starting from SysVAD's TabletAudioSample and reduced to the endpoints above. The sample's license notice is kept. Build steps and the list of changes are in [driver/README.md](../../driver/README.md). The INF passes `infverif /w`.

**Test (Phase 2), in a VM, not on a gaming PC.** Test-signing needs Secure Boot off, and kernel bugs crash the machine.

1. VirtualBox (Windows 11 Home has no Hyper-V) + Windows 11 evaluation VM, Secure Boot off, `bcdedit /set testsigning on`.
2. Install with a test certificate. All 6 endpoints appear in Sound settings with the right names, formats and form factors.
3. Each render endpoint plays at 48 kHz stereo (and 7.1 for Game); WASAPI loopback reads back what was played.
4. Mic loopback: tone into "BitGG Mic (in)" → read from "BitGG Mic" at the same level (±0.1 dB at 100 % volume); silence when nothing renders; no stale audio after stop/start.
5. 30-minute soak with Driver Verifier on; sleep/resume; disable/enable endpoints; uninstall/reinstall.
6. BitGG end to end inside the VM: DSP mode, EQ, ChatMix, mic gate, clips.

**Release (Phase 3):** Microsoft **attestation signing** through the Windows Hardware Dev Center. It needs an **EV code-signing certificate**, which is issued to registered organisations only. Until then, the driver is for test machines only and BitGG keeps working with Sonar / VB-Cable / DIRECT mode.

## 7. Open questions

1. Is 48 kHz-only acceptable, or do any target apps insist on 44.1 kHz exclusive mode? (Shared mode is always fine.)
2. Do we want the Stream pair (a separate mix for streaming/recording), or keep the driver minimal?
3. Who owns signing: a registered business for the EV certificate, or community builds that users test-sign themselves?
4. ARM64 build: wait for demand.
