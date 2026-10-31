//! Behaviour tests for `PortablePtyBackend` (M3 step 1). Every wait is a deadline poll on an output
//! marker, an exit code or EOF; no test relies on a fixed sleep.

mod support;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lune_pty::{PortablePtyBackend, PtyBackend, RawMode, SpawnOptions};
use support::{DEADLINE, Session, command, size};

/// A fresh, unique directory under the temp dir; removed by the caller.
fn unique_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("lune-pty-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).expect("create temp dir");
    dir
}

fn spawn_err(options: &SpawnOptions, columns: u16, rows: u16) -> io::ErrorKind {
    match PortablePtyBackend::new().spawn(options, size(columns, rows)) {
        Ok(_) => panic!("spawn accepted {options:?} at {columns}x{rows}"),
        Err(e) => e.kind(),
    }
}

#[cfg(unix)]
mod unix {
    use super::*;

    fn sh(script: &str) -> SpawnOptions {
        command("/bin/sh", &["-c", script])
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn echo_round_trip_then_eof_and_exit_zero() {
        let mut s = Session::spawn(&sh(r#"read x; echo "got:$x""#), size(80, 24));
        s.write(b"abc\n");
        s.wait_for("got:abc", DEADLINE);
        s.wait_eof(DEADLINE);
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn exit_codes_are_reported_and_stable() {
        for code in [0, 7, 255] {
            let mut s = Session::spawn(&sh(&format!("exit {code}")), size(80, 24));
            assert_eq!(s.wait_exit(DEADLINE), code);
            assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), Some(code));
            assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), Some(code));
        }
    }

