//! Port of xterm.js src/common/InputHandler.ts @ f447274.
//!
//! The terminal's standard handler set: everything the escape sequence parser dispatches lands
//! here and changes the buffers and services. Refer to
//! <http://invisible-island.net/xterm/ctlseqs/ctlseqs.html> for each handler's sequence.
//!
//! Ported so far (plan M6 step 2): the struct, `parse`, `print`, the register forwards (with the
//! `t` window-option guard), BEL LF VT FF CR BS HT SO SI IND NEL HTS, CSI A B C D E F G H I Z
//! `` ` `` a d e f g r s u, ESC 7 8 D E H M, the five fallbacks and the error handler, and
//! `DirtyRowTracker`, `reset` (step 3). The other handler groups follow in steps 5 to 8.
//!
//! Rust shape (plan M6 D1, D2, D3, D8):
//!  - The parser is generic over a `'static` host, so [`InputHandler`] owns the services
//!    ([`CoreServices`]) and is the [`ParserHost`]. The parser lives beside it (in
//!    `CoreTerminal`); [`InputHandler::parse`] takes it as an argument and lends `self` to it.
//!    Handlers are closures registered by [`InputHandler::register_handlers`].
//!  - Upstream's emitters become a queue of [`HostEvent`]s (in fire order) plus a coalesced
//!    [`Damage`]: `onRequestBell` -> `Bell`, `onA11yChar`/`onA11yTab` -> `A11yChar`/`A11yTab`,
//!    `onRequestRefreshRows` -> `Damage::rows`, `onCursorMove` -> `Damage::cursor_moved`,
//!    `bufferService.onScroll` (forwarded by `CoreTerminal`) -> [`InputHandler::on_scroll`].
//!    `onLineFeed` has no public consumer: it drives only the Windows wrapping heuristic, which
//!    runs inside `line_feed` when `windows_heuristics` is set (D6, set by `CoreTerminal`).
//!    `onRequestSyncScrollBar` (keypad mode switches, step 7) has no consumer and is dropped.
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

use crate::buffer::attribute_data::AttributeData;
use crate::buffer::buffer::Buffer;
use crate::buffer::buffer_line::{BufferLine, DEFAULT_ATTR_DATA};
use crate::buffer::constants::{Attributes, NULL_CELL_CODE, NULL_CELL_WIDTH};
use crate::data::escape_sequences::{C0, C1};
use crate::engine::{Damage, HostEvent};
use crate::input::text_decoder::{Utf8ToUtf32, string_from_code_point, utf32_to_string};
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
use crate::services::options_service::{OptionsService, WindowOptions};
use crate::services::osc_link_service::OscLinkService;
use crate::services::unicode_service::UnicodeService;
use crate::windows_mode::update_windows_mode_wrapped_state;

/// Max length of the UTF-32 input buffer; longer writes are parsed in chunks of this many bytes.
pub const MAX_PARSEBUFFER_LENGTH: usize = 131_072;

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
    events: Vec<HostEvent>,
    damage: Damage,
    /// The window title (upstream `_windowTitle`).
    window_title: String,
    /// Upstream's `onLineFeed` listener for the Windows wrapping heuristics (`CoreTerminal`'s
    /// `_handleWindowsPtyOptionChange`, plan M6 D6). Off until the host turns it on.
    pub(crate) windows_heuristics: bool,
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

/// Bytes as Latin-1 characters (upstream `String.fromCharCode` per byte).
struct Latin1<'a>(&'a [u8]);

impl fmt::Display for Latin1<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write;
        self.0.iter().try_for_each(|&b| f.write_char(char::from(b)))
    }
}

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

/// A UTF-32 payload chunk as a string.
struct Utf32<'a>(&'a [u32]);

