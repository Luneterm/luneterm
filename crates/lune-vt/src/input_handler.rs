//! Port of xterm.js src/common/InputHandler.ts @ f447274.
//!
//! The terminal's standard handler set: everything the escape sequence parser dispatches lands
//! here and changes the buffers and services. Refer to
//! <http://invisible-island.net/xterm/ctlseqs/ctlseqs.html> for each handler's sequence.
//!
//! Ported so far (plan M6 step 2): the struct, `parse`, `print`, the register forwards (with the
//! `t` window-option guard), BEL LF VT FF CR BS HT SO SI IND NEL HTS, CSI A B C D E F G H I Z
//! `` ` `` a d e f g r s u, ESC 7 8 D E H M, the five fallbacks and the error handler, and
//! `DirtyRowTracker`, `reset` (step 3); step 5: CSI @ SP@ SPA J ?J K ?K L M P S T X b `"q`
//! `'}` `'~` and ESC # 8 (erase, insert/delete, scrolling, columns, ECH, REP, DECSCA, DECALN);
//! step 6: CSI m (SGR with colon notation, 256/RGB colours, underline styles and colours);
//! step 7: CSI c > c h ? h l ? l n ? n ! p SP q t $ p ? $ p (DA1, DA2, SM, DECSET, RM, DECRST,
//! DSR, DECDSR, DECSTR, DECSCUSR, XTWINOPS, DECRQM), ESC = > c n o | } ~ % @ % G and the charset
//! designators (keypad, RIS, locking shifts, charsets), and `set_title`/`set_icon_name` (the
//! stacks of XTWINOPS 22/23); step 8: OSC 0 1 2 4 8 10 11 12 104 110 111 112 (titles, colour
//! requests, hyperlinks) and DCS `$ q` (DECRQSS). That is every handler upstream registers.
//!
//! Rust shape (plan M6 D1, D2, D3, D8):
//!  - The parser is generic over a `'static` host, so [`InputHandler`] owns the services
//!    ([`CoreServices`]) and is the [`ParserHost`]. The parser lives beside it (in
//!    `CoreTerminal`); [`InputHandler::parse`] takes it as an argument and lends `self` to it.
//!    Handlers are closures registered by [`InputHandler::register_handlers`].
//!  - Upstream's emitters become a queue of [`HostEvent`]s (in fire order) plus a coalesced
//!    [`Damage`]: `onRequestBell` -> `Bell`, `onA11yChar`/`onA11yTab` -> `A11yText`/`A11yTab`,
//!    `onRequestRefreshRows` -> `Damage::rows`, `onCursorMove` -> `Damage::cursor_moved`,
//!    `bufferService.onScroll` (forwarded by `CoreTerminal`) -> [`InputHandler::on_scroll`].
//!    `onLineFeed` has no public consumer: it drives only the Windows wrapping heuristic, which
//!    runs inside `line_feed` when `windows_heuristics` is set (D6, set by `CoreTerminal`).
//!    `onRequestSyncScrollBar` (keypad mode switches, alt buffer switches, DECSTR) has no consumer
//!    and is dropped. `onRequestSendFocus` (DECSET 1004) is `HostEvent::FocusReportRequested` and
//!    `onRequestWindowsOptionsReport` (XTWINOPS 14, 16) is `HostEvent::WindowReport`;
//!    `onRequestReset` (DECCOLM, RIS) runs the headless `Terminal.reset` in place
//!    ([`reset_terminal`]); a buffer activation (DECSET 47, 1047, 1049) is [`InputHandler::on_scroll`]
//!    plus full damage, and `onRequestRefreshRows(undefined)` is damage over every viewport row.
//!  - RIS (D7): upstream's `fullReset` calls `this._parser.reset()` before the terminal reset. The
//!    parser is mid-dispatch of the ESC here and cannot be reached (it is borrowed by `parse`),
//!    and the call changes nothing: the transition after an ESC dispatch is to GROUND, collect and
//!    params are cleared when the next sequence starts, the OSC and DCS parsers are idle, and the
//!    join state is cleared by the parser itself before it dispatches the ESC. Tests pin it
//!    against the oracle.
//!  - Colours: upstream's `onColor` event is [`HostEvent::Color`] (a list of [`ColorRequest`]s).
//!    The engine never answers a report: the host knows the theme. OSC 52 and every other OSC
//!    without a handler reach the fallback, which only logs (ledger row 13). `onTitleChange` is
//!    [`HostEvent::TitleChanged`]; the icon name (OSC 1) is stored and fires nothing, as headless.
//!  - Event coalescing (D2 amendment, Q10/Q11; [`EventQueue`]): one payload-free `TitleChanged`
//!    per drain (the host reads the title), and `onA11yChar` characters merged into one `A11yText` capped at
//!    [`A11Y_TEXT_LIMIT`] bytes per drain. Upstream hands listeners shared strings, one event per
//!    fire; queued copies would let a few bytes of XTWINOPS 22/23 or REP queue gigabytes.
//!  - JS string functions: `String.trim` is `js_trim` (ECMAScript WhiteSpace, not Rust's), and
//!    the `/^\d+$/` + `parseInt` of the colour indexes is `parse_js_index`. The decoder drops
//!    U+FEFF before the parser sees it (upstream's `Utf8ToUtf32`), so that trim case is only
//!    reachable through a direct call.
//!  - Marker disposal (M5 host duty): upstream frees an OSC 8 link the moment the last marker
//!    of its lines is disposed. Here the buffers log disposals, and
//!    [`InputHandler::forward_disposed_markers`] hands them to the `OscLinkService`. It runs
//!    before every `OscLinkService` call (so a link trimmed earlier in the same write is gone
//!    when the next call looks for it) and at the end of every public mutation (`CoreTerminal`).
//!  - No async handlers (M4 D3): the parse stack, `_preserveStack` and `_logSlowResolvingAsync`
//!    are not ported. String input (`StringToUtf32`) is not ported: `parse` takes UTF-8 bytes.
//!  - D8, JS throws and `undefined`:
//!    * Every `lines.get(..)!` goes through [`line_mut`]. Upstream throws on `undefined` and the
//!      throw abandons the rest of the `parse` call; here the handler stops at that point with
//!      its upstream return value and parsing continues with the next sequence. A missing line
//!      at the cursor is reachable only through M5's scrollback-0 soft invariant (`ybase + rows`
//!      past the list after a reflow without scrollback); `print` then writes nothing (test
//!      `missing_cursor_line_stops_the_handler_without_panic`).
//!    * `params.params[i]` is [`Params::get`] (stale values past `length()` kept, `None` past
//!      the store = JS `undefined`), see [`param_or`].
//!    * Cursor and param arithmetic is done in `i64` and clamped as upstream.
//!  - `print` keeps upstream's `oldRow` object across `bufferService.scroll`: the row index the
//!    old row has after the scroll is derived from the scroll branch ([`old_row_after_scroll`]),
//!    including the case where the recycled line is the new row (one row, no scrollback) and the
//!    case where the old row leaves the list (upstream then writes to a detached object: no
//!    visible effect).
//!  - Differences (step 5): upstream loops `Ps` times in IL, DL, SU and SD (up to 2^31 - 1
//!    splices); the loops stop after as many iterations as the region has rows, when every line
//!    of it is blank and further iterations change nothing (state and markers are the same).
//!    The splices of one sequence reach the markers as one composed event
//!    (`CircularList::splice_rotate`, review S6): one walk of the marker list per sequence
//!    instead of two per line, same end state and disposal order.
//!    REP allocates `text length * Ps` code points upstream; here the repeated data is capped at
//!    [`MAX_REP_CODE_POINTS`], so a larger count prints fewer repetitions. Of those, REP
//!    prints only as many as still change the state (review S3), a cut proven to leave the
//!    same state as the full print where it applies (`rep_equivalent_repeats`). A unit that
//!    prints alike every time (a character with its joiners) is written line by line
//!    (`print_repeated`, reviews S3 residual and S7): the same cells, side-table entries,
//!    insert-mode shifts, link lines and screen reader text as `print`, batched per line.
//!    ED 1 clears the wrapped flag of line `y + 1` counted from the start of the list, not from
//!    `ybase` (upstream's index, kept). IL, DL, SU, SD, ICH, DCH, ECH and the column handlers
//!    pass param-derived counts to the buffer line / list calls, which tolerate any count.
//!  - Differences: CHT/CBT stop looping once the cursor no longer moves (upstream loops `Ps`
//!    times, up to 2^31, with the same result). In `print`'s insert-mode path a negative insert
//!    count (a joining character at column 0, reachable only with a join state that survived a
//!    move to column 0 outside the parser) inserts nothing, where upstream's `insertCells` would
//!    shift cells left. Not a difference: `restoreCursor` sets the charset to `None` before
//!    restoring the saved one, as upstream's `(this as any)._savedCharset` (always `undefined`)
//!    does.

// No-panic gate (plan M6 D8): ported non-test code must not index, slice a string, unwrap or
// panic. Tests are exempt.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::string_slice,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

use std::fmt;
use std::mem;
use std::sync::Arc;

use crate::buffer::attribute_data::AttributeData;
use crate::buffer::buffer::Buffer;
use crate::buffer::buffer_line::{
    BufferLine, CellsSource, DEFAULT_ATTR_DATA, JoinedHead, MAX_COMBINED_CODE_POINTS, RangeState,
    WrapUnit,
};
use crate::buffer::buffer_set::BufferActivate;
use crate::buffer::cell_data::CellData;
use crate::buffer::constants::{
    Attributes, BgFlags, Content, FgFlags, NULL_CELL_CODE, NULL_CELL_WIDTH, UnderlineStyle,
};
use crate::data::charsets::{Charset, charset};
use crate::data::escape_sequences::{C0, C1};
use crate::engine::{ColorIndex, ColorRequest, Damage, HostEvent, WindowReportKind};
use crate::ext::ExtState;
use crate::headless::terminal::reset_terminal;
use crate::input::text_decoder::{Utf8ToUtf32, string_from_code_point};
use crate::input::x_parse_color::parse_color;
use crate::parser::dcs_parser::DcsStringHandler;
use crate::parser::escape_sequence_parser::{EscapeSequenceParser, ident_to_string};
use crate::parser::osc_parser::OscStringHandler;
use crate::parser::params::Params;
use crate::parser::types::{
    DcsFallbackAction, FunctionIdentifier, HandlerId, IdentifierError, OscFallbackAction,
    ParserHost,
};
use crate::services::buffer_service::BufferService;
use crate::services::charset_service::CharsetService;
use crate::services::core_mouse_service::CoreMouseService;
use crate::services::core_service::CoreService;
use crate::services::log_service::LogService;
use crate::services::options_service::{CursorStyle, OptionValue, OptionsService, WindowOptions};
use crate::services::osc_link_service::{OscLinkData, OscLinkService};
use crate::services::unicode_service::UnicodeService;
use crate::windows_mode::update_windows_mode_wrapped_state;

/// Max length of the UTF-32 input buffer; longer writes are parsed in chunks of this many bytes.
pub const MAX_PARSEBUFFER_LENGTH: usize = 131_072;

/// Limit length of the title and icon name stacks (XTWINOPS 22).
const STACK_LIMIT: usize = 10;

/// The designators `CHARSETS` has an entry for (`B`, US, is an entry holding `undefined`).
const CHARSET_FLAGS: [char; 16] = [
    '0', 'A', 'B', '4', 'C', '5', 'R', 'Q', 'K', 'Y', 'E', '6', 'Z', 'H', '7', '=',
];

/// The intermediates that designate a character set (`ESC ( C` and the others).
const CHARSET_INTERMEDIATES: [char; 7] = ['(', ')', '*', '+', '-', '.', '/'];

/// Most code points one REP (CSI Ps b) prints; upstream allocates `Ps` times the text.
pub const MAX_REP_CODE_POINTS: usize = 1 << 20;

/// Most bytes of [`HostEvent::A11yText`] text queued per drain (plan M6 D2 amendment, Q10).
/// Characters printed past it are not voiced, as upstream's `AccessibilityManager` stops
/// announcing after `MAX_ROWS_TO_READ` (20) rows. Half the engine fuzz's flat event allowance
/// (4 KiB), so a write that hits it (REP with screen reader mode on) stays within the bound
/// `events <= 16 x bytes written + 4 KiB`.
pub const A11Y_TEXT_LIMIT: usize = 2048;

/// Most [`HostEvent::UnknownOsc`] events queued per drain (Lunegit's per-write cap on its
/// unknown-OSC channel, `driver.js`): a stream cycling identifiers cannot grow the queue.
pub const UNKNOWN_OSC_LIMIT: usize = 64;

/// The services every handler reads or writes (upstream's constructor arguments).
#[derive(Debug)]
pub struct CoreServices {
    pub options: OptionsService,
    pub buffer: BufferService,
    pub log: LogService,
    pub core: CoreService,
    pub mouse: CoreMouseService,
    pub unicode: UnicodeService,
    pub charset: CharsetService,
    pub osc_link: OscLinkService,
}

impl CoreServices {
    /// The services over `options`, built as `CoreTerminal`'s constructor does
    /// (`CoreTerminal.ts:106-121`): log level and buffer size from the options, V6 Unicode
    /// active.
    pub fn new(options: OptionsService) -> Self {
        let opts = options.options();
        let log = LogService::new(opts.log_level);
        let buffer = BufferService::new(opts);
        CoreServices {
            log,
            buffer,
            core: CoreService::new(),
            mouse: CoreMouseService::new(),
            unicode: UnicodeService::new(),
            charset: CharsetService::new(),
            osc_link: OscLinkService::new(),
            options,
        }
    }
}

/// Rows changed by the current `parse` call, relative to `ybase` (upstream `DirtyRowTracker`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirtyRowTracker {
    pub start: i64,
    pub end: i64,
}

impl DirtyRowTracker {
    /// A tracker cleared to the cursor row `y`.
    pub fn new(y: usize) -> Self {
        let mut tracker = DirtyRowTracker::default();
        tracker.clear_range(y);
        tracker
    }

    /// Start a new range at the cursor row `y` (the cursor row is always refreshed).
    pub fn clear_range(&mut self, y: usize) {
        self.start = to_i64(y);
        self.end = self.start;
    }

    pub fn mark_dirty(&mut self, y: usize) {
        let y = to_i64(y);
        if y < self.start {
            self.start = y;
        } else if y > self.end {
            self.end = y;
        }
    }

    pub fn mark_range_dirty(&mut self, y1: i64, y2: i64) {
        let (y1, y2) = if y1 > y2 { (y2, y1) } else { (y1, y2) };
        if y1 < self.start {
            self.start = y1;
        }
        if y2 > self.end {
            self.end = y2;
        }
    }

    /// Marks rows `0 ..= rows - 1`.
    pub fn mark_all_dirty(&mut self, rows: usize) {
        self.mark_range_dirty(0, to_i64(rows) - 1);
    }
}

/// The host event queue (plan M6 D2, amended by Q10). Events keep upstream's fire order, with
/// two coalescing rules that keep one write's events proportional to its bytes:
///  - at most one [`HostEvent::TitleChanged`] per drain, where the first title change since the
///    last drain fired; it carries no text (the host reads the current title), so a stored title
///    popped by XTWINOPS 23 is never copied into the queue;
///  - consecutive printed characters merge into one [`HostEvent::A11yText`], and at most
///    [`A11Y_TEXT_LIMIT`] bytes of such text are queued per drain; the first character that does
///    not fit ends voicing until the next drain.
///
///
/// LUNETERM: (plan M7 D8) three more events keep only the last (the host acts on the latest
/// value), replaced in place at the position of the first one queued since the last drain, so a
/// stream of them costs one event and one payload ([`HostEvent::ClipboardWrite`],
/// [`HostEvent::CwdChanged`], [`HostEvent::Progress`]); [`HostEvent::UnknownOsc`] stops after
/// [`UNKNOWN_OSC_LIMIT`] per drain.
///
/// Every other event is queued as fired.
#[derive(Debug, Default)]
struct EventQueue {
    events: Vec<HostEvent>,
    /// Whether a [`HostEvent::TitleChanged`] is queued.
    title_queued: bool,
    /// Bytes of [`HostEvent::A11yText`] text queued since the last drain.
    a11y_bytes: usize,
    /// Index in `events` of the queued [`HostEvent::ClipboardWrite`], `CwdChanged` and
    /// `Progress`.
    last_wins: [Option<usize>; 3],
    /// [`HostEvent::UnknownOsc`] events queued since the last drain.
    unknown_osc: usize,
}

impl EventQueue {
    fn push(&mut self, event: HostEvent) {
        match event {
            HostEvent::TitleChanged => self.title_changed(),
            HostEvent::A11yText(text) => self.push_a11y_text(&text),
            event @ HostEvent::ClipboardWrite(_) => self.push_last_wins(0, event),
            event @ HostEvent::CwdChanged(_) => self.push_last_wins(1, event),
            event @ HostEvent::Progress(_) => self.push_last_wins(2, event),
            HostEvent::UnknownOsc { .. } if self.unknown_osc >= UNKNOWN_OSC_LIMIT => {}
            event @ HostEvent::UnknownOsc { .. } => {
                self.unknown_osc += 1;
                self.events.push(event);
            }
            event => self.events.push(event),
        }
    }

    /// Queue `event`, or replace the one of its kind (slot `slot`) already queued.
    fn push_last_wins(&mut self, slot: usize, event: HostEvent) {
        let queued = self
            .last_wins
            .get(slot)
            .copied()
            .flatten()
            .and_then(|index| self.events.get_mut(index));
        if let Some(queued) = queued {
            *queued = event;
        } else {
            if let Some(entry) = self.last_wins.get_mut(slot) {
                *entry = Some(self.events.len());
            }
            self.events.push(event);
        }
    }

    /// The window title changed.
    fn title_changed(&mut self) {
        if !self.title_queued {
            self.title_queued = true;
            self.events.push(HostEvent::TitleChanged);
        }
    }

    /// A printed character, with screen reader mode on (upstream `onA11yChar`).
    fn a11y_char(&mut self, code: u32) {
        if self.a11y_bytes < A11Y_TEXT_LIMIT {
            self.push_a11y_text(&string_from_code_point(code));
        }
    }

    /// No more [`HostEvent::A11yText`] text is taken until the next drain: every later
    /// [`EventQueue::a11y_char`] is a no-op.
    fn a11y_full(&self) -> bool {
        self.a11y_bytes >= A11Y_TEXT_LIMIT
    }

    fn push_a11y_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let total = self.a11y_bytes.saturating_add(text.len());
        if total > A11Y_TEXT_LIMIT {
            // stop here rather than voice a later, shorter character out of order
            self.a11y_bytes = A11Y_TEXT_LIMIT;
            return;
        }
        self.a11y_bytes = total;
        if let Some(HostEvent::A11yText(queued)) = self.events.last_mut() {
            queued.push_str(text);
        } else {
            self.events.push(HostEvent::A11yText(text.to_owned()));
        }
    }

    /// The queued events; starts a new drain.
    fn take(&mut self) -> Vec<HostEvent> {
        self.title_queued = false;
        self.a11y_bytes = 0;
        self.last_wins = [None; 3];
        self.unknown_osc = 0;
        mem::take(&mut self.events)
    }
}

/// The terminal's standard implementation of the parser's handlers (upstream `InputHandler`).
#[derive(Debug)]
pub struct InputHandler {
    pub(crate) services: CoreServices,
    parse_buffer: Vec<u32>,
    utf8_decoder: Utf8ToUtf32,
    cur_attr_data: AttributeData,
    erase_attr_data_internal: AttributeData,
    dirty_row_tracker: DirtyRowTracker,
    /// The parser's `precedingJoinState` ([`ParserHost`]).
    preceding_join_state: u32,
    events: EventQueue,
    damage: Damage,
    /// The window title (upstream `_windowTitle`). Titles are shared (`Arc`) so XTWINOPS 22/23
    /// move them between the stack and here without copying (a title can be 10 MB).
    window_title: Arc<str>,
    /// The icon name (upstream `_iconName`, OSC 1).
    icon_name: Arc<str>,
    /// Titles saved by XTWINOPS 22, newest last (upstream `_windowTitleStack`).
    window_title_stack: Vec<Arc<str>>,
    /// Icon names saved by XTWINOPS 22 (upstream `_iconNameStack`).
    icon_name_stack: Vec<Arc<str>>,
    /// Upstream's `onLineFeed` listener for the Windows wrapping heuristics (`CoreTerminal`'s
    /// `_handleWindowsPtyOptionChange`, plan M6 D6). Off until the host turns it on.
    pub(crate) windows_heuristics: bool,
    /// LUNETERM: (plan M7 D1) the standard extensions' state. It lives here so the parser
    /// closures (which get `&mut InputHandler`) and the encoders (which get the terminal) see
    /// the same state, with no reference back to the host.
    pub(crate) ext: ExtState,
    /// REPs printed by the line-fill path (tests check which path a case takes).
    #[cfg(test)]
    pub(crate) rep_fills: u64,
    /// REPs whose stuck inserts were batched (`print_stuck_inserts`; tests check the path).
    #[cfg(test)]
    pub(crate) rep_stuck_batches: u64,
    /// Line-fill passes skipped as repeats of a pass that changed nothing it reads (M7 fix
    /// round 3, `print_repeated`; tests check the path).
    #[cfg(test)]
    pub(crate) rep_cycle_batches: u64,
    /// Whole passes over the last row below the scroll region whose string copies were taken
    /// at once (M7 fix round 5, `print_repeated`; tests check the path).
    #[cfg(test)]
    pub(crate) rep_copy_batches: u64,
    /// Repetitions a widened unit's joiner wrapped on the last row below the scroll region,
    /// through `print`'s own steps (M7 fix round 5; tests bound them per REP).
    #[cfg(test)]
    pub(crate) rep_joiner_wraps: u64,
    /// REPs that skipped the strings' copy of their first joiner wraps at the scroll bottom
    /// (M7 fix round 5, `chain_copies_to_skip`; tests check the bound).
    #[cfg(test)]
    pub(crate) rep_chain_skips: u64,
    /// Insert-mode whole passes over the last row below the scroll region replayed densely
    /// (M7 fix round 7, `replay_wrap_passes`; tests check the path): the replays, and the
    /// passes they computed before the row stopped changing.
    #[cfg(test)]
    pub(crate) rep_wrap_replays: u64,
    #[cfg(test)]
    pub(crate) rep_wrap_replay_passes: u64,
}

impl ParserHost for InputHandler {
    fn preceding_join_state(&mut self) -> &mut u32 {
        &mut self.preceding_join_state
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// `n` clamped to `0 ..= usize::MAX`.
fn to_usize(n: i64) -> usize {
    usize::try_from(n.max(0)).unwrap_or(usize::MAX)
}

/// `params.params[i] || default`: a raw param that is `0` or `undefined` gives `default`.
fn param_or(params: &Params, i: usize, default: i64) -> i64 {
    match params.get(i) {
        Some(value) if value != 0 => i64::from(value),
        _ => default,
    }
}

/// `print`'s charset replacement: a code point below 127 maps through the active charset, to
/// the first UTF-16 unit of its replacement (upstream `ch.charCodeAt(0)`).
fn charset_replacement(charset: Option<&'static Charset>, code: u32) -> u32 {
    if code < 127 {
        if let Some(set) = charset {
            if let Some(unit) = char::from_u32(code)
                .and_then(|ch| set.get(ch))
                .and_then(|ch| ch.encode_utf16().next())
            {
                return u32::from(unit);
            }
        }
    }
    code
}

/// The absolute row of the cursor (`ybase + y`).
fn abs_y(buffer: &Buffer) -> usize {
    buffer.ybase.saturating_add(buffer.y)
}

/// Upstream `lines.get(row)!` (plan M6 D8): `None` where upstream would throw on `undefined`.
/// Every such lookup in this file goes through here.
fn line_mut(buffer: &mut Buffer, row: usize) -> Option<&mut BufferLine> {
    buffer.lines.get_mut(row)
}

/// [`InputHandler::forward_disposed_markers`] on the services alone.
fn forward_disposed(services: &mut CoreServices) {
    let buffers = &mut services.buffer.buffers;
    let normal = buffers.normal_mut().take_disposed_markers();
    let alt = buffers.alt_mut().take_disposed_markers();
    if !normal.is_empty() {
        services.osc_link.on_markers_disposed(&normal);
    }
    if !alt.is_empty() {
        services.osc_link.on_markers_disposed(&alt);
    }
}

/// `print`'s link step (`InputHandler.ts:553-555`): the cursor's line joins the open link,
/// after the pending marker disposals are forwarded.
fn add_cursor_line_to_link(services: &mut CoreServices, link_id: u32) {
    forward_disposed(services);
    let y = abs_y(services.buffer.buffer());
    services
        .osc_link
        .add_line_to_link(&mut services.buffer, link_id, y);
}

/// LUNETERM: (review S3, S7) how a REP unit prints when every repetition prints alike
/// ([`InputHandler::rep_fill`]).
struct RepFill {
    /// The unit's code points with the charset applied: the base, then its joiners.
    codes: Vec<u32>,
    /// The unit's width in cells (1 or 2): the base's, or 2 where a joiner widens a narrow base
    /// (M7 fix round 1).
    width: u32,
    /// The base's width in cells: the cells each repetition inserts in insert mode.
    base_width: u32,
    /// Each code point's width as `print` writes it (the base's, then each joiner's joined
    /// width).
    widths: Vec<u32>,
    /// The index of the joiner that widens the base to 2 cells, if one does.
    widens_at: Option<usize>,
    /// The join state each repetition leaves.
    end_state: u32,
}

/// JS `String.prototype.trim`: strips the ECMAScript WhiteSpace and LineTerminator characters
/// (not Rust's `White_Space`: U+FEFF counts, U+0085 does not).
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(
            c,
            '\u{9}'..='\u{d}'
                | ' '
                | '\u{a0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
        )
    })
}

