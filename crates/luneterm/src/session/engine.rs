//! The engine thread's work, as plain methods (plan M9 D1, D2, D10; invariants I4-I6, I8-I15).
//!
//! [`EngineCore`] owns the `Terminal` and the `Pty` and does everything the engine thread does
//! between two waits: apply commands, parse one time-bounded slice of output, drain events and
//! damage after every write, poll the exit status, and build an Update when one may be
//! published. It never blocks: PTY input goes to the writer queue (`WriteQueue::push`, which
//! drops past its caps). The thread loop in `threads.rs` only waits and hands it work, so the
//! tests can step it with a fake clock and no threads.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lune_pty::Pty;
use lune_vt::services::options_service::OptionValue;
use lune_vt::{HostEvent, StandardExtensions, Terminal, ViewportCell, ViewportState};

use super::host::{ColorState, window_report};
use super::threads::{Stats, WriteKind, WriteQueue};
use super::update::{Exit, MARKS_CAP, Update, UpdateModes, to_cell};
use super::{Command, Geometry, MAX_SCROLLBACK, Scroll, SessionConfig};

/// One output slice runs at most this long (plus the write in flight).
pub(crate) const SLICE: Duration = Duration::from_millis(4);
/// Sub-write bounds.
pub(crate) const STEP_MIN: usize = 256;
pub(crate) const STEP_MAX: usize = 64 * 1024;
const STEP_START: usize = 16 * 1024;
/// A write slower than this halves the step; one faster than `FAST_WRITE` doubles it.
const SLOW_WRITE: Duration = Duration::from_millis(2);
const FAST_WRITE: Duration = Duration::from_micros(500);
/// Mode 2026 holds publishing at most this long (xterm.js `SYNCHRONIZED_OUTPUT_TIMEOUT_MS`).
pub(crate) const SYNC_TIMEOUT: Duration = Duration::from_millis(1000);
/// While hidden, at most one Update per this interval.
pub(crate) const HIDDEN_INTERVAL: Duration = Duration::from_millis(100);
/// After the output ends, the exit status is polled this often, for at most `EXIT_WAIT`.
const EXIT_POLL: Duration = Duration::from_millis(20);
const EXIT_WAIT: Duration = Duration::from_secs(1);

/// The engine's time source: monotonic time since some start. Tests inject a fake one.
pub(crate) trait Clock: Send {
    fn now(&self) -> Duration;
    /// Called after every `Terminal::write` with its length; a fake clock charges for it.
    fn charge(&self, _bytes: usize) {}
}

#[derive(Debug)]
pub(crate) struct RealClock(Instant);

