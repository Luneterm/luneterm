//! One terminal session: a child on a PTY, the engine that parses its output, and the queues
//! between them and the UI (plan M9 step 3, decisions D1, D2, D8-D12).
//!
//! Three threads per session: `luneterm-reader` blocks on the PTY output, `luneterm-engine` owns
//! the `Terminal` and the `Pty`, and `luneterm-writer` does every PTY write. The UI thread only
//! calls [`Session`]'s methods, each O(1) under a lock held for one queue operation (I1). The
//! protocol is in `threads.rs`, the engine's work in `engine.rs`.

mod engine;
mod host;
mod threads;
mod update;

#[cfg(test)]
mod fake_pty;
#[cfg(test)]
mod tests;

use std::fmt;
use std::io;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lune_pty::{Pty, PtySize};
use lune_vt::services::options_service::WindowsPty;
use lune_vt::{TerminalKeyEvent, TerminalMouseEvent};

pub use crate::outline::CommandState;
pub use host::HostPalette;
pub use update::{
    Exit, ForegroundState, LINK_URIS_CAP, MARKS_CAP, MAX_LINK_URI, Update, UpdateModes,
    apply_update,
};

use engine::{EngineCore, RealClock, build_terminal};
use threads::{Shared, Stats, WriteQueue, engine_loop, reader_loop, writer_loop};

/// Size caps (D11): the session clamps rather than rejects.
pub const MAX_COLS: usize = 4096;
pub const MAX_ROWS: usize = 2048;
pub const MIN_COLS: usize = 2;
pub const MIN_ROWS: usize = 1;
pub const MAX_SCROLLBACK: usize = 100_000;

/// The grid's size in cells and pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub cols: usize,
    pub rows: usize,
    /// One cell, logical px (width, height).
    pub cell_px: (f32, f32),
    /// The grid area, physical px (width, height).
    pub pixel_size: (u32, u32),
}

impl Geometry {
    /// Cols in 2..=4096, rows in 1..=2048 (I10).
    pub fn clamped(self) -> Geometry {
        Geometry {
            cols: self.cols.clamp(MIN_COLS, MAX_COLS),
            rows: self.rows.clamp(MIN_ROWS, MAX_ROWS),
            ..self
        }
    }

    /// The PTY size of the clamped geometry; pixel sizes saturate to `u16`.
    pub fn pty_size(self) -> PtySize {
        let g = self.clamped();
        let sat = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
        PtySize {
            // At most 4096 and 2048 after clamping.
            columns: u16::try_from(g.cols).unwrap_or(u16::MAX),
            rows: u16::try_from(g.rows).unwrap_or(u16::MAX),
            pixel_width: sat(g.pixel_size.0),
            pixel_height: sat(g.pixel_size.1),
        }
    }
}

/// How to scroll the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Lines; negative scrolls up.
    Lines(i64),
    /// Pages of `rows - 1` lines; negative scrolls up.
    Pages(i32),
    Top,
    Bottom,
}

/// UI → engine.
#[derive(Clone)]
pub enum Command {
    Key(TerminalKeyEvent),
    /// Typed or IME-committed text, sent as user input.
    Text(String),
    /// Clipboard text; the engine runs `Terminal::paste` (the only paste path).
    Paste(String),
    Mouse(TerminalMouseEvent),
    /// Alternate-screen wheel without mouse tracking: one arrow key, by DECCKM (D7).
    WheelArrows {
        up: bool,
    },
    Focus(bool),
    Scroll(Scroll),
    /// Collapsed in `send` (last wins).
    Resize(Geometry),
    /// The OSC 52 engine gate.
    SetClipboardWrite(bool),
    /// Scrollback lines, capped at [`MAX_SCROLLBACK`] (a setting changed live).
    SetScrollback(u32),
}

impl fmt::Debug for Command {
    /// Variant and sizes only: never typed or pasted text (I16).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Key(_) => f.write_str("Key"),
            Command::Text(t) => write!(f, "Text({} bytes)", t.len()),
            Command::Paste(t) => write!(f, "Paste({} bytes)", t.len()),
            Command::Mouse(_) => f.write_str("Mouse"),
            Command::WheelArrows { up } => write!(f, "WheelArrows {{ up: {up} }}"),
            Command::Focus(b) => write!(f, "Focus({b})"),
            Command::Scroll(s) => write!(f, "Scroll({s:?})"),
            Command::Resize(g) => write!(f, "Resize({g:?})"),
            Command::SetClipboardWrite(b) => write!(f, "SetClipboardWrite({b})"),
            Command::SetScrollback(n) => write!(f, "SetScrollback({n})"),
        }
    }
}

