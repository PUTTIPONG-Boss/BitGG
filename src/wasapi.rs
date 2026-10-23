//! Minimal WASAPI shared-mode streams, always 48 kHz float stereo (the OS
//! converts via AUTOCONVERTPCM so we never resample ourselves).

use crate::win_audio::device;
use std::collections::VecDeque;
use windows::core::{GUID, Result};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const LOOPBACK: u32 = 0x0002_0000;
const EVENTCALLBACK: u32 = 0x0004_0000;
const AUTOCONVERT: u32 = 0x8000_0000;
const SRC_QUALITY: u32 = 0x0800_0000;
const SILENT: u32 = 0x2;
const FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

pub const RATE: u32 = 48_000;

fn fmt(channels: u16, mask: u32) -> WAVEFORMATEXTENSIBLE {
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 4 * channels as u32,
            nBlockAlign: 4 * channels,
            wBitsPerSample: 32,
            cbSize: 22,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: mask,
        SubFormat: FLOAT,
    }
}

/// Device's own channel layout (count, speaker mask).
unsafe fn mix_layout(client: &IAudioClient) -> (u16, u32) {
    let Ok(f) = client.GetMixFormat() else { return (2, 3) };
    let ch = (*f).nChannels;
    let mask = if (*f).wFormatTag == WAVE_FORMAT_EXTENSIBLE as u16 {
        (*(f as *const WAVEFORMATEXTENSIBLE)).dwChannelMask
    } else {
        3
    };
    windows::Win32::System::Com::CoTaskMemFree(Some(f as *const _));
    (ch, mask)
}

/// Endpoint mix format as (sample rate, channels).
pub fn mix_format(id: &str) -> Option<(u32, u16)> {
    unsafe {
        let client: IAudioClient = device(id).ok()?.Activate(CLSCTX_ALL, None).ok()?;
        let f = client.GetMixFormat().ok()?;
        let r = ((*f).nSamplesPerSec, (*f).nChannels);
        windows::Win32::System::Com::CoTaskMemFree(Some(f as *const _));
        Some(r)
    }
}

fn open(id: &str, flags: u32, buffer_ms: i64) -> Result<IAudioClient> {
    open_ch(id, flags, buffer_ms, 2, 3)
}

fn open_ch(id: &str, flags: u32, buffer_ms: i64, channels: u16, mask: u32) -> Result<IAudioClient> {
    unsafe {
        let dev = device(id)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let f = fmt(channels, mask);
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags | AUTOCONVERT | SRC_QUALITY,
            buffer_ms * 10_000,
            0,
            &f.Format,
            None,
        )?;
        Ok(client)
    }
}

pub struct Capture {
    client: IAudioClient,
    cap: IAudioCaptureClient,
    /// Per input channel (left gain, right gain) for folding to stereo.
    fold: Vec<(f32, f32)>,
    /// Running per-channel peak of the raw (pre-fold) signal, for diagnostics.
    pub raw_peaks: Vec<f32>,
}

/// Stereo fold-down weights by speaker position. Stereo content in FL/FR stays
/// at unity (Windows' own 7.1->2.0 downmix in AUTOCONVERT attenuates it a lot,
/// which is what made Sonar's 8-channel Gaming/Media/Aux VADs sound quiet).
fn fold_weights(channels: u16, mask: u32) -> Vec<(f32, f32)> {
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let mut w = Vec::new();
    let mut bit = 0;
    for _ in 0..channels {
        while bit < 32 && mask & (1 << bit) == 0 {
            bit += 1;
        }
        let g = match 1u32.checked_shl(bit).unwrap_or(0) {
            0x1 => (1.0, 0.0),             // FL
            0x2 => (0.0, 1.0),             // FR
            0x4 => (H, H),                 // FC
            0x8 => (0.0, 0.0),             // LFE (headphones: drop)
            0x10 | 0x200 => (H, 0.0),      // BL, SL
            0x20 | 0x400 => (0.0, H),      // BR, SR
            0x40 => (1.0, 0.0),            // FLC
            0x80 => (0.0, 1.0),            // FRC
            0x100 => (0.5, 0.5),           // BC
            _ => (0.5, 0.5),
        };
        w.push(g);
        bit += 1;
    }
    w
}

impl Capture {
    /// `loopback = true` records what is being played on a render endpoint.
    pub fn new(id: &str, loopback: bool) -> Result<Capture> {
        // capture in the device's own channel layout (Windows still converts
        // the rate to 48 kHz) and fold to stereo ourselves
        let (channels, mask) = unsafe {
            let dev = device(id)?;
            let probe: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
            mix_layout(&probe)
        };
        let flags = if loopback { LOOPBACK } else { 0 };
        let (client, fold) = match (channels > 2).then(|| open_ch(id, flags, 100, channels, mask)) {
            Some(Ok(c)) => (c, fold_weights(channels, mask)),
            _ => (open(id, flags, 100)?, Vec::new()),
        };
        unsafe {
            let cap: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            let raw_peaks = vec![0.0; fold.len()];
            Ok(Capture { client, cap, fold, raw_peaks })
        }
    }