    #[test]
    fn running_then_kill() {
        let mut s = Session::spawn(&sh("echo R; sleep 30"), size(80, 24));
        s.wait_for("R", DEADLINE);
        assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), None);
        s.pty.kill().expect("kill");
        let code = s.wait_exit(DEADLINE);
        assert!(matches!(code, 129 | 137), "exit code {code}");
        s.pty.kill().expect("second kill");
        assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), Some(code));
    }

    #[test]
    fn initial_size() {
        let mut s = Session::spawn(&sh("stty size"), size(90, 20));
        let text = s.wait_for("20 90", DEADLINE);
        assert!(lines(&text).contains(&"20 90"), "output: {text:?}");
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn resize_reaches_the_child() {
        let mut s = Session::spawn(&sh("echo R; read _; stty size"), size(80, 24));
        s.wait_for("R", DEADLINE);
        s.pty.resize(size(100, 30)).expect("resize");
        s.write(b"\n");
        let text = s.wait_for("30 100", DEADLINE);
        assert!(lines(&text).contains(&"30 100"), "output: {text:?}");
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn env_is_passed_exactly() {
        let mut options = sh(r#"printf 'V=%s|%s|%s\n' "$TERM" "$COLORTERM" "$LUNE_PTY_SENTINEL""#);
        options.env = vec![
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("LUNE_PTY_SENTINEL".into(), "sentinel value 42".into()),
        ];
        let mut s = Session::spawn(&options, size(80, 24));
        let want = "V=xterm-256color|truecolor|sentinel value 42";
        let text = s.wait_for(want, DEADLINE);
        assert!(lines(&text).contains(&want), "output: {text:?}");
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn no_terminal_identity_is_added() {
        // Quiet: the output is the environment, which can hold secrets.
        let mut s = Session::spawn(&command("/usr/bin/env", &[]), size(200, 24)).quiet();
        assert_eq!(s.wait_exit(DEADLINE), 0);
        s.wait_eof(DEADLINE);
        let text = s.text();
        for key in [
            "TERM",
            "COLORTERM",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "LC_TERMINAL",
            "LC_TERMINAL_VERSION",
        ] {
            let prefix = format!("{key}=");
            let child = text
                .lines()
                .find_map(|line| line.strip_prefix(prefix.as_str()));
            let ours = std::env::var(key).ok();
            // Never print the whole environment: it can hold secrets.
            assert_eq!(child, ours.as_deref(), "{key} differs");
        }
    }

    #[test]
    fn cwd_is_used() {
        let dir = unique_dir("cwd");
        let canonical = dir.canonicalize().expect("canonicalize");
        let mut options = command("/bin/pwd", &["-P"]);
        options.cwd = Some(dir.clone());
        let mut s = Session::spawn(&options, size(200, 24));
        assert_eq!(s.wait_exit(DEADLINE), 0);
        s.wait_eof(DEADLINE);
        let text = s.text();
        let _ = std::fs::remove_dir(&dir);
        let want = canonical.to_str().expect("utf-8 temp path");
        assert!(
            lines(&text).contains(&want),
            "want {want:?}, output: {text:?}"
        );
    }

    #[test]
    fn default_program_is_a_login_shell() {
        let options = SpawnOptions {
            env: vec![("SHELL".into(), "/bin/sh".into())],
            ..SpawnOptions::default()
        };
        let mut s = Session::spawn(&options, size(80, 24));
        s.write(b"echo \"$0\"; exit\n");
        s.wait_for("-sh", DEADLINE);
        s.wait_eof(DEADLINE);
        let text = s.text();
        assert!(
            // The prompt (`$ ` / `# `) can share the line, since the input was typed ahead.
            text.lines()
                .any(|line| line.split_whitespace().last() == Some("-sh")),
            "output: {text:?}"
        );
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn drop_while_running_kills_and_reaps() {
        let s = Session::spawn(&sh("echo R; sleep 30"), size(80, 24));
        s.wait_for("R", DEADLINE);
        let pid = s.pty.process_id().expect("pid");
        drop(s);
        let start = std::time::Instant::now();
        loop {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .expect("run kill -0")
                .success();
            if !alive {
                break;
            }
            assert!(start.elapsed() < DEADLINE, "pid {pid} still alive");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Polls `check` until it returns `Some`; panics with `what` at the deadline.
    fn poll<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
        let start = std::time::Instant::now();
        loop {
            if let Some(found) = check() {
                return found;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "{what} not reached within {DEADLINE:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Spawns `script`, waits for `marker`, and returns the session with the termios state final.
    fn after(script: &str, marker: &str) -> Session {
        let s = Session::spawn(&sh(script), size(80, 24));
        s.wait_for(marker, DEADLINE);
        s
    }

    #[test]
    fn raw_mode_cooked() {
        let s = after("stty sane && echo C && sleep 30", "C");
        assert_eq!(s.pty.raw_mode(), RawMode::Cooked);
    }

    #[test]
    fn raw_mode_raw() {
        let s = after("stty raw -echo && echo R && sleep 30", "R");
        assert_eq!(s.pty.raw_mode(), RawMode::Raw);
    }

    #[test]
    fn raw_mode_non_canonical_only() {
        let s = after("stty -icanon && echo B && sleep 30", "B");
        assert_eq!(s.pty.raw_mode(), RawMode::Raw);
    }

    #[test]
    fn raw_mode_follows_changes() {
        let mut s = after(
            "stty sane; echo C; read _; stty -icanon; echo B; sleep 30",
            "C",
        );
        assert_eq!(s.pty.raw_mode(), RawMode::Cooked);
        s.write(b"\n");
        s.wait_for("B", DEADLINE);
        assert_eq!(s.pty.raw_mode(), RawMode::Raw);
    }

    #[test]
    fn raw_mode_after_exit_returns() {
        let mut s = Session::spawn(&sh("exit 0"), size(80, 24));
        assert_eq!(s.wait_exit(DEADLINE), 0);
        s.wait_eof(DEADLINE);
        // Any variant is fine: the call must simply return.
        let _ = s.pty.raw_mode();
        let _ = s.pty.foreground_process();
    }

    #[test]
    fn foreground_process_follows_the_job() {
        let mut options = command("/bin/sh", &["-i"]);
        options.env = vec![("PS1".into(), "lune$ ".into())];
        let mut s = Session::spawn(&options, size(80, 24));
        let pid = s.pty.process_id().expect("pid");
        s.wait_for("lune$ ", DEADLINE);
        // Job control may take a moment to set the foreground group at startup.
        poll("shell is foreground", || {
            (s.pty.foreground_process() == Some(pid)).then_some(())
        });
        s.write(b"sleep 30\n");
        let job = poll("a foreground job", || {
            s.pty.foreground_process().filter(|p| *p != pid)
        });
        assert_ne!(job, pid);
        s.write(b"\x03");
        poll("shell is foreground again", || {
            (s.pty.foreground_process() == Some(pid)).then_some(())
        });
    }

    pub(super) fn long_running() -> SpawnOptions {
        sh("sleep 30")
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    const READY: &str = "LUNE_READY";

    fn cmd(args: &[&str]) -> SpawnOptions {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        command(&comspec, args)
    }

    /// An interactive `cmd` that prints `READY` once it is up.
    fn interactive() -> SpawnOptions {
        cmd(&["/d", "/q", "/k", "echo", READY])
    }

    #[test]
    fn echo_round_trip_then_eof_and_exit_zero() {
        let mut s = Session::spawn(&interactive(), size(120, 30));
        s.wait_for(READY, DEADLINE);
        s.write(b"set /a 1234*4\r");
        s.wait_for("4936", DEADLINE);
        s.write(b"exit\r");
        assert_eq!(s.wait_exit(DEADLINE), 0);
        s.wait_eof(DEADLINE);
    }

    #[test]
    fn exit_codes_are_reported_and_stable() {
        for code in [0, 7, 255] {
            let mut s =
                Session::spawn(&cmd(&["/d", "/c", "exit", &code.to_string()]), size(80, 24));
            assert_eq!(s.wait_exit(DEADLINE), code);
            assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), Some(code));
            assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), Some(code));
            s.wait_eof(DEADLINE);
        }
    }

    #[test]
    fn running_then_kill() {
        let mut s = Session::spawn(&interactive(), size(120, 30));
        s.wait_for(READY, DEADLINE);
        assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), None);
        s.pty.kill().expect("kill");
        let code = s.wait_exit(DEADLINE);
        s.wait_eof(DEADLINE);
        s.pty.kill().expect("second kill");
        assert_eq!(s.pty.try_exit_code().expect("try_exit_code"), Some(code));
    }

    #[test]
    fn resize_reaches_the_child() {
        let mut s = Session::spawn(&interactive(), size(80, 30));
        s.wait_for(READY, DEADLINE);
        s.pty.resize(size(100, 30)).expect("resize");
        s.write(b"mode con\r");
        let text = s.wait_for("Columns:", DEADLINE);
        // Wait for the value too: it may arrive in a later read.
        let start = std::time::Instant::now();
        let mut text = text;
        loop {
            let value = text
                .split("Columns:")
                .nth(1)
                .map(|rest| rest.trim_start().chars().take(3).collect::<String>());
            if value.as_deref() == Some("100") {
                break;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "Columns not 100; output: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
            text = s.text();
        }
        s.write(b"exit\r");
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn env_is_passed_exactly() {
        let mut options = cmd(&[
            "/d",
            "/c",
            "echo",
            "V=%TERM%+%COLORTERM%+%LUNE_PTY_SENTINEL%",
        ]);
        options.env = vec![
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("LUNE_PTY_SENTINEL".into(), "sentinel-42".into()),
        ];
        let mut s = Session::spawn(&options, size(120, 24));
        s.wait_for("V=xterm-256color+truecolor+sentinel-42", DEADLINE);
        assert_eq!(s.wait_exit(DEADLINE), 0);
    }

    #[test]
    fn cwd_is_used() {
        let dir = unique_dir("cwd");
        let leaf = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("leaf")
            .to_string();
        let mut options = cmd(&["/d", "/c", "cd"]);
        options.cwd = Some(dir.clone());
        let mut s = Session::spawn(&options, size(250, 24));
        s.wait_for(&leaf, DEADLINE);
        assert_eq!(s.wait_exit(DEADLINE), 0);
        drop(s);
        let _ = std::fs::remove_dir(&dir);
    }

    pub(super) fn long_running() -> SpawnOptions {
        interactive()
    }
}

#[cfg(unix)]
use unix::long_running;
#[cfg(windows)]
use windows::long_running;

#[test]
fn validation_fails_closed() {
    let ok = long_running();
    for (c, r) in [(0, 24), (80, 0), (32768, 24), (80, 32768)] {
        assert_eq!(spawn_err(&ok, c, r), io::ErrorKind::InvalidInput, "{c}x{r}");
    }

    let mut missing = long_running();
    missing.cwd = Some(std::env::temp_dir().join("lune-pty-no-such-dir-7f3a9c"));
    assert_eq!(spawn_err(&missing, 80, 24), io::ErrorKind::NotFound);

    let args_only = SpawnOptions {
        args: vec!["-c".into(), "true".into()],
        ..SpawnOptions::default()
    };
    assert_eq!(spawn_err(&args_only, 80, 24), io::ErrorKind::InvalidInput);

    let mut pty = PortablePtyBackend::new()
        .spawn(&ok, size(80, 24))
        .expect("spawn");
    for (c, r) in [(0, 24), (80, 0), (32768, 24), (80, 32768)] {
        let err = pty.resize(size(c, r)).expect_err("resize accepted");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{c}x{r}");
    }
}

#[test]
fn reader_is_handed_over_once() {
    let mut pty = PortablePtyBackend::new()
        .spawn(&long_running(), size(80, 24))
        .expect("spawn");
    let reader = pty.take_reader().expect("first take_reader");
    assert!(pty.take_reader().is_err(), "second take_reader succeeded");
    let mut buf = [0u8; 16];
    assert!(pty.read(&mut buf).is_err(), "Pty::read after take_reader");
    drop(reader);
}

#[cfg(windows)]
#[test]
fn raw_mode_and_foreground_are_unsupported() {
    let pty = PortablePtyBackend::new()
        .spawn(&long_running(), size(80, 24))
        .expect("spawn");
    assert_eq!(pty.raw_mode(), RawMode::Unknown);
    assert_eq!(pty.foreground_process(), None);
}