/// `/^\d+$/.exec(s)` then `parseInt(s)`, saturating at 1000 (anything past 255 is an invalid
/// colour index anyway, and a longer digit string would overflow).
fn parse_js_index(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value: u32 = 0;
    for b in s.bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(b.saturating_sub(b'0')))
            .min(1000);
    }
    Some(value)
}

/// `isValidColorIndex(value)` as a palette index.
fn valid_color_index(value: u32) -> Option<ColorIndex> {
    u8::try_from(value).ok().map(ColorIndex::Palette)
}

/// back_color_erase feature for xterm (upstream `_eraseAttrData`).
fn erase_attr(internal: &mut AttributeData, cur: &AttributeData) -> AttributeData {
    internal.bg &= !(Attributes::CM_MASK | 0xFF_FFFF);
    internal.bg |= cur.bg & !0xFC00_0000;
    *internal
}

/// `bufferService.onScroll` as `CoreTerminal` forwards it: scroll damage and the scroll region
/// marked dirty.
fn scrolled(damage: &mut Damage, tracker: &mut DirtyRowTracker, buffer: &Buffer) {
    damage.scrolled = true;
    tracker.mark_range_dirty(to_i64(buffer.scroll_top), to_i64(buffer.scroll_bottom));
}

/// Where the line at `old_row` will be after `BufferService::scroll` on `buffer`
/// (`services/buffer_service.rs` `scroll`), `None` if it leaves the list. Mirrors upstream's
/// object identity for `print`'s `oldRow`.
fn old_row_after_scroll(buffer: &Buffer, old_row: usize) -> Option<usize> {
    let len = buffer.lines.len();
    let full = buffer.lines.is_full();
    let top_row = buffer.ybase.saturating_add(buffer.scroll_top);
    let bottom_row = buffer.ybase.saturating_add(buffer.scroll_bottom);
    if buffer.scroll_top == 0 {
        if bottom_row.saturating_add(1) == len {
            if full {
                // recycle: the oldest line becomes the new last line, the rest move up one
                Some(old_row.checked_sub(1).unwrap_or(len.saturating_sub(1)))
            } else {
                // push
                Some(old_row)
            }
        } else {
            // splice after bottom_row, then trim one line from the start if the list was full
            let shifted = if old_row > bottom_row {
                old_row.saturating_add(1)
            } else {
                old_row
            };
            if full {
                shifted.checked_sub(1)
            } else {
                Some(shifted)
            }
        }
    } else if old_row > top_row && old_row <= bottom_row {
        // the region moves up one row
        Some(old_row - 1)
    } else if old_row == top_row {
        // overwritten by the shift (or replaced by the new line when the region is one row)
        None
    } else {
        Some(old_row)
    }
}

/// `paramToWindowOption`: whether the window option of CSI `n` t is enabled.
pub fn param_to_window_option(n: Option<i32>, opts: &WindowOptions) -> bool {
    let Some(n) = n else {
        return false;
    };
    if n > 24 {
        return opts.set_win_lines;
    }
    match n {
        1 => opts.restore_win,
        2 => opts.minimize_win,
        3 => opts.set_win_position,
        4 => opts.set_win_size_pixels,
        5 => opts.raise_win,
        6 => opts.lower_win,
        7 => opts.refresh_win,
        8 => opts.set_win_size_chars,
        9 => opts.maximize_win,
        10 => opts.fullscreen_win,
        11 => opts.get_win_state,
        13 => opts.get_win_position,
        14 => opts.get_win_size_pixels,
        15 => opts.get_screen_size_pixels,
        16 => opts.get_cell_size_pixels,
        18 => opts.get_win_size_chars,
        19 => opts.get_screen_size_chars,
        20 => opts.get_icon_title,
        21 => opts.get_win_title,
        22 => opts.push_title,
        23 => opts.pop_title,
        24 => opts.set_win_lines,
        _ => false,
    }
}

// Lazy log arguments: formatted only when the log level lets the message through.

/// `identToString`.
struct Ident(u32);

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&ident_to_string(self.0))
    }
}

/// `params.toArray()`.
struct ParamsArray<'a>(&'a Params);

impl fmt::Display for ParamsArray<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0.to_array())
    }
}

impl InputHandler {
    /// A handler over `services`. Register it with a parser through
    /// [`InputHandler::register_handlers`].
    pub fn new(services: CoreServices) -> Self {
        let y = services.buffer.buffer().y;
        InputHandler {
            services,
            parse_buffer: Vec::with_capacity(4096),
            utf8_decoder: Utf8ToUtf32::new(),
            cur_attr_data: DEFAULT_ATTR_DATA,
            erase_attr_data_internal: DEFAULT_ATTR_DATA,
            dirty_row_tracker: DirtyRowTracker::new(y),
            preceding_join_state: 0,
            events: EventQueue::default(),
            damage: Damage::default(),
            window_title: Arc::from(""),
            icon_name: Arc::from(""),
            window_title_stack: Vec::new(),
            icon_name_stack: Vec::new(),
            windows_heuristics: false,
            ext: ExtState::default(),
            #[cfg(test)]
            rep_fills: 0,
            #[cfg(test)]
            rep_stuck_batches: 0,
            #[cfg(test)]
            rep_cycle_batches: 0,
            #[cfg(test)]
            rep_copy_batches: 0,
            #[cfg(test)]
            rep_joiner_wraps: 0,
            #[cfg(test)]
            rep_chain_skips: 0,
            #[cfg(test)]
            rep_wrap_replays: 0,
            #[cfg(test)]
            rep_wrap_replay_passes: 0,
        }
    }

    /// The services.
    pub fn services(&self) -> &CoreServices {
        &self.services
    }

    /// The current text attributes (upstream `getAttrData`).
    pub fn get_attr_data(&self) -> &AttributeData {
        &self.cur_attr_data
    }

    /// The events fired since the last call, in fire order, coalesced as [`EventQueue`]
    /// describes: one [`HostEvent::TitleChanged`] (read [`InputHandler::window_title`]), printed
    /// characters merged into [`HostEvent::A11yText`] up to [`A11Y_TEXT_LIMIT`] bytes.
    pub fn take_events(&mut self) -> Vec<HostEvent> {
        self.events.take()
    }

    /// The damage accumulated since the last call.
    pub fn take_damage(&mut self) -> Damage {
        mem::take(&mut self.damage)
    }

    /// The lengths of the window title and icon name stacks (XTWINOPS 22/23); each is capped at
    /// ten entries (read by the engine fuzz).
    pub fn title_stack_lens(&self) -> (usize, usize) {
        (self.window_title_stack.len(), self.icon_name_stack.len())
    }

    /// The window title (OSC 0/2).
    pub fn window_title(&self) -> &str {
        &self.window_title
    }

    /// The window title as the shared string the handler holds (no copy).
    pub fn window_title_shared(&self) -> Arc<str> {
        Arc::clone(&self.window_title)
    }

    /// Queue an event for the host (an upstream `fire` whose listener is the host).
    pub(crate) fn push_event(&mut self, event: HostEvent) {
        self.events.push(event);
    }

    /// The damage accumulated so far, for the host duties of `CoreTerminal` and `Terminal`.
    pub(crate) fn damage_mut(&mut self) -> &mut Damage {
        &mut self.damage
    }

    /// Hand the markers both buffers disposed since the last call to the `OscLinkService`
    /// (upstream `marker.onDispose(() => _removeMarkerFromLink)`). Call before every
    /// `OscLinkService` call and at the end of every public mutation.
    pub fn forward_disposed_markers(&mut self) {
        forward_disposed(&mut self.services);
    }

    /// Reset the text attributes (upstream `reset`, called by `CoreTerminal.reset`).
    pub fn reset(&mut self) {
        self.cur_attr_data = DEFAULT_ATTR_DATA;
        self.erase_attr_data_internal = DEFAULT_ATTR_DATA;
    }

    /// The OSC fallback (upstream's `setOscHandlerFallback` callback): logs the unhandled
    /// sequence. The standard extensions' unknown-OSC reporter calls it too.
    pub(crate) fn log_unknown_osc(
        h: &mut InputHandler,
        identifier: i32,
        action: OscFallbackAction<'_>,
    ) {
        match action {
            OscFallbackAction::Start => h.services.log.debug(format_args!(
                "Unknown OSC code: {{ identifier: {identifier}, action: START }}"
            )),
            // LUNETERM: the length, not the data (upstream logs it): an unhandled OSC can carry
            // secrets, e.g. OSC 52 clipboard contents (user decision 2026-11-01)
            OscFallbackAction::Put(data) => h.services.log.debug(format_args!(
                "Unknown OSC code: {{ identifier: {identifier}, action: PUT, length: {} }}",
                data.len()
            )),
            OscFallbackAction::End(success) => h.services.log.debug(format_args!(
                "Unknown OSC code: {{ identifier: {identifier}, action: END, data: {success} }}"
            )),
        }
    }

