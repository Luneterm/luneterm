//! Public engine contract.
//!
//! Mirrors Lunegit's `IVtEngine` (`Lunegit.Core/Terminal/IVtEngine.cs`) so Lunegit can later
//! swap its V8-hosted xterm.js for this engine through a C ABI crate without changing callers.
//! Provisional: the viewport/snapshot types land with the buffer port.

/// Something the engine wants the host to act on, produced while processing [`VtEngine::write`].
///
/// Lunegit fires these as synchronous events inside `Write`; here the engine queues them and the
/// host drains them after each write, which keeps the engine free of callbacks into the host.
/// Events are queued in the order upstream fires them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Bytes to send back to the application (DA/DSR/DECRQM/XTVERSION replies).
    Response(Vec<u8>),
    /// OSC 52 clipboard write. Queries are deliberately never answered.
    ClipboardWrite { selection: String, data: String },
    /// An OSC the engine and its extensions did not handle (ident and payload length only).
    UnknownOsc { ident: u32, len: usize },
    /// BEL (upstream `onRequestBell`).
    Bell,
    /// Window title changed (OSC 0/2).
    Title(String),
    /// A printed character, fired only with `screen_reader_mode` on (upstream `onA11yChar`).
    A11yChar(String),
    /// HT moved the cursor by this many cells, fired only with `screen_reader_mode` on (upstream
    /// `onA11yTab`).
    A11yTab(usize),
}

/// What changed on screen since the host last took it, coalesced (upstream's
/// `onRequestRefreshRows`, `onCursorMove` and `onScroll` events, which fire far too often to
/// queue one by one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Damage {
    /// Viewport rows to redraw, first and last inclusive; the union of every refresh request.
    pub rows: Option<(usize, usize)>,
    /// Everything must be redrawn (resize, buffer switch).
    pub full: bool,
    /// The cursor moved during a write.
    pub cursor_moved: bool,
    /// The buffer scrolled (new lines, `ydisp` may have changed).
    pub scrolled: bool,
}

impl Damage {
    /// Adds the viewport rows `start ..= end` to [`Damage::rows`].
    pub fn add_rows(&mut self, start: usize, end: usize) {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.rows = Some(match self.rows {
            Some((old_start, old_end)) => (old_start.min(start), old_end.max(end)),
            None => (start, end),
        });
    }

    /// Nothing to redraw.
    pub fn is_empty(&self) -> bool {
        *self == Damage::default()
    }
}

/// A VT engine instance: one per terminal session. Not thread-safe by design; the session owns it.
pub trait VtEngine {
    fn columns(&self) -> u16;
    fn rows(&self) -> u16;
    /// Feed raw bytes from the PTY. Partial UTF-8 and partial escape sequences may be split
    /// across calls.
    fn write(&mut self, data: &[u8]);
    fn resize(&mut self, columns: u16, rows: u16);
    fn scroll_lines(&mut self, delta: i32);
    /// Take the events produced since the last call.
    fn drain_events(&mut self) -> Vec<HostEvent>;
}
