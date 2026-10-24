//! `bitgg --drivertest [soak <minutes>]`: checks the BitGG Virtual Audio
//! driver (driver/bitggvad) from user mode, using only public audio APIs.
//! Meant to run inside the test VM (docs/driver/TESTPLAN.md). Prints one
//! PASS/FAIL line per check; the exit code is the number of failures.

use crate::{wasapi, win_audio};
use std::collections::VecDeque;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RENDERS: [&str; 5] = ["BitGG Game", "BitGG Chat", "BitGG Media", "BitGG Aux", "BitGG Mic (in)"];
const MIC: &str = "BitGG Mic";
const TONE_HZ: f32 = 440.0;
const TONE_AMP: f32 = 0.5;

struct Endpoint {
    label: &'static str,
    id: String,
    render: bool,
}

struct Report {
    fails: u32,
}

impl Report {
    fn check(&mut self, ok: bool, name: &str, detail: String) {
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.fails += 1;
        }
    }
}

/// Endpoint names are "<label> (BitGG Virtual Audio)".
fn find(label: &'static str, render: bool) -> Option<Endpoint> {
    let prefix = format!("{label} (");
    win_audio::list_devices()
        .into_iter()
        .find(|d| d.render == render && d.name.starts_with(&prefix))
        .map(|d| Endpoint { label, id: d.id, render })
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn tone(id: String, secs: f32) -> JoinHandle<()> {
    std::thread::spawn(move || {
        win_audio::com_init();
        let Ok(mut r) = wasapi::Render::new(&id, false, 100) else { return };
        let mut ph = 0f32;
        let t = Instant::now();
        while t.elapsed().as_secs_f32() < secs {
            let n = r.free_frames().unwrap_or(0) as usize;
            let mut v = Vec::with_capacity(n * 2);
            for _ in 0..n {
                let x = (ph * std::f32::consts::TAU).sin() * TONE_AMP;
                ph = (ph + TONE_HZ / wasapi::RATE as f32).fract();
                v.push(x);
                v.push(x);
            }
            let _ = r.write(&v);
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

/// Record `ms` of interleaved stereo from an endpoint (loopback for render).
fn record(id: &str, loopback: bool, ms: u64) -> Option<Vec<f32>> {
    let mut c = wasapi::Capture::new(id, loopback).ok()?;
    let mut q = VecDeque::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(ms) {
        c.drain(&mut q).ok()?;
        std::thread::sleep(Duration::from_millis(10));
    }
    Some(q.into_iter().collect())
}

/// Level and purity of a recording: (peak, 440 Hz amplitude / peak, longest silent gap in ms).
fn analyse(x: &[f32]) -> (f32, f32, f32) {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // Goertzel on the left channel; a sine of amplitude A gives A/2.
    let w = std::f32::consts::TAU * TONE_HZ / wasapi::RATE as f32;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    let n = x.len() / 2;
    for i in 0..n {
        let s0 = x[i * 2] + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let g = (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0).sqrt() / n.max(1) as f32;
    let purity = if peak > 0.0 { 2.0 * g / peak } else { 0.0 };
    // A 440 Hz sine crosses zero every ~1.1 ms, so a run of near-zero samples
    // longer than that is a dropout.
    let (mut run, mut gap) = (0usize, 0usize);
    for fr in x.chunks_exact(2) {
        if fr[0].abs() < 1e-4 {
            run += 1;
            gap = gap.max(run);
        } else {
            run = 0;
        }
    }
    (peak, purity, gap as f32 * 1000.0 / wasapi::RATE as f32)
}

/// Play the tone on `play` and record `rec` while it plays (skipping start-up).
fn tone_through(play: &str, rec: &str, loopback: bool) -> Option<Vec<f32>> {
    let h = tone(play.to_string(), 1.8);
    std::thread::sleep(Duration::from_millis(300));
    let x = record(rec, loopback, 1000);
    let _ = h.join();
    x
}

fn tone_check(rep: &mut Report, name: &str, x: Option<Vec<f32>>, tol_db: f32) {
    match x {
        None => rep.check(false, name, "could not open stream".into()),
        Some(x) => {
            let (peak, purity, gap) = analyse(&x);
            let err = db(peak) - db(TONE_AMP);
            let ok = err.abs() <= tol_db && purity > 0.95 && gap < 2.0;
            rep.check(ok, name, format!("peak {peak:.4} ({err:+.2} dB, limit ±{tol_db}), 440 Hz purity {purity:.3}, longest gap {gap:.1} ms"));
        }
    }
}

fn silence_check(rep: &mut Report, name: &str, x: Option<Vec<f32>>) {
    match x {
        None => rep.check(false, name, "could not open stream".into()),
        Some(x) => {
            let (peak, _, _) = analyse(&x);
            rep.check(peak < 1e-4 && !x.is_empty(), name, format!("{} frames, peak {peak:.6}", x.len() / 2));
        }
    }
}

pub fn run(args: &[String]) -> i32 {
    win_audio::com_init();
    let mut rep = Report { fails: 0 };

    // T1: all six endpoints present with the expected flow and format.
    let mut eps: Vec<Endpoint> = Vec::new();
    for label in RENDERS {
        match find(label, true) {
            Some(e) => eps.push(e),
            None => rep.check(false, "endpoint", format!("{label} (render) not found or not active")),
        }
    }
    let mic = find(MIC, false);
    if mic.is_none() {
        rep.check(false, "endpoint", format!("{MIC} (capture) not found or not active"));
    }
    for e in eps.iter().chain(mic.iter()) {
        let fmt = wasapi::mix_format(&e.id);
        let ok = matches!(fmt, Some((48_000, 2)));
        rep.check(ok, "endpoint", format!("{} ({}) mix format {fmt:?}, want 48000 Hz 2 ch", e.label, if e.render { "render" } else { "capture" }));
    }
    if eps.len() < RENDERS.len() || mic.is_none() {
        println!("endpoints missing: is the driver installed? (driver/test/Install-BitggVad.ps1)");
        return rep.fails.max(1) as i32;
    }
    let mic = mic.unwrap();
    let mic_in = eps.iter().find(|e| e.label == "BitGG Mic (in)").unwrap().id.clone();

    // Unity endpoint volume for the level checks; restored at the end.
    let saved: Vec<(String, f32, bool)> = eps
        .iter()
        .chain(std::iter::once(&mic))
        .filter_map(|e| {
            let v = win_audio::endpoint_volume(&e.id)?;
            let (vol, mute) = unsafe { (v.GetMasterVolumeLevelScalar().ok()?, v.GetMute().ok()?.as_bool()) };
            win_audio::set_endpoint(&e.id, Some(1.0), Some(false));
            Some((e.id.clone(), vol, mute))
        })
        .collect();

    if args.first().map(|s| s == "soak").unwrap_or(false) {
        let minutes: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
        soak(&mut rep, &eps, &mic.id, &mic_in, minutes);
    } else {
        // T2: every render endpoint is a working sink that WASAPI loopback can read.
        for e in &eps {
            tone_check(&mut rep, &format!("render loopback {}", e.label), tone_through(&e.id, &e.id, true), 0.5);
        }
        // T3: mic loopback is bit-transparent at 100 % volume.
        tone_check(&mut rep, "mic loopback level", tone_through(&mic_in, &mic.id, false), 0.1);
        // T4: silence when nothing renders to Mic (in).
        std::thread::sleep(Duration::from_millis(300));
        silence_check(&mut rep, "mic silent when idle", record(&mic.id, false, 1000));
        // T5: no stale audio after the render side stops.
        let _ = tone(mic_in.clone(), 1.0).join();
        std::thread::sleep(Duration::from_millis(500));
        silence_check(&mut rep, "mic no stale audio after stop", record(&mic.id, false, 1000));
        // T6: two apps recording the mic at once both get the full signal.
        let h = tone(mic_in.clone(), 1.8);
        std::thread::sleep(Duration::from_millis(300));
        let id2 = mic.id.clone();
        let other = std::thread::spawn(move || {
            win_audio::com_init();
            record(&id2, false, 1000)
        });
        let first = record(&mic.id, false, 1000);
        let second = other.join().ok().flatten();
        let _ = h.join();
        tone_check(&mut rep, "mic two readers (1)", first, 0.1);
        tone_check(&mut rep, "mic two readers (2)", second, 0.1);
        // T7: repeated open/close of every stream.
        let mut errors = 0;
        for _ in 0..20 {
            for e in &eps {
                errors += wasapi::Render::new(&e.id, false, 50).is_err() as u32;
            }
            errors += wasapi::Capture::new(&mic.id, false).is_err() as u32;
        }
        rep.check(errors == 0, "open/close x20", format!("{errors} errors"));
    }

    for (id, vol, mute) in saved {
        win_audio::set_endpoint(&id, Some(vol), Some(mute));
    }
    println!("{} failure(s)", rep.fails);
    rep.fails as i32
}

/// Long run for Driver Verifier: repeat the level checks until time is up.
fn soak(rep: &mut Report, eps: &[Endpoint], mic: &str, mic_in: &str, minutes: u64) {
    let end = Instant::now() + Duration::from_secs(minutes * 60);
    let (mut rounds, mut bad) = (0u32, 0u32);
    while Instant::now() < end {
        let before = rep.fails;
        let e = &eps[rounds as usize % eps.len()];
        tone_check(rep, &format!("soak render {}", e.label), tone_through(&e.id, &e.id, true), 0.5);
        tone_check(rep, "soak mic loopback", tone_through(mic_in, mic, false), 0.1);
        rounds += 1;
        bad += (rep.fails > before) as u32;
    }
    rep.check(bad == 0, "soak", format!("{rounds} rounds in {minutes} min, {bad} with failures"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(secs: f32) -> Vec<f32> {
        let n = (secs * wasapi::RATE as f32) as usize;
        (0..n)
            .flat_map(|i| {
                let x = (std::f32::consts::TAU * TONE_HZ * i as f32 / wasapi::RATE as f32).sin() * TONE_AMP;
                [x, x]
            })
            .collect()
    }

    #[test]
    fn clean_tone_passes() {
        let (peak, purity, gap) = analyse(&sine(1.0));
        assert!((db(peak) - db(TONE_AMP)).abs() < 0.01);
        assert!(purity > 0.99, "purity {purity}");
        assert!(gap < 1.0, "gap {gap}");
    }

    #[test]
    fn dropout_is_detected() {
        let mut x = sine(1.0);
        let start = 20_000 * 2;
        x[start..start + 480 * 2].iter_mut().for_each(|v| *v = 0.0); // 10 ms hole
        let (_, _, gap) = analyse(&x);
        assert!(gap >= 9.9, "gap {gap}");
    }

    #[test]
    fn wrong_signal_has_low_purity() {
        // a byte-misaligned 16-bit stream sounds like noise: no 440 Hz peak
        let mut s = 12345u32;
        let x: Vec<f32> = (0..96_000)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((s >> 16) as f32 / 32768.0 - 1.0) * 0.5
            })
            .collect();
        let (_, purity, _) = analyse(&x);
        assert!(purity < 0.2, "purity {purity}");
    }
}
