//! Test helper: a spawned PTY plus a reader thread that accumulates its output.
//!
//! On Windows the reader thread answers ConPTY's cursor-position request (`ESC[6n`) with
//! `ESC[1;1R`, as the VT engine does in the product, and output is compared with CSI and OSC
//! sequences stripped (ConPTY re-renders the screen).

use std::io::Read;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use lune_pty::{PortablePtyBackend, Pty, PtyBackend, PtySize, SpawnOptions};

/// Generous upper bound for anything asynchronous (output, exit, EOF). Never a fixed sleep.
pub(crate) const DEADLINE: Duration = Duration::from_secs(20);

pub(crate) fn size(columns: u16, rows: u16) -> PtySize {
    PtySize {
        columns,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub(crate) fn command(program: &str, args: &[&str]) -> SpawnOptions {
    SpawnOptions {
        program: Some(program.into()),
        args: args.iter().map(|a| a.to_string()).collect(),
        ..SpawnOptions::default()
    }
}

#[derive(Default)]
struct State {
    bytes: Vec<u8>,
    eof: bool,
    error: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("output lock")
    }
}

pub(crate) struct Session {
    pub(crate) pty: Box<dyn Pty>,
    shared: Arc<Shared>,
    /// Keeps the output out of panic messages (for output that may hold secrets).
    quiet: bool,
}

impl Session {
    pub(crate) fn spawn(options: &SpawnOptions, size: PtySize) -> Self {
        let pty = PortablePtyBackend::new()
            .spawn(options, size)
            .expect("spawn");
        Self::start(pty)
    }

    pub(crate) fn start(mut pty: Box<dyn Pty>) -> Self {
        let mut reader = pty.take_reader().expect("take_reader");
        let writer = pty.writer();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("test-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                let mut answered = 0usize;
                loop {
                    let result = reader.read(&mut buf);
                    let mut state = thread_shared.lock();
                    match result {
                        Ok(0) => state.eof = true,
                        Ok(n) => {
                            state.bytes.extend_from_slice(&buf[..n]);
                            if cfg!(windows) {
                                // Count over the whole output: the 4 bytes can split across reads.
                                let asked = count(&state.bytes, b"\x1b[6n");
                                while answered < asked {
                                    let _ = writer.write_all(b"\x1b[1;1R");
                                    answered += 1;
                                }
                            }
                        }
                        Err(e) => {
                            state.error = Some(e.to_string());
                            state.eof = true;
                        }
                    }
                    let done = state.eof;
                    drop(state);
                    thread_shared.changed.notify_all();
                    if done {
                        break;
                    }
                }
            })
            .expect("reader thread");
        Self {
            pty,
            shared,
            quiet: false,
        }
    }

    /// Hides the output in panic messages, e.g. for a child that prints its environment.
    #[cfg_attr(windows, allow(dead_code))] // only the Unix env-dump test uses it
    pub(crate) fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    pub(crate) fn write(&mut self, data: &[u8]) {
        self.pty.write_all(data).expect("write");
    }

    /// Output so far as text (`\r` removed; on Windows CSI/OSC stripped).
    pub(crate) fn text(&self) -> String {
        visible(&self.shared.lock().bytes)
    }

    /// Waits until the output contains `needle`; returns the output. Panics at the deadline.
    pub(crate) fn wait_for(&self, needle: &str, deadline: Duration) -> String {
        let start = Instant::now();
        let mut state = self.shared.lock();
        loop {
            let text = visible(&state.bytes);
            if text.contains(needle) {
                return text;
            }
            let left = deadline.saturating_sub(start.elapsed());
            assert!(
                !left.is_zero(),
                "{needle:?} not seen within {deadline:?}; eof={} error={:?} output: {:?}",
                state.eof,
                state.error,
                self.shown(text)
            );
            if state.eof {
                panic!(
                    "EOF before {needle:?}; error={:?} output: {:?}",
                    state.error,
                    self.shown(text)
                );
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left.min(Duration::from_millis(200)))
                .expect("output lock")
                .0;
        }
    }

    /// Waits for the reader's `Ok(0)`. Panics at the deadline or on a read error.
    pub(crate) fn wait_eof(&self, deadline: Duration) {
        let start = Instant::now();
        let mut state = self.shared.lock();
        while !state.eof {
            let left = deadline.saturating_sub(start.elapsed());
            assert!(
                !left.is_zero(),
                "no EOF within {deadline:?}; output: {:?}",
                self.shown(visible(&state.bytes))
            );
            state = self
                .shared
                .changed
                .wait_timeout(state, left.min(Duration::from_millis(200)))
                .expect("output lock")
                .0;
        }
        assert_eq!(state.error, None, "reader ended with an error");
    }

    /// Polls `try_exit_code` until it is `Some`. Panics at the deadline.
    pub(crate) fn wait_exit(&mut self, deadline: Duration) -> i32 {
        let start = Instant::now();
        loop {
            if let Some(code) = self.pty.try_exit_code().expect("try_exit_code") {
                return code;
            }
            assert!(
                start.elapsed() < deadline,
                "child did not exit within {deadline:?}; output: {:?}",
                self.shown(self.text())
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Session {
    fn shown(&self, text: String) -> String {
        if self.quiet { "<hidden>".into() } else { text }
    }
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

fn visible(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace('\r', "");
    if cfg!(windows) {
        strip_escapes(&text)
    } else {
        text
    }
}

/// Removes CSI (`ESC [ ... final`), OSC (`ESC ] ... BEL | ESC \`) and other two-byte escapes.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
