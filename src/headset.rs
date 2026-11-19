//! Wireless headset battery for the tray icon.
//!
//! SteelSeries Arctis Nova 5 / 5X dongles answer a status query (output report
//! `00 b0`) on their vendor HID interface (MI_03, usage page 0xFFC0). This only
//! reads; no headset setting is changed. GG's engine treats the battery as a
//! 0-100 % value (`ArctisNova5TxDeviceEventConverter`, compared against 100).

use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// (USB product id, name) of supported dongles, SteelSeries vendor id 0x1038.
const SUPPORTED: [(u16, &str); 2] = [(0x2232, "Arctis Nova 5"), (0x2253, "Arctis Nova 5X")];
const REPORT_LEN: usize = 65; // report id + 64 bytes
const POLL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub name: &'static str,
    /// None while the headset is off / out of range (the dongle still answers).
    pub percent: Option<u8>,
    pub charging: bool,
}

/// Reply to `00 b0`: `00 b0 <status> <status> <battery %> ..`.
/// Observed on an Arctis Nova 5 (the dongle also announces battery changes by
/// itself as event `00 b7 <battery %> <status>` on MI_05):
///   `00 b0 02 02 30 01`  on battery, 48 %
///   `00 b0 03 03 39 01`  charger plugged in (the level jumps up), 57 %
/// Any other status (headset off / out of range) shows no level.
pub fn decode(name: &'static str, r: &[u8]) -> Option<Battery> {
    if r.len() < 6 || r[1] != 0xb0 {
        return None;
    }
    let connected = matches!(r[2], 0x02 | 0x03) && r[4] <= 100;
    Some(Battery { name, percent: connected.then_some(r[4]), charging: connected && r[2] == 0x03 })
}

/// Device paths of the vendor interface of every supported dongle present.
fn find() -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(set) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
            return out;
        };
        for i in 0.. {
            let mut d = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInterfaces(set, None, &guid, i, &mut d).is_err() {
                break;
            }
            let mut need = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &d, None, 0, Some(&mut need), None);
            if need == 0 {
                continue;
            }
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &d, Some(detail), need, None, None).is_err() {
                continue;
            }
            let path = PCWSTR((*detail).DevicePath.as_ptr()).to_string().unwrap_or_default();
            let lower = path.to_lowercase();
            if !lower.contains("vid_1038&") || !lower.contains("&mi_03") {
                continue;
            }
            if let Some((_, name)) = SUPPORTED.iter().find(|(pid, _)| lower.contains(&format!("pid_{pid:04x}"))) {
                out.push((path, *name));
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    out
}

/// Wait for one overlapped transfer, cancelling it after `ms`.
unsafe fn finish(h: HANDLE, ov: &OVERLAPPED, ms: u32) -> Option<u32> {
    if WaitForSingleObject(ov.hEvent, ms) != WAIT_OBJECT_0 {
        let _ = CancelIoEx(h, Some(ov));
    }
    let mut n = 0u32;
    GetOverlappedResult(h, ov, &mut n, true).ok().map(|_| n)
}

/// Send the status query and return the raw reply.
pub fn query(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let h = CreateFileW(
            PCWSTR(wide.as_ptr()),
            (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
        .ok()?;
        let ev = CreateEventW(None, true, false, None).ok();
        let result = ev.and_then(|ev| {
            let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
            let mut req = [0u8; REPORT_LEN];
            req[1] = 0xb0;
            let _ = WriteFile(h, Some(&req), None, Some(&mut ov));
            finish(h, &ov, 500)?;
            let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
            let mut reply = vec![0u8; REPORT_LEN];
            let _ = ReadFile(h, Some(&mut reply), None, Some(&mut ov));
            let n = finish(h, &ov, 1000)? as usize;
            reply.truncate(n);
            Some(reply)
        });
        if let Some(ev) = ev {
            let _ = CloseHandle(ev);
        }
        let _ = CloseHandle(h);
        result
    }
}

/// Current battery of the first supported headset, if one is plugged in.
pub fn read() -> Option<Battery> {
    find().into_iter().find_map(|(path, name)| query(&path).and_then(|r| decode(name, &r)))
}

/// Poll in the background and keep the tray's battery icon up to date.
pub fn spawn() {
    std::thread::Builder::new()
        .name("headset".into())
        .spawn(|| loop {
            crate::tray::set_battery(read());
            std::thread::sleep(POLL);
        })
        .ok();
}

/// `bitgg --headset`: print what the dongle answers, for checking the decoding.
pub fn diag() {
    let found = find();
    if found.is_empty() {
        println!("no supported SteelSeries dongle found");
    }
    for (path, name) in found {
        match query(&path) {
            Some(r) => {
                let hex: Vec<String> = r.iter().take(12).map(|b| format!("{b:02x}")).collect();
                println!("{name}: reply {}  ->  {:?}", hex.join(" "), decode(name, &r));
            }
            None => println!("{name}: no reply"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_observed_replies() {
        let on_battery = decode("Arctis Nova 5", &[0x00, 0xb0, 0x02, 0x02, 0x30, 0x01, 0, 0]).unwrap();
        assert_eq!((on_battery.percent, on_battery.charging), (Some(48), false));
        let charging = decode("Arctis Nova 5", &[0x00, 0xb0, 0x03, 0x03, 0x39, 0x01, 0, 0]).unwrap();
        assert_eq!((charging.percent, charging.charging), (Some(57), true));
    }

    #[test]
    fn unknown_status_shows_no_level() {
        let off = decode("x", &[0x00, 0xb0, 0x01, 0x01, 0x00, 0x00]).unwrap();
        assert_eq!((off.percent, off.charging), (None, false));
    }

    #[test]
    fn rejects_other_reports() {
        assert!(decode("x", &[0x00, 0xb1, 0x03, 0x03, 0x0a, 0x03]).is_none());
        assert!(decode("x", &[0x00, 0xb0]).is_none());
        assert_eq!(decode("x", &[0x00, 0xb0, 0x02, 0x02, 0xff, 0x01]).unwrap().percent, None);
    }
}
