# BitGG

**A tiny, native Windows replacement for the two parts of SteelSeries GG that people actually use every day: Sonar (audio mixer / per-app routing / EQ / mic) and Moments (replay buffer + screen recording).** 8-bit pixel-art UI, drawn by hand with no GPU, no Electron, no .NET.

🇹🇭 ภาษาไทย: [README.th.md](README.th.md)

| | RAM | CPU (idle) |
|---|---|---|
| BitGG window open | ~26 MB | ~0 % |
| BitGG + 30 s replay buffer @ 1080p60 | ~26 MB + the clip buffer (~60 MB) | <1 % |
| SteelSeries GG (for comparison, same PC) | ~740 MB | — |

The whole app is a single ~1 MB `.exe` with no dependencies.

> **Unofficial / independent project.** Not affiliated with SteelSeries. It reuses the Sonar *virtual audio devices* that GG already installed on your PC; it does not ship or modify any SteelSeries driver.

---

## Install (easy)

BitGG is **portable** — there is nothing to install and it never needs admin rights.

1. Download **[BitGG.exe](https://github.com/PUTTIPONG-Boss/BitGG/releases/latest/download/BitGG.exe)** (always the latest release; all versions are on the [Releases](../../releases) page).
2. Double-click it. The window opens and it works right away.
3. (Optional) Go to the **SETUP** tab and click **ADD SHORTCUTS** for Desktop + Start Menu icons, or turn on **STARTUP** to launch it (in the tray) when you log in.

Settings are saved to `%APPDATA%\BitGG\config.json`. Clips go to `Videos\BitGG` by default (changeable in the CLIPS tab).

### "Windows protected your PC" / antivirus warning

BitGG is open source and clean — but it is a brand-new, **unsigned** native app that captures the screen, so Windows SmartScreen or some antivirus engines may show a first-run warning until the file builds reputation. This is a false positive. You can:

- Click **More info → Run anyway** on the SmartScreen dialog, **or**
- **Build it yourself** from source (below) so you are running exactly the code you can read, **or**
- Verify the download: every release lists its SHA-256, and is built by GitHub Actions with a signed provenance attestation. With the [GitHub CLI](https://cli.github.com): `gh attestation verify BitGG.exe -R PUTTIPONG-Boss/BitGG`.

BitGG does not obfuscate or pack its binary, does not phone home, and the one feature that looks like a keylogger — a global keyboard hook — is **off by default**. See [Is it safe?](#is-it-safe) below.

---

## What it does

### Sound (the Sonar part)
- **5 strips** — Game / Chat / Media / Aux / Mic — plus a Master, each with a fader, mute, and live meters.
- **Per-app routing (APPS tab):** send each program to a channel (e.g. game → GAME, Discord → CHAT) using the same hidden Windows API Sonar uses.
- **10-band parametric EQ (EQ tab):** drag points on the response graph, scroll to change Q, presets (Flat / Bass / FPS / Voice / Music). Works on each channel and the mic.
- **ChatMix:** one slider to balance game vs. chat.
- **Mic:** noise gate + EQ.
- **Two modes, automatic:**
  - **DSP** (green dot) — when a channel points at a *virtual audio device* (Sonar's VADs or e.g. VB-Cable), audio is routed through it and fully processed (EQ, gate, mix).
  - **DIRECT** (purple dot) — when there is no virtual device, the fader simply scales the real volume of the apps you assigned. No EQ, but it always works with zero setup.
- **Use Sonar's virtual devices without running GG:** the SETUP button **ENABLE SONAR VADS** turns Sonar's (disabled) virtual devices back on with no admin — so you get full EQ/routing without GG's 700 MB running.

### Clips (the Moments part)
- **Replay buffer:** always encoding the last *N* seconds (15–300 s) to a RAM ring using your GPU (NVENC / Quick Sync / AMF) via Windows Media Foundation — **no FFmpeg needed**.
- **Save the last N seconds instantly:** press the hotkey (default auto-picked, e.g. **Ctrl+Alt+S**) *while in-game* — the window can be in the tray. Saving is a stream copy (no re-encode), ~0.1 s, with a confirmation sound.
- **Record mode:** a normal start/stop recording to MP4.
- **Pick what to record:** output device + volume, and microphone + volume, each with a live level bar and a soft limiter.
- Change the save folder, the key, resolution, fps, bitrate, encoder, and monitor in the CLIPS tab.

### Lives in the tray
Minimize or press **X** → BitGG tucks into the system tray and keeps running (engine + replay stay on). Left-click the tray icon to reopen, right-click for a menu (Open / Save clip / Replay on-off / Exit). Launching it again just reopens the existing window.

---

## Build from source

Requires the [Rust toolchain](https://rustup.rs) (stable). Either MSVC or GNU works.

```sh
git clone https://github.com/PUTTIPONG-Boss/BitGG.git
cd BitGG
cargo build --release
```

The binary is at `target/release/bitgg.exe`.

- The app icon is embedded via `build.rs` using **windres** (part of the MinGW toolchain). If windres isn't present (e.g. a pure MSVC setup), the build still succeeds — just without the embedded icon.
- If your checkout path is very long (Windows `MAX_PATH`), point the build output somewhere short: create `.cargo/config.toml` with `[build]` and `target-dir = "C:/bitgg-target"`.

---

## Branches & releases (maintainers)

| Branch | Purpose |
|---|---|
| `main` | **Releases only.** Every `v*` tag is cut here; the release workflow refuses tags that aren't on `main`. |
| `master` | Development. Start every piece of work from here. |
| `<what-you-do>` | One branch per task, named after it (e.g. `code-signing`, `fix-mic-gain`). |

```sh
git switch master && git pull
git switch -c fix-mic-gain        # work, commit
git push -u origin fix-mic-gain   # open a PR: fix-mic-gain -> master
```

When `master` is ready to ship, open a PR **`master` → `main`**, merge it, then tag on `main`:

```sh
git switch main && git pull
git tag v0.2.0
git push origin v0.2.0
```

GitHub Actions then builds, tests (including the anti-cheat guard), signs (if configured), and publishes the release with `BitGG.exe`, `SHA256SUMS.txt` and a provenance attestation. CI also builds and tests every push and PR.

### Code signing (optional)

Signing removes the "unknown publisher" label and lets SmartScreen reputation build up on your certificate instead of on each new file. Add two **repository secrets** (Settings → Secrets and variables → Actions):

| Secret | Value |
|---|---|
| `WINDOWS_CERT_PFX_BASE64` | your code-signing certificate (`.pfx`), base64-encoded |
| `WINDOWS_CERT_PASSWORD` | the `.pfx` password |

Create the base64 value in PowerShell:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("C:\path\to\cert.pfx")) | Set-Clipboard
```

Optional repository **variable** `TIMESTAMP_URL` (default `http://timestamp.digicert.com`). Without the secrets, releases are published unsigned, as before. The signing step ([`.github/scripts/sign.ps1`](.github/scripts/sign.ps1)) fails the release rather than shipping a bad signature, timestamps the signature so it stays valid after the certificate expires, and never leaves the key on disk.

Where to get a certificate: a code-signing certificate from a public CA (OV/EV, Sectigo, DigiCert, SSL.com and others; EV/OV keys now usually live on a hardware token or cloud HSM, which needs a different signing step), or Microsoft's **Trusted Signing** service, a cheap monthly option for individuals and small teams that also needs its own signing step instead of a `.pfx`. A self-signed certificate does **not** help with SmartScreen.

> Note: since 2024, even signed apps can still see SmartScreen warnings until the certificate has built reputation from downloads. Signing is what makes that reputation possible.

---

## Is it safe?

Yes, and it's designed to be auditable:

- **Open source, no obfuscation, no packer.** What you read is what runs.
- **No network.** BitGG makes no outbound connections.
- **No admin required.** Everything runs as your normal user.
- **Portable.** It doesn't copy itself around or edit the registry. "ADD SHORTCUTS"/"STARTUP" only create `.lnk` files that point at the exe you ran, and only when you choose.
- **The keyboard hook is opt-in.** A global low-level keyboard hook is the one thing that both antivirus heuristics and users are right to be wary of. BitGG registers normal global hotkeys instead, and if your chosen key is taken it automatically moves to a free one. The hook (`SETUP → KEY HOOK`) is **off by default** and only there for the rare case where every candidate key is occupied.
- **Why it still might get flagged:** an unsigned executable that uses the Desktop Duplication / Media Foundation capture APIs looks, to a dumb heuristic, a bit like screen-capture malware. The real fixes are a code-signing certificate and download reputation, both of which take time/money for a hobby project. Building from source sidesteps it entirely.

If your antivirus quarantines it and you want to help, submit it as a false positive to your vendor — that improves detection for everyone.

---

## Anti-cheat & game safety

BitGG is built so it **never touches a game process**, the same "outside the game" approach as Windows' own volume mixer and Game Bar, and *not* the way GG Moments / most overlays work (GG ships `SteelSeriesGameOverlay.dll` and injects it into games).

| What BitGG does | How | Touches the game? |
|---|---|---|
| Per-app volume, mute, routing | Windows audio service (`ISimpleAudioVolume`, `AudioPolicyConfig`) | ❌ No |
| App names in the APPS list | One system process snapshot (Toolhelp) | ❌ No (no handle is ever opened to the game) |
| Screen capture | DXGI Desktop Duplication (captures the finished desktop image) | ❌ No |
| Game audio capture | WASAPI loopback of the output device | ❌ No |
| Save hotkey | `RegisterHotKey` (OS-level) | ❌ No |
| Optional keyboard hook (**off** by default) | `WH_KEYBOARD_LL`: runs inside BitGG, not injected | ❌ No |

No DLL injection, no memory reading/writing, no remote threads, no Present/swap-chain hooks, no in-game overlay. A test in [`tests/no_game_injection.rs`](tests/no_game_injection.rs) scans the source on every build and fails if any of those APIs ever appear.

**Honest limits:**
- BitGG doesn't modify games, but **each game's EULA is the game's own**. Kernel anti-cheats (Vanguard, EAC, BattlEye...) don't flag the APIs above, the same ones OBS, Discord and Game Bar use, but no third-party tool can *guarantee* a specific anti-cheat's future behaviour.
- If an anti-cheat blocks screen capture of its game (rare), the clip will show a black frame. Nothing gets flagged; the capture just doesn't get the image.
- The optional **KEY HOOK** is a standard low-level keyboard hook (the same mechanism as Discord push-to-talk). It is off by default; leave it off unless every save hotkey is taken.
- BitGG has no GameSense / RGB features. It never talks to SteelSeries Engine.

## License

[MIT](LICENSE). SteelSeries, Sonar, GG and Moments are trademarks of their respective owners; BitGG is an independent interoperability project.
