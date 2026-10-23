use crate::dsp::EqSettings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The four Sonar render channels (VAD endpoints "SteelSeries Sonar - Gaming/Chat/Media/Aux").
pub const CH_NAMES: [&str; 4] = ["GAME", "CHAT", "MEDIA", "AUX"];
/// Substrings used to auto-detect the installed Sonar virtual devices.
pub const CH_DETECT: [&str; 4] = ["Sonar - Gaming", "Sonar - Chat", "Sonar - Media", "Sonar - Aux"];
pub const MIC_DETECT: &str = "Sonar - Microphone";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    /// Endpoint id of the virtual render device feeding this channel.
    pub source: String,
    pub vol: f32,
    pub mute: bool,
    pub eq: EqSettings,
}

impl Default for Channel {
    fn default() -> Self {
        Channel { source: String::new(), vol: 0.85, mute: false, eq: EqSettings::default() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mic {
    /// Physical microphone (capture endpoint id).
    pub input: String,
    /// Render side of the virtual mic ("Sonar - Microphone" render endpoint).
    pub target: String,
    pub vol: f32,
    pub mute: bool,
    pub gate_on: bool,
    pub gate_db: f32,
    pub eq: EqSettings,
}

impl Default for Mic {
    fn default() -> Self {
        Mic {
            input: String::new(),
            target: String::new(),
            vol: 0.9,
            mute: false,
            gate_on: true,
            gate_db: -45.0,
            eq: EqSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Audio {
    pub engine_on: bool,
    /// Physical output (headphones) the mix is sent to.
    pub output: String,
    pub master: f32,
    pub master_mute: bool,
    pub chatmix: f32,
    pub ch: [Channel; 4],
    pub mic: Mic,
    /// Per-program channel (0 = default, 1..4 = GAME/CHAT/MEDIA/AUX) and base volume.
    #[serde(default)]
    pub apps: BTreeMap<String, AppCfg>,
    /// Re-enable Sonar's virtual devices at startup when GG isn't running.
    #[serde(default)]
    pub auto_vads: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppCfg {
    pub ch: u8,
    /// The app's own volume; DIRECT channels scale this (never lose it).
    pub vol: Option<f32>,
}

impl Default for Audio {
    fn default() -> Self {
        Audio {
            engine_on: false,
            output: String::new(),
            master: 0.9,
            master_mute: false,
            chatmix: 0.0,
            ch: Default::default(),
            mic: Mic::default(),
            apps: BTreeMap::new(),
            auto_vads: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clips {
    pub replay_on: bool,
    pub replay_secs: u32,
    pub fps: u32,
    /// Output height (0 = native).
    pub height: u32,
    pub bitrate_kbps: u32,
    pub encoder: String,
    pub monitor: u32,
    pub record_mic: bool,
    pub folder: String,
    pub ffmpeg: String,
    #[serde(default = "default_save_key")]
    pub save_key: String,
    /// Sound recorded into clips: "" = AUTO (what you hear), else a render endpoint id.
    #[serde(default)]
    pub rec_audio: String,
    /// Mic recorded into clips: "" = AUTO (Windows default mic), "-" = none, else a capture id.
    #[serde(default)]
    pub rec_mic: String,
    /// Slider 0..1, 0.5 = 0 dB, range -12..+12 dB (bottom = silent).
    #[serde(default = "half")]
    pub rec_audio_gain: f32,
    #[serde(default = "half")]
    pub rec_mic_gain: f32,
    /// Opt-in keyboard-hook fallback for hotkeys (off by default).
    #[serde(default)]
    pub kbd_hook: bool,
}

fn half() -> f32 {
    0.5
}

/// Recording gain slider -> linear gain.
pub fn slider_gain(v: f32) -> f32 {
    if v < 0.02 {
        0.0
    } else {
        10f32.powf((v - 0.5) * 24.0 / 20.0)
    }
}

fn default_save_key() -> String {
    "ALT+F10".into()
}

impl Default for Clips {
    fn default() -> Self {
        let folder = std::env::var("USERPROFILE")
            .map(|p| format!("{p}\\Videos\\BitGG"))
            .unwrap_or_else(|_| "C:\\BitGG".into());
        Clips {
            replay_on: false,
            replay_secs: 30,
            fps: 60,
            height: 1080,
            bitrate_kbps: 15_000,
            encoder: "h264_nvenc".into(),
            monitor: 0,
            record_mic: true,
            folder,
            ffmpeg: String::new(),
            save_key: default_save_key(),
            rec_audio: String::new(),
            rec_mic: String::new(),
            rec_audio_gain: 0.5,
            rec_mic_gain: 0.5,
            kbd_hook: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub audio: Audio,
    pub clips: Clips,
    #[serde(default)]
    pub scale: u32,
    /// true = never contact GitHub for updates (default: check)
    #[serde(default)]
    pub update_check_off: bool,
}

pub fn dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("BitGG")
}

impl Config {
    pub fn load() -> Config {
        std::fs::read_to_string(dir().join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        let d = dir();
        let _ = std::fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(d.join("config.json"), s);
        }
    }
}
