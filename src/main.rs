#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! BitGG - an 8-bit, ultra-light local stand-in for SteelSeries GG's
//! Sonar (mixer / per-app routing / EQ / ChatMix / mic gate) and
//! Moments (replay buffer + recording), reconstructed from the shipped GG 103.

mod config;
mod dsp;
mod dxgi;
mod engine;
mod mf;
mod recorder;
mod tgui;
mod tray;
mod update;
mod picker;
mod shortcuts;
mod hotkey;
mod wasapi;
mod win_audio;

use config::{Config, CH_DETECT, CH_NAMES, MIC_DETECT};
use engine::{Engine, Shared};
use recorder::{Mode, Recorder};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tgui::{id, pal::*, r, Ui, H, R, W};
use win_audio::{Dev, Session};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{StartCause, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
use winit::window::{Window, WindowId};

/// "Clip saved" sound you can hear inside a game. Uses Moments' own sound
/// when GG is installed, otherwise the Windows default beep.
pub fn beep() {
    let wav = r"C:\Program Files\SteelSeries\GGpps\moments\sounds\clip-taken.wav";
    unsafe {
        if std::path::Path::new(wav).is_file() {
            use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
            if PlaySoundW(&windows::core::HSTRING::from(wav), None, SND_FILENAME | SND_ASYNC | SND_NODEFAULT).as_bool() {
                return;
            }
        }
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(windows::Win32::UI::WindowsAndMessaging::MB_OK);
    }
}

/// Optional diagnostics: set BITGG_LOG=1 to append to %TEMP%\bitgg.log.
pub fn dlog(s: &str) {
    if std::env::var_os("BITGG_LOG").is_some() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("bitgg.log")) {
            let _ = writeln!(f, "{} {s}", recorder::timestamp());
        }
    }
}

