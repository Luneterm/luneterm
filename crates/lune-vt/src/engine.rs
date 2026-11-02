//! Public engine contract (plan M6 D9, roadmap contract C6).
//!
//! Mirrors Lunegit's `IVtEngine` (`Lunegit.Core/Terminal/IVtEngine.cs`) so Lunegit can later
//! swap its V8-hosted xterm.js for this engine through a C ABI crate without changing callers.
//! [`crate::Terminal`] implements [`VtEngine`]. These types are not a port: they are what the
//! host sees in place of xterm.js's event emitters and buffer API objects.
//!
//! The engine is callback-free: events are queued while [`VtEngine::write`] runs and the host
//! drains them ([`VtEngine::drain_events`]) and the coalesced [`Damage`]
//! ([`VtEngine::take_damage`]) after each write. The queue holds one write's output until it is
//! drained, so the host must drain after every write.

use std::collections::BTreeMap;
use std::sync::Arc;

pub use crate::services::options_service::CursorStyle;

/// Something the engine wants the host to act on, produced while processing [`VtEngine::write`].
///
/// Lunegit fires these as synchronous events inside `Write`; here the engine queues them and the
/// host drains them after each write, which keeps the engine free of callbacks into the host.
/// Events are queued in the order upstream fires them; two responses are never merged. The two
/// exceptions keep one write's events proportional to its bytes: [`HostEvent::TitleChanged`] (one
/// per drain, no payload) and [`HostEvent::A11yText`] (merged, capped per drain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Bytes to send back to the application: the UTF-8 of an upstream `onData` string, or one
    /// byte per character of an `onBinary` string (DA/DSR/DECRQM replies, `Terminal::input`).
    Response(Vec<u8>),
    /// BEL (upstream `onRequestBell`).
    Bell,
    /// The window title changed (OSC 0/2, XTWINOPS 23; upstream `onTitleChange`). Carries no
    /// text: the host reads the current title from [`Viewport::title`]. At most one per drain, at
    /// the place of the first change since the last drain, so neither a title storm nor a pop of
    /// a large stored title (up to the parser's 10 MB payload limit) costs more than one event.
    TitleChanged,
    /// Colour requests from OSC 4, 10, 11, 12, 104, 110, 111 and 112 (upstream `onColor`). The
    /// engine never answers a [`ColorRequest::Report`]; the host knows the theme and may.
    Color(Vec<ColorRequest>),
    /// Focus reporting was turned on (`CSI ? 1004 h`, upstream `onRequestSendFocus`): the host
    /// should report the current focus state.
    FocusReportRequested,
    /// A window report the host has to answer (upstream `onRequestWindowsOptionsReport`).
    WindowReport(WindowReportKind),
    /// Printed characters, fired only with `screen_reader_mode` on (upstream `onA11yChar`, one
    /// event per character). Consecutive characters are merged into one event, and at most
    /// [`crate::input_handler::A11Y_TEXT_LIMIT`] bytes are queued per drain; characters past
    /// it are not voiced (upstream's `AccessibilityManager` likewise stops announcing long
    /// output).
    A11yText(String),
    /// HT moved the cursor by this many cells, fired only with `screen_reader_mode` on (upstream
    /// `onA11yTab`).
    A11yTab(usize),
    /// An OSC the engine and its extensions did not handle (ident and payload length only).
    UnknownOsc { ident: i32, len: usize },
    /// OSC 52 clipboard write. Queries are deliberately never answered. The core never produces
    /// it (ledger row 13); the gated write is an extension.
    ClipboardWrite { selection: String, data: String },
}

/// A palette or special colour index (upstream `AllColorIndex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorIndex {
    /// An entry of the 256-colour palette (upstream `ColorIndex`, 0..=255).
    Palette(u8),
    /// The default foreground (upstream `SpecialColorIndex.FOREGROUND`, 256).
    Foreground,
    /// The default background (`SpecialColorIndex.BACKGROUND`, 257).
    Background,
    /// The cursor colour (`SpecialColorIndex.CURSOR`, 258).
    Cursor,
}

/// One request of a [`HostEvent::Color`] (upstream `IColorReportRequest`, `IColorSetRequest` and
/// `IColorRestoreRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRequest {
    /// `?` in the sequence: report the current colour.
    Report(ColorIndex),
    /// Set the colour to this RGB value.
    Set(ColorIndex, [u8; 3]),
    /// Restore the themed colour; `None` restores the whole palette (`OSC 104` without params).
    Restore(Option<ColorIndex>),
}

/// Upstream `WindowsOptionsReportType`: which report `CSI 14 t` / `CSI 16 t` asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowReportKind {
    /// `GET_WIN_SIZE_PIXELS` (`CSI 14 t`).
    WinSizePixels,
    /// `GET_CELL_SIZE_PIXELS` (`CSI 16 t`).
    CellSizePixels,
}

