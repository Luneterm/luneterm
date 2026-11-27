//! `luneterm ctl <words…>`: checks the words, finds the running instance, sends them and prints
//! what comes back. On success exactly the JSON line and a newline on stdout and exit 0;
//! otherwise nothing on stdout, one line `luneterm: <reason>` on stderr and the [`Code`] as the
//! exit code.
//!
//! It opens no window, reads no settings, loads no fonts and never runs the updater, and the
//! whole run is over within [`BUDGET`]: the work is done on a second thread, so a call the OS
//! does not return from (a connect, a look at a directory on a dead mount) cannot hold the exit.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::endpoint::{Connection, Endpoint, Native};
use super::wire::{WireError, decode_reply, encode_request};
use super::{Code, Request, parse, token_ok, words};
use crate::persist::Store;

/// The longest a run takes.
const BUDGET: Duration = Duration::from_secs(2);

/// The variable that names the instance to reach.
const INSTANCE_VAR: &str = "LUNETERM_INSTANCE";

/// Why a run failed: the exit code and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure(Code, String);

fn failure(code: Code, reason: impl Into<String>) -> Failure {
    Failure(code, reason.into())
}

fn timed_out() -> Failure {
    failure(Code::Failed, "no answer within 2 s")
}

/// The client: `args` are the words after `ctl`.
pub fn main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let deadline = Instant::now() + BUDGET;
    let args: Vec<OsString> = args.into_iter().collect();
    let instance = std::env::var_os(INSTANCE_VAR);
    let config_dir = config_dir();
    report(within(deadline, move || {
        run::<Native>(args, instance, config_dir.as_deref(), deadline)
    }))
}

/// The config directory, found as the app finds it: the one `LUNETERM_CONFIG_DIR` names, or the
/// platform's.
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("LUNETERM_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(Store::default_dir)
}

/// What `work` returns, or the time-out failure if it has not by `deadline`. The thread is left
/// behind then: the process is about to exit.
fn within(
    deadline: Instant,
    work: impl FnOnce() -> Result<String, Failure> + Send + 'static,
) -> Result<String, Failure> {
    let (done, result) = mpsc::sync_channel(1);
    let started = thread::Builder::new()
        .name("ctl-client".to_owned())
        .spawn(move || {
            let _ = done.send(work());
        });
    if started.is_err() {
        return Err(failure(Code::Failed, "cannot start a thread"));
    }
    result
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_else(|_| Err(timed_out()))
}

/// Everything but the printing: the JSON line of a command that was done.
fn run<E: Endpoint>(
    args: Vec<OsString>,
    instance: Option<OsString>,
    config_dir: Option<&Path>,
    deadline: Instant,
) -> Result<String, Failure> {
    let usage = |reason: &str| failure(Code::Usage, reason);
    // All that can be wrong with the command is found before anything is connected to.
    let words = words(args).map_err(|e| usage(e.0))?;
    let request = parse(&words).map_err(|e| usage(e.0))?;
    if let Request::TabNew { cwd, .. } = &request
        && !cwd.is_dir()
    {
        return Err(usage("--cwd is not an existing directory"));
    }
    let frame = encode_request(&words).map_err(|e| usage(&e.to_string()))?;
    let config_dir = config_dir.ok_or_else(|| failure(Code::Unreachable, "no config directory"))?;
    let mut connection = reach::<E>(config_dir, instance, deadline)?;
    connection.set_deadline(deadline);
    let lost = |kind: io::ErrorKind| match kind {
        io::ErrorKind::TimedOut => timed_out(),
        _ => failure(
            Code::Failed,
            "Luneterm closed the connection without an answer",
        ),
    };
    connection.write_all(&frame).map_err(|e| lost(e.kind()))?;
    match decode_reply(&mut connection) {
        Ok((Code::Done, line)) => Ok(line),
        Ok((code, reason)) => Err(Failure(code, reason)),
        Err(WireError::Read(kind)) => Err(lost(kind)),
        Err(bad) => Err(failure(Code::Failed, bad.to_string())),
    }
}

/// A connection to the instance to talk to: the one `instance` names, or the only one running.
fn reach<E: Endpoint>(
    config_dir: &Path,
    instance: Option<OsString>,
    deadline: Instant,
) -> Result<E::Connection, Failure> {
    let unreachable = |reason: &str| failure(Code::Unreachable, reason);
    if let Some(named) = instance.filter(|v| !v.is_empty()) {
        let token = named
            .to_str()
            .filter(|t| token_ok(t))
            .ok_or_else(|| unreachable("LUNETERM_INSTANCE is not an instance token"))?;
        return E::connect(config_dir, token, deadline).map_err(|e| match e.kind() {
            io::ErrorKind::TimedOut => timed_out(),
            _ => unreachable("the Luneterm that LUNETERM_INSTANCE names cannot be reached"),
        });
    }
    let mut live = None;
    for token in E::instances(config_dir) {
        match E::connect(config_dir, &token, deadline) {
            Ok(_) if live.is_some() => {
                return Err(unreachable(
                    "more than one Luneterm is running; set LUNETERM_INSTANCE",
                ));
            }
            Ok(connection) => live = Some(connection),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err(timed_out()),
            // Left over from an instance that is gone.
            Err(_) => {}
        }
    }
    live.ok_or_else(|| unreachable("no running Luneterm"))
}

/// Prints the outcome and gives the exit code. A closed stdout or stderr is not a panic.
fn report(outcome: Result<String, Failure>) -> ExitCode {
    let Failure(code, reason) = match outcome {
        Ok(line) => {
            let mut stdout = io::stdout().lock();
            match writeln!(stdout, "{line}").and_then(|()| stdout.flush()) {
                Ok(()) => return ExitCode::SUCCESS,
                Err(_) => failure(Code::Failed, "cannot write the answer"),
            }
        }
        Err(failed) => failed,
    };
    let _ = writeln!(io::stderr().lock(), "luneterm: {}", one_line(&reason));
    ExitCode::from(code as u8)
}

/// `reason` without control characters: one line, and no escape sequence for the terminal that
/// shows it.
fn one_line(reason: &str) -> String {
    reason.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_is_printed_without_control_characters() {
        assert_eq!(
            one_line("a\nb\r\u{1b}]0;x\u{7}c\u{85}\u{9b}d\te\0é"),
            "ab]0;xcdeé"
        );
    }

    #[test]
    fn work_that_is_late_is_a_time_out() {
        let start = Instant::now();
        let deadline = start + Duration::from_millis(100);
        let outcome = within(deadline, || {
            thread::sleep(Duration::from_secs(30));
            Ok(String::new())
        });
        assert_eq!(outcome, Err(timed_out()));
        assert!(Instant::now() >= deadline);
        assert!(start.elapsed() < Duration::from_secs(5));
        let soon = Instant::now() + Duration::from_secs(5);
        assert_eq!(
            within(soon, || Ok("done".to_owned())),
            Ok("done".to_owned())
        );
    }
}