impl RealClock {
    pub(crate) fn new() -> RealClock {
        RealClock(Instant::now())
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// What the thread loop hands one tick.
#[derive(Debug, Default)]
pub(crate) struct TickInput {
    pub(crate) resize: Option<Geometry>,
    pub(crate) commands: Vec<Command>,
    pub(crate) chunks: Vec<Vec<u8>>,
    /// The reader has ended, and every chunk it read is in `chunks` or already handed over.
    pub(crate) reader_ended: bool,
    /// The UI took the last Update.
    pub(crate) ui_ready: bool,
    pub(crate) visible: bool,
}

/// What changed since the last Update.
#[derive(Debug, Default)]
struct Acc {
    rows: Option<(usize, usize)>,
    full: bool,
    bells: u32,
    title: bool,
    clipboard: Option<String>,
    palette: bool,
    cwd: Option<String>,
    progress: Option<lune_vt::Progress>,
    marks: VecDeque<lune_vt::CommandMark>,
    sync_in_command: bool,
}

impl Acc {
    fn any(&self) -> bool {
        self.full
            || self.rows.is_some()
            || self.bells > 0
            || self.title
            || self.clipboard.is_some()
            || self.palette
            || self.cwd.is_some()
            || self.progress.is_some()
            || !self.marks.is_empty()
            || self.sync_in_command
    }

    fn add_rows(&mut self, start: usize, end: usize) {
        self.rows = Some(match self.rows {
            Some((a, b)) => (a.min(start), b.max(end)),
            None => (start, end),
        });
    }
}

#[derive(Debug)]
struct Published {
    state: ViewportState,
    modes: UpdateModes,
}

/// The active buffer holds fewer lines than `base_y + rows` (lune-vt's scrollback-0 soft
/// invariant, M5): viewport rows then map to lines the engine's damage rows do not describe, so
/// damage cannot be trusted while it holds or as it ends.
fn lines_missing(st: &ViewportState) -> bool {
    st.total_lines < st.base_y.saturating_add(st.rows)
}

/// Timing of the last slice (for the slice tests).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SliceLog {
    pub(crate) elapsed: Duration,
    pub(crate) last_write: Duration,
    pub(crate) writes: usize,
}

pub(crate) struct EngineCore {
    pub(crate) terminal: Terminal,
    pty: Box<dyn Pty>,
    clock: Box<dyn Clock>,
    out: Arc<WriteQueue>,
    stats: Arc<Stats>,
    colors: ColorState,
    geometry: Geometry,
    focused: bool,
    step: usize,
    pending: VecDeque<Vec<u8>>,
    offset: usize,
    acc: Acc,
    last: Option<Published>,
    seq: u64,
    last_publish: Option<Duration>,
    sync_since: Option<Duration>,
    sync_timed_out: bool,
    idle_pending: bool,
    reader_end_at: Option<Duration>,
    next_exit_poll: Duration,
    exit_reported: bool,
    exit_pending: Option<Exit>,
    scratch: Vec<ViewportCell>,
    pub(crate) slice_log: SliceLog,
}

impl std::fmt::Debug for EngineCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Counts only (I16): no screen content, title or queued bytes.
        f.debug_struct("EngineCore")
            .field("step", &self.step)
            .field("pending_chunks", &self.pending.len())
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

/// The engine's terminal for `config` (I9): size from the clamped geometry, capped scrollback,
/// `WindowsPty` when given, the standard extensions, no logger.
pub(crate) fn build_terminal(config: &SessionConfig) -> io::Result<Terminal> {
    let g = config.geometry.clamped();
    let mut options = vec![
        OptionValue::Cols(g.cols),
        OptionValue::Rows(g.rows),
        OptionValue::Scrollback(config.scrollback.min(MAX_SCROLLBACK)),
    ];
    if let Some(pty) = config.windows_pty {
        options.push(OptionValue::WindowsPty(Some(pty)));
    }
    let mut terminal = Terminal::new(options);
    terminal
        .install_standard_extensions(&StandardExtensions {
            xtversion: Some(config.xtversion.clone()),
            vs16: true,
            clipboard_write: config.clipboard_write,
        })
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    Ok(terminal)
}

impl EngineCore {
    pub(crate) fn new(
        terminal: Terminal,
        pty: Box<dyn Pty>,
        config: &SessionConfig,
        clock: Box<dyn Clock>,
        out: Arc<WriteQueue>,
        stats: Arc<Stats>,
    ) -> EngineCore {
        let next_exit_poll = clock.now();
        EngineCore {
            terminal,
            pty,
            clock,
            out,
            stats,
            colors: ColorState::new(config.palette.clone()),
            geometry: config.geometry.clamped(),
            focused: config.focused,
            step: STEP_START,
            pending: VecDeque::new(),
            offset: 0,
            acc: Acc::default(),
            last: None,
            seq: 0,
            last_publish: None,
            sync_since: None,
            sync_timed_out: false,
            idle_pending: false,
            reader_end_at: None,
            next_exit_poll,
            exit_reported: false,
            exit_pending: None,
            scratch: Vec::new(),
            slice_log: SliceLog::default(),
        }
    }

    pub(crate) fn now(&self) -> Duration {
        self.clock.now()
    }

    #[cfg(test)]
    pub(crate) fn step(&self) -> usize {
        self.step
    }

    /// The engine has parsed everything it was handed: the thread loop may give it more chunks.
    pub(crate) fn wants_chunks(&self) -> bool {
        self.pending.is_empty()
    }

    /// One iteration of the engine loop: commands first (latency), then one output slice, idle
    /// work, the exit poll, and an Update if one may be published now.
    pub(crate) fn tick(
        &mut self,
        input: TickInput,
        interrupt: &dyn Fn() -> bool,
    ) -> Option<Update> {
        if let Some(g) = input.resize {
            self.apply_command(Command::Resize(g));
        }
        for c in input.commands {
            self.apply_command(c);
        }
        self.pending
            .extend(input.chunks.into_iter().filter(|c| !c.is_empty()));
        if input.reader_ended && self.reader_end_at.is_none() {
            let now = self.clock.now();
            self.reader_end_at = Some(now);
            self.next_exit_poll = now;
        }
        self.run_slice(interrupt);
        if self.pending.is_empty() && self.idle_pending {
            self.idle_pending = self.terminal.idle();
        }
        let now = self.clock.now();
        self.poll_exit(now);
        if input.ui_ready && self.unpublished() && self.may_publish(now, input.visible) {
            Some(self.build_update(now))
        } else {
            None
        }
    }