    /// The constructor's handler registrations (`InputHandler.ts:189-381`) for the handlers
    /// ported so far, plus the fallbacks and the error handler.
    pub fn register_handlers(
        parser: &mut EscapeSequenceParser<InputHandler>,
    ) -> Result<(), IdentifierError> {
        // custom fallback handlers
        parser.set_csi_handler_fallback(|h, ident, params| {
            h.services.log.debug(format_args!(
                "Unknown CSI code: {{ identifier: {}, params: {} }}",
                Ident(ident),
                ParamsArray(params)
            ));
        });
        parser.set_esc_handler_fallback(|h, ident| {
            h.services.log.debug(format_args!(
                "Unknown ESC code: {{ identifier: {} }}",
                Ident(ident)
            ));
        });
        parser.set_execute_handler_fallback(|h, code| {
            h.services
                .log
                .debug(format_args!("Unknown EXECUTE code: {{ code: {code} }}"));
        });
        parser.set_osc_handler_fallback(Self::log_unknown_osc);
        parser.set_dcs_handler_fallback(|h, ident, action| match action {
            DcsFallbackAction::Hook(params) => h.services.log.debug(format_args!(
                "Unknown DCS code: {{ identifier: {}, action: HOOK, payload: {} }}",
                Ident(ident),
                ParamsArray(params)
            )),
            // LUNETERM: the length, not the payload (as for OSC above)
            DcsFallbackAction::Put(data) => h.services.log.debug(format_args!(
                "Unknown DCS code: {{ identifier: {}, action: PUT, length: {} }}",
                Ident(ident),
                data.len()
            )),
            DcsFallbackAction::Unhook(success) => h.services.log.debug(format_args!(
                "Unknown DCS code: {{ identifier: {}, action: UNHOOK, payload: {success} }}",
                Ident(ident)
            )),
        });

        // print handler
        parser.set_print_handler(|h, data| h.print(data));

        // CSI handler
        let csi = FunctionIdentifier::new;
        parser.register_csi_handler(csi('A'), |h, p| h.cursor_up(p))?;
        parser.register_csi_handler(csi('B'), |h, p| h.cursor_down(p))?;
        parser.register_csi_handler(csi('C'), |h, p| h.cursor_forward(p))?;
        parser.register_csi_handler(csi('D'), |h, p| h.cursor_backward(p))?;
        parser.register_csi_handler(csi('E'), |h, p| h.cursor_next_line(p))?;
        parser.register_csi_handler(csi('F'), |h, p| h.cursor_preceding_line(p))?;
        parser.register_csi_handler(csi('G'), |h, p| h.cursor_char_absolute(p))?;
        parser.register_csi_handler(csi('H'), |h, p| h.cursor_position(p))?;
        parser.register_csi_handler(csi('I'), |h, p| h.cursor_forward_tab(p))?;
        parser.register_csi_handler(csi('Z'), |h, p| h.cursor_backward_tab(p))?;
        parser.register_csi_handler(csi('`'), |h, p| h.char_pos_absolute(p))?;
        parser.register_csi_handler(csi('a'), |h, p| h.h_position_relative(p))?;
        parser.register_csi_handler(csi('d'), |h, p| h.line_pos_absolute(p))?;
        parser.register_csi_handler(csi('e'), |h, p| h.v_position_relative(p))?;
        parser.register_csi_handler(csi('f'), |h, p| h.hv_position(p))?;
        parser.register_csi_handler(csi('g'), |h, p| h.tab_clear(p))?;
        parser.register_csi_handler(csi('r'), |h, p| h.set_scroll_region(p))?;
        parser.register_csi_handler(csi('s'), |h, _| h.save_cursor())?;
        parser.register_csi_handler(csi('u'), |h, _| h.restore_cursor())?;
        parser.register_csi_handler(csi('@'), |h, p| h.insert_chars(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates(" ", '@'), |h, p| {
            h.scroll_left(p)
        })?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates(" ", 'A'), |h, p| {
            h.scroll_right(p)
        })?;
        parser.register_csi_handler(csi('J'), |h, p| h.erase_in_display(p, false))?;
        parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'J'), |h, p| {
            h.erase_in_display(p, true)
        })?;
        parser.register_csi_handler(csi('K'), |h, p| h.erase_in_line(p, false))?;
        parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'K'), |h, p| {
            h.erase_in_line(p, true)
        })?;
        parser.register_csi_handler(csi('L'), |h, p| h.insert_lines(p))?;
        parser.register_csi_handler(csi('M'), |h, p| h.delete_lines(p))?;
        parser.register_csi_handler(csi('P'), |h, p| h.delete_chars(p))?;
        parser.register_csi_handler(csi('S'), |h, p| h.scroll_up(p))?;
        parser.register_csi_handler(csi('T'), |h, p| h.scroll_down(p))?;
        parser.register_csi_handler(csi('X'), |h, p| h.erase_chars(p))?;
        parser.register_csi_handler(csi('b'), |h, p| h.repeat_preceding_character(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates("'", '}'), |h, p| {
            h.insert_columns(p)
        })?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates("'", '~'), |h, p| {
            h.delete_columns(p)
        })?;
        parser
            .register_csi_handler(FunctionIdentifier::with_intermediates("\"", 'q'), |h, p| {
                h.select_protected(p)
            })?;
        parser.register_csi_handler(csi('m'), |h, p| h.char_attributes(p))?;
        parser.register_csi_handler(csi('c'), |h, p| h.send_device_attributes_primary(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_prefix('>', 'c'), |h, p| {
            h.send_device_attributes_secondary(p)
        })?;
        parser.register_csi_handler(csi('h'), |h, p| h.set_mode(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'h'), |h, p| {
            h.set_mode_private(p)
        })?;
        parser.register_csi_handler(csi('l'), |h, p| h.reset_mode(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'l'), |h, p| {
            h.reset_mode_private(p)
        })?;
        parser.register_csi_handler(csi('n'), |h, p| h.device_status(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'n'), |h, p| {
            h.device_status_private(p)
        })?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates("!", 'p'), |h, p| {
            h.soft_reset(p)
        })?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates(" ", 'q'), |h, p| {
            h.set_cursor_style(p)
        })?;
        // The window option guard is part of the handler itself (upstream registers the plain
        // method on the parser; only handlers added through `InputHandler.registerCsiHandler`
        // are wrapped).
        parser.register_csi_handler(csi('t'), |h, p| h.window_options(p))?;
        parser.register_csi_handler(FunctionIdentifier::with_intermediates("$", 'p'), |h, p| {
            h.request_mode(p, true)
        })?;
        parser.register_csi_handler(
            FunctionIdentifier::with_prefix_and_intermediates('?', "$", 'p'),
            |h, p| h.request_mode(p, false),
        )?;

        // execute handler
        parser.set_execute_handler(C0::BEL, |h| {
            h.bell();
        });
        parser.set_execute_handler(C0::LF, |h| {
            h.line_feed();
        });
        parser.set_execute_handler(C0::VT, |h| {
            h.line_feed();
        });
        parser.set_execute_handler(C0::FF, |h| {
            h.line_feed();
        });
        parser.set_execute_handler(C0::CR, |h| {
            h.carriage_return();
        });
        parser.set_execute_handler(C0::BS, |h| {
            h.backspace();
        });
        parser.set_execute_handler(C0::HT, |h| {
            h.tab();
        });
        parser.set_execute_handler(C0::SO, |h| {
            h.shift_out();
        });
        parser.set_execute_handler(C0::SI, |h| {
            h.shift_in();
        });
        parser.set_execute_handler(C1::IND, |h| {
            h.index();
        });
        parser.set_execute_handler(C1::NEL, |h| {
            h.next_line();
        });
        parser.set_execute_handler(C1::HTS, |h| {
            h.tab_set();
        });

        // ESC handlers
        parser.register_esc_handler(csi('7'), |h| h.save_cursor())?;
        parser.register_esc_handler(csi('8'), |h| h.restore_cursor())?;
        parser.register_esc_handler(csi('D'), |h| h.index())?;
        parser.register_esc_handler(csi('E'), |h| h.next_line())?;
        parser.register_esc_handler(csi('H'), |h| h.tab_set())?;
        parser.register_esc_handler(csi('M'), |h| h.reverse_index())?;
        parser.register_esc_handler(csi('='), |h| h.keypad_application_mode())?;
        parser.register_esc_handler(csi('>'), |h| h.keypad_numeric_mode())?;
        parser.register_esc_handler(csi('c'), |h| h.full_reset())?;
        parser.register_esc_handler(csi('n'), |h| h.set_g_level(2))?;
        parser.register_esc_handler(csi('o'), |h| h.set_g_level(3))?;
        parser.register_esc_handler(csi('|'), |h| h.set_g_level(3))?;
        parser.register_esc_handler(csi('}'), |h| h.set_g_level(2))?;
        parser.register_esc_handler(csi('~'), |h| h.set_g_level(1))?;
        parser.register_esc_handler(FunctionIdentifier::with_intermediates("%", '@'), |h| {
            h.select_default_charset()
        })?;
        parser.register_esc_handler(FunctionIdentifier::with_intermediates("%", 'G'), |h| {
            h.select_default_charset()
        })?;
        for flag in CHARSET_FLAGS {
            for intermediate in CHARSET_INTERMEDIATES {
                let id = FunctionIdentifier::with_intermediates(&intermediate.to_string(), flag);
                parser.register_esc_handler(id, move |h| h.select_charset(intermediate, flag))?;
            }
        }
        parser.register_esc_handler(FunctionIdentifier::with_intermediates("#", '8'), |h| {
            h.screen_alignment_pattern()
        })?;

        // OSC handler
        InputHandler::register_osc_handler(parser, 0, |h, data| h.set_title_and_icon_name(data));
        InputHandler::register_osc_handler(parser, 1, |h, data| h.set_icon_name(data));
        InputHandler::register_osc_handler(parser, 2, |h, data| h.set_title(data));
        InputHandler::register_osc_handler(parser, 4, |h, data| {
            h.set_or_report_indexed_color(data)
        });
        InputHandler::register_osc_handler(parser, 8, |h, data| h.set_hyperlink(data));
        InputHandler::register_osc_handler(parser, 10, |h, data| h.set_or_report_fg_color(data));
        InputHandler::register_osc_handler(parser, 11, |h, data| h.set_or_report_bg_color(data));
        InputHandler::register_osc_handler(parser, 12, |h, data| {
            h.set_or_report_cursor_color(data)
        });
        InputHandler::register_osc_handler(parser, 104, |h, data| h.restore_indexed_color(data));
        InputHandler::register_osc_handler(parser, 110, |h, _| {
            h.restore_special_color(ColorIndex::Foreground)
        });
        InputHandler::register_osc_handler(parser, 111, |h, _| {
            h.restore_special_color(ColorIndex::Background)
        });
        InputHandler::register_osc_handler(parser, 112, |h, _| {
            h.restore_special_color(ColorIndex::Cursor)
        });

        // DCS handler
        InputHandler::register_dcs_handler(
            parser,
            FunctionIdentifier::with_intermediates("$", 'q'),
            |h, data, _| h.request_status_string(data),
        )?;

        // error handler: log, never abort (upstream returns the state unchanged)
        parser.set_error_handler(|h, state| {
            h.services
                .log
                .error(format_args!("Parsing error: {state:?}"));
            false
        });
        Ok(())
    }

    /// Forward `registerCsiHandler` to `parser`. A plain CSI `t` handler only runs for window
    /// options the options allow (security: always checked).
    pub fn register_csi_handler(
        parser: &mut EscapeSequenceParser<InputHandler>,
        id: FunctionIdentifier,
        mut callback: impl FnMut(&mut InputHandler, &Params) -> bool + Send + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        if id.final_byte == 't' && id.prefix.is_none() && id.intermediates.is_empty() {
            return parser.register_csi_handler(id, move |h, params| {
                if !param_to_window_option(
                    params.get(0),
                    &h.services.options.options().window_options,
                ) {
                    return true;
                }
                callback(h, params)
            });
        }
        parser.register_csi_handler(id, callback)
    }

    /// Forward `registerDcsHandler` to `parser` (the payload collected as a string).
    pub fn register_dcs_handler(
        parser: &mut EscapeSequenceParser<InputHandler>,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler, &str, &Params) -> bool + Send + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        parser.register_dcs_handler(id, DcsStringHandler::new(callback))
    }

    /// Forward `registerEscHandler` to `parser`.
    pub fn register_esc_handler(
        parser: &mut EscapeSequenceParser<InputHandler>,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler) -> bool + Send + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        parser.register_esc_handler(id, callback)
    }

    /// Forward `registerOscHandler` to `parser` (the payload collected as a string).
    pub fn register_osc_handler(
        parser: &mut EscapeSequenceParser<InputHandler>,
        ident: i32,
        callback: impl FnMut(&mut InputHandler, &str) -> bool + Send + 'static,
    ) -> HandlerId {
        parser.register_osc_handler(ident, OscStringHandler::new(callback))
    }

    /// Parse UTF-8 `data` with `parser` (whose handlers were registered by
    /// [`InputHandler::register_handlers`]). Writes longer than [`MAX_PARSEBUFFER_LENGTH`] bytes
    /// are decoded and parsed in chunks of that many bytes. Afterwards the cursor move and the
    /// dirty viewport rows are added to the damage.
    pub fn parse(&mut self, parser: &mut EscapeSequenceParser<InputHandler>, data: &[u8]) {
        let (cursor_start_x, cursor_start_y) = {
            let buffer = self.services.buffer.buffer();
            (buffer.x, buffer.y)
        };

        // LUNETERM: (user decision 2026-11-02, review S5) upstream logs the data at debug and
        // its codes at trace; the data is the pty output (an OSC 52 payload, a password echoed
        // by a program), so only its length is logged and the trace line is dropped.
        self.services
            .log
            .debug(format_args!("parsing data ({} bytes)", data.len()));

        // Clear the dirty row tracker so we know which lines changed as a result of parsing
        self.dirty_row_tracker
            .clear_range(self.services.buffer.buffer().y);

        // process big data in smaller chunks; the buffer is lent out while the parser runs
        let mut parse_buffer = mem::take(&mut self.parse_buffer);
        for chunk in data.chunks(MAX_PARSEBUFFER_LENGTH) {
            let len = self.utf8_decoder.decode(chunk, &mut parse_buffer);
            parser.parse(self, parse_buffer.get(..len).unwrap_or(&[]));
        }
        self.parse_buffer = parse_buffer;

        let buffer = self.services.buffer.buffer();
        if buffer.x != cursor_start_x || buffer.y != cursor_start_y {
            self.damage.cursor_moved = true;
        }

        // Refresh any dirty rows accumulated as part of parsing, only for rows within the
        // _viewport_ which is relative to ydisp, not relative to ybase.
        let shift = to_i64(buffer.ybase) - to_i64(buffer.ydisp);
        let rows = to_i64(self.services.buffer.rows);
        let viewport_end = self.dirty_row_tracker.end.saturating_add(shift);
        let viewport_start = self.dirty_row_tracker.start.saturating_add(shift);
        if viewport_start < rows {
            self.damage.add_rows(
                to_usize(viewport_start.min(rows - 1)),
                to_usize(viewport_end.min(rows - 1)),
            );
        }
    }

    /// Print `data` (code points) at the cursor.
    pub fn print(&mut self, data: &[u32]) {
        self.print_inner(data, true, true, true);
    }

    /// LUNETERM: (M7 fix round 1) `print`'s body (upstream's). REP's line fill prints a
    /// repetition in the middle of its stream through here ([`InputHandler::print_repeated`]):
    /// without the end fix-up (`fix_up_end`), which `print` makes once after its last code point,
    /// and without the screen reader text (`voice`), which the fill queues up front; (M7 fix
    /// round 5) and without the strings' copy of a joiner's wrap (`wrap_strings`, see
    /// `print_wrap`) where the line fill's own scrolls discard the line it lands on. Returns
    /// false where `print` stops early (a missing line).
    fn print_inner(
        &mut self,
        data: &[u32],
        fix_up_end: bool,
        voice: bool,
        wrap_strings: bool,
    ) -> bool {
        let InputHandler {
            services,
            cur_attr_data,
            erase_attr_data_internal,
            dirty_row_tracker,
            preceding_join_state,
            events,
            damage,
            ..
        } = self;
        let charset = services.charset.charset;
        let screen_reader_mode = services.options.options().screen_reader_mode;
        let cols = services.buffer.cols;
        let wraparound_mode = services.core.dec_private_modes.wraparound;
        let insert_mode = services.core.modes.insert_mode;
        let cur_attr = *cur_attr_data;
        let null_cell = services.buffer.buffer().get_null_cell(Some(&cur_attr));

        let buffer = services.buffer.buffer_mut();
        let mut row = abs_y(buffer);
        let x = buffer.x;
        let Some(buffer_row) = line_mut(buffer, row) else {
            return false;
        };
        // handle wide chars: reset start_cell-1 if we would overwrite the second cell of a wide
        // char
        if x != 0 && !data.is_empty() && buffer_row.get_width(x - 1) == 2 {
            buffer_row.set_cell_from_codepoint(x - 1, 0, 1, &cur_attr);
        }
        dirty_row_tracker.mark_dirty(buffer.y);

        let mut preceding = *preceding_join_state;
        for &input in data {
            let mut code = input;

            // get charset replacement character; charset is only defined for ASCII
            code = charset_replacement(charset, code);

            let current_info = services.unicode.char_properties(code, preceding);
            let mut ch_width = UnicodeService::extract_width(current_info);
            let should_join = UnicodeService::extract_should_join(current_info);
            let old_width = if should_join {
                UnicodeService::extract_width(preceding)
            } else {
                0
            };
            preceding = current_info;

            if screen_reader_mode && voice {
                events.a11y_char(code);
            }
            let link_id = cur_attr.extended.url_id();
            if link_id != 0 {
                add_cursor_line_to_link(services, link_id);
            }

            // goto next line if ch would overflow (the terminal does not allow cols < 2)
            let x = services.buffer.buffer().x;
            if to_i64(x) + i64::from(ch_width) - i64::from(old_width) > to_i64(cols) {
                if wraparound_mode {
                    // autowrap - DECAWM
                    match print_wrap(
                        services,
                        dirty_row_tracker,
                        damage,
                        erase_attr_data_internal,
                        &cur_attr,
                        row,
                        old_width,
                        wrap_strings,
                    ) {
                        Some(new_row) => row = new_row,
                        // upstream throws on the missing line
                        None => return false,
                    }
                } else {
                    services.buffer.buffer_mut().x = cols.saturating_sub(1);
                    if ch_width == 2 {
                        // a wide char that does not fit into the last cell
                        continue;
                    }
                }
            }

            let Buffer { lines, x, .. } = services.buffer.buffer_mut();
            let Some(buffer_row) = lines.get_mut(row) else {
                return false;
            };

            // insert combining char at last cursor position; x is never 0 for a combining char
            // after a cell consuming char, so testing x avoids overflow left
            if should_join && *x != 0 {
                // if empty cell after fullwidth, need to go 2 cells back
                let offset = if buffer_row.get_width(*x - 1) != 0 {
                    1
                } else {
                    2
                };
                // a negative cell index is a no-op typed-array write upstream
                if let Some(cell) = x.checked_sub(offset) {
                    buffer_row.add_codepoint_to_cell(cell, code, ch_width);
                }
                let mut delta = i64::from(ch_width) - i64::from(old_width);
                loop {
                    delta -= 1;
                    if delta < 0 {
                        break;
                    }
                    buffer_row.set_cell_from_codepoint(*x, 0, 0, &cur_attr);
                    *x += 1;
                }
                continue;
            }

            // insert mode: move characters to right
            if insert_mode {
                // right shift cells according to the width
                let n = to_usize(i64::from(ch_width) - i64::from(old_width));
                buffer_row.insert_cells(*x, n, &null_cell);
                // a fullwidth char shifted into the last cell is lost and set to an empty cell
                if buffer_row.get_width(cols.saturating_sub(1)) == 2 {
                    buffer_row.set_cell_from_codepoint(
                        cols.saturating_sub(1),
                        NULL_CELL_CODE,
                        NULL_CELL_WIDTH,
                        &cur_attr,
                    );
                }
            }

            // write current char to buffer and advance cursor
            buffer_row.set_cell_from_codepoint(*x, code, ch_width, &cur_attr);
            *x += 1;

            // fullwidth char - also set next cell to placeholder stub and advance cursor
            if ch_width > 0 {
                loop {
                    ch_width -= 1;
                    if ch_width == 0 {
                        break;
                    }
                    // other than a regular empty cell a cell following a wide char has no width
                    buffer_row.set_cell_from_codepoint(*x, 0, 0, &cur_attr);
                    *x += 1;
                }
            }
        }

        *preceding_join_state = preceding;

        // handle wide chars: reset cell to the right if it is second cell of a wide char
        let buffer = services.buffer.buffer_mut();
        let x = buffer.x;
        if fix_up_end && x < cols && !data.is_empty() {
            if let Some(buffer_row) = line_mut(buffer, row) {
                if buffer_row.get_width(x) == 0 && buffer_row.has_content(x) == 0 {
                    buffer_row.set_cell_from_codepoint(x, 0, 1, &cur_attr);
                }
            }
        }
        dirty_row_tracker.mark_dirty(buffer.y);
        true
    }

    /// `bufferService.onScroll` as `CoreTerminal` forwards it (`CoreTerminal.ts:137-140`): scroll
    /// damage and the scroll region marked dirty. `_ydisp` is the event payload; the host reads
    /// the position from the buffer.
    pub fn on_scroll(&mut self, _ydisp: usize) {
        scrolled(
            &mut self.damage,
            &mut self.dirty_row_tracker,
            self.services.buffer.buffer(),
        );
    }

    /// Upstream `markRangeDirty`.
    pub fn mark_range_dirty(&mut self, y1: i64, y2: i64) {
        self.dirty_row_tracker.mark_range_dirty(y1, y2);
    }

    /// back_color_erase feature for xterm (upstream `_eraseAttrData`).
    fn erase_attr_data(&mut self) -> AttributeData {
        erase_attr(&mut self.erase_attr_data_internal, &self.cur_attr_data)
    }

    /// `bufferService.scroll` followed by the forwarded `onScroll`.
    fn scroll_buffer(&mut self, is_wrapped: bool) {
        let erase = self.erase_attr_data();
        let ydisp = self.services.buffer.scroll(&erase, is_wrapped);
        self.on_scroll(ydisp);
    }

    /// BEL: ring the bell.
    pub fn bell(&mut self) -> bool {
        self.events.push(HostEvent::Bell);
        true
    }

    /// LF (also VT and FF): move the cursor one row down, scrolling at the bottom margin.
    pub fn line_feed(&mut self) -> bool {
        let convert_eol = self.services.options.options().convert_eol;
        let rows = self.services.buffer.rows;
        let cols = self.services.buffer.cols;
        let buffer = self.services.buffer.buffer_mut();
        self.dirty_row_tracker.mark_dirty(buffer.y);
        if convert_eol {
            buffer.x = 0;
        }
        buffer.y += 1;
        if buffer.y == buffer.scroll_bottom.saturating_add(1) {
            buffer.y -= 1;
            self.scroll_buffer(false);
        } else if buffer.y >= rows {
            buffer.y = rows.saturating_sub(1);
        } else {
            // An explicit line feed clears the wrapped state of the line.
            let row = abs_y(buffer);
            let Some(line) = line_mut(buffer, row) else {
                return true;
            };
            line.is_wrapped = false;
        }
        let buffer = self.services.buffer.buffer_mut();
        // If the end of the line is hit, prevent this action from wrapping around to the next
        // line.
        if buffer.x >= cols {
            buffer.x -= 1;
        }
        self.dirty_row_tracker.mark_dirty(buffer.y);

        // onLineFeed: the Windows wrapping heuristic (plan M6 D6)
        if self.windows_heuristics {
            update_windows_mode_wrapped_state(&mut self.services.buffer);
        }
        true
    }

    /// CR: move the cursor to the beginning of the row.
    pub fn carriage_return(&mut self) -> bool {
        self.services.buffer.buffer_mut().x = 0;
        true
    }

    /// BS: move the cursor one position to the left. With reverse wrap-around (`CSI ? 45 h`) a
    /// previous soft wrap within the scroll margins is undone.
    pub fn backspace(&mut self) -> bool {
        // reverse wrap-around is disabled
        if !self.services.core.dec_private_modes.reverse_wraparound {
            self.restrict_cursor(None);
            let buffer = self.services.buffer.buffer_mut();
            if buffer.x > 0 {
                buffer.x -= 1;
            }
            return true;
        }

        // reverse wrap-around is enabled: the cursor may be at x = cols to address the last cell
        let cols = self.services.buffer.cols;
        self.restrict_cursor(Some(cols));

        let buffer = self.services.buffer.buffer_mut();
        if buffer.x > 0 {
            buffer.x -= 1;
        } else {
            let row = abs_y(buffer);
            if buffer.x == 0
                && buffer.y > buffer.scroll_top
                && buffer.y <= buffer.scroll_bottom
                && buffer.lines.get(row).is_some_and(|line| line.is_wrapped)
            {
                if let Some(line) = line_mut(buffer, row) {
                    line.is_wrapped = false;
                }
                buffer.y -= 1;
                buffer.x = cols.saturating_sub(1);
                // find last taken cell: an empty cell of width 1 before an early wrapping wide
                // char is skipped
                let row = abs_y(buffer);
                let x = buffer.x;
                let Some(line) = line_mut(buffer, row) else {
                    return true;
                };
                if line.has_width(x) != 0 && line.has_content(x) == 0 {
                    buffer.x -= 1;
                }
            }
        }
        self.restrict_cursor(None);
        true
    }

    /// HT: move the cursor to the next tab stop.
    pub fn tab(&mut self) -> bool {
        let cols = self.services.buffer.cols;
        let screen_reader_mode = self.services.options.options().screen_reader_mode;
        let buffer = self.services.buffer.buffer_mut();
        if buffer.x >= cols {
            return true;
        }
        let original_x = buffer.x;
        buffer.x = buffer.next_stop(None);
        if screen_reader_mode {
            self.events
                .push(HostEvent::A11yTab(buffer.x.saturating_sub(original_x)));
        }
        true
    }

    /// SO: invoke the G1 character set.
    pub fn shift_out(&mut self) -> bool {
        self.services.charset.set_g_level(1);
        true
    }

    /// SI: invoke the G0 character set.
    pub fn shift_in(&mut self) -> bool {
        self.services.charset.set_g_level(0);
        true
    }

    /// `x`, `y` restricted to the viewport (`max_col`, default `cols - 1`) or, in origin mode,
    /// to the scroll margins (upstream `_restrictCursor` on the given values).
    fn restricted(&self, x: i64, y: i64, max_col: Option<usize>) -> (usize, usize) {
        let cols = self.services.buffer.cols;
        let rows = self.services.buffer.rows;
        let buffer = self.services.buffer.buffer();
        let max_col = to_i64(max_col.unwrap_or(cols.saturating_sub(1)));
        let x = max_col.min(x.max(0));
        let y = if self.services.core.dec_private_modes.origin {
            to_i64(buffer.scroll_bottom).min(to_i64(buffer.scroll_top).max(y))
        } else {
            (to_i64(rows) - 1).min(y.max(0))
        };
        (to_usize(x), to_usize(y))
    }

    /// Restrict the cursor to the viewport size / scroll margin (origin mode).
    fn restrict_cursor(&mut self, max_col: Option<usize>) {
        let buffer = self.services.buffer.buffer();
        let (x, y) = self.restricted(to_i64(buffer.x), to_i64(buffer.y), max_col);
        let buffer = self.services.buffer.buffer_mut();
        buffer.x = x;
        buffer.y = y;
        self.dirty_row_tracker.mark_dirty(y);
    }

    /// Set the absolute cursor position.
    fn set_cursor(&mut self, x: i64, y: i64) {
        let buffer = self.services.buffer.buffer();
        self.dirty_row_tracker.mark_dirty(buffer.y);
        let y = if self.services.core.dec_private_modes.origin {
            to_i64(buffer.scroll_top).saturating_add(y)
        } else {
            y
        };
        let (x, y) = self.restricted(x, y, None);
        let buffer = self.services.buffer.buffer_mut();
        buffer.x = x;
        buffer.y = y;
        self.dirty_row_tracker.mark_dirty(y);
    }

    /// Set the relative cursor position; the cursor is restricted first.
    fn move_cursor(&mut self, x: i64, y: i64) {
        self.restrict_cursor(None);
        let buffer = self.services.buffer.buffer();
        let (bx, by) = (to_i64(buffer.x), to_i64(buffer.y));
        self.set_cursor(bx.saturating_add(x), by.saturating_add(y));
    }

    /// CSI Ps A (CUU): move the cursor `Ps` rows up, stopping at the top margin.
    pub fn cursor_up(&mut self, params: &Params) -> bool {
        let buffer = self.services.buffer.buffer();
        let diff_to_top = to_i64(buffer.y) - to_i64(buffer.scroll_top);
        let n = param_or(params, 0, 1);
        if diff_to_top >= 0 {
            self.move_cursor(0, -diff_to_top.min(n));
        } else {
            self.move_cursor(0, -n);
        }
        true
    }

    /// CSI Ps B (CUD): move the cursor `Ps` rows down, stopping at the bottom margin.
    pub fn cursor_down(&mut self, params: &Params) -> bool {
        let buffer = self.services.buffer.buffer();
        let diff_to_bottom = to_i64(buffer.scroll_bottom) - to_i64(buffer.y);
        let n = param_or(params, 0, 1);
        if diff_to_bottom >= 0 {
            self.move_cursor(0, diff_to_bottom.min(n));
        } else {
            self.move_cursor(0, n);
        }
        true
    }

    /// CSI Ps C (CUF): move the cursor `Ps` columns forward.
    pub fn cursor_forward(&mut self, params: &Params) -> bool {
        self.move_cursor(param_or(params, 0, 1), 0);
        true
    }

    /// CSI Ps D (CUB): move the cursor `Ps` columns backward.
    pub fn cursor_backward(&mut self, params: &Params) -> bool {
        self.move_cursor(-param_or(params, 0, 1), 0);
        true
    }

    /// CSI Ps E (CNL): as CUD, then to the first column.
    pub fn cursor_next_line(&mut self, params: &Params) -> bool {
        self.cursor_down(params);
        self.services.buffer.buffer_mut().x = 0;
        true
    }

    /// CSI Ps F (CPL): as CUU, then to the first column.
    pub fn cursor_preceding_line(&mut self, params: &Params) -> bool {
        self.cursor_up(params);
        self.services.buffer.buffer_mut().x = 0;
        true
    }

    /// CSI Ps G (CHA): move the cursor to column `Ps` of the active row.
    pub fn cursor_char_absolute(&mut self, params: &Params) -> bool {
        let y = to_i64(self.services.buffer.buffer().y);
        self.set_cursor(param_or(params, 0, 1) - 1, y);
        true
    }

    /// CSI Ps ; Ps H (CUP): set the cursor to row, column (1-based; within the margins in
    /// origin mode).
    pub fn cursor_position(&mut self, params: &Params) -> bool {
        let col = if params.length() >= 2 {
            param_or(params, 1, 1) - 1
        } else {
            0
        };
        self.set_cursor(col, param_or(params, 0, 1) - 1);
        true
    }

    /// CSI Ps ` (HPA): same as CHA.
    pub fn char_pos_absolute(&mut self, params: &Params) -> bool {
        let y = to_i64(self.services.buffer.buffer().y);
        self.set_cursor(param_or(params, 0, 1) - 1, y);
        true
    }

    /// CSI Ps a (HPR): same as CUF.
    pub fn h_position_relative(&mut self, params: &Params) -> bool {
        self.move_cursor(param_or(params, 0, 1), 0);
        true
    }

    /// CSI Ps d (VPA): move the cursor to row `Ps`.
    pub fn line_pos_absolute(&mut self, params: &Params) -> bool {
        let x = to_i64(self.services.buffer.buffer().x);
        self.set_cursor(x, param_or(params, 0, 1) - 1);
        true
    }

    /// CSI Ps e (VPR): move the cursor `Ps` rows down.
    pub fn v_position_relative(&mut self, params: &Params) -> bool {
        self.move_cursor(0, param_or(params, 0, 1));
        true
    }

    /// CSI Ps ; Ps f (HVP): same as CUP.
    pub fn hv_position(&mut self, params: &Params) -> bool {
        self.cursor_position(params);
        true
    }

    /// CSI Ps g (TBC): clear the tab stop at the cursor (0) or all tab stops (3).
    pub fn tab_clear(&mut self, params: &Params) -> bool {
        let buffer = self.services.buffer.buffer_mut();
        match params.get(0) {
            Some(0) => {
                let x = buffer.x;
                buffer.tabs.remove(&x);
            }
            Some(3) => buffer.tabs.clear(),
            _ => {}
        }
        true
    }

    /// CSI Ps I (CHT): move the cursor `Ps` tab stops forward.
    pub fn cursor_forward_tab(&mut self, params: &Params) -> bool {
        let cols = self.services.buffer.cols;
        let buffer = self.services.buffer.buffer_mut();
        if buffer.x >= cols {
            return true;
        }
        let mut param = param_or(params, 0, 1);
        while param != 0 {
            param -= 1;
            let next = buffer.next_stop(None);
            // the last column is a fixed point: further iterations change nothing
            if next == buffer.x {
                break;
            }
            buffer.x = next;
        }
        true
    }

    /// CSI Ps Z (CBT): move the cursor `Ps` tab stops backward.
    pub fn cursor_backward_tab(&mut self, params: &Params) -> bool {
        let cols = self.services.buffer.cols;
        let buffer = self.services.buffer.buffer_mut();
        if buffer.x >= cols {
            return true;
        }
        let mut param = param_or(params, 0, 1);
        while param != 0 {
            param -= 1;
            let prev = buffer.prev_stop(None);
            // column 0 is a fixed point: further iterations change nothing
            if prev == buffer.x {
                break;
            }
            buffer.x = prev;
        }
        true
    }

    /// CSI Ps ; Ps r (DECSTBM): set the top and bottom margins (default: the whole viewport)
    /// and home the cursor. Only applies for top < bottom.
    pub fn set_scroll_region(&mut self, params: &Params) -> bool {
        let rows = to_i64(self.services.buffer.rows);
        let top = param_or(params, 0, 1);
        let bottom = if params.length() < 2 {
            Some(rows)
        } else {
            match params.get(1).map(i64::from) {
                Some(bottom) if bottom > rows || bottom == 0 => Some(rows),
                // `undefined > rows` and `undefined > top` are false: no change
                other => other,
            }
        };
        if let Some(bottom) = bottom {
            if bottom > top {
                let buffer = self.services.buffer.buffer_mut();
                buffer.scroll_top = to_usize(top - 1);
                buffer.scroll_bottom = to_usize(bottom - 1);
                self.set_cursor(0, 0);
            }
        }
        true
    }

    /// CSI s, ESC 7 (DECSC): save the cursor position, the colours and the charset.
    pub fn save_cursor(&mut self) -> bool {
        let charset = self.services.charset.charset;
        let cur = self.cur_attr_data;
        let buffer = self.services.buffer.buffer_mut();
        buffer.saved_x = buffer.x;
        buffer.saved_y = abs_y(buffer);
        buffer.saved_cur_attr_data.fg = cur.fg;
        buffer.saved_cur_attr_data.bg = cur.bg;
        buffer.saved_charset = charset;
        true
    }

    /// CSI u, ESC 8 (DECRC): restore what [`InputHandler::save_cursor`] saved.
    pub fn restore_cursor(&mut self) -> bool {
        let buffer = self.services.buffer.buffer_mut();
        buffer.x = buffer.saved_x;
        buffer.y = buffer.saved_y.saturating_sub(buffer.ybase);
        self.cur_attr_data.fg = buffer.saved_cur_attr_data.fg;
        self.cur_attr_data.bg = buffer.saved_cur_attr_data.bg;
        // upstream assigns `(this as any)._savedCharset`, which is never set
        self.services.charset.charset = None;
        if buffer.saved_charset.is_some() {
            self.services.charset.charset = buffer.saved_charset;
        }
        self.restrict_cursor(None);
        true
    }

    /// NEL, ESC E: move the cursor to the beginning of the next row.
    pub fn next_line(&mut self) -> bool {
        self.services.buffer.buffer_mut().x = 0;
        self.index();
        true
    }

    /// IND, ESC D: move the cursor one row down, scrolling at the bottom margin.
    pub fn index(&mut self) -> bool {
        self.restrict_cursor(None);
        let rows = self.services.buffer.rows;
        let buffer = self.services.buffer.buffer_mut();
        buffer.y += 1;
        if buffer.y == buffer.scroll_bottom.saturating_add(1) {
            buffer.y -= 1;
            self.scroll_buffer(false);
        } else if buffer.y >= rows {
            buffer.y = rows.saturating_sub(1);
        }
        self.restrict_cursor(None);
        true
    }

    /// HTS, ESC H: set a tab stop at the cursor column.
    pub fn tab_set(&mut self) -> bool {
        let buffer = self.services.buffer.buffer_mut();
        let x = buffer.x;
        buffer.tabs.insert(x);
        true
    }

    /// ESC M (RI): move the cursor one row up; at the top margin the region scrolls down.
    pub fn reverse_index(&mut self) -> bool {
        self.restrict_cursor(None);
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        if buffer.y == buffer.scroll_top {
            // blankLine(true) is xterm/linux behavior
            let scroll_region_height = to_i64(buffer.scroll_bottom) - to_i64(buffer.scroll_top);
            let row = abs_y(buffer);
            let start = isize::try_from(row).unwrap_or(isize::MAX);
            let count = isize::try_from(scroll_region_height).unwrap_or(isize::MAX);
            if buffer.lines.shift_elements(start, count, 1).is_err() {
                // upstream throws
                return true;
            }
            let blank = buffer.get_blank_line(&erase, false);
            buffer.lines.set(row, Some(blank));
            let (top, bottom) = (to_i64(buffer.scroll_top), to_i64(buffer.scroll_bottom));
            self.dirty_row_tracker.mark_range_dirty(top, bottom);
        } else {
            // y may become -1 upstream; the restriction below clamps it to the same row
            buffer.y = buffer.y.saturating_sub(1);
            self.restrict_cursor(None); // quickfix to not run out of bounds
        }
        true
    }

    /// CSI Ps " q (DECSCA): whether DECSED and DECSEL can erase (0, 2) or not (1).
    pub fn select_protected(&mut self, params: &Params) -> bool {
        match params.get(0) {
            Some(1) => self.cur_attr_data.bg |= BgFlags::PROTECTED,
            Some(0 | 2) => self.cur_attr_data.bg &= !BgFlags::PROTECTED,
            _ => {}
        }
        true
    }

    /// Erase the cells `start .. end` of the cursor-relative row `y` with the erase attributes
    /// (upstream `_eraseInBufferLine`). `None` where upstream throws on a missing line.
    fn erase_in_buffer_line(
        &mut self,
        y: usize,
        start: usize,
        end: usize,
        clear_wrap: bool,
        respect_protect: bool,
    ) -> Option<()> {
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let null_cell = buffer.get_null_cell(Some(&erase));
        let row = buffer.ybase.saturating_add(y);
        let line = line_mut(buffer, row)?;
        line.replace_cells(start, end, &null_cell, respect_protect);
        if clear_wrap {
            line.is_wrapped = false;
        }
        Some(())
    }

    /// Fill the cursor-relative row `y` with the erase attributes, drop its markers and clear
    /// its wrapped flag (upstream `_resetBufferLine`; a missing line is skipped).
    fn reset_buffer_line(&mut self, y: usize, respect_protect: bool) {
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let null_cell = buffer.get_null_cell(Some(&erase));
        let row = buffer.ybase.saturating_add(y);
        let Some(line) = line_mut(buffer, row) else {
            return;
        };
        line.fill(&null_cell, respect_protect);
        line.is_wrapped = false;
        buffer.clear_markers(to_i64(row));
    }

    /// CSI Ps J (ED), CSI ? Ps J (DECSED, `respect_protect`): erase below (0), above (1), all
    /// (2) or the scrollback (3).
    pub fn erase_in_display(&mut self, params: &Params, respect_protect: bool) -> bool {
        let cols = self.services.buffer.cols;
        let rows = self.services.buffer.rows;
        self.restrict_cursor(Some(cols));
        match params.get(0) {
            Some(0) => {
                let (mut j, x) = {
                    let buffer = self.services.buffer.buffer();
                    (buffer.y, buffer.x)
                };
                self.dirty_row_tracker.mark_dirty(j);
                if self
                    .erase_in_buffer_line(j, x, cols, x == 0, respect_protect)
                    .is_none()
                {
                    return true;
                }
                j += 1;
                while j < rows {
                    self.reset_buffer_line(j, respect_protect);
                    j += 1;
                }
                self.dirty_row_tracker.mark_dirty(j);
            }
            Some(1) => {
                let (mut j, x) = {
                    let buffer = self.services.buffer.buffer();
                    (buffer.y, buffer.x)
                };
                self.dirty_row_tracker.mark_dirty(j);
                // Deleted front part of line and everything before. This line will no longer be
                // wrapped.
                if self
                    .erase_in_buffer_line(j, 0, x.saturating_add(1), true, respect_protect)
                    .is_none()
                {
                    return true;
                }
                if x.saturating_add(1) >= cols {
                    // Deleted entire previous line. This next line can no longer be wrapped.
                    // (Upstream indexes the list with `j + 1`, not relative to `ybase`.)
                    let buffer = self.services.buffer.buffer_mut();
                    match line_mut(buffer, j.saturating_add(1)) {
                        Some(line) => line.is_wrapped = false,
                        None => return true,
                    }
                }
                while j > 0 {
                    j -= 1;
                    self.reset_buffer_line(j, respect_protect);
                }
                self.dirty_row_tracker.mark_dirty(0);
            }
            Some(2) => {
                if self.services.options.options().scroll_on_erase_in_display {
                    self.dirty_row_tracker
                        .mark_range_dirty(0, to_i64(rows).saturating_sub(1));
                    // `while (j--)`: the last row with content, -1 if there is none
                    let mut j = to_i64(rows);
                    loop {
                        if j <= 0 {
                            j = -1;
                            break;
                        }
                        j -= 1;
                        let buffer = self.services.buffer.buffer();
                        let row = buffer.ybase.saturating_add(to_usize(j));
                        if buffer
                            .lines
                            .get(row)
                            .is_some_and(|line| line.get_trimmed_length() != 0)
                        {
                            break;
                        }
                    }
                    while j >= 0 {
                        self.scroll_buffer(false);
                        j -= 1;
                    }
                } else {
                    self.dirty_row_tracker.mark_dirty(rows.saturating_sub(1));
                    let mut j = rows;
                    while j > 0 {
                        j -= 1;
                        self.reset_buffer_line(j, respect_protect);
                    }
                    self.dirty_row_tracker.mark_dirty(0);
                }
            }
            Some(3) => {
                // Clear scrollback (everything not in viewport)
                let buffer = self.services.buffer.buffer_mut();
                let scrollback_size = buffer.lines.len().saturating_sub(rows);
                if scrollback_size > 0 {
                    buffer.lines.trim_start(scrollback_size);
                    buffer.ybase = buffer.ybase.saturating_sub(scrollback_size);
                    buffer.ydisp = buffer.ydisp.saturating_sub(scrollback_size);
                    // Force a scroll event to refresh viewport
                    self.on_scroll(0);
                }
            }
            _ => {}
        }
        true
    }

    /// CSI Ps K (EL), CSI ? Ps K (DECSEL, `respect_protect`): erase to the right (0), to the
    /// left (1) or the whole line (2).
    pub fn erase_in_line(&mut self, params: &Params, respect_protect: bool) -> bool {
        let cols = self.services.buffer.cols;
        self.restrict_cursor(Some(cols));
        let (x, y) = {
            let buffer = self.services.buffer.buffer();
            (buffer.x, buffer.y)
        };
        let erased = match params.get(0) {
            Some(0) => self.erase_in_buffer_line(y, x, cols, x == 0, respect_protect),
            Some(1) => self.erase_in_buffer_line(y, 0, x.saturating_add(1), false, respect_protect),
            Some(2) => self.erase_in_buffer_line(y, 0, cols, true, respect_protect),
            _ => Some(()),
        };
        if erased.is_none() {
            return true;
        }
        self.dirty_row_tracker.mark_dirty(y);
        true
    }

    /// Whether the cursor row is inside the scroll margins (the guard of IL, DL, SL, SR, DECIC
    /// and DECDC).
    fn cursor_in_scroll_region(&self) -> bool {
        let buffer = self.services.buffer.buffer();
        !(buffer.y > buffer.scroll_bottom || buffer.y < buffer.scroll_top)
    }

    /// CSI Ps L (IL): insert `Ps` blank lines at the active row; the lines at the bottom margin
    /// are lost. No effect outside the margins. The cursor moves to the first column.
    ///
    /// LUNETERM: upstream loops `Ps` times (up to 2^31 - 1); once the region from the cursor row
    /// to the bottom margin is blank further iterations change nothing, so the loop stops after
    /// that many.
    pub fn insert_lines(&mut self, params: &Params) -> bool {
        self.restrict_cursor(None);
        let param = param_or(params, 0, 1);
        if !self.cursor_in_scroll_region() {
            return true;
        }
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let row = abs_y(buffer);
        let bottom_row = buffer.ybase.saturating_add(buffer.scroll_bottom);
        let height = to_i64(buffer.scroll_bottom - buffer.y + 1);
        // LUNETERM: upstream loops `param` times; the iterations past the height change nothing.
        // The markers hear the loop as one event (`splice_rotate`, review S6).
        let count = to_usize(param.min(height));
        // blankLine(true) - xterm/linux behavior
        let blank = buffer.get_blank_line(&erase, false);
        buffer
            .lines
            .splice_rotate(bottom_row, row, count, || blank.clone());
        let (y, bottom) = (buffer.y, buffer.scroll_bottom);
        buffer.x = 0; // see https://vt100.net/docs/vt220-rm/chapter4.html - vt220 only?
        self.dirty_row_tracker
            .mark_range_dirty(to_i64(y), to_i64(bottom));
        true
    }

    /// CSI Ps M (DL): delete `Ps` lines at the active row, blank lines appear at the bottom
    /// margin. No effect outside the margins. The cursor moves to the first column.
    ///
    /// LUNETERM: the loop is bounded as in [`InputHandler::insert_lines`].
    pub fn delete_lines(&mut self, params: &Params) -> bool {
        self.restrict_cursor(None);
        let param = param_or(params, 0, 1);
        if !self.cursor_in_scroll_region() {
            return true;
        }
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let row = abs_y(buffer);
        let bottom_row = buffer.ybase.saturating_add(buffer.scroll_bottom);
        let height = to_i64(buffer.scroll_bottom - buffer.y + 1);
        // LUNETERM: upstream loops `param` times; the iterations past the height change nothing.
        // The markers hear the loop as one event (`splice_rotate`, review S6).
        let count = to_usize(param.min(height));
        // blankLine(true) - xterm/linux behavior
        let blank = buffer.get_blank_line(&erase, false);
        buffer
            .lines
            .splice_rotate(row, bottom_row, count, || blank.clone());
        let (y, bottom) = (buffer.y, buffer.scroll_bottom);
        buffer.x = 0; // see https://vt100.net/docs/vt220-rm/chapter4.html - vt220 only?
        self.dirty_row_tracker
            .mark_range_dirty(to_i64(y), to_i64(bottom));
        true
    }

    /// CSI Ps @ (ICH): insert `Ps` blank characters at the cursor; characters moved past the
    /// right margin are lost.
    pub fn insert_chars(&mut self, params: &Params) -> bool {
        self.restrict_cursor(None);
        let n = to_usize(param_or(params, 0, 1));
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let null_cell = buffer.get_null_cell(Some(&erase));
        let (row, x, y) = (abs_y(buffer), buffer.x, buffer.y);
        if let Some(line) = line_mut(buffer, row) {
            line.insert_cells(x, n, &null_cell);
            self.dirty_row_tracker.mark_dirty(y);
        }
        true
    }

    /// CSI Ps P (DCH): delete `Ps` characters at the cursor; blank characters appear at the
    /// right margin.
    pub fn delete_chars(&mut self, params: &Params) -> bool {
        self.restrict_cursor(None);
        let n = to_usize(param_or(params, 0, 1));
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let null_cell = buffer.get_null_cell(Some(&erase));
        let (row, x, y) = (abs_y(buffer), buffer.x, buffer.y);
        if let Some(line) = line_mut(buffer, row) {
            line.delete_cells(x, n, &null_cell);
            self.dirty_row_tracker.mark_dirty(y);
        }
        true
    }

    /// CSI Ps S (SU): scroll the region up `Ps` lines.
    ///
    /// LUNETERM: upstream loops `Ps` times; after as many iterations as the region has rows
    /// every line is blank and further iterations change nothing, so the loop stops there.
    pub fn scroll_up(&mut self, params: &Params) -> bool {
        let param = param_or(params, 0, 1);
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let top_row = buffer.ybase.saturating_add(buffer.scroll_top);
        let bottom_row = buffer.ybase.saturating_add(buffer.scroll_bottom);
        let height = to_i64(buffer.scroll_bottom.saturating_sub(buffer.scroll_top)) + 1;
        // LUNETERM: upstream loops `param` times; the iterations past the height change nothing.
        // The markers hear the loop as one event (`splice_rotate`, review S6).
        let count = to_usize(param.min(height));
        let blank = buffer.get_blank_line(&erase, false);
        buffer
            .lines
            .splice_rotate(top_row, bottom_row, count, || blank.clone());
        let (top, bottom) = (to_i64(buffer.scroll_top), to_i64(buffer.scroll_bottom));
        self.dirty_row_tracker.mark_range_dirty(top, bottom);
        true
    }

    /// CSI Ps T (SD): scroll the region down `Ps` lines (the new lines use the default
    /// attributes, as upstream).
    ///
    /// LUNETERM: the loop is bounded as in [`InputHandler::scroll_up`].
    pub fn scroll_down(&mut self, params: &Params) -> bool {
        let param = param_or(params, 0, 1);
        let buffer = self.services.buffer.buffer_mut();
        let top_row = buffer.ybase.saturating_add(buffer.scroll_top);
        let bottom_row = buffer.ybase.saturating_add(buffer.scroll_bottom);
        let height = to_i64(buffer.scroll_bottom.saturating_sub(buffer.scroll_top)) + 1;
        // LUNETERM: upstream loops `param` times; the iterations past the height change nothing.
        // The markers hear the loop as one event (`splice_rotate`, review S6).
        let count = to_usize(param.min(height));
        let blank = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
        buffer
            .lines
            .splice_rotate(bottom_row, top_row, count, || blank.clone());
        let (top, bottom) = (to_i64(buffer.scroll_top), to_i64(buffer.scroll_bottom));
        self.dirty_row_tracker.mark_range_dirty(top, bottom);
        true
    }

    /// Apply `edit` to every line of the scroll region and clear its wrapped flag (SL, SR,
    /// DECIC, DECDC). Upstream throws on a missing line, abandoning the rest of the handler.
    fn edit_region_cells(
        &mut self,
        n: usize,
        edit: impl Fn(&mut BufferLine, usize, usize, &CellData),
    ) -> bool {
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let null_cell = buffer.get_null_cell(Some(&erase));
        let (top, bottom, x) = (buffer.scroll_top, buffer.scroll_bottom, buffer.x);
        for y in top..=bottom {
            let row = buffer.ybase.saturating_add(y);
            let Some(line) = line_mut(buffer, row) else {
                return true;
            };
            edit(line, x, n, &null_cell);
            line.is_wrapped = false;
        }
        self.dirty_row_tracker
            .mark_range_dirty(to_i64(top), to_i64(bottom));
        true
    }

    /// CSI Ps SP @ (SL): move the content of the region `Ps` columns to the left. No effect
    /// outside the margins.
    pub fn scroll_left(&mut self, params: &Params) -> bool {
        if !self.cursor_in_scroll_region() {
            return true;
        }
        let n = to_usize(param_or(params, 0, 1));
        self.edit_region_cells(n, |line, _, n, fill| line.delete_cells(0, n, fill))
    }

    /// CSI Ps SP A (SR): move the content of the region `Ps` columns to the right. No effect
    /// outside the margins.
    pub fn scroll_right(&mut self, params: &Params) -> bool {
        if !self.cursor_in_scroll_region() {
            return true;
        }
        let n = to_usize(param_or(params, 0, 1));
        self.edit_region_cells(n, |line, _, n, fill| line.insert_cells(0, n, fill))
    }

    /// CSI Ps ' } (DECIC): insert `Ps` blank columns at the cursor in every line of the region.
    pub fn insert_columns(&mut self, params: &Params) -> bool {
        if !self.cursor_in_scroll_region() {
            return true;
        }
        let n = to_usize(param_or(params, 0, 1));
        self.edit_region_cells(n, |line, x, n, fill| line.insert_cells(x, n, fill))
    }

    /// CSI Ps ' ~ (DECDC): delete `Ps` columns at the cursor in every line of the region.
    pub fn delete_columns(&mut self, params: &Params) -> bool {
        if !self.cursor_in_scroll_region() {
            return true;
        }
        let n = to_usize(param_or(params, 0, 1));
        self.edit_region_cells(n, |line, x, n, fill| line.delete_cells(x, n, fill))
    }

    /// CSI Ps X (ECH): erase `Ps` characters from the cursor to the right.
    pub fn erase_chars(&mut self, params: &Params) -> bool {
        self.restrict_cursor(None);
        let n = to_usize(param_or(params, 0, 1));
        let erase = self.erase_attr_data();
        let buffer = self.services.buffer.buffer_mut();
        let null_cell = buffer.get_null_cell(Some(&erase));
        let (row, x, y) = (abs_y(buffer), buffer.x, buffer.y);
        if let Some(line) = line_mut(buffer, row) {
            line.replace_cells(x, x.saturating_add(n), &null_cell, false);
            self.dirty_row_tracker.mark_dirty(y);
        }
        true
    }

    /// CSI Ps b (REP): repeat the preceding graphic character (the cluster before the cursor)
    /// `Ps` times through `print`. Nothing if the last parser action was not a print.
    ///
    /// LUNETERM: upstream allocates `text.length * Ps` code points (up to 2^31 - 1 times the
    /// text). The repeated data is capped at [`MAX_REP_CODE_POINTS`]; a larger count prints that
    /// many fewer repetitions (the visible difference is only the cursor column and which
    /// repetition ends the output). Of those, only as many are printed as can still change the
    /// state (about two buffers' worth of lines; `rep_equivalent_repeats`), which
    /// leaves the same state as printing all of them. A unit that prints alike every time
    /// takes the line-fill path (`rep_fill`, `print_repeated`).
    pub fn repeat_preceding_character(&mut self, params: &Params) -> bool {
        let join_state = self.preceding_join_state;
        if join_state == 0 {
            return true;
        }
        // call print to insert the chars and handle correct wrapping
        let length = param_or(params, 0, 1);
        let ch_width = UnicodeService::extract_width(join_state);
        let buffer = self.services.buffer.buffer();
        let x = to_i64(buffer.x) - i64::from(ch_width);
        let Some(buffer_row) = buffer.lines.get(abs_y(buffer)) else {
            return true;
        };
        // a negative index reads `undefined` upstream: the empty string
        let text = usize::try_from(x)
            .map(|x| buffer_row.get_string(x))
            .unwrap_or_default();
        let unit: Vec<u32> = text.chars().map(u32::from).collect();
        let data: Vec<u32> = if unit.is_empty() {
            Vec::new()
        } else {
            // LUNETERM: upstream allocates unit.len() * length code points; capped
            let repeats = usize::try_from(length)
                .unwrap_or(usize::MAX)
                .min(MAX_REP_CODE_POINTS / unit.len());
            // LUNETERM: and cut to as many as still change the state (review S3)
            let repeats = self.rep_equivalent_repeats(&unit, repeats);
            // LUNETERM: (review S3 and S7, user decisions Q16 and Q17) a unit that prints the
            // same way every time fills whole lines instead of going through `print` per code
            // point
            if let Some(fill) = self.rep_fill(&unit).filter(|_| repeats > 0) {
                self.print_repeated(&fill, repeats);
                return true;
            }
            // LUNETERM: (plan M7 step 13, S7 residual 5) a narrow unit stuck at the last
            // column without wraparound in insert mode: its inserts batched
            if repeats > 0 && self.print_stuck_inserts(&unit, repeats) {
                return true;
            }
            unit.iter()
                .copied()
                .cycle()
                .take(unit.len() * repeats)
                .collect()
        };
        self.print(&data);
        true
    }

    /// LUNETERM: (review S3) the fewest repetitions of `unit` that leave the same state as
    /// `repeats` repetitions printed by REP. Printing past what the buffer holds only rewrites
    /// it, so once the printed lines have replaced every line the print can reach, the state
    /// repeats with a period of one line's worth of units. Returns `repeats` unchanged where
    /// that period is not proven (see the conditions below).
    ///
    /// Why the result is state-identical: printing `n` units is a prefix of printing
    /// `n + k * per_line`. With wraparound on, every repetition after the first places its
    /// first code point (non-joining, width `w`) and joins the rest onto it without moving the
    /// cursor (the checks below), so every wrap after the first starts a line at column 0 in
    /// the same state, and a fully printed line of `cols` cells is the same line (the old
    /// row's cells from the wrap column on are cleared, also in insert mode; for longer lines
    /// see below; for a widened unit on an odd number of columns, whose wraps also copy
    /// strings, see the last paragraph). How many lines the print can reach depends on where
    /// the cursor is (review S2 round 2):
    ///  - In or above the scroll region, a wrap at the scroll bottom scrolls, which (with
    ///    `scroll_top == 0`) feeds the scrollback until it is full, so the print can reach
    ///    every line up to `max_lines`; after `rows + max_lines + 3` wraps the buffer is full
    ///    (`ybase` and a user-scrolled `ydisp` no longer move) and every reachable line holds
    ///    the full line.
    ///  - Below the region (`y > scroll_bottom`), a wrap neither scrolls nor trims:
    ///    `print_wrap` scrolls only when the cursor reaches `scroll_bottom + 1`, so below the
    ///    region (where `y + 1 > scroll_bottom + 1`) it clamps the cursor to the last row
    ///    (`rows - 1`) with `ybase` and `ydisp` fixed and nothing enters the scrollback. The
    ///    print reaches only the rows from the cursor down to the last (`rows` over-counts the
    ///    descent); after `rows + 3` wraps the cursor stays on the last row and every reachable
    ///    line holds the full line. The scrollback is unreachable, so the floor drops
    ///    `max_lines` here — without that, a full scrollback made every one of `max_lines`
    ///    units a pass over the (possibly long) last row, the cost of review S2.
    ///
    /// Either way, `per_line` more units then complete the current line, scroll or rewrite it,
    /// and print the same partial line again. Lines outside the reach keep their content and
    /// markers. Damage, dirty rows and the screen reader text (capped per drain; with screen
    /// reader mode on, the `A11Y_TEXT_LIMIT` floor fills the cap in both) are the same; link
    /// markers on the reachable lines are created anew in both, so only marker ids differ.
    /// Without wraparound the cursor stays on its row: once it reaches the last column
    /// (`cols + 2` units), each single-code-point unit rewrites or skips the same cell, a
    /// period of one unit. A unit with joiners (review S7) reaches the last column within
    /// `cols / w + 1` units too; then a narrow unit rewrites the last cell with a fresh base
    /// and the same joiners, and a wide one skips its base and stacks its joiners onto the
    /// same cell (the one before the cursor), whose combined string stops growing at
    /// [`MAX_COMBINED_CODE_POINTS`] code points (at most that many units more, plus one for a
    /// cell that had no text); from then on every unit leaves the state as it found it, so
    /// `cols + MAX_COMBINED_CODE_POINTS + 3` units suffice.
    ///
    /// Insert mode on a line longer than `cols` (left by a column shrink without reflow; code
    /// review F9): each insert also shifts the line's hidden tail `[cols, len)`, which keeps
    /// changing after the visible cells repeat, so the floor waits until the tail holds only
    /// cells the print made. Two facts carry the argument. (1) A unit's insert, the fix-ups
    /// (`insert_cells`' at `pos - 1` and at the line end, `print`'s at `cols - 1`) and the
    /// wrap's clear of `[wrap column, cols)` act on a cell from its own value and position
    /// only, and every cell right of the insert moves right by the inserted width: content
    /// that was at column `q` before `s` inserted cells is at `q + s`, or gone past `len`.
    /// (2) The cells inserted, written and cleared are the same in every repetition. So once
    /// every cell the row held has been pushed past `len`, its cells no longer depend on what
    /// it held, and the same repetitions applied again give the same cells (a fixed point).
    /// One more round of the same writes then fixes the side tables too, whose entries change
    /// only when a flagged cell is written to their key — except where a widened unit's wrap
    /// copies strings (an odd number of columns, last paragraph), whose term the floor adds on
    /// top of this one.
    ///  - Without wraparound, a narrow unit inserts once per repetition at the cursor (stuck
    ///    at `cols - 1` past the last column), so the row's content from `min(x, cols - 1)` on
    ///    has left the line after `len - min(x, cols - 1)` units and `len + 1` units suffice:
    ///    the floor gets `len - cols` more (`len + 2` in all for one code point). A stuck wide
    ///    unit is skipped before its insert, so its tail stops changing once it is stuck; the
    ///    extra units change nothing there.
    ///  - With wraparound, a line is printed again without scrolling only when the cursor is
    ///    below the scroll region: every wrap then lands on the last row (`y` stops at
    ///    `rows - 1`), and each full pass over it inserts `per_line * w` cells. Lines a wrap
    ///    reaches by scrolling are blank lines of `cols` cells, and a line passed once is the
    ///    same prefix in both prints. The last row needs `ceil(len / (per_line * w)) + 1` full
    ///    passes, at most `ceil(excess / (per_line * w)) + 3` (`per_line * w >= cols - 1` and
    ///    `cols >= 2`, so `ceil(cols / (per_line * w)) <= 2`). Of the floor's `rows + 3` base
    ///    lines (review S2 round 2 dropped the scrollback, unreachable below the region) at
    ///    most `rows - 1` are the descent and one pass is partial, which leaves at least three
    ///    full passes; the floor gets `ceil(excess / (per_line * w)) + 1` more lines of units.
    ///
    /// The extra units print as before. Without wraparound they go through `print`, one
    /// `O(len)` insert each, so such a REP costs up to `O(len * (len - cols))` (the longest
    /// line a resize left behind; plan M7 step 13 batches the stuck inserts); with wraparound
    /// they take the line fill, `O(len)` per pass, and below the region the line fill takes the
    /// passes that repeat a settled one at once (M7 fix round 3, see
    /// [`InputHandler::print_repeated`]), so the floor's size no longer sets the cost there.
    ///
    /// A joiner that widens a narrow base to two cells (M7 fix round 1, review S1: VS16 under
    /// the `11-vs16` provider) makes the unit 2 cells wide (`w = 2`) and moves the cursor.
    /// With wraparound each repetition still fills its own two cells: a base that lands on the
    /// last column is carried to the next line by the joiner's wrap, which clears that column
    /// as a wide base's wrap does, so lines start at column 0 in the same state and the
    /// argument above holds with `w = 2`. In insert mode a repetition inserts the base's one
    /// cell (its stub overwrites the next), so below the region a pass inserts `per_line`
    /// cells; `ceil(cols / per_line) <= 3` there, hence one more line of units. Without
    /// wraparound a stuck repetition writes its base (and any joiner before the widening one)
    /// into the last cell, skips the widening joiner, which does not fit, and leaves the
    /// cursor on the last column, so a later joiner stacks onto the cell left of it as a stuck
    /// wide unit's joiners do: the floor for units with joiners covers it.
    ///
    /// On an odd number of columns that widened unit's wraps also copy strings (M7 fix round 4,
    /// code review F6), and the argument above does not hold as it stands: a line is not "the
    /// same line" once printed in full, and the reachable lines do not hold the full line after
    /// the base floor. With wraparound on, the repetition that starts at `per_line * w =
    /// cols - 1` puts its base on the last column and its joiner wraps it (`print_wrap` with
    /// `old_width = 1`), which runs upstream's `copyCellsFrom(oldRow, cols - 1, 0, 1)`: besides
    /// the base, it copies every combined string of the old row at a key `k >= cols - 1` to key
    /// `k - (cols - 1)` of the new row (the FIXME copy), stale entries (cells no longer
    /// flagged) and keys past the old row's end included, over the new row's entries, flagged
    /// or not. Below the region the new row is the same row; at the scroll bottom it is a fresh
    /// line, which receives the keys past its end too and passes them on to the next. Without
    /// insert mode nothing else moves a string at a key `>= cols - 1` (a pass writes strings at
    /// the heads left of `cols - 1` only, and the base it writes on the last column carries no
    /// string), so after `m` such wraps every key `j` holds the string of the largest key among
    /// `j, j + (cols - 1), ..., j + m (cols - 1)` that held one: the strings settle once `m`
    /// reaches `ceil((K + 1) / (cols - 1))`, where `K` is the largest key on the rows the print
    /// reaches (from the cursor's row down to the last; a fresh line's keys are below the
    /// row's it came from). One more line of units, and the argument above holds from there.
    /// In insert mode the shifts also carry flagged strings right; there the floor takes `K`
    /// at least each row's last cell (the shifts write strings up to it) and adds the same
    /// term on top of the insert passes', so the copy's lines start once the cells have
    /// settled. That insert-mode bound is the sum of the two settling times; it is checked by
    /// the tests (`rep_widened_unit_at_odd_widths_over_rows_of_strings…`, the matrix, the
    /// seeded differential) rather than proven tight. The floor gets `K / (cols - 1) + 2` more
    /// lines (`ceil((K + 1) / (cols - 1)) + 1`), read in `O(log n)` per reachable row
    /// ([`BufferLine::last_combined_key`]). Cost: below the region with insert mode the line
    /// fill takes the settled passes at once, so only the passes before the strings settle
    /// cost (`O(len)` each, `O(len^2 / cols)` in all on a long row of strings; M7 fix round 7:
    /// replayed in dense arrays for every such unit, M7 fix round 8 included those with joiners
    /// before the widening one, still `O(len^2 / cols)`); without insert
    /// mode (M7 fix round 5) the line fill takes the string copies of all but the last whole
    /// pass at once (`O(K + runs)`, [`InputHandler::print_repeated`]), so the floor's lines
    /// cost one batch and a pass or two there, not a pass each.
    fn rep_equivalent_repeats(&self, unit: &[u32], repeats: usize) -> usize {
        let services = &self.services;
        let unicode = &services.unicode;
        let charset = services.charset.charset;
        let cols = services.buffer.cols;
        let rows = services.buffer.rows;
        let max_lines = services.buffer.buffer().lines.max_length();

        // The join state at the end of the first repetition, then the steady state check on a
        // second one.
        let mut preceding = self.preceding_join_state;
        for &code in unit {
            preceding = unicode.char_properties(charset_replacement(charset, code), preceding);
        }
        let after_first = preceding;
        let (mut width, mut base_width) = (0, 0);
        for (i, &code) in unit.iter().enumerate() {
            let info = unicode.char_properties(charset_replacement(charset, code), preceding);
            let joins = UnicodeService::extract_should_join(info);
            let w = UnicodeService::extract_width(info);
            if i == 0 {
                // the base: never joins, one or two cells. (review S3 round 2) a unit whose
                // base joins can still be cut, but only when every code point joins the cell
                // before the cursor without moving it, so its own helper decides
                if joins {
                    return self.rep_joining_base_repeats(unit, repeats);
                }
                if w == 0 || w > 2 {
                    return repeats;
                }
                (width, base_width) = (w, w);
            } else {
                // a joining character that does not move the cursor, or (M7 fix round 1) one
                // that widens a narrow base to two cells
                let held = UnicodeService::extract_width(preceding);
                if !joins || (w != held && (held != 1 || w != 2)) {
                    return repeats;
                }
                width = width.max(w);
            }
            preceding = info;
        }
        // (the unit fits a line)
        if preceding != after_first || width == 0 || to_usize(i64::from(width)) > cols {
            return repeats;
        }

        // LUNETERM: (code review F9) in insert mode, how far a line reaches past `cols`: the
        // hidden tail every insert shifts (see the doc comment)
        let insert_mode = services.core.modes.insert_mode;
        let buffer = services.buffer.buffer();
        let excess = |row: usize| {
            buffer
                .lines
                .get(row)
                .map_or(0, |line| line.len().saturating_sub(cols))
        };
        let (period, floor) = if services.core.dec_private_modes.wraparound {
            let w = to_usize(i64::from(width));
            let per_line = cols / w;
            // LUNETERM: (review S2 round 2) below a scroll region a wrap neither scrolls nor
            // trims (`print_wrap` scrolls only at `scroll_bottom + 1`; below the region the
            // cursor is clamped to the last row with `ybase`/`ydisp` fixed, see the doc
            // comment), so no line enters the scrollback: the print reaches only the rows from
            // the cursor down to the last. Elsewhere a wrap at the scroll bottom can scroll, so
            // the print can reach every line up to the full scrollback (`max_lines`).
            let below_region = buffer.y > buffer.scroll_bottom;
            let mut lines = if below_region {
                rows.saturating_add(3)
            } else {
                rows.saturating_add(max_lines).saturating_add(3)
            };
            if insert_mode && below_region {
                // every wrap rewrites the last row, shifting it by `per_line * inserted` cells
                // (M7 fix round 1: a widened unit inserts its base's one cell, and one more
                // line covers `ceil(cols / (per_line * inserted)) <= 3`)
                let inserted = to_usize(i64::from(base_width));
                let last = buffer.ybase.saturating_add(rows).saturating_sub(1);
                let passes = excess(last).div_ceil(per_line.saturating_mul(inserted).max(1));
                lines = lines
                    .saturating_add(passes)
                    .saturating_add(1)
                    .saturating_add(usize::from(inserted < w));
            }
            // LUNETERM: (M7 fix round 4, code review F6) a widened unit whose joiner wraps its
            // base (an odd number of columns): every such wrap copies the old row's strings
            // from `cols - 1` on to the new row, `cols - 1` to the left (see the doc comment)
            let base = to_usize(i64::from(base_width));
            if base < w && per_line.saturating_mul(w).saturating_add(base) <= cols {
                if let Some(reach) = self.copy_reach() {
                    // `ceil((reach + 1) / (cols - 1)) + 1` lines of units
                    lines = lines
                        .saturating_add(reach / cols.saturating_sub(1).max(1))
                        .saturating_add(2);
                }
            }
            (per_line, per_line.saturating_mul(lines))
        } else {
            let floor = if unit.len() == 1 {
                cols.saturating_add(2)
            } else {
                // LUNETERM: (review S7) see the doc comment: the joiners of a stuck wide unit
                // stack onto one cell until its combined string is full
                cols.saturating_add(MAX_COMBINED_CODE_POINTS)
                    .saturating_add(3)
            };
            let excess = if insert_mode {
                excess(abs_y(buffer))
            } else {
                0
            };
            (1, floor.saturating_add(excess))
        };
        // the screen reader text of the cut REP must reach the per-drain cap as the uncut does
        let floor = if services.options.options().screen_reader_mode {
            floor.max(A11Y_TEXT_LIMIT)
        } else {
            floor
        };
        if period == 0 || repeats <= floor {
            return repeats;
        }
        floor + (repeats - floor) % period
    }

    /// LUNETERM: (M7 fix round 4, code review F6) how far a widened unit's joiner wraps can
    /// copy strings from (see [`InputHandler::rep_equivalent_repeats`]): the largest combined
    /// key over the rows from the cursor's down to the buffer's last, in insert mode at least
    /// each row's last cell (an insert writes the strings of the cells it shifts, up to the
    /// line's end); `None` where no such row holds one (and, in insert mode, none exists).
    /// `O(rows × log n)` ([`BufferLine::last_combined_key`]).
    fn copy_reach(&self) -> Option<usize> {
        let insert_mode = self.services.core.modes.insert_mode;
        let rows = self.services.buffer.rows;
        let buffer = self.services.buffer.buffer();
        let first = abs_y(buffer);
        let last = buffer.ybase.saturating_add(rows).saturating_sub(1);
        (first..=last)
            .filter_map(|row| buffer.lines.get(row))
            .filter_map(|line| {
                let key = line.last_combined_key();
                match insert_mode {
                    true => Some(key.unwrap_or(0).max(line.len().saturating_sub(1))),
                    false => key,
                }
            })
            .max()
    }

    /// LUNETERM: (review S3 round 2) the fewest repetitions of `unit` that leave the state
    /// `repeats` of them leave, for a unit whose base (its first code point) joins. REP reads
    /// such a unit when a zero-width code point was stored alone in a cell and a wide character
    /// was then skipped at the last column without wraparound (the cursor is left on a cell
    /// whose text begins with a joiner). Uncut (returns `repeats`) unless every code point
    /// joins the cell before the cursor without moving it — its joined width equals the width
    /// it joins, so `print` appends it to that cell ([`InputHandler::print_inner`]'s
    /// `should_join` branch) and writes no stub and does not advance the cursor — and the join
    /// state after one repetition equals the state entering it. Then every repetition appends
    /// the same code points to that one cell, whose combined string stops growing at
    /// [`MAX_COMBINED_CODE_POINTS`] code points ([`BufferLine::add_codepoint_to_cell`] is a
    /// no-op once full); from then on each repetition is a complete no-op (no cursor move, no
    /// new cell, no stub, the same dirty row, the same link line), so the state is fixed with a
    /// period of one unit. `MAX_COMBINED_CODE_POINTS + 2` units leave it full (one repetition
    /// appends at least one code point, plus a margin). The screen reader text is queued per
    /// code point up to the per-drain cap, so the `A11Y_TEXT_LIMIT` floor still applies. The
    /// conditions are read in the steady state (entering the second repetition), where the cell
    /// before the cursor already holds the growing string, not the first (whose cursor may
    /// arrive carrying a wide character's width).
    fn rep_joining_base_repeats(&self, unit: &[u32], repeats: usize) -> usize {
        let services = &self.services;
        let unicode = &services.unicode;
        let charset = services.charset.charset;
        // the join state entering the second repetition (after one full unit)
        let mut preceding = self.preceding_join_state;
        for &code in unit {
            preceding = unicode.char_properties(charset_replacement(charset, code), preceding);
        }
        let after_first = preceding;
        // every code point joins the cell before the cursor keeping the width it joins, so
        // `print` moves the cursor for none of them
        for &code in unit {
            let info = unicode.char_properties(charset_replacement(charset, code), preceding);
            let held = UnicodeService::extract_width(preceding);
            if !UnicodeService::extract_should_join(info)
                || UnicodeService::extract_width(info) != held
            {
                return repeats;
            }
            preceding = info;
        }
        // periodic: the second repetition leaves the state the first did
        if preceding != after_first {
            return repeats;
        }
        let mut floor = MAX_COMBINED_CODE_POINTS.saturating_add(2);
        if services.options.options().screen_reader_mode {
            floor = floor.max(A11Y_TEXT_LIMIT);
        }
        repeats.min(floor)
    }

    /// LUNETERM: (review S3, S7) whether REP of `unit` (the repeated cell's code points) can
    /// take [`InputHandler::print_repeated`], and how it prints. It can when every repetition
    /// prints alike: with the charset applied, the code points get the same properties in
    /// every repetition (the first one's from the current join state equal the second one's
    /// from the state the first leaves, so by induction all are equal); the first never joins
    /// and is one or two cells wide (no wider than a line); every other joins without changing
    /// the width it joins, except that one may widen a narrow base to two cells (M7 fix round
    /// 1: VS16 under the `11-vs16` provider); and wraparound is on. Insert mode, an open link
    /// and screen reader mode are handled there.
    fn rep_fill(&self, unit: &[u32]) -> Option<RepFill> {
        if !self.services.core.dec_private_modes.wraparound {
            return None;
        }
        self.rep_alike(unit)
    }

    /// LUNETERM: (review S3, S7) how `unit` prints when every repetition prints alike: the
    /// conditions of [`InputHandler::rep_fill`] but wraparound.
    fn rep_alike(&self, unit: &[u32]) -> Option<RepFill> {
        let services = &self.services;
        let charset = services.charset.charset;
        let unicode = &services.unicode;
        let codes: Vec<u32> = unit
            .iter()
            .map(|&code| charset_replacement(charset, code))
            .collect();
        let properties = |mut preceding: u32| -> Vec<u32> {
            codes
                .iter()
                .map(|&code| {
                    preceding = unicode.char_properties(code, preceding);
                    preceding
                })
                .collect()
        };
        let first = properties(self.preceding_join_state);
        let (&base, joiners) = first.split_first()?;
        let end_state = *first.last()?;
        let base_width = UnicodeService::extract_width(base);
        if UnicodeService::extract_should_join(base)
            || base_width == 0
            || base_width > 2
            || properties(end_state) != first
        {
            return None;
        }
        // every joiner keeps the width it joins (`print` neither moves the cursor nor writes a
        // stub), or (M7 fix round 1) widens a narrow base to two cells once (`print` writes a
        // stub and moves the cursor one column, or wraps the base to the next line)
        let mut widths = vec![base_width];
        let mut widens_at = None;
        let mut width = base_width;
        for (i, &info) in joiners.iter().enumerate() {
            let joined = UnicodeService::extract_width(info);
            if !UnicodeService::extract_should_join(info) {
                return None;
            }
            if joined != width {
                if width != 1 || joined != 2 {
                    return None;
                }
                widens_at = Some(i + 1);
                width = 2;
            }
            widths.push(joined);
        }
        if to_usize(i64::from(width)) > services.buffer.cols {
            return None;
        }
        Some(RepFill {
            codes,
            width,
            base_width,
            widths,
            widens_at,
            end_state,
        })
    }

    /// LUNETERM: (review S3, S7) `print` of `count` repetitions of `fill.codes` under the
    /// conditions of [`InputHandler::rep_fill`], line by line: the same cells, wraps
    /// (`print_wrap` with no joined width), dirty rows, join state, insert-mode shifts, link
    /// lines and screen reader text as `print`, without `print`'s per code point work
    /// (`rep_cut_to_the_buffer_leaves_the_state_of_the_uncapped_print` and
    /// `rep_fill_leaves_the_state_of_print_over_existing_content` compare the two). How each
    /// per code point effect of `print` is batched:
    ///  - Cells: every repetition writes the same head cell (the base and its joiners, built
    ///    once on a scratch line by the calls `print` makes) and, when wide, a stub.
    ///  - Cells are written with [`BufferLine::set_cells_repeatedly`]: one pass over the data
    ///    and one sorted batch per side table, where `print` makes a tree insert per cell
    ///    with extended attributes (review S7 round 4).
    ///  - Insert mode: one [`BufferLine::insert_cells_repeatedly`] per line, the inserts of all
    ///    the line's repetitions and `print`'s own fix-up of a wide cell at `cols - 1` after
    ///    each (it fires only on a line longer than `cols`, left by a column shrink without
    ///    reflow). The written cells are left of every later insert and of `cols - 1` while a
    ///    later repetition follows (each repetition ends at or before `cols`), and the
    ///    `pos - 1` fix-up of a later insert finds the head or stub written there narrow in
    ///    `print` and the narrow fill without the writes, so writing them after the inserts
    ///    changes nothing. A line shorter than `cols` takes the per-repetition calls.
    ///  - Link: `print` adds the cursor's line before each code point, so a repetition that
    ///    wraps adds the line it leaves. A second add of the same line with only cell writes in
    ///    between finds the marker the first one cached and changes nothing (the first add may
    ///    evict other links; forwarding their disposals later finds them gone either way), so
    ///    one add per line and wrap is the same.
    ///  - Screen reader: the only events `print` fires are its characters' text, which the
    ///    queue stops taking at its per-drain cap; they are queued up front in order until the
    ///    queue is full.
    ///  - (M7 fix round 1) A fresh line's combined strings are shared with the next fresh line
    ///    ([`BufferLine::set_cells_repeatedly_shared`]): a wide cluster's line holds one per
    ///    cell.
    ///  - (M7 fix round 1, review S1) A joiner that widens a narrow base prints the unit as a
    ///    wide one (the head is built with each code point's own width), except in two places.
    ///    The repetition whose base lands on the last column wraps at the joiner: `print`
    ///    writes the base there and `print_wrap` carries it to the next line (with the old
    ///    row's combined strings from that column on, upstream's copy), so that repetition goes
    ///    through `print`'s own steps ([`InputHandler::print_inner`], without the end fix-up,
    ///    which `print` makes only after its last code point, and without the screen reader
    ///    text queued here); its link adds are `print`'s. And in insert mode a repetition
    ///    inserts one cell, the base's, and its stub overwrites the cell the insert shifted:
    ///    the line's inserts are one [`BufferLine::insert_cell_strided`], written over after.
    ///    That leaves `print`'s state: the writes are left of every later insert, of
    ///    `cols - 1` and of the line's end, and a later insert's `pos - 1` fix-up, which finds
    ///    the narrow stub in `print`, may rewrite that cell here, which the stub then
    ///    overwrites, with the fill's extended entry, which is the stub's (the fill is the null
    ///    cell carrying the current attributes); combined strings it does not touch.
    ///  - (M7 fix round 3, review S2) Insert mode on the last row below the scroll region: every
    ///    wrap lands on that row again, so from the cursor at `span = per_line × w` (where a
    ///    pass ends) each pass of `per_line` units (a wrap and a line's worth; for a widened unit
    ///    on an odd number of columns, the unit its joiner wraps and the rest of the line) is the
    ///    same function of the row. When a pass leaves what it reads as it found it, every later
    ///    pass starts from the same state and does the same, so the rest of the whole passes are
    ///    taken at once and the last part pass prints as usual. What a pass reads: with the row
    ///    no longer than `cols` plus the `pushed = per_line × inserted` cells a pass pushes over
    ///    `cols` (or where the joiner's wrap copies the old row's strings from `cols - 1` on,
    ///    upstream's copy, which reads the hidden tail and keys past the line), the whole line
    ///    and every entry past it: then the skipped passes change nothing. Otherwise only the
    ///    row's first `cols` cells: the shifts move cells right, the fix-ups act at `x - 1`,
    ///    `cols - 1` and `len - 1 >= cols + pushed`, and the wraps clear and the writes cover
    ///    cells left of `cols`, so a pass's effect there and the cells it pushes over `cols`
    ///    depend on those `cols` cells only; the cells `[cols, cols + pushed)` are the ones the
    ///    last pass pushed (the line-end fix-up does not reach them), every later pass pushes
    ///    them again `inserted` at a time, and the skipped passes are that one
    ///    [`BufferLine::insert_block_repeatedly`] on the tail. Their link adds find the marker
    ///    the last pass cached (as above), their wraps only mark the row wrapped again, and their
    ///    screen reader text is queued up front. Comparing a pass's state costs `O(cols)` (the
    ///    whole line when it is short), the batch `O(len)`: a REP below the region makes a few
    ///    passes over the row, where it made one per line of the cut floor (`rows + 3` and one
    ///    per `pushed` cells of tail: `O(len^2 / cols)` on a long row). Not for a widened unit on
    ///    an odd number of columns on a row longer than `cols`: its passes settle only once the
    ///    tail has, and once the strings the joiner's wrap copies have (M7 fix round 4, code
    ///    review F6; see [`InputHandler::rep_equivalent_repeats`]), which still takes one
    ///    `O(len)` pass per `pushed` cells of tail and per `cols - 1` of the row's last key.
    ///  - (M7 fix round 5, code review F6 residual) Without insert mode, on the last row below
    ///    the scroll region, that widened unit's passes from `span` differ only in the strings
    ///    the joiner's wrap copies; there `p` whole passes leave the row's state of the first
    ///    `p - 1` passes' copies taken in closed form and one real pass
    ///    ([`InputHandler::copy_passes_at_once`] has the proof), `O(K + runs)` where they cost
    ///    `O(len)` each.
    ///  - (M7 fix round 5) At the scroll bottom, that widened unit's wraps chain the strings
    ///    from line to line (each copy brings the old row's keys `>= cols - 1` onto the fresh
    ///    line); the first ones skip that copy where the REP's own later scrolls discard the
    ///    lines it would land on ([`InputHandler::chain_copies_to_skip`] has the bound and the
    ///    proof). Its repetition that wraps goes through [`InputHandler::print_widened_wrap`]
    ///    (`print`'s steps, the head shared by the lines that hold no other string), and the
    ///    line fill's template then takes the line's strings in `O(1)`: a line costs about what
    ///    a line of plain cells does.
    ///  - (M7 fix round 7, security S4) In insert mode on the last row below the scroll region,
    ///    that widened unit's whole passes over a row longer than `cols` (each `O(len)`, about
    ///    `(len - cols) / per_line + K / (cols - 1)` of them before the row repeats) are replayed
    ///    in dense arrays (VS16, a keycap; M7 fix round 8: and units with joiners before the
    ///    widening one, `a U+0301 U+FE0F`; [`InputHandler::replay_wrap_passes`],
    ///    [`BufferLine::replay_wrap_passes`]): the same writes as the passes, with the row
    ///    compared after passes 1, 2, 4, 8, … to stop once it repeats (with an odd `per_line`
    ///    the pushed cells alternate, period 2). Still `O(len^2 / cols)` per REP that runs the
    ///    transient, at a lower constant; rows the replay declines (keys far past the end) keep
    ///    the passes and the batch above.
    fn print_repeated(&mut self, fill: &RepFill, count: usize) {
        #[cfg(test)]
        {
            self.rep_fills += 1;
        }
        let cols = self.services.buffer.cols;
        let insert_mode = self.services.core.modes.insert_mode;
        let cur_attr = self.cur_attr_data;
        let link_id = cur_attr.extended.url_id();
        let w = to_usize(i64::from(fill.width));
        // the cells a repetition inserts in insert mode: the base's (a widening joiner writes
        // its stub over the next cell)
        let inserted = to_usize(i64::from(fill.base_width));
        let null_cell = self.services.buffer.buffer().get_null_cell(Some(&cur_attr));

        if self.services.options.options().screen_reader_mode {
            let events = &mut self.events;
            'voice: for _ in 0..count {
                for &code in &fill.codes {
                    if events.a11y_full() {
                        break 'voice;
                    }
                    events.a11y_char(code);
                }
            }
        }

        let (head, stub) = rep_head(fill, &cur_attr);

        {
            let buffer = self.services.buffer.buffer_mut();
            let row = abs_y(buffer);
            let x = buffer.x;
            let Some(buffer_row) = line_mut(buffer, row) else {
                return;
            };
            // handle wide chars: reset start_cell-1 if we would overwrite the second cell of a
            // wide char
            if x != 0 && buffer_row.get_width(x - 1) == 2 {
                buffer_row.set_cell_from_codepoint(x - 1, 0, 1, &cur_attr);
            }
            let y = buffer.y;
            self.dirty_row_tracker.mark_dirty(y);
        }

        // (M7 fix round 1) the combined strings of a line filled afresh, shared by the next
        let mut template = None;
        // (M7 fix round 5) the head string a widened unit's joiner wrap leaves at column 0,
        // shared by the lines it lands on
        let mut joined = None;
        // (M7 fix round 3, review S2) whole passes over the last row below the scroll region:
        // from the cursor at `span` (where a pass ends), `per_line` units (a wrap and a line's
        // worth; or, for a widened unit on an odd number of columns, the unit its joiner wraps
        // and the rest of the line) pushing `pushed` cells over `cols`
        let per_line = cols / w;
        let span = per_line * w;
        let pushed = per_line * inserted;
        // (a pass whose wrap copies the old row's strings from `cols - 1` on, upstream's copy,
        // reads the hidden tail's: only the whole line and the entries past it settle it)
        let wrap_copies = fill.widens_at.is_some() && span + inserted <= cols;
        // where the last pass started: the units left and what it read
        let mut last_pass: Option<(usize, RangeState)> = None;
        // (M7 fix round 5) the joiner wraps at the scroll bottom, from the first, whose
        // strings' copy lands on a line this REP's own scrolls discard
        let mut chain_skips = match wrap_copies {
            true => self.chain_copies_to_skip(count, w, inserted),
            false => 0,
        };
        #[cfg(test)]
        {
            self.rep_chain_skips += u64::from(chain_skips > 0);
        }
        let mut left = count;
        while left > 0 {
            let x = self.services.buffer.buffer().x;
            if !insert_mode && wrap_copies && x == span && self.copy_passes_at_once(left, per_line)
            {
                // (M7 fix round 5) all but the last of the whole passes left, at once
                left -= (left / per_line - 1) * per_line;
                #[cfg(test)]
                {
                    self.rep_copy_batches += 1;
                }
            }
            if insert_mode && wrap_copies && x == span && left >= per_line && per_line > 0 {
                // (M7 fix round 7) the whole passes left, replayed in dense arrays
                if let Some(_passes) =
                    self.replay_wrap_passes(fill, &head, &stub, &null_cell, left / per_line)
                {
                    left -= (left / per_line) * per_line;
                    last_pass = None;
                    #[cfg(test)]
                    {
                        self.rep_wrap_replays += 1;
                        self.rep_wrap_replay_passes += _passes as u64;
                    }
                    continue;
                }
            }
            if insert_mode && per_line > 0 && x == span {
                // once a pass leaves what it reads as it found it, every later pass does the
                // same, so the rest of the whole passes are taken at once
                let rows = self.services.buffer.rows;
                let buffer = self.services.buffer.buffer_mut();
                let below_last = buffer.y + 1 == rows && buffer.y > buffer.scroll_bottom;
                let row = abs_y(buffer);
                let line = line_mut(buffer, row)
                    .filter(|line| below_last && left >= per_line && line.len() >= cols);
                match line {
                    Some(line) => {
                        let tail = line.len() - cols;
                        // (the row's first `cols` cells, or the whole line when the passes
                        // reach all of it)
                        let whole = wrap_copies || tail <= pushed;
                        let state = match whole {
                            true => line.line_state(),
                            false => line.range_state(0, cols),
                        };
                        let repeated = last_pass.as_ref().is_some_and(|(units, last)| {
                            units.checked_sub(left) == Some(per_line) && *last == state
                        });
                        if repeated {
                            let periods = left / per_line;
                            if !whole {
                                line.insert_block_repeatedly(
                                    cols, pushed, inserted, periods, &null_cell,
                                );
                            }
                            left -= periods * per_line;
                            last_pass = None;
                            #[cfg(test)]
                            {
                                self.rep_cycle_batches += 1;
                            }
                            continue;
                        }
                        last_pass = Some((left, state));
                    }
                    None => last_pass = None,
                }
            } else if last_pass
                .as_ref()
                .is_some_and(|(units, _)| units.saturating_sub(left) >= per_line)
            {
                // (not within a pass from `span`)
                last_pass = None;
            }
            if fill.widens_at.is_some() && x + inserted <= cols && x + w > cols {
                // (M7 fix round 1) the base fits the line and its widening joiner wraps it to
                // the next one: `print`'s own steps for this repetition
                #[cfg(test)]
                {
                    let rows = self.services.buffer.rows;
                    let buffer = self.services.buffer.buffer();
                    if buffer.y + 1 == rows && buffer.y > buffer.scroll_bottom {
                        self.rep_joiner_wraps += 1;
                    }
                }
                let buffer = self.services.buffer.buffer();
                let skip = chain_skips > 0 && buffer.y == buffer.scroll_bottom;
                chain_skips -= usize::from(skip);
                if !self.print_widened_wrap(fill, &head, &mut joined, !skip) {
                    return;
                }
                left -= 1;
                continue;
            }
            let InputHandler {
                services,
                erase_attr_data_internal,
                dirty_row_tracker,
                damage,
                ..
            } = self;
            let mut row = abs_y(services.buffer.buffer());
            // goto next line if ch would overflow
            let wraps = x.saturating_add(w) > cols;
            if link_id != 0 {
                // the first code point's line: the line it leaves when it wraps
                add_cursor_line_to_link(services, link_id);
            }
            if wraps {
                match print_wrap(
                    services,
                    dirty_row_tracker,
                    damage,
                    erase_attr_data_internal,
                    &cur_attr,
                    row,
                    0,
                    true,
                ) {
                    Some(new_row) => row = new_row,
                    // upstream throws on the missing line
                    None => return,
                }
            }
            // as many as fit on the line: at least one (after a wrap x is 0 and w <= cols)
            let fit = (cols.saturating_sub(services.buffer.buffer().x) / w).min(left);
            if link_id != 0 && wraps && (fit > 1 || fill.codes.len() > 1) {
                // the other code points' line, after the wrap
                add_cursor_line_to_link(services, link_id);
            }
            let Buffer { lines, x, .. } = services.buffer.buffer_mut();
            let Some(buffer_row) = lines.get_mut(row) else {
                return;
            };
            if insert_mode && buffer_row.len() < cols {
                // a line shorter than `cols` (the buffer widens every line with the columns):
                // `print`'s calls per repetition, since an insert past the line's end wraps
                // around to its start
                for _ in 0..fit {
                    buffer_row.insert_cells(*x, inserted, &null_cell);
                    if buffer_row.get_width(cols.saturating_sub(1)) == 2 {
                        buffer_row.set_cell_from_codepoint(
                            cols.saturating_sub(1),
                            NULL_CELL_CODE,
                            NULL_CELL_WIDTH,
                            &cur_attr,
                        );
                    }
                    buffer_row.set_cells_repeatedly(*x, w, 1, &head, &stub);
                    *x += w;
                }
                left -= fit;
                continue;
            }
            if insert_mode && inserted == w {
                // `print`'s insert-mode shifts and fix-ups of the line's repetitions, and the
                // writes (M7 fix round 3: in one call, which skips what the writes hide)
                buffer_row.insert_cells_repeatedly_then_set(
                    *x,
                    w,
                    fit,
                    &null_cell,
                    cols.saturating_sub(1),
                    &cur_attr,
                    (&head, &stub),
                    &mut template,
                );
                *x += fit * w;
                left -= fit;
                continue;
            } else if insert_mode {
                // (M7 fix round 1) one cell per repetition, `w` apart, and the writes
                buffer_row.insert_cell_strided_then_set(
                    *x,
                    w,
                    fit,
                    &null_cell,
                    cols.saturating_sub(1),
                    &cur_attr,
                    (&head, &stub),
                    &mut template,
                );
                *x += fit * w;
                left -= fit;
                continue;
            }
            buffer_row.set_cells_repeatedly_shared(*x, w, fit, &head, &stub, &mut template);
            *x += fit * w;
            left -= fit;
        }

        self.preceding_join_state = fill.end_state;

        // handle wide chars: reset cell to the right if it is second cell of a wide char
        let buffer = self.services.buffer.buffer_mut();
        let row = abs_y(buffer);
        let x = buffer.x;
        if x < cols {
            if let Some(buffer_row) = line_mut(buffer, row) {
                if buffer_row.get_width(x) == 0 && buffer_row.has_content(x) == 0 {
                    buffer_row.set_cell_from_codepoint(x, 0, 1, &cur_attr);
                }
            }
        }
        let y = buffer.y;
        self.dirty_row_tracker.mark_dirty(y);
    }

    /// LUNETERM: (M7 fix round 5, code review F6 residual) `print_inner(&fill.codes, false,
    /// false, strings)` for [`InputHandler::print_repeated`]'s repetition whose base lands on the line
    /// and whose widening joiner wraps it (`x + 1 <= cols < x + 2`), for a unit whose first
    /// joiner is the widening one (a base of one cell; VS16, a keycap); any other unit takes
    /// `print_inner`. The same steps in the same order, from what `rep_alike` read of the unit
    /// (every repetition prints alike, so each code point's width and join are known): the
    /// start fix-up and dirty row; the base (link add, insert-mode shift and edge fix-up,
    /// write); the widening joiner (link add on the old row, `print_wrap` with `old_width` 1,
    /// the joiner added to column 0, the stub at 1); each later joiner (link add on the new
    /// row, added to column 0, which the stub puts two cells back); the dirty row. One step
    /// differs in how, not in what: where the new row holds no string and its first cell the
    /// base alone (the copy brought none), the joiners' `add_codepoint_to_cell` calls are
    /// [`BufferLine::join_head_shared`], which leaves the same cell and strings from `head` (the
    /// cell every repetition writes, built by the same calls) with one map shared by every such
    /// line instead of a string and a map made and then dropped per line (the line fill's
    /// template then takes it in `O(1)`). `rep_widened_wrap_equals_print_inner` compares the
    /// two over rows the wrap scrolls in, the same row below the region and rows reached by
    /// descent. Returns false where `print_inner` would (a missing line).
    fn print_widened_wrap(
        &mut self,
        fill: &RepFill,
        head: &CellData,
        joined: &mut Option<JoinedHead>,
        strings: bool,
    ) -> bool {
        if fill.widens_at != Some(1) || fill.base_width != 1 || fill.codes.len() < 2 {
            return self.print_inner(&fill.codes, false, false, strings);
        }
        let InputHandler {
            services,
            cur_attr_data,
            erase_attr_data_internal,
            dirty_row_tracker,
            preceding_join_state,
            damage,
            ..
        } = self;
        let cols = services.buffer.cols;
        let insert_mode = services.core.modes.insert_mode;
        let cur_attr = *cur_attr_data;
        let link_id = cur_attr.extended.url_id();
        let null_cell = services.buffer.buffer().get_null_cell(Some(&cur_attr));
        let base = fill.codes.first().copied().unwrap_or(0);

        let buffer = services.buffer.buffer_mut();
        let row = abs_y(buffer);
        let x = buffer.x;
        let Some(buffer_row) = line_mut(buffer, row) else {
            return false;
        };
        // handle wide chars: reset start_cell-1 if we would overwrite the second cell of a wide
        // char
        if x != 0 && buffer_row.get_width(x - 1) == 2 {
            buffer_row.set_cell_from_codepoint(x - 1, 0, 1, &cur_attr);
        }
        dirty_row_tracker.mark_dirty(buffer.y);

        // the base: it fits (`x + 1 <= cols`)
        if link_id != 0 {
            add_cursor_line_to_link(services, link_id);
        }
        let Buffer { lines, x, .. } = services.buffer.buffer_mut();
        let Some(buffer_row) = lines.get_mut(row) else {
            return false;
        };
        if insert_mode {
            buffer_row.insert_cells(*x, 1, &null_cell);
            if buffer_row.get_width(cols.saturating_sub(1)) == 2 {
                buffer_row.set_cell_from_codepoint(
                    cols.saturating_sub(1),
                    NULL_CELL_CODE,
                    NULL_CELL_WIDTH,
                    &cur_attr,
                );
            }
        }
        buffer_row.set_cell_from_codepoint(*x, base, 1, &cur_attr);
        *x += 1;

        // the widening joiner: `x + 2 - 1 > cols`, so it wraps the base to the next line
        if link_id != 0 {
            add_cursor_line_to_link(services, link_id);
        }
        let Some(row) = print_wrap(
            services,
            dirty_row_tracker,
            damage,
            erase_attr_data_internal,
            &cur_attr,
            row,
            1,
            strings,
        ) else {
            return false;
        };
        let Buffer { lines, x, .. } = services.buffer.buffer_mut();
        let Some(buffer_row) = lines.get_mut(row) else {
            return false;
        };
        // (x is 1: the base at 0, one cell back)
        let shared = buffer_row.join_head_shared(0, head, joined);
        if !shared {
            if let (Some(&code), Some(&width)) = (fill.codes.get(1), fill.widths.get(1)) {
                buffer_row.add_codepoint_to_cell(0, code, width);
            }
        }
        buffer_row.set_cell_from_codepoint(*x, 0, 0, &cur_attr);
        *x += 1;

        // the later joiners: they keep the width, so they neither wrap nor move the cursor;
        // the stub at 1 puts them two cells back, on column 0
        for (&code, &width) in fill.codes.iter().zip(&fill.widths).skip(2) {
            if link_id != 0 {
                add_cursor_line_to_link(services, link_id);
            }
            if !shared {
                if let Some(buffer_row) = line_mut(services.buffer.buffer_mut(), row) {
                    buffer_row.add_codepoint_to_cell(0, code, width);
                }
            }
        }

        *preceding_join_state = fill.end_state;
        dirty_row_tracker.mark_dirty(services.buffer.buffer().y);
        true
    }

    /// LUNETERM: (M7 fix round 7, plan revision 2 fix6; security S4) [`InputHandler::print_repeated`]'s
    /// insert-mode whole passes over the last row below the scroll region, for a widened unit
    /// whose joiner wraps its base (`wrap_copies`, an odd number of columns; the base is one
    /// cell): the units whose first joiner is the widening one (VS16, a keycap: the units
    /// [`InputHandler::print_widened_wrap`] prints by its own steps) and (M7 fix round 8,
    /// security S4 round 8) those with joiners before it (`a U+0301 U+FE0F`, `print_inner`'s
    /// steps: each such joiner keeps width 1 and is added to the base's cell at `c`, which
    /// `pre`, built by the same calls on a scratch line, stands for). Called at a loop top with
    /// the cursor at `span = cols - 1`: `passes` passes replayed in dense arrays
    /// ([`BufferLine::replay_wrap_passes`], the same writes as the passes, its doc has the
    /// argument). Returns the passes computed, or `None`, having done nothing, where it does not
    /// apply (another unit, another row, a row no longer than `cols`, or the primitive's guards);
    /// the caller then prints the passes as before.
    ///
    /// What the passes do besides the row's cells and side tables, and how it is kept: the
    /// row is marked wrapped (each pass's joiner wrap does it; here once); the cursor ends
    /// where it started (each pass ends at `span`), on this row (below the region a wrap does
    /// not scroll, so no damage and no other row); the row was marked dirty when the REP
    /// started and each pass marks the same row; the link adds go to this row, the first
    /// caches its marker and every later one finds it with only cell writes between (as the
    /// line fill's other batches argue), so one add is the same; the screen reader text was
    /// queued up front and the join state is set when the REP ends; the line fill's template
    /// and the shared joined head are caches checked against what they stand for. Before the
    /// passes the cut floor (`rep_equivalent_repeats`, unchanged) already bounds their number.
    /// `print_inner`'s link add per code point lands on this row too (the joiners before the
    /// widening one neither wrap nor move the cursor), so one add stands for them as well.
    fn replay_wrap_passes(
        &mut self,
        fill: &RepFill,
        head: &CellData,
        stub: &CellData,
        null_cell: &CellData,
        passes: usize,
    ) -> Option<usize> {
        let widens_at = fill.widens_at?;
        if widens_at == 0 || fill.base_width != 1 || fill.codes.len() <= widens_at {
            return None;
        }
        let cols = self.services.buffer.cols;
        let rows = self.services.buffer.rows;
        let cur_attr = self.cur_attr_data;
        let link_id = cur_attr.extended.url_id();
        let buffer = self.services.buffer.buffer_mut();
        let below_last = buffer.y + 1 == rows && buffer.y > buffer.scroll_bottom;
        if !below_last {
            return None;
        }
        let row = abs_y(buffer);
        // (a row of `cols` cells has no old tail to push out: its passes repeat within a few,
        // which the round-3 batch takes as cheaply, sharing the strings the replay would rebuild)
        let line = line_mut(buffer, row).filter(|line| line.len() > cols)?;
        let base = fill.codes.first().copied().unwrap_or(0);
        // (M7 fix round 8) the cell `print_inner` leaves at `c` before the widening joiner:
        // the base, then each earlier joiner added to it with its width (1, `rep_alike`)
        let pre = (widens_at > 1).then(|| {
            let mut scratch = BufferLine::new(1, None, false);
            scratch.set_cell_from_codepoint(0, base, 1, &cur_attr);
            let joiners = fill.codes.iter().zip(&fill.widths).take(widens_at).skip(1);
            for (&code, &width) in joiners {
                scratch.add_codepoint_to_cell(0, code, width);
            }
            let mut cell = CellData::new();
            scratch.load_cell(0, &mut cell);
            cell
        });
        let unit = WrapUnit {
            cols,
            base,
            pre: pre.as_ref(),
            head,
            stub,
            fill: null_cell,
            attrs: &cur_attr,
        };
        let done = line.replay_wrap_passes(&unit, passes)?;
        line.is_wrapped = true;
        if link_id != 0 {
            add_cursor_line_to_link(&mut self.services, link_id);
        }
        Some(done)
    }

    /// LUNETERM: (M7 fix round 5, code review F6 residual) how many of the first joiner wraps
    /// that scroll at the scroll bottom may skip their strings' copy, for `count` repetitions of
    /// a widened unit (`w` = 2 cells, `inserted` = 1) whose joiner wraps its base (an odd number
    /// of columns, `wrap_copies`), from the cursor; 0 below the scroll region (no wrap scrolls
    /// there) or where the REP is too short. Each such wrap runs upstream's `copyCellsFrom`
    /// FIXME copy of the old row's strings from `c = cols - 1` on, `c` to the left, onto the
    /// fresh line the scroll made, keys past its end included: a chain from line to line that
    /// costs `Σ (K - i c)` entry copies and leaves stale entries on the lines.
    ///
    /// Let `K` be [`InputHandler::copy_reach`] (the floor's reach), `m = ceil(K / c) - 1` (0
    /// for `K < c + 1`) and `max_length` = the buffer's line capacity (`rows + scrollback`,
    /// buffer.rs `max_length`). Skipping the strings' copy of the first `m` joiner wraps that
    /// scroll, counted from the first, leaves the state of the print with every copy, when at
    /// least `max_length` scrolls follow the `m`-th of them:
    ///  - (i) A line a scroll at the scroll bottom made is out of the buffer after `max_length`
    ///    more scrolls. With `scroll_top` 0 every scroll inserts one line right after the
    ///    bottom row, that is after this line (buffer_service.rs `scroll`; the oldest line is
    ///    recycled through `copy_from` of a blank line once the list is full), and trims the
    ///    first line once the list is full: the line starts at an index at most `len - 1` and
    ///    its index drops by one per scroll once full, so it is trimmed within `(max_length -
    ///    len) + len` scrolls. With `scroll_top > 0` the region's lines move up one per scroll
    ///    and the top one is dropped: within the region's height, at most `rows`.
    ///  - (ii) The chain dies out after `m` lines. The first scrolling wrap's old row holds no
    ///    key above `K`: at the REP's start no row from the cursor's down to the last does, a
    ///    pass writes keys below `cols` (and in insert mode shifts strings within a row, below
    ///    its length, which `K` counts), and a copy into an existing row by descent writes keys
    ///    up to `K - c`. A scrolling wrap's new line is blank; it takes the old row's keys
    ///    `>= c`, `c` lower, then the pass writes keys below `cols`, and an insert moves only
    ///    the strings of flagged cells (the pass's own; the copied entries sit on cells without
    ///    the flag, but column 0, which no insert moves). So the `i`-th new line holds no copied
    ///    key above `K - i c`, and the `m`-th none above `c` (`K - m c <= c`).
    ///  - (iii) Hence the `(m + 1)`-th wrap's copy reads the same keys above `c` in both prints
    ///    (none), and the value at `c` it carries to key 0 of its new line is overwritten there
    ///    before anything reads it (the joiners rebuild the head from the base, which carries no
    ///    string; with a joiner before the widening one, `c` itself is rewritten before the
    ///    copy in both). The lines that can differ are the `m` lines the skipped copies made,
    ///    only in their strings, and nothing reads those but the next wrap's copy (keys `>= c`)
    ///    and their own pass (inserts, the fill's template); with `max_length` scrolls after
    ///    the `m`-th, (i) discards all `m` before the REP ends. Copies into rows reached by
    ///    descent (above the region, or before the first scroll) are never skipped.
    ///
    /// The scrolls that follow are counted from the REP's own wraps: from column `x` the first
    /// line takes `u0 = (cols - x) / 2` more units where the unit fits there (else 0), then a
    /// wrap; every later line takes `per_line = cols / 2` units (a joiner wrap's line: the
    /// wrapped unit and `per_line - 1`; after a plain wrap from column `cols`: `per_line`), so
    /// the REP wraps `W = ceil((count - u0) / per_line)` times. The first `d = scroll_bottom -
    /// y` of them descend; the first scrolling one is a plain wrap (no copy) when `d = 0` and
    /// the first line ends at column `cols`. The skip applies when `W >= d + plain + m +
    /// max_length`.
    fn chain_copies_to_skip(&self, count: usize, w: usize, inserted: usize) -> usize {
        let cols = self.services.buffer.cols;
        let buffer = self.services.buffer.buffer();
        let (x, y) = (buffer.x, buffer.y);
        let c = cols.saturating_sub(1);
        let per_line = cols / w.max(1);
        if y > buffer.scroll_bottom || c == 0 || per_line == 0 {
            return 0;
        }
        let Some(reach) = self.copy_reach() else {
            return 0;
        };
        // `ceil(K / c) - 1`
        let m = reach.saturating_sub(1) / c;
        if m == 0 {
            return 0;
        }
        let u0 = match x + w <= cols {
            true => (cols - x) / w,
            false => 0,
        };
        let wraps = count.saturating_sub(u0).div_ceil(per_line);
        let descents = buffer.scroll_bottom - y;
        let first_end = x + w * u0.min(count);
        let plain = usize::from(descents == 0 && first_end + inserted > cols);
        let max_length = buffer.lines.max_length();
        let needed = descents
            .saturating_add(plain)
            .saturating_add(m)
            .saturating_add(max_length);
        match wraps >= needed {
            true => m,
            false => 0,
        }
    }

    /// LUNETERM: (M7 fix round 5, code review F6 residual) [`InputHandler::print_repeated`]'s
    /// copy batch, called at a loop top with the cursor at `span = per_line × 2 = cols - 1`
    /// (`cols` odd), insert mode off, for a widened unit whose joiner wraps its base
    /// (`wrap_copies`) and `left` units to print. On the last row below the scroll region, with
    /// the row at least `cols` long and `p = left / per_line >= 2` whole passes left, it gives
    /// the row's combined strings the effect of the first `p - 1` passes
    /// ([`BufferLine::shift_combined_classes_repeatedly`] with `c = cols - 1`, `k = p - 1`) and
    /// returns true; the caller drops those `(p - 1) × per_line` units, and the last whole pass
    /// then prints for real. Otherwise (or where the strings are sparse high keys) it changes
    /// nothing and returns false.
    ///
    /// A pass from `span` is `per_line` units: the one whose base lands on the last column `c`
    /// (`print`'s own steps, [`InputHandler::print_inner`]) and `per_line - 1` units of the line
    /// fill from column 2. Below the region its wrap lands on the same row. On the row's
    /// combined map it is `P = W_post ∘ S ∘ W_pre`:
    ///  - `W_pre`: the base written at `c` (no string), and any joiner before the widening one
    ///    added to it (a string at `c`, built from the unit alone);
    ///  - `S`: the wrap's `copy_cells_from(snapshot, c, 0, 1)`, whose map part (upstream's
    ///    FIXME) gives every key `j` the snapshot's value at `j + c` where held;
    ///  - `W_post`: key 0 (the widening joiner and those after it added to the cell the copy
    ///    put there: a fresh string from the base, or the string the copy brought from `c`,
    ///    which `W_pre` wrote — either way the unit's head string, whatever the map held) and
    ///    the heads of the fill at `2, 4, …, c - 2` (the head's string, a full overwrite).
    ///
    /// Every write of `W_pre` and `W_post` is at a key `< cols`, with a value that depends on
    /// the unit only. **Lemma**: for `p >= 1`, `P^p(X) = P(F_{p-1}(X))`, `F` as in
    /// [`BufferLine::shift_combined_classes_repeatedly`]. Let `A = {0, 2, …, c - 2}` be
    /// `W_post`'s keys and `A' ⊆ {c}` `W_pre`'s. For a key `q >= 1` outside `A ∪ A'`, `P(Y)[q]`
    /// is `Y[q + c]` where held and `Y[q]` otherwise (`q + c >= cols` is outside every write
    /// set), and `q + c` is again such a key, so by induction (the step of `F_k = S^k`)
    /// `P^m(X)[q] = F_m(X)[q]` for every `m`. `P(Y)` reads `Y` only at such keys: `S`'s sources
    /// are the keys `>= c` (with `c` rewritten by `W_pre` first where `A'` holds it), its kept
    /// values at keys of `A` and at key 0 are overwritten after it, and `W_post`'s values do not
    /// depend on `Y`. So `P(P^{p-1}(X)) = P(F_{p-1}(X))`.
    ///
    /// Everything else a pass does is the same in every pass, so `p - 1` skipped passes and one
    /// real one leave what `p` passes leave: every cell of `[0, cols)` is rewritten from the
    /// unit and the current attributes alone (the base and its clear at `c`, the copied base and
    /// head at 0, the stub at 1, the fill's heads and stubs at `2..c`; the start fix-up at
    /// `c - 1` is overwritten by the fill's stub, or by the stub at 1 when `cols = 3`), and so
    /// is each extended entry there (the writes insert the current attributes' entry, or none
    /// without one: the copy moves an entry only from a cell carrying the flag, which the base
    /// at `c` carries exactly when the attributes do); nothing is written at `>= cols` but by
    /// `S`. The row is marked wrapped and dirty again; the cursor ends at `span` on the same
    /// row; a link add finds the marker the first one cached on this row (as the line fill's
    /// other batches argue); the screen reader text was queued up front; below the region a
    /// wrap does not scroll, so no damage. The cut floor (`rep_equivalent_repeats`) is not
    /// changed by this: it only makes those floor units cheap. Cost: `O(K + runs)` for the
    /// batch, then the last pass, where the passes cost `O(len)` each (`O(len^2 / cols)` in all
    /// on a long row of strings, about `K / (cols - 1)` of them until the strings settle).
    fn copy_passes_at_once(&mut self, left: usize, per_line: usize) -> bool {
        let cols = self.services.buffer.cols;
        let rows = self.services.buffer.rows;
        let passes = left / per_line.max(1);
        let buffer = self.services.buffer.buffer_mut();
        let below_last = buffer.y + 1 == rows && buffer.y > buffer.scroll_bottom;
        if !below_last || passes < 2 {
            return false;
        }
        let row = abs_y(buffer);
        line_mut(buffer, row)
            .filter(|line| line.len() >= cols)
            .is_some_and(|line| line.shift_combined_classes_repeatedly(cols - 1, passes - 1))
    }

    /// LUNETERM: (plan M7 step 13, S7 residual 5) `print` of `count` repetitions of `unit`
    /// (the repeated cell's code points) with wraparound off and insert mode on, for a unit
    /// with a narrow base whose repetitions print alike ([`InputHandler::rep_alike`]) on a line
    /// at least `cols` long: `print` of the units that bring the cursor to the pending column
    /// `cols` (one per column left, at least one), then the rest as one
    /// [`BufferLine::stuck_insert_repeatedly`] at `cols - 1`, where `print` costs an `O(len)`
    /// insert per unit (the F9 floor's `len - cols` extra units made such a REP
    /// `O(len × (len - cols))`). Returns false, having done nothing, in every other case and
    /// when no unit is left once the cursor is stuck.
    ///
    /// Why the state is `print`'s of all the units:
    ///  - `print` of the data in two calls is the one call here: the second call's start
    ///    fix-up finds the narrow head at `cols - 1` and the first call's end fix-up the cursor
    ///    at `cols`, so neither does anything; the join state carries over and the dirty row is
    ///    the same row.
    ///  - Every later unit's base overflows (`cols + 1 > cols`), so without wraparound the
    ///    cursor goes back to `cols - 1`; then `insert_cells(cols - 1, 1, null)`, the edge
    ///    fix-up, the base written, its joiners added to the cell at `cols - 1` (narrow, so one
    ///    cell back; the joined width is the base's, so the cursor does not move), and the
    ///    cursor at `cols` again. The base's and joiners' writes do not depend on the cell's
    ///    old value, so they leave what `set_cell(cols - 1, head)` leaves, `head` loaded from
    ///    the cell the last printed unit wrote. That is `stuck_insert_repeatedly`'s loop.
    ///  - Link: `print` adds the cursor's line before each code point. The printed units
    ///    added this row; each later add first forwards the pending marker disposals (the
    ///    first does it here) and then finds the marker it cached with only cell writes in
    ///    between, which changes nothing, so one add is the same.
    ///  - Screen reader: the other code points' text, queued in order until the per-drain
    ///    cap, as [`InputHandler::print_repeated`] does.
    ///
    /// (M7 fix round 1, review S1) A joiner that widens the base to two cells does not fit
    /// once the unit is stuck: `print` skips it and leaves the cursor at `cols - 1`, so a
    /// stuck unit inserts at `cols - 1` as before, writes its base and any joiner before the
    /// widening one there (the head), and ends at `cols - 1`; the units brought there are
    /// stuck once the cursor is (at most `ceil((cols - 1 - x) / 2) + 1 <= max(cols - x, 1)`
    /// units). Joiners after the widening one join the cell left of the cursor (`cols - 2`, or
    /// `cols - 3` behind a stub), untouched by the inserts: units go through `print` one at a
    /// time until one leaves those two cells as it found them (their string full, at most
    /// [`MAX_COMBINED_CODE_POINTS`] units), which every later unit then does too, as a
    /// unit's effect there depends on them only; the batch's one `pos - 1` fix-up is undone by
    /// writing `cols - 2` back as the units leave it. Split `print` calls at the stuck cursor
    /// do what one call does: the start fix-up is the insert's own `pos - 1` fix-up, and the
    /// end fix-up finds the narrow head.
    fn print_stuck_inserts(&mut self, unit: &[u32], count: usize) -> bool {
        let services = &self.services;
        if services.core.dec_private_modes.wraparound || !services.core.modes.insert_mode {
            return false;
        }
        let Some(alike) = self.rep_alike(unit).filter(|alike| alike.base_width == 1) else {
            return false;
        };
        let cols = services.buffer.cols;
        // (M7 fix round 1) where a stuck unit leaves the cursor: a widening joiner does not fit,
        // so `print` skips it with the cursor back at the last column
        let stuck_x = match alike.widens_at {
            Some(_) => cols.saturating_sub(1),
            None => cols,
        };
        // (M7 fix round 1) joiners after a widening one join the cell left of the cursor
        let joins_left = alike.widens_at.is_some_and(|at| at + 1 < alike.codes.len());
        let buffer = services.buffer.buffer();
        let long_enough = buffer
            .lines
            .get(abs_y(buffer))
            .is_some_and(|line| line.len() >= cols);
        let to_stuck = cols.saturating_sub(buffer.x).max(1);
        if !long_enough || cols < 2 || count <= to_stuck {
            return false;
        }
        let repeated = |times: usize| -> Vec<u32> {
            unit.iter()
                .copied()
                .cycle()
                .take(unit.len() * times)
                .collect()
        };
        self.print(&repeated(to_stuck));
        let mut left = count - to_stuck;
        let stuck = |handler: &Self| handler.services.buffer.buffer().x == stuck_x;
        if !stuck(self) {
            // (not reached: a narrow base moves the cursor one column, up to `cols`, and a
            // widened one two, until it is stuck at `cols - 1`)
            debug_assert!(false, "REP unit not stuck at the last column");
            self.print(&repeated(left));
            return true;
        }
        // (M7 fix round 1) the cells left of the cursor that those joiners change: one unit at
        // a time (the split `print` calls leave what one call does: at the stuck cursor the
        // start fix-up is the insert's own and the end fix-up finds the narrow head) until a
        // unit leaves them as it found them; the unit's effect there depends on them only, so
        // every later unit leaves them too
        let left_cells = |handler: &Self| {
            let buffer = handler.services.buffer.buffer();
            buffer.lines.get(abs_y(buffer)).map(|line| {
                (cols.saturating_sub(3)..cols - 1)
                    .map(|c| line.cell_state(c))
                    .collect::<Vec<_>>()
            })
        };
        let mut steady_left = None;
        if joins_left {
            for _ in 0..MAX_COMBINED_CODE_POINTS + 3 {
                if left == 0 {
                    return true;
                }
                let before = left_cells(self);
                self.print(unit);
                left -= 1;
                let after = left_cells(self);
                if !stuck(self) {
                    debug_assert!(false, "REP unit not stuck at the last column");
                    break;
                }
                if before == after {
                    steady_left = after;
                    break;
                }
            }
            if steady_left.is_none() {
                // (not reached: the joined cell's string stops growing at its cap)
                self.print(&repeated(left));
                return true;
            }
            if left == 0 {
                return true;
            }
        }
        #[cfg(test)]
        {
            self.rep_stuck_batches += 1;
        }

        let InputHandler {
            services,
            cur_attr_data,
            dirty_row_tracker,
            events,
            ..
        } = self;
        let cur_attr = *cur_attr_data;
        if services.options.options().screen_reader_mode {
            'voice: for _ in 0..left {
                for &code in &alike.codes {
                    if events.a11y_full() {
                        break 'voice;
                    }
                    events.a11y_char(code);
                }
            }
        }
        let link_id = cur_attr.extended.url_id();
        if link_id != 0 {
            add_cursor_line_to_link(services, link_id);
        }
        let null_cell = services.buffer.buffer().get_null_cell(Some(&cur_attr));
        let buffer = services.buffer.buffer_mut();
        let y = buffer.y;
        let row = abs_y(buffer);
        if let Some(line) = line_mut(buffer, row) {
            let mut head = CellData::new();
            line.load_cell(cols - 1, &mut head);
            line.stuck_insert_repeatedly(cols - 1, left, &null_cell, &head, &cur_attr);
            // (M7 fix round 1) the cell left of the head as every unit leaves it, where the
            // batch's one `pos - 1` fix-up rewrote it
            if let Some(cell) = steady_left.as_ref().and_then(|cells| cells.last()) {
                line.set_cell_state(cols - 2, cell);
            }
        }
        dirty_row_tracker.mark_dirty(y);
        true
    }

    /// ESC # 8 (DECALN): fill the viewport with `E` in the current colours and home the cursor.
    pub fn screen_alignment_pattern(&mut self) -> bool {
        // prepare cell data
        let mut cell = CellData::new();
        cell.content = (1 << Content::WIDTH_SHIFT) | u32::from('E');
        cell.fg = self.cur_attr_data.fg;
        cell.bg = self.cur_attr_data.bg;

        self.set_cursor(0, 0);
        let rows = self.services.buffer.rows;
        let buffer = self.services.buffer.buffer_mut();
        let first = abs_y(buffer);
        for y_offset in 0..rows {
            if let Some(line) = line_mut(buffer, first.saturating_add(y_offset)) {
                line.fill(&cell, false);
                line.is_wrapped = false;
            }
        }
        self.dirty_row_tracker.mark_all_dirty(rows);
        self.set_cursor(0, 0);
        true
    }

    /// Send `data` to the application (upstream `coreService.triggerDataEvent(data)`): a
    /// [`HostEvent::Response`], nothing with `disable_stdin`.
    pub(crate) fn send_data(&mut self, data: &str) {
        let services = &mut self.services;
        let event = services.core.trigger_data_event(
            data,
            false,
            services.buffer.buffer(),
            services.options.options(),
            &services.log,
        );
        if let Some(event) = event {
            self.events
                .push(HostEvent::Response(event.data.into_bytes()));
        }
    }

    /// Whether the terminal name starts with `term` (upstream `_is`).
    fn is_term(&self, term: &str) -> bool {
        self.services.options.options().term_name.starts_with(term)
    }

    /// CSI Ps c (DA1): the primary device attributes. Only Ps = 0 or omitted is answered.
    pub fn send_device_attributes_primary(&mut self, params: &Params) -> bool {
        if params.get(0).is_some_and(|p| p > 0) {
            return true;
        }
        if self.is_term("xterm") || self.is_term("rxvt-unicode") || self.is_term("screen") {
            self.send_data("\x1b[?1;2c");
        } else if self.is_term("linux") {
            self.send_data("\x1b[?6c");
        }
        true
    }

    /// CSI > Ps c (DA2): the secondary device attributes. Only Ps = 0 or omitted is answered.
    pub fn send_device_attributes_secondary(&mut self, params: &Params) -> bool {
        if params.get(0).is_some_and(|p| p > 0) {
            return true;
        }
        // xterm and urxvt seem to spit this out around ~370 times (?).
        if self.is_term("xterm") {
            self.send_data("\x1b[>0;276;0c");
        } else if self.is_term("rxvt-unicode") {
            self.send_data("\x1b[>85;95;0c");
        } else if self.is_term("linux") {
            // not supported by the linux console, which echoes the parameters
            let reply = match params.get(0) {
                Some(p) => format!("{p}c"),
                None => "undefinedc".to_owned(),
            };
            self.send_data(&reply);
        } else if self.is_term("screen") {
            self.send_data("\x1b[>83;40003;0c");
        }
        true
    }

    /// `options.<option> = value` for the options the handlers write (convertEol, cursorBlink);
    /// they are booleans and neither has a listener in the core.
    fn set_option_value(&mut self, value: OptionValue) {
        // A boolean option cannot be rejected.
        let _ = self.services.options.set(value);
    }

    /// Request a repaint of every viewport row (upstream `onRequestRefreshRows(undefined)`).
    fn request_refresh_all_rows(&mut self) {
        let rows = self.services.buffer.rows;
        self.damage.add_rows(0, rows.saturating_sub(1));
    }

    /// A buffer activation (upstream `onBufferActivate`): `BufferService` forwards the active
    /// buffer's `ydisp` as `onScroll`, and the whole screen is different.
    fn buffer_activated(&mut self, activate: BufferActivate) {
        self.on_scroll(activate.active_ydisp);
        self.damage.full = true;
    }

    /// DECSET 47, 1047, 1049: switch to the alternate buffer, filled with the erase attributes.
    fn use_alt_buffer(&mut self) {
        let erase = self.erase_attr_data();
        if let Some(activate) = self
            .services
            .buffer
            .buffers
            .activate_alt_buffer(Some(&erase))
        {
            self.buffer_activated(activate);
        }
        self.services.core.is_cursor_initialized = true;
        self.request_refresh_all_rows();
    }

    /// DECRST 47, 1047, 1049: switch back to the normal buffer.
    fn use_normal_buffer(&mut self) {
        let services = &mut self.services;
        let activate = services
            .buffer
            .buffers
            .activate_normal_buffer(services.options.options());
        if let Some(activate) = activate {
            self.buffer_activated(activate);
        }
    }

    /// DECCOLM (`CSI ? 3 h/l`): resize to `cols` columns and reset the terminal. Only with the
    /// window option `SetWinLines`.
    fn set_column_mode(&mut self, cols: usize) {
        if !self.services.options.options().window_options.set_win_lines {
            return;
        }
        let services = &mut self.services;
        let rows = services.buffer.rows;
        services
            .buffer
            .resize(cols, rows, services.options.options());
        // onResize
        self.damage.full = true;
        self.forward_disposed_markers();
        // onRequestReset
        reset_terminal(self);
    }

    /// Set the mouse protocol by its `CoreMouseService` name. The names used are all registered.
    fn set_mouse_protocol(&mut self, name: &str) {
        let _ = self.services.mouse.set_active_protocol(name);
    }

    /// Set the mouse encoding by its `CoreMouseService` name. The names used are all registered.
    fn set_mouse_encoding(&mut self, name: &str) {
        let _ = self.services.mouse.set_active_encoding(name);
    }

    /// CSI Pm h (SM): only insert mode (4) and automatic newline (20) are supported.
    pub fn set_mode(&mut self, params: &Params) -> bool {
        for i in 0..params.length() {
            match params.get(i) {
                Some(4) => self.services.core.modes.insert_mode = true,
                Some(20) => self.set_option_value(OptionValue::ConvertEol(true)),
                _ => {}
            }
        }
        true
    }

    /// CSI ? Pm h (DECSET): set DEC private modes (the supported ones are in the table of
    /// upstream's `setModePrivate`).
    pub fn set_mode_private(&mut self, params: &Params) -> bool {
        for i in 0..params.length() {
            match params.get(i) {
                Some(1) => self.services.core.dec_private_modes.application_cursor_keys = true,
                Some(2) => {
                    // designate US-ASCII for G0-G3 (VT100 mode is not set)
                    for level in 0..4 {
                        self.services.charset.set_g_charset(level, None);
                    }
                }
                Some(3) => self.set_column_mode(132),
                Some(6) => {
                    self.services.core.dec_private_modes.origin = true;
                    self.set_cursor(0, 0);
                }
                Some(7) => self.services.core.dec_private_modes.wraparound = true,
                Some(12) => self.set_option_value(OptionValue::CursorBlink(true)),
                Some(45) => self.services.core.dec_private_modes.reverse_wraparound = true,
                Some(66) => {
                    self.services
                        .log
                        .debug(format_args!("Serial port requested application keypad."));
                    self.services.core.dec_private_modes.application_keypad = true;
                }
                // X10 Mouse: no release, no motion, no wheel, no modifiers
                Some(9) => self.set_mouse_protocol("X10"),
                // vt200 mouse: no motion
                Some(1000) => self.set_mouse_protocol("VT200"),
                // button event mouse
                Some(1002) => self.set_mouse_protocol("DRAG"),
                // any event mouse: sends motion events, even if there is no button held down
                Some(1003) => self.set_mouse_protocol("ANY"),
                // send focusin/focusout events: focusin ^[[I, focusout ^[[O
                Some(1004) => {
                    self.services.core.dec_private_modes.send_focus = true;
                    self.events.push(HostEvent::FocusReportRequested);
                }
                // utf8 ext mode mouse - removed in #2507
                Some(1005) => self
                    .services
                    .log
                    .debug(format_args!("DECSET 1005 not supported (see #2507)")),
                // sgr ext mode mouse
                Some(1006) => self.set_mouse_encoding("SGR"),
                // urxvt ext mode mouse - removed in #2507
                Some(1015) => self
                    .services
                    .log
                    .debug(format_args!("DECSET 1015 not supported (see #2507)")),
                // sgr pixels mode mouse
                Some(1016) => self.set_mouse_encoding("SGR_PIXELS"),
                // show cursor
                Some(25) => self.services.core.is_cursor_hidden = false,
                // alt screen cursor
                Some(1048) => {
                    self.save_cursor();
                }
                // alt screen buffer cursor, then as 47
                Some(1049) => {
                    self.save_cursor();
                    self.use_alt_buffer();
                }
                // alt screen buffer
                Some(47 | 1047) => self.use_alt_buffer(),
                // bracketed paste mode (https://cirw.in/blog/bracketed-paste)
                Some(2004) => self.services.core.dec_private_modes.bracketed_paste_mode = true,
                // synchronized output
                Some(2026) => self.services.core.dec_private_modes.synchronized_output = true,
                _ => {}
            }
        }
        true
    }

    /// CSI Pm l (RM): only replace mode (4) and normal linefeed (20) are supported.
    pub fn reset_mode(&mut self, params: &Params) -> bool {
        for i in 0..params.length() {
            match params.get(i) {
                Some(4) => self.services.core.modes.insert_mode = false,
                Some(20) => self.set_option_value(OptionValue::ConvertEol(false)),
                _ => {}
            }
        }
        true
    }

    /// CSI ? Pm l (DECRST): reset DEC private modes.
    pub fn reset_mode_private(&mut self, params: &Params) -> bool {
        for i in 0..params.length() {
            let param = params.get(i);
            match param {
                Some(1) => self.services.core.dec_private_modes.application_cursor_keys = false,
                // DECCOLM: 80 columns (upstream switches to 80, not to the old width)
                Some(3) => self.set_column_mode(80),
                Some(6) => {
                    self.services.core.dec_private_modes.origin = false;
                    self.set_cursor(0, 0);
                }
                Some(7) => self.services.core.dec_private_modes.wraparound = false,
                Some(12) => self.set_option_value(OptionValue::CursorBlink(false)),
                Some(45) => self.services.core.dec_private_modes.reverse_wraparound = false,
                Some(66) => {
                    self.services
                        .log
                        .debug(format_args!("Switching back to normal keypad."));
                    self.services.core.dec_private_modes.application_keypad = false;
                }
                // X10, vt200, button event and any event mouse
                Some(9 | 1000 | 1002 | 1003) => self.set_mouse_protocol("NONE"),
                Some(1004) => self.services.core.dec_private_modes.send_focus = false,
                Some(1005) => self
                    .services
                    .log
                    .debug(format_args!("DECRST 1005 not supported (see #2507)")),
                Some(1006) => self.set_mouse_encoding("DEFAULT"),
                Some(1015) => self
                    .services
                    .log
                    .debug(format_args!("DECRST 1015 not supported (see #2507)")),
                Some(1016) => self.set_mouse_encoding("DEFAULT"),
                // hide cursor
                Some(25) => self.services.core.is_cursor_hidden = true,
                // alt screen cursor
                Some(1048) => {
                    self.restore_cursor();
                }
                // normal screen buffer (1049 also restores the cursor)
                Some(47 | 1047 | 1049) => {
                    self.use_normal_buffer();
                    if param == Some(1049) {
                        self.restore_cursor();
                    }
                    self.services.core.is_cursor_initialized = true;
                    self.request_refresh_all_rows();
                }
                Some(2004) => self.services.core.dec_private_modes.bracketed_paste_mode = false,
                Some(2026) => {
                    self.services.core.dec_private_modes.synchronized_output = false;
                    self.request_refresh_all_rows();
                }
                _ => {}
            }
        }
        true
    }

    /// CSI Ps $ p (DECRQM, `ansi`) and CSI ? Ps $ p: report whether a mode is set, as
    /// `CSI [?] Ps ; Pm $ y` (Pm: 0 not recognized, 1 set, 2 reset, 3 permanently set, 4
    /// permanently reset).
    pub fn request_mode(&mut self, params: &Params, ansi: bool) -> bool {
        const NOT_RECOGNIZED: u8 = 0;
        const SET: u8 = 1;
        const RESET: u8 = 2;
        const PERMANENTLY_SET: u8 = 3;
        const PERMANENTLY_RESET: u8 = 4;
        fn b2v(value: bool) -> u8 {
            if value { SET } else { RESET }
        }

        let services = &self.services;
        let dm = &services.core.dec_private_modes;
        let mouse_protocol = services.mouse.active_protocol();
        let mouse_encoding = services.mouse.active_encoding();
        let cols = services.buffer.cols;
        let opts = services.options.options();
        let alt_active = services.buffer.buffers.is_alt_active();

        let p = params.get(0);
        let value = match (ansi, p) {
            (true, Some(2)) => PERMANENTLY_RESET,
            (true, Some(4)) => b2v(services.core.modes.insert_mode),
            (true, Some(12)) => PERMANENTLY_SET,
            (true, Some(20)) => b2v(opts.convert_eol),
            (true, _) => NOT_RECOGNIZED,
            (false, Some(1)) => b2v(dm.application_cursor_keys),
            (false, Some(3)) => {
                if !opts.window_options.set_win_lines {
                    NOT_RECOGNIZED
                } else if cols == 80 {
                    RESET
                } else if cols == 132 {
                    SET
                } else {
                    NOT_RECOGNIZED
                }
            }
            (false, Some(6)) => b2v(dm.origin),
            (false, Some(7)) => b2v(dm.wraparound),
            (false, Some(8)) => PERMANENTLY_SET,
            (false, Some(9)) => b2v(mouse_protocol == "X10"),
            (false, Some(12)) => b2v(opts.cursor_blink),
            (false, Some(25)) => b2v(!services.core.is_cursor_hidden),
            (false, Some(45)) => b2v(dm.reverse_wraparound),
            (false, Some(66)) => b2v(dm.application_keypad),
            (false, Some(67)) => PERMANENTLY_RESET,
            (false, Some(1000)) => b2v(mouse_protocol == "VT200"),
            (false, Some(1002)) => b2v(mouse_protocol == "DRAG"),
            (false, Some(1003)) => b2v(mouse_protocol == "ANY"),
            (false, Some(1004)) => b2v(dm.send_focus),
            (false, Some(1005)) => PERMANENTLY_RESET,
            (false, Some(1006)) => b2v(mouse_encoding == "SGR"),
            (false, Some(1015)) => PERMANENTLY_RESET,
            (false, Some(1016)) => b2v(mouse_encoding == "SGR_PIXELS"),
            // xterm always returns SET here
            (false, Some(1048)) => SET,
            (false, Some(47 | 1047 | 1049)) => b2v(alt_active),
            (false, Some(2004)) => b2v(dm.bracketed_paste_mode),
            (false, Some(2026)) => b2v(dm.synchronized_output),
            (false, _) => NOT_RECOGNIZED,
        };
        let number = p.map_or_else(|| "undefined".to_owned(), |p| p.to_string());
        let prefix = if ansi { "" } else { "?" };
        self.send_data(&format!("\x1b[{prefix}{number};{value}$y"));
        true
    }

    /// CSI Ps n (DSR): 5 reports the status, 6 the cursor position (`CSI y ; x R`, not adjusted
    /// for origin mode).
    pub fn device_status(&mut self, params: &Params) -> bool {
        match params.get(0) {
            // status report
            Some(5) => self.send_data("\x1b[0n"),
            // cursor position
            Some(6) => {
                let buffer = self.services.buffer.buffer();
                let (y, x) = (buffer.y.saturating_add(1), buffer.x.saturating_add(1));
                self.send_data(&format!("\x1b[{y};{x}R"));
            }
            _ => {}
        }
        true
    }

    /// CSI ? Ps n (DECDSR): only the cursor position (6) is answered (`CSI ? y ; x R`).
    pub fn device_status_private(&mut self, params: &Params) -> bool {
        // modern xterm doesnt seem to respond to any of these except ?6, 6, and 5
        if params.get(0) == Some(6) {
            let buffer = self.services.buffer.buffer();
            let (y, x) = (buffer.y.saturating_add(1), buffer.x.saturating_add(1));
            self.send_data(&format!("\x1b[?{y};{x}R"));
        }
        true
    }

    /// CSI ! p (DECSTR): soft terminal reset. Resets IRM and the other modes, the cursor
    /// visibility, the scroll margins, the text attributes, the charsets, the DECSC data and
    /// DECOM.
    pub fn soft_reset(&mut self, _params: &Params) -> bool {
        self.services.core.is_cursor_hidden = false;
        let rows = self.services.buffer.rows;
        let buffer = self.services.buffer.buffer_mut();
        buffer.scroll_top = 0;
        buffer.scroll_bottom = rows.saturating_sub(1);
        self.cur_attr_data = DEFAULT_ATTR_DATA;
        self.services.core.reset();
        self.services.charset.reset();

        // reset DECSC data
        let (fg, bg) = (self.cur_attr_data.fg, self.cur_attr_data.bg);
        let charset = self.services.charset.charset;
        let buffer = self.services.buffer.buffer_mut();
        buffer.saved_x = 0;
        buffer.saved_y = buffer.ybase;
        buffer.saved_cur_attr_data.fg = fg;
        buffer.saved_cur_attr_data.bg = bg;
        buffer.saved_charset = charset;

        // reset DECOM
        self.services.core.dec_private_modes.origin = false;
        true
    }

    /// CSI Ps SP q (DECSCUSR): set the cursor style (0 resets to the option; 1 and 2 block,
    /// 3 and 4 underline, 5 and 6 bar; odd values blink).
    pub fn set_cursor_style(&mut self, params: &Params) -> bool {
        let param = if params.length() == 0 {
            Some(1)
        } else {
            params.get(0)
        };
        let modes = &mut self.services.core.dec_private_modes;
        if param == Some(0) {
            modes.cursor_style = None;
            modes.cursor_blink = None;
        } else {
            match param {
                Some(1 | 2) => modes.cursor_style = Some(CursorStyle::Block),
                Some(3 | 4) => modes.cursor_style = Some(CursorStyle::Underline),
                Some(5 | 6) => modes.cursor_style = Some(CursorStyle::Bar),
                _ => {}
            }
            modes.cursor_blink = Some(param.is_some_and(|p| p % 2 == 1));
        }
        true
    }

    /// CSI Ps ; Ps ; Ps t (XTWINOPS): the window manipulations and reports that are enabled in
    /// the `window_options` (all are off by default): 14 and 16 ask the host for a report, 18
    /// reports the size in characters, 22 and 23 push and pop the titles.
    pub fn window_options(&mut self, params: &Params) -> bool {
        let first = params.get(0);
        if !param_to_window_option(first, &self.services.options.options().window_options) {
            return true;
        }
        let second = if params.length() > 1 {
            params.get(1)
        } else {
            Some(0)
        };
        match first {
            // GetWinSizePixels, returns CSI 4 ; height ; width t
            Some(14) => {
                if second != Some(2) {
                    self.events
                        .push(HostEvent::WindowReport(WindowReportKind::WinSizePixels));
                }
            }
            // GetCellSizePixels, returns CSI 6 ; height ; width t
            Some(16) => self
                .events
                .push(HostEvent::WindowReport(WindowReportKind::CellSizePixels)),
            // GetWinSizeChars, returns CSI 8 ; height ; width t
            Some(18) => {
                let (rows, cols) = (self.services.buffer.rows, self.services.buffer.cols);
                self.send_data(&format!("\x1b[8;{rows};{cols}t"));
            }
            // PushTitle
            Some(22) => {
                if second == Some(0) || second == Some(2) {
                    self.window_title_stack.push(Arc::clone(&self.window_title));
                    if self.window_title_stack.len() > STACK_LIMIT {
                        self.window_title_stack.remove(0);
                    }
                }
                if second == Some(0) || second == Some(1) {
                    self.icon_name_stack.push(Arc::clone(&self.icon_name));
                    if self.icon_name_stack.len() > STACK_LIMIT {
                        self.icon_name_stack.remove(0);
                    }
                }
            }
            // PopTitle
            Some(23) => {
                if second == Some(0) || second == Some(2) {
                    if let Some(title) = self.window_title_stack.pop() {
                        self.window_title = title;
                        self.events.title_changed();
                    }
                }
                if second == Some(0) || second == Some(1) {
                    if let Some(name) = self.icon_name_stack.pop() {
                        self.icon_name = name;
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// OSC 0 and 2: set the window title and tell the host (`HostEvent::TitleChanged`, one per
    /// drain).
    pub fn set_title(&mut self, data: &str) -> bool {
        self.window_title = Arc::from(data);
        self.events.title_changed();
        true
    }

    /// OSC 0 and 1: set the icon name (not exposed to the host).
    pub fn set_icon_name(&mut self, data: &str) -> bool {
        self.icon_name = Arc::from(data);
        true
    }

    /// OSC 0: set the title and the icon name.
    pub fn set_title_and_icon_name(&mut self, data: &str) -> bool {
        self.set_title(data);
        self.set_icon_name(data)
    }

    /// OSC 4 ; c ; spec: set (`spec` is a colour) or report (`spec` is `?`) the palette colour
    /// `c`, any number of pairs. Pairs with an invalid index or colour are skipped; one
    /// [`HostEvent::Color`] holds the valid ones.
    pub fn set_or_report_indexed_color(&mut self, data: &str) -> bool {
        let mut event = Vec::new();
        let mut slots = data.split(';');
        while let (Some(idx), Some(spec)) = (slots.next(), slots.next()) {
            let Some(index) = parse_js_index(idx).and_then(valid_color_index) else {
                continue;
            };
            if spec == "?" {
                event.push(ColorRequest::Report(index));
            } else if let Some(color) = parse_color(spec) {
                event.push(ColorRequest::Set(index, color));
            }
        }
        if !event.is_empty() {
            self.events.push(HostEvent::Color(event));
        }
        true
    }

    /// OSC 8 ; params ; uri: start a hyperlink (or finish it, with an empty `uri` and empty
    /// `params`). The `;` inside the uri is kept (xterm.js #4944).
    pub fn set_hyperlink(&mut self, data: &str) -> bool {
        let Some((params, uri)) = data.split_once(';') else {
            // malformed sequence, just return as handled
            return true;
        };
        let params = js_trim(params);
        if !uri.is_empty() {
            return self.create_hyperlink(params, uri);
        }
        if !params.is_empty() {
            return false;
        }
        self.finish_hyperlink()
    }

    fn create_hyperlink(&mut self, params: &str, uri: &str) -> bool {
        // It's legal to open a new hyperlink without explicitly finishing the previous one
        if self.cur_attr_data.extended.url_id() != 0 {
            self.finish_hyperlink();
        }
        let id = params
            .split(':')
            .find_map(|part| part.strip_prefix("id="))
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        // The step-3 invariant: a link trimmed earlier in this write is gone before the
        // service looks for it.
        self.forward_disposed_markers();
        let link_id = self.services.osc_link.register_link(
            &mut self.services.buffer,
            OscLinkData {
                id,
                uri: uri.to_owned(),
            },
        );
        self.cur_attr_data.extended.set_url_id(link_id);
        self.cur_attr_data.update_extended();
        true
    }

    fn finish_hyperlink(&mut self) -> bool {
        self.cur_attr_data.extended.set_url_id(0);
        self.cur_attr_data.update_extended();
        true
    }

    /// Apply the colour requests of OSC 10, 11 and 12: the slots map to the foreground,
    /// background and cursor colours starting at `offset` (10 -> 0, 11 -> 1, 12 -> 2); slots past
    /// the cursor colour are ignored. Each slot is its own [`HostEvent::Color`].
    fn set_or_report_special_color(&mut self, data: &str, offset: usize) -> bool {
        const SPECIAL: [ColorIndex; 3] = [
            ColorIndex::Foreground,
            ColorIndex::Background,
            ColorIndex::Cursor,
        ];
        for (slot, index) in data.split(';').zip(SPECIAL.iter().skip(offset)) {
            if slot == "?" {
                self.events
                    .push(HostEvent::Color(vec![ColorRequest::Report(*index)]));
            } else if let Some(color) = parse_color(slot) {
                self.events
                    .push(HostEvent::Color(vec![ColorRequest::Set(*index, color)]));
            }
        }
        true
    }

    /// OSC 10: set or query the default foreground colour.
    pub fn set_or_report_fg_color(&mut self, data: &str) -> bool {
        self.set_or_report_special_color(data, 0)
    }

    /// OSC 11: set or query the default background colour.
    pub fn set_or_report_bg_color(&mut self, data: &str) -> bool {
        self.set_or_report_special_color(data, 1)
    }

    /// OSC 12: set or query the cursor colour.
    pub fn set_or_report_cursor_color(&mut self, data: &str) -> bool {
        self.set_or_report_special_color(data, 2)
    }

    /// OSC 104 ; c: restore the palette colours listed (all of them without params).
    pub fn restore_indexed_color(&mut self, data: &str) -> bool {
        if data.is_empty() {
            self.events
                .push(HostEvent::Color(vec![ColorRequest::Restore(None)]));
            return true;
        }
        let event: Vec<ColorRequest> = data
            .split(';')
            .filter_map(|slot| parse_js_index(slot).and_then(valid_color_index))
            .map(|index| ColorRequest::Restore(Some(index)))
            .collect();
        if !event.is_empty() {
            self.events.push(HostEvent::Color(event));
        }
        true
    }

    /// OSC 110, 111, 112: restore one special colour.
    fn restore_special_color(&mut self, index: ColorIndex) -> bool {
        self.events
            .push(HostEvent::Color(vec![ColorRequest::Restore(Some(index))]));
        true
    }

    /// DCS $ q Pt ST (DECRQSS): report the setting selected by `data` (the text of the
    /// sequence that sets it, without the final parameters) as `DCS 1 $ r Pt ST`, or
    /// `DCS 0 $ r ST` for a setting that is not known. The SGR report is always `0m`
    /// (upstream's FIXME).
    pub fn request_status_string(&mut self, data: &str) -> bool {
        let buffer = self.services.buffer.buffer();
        let opts = self.services.options.options();
        let reply = match data {
            "\"q" => format!(
                "P1$r{}\"q",
                u8::from(self.cur_attr_data.is_protected() != 0)
            ),
            "\"p" => "P1$r61;1\"p".to_owned(),
            "r" => format!(
                "P1$r{};{}r",
                buffer.scroll_top.saturating_add(1),
                buffer.scroll_bottom.saturating_add(1)
            ),
            "m" => "P1$r0m".to_owned(),
            " q" => {
                let style = match opts.cursor_style {
                    CursorStyle::Block => 2,
                    CursorStyle::Underline => 4,
                    CursorStyle::Bar => 6,
                };
                format!("P1$r{} q", style - u8::from(opts.cursor_blink))
            }
            _ => "P0$r".to_owned(),
        };
        self.send_data(&format!("\x1b{reply}\x1b\\"));
        true
    }

    /// ESC = (DECKPAM): the numeric keypad sends application sequences.
    pub fn keypad_application_mode(&mut self) -> bool {
        self.services
            .log
            .debug(format_args!("Serial port requested application keypad."));
        self.services.core.dec_private_modes.application_keypad = true;
        true
    }

    /// ESC > (DECKPNM): the keypad sends numeric characters.
    pub fn keypad_numeric_mode(&mut self) -> bool {
        self.services
            .log
            .debug(format_args!("Switching back to normal keypad."));
        self.services.core.dec_private_modes.application_keypad = false;
        true
    }

    /// ESC % @, ESC % G: select the default character set. UTF-8 is not supported (the input is
    /// Unicode anyway), so ESC % G does the same.
    pub fn select_default_charset(&mut self) -> bool {
        self.services.charset.set_g_level(0);
        self.services.charset.set_g_charset(0, None); // US (default)
        true
    }

    /// ESC ( C, ESC ) C, ESC * C, ESC + C, ESC - C, ESC . C, ESC / C: designate the character
    /// set `flag` as G0, G1, G2, G3, G1, G2 (and for `/`: nothing, upstream's TODO).
    pub fn select_charset(&mut self, intermediate: char, flag: char) -> bool {
        let level = match intermediate {
            '(' => 0,
            ')' | '-' => 1,
            '*' | '.' => 2,
            '+' => 3,
            // `/` (G3 of ISO Latin-1 Supplemental) is not supported
            _ => return true,
        };
        self.services.charset.set_g_charset(level, charset(flag));
        true
    }

    /// ESC n, ESC o, ESC |, ESC }, ESC ~ (LS2, LS3, LS3R, LS2R, LS1R): lock a G set into GL.
    pub fn set_g_level(&mut self, level: usize) -> bool {
        self.services.charset.set_g_level(level);
        true
    }

    /// ESC c (RIS): reset to the initial state. Upstream also resets the parser first, which
    /// changes nothing here (module notes, D7), then asks the terminal to reset.
    pub fn full_reset(&mut self) -> bool {
        // LUNETERM: no `parser.reset()` (plan M6 D7).
        reset_terminal(self);
        true
    }

    /// CSI Pm m (SGR): select graphic rendition. Applies the params left to right to the
    /// current attributes (`charAttributes`, `InputHandler.ts:2510`). `38`/`48`/`58` hand the
    /// params from there on to [`extract_color`] and skip what it consumed. Unknown values are
    /// logged at debug level. (Upstream's `p === 100` branch is dead: `100..=107` is matched
    /// first; it is not ported.)
    pub fn char_attributes(&mut self, params: &Params) -> bool {
        // Optimize a single SGR0.
        if params.length() == 1 && params.get(0) == Some(0) {
            process_sgr0(&mut self.cur_attr_data);
            return true;
        }

        let l = params.length();
        let attr = &mut self.cur_attr_data;
        let mut i = 0;
        while i < l {
            // `params.params[i]`: `None` is JS `undefined`, which matches no branch below.
            match params.get(i) {
                // fg color 8
                Some(p @ 30..=37) => {
                    attr.fg &= !(Attributes::CM_MASK | Attributes::PCOLOR_MASK);
                    attr.fg |= Attributes::CM_P16 | (p - 30) as u32;
                }
                // bg color 8
                Some(p @ 40..=47) => {
                    attr.bg &= !(Attributes::CM_MASK | Attributes::PCOLOR_MASK);
                    attr.bg |= Attributes::CM_P16 | (p - 40) as u32;
                }
                // fg color 16
                Some(p @ 90..=97) => {
                    attr.fg &= !(Attributes::CM_MASK | Attributes::PCOLOR_MASK);
                    attr.fg |= Attributes::CM_P16 | (p - 90) as u32 | 8;
                }
                // bg color 16
                Some(p @ 100..=107) => {
                    attr.bg &= !(Attributes::CM_MASK | Attributes::PCOLOR_MASK);
                    attr.bg |= Attributes::CM_P16 | (p - 100) as u32 | 8;
                }
                // default
                Some(0) => process_sgr0(attr),
                // bold text
                Some(1) => attr.fg |= FgFlags::BOLD,
                // italic text
                Some(3) => attr.bg |= BgFlags::ITALIC,
                // underlined text: `4:x` carries the style
                Some(4) => {
                    attr.fg |= FgFlags::UNDERLINE;
                    let style = if params.has_sub_params(i) {
                        // `getSubParams(i)![0]`
                        params
                            .get_sub_params(i)
                            .and_then(|sub| sub.first())
                            .copied()
                    } else {
                        Some(UnderlineStyle::SINGLE as i32)
                    };
                    process_underline(style, attr);
                }
                // blink
                Some(5) => attr.fg |= FgFlags::BLINK,
                // inverse and positive
                Some(7) => attr.fg |= FgFlags::INVERSE,
                // invisible
                Some(8) => attr.fg |= FgFlags::INVISIBLE,
                // strikethrough
                Some(9) => attr.fg |= FgFlags::STRIKETHROUGH,
                // dimmed text
                Some(2) => attr.bg |= BgFlags::DIM,
                // double underline
                Some(21) => process_underline(Some(UnderlineStyle::DOUBLE as i32), attr),
                // not bold nor faint
                Some(22) => {
                    attr.fg &= !FgFlags::BOLD;
                    attr.bg &= !BgFlags::DIM;
                }
                // not italic
                Some(23) => attr.bg &= !BgFlags::ITALIC,
                // not underlined
                Some(24) => {
                    attr.fg &= !FgFlags::UNDERLINE;
                    process_underline(Some(UnderlineStyle::NONE as i32), attr);
                }
                // not blink
                Some(25) => attr.fg &= !FgFlags::BLINK,
                // not inverse
                Some(27) => attr.fg &= !FgFlags::INVERSE,
                // not invisible
                Some(28) => attr.fg &= !FgFlags::INVISIBLE,
                // not strikethrough
                Some(29) => attr.fg &= !FgFlags::STRIKETHROUGH,
                // reset fg
                Some(39) => {
                    attr.fg &= !(Attributes::CM_MASK | Attributes::RGB_MASK);
                    attr.fg |=
                        DEFAULT_ATTR_DATA.fg & (Attributes::PCOLOR_MASK | Attributes::RGB_MASK);
                }
                // reset bg
                Some(49) => {
                    attr.bg &= !(Attributes::CM_MASK | Attributes::RGB_MASK);
                    attr.bg |=
                        DEFAULT_ATTR_DATA.bg & (Attributes::PCOLOR_MASK | Attributes::RGB_MASK);
                }
                // fg / bg / underline color 256 and RGB
                Some(38 | 48 | 58) => i += extract_color(params, i, attr),
                // overline
                Some(53) => attr.bg |= BgFlags::OVERLINE,
                // not overline
                Some(55) => attr.bg &= !BgFlags::OVERLINE,
                // reset the underline colour
                Some(59) => {
                    attr.extended
                        .set_underline_color(Attributes::CM_MASK | Attributes::RGB_MASK);
                    attr.update_extended();
                }
                Some(p) => self
                    .services
                    .log
                    .debug(format_args!("Unknown SGR attribute: {p}.")),
                None => self
                    .services
                    .log
                    .debug(format_args!("Unknown SGR attribute: NaN.")),
            }
            i += 1;
        }
        true
    }
}

/// SGR 0 (`_processSGR0`): default colours, no underline style and underline colour; the
/// other extended fields (the url id) stay.
fn process_sgr0(attr: &mut AttributeData) {
    attr.fg = DEFAULT_ATTR_DATA.fg;
    attr.bg = DEFAULT_ATTR_DATA.bg;
    // Reset underline style and color. Note that we don't want to reset other
    // fields such as the url id. (Upstream clones `extended` first; it is `Copy` here.)
    attr.extended.set_underline_style(UnderlineStyle::NONE);
    let color = attr.extended.underline_color();
    attr.extended
        .set_underline_color(color & !(Attributes::CM_MASK | Attributes::RGB_MASK));
    attr.update_extended();
}

/// SGR 4 / 21 / 24 (`_processUnderline`). `style` is the `4:x` sub param, `None` for JS
/// `undefined`. `4:0` equals SGR 24, `4:1` SGR 4, `4:2` SGR 21, `4:3` curly, `4:4` dotted,
/// `4:5` dashed.
fn process_underline(style: Option<i32>, attr: &mut AttributeData) {
    // default to 1 == single underline: `!~style || style > 5` (`-1`, or above 5)
    let style = match style {
        Some(-1) => Some(UnderlineStyle::SINGLE),
        Some(s) if s > 5 => Some(UnderlineStyle::SINGLE),
        Some(s) => Some(u32::try_from(s).unwrap_or(UnderlineStyle::SINGLE)),
        // `undefined` stores as 0 and is not `=== 0`
        None => None,
    };
    attr.extended
        .set_underline_style(style.unwrap_or(UnderlineStyle::NONE));
    attr.fg |= FgFlags::UNDERLINE;

    // 0 deactivates underline
    if style == Some(0) {
        attr.fg &= !FgFlags::UNDERLINE;
    }

    // update HAS_EXTENDED in BG
    attr.update_extended();
}

/// A JS value in a bit operation: `undefined` is 0, an int32 keeps its bits.
fn js_bits(value: Option<i32>) -> u32 {
    u32::from_ne_bytes(value.unwrap_or(0).to_ne_bytes())
}

/// Helper to write color information packed with color mode (`_updateAttrColor`).
fn update_attr_color(
    mut color: u32,
    mode: Option<i32>,
    c1: Option<i32>,
    c2: Option<i32>,
    c3: Option<i32>,
) -> u32 {
    if mode == Some(2) {
        color |= Attributes::CM_RGB;
        color &= !Attributes::RGB_MASK;
        color |= AttributeData::from_color_rgb([js_bits(c1), js_bits(c2), js_bits(c3)]);
    } else if mode == Some(5) {
        color &= !(Attributes::CM_MASK | Attributes::PCOLOR_MASK);
        color |= Attributes::CM_P256 | (js_bits(c1) & 0xff);
    }
    color
}

/// Helper to extract and apply color params/subparams (`_extractColor`). Returns the advance
/// for the params index.
fn extract_color(params: &Params, pos: usize, attr: &mut AttributeData) -> usize {
    // normalize params
    // meaning: [target, CM, ign, val, val, val]
    // RGB    : [ 38/48,  2, ign,   r,   g,   b]
    // P256   : [ 38/48,  5, ign,   v, ign, ign]
    // (`None` is a JS `undefined`; the array is never longer than 6 where it matters: writes
    // past index 5 are never read.)
    let mut accu: [Option<i32>; 6] = [Some(0), Some(0), Some(-1), Some(0), Some(0), Some(0)];

    // alignment placeholder for non color space sequences
    let mut c_space = 0_usize;

    // return advance we took in params
    let mut advance = 0_usize;

    loop {
        set_slot(&mut accu, advance + c_space, params.get(pos + advance));
        if params.has_sub_params(pos + advance) {
            // `getSubParams(..)!`
            let subparams = params.get_sub_params(pos + advance).unwrap_or(&[]);
            let mut i = 0;
            loop {
                if cm(&accu) == Some(5) {
                    c_space = 1;
                }
                set_slot(
                    &mut accu,
                    advance + i + 1 + c_space,
                    subparams.get(i).copied(),
                );
                i += 1;
                // upstream's `accu.length` grows with writes past index 5; those are never read
                if !(i < subparams.len() && i + advance + 1 + c_space < 6) {
                    break;
                }
            }
            break;
        }
        // exit early if can decide color mode with semicolons
        if (cm(&accu) == Some(5) && advance + c_space >= 2)
            || (cm(&accu) == Some(2) && advance + c_space >= 5)
        {
            break;
        }
        // offset colorSpace slot for semicolon mode
        if matches!(cm(&accu), Some(v) if v != 0) {
            c_space = 1;
        }
        advance += 1;
        if !(advance + pos < params.length() && advance + c_space < 6) {
            break;
        }
    }

    // set default values to 0
    for slot in accu.iter_mut().skip(2) {
        if *slot == Some(-1) {
            *slot = Some(0);
        }
    }

    // apply colors
    let [target, mode, _, c1, c2, c3] = accu;
    match target {
        Some(38) => attr.fg = update_attr_color(attr.fg, mode, c1, c2, c3),
        Some(48) => attr.bg = update_attr_color(attr.bg, mode, c1, c2, c3),
        Some(58) => {
            let color = update_attr_color(attr.extended.underline_color(), mode, c1, c2, c3);
            attr.extended.set_underline_color(color);
        }
        _ => {}
    }

    advance
}

/// `accu[1]`, the colour mode slot.
fn cm(accu: &[Option<i32>; 6]) -> Option<i32> {
    accu.get(1).copied().flatten()
}

/// `accu[index] = value` where a write past the fixed array is dropped (see [`extract_color`]).
fn set_slot(accu: &mut [Option<i32>; 6], index: usize, value: Option<i32>) {
    if let Some(slot) = accu.get_mut(index) {
        *slot = value;
    }
}

/// The autowrap part of `print` (`InputHandler.ts:563-590`): moves the cursor to the next row
/// (scrolling at the bottom margin), carries the cells a joining character widens over, and
/// clears the rest of the old row. Returns the new cursor row, or `None` where upstream throws
/// on a missing line.
///
/// LUNETERM: (M7 fix round 5) without `strings` the carry copies the cells but not the old
/// row's combined strings from `old_col` on (upstream's copy); only REP's line fill asks for
/// that, for a wrap whose new line its own later scrolls discard
/// ([`InputHandler::chain_copies_to_skip`]).
#[allow(clippy::too_many_arguments)]
fn print_wrap(
    services: &mut CoreServices,
    tracker: &mut DirtyRowTracker,
    damage: &mut Damage,
    erase_internal: &mut AttributeData,
    cur_attr: &AttributeData,
    old_row: usize,
    old_width: u32,
    strings: bool,
) -> Option<usize> {
    let cols = services.buffer.cols;
    let rows = services.buffer.rows;
    let old_width = usize::try_from(old_width).unwrap_or(0);
    let buffer = services.buffer.buffer_mut();
    let old_col = buffer.x.saturating_sub(old_width);
    // LUNETERM: (M7 fix round 5, code review F6 residual) what the copy below reads of the old
    // row: its `old_width` cells from `old_col` and its combined strings (shared), not a clone of
    // the whole row (`O(len)` per wrap on a long row). Read where upstream reads it: the old
    // row's place after the scroll (which, with one row and no scrollback, is the recycled
    // blank row), or the row itself, taken before the scroll, when it leaves the list.
    let source_of = |line: &BufferLine| {
        line.cells_source(old_col, old_width)
            .map_or_else(|| WrapSource::Line(line.clone()), WrapSource::Cells)
    };
    let mut detached = None;
    buffer.x = old_width;
    buffer.y += 1;
    // where `oldRow` is after this branch
    let old_row_after;
    if buffer.y == buffer.scroll_bottom.saturating_add(1) {
        buffer.y -= 1;
        old_row_after = old_row_after_scroll(buffer, old_row);
        if old_width > 0 && old_row_after.is_none() {
            detached = buffer.lines.get(old_row).map(source_of);
        }
        let erase = erase_attr(erase_internal, cur_attr);
        services.buffer.scroll(&erase, true);
        scrolled(damage, tracker, services.buffer.buffer());
    } else {
        if buffer.y >= rows {
            buffer.y = rows.saturating_sub(1);
        }
        // The line already exists (eg. the initial viewport), mark it as a wrapped line
        let row = abs_y(buffer);
        line_mut(buffer, row)?.is_wrapped = true;
        old_row_after = Some(old_row);
    }

    // row changed, get it again
    let buffer = services.buffer.buffer_mut();
    let row = abs_y(buffer);
    let has_row = buffer.lines.get(row).is_some();
    if old_width > 0 && has_row {
        // Combining character widens 1 column to 2. Move old character to next line.
        let source = match old_row_after {
            Some(index) => buffer.lines.get(index).map(source_of),
            None => detached,
        };
        if let (Some(source), Some(buffer_row)) = (source, line_mut(buffer, row)) {
            match source {
                WrapSource::Cells(cells) => {
                    buffer_row.copy_cells_from_source(&cells, 0, strings);
                }
                WrapSource::Line(line) => {
                    buffer_row.copy_cells_from(&line, old_col, 0, old_width, false);
                }
            }
        }
    }
    // clear left over cells to the right (on a detached old row: no visible effect)
    if let Some(old) = old_row_after.and_then(|index| line_mut(buffer, index)) {
        for col in old_col..cols {
            old.set_cell_from_codepoint(col, 0, 1, cur_attr);
        }
    }
    // upstream throws on the next use of a missing row
    has_row.then_some(row)
}

/// LUNETERM: (review S3, S7) the head cell every repetition of `fill` writes and, for a wide
/// unit, its stub, made by `print`'s calls on a scratch line.
fn rep_head(fill: &RepFill, cur_attr: &AttributeData) -> (CellData, CellData) {
    let mut scratch = BufferLine::new(2, None, false);
    let mut codes = fill.codes.iter().copied().zip(fill.widths.iter().copied());
    if let Some((base, width)) = codes.next() {
        scratch.set_cell_from_codepoint(0, base, width, cur_attr);
    }
    for (code, width) in codes {
        scratch.add_codepoint_to_cell(0, code, width);
    }
    scratch.set_cell_from_codepoint(1, 0, 0, cur_attr);
    let (mut head, mut stub) = (CellData::new(), CellData::new());
    scratch.load_cell(0, &mut head);
    scratch.load_cell(1, &mut stub);
    (head, stub)
}

/// LUNETERM: (M7 fix round 5) what `print_wrap`'s copy reads of the old row: the cells it
/// copies and the shared strings ([`CellsSource`]), or (more than two cells, which no width
/// gives) the whole row.
enum WrapSource {
    Cells(CellsSource),
    Line(BufferLine),
}

#[cfg(test)]
#[path = "input_handler_tests.rs"]
mod tests;