/// What changed on screen since the host last took it, coalesced (upstream's
/// `onRequestRefreshRows`, `onCursorMove` and `onScroll` events, which fire far too often to
/// queue one by one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Damage {
    /// Viewport rows to redraw, first and last inclusive; the union of every refresh request.
    pub rows: Option<(usize, usize)>,
    /// Everything must be redrawn (resize, buffer switch, reset).
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

/// The active mouse tracking protocol (upstream `IModes.mouseTrackingMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseTracking {
    #[default]
    None,
    X10,
    Vt200,
    Drag,
    Any,
}

/// The active mouse report encoding (`CoreMouseService.activeEncoding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    #[default]
    Default,
    Sgr,
    SgrPixels,
}

/// Terminal modes the host needs for input encoding (upstream `IModes`, the headless public
/// `modes` getter, plus `cursor_hidden` and `mouse_encoding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modes {
    pub application_cursor_keys: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub insert: bool,
    pub origin: bool,
    pub reverse_wraparound: bool,
    pub send_focus: bool,
    pub synchronized_output: bool,
    pub wraparound: bool,
    /// DECTCEM off (`CSI ? 25 l`).
    pub cursor_hidden: bool,
    pub mouse_tracking: MouseTracking,
    pub mouse_encoding: MouseEncoding,
}

/// A cell colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellColor {
    /// The theme's default (foreground, background, or for an underline: follow the foreground).
    #[default]
    Default,
    /// A palette index (16- or 256-colour mode).
    Palette(u8),
    /// A true colour.
    Rgb(u8, u8, u8),
}

/// Underline style of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// One cell of the [`Viewport`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewportCell {
    /// The cell's text; `""` for an empty cell and for the second cell of a wide character.
    pub text: String,
    /// 1 for a normal cell, 2 for the first cell of a wide character, 0 for its second cell.
    pub width: u8,
    pub fg: CellColor,
    pub bg: CellColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    /// The underline the application set (SGR 4 and its colon forms); a hyperlink does not
    /// change it, [`ViewportCell::link_id`] carries the link.
    pub underline: UnderlineStyle,
    /// [`CellColor::Default`] = follow the foreground.
    pub underline_color: CellColor,
    /// OSC 8 link id, 0 for none; the uri is in [`Viewport::links`].
    pub link_id: u32,
}

/// The cursor as the viewport shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewportCursor {
    /// Column; equals `cols` while a wrap is pending (upstream `buffer.x`).
    pub x: usize,
    /// Viewport row (0 when the cursor is not inside the viewport).
    pub y: usize,
    /// Not hidden by DECTCEM and inside the viewport.
    pub visible: bool,
    pub style: CursorStyle,
    pub blink: bool,
}

/// A snapshot of what the screen shows (Lunegit `VtViewport`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Viewport {
    pub cols: usize,
    pub rows: usize,
    /// `rows` rows of `cols` cells.
    pub cells: Vec<Vec<ViewportCell>>,
    pub cursor: ViewportCursor,
    /// First buffer line shown (upstream `ydisp`).
    pub viewport_top: usize,
    /// First line of the bottom page (upstream `ybase`).
    pub base_y: usize,
    /// Lines in the active buffer, scrollback included.
    pub total_lines: usize,
    /// The viewport shows the bottom page.
    pub is_at_bottom: bool,
    /// The window title, shared with the engine (a snapshot does not copy it; it can be up to
    /// the parser's 10 MB payload limit).
    pub title: Arc<str>,
    pub alt_screen: bool,
    /// The uri of every link id used by a cell in `cells`.
    pub links: BTreeMap<u32, String>,
}

/// A VT engine instance: one per terminal session. Not thread-safe by design; the session owns it.
pub trait VtEngine {
    fn columns(&self) -> u16;
    fn rows(&self) -> u16;
    /// Feed raw bytes from the PTY. Partial UTF-8 and partial escape sequences may be split
    /// across calls.
    fn write(&mut self, data: &[u8]);
    /// Resize; clamped to at least 2 columns and 1 row.
    fn resize(&mut self, columns: u16, rows: u16);
    /// Scroll the viewport by `delta` lines (negative = up).
    fn scroll_lines(&mut self, delta: i32);
    fn scroll_to_top(&mut self);
    fn scroll_to_bottom(&mut self);
    /// Take the events produced since the last call.
    fn drain_events(&mut self) -> Vec<HostEvent>;
    /// Take the damage accumulated since the last call.
    fn take_damage(&mut self) -> Damage;
    /// A snapshot of the screen.
    fn viewport(&self) -> Viewport;
    fn modes(&self) -> Modes;
}