/// How a session's engine is set up.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Clamped (I10).
    pub geometry: Geometry,
    /// Capped at [`MAX_SCROLLBACK`].
    pub scrollback: usize,
    /// What XTVERSION reports, e.g. `Luneterm(0.1.0)`.
    pub xtversion: String,
    /// The OSC 52 engine gate at start.
    pub clipboard_write: bool,
    /// Windows: the ConPTY build, for the engine's reflow heuristics.
    pub windows_pty: Option<WindowsPty>,
    /// The theme colour reports answer from, and restores return to.
    pub palette: HostPalette,
    /// The window's focus at start (focus reports answer from it until a `Focus` command).
    pub focused: bool,
}

/// Counters for memory bounds and flow control (counts only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStats {
    /// Most output bytes queued in the inbox at once.
    pub inbox_high_water: usize,
    /// Times the reader waited for room.
    pub reader_waits: u64,
    pub bytes_read: u64,
    /// Most events one drain returned.
    pub events_per_drain_high_water: usize,
    pub responses_high_water: usize,
    pub responses_dropped: u64,
    pub input_high_water: usize,
    pub input_dropped: u64,
    pub pastes_high_water: usize,
    pub pastes_dropped: u64,
    /// Bytes the writer wrote to the PTY.
    pub bytes_written: u64,
    pub updates_published: u64,
    /// The current sub-write size.
    pub step: usize,
}

/// A running session. Dropping it shuts the session down without waiting.
pub struct Session {
    shared: Arc<Shared>,
    out: Arc<WriteQueue>,
    engine: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    /// Starts the reader, engine and writer threads on `pty`. `waker` is called on the engine
    /// thread once per published Update; the UI then calls [`Session::take_update`]. Fails, with
    /// no thread left running, when the engine cannot be built or a thread cannot be spawned.
    pub fn start(
        mut pty: Box<dyn Pty>,
        config: SessionConfig,
        waker: Box<dyn Fn() + Send>,
    ) -> io::Result<Session> {
        let terminal = build_terminal(&config)?;
        let reader = pty.take_reader()?;
        let pty_writer = pty.writer();
        let stats = Arc::new(Stats::default());
        let shared = Arc::new(Shared::new(Arc::clone(&stats)));
        let out = Arc::new(WriteQueue::new(Arc::clone(&stats)));
        let core = EngineCore::new(
            terminal,
            pty,
            &config,
            Box::new(RealClock::new()),
            Arc::clone(&out),
            stats,
        );

        let abort = |shared: &Shared, out: &WriteQueue| {
            shared.shutdown();
            out.shutdown();
        };
        let q = Arc::clone(&out);
        let writer = thread::Builder::new()
            .name("luneterm-writer".into())
            .spawn(move || writer_loop(q, pty_writer))?;
        let s = Arc::clone(&shared);
        let engine = match thread::Builder::new()
            .name("luneterm-engine".into())
            .spawn(move || engine_loop(core, s, waker))
        {
            Ok(h) => h,
            Err(e) => {
                abort(&shared, &out);
                return Err(e);
            }
        };
        let s = Arc::clone(&shared);
        if let Err(e) = thread::Builder::new()
            .name("luneterm-reader".into())
            .spawn(move || reader_loop(reader, s))
        {
            abort(&shared, &out);
            return Err(e);
        }
        Ok(Session {
            shared,
            out,
            engine: Some(engine),
            writer: Some(writer),
        })
    }

    /// Queues a command for the engine. O(1); never waits on engine work.
    pub fn send(&self, command: Command) {
        self.shared.send(command);
    }

    /// The published Update, if any. O(1). The next one is built only after this is taken.
    pub fn take_update(&self) -> Option<Update> {
        self.shared.take_update()
    }

    /// While hidden, Updates come at most every 100 ms.
    pub fn set_visible(&self, visible: bool) {
        self.shared.set_visible(visible);
    }

    pub fn stats(&self) -> SessionStats {
        self.shared.stats.snapshot()
    }

    /// Shuts down and waits up to `timeout` for the engine and writer threads (never the
    /// reader, which can stay blocked while a background job holds the PTY). `true` if both
    /// ended in time.
    pub fn close(mut self, timeout: Duration) -> bool {
        self.signal_shutdown();
        let deadline = Instant::now() + timeout;
        let handles = [self.engine.take(), self.writer.take()];
        loop {
            if handles.iter().flatten().all(JoinHandle::is_finished) {
                for h in handles.into_iter().flatten() {
                    let _ = h.join();
                }
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Signals shutdown without waiting or consuming: the threads stop and the child is killed
    /// in the background. Idempotent; [`Session::close`] after it only waits.
    pub fn begin_close(&self) {
        self.signal_shutdown();
    }

    fn signal_shutdown(&self) {
        self.shared.shutdown();
        self.out.shutdown();
    }
}

impl Drop for Session {
    /// Signals shutdown and returns without joining.
    fn drop(&mut self) {
        self.signal_shutdown();
    }
}
