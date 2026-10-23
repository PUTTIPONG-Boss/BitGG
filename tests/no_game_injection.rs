//! Anti-cheat safety guard.
//!
//! BitGG must never touch a game process: no handles to other processes, no
//! memory reads/writes, no remote threads, no DLL injection, no in-process
//! hooks (only WH_KEYBOARD_LL, which runs inside BitGG itself), no swap-chain /
//! Present hooking. Audio goes through the Windows audio service and video
//! through DXGI Desktop Duplication, both from outside the game.
//!
//! This test scans the source and fails if any of those APIs appear, so the
//! promise in the README can't silently break.

const FORBIDDEN: &[&str] = &[
    // touching another process
    "OpenProcess",
    "OpenThread",
    "ReadProcessMemory",
    "WriteProcessMemory",
    "VirtualAllocEx",
    "VirtualProtectEx",
    "CreateRemoteThread",
    "NtCreateThreadEx",
    "RtlCreateUserThread",
    "QueueUserAPC",
    "SetThreadContext",
    "SuspendThread",
    "DebugActiveProcess",
    "DuplicateHandle",
    // DLL loading / injection
    "LoadLibrary",
    "LdrLoadDll",
    // hooks that get injected into other processes (WH_KEYBOARD_LL is allowed)
    "WH_KEYBOARD)",
    "WH_KEYBOARD,",
    "WH_MOUSE,",
    "WH_MOUSE)",
    "WH_GETMESSAGE",
    "WH_CALLWNDPROC",
    "WH_CBT",
    "WH_SHELL",
    "SetWinEventHook",
    // in-game overlay / capture hooking
    "IDXGISwapChain",
    "Present1",
    "vkQueuePresent",
    "wglSwapBuffers",
];

#[test]
fn never_touches_game_processes() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map(|e| e != "rs").unwrap_or(true) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue; // comments may explain what we avoid
            }
            for f in FORBIDDEN {
                if code.contains(f) {
                    hits.push(format!("{}:{}: {}", path.file_name().unwrap().to_string_lossy(), n + 1, f));
                }
            }
        }
    }
    assert!(hits.is_empty(), "game-process APIs found (anti-cheat risk):\n{}", hits.join("\n"));
}