impl fmt::Display for Utf32<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&utf32_to_string(self.0))
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
            events: Vec::new(),
            damage: Damage::default(),
            window_title: String::new(),
            windows_heuristics: false,
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

    /// The events fired since the last call, in fire order.
    pub fn take_events(&mut self) -> Vec<HostEvent> {
        mem::take(&mut self.events)
    }

    /// The damage accumulated since the last call.
    pub fn take_damage(&mut self) -> Damage {
        mem::take(&mut self.damage)
    }

    /// The window title (OSC 0/2).
    pub fn window_title(&self) -> &str {
        &self.window_title
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
        parser.set_osc_handler_fallback(|h, identifier, action| match action {
            OscFallbackAction::Start => h.services.log.debug(format_args!(
                "Unknown OSC code: {{ identifier: {identifier}, action: START }}"
            )),
            OscFallbackAction::Put(data) => h.services.log.debug(format_args!(
                "Unknown OSC code: {{ identifier: {identifier}, action: PUT, data: {} }}",
                Utf32(data)
            )),
            OscFallbackAction::End(success) => h.services.log.debug(format_args!(
                "Unknown OSC code: {{ identifier: {identifier}, action: END, data: {success} }}"
            )),
        });
        parser.set_dcs_handler_fallback(|h, ident, action| match action {
            DcsFallbackAction::Hook(params) => h.services.log.debug(format_args!(
                "Unknown DCS code: {{ identifier: {}, action: HOOK, payload: {} }}",
                Ident(ident),
                ParamsArray(params)
            )),
            DcsFallbackAction::Put(data) => h.services.log.debug(format_args!(
                "Unknown DCS code: {{ identifier: {}, action: PUT, payload: {} }}",
                Ident(ident),
                Utf32(data)
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
        mut callback: impl FnMut(&mut InputHandler, &Params) -> bool + 'static,
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
        callback: impl FnMut(&mut InputHandler, &str, &Params) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        parser.register_dcs_handler(id, DcsStringHandler::new(callback))
    }

    /// Forward `registerEscHandler` to `parser`.
    pub fn register_esc_handler(
        parser: &mut EscapeSequenceParser<InputHandler>,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        parser.register_esc_handler(id, callback)
    }

    /// Forward `registerOscHandler` to `parser` (the payload collected as a string).
    pub fn register_osc_handler(
        parser: &mut EscapeSequenceParser<InputHandler>,
        ident: i32,
        callback: impl FnMut(&mut InputHandler, &str) -> bool + 'static,
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

        // Log debug data (formatted only when the level lets it through)
        self.services
            .log
            .debug(format_args!("parsing data \"{}\"", Latin1(data)));
        self.services
            .log
            .trace(format_args!("parsing data (codes) {data:?}"));

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
            return;
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
            if code < 127 {
                if let Some(set) = charset {
                    // upstream takes `ch.charCodeAt(0)`, the first UTF-16 unit
                    if let Some(unit) = char::from_u32(code)
                        .and_then(|ch| set.get(ch))
                        .and_then(|ch| ch.encode_utf16().next())
                    {
                        code = u32::from(unit);
                    }
                }
            }

            let current_info = services.unicode.char_properties(code, preceding);
            let mut ch_width = UnicodeService::extract_width(current_info);
            let should_join = UnicodeService::extract_should_join(current_info);
            let old_width = if should_join {
                UnicodeService::extract_width(preceding)
            } else {
                0
            };
            preceding = current_info;

            if screen_reader_mode {
                events.push(HostEvent::A11yChar(string_from_code_point(code)));
            }
            let link_id = cur_attr.extended.url_id();
            if link_id != 0 {
                forward_disposed(services);
                let y = abs_y(services.buffer.buffer());
                services
                    .osc_link
                    .add_line_to_link(&mut services.buffer, link_id, y);
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
                    ) {
                        Some(new_row) => row = new_row,
                        // upstream throws on the missing line
                        None => return,
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
                return;
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
        if x < cols && !data.is_empty() {
            if let Some(buffer_row) = line_mut(buffer, row) {
                if buffer_row.get_width(x) == 0 && buffer_row.has_content(x) == 0 {
                    buffer_row.set_cell_from_codepoint(x, 0, 1, &cur_attr);
                }
            }
        }
        dirty_row_tracker.mark_dirty(buffer.y);
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
}

/// The autowrap part of `print` (`InputHandler.ts:563-590`): moves the cursor to the next row
/// (scrolling at the bottom margin), carries the cells a joining character widens over, and
/// clears the rest of the old row. Returns the new cursor row, or `None` where upstream throws
/// on a missing line.
fn print_wrap(
    services: &mut CoreServices,
    tracker: &mut DirtyRowTracker,
    damage: &mut Damage,
    erase_internal: &mut AttributeData,
    cur_attr: &AttributeData,
    old_row: usize,
    old_width: u32,
) -> Option<usize> {
    let cols = services.buffer.cols;
    let rows = services.buffer.rows;
    let old_width = usize::try_from(old_width).unwrap_or(0);
    let buffer = services.buffer.buffer_mut();
    let old_col = buffer.x.saturating_sub(old_width);
    buffer.x = old_width;
    buffer.y += 1;
    // where `oldRow` is after this branch, and its cells if it leaves the list
    let old_row_after;
    let mut detached = None;
    if buffer.y == buffer.scroll_bottom.saturating_add(1) {
        buffer.y -= 1;
        old_row_after = old_row_after_scroll(buffer, old_row);
        if old_width > 0 && old_row_after.is_none() {
            detached = buffer.lines.get(old_row).cloned();
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
            Some(index) => buffer.lines.get(index).cloned(),
            None => detached,
        };
        if let (Some(source), Some(buffer_row)) = (source, line_mut(buffer, row)) {
            buffer_row.copy_cells_from(&source, old_col, 0, old_width, false);
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

#[cfg(test)]
#[path = "input_handler_tests.rs"]
mod tests;
