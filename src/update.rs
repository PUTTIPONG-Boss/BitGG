//! Self-update from GitHub Releases, using only what Windows ships
//! (WinHTTP for HTTPS, CNG for SHA-256).
//!
//! 1. GET api.github.com/repos/<repo>/releases/latest (no data about the user
//!    is sent; just the request). Can be turned off in SETUP.
//! 2. If its tag is newer than this build, download BitGG.exe and
//!    SHA256SUMS.txt from that release; keep the exe only if the hash matches.
//! 3. "Restart to update": rename the running exe to *.exe.old (Windows allows
//!    renaming a running exe, not overwriting it), move the new file into its
//!    place, start it, exit. The new build deletes *.exe.old on startup.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

pub const REPO: &str = "PUTTIPONG-Boss/BitGG";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// verified new exe saved next to the running one
    Ready { version: String, path: PathBuf },
    Error(String),
}

/// This build's version (BITGG_FAKE_VERSION lets tests pretend to be older).
pub fn current() -> String {
    std::env::var("BITGG_FAKE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

pub fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

/// HTTPS GET with WinHTTP (follows GitHub's redirects to its download CDN).
pub fn http_get(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("https only")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    unsafe {
        let ua = HSTRING::from(concat!("BitGG/", env!("CARGO_PKG_VERSION")));
        let session = WinHttpOpen(&ua, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            return Err("winhttp open failed".into());
        }
        let close = |h: *mut core::ffi::c_void| {
            if !h.is_null() {
                let _ = WinHttpCloseHandle(h);
            }
        };
        let conn = WinHttpConnect(session, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT as u16, 0);
        if conn.is_null() {
            close(session);
            return Err("connect failed".into());
        }
        let req = WinHttpOpenRequest(conn, &HSTRING::from("GET"), &HSTRING::from(path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE);
        let result = (|| -> Result<Vec<u8>, String> {
            if req.is_null() {
                return Err("request failed".into());
            }
            let headers: Vec<u16> = "Accept: application/vnd.github+json\r\n".encode_utf16().collect();
            WinHttpSendRequest(req, Some(&headers), None, 0, 0, 0).map_err(|e| format!("send: {}", e.message()))?;
            WinHttpReceiveResponse(req, std::ptr::null_mut()).map_err(|e| format!("receive: {}", e.message()))?;
            let mut status = 0u32;
            let mut len = 4u32;
            WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut _), &mut len, std::ptr::null_mut())
                .map_err(|e| format!("status: {}", e.message()))?;
            if status != 200 {
                return Err(format!("HTTP {status}"));
            }
            let mut body = Vec::new();
            loop {
                let mut avail = 0u32;
                WinHttpQueryDataAvailable(req, &mut avail).map_err(|e| format!("read: {}", e.message()))?;
                if avail == 0 {
                    break;
                }
                let start = body.len();
                body.resize(start + avail as usize, 0);
                let mut got = 0u32;
                WinHttpReadData(req, body[start..].as_mut_ptr() as *mut _, avail, &mut got).map_err(|e| format!("read: {}", e.message()))?;
                body.truncate(start + got as usize);
                if body.len() > max_bytes {
                    return Err("response too large".into());
                }
            }
            Ok(body)
        })();
        close(req);
        close(conn);
        close(session);
        result
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut out = [0u8; 32];
    unsafe {
        let _ = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out);
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

fn exe_paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");
    Some((exe, new, old))
}

/// Remove what a previous update left behind (call at startup).
pub fn cleanup() {
    if let Some((_, new, old)) = exe_paths() {
        let _ = std::fs::remove_file(old);
        let _ = std::fs::remove_file(new);
    }
}

/// Check GitHub; if newer, download + verify into BitGG.exe.new. Blocking.
pub fn check_and_fetch() -> State {
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let json = match http_get(&api, 1 << 20) {
        Ok(b) => b,
        Err(e) => return State::Error(format!("check: {e}")),
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&json) else { return State::Error("bad release info".into()) };
    let tag = v["tag_name"].as_str().unwrap_or_default().to_string();
    if !is_newer(&tag, &current()) {
        return State::UpToDate;
    }
    let asset = |name: &str| {
        v["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name)).and_then(|x| x["browser_download_url"].as_str()).map(String::from)
    };
    let (Some(exe_url), Some(sums_url)) = (asset("BitGG.exe"), asset("SHA256SUMS.txt")) else {
        return State::Error(format!("{tag}: release files missing"));
    };
    let fetched = (|| -> Result<PathBuf, String> {
        let sums = String::from_utf8_lossy(&http_get(&sums_url, 64 << 10)?).to_string();
        let want = sums.lines().find(|l| l.contains("BitGG.exe")).and_then(|l| l.split_whitespace().next()).ok_or("no checksum for BitGG.exe")?.to_lowercase();
        let bin = http_get(&exe_url, 64 << 20)?;
        if bin.len() < 100_000 || !bin.starts_with(b"MZ") {
            return Err("download is not an exe".into());
        }
        let got = sha256_hex(&bin);
        if got != want {
            return Err("checksum mismatch: not installing".into());
        }
        let (_, new, _) = exe_paths().ok_or("exe path")?;
        std::fs::write(&new, &bin).map_err(|e| format!("can't write next to BitGG.exe ({e}); move it to a writable folder"))?;
        Ok(new)
    })();
    match fetched {
        Ok(path) => State::Ready { version: tag, path },
        Err(e) => State::Error(e),
    }
}

/// Background check that reports through `state`.
pub fn spawn_check(state: Arc<Mutex<State>>) {
    {
        let mut s = state.lock().unwrap();
        if matches!(*s, State::Checking | State::Downloading(_) | State::Ready { .. }) {
            return; // already busy or done
        }
        *s = State::Checking;
    }
    std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let r = check_and_fetch();
            *state.lock().unwrap() = r;
        })
        .ok();
}

/// Put the verified new exe in place of the running one. On failure the
/// original file is put back.
pub fn swap_in(new: &Path) -> Result<PathBuf, String> {
    let (exe, _, old) = exe_paths().ok_or("exe path")?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("rename current: {e}"))?;
    if let Err(e) = std::fs::rename(new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("install new: {e}"));
    }
    Ok(exe)
}

/// Undo swap_in (if the new exe fails to start).
pub fn swap_back() {
    if let Some((exe, new, old)) = exe_paths() {
        let _ = std::fs::rename(&exe, &new);
        let _ = std::fs::rename(&old, &exe);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.1.2", "0.1.1"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
        assert!(!is_newer("v0.1.0", "0.1.1"));
        assert!(!is_newer("garbage", "0.1.1"));
        assert!(!is_newer("v0.1", "0.0.1")); // malformed tag never wins
    }

    #[test]
    fn sha256_known_vectors() {
        // FIPS 180-2 test vectors
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
