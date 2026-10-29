//! Luneterm app: window, event loop and session wiring. Placeholder until the engine port and the
//! glass window spike land.

fn main() {
    println!(
        "luneterm {} (xterm.js {} port)",
        env!("CARGO_PKG_VERSION"),
        lune_vt::XTERM_JS_TAG
    );
}
