// Windows only: embeds the app icon (assets/icon/luneterm.ico, made by scripts/gen-icons.sh) in
// luneterm.exe, so Explorer, the taskbar and the shortcuts show it. Other targets do nothing. The
// build dependency is matched against the host: build on Windows (cross builds get no icon).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon/luneterm.ico");
    // `CARGO_CFG_TARGET_OS` is the target, not the host (the build script itself runs on the host).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_icon();
    }
}

#[cfg(windows)]
fn embed_icon() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon/luneterm.ico");
    if let Err(err) = res.compile() {
        // A build with no icon is worse than a failed build: nobody would notice it.
        panic!("embedding the app icon failed: {err}");
    }
}

#[cfg(not(windows))]
fn embed_icon() {}
