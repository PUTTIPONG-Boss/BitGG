//! The Sonar-style mixing engine, user-mode only.
//!
//! Sonar (observed): apps play into VAD render endpoints Game/Chat/Media/Aux,
//! the APO applies per-channel DSP and "redirects" (SetCopyOutputDevice) to the
//! physical headset; the processed mic is written into the VAD mic, which
//! loops back to the "Sonar - Microphone" capture endpoint.
//!
//! Here: WASAPI loopback of each VAD render endpoint -> EQ/gain/ChatMix ->
//! mix -> physical output; physical mic -> gate/EQ/gain -> VAD mic render.

use crate::config::Audio;
use crate::dsp::{self, EqProc, Gate};
use crate::wasapi::{Capture, Jitter, Render};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub const METERS: usize = 6; // game chat media aux mic master

pub struct Shared {
    pub params: Mutex<Audio>,
    pub version: AtomicU64,
    pub meters: [AtomicU32; METERS],
    pub status: Mutex<String>,
    pub stop: AtomicBool,
}

impl Shared {
    pub fn new(a: Audio) -> Arc<Shared> {
        Arc::new(Shared {
            params: Mutex::new(a),
            version: AtomicU64::new(1),
            meters: Default::default(),
            status: Mutex::new("ENGINE OFF".into()),
            stop: AtomicBool::new(false),
        })
    }
    pub fn update(&self, a: &Audio) {
        *self.params.lock().unwrap() = a.clone();
        self.version.fetch_add(1, Ordering::Release);
    }
    pub fn meter(&self, i: usize) -> f32 {
        f32::from_bits(self.meters[i].load(Ordering::Relaxed))
    }
    fn set_meter(&self, i: usize, v: f32) {
        // fall-off so the UI sees peaks
        let old = self.meter(i);
        let v = if v > old { v } else { old * 0.85 + v * 0.15 };
        self.meters[i].store(v.to_bits(), Ordering::Relaxed);
    }
    fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap() = s.into();
    }
}

