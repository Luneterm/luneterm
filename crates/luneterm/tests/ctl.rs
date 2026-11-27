//! `luneterm ctl` end to end: the real binary as the client, against the app's server with a
//! canned handler in this process. No window, and every run gets a config directory of the test's
//! own (`LUNETERM_CONFIG_DIR`).
//!
//! On Windows the pipe names are machine-wide, so a client that looks for "the" instance sees
//! every other test's: there the tests run one at a time ([`one_at_a_time`]).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, Once};
use std::thread;
use std::time::{Duration, Instant};

use luneterm::ctl::server::{Handler, Reply, Server};
use luneterm::ctl::{Code, Request, token};

const BIN: &str = env!("CARGO_BIN_EXE_luneterm");
const INSTANCE: &str = "LUNETERM_INSTANCE";

// ------------------------------------------------------------------------------- the fixtures

/// A new directory with a short name (a socket path has little room); removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("ltc{}-{n}", std::process::id()));
            match private_dir(&dir) {
                Ok(()) => return Scratch(dir),
                // Not ours: left alone.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("cannot create a scratch directory: {e}"),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // `new` created it.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// On Windows, the lock every test holds for its whole run; elsewhere nothing.
fn one_at_a_time() -> Option<MutexGuard<'static, ()>> {
    static ONE: Mutex<()> = Mutex::new(());
    // A test that failed holding it says nothing about the next.
    cfg!(windows).then(|| {
        ONE.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    })
}

/// Answers every request with the reply it holds, after `delay`, and keeps what it was asked.
#[derive(Clone)]
struct Canned {
    reply: Arc<Mutex<Reply>>,
    seen: Arc<Mutex<Vec<Request>>>,
    delay: Duration,
}

impl Canned {
    fn new(body: &str) -> Canned {
        Canned {
            reply: Arc::new(Mutex::new(Reply::new(Code::Done, body))),
            seen: Arc::default(),
            delay: Duration::ZERO,
        }
    }

    fn answer(&self, code: Code, body: &str) {
        *self.reply.lock().expect("reply") = Reply::new(code, body);
    }

    fn seen(&self) -> Vec<Request> {
        self.seen.lock().expect("seen").clone()
    }
}

impl Handler for Canned {
    fn handle(&self, request: Request) -> Reply {
        self.seen.lock().expect("seen").push(request);
        thread::sleep(self.delay);
        self.reply.lock().expect("reply").clone()
    }
}

/// A running instance: a server under a new token.
struct Instance {
    token: String,
    handler: Canned,
    _server: Server,
}

impl Instance {
    fn start(config: &Path, handler: Canned) -> Instance {
        let token = token::new().expect("a token");
        let server = Server::start(config, &token, handler.clone()).expect("the server starts");
        Instance {
            token,
            handler,
            _server: server,
        }
    }
}

/// Starts the binary once. The first start of a new binary may take a second (macOS looks at it),
/// which is no part of what a test times.
fn warm() {
    static WARM: Once = Once::new();
    WARM.call_once(|| {
        let status = Command::new(BIN)
            .arg("ctl")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("runs");
        assert_eq!(status.code(), Some(2));
    });
}

/// `luneterm ctl <words>` with `config` as the config directory and `LUNETERM_INSTANCE` set to
/// `instance`, or not set at all.
fn ctl_os(config: &Path, instance: Option<&OsStr>, words: &[&OsStr]) -> Output {
    warm();
    let mut command = Command::new(BIN);
    command
        .arg("ctl")
        .args(words)
        .env("LUNETERM_CONFIG_DIR", config)
        .env_remove(INSTANCE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(instance) = instance {
        command.env(INSTANCE, instance);
    }
    let mut child = command.spawn().expect("runs");
    let start = Instant::now();
    while child.try_wait().expect("wait").is_none() {
        if start.elapsed() > Duration::from_secs(20) {
            // Ours: started just above.
            let _ = child.kill();
            panic!("ctl did not exit");
        }
        thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().expect("output")
}

fn ctl(config: &Path, instance: Option<&str>, words: &[&str]) -> Output {
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    ctl_os(config, instance.map(OsStr::new), &words)
}

/// Exit 0, exactly `line` and a newline on stdout, nothing on stderr.
#[track_caller]
fn done(output: &Output, line: &str) {
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).as_ref(),
            String::from_utf8_lossy(&output.stderr).as_ref(),
        ),
        (Some(0), format!("{line}\n").as_str(), "")
    );
}

