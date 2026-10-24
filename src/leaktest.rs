//! `bitgg --leaktest`: repeats the operations the app performs over and over
//! (device/session polling, per-app routing lookups, stream open/close,
//! engine and replay-buffer restarts) and prints how many threads and handles
//! this process holds after each phase. Anything that keeps growing leaks.

use crate::{config, dxgi, engine, mf, recorder, wasapi, win_audio};
use std::sync::Arc;
use std::time::Duration;

fn counts() -> (u32, u32) {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount};
    unsafe {
        let pid = GetCurrentProcessId();
        let mut threads = 0;
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
            let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut ok = Thread32First(snap, &mut te).is_ok();
            while ok {
                threads += (te.th32OwnerProcessID == pid) as u32;
                ok = Thread32Next(snap, &mut te).is_ok();
            }
            let _ = windows::Win32::Foundation::CloseHandle(snap);
        }
        let mut handles = 0;
        let _ = GetProcessHandleCount(GetCurrentProcess(), &mut handles);
        (threads, handles)
    }
}

fn phase(name: &str, rounds: u32, mut f: impl FnMut()) {
    let before = counts();
    for _ in 0..rounds {
        f();
    }
    std::thread::sleep(Duration::from_millis(1500)); // let worker threads wind down
    let after = counts();
    let (dt, dh) = (after.0 as i64 - before.0 as i64, after.1 as i64 - before.1 as i64);
    let flag = if dt > 2 || dh > 20 { "  <-- grows" } else { "" };
    println!("{name:<34} x{rounds:<4} threads {:>4} -> {:>4} ({dt:+}), handles {:>5} -> {:>5} ({dh:+}){flag}", before.0, after.0, before.1, after.1);
}

pub fn run() {
    win_audio::com_init();
    let out = win_audio::default_id(true).unwrap_or_default();
    println!("default output: {}", if out.is_empty() { "(none)" } else { "found" });
    println!("start: threads {}, handles {}", counts().0, counts().1);

    phase("list_devices", 200, || {
        let _ = win_audio::list_devices();
    });
    let devs = win_audio::list_devices();
    phase("list_sessions", 100, || {
        let _ = win_audio::list_sessions(&devs);
    });
    let pids: Vec<u32> = win_audio::list_sessions(&devs).iter().map(|s| s.pid).collect();
    phase("get_app_output (all sessions)", 50, || {
        for p in &pids {
            let _ = win_audio::get_app_output(*p);
        }
    });
    phase("endpoint_meter", 200, || {
        let _ = win_audio::endpoint_meter(&out);
    });
    phase("Render open/close", 30, || {
        let _ = wasapi::Render::new(&out, true, 20);
    });
    phase("Capture (loopback) open/close", 30, || {
        let _ = wasapi::Capture::new(&out, true);
    });

    let mut a = config::Audio::default();
    a.output = out.clone();
    let sh = engine::Shared::new(a);
    phase("Engine start/stop", 10, || {
        let mut e = engine::Engine::start(sh.clone());
        std::thread::sleep(Duration::from_millis(500));
        e.stop();
    });

    let clips = config::Config::default().clips;
    if let Some(mon) = dxgi::monitors().into_iter().next() {
        phase("Grabber (D3D11 + duplication)", 20, || {
            let _ = dxgi::Grabber::new(&mon, 1080, 60);
        });
        phase("Grabber + VideoEnc", 5, || {
            if let Ok(g) = dxgi::Grabber::new(&mon, 1080, 60) {
                match mf::VideoEnc::new(g.device(), g.out_w, g.out_h, 60, 20_000_000, &clips.encoder) {
                    Ok(e) => drop(e),
                    Err(e) => println!("  video encoder: {e}"),
                }
            }
        });
    }
    phase("AudioEnc", 5, || {
        let _ = mf::AudioEnc::new(24_000);
    });
    let levels = Arc::new(recorder::RecLevels::default());
    phase("Replay recorder start/stop", 10, || {
        match recorder::Recorder::start(&clips, recorder::Mode::Replay, Some(out.clone()), None, levels.clone()) {
            Ok(mut r) => {
                std::thread::sleep(Duration::from_secs(3));
                r.stop();
            }
            Err(e) => println!("  recorder: {e}"),
        }
    });
    println!("end: threads {}, handles {}", counts().0, counts().1);
}
