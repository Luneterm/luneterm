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

/// The config directory: the one `LUNETERM_CONFIG_DIR` names, or the platform's. The app and the
/// client both find it here (the only reader of the variable but the child environment's
/// rewrite, `spawn::child_env`).
pub fn config_dir() -> Option<PathBuf> {
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
    scan::<E>(config_dir, deadline)
}

/// A connection to the only instance running.
///
/// Where every endpoint is this user's, each is waited for until `deadline`, and one that has not
/// accepted by then ends the run. Where other users' endpoints can be among them
/// ([`Endpoint::SCAN_SLICE`]), those cannot hold the run: first each is tried without a wait, so a
/// free one is found however many are busy; then the busy ones take turns of a slice each, for the
/// rest of the budget while none has answered, and once one has, for one slice more, which finds a
/// second Luneterm of this user that was busy for a moment. One still busy after that may be a
/// second Luneterm of this user, so the scan fails closed: it never acts on one of two. So does a
/// list that was cut ([`Endpoint::SCAN_CUT_AT`]): one of this user's may be past the cut.
fn scan<E: Endpoint>(config_dir: &Path, deadline: Instant) -> Result<E::Connection, Failure> {
    let unreachable = |reason: &str| failure(Code::Unreachable, reason);
    let mut live = None;
    // When the turns end, once one has answered.
    let mut turns_end: Option<Instant> = None;
    let mut wait = Duration::ZERO;
    let mut pending = E::instances(config_dir);
    if E::SCAN_CUT_AT.is_some_and(|cap| pending.len() > cap) {
        return Err(unreachable(
            "too many candidate pipes; set LUNETERM_INSTANCE",
        ));
    }
    loop {
        let mut busy = Vec::new();
        for token in pending {
            let by = match E::SCAN_SLICE {
                None => deadline,
                Some(_) => (Instant::now() + wait).min(turns_end.unwrap_or(deadline).min(deadline)),
            };
            match E::connect(config_dir, &token, by) {
                Ok(_) if live.is_some() => {
                    return Err(unreachable(
                        "more than one Luneterm is running; set LUNETERM_INSTANCE",
                    ));
                }
                Ok(connection) => {
                    live = Some(connection);
                    if !wait.is_zero() {
                        turns_end = E::SCAN_SLICE.map(|slice| Instant::now() + slice);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => busy.push(token),
                // Left over from an instance that is gone, or not this user's.
                Err(_) => {}
            }
        }
        if busy.is_empty() {
            break;
        }
        let now = Instant::now();
        let Some(slice) = E::SCAN_SLICE.filter(|_| now < deadline) else {
            return Err(timed_out());
        };
        if live.is_some() && now >= *turns_end.get_or_insert(now + slice) {
            return Err(unreachable(
                "another Luneterm may be running and is busy; set LUNETERM_INSTANCE",
            ));
        }
        pending = busy;
        wait = slice;
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
    use std::cell::RefCell;
    use std::io::Read;

    use super::super::endpoint::{BindError, Interrupt, Listener};
    use super::*;

    thread_local! {
        /// The candidates a [`Fake`] scan lists, and when they were set up.
        static SCENE: RefCell<(Vec<String>, Instant)> = RefCell::new((Vec::new(), Instant::now()));
        /// How many connects a [`Fake`] scan made since its scene was set.
        static CONNECTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Where a list of the fake shared namespace is cut.
    const CUT_AT: usize = 4096;

    /// The slice of the fake shared namespace.
    const SLICE: Duration = Duration::from_millis(200);

    /// An endpoint of scripted candidates, told apart by their names: `free…` accepts;
    /// `busy…` never does; `later<ms>` does from that long after the scene was set; `alien…` is
    /// another user's; anything else is gone. As the real ones, it waits until its deadline and
    /// looks once whatever the deadline. `SHARED`: others' endpoints can be listed (Windows).
    struct Fake<const SHARED: bool>;

    /// The name of the candidate connected to.
    #[derive(Debug)]
    struct Connected(String);

    impl Read for Connected {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for Connected {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Connection for Connected {
        fn set_deadline(&mut self, _deadline: Instant) {}
    }

    /// The fake never listens.
    struct Deaf;

    impl Interrupt for Deaf {
        fn interrupt(&self) {}
    }

    impl Listener for Deaf {
        type Connection = Connected;
        type Interrupt = Deaf;

        fn interrupter(&self) -> io::Result<Deaf> {
            Ok(Deaf)
        }

        fn accept(&mut self, _deadline: Instant) -> io::Result<Option<Connected>> {
            Ok(None)
        }
    }

    impl<const SHARED: bool> Endpoint for Fake<SHARED> {
        type Listener = Deaf;
        type Connection = Connected;

        const SCAN_SLICE: Option<Duration> = if SHARED { Some(SLICE) } else { None };
        const SCAN_CUT_AT: Option<usize> = if SHARED { Some(CUT_AT) } else { None };

        fn bind(_config_dir: &Path, _token: &str) -> Result<Deaf, BindError> {
            Ok(Deaf)
        }

        fn connect(_config_dir: &Path, token: &str, deadline: Instant) -> io::Result<Connected> {
            CONNECTS.set(CONNECTS.get() + 1);
            let set_up = SCENE.with_borrow(|scene| scene.1);
            let free_from = if token.starts_with("free") {
                Some(set_up)
            } else if let Some(ms) = token.strip_prefix("later") {
                Some(set_up + Duration::from_millis(ms.parse().expect("milliseconds")))
            } else if token.starts_with("busy") {
                None
            } else if token.starts_with("alien") {
                return Err(io::ErrorKind::PermissionDenied.into());
            } else {
                return Err(io::ErrorKind::NotFound.into());
            };
            loop {
                if free_from.is_some_and(|from| Instant::now() >= from) {
                    return Ok(Connected(token.to_owned()));
                }
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                thread::sleep(Duration::from_millis(1));
            }
        }

        fn instances(_config_dir: &Path) -> Vec<String> {
            SCENE.with_borrow(|scene| scene.0.clone())
        }
    }

    /// A scan of `candidates` with `budget` to do it in: whom it reached, or why not, and how
    /// long it took.
    fn scanned<const SHARED: bool>(
        candidates: &[&str],
        budget: Duration,
    ) -> (Result<String, Failure>, Duration) {
        let start = Instant::now();
        SCENE.set((candidates.iter().map(|c| (*c).to_owned()).collect(), start));
        CONNECTS.set(0);
        let reached = reach::<Fake<SHARED>>(Path::new("unused"), None, start + budget);
        (reached.map(|c| c.0), start.elapsed())
    }

    fn two() -> Failure {
        failure(
            Code::Unreachable,
            "more than one Luneterm is running; set LUNETERM_INSTANCE",
        )
    }

    fn none() -> Failure {
        failure(Code::Unreachable, "no running Luneterm")
    }

    fn maybe_two() -> Failure {
        failure(
            Code::Unreachable,
            "another Luneterm may be running and is busy; set LUNETERM_INSTANCE",
        )
    }

    /// Where others' endpoints are listed, one still busy after the extra slice may be a second
    /// Luneterm of this user: the scan fails closed with the remedy, without waiting out its
    /// budget, however many are busy.
    #[test]
    fn a_shared_scan_with_a_busy_candidate_besides_a_live_one_fails_closed() {
        let long = Duration::from_secs(10);
        let mut many: Vec<String> = (0..2000).map(|n| format!("busy{n}")).collect();
        many.push("free".to_owned());
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        for candidates in [
            &["free", "busy"][..],
            &["busy", "free"],
            &["busy1", "alien", "busy2", "gone", "free"],
            &many,
        ] {
            let (reached, took) = scanned::<true>(candidates, long);
            assert_eq!(reached, Err(maybe_two()), "{candidates:?}");
            assert!(took >= SLICE && took < long / 2, "{took:?}");
        }
        // One busy for longer than a slice answers in its turns; the other still busy one slice
        // after that is not passed over.
        let (reached, took) = scanned::<true>(&["busy", "later500"], long);
        assert_eq!(reached, Err(maybe_two()));
        assert!(
            took >= Duration::from_millis(500) + SLICE && took < long / 2,
            "{took:?}"
        );
    }

    /// With no candidate busy, a shared scan reaches the only live one; one busy for longer than a
    /// slice and then free is waited for in its turns.
    #[test]
    fn a_shared_scan_reaches_the_only_instance() {
        let long = Duration::from_secs(10);
        for candidates in [&["alien", "gone", "free"][..], &["free"]] {
            let (reached, took) = scanned::<true>(candidates, long);
            assert_eq!(reached, Ok("free".to_owned()), "{candidates:?}");
            assert!(took < SLICE, "{took:?}");
        }
        let (reached, took) = scanned::<true>(&["alien", "later500", "gone"], long);
        assert_eq!(reached, Ok("later500".to_owned()));
        assert!(
            took >= Duration::from_millis(500) && took < long / 2,
            "{took:?}"
        );
    }

    /// A second instance is told of when it is free, or becomes free within the extra slice.
    #[test]
    fn a_shared_scan_still_finds_a_second_instance() {
        let long = Duration::from_secs(10);
        for candidates in [
            &["free1", "busy", "free2"][..],
            &["free", "later20"],
            &["later20", "free"],
        ] {
            assert_eq!(
                scanned::<true>(candidates, long).0,
                Err(two()),
                "{candidates:?}"
            );
        }
    }

    #[test]
    fn a_shared_scan_with_nobody_to_reach_says_which_way() {
        let budget = Duration::from_millis(500);
        let (reached, took) = scanned::<true>(&["busy1", "alien", "gone", "busy2"], budget);
        assert_eq!(reached, Err(timed_out()));
        assert!(took >= budget, "{took:?}");
        assert_eq!(scanned::<true>(&["alien", "gone"], budget).0, Err(none()));
        assert_eq!(scanned::<true>(&[], budget).0, Err(none()));
    }

    /// A shared list longer than the cut may hide a second instance of ours: the scan connects to
    /// none and names the remedy. A list of exactly the cut is whole.
    #[test]
    fn a_shared_scan_of_a_cut_list_fails_closed() {
        let long = Duration::from_secs(10);
        let list = |len: usize| -> Vec<String> {
            let mut list = vec!["free".to_owned()];
            list.extend((1..len).map(|n| format!("gone{n}")));
            list
        };
        let cut = list(CUT_AT + 1);
        let cut: Vec<&str> = cut.iter().map(String::as_str).collect();
        assert_eq!(
            scanned::<true>(&cut, long).0,
            Err(failure(
                Code::Unreachable,
                "too many candidate pipes; set LUNETERM_INSTANCE"
            ))
        );
        assert_eq!(CONNECTS.get(), 0);
        let whole = list(CUT_AT);
        let whole: Vec<&str> = whole.iter().map(String::as_str).collect();
        assert_eq!(scanned::<true>(&whole, long).0, Ok("free".to_owned()));
        assert_eq!(CONNECTS.get(), CUT_AT);
        // Where no list is cut, a long one is scanned as any other.
        assert_eq!(scanned::<false>(&cut, long).0, Ok("free".to_owned()));
    }

    /// Where every endpoint is the user's, one that does not accept ends the run at the deadline,
    /// also when another answered.
    #[test]
    fn an_own_scan_waits_for_every_candidate() {
        let budget = Duration::from_millis(300);
        for candidates in [&["free", "busy"][..], &["busy", "free"], &["busy"]] {
            let (reached, took) = scanned::<false>(candidates, budget);
            assert_eq!(reached, Err(timed_out()), "{candidates:?}");
            assert!(took >= budget, "{took:?}");
        }
        let long = Duration::from_secs(10);
        assert_eq!(
            scanned::<false>(&["gone", "free"], long).0,
            Ok("free".to_owned())
        );
        assert_eq!(scanned::<false>(&["free1", "free2"], long).0, Err(two()));
        assert_eq!(
            scanned::<false>(&["later50"], long).0,
            Ok("later50".to_owned())
        );
        assert_eq!(scanned::<false>(&["gone"], long).0, Err(none()));
    }

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
