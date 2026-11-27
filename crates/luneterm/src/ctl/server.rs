//! The app's end of `ctl`: a listener thread that takes one connection at a time, reads its
//! request under a deadline, parses the words with the parser the client ran, and asks a
//! [`Handler`] for the reply, which has a deadline too. What a handler does with a request is not
//! decided here, so the server runs without a window.
//!
//! The handler runs on a thread of its own, so a late one cannot hold the listener. While a
//! request is still unanswered no other is handed to it: nothing queues up behind a stuck
//! handler to run later.

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::endpoint::{BindError, Connection, Endpoint, Interrupt, Listener, Native};
use super::wire::{WireError, decode_request, encode_reply};
use super::{Code, Request, parse};

/// The longest a client may take to send its request.
const READ_DEADLINE: Duration = Duration::from_secs(1);
/// The longest the handler may take to answer.
const ANSWER_DEADLINE: Duration = Duration::from_millis(1500);
/// The longest a client may take to read its reply.
const WRITE_DEADLINE: Duration = Duration::from_secs(1);
/// How often a wait for the handler looks whether the server is stopping.
const STOP_CHECK: Duration = Duration::from_millis(20);
/// One wait for a connection; the server's stop interrupts it.
const IDLE: Duration = Duration::from_secs(3600);
/// The pause after an accept that failed.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

const NO_ANSWER: &str = "Luneterm did not answer";
const UNSENDABLE: &str = "the answer could not be sent";

/// What answers the requests.
pub trait Handler: Send + 'static {
    fn handle(&self, request: Request) -> Reply;
}

/// The answer to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// Never [`Code::Unreachable`], which only the client can find.
    pub code: Code,
    /// The JSON line without its newline when `code` is [`Code::Done`], else the reason.
    pub body: String,
}

impl Reply {
    pub fn new(code: Code, body: impl Into<String>) -> Reply {
        Reply {
            code,
            body: body.into(),
        }
    }
}

/// A listening instance. Dropping it stops the listener and takes its endpoint down; it does not
/// wait for the handler.
#[must_use = "dropping the server stops it"]
pub struct Server {
    stop: Arc<AtomicBool>,
    interrupt: Box<dyn Interrupt>,
    listener: Option<JoinHandle<()>>,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

impl Server {
    /// Listens as the instance `token` at this system's endpoint.
    pub fn start(
        config_dir: &Path,
        token: &str,
        handler: impl Handler,
    ) -> Result<Server, BindError> {
        Server::serve(Native::bind(config_dir, token)?, handler)
    }

