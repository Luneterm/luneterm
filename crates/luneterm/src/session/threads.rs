//! The three session threads and what they share (plan M9 D1, D2; invariants I1-I3, I6, I13).
//!
//! ```text
//!  reader ──chunks──▶ Inbox ◀──commands── UI
//!                       │                  ▲ take_update (mailbox, O(1))
//!                       ▼                  │
//!                    engine ──Update──▶ Mailbox ──waker()──▶ UI event loop
//!                       │
//!                       └──WriteItem──▶ WriteQueue ──▶ writer ──▶ PtyWriter
//! ```
//!
//! Locks: `inbox` and `mailbox` (shared with the UI) and the write queue's (engine and writer
//! only). Each is held for a queue operation only, never across a `Terminal` call, PTY I/O or
//! `waker()` (I1). The engine never touches PTY input (I2): every write goes through the writer
//! thread, so a child that blocks writing output while we write its input cannot deadlock us.
//!
//! Publish/wake order (sketch trap 1). Engine: `ui_ready = false`, `wake_pending.swap(true)`,
//! store the Update, `waker()` if the swap saw `false`. UI (`take_update`): clear
//! `wake_pending`, take the Update, then `ui_ready = true`. The engine publishes only after
//! `ui_ready`, which the UI sets only after clearing the flag, so the swap always sees `false`:
//! exactly one wake per Update, and none is lost.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use lune_pty::PtyWriter;

use super::engine::{EngineCore, TickInput};
use super::update::Update;
use super::{Command, Geometry, SessionStats};

/// The reader waits while this much output is queued (D2).
pub(crate) const INBOX_HIGH: usize = 1 << 20;
/// One PTY read.
pub(crate) const READ_SIZE: usize = 64 * 1024;
/// Writer caps (D2).
pub(crate) const RESPONSE_CAP: usize = 64 * 1024;
pub(crate) const INPUT_CAP: usize = 1 << 20;
pub(crate) const PASTE_CAP: usize = 2;
/// The writer writes at most this much per `write_all`.
pub(crate) const WRITE_PIECE: usize = 64 * 1024;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn max(a: &AtomicUsize, v: usize) {
    a.fetch_max(v, Ordering::Relaxed);
}

/// Counters (I3, I16: counts only).
#[derive(Debug, Default)]
pub(crate) struct Stats {
    inbox_high_water: AtomicUsize,
    reader_waits: AtomicU64,
    bytes_read: AtomicU64,
    events_per_drain_high_water: AtomicUsize,
    responses_high_water: AtomicUsize,
    responses_dropped: AtomicU64,
    input_high_water: AtomicUsize,
    input_dropped: AtomicU64,
    pastes_high_water: AtomicUsize,
    pastes_dropped: AtomicU64,
    bytes_written: AtomicU64,
    updates_published: AtomicU64,
    step: AtomicUsize,
}

impl Stats {
    pub(crate) fn snapshot(&self) -> SessionStats {
        let r = Ordering::Relaxed;
        SessionStats {
            inbox_high_water: self.inbox_high_water.load(r),
            reader_waits: self.reader_waits.load(r),
            bytes_read: self.bytes_read.load(r),
            events_per_drain_high_water: self.events_per_drain_high_water.load(r),
            responses_high_water: self.responses_high_water.load(r),
            responses_dropped: self.responses_dropped.load(r),
            input_high_water: self.input_high_water.load(r),
            input_dropped: self.input_dropped.load(r),
            pastes_high_water: self.pastes_high_water.load(r),
            pastes_dropped: self.pastes_dropped.load(r),
            bytes_written: self.bytes_written.load(r),
            updates_published: self.updates_published.load(r),
            step: self.step.load(r),
        }
    }

    pub(crate) fn events_per_drain(&self, n: usize) {
        max(&self.events_per_drain_high_water, n);
    }

    pub(crate) fn set_step(&self, step: usize) {
        self.step.store(step, Ordering::Relaxed);
    }
}

/// Reader → engine and UI → engine.
#[derive(Debug)]
pub(crate) struct Inbox {
    pub(crate) chunks: VecDeque<Vec<u8>>,
    /// Bytes in `chunks`.
    pub(crate) queued: usize,
    pub(crate) reader_ended: bool,
    /// The latest Resize (I11: collapsed, last wins; applied before the other commands).
    pub(crate) resize: Option<Geometry>,
    pub(crate) commands: VecDeque<Command>,
    pub(crate) ui_ready: bool,
    pub(crate) visible: bool,
    pub(crate) shutdown: bool,
}

