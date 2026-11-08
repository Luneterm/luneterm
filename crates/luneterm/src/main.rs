//! Luneterm: a standalone glass terminal. The app is `luneterm::app`; this is its entry point.

#![forbid(unsafe_code)]
// The release build on Windows is a GUI program: no console window. Stderr diagnostics still
// reach a handle the launcher redirected.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    // First, before the event loop or any GPU object: the GPU stack loads `dxcompiler.dll` by bare
    // name, and with the default search order a copy in the current directory or on PATH wins
    // (plan M9 D17). No GPU with the legacy order.
    if let Err(e) = lune_pty::restrict_dll_search() {
        eprintln!("luneterm: cannot restrict the DLL search path: {e}");
        return std::process::ExitCode::from(1);
    }
    luneterm::app::run()
}
