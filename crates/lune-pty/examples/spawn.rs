//! Spawns the platform shell with one fixed command and prints its output. Exists to measure the
//! release-size cost of portable-pty (`cargo build --release -p lune-pty --example spawn`).

use std::io::{Read, Write};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

fn main() {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    #[cfg(windows)]
    let cmd = {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut c = CommandBuilder::new(comspec);
        c.args(["/d", "/c", "echo lune-pty-smoke"]);
        c
    };
    #[cfg(not(windows))]
    let cmd = {
        let mut c = CommandBuilder::new("/bin/sh");
        c.args(["-c", "echo lune-pty-smoke"]);
        c
    };

    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");

    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write_all(b"\x1b[1;1R");
                }
                if String::from_utf8_lossy(&out).contains("lune-pty-smoke") {
                    break;
                }
            }
        }
    }
    println!("{}", String::from_utf8_lossy(&out));
    let _ = child.wait();
}
