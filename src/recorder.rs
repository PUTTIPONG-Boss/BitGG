//! Moments-style clip recorder, fully native (no ffmpeg).
//!
//! Observed in gsdk.dll / SteelSeriesCaptureSvc.exe: duplicator_monitor_capture
//! -> ffmpeg_h264_encoder (h264_nvenc / h264_amf / h264_qsv) + mf_aac_encoder
//! -> clip buffer -> ffmpeg_muxer ("Write clip buffer").
//!
//! Here: DXGI duplication -> GPU NV12 -> hardware H.264 MFT (reads the GPU
//! texture directly) + WASAPI loopback/mic -> AAC MFT. Compressed packets go
//! into a RAM ring (replay) and/or straight into an MP4 (record). Saving a
//! clip is a pass-through mux of the ring: no re-encode, near-instant.

use crate::config::Clips;
use crate::dxgi::{self, Grabber};
use crate::mf::{self, AInfo, AudioEnc, Mp4, Packet, VInfo, VideoEnc, A_RATE};
use crate::wasapi::{Capture, Jitter};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const SEC: i64 = 10_000_000;

/// Live recording-audio controls shared with the UI: gains are read every
/// 10 ms by the audio thread (no restart needed), peaks feed the level bars.
#[derive(Default)]
pub struct RecLevels {
    pub audio_gain: std::sync::atomic::AtomicU32,
    pub mic_gain: std::sync::atomic::AtomicU32,
    pub audio_peak: std::sync::atomic::AtomicU32,
    pub mic_peak: std::sync::atomic::AtomicU32,
}

