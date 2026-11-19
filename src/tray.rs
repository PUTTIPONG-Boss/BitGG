//! System-tray icon (notification area, bottom-right) on its own tiny thread.
//! Left click = open, right click = menu. A second launch of BitGG posts
//! WM_SHOW_EXISTING to this window instead of starting another copy.

use crate::headset::Battery;
use crate::tgui::{pal, tiny_glyph};
use crate::UserEv;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
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
/// Headset battery shown as its own tray icon (ID 2) while a headset is found.
static BATTERY: Mutex<Option<Battery>> = Mutex::new(None);
static BATTERY_SHOWN: AtomicBool = AtomicBool::new(false);
static BATTERY_ICON: AtomicIsize = AtomicIsize::new(0);
const BATTERY_ID: u32 = 2;

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

/// Remove the icons (otherwise a ghost icon stays until hovered).
pub fn remove() {
    let raw = HWND_RAW.load(Ordering::Acquire);
    if raw != 0 {
        unsafe {
            let n = nid(HWND(raw as *mut _));
            let _ = Shell_NotifyIconW(NIM_DELETE, &n);
            if BATTERY_SHOWN.swap(false, Ordering::AcqRel) {
                let _ = Shell_NotifyIconW(NIM_DELETE, &NOTIFYICONDATAW { uID: BATTERY_ID, ..n });
            }
        }
    }
}

/// Headset battery from the headset thread; None hides the battery icon.
pub fn set_battery(b: Option<Battery>) {
    {
        let mut cur = BATTERY.lock().unwrap();
        if *cur == b {
            return;
        }
        *cur = b;
    }
    let raw = HWND_RAW.load(Ordering::Acquire);
    if raw != 0 {
        unsafe { show_battery(HWND(raw as *mut _)) };
    }
}

/// Add, update or remove the battery icon to match BATTERY.
unsafe fn show_battery(hwnd: HWND) {
    let b = BATTERY.lock().unwrap().clone();
    let mut n = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: BATTERY_ID, ..Default::default() };
    let Some(b) = b else {
        if BATTERY_SHOWN.swap(false, Ordering::AcqRel) {
            let _ = Shell_NotifyIconW(NIM_DELETE, &n);
        }
        return;
    };
    let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64);
    let Some(icon) = make_icon(size, &battery_pixels(size as usize, &b)) else { return };
    let tip = match b.percent {
        Some(p) if b.charging => format!("{}: {p}% (charging)", b.name),
        Some(p) => format!("{}: {p}%", b.name),
        None => format!("{}: off", b.name),
    };
    let t: Vec<u16> = tip.encode_utf16().take(127).collect();
    n.szTip[..t.len()].copy_from_slice(&t);
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = icon;
    let shown = BATTERY_SHOWN.load(Ordering::Acquire);
    let ok = Shell_NotifyIconW(if shown { NIM_MODIFY } else { NIM_ADD }, &n).as_bool()
        || (shown && Shell_NotifyIconW(NIM_ADD, &n).as_bool());
    BATTERY_SHOWN.store(ok, Ordering::Release);
    // the shell keeps its own copy: free the previous icon
    let old = BATTERY_ICON.swap(icon.0 as isize, Ordering::AcqRel);
    if old != 0 {
        let _ = DestroyIcon(HICON(old as *mut _));
    }
}

/// Headset (top) and battery number (below) on a 16x16 grid, scaled to
/// `size`, as ARGB with a dark outline so it reads on light and dark taskbars.
pub fn battery_pixels(size: usize, b: &Battery) -> Vec<u32> {
    const N: usize = 16;
    let mut grid = [[0u32; N]; N]; // 0 = transparent, else RGB
    let (band, digit) = match b.percent {
        None => (pal::DGREY, pal::DGREY),
        Some(_) if b.charging => (pal::LGREY, pal::BLUE),
        Some(p) if p <= 20 => (pal::LGREY, pal::RED),
        Some(_) => (pal::LGREY, pal::WHITE),
    };
    // headband + ear cups, rows 0-4:
    //   .....######.....
    //   ....#......#....
    //   ..###......###..
    //   ..###......###..
    //   ..###......###..
    for x in 5..=10 {
        grid[0][x] = band;
    }
    grid[1][4] = band;
    grid[1][11] = band;
    for y in 2..=4 {
        for x in [2, 3, 4, 11, 12, 13] {
            grid[y][x] = band;
        }
    }
    // charging: lightning bolt inside the headband, rows 1-5:
    //   ........##......
    //   .......##.......
    //   ......####......
    //   .......##.......
    //   ......##........
    if b.charging && b.percent.is_some() {
        for (y, xs) in [(1, &[8, 9][..]), (2, &[7, 8]), (3, &[6, 7, 8, 9]), (4, &[7, 8]), (5, &[6, 7])] {
            for &x in xs {
                grid[y][x] = pal::YELLOW;
            }
        }
    }
    // number, rows 6-15 (2x scale) or 8-12 (1x, for "100")
    let text = b.percent.map(|p| p.to_string()).unwrap_or_else(|| "--".into());
    let scale = if text.len() <= 2 { 2 } else { 1 };
    let glyphs: Vec<_> = text.chars().filter_map(tiny_glyph).collect();
    let w: usize = glyphs.iter().map(|(gw, _)| *gw as usize * scale).sum::<usize>() + (glyphs.len().saturating_sub(1)) * scale;
    let (mut x0, y0) = ((N - w.min(N)) / 2, if scale == 2 { 6 } else { 8 });
    for (gw, rows) in glyphs {
        for (ry, bits) in rows.iter().enumerate() {
            for col in 0..gw as usize {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (x, y) = (x0 + col * scale + dx, y0 + ry * scale + dy);
                            if x < N && y < N {
                                grid[y][x] = digit;
                            }
                        }
                    }
                }
            }
        }
        x0 += (gw as usize + 1) * scale;
    }
    let lit = |x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < N && (y as usize) < N && grid[y as usize][x as usize] != 0;
    let mut out = vec![0u32; size * size];
    for py in 0..size {
        for px in 0..size {
            let (gx, gy) = ((px * N / size) as i32, (py * N / size) as i32);
            out[py * size + px] = if lit(gx, gy) {
                0xFF00_0000 | grid[gy as usize][gx as usize]
            } else if (-1..=1).any(|dy| (-1..=1).any(|dx| lit(gx + dx, gy + dy))) {
                0xD000_0000 // outline
            } else {
                0
            };
        }
    }
    out
}

/// 32-bit ARGB pixels -> icon (alpha from the pixels; the mask is unused).
unsafe fn make_icon(size: i32, px: &[u32]) -> Option<HICON> {
    let color = CreateBitmap(size, size, 1, 32, Some(px.as_ptr() as *const _));
    let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const _));
    let info = ICONINFO { fIcon: BOOL(1), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info).ok();
    let _ = DeleteObject(color);
    let _ = DeleteObject(mask);
    icon
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
    show_battery(hwnd); // the headset thread may have read it already
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
            add_icon(hwnd); // Explorer restarted: put the icons back
            BATTERY_SHOWN.store(false, Ordering::Release);
            show_battery(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
