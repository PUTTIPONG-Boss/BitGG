//! Windows Core Audio helpers: endpoints, per-app sessions, per-app routing.
//!
//! Per-app routing uses the same undocumented WinRT factory Sonar's
//! `AudioPolicyConfigProxyFor21H2Win11` / `...ForDownlevel` wrap
//! (`Windows.Media.Internal.AudioPolicyConfig`, `SetPersistedDefaultAudioEndpoint`).
//! Default-device switching uses the undocumented `IPolicyConfig` COM class
//! (Sonar: `PolicyConfigClientCOM`).

#![allow(non_snake_case)]

use std::ffi::c_void;
use windows::core::{interface, Interface, GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, PCWSTR, PROPVARIANT, PWSTR};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, BOOL};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::WinRT::RoGetActivationFactory;

pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

#[derive(Clone, Debug)]
pub struct Dev {
    pub id: String,
    pub name: String,
    pub render: bool,
}

pub fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

unsafe fn pwstr_take(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const c_void));
    s
}

pub fn list_devices() -> Vec<Dev> {
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = enumerator() else { return out };
        let Ok(col) = en.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE) else { return out };
        let n = col.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(d) = col.Item(i) else { continue };
            let id = d.GetId().map(|p| pwstr_take(p)).unwrap_or_default();
            let name = d
                .OpenPropertyStore(STGM_READ)
                .and_then(|ps| ps.GetValue(&PKEY_Device_FriendlyName))
                .map(|v| v.to_string())
                .unwrap_or_else(|_| id.clone());
            let render = d
                .cast::<IMMEndpoint>()
                .and_then(|e| e.GetDataFlow())
                .map(|f| f == eRender)
                .unwrap_or(false);
            out.push(Dev { id, name, render });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn device(id: &str) -> windows::core::Result<IMMDevice> {
    unsafe { enumerator()?.GetDevice(&HSTRING::from(id)) }
}

pub fn default_id(render: bool) -> Option<String> {
    unsafe {
        let d = enumerator()
            .ok()?
            .GetDefaultAudioEndpoint(if render { eRender } else { eCapture }, eConsole)
            .ok()?;
        d.GetId().ok().map(|p| pwstr_take(p))
    }
}

pub fn endpoint_volume(id: &str) -> Option<IAudioEndpointVolume> {
    unsafe { device(id).ok()?.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok() }
}

pub fn endpoint_meter(id: &str) -> Option<IAudioMeterInformation> {
    unsafe { device(id).ok()?.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok() }
}

/// Set a whole device's volume (0..1 scalar) / mute, e.g. the microphone.
pub fn set_endpoint(id: &str, vol: Option<f32>, mute: Option<bool>) {
    if let Some(ev) = endpoint_volume(id) {
        unsafe {
            if let Some(v) = vol {
                let _ = ev.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null());
            }
            if let Some(m) = mute {
                let _ = ev.SetMute(BOOL::from(m), std::ptr::null());
            }
        }
    }
}

/// Virtual cables / VADs are the only sensible channel sources: loopback of a
/// real speaker would just duplicate audio.
pub fn is_virtual(name: &str) -> bool {
    let n = name.to_lowercase();
    ["sonar", "virtual", "cable", "vb-audio", "voicemeeter", "steam streaming"].iter().any(|k| n.contains(k))
}

// ---------------------------------------------------------------- sessions

#[derive(Clone, Debug)]
pub struct Session {
    /// First pid; `pids` holds every process of the same app (e.g. Discord helpers).
    pub pid: u32,
    pub pids: Vec<u32>,
    pub name: String,
    pub vol: f32,
    pub mute: bool,
    pub peak: f32,
    /// Endpoint the session currently plays on.
    pub device: String,
    /// Every endpoint this app has a live (non-expired) session on.
    pub devices: Vec<String>,
    /// Live peak meters (one per audio session of this app) for smooth UI meters.
    pub meters: Vec<IAudioMeterInformation>,
}

impl Session {
    pub fn live_peak(&self) -> f32 {
        self.meters.iter().map(|m| unsafe { m.GetPeakValue().unwrap_or(0.0) }).fold(0.0, f32::max)
    }
}