pub struct Engine {
    pub shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(shared: Arc<Shared>) -> Engine {
        shared.stop.store(false, Ordering::Release);
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("audio-engine".into())
            .spawn(move || run(s))
            .ok();
        Engine { shared, thread }
    }
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.shared.set_status("ENGINE OFF");
        for m in &self.shared.meters {
            m.store(0, Ordering::Relaxed);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Chan {
    cap: Option<Capture>,
    jit: Jitter,
    eq: EqProc,
    gain: f32,
}

fn run(sh: Arc<Shared>) {
    crate::win_audio::com_init();
    // Retry loop: devices come and go (headset unplugged, etc.).
    while !sh.stop.load(Ordering::Acquire) {
        match session(&sh) {
            Ok(true) => continue, // routing changed: rebuild right away
            Ok(false) => {}
            Err(e) => sh.set_status(format!("ERR {e}")),
        }
        for _ in 0..20 {
            if sh.stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn session(sh: &Shared) -> windows::core::Result<bool> {
    let mut p = sh.params.lock().unwrap().clone();
    let mut seen = sh.version.load(Ordering::Acquire);
    if p.output.is_empty() {
        sh.set_status("PICK AN OUTPUT DEVICE IN SETUP");
        return Ok(false);
    }
    let mut out = Render::new(&p.output, true, 20)?;
    let mut chans: Vec<Chan> = p
        .ch
        .iter()
        .map(|c| Chan {
            cap: if c.source.is_empty() || c.source == p.output {
                None
            } else {
                Capture::new(&c.source, true).ok()
            },
            jit: Jitter::new(30, 120),
            eq: EqProc::default(),
            gain: 0.0,
        })
        .collect();
    for (c, cfg) in chans.iter_mut().zip(&p.ch) {
        c.eq.configure(&cfg.eq);
    }
    // mic chain
    let mut mic_cap = if p.mic.input.is_empty() { None } else { Capture::new(&p.mic.input, false).ok() };
    let mut mic_out = if p.mic.target.is_empty() { None } else { Render::new(&p.mic.target, false, 80).ok() };
    let mut mic_jit = Jitter::new(20, 100);
    let mut mic_eq = EqProc::default();
    mic_eq.configure(&p.mic.eq);
    let mut gate = Gate::default();
    let mut mic_gain = 0.0f32;
    let mut master_gain = 0.0f32;

    let live = chans.iter().filter(|c| c.cap.is_some()).count();
    sh.set_status(format!(
        "RUNNING  {live}/4 CH  MIC:{}",
        if mic_cap.is_some() && mic_out.is_some() { "ON" } else { "OFF" }
    ));

    let mut mix: Vec<f32> = Vec::with_capacity(4096);
    let mut tmp: Vec<f32> = Vec::with_capacity(4096);
    let mut mic_buf: Vec<f32> = Vec::with_capacity(8192);

    while !sh.stop.load(Ordering::Acquire) {
        out.wait(50);

        // hot-reload parameters
        let v = sh.version.load(Ordering::Acquire);
        if v != seen {
            seen = v;
            let np = sh.params.lock().unwrap().clone();
            let routing_changed = np.output != p.output
                || np.mic.input != p.mic.input
                || np.mic.target != p.mic.target
                || np.ch.iter().zip(&p.ch).any(|(a, b)| a.source != b.source);
            if routing_changed || !np.engine_on {
                return Ok(true); // rebuild streams
            }
            for (c, cfg) in chans.iter_mut().zip(&np.ch) {
                c.eq.configure(&cfg.eq);
            }
            mic_eq.configure(&np.mic.eq);
            p = np;
        }

        // ---- capture side
        for c in chans.iter_mut() {
            if let Some(cap) = c.cap.as_mut() {
                if cap.drain(&mut c.jit.q).is_err() {
                    return Ok(false);
                }
            }
        }

        // ---- render (headset) side
        let n = out.free_frames()? as usize * 2;
        if n > 0 {
            mix.clear();
            mix.resize(n, 0.0);
            let (cm_game, cm_chat) = dsp::chatmix(p.chatmix);
            for (i, (c, cfg)) in chans.iter_mut().zip(&p.ch).enumerate() {
                if c.cap.is_none() {
                    sh.set_meter(i, 0.0);
                    continue;
                }
                tmp.clear();
                tmp.resize(n, 0.0);
                c.jit.pull(&mut tmp);
                c.eq.process(&mut tmp);
                let cm = match i {
                    0 => cm_game,
                    1 => cm_chat,
                    _ => 1.0,
                };
                let target = if cfg.mute { 0.0 } else { dsp::fader_to_lin(cfg.vol) * cm };
                dsp::apply_gain_ramp(&mut tmp, c.gain, target);
                c.gain = target;
                sh.set_meter(i, dsp::peak(&tmp));
                for (m, t) in mix.iter_mut().zip(&tmp) {
                    *m += *t;
                }
            }
            let mt = if p.master_mute { 0.0 } else { dsp::fader_to_lin(p.master) };
            dsp::apply_gain_ramp(&mut mix, master_gain, mt);
            master_gain = mt;
            for s in mix.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
            sh.set_meter(5, dsp::peak(&mix));
            out.write(&mix)?;
        }

        // ---- mic chain
        if let (Some(cap), Some(mo)) = (mic_cap.as_mut(), mic_out.as_mut()) {
            if cap.drain(&mut mic_jit.q).is_err() {
                return Ok(false);
            }
            let free = mo.free_frames()? as usize * 2;
            let want = free.min(mic_jit.q.len().max(0));
            if want > 0 {
                mic_buf.clear();
                mic_buf.resize(want, 0.0);
                mic_jit.pull(&mut mic_buf);
                if p.mic.gate_on {
                    gate.process(&mut mic_buf, p.mic.gate_db);
                }
                mic_eq.process(&mut mic_buf);
                let t = if p.mic.mute { 0.0 } else { dsp::fader_to_lin(p.mic.vol) * 2.0 };
                dsp::apply_gain_ramp(&mut mic_buf, mic_gain, t);
                mic_gain = t;
                for s in mic_buf.iter_mut() {
                    *s = s.clamp(-1.0, 1.0);
                }
                sh.set_meter(4, dsp::peak(&mic_buf));
                mo.write(&mic_buf)?;
            }
        } else {
            sh.set_meter(4, 0.0);
        }
    }
    Ok(false)
}