    fn serve(listener: impl Listener, handler: impl Handler) -> Result<Server, BindError> {
        let failed = |e: io::Error| BindError::Io("start listening for control requests", e.kind());
        let interrupt = Box::new(listener.interrupter().map_err(failed)?);
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker::start(handler).map_err(failed)?;
        let thread = thread::Builder::new()
            .name("ctl-listener".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                move || listen(listener, &worker, &stop)
            })
            .map_err(failed)?;
        Ok(Server {
            stop,
            interrupt,
            listener: Some(thread),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.interrupt.interrupt();
        // The listener owns the endpoint: once it has ended, the endpoint is gone.
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

fn listen(mut listener: impl Listener, worker: &Worker, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept(Instant::now() + IDLE) {
            Ok(Some(mut connection)) => answer(&mut connection, worker, stop),
            Ok(None) => {}
            // No descriptor left, say: tried again, not spun on.
            Err(_) => thread::sleep(ACCEPT_RETRY),
        }
    }
}

/// One request and its reply.
fn answer(connection: &mut impl Connection, worker: &Worker, stop: &AtomicBool) {
    connection.set_deadline(Instant::now() + READ_DEADLINE);
    let reply = match decode_request(connection) {
        // Gone, silent or too slow: there is nobody to answer.
        Err(WireError::Read(_)) => return,
        Err(refused) => Reply::new(Code::Usage, refused.to_string()),
        Ok(words) => match parse(&words) {
            Err(usage) => Reply::new(Code::Usage, usage.to_string()),
            Ok(request) => worker
                .ask(request, stop)
                .unwrap_or_else(|| Reply::new(Code::Failed, NO_ANSWER)),
        },
    };
    let Ok(frame) =
        encode_reply(reply.code, &reply.body).or_else(|_| encode_reply(Code::Failed, UNSENDABLE))
    else {
        return;
    };
    connection.set_deadline(Instant::now() + WRITE_DEADLINE);
    // A client that is gone, or does not read, loses its reply.
    let _ = connection
        .write_all(&frame)
        .and_then(|()| connection.flush());
}

/// A request and where its reply goes.
type Job = (Request, SyncSender<Reply>);

/// The handler's thread. It ends when this is dropped and the handler has returned.
struct Worker {
    jobs: Sender<Job>,
    busy: Arc<AtomicBool>,
}

impl Worker {
    fn start(handler: impl Handler) -> io::Result<Worker> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let busy = Arc::new(AtomicBool::new(false));
        let working = Arc::clone(&busy);
        thread::Builder::new()
            .name("ctl-handler".to_owned())
            .spawn(move || {
                for (request, answer) in queue {
                    let reply = handler.handle(request);
                    // Free before the reply is out: the next request may follow at once.
                    working.store(false, Ordering::Release);
                    // Nobody waits for a late one.
                    let _ = answer.send(reply);
                }
            })?;
        Ok(Worker { jobs, busy })
    }

    /// The handler's reply; `None` if it does not come within [`ANSWER_DEADLINE`], if the handler
    /// still has an earlier request, or if the server is stopping.
    fn ask(&self, request: Request, stop: &AtomicBool) -> Option<Reply> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return None;
        }
        let deadline = Instant::now() + ANSWER_DEADLINE;
        let (answer, reply) = mpsc::sync_channel(1);
        self.jobs.send((request, answer)).ok()?;
        while !stop.load(Ordering::Acquire) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match reply.recv_timeout(left.min(STOP_CHECK)) {
                Ok(reply) => return Some(reply),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver;

    use super::*;
    use crate::ctl::endpoint::scratch::Scratch;
    use crate::ctl::token;
    use crate::ctl::wire::{decode_reply, encode_request};

    /// A config directory and a token of the test's own (pipe names on Windows are
    /// machine-wide).
    struct Place {
        scratch: Scratch,
        token: String,
    }

    impl Place {
        fn new() -> Place {
            Place {
                scratch: Scratch::new(),
                token: token::new().expect("a token"),
            }
        }

        fn path(&self) -> &Path {
            self.scratch.path()
        }

        fn listed(&self) -> bool {
            Native::instances(self.path()).contains(&self.token)
        }
    }

    /// Answers `status` at once with how often it was asked, after `slow_first` the first time.
    struct Counts {
        calls: Arc<AtomicUsize>,
        slow_first: Duration,
        code: Code,
    }

    impl Handler for Counts {
        fn handle(&self, _: Request) -> Reply {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                thread::sleep(self.slow_first);
            }
            Reply::new(self.code, format!("call {n}"))
        }
    }

    fn serving(scratch: &Place, slow_first: Duration, code: Code) -> (Server, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = Counts {
            calls: Arc::clone(&calls),
            slow_first,
            code,
        };
        let server = Server::start(scratch.path(), &scratch.token, handler).expect("starts");
        (server, calls)
    }

    fn connect(scratch: &Place) -> impl Connection {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut connection =
            Native::connect(scratch.path(), &scratch.token, deadline).expect("connects");
        connection.set_deadline(deadline);
        connection
    }

    fn send(scratch: &Place, bytes: &[u8]) -> (Code, String) {
        let mut connection = connect(scratch);
        connection.write_all(bytes).expect("sends");
        decode_reply(&mut connection).expect("a reply")
    }

    fn ask(scratch: &Place, words: &[&str]) -> (Code, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        send(scratch, &encode_request(&words).expect("encodes"))
    }