/// pid -> exe name for every running process, from one Toolhelp snapshot.
///
/// Deliberately does NOT OpenProcess() each pid: the audio sessions include the
/// game, and anti-cheats (EAC, BattlEye, Vanguard...) watch for handles opened
/// to the game process. A snapshot is a kernel list read; it touches no process.
pub fn process_names() -> std::collections::HashMap<u32, String> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut m = std::collections::HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return m };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let file = String::from_utf16_lossy(&e.szExeFile[..len]);
            let name = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(&file).to_string();
            m.insert(e.th32ProcessID, name);
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    m
}

/// All active (non-expired) app sessions on every render endpoint.
pub fn list_sessions(devs: &[Dev]) -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    let names = process_names();
    for d in devs.iter().filter(|d| d.render) {
        unsafe {
            let Ok(dev) = device(&d.id) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(en) = mgr.GetSessionEnumerator() else { continue };
            let n = en.GetCount().unwrap_or(0);
            for i in 0..n {
                let Ok(ctl) = en.GetSession(i) else { continue };
                let Ok(c2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if c2.IsSystemSoundsSession() == HRESULT(0) {
                    continue;
                }
                if matches!(ctl.GetState(), Ok(s) if s == AudioSessionStateExpired) {
                    continue;
                }
                let pid = c2.GetProcessId().unwrap_or(0);
                if pid == 0 || pid == std::process::id() {
                    continue;
                }
                let name = names.get(&pid).cloned().unwrap_or_else(|| format!("PID {pid}"));
                // audiodg = Sonar's own APO stream; GG helpers are noise here
                if name.eq_ignore_ascii_case("audiodg") || name.eq_ignore_ascii_case("bitgg") || name.starts_with("SteelSeries") {
                    continue;
                }
                let (vol, mute) = match ctl.cast::<ISimpleAudioVolume>() {
                    Ok(v) => (v.GetMasterVolume().unwrap_or(1.0), v.GetMute().map(|b| b.as_bool()).unwrap_or(false)),
                    Err(_) => (1.0, false),
                };
                let meter = ctl.cast::<IAudioMeterInformation>().ok();
                let peak = meter.as_ref().and_then(|m| m.GetPeakValue().ok()).unwrap_or(0.0);
                if let Some(s) = out.iter_mut().find(|s| s.name == name) {
                    s.peak = s.peak.max(peak);
                    if !s.pids.contains(&pid) {
                        s.pids.push(pid);
                    }
                    s.meters.extend(meter);
                    if !s.devices.contains(&d.id) {
                        s.devices.push(d.id.clone());
                    }
                    continue;
                }
                out.push(Session { pid, pids: vec![pid], name, vol, mute, peak, device: d.id.clone(), devices: vec![d.id.clone()], meters: meter.into_iter().collect() });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Apply volume / mute to every session of `pid` on every render device.
pub fn set_session(devs: &[Dev], pid: u32, vol: Option<f32>, mute: Option<bool>) {
    for d in devs.iter().filter(|d| d.render) {
        unsafe {
            let Ok(dev) = device(&d.id) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(en) = mgr.GetSessionEnumerator() else { continue };
            for i in 0..en.GetCount().unwrap_or(0) {
                let Ok(ctl) = en.GetSession(i) else { continue };
                let Ok(c2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if c2.GetProcessId().unwrap_or(0) != pid {
                    continue;
                }
                if let Ok(v) = ctl.cast::<ISimpleAudioVolume>() {
                    if let Some(x) = vol {
                        let _ = v.SetMasterVolume(x.clamp(0.0, 1.0), std::ptr::null());
                    }
                    if let Some(m) = mute {
                        let _ = v.SetMute(BOOL::from(m), std::ptr::null());
                    }
                }
            }
        }
    }
}

// ------------------------------------------------- per-app routing (hidden API)

#[interface("ab3d4648-e242-459f-b02f-541c70306324")]
unsafe trait IAudioPolicyConfigFactory: IUnknown {
    // IInspectable
    fn GetIids(&self) -> HRESULT;
    fn GetRuntimeClassName(&self) -> HRESULT;
    fn GetTrustLevel(&self) -> HRESULT;
    // IAudioPolicyConfigFactory
    fn add_CtxVolumeChange(&self) -> HRESULT;
    fn remove_CtxVolumeChanged(&self) -> HRESULT;
    fn add_RingerVibrateStateChanged(&self) -> HRESULT;
    fn remove_RingerVibrateStateChange(&self) -> HRESULT;
    fn SetVolumeGroupGainForId(&self) -> HRESULT;
    fn GetVolumeGroupGainForId(&self) -> HRESULT;
    fn GetActiveVolumeGroupForEndpointId(&self) -> HRESULT;
    fn GetVolumeGroupsForEndpoint(&self) -> HRESULT;
    fn GetCurrentVolumeContext(&self) -> HRESULT;
    fn SetVolumeGroupMuteForId(&self) -> HRESULT;
    fn GetVolumeGroupMuteForId(&self) -> HRESULT;
    fn SetRingerVibrateState(&self) -> HRESULT;
    fn GetRingerVibrateState(&self) -> HRESULT;
    fn SetPreferredChatApplication(&self) -> HRESULT;
    fn ResetPreferredChatApplication(&self) -> HRESULT;
    fn GetPreferredChatApplication(&self) -> HRESULT;
    fn GetCurrentChatApplications(&self) -> HRESULT;
    fn add_ChatContextChanged(&self) -> HRESULT;
    fn remove_ChatContextChanged(&self) -> HRESULT;
    fn SetPersistedDefaultAudioEndpoint(&self, pid: u32, flow: EDataFlow, role: ERole, device: *mut c_void) -> HRESULT;
    fn GetPersistedDefaultAudioEndpoint(&self, pid: u32, flow: EDataFlow, role: ERole, device: *mut *mut c_void) -> HRESULT;
    fn ClearAllPersistedApplicationDefaultEndpoints(&self) -> HRESULT;
}

/// Same vtable, pre-21H2 IID.
#[interface("2a59116d-6c4f-45e0-a74f-707e3fef9258")]
unsafe trait IAudioPolicyConfigFactoryDownlevel: IUnknown {
    fn GetIids(&self) -> HRESULT;
    fn GetRuntimeClassName(&self) -> HRESULT;
    fn GetTrustLevel(&self) -> HRESULT;
    fn add_CtxVolumeChange(&self) -> HRESULT;
    fn remove_CtxVolumeChanged(&self) -> HRESULT;
    fn add_RingerVibrateStateChanged(&self) -> HRESULT;
    fn remove_RingerVibrateStateChange(&self) -> HRESULT;
    fn SetVolumeGroupGainForId(&self) -> HRESULT;
    fn GetVolumeGroupGainForId(&self) -> HRESULT;
    fn GetActiveVolumeGroupForEndpointId(&self) -> HRESULT;
    fn GetVolumeGroupsForEndpoint(&self) -> HRESULT;
    fn GetCurrentVolumeContext(&self) -> HRESULT;
    fn SetVolumeGroupMuteForId(&self) -> HRESULT;
    fn GetVolumeGroupMuteForId(&self) -> HRESULT;
    fn SetRingerVibrateState(&self) -> HRESULT;
    fn GetRingerVibrateState(&self) -> HRESULT;
    fn SetPreferredChatApplication(&self) -> HRESULT;
    fn ResetPreferredChatApplication(&self) -> HRESULT;
    fn GetPreferredChatApplication(&self) -> HRESULT;
    fn GetCurrentChatApplications(&self) -> HRESULT;
    fn add_ChatContextChanged(&self) -> HRESULT;
    fn remove_ChatContextChanged(&self) -> HRESULT;
    fn SetPersistedDefaultAudioEndpoint(&self, pid: u32, flow: EDataFlow, role: ERole, device: *mut c_void) -> HRESULT;
    fn GetPersistedDefaultAudioEndpoint(&self, pid: u32, flow: EDataFlow, role: ERole, device: *mut *mut c_void) -> HRESULT;
    fn ClearAllPersistedApplicationDefaultEndpoints(&self) -> HRESULT;
}

enum Factory {
    New(IAudioPolicyConfigFactory),
    Old(IAudioPolicyConfigFactoryDownlevel),
}

fn factory() -> Option<Factory> {
    let cls = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
    unsafe {
        if let Ok(f) = RoGetActivationFactory::<IAudioPolicyConfigFactory>(&cls) {
            return Some(Factory::New(f));
        }
        RoGetActivationFactory::<IAudioPolicyConfigFactoryDownlevel>(&cls).ok().map(Factory::Old)
    }
}

const RENDER_IF: &str = "{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const CAPTURE_IF: &str = "{2eef81be-33fa-4800-9670-1cd474972c3f}";

fn mmdev_path(id: &str, render: bool) -> String {
    format!("\\\\?\\SWD#MMDEVAPI#{}#{}", id, if render { RENDER_IF } else { CAPTURE_IF })
}

fn short_id(path: &str) -> String {
    // "\\?\SWD#MMDEVAPI#{0.0.0.00000000}.{guid}#{iface}" -> "{0.0.0.00000000}.{guid}"
    path.split('#').nth(2).unwrap_or("").to_string()
}

/// Route `pid`'s render audio to endpoint `dev_id` ("" resets to Windows default).
pub fn set_app_output(pid: u32, dev_id: &str) -> bool {
    let Some(f) = factory() else { return false };
    let h = if dev_id.is_empty() { HSTRING::new() } else { HSTRING::from(mmdev_path(dev_id, true)) };
    let raw: *mut c_void = unsafe { std::mem::transmute_copy(&h) };
    let mut ok = true;
    for role in [eConsole, eMultimedia] {
        let hr = unsafe {
            match &f {
                Factory::New(f) => f.SetPersistedDefaultAudioEndpoint(pid, eRender, role, raw),
                Factory::Old(f) => f.SetPersistedDefaultAudioEndpoint(pid, eRender, role, raw),
            }
        };
        ok &= hr.is_ok();
    }
    drop(h);
    ok
}

/// Endpoint id the app is pinned to, or "" if it follows the Windows default.
pub fn get_app_output(pid: u32) -> String {
    let Some(f) = factory() else { return String::new() };
    let mut raw: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        match &f {
            Factory::New(f) => f.GetPersistedDefaultAudioEndpoint(pid, eRender, eMultimedia, &mut raw),
            Factory::Old(f) => f.GetPersistedDefaultAudioEndpoint(pid, eRender, eMultimedia, &mut raw),
        }
    };
    if hr.is_err() || raw.is_null() {
        return String::new();
    }
    let h: HSTRING = unsafe { std::mem::transmute(raw) };
    short_id(&h.to_string_lossy())
}

pub fn clear_all_app_outputs() {
    if let Some(f) = factory() {
        unsafe {
            let _ = match &f {
                Factory::New(f) => f.ClearAllPersistedApplicationDefaultEndpoints(),
                Factory::Old(f) => f.ClearAllPersistedApplicationDefaultEndpoints(),
            };
        }
    }
}

// --------------------------------------------- default endpoint (IPolicyConfig)

#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self) -> HRESULT;
    fn GetDeviceFormat(&self) -> HRESULT;
    fn ResetDeviceFormat(&self) -> HRESULT;
    fn SetDeviceFormat(&self) -> HRESULT;
    fn GetProcessingPeriod(&self) -> HRESULT;
    fn SetProcessingPeriod(&self) -> HRESULT;
    fn GetShareMode(&self) -> HRESULT;
    fn SetShareMode(&self) -> HRESULT;
    fn GetPropertyValue(&self, id: PCWSTR, fx_store: BOOL, key: *const PROPERTYKEY, value: *mut PROPVARIANT) -> HRESULT;
    fn SetPropertyValue(&self, id: PCWSTR, fx_store: BOOL, key: *const PROPERTYKEY, value: *const PROPVARIANT) -> HRESULT;
    fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
    /// Sonar: `PolicyConfigClient.SetEndpointVisibility(id, isVisible)` -
    /// this is "Enable/Disable" in the Sound control panel.
    fn SetEndpointVisibility(&self, id: PCWSTR, visible: BOOL) -> HRESULT;
}

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Sonar VAD endpoints of every state: (id, friendly name, DEVICE_STATE_*).
///
/// Observed: when GG/Sonar quits, Sonar leaves its endpoints in
/// DEVICE_STATE_DISABLED (2) - the VAD driver (ROOT\MEDIA\0000) keeps running,
/// so PnP shows the endpoint devices as not present ("Unknown").
pub fn sonar_vads() -> Vec<(String, String, u32)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = enumerator() else { return out };
        let Ok(col) = en.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL)) else { return out };
        for i in 0..col.GetCount().unwrap_or(0) {
            let Ok(d) = col.Item(i) else { continue };
            let name = d
                .OpenPropertyStore(STGM_READ)
                .and_then(|ps| ps.GetValue(&PKEY_Device_FriendlyName))
                .map(|v| v.to_string())
                .unwrap_or_default();
            if !name.contains("SteelSeries Sonar") {
                continue;
            }
            let id = d.GetId().map(|p| pwstr_take(p)).unwrap_or_default();
            let state = d.GetState().map(|s| s.0).unwrap_or(0);
            out.push((id, name, state));
        }
    }
    out
}

