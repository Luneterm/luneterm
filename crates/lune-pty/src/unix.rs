//! Unix parts of the portable-pty backend.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;

use nix::fcntl::OFlag;
use nix::sys::termios::{self, LocalFlags, Termios};
use portable_pty::{Child, MasterPty};

use crate::RawMode;

/// Non-blocking exit code. A signal death reports `128 + signo` (shell convention); portable-pty's
/// own `ExitStatus` reports 1 for every signal, so the std child is read directly.
pub(crate) fn try_exit_code(child: &mut (dyn Child + Send + Sync)) -> io::Result<Option<i32>> {
    let child: &mut dyn Child = child;
    if let Some(std_child) = child.downcast_mut::<std::process::Child>() {
        return Ok(std_child.try_wait()?.map(|status| {
            status
                .code()
                .or_else(|| status.signal().map(|signo| 128 + signo))
                .unwrap_or(1)
        }));
    }
    // Not reached with portable-pty 0.9.0 (its Unix child is a std::process::Child).
    Ok(child
        .try_wait()?
        .map(|status| i32::try_from(status.exit_code()).unwrap_or(1)))
}

fn mode_of(termios: &Termios) -> RawMode {
    if termios.local_flags.contains(LocalFlags::ICANON) {
        RawMode::Cooked
    } else {
        RawMode::Raw
    }
}

/// Line discipline read with `tcgetattr` on the master. `None` when the OS refuses (older macOS
/// is reported to) or the master has no termios.
pub(crate) fn raw_mode_from_master(master: &dyn MasterPty) -> Option<RawMode> {
    master.get_termios().as_ref().map(mode_of)
}

/// Line discipline read by opening the slave `path` transiently. `O_NONBLOCK` keeps the open from
/// waiting, `O_NOCTTY` keeps it from becoming a controlling terminal, and the `File` closes the
/// descriptor on every path (a leaked slave descriptor would starve the reader's EOF).
pub(crate) fn raw_mode_from_slave(path: &Path) -> Option<RawMode> {
    let slave = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOCTTY).bits())
        .open(path)
        .ok()?;
    termios::tcgetattr(&slave).ok().as_ref().map(mode_of)
}

/// Total: the master first, then the slave by name, else `Unknown`.
pub(crate) fn raw_mode(master: &dyn MasterPty) -> RawMode {
    raw_mode_from_master(master)
        .or_else(|| {
            master
                .tty_name()
                .and_then(|path| raw_mode_from_slave(&path))
        })
        .unwrap_or(RawMode::Unknown)
}

/// Process id of the foreground process group leader (`tcgetpgrp` on the master).
pub(crate) fn foreground_process(master: &dyn MasterPty) -> Option<u32> {
    master
        .process_group_leader()
        .and_then(|pid| u32::try_from(pid).ok())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(20);

    /// A child on a fresh PTY that has printed `marker` (so its termios state is final), plus the
    /// master. The child sleeps afterwards; dropping the guard kills it.
    struct Fixture {
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn fixture(script: &str, marker: &str) -> Fixture {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", script]);
        let child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 256];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let fixture = Fixture {
            master: pair.master,
            child,
        };
        let start = Instant::now();
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains(marker) {
            let left = DEADLINE.saturating_sub(start.elapsed());
            assert!(!left.is_zero(), "marker {marker:?} not seen");
            seen.extend(rx.recv_timeout(left).expect("child output"));
        }
        fixture
    }

    fn cooked() -> Fixture {
        fixture("stty sane; echo C; sleep 30", "C")
    }

    fn raw() -> Fixture {
        fixture("stty raw -echo; echo R; sleep 30", "R")
    }

    #[test]
    fn slave_path_reports_both_states() {
        let c = cooked();
        let path = c.master.tty_name().expect("tty_name");
        assert_eq!(raw_mode_from_slave(&path), Some(RawMode::Cooked));
        let r = raw();
        let path = r.master.tty_name().expect("tty_name");
        assert_eq!(raw_mode_from_slave(&path), Some(RawMode::Raw));
    }

    #[test]
    fn master_path_is_correct_on_linux_and_correct_or_absent_on_macos() {
        for (f, want) in [(cooked(), RawMode::Cooked), (raw(), RawMode::Raw)] {
            let got = raw_mode_from_master(f.master.as_ref());
            eprintln!("raw_mode_from_master -> {got:?} (want {want:?})");
            if cfg!(target_os = "linux") {
                assert_eq!(got, Some(want));
            } else {
                assert!(got.is_none() || got == Some(want), "got {got:?}");
            }
        }
    }

    #[test]
    fn combined_query_is_correct_and_total() {
        assert_eq!(raw_mode(cooked().master.as_ref()), RawMode::Cooked);
        assert_eq!(raw_mode(raw().master.as_ref()), RawMode::Raw);
    }

    #[test]
    fn slave_open_failure_is_none() {
        assert_eq!(
            raw_mode_from_slave(Path::new("/nonexistent/lune-pty-tty")),
            None
        );
    }
}