    /// Something to do without waiting for new input. `unpublished` is [`Self::unpublished`],
    /// read by the caller before it took the inbox lock: no `Terminal` call runs under a lock
    /// the UI takes (I1).
    pub(crate) fn wants_tick(
        &mut self,
        now: Duration,
        ui_ready: bool,
        visible: bool,
        unpublished: bool,
    ) -> bool {
        !self.pending.is_empty()
            || self.idle_pending
            || self.exit_poll_due(now)
            || (ui_ready && unpublished && self.may_publish(now, visible))
    }

    /// When to look again with no new input (2026 timeout, hidden tick, exit poll).
    /// `unpublished` as for [`Self::wants_tick`].
    pub(crate) fn next_deadline(
        &mut self,
        now: Duration,
        ui_ready: bool,
        visible: bool,
        unpublished: bool,
    ) -> Option<Duration> {
        let mut deadline: Option<Duration> = None;
        let mut at = |d: Duration| deadline = Some(deadline.map_or(d, |old| old.min(d)));
        if self.reader_end_at.is_some() && !self.exit_reported {
            at(self.next_exit_poll);
        }
        if ui_ready && unpublished {
            if self.sync_deferring(now) {
                if let Some(since) = self.sync_since {
                    at(since + SYNC_TIMEOUT);
                }
            } else if !visible {
                if let Some(last) = self.last_publish {
                    at(last + HIDDEN_INTERVAL);
                }
            }
        }
        deadline
    }

    pub(crate) fn apply_command(&mut self, command: Command) {
        let mut typed = true;
        match command {
            Command::Key(event) => {
                if let Some(bytes) = self.terminal.key_event(&event) {
                    self.out.push(WriteKind::Input, bytes);
                }
            }
            Command::Text(text) => self.terminal.input(&text, true),
            Command::Paste(text) => {
                if let Some(bytes) = self.terminal.paste(&text) {
                    self.out.push(WriteKind::Paste, bytes);
                }
            }
            Command::Mouse(event) => {
                if let Some(bytes) = self.terminal.mouse_event(&event) {
                    self.out.push(WriteKind::Input, bytes);
                }
            }
            Command::WheelArrows { up } => {
                let seq = match (up, self.terminal.modes().application_cursor_keys) {
                    (true, false) => "\x1b[A",
                    (false, false) => "\x1b[B",
                    (true, true) => "\x1bOA",
                    (false, true) => "\x1bOB",
                };
                self.terminal.input(seq, true);
            }
            Command::Focus(focused) => {
                self.focused = focused;
                if let Some(bytes) = self.terminal.focus_event(focused) {
                    self.out.push(WriteKind::Input, bytes);
                }
            }
            Command::Scroll(scroll) => {
                match scroll {
                    Scroll::Lines(n) => self.terminal.scroll_lines(n),
                    Scroll::Pages(n) => {
                        let page = i64::try_from(self.terminal.rows().saturating_sub(1))
                            .unwrap_or(i64::MAX);
                        self.terminal
                            .scroll_lines(i64::from(n).saturating_mul(page));
                    }
                    Scroll::Top => self.terminal.scroll_to_top(),
                    Scroll::Bottom => self.terminal.scroll_to_bottom(),
                }
                self.acc.full = true;
            }
            Command::Resize(geometry) => {
                let g = geometry.clamped();
                self.geometry = g;
                // Engine first, then the PTY (I11). A failed PTY resize leaves the child at its
                // old size; the next resize retries.
                self.terminal.resize(g.cols, g.rows);
                let _ = self.pty.resize(g.pty_size());
                self.acc.full = true;
                self.idle_pending = true;
            }
            Command::SetClipboardWrite(enabled) => {
                self.terminal.set_clipboard_write(enabled);
                typed = false;
            }
        }
        self.drain(typed);
    }

