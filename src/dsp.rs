//! Tiny DSP kit mirroring what the Sonar APO exposes (observed setting names:
//! ParametricEqFilter{1..10}{Type,FreqHz,GainDb,Q,State}, NoiseGateThresholdDB,
//! ChatMixGainLin, GeneralGainDB). RBJ "Audio EQ Cookbook" biquads.

use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

pub const SR: f32 = 48_000.0;
pub const BANDS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterType {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}

impl FilterType {
    pub const ALL: [FilterType; 6] = [
        FilterType::Peaking,
        FilterType::LowShelf,
        FilterType::HighShelf,
        FilterType::LowPass,
        FilterType::HighPass,
        FilterType::Notch,
    ];
    pub fn short(self) -> &'static str {
        match self {
            FilterType::Peaking => "PK",
            FilterType::LowShelf => "LS",
            FilterType::HighShelf => "HS",
            FilterType::LowPass => "LP",
            FilterType::HighPass => "HP",
            FilterType::Notch => "NT",
        }
    }
    pub fn next(self) -> FilterType {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Band {
    pub on: bool,
    pub kind: FilterType,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EqSettings {
    pub on: bool,
    pub bands: [Band; BANDS],
}

/// Same default centre frequencies as a classic 10-band graphic EQ.
pub const DEFAULT_FREQS: [f32; BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

impl Default for EqSettings {
    fn default() -> Self {
        let mut bands = [Band { on: true, kind: FilterType::Peaking, freq: 1000.0, gain_db: 0.0, q: 1.41 }; BANDS];
        for (i, b) in bands.iter_mut().enumerate() {
            b.freq = DEFAULT_FREQS[i];
        }
        bands[0].kind = FilterType::LowShelf;
        bands[0].q = 0.71;
        bands[BANDS - 1].kind = FilterType::HighShelf;
        bands[BANDS - 1].q = 0.71;
        EqSettings { on: false, bands }
    }
}

/// Named presets in the spirit of Sonar's built-in configs.
pub fn preset(name: &str) -> EqSettings {
    let mut e = EqSettings::default();
    e.on = true;
    let g: [f32; BANDS] = match name {
        "BASS" => [7.0, 6.0, 4.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        "FPS" => [-6.0, -5.0, -3.0, -1.0, 0.0, 2.0, 4.0, 6.0, 4.0, 1.0],
        "VOICE" => [-8.0, -6.0, -3.0, 0.0, 1.0, 3.0, 4.0, 3.0, 0.0, -2.0],
        "MUSIC" => [4.0, 3.0, 1.0, 0.0, -1.0, 0.0, 1.0, 2.0, 3.0, 3.0],
        _ => [0.0; BANDS],
    };
    for (b, g) in e.bands.iter_mut().zip(g) {
        b.gain_db = g;
    }
    if name == "FLAT" {
        e.on = false;
    }
    e
}

#[derive(Clone, Copy, Default)]
pub struct Coef {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coef {
    pub fn design(b: &Band) -> Coef {
        let f = b.freq.clamp(10.0, SR * 0.49);
        let q = b.q.max(0.1);
        let a = 10f32.powf(b.gain_db / 40.0);
        let w0 = 2.0 * PI * f / SR;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match b.kind {
            FilterType::Peaking => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            FilterType::LowShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + sa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - sa),
                    (a + 1.0) + (a - 1.0) * cs + sa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - sa,
                )
            }
            FilterType::HighShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + sa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - sa),
                    (a + 1.0) - (a - 1.0) * cs + sa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - sa,
                )
            }
            FilterType::LowPass => (
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterType::HighPass => (
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterType::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        };
        Coef { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// Magnitude response in dB at frequency `f` (for drawing the EQ curve).
    pub fn mag_db(&self, f: f32) -> f32 {
        let w = 2.0 * PI * f / SR;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let m = ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-12)).sqrt();
        20.0 * m.max(1e-6).log10()
    }
}

/// Stereo biquad, transposed direct form II.
#[derive(Clone, Copy, Default)]
struct Biquad {
    c: Coef,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Biquad {
    #[inline]
    fn run(&mut self, ch: usize, x: f32) -> f32 {
        let c = &self.c;
        let y = c.b0 * x + self.z1[ch];
        self.z1[ch] = c.b1 * x - c.a1 * y + self.z2[ch];
        self.z2[ch] = c.b2 * x - c.a2 * y;
        y
    }
}

#[derive(Default)]
pub struct EqProc {
    stages: Vec<Biquad>,
}

impl EqProc {
    pub fn configure(&mut self, s: &EqSettings) {
        let old = std::mem::take(&mut self.stages);
        if !s.on {
            return;
        }
        for (i, b) in s.bands.iter().filter(|b| b.on).enumerate() {
            let mut bq = old.get(i).copied().unwrap_or_default();
            bq.c = Coef::design(b);
            self.stages.push(bq);
        }
    }

    /// Interleaved stereo in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        if self.stages.is_empty() {
            return;
        }
        for fr in buf.chunks_exact_mut(2) {
            for st in self.stages.iter_mut() {
                fr[0] = st.run(0, fr[0]);
                fr[1] = st.run(1, fr[1]);
            }
        }
    }
}