impl Inbox {
    /// O(1).
    pub(crate) fn push_command(&mut self, command: Command) {
        match command {
            Command::Resize(g) => self.resize = Some(g),
            other => self.commands.push_back(other),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) inbox: Mutex<Inbox>,
    pub(crate) inbox_cv: Condvar,
    /// A command is waiting: ends the current slice without taking the lock.
    pub(crate) has_commands: AtomicBool,
    pub(crate) mailbox: Mutex<Option<Update>>,
    pub(crate) wake_pending: AtomicBool,
    pub(crate) stats: Arc<Stats>,
}

impl Shared {
    pub(crate) fn new(stats: Arc<Stats>) -> Shared {
        Shared {
            inbox: Mutex::new(Inbox {
                chunks: VecDeque::new(),
                queued: 0,
                reader_ended: false,
                resize: None,
                commands: VecDeque::new(),
                ui_ready: true,
                visible: true,
                shutdown: false,
            }),
            inbox_cv: Condvar::new(),
            has_commands: AtomicBool::new(false),
            mailbox: Mutex::new(None),
            wake_pending: AtomicBool::new(false),
            stats,
        }
    }

    pub(crate) fn send(&self, command: Command) {
        let mut inbox = lock(&self.inbox);
        inbox.push_command(command);
        self.has_commands.store(true, Ordering::SeqCst);
        drop(inbox);
        self.inbox_cv.notify_all();
    }

    pub(crate) fn take_update(&self) -> Option<Update> {
        // Order matters (module docs): clear, take, then ready.
        self.wake_pending.store(false, Ordering::SeqCst);
        let update = lock(&self.mailbox).take();
        if update.is_some() {
            lock(&self.inbox).ui_ready = true;
            self.inbox_cv.notify_all();
        }
        update
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        lock(&self.inbox).visible = visible;
        self.inbox_cv.notify_all();
    }

    pub(crate) fn shutdown(&self) {
        lock(&self.inbox).shutdown = true;
        self.inbox_cv.notify_all();
    }

