//! Luneterm: a standalone glass terminal. The app is `luneterm::app`; this is its entry point.

#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    luneterm::app::run()
}
