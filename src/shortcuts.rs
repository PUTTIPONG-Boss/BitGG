//! Create Start Menu / Desktop / Startup shortcuts to the running exe, with no
//! installer and no admin: BitGG stays wherever the user put it (portable), we
//! only drop .lnk files that point at it. Uses IShellLinkW on a short STA thread
//! (the UI thread is MTA for Core Audio).

use std::path::PathBuf;
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::System::Com::IPersistFile;

fn known_dir(var: &str, sub: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(|p| PathBuf::from(p).join(sub))
}

fn desktop() -> Option<PathBuf> {
    known_dir("USERPROFILE", "Desktop")
}
fn start_menu() -> Option<PathBuf> {
    known_dir("APPDATA", r"Microsoft\Windows\Start Menu\Programs")
}
fn startup() -> Option<PathBuf> {
    known_dir("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup")
}

unsafe fn make_link(path: &PathBuf, target: &str, args: &str, icon: &str) -> bool {
    let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else {
        return false;
    };
    let _ = link.SetPath(&HSTRING::from(target));
    let _ = link.SetArguments(&HSTRING::from(args));
    let _ = link.SetIconLocation(&HSTRING::from(icon), 0);
    let _ = link.SetDescription(&HSTRING::from("BitGG - Sound & Clips"));
    if let Some(dir) = PathBuf::from(target).parent() {
        let _ = link.SetWorkingDirectory(&HSTRING::from(dir.to_string_lossy().as_ref()));
    }
    let Ok(file) = link.cast::<IPersistFile>() else { return false };
    file.Save(PCWSTR(HSTRING::from(path.to_string_lossy().as_ref()).as_ptr()), true).is_ok()
}

fn run_sta<F: FnOnce() -> bool + Send + 'static>(f: F) -> bool {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = f();
        CoUninitialize();
        r
    })
    .join()
    .unwrap_or(false)
}

/// Desktop + Start Menu shortcuts to the current exe. Returns how many were made.
pub fn add_shortcuts() -> u32 {
    let Ok(exe) = std::env::current_exe() else { return 0 };
    run_sta2(exe)
}

fn run_sta2(exe: PathBuf) -> u32 {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let target = exe.to_string_lossy().to_string();
        let mut n = 0;
        for dir in [desktop(), start_menu()].into_iter().flatten() {
            let _ = std::fs::create_dir_all(&dir);
            if make_link(&dir.join("BitGG.lnk"), &target, "", &target) {
                n += 1;
            }
        }
        CoUninitialize();
        n
    })
    .join()
    .unwrap_or(0)
}

/// Turn "run at login (in tray)" on or off.
pub fn set_startup(on: bool) -> bool {
    let Some(dir) = startup() else { return false };
    let link = dir.join("BitGG.lnk");
    if !on {
        return std::fs::remove_file(&link).is_ok() || !link.exists();
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let target = exe.to_string_lossy().to_string();
    run_sta(move || unsafe {
        let _ = std::fs::create_dir_all(&dir);
        make_link(&link, &target, "--tray", &target)
    })
}

pub fn startup_enabled() -> bool {
    startup().map(|d| d.join("BitGG.lnk").exists()).unwrap_or(false)
}