    #[test]
    fn a_request_is_handed_over_and_its_reply_sent_back() {
        let scratch = Place::new();
        let (_server, calls) = serving(&scratch, Duration::ZERO, Code::NoSuchTab);
        for n in 1..=50 {
            assert_eq!(
                ask(&scratch, &["status"]),
                (Code::NoSuchTab, format!("call {n}"))
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 50);
    }

    #[test]
    fn what_is_no_request_is_answered_2_and_never_reaches_the_handler() {
        let scratch = Place::new();
        let (_server, calls) = serving(&scratch, Duration::ZERO, Code::Done);
        for frame in [
            &b"GET / HTTP/1.1\r\n\r\n"[..],
            // 301 words; one word longer than a frame may be; a word that is not UTF-8.
            b"LTC1\x2d\x01\0\0",
            b"LTC1\x01\0\0\0\xff\xff\xff\xff",
            b"LTC1\x01\0\0\0\x01\0\0\0\xff",
        ] {
            assert_eq!(send(&scratch, frame).0, Code::Usage, "{frame:?}");
        }
        let mut long = vec!["tab", "new", "--cwd", "/tmp", "--"];
        long.extend(["a"; 257]);
        for words in [
            &["shutdown"][..],
            &["tab", "new", "--cwd", "relative"],
            &["tab", "close", "1", "--force"],
            &long,
        ] {
            assert_eq!(ask(&scratch, words).0, Code::Usage, "{}", words[0]);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // The positive control.
        assert_eq!(ask(&scratch, &["status"]).0, Code::Done);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_silent_client_is_dropped_after_a_second_and_the_next_is_served() {
        let scratch = Place::new();
        let (_server, calls) = serving(&scratch, Duration::ZERO, Code::Done);
        let start = Instant::now();
        let mut silent = connect(&scratch);
        let mut got = Vec::new();
        thread::scope(|scope| {
            let next = scope.spawn(|| ask(&scratch, &["status"]));
            silent.read_to_end(&mut got).expect("closed by the server");
            assert_eq!(next.join().expect("join").0, Code::Done);
        });
        let took = start.elapsed();
        assert!(got.is_empty(), "no reply to nothing");
        assert!(
            took >= READ_DEADLINE && took < READ_DEADLINE + Duration::from_secs(1),
            "{took:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_client_that_drips_its_request_gets_one_second_in_all() {
        let scratch = Place::new();
        let (_server, calls) = serving(&scratch, Duration::ZERO, Code::Done);
        let frame = encode_request(&["a".repeat(200)]).expect("encodes");
        let start = Instant::now();
        let mut connection = connect(&scratch);
        let mut sent = 0;
        for byte in &frame {
            if connection.write_all(&[*byte]).is_err() {
                break;
            }
            sent += 1;
            thread::sleep(Duration::from_millis(50));
        }
        let took = start.elapsed();
        assert!(sent < frame.len(), "the server read the whole drip");
        assert!(
            took >= READ_DEADLINE && took < READ_DEADLINE + Duration::from_secs(1),
            "{took:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_late_handler_is_answered_for_and_nothing_queues_behind_it() {
        let scratch = Place::new();
        let slow = ANSWER_DEADLINE + Duration::from_secs(1);
        let (_server, calls) = serving(&scratch, slow, Code::Done);
        let start = Instant::now();
        assert_eq!(
            ask(&scratch, &["status"]),
            (Code::Failed, NO_ANSWER.to_owned())
        );
        let took = start.elapsed();
        assert!(took >= ANSWER_DEADLINE && took < slow, "{took:?}");
        // Refused at once, and never handed over.
        let start = Instant::now();
        assert_eq!(
            ask(&scratch, &["status"]),
            (Code::Failed, NO_ANSWER.to_owned())
        );
        assert!(start.elapsed() < Duration::from_millis(500));
        // Once the handler is back, requests reach it again; the refused one never did.
        thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            ask(&scratch, &["status"]),
            (Code::Done, "call 2".to_owned())
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// Says it was asked, then waits to be let go.
    struct Stuck {
        asked: SyncSender<()>,
        release: Receiver<()>,
    }

    impl Handler for Stuck {
        fn handle(&self, _: Request) -> Reply {
            let _ = self.asked.send(());
            let _ = self.release.recv_timeout(Duration::from_secs(30));
            Reply::new(Code::Done, "")
        }
    }

    #[test]
    fn dropping_the_server_takes_the_endpoint_away_without_waiting_for_the_handler() {
        let scratch = Place::new();
        let (asked, was_asked) = mpsc::sync_channel(1);
        let (let_go, release) = mpsc::channel();
        let server = Server::start(scratch.path(), &scratch.token, Stuck { asked, release })
            .expect("starts");
        assert!(scratch.listed());
        let mut connection = connect(&scratch);
        let status = encode_request(&["status".to_owned()]).expect("encodes");
        connection.write_all(&status).expect("sends");
        was_asked
            .recv_timeout(Duration::from_secs(10))
            .expect("the handler has it");
        let start = Instant::now();
        drop(server);
        let took = start.elapsed();
        assert!(took < Duration::from_millis(500), "{took:?}");
        assert!(!scratch.listed());
        drop(let_go);
    }

    #[test]
    fn dropping_an_idle_server_is_immediate() {
        let scratch = Place::new();
        let (server, _) = serving(&scratch, Duration::ZERO, Code::Done);
        assert_eq!(ask(&scratch, &["status"]).0, Code::Done);
        let start = Instant::now();
        drop(server);
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(!scratch.listed());
    }

    #[test]
    fn a_reply_that_cannot_be_sent_becomes_a_failure() {
        let scratch = Place::new();
        let (_server, _) = serving(&scratch, Duration::ZERO, Code::Unreachable);
        assert_eq!(
            ask(&scratch, &["status"]),
            (Code::Failed, UNSENDABLE.to_owned())
        );
    }
}