/// Error sound (e.g. save pressed while the replay buffer is off).
pub fn beep_err() {
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(windows::Win32::UI::WindowsAndMessaging::MB_ICONHAND);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum UserEv {
    Hotkey(i32),
    Tray(u32),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Mixer,
    Apps,
    Eq,
    Clips,
    Setup,
}
const TABS: [(Tab, &str); 5] = [
    (Tab::Mixer, "MIXER"),
    (Tab::Apps, "APPS"),
    (Tab::Eq, "EQ"),
    (Tab::Clips, "CLIPS"),
    (Tab::Setup, "SETUP"),
];
const DGREEN_DOT: u32 = 0x008751;
const CH_COL: [u32; 6] = [GREEN, BLUE, PINK, ORANGE, YELLOW, WHITE];

const REPLAY_LENS: [u32; 6] = [15, 30, 60, 120, 180, 300];
const FPS_OPTS: [u32; 3] = [30, 60, 120];
const RES_OPTS: [u32; 4] = [720, 1080, 1440, 0];
const BR_OPTS: [u32; 6] = [6_000, 10_000, 15_000, 20_000, 30_000, 50_000];
const ENC_OPTS: [&str; 4] = ["h264_nvenc", "h264_qsv", "h264_amf", "software"];

fn cycle<T: PartialEq + Copy>(opts: &[T], cur: T, d: i32) -> T {
    let i = opts.iter().position(|x| *x == cur).unwrap_or(0) as i32;
    opts[((i + d).rem_euclid(opts.len() as i32)) as usize]
}

struct App {
    cfg: Config,
    ui: Ui,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    map: (usize, usize, usize),
    tab: Tab,
    devs: Vec<Dev>,
    sessions: Vec<Session>,
    routes: HashMap<u32, String>,
    last_refresh: Instant,
    shared: Arc<Shared>,
    engine: Option<Engine>,
    rec: Option<Recorder>,
    rec_msg: Arc<Mutex<String>>,
    monitors: Vec<dxgi::Monitor>,
    eq_ch: usize,
    eq_band: usize,
    apps_scroll: usize,
    dirty_since: Option<Instant>,
    visible: bool,
    sonar_running: bool,
    clips: Vec<std::path::PathBuf>,
    slow_refresh: Instant,
    last_frame: Vec<u32>,
    force_present: bool,
    start_hidden: bool,
    /// last volume BitGG wrote per pid (DIRECT channels), to avoid churn
    applied: HashMap<u32, f32>,
    mic_applied: Option<(f32, bool)>,
    vads: Vec<(String, String, u32)>,
    mic_meter: Option<windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation>,
    /// result slot of the folder picker thread
    picked_folder: Arc<Mutex<Option<String>>>,
    rec_levels: Arc<recorder::RecLevels>,
    /// Sonar APO bypass: original "enhancements enabled" per VAD id we changed
    fx_saved: HashMap<String, bool>,
    update: Arc<Mutex<update::State>>,
    last_update_check: Option<Instant>,
    update_notified: bool,
    update_requested: bool,
}

impl App {
    fn new(cfg: Config) -> App {
        let shared = Shared::new(cfg.audio.clone());
        let mut a = App {
            ui: Ui::new(),
            window: None,
            surface: None,
            map: (1, 0, 0),
            tab: Tab::Mixer,
            devs: Vec::new(),
            sessions: Vec::new(),
            routes: HashMap::new(),
            last_refresh: Instant::now() - Duration::from_secs(10),
            shared,
            engine: None,
            rec: None,
            rec_msg: Arc::new(Mutex::new(String::new())),
            monitors: dxgi::monitors(),
            eq_ch: 0,
            eq_band: 5,
            apps_scroll: 0,
            dirty_since: None,
            visible: true,
            sonar_running: false,
            clips: Vec::new(),
            slow_refresh: Instant::now(),
            last_frame: Vec::new(),
            force_present: true,
            start_hidden: false,
            applied: HashMap::new(),
            mic_applied: None,
            vads: Vec::new(),
            mic_meter: None,
            picked_folder: Arc::new(Mutex::new(None)),
            rec_levels: Arc::new(recorder::RecLevels::default()),
            fx_saved: HashMap::new(),
            update: Arc::new(Mutex::new(update::State::Idle)),
            last_update_check: None,
            update_notified: false,
            update_requested: false,
            cfg,
        };
        // GG leaves its VADs disabled when it quits; bring them back if the user opted in
        if a.cfg.audio.auto_vads && !is_running("SteelSeriesSonar.exe") {
            let (n, _) = win_audio::set_sonar_vads(true);
            if n > 0 {
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        a.refresh();
        if a.cfg.audio.ch.iter().all(|c| c.source.is_empty()) {
            a.autodetect();
        }
        // a real speaker is never a channel source (it only duplicated audio)
        for i in 0..4 {
            let src = a.cfg.audio.ch[i].source.clone();
            if a.devs.iter().any(|d| d.id == src && !win_audio::is_virtual(&d.name)) {
                a.cfg.audio.ch[i].source.clear();
            }
        }
        a.rec_levels.set_gains(config::slider_gain(a.cfg.clips.rec_audio_gain), config::slider_gain(a.cfg.clips.rec_mic_gain));
        a.dirty(); // pushes params; starts the DSP engine only if a channel needs it
        a.guard_defaults(false);
        if a.cfg.clips.replay_on {
            a.start_replay();
        }
        a
    }

    fn dirty(&mut self) {
        self.shared.update(&self.engine_params());
        self.sync_engine(); // also switches the Windows defaults before opening streams
        self.apply_direct(self.cfg.audio.engine_on, false);
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    // ---------------------------------------------------- channel modes
    // A channel is "DSP" when its source is a present virtual device (Sonar VAD,
    // VB-Cable...): apps are routed into it and the engine processes it (EQ).
    // Otherwise it is "DIRECT": a group of apps whose own volumes get scaled.

    fn dev_live(&self, id: &str) -> bool {
        !id.is_empty() && self.devs.iter().any(|d| d.id == id && win_audio::is_virtual(&d.name))
    }
    fn ch_live(&self, i: usize) -> bool {
        self.dev_live(&self.cfg.audio.ch[i].source)
    }
    fn mic_live(&self) -> bool {
        self.dev_live(&self.cfg.audio.mic.target) && !self.cfg.audio.mic.input.is_empty()
    }
    fn needs_engine(&self) -> bool {
        self.cfg.audio.engine_on && !self.cfg.audio.output.is_empty() && ((0..4).any(|i| self.ch_live(i)) || self.mic_live())
    }
    fn engine_params(&self) -> config::Audio {
        let mut a = self.cfg.audio.clone();
        for i in 0..4 {
            if !self.ch_live(i) {
                a.ch[i].source.clear();
            }
        }
        if !self.mic_live() {
            a.mic.target.clear();
        }
        a
    }
    fn sync_engine(&mut self) {
        let want = self.needs_engine();
        if want {
            // Order matters: changing a device's enhancements makes Windows
            // rebuild its audio pipeline, which silently kills streams that are
            // already open. Bypass first, let it settle, then open the engine.
            let bypassed = self.apo_bypass(true);
            // switch Windows' defaults BEFORE opening streams (a later switch
            // can leave already-open captures delivering silence)
            let rerouted = self.route_defaults();
            if (bypassed || rerouted) && self.engine.is_some() {
                if let Some(mut e) = self.engine.take() {
                    e.stop(); // reopen on the new pipeline below
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            if self.engine.is_none() {
                self.engine = Some(Engine::start(self.shared.clone()));
            }
        } else {
            if let Some(mut e) = self.engine.take() {
                e.stop();
            }
            self.apo_bypass(false);
        }
    }

    /// Sonar's APO inside the VADs keeps GG's last settings, including its own
    /// "copy to headset" forwarding: with BitGG's engine also forwarding, every
    /// sound plays twice. While the engine runs, switch the VADs' enhancements
    /// off (the Sound panel's "Disable all enhancements") so BitGG is the only
    /// path; put each device's original setting back when the engine stops.
    /// Returns true if any device setting was changed (pipelines restarted).
    fn apo_bypass(&mut self, on: bool) -> bool {
        let mut changed = false;
        if on {
            for i in 0..4 {
                if !self.ch_live(i) {
                    continue;
                }
                let id = self.cfg.audio.ch[i].source.clone();
                if self.fx_saved.contains_key(&id) {
                    continue;
                }
                if let Some(enabled) = win_audio::enhancements_enabled(&id) {
                    if enabled && win_audio::set_enhancements(&id, false) {
                        self.fx_saved.insert(id, true);
                        changed = true;
                    }
                }
            }
            if changed {
                // Windows rebuilds the device pipeline asynchronously; streams
                // opened too early can go silently stale (the engine's watchdog
                // also catches that, but don't rely on it)
                std::thread::sleep(Duration::from_millis(1500));
            }
        } else {
            for (id, orig) in std::mem::take(&mut self.fx_saved) {
                changed |= win_audio::set_enhancements(&id, orig);
            }
        }
        changed
    }
    fn direct_gain(&self, i: usize) -> f32 {
        let a = &self.cfg.audio;
        if a.ch[i].mute || a.master_mute {
            return 0.0;
        }
        let (g, c) = dsp::chatmix(a.chatmix);
        let cm = match i {
            0 => g,
            1 => c,
            _ => 1.0,
        };
        a.ch[i].vol * cm * a.master
    }
    fn app_ch(&self, name: &str) -> usize {
        self.cfg.audio.apps.get(name).map(|a| a.ch as usize).unwrap_or(0)
    }

    /// Push channel state onto real apps. `on = false` gives every assigned app
    /// its own volume and default output back (engine off / quitting).
    fn apply_direct(&mut self, on: bool, force: bool) {
        let sessions = self.sessions.clone();
        // Unassigned apps belong to GAME (like Sonar). When GAME is DSP they get
        // there through the default device; when GAME is DIRECT we scale them.
        let game_direct = !self.ch_live(0);
        for s in &sessions {
            if on && game_direct && !self.cfg.audio.apps.contains_key(&s.name) {
                // remember the app's own volume before we ever scale it
                self.cfg.audio.apps.insert(s.name.clone(), config::AppCfg { ch: 0, vol: Some(s.vol) });
            }
            let Some(ac) = self.cfg.audio.apps.get(&s.name).cloned() else { continue };
            if ac.ch == 0 {
                // unassigned: scaled as GAME while GAME is DIRECT, else its own volume
                let Some(base) = ac.vol else { continue };
                let scaled = on && game_direct;
                if !scaled && !s.pids.iter().any(|p| self.applied.contains_key(p)) {
                    continue; // never touched by us: leave it alone
                }
                let target = if scaled { base * self.direct_gain(0) } else { base };
                for p in &s.pids {
                    if force || self.applied.get(p).map(|v| (v - target).abs() > 0.002).unwrap_or(true) {
                        win_audio::set_session(&self.devs, *p, Some(target), None);
                    }
                    if scaled {
                        self.applied.insert(*p, target);
                    } else {
                        self.applied.remove(p);
                    }
                }
                continue;
            }
            let c = ac.ch as usize;
            if c > 4 {
                continue;
            }
            let base = match ac.vol {
                Some(v) => v,
                None => {
                    // remember the app's own volume before we ever scale it
                    if let Some(x) = self.cfg.audio.apps.get_mut(&s.name) {
                        x.vol = Some(s.vol);
                    }
                    s.vol
                }
            };
            let live = self.ch_live(c - 1);
            let want_route = if on && live { self.cfg.audio.ch[c - 1].source.clone() } else { String::new() };
            for p in &s.pids {
                let cur = self.routes.get(p).cloned().unwrap_or_default();
                if cur != want_route {
                    win_audio::set_app_output(*p, &want_route);
                    self.routes.insert(*p, want_route.clone());
                }
            }
            let target = if on && !live { base * self.direct_gain(c - 1) } else { base };
            for p in &s.pids {
                if force || self.applied.get(p).map(|v| (v - target).abs() > 0.002).unwrap_or(true) {
                    win_audio::set_session(&self.devs, *p, Some(target), None);
                    self.applied.insert(*p, target);
                }
            }
        }
        // microphone without a virtual mic: control the real mic device
        let m = self.cfg.audio.mic.clone();
        if on && !self.mic_live() && !m.input.is_empty() {
            let want = (m.vol, m.mute);
            if force || self.mic_applied != Some(want) {
                win_audio::set_endpoint(&m.input, Some(want.0), Some(want.1));
                self.mic_applied = Some(want);
            }
        }
    }

    /// While the DSP engine runs: the device an app should be playing on
    /// (its channel's virtual device; unassigned apps -> GAME). None = no rule.
    fn expected_device(&self, s: &win_audio::Session) -> Option<String> {
        self.engine.as_ref()?;
        let c = self.app_ch(&s.name);
        let ch = if c == 0 { 0 } else { c - 1 };
        self.ch_live(ch).then(|| self.cfg.audio.ch[ch].source.clone())
    }

    /// Apps that kept their old audio stream (they only switch device when they
    /// reopen it, e.g. Chromium-based apps and many games): they need a restart.
    fn needs_restart(&self, s: &win_audio::Session) -> bool {
        self.expected_device(s).map(|d| !s.devices.contains(&d)).unwrap_or(false)
    }

    fn direct_peak(&self, ch: usize) -> f32 {
        // unassigned apps count as GAME
        self.sessions
            .iter()
            .filter(|s| {
                let c = self.app_ch(&s.name);
                c == ch + 1 || (ch == 0 && c == 0)
            })
            .map(|s| s.live_peak())
            .fold(0.0, f32::max)
    }

    /// Meter for a mixer strip (0..3 channels, 4 mic, 5 master).
    fn strip_meter(&self, i: usize) -> f32 {
        let eng = self.engine.is_some();
        match i {
            0..=3 if eng && self.ch_live(i) => self.shared.meter(i),
            0..=3 => self.direct_peak(i),
            4 if eng && self.mic_live() => self.shared.meter(4),
            4 => self.mic_meter.as_ref().map(|m| unsafe { m.GetPeakValue().unwrap_or(0.0) }).unwrap_or(0.0),
            _ if eng => self.shared.meter(5),
            _ => {
                let g = if self.cfg.audio.master_mute { 0.0 } else { self.cfg.audio.master };
                (0..4).map(|c| self.direct_peak(c)).fold(0.0, f32::max) * g
            }
        }
    }

    fn refresh(&mut self) {
        self.devs = win_audio::list_devices();
        self.vads = win_audio::sonar_vads();
        self.mic_meter = if self.cfg.audio.mic.input.is_empty() { None } else { win_audio::endpoint_meter(&self.cfg.audio.mic.input) };
        self.refresh_light();
        self.sonar_running = is_running("SteelSeriesSonar.exe");
        self.clips = recent_clips(&self.cfg.clips.folder);
    }

    /// Cheap periodic refresh: only what the visible tab needs.
    fn refresh_light(&mut self) {
        self.sessions = win_audio::list_sessions(&self.devs);
        self.routes = self.sessions.iter().flat_map(|s| s.pids.clone()).map(|p| (p, win_audio::get_app_output(p))).collect();
        self.last_refresh = Instant::now();
        // follow a default-device change made in Windows; reopen the engine's
        // streams afterwards (see sync_engine)
        if self.engine.is_some() && self.route_defaults() {
            if let Some(mut e) = self.engine.take() {
                e.stop();
            }
            std::thread::sleep(Duration::from_millis(300));
            self.sync_engine();
        }
        // newly started processes of an assigned program join their channel
        self.apply_direct(self.cfg.audio.engine_on, false);
    }

    fn autodetect(&mut self) {
        let find = |pat: &str, render: bool| {
            self.devs.iter().find(|d| d.render == render && d.name.contains(pat)).map(|d| d.id.clone())
        };
        for i in 0..4 {
            if let Some(id) = find(CH_DETECT[i], true) {
                self.cfg.audio.ch[i].source = id;
            }
        }
        if let Some(id) = find(MIC_DETECT, true) {
            self.cfg.audio.mic.target = id;
        }
        // output: first physical render device that is not a Sonar VAD, prefer a headset
        let phys: Vec<&Dev> = self.devs.iter().filter(|d| d.render && !d.name.contains("Sonar")).collect();
        if self.cfg.audio.output.is_empty() {
            let score = |n: &str| {
                let n = n.to_lowercase();
                let mut s = 0;
                for (k, v) in [("arctis", 5), ("steelseries", 4), ("headset", 3), ("headphone", 2), ("usb", 1), ("realtek", -1), ("nvidia", -2), ("steam", -4)] {
                    if n.contains(k) {
                        s += v;
                    }
                }
                s
            };
            if let Some(d) = phys.iter().max_by_key(|d| score(&d.name)) {
                self.cfg.audio.output = d.id.clone();
            }
        }
        if self.cfg.audio.mic.input.is_empty() {
            if let Some(d) = self.devs.iter().find(|d| !d.render && !d.name.contains("Sonar")) {
                self.cfg.audio.mic.input = d.id.clone();
            }
        }
        self.dirty();
    }

    fn dev_name(&self, id: &str) -> String {
        if id.is_empty() {
            return "(NONE)".into();
        }
        self.devs
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.replace(" (SteelSeries Sonar Virtual Audio Device)", "").replace("SteelSeries ", ""))
            .unwrap_or_else(|| "(MISSING)".into())
    }

    fn cycle_dev(&self, cur: &str, render: bool, d: i32, virt_only: bool) -> String {
        let mut ids: Vec<String> = vec![String::new()];
        ids.extend(self.devs.iter().filter(|x| x.render == render && (!virt_only || win_audio::is_virtual(&x.name))).map(|x| x.id.clone()));
        let i = ids.iter().position(|x| x == cur).unwrap_or(0) as i32;
        ids[((i + d).rem_euclid(ids.len() as i32)) as usize].clone()
    }

    fn set_engine(&mut self, on: bool) {
        self.cfg.audio.engine_on = on;
        self.apply_direct(on, true); // off = every app gets its own volume/output back
        self.mic_applied = None;
        self.dirty();
        self.guard_defaults(false);
    }

    /// What Sonar does: while the DSP engine runs, Windows' default output is the
    /// GAME virtual device (so every app that isn't assigned elsewhere goes
    /// through GAME and the mixer/EQ) and the default mic is the virtual mic.
    /// If the user picks a real device as default in Windows, BitGG adopts it as
    /// its own output/mic input and points the default back at the virtual one.
    /// Returns true if a Windows default device was switched.
    fn route_defaults(&mut self) -> bool {
        if !self.needs_engine() {
            return false; // guard_defaults() restores real devices when the engine is off
        }
        let mut switched = false;
        let is_phys = |a: &App, id: &str| a.devs.iter().any(|d| d.id == id && !win_audio::is_virtual(&d.name));
        let mut changed = false;
        if self.ch_live(0) {
            let game = self.cfg.audio.ch[0].source.clone();
            if let Some(def) = win_audio::default_id(true) {
                if def != game {
                    if is_phys(self, &def) && def != self.cfg.audio.output {
                        *self.rec_msg.lock().unwrap() = format!("OUTPUT -> {}", self.dev_name(&def));
                        self.cfg.audio.output = def;
                        changed = true;
                    }
                    switched |= win_audio::set_default_device(&game);
                }
            }
        }
        if self.mic_live() {
            let vmic = self
                .devs
                .iter()
                .find(|d| !d.render && d.name.contains(MIC_DETECT))
                .map(|d| d.id.clone());
            if let (Some(vmic), Some(def)) = (vmic, win_audio::default_id(false)) {
                if def != vmic {
                    if is_phys(self, &def) && def != self.cfg.audio.mic.input {
                        self.cfg.audio.mic.input = def;
                        changed = true;
                    }
                    switched |= win_audio::set_default_device(&vmic);
                }
            }
        }
        if changed {
            self.shared.update(&self.engine_params()); // engine rebuilds its streams
            if self.dirty_since.is_none() {
                self.dirty_since = Some(Instant::now());
            }
        }
        switched
    }

    /// Safety net: Windows' default output/mic may only be a virtual device while
    /// the engine is forwarding it; otherwise the PC (or the mic) goes silent.
    fn guard_defaults(&mut self, force: bool) {
        if !force && self.engine.is_some() {
            return;
        }
        let virt = |a: &App, id: &Option<String>| {
            id.as_ref().map(|i| a.devs.iter().any(|d| &d.id == i && win_audio::is_virtual(&d.name))).unwrap_or(false)
        };
        let out = self.cfg.audio.output.clone();
        if !out.is_empty() && virt(self, &win_audio::default_id(true)) {
            win_audio::set_default_device(&out);
        }
        let mic = self.cfg.audio.mic.input.clone();
        if !mic.is_empty() && virt(self, &win_audio::default_id(false)) {
            win_audio::set_default_device(&mic);
        }
    }

    fn rec_sources(&self) -> (Option<String>, Option<String>) {
        let c = &self.cfg.clips;
        // AUTO sound = what you hear: BitGG's output while its DSP engine runs,
        // otherwise the Windows default output
        let lb = if !c.rec_audio.is_empty() {
            Some(c.rec_audio.clone())
        } else if self.engine.is_some() && !self.cfg.audio.output.is_empty() {
            Some(self.cfg.audio.output.clone())
        } else {
            win_audio::default_id(true)
        };
        // AUTO mic = the Windows default microphone (what Discord etc. use)
        let mic = match c.rec_mic.as_str() {
            "-" => None,
            "" if !c.record_mic => None,
            "" => win_audio::default_id(false),
            id => Some(id.to_string()),
        };
        (lb, mic)
    }

    fn rec_dev_label(&self, id: &str, auto: &str) -> String {
        match id {
            "" => auto.to_string(),
            "-" => "NO MIC".to_string(),
            _ => self.dev_name(id),
        }
    }

    fn start_replay(&mut self) {
        self.rec = None;
        let (lb, mic) = self.rec_sources();
        match Recorder::start(&self.cfg.clips, Mode::Replay, lb, mic, self.rec_levels.clone()) {
            Ok(r) => self.rec = Some(r),
            Err(e) => *self.rec_msg.lock().unwrap() = format!("ERR {e}"),
        }
    }

    fn toggle_replay(&mut self) {
        let on = !matches!(&self.rec, Some(r) if r.mode == Mode::Replay);
        self.cfg.clips.replay_on = on;
        self.dirty();
        if on {
            self.start_replay();
        } else {
            self.rec = None;
        }
    }

    fn save_clip(&mut self) {
        dlog("save_clip called");
        match &self.rec {
            Some(r) if r.mode == Mode::Replay => {
                *self.rec_msg.lock().unwrap() = "SAVING CLIP...".into();
                r.save_clip(self.cfg.clips.folder.clone(), self.rec_msg.clone());
            }
            _ => {
                beep_err(); // audible even while a game has focus
                *self.rec_msg.lock().unwrap() = "REPLAY BUFFER IS OFF".into();
            }
        }
    }

    fn toggle_record(&mut self) {
        if matches!(&self.rec, Some(r) if matches!(r.mode, Mode::Record(_))) {
            if let Some(mut r) = self.rec.take() {
                r.stop();
                *self.rec_msg.lock().unwrap() = r.status.lock().unwrap().clone();
            }
            if self.cfg.clips.replay_on {
                self.start_replay();
            }
            return;
        }
        self.rec = None; // a single capture pipeline at a time keeps it light
        let path = std::path::PathBuf::from(&self.cfg.clips.folder).join(format!("rec_{}.mp4", recorder::timestamp()));
        let (lb, mic) = self.rec_sources();
        match Recorder::start(&self.cfg.clips, Mode::Record(path), lb, mic, self.rec_levels.clone()) {
            Ok(r) => self.rec = Some(r),
            Err(e) => *self.rec_msg.lock().unwrap() = format!("ERR {e}"),
        }
    }

    fn restart_rec_if_replay(&mut self) {
        self.dirty();
        if matches!(&self.rec, Some(r) if r.mode == Mode::Replay) {
            self.start_replay();
        }
    }

    // ================================================================ UI

    fn draw(&mut self) {
        let ticks = self.ui.frame;
        if (self.tab == Tab::Apps || self.cfg.audio.engine_on) && self.last_refresh.elapsed() > Duration::from_millis(1500) {
            self.refresh_light();
        }
        if self.slow_refresh.elapsed() > Duration::from_secs(8) {
            self.slow_refresh = Instant::now();
            self.refresh();
            self.maybe_check_update();
        }
        let _ = ticks;
        self.ui.begin();
        self.ui.clear(0x10142a);
        // scanline-ish background stripes
        for y in (0..H as i32).step_by(4) {
            self.ui.hline(0, y, W as i32, 0x121830);
        }
        self.header();
        match self.tab {
            Tab::Mixer => self.tab_mixer(),
            Tab::Apps => self.tab_apps(),
            Tab::Eq => self.tab_eq(),
            Tab::Clips => self.tab_clips(),
            Tab::Setup => self.tab_setup(),
        }
        self.footer();
        if let Some(t) = self.ui.tooltip.clone() {
            let w = t.len() as i32 * 8 + 6;
            let x = (self.ui.inp.mx + 8).min(W as i32 - w - 1);
            let y = (self.ui.inp.my + 10).min(H as i32 - 13);
            self.ui.boxed(r(x, y, w, 12), BLACK, YELLOW);
            self.ui.text(x + 3, y + 2, &t, YELLOW);
        }
        self.ui.end();
    }

    fn header(&mut self) {
        let ui = &mut self.ui;
        ui.fill(r(0, 0, W as i32, 14), NAVY);
        ui.hline(0, 14, W as i32, BLUE);
        // tiny pixel logo
        ui.text_s(4, 3, "BIT", ORANGE);
        ui.text_s(28, 3, "GG", YELLOW);
        // version from Cargo.toml, tiny, bottom-aligned with the logo
        const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
        ui.text_tiny(48, 7, VERSION, BLACK); // shadow
        ui.text_tiny(47, 6, VERSION, LGREY);
        self.update_badge();
        let ui = &mut self.ui;
        let rec_on = self.rec.as_ref().map(|r| r.alive()).unwrap_or(false);
        if rec_on {
            let blink = (ui.frame / 10) % 2 == 0;
            let is_rec = matches!(self.rec.as_ref().map(|r| &r.mode), Some(Mode::Record(_)));
            if blink || !is_rec {
                ui.fill(r(W as i32 - 60, 4, 6, 6), if is_rec { RED } else { GREEN });
            }
            ui.text(W as i32 - 52, 3, if is_rec { "REC" } else { "BUF" }, WHITE);
        }
        let eng = self.cfg.audio.engine_on;
        ui.fill(r(W as i32 - 22, 4, 6, 6), if eng { GREEN } else { DGREY });
        ui.tip(r(W as i32 - 24, 0, 24, 14), "AUDIO ENGINE");
        let mut x = 52;
        for (t, name) in TABS {
            let w = name.len() as i32 * 8 + 10;
            let q = r(x, 17, w, 13);
            let sel = self.tab == t;
            let hov = q.has(self.ui.inp.mx, self.ui.inp.my);
            self.ui.boxed(q, if sel { BLUE } else { BLACK }, if sel || hov { BLUE } else { DGREY });
            self.ui.text_c(q, name, if sel { BLACK } else if hov { BLUE } else { LGREY });
            if self.ui.clicked_in(q) {
                self.tab = t;
            }
            x += w + 3;
        }
    }

    fn footer(&mut self) {
        let st = if !self.cfg.audio.engine_on {
            "ENGINE OFF".to_string()
        } else {
            let es = self.shared.status.lock().unwrap().clone();
            let live = (0..4).filter(|i| self.ch_live(*i)).count();
            if self.engine.is_some() && es.starts_with("ERR") {
                es
            } else {
                format!("RUN {live} DSP {} DIRECT", 4 - live)
            }
        };
        let msg = self.rec_msg.lock().unwrap().clone();
        let rs = self.rec.as_ref().map(|r| r.status.lock().unwrap().clone()).unwrap_or_default();
        let ui = &mut self.ui;
        ui.fill(r(0, H as i32 - 11, W as i32, 11), BLACK);
        ui.hline(0, H as i32 - 12, W as i32, DGREY);
        ui.text_fit(2, H as i32 - 9, &st, 22, if st.starts_with("RUN") { GREEN } else { LGREY });
        let right = if !msg.is_empty() { msg } else { rs };
        ui.text_fit(186, H as i32 - 9, &right, 26, if right.starts_with("ERR") || right.contains("FAIL") { RED } else { YELLOW });
    }

    fn tab_mixer(&mut self) {
        let labels = ["GAME", "CHAT", "MEDIA", "AUX", "MIC", "MASTER"];
        let mut changed = false;
        for i in 0..6 {
            let x = 6 + i as i32 * 64;
            let y = 36;
            let col = CH_COL[i];
            let m = self.strip_meter(i);
            self.ui.boxed(r(x, y, 60, 172), 0x0a0d1c, DGREY);
            self.ui.fill(r(x + 1, y + 1, 58, 11), col);
            self.ui.text_c(r(x, y + 2, 60, 9), labels[i], BLACK);
            let (vol, mute): (&mut f32, &mut bool) = match i {
                0..=3 => {
                    let c = &mut self.cfg.audio.ch[i];
                    (&mut c.vol, &mut c.mute)
                }
                4 => (&mut self.cfg.audio.mic.vol, &mut self.cfg.audio.mic.mute),
                _ => (&mut self.cfg.audio.master, &mut self.cfg.audio.master_mute),
            };
            changed |= self.ui.vslider(id("fader", i), r(x + 10, y + 18, 18, 122), vol, col);
            let pct = format!("{:3}", (*vol * 100.0).round() as i32);
            self.ui.meter(r(x + 34, y + 18, 6, 122), m);
            self.ui.meter(r(x + 42, y + 18, 6, 122), m * 0.97);
            self.ui.text(x + 8, y + 144, &pct, WHITE);
            self.ui.text(x + 34, y + 144, "%", DGREY);
            changed |= self.ui.toggle(r(x + 6, y + 156, 48, 12), if *mute { "MUTED" } else { "MUTE" }, mute, RED);
            if i < 5 {
                let live = if i < 4 { self.ch_live(i) } else { self.mic_live() };
                // corner dot: green = DSP (virtual device + EQ), lavender = DIRECT volume
                self.ui.fill(r(x + 52, y + 4, 4, 4), if live { DGREEN_DOT } else { LAVENDER });
                let tip = if live {
                    "DSP: VIRTUAL DEVICE + EQ"
                } else if i < 4 {
                    "DIRECT: SCALES APPS SET IN APPS"
                } else {
                    "DIRECT: REAL MIC VOLUME"
                };
                self.ui.tip(r(x, y, 60, 14), tip);
            }
        }
        // ChatMix
        let y = 212;
        self.ui.text_s(6, y + 2, "GAME", GREEN);
        let mut t = (self.cfg.audio.chatmix + 1.0) / 2.0;
        if self.ui.hslider(id("chatmix", 0), r(42, y, 200, 12), &mut t, LAVENDER) {
            self.cfg.audio.chatmix = t * 2.0 - 1.0;
            if self.cfg.audio.chatmix.abs() < 0.06 {
                self.cfg.audio.chatmix = 0.0;
            }
            changed = true;
        }
        self.ui.fill(r(141, y - 2, 2, 16), WHITE);
        self.ui.text_s(246, y + 2, "CHAT", BLUE);
        self.ui.tip(r(42, y, 200, 12), "CHATMIX BALANCE");
        let mut on = self.cfg.audio.engine_on;
        if self.ui.toggle(r(300, y - 1, 94, 14), if on { "ENGINE ON" } else { "ENGINE OFF" }, &mut on, GREEN) {
            self.set_engine(on);
        }
        let stuck: Vec<String> = self.sessions.iter().filter(|s| self.needs_restart(s)).map(|s| s.name.clone()).collect();
        if self.sonar_running {
            self.ui.text_s(6, 228, "QUIT GG SONAR FIRST (DOUBLE AUDIO)", ORANGE);
        } else if !stuck.is_empty() {
            let msg = format!("RESTART {} TO USE MIXER", if stuck.len() == 1 { stuck[0].to_uppercase() } else { format!("{} APPS", stuck.len()) });
            self.ui.text_fit(6, 228, &msg, 48, ORANGE);
            self.ui.tip(r(0, 224, W as i32, 12), &format!("STILL ON OLD DEVICE: {}", stuck.join(", ").to_uppercase()));
        }
        if changed {
            self.dirty();
        }
    }

    fn tab_apps(&mut self) {
        let mut ui_y = 36;
        self.ui.text_s(6, ui_y, "APP", WHITE);
        self.ui.text_s(150, ui_y, "CHANNEL", WHITE);
        self.ui.text_s(240, ui_y, "VOLUME", WHITE);
        ui_y += 12;
        let rows = 14usize;
        let total = self.sessions.len();
        let wheel = self.ui.wheel_in(r(0, ui_y, W as i32, rows as i32 * 13));
        if wheel != 0 {
            self.apps_scroll = (self.apps_scroll as i32 - wheel).clamp(0, total.saturating_sub(rows) as i32) as usize;
        }
        if total == 0 {
            self.ui.text_s(6, ui_y + 10, "NO APPS ARE PLAYING AUDIO", DGREY);
        }
        let opts = ["DEFAULT", "GAME", "CHAT", "MEDIA", "AUX"];
        let sessions = self.sessions.clone();
        for (row, s) in sessions.iter().enumerate().skip(self.apps_scroll).take(rows) {
            let y = ui_y + (row - self.apps_scroll) as i32 * 13;
            if row % 2 == 0 {
                self.ui.fill(r(2, y - 1, W as i32 - 4, 13), 0x161c38);
            }
            // peak pip
            let pk = (s.peak * 10.0).min(1.0);
            self.ui.fill(r(4, y + 2, 4, 7), if pk > 0.05 { GREEN } else { DGREY });
            self.ui.text_fit(12, y + 1, &s.name, 16, WHITE);
            // routed to a channel but still playing elsewhere (e.g. Chromium apps
            // only switch device after a restart): say so instead of failing silently
            if self.needs_restart(&s) {
                self.ui.fill(r(138, y + 1, 6, 8), ORANGE);
                self.ui.text(138, y + 1, "!", BLACK);
                self.ui.tip(r(4, y - 1, 140, 12), "NOT IN ITS CHANNEL YET: RESTART THIS APP");
            }
            let cur = self.app_ch(&s.name);
            let col = if cur == 0 { LGREY } else { CH_COL[cur - 1] };
            let d = self.ui.spinner(r(146, y - 1, 88, 12), opts[cur], col);
            if d != 0 {
                let nxt = ((cur as i32 + d).rem_euclid(5)) as usize;
                let on = self.cfg.audio.engine_on;
                if cur != 0 {
                    // leaving a channel: the app gets its own volume and output back
                    self.apply_direct(false, true);
                }
                let e = self.cfg.audio.apps.entry(s.name.clone()).or_default();
                if e.vol.is_none() || cur == 0 {
                    e.vol = Some(s.vol);
                }
                e.ch = nxt as u8;
                self.applied.clear();
                self.dirty();
                self.apply_direct(on, true);
                if nxt != 0 && !on {
                    *self.rec_msg.lock().unwrap() = "TURN ENGINE ON (MIXER)".into();
                }
            }
            // apps a channel scales (assigned, or unassigned while GAME is DIRECT):
            // the slider edits the app's own base volume, the channel scales it
            let scaled = cur != 0 || (self.cfg.audio.engine_on && !self.ch_live(0));
            let base = self.cfg.audio.apps.get(&s.name).and_then(|a| a.vol);
            let mut v = if scaled { base.unwrap_or(s.vol) } else { s.vol };
            if self.ui.hslider(id("appvol", s.pid as usize), r(240, y - 1, 110, 12), &mut v, col) {
                if scaled {
                    let e = self.cfg.audio.apps.entry(s.name.clone()).or_default();
                    e.vol = Some(v);
                    self.dirty();
                } else {
                    for p in &s.pids {
                        win_audio::set_session(&self.devs, *p, Some(v), None);
                    }
                    if let Some(x) = self.sessions.iter_mut().find(|x| x.pid == s.pid) {
                        x.vol = v;
                    }
                    // keep a stored base in sync so a later restore doesn't undo this
                    if let Some(e) = self.cfg.audio.apps.get_mut(&s.name) {
                        if e.vol.is_some() {
                            e.vol = Some(v);
                        }
                    }
                }
            }
            let mut m = s.mute;
            if self.ui.toggle(r(354, y - 1, 40, 12), "MUTE", &mut m, RED) {
                for p in &s.pids {
                    win_audio::set_session(&self.devs, *p, None, Some(m));
                }
                if let Some(x) = self.sessions.iter_mut().find(|x| x.pid == s.pid) {
                    x.mute = m;
                }
            }
        }
        if total > rows {
            self.ui.text(330, 36, &format!("{}/{}", self.apps_scroll + 1, total - rows + 1), DGREY);
        }
        self.ui.text(6, 211, "DEFAULT = GAME CHANNEL (LIKE SONAR)", DGREY);
        self.ui.text(6, 220, "CHANNEL W/ VIRTUAL DEVICE = ROUTED + EQ", DGREY);
        self.ui.text(6, 229, "NO DEVICE = DIRECT: MIXER SCALES APP VOL", DGREY);
    }

    fn tab_eq(&mut self) {
        let names = ["GAME", "CHAT", "MEDIA", "AUX", "MIC"];
        let col = CH_COL[self.eq_ch];
        let d = self.ui.spinner(r(6, 34, 90, 13), names[self.eq_ch], col);
        if d != 0 {
            self.eq_ch = ((self.eq_ch as i32 + d).rem_euclid(5)) as usize;
        }
        let mut changed = false;
        let preset_btns = dsp::PRESETS;
        {
            let eq = if self.eq_ch < 4 { &mut self.cfg.audio.ch[self.eq_ch].eq } else { &mut self.cfg.audio.mic.eq };
            let mut on = eq.on;
            if self.ui.toggle(r(100, 34, 54, 13), if on { "EQ ON" } else { "EQ OFF" }, &mut on, col) {
                eq.on = on;
                changed = true;
            }
            // the preset in use lights up in the channel colour (like EQ ON);
            // none lit = custom settings
            let active = dsp::active_preset(eq);
            let mut x = 160;
            for p in preset_btns {
                let w = p.len() as i32 * 8 + 8;
                let q = r(x, 34, w, 13);
                if active == Some(p) {
                    self.ui.boxed(q, col, col);
                    self.ui.text_c(q, p, BLACK);
                    if self.ui.clicked_in(q) {
                        *eq = dsp::preset(p); // re-apply resets any stray edits
                        changed = true;
                    }
                } else if self.ui.button(q, p, col) {
                    *eq = dsp::preset(p);
                    changed = true;
                }
                x += w + 3;
            }
            self.ui.tip(r(160, 34, x - 160, 13), if active.is_some() { "PRESET IN USE IS LIT" } else { "CUSTOM EQ (NO PRESET)" });
        }
        // ---- response graph
        let g = r(6, 52, 388, 100);
        self.ui.boxed(g, BLACK, DGREY);
        let fx = |f: f32| g.x + 2 + (((f / 20.0).log10() / 3.0) * (g.w - 4) as f32) as i32;
        let fy = |db: f32| g.y + g.h / 2 - (db / 15.0 * (g.h / 2 - 3) as f32) as i32;
        for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
            let x = fx(f);
            self.ui.vline(x, g.y + 1, g.h - 2, 0x1a2140);
        }
        for db in [-12.0, -6.0, 6.0, 12.0] {
            self.ui.hline(g.x + 1, fy(db), g.w - 2, 0x1a2140);
        }
        self.ui.hline(g.x + 1, fy(0.0), g.w - 2, DGREY);
        for (f, l) in [(100.0, "100"), (1000.0, "1K"), (10000.0, "10K")] {
            self.ui.text(fx(f) + 2, g.y + g.h - 10, l, DGREY);
        }
        let eq = if self.eq_ch < 4 { self.cfg.audio.ch[self.eq_ch].eq.clone() } else { self.cfg.audio.mic.eq.clone() };
        let mut prev: Option<i32> = None;
        for px in 0..(g.w - 4) {
            let f = 20.0 * 10f32.powf(3.0 * px as f32 / (g.w - 4) as f32);
            let db = dsp::response_db(&eq, f).clamp(-15.0, 15.0);
            let y = fy(db);
            let x = g.x + 2 + px;
            let y0 = prev.unwrap_or(y);
            let (a, b) = if y0 < y { (y0, y) } else { (y, y0) };
            self.ui.vline(x, a, b - a + 1, if eq.on { col } else { DGREY });
            prev = Some(y);
        }
        // band handles
        for (i, b) in eq.bands.iter().enumerate() {
            let x = fx(b.freq);
            let y = fy(b.gain_db.clamp(-15.0, 15.0));
            let c = if i == self.eq_band { WHITE } else if b.on { col } else { DGREY };
            self.ui.boxed(r(x - 3, y - 3, 7, 7), BLACK, c);
            self.ui.put(x, y, c);
        }
        // drag on graph moves the selected band
        let gid = id("eqgraph", self.eq_ch);
        if g.has(self.ui.inp.mx, self.ui.inp.my) && self.ui.inp.pressed {
            // pick nearest handle
            let mut best = (i32::MAX, self.eq_band);
            for (i, b) in eq.bands.iter().enumerate() {
                let dx = fx(b.freq) - self.ui.inp.mx;
                let dy = fy(b.gain_db) - self.ui.inp.my;
                let dd = dx * dx + dy * dy;
                if dd < best.0 {
                    best = (dd, i);
                }
            }
            self.eq_band = best.1;
            self.ui.active = gid;
        }
        {
            let eqm = if self.eq_ch < 4 { &mut self.cfg.audio.ch[self.eq_ch].eq } else { &mut self.cfg.audio.mic.eq };
            let band = &mut eqm.bands[self.eq_band];
            if self.ui.active == gid && self.ui.inp.down {
                let t = ((self.ui.inp.mx - g.x - 2) as f32 / (g.w - 4) as f32).clamp(0.0, 1.0);
                band.freq = (20.0 * 10f32.powf(3.0 * t)).round();
                let db = (g.y + g.h / 2 - self.ui.inp.my) as f32 / (g.h / 2 - 3) as f32 * 15.0;
                band.gain_db = (db.clamp(-12.0, 12.0) * 2.0).round() / 2.0;
                eqm.on = true;
                changed = true;
            }
            let wh = self.ui.wheel_in(g);
            if wh != 0 {
                band.q = (band.q * if wh > 0 { 1.15 } else { 1.0 / 1.15 }).clamp(0.1, 12.0);
                changed = true;
            }
        }
        self.ui.tip(r(g.x + g.w - 40, g.y, 40, 10), "DRAG = FREQ/GAIN, WHEEL = Q");
        // band selector
        for i in 0..dsp::BANDS {
            let q = r(6 + i as i32 * 21, 156, 19, 12);
            let on = eq.bands[i].on;
            let sel = i == self.eq_band;
            self.ui.boxed(q, if sel { col } else { BLACK }, if on { col } else { DGREY });
            self.ui.text_c(q, &format!("{}", (i + 1) % 10), if sel { BLACK } else { WHITE });
            if self.ui.clicked_in(q) {
                self.eq_band = i;
            }
        }
        // band detail
        {
            let eqm = if self.eq_ch < 4 { &mut self.cfg.audio.ch[self.eq_ch].eq } else { &mut self.cfg.audio.mic.eq };
            let b = &mut eqm.bands[self.eq_band];
            let y = 174;
            self.ui.text_s(6, y + 2, "TYPE", LGREY);
            if self.ui.spinner(r(42, y, 50, 12), b.kind.short(), col) != 0 {
                b.kind = b.kind.next();
                changed = true;
            }
            self.ui.text_s(98, y + 2, "HZ", LGREY);
            let d = self.ui.spinner(r(116, y, 72, 12), &format!("{}", b.freq.round() as i32), col);
            if d != 0 {
                b.freq = (b.freq * 1.0595f32.powi(d)).clamp(20.0, 20000.0).round();
                changed = true;
            }
            self.ui.text_s(194, y + 2, "DB", LGREY);
            let d = self.ui.spinner(r(212, y, 64, 12), &format!("{:+.1}", b.gain_db), col);
            if d != 0 {
                b.gain_db = (b.gain_db + d as f32 * 0.5).clamp(-12.0, 12.0);
                changed = true;
            }
            self.ui.text_s(282, y + 2, "Q", LGREY);
            let d = self.ui.spinner(r(292, y, 58, 12), &format!("{:.2}", b.q), col);
            if d != 0 {
                b.q = (b.q * 1.12f32.powi(d)).clamp(0.1, 12.0);
                changed = true;
            }
            let mut on = b.on;
            if self.ui.toggle(r(354, y, 40, 12), "ON", &mut on, col) {
                b.on = on;
                changed = true;
            }
        }
        // mic extras: noise gate
        if self.eq_ch == 4 {
            let y = 194;
            let m = &mut self.cfg.audio.mic;
            let mut on = m.gate_on;
            if self.ui.toggle(r(6, y, 80, 12), "NOISE GATE", &mut on, YELLOW) {
                m.gate_on = on;
                changed = true;
            }
            let mut t = (m.gate_db + 80.0) / 70.0;
            if self.ui.hslider(id("gate", 0), r(92, y, 200, 12), &mut t, YELLOW) {
                m.gate_db = (t * 70.0 - 80.0).round();
                changed = true;
            }
            let gdb = m.gate_db;
            self.ui.text(298, y + 2, &format!("{:+} DB", gdb as i32), WHITE);
            let lvl = self.shared.meter(4);
            let mut lv = ((20.0 * lvl.max(1e-5).log10() + 80.0) / 70.0).clamp(0.0, 1.0);
            self.ui.hslider(id("micl", 0), r(92, y + 15, 200, 6), &mut lv, GREEN);
        }
        if changed {
            self.dirty();
        }
    }

    fn tab_clips(&mut self) {
        let c = self.cfg.clips.clone();
        let mut restart = false;
        let replay_on = matches!(&self.rec, Some(r) if r.mode == Mode::Replay);
        let mut on = replay_on;
        if self.ui.toggle(r(6, 36, 120, 16), if on { "REPLAY: ON" } else { "REPLAY: OFF" }, &mut on, GREEN) {
            self.toggle_replay();
        }
        let save_lbl = format!("SAVE {}", self.cfg.clips.save_key);
        if self.ui.button(r(132, 36, 128, 16), &save_lbl, YELLOW) {
            self.save_clip();
        }
        let is_rec = matches!(&self.rec, Some(r) if matches!(r.mode, Mode::Record(_)));
        let lbl = if is_rec {
            let s = self.rec.as_ref().unwrap().started.elapsed().as_secs();
            format!("STOP {:02}:{:02}", s / 60, s % 60)
        } else {
            "REC  ALT+F9".into()
        };
        if self.ui.button(r(266, 36, 128, 16), &lbl, RED) {
            self.toggle_record();
        }
        let audio_lbl = self.rec_dev_label(&c.rec_audio, "AUTO (WHAT YOU HEAR)");
        let mic_lbl = if c.rec_mic.is_empty() && !c.record_mic { "NO MIC".to_string() } else { self.rec_dev_label(&c.rec_mic, "AUTO (DEFAULT MIC)") };
        let rows: [(&str, String); 8] = [
            ("BUFFER", format!("{} SEC", c.replay_secs)),
            ("FPS", format!("{}", c.fps)),
            ("RES", if c.height == 0 { "NATIVE".into() } else { format!("{}P", c.height) }),
            ("BITRATE", format!("{} MBPS", c.bitrate_kbps / 1000)),
            ("ENCODER", c.encoder.to_uppercase()),
            ("SCREEN", self.monitors.get(c.monitor as usize).map(|m| format!("{} {}X{}", m.name, m.w, m.h)).unwrap_or("?".into())),
            ("AUDIO", audio_lbl),
            ("MIC", mic_lbl),
        ];
        // row y: settings 0..5, then AUDIO / AUDIO VOL / MIC / MIC VOL
        let row_y = |i: usize| -> i32 {
            let slot = match i {
                0..=6 => i,
                _ => i + 1, // MIC sits below AUDIO VOL
            };
            56 + slot as i32 * 13
        };
        for (i, (k, v)) in rows.iter().enumerate() {
            let y = row_y(i);
            self.ui.text_s(6, y + 2, k, LGREY);
            let d = self.ui.spinner(r(80, y, 180, 11), v, if i >= 6 { GREEN } else { BLUE });
            if d == 0 {
                continue;
            }
            match i {
                0 => self.cfg.clips.replay_secs = cycle(&REPLAY_LENS, self.cfg.clips.replay_secs, d),
                1 => self.cfg.clips.fps = cycle(&FPS_OPTS, self.cfg.clips.fps, d),
                2 => self.cfg.clips.height = cycle(&RES_OPTS, self.cfg.clips.height, d),
                3 => self.cfg.clips.bitrate_kbps = cycle(&BR_OPTS, self.cfg.clips.bitrate_kbps, d),
                4 => self.cfg.clips.encoder = cycle(&ENC_OPTS, self.cfg.clips.encoder.as_str(), d).to_string(),
                5 => {
                    self.monitors = dxgi::monitors();
                    let n = self.monitors.len().max(1) as i32;
                    self.cfg.clips.monitor = ((self.cfg.clips.monitor as i32 + d).rem_euclid(n)) as u32;
                }
                6 => {
                    let mut ids: Vec<String> = vec![String::new()];
                    ids.extend(self.devs.iter().filter(|x| x.render).map(|x| x.id.clone()));
                    let k = ids.iter().position(|x| *x == self.cfg.clips.rec_audio).unwrap_or(0) as i32;
                    self.cfg.clips.rec_audio = ids[((k + d).rem_euclid(ids.len() as i32)) as usize].clone();
                }
                _ => {
                    let cur = if self.cfg.clips.rec_mic.is_empty() && !self.cfg.clips.record_mic { "-".to_string() } else { self.cfg.clips.rec_mic.clone() };
                    let mut ids: Vec<String> = vec![String::new(), "-".to_string()];
                    ids.extend(self.devs.iter().filter(|x| !x.render).map(|x| x.id.clone()));
                    let k = ids.iter().position(|x| *x == cur).unwrap_or(0) as i32;
                    let nv = ids[((k + d).rem_euclid(ids.len() as i32)) as usize].clone();
                    self.cfg.clips.record_mic = nv != "-";
                    self.cfg.clips.rec_mic = nv;
                }
            }
            restart = true;
        }
        // volume sliders + live level bars (gain applies instantly, no restart)
        let mut gain_changed = false;
        for (slot, label, which) in [(7usize, "AUDIO VOL", 0), (9usize, "MIC VOL", 1)] {
            let y = 56 + slot as i32 * 13;
            self.ui.text_s(6, y + 2, label, LGREY);
            let mut v = if which == 0 { self.cfg.clips.rec_audio_gain } else { self.cfg.clips.rec_mic_gain };
            if self.ui.hslider(id("recgain", which), r(80, y, 130, 9), &mut v, GREEN) {
                if which == 0 {
                    self.cfg.clips.rec_audio_gain = v;
                } else {
                    self.cfg.clips.rec_mic_gain = v;
                }
                gain_changed = true;
            }
            let g = config::slider_gain(v);
            let db = if g == 0.0 { "OFF".to_string() } else { format!("{:+.0}DB", 20.0 * g.log10()) };
            self.ui.text(214, y + 1, &db, WHITE);
            let lvl = if which == 0 { self.rec_levels.audio_level() } else { self.rec_levels.mic_level() };
            let t = ((20.0 * lvl.max(1e-5).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
            let col = if lvl > 0.89 { RED } else if lvl > 0.5 { YELLOW } else { GREEN };
            self.ui.fill(r(80, y + 10, 130, 2), NAVY);
            self.ui.fill(r(80, y + 10, (130.0 * t) as i32, 2), col);
            self.ui.tip(r(80, y, 170, 12), "LEVEL IN RECORDING (LIVE WHILE REPLAY/REC RUNS)");
        }
        if gain_changed {
            self.rec_levels.set_gains(config::slider_gain(self.cfg.clips.rec_audio_gain), config::slider_gain(self.cfg.clips.rec_mic_gain));
            self.dirty();
        }
        // compressed packets live in RAM: show the budget and the live size
        let mb = c.bitrate_kbps as f32 / 8.0 / 1024.0 * (c.replay_secs + 3) as f32;
        self.ui.text(270, 62, &format!("RAM ~{:.0} MB", mb), DGREY);
        if let Some(r) = self.rec.as_ref().filter(|r| r.mode == Mode::Replay) {
            let live = r.ring_mb();
            self.ui.text(270, 74, &format!("NOW {:.0} MB", live), GREEN);
        }
        // save hotkey: works in-game while BitGG is in the tray
        self.ui.text_s(270, 140, "SAVE KEY", LGREY);
        let d = self.ui.spinner(r(270, 150, 124, 12), &self.cfg.clips.save_key.clone(), YELLOW);
        if d != 0 {
            self.cfg.clips.save_key = cycle(&hotkey::SAVE_KEYS, self.cfg.clips.save_key.as_str(), d).to_string();
            hotkey::rebind(&self.cfg.clips.save_key);
            self.dirty();
        }
        let hs = hotkey::STATUS.lock().unwrap().clone();
        // the wanted key was taken (e.g. NVIDIA owns Alt+F10): adopt the free one
        let active = hotkey::ACTIVE_SAVE.lock().unwrap().clone();
        if !active.is_empty() && active != self.cfg.clips.save_key {
            *self.rec_msg.lock().unwrap() = format!("{} IN USE -> SAVE KEY {active}", self.cfg.clips.save_key);
            self.cfg.clips.save_key = active;
            self.dirty();
        }
        let tip = if hs.contains("TAKEN") {
            "WANTED KEY IS USED BY ANOTHER APP (E.G. NVIDIA)"
        } else if hs.ends_with("HOOK") {
            "ALL KEYS TAKEN: USING KEYBOARD HOOK"
        } else {
            "GLOBAL HOTKEY: WORKS IN GAMES, FROM TRAY"
        };
        self.ui.tip(r(270, 140, 124, 22), tip);
        self.ui.text(270, 90, if hs.ends_with("HOOK") { "KEY: HOOK MODE" } else { "KEY: READY" }, if hs.is_empty() { DGREY } else { GREEN });
        if self.ui.button(r(270, 104, 124, 13), "OPEN FOLDER", BLUE) {
            let _ = std::fs::create_dir_all(&self.cfg.clips.folder);
            let _ = std::process::Command::new("explorer").arg(&self.cfg.clips.folder).spawn();
        }
        if self.ui.button(r(270, 121, 124, 13), "CHANGE FOLDER", BLUE) {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let owner = self
                .window
                .as_ref()
                .and_then(|w| w.window_handle().ok())
                .and_then(|h| match h.as_raw() {
                    RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                    _ => None,
                })
                .unwrap_or(0);
            picker::pick_folder(owner, self.cfg.clips.folder.clone(), self.picked_folder.clone());
        }
        let picked = self.picked_folder.lock().unwrap().take();
        if let Some(f) = picked {
            self.cfg.clips.folder = f;
            self.clips = recent_clips(&self.cfg.clips.folder);
            *self.rec_msg.lock().unwrap() = "CLIP FOLDER CHANGED".into();
            self.dirty(); // saved to config; the replay buffer keeps running
        }
        self.ui.text_s(6, 190, "SAVE TO", LGREY);
        let folder = self.cfg.clips.folder.clone();
        // show the tail of long paths, it is the informative part
        let max = 40usize;
        let shown: String = if folder.chars().count() > max {
            format!("~{}", folder.chars().rev().take(max - 1).collect::<Vec<_>>().into_iter().rev().collect::<String>())
        } else {
            folder.clone()
        };
        self.ui.text(70, 190, &shown, WHITE);
        self.ui.tip(r(70, 188, 324, 11), &folder.to_uppercase());
        // recent clips
        self.ui.text_s(6, 203, "RECENT", WHITE);
        let clips = self.clips.clone();
        for (i, p) in clips.iter().take(2).enumerate() {
            let y = 215 + i as i32 * 11;
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let q = r(6, y - 1, 388, 10);
            let hov = q.has(self.ui.inp.mx, self.ui.inp.my);
            self.ui.text_fit(10, y, &format!("> {name}"), 46, if hov { YELLOW } else { LGREY });
            if self.ui.clicked_in(q) {
                let _ = std::process::Command::new("explorer").arg(p).spawn();
            }
        }
        if restart {
            self.restart_rec_if_replay();
        }
    }

    fn tab_setup(&mut self) {
        let mut changed = false;
        let mut rows: Vec<(String, String, bool, usize)> = Vec::new();
        rows.push(("OUTPUT".into(), self.cfg.audio.output.clone(), true, 0));
        for i in 0..4 {
            rows.push((format!("{} SRC", CH_NAMES[i]), self.cfg.audio.ch[i].source.clone(), true, 1 + i));
        }
        rows.push(("MIC IN".into(), self.cfg.audio.mic.input.clone(), false, 5));
        rows.push(("MIC OUT".into(), self.cfg.audio.mic.target.clone(), true, 6));
        for (n, (label, cur, render, slot)) in rows.iter().enumerate() {
            let y = 36 + n as i32 * 15;
            self.ui.text_s(6, y + 2, label, LGREY);
            let virt = (1..=4).contains(slot) || *slot == 6;
            let gone = virt && !self.dev_live(cur);
            let name = if gone {
                if cur.is_empty() { "(NONE) = DIRECT MODE".to_string() } else { "(GONE) = DIRECT MODE".to_string() }
            } else {
                self.dev_name(cur)
            };
            let d = self.ui.spinner(r(86, y, 308, 12), &name, if gone { LAVENDER } else { BLUE });
            if d != 0 {
                let nv = self.cycle_dev(cur, *render, d, virt);
                match slot {
                    0 => self.cfg.audio.output = nv,
                    1..=4 => self.cfg.audio.ch[slot - 1].source = nv,
                    5 => self.cfg.audio.mic.input = nv,
                    _ => self.cfg.audio.mic.target = nv,
                }
                changed = true;
            }
        }
        let y = 146;
        if self.ui.button(r(6, y, 120, 14), "AUTO-DETECT", GREEN) {
            self.refresh();
            self.cfg.audio.output.clear();
            self.cfg.audio.mic.input.clear();
            self.autodetect();
        }
        self.ui.tip(r(6, y, 120, 14), "FIND SONAR VIRTUAL DEVICES");
        if self.ui.button(r(132, y, 128, 14), "WIN DEFAULT=GAME", ORANGE) {
            let ok = !self.cfg.audio.ch[0].source.is_empty() && win_audio::set_default_device(&self.cfg.audio.ch[0].source);
            *self.rec_msg.lock().unwrap() = if ok { "DEFAULT OUTPUT -> GAME".into() } else { "FAILED".into() };
        }
        self.ui.tip(r(132, y, 128, 14), "NEW APPS PLAY INTO GAME CHANNEL");
        if self.ui.button(r(266, y, 128, 14), "RESET APP ROUTES", RED) {
            win_audio::clear_all_app_outputs();
            self.refresh();
        }
        // ---- UI scale + Sonar VADs
        let y = 164;
        self.ui.text_s(6, y + 2, "UI SCALE", LGREY);
        let sc = if self.cfg.scale == 0 { "AUTO".to_string() } else { format!("{}X", self.cfg.scale) };
        let d = self.ui.spinner(r(86, y, 80, 12), &sc, BLUE);
        if d != 0 {
            self.cfg.scale = ((self.cfg.scale as i32 + d).rem_euclid(5)) as u32;
            if let Some(w) = &self.window {
                let s = if self.cfg.scale == 0 { 2 } else { self.cfg.scale };
                let _ = w.request_inner_size(LogicalSize::new((W as u32 * s) as f64, (H as u32 * s) as f64));
            }
            changed = true;
        }
        // Sonar leaves its VADs disabled when GG quits; re-enable them like Sonar does
        let vads = self.vads.clone();
        if !vads.is_empty() {
            let active = vads.iter().filter(|v| v.2 == 1).count();
            let all_on = active == vads.len();
            let label = if all_on { "SONAR VADS: ON" } else { "ENABLE SONAR VADS" };
            if self.ui.button(r(200, y, 194, 12), label, if all_on { GREEN } else { YELLOW }) {
                if all_on {
                    self.guard_defaults(true); // about to remove the devices
                }
                let (n, total) = win_audio::set_sonar_vads(!all_on);
                self.cfg.audio.auto_vads = !all_on;
                *self.rec_msg.lock().unwrap() = format!("SONAR VADS {} {n}/{total}", if all_on { "OFF" } else { "ON" });
                std::thread::sleep(Duration::from_millis(400)); // let PnP publish the endpoints
                self.refresh();
                if !all_on {
                    self.autodetect();
                    self.set_engine(true);
                }
                self.guard_defaults(false);
                changed = true;
            }
            self.ui.tip(r(200, y, 194, 12), if all_on { "CLICK = DISABLE AGAIN" } else { "TURN ON GAME/CHAT/MEDIA/AUX/MIC VADS" });
        }
        // ---- shortcuts / autostart / keyboard-hook opt-in
        let y = 181;
        if self.ui.button(r(6, y, 120, 12), "ADD SHORTCUTS", GREEN) {
            let n = shortcuts::add_shortcuts();
            *self.rec_msg.lock().unwrap() = format!("{n} SHORTCUT(S) ADDED");
        }
        self.ui.tip(r(6, y, 120, 12), "DESKTOP + START MENU LINKS TO THIS EXE");
        let mut boot = shortcuts::startup_enabled();
        if self.ui.toggle(r(132, y, 120, 12), if boot { "STARTUP: ON" } else { "STARTUP: OFF" }, &mut boot, GREEN) {
            shortcuts::set_startup(boot);
            *self.rec_msg.lock().unwrap() = if boot { "RUNS AT LOGIN (TRAY)".into() } else { "AUTOSTART OFF".into() };
        }
        self.ui.tip(r(132, y, 120, 12), "START BITGG IN TRAY WHEN YOU LOG IN");
        let mut hook = self.cfg.clips.kbd_hook;
        let hook_col = if hook { ORANGE } else { GREEN };
        let hook_lbl = if hook { "KEY HOOK: ON" } else { "KEY HOOK: OFF" };
        if self.ui.toggle(r(258, y, 136, 12), hook_lbl, &mut hook, hook_col) {
            self.cfg.clips.kbd_hook = hook;
            hotkey::set_allow_hook(hook);
            changed = true;
        }
        self.ui.tip(r(258, y, 136, 12), "ONLY IF ALL HOTKEYS ARE TAKEN. OFF IS CLEANER");
        // ---- help text
        let hk = format!("HOTKEYS {} SAVE  ALT+F9 REC  ALT+F8 REPLAY", hotkey::ACTIVE_SAVE.lock().unwrap());
        self.ui.text(6, 198, &hk, DGREY);
        self.ui.text(6, 208, "NO VIRTUAL DEVICE = DIRECT MODE (VOLUME).", DGREY);
        // updates: the only network use in BitGG (GitHub release check)
        let mut on = !self.cfg.update_check_off;
        if self.ui.toggle(r(6, 218, 120, 11), if on { "UPDATES: AUTO" } else { "UPDATES: OFF" }, &mut on, GREEN) {
            self.cfg.update_check_off = !on;
            if on {
                self.last_update_check = None;
                self.maybe_check_update();
            }
            changed = true;
        }
        self.ui.tip(r(6, 218, 120, 11), "CHECKS GITHUB FOR A NEW RELEASE (6H)");
        let st = match self.update.lock().unwrap().clone() {
            update::State::Idle => String::new(),
            update::State::Checking => "CHECKING...".into(),
            update::State::UpToDate => format!("UP TO DATE (V{})", update::current()),
            update::State::Downloading(v) => format!("DOWNLOADING {}", v.to_uppercase()),
            update::State::Ready { version, .. } => format!("{} READY: CLICK UPDATE", version.to_uppercase()),
            update::State::Error(e) => format!("UPDATE: {}", e.to_uppercase()),
        };
        self.ui.text_fit(132, 220, &st, 32, if st.contains("READY") { GREEN } else { DGREY });
        if self.sonar_running {
            self.ui.text_s(6, 231, "GG SONAR RUNNING: QUIT GG FIRST", ORANGE);
        }
        if changed {
            self.dirty();
        }
    }

    fn show_window(&mut self) {
        if let Some(w) = &self.window {
            w.set_visible(true);
            w.set_minimized(false);
            w.focus_window();
            w.request_redraw();
        }
        self.visible = true;
        self.force_present = true;
    }

    fn hide_to_tray(&mut self) {
        if let Some(w) = &self.window {
            w.set_visible(false);
        }
        self.visible = false;
        self.cfg.save();
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
        self.shutdown();
        el.exit();
    }

    /// Leave the PC exactly as we found it (also used before an update restart).
    fn shutdown(&mut self) {
        tray::remove();
        self.apply_direct(false, true); // leave every app as we found it
        if let Some(mut e) = self.engine.take() {
            e.stop();
        }
        let _ = self.apo_bypass(false); // give Sonar's APO its settings back
        self.guard_defaults(true); // nothing forwards the VADs after exit
        self.cfg.save();
        self.rec = None;
        self.engine = None;
    }

    // ---------------------------------------------------------- updates

    fn maybe_check_update(&mut self) {
        if self.cfg.update_check_off {
            return;
        }
        let due = match self.last_update_check {
            None => true,
            Some(t) => t.elapsed() > Duration::from_secs(6 * 3600),
        };
        if due {
            self.last_update_check = Some(Instant::now());
            update::spawn_check(self.update.clone());
        }
    }

    /// Header: "UPDATE vX.Y.Z" button once a verified download is ready.
    fn update_badge(&mut self) {
        let st = self.update.lock().unwrap().clone();
        match st {
            update::State::Downloading(v) => {
                self.ui.text_tiny(78, 6, &v, DGREY);
            }
            update::State::Ready { version, .. } => {
                if !self.update_notified {
                    self.update_notified = true;
                    tray::notify("BitGG update ready", &format!("{version} is downloaded and verified. Open BitGG and click UPDATE to restart into it."));
                }
                let label = format!("UPDATE {}", version.to_uppercase());
                let w = label.len() as i32 * 8 + 8;
                let q = r(78, 1, w, 12);
                // solid pill so it can't be missed: green/yellow blink, white on hover
                let blink = (self.ui.frame / 8) % 2 == 0;
                let hov = q.has(self.ui.inp.mx, self.ui.inp.my);
                let bg = if hov { WHITE } else if blink { GREEN } else { YELLOW };
                self.ui.boxed(q, bg, BLACK);
                self.ui.text_c(q, &label, BLACK);
                if self.ui.clicked_in(q) {
                    self.update_requested = true;
                }
                // test hook: press the button as soon as it appears (after one shown frame)
                if std::env::var_os("BITGG_AUTO_APPLY").is_some() && self.ui.frame > 30 {
                    self.update_requested = true;
                }
                self.ui.tip(q, "RESTART TO UPDATE (SETTINGS ARE KEPT)");
            }
            _ => {}
        }
    }

    /// Swap in the verified exe, clean up, restart into it.
    fn apply_update(&mut self, el: &ActiveEventLoop) {
        let st = self.update.lock().unwrap().clone();
        let update::State::Ready { version, path } = st else { return };
        let exe = match update::swap_in(&path) {
            Ok(exe) => exe,
            Err(e) => {
                *self.update.lock().unwrap() = update::State::Error(e);
                return;
            }
        };
        self.shutdown(); // restore audio routing, devices, save config
        release_single_instance(); // let the new build start as the only instance
        match std::process::Command::new(&exe).spawn() {
            Ok(_) => el.exit(),
            Err(e) => {
                // keep running the old build rather than leave nothing running
                update::swap_back();
                let _ = std::process::Command::new(&exe).spawn();
                *self.update.lock().unwrap() = update::State::Error(format!("start {version}: {e}"));
                el.exit();
            }
        }
    }

    fn present(&mut self, force: bool) {
        if !force && self.ui.px == self.last_frame {
            return; // nothing changed on screen: no blit, no present
        }
        self.last_frame.clone_from(&self.ui.px);
        let (Some(win), Some(surf)) = (self.window.clone(), self.surface.as_mut()) else { return };
        let size = win.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surf.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surf.buffer_mut() else { return };
        self.map = self.ui.blit(&mut buf, w.get() as usize, h.get() as usize);
        let _ = buf.present();
    }
}

fn is_running(exe: &str) -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let n = String::from_utf16_lossy(&e.szExeFile[..len]);
            if n.trim().eq_ignore_ascii_case(exe) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        found
    }
}

#[allow(dead_code)]
trait NoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}
impl NoWindow for std::process::Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }
}

fn recent_clips(folder: &str) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut v: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "mp4").unwrap_or(false))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|x| x.1).collect()
}

