// Embed the pixel-art icon (resource id 1) so the exe, taskbar, tray and
// shortcuts all show it. embed-resource uses rc.exe on MSVC and windres on GNU.
fn main() {
    println!("cargo:rerun-if-changed=assets/bitgg.ico");
    let _ = embed_resource::compile("assets/bitgg.rc", embed_resource::NONE);
}