/// Exit `code`, nothing on stdout, exactly `luneterm: <reason>` and a newline on stderr.
#[track_caller]
fn failed(output: &Output, code: i32, reason: &str) {
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).as_ref(),
            String::from_utf8_lossy(&output.stderr).as_ref(),
        ),
        (Some(code), "", format!("luneterm: {reason}\n").as_str())
    );
}

const NONE_RUNNING: &str = "no running Luneterm";
const SEVERAL: &str = "more than one Luneterm is running; set LUNETERM_INSTANCE";
const ABSENT: &str = "the Luneterm that LUNETERM_INSTANCE names cannot be reached";
const NOT_A_TOKEN: &str = "LUNETERM_INSTANCE is not an instance token";
const TOO_LATE: &str = "no answer within 2 s";

// ---------------------------------------------------------------------------------- the tests

#[test]
fn each_status_a_reply_carries_is_the_exit_code_and_the_output() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let line = r#"{"protocol":1,"version":"0","instance":"x","control":"on"}"#;
    let instance = Instance::start(scratch.path(), Canned::new(line));
    for named in [None, Some(instance.token.as_str())] {
        instance.handler.answer(Code::Done, line);
        done(&ctl(scratch.path(), named, &["status"]), line);
        for (code, exit, reason) in [
            (Code::Failed, 1, "too many tabs"),
            (Code::Usage, 2, "--cwd is not an existing directory"),
            (Code::ControlOff, 3, "tab control is turned off in settings"),
            (Code::NoSuchTab, 5, "no such tab"),
            (Code::Refused, 6, "the tab was not created through ctl"),
        ] {
            instance.handler.answer(code, reason);
            failed(&ctl(scratch.path(), named, &["tab", "list"]), exit, reason);
        }
    }
    assert_eq!(instance.handler.seen().len(), 12);
}

#[test]
fn a_reason_is_printed_as_one_line_without_control_characters() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let instance = Instance::start(scratch.path(), Canned::new(""));
    instance
        .handler
        .answer(Code::Failed, "one\ntwo\r\u{1b}[31m red\u{7}\u{9b}");
    failed(&ctl(scratch.path(), None, &["status"]), 1, "onetwo[31m red");
}

#[test]
fn the_words_reach_the_handler_as_they_were_given() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let instance = Instance::start(scratch.path(), Canned::new(r#"{"id":7}"#));
    let cwd = scratch.path().to_str().expect("UTF-8");
    // After the first `--` everything is the program's, a second `--` and option look-alikes
    // included.
    let program = [
        "/usr/bin/python3",
        "-I",
        "/x/orch-pa-exec",
        "--agent",
        "a1",
        "--env",
        "K=V W",
        "--",
        "/x/claude",
        "--focus",
        "--cwd",
        "",
        "two words",
        "$(id); `id` | cat > x",
        "line\nbreak",
        "\u{65e5}\u{672c}",
        "--",
    ];
    let mut words = vec![
        "tab", "new", "--cwd", cwd, "--group", "orch: x", "--title", "repo", "--",
    ];
    words.extend_from_slice(&program);
    done(&ctl(scratch.path(), None, &words), r#"{"id":7}"#);
    assert_eq!(
        instance.handler.seen(),
        [Request::TabNew {
            cwd: PathBuf::from(cwd),
            group: Some("orch: x".to_owned()),
            title: Some("repo".to_owned()),
            focus: false,
            argv: program.iter().map(|w| (*w).to_owned()).collect(),
        }]
    );
}

#[test]
fn with_no_server_it_is_exit_4_and_nothing_is_made() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let config = scratch.path().join("c");
    failed(&ctl(&config, None, &["status"]), 4, NONE_RUNNING);
    failed(&ctl(&config, Some("abcdefgh"), &["status"]), 4, ABSENT);
    assert!(!config.exists(), "the client makes no directory");
    // A server that was there and is gone.
    drop(Instance::start(&config, Canned::new("{}")));
    failed(&ctl(&config, None, &["status"]), 4, NONE_RUNNING);
}

#[test]
fn two_servers_and_no_instance_is_exit_4() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let a = Instance::start(scratch.path(), Canned::new("\"a\""));
    let b = Instance::start(scratch.path(), Canned::new("\"b\""));
    failed(&ctl(scratch.path(), None, &["status"]), 4, SEVERAL);
    // An empty variable names nobody.
    failed(&ctl(scratch.path(), Some(""), &["status"]), 4, SEVERAL);
    assert!(a.handler.seen().is_empty() && b.handler.seen().is_empty());
    done(&ctl(scratch.path(), Some(&a.token), &["status"]), "\"a\"");
    done(&ctl(scratch.path(), Some(&b.token), &["status"]), "\"b\"");
}