impl ApplicationHandler<UserEv> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let s = if self.cfg.scale == 0 { 2 } else { self.cfg.scale };
        let attrs = Window::default_attributes()
            .with_title(concat!("BitGG v", env!("CARGO_PKG_VERSION")))
            .with_inner_size(LogicalSize::new((W as u32 * s) as f64, (H as u32 * s) as f64))
            .with_min_inner_size(LogicalSize::new(W as f64, H as f64))
            // main thread is MTA for Core Audio; OLE drag&drop would need STA
            .with_drag_and_drop(false);
        let attrs = match winit::window::Icon::from_resource(1, None) {
            Ok(i) => attrs.with_window_icon(Some(i.clone())).with_taskbar_icon(Some(i)),
            Err(_) => attrs,
        };
        let win = Rc::new(el.create_window(attrs).expect("window"));
        let ctx = softbuffer::Context::new(win.clone()).expect("softbuffer ctx");
        self.surface = Some(softbuffer::Surface::new(&ctx, win.clone()).expect("surface"));
        self.window = Some(win);
        if self.start_hidden {
            self.hide_to_tray();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEv) {
        dlog(&format!("user_event {ev:?} replay={}", self.rec.as_ref().map(|r| r.status.lock().unwrap().clone()).unwrap_or_default()));
        match ev {
            UserEv::Tray(tray::CMD_OPEN) => self.show_window(),
            UserEv::Tray(tray::CMD_SAVE) => self.save_clip(),
            UserEv::Tray(tray::CMD_REPLAY) => self.toggle_replay(),
            UserEv::Tray(tray::CMD_EXIT) => return self.quit(el),
            UserEv::Hotkey(1) => self.save_clip(),
            UserEv::Hotkey(2) => self.toggle_record(),
            UserEv::Hotkey(3) => self.toggle_replay(),
            _ => {}
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        let redraw = |s: &App| {
            if let Some(w) = &s.window {
                w.request_redraw();
            }
        };
        match ev {
            // X and minimize both tuck BitGG into the tray; Exit lives in the tray menu
            WindowEvent::CloseRequested => self.hide_to_tray(),
            WindowEvent::RedrawRequested => {
                self.draw();
                let force = std::mem::take(&mut self.force_present);
                self.present(force);
                if std::mem::take(&mut self.update_requested) {
                    self.apply_update(_el);
                }
            }
            WindowEvent::Resized(_) => {
                if self.window.as_ref().and_then(|w| w.is_minimized()).unwrap_or(false) {
                    self.hide_to_tray();
                } else {
                    self.force_present = true;
                    redraw(self)
                }
            }
            WindowEvent::Occluded(o) => self.visible = !o,
            WindowEvent::CursorMoved { position, .. } => {
                let (s, ox, oy) = self.map;
                self.ui.inp.mx = ((position.x as i32 - ox as i32) / s as i32).clamp(-1, W as i32);
                self.ui.inp.my = ((position.y as i32 - oy as i32) / s as i32).clamp(-1, H as i32);
                redraw(self);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let down = state == ElementState::Pressed;
                self.ui.inp.pressed = down;
                self.ui.inp.released = !down;
                self.ui.inp.down = down;
                redraw(self);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.ui.inp.wheel += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.signum() as i32,
                    MouseScrollDelta::PixelDelta(p) => (p.y.signum()) as i32,
                };
                redraw(self);
            }
            _ => {}
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) && self.visible {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(t) = self.dirty_since {
            if t.elapsed() > Duration::from_secs(2) {
                self.cfg.save();
                self.dirty_since = None;
            }
        }
        // Redraw on a timer only (see new_events): fast while something animates.
        let animating = self.cfg.audio.engine_on
            || self.ui.active != 0
            || self.rec.as_ref().map(|r| r.alive()).unwrap_or(false)
            || self.tab == Tab::Apps;
        let ms = if !self.visible { 2000 } else if animating { 66 } else { 500 };
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(ms)));
    }
}