    /// Drain everything available into `out` (interleaved stereo f32).
    pub fn drain(&mut self, out: &mut VecDeque<f32>) -> Result<()> {
        unsafe {
            loop {
                let n = self.cap.GetNextPacketSize()?;
                if n == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let len = frames as usize * 2;
                if flags & SILENT != 0 || data.is_null() {
                    out.extend(std::iter::repeat(0.0).take(len));
                } else if self.fold.is_empty() {
                    out.extend(std::slice::from_raw_parts(data as *const f32, len));
                } else {
                    let nch = self.fold.len();
                    let src = std::slice::from_raw_parts(data as *const f32, frames as usize * nch);
                    for fr in src.chunks_exact(nch) {
                        for (p, x) in self.raw_peaks.iter_mut().zip(fr) {
                            *p = p.max(x.abs());
                        }
                        let (mut l, mut r) = (0.0, 0.0);
                        for (x, (gl, gr)) in fr.iter().zip(&self.fold) {
                            l += x * gl;
                            r += x * gr;
                        }
                        out.push_back(l);
                        out.push_back(r);
                    }
                }
                self.cap.ReleaseBuffer(frames)?;
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

pub struct Render {
    client: IAudioClient,
    ren: IAudioRenderClient,
    pub size: u32,
    pub event: Option<HANDLE>,
}

impl Render {
    pub fn new(id: &str, event_driven: bool, buffer_ms: i64) -> Result<Render> {
        let client = open(id, if event_driven { EVENTCALLBACK } else { 0 }, buffer_ms)?;
        unsafe {
            let event = if event_driven {
                let e = CreateEventW(None, false, false, None)?;
                client.SetEventHandle(e)?;
                Some(e)
            } else {
                None
            };
            let ren: IAudioRenderClient = client.GetService()?;
            let size = client.GetBufferSize()?;
            // pre-roll silence so the device starts cleanly
            let p = ren.GetBuffer(size)?;
            std::ptr::write_bytes(p, 0, size as usize * 8);
            ren.ReleaseBuffer(size, 0)?;
            client.Start()?;
            Ok(Render { client, ren, size, event })
        }
    }

    /// Wait for the device to ask for more data (or timeout).
    pub fn wait(&self, ms: u32) -> bool {
        match self.event {
            Some(e) => unsafe { WaitForSingleObject(e, ms) == WAIT_OBJECT_0 },
            None => {
                std::thread::sleep(std::time::Duration::from_millis(ms as u64));
                true
            }
        }
    }

    pub fn free_frames(&self) -> Result<u32> {
        unsafe { Ok(self.size - self.client.GetCurrentPadding()?) }
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        let frames = (samples.len() / 2) as u32;
        if frames == 0 {
            return Ok(());
        }
        unsafe {
            let p = self.ren.GetBuffer(frames)? as *mut f32;
            std::ptr::copy_nonoverlapping(samples.as_ptr(), p, samples.len());
            self.ren.ReleaseBuffer(frames, 0)?;
        }
        Ok(())
    }
}

impl Drop for Render {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if let Some(e) = self.event {
                let _ = windows::Win32::Foundation::CloseHandle(e);
            }
        }
    }
}

/// Jitter buffer over a capture stream feeding a differently-clocked render
/// stream: waits for `prefill` before playing, trims if it drifts too far ahead.
pub struct Jitter {
    pub q: VecDeque<f32>,
    playing: bool,
    prefill: usize,
    max: usize,
}

impl Jitter {
    pub fn new(prefill_ms: usize, max_ms: usize) -> Jitter {
        Jitter {
            q: VecDeque::with_capacity(RATE as usize),
            playing: false,
            prefill: prefill_ms * RATE as usize / 1000 * 2,
            max: max_ms * RATE as usize / 1000 * 2,
        }
    }

    /// Fill `out` (zeroed by caller) with up to out.len() samples.
    pub fn pull(&mut self, out: &mut [f32]) {
        if self.q.len() > self.max {
            let drop = self.q.len() - self.prefill;
            self.q.drain(..drop & !1);
        }
        if !self.playing {
            if self.q.len() >= self.prefill {
                self.playing = true;
            } else {
                return;
            }
        }
        let n = out.len().min(self.q.len());
        for (o, s) in out[..n].iter_mut().zip(self.q.drain(..n)) {
            *o = s;
        }
        if n < out.len() {
            self.playing = false; // underrun -> re-buffer
        }
    }
}
