//! Global hotkeys that work while BitGG sits in the tray and a game has focus.
//!
//! First choice is RegisterHotKey. If another program already owns the combo
//! (GG Moments, a game, an overlay...) we fall back to a low-level keyboard
//! hook (WH_KEYBOARD_LL, the same mechanism Discord push-to-talk uses), which
//! sees the key even when the registration is taken. The hook only compares a
//! couple of integers per key press and never blocks or swallows keys.

use crate::UserEv;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use winit::event_loop::EventLoopProxy;

pub const SAVE: i32 = 1;
pub const RECORD: i32 = 2;
pub const REPLAY: i32 = 3;

/// Choices offered for the SAVE key.
/// (Alt+Z is left out: it is the NVIDIA overlay key.)
pub const SAVE_KEYS: [&str; 10] = [
    "ALT+F10", "CTRL+ALT+S", "CTRL+SHIFT+S", "F8", "F9", "F10", "F11", "F12", "PAUSE", "SCROLL",
];
const RECORD_KEY: &str = "ALT+F9";
const REPLAY_KEY: &str = "ALT+F8";
const WM_REBIND: u32 = WM_APP + 7;

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEv>>> = OnceLock::new();
static THREAD: AtomicU32 = AtomicU32::new(0);
static WANT_SAVE: Mutex<String> = Mutex::new(String::new());
/// Opt-in: install a low-level keyboard hook as a last resort when every key is
/// taken. Off by default — a hook is what makes AV heuristics flag an app, and
/// BitGG normally just moves to a free key instead.
static ALLOW_HOOK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// (id, modifiers, vk) handled by the hook because RegisterHotKey failed.
static HOOKED: Mutex<Vec<(i32, u32, u32)>> = Mutex::new(Vec::new());
pub static STATUS: Mutex<String> = Mutex::new(String::new());
/// The SAVE key actually in use (may differ from the wanted one if it was taken).
pub static ACTIVE_SAVE: Mutex<String> = Mutex::new(String::new());

const M_ALT: u32 = 1;
const M_CTRL: u32 = 2;
const M_SHIFT: u32 = 4;

pub fn parse(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0;
    let mut vk = 0u32;
    for part in s.split('+') {
        match part {
            "ALT" => mods |= M_ALT,
            "CTRL" => mods |= M_CTRL,
            "SHIFT" => mods |= M_SHIFT,
            "PAUSE" => vk = VK_PAUSE.0 as u32,
            "SCROLL" => vk = VK_SCROLL.0 as u32,
            f if f.starts_with('F') && f.len() > 1 => vk = VK_F1.0 as u32 + f[1..].parse::<u32>().ok()? - 1,
            c if c.len() == 1 => vk = c.as_bytes()[0] as u32, // A-Z / 0-9 match their VK codes
            _ => return None,
        }
    }
    (vk != 0).then_some((mods, vk))
}

static LAST_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Debounced: key repeat, or hotkey + hook firing together, saves only once.
fn send(id: i32) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if now.saturating_sub(LAST_MS.swap(now, Ordering::AcqRel)) < 400 {
        return;
    }
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(UserEv::Hotkey(id));
    }
}

pub fn spawn(proxy: EventLoopProxy<UserEv>, save_key: &str, allow_hook: bool) {
    let _ = PROXY.set(Mutex::new(proxy));
    *WANT_SAVE.lock().unwrap() = save_key.to_string();
    ALLOW_HOOK.store(allow_hook, Ordering::Release);
    std::thread::Builder::new().name("hotkeys".into()).spawn(|| unsafe { run() }).ok();
}

/// Enable/disable the keyboard-hook fallback at runtime (SETUP toggle).
pub fn set_allow_hook(allow: bool) {
    ALLOW_HOOK.store(allow, Ordering::Release);
    let t = THREAD.load(Ordering::Acquire);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_REBIND, WPARAM(0), LPARAM(0));
        }
    }
}