#[test]
fn an_instance_that_is_set_and_absent_is_exit_4() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let running = Instance::start(scratch.path(), Canned::new("{}"));
    let absent = token::new().expect("a token");
    // The one that does run is not taken in its place.
    failed(&ctl(scratch.path(), Some(&absent), &["status"]), 4, ABSENT);
    assert!(running.handler.seen().is_empty());
    done(&ctl(scratch.path(), Some(""), &["status"]), "{}");
}

#[test]
fn an_instance_that_is_no_token_is_exit_4() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let running = Instance::start(scratch.path(), Canned::new("{}"));
    let long = "a".repeat(65);
    let path = format!("../run/{}", running.token);
    for bad in ["short", " ", "has space 1", "abcdefgh.sock", &path, &long] {
        failed(&ctl(scratch.path(), Some(bad), &["status"]), 4, NOT_A_TOKEN);
    }
    #[cfg(unix)]
    {
        let bad: &OsStr = std::os::unix::ffi::OsStrExt::from_bytes(b"abcdefgh\xff");
        failed(
            &ctl_os(scratch.path(), Some(bad), &[OsStr::new("status")]),
            4,
            NOT_A_TOKEN,
        );
    }
    assert!(running.handler.seen().is_empty());
}

#[test]
fn a_handler_that_takes_3_s_is_exit_1_within_2_5_s() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let mut slow = Canned::new("{}");
    slow.delay = Duration::from_secs(3);
    let instance = Instance::start(scratch.path(), slow);
    warm();
    let start = Instant::now();
    let output = ctl(scratch.path(), Some(&instance.token), &["status"]);
    let took = start.elapsed();
    failed(&output, 1, "Luneterm did not answer");
    assert!(took < Duration::from_millis(2500), "{took:?}");
    assert_eq!(instance.handler.seen(), [Request::Status]);
}

#[test]
fn usage_errors_are_exit_2_before_anything_is_connected_to() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let raw = raw::Raw::new(scratch.path());
    let cwd = scratch.path().to_str().expect("UTF-8");
    let file = scratch.path().join("file");
    fs::write(&file, b"").expect("write");
    let file = file.to_str().expect("UTF-8");
    let missing = scratch.path().join("missing");
    let missing = missing.to_str().expect("UTF-8");
    let title = "t".repeat(129);
    let mut long = vec!["tab", "new", "--cwd", cwd, "--"];
    long.extend(["a"; 257]);
    let cases: &[&[&str]] = &[
        &[],
        &["bogus"],
        &["status", "extra"],
        &["tab", "new"],
        &["tab", "new", "--cwd", "relative"],
        &["tab", "new", "--cwd", "."],
        &["tab", "new", "--cwd", missing],
        &["tab", "new", "--cwd", file],
        &["tab", "new", "--cwd", cwd, "--title", &title],
        &["tab", "new", "--cwd", cwd, "--focus", "--focus"],
        &["tab", "close", "1", "--force"],
        &["tab", "focus", "one"],
        &long,
    ];
    for named in [None, Some(raw.token.as_str()), Some("no token")] {
        for words in cases {
            let output = ctl(scratch.path(), named, words);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(2), "{words:?}: {stderr}");
            assert!(output.stdout.is_empty(), "{words:?}");
            assert!(
                stderr.starts_with("luneterm: ") && stderr.matches('\n').count() == 1,
                "{words:?}: {stderr:?}"
            );
        }
    }
    #[cfg(unix)]
    {
        let bad: &OsStr = std::os::unix::ffi::OsStrExt::from_bytes(b"status\xff");
        let output = ctl_os(scratch.path(), None, &[bad]);
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(!raw.was_connected_to());
    // The positive control: a good command does connect there.
    let output = ctl(scratch.path(), None, &["tab", "new", "--cwd", cwd]);
    assert_eq!(output.status.code(), Some(1), "nobody answers it");
    assert!(raw.was_connected_to());
}