    /// Engine side of the publish order (module docs).
    fn publish(&self, update: Update, waker: &dyn Fn()) {
        lock(&self.inbox).ui_ready = false;
        let woken = self.wake_pending.swap(true, Ordering::SeqCst);
        self.stats.updates_published.fetch_add(1, Ordering::SeqCst);
        *lock(&self.mailbox) = Some(update);
        if !woken {
            waker();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteKind {
    /// A reply to the child (DA, DSR, colour reports, ...): capped at `RESPONSE_CAP` pending.
    Response,
    /// Typed input (keys, text, mouse, focus, wheel arrows): capped at `INPUT_CAP` pending.
    Input,
    /// One paste: at most `PASTE_CAP` queued or in progress.
    Paste,
}

#[derive(Debug)]
pub(crate) struct WriteItem {
    pub(crate) kind: WriteKind,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct WriteState {
    items: VecDeque<WriteItem>,
    response_bytes: usize,
    input_bytes: usize,
    pastes: usize,
    closed: bool,
}

/// Engine → writer. One FIFO for every kind, so replies and input reach the child in order.
#[derive(Debug)]
pub(crate) struct WriteQueue {
    state: Mutex<WriteState>,
    cv: Condvar,
    shutdown: AtomicBool,
    stats: Arc<Stats>,
}

impl WriteQueue {
    pub(crate) fn new(stats: Arc<Stats>) -> WriteQueue {
        WriteQueue {
            state: Mutex::new(WriteState::default()),
            cv: Condvar::new(),
            shutdown: AtomicBool::new(false),
            stats,
        }
    }

    /// Never blocks: past a cap the item is dropped and counted.
    pub(crate) fn push(&self, kind: WriteKind, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let mut s = lock(&self.state);
        if s.closed {
            return;
        }
        let len = bytes.len();
        let stats = &self.stats;
        match kind {
            WriteKind::Response => {
                if s.response_bytes.saturating_add(len) > RESPONSE_CAP {
                    stats.responses_dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                s.response_bytes += len;
                max(&stats.responses_high_water, s.response_bytes);
            }
            WriteKind::Input => {
                if s.input_bytes.saturating_add(len) > INPUT_CAP {
                    stats.input_dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                s.input_bytes += len;
                max(&stats.input_high_water, s.input_bytes);
            }
            WriteKind::Paste => {
                if s.pastes >= PASTE_CAP {
                    stats.pastes_dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                s.pastes += 1;
                max(&stats.pastes_high_water, s.pastes);
            }
        }
        s.items.push_back(WriteItem { kind, bytes });
        drop(s);
        self.cv.notify_all();
    }

    pub(crate) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _guard = lock(&self.state);
        self.cv.notify_all();
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// The next item for the writer; `None` on shutdown.
    fn next(&self) -> Option<WriteItem> {
        let mut s = lock(&self.state);
        loop {
            if self.is_shutdown() {
                return None;
            }
            if let Some(item) = s.items.pop_front() {
                match item.kind {
                    WriteKind::Response => s.response_bytes -= item.bytes.len(),
                    WriteKind::Input => s.input_bytes -= item.bytes.len(),
                    // A paste counts until it is written.
                    WriteKind::Paste => {}
                }
                return Some(item);
            }
            s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn finished(&self, item: &WriteItem) {
        if item.kind == WriteKind::Paste {
            let mut s = lock(&self.state);
            s.pastes = s.pastes.saturating_sub(1);
        }
    }

    /// The writer failed: nothing more is queued.
    fn close(&self) {
        let mut s = lock(&self.state);
        s.closed = true;
        s.items.clear();
        s.response_bytes = 0;
        s.input_bytes = 0;
        s.pastes = 0;
    }

    /// Every queued item, oldest first, accounted as written (single-threaded tests).
    #[cfg(test)]
    pub(crate) fn take_all(&self) -> Vec<WriteItem> {
        let mut s = lock(&self.state);
        s.response_bytes = 0;
        s.input_bytes = 0;
        s.pastes = 0;
        s.items.drain(..).collect()
    }
}

/// Reader thread: 64 KiB reads into the inbox; waits (backpressure) while 1 MiB is queued. Holds
/// no lock while blocked in `read` (I13).
pub(crate) fn reader_loop(mut reader: Box<dyn Read + Send>, shared: Arc<Shared>) {
    let mut buf = vec![0u8; READ_SIZE];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // Linux reports EIO once the child is gone: the end of output, like EOF.
            Err(_) => break,
        };
        let mut inbox = lock(&shared.inbox);
        if inbox.queued >= INBOX_HIGH && !inbox.shutdown {
            shared.stats.reader_waits.fetch_add(1, Ordering::Relaxed);
            while inbox.queued >= INBOX_HIGH && !inbox.shutdown {
                inbox = shared
                    .inbox_cv
                    .wait(inbox)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
        if inbox.shutdown {
            return;
        }
        inbox
            .chunks
            .push_back(buf.get(..n).unwrap_or_default().to_vec());
        inbox.queued += n;
        max(&shared.stats.inbox_high_water, inbox.queued);
        shared
            .stats
            .bytes_read
            .fetch_add(n as u64, Ordering::Relaxed);
        drop(inbox);
        shared.inbox_cv.notify_all();
    }
    lock(&shared.inbox).reader_ended = true;
    shared.inbox_cv.notify_all();
}

/// Engine thread: waits for work, takes it under the lock, runs one tick without it, publishes.
/// Returns on shutdown; dropping `core` drops the `Pty`, which kills the child (I13).
pub(crate) fn engine_loop(mut core: EngineCore, shared: Arc<Shared>, waker: Box<dyn Fn() + Send>) {
    loop {
        // Read before taking the lock: it calls the `Terminal` (I1). Only this thread's tick
        // changes it, so it holds for the whole wait.
        let unpublished = core.unpublished();
        let input = {
            let mut inbox = lock(&shared.inbox);
            loop {
                if inbox.shutdown {
                    return;
                }
                let now = core.now();
                let (ui_ready, visible) = (inbox.ui_ready, inbox.visible);
                let new_output = !inbox.chunks.is_empty() && core.wants_chunks();
                let ended = inbox.reader_ended && inbox.chunks.is_empty();
                if inbox.resize.is_some()
                    || !inbox.commands.is_empty()
                    || new_output
                    || ended
                    || core.wants_tick(now, ui_ready, visible, unpublished)
                {
                    break;
                }
                inbox = match core.next_deadline(now, ui_ready, visible, unpublished) {
                    Some(at) => {
                        shared
                            .inbox_cv
                            .wait_timeout(inbox, at.saturating_sub(now))
                            .unwrap_or_else(|e| e.into_inner())
                            .0
                    }
                    None => shared
                        .inbox_cv
                        .wait(inbox)
                        .unwrap_or_else(|e| e.into_inner()),
                };
            }
            let mut input = TickInput {
                ui_ready: inbox.ui_ready,
                visible: inbox.visible,
                ..TickInput::default()
            };
            if core.wants_chunks() {
                input.chunks = inbox.chunks.drain(..).collect();
                inbox.queued = 0;
            }
            if inbox.reader_ended && inbox.chunks.is_empty() {
                // Every chunk is handed over; the core reports the end after parsing them.
                // Handed over once.
                input.reader_ended = true;
                inbox.reader_ended = false;
            }
            input.resize = inbox.resize.take();
            input.commands = inbox.commands.drain(..).collect();
            shared.has_commands.store(false, Ordering::SeqCst);
            drop(inbox);
            // Room for the reader.
            shared.inbox_cv.notify_all();
            input
        };
        let interrupt = || shared.has_commands.load(Ordering::SeqCst);
        if let Some(update) = core.tick(input, &interrupt) {
            shared.publish(update, waker.as_ref());
        }
    }
}

/// Writer thread: the only place PTY input is written (I2). Exits on shutdown or on its first
/// write error (I13).
pub(crate) fn writer_loop(queue: Arc<WriteQueue>, writer: PtyWriter) {
    while let Some(item) = queue.next() {
        let mut failed = false;
        for piece in item.bytes.chunks(WRITE_PIECE) {
            if queue.is_shutdown() {
                break;
            }
            if writer.write_all(piece).is_err() {
                failed = true;
                break;
            }
            queue
                .stats
                .bytes_written
                .fetch_add(piece.len() as u64, Ordering::Relaxed);
        }
        queue.finished(&item);
        if failed {
            queue.close();
            return;
        }
    }
}