fn write_bmp(path: &std::path::Path, px: &[u32], w: usize, h: usize, s: usize) {
    let (ow, oh) = (w * s, h * s);
    let mut f = Vec::with_capacity(54 + ow * oh * 4);
    let size = (54 + ow * oh * 4) as u32;
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&size.to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
    f.extend_from_slice(&(ow as i32).to_le_bytes());
    f.extend_from_slice(&(-(oh as i32)).to_le_bytes());
    f.extend_from_slice(&[1, 0, 32, 0, 0, 0, 0, 0]);
    f.extend_from_slice(&((ow * oh * 4) as u32).to_le_bytes());
    f.extend_from_slice(&[0; 16]);
    for y in 0..oh {
        for x in 0..ow {
            f.extend_from_slice(&px[(y / s) * w + x / s].to_le_bytes());
        }
    }
    let _ = std::fs::write(path, f);
}

/// Non-interactive check: devices, sessions, routing read-back, UI renders,
/// and an optional short screen+audio recording. Never starts the mixer.
fn selftest(out: &std::path::Path, rec_secs: u64) {
    let _ = std::fs::create_dir_all(out);
    let mut cfg = Config::load();
    cfg.audio.engine_on = false;
    cfg.clips.replay_on = false;
    let mut app = App::new(cfg);
    println!("== devices ({})", app.devs.len());
    for d in &app.devs {
        println!("  {} {:<70} {}", if d.render { "OUT" } else { "IN " }, d.name, d.id);
    }
    println!("== sessions ({})", app.sessions.len());
    for s in &app.sessions {
        println!("  pid {:>6} {:<24} vol {:.2} mute {} peak {:.3} route '{}'", s.pid, s.name, s.vol, s.mute, s.peak, app.routes.get(&s.pid).cloned().unwrap_or_default());
    }
    println!("== autodetect: out='{}' mic_in='{}' mic_out='{}'", app.dev_name(&app.cfg.audio.output), app.dev_name(&app.cfg.audio.mic.input), app.dev_name(&app.cfg.audio.mic.target));
    for i in 0..4 {
        println!("   {} <- {}", CH_NAMES[i], app.dev_name(&app.cfg.audio.ch[i].source));
    }
    println!("== monitors");
    for m in &app.monitors {
        println!("  {:?}", m);
    }
    app.cfg.audio.ch[0].eq = dsp::preset("FPS");
    app.eq_ch = 0;
    for (t, name) in TABS {
        app.tab = t;
        app.ui.inp.mx = -10;
        app.draw();
        write_bmp(&out.join(format!("tab_{}.bmp", name.to_lowercase())), &app.ui.px, W, H, 2);
    }
    println!("== ui shots written to {}", out.display());
    if rec_secs > 0 {
        let path = out.join("selftest_rec.mp4");
        let (lb, mic) = app.rec_sources();
        let t0 = Instant::now();
        match Recorder::start(&app.cfg.clips, Mode::Record(path.clone()), lb, mic, app.rec_levels.clone()) {
            Ok(mut r) => {
                std::thread::sleep(Duration::from_secs(rec_secs));
                println!("   status while running: {}", r.status.lock().unwrap());
                println!("   levels in recording: audio peak {:.3}  mic peak {:.3}", app.rec_levels.audio_level(), app.rec_levels.mic_level());
                r.stop();
                println!("   status after stop: {} ({:.1}s)", r.status.lock().unwrap(), t0.elapsed().as_secs_f32());
            }
            Err(e) => println!("   recorder error: {e}"),
        }
        println!("== recording {:?} {} bytes", path, std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
        // replay buffer: run, then save the ring
        let mut c = app.cfg.clips.clone();
        c.folder = out.display().to_string();
        c.replay_secs = 15;
        let (lb, mic) = app.rec_sources();
        let msg = Arc::new(Mutex::new(String::new()));
        match Recorder::start(&c, Mode::Replay, lb, mic, app.rec_levels.clone()) {
            Ok(mut r) => {
                std::thread::sleep(Duration::from_secs(rec_secs));
                println!("   replay status: {}  ring {:.1} MB", r.status.lock().unwrap(), r.ring_mb());
                let t = Instant::now();
                r.save_clip(c.folder.clone(), msg.clone());
                while msg.lock().unwrap().is_empty() && t.elapsed() < Duration::from_secs(10) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                println!("   save: {} in {} ms", msg.lock().unwrap(), t.elapsed().as_millis());
                r.stop();
            }
            Err(e) => println!("   replay error: {e}"),
        }
    }
}

/// Runs the real mixing engine for a few seconds but sends the mix and the
/// processed mic to the (inaudible) Steam Streaming virtual devices.
fn enginetest() {
    let mut b = dsp::Band { on: true, kind: dsp::FilterType::Peaking, freq: 1000.0, gain_db: 6.0, q: 1.0 };
    println!("== dsp: +6dB peak @1k -> {:.2} dB @1k, {:.2} dB @100", dsp::Coef::design(&b).mag_db(1000.0), dsp::Coef::design(&b).mag_db(100.0));
    b.kind = dsp::FilterType::HighPass;
    println!("        highpass @1k -> {:.2} dB @100", dsp::Coef::design(&b).mag_db(100.0));
    let devs = win_audio::list_devices();
    let find = |pat: &str, render: bool| devs.iter().find(|d| d.render == render && d.name.contains(pat)).map(|d| d.id.clone()).unwrap_or_default();
    let mut a = config::Audio::default();
    for i in 0..4 {
        a.ch[i].source = find(CH_DETECT[i], true);
    }
    // AUX <- tone we play into "Steam Streaming Speakers"; mix -> "Steam Streaming Microphone" speakers
    a.ch[3].source = find("Steam Streaming Speakers", true);
    a.output = find("Speakers (Steam Streaming Microphone)", true);
    a.mic.input = find("Arctis Nova 5", false);
    a.mic.target = String::new();
    let tone_dev = a.ch[3].source.clone();
    std::thread::spawn(move || {
        win_audio::com_init();
        let Ok(mut r) = wasapi::Render::new(&tone_dev, false, 100) else { return };
        let mut ph = 0f32;
        for _ in 0..450 {
            let n = r.free_frames().unwrap_or(0) as usize;
            let mut v = Vec::with_capacity(n * 2);
            for _ in 0..n {
                let x = (ph * std::f32::consts::TAU).sin() * 0.5;
                ph = (ph + 440.0 / 48000.0).fract();
                v.push(x);
                v.push(x);
            }
            let _ = r.write(&v);
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    a.ch[0].eq = dsp::preset("FPS");
    a.engine_on = true;
    println!("== engine: out={} mic_in={} mic_out={}", !a.output.is_empty(), !a.mic.input.is_empty(), !a.mic.target.is_empty());
    let sh = Shared::new(a.clone());
    let mut e = Engine::start(sh.clone());
    for t in 0..8 {
        std::thread::sleep(Duration::from_millis(500));
        let m: Vec<String> = (0..engine::METERS).map(|i| format!("{:.3}", sh.meter(i))).collect();
        println!("  t={:.1}s [{}] game/chat/media/aux/mic/master = {}", (t + 1) as f32 * 0.5, sh.status.lock().unwrap(), m.join(" "));
        if t == 3 {
            a.chatmix = -1.0; // hot param change must not rebuild
            a.ch[0].eq = dsp::preset("BASS");
            sh.update(&a);
        }
    }
    e.stop();
    println!("== engine stopped: {}", sh.status.lock().unwrap());
}

/// Child helper for --directtest: a silent stream on the default output so the
/// process shows up as an app session.
fn silent_tone(secs: u64) {
    let Some(id) = win_audio::default_id(true) else { return };
    let Ok(mut r) = wasapi::Render::new(&id, false, 100) else { return };
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        let n = r.free_frames().unwrap_or(0) as usize;
        let _ = r.write(&vec![0.0; n * 2]);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// DIRECT-mode check against a throwaway child process only.
fn directtest() {
    let tmp = std::env::temp_dir().join("bitgg-directtest");
    let _ = std::fs::create_dir_all(&tmp);
    std::env::set_var("APPDATA", &tmp);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap()).arg("--silent").spawn().expect("child");
    std::thread::sleep(Duration::from_millis(1500));
    let mut cfg = Config::default();
    cfg.audio.engine_on = false;
    let mut a = App::new(cfg);
    let vol = |a: &App| {
        let s = win_audio::list_sessions(&a.devs);
        s.iter().find(|x| x.pids.contains(&child.id())).map(|x| x.vol)
    };
    let name = a.sessions.iter().find(|x| x.pids.contains(&child.id())).map(|x| x.name.clone());
    println!("child session found: {:?}  vol={:?}", name, vol(&a));
    let Some(name) = name else { let _ = child.kill(); return };
    for i in 0..4 {
        println!("  ch{} live(DSP)={}", i, a.ch_live(i));
    }
    a.cfg.audio.apps.insert(name.clone(), config::AppCfg { ch: 3, vol: Some(0.8) }); // MEDIA
    a.set_engine(true);
    println!("engine ON, MEDIA 85% master 90%: vol={:?} expect {:.3}", vol(&a), 0.8 * 0.85 * 0.9);
    a.cfg.audio.ch[2].vol = 0.5;
    a.dirty();
    println!("MEDIA fader 50%:  vol={:?} expect {:.3}", vol(&a), 0.8 * 0.5 * 0.9);
    a.cfg.audio.ch[2].mute = true;
    a.dirty();
    println!("MEDIA MUTE:       vol={:?} expect 0", vol(&a));
    a.cfg.audio.ch[2].mute = false;
    a.cfg.audio.master = 1.0;
    a.dirty();
    println!("unmute, master 100%: vol={:?} expect {:.3}", vol(&a), 0.8 * 0.5);
    a.set_engine(false);
    println!("engine OFF (restore): vol={:?} expect 0.800", vol(&a));
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Does audio pass through the Sonar VADs without GG? Plays a tone into a VAD
/// render endpoint and reads it back the way BitGG does (loopback / mic pair).
fn vadtest() {
    let devs = win_audio::list_devices();
    let find = |pat: &str, render: bool| devs.iter().find(|d| d.render == render && d.name.contains(pat)).map(|d| d.id.clone());
    let tone = |id: String, secs: f32| {
        std::thread::spawn(move || {
            win_audio::com_init();
            let Ok(mut r) = wasapi::Render::new(&id, false, 100) else { return };
            let mut ph = 0f32;
            let t = Instant::now();
            while t.elapsed().as_secs_f32() < secs {
                let n = r.free_frames().unwrap_or(0) as usize;
                let mut v = Vec::with_capacity(n * 2);
                for _ in 0..n {
                    let x = (ph * std::f32::consts::TAU).sin() * 0.5;
                    ph = (ph + 440.0 / 48000.0).fract();
                    v.push(x);
                    v.push(x);
                }
                let _ = r.write(&v);
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let measure = |id: &str, loopback: bool| -> Option<f32> {
        let mut c = wasapi::Capture::new(id, loopback).ok()?;
        let mut q = std::collections::VecDeque::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(1500) {
            let _ = c.drain(&mut q);
            std::thread::sleep(Duration::from_millis(10));
        }
        if !c.raw_peaks.is_empty() {
            println!("    raw per-channel peaks: {:?}", c.raw_peaks.iter().map(|p| (p * 1000.0).round() / 1000.0).collect::<Vec<_>>());
        }
        Some(q.iter().fold(0.0f32, |m, x| m.max(x.abs())))
    };
    for ch in ["Sonar - Gaming", "Sonar - Media"] {
        match find(ch, true) {
            Some(id) => {
                let h = tone(id.clone(), 2.0);
                std::thread::sleep(Duration::from_millis(200));
                println!("{ch}: tone 0.5 -> loopback peak {:?}", measure(&id, true));
                let _ = h.join();
            }
            None => println!("{ch}: not active"),
        }
    }
    match (find("Sonar - Microphone", true), find("Sonar - Microphone", false)) {
        (Some(r), Some(c)) => {
            let h = tone(r, 2.0);
            std::thread::sleep(Duration::from_millis(200));
            println!("Sonar mic: tone 0.5 into render -> capture peak {:?}", measure(&c, false));
            let _ = h.join();
        }
        _ => println!("Sonar mic pair: not active"),
    }
}

/// README screenshots: renders tabs with a demo config, so no personal paths,
/// device or program names end up in the docs. CLIPS shows a real running
/// replay buffer (5 s, kept in RAM only, never saved).
fn screenshots(out: &std::path::Path) {
    const DEMO_DIR: &str = r"C:\Users\you\Videos\BitGG";
    let _ = std::fs::create_dir_all(out);
    let mut cfg = Config::default();
    cfg.clips.folder = DEMO_DIR.into();
    cfg.clips.save_key = "CTRL+ALT+S".into();
    cfg.clips.replay_secs = 60;
    cfg.clips.rec_audio_gain = 0.75;
    cfg.audio.ch[0].eq = dsp::preset("FPS");
    let mut app = App::new(cfg);
    *hotkey::ACTIVE_SAVE.lock().unwrap() = "CTRL+ALT+S".into();
    *hotkey::STATUS.lock().unwrap() = "CTRL+ALT+S OK".into();
    let demo_clips = ["clip_2026-10-23_21-14-05.mp4", "clip_2026-10-23_20-52-41.mp4"];
    let shoot = |app: &mut App, tab: Tab, name: &str| {
        app.tab = tab;
        app.clips = demo_clips.iter().map(|c| std::path::PathBuf::from(DEMO_DIR).join(c)).collect();
        app.slow_refresh = Instant::now();
        app.ui.inp.mx = -10;
        app.draw();
        write_bmp(&out.join(format!("{name}.bmp")), &app.ui.px, W, H, 2);
    };
    shoot(&mut app, Tab::Mixer, "mixer");
    app.eq_ch = 0;
    app.eq_band = 7;
    shoot(&mut app, Tab::Eq, "eq");
    app.start_replay();
    std::thread::sleep(Duration::from_secs(5));
    shoot(&mut app, Tab::Clips, "clips");
    app.rec = None;
    println!("screenshots written to {}", out.display());
}

/// DIRECT mode with an UNASSIGNED app: it must follow the GAME fader.
fn directtest_unassigned() {
    let tmp = std::env::temp_dir().join("bitgg-directtest2");
    let _ = std::fs::create_dir_all(&tmp);
    std::env::set_var("APPDATA", &tmp);
    // the APPS list hides "bitgg" itself, so run the tone child under another name
    let exe = tmp.join("tonechild.exe");
    let _ = std::fs::copy(std::env::current_exe().unwrap(), &exe);
    let mut child = std::process::Command::new(&exe).arg("--silent").spawn().expect("child");
    std::thread::sleep(Duration::from_millis(1500));
    let mut a = App::new(Config::default());
    // force every channel (and the mic) to DIRECT: no virtual devices
    for c in a.cfg.audio.ch.iter_mut() {
        c.source.clear();
    }
    a.cfg.audio.mic.target.clear();
    a.dirty();
    let vol = |a: &App| win_audio::list_sessions(&a.devs).iter().find(|x| x.pids.contains(&child.id())).map(|x| x.vol);
    println!("unassigned child, before engine: vol={:?}  engine thread={}", vol(&a), a.engine.is_some());
    a.set_engine(true);
    println!("engine ON (GAME 85%, master 90%): vol={:?} expect {:.3}", vol(&a), 0.85 * 0.9);
    a.cfg.audio.ch[0].vol = 0.5;
    a.dirty();
    println!("GAME fader 50%:  vol={:?} expect {:.3}", vol(&a), 0.5 * 0.9);
    a.cfg.audio.master_mute = true;
    a.dirty();
    println!("MASTER mute:     vol={:?} expect 0", vol(&a));
    a.cfg.audio.master_mute = false;
    a.set_engine(false);
    println!("engine OFF:      vol={:?} expect 1.000 (own volume back)", vol(&a));
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Child helper: very quiet 440 Hz tone on whatever is the default output now.
fn quiet_tone(secs: u64) {
    let Some(id) = win_audio::default_id(true) else { return };
    let Ok(mut r) = wasapi::Render::new(&id, false, 100) else { return };
    let mut ph = 0f32;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        let n = r.free_frames().unwrap_or(0) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for _ in 0..n {
            let x = (ph * std::f32::consts::TAU).sin() * 0.03;
            ph = (ph + 440.0 / 48000.0).fract();
            v.push(x);
            v.push(x);
        }
        let _ = r.write(&v);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// DSP mode end to end on the real config: unassigned apps must reach GAME
/// through the default device, and the GAME fader must change what you hear.
/// Does not save the config; restores Windows' defaults at the end.
fn dsptest() {
    let devs0 = win_audio::list_devices();
    let nm = |id: &str| devs0.iter().find(|d| d.id == id).map(|d| d.name.replace(" (SteelSeries Sonar Virtual Audio Device)", "")).unwrap_or(id.to_string());
    println!("before: default output = {}", nm(&win_audio::default_id(true).unwrap_or_default()));
    let mut a = App::new(Config::load());
    if !a.cfg.audio.engine_on {
        a.set_engine(true);
    }
    // test with GAME/MASTER audible regardless of the saved mute state (not saved)
    a.cfg.audio.ch[0].mute = false;
    a.cfg.audio.master_mute = false;
    a.dirty();
    a.dirty_since = None; // never write the config from a test
    // timeline: does GAME carry audio, and does the engine recover?
    let exe0 = std::env::temp_dir().join("tonechild0.exe");
    let _ = std::fs::copy(std::env::current_exe().unwrap(), &exe0);
    std::thread::sleep(Duration::from_millis(300));
    a.refresh();
    let mut child0 = std::process::Command::new(&exe0).arg("--quiet-tone").spawn().expect("child0");
    let gm = win_audio::endpoint_meter(&a.cfg.audio.ch[0].source);
    for k in 0..12 {
        std::thread::sleep(Duration::from_millis(500));
        let dev_pk = gm.as_ref().map(|m| unsafe { m.GetPeakValue().unwrap_or(-1.0) }).unwrap_or(-2.0);
        let child_dev = win_audio::list_sessions(&a.devs).iter().find(|s| s.pids.contains(&child0.id())).map(|s| nm(&s.device)).unwrap_or("no session".into());
        println!("  t={:>4.1}s  Windows meter on Gaming {:.3} | engine GAME {:.3} MASTER {:.3} [{}] | tone app on: {}", (k + 1) as f32 * 0.5, dev_pk, a.shared.meter(0), a.shared.meter(5), a.shared.status.lock().unwrap(), child_dev);
    }
    let _ = child0.kill();
    let _ = child0.wait();
    let _ = std::fs::remove_file(&exe0);
    std::thread::sleep(Duration::from_secs(1));
    a.refresh();
    println!("engine running: {}  GAME is DSP: {}", a.engine.is_some(), a.ch_live(0));
    println!("after start: default output = {}   BitGG output = {}", nm(&win_audio::default_id(true).unwrap_or_default()), nm(&a.cfg.audio.output));
    for s in a.sessions.iter().filter(|s| s.live_peak() > 0.0 || s.name.contains("stove")) {
        println!("  app {:<18} now plays on {}", s.name, nm(&s.device));
    }
    let hear = |id: &str| -> f32 {
        let Ok(mut c) = wasapi::Capture::new(id, true) else { return -1.0 };
        let mut q = std::collections::VecDeque::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            let _ = c.drain(&mut q);
            std::thread::sleep(Duration::from_millis(10));
        }
        q.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    };
    // an app started now opens the new default device -> must land in GAME
    let exe = std::env::temp_dir().join("tonechild.exe");
    let _ = std::fs::copy(std::env::current_exe().unwrap(), &exe);
    let mut child = std::process::Command::new(&exe).arg("--quiet-tone").spawn().expect("child");
    std::thread::sleep(Duration::from_millis(1500));
    a.refresh();
    if let Some(s) = a.sessions.iter().find(|s| s.pids.contains(&child.id())) {
        println!("  NEW app (started after engine on) plays on {}", nm(&s.device));
    }
    for s in &a.sessions {
        println!("  {:<18} needs restart: {}", if s.pids.contains(&child.id()) { "NEW test app" } else { s.name.as_str() }, a.needs_restart(s));
    }
    let out = a.cfg.audio.output.clone();
    let orig = a.cfg.audio.ch[0].vol;
    a.cfg.audio.ch[0].vol = 1.0;
    a.dirty();
    std::thread::sleep(Duration::from_millis(500));
    let full = hear(&out);
    println!("GAME 100%: GAME meter {:.3}, MASTER meter {:.3}, what you hear {:.3}", a.shared.meter(0), a.shared.meter(5), full);
    println!("engine status: {}", a.shared.status.lock().unwrap());
    a.cfg.audio.ch[0].vol = 0.2;
    a.dirty();
    std::thread::sleep(Duration::from_millis(500));
    let low = hear(&out);
    println!("GAME  20%: GAME meter {:.3}, what you hear {:.3}  (expected ~{:.3})", a.shared.meter(0), low, full * dsp::fader_to_lin(0.2));
    a.cfg.audio.ch[0].vol = orig;
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&exe);
    // clean shutdown, as on Exit
    a.apply_direct(false, true);
    if let Some(mut e) = a.engine.take() {
        e.stop();
    }
    let _ = a.apo_bypass(false);
    a.guard_defaults(true);
    println!("after stop: default output = {}", nm(&win_audio::default_id(true).unwrap_or_default()));
}

/// Does anything besides BitGG forward a VAD to a real output (e.g. Sonar's
/// APO "copy output device" left on by GG)? Plays a quiet 440 Hz tone into a
/// VAD and looks for exactly that frequency on every physical output.
fn apotest() {
    fn goertzel(x: &[f32], f: f32) -> f32 {
        // left channel only, magnitude at f (normalised by length)
        let w = 2.0 * std::f32::consts::PI * f / 48_000.0;
        let c = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        let n = x.len() / 2;
        for i in 0..n {
            let s0 = x[i * 2] + c * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0).sqrt() / n.max(1) as f32
    }
    let devs = win_audio::list_devices();
    let outs: Vec<_> = devs.iter().filter(|d| d.render && !win_audio::is_virtual(&d.name)).cloned().collect();
    let listen = |id: &str| -> f32 {
        let Ok(mut c) = wasapi::Capture::new(id, true) else { return -1.0 };
        let mut q = std::collections::VecDeque::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(1500) {
            let _ = c.drain(&mut q);
            std::thread::sleep(Duration::from_millis(10));
        }
        let v: Vec<f32> = q.into_iter().collect();
        goertzel(&v, 440.0)
    };
    for vad in ["Sonar - Gaming", "Sonar - Media"] {
        let Some(src) = devs.iter().find(|d| d.render && d.name.contains(vad)).map(|d| d.id.clone()) else { continue };
        println!("== tone into {vad}");
        for o in &outs {
            let base = listen(&o.id);
            let s2 = src.clone();
            let h = std::thread::spawn(move || {
                win_audio::com_init();
                let Ok(mut r) = wasapi::Render::new(&s2, false, 100) else { return };
                let mut ph = 0f32;
                let t = Instant::now();
                while t.elapsed() < Duration::from_millis(2000) {
                    let n = r.free_frames().unwrap_or(0) as usize;
                    let mut v = Vec::with_capacity(n * 2);
                    for _ in 0..n {
                        let x = (ph * std::f32::consts::TAU).sin() * 0.03;
                        ph = (ph + 440.0 / 48000.0).fract();
                        v.push(x);
                        v.push(x);
                    }
                    let _ = r.write(&v);
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            std::thread::sleep(Duration::from_millis(300));
            let with = listen(&o.id);
            let _ = h.join();
            let verdict = if with > base * 4.0 && with > 0.002 { "FORWARDED HERE" } else { "-" };
            println!("   {:<45} 440Hz without {:.4}  with tone {:.4}  {verdict}", o.name, base, with);
        }
    }
}

/// The engine alone on the Gaming VAD (enhancements off, settled first),
/// output to the silent Steam device. Restores the device setting at the end.
fn enginetest_vad() {
    let devs = win_audio::list_devices();
    let find = |pat: &str| devs.iter().find(|d| d.render && d.name.contains(pat)).map(|d| d.id.clone()).unwrap_or_default();
    let gaming = find("Sonar - Gaming");
    let orig = win_audio::enhancements_enabled(&gaming).unwrap_or(true);
    let orig_def = win_audio::default_id(true).unwrap_or_default();
    for (fx_on, as_default) in [(true, false), (false, false), (true, true), (false, true)] {
        win_audio::set_enhancements(&gaming, fx_on);
        win_audio::set_default_device(if as_default { &gaming } else { &orig_def });
        std::thread::sleep(Duration::from_millis(1500));
        let mut a = config::Audio::default();
        a.ch[0].source = gaming.clone();
        a.ch[0].vol = 1.0;
        a.master = 1.0;
        a.output = find("Speakers (Steam Streaming Microphone)");
        a.engine_on = true;
        let sh = Shared::new(a);
        let mut e = Engine::start(sh.clone());
        std::thread::sleep(Duration::from_millis(800));
        let g = gaming.clone();
        let h = std::thread::spawn(move || {
            win_audio::com_init();
            let Ok(mut r) = wasapi::Render::new(&g, false, 100) else { return };
            let mut ph = 0f32;
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(2500) {
                let n = r.free_frames().unwrap_or(0) as usize;
                let mut v = Vec::with_capacity(n * 2);
                for _ in 0..n {
                    let x = (ph * std::f32::consts::TAU).sin() * 0.3;
                    ph = (ph + 440.0 / 48000.0).fract();
                    v.push(x);
                    v.push(x);
                }
                let _ = r.write(&v);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let mut best = 0.0f32;
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(250));
            best = best.max(sh.meter(0));
        }
        let _ = h.join();
        println!("enhancements {:<3} | Gaming is default: {:<5} -> engine GAME meter peak {:.3} (tone 0.3)  [{}]", if fx_on { "on" } else { "off" }, as_default, best, sh.status.lock().unwrap());
        e.stop();
    }
    win_audio::set_enhancements(&gaming, orig);
    win_audio::set_default_device(&orig_def);
}

/// Does the VAD loopback ever deliver NaN / inf samples, and does the EQ survive them?
fn nantest() {
    let devs = win_audio::list_devices();
    let gaming = devs.iter().find(|d| d.render && d.name.contains("Sonar - Gaming")).map(|d| d.id.clone()).unwrap();
    let orig = win_audio::enhancements_enabled(&gaming).unwrap_or(true);
    for fx_on in [true, false] {
        win_audio::set_enhancements(&gaming, fx_on);
        std::thread::sleep(Duration::from_millis(1500));
        let Ok(mut c) = wasapi::Capture::new(&gaming, true) else { println!("open failed"); continue };
        let mut q = std::collections::VecDeque::new();
        let g = gaming.clone();
        let h = std::thread::spawn(move || {
            win_audio::com_init();
            let Ok(mut r) = wasapi::Render::new(&g, false, 100) else { return };
            let mut ph = 0f32;
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(3000) {
                let n = r.free_frames().unwrap_or(0) as usize;
                let mut v = Vec::with_capacity(n * 2);
                for _ in 0..n {
                    let x = (ph * std::f32::consts::TAU).sin() * 0.3;
                    ph = (ph + 440.0 / 48000.0).fract();
                    v.push(x);
                    v.push(x);
                }
                let _ = r.write(&v);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            let _ = c.drain(&mut q);
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = h.join();
        let v: Vec<f32> = q.into_iter().collect();
        let bad = v.iter().filter(|x| !x.is_finite()).count();
        let big = v.iter().filter(|x| x.is_finite() && x.abs() > 4.0).count();
        let mut eq = dsp::EqProc::default();
        eq.configure(&dsp::preset("BASS"));
        let mut w = v.clone();
        eq.process(&mut w);
        let bad_after = w.iter().filter(|x| !x.is_finite()).count();
        let pk = v.iter().filter(|x| x.is_finite()).fold(0.0f32, |m, x| m.max(x.abs()));
        println!("enhancements {:<3}: {} samples, peak {:.3}, NaN/inf {}, |x|>4 {}, after BASS EQ NaN/inf {}", if fx_on { "on" } else { "off" }, v.len(), pk, bad, big, bad_after);
    }
    win_audio::set_enhancements(&gaming, orig);
}

/// Which device change kills an already-open loopback capture?
fn staletest() {
    let devs = win_audio::list_devices();
    let gaming = devs.iter().find(|d| d.render && d.name.contains("Sonar - Gaming")).map(|d| d.id.clone()).unwrap();
    let orig_def = win_audio::default_id(true).unwrap_or_default();
    let tone_into = |id: String| {
        std::thread::spawn(move || {
            win_audio::com_init();
            let Ok(mut r) = wasapi::Render::new(&id, false, 100) else { return };
            let mut ph = 0f32;
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(1500) {
                let n = r.free_frames().unwrap_or(0) as usize;
                let mut v = Vec::with_capacity(n * 2);
                for _ in 0..n {
                    let x = (ph * std::f32::consts::TAU).sin() * 0.03;
                    ph = (ph + 440.0 / 48000.0).fract();
                    v.push(x);
                    v.push(x);
                }
                let _ = r.write(&v);
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    for (label, set_def, fx_off) in [("nothing", false, false), ("set default", true, false), ("enhancements off", false, true), ("both", true, true)] {
        let mut cap = wasapi::Capture::new(&gaming, true).expect("capture");
        let mut q = std::collections::VecDeque::new();
        if set_def {
            win_audio::set_default_device(&gaming);
        }
        if fx_off {
            win_audio::set_enhancements(&gaming, false);
        }
        std::thread::sleep(Duration::from_millis(600));
        let h = tone_into(gaming.clone());
        let t = Instant::now();
        let mut err = None;
        while t.elapsed() < Duration::from_millis(1500) {
            if let Err(e) = cap.drain(&mut q) {
                err = Some(e.message());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = h.join();
        let pk = q.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        println!("open capture, then {:<17} -> peak {:.4}  error: {:?}", label, pk, err);
        drop(cap);
        win_audio::set_enhancements(&gaming, true);
        win_audio::set_default_device(&orig_def);
        std::thread::sleep(Duration::from_millis(600));
    }
}

/// Read-only: where does each app's sound actually go right now?
fn diag() {
    let devs = win_audio::list_devices();
    let name = |id: &str| {
        if id.is_empty() {
            return "(Windows default)".to_string();
        }
        devs.iter().find(|d| d.id == id).map(|d| d.name.replace(" (SteelSeries Sonar Virtual Audio Device)", "")).unwrap_or(format!("? {id}"))
    };
    println!("default output: {}", name(&win_audio::default_id(true).unwrap_or_default()));
    println!("default mic:    {}", name(&win_audio::default_id(false).unwrap_or_default()));
    let sessions = win_audio::list_sessions(&devs);
    std::thread::sleep(Duration::from_millis(300));
    for s in &sessions {
        let pk = (0..10).map(|_| { std::thread::sleep(Duration::from_millis(50)); s.live_peak() }).fold(0.0f32, f32::max);
        println!("app {:<20} vol {:.2} mute {:<5} peak {:.3}  plays on: {:<40} pinned: {}", s.name, s.vol, s.mute, pk, name(&s.device),
            s.pids.iter().map(|p| name(&win_audio::get_app_output(*p))).collect::<Vec<_>>().join(", "));
    }
}

static SINGLE_INSTANCE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn release_single_instance() {
    let raw = SINGLE_INSTANCE.swap(0, std::sync::atomic::Ordering::AcqRel);
    if raw != 0 {
        unsafe {
            let h = windows::Win32::Foundation::HANDLE(raw as *mut _);
            let _ = windows::Win32::System::Threading::ReleaseMutex(h);
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
}

/// Test: check GitHub, download + verify, and (with --apply) swap the exe.
fn update_test(apply: bool) {
    println!("this build: v{}  (repo {})", update::current(), update::REPO);
    let st = update::check_and_fetch();
    println!("result: {st:?}");
    if let (true, update::State::Ready { path, .. }) = (apply, &st) {
        match update::swap_in(path) {
            Ok(exe) => println!("swapped in: {}", exe.display()),
            Err(e) => println!("swap failed: {e}"),
        }
    }
}

fn main() {
    win_audio::com_init();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s == "--directtest2").unwrap_or(false) {
        directtest_unassigned();
        return;
    }
    if args.get(1).map(|s| s == "--quiet-tone").unwrap_or(false) {
        quiet_tone(12);
        return;
    }
    if args.get(1).map(|s| s == "--dsptest").unwrap_or(false) {
        dsptest();
        return;
    }
    if args.get(1).map(|s| s == "--apotest").unwrap_or(false) {
        apotest();
        return;
    }
    if args.get(1).map(|s| s == "--enginetest-vad").unwrap_or(false) {
        enginetest_vad();
        return;
    }
    if args.get(1).map(|s| s == "--nantest").unwrap_or(false) {
        nantest();
        return;
    }
    if args.get(1).map(|s| s == "--staletest").unwrap_or(false) {
        staletest();
        return;
    }
    if args.get(1).map(|s| s == "--update-test").unwrap_or(false) {
        update_test(args.get(2).map(|s| s == "--apply").unwrap_or(false));
        return;
    }
    if args.get(1).map(|s| s == "--diag").unwrap_or(false) {
        diag();
        return;
    }
    if args.get(1).map(|s| s == "--screenshots").unwrap_or(false) {
        screenshots(std::path::Path::new(args.get(2).map(|s| s.as_str()).unwrap_or("screenshots")));
        return;
    }
    if args.get(1).map(|s| s == "--vads").unwrap_or(false) {
        // `--vads` lists; `--vads on|off` enables/disables like the SETUP button
        match args.get(2).map(|s| s.as_str()) {
            Some("fx-off") | Some("fx-on") => {
                for (id, name, st) in win_audio::sonar_vads() {
                    if st == 1 && (args.len() < 4 || name.contains(args[3].as_str())) {
                        println!("enhancements {} -> {} : {name}", args[2], win_audio::set_enhancements(&id, args[2] == "fx-on"));
                    }
                }
            }
            Some("on") | Some("off") => {
                let (n, t) = win_audio::set_sonar_vads(args[2] == "on");
                println!("changed {n}/{t}");
            }
            _ => {}
        }
        for (id, name, st) in win_audio::sonar_vads() {
            let v = win_audio::endpoint_volume(&id).map(|e| unsafe { (e.GetMasterVolumeLevelScalar().unwrap_or(-1.0), e.GetMute().map(|b| b.as_bool()).unwrap_or(false)) });
            let ch = if st == 1 {
                win_audio::device(&id).ok().and_then(|d| unsafe {
                    let c: windows::Win32::Media::Audio::IAudioClient = d.Activate(windows::Win32::System::Com::CLSCTX_ALL, None).ok()?;
                    let f = c.GetMixFormat().ok()?;
                    let r = ((*f).nChannels, (*f).nSamplesPerSec);
                    windows::Win32::System::Com::CoTaskMemFree(Some(f as *const _));
                    Some(r)
                })
            } else {
                None
            };
            println!("{st} vol={:?} fx={:?} mix={:?} {name}", v, win_audio::enhancements_enabled(&id), ch);
        }
        return;
    }
    if args.get(1).map(|s| s == "--set-default").unwrap_or(false) {
        for id in &args[2..] {
            println!("default <- {id}: {}", win_audio::set_default_device(id));
        }
        return;
    }
    if args.get(1).map(|s| s == "--guardtest").unwrap_or(false) {
        let tmp = std::env::temp_dir().join("bitgg-guardtest");
        let _ = std::fs::create_dir_all(&tmp);
        std::env::set_var("APPDATA", &tmp);
        let devs = win_audio::list_devices();
        let gaming = devs.iter().find(|d| d.render && d.name.contains("Sonar - Gaming")).map(|d| d.id.clone()).unwrap_or_default();
        println!("force default -> Gaming VAD: {}", win_audio::set_default_device(&gaming));
        println!("default now is Gaming: {}", win_audio::default_id(true).as_deref() == Some(gaming.as_str()));
        let mut cfg = Config::default();
        cfg.audio.engine_on = false;
        cfg.audio.output = args[2].clone();
        cfg.audio.mic.input = args[3].clone();
        let a = App::new(cfg);
        println!("after BitGG start (engine off): default output = arctis? {}", win_audio::default_id(true).as_deref() == Some(args[2].as_str()));
        println!("default mic = arctis? {}", win_audio::default_id(false).as_deref() == Some(args[3].as_str()));
        drop(a);
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    if args.get(1).map(|s| s == "--pickertest").unwrap_or(false) {
        let out = Arc::new(Mutex::new(None));
        picker::pick_folder(0, std::env::temp_dir().display().to_string(), out.clone());
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(100));
            if let Some(f) = out.lock().unwrap().take() {
                println!("picked: {f}");
                return;
            }
        }
        println!("no folder picked");
        return;
    }
    if args.get(1).map(|s| s == "--shortcuttest").unwrap_or(false) {
        println!("add_shortcuts made {}", shortcuts::add_shortcuts());
        println!("startup enabled before: {}", shortcuts::startup_enabled());
        println!("set_startup(true): {}", shortcuts::set_startup(true));
        println!("startup enabled now: {}", shortcuts::startup_enabled());
        println!("set_startup(false): {}", shortcuts::set_startup(false));
        println!("startup enabled after off: {}", shortcuts::startup_enabled());
        return;
    }
    if args.get(1).map(|s| s == "--mics").unwrap_or(false) {
        let devs = win_audio::list_devices();
        let def = win_audio::default_id(false).unwrap_or_default();
        for d in devs.iter().filter(|d| !d.render) {
            let r = wasapi::Capture::new(&d.id, false).map(|mut c| {
                let mut q = std::collections::VecDeque::new();
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs(2) {
                    let _ = c.drain(&mut q);
                    std::thread::sleep(Duration::from_millis(10));
                }
                q.iter().fold(0.0f32, |m, x| m.max(x.abs()))
            });
            match r {
                Ok(pk) => println!("{} {:<70} peak {:.4} ({:.0} dBFS)", if d.id == def { "*" } else { " " }, d.name, pk, 20.0 * pk.max(1e-6).log10()),
                Err(e) => println!("  {:<70} open failed {e}", d.name),
            }
        }
        return;
    }
    if args.get(1).map(|s| s == "--levels").unwrap_or(false) {
        // read-only: what would the recorder hear right now?
        let cfg = Config::load();
        let devs = win_audio::list_devices();
        let name = |id: &str| devs.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or(id.to_string());
        let out = win_audio::default_id(true).unwrap_or_default();
        let mic = if cfg.audio.mic.input.is_empty() { win_audio::default_id(false).unwrap_or_default() } else { cfg.audio.mic.input.clone() };
        for (label, id, lb) in [("output loopback", out, true), ("mic", mic, false)] {
            match wasapi::Capture::new(&id, lb) {
                Ok(mut c) => {
                    let mut q = std::collections::VecDeque::new();
                    let t = Instant::now();
                    while t.elapsed() < Duration::from_secs(3) {
                        let _ = c.drain(&mut q);
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    let pk = q.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                    let rms = (q.iter().map(|x| x * x).sum::<f32>() / q.len().max(1) as f32).sqrt();
                    let ev = win_audio::endpoint_volume(&id).map(|e| unsafe { (e.GetMasterVolumeLevelScalar().unwrap_or(-1.0), e.GetMute().map(|b| b.as_bool()).unwrap_or(false)) });
                    println!("{label}: {} | peak {:.3} ({:.1} dBFS) rms {:.4} | channels folded {} | device vol/mute {:?}", name(&id), pk, 20.0 * pk.max(1e-6).log10(), rms, c.raw_peaks.len(), ev);
                }
                Err(e) => println!("{label}: {} | OPEN FAILED {e}", name(&id)),
            }
        }
        return;
    }
    if args.get(1).map(|s| s == "--vadtest").unwrap_or(false) {
        vadtest();
        return;
    }
    if args.get(1).map(|s| s == "--silent").unwrap_or(false) {
        silent_tone(20);
        return;
    }
    if args.get(1).map(|s| s == "--directtest").unwrap_or(false) {
        directtest();
        return;
    }
    if args.get(1).map(|s| s == "--enginetest").unwrap_or(false) {
        enginetest();
        return;
    }
    if args.get(1).map(|s| s == "--selftest").unwrap_or(false) {
        let out = std::path::PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "selftest".into()));
        let secs = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        selftest(&out, secs);
        return;
    }
    // one BitGG at a time: a second launch (e.g. the shortcut) just reopens it
    unsafe {
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        let m = windows::Win32::System::Threading::CreateMutexW(None, true, windows::core::w!("BitGG-single-instance"));
        if m.is_ok() && GetLastError() == ERROR_ALREADY_EXISTS {
            tray::signal_existing();
            return;
        }
        if let Ok(h) = m {
            SINGLE_INSTANCE.store(h.0 as isize, std::sync::atomic::Ordering::Release);
        }
    }
    update::cleanup(); // remove BitGG.exe.old / .new left by a previous update
    let start_hidden = args.iter().any(|a| a == "--tray");
    let cfg = Config::load();
    let el = EventLoop::<UserEv>::with_user_event().build().expect("event loop");
    hotkey::spawn(el.create_proxy(), &cfg.clips.save_key, cfg.clips.kbd_hook);
    tray::spawn(el.create_proxy());
    let mut app = App::new(cfg);
    app.start_hidden = start_hidden;
    let _ = el.run_app(&mut app);
    tray::remove();
    app.cfg.save();
}

#[allow(dead_code)]
fn _unused(_: R) {}
