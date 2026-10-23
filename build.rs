// Embed the pixel-art icon (resource id 1) so the exe, taskbar, tray and
// shortcuts all show it. Uses MinGW windres (GNU toolchain).
fn main() {
    println!("cargo:rerun-if-changed=assets/bitgg.rc");
    println!("cargo:rerun-if-changed=assets/bitgg.ico");
    let out = std::env::var("OUT_DIR").unwrap();
    let obj = format!("{out}/bitgg_res.o");
    let ok = std::process::Command::new("windres")
        .args(["assets/bitgg.rc", "-O", "coff", "-o", &obj])
        .current_dir(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={obj}");
    } else {
        println!("cargo:warning=windres not found: building without an icon");
    }
}
