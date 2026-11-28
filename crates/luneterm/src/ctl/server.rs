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
    use std::sync::mpsc::{Receiver, TryRecvError};

    use super::*;
    use crate::ctl::endpoint::scratch::Scratch;
    use crate::ctl::token;
    use crate::ctl::wire::{decode_reply, encode_request};

    // The tests run beside others on loaded machines (the Windows gate, with the GPU dump tests:
    // one round trip took 5 s), so none of their upper time limits is near what the code takes.

    /// How long a test waits for what must come: running out means it never does.
    const PATIENCE: Duration = Duration::from_secs(60);
    /// The longest a test's repeated quick operations may take together.
    const TOGETHER: Duration = Duration::from_secs(30);

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

    /// Answers at once with how often it was asked.
    struct Counts {
        calls: Arc<AtomicUsize>,
        code: Code,
    }

    impl Handler for Counts {
        fn handle(&self, _: Request) -> Reply {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Reply::new(self.code, format!("call {n}"))
        }
    }

    fn serving(scratch: &Place, code: Code) -> (Server, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = Counts {
            calls: Arc::clone(&calls),
            code,
        };
        let server = Server::start(scratch.path(), &scratch.token, handler).expect("starts");
        (server, calls)
    }

    fn connect(scratch: &Place) -> impl Connection {
        let deadline = Instant::now() + PATIENCE;
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
        let (_server, calls) = serving(&scratch, Code::NoSuchTab);
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
        let (_server, calls) = serving(&scratch, Code::Done);
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

    /// A connection that hands out its request a byte per read and notes each deadline set on
    /// it: the deadline, when it was set, and how much had been read by then.
    struct Noted {
        request: Vec<u8>,
        read: usize,
        deadlines: Vec<(Instant, Instant, usize)>,
    }

    impl Read for Noted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match (self.request.get(self.read), buf.first_mut()) {
                (Some(&byte), Some(into)) => {
                    *into = byte;
                    self.read += 1;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    impl Write for Noted {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Connection for Noted {
        fn set_deadline(&mut self, deadline: Instant) {
            self.deadlines.push((deadline, Instant::now(), self.read));
        }
    }

    // How far away the read deadline is, which the two tests after this one cannot show without
    // timing the machine. Here no load can make a right deadline look wrong.
    #[test]
    fn the_whole_request_is_read_under_one_deadline_a_read_deadline_away() {
        let worker = Worker::start(Counts {
            calls: Arc::new(AtomicUsize::new(0)),
            code: Code::Done,
        })
        .expect("starts");
        let request = encode_request(&["shutdown".to_owned()]).expect("encodes");
        let length = request.len();
        let mut connection = Noted {
            request,
            read: 0,
            deadlines: Vec::new(),
        };
        let before = Instant::now();
        answer(&mut connection, &worker, &AtomicBool::new(false));
        assert_eq!(connection.read, length);
        let (deadline, set, read) = connection.deadlines[0];
        assert_eq!(read, 0, "set before anything is read");
        assert!(deadline >= before + READ_DEADLINE, "too short");
        assert!(deadline <= set + READ_DEADLINE, "too long");
        // Any later one is for the reply: none gives a read more time.
        assert!(
            connection.deadlines[1..]
                .iter()
                .all(|&(_, _, read)| read == length)
        );
    }

    // No upper time limit: a server that never drops the client runs into the client's own
    // deadline, PATIENCE away, and `read_to_end` fails.
    #[test]
    fn a_silent_client_is_dropped_after_a_second_and_the_next_is_served() {
        let scratch = Place::new();
        let (_server, calls) = serving(&scratch, Code::Done);
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
        assert!(took >= READ_DEADLINE, "{took:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // No upper time limit: a server that gave the request more time with each byte would read
    // all of this drip, which is over 40 s long.
    #[test]
    fn a_client_that_drips_its_request_gets_one_second_in_all() {
        let scratch = Place::new();
        let (_server, calls) = serving(&scratch, Code::Done);
        let frame = encode_request(&["a".repeat(800)]).expect("encodes");
        let start = Instant::now();
        let mut connection = connect(&scratch);
        let mut sent = 0;
        let mut failed = None;
        for byte in &frame {
            if let Err(e) = connection.write_all(&[*byte]) {
                failed = Some(e.kind());
                break;
            }
            sent += 1;
            thread::sleep(Duration::from_millis(50));
        }
        let took = start.elapsed();
        assert!(sent < frame.len(), "the server read the whole drip");
        assert_ne!(
            failed,
            Some(io::ErrorKind::TimedOut),
            "the client's own deadline ended it, not the server"
        );
        assert!(took >= READ_DEADLINE, "{took:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Says what it is asked, then waits to be let go: until the sender of `release` is
    /// dropped, which lets every later request through at once.
    struct Stuck {
        asked: Sender<Request>,
        release: Receiver<()>,
    }

    impl Handler for Stuck {
        fn handle(&self, request: Request) -> Reply {
            let _ = self.asked.send(request);
            let _ = self.release.recv_timeout(PATIENCE);
            Reply::new(Code::Done, "")
        }
    }

    /// A [`Stuck`], what it was asked, and what lets it go.
    fn stuck() -> (Stuck, Receiver<Request>, Sender<()>) {
        let (asked, was_asked) = mpsc::channel();
        let (let_go, release) = mpsc::channel();
        (Stuck { asked, release }, was_asked, let_go)
    }

    // That the first answer is the deadline's is shown by the handler holding the request until
    // after it, not by an upper time limit.
    #[test]
    fn a_late_handler_is_answered_for_and_nothing_queues_behind_it() {
        let scratch = Place::new();
        let (handler, was_asked, let_go) = stuck();
        let _server = Server::start(scratch.path(), &scratch.token, handler).expect("starts");
        let start = Instant::now();
        assert_eq!(
            ask(&scratch, &["status"]),
            (Code::Failed, NO_ANSWER.to_owned())
        );
        let took = start.elapsed();
        assert!(took >= ANSWER_DEADLINE, "{took:?}");
        // Its thread may get to run only after the answer has come.
        assert_eq!(was_asked.recv_timeout(PATIENCE), Ok(Request::Status));
        // Refused and not handed over: the handler still holds the first.
        assert_eq!(
            ask(&scratch, &["tab", "list"]),
            (Code::Failed, NO_ANSWER.to_owned())
        );
        assert_eq!(was_asked.try_recv(), Err(TryRecvError::Empty));
        // Once the handler is back, requests reach it again. It is back a moment after it is
        // let go, so asked until it answers.
        drop(let_go);
        let back = Instant::now() + PATIENCE;
        let answered = loop {
            match ask(&scratch, &["status"]) {
                (Code::Failed, reason) if reason == NO_ANSWER && Instant::now() < back => {
                    thread::sleep(Duration::from_millis(50));
                }
                other => break other,
            }
        };
        assert_eq!(answered, (Code::Done, String::new()));
        // A refused request that had queued would have reached the handler before this one.
        let later: Vec<Request> = was_asked.try_iter().collect();
        assert!(
            !later.is_empty() && later.iter().all(|r| *r == Request::Status),
            "{later:?}"
        );
    }

    // Only the clock tells a refusal at once from one that waits. One that waited out the answer
    // deadline would take 150 s for these hundred; a stall of a loaded machine stays far under
    // the limit.
    #[test]
    fn a_busy_handler_has_further_requests_refused_without_a_wait() {
        let (handler, was_asked, let_go) = stuck();
        let worker = Worker::start(handler).expect("starts");
        let stop = AtomicBool::new(false);
        assert_eq!(worker.ask(Request::Status, &stop), None);
        assert_eq!(was_asked.recv_timeout(PATIENCE), Ok(Request::Status));
        let start = Instant::now();
        for n in 1..=100 {
            assert_eq!(worker.ask(Request::TabList, &stop), None);
            let spent = start.elapsed();
            assert!(spent < TOGETHER, "{n} refusals took {spent:?}");
        }
        assert_eq!(was_asked.try_recv(), Err(TryRecvError::Empty));
        drop(let_go);
    }

    // A drop that waited out the answer deadline is within what a loaded machine may take for a
    // quick one, so the drop is tried 60 times: such a drop takes 90 s for them, against a limit
    // of 30 s (half a second each, on average). One that waits for the handler takes PATIENCE.
    #[test]
    fn dropping_the_server_takes_the_endpoint_away_without_waiting_for_the_handler() {
        let mut spent = Duration::ZERO;
        for n in 1..=60 {
            let scratch = Place::new();
            let (handler, was_asked, let_go) = stuck();
            let server = Server::start(scratch.path(), &scratch.token, handler).expect("starts");
            assert!(scratch.listed());
            let mut connection = connect(&scratch);
            let status = encode_request(&["status".to_owned()]).expect("encodes");
            connection.write_all(&status).expect("sends");
            was_asked
                .recv_timeout(PATIENCE)
                .expect("the handler has it");
            let start = Instant::now();
            drop(server);
            spent += start.elapsed();
            assert!(spent < TOGETHER, "{n} drops took {spent:?}");
            assert!(!scratch.listed());
            drop(let_go);
        }
    }

    // Without the interrupt the listener sits in its accept for IDLE, an hour: the drop is made
    // on a thread of its own, so that this fails instead of hanging.
    #[test]
    fn dropping_an_idle_server_is_immediate() {
        let scratch = Place::new();
        let (server, _) = serving(&scratch, Code::Done);
        assert_eq!(ask(&scratch, &["status"]).0, Code::Done);
        let (dropped, was_dropped) = mpsc::channel();
        thread::spawn(move || {
            drop(server);
            let _ = dropped.send(());
        });
        was_dropped
            .recv_timeout(TOGETHER)
            .expect("the drop returns");
        assert!(!scratch.listed());
    }

    #[test]
    fn a_reply_that_cannot_be_sent_becomes_a_failure() {
        let scratch = Place::new();
        let (_server, _) = serving(&scratch, Code::Unreachable);
        assert_eq!(
            ask(&scratch, &["status"]),
            (Code::Failed, UNSENDABLE.to_owned())
        );
    }
}
