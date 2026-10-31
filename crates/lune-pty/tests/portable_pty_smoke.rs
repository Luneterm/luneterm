//! Upstream canary: portable-pty used directly (not through `lune-pty`). If this fails on an OS,
//! the P7/P8 premise (portable-pty spawns a shell there) is broken. Keep it in the suite.

use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const MARKER: &str = "lune-pty-smoke";
const DEADLINE: Duration = Duration::from_secs(30);

fn command() -> CommandBuilder {
    #[cfg(windows)]
    {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut cmd = CommandBuilder::new(comspec);
        cmd.args(["/d", "/c", &format!("echo {MARKER}")]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", &format!("echo {MARKER}")]);
        cmd
    }
}

#[test]
fn spawn_echo_and_exit_zero() {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut child = pair.slave.spawn_command(command()).expect("spawn");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    // On Windows the writer answers ConPTY's cursor-position request (ESC[6n).
    let mut writer = pair.master.take_writer().expect("writer");

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let start = Instant::now();
    let mut seen: Vec<u8> = Vec::new();
    let mut answered = 0usize;
    let mut found = false;
    while start.elapsed() < DEADLINE {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                seen.extend_from_slice(&chunk);
                let dsr = seen.windows(4).filter(|w| *w == b"\x1b[6n").count();
                while answered < dsr {
                    use std::io::Write;
                    writer.write_all(b"\x1b[1;1R").expect("answer DSR");
                    writer.flush().expect("flush");
                    answered += 1;
                }
                let text = String::from_utf8_lossy(&seen);
                if text.contains(MARKER) {
                    found = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(
        found,
        "marker not seen within {DEADLINE:?}; output so far: {:?}",
        String::from_utf8_lossy(&seen)
    );

    // Wait for exit by polling until the deadline.
    let code = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break status.exit_code(),
            None => {
                assert!(start.elapsed() < DEADLINE, "child did not exit in time");
                // Keep answering/draining so ConPTY can finish.
                while let Ok(chunk) = rx.try_recv() {
                    seen.extend_from_slice(&chunk);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    assert_eq!(code, 0);
}