    /// Parse one slice of the pending output (I4): sub-writes of `step` bytes, the clock read
    /// after each, until `SLICE` has passed or a command is pending. The first write always runs.
    fn run_slice(&mut self, interrupt: &dyn Fn() -> bool) {
        if self.pending.is_empty() {
            return;
        }
        let start = self.clock.now();
        let mut log = SliceLog::default();
        while let Some(front) = self.pending.front() {
            let end = front.len().min(self.offset.saturating_add(self.step));
            let t0 = self.clock.now();
            if let Some(sub) = front.get(self.offset..end) {
                self.terminal.write(sub);
            }
            self.clock.charge(end - self.offset);
            if end >= front.len() {
                self.pending.pop_front();
                self.offset = 0;
            } else {
                self.offset = end;
            }
            self.drain(false);
            self.observe_sync();
            let t1 = self.clock.now();
            let took = t1.saturating_sub(t0);
            if took > SLOW_WRITE {
                self.step = (self.step / 2).max(STEP_MIN);
            } else if took < FAST_WRITE {
                self.step = self.step.saturating_mul(2).min(STEP_MAX);
            }
            log.writes += 1;
            log.last_write = took;
            log.elapsed = t1.saturating_sub(start);
            if log.elapsed >= SLICE || interrupt() {
                break;
            }
        }
        self.slice_log = log;
        self.idle_pending = true;
        self.stats.set_step(self.step);
    }

    /// Drain events and damage (I5). `typed`: responses are typed input (they came from a
    /// user command), not replies to the child.
    fn drain(&mut self, typed: bool) {
        let events = self.terminal.drain_events();
        self.stats.events_per_drain(events.len());
        let reply = if typed {
            WriteKind::Input
        } else {
            WriteKind::Response
        };
        for event in events {
            match event {
                HostEvent::Response(bytes) => self.out.push(reply, bytes),
                HostEvent::Bell => self.acc.bells = self.acc.bells.saturating_add(1),
                HostEvent::TitleChanged => self.acc.title = true,
                HostEvent::Color(requests) => {
                    for request in requests {
                        if let Some(bytes) = self.colors.apply(request, &mut self.acc.palette) {
                            self.out.push(WriteKind::Response, bytes);
                        }
                    }
                    if self.acc.palette {
                        self.acc.full = true;
                    }
                }
                HostEvent::FocusReportRequested => {
                    if let Some(bytes) = self.terminal.focus_event(self.focused) {
                        self.out.push(WriteKind::Response, bytes);
                    }
                }
                HostEvent::WindowReport(kind) => {
                    self.out
                        .push(WriteKind::Response, window_report(kind, &self.geometry));
                }
                HostEvent::ClipboardWrite(text) => self.acc.clipboard = Some(text),
                HostEvent::CwdChanged(cwd) => self.acc.cwd = Some(cwd),
                HostEvent::Progress(p) => self.acc.progress = Some(p),
                HostEvent::CommandMark(mark) => {
                    if self.acc.marks.len() >= MARKS_CAP {
                        self.acc.marks.pop_front();
                    }
                    self.acc.marks.push_back(mark);
                }
                HostEvent::SyncUpdateInCommand => self.acc.sync_in_command = true,
                // Dropped, never logged (I16).
                HostEvent::A11yText(_) | HostEvent::A11yTab(_) | HostEvent::UnknownOsc { .. } => {}
            }
        }
        let damage = self.terminal.take_damage();
        if damage.full || damage.scrolled {
            self.acc.full = true;
        }
        if let Some((a, b)) = damage.rows {
            self.acc.add_rows(a, b);
        }
    }

    /// Follow mode 2026 after each write (D10): the timer starts when the mode is first seen set
    /// and stops when it is seen reset. The engine's mode itself is never touched.
    fn observe_sync(&mut self) {
        if self.terminal.modes().synchronized_output {
            if self.sync_since.is_none() {
                self.sync_since = Some(self.clock.now());
            }
        } else {
            self.sync_since = None;
            self.sync_timed_out = false;
        }
    }

    /// Mode 2026 holds publishing now (and marks the timeout once it passes).
    fn sync_deferring(&mut self, now: Duration) -> bool {
        let Some(since) = self.sync_since else {
            return false;
        };
        if !self.sync_timed_out && now.saturating_sub(since) >= SYNC_TIMEOUT {
            self.sync_timed_out = true;
        }
        !self.sync_timed_out
    }