/// Enable (or disable) the Sonar virtual endpoints the same way Sonar does
/// (IPolicyConfig::SetEndpointVisibility). Only touches endpoints that are
/// currently ACTIVE/DISABLED; never NOTPRESENT ones. Returns (changed, total).
pub fn set_sonar_vads(visible: bool) -> (usize, usize) {
    let vads = sonar_vads();
    let mut changed = 0;
    unsafe {
        let Ok(pc) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else {
            return (0, vads.len());
        };
        for (id, _, state) in &vads {
            let want = if visible { DEVICE_STATE_DISABLED.0 } else { DEVICE_STATE_ACTIVE.0 };
            if *state != want {
                continue;
            }
            let w = HSTRING::from(id.as_str());
            if pc.SetEndpointVisibility(PCWSTR(w.as_ptr()), BOOL::from(visible)).is_ok() {
                changed += 1;
            }
        }
    }
    if visible {
        // GG drove its channel faders through these device volumes (left at e.g.
        // 37%); BitGG's faders replace that, so open the virtual outputs fully.
        std::thread::sleep(std::time::Duration::from_millis(300));
        for (id, _, _) in sonar_vads() {
            if id.starts_with("{0.0.0.") {
                set_endpoint(&id, Some(1.0), Some(false));
            }
        }
    }
    (changed, vads.len())
}