impl RecLevels {
    pub fn set_gains(&self, audio: f32, mic: f32) {
        self.audio_gain.store(audio.to_bits(), Ordering::Relaxed);
        self.mic_gain.store(mic.to_bits(), Ordering::Relaxed);
    }
    fn get(a: &std::sync::atomic::AtomicU32) -> f32 {
        f32::from_bits(a.load(Ordering::Relaxed))
    }
    pub fn audio_level(&self) -> f32 {
        Self::get(&self.audio_peak)
    }
    pub fn mic_level(&self) -> f32 {
        Self::get(&self.mic_peak)
    }
    fn meter(a: &std::sync::atomic::AtomicU32, v: f32) {
        let old = Self::get(a);
        let n = if v > old { v } else { old * 0.9 + v * 0.1 };
        a.store(n.to_bits(), Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Replay,
    Record(PathBuf),
}

#[derive(Default)]
struct Sink {
    ring: bool,
    keep: i64,
    vring: VecDeque<Packet>,
    aring: VecDeque<Packet>,
    /// audio waiting for the record-mode MP4 (owned by the video thread)
    a_pending: Vec<Packet>,
    vinfo: Option<VInfo>,
    ainfo: Option<AInfo>,
    ring_bytes: usize,
}

impl Sink {
    fn push_video(&mut self, p: Packet) {
        if !self.ring {
            return;
        }
        self.ring_bytes += p.data.len();
        let newest = p.pts;
        self.vring.push_back(p);
        let cutoff = newest - self.keep;
        // drop whole GOPs that are entirely older than the window
        loop {
            let next_key = self.vring.iter().skip(1).position(|x| x.key).map(|i| i + 1);
            match next_key {
                Some(k) if self.vring[k].pts <= cutoff => {
                    for x in self.vring.drain(..k) {
                        self.ring_bytes -= x.data.len();
                    }
                }
                _ => break,
            }
        }
        // hard cap if the encoder never emits keyframes
        while self.vring.front().map(|x| x.pts < cutoff - 10 * SEC).unwrap_or(false) {
            let x = self.vring.pop_front().unwrap();
            self.ring_bytes -= x.data.len();
        }
        let vfront = self.vring.front().map(|x| x.pts).unwrap_or(newest);
        while self.aring.front().map(|x| x.pts < vfront - SEC).unwrap_or(false) {
            let x = self.aring.pop_front().unwrap();
            self.ring_bytes -= x.data.len();
        }
    }

    fn push_audio(&mut self, p: Packet, record: bool) {
        if record {
            self.a_pending.push(p.clone());
        }
        if self.ring {
            self.ring_bytes += p.data.len();
            self.aring.push_back(p);
        }
    }
}

pub struct Recorder {
    pub mode: Mode,
    stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<String>>,
    sink: Arc<Mutex<Sink>>,
    thread: Option<JoinHandle<()>>,
    cfg: Clips,
    pub started: Instant,
}

pub fn timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

impl Recorder {
    pub fn start(cfg: &Clips, mode: Mode, loop_dev: Option<String>, mic_dev: Option<String>, levels: Arc<RecLevels>) -> Result<Recorder, String> {
        let mons = dxgi::monitors();
        let mon = mons.get(cfg.monitor as usize).or(mons.first()).cloned().ok_or("no monitor found")?;
        if let Mode::Record(p) = &mode {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("STARTING...".to_string()));
        let sink = Arc::new(Mutex::new(Sink {
            ring: mode == Mode::Replay,
            keep: cfg.replay_secs as i64 * SEC + SEC,
            ..Default::default()
        }));
        let (c2, m2, s2, st2, sk2, lv2) = (cfg.clone(), mode.clone(), stop.clone(), status.clone(), sink.clone(), levels);
        let thread = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                crate::win_audio::com_init();
                if let Err(e) = video_main(&c2, &m2, &mon, loop_dev, mic_dev, &s2, &st2, &sk2, lv2) {
                    *st2.lock().unwrap() = format!("ERR {e}");
                }
                s2.store(true, Ordering::Release);
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { mode, stop, status, sink, thread: Some(thread), cfg: cfg.clone(), started: Instant::now() })
    }

    pub fn alive(&self) -> bool {
        self.thread.as_ref().map(|t| !t.is_finished()).unwrap_or(false)
    }

    pub fn ring_mb(&self) -> f32 {
        self.sink.lock().unwrap().ring_bytes as f32 / 1_048_576.0
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Mux the newest `replay_secs` from RAM into an MP4 on a worker thread.
    pub fn save_clip(&self, folder: String, out_status: Arc<Mutex<String>>) {
        let (v, a, vi, ai) = {
            let s = self.sink.lock().unwrap();
            let newest = s.vring.back().map(|p| p.pts).unwrap_or(0);
            let want = newest - self.cfg.replay_secs as i64 * SEC;
            // last keyframe at/before the window start, else the first one
            let start = s
                .vring
                .iter()
                .enumerate()
                .filter(|(_, p)| p.key && p.pts <= want)
                .map(|(i, _)| i)
                .last()
                .or_else(|| s.vring.iter().position(|p| p.key));
            let v: Vec<Packet> = match start {
                Some(i) => s.vring.iter().skip(i).cloned().collect(),
                None => Vec::new(),
            };
            let t0 = v.first().map(|p| p.pts).unwrap_or(0);
            let a: Vec<Packet> = s.aring.iter().filter(|p| p.pts >= t0).cloned().collect();
            (v, a, s.vinfo.clone(), s.ainfo.clone())
        };
        std::thread::spawn(move || {
            crate::win_audio::com_init();
            let r = (|| -> Result<PathBuf, String> {
                let (vi, ai) = (vi.ok_or("no video yet")?, ai.ok_or("no audio yet")?);
                if v.is_empty() {
                    return Err("buffer empty".into());
                }
                let _ = std::fs::create_dir_all(&folder);
                let out = PathBuf::from(&folder).join(format!("clip_{}.mp4", timestamp()));
                write_mp4(&out, &vi, &ai, &v, &a).map_err(|e| e.to_string())?;
                Ok(out)
            })();
            crate::dlog(&format!("save result: {:?}", r.as_ref().map(|p| p.display().to_string())));
            *out_status.lock().unwrap() = match r {
                Ok(p) => {
                    crate::beep();
                    format!("SAVED {}", p.file_name().unwrap_or_default().to_string_lossy())
                }
                Err(e) => format!("SAVE FAILED: {e}"),
            };
        });
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_mp4(path: &Path, vi: &VInfo, ai: &AInfo, v: &[Packet], a: &[Packet]) -> windows::core::Result<()> {
    let mut m = Mp4::create(path, vi, ai)?;
    m.set_t0(v[0].pts);
    // interleave by timestamp
    let (mut i, mut j) = (0, 0);
    while i < v.len() || j < a.len() {
        let take_v = j >= a.len() || (i < v.len() && v[i].pts <= a[j].pts);
        if take_v {
            m.write(true, &v[i])?;
            i += 1;
        } else {
            m.write(false, &a[j])?;
            j += 1;
        }
    }
    m.finish()
}

#[allow(clippy::too_many_arguments)]
fn video_main(
    cfg: &Clips,
    mode: &Mode,
    mon: &dxgi::Monitor,
    loop_dev: Option<String>,
    mic_dev: Option<String>,
    stop: &Arc<AtomicBool>,
    status: &Arc<Mutex<String>>,
    sink: &Arc<Mutex<Sink>>,
    levels: Arc<RecLevels>,
) -> Result<(), String> {
    let fps = cfg.fps.clamp(10, 144);
    let mut g = Grabber::new(mon, cfg.height, fps).map_err(|e| format!("capture: {e}"))?;
    let (w, h) = (g.out_w, g.out_h);
    let bitrate = cfg.bitrate_kbps.max(1000) * 1000;
    let mut enc = VideoEnc::new(g.device(), w, h, fps, bitrate, &cfg.encoder).map_err(|e| format!("h264 encoder: {e}"))?;
    let record = matches!(mode, Mode::Record(_));
    *status.lock().unwrap() = format!(
        "{} {w}x{h}@{fps} {}{}",
        if record { "REC" } else { "REPLAY" },
        short_name(&enc.name),
        if enc.hw_textures { " GPU" } else { " CPU" }
    );

    let start = Instant::now();
    let (stop_a, status_a, sink_a) = (stop.clone(), status.clone(), sink.clone());
    let audio = std::thread::Builder::new()
        .name("rec-audio".into())
        .spawn(move || {
            crate::win_audio::com_init();
            if let Err(e) = audio_main(start, loop_dev, mic_dev, &stop_a, &sink_a, record, &levels) {
                *status_a.lock().unwrap() = format!("AUDIO ERR {e}");
            }
        })
        .map_err(|e| e.to_string())?;

    let dt = SEC / fps as i64;
    let mut n: i64 = 0;
    let mut pk: Vec<Packet> = Vec::new();
    let mut cpu: Vec<u8> = Vec::new();
    let mut mp4: Option<Mp4> = None;
    let mut early: Vec<Packet> = Vec::new(); // record mode: video before the file exists
    let mut have_frame = false;

    let res: Result<(), String> = 'outer: loop {
        if stop.load(Ordering::Acquire) {
            break Ok(());
        }
        let due = Duration::from_nanos((n * dt * 100) as u64);
        let el = start.elapsed();
        let wait = due.saturating_sub(el).as_millis() as u32;
        match g.grab(wait.max(1)) {
            Ok(f) => have_frame |= f,
            Err(e) => break Err(format!("grab: {e}")),
        }
        let el = start.elapsed();
        if el < due {
            continue;
        }
        // fell far behind (e.g. long stall): skip ahead instead of bursting
        let behind = (el.as_nanos() as i64 / 100) / dt - n;
        if behind > 5 {
            n += behind;
        }
        if !have_frame {
            n += 1;
            continue;
        }
        let pts = n * dt;
        if n % (fps as i64 * 2) == 0 {
            enc.force_keyframe();
        }
        let sample = unsafe {
            if enc.hw_textures {
                match g.convert() {
                    Ok(t) => VideoEnc::texture_sample(&t, pts, dt),
                    Err(e) => break Err(format!("convert: {e}")),
                }
            } else {
                if let Err(e) = g.read_nv12(&mut cpu) {
                    break Err(format!("readback: {e}"));
                }
                mf::mem_sample(&cpu, pts, dt, false)
            }
        };
        let sample = match sample {
            Ok(s) => s,
            Err(e) => break Err(format!("sample: {e}")),
        };
        pk.clear();
        if let Err(e) = enc.encode(&sample, &mut pk) {
            break Err(format!("encode: {e}"));
        }
        n += 1;

        let mut s = sink.lock().unwrap();
        if s.vinfo.is_none() {
            if let Some(hdr) = &enc.hdr {
                s.vinfo = Some(VInfo { w, h, fps, bitrate, hdr: hdr.clone() });
            }
        }
        for p in pk.drain(..) {
            if record {
                early.push(p.clone());
            }
            s.push_video(p);
        }
        if let Mode::Record(path) = mode {
            if mp4.is_none() {
                if let (Some(vi), Some(ai)) = (s.vinfo.clone(), s.ainfo.clone()) {
                    // start the file at the first keyframe
                    if let Some(k) = early.iter().position(|p| p.key) {
                        early.drain(..k);
                        match Mp4::create(path, &vi, &ai) {
                            Ok(mut m) => {
                                m.set_t0(early[0].pts);
                                mp4 = Some(m);
                            }
                            Err(e) => break 'outer Err(format!("mp4: {e}")),
                        }
                    }
                }
            }
            if let Some(m) = mp4.as_mut() {
                let t0 = early.first().map(|p| p.pts).unwrap_or(0);
                for p in early.drain(..) {
                    if let Err(e) = m.write(true, &p) {
                        break 'outer Err(format!("mux: {e}"));
                    }
                }
                for p in s.a_pending.drain(..) {
                    if p.pts >= t0 - SEC / 2 {
                        let _ = m.write(false, &p);
                    }
                }
            } else {
                // keep pending audio bounded until the file opens
                let len = s.a_pending.len();
                if len > 500 {
                    s.a_pending.drain(..len - 500);
                }
            }
        }
    };
    stop.store(true, Ordering::Release);
    let _ = audio.join();
    if let Some(m) = mp4 {
        let r = m.finish();
        if let Mode::Record(p) = mode {
            *status.lock().unwrap() = match r {
                Ok(()) => {
                    crate::beep();
                    format!("SAVED {}", p.file_name().unwrap_or_default().to_string_lossy())
                }
                Err(e) => format!("ERR finalize {e}"),
            };
        }
    }
    res
}

fn short_name(n: &str) -> String {
    let l = n.to_lowercase();
    if l.contains("nvidia") {
        "NVENC".into()
    } else if l.contains("intel") || l.contains("quick") {
        "QSV".into()
    } else if l.contains("amd") {
        "AMF".into()
    } else {
        "SW H264".into()
    }
}

fn audio_main(
    start: Instant,
    loop_dev: Option<String>,
    mic_dev: Option<String>,
    stop: &AtomicBool,
    sink: &Mutex<Sink>,
    record: bool,
    levels: &RecLevels,
) -> Result<(), String> {
    let mut enc = AudioEnc::new(20_000).map_err(|e| format!("aac: {e}"))?; // 160 kbps
    sink.lock().unwrap().ainfo = Some(AInfo { user_data: enc.user_data.clone(), avg_bytes: enc.avg_bytes });
    let mut lb = loop_dev.as_deref().and_then(|d| Capture::new(d, true).ok());
    let mut mic = mic_dev.as_deref().and_then(|d| Capture::new(d, false).ok());
    let mut lj = Jitter::new(40, 250);
    let mut mj = Jitter::new(40, 250);
    let mut written: u64 = 0; // frames
    let mut buf: Vec<f32> = Vec::new();
    let mut tmp: Vec<f32> = Vec::new();
    let mut pcm: Vec<i16> = Vec::new();
    let mut pk: Vec<Packet> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
        if let Some(c) = lb.as_mut() {
            let _ = c.drain(&mut lj.q);
        }
        if let Some(c) = mic.as_mut() {
            let _ = c.drain(&mut mj.q);
        }
        let due = (start.elapsed().as_secs_f64() * A_RATE as f64) as u64;
        if due <= written {
            continue;
        }
        let frames = (due - written) as usize;
        buf.clear();
        buf.resize(frames * 2, 0.0);
        lj.pull(&mut buf);
        let ga = RecLevels::get(&levels.audio_gain);
        for b in buf.iter_mut() {
            *b *= ga;
        }
        RecLevels::meter(&levels.audio_peak, crate::dsp::peak(&buf));
        if mic.is_some() {
            tmp.clear();
            tmp.resize(frames * 2, 0.0);
            mj.pull(&mut tmp);
            let gm = RecLevels::get(&levels.mic_gain);
            for m in tmp.iter_mut() {
                *m *= gm;
            }
            RecLevels::meter(&levels.mic_peak, crate::dsp::peak(&tmp));
            for (b, m) in buf.iter_mut().zip(&tmp) {
                *b += *m;
            }
        } else {
            RecLevels::meter(&levels.mic_peak, 0.0);
        }
        // soft limiter: boosted game + mic must never hard-clip
        for b in buf.iter_mut() {
            let a = b.abs();
            if a > 0.9 {
                *b = b.signum() * (0.9 + 0.1 * ((a - 0.9) / 0.1).tanh());
            }
        }
        pcm.clear();
        pcm.extend(buf.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        let pts = written as i64 * SEC / A_RATE as i64;
        pk.clear();
        enc.encode(&pcm, pts, &mut pk).map_err(|e| format!("aac encode: {e}"))?;
        written = due;
        if !pk.is_empty() {
            let mut s = sink.lock().unwrap();
            for p in pk.drain(..) {
                s.push_audio(p, record);
            }
        }
    }
    Ok(())
}
