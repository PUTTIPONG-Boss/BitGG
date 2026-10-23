//! Native "choose folder" dialog. Runs on its own STA thread (the UI thread is
//! MTA for Core Audio) and hands the result back through `out`.

use std::sync::{Arc, Mutex};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

pub fn pick_folder(owner: isize, initial: String, out: Arc<Mutex<Option<String>>>) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return };
        let opts = dlg.GetOptions().unwrap_or_default();
        let _ = dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
        let _ = dlg.SetTitle(&HSTRING::from("BitGG - choose where clips are saved"));
        let _ = std::fs::create_dir_all(&initial);
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(initial.as_str()), None) {
            let _ = dlg.SetFolder(&item);
        }
        if dlg.Show(HWND(owner as *mut _)).is_ok() {
            if let Ok(item) = dlg.GetResult() {
                if let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    let _: PWSTR = p;
                    if !s.is_empty() {
                        *out.lock().unwrap() = Some(s);
                    }
                }
            }
        }
        CoUninitialize();
    });
}