/// Total EQ response (dB) at `f`, used by the UI.
pub fn response_db(s: &EqSettings, f: f32) -> f32 {
    if !s.on {
        return 0.0;
    }
    s.bands.iter().filter(|b| b.on).map(|b| Coef::design(b).mag_db(f)).sum()
}

/// Simple downward noise gate (Sonar: NoiseGateState / NoiseGateThresholdDB).
pub struct Gate {
    env: f32,
    gain: f32,
    hold: u32,
}

impl Default for Gate {
    fn default() -> Self {
        Gate { env: 0.0, gain: 1.0, hold: 0 }
    }
}

impl Gate {
    pub fn process(&mut self, buf: &mut [f32], thresh_db: f32) {
        let th = db_to_lin(thresh_db);
        let atk_env = 0.2;
        let rel_env = 0.0015;
        let open_step = 1.0 / (0.002 * SR); // 2 ms open
        let close_step = 1.0 / (0.08 * SR); // 80 ms close
        for fr in buf.chunks_exact_mut(2) {
            let x = fr[0].abs().max(fr[1].abs());
            let k = if x > self.env { atk_env } else { rel_env };
            self.env += (x - self.env) * k;
            if self.env > th {
                self.hold = (0.12 * SR) as u32;
            } else if self.hold > 0 {
                self.hold -= 1;
            }
            if self.hold > 0 {
                self.gain = (self.gain + open_step).min(1.0);
            } else {
                self.gain = (self.gain - close_step).max(0.0);
            }
            fr[0] *= self.gain;
            fr[1] *= self.gain;
        }
    }
}

/// Click-free gain change across one buffer.
pub fn apply_gain_ramp(buf: &mut [f32], from: f32, to: f32) {
    let n = (buf.len() / 2).max(1) as f32;
    for (i, fr) in buf.chunks_exact_mut(2).enumerate() {
        let g = from + (to - from) * (i as f32 / n);
        fr[0] *= g;
        fr[1] *= g;
    }
}

pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Perceptual fader (slider 0..1 -> linear gain), roughly -60 dB..0 dB.
pub fn fader_to_lin(v: f32) -> f32 {
    if v <= 0.001 {
        0.0
    } else {
        v * v * v
    }
}

/// Sonar ChatMix: balance -1 (all game) .. +1 (all chat). Returns (game, chat) gains.
pub fn chatmix(balance: f32) -> (f32, f32) {
    let b = balance.clamp(-1.0, 1.0);
    ((1.0 - b).min(1.0), (1.0 + b).min(1.0))
}
