//! System-tray icon (notification area, bottom-right) on its own tiny thread.
//! Left click = open, right click = menu. A second launch of BitGG posts
//! WM_SHOW_EXISTING to this window instead of starting another copy.

use crate::UserEv;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use winit::event_loop::EventLoopProxy;

pub const CLASS: PCWSTR = w!("BitGGTrayWindow");
const WM_TRAY: u32 = WM_APP + 1;
pub const WM_SHOW_EXISTING: u32 = WM_APP + 2;

pub const CMD_OPEN: u32 = 1;
pub const CMD_SAVE: u32 = 2;
pub const CMD_REPLAY: u32 = 3;
pub const CMD_EXIT: u32 = 9;

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEv>>> = OnceLock::new();
static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn send(cmd: u32) {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(UserEv::Tray(cmd));
    }
}

pub fn spawn(proxy: EventLoopProxy<UserEv>) {
    let _ = PROXY.set(Mutex::new(proxy));
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(|| unsafe { run() })
        .ok();
}

/// If another BitGG already runs, ask it to show its window. Returns true if so.
pub fn signal_existing() -> bool {
    unsafe {
        match FindWindowW(CLASS, None) {
            Ok(h) if !h.is_invalid() => PostMessageW(h, WM_SHOW_EXISTING, WPARAM(0), LPARAM(0)).is_ok(),
            _ => false,
        }
    }
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut n = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let tip: Vec<u16> = "BitGG - Sound & Clips".encode_utf16().collect();
    n.szTip[..tip.len()].copy_from_slice(&tip);
    n
}

unsafe fn add_icon(hwnd: HWND) {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let icon = LoadIconW(hinst, PCWSTR(1 as _)).or_else(|_| LoadIconW(None, IDI_APPLICATION)).unwrap_or_default();
    let mut n = nid(hwnd);
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = icon;
    let _ = Shell_NotifyIconW(NIM_ADD, &n);
}

/// Show a Windows notification from the tray icon.
pub fn notify(title: &str, text: &str) {
    let raw = HWND_RAW.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    let mut n = nid(HWND(raw as *mut _));
    n.uFlags = NIF_INFO;
    let t: Vec<u16> = title.encode_utf16().take(63).collect();
    n.szInfoTitle[..t.len()].copy_from_slice(&t);
    let b: Vec<u16> = text.encode_utf16().take(255).collect();
    n.szInfo[..b.len()].copy_from_slice(&b);
    n.dwInfoFlags = NIIF_INFO;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
    }
}

/// Remove the icon (otherwise a ghost icon stays until hovered).
pub fn remove() {
    let raw = HWND_RAW.load(Ordering::Acquire);
    if raw != 0 {
        unsafe {
            let n = nid(HWND(raw as *mut _));
            let _ = Shell_NotifyIconW(NIM_DELETE, &n);
        }
    }
}

unsafe fn run() {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: CLASS, ..Default::default() };
    RegisterClassW(&wc);
    let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, w!("BitGG"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, hinst, None) else {
        return;
    };
    HWND_RAW.store(hwnd.0 as isize, Ordering::Release);
    TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Release);
    add_icon(hwnd);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe fn menu(hwnd: HWND) {
    let Ok(m) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(m, MF_STRING, CMD_OPEN as usize, w!("&Open BitGG"));
    let _ = AppendMenuW(m, MF_STRING, CMD_SAVE as usize, w!("&Save clip (replay)"));
    let _ = AppendMenuW(m, MF_STRING, CMD_REPLAY as usize, w!("&Replay buffer on/off\tAlt+F8"));
    let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(m, MF_STRING, CMD_EXIT as usize, w!("E&xit"));
    let _ = SetMenuDefaultItem(m, CMD_OPEN, 0);
    let mut p = POINT::default();
    let _ = GetCursorPos(&mut p);
    let _ = SetForegroundWindow(hwnd); // required so the menu closes when clicking elsewhere
    let cmd = TrackPopupMenu(m, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY | TPM_BOTTOMALIGN, p.x, p.y, 0, hwnd, None);
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(m);
    if cmd.0 != 0 {
        send(cmd.0 as u32);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match lp.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(CMD_OPEN),
                WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_SHOW_EXISTING => {
            send(CMD_OPEN);
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Acquire) => {
            add_icon(hwnd); // Explorer restarted: put the icon back
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