    fn may_publish(&mut self, now: Duration, visible: bool) -> bool {
        if self.exit_pending.is_some() {
            return true;
        }
        if self.sync_deferring(now) {
            return false;
        }
        if !visible {
            if let Some(last) = self.last_publish {
                if now.saturating_sub(last) < HIDDEN_INTERVAL {
                    return false;
                }
            }
        }
        true
    }

    fn modes(&self) -> UpdateModes {
        let m = self.terminal.modes();
        UpdateModes {
            mouse_tracking: m.mouse_tracking,
            application_cursor_keys: m.application_cursor_keys,
            bracketed_paste: m.bracketed_paste,
            synchronized_output: m.synchronized_output,
        }
    }

    /// Something changed since the last Update. O(1) (`viewport_state`, `modes`).
    pub(crate) fn unpublished(&self) -> bool {
        if self.acc.any() || self.exit_pending.is_some() {
            return true;
        }
        let Some(last) = &self.last else {
            return true;
        };
        let st = self.terminal.viewport_state();
        let l = &last.state;
        st.cols != l.cols
            || st.rows != l.rows
            || st.cursor != l.cursor
            || st.viewport_top != l.viewport_top
            || st.base_y != l.base_y
            || st.total_lines != l.total_lines
            || st.is_at_bottom != l.is_at_bottom
            || st.alt_screen != l.alt_screen
            || self.modes() != last.modes
    }

    fn exit_poll_due(&self, now: Duration) -> bool {
        self.reader_end_at.is_some()
            && !self.exit_reported
            && self.pending.is_empty()
            && now >= self.next_exit_poll
    }

    /// I12: once the output has ended and been parsed, poll the exit code for up to 1 s.
    fn poll_exit(&mut self, now: Duration) {
        if !self.exit_poll_due(now) {
            return;
        }
        let Some(end_at) = self.reader_end_at else {
            return;
        };
        let code = self.pty.try_exit_code().ok().flatten();
        if code.is_some() || now.saturating_sub(end_at) >= EXIT_WAIT {
            self.exit_pending = Some(Exit { code });
            self.exit_reported = true;
        } else {
            self.next_exit_poll = now + EXIT_POLL;
        }
    }

    fn build_update(&mut self, now: Duration) -> Update {
        let st = self.terminal.viewport_state();
        let modes = self.modes();
        // Damage rows refer to the screen: refresh everything after any scroll, size change or
        // buffer switch, and while the user looks at scrollback (sketch trap 6).
        let full = self.acc.full
            || st.viewport_top != st.base_y
            || lines_missing(&st)
            || self.last.as_ref().is_none_or(|l| {
                l.state.viewport_top != st.viewport_top
                    || l.state.cols != st.cols
                    || l.state.rows != st.rows
                    || l.state.alt_screen != st.alt_screen
                    || lines_missing(&l.state)
            });
        let range = if full {
            (st.rows > 0).then(|| (0, st.rows - 1))
        } else {
            self.acc
                .rows
                .filter(|&(a, _)| a < st.rows)
                .map(|(a, b)| (a, b.min(st.rows.saturating_sub(1))))
        };
        let mut rows_changed = Vec::new();
        if let Some((a, b)) = range {
            rows_changed.reserve(b - a + 1);
            for y in a..=b {
                if self.terminal.read_row(y, &mut self.scratch) {
                    rows_changed.push((y, self.scratch.iter().map(to_cell).collect()));
                }
            }
        }
        let acc = std::mem::take(&mut self.acc);
        self.seq += 1;
        let update = Update {
            seq: self.seq,
            cols: st.cols,
            rows: st.rows,
            full,
            rows_changed,
            cursor: st.cursor,
            viewport_top: st.viewport_top,
            base_y: st.base_y,
            total_lines: st.total_lines,
            is_at_bottom: st.is_at_bottom,
            alt_screen: st.alt_screen,
            modes,
            title: (acc.title || self.last.is_none()).then(|| st.title.clone()),
            bells: acc.bells,
            clipboard_write: acc.clipboard,
            palette: acc.palette.then(|| self.colors.current().clone()),
            cwd: acc.cwd,
            progress: acc.progress,
            marks: acc.marks.into(),
            sync_update_in_command: acc.sync_in_command,
            exited: self.exit_pending.take(),
        };
        self.last = Some(Published { state: st, modes });
        self.last_publish = Some(now);
        update
    }
}
