//! Spawns the platform shell with one fixed command through `PortablePtyBackend` and prints its
//! output. Also measures the release-size cost of the backend
//! (`cargo build --release -p lune-pty --example spawn`).

use std::io::Read;

use lune_pty::{PortablePtyBackend, PtyBackend, PtySize, SpawnOptions};

const MARKER: &str = "lune-pty-smoke";

fn main() {
    #[cfg(windows)]
    let options = SpawnOptions {
        program: Some(
            std::env::var("ComSpec")
                .unwrap_or_else(|_| "cmd.exe".to_string())
                .into(),
        ),
        args: vec!["/d".into(), "/c".into(), format!("echo {MARKER}")],
        ..SpawnOptions::default()
    };
    #[cfg(not(windows))]
    let options = SpawnOptions {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), format!("echo {MARKER}")],
        ..SpawnOptions::default()
    };

    let size = PtySize {
        columns: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    };
    let mut pty = PortablePtyBackend::new()
        .spawn(&options, size)
        .expect("spawn");
    let mut reader = pty.take_reader().expect("reader");
    let writer = pty.writer();

    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    let mut answered = 0;
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                // The host answers ConPTY's cursor-position request (see PortablePtyBackend).
                let asked = out.windows(4).filter(|w| *w == b"\x1b[6n").count();
                while answered < asked {
                    let _ = writer.write_all(b"\x1b[1;1R");
                    answered += 1;
                }
                if String::from_utf8_lossy(&out).contains(MARKER) {
                    break;
                }
            }
        }
    }
    println!("{}", String::from_utf8_lossy(&out));
    while let Ok(None) = pty.try_exit_code() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