/// Change the SAVE key at runtime (from the CLIPS tab).
pub fn rebind(save_key: &str) {
    *WANT_SAVE.lock().unwrap() = save_key.to_string();
    let t = THREAD.load(Ordering::Acquire);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_REBIND, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe fn register(id: i32, key: &str) -> bool {
    let Some((mods, vk)) = parse(key) else { return false };
    let mut m = MOD_NOREPEAT;
    if mods & M_ALT != 0 {
        m |= MOD_ALT;
    }
    if mods & M_CTRL != 0 {
        m |= MOD_CONTROL;
    }
    if mods & M_SHIFT != 0 {
        m |= MOD_SHIFT;
    }
    RegisterHotKey(None, id, m, vk).is_ok()
}

unsafe fn bind_all() {
    for id in [SAVE, RECORD, REPLAY] {
        let _ = UnregisterHotKey(None, id);
    }
    let want = WANT_SAVE.lock().unwrap().clone();
    let mut hooked = Vec::new();
    // SAVE: the wanted key, else the first free one (another app - e.g. NVIDIA
    // Instant Replay on Alt+F10 - would otherwise win the key press in games)
    let mut active = String::new();
    let mut status = String::new();
    if register(SAVE, &want) {
        active = want.clone();
        status = format!("{want} OK");
    } else if let Some(k) = SAVE_KEYS.iter().find(|k| **k != want && register(SAVE, k)) {
        active = k.to_string();
        status = format!("{want} TAKEN -> {k}");
    } else if ALLOW_HOOK.load(Ordering::Acquire) {
        if let Some((mods, vk)) = parse(&want) {
            hooked.push((SAVE, mods, vk));
            active = want.clone();
            status = format!("{want} HOOK");
        }
    } else {
        // every candidate key is taken and the hook is off: tell the user
        active = want.clone();
        status = format!("{want} TAKEN");
    }
    if ALLOW_HOOK.load(Ordering::Acquire) {
        for (id, key) in [(RECORD, RECORD_KEY), (REPLAY, REPLAY_KEY)] {
            if !register(id, key) {
                if let Some((mods, vk)) = parse(key) {
                    hooked.push((id, mods, vk));
                }
            }
        }
    } else {
        for (id, key) in [(RECORD, RECORD_KEY), (REPLAY, REPLAY_KEY)] {
            let _ = register(id, key);
        }
    }
    *HOOKED.lock().unwrap() = hooked;
    *ACTIVE_SAVE.lock().unwrap() = active;
    *STATUS.lock().unwrap() = status;
    crate::dlog(&format!("hotkeys bound: {} hooked={:?}", STATUS.lock().unwrap(), HOOKED.lock().unwrap()));
}

unsafe extern "system" fn ll_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && (wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN) {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        if std::env::var_os("BITGG_LOGKEYS").is_some() {
            crate::dlog(&format!("hook saw vk={} msg={:#x}", k.vkCode, wp.0));
        }
        let hooked = HOOKED.lock().unwrap();
        for &(id, mods, vk) in hooked.iter() {
            if k.vkCode == vk {
                let down = |v: VIRTUAL_KEY| GetAsyncKeyState(v.0 as i32) as u16 & 0x8000 != 0;
                let have = (if down(VK_MENU) { M_ALT } else { 0 })
                    | (if down(VK_CONTROL) { M_CTRL } else { 0 })
                    | (if down(VK_SHIFT) { M_SHIFT } else { 0 });
                if have == mods {
                    send(id);
                }
            }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

unsafe fn run() {
    THREAD.store(windows::Win32::System::Threading::GetCurrentThreadId(), Ordering::Release);
    // make sure this thread has a message queue before anyone posts to it
    let mut msg = MSG::default();
    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    let mut hook: Option<HHOOK> = None;
    // the keyboard hook is only installed while some key could not be registered
    let sync_hook = |hook: &mut Option<HHOOK>| {
        let need = !HOOKED.lock().unwrap().is_empty();
        if need && hook.is_none() {
            let hmod = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            let r = SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_hook), hmod, 0);
            crate::dlog(&format!("keyboard hook installed: {:?}", r.as_ref().map(|_| ()).map_err(|e| e.message())));
            *hook = r.ok();
        } else if !need {
            if let Some(h) = hook.take() {
                let _ = UnhookWindowsHookEx(h);
            }
        }
    };
    bind_all();
    sync_hook(&mut hook);
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        match msg.message {
            WM_HOTKEY => send(msg.wParam.0 as i32),
            WM_REBIND => {
                bind_all();
                sync_hook(&mut hook);
            }
            _ => {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    if let Some(h) = hook {
        let _ = UnhookWindowsHookEx(h);
    }
}