/// PKEY_AudioEndpoint_Disable_SysFx: the "Disable all enhancements" checkbox.
/// With GG closed, Sonar's APO inside the VADs keeps applying GG's last
/// settings; switching it off makes BitGG the only DSP in the chain.
const PKEY_DISABLE_SYSFX: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e), pid: 5 };

pub fn set_enhancements(id: &str, enabled: bool) -> bool {
    unsafe {
        let Ok(pc) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else {
            return false;
        };
        let w = HSTRING::from(id);
        let v = PROPVARIANT::from(if enabled { 0u32 } else { 1u32 });
        pc.SetPropertyValue(PCWSTR(w.as_ptr()), BOOL(0), &PKEY_DISABLE_SYSFX, &v).is_ok()
    }
}

pub fn enhancements_enabled(id: &str) -> Option<bool> {
    unsafe {
        let ps = device(id).ok()?.OpenPropertyStore(STGM_READ).ok()?;
        let v = ps.GetValue(&PKEY_DISABLE_SYSFX).ok()?;
        Some(u32::try_from(&v).unwrap_or(0) == 0)
    }
}

pub fn set_default_device(id: &str) -> bool {
    unsafe {
        let Ok(pc) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else {
            return false;
        };
        let w = HSTRING::from(id);
        let mut ok = true;
        for role in [eConsole, eMultimedia, eCommunications] {
            ok &= pc.SetDefaultEndpoint(PCWSTR(w.as_ptr()), role).is_ok();
        }
        ok
    }
}