#[test]
fn a_server_that_never_answers_is_exit_1_at_2_s() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    // It listens and never accepts: the request goes into the socket and nothing comes back.
    let raw = raw::Raw::new(scratch.path());
    warm();
    for named in [None, Some(raw.token.as_str())] {
        let start = Instant::now();
        let output = ctl(scratch.path(), named, &["status"]);
        let took = start.elapsed();
        failed(&output, 1, TOO_LATE);
        assert!(
            took >= Duration::from_secs(2) && took < Duration::from_millis(2900),
            "{took:?}"
        );
    }
}

#[test]
fn a_server_that_hangs_up_is_exit_1() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    let raw = raw::Raw::new(scratch.path());
    let server = thread::spawn(move || raw.hang_up_on_one());
    failed(
        &ctl(scratch.path(), None, &["status"]),
        1,
        "Luneterm closed the connection without an answer",
    );
    server.join().expect("join");
}

#[test]
fn two_servers_started_at_once_both_stay_reachable() {
    let _one = one_at_a_time();
    let scratch = Scratch::new();
    for round in 0..20 {
        let barrier = Barrier::new(2);
        let start = |name: String| {
            barrier.wait();
            Instance::start(scratch.path(), Canned::new(&name))
        };
        let (a, b) = thread::scope(|scope| {
            let a = scope.spawn(|| start(format!("\"a{round}\"")));
            let b = scope.spawn(|| start(format!("\"b{round}\"")));
            (a.join().expect("a starts"), b.join().expect("b starts"))
        });
        done(
            &ctl(scratch.path(), Some(&a.token), &["status"]),
            &format!("\"a{round}\""),
        );
        done(
            &ctl(scratch.path(), Some(&b.token), &["status"]),
            &format!("\"b{round}\""),
        );
    }
}

#[cfg(unix)]
use unix as raw;

#[cfg(windows)]
use windows as raw;

#[cfg(windows)]
mod windows {
    //! An endpoint held by the test itself, which never answers.

    use std::io::Read;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::thread::JoinHandle;

    use luneterm::ctl::endpoint::{Connection, Endpoint, Interrupt, Listener, Native};

    use super::*;

    /// A pipe where an instance's would be. A thread takes its connections one by one, counts
    /// them, and reads each until the client closes (or, once asked to, hangs up on it after the
    /// first byte of the request), never answering.
    pub(super) struct Raw {
        pub(super) token: String,
        connections: Arc<AtomicUsize>,
        hang_up: Arc<AtomicBool>,
        hung_up: Arc<AtomicUsize>,
        stop: Option<Box<dyn Interrupt>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Raw {
        pub(super) fn new(config: &Path) -> Raw {
            let token = token::new().expect("a token");
            let mut listener = Native::bind(config, &token).expect("binds");
            let stop: Box<dyn Interrupt> = Box::new(listener.interrupter().expect("interrupter"));
            let connections = Arc::new(AtomicUsize::new(0));
            let hang_up = Arc::new(AtomicBool::new(false));
            let hung_up = Arc::new(AtomicUsize::new(0));
            let thread = thread::spawn({
                let connections = Arc::clone(&connections);
                let hang_up = Arc::clone(&hang_up);
                let hung_up = Arc::clone(&hung_up);
                move || {
                    while let Ok(Some(mut connection)) =
                        listener.accept(Instant::now() + Duration::from_secs(600))
                    {
                        connections.fetch_add(1, Ordering::SeqCst);
                        connection.set_deadline(Instant::now() + Duration::from_secs(30));
                        if hang_up.load(Ordering::SeqCst) {
                            // The first bytes of the request: the client has checked who the
                            // server is by then, so what it sees is the hang-up, not a server
                            // it cannot name.
                            let _ = connection.read(&mut [0; 1]);
                            drop(connection);
                            hung_up.fetch_add(1, Ordering::SeqCst);
                        } else {
                            let _ = std::io::copy(&mut connection, &mut std::io::sink());
                        }
                    }
                }
            });
            Raw {
                token,
                connections,
                hang_up,
                hung_up,
                stop: Some(stop),
                thread: Some(thread),
            }
        }

        /// Whether a connection came since the last look.
        pub(super) fn was_connected_to(&self) -> bool {
            self.connections.swap(0, Ordering::SeqCst) > 0
        }

        /// Hangs up on the next connection, and waits until it has: dropping `Raw` (which
        /// interrupts the pipe) any earlier could end the connection before the client has
        /// checked who the server is, and the client would then not count it at all.
        pub(super) fn hang_up_on_one(&self) {
            self.hang_up.store(true, Ordering::SeqCst);
            let start = Instant::now();
            while self.hung_up.load(Ordering::SeqCst) == 0 {
                assert!(start.elapsed() < Duration::from_secs(20), "no connection");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl Drop for Raw {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                stop.interrupt();
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(unix)]
mod unix {
    //! What needs a Unix socket made by hand.

    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    use super::*;

    /// A socket where an instance's would be, held by the test itself.
    pub(super) struct Raw {
        pub(super) token: String,
        listener: UnixListener,
    }

    impl Raw {
        pub(super) fn new(config: &Path) -> Raw {
            let run = config.join("run");
            if let Err(e) = private_dir(&run) {
                assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists, "{e}");
            }
            let token = token::new().expect("a token");
            let listener = UnixListener::bind(run.join(format!("{token}.sock"))).expect("binds");
            Raw { token, listener }
        }

        /// Whether a connection came since the last look.
        pub(super) fn was_connected_to(&self) -> bool {
            self.listener.set_nonblocking(true).expect("nonblocking");
            let came = self.listener.accept().is_ok();
            while self.listener.accept().is_ok() {}
            came
        }

        /// Takes one connection and closes it unanswered.
        pub(super) fn hang_up_on_one(&self) {
            drop(self.listener.accept().expect("a connection"));
        }
    }

    fn sockets(run: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(run)
            .expect("read run")
            .map(|e| e.expect("entry").file_name().into_string().expect("a name"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_stale_socket_is_ignored_and_the_next_server_removes_it() {
        let scratch = Scratch::new();
        let raw = Raw::new(scratch.path());
        let stale = raw.token.clone();
        // The listener goes and the file stays, as after a crash.
        drop(raw);
        let run = scratch.path().join("run");
        assert_eq!(sockets(&run), [format!("{stale}.sock")]);
        failed(&ctl(scratch.path(), None, &["status"]), 4, NONE_RUNNING);
        failed(&ctl(scratch.path(), Some(&stale), &["status"]), 4, ABSENT);

        let live = Instance::start(scratch.path(), Canned::new("{}"));
        assert_eq!(sockets(&run), [format!("{}.sock", live.token)], "swept");
        // One left behind while a server runs is passed over.
        drop(Raw::new(scratch.path()));
        assert_eq!(sockets(&run).len(), 2);
        done(&ctl(scratch.path(), None, &["status"]), "{}");
    }

    #[test]
    fn a_run_directory_open_to_others_refuses_the_server_and_the_client() {
        let scratch = Scratch::new();
        let run = scratch.path().join("run");
        let live = Raw::new(scratch.path());
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).expect("chmod");
        let refused = Server::start(
            scratch.path(),
            &token::new().expect("a token"),
            Canned::new("{}"),
        )
        .expect_err("refuses to bind");
        assert_eq!(
            refused.to_string(),
            "the control directory is open to other users"
        );
        assert_eq!(sockets(&run), [format!("{}.sock", live.token)]);
        failed(&ctl(scratch.path(), None, &["status"]), 4, NONE_RUNNING);
        failed(
            &ctl(scratch.path(), Some(&live.token), &["status"]),
            4,
            ABSENT,
        );
        assert!(!live.was_connected_to());
    }

    #[test]
    fn a_config_directory_open_to_writing_by_others_refuses_the_server() {
        let scratch = Scratch::new();
        let config = scratch.path().join("c");
        private_dir(&config).expect("create");
        fs::set_permissions(&config, fs::Permissions::from_mode(0o770)).expect("chmod");
        assert!(
            Server::start(&config, &token::new().expect("a token"), Canned::new("{}")).is_err()
        );
        assert!(!config.join("run").exists());
    }
}
