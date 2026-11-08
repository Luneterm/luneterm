//! Output exports (M8 plan step 3): `viewport`, `drain_events`, `take_damage` and `modes`.
//!
//! The viewport and the event drain are flattened into engine-owned *slots* (D3): arrays and byte
//! pools the returned structs point into. A slot is refilled only by the next call of its own
//! export (or released by `luneterm_engine_free`); every other call leaves it alone, so a pointer
//! stays readable and unchanged until then. A call that reaches its slot and fails after the
//! argument checks (`ERR_TOO_LARGE`) also leaves the slot unspecified; its out-param is zeroed,
//! so the host holds no pointer into it.
//!
//! Every conversion from a lune-vt type destructures or matches exhaustively (no `_ =>`, no
//! `..`), so a new `HostEvent` variant or a new `ViewportCell` field breaks the build instead of
//! being dropped.

use std::ptr::null;
use std::sync::Arc;

use lune_vt::{
    CellColor, ColorIndex, ColorRequest, CommandMarkKind, CursorStyle, Damage, HostEvent, Modes,
    MouseEncoding, MouseTracking, ProgressState, Terminal, UnderlineStyle, Viewport, ViewportCell,
    ViewportCursor, WindowReportKind,
};

use crate::guard::{LUNETERM_ERR_TOO_LARGE, LUNETERM_OK, LunetermEngine, out_init, with_engine};
use crate::types::{
    LUNETERM_CELL_BLINK, LUNETERM_CELL_BOLD, LUNETERM_CELL_DIM, LUNETERM_CELL_INVERSE,
    LUNETERM_CELL_INVISIBLE, LUNETERM_CELL_ITALIC, LUNETERM_CELL_OVERLINE,
    LUNETERM_CELL_STRIKETHROUGH, LUNETERM_COLOR_DEFAULT, LUNETERM_COLOR_INDEX_BACKGROUND,
    LUNETERM_COLOR_INDEX_CURSOR, LUNETERM_COLOR_INDEX_FOREGROUND, LUNETERM_COLOR_PALETTE,
    LUNETERM_COLOR_REQUEST_REPORT, LUNETERM_COLOR_REQUEST_RESTORE,
    LUNETERM_COLOR_REQUEST_RESTORE_ALL, LUNETERM_COLOR_REQUEST_SET, LUNETERM_COLOR_RGB,
    LUNETERM_CURSOR_BAR, LUNETERM_CURSOR_BLOCK, LUNETERM_CURSOR_UNDERLINE, LUNETERM_EVENT_A11Y_TAB,
    LUNETERM_EVENT_A11Y_TEXT, LUNETERM_EVENT_BELL, LUNETERM_EVENT_CLIPBOARD_WRITE,
    LUNETERM_EVENT_COLOR, LUNETERM_EVENT_COMMAND_MARK, LUNETERM_EVENT_CWD_CHANGED,
    LUNETERM_EVENT_FOCUS_REPORT_REQUESTED, LUNETERM_EVENT_HAS_EXIT_CODE, LUNETERM_EVENT_PROGRESS,
    LUNETERM_EVENT_RESPONSE, LUNETERM_EVENT_SYNC_UPDATE_IN_COMMAND, LUNETERM_EVENT_TITLE_CHANGED,
    LUNETERM_EVENT_UNKNOWN_OSC, LUNETERM_EVENT_WINDOW_REPORT, LUNETERM_MOUSE_ENCODING_DEFAULT,
    LUNETERM_MOUSE_ENCODING_SGR, LUNETERM_MOUSE_ENCODING_SGR_PIXELS, LUNETERM_MOUSE_TRACKING_ANY,
    LUNETERM_MOUSE_TRACKING_DRAG, LUNETERM_MOUSE_TRACKING_NONE, LUNETERM_MOUSE_TRACKING_VT200,
    LUNETERM_MOUSE_TRACKING_X10, LUNETERM_UNDERLINE_CURLY, LUNETERM_UNDERLINE_DASHED,
    LUNETERM_UNDERLINE_DOTTED, LUNETERM_UNDERLINE_DOUBLE, LUNETERM_UNDERLINE_NONE,
    LUNETERM_UNDERLINE_SINGLE, LunetermCell, LunetermColorRequest, LunetermDamage, LunetermEvent,
    LunetermEvents, LunetermLink, LunetermModes, LunetermViewport,
};

/// After a fill, a slot array keeps at most `2 x what the call needed + floor` capacity, so one
/// huge viewport or drain does not pin its memory for the handle's life. Floors are in elements
/// for the arrays and in bytes for the pools.
const ARRAY_FLOOR: usize = 4096;
const POOL_FLOOR: usize = 64 * 1024;

/// What `u32::try_from` accepts: the largest text offset or length a [`LunetermCell`] can hold.
const U32_LIMIT: usize = u32::MAX as usize;

/// The viewport slot: the arrays `LunetermViewport` points into.
#[derive(Debug, Default)]
pub(crate) struct ViewportSlot {
    cells: Vec<LunetermCell>,
    text: Vec<u8>,
    links: Vec<LunetermLink>,
    link_data: Vec<u8>,
    /// Held, not copied: a title can be megabytes.
    title: Arc<str>,
}

/// The events slot: the arrays `LunetermEvents` points into.
#[derive(Debug, Default)]
pub(crate) struct EventsSlot {
    events: Vec<LunetermEvent>,
    data: Vec<u8>,
    colors: Vec<LunetermColorRequest>,
}

/// Gives back capacity beyond `2 x len + floor` (the length is what the last fill needed).
fn trim<T>(vec: &mut Vec<T>, floor: usize) {
    let limit = vec.len().saturating_mul(2).saturating_add(floor);
    if vec.capacity() > limit {
        vec.shrink_to(limit);
    }
}

/// `usize` to `u64` (the crate builds for 64-bit targets only).
fn wide(value: usize) -> u64 {
    value as u64
}

/// The pointer a host gets for a slice: NULL when empty, never a dangling non-null.
fn array_ptr<T>(slice: &[T]) -> *const T {
    if slice.is_empty() {
        null()
    } else {
        slice.as_ptr()
    }
}

fn flag(value: bool) -> u8 {
    u8::from(value)
}

fn pack_color(color: CellColor) -> u32 {
    match color {
        CellColor::Default => LUNETERM_COLOR_DEFAULT << 24,
        CellColor::Palette(index) => (LUNETERM_COLOR_PALETTE << 24) | u32::from(index),
        CellColor::Rgb(r, g, b) => {
            (LUNETERM_COLOR_RGB << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        }
    }
}

fn underline_value(style: UnderlineStyle) -> u8 {
    match style {
        UnderlineStyle::None => LUNETERM_UNDERLINE_NONE,
        UnderlineStyle::Single => LUNETERM_UNDERLINE_SINGLE,
        UnderlineStyle::Double => LUNETERM_UNDERLINE_DOUBLE,
        UnderlineStyle::Curly => LUNETERM_UNDERLINE_CURLY,
        UnderlineStyle::Dotted => LUNETERM_UNDERLINE_DOTTED,
        UnderlineStyle::Dashed => LUNETERM_UNDERLINE_DASHED,
    }
}

/// Appends `cell`'s text to the pool and returns its C cell. `limit` is the largest offset and
/// length that fits `uint32_t` (injectable so a test can overflow it); an empty text has offset 0.
fn convert_cell(
    cell: &ViewportCell,
    text_pool: &mut Vec<u8>,
    limit: usize,
) -> Result<LunetermCell, i32> {
    let ViewportCell {
        text,
        width,
        fg,
        bg,
        bold,
        dim,
        italic,
        blink,
        inverse,
        invisible,
        strikethrough,
        overline,
        underline,
        underline_color,
        link_id,
    } = cell;
    let (text_offset, text_len) = if text.is_empty() {
        (0, 0)
    } else {
        let offset = text_pool.len();
        if offset > limit || text.len() > limit {
            return Err(LUNETERM_ERR_TOO_LARGE);
        }
        text_pool.extend_from_slice(text.as_bytes());
        // Both are at most `limit`, which callers keep within `u32`.
        (
            u32::try_from(offset).map_err(|_| LUNETERM_ERR_TOO_LARGE)?,
            u32::try_from(text.len()).map_err(|_| LUNETERM_ERR_TOO_LARGE)?,
        )
    };
    let mut flags = 0;
    for (set, bit) in [
        (*bold, LUNETERM_CELL_BOLD),
        (*dim, LUNETERM_CELL_DIM),
        (*italic, LUNETERM_CELL_ITALIC),
        (*blink, LUNETERM_CELL_BLINK),
        (*inverse, LUNETERM_CELL_INVERSE),
        (*invisible, LUNETERM_CELL_INVISIBLE),
        (*strikethrough, LUNETERM_CELL_STRIKETHROUGH),
        (*overline, LUNETERM_CELL_OVERLINE),
    ] {
        if set {
            flags |= bit;
        }
    }
    Ok(LunetermCell {
        text_offset,
        text_len,
        fg: pack_color(*fg),
        bg: pack_color(*bg),
        underline_color: pack_color(*underline_color),
        link_id: *link_id,
        flags,
        width: *width,
        underline: underline_value(*underline),
    })
}

/// Refills `slot` from `term`'s screen and returns the struct pointing into it. `limit` is as
/// for [`convert_cell`]. On error the slot's contents are unspecified.
///
/// The cells are converted row by row as the engine produces them
/// ([`Terminal::viewport_with_rows`]), so the only full-screen copy is the slot itself: a
/// whole owned `Viewport` (a `String` per cell) would cost about 70 bytes per cell on top, which
/// is 0.5 GiB more at the 4096 x 2048 cap.
fn flatten_viewport(
    term: &Terminal,
    slot: &mut ViewportSlot,
    limit: usize,
) -> Result<LunetermViewport, i32> {
    // The slot is cleared and reserved before the walk; a failed row leaves the rest unread.
    slot.cells.clear();
    slot.text.clear();
    slot.links.clear();
    slot.link_data.clear();
    slot.cells.reserve(term.cols().saturating_mul(term.rows()));
    let mut failure = None;
    let viewport = term.viewport_with_rows(|row| {
        if failure.is_some() {
            return;
        }
        for cell in row {
            match convert_cell(cell, &mut slot.text, limit) {
                Ok(converted) => slot.cells.push(converted),
                Err(code) => {
                    failure = Some(code);
                    return;
                }
            }
        }
    });
    if let Some(code) = failure {
        return Err(code);
    }
    let Viewport {
        cols,
        rows,
        cells: _,
        cursor,
        viewport_top,
        base_y,
        total_lines,
        is_at_bottom,
        title,
        alt_screen,
        links,
    } = &viewport;
    let ViewportCursor {
        x,
        y,
        visible,
        style,
        blink,
    } = *cursor;
    let cols32 = u32::try_from(*cols).map_err(|_| LUNETERM_ERR_TOO_LARGE)?;
    let rows32 = u32::try_from(*rows).map_err(|_| LUNETERM_ERR_TOO_LARGE)?;
    let cursor_x = u32::try_from(x).map_err(|_| LUNETERM_ERR_TOO_LARGE)?;
    let cursor_y = u32::try_from(y).map_err(|_| LUNETERM_ERR_TOO_LARGE)?;

    // The ABI promises `cell_count == cols * rows`; a snapshot that breaks it is an engine bug,
    // and a host indexing past the array is worse than a poisoned handle (a caught panic).
    assert_eq!(
        slot.cells.len(),
        cols.saturating_mul(*rows),
        "viewport cell count"
    );
    for (id, uri) in links {
        slot.links.push(LunetermLink {
            id: *id,
            reserved: 0,
            offset: wide(slot.link_data.len()),
            len: wide(uri.len()),
        });
        slot.link_data.extend_from_slice(uri.as_bytes());
    }
    slot.title = Arc::clone(title);
    trim(&mut slot.cells, ARRAY_FLOOR);
    trim(&mut slot.text, POOL_FLOOR);
    trim(&mut slot.links, ARRAY_FLOOR);
    trim(&mut slot.link_data, POOL_FLOOR);

    Ok(LunetermViewport {
        cols: cols32,
        rows: rows32,
        cells: array_ptr(&slot.cells),
        cell_count: wide(slot.cells.len()),
        text: array_ptr(&slot.text),
        text_len: wide(slot.text.len()),
        links: array_ptr(&slot.links),
        link_count: wide(slot.links.len()),
        link_data: array_ptr(&slot.link_data),
        link_data_len: wide(slot.link_data.len()),
        title: array_ptr(slot.title.as_bytes()),
        title_len: wide(slot.title.len()),
        viewport_top: wide(*viewport_top),
        base_y: wide(*base_y),
        total_lines: wide(*total_lines),
        cursor_x,
        cursor_y,
        cursor_visible: flag(visible),
        cursor_style: match style {
            CursorStyle::Block => LUNETERM_CURSOR_BLOCK,
            CursorStyle::Underline => LUNETERM_CURSOR_UNDERLINE,
            CursorStyle::Bar => LUNETERM_CURSOR_BAR,
        },
        cursor_blink: flag(blink),
        is_at_bottom: flag(*is_at_bottom),
        alt_screen: flag(*alt_screen),
        reserved: [0; 3],
    })
}

fn color_index_value(index: ColorIndex) -> u32 {
    match index {
        ColorIndex::Palette(entry) => u32::from(entry),
        ColorIndex::Foreground => LUNETERM_COLOR_INDEX_FOREGROUND,
        ColorIndex::Background => LUNETERM_COLOR_INDEX_BACKGROUND,
        ColorIndex::Cursor => LUNETERM_COLOR_INDEX_CURSOR,
    }
}

fn convert_color_request(request: &ColorRequest) -> LunetermColorRequest {
    let mut out = LunetermColorRequest::default();
    match *request {
        ColorRequest::Report(index) => {
            out.kind = LUNETERM_COLOR_REQUEST_REPORT;
            out.index = color_index_value(index);
        }
        ColorRequest::Set(index, [r, g, b]) => {
            out.kind = LUNETERM_COLOR_REQUEST_SET;
            out.index = color_index_value(index);
            (out.r, out.g, out.b) = (r, g, b);
        }
        ColorRequest::Restore(Some(index)) => {
            out.kind = LUNETERM_COLOR_REQUEST_RESTORE;
            out.index = color_index_value(index);
        }
        ColorRequest::Restore(None) => out.kind = LUNETERM_COLOR_REQUEST_RESTORE_ALL,
    }
    out
}

/// Sets `event`'s `offset`/`len` to a new range of the data pool holding `bytes`.
fn put_data(pool: &mut Vec<u8>, event: &mut LunetermEvent, bytes: &[u8]) {
    event.offset = wide(pool.len());
    event.len = wide(bytes.len());
    pool.extend_from_slice(bytes);
}

fn convert_event(event: &HostEvent, slot: &mut EventsSlot) -> LunetermEvent {
    let mut out = LunetermEvent::default();
    match event {
        HostEvent::Response(bytes) => {
            out.kind = LUNETERM_EVENT_RESPONSE;
            put_data(&mut slot.data, &mut out, bytes);
        }
        HostEvent::Bell => out.kind = LUNETERM_EVENT_BELL,
        HostEvent::TitleChanged => out.kind = LUNETERM_EVENT_TITLE_CHANGED,
        HostEvent::Color(requests) => {
            out.kind = LUNETERM_EVENT_COLOR;
            out.offset = wide(slot.colors.len());
            out.len = wide(requests.len());
            slot.colors
                .extend(requests.iter().map(convert_color_request));
        }
        HostEvent::FocusReportRequested => out.kind = LUNETERM_EVENT_FOCUS_REPORT_REQUESTED,
        HostEvent::WindowReport(kind) => {
            out.kind = LUNETERM_EVENT_WINDOW_REPORT;
            out.value = match kind {
                WindowReportKind::WinSizePixels => 0,
                WindowReportKind::CellSizePixels => 1,
            };
        }
        HostEvent::A11yText(text) => {
            out.kind = LUNETERM_EVENT_A11Y_TEXT;
            put_data(&mut slot.data, &mut out, text.as_bytes());
        }
        HostEvent::A11yTab(count) => {
            out.kind = LUNETERM_EVENT_A11Y_TAB;
            out.value = i32::try_from(*count).unwrap_or(i32::MAX);
        }
        HostEvent::UnknownOsc { ident, len } => {
            out.kind = LUNETERM_EVENT_UNKNOWN_OSC;
            out.value = *ident;
            // The payload length is child-chosen: it goes in `value2` (saturated), never in
            // `len`, so `offset`/`len` stay a range of the data pool for every event.
            out.value2 = i32::try_from(*len).unwrap_or(i32::MAX);
        }
        HostEvent::ClipboardWrite(text) => {
            out.kind = LUNETERM_EVENT_CLIPBOARD_WRITE;
            put_data(&mut slot.data, &mut out, text.as_bytes());
        }
        HostEvent::CwdChanged(text) => {
            out.kind = LUNETERM_EVENT_CWD_CHANGED;
            put_data(&mut slot.data, &mut out, text.as_bytes());
        }
        HostEvent::Progress(progress) => {
            out.kind = LUNETERM_EVENT_PROGRESS;
            out.value = match progress.state {
                ProgressState::None => 0,
                ProgressState::Normal => 1,
                ProgressState::Error => 2,
                ProgressState::Indeterminate => 3,
                ProgressState::Warning => 4,
            };
            out.value2 = i32::from(progress.percent);
        }
        HostEvent::CommandMark(mark) => {
            out.kind = LUNETERM_EVENT_COMMAND_MARK;
            out.value = match mark.kind {
                CommandMarkKind::PromptStart => 0,
                CommandMarkKind::PromptEnd => 1,
                CommandMarkKind::OutputStart => 2,
                CommandMarkKind::CommandFinished => 3,
            };
            if let Some(code) = mark.exit_code {
                out.value2 = code;
                out.flags = LUNETERM_EVENT_HAS_EXIT_CODE;
            }
        }
        HostEvent::SyncUpdateInCommand => out.kind = LUNETERM_EVENT_SYNC_UPDATE_IN_COMMAND,
    }
    out
}

/// Refills `slot` from `events` and returns the struct pointing into it.
fn flatten_events(events: &[HostEvent], slot: &mut EventsSlot) -> LunetermEvents {
    slot.events.clear();
    slot.data.clear();
    slot.colors.clear();
    for event in events {
        let converted = convert_event(event, slot);
        slot.events.push(converted);
    }
    trim(&mut slot.events, ARRAY_FLOOR);
    trim(&mut slot.data, POOL_FLOOR);
    trim(&mut slot.colors, ARRAY_FLOOR);
    LunetermEvents {
        events: array_ptr(&slot.events),
        count: wide(slot.events.len()),
        data: array_ptr(&slot.data),
        data_len: wide(slot.data.len()),
        colors: array_ptr(&slot.colors),
        color_count: wide(slot.colors.len()),
    }
}

fn convert_damage(damage: Damage) -> LunetermDamage {
    let Damage {
        rows,
        full,
        cursor_moved,
        scrolled,
    } = damage;
    // Rows are viewport rows, at most `LUNETERM_MAX_ROWS`; saturating keeps this infallible,
    // because the damage is already taken and cannot be put back.
    let row32 = |row: usize| u32::try_from(row).unwrap_or(u32::MAX);
    let (has_rows, first_row, last_row) = match rows {
        Some((first, last)) => (1, row32(first), row32(last)),
        None => (0, 0, 0),
    };
    LunetermDamage {
        has_rows,
        full: flag(full),
        cursor_moved: flag(cursor_moved),
        scrolled: flag(scrolled),
        first_row,
        last_row,
    }
}

fn convert_modes(modes: Modes) -> LunetermModes {
    let Modes {
        application_cursor_keys,
        application_keypad,
        bracketed_paste,
        insert,
        origin,
        reverse_wraparound,
        send_focus,
        synchronized_output,
        wraparound,
        cursor_hidden,
        mouse_tracking,
        mouse_encoding,
        kitty_keyboard_flags,
        modify_other_keys,
    } = modes;
    LunetermModes {
        application_cursor_keys: flag(application_cursor_keys),
        application_keypad: flag(application_keypad),
        bracketed_paste: flag(bracketed_paste),
        insert: flag(insert),
        origin: flag(origin),
        reverse_wraparound: flag(reverse_wraparound),
        send_focus: flag(send_focus),
        synchronized_output: flag(synchronized_output),
        wraparound: flag(wraparound),
        cursor_hidden: flag(cursor_hidden),
        mouse_tracking: match mouse_tracking {
            MouseTracking::None => LUNETERM_MOUSE_TRACKING_NONE,
            MouseTracking::X10 => LUNETERM_MOUSE_TRACKING_X10,
            MouseTracking::Vt200 => LUNETERM_MOUSE_TRACKING_VT200,
            MouseTracking::Drag => LUNETERM_MOUSE_TRACKING_DRAG,
            MouseTracking::Any => LUNETERM_MOUSE_TRACKING_ANY,
        },
        mouse_encoding: match mouse_encoding {
            MouseEncoding::Default => LUNETERM_MOUSE_ENCODING_DEFAULT,
            MouseEncoding::Sgr => LUNETERM_MOUSE_ENCODING_SGR,
            MouseEncoding::SgrPixels => LUNETERM_MOUSE_ENCODING_SGR_PIXELS,
        },
        kitty_keyboard_flags,
        modify_other_keys,
    }
}

/// The body of `luneterm_engine_viewport`, with the `uint32_t` limit injectable.
///
/// # Safety
///
/// As `luneterm_engine_viewport`.
unsafe fn viewport_with_limit(
    engine: *mut LunetermEngine,
    out: *mut LunetermViewport,
    limit: usize,
) -> i32 {
    // SAFETY: forwarded from the caller's contract on `out`.
    let out = match unsafe { out_init(out, LunetermViewport::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from the caller's contract on `engine`.
    let result = unsafe {
        with_engine(engine, |inner| {
            flatten_viewport(&inner.term, &mut inner.viewport, limit)
        })
    };
    match result {
        Ok(viewport) => {
            *out = viewport;
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

/// Snapshots the screen into the handle's viewport slot. `*out` is zeroed first and filled only
/// on `OK`; its pointers stay valid until the next `luneterm_engine_viewport` on this handle (or
/// `luneterm_engine_free`). `ERR_TOO_LARGE`: the cell text pool does not fit `uint32_t` offsets.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_viewport(
    engine: *mut LunetermEngine,
    out: *mut LunetermViewport,
) -> i32 {
    // SAFETY: forwarded from this function's contract.
    unsafe { viewport_with_limit(engine, out, U32_LIMIT) }
}

/// Drains the event queue into the handle's events slot, in order. `*out` is zeroed first and
/// filled only on `OK`; its pointers stay valid until the next `luneterm_engine_drain_events` on
/// this handle (or `luneterm_engine_free`). An empty queue is `count == 0` with NULL pointers.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_drain_events(
    engine: *mut LunetermEngine,
    out: *mut LunetermEvents,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermEvents::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    let result = unsafe {
        with_engine(engine, |inner| {
            let events = inner.term.drain_events();
            Ok(flatten_events(&events, &mut inner.events))
        })
    };
    match result {
        Ok(events) => {
            *out = events;
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

/// Takes the damage accumulated since the last call.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_take_damage(
    engine: *mut LunetermEngine,
    out: *mut LunetermDamage,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermDamage::default()) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    let result =
        unsafe { with_engine(engine, |inner| Ok(convert_damage(inner.term.take_damage()))) };
    match result {
        Ok(damage) => {
            *out = damage;
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

/// Reads the terminal modes.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_modes(
    engine: *mut LunetermEngine,
    out: *mut LunetermModes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermModes::default()) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    let result = unsafe { with_engine(engine, |inner| Ok(convert_modes(inner.term.modes()))) };
    match result {
        Ok(modes) => {
            *out = modes;
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    //! The output exports against a second `Terminal` fed the same bytes: the C structs are
    //! decoded back into lune-vt types (with literal numbers, not this module's constants, so a
    //! wrong constant fails) and compared after every step. Known-answer tests pin the packing
    //! with literals as well.
    //!
    //! Axes of the differential (the lessons say to list them): odd and even columns (3, 80, 81),
    //! one and many rows, Unicode 6 and 11-vs16, wide and VS16 and 64+ code point clusters, every
    //! colour kind on fg, bg and underline colour, every attribute and underline style, shared and
    //! distinct links and a 64 KiB uri, wrap pending and hidden cursor, scrollback, alt screen,
    //! empty and 1 MiB titles, resize, and every `HostEvent` variant.

    use std::collections::{BTreeMap, BTreeSet};
    use std::ptr::null_mut;

    use lune_vt::services::options_service::{OptionValue, WindowOptions};
    use lune_vt::{ProgressState as Ps, StandardExtensions, Terminal};

    use super::*;
    use crate::guard::{LUNETERM_ERR_INVALID_ARGUMENT, LUNETERM_ERR_NULL_POINTER, lock_for_test};
    use crate::{
        LUNETERM_UNICODE_6, LUNETERM_UNICODE_11, LUNETERM_UNICODE_11_VS16, LunetermConfig,
        luneterm_engine_free, luneterm_engine_new, luneterm_engine_resize,
        luneterm_engine_scroll_lines, luneterm_engine_scroll_to_bottom,
        luneterm_engine_scroll_to_top, luneterm_engine_write,
    };

    // ---- raw access -------------------------------------------------------------------------

    fn slice<'a, T>(ptr: *const T, len: u64) -> &'a [T] {
        if len == 0 {
            assert!(ptr.is_null(), "an empty array or pool must be NULL");
            return &[];
        }
        assert!(!ptr.is_null(), "a non-empty array or pool must not be NULL");
        // SAFETY: `ptr`/`len` come from a slot the test keeps alive and does not refill while it
        // reads; the slot holds `len` initialized elements.
        unsafe { std::slice::from_raw_parts(ptr, usize::try_from(len).expect("len")) }
    }

    fn config(
        cols: u32,
        rows: u32,
        scrollback: u32,
        unicode: u32,
        clipboard: bool,
    ) -> LunetermConfig {
        LunetermConfig {
            cols,
            rows,
            scrollback,
            unicode,
            xtversion: std::ptr::null(),
            xtversion_len: 0,
            clipboard_write: u8::from(clipboard),
            reserved: [0; 7],
        }
    }

    fn new_engine(config: &LunetermConfig) -> *mut LunetermEngine {
        let mut engine = null_mut();
        // SAFETY: a live config and out-param.
        assert_eq!(
            unsafe { luneterm_engine_new(config, &mut engine) },
            LUNETERM_OK
        );
        engine
    }

    fn write_to(engine: *mut LunetermEngine, bytes: &[u8]) {
        // SAFETY: a live handle and a live slice.
        let code = unsafe { luneterm_engine_write(engine, bytes.as_ptr(), bytes.len()) };
        assert_eq!(code, LUNETERM_OK);
    }

    fn viewport_of(engine: *mut LunetermEngine) -> LunetermViewport {
        let mut out = LunetermViewport::EMPTY;
        // SAFETY: a live handle and a live out-param.
        assert_eq!(
            unsafe { luneterm_engine_viewport(engine, &mut out) },
            LUNETERM_OK
        );
        out
    }

    fn events_of(engine: *mut LunetermEngine) -> LunetermEvents {
        let mut out = LunetermEvents::EMPTY;
        // SAFETY: a live handle and a live out-param.
        let code = unsafe { luneterm_engine_drain_events(engine, &mut out) };
        assert_eq!(code, LUNETERM_OK);
        out
    }

    fn damage_of(engine: *mut LunetermEngine) -> LunetermDamage {
        let mut out = LunetermDamage::default();
        // SAFETY: a live handle and a live out-param.
        assert_eq!(
            unsafe { luneterm_engine_take_damage(engine, &mut out) },
            LUNETERM_OK
        );
        out
    }

    fn modes_of(engine: *mut LunetermEngine) -> LunetermModes {
        let mut out = LunetermModes::default();
        // SAFETY: a live handle and a live out-param.
        assert_eq!(
            unsafe { luneterm_engine_modes(engine, &mut out) },
            LUNETERM_OK
        );
        out
    }

    /// Runs `f` on the engine behind a live handle, under its lock.
    fn with_inner<R>(
        engine: *mut LunetermEngine,
        f: impl FnOnce(&mut crate::guard::Inner) -> R,
    ) -> R {
        // SAFETY: a live handle; shared reference, as the exports take it.
        let shared = unsafe { &*engine };
        f(&mut lock_for_test(shared))
    }

    // ---- decoders (literal numbers on purpose) -----------------------------------------------

    fn unpack_color(packed: u32) -> CellColor {
        match packed >> 24 {
            0 => {
                assert_eq!(packed, 0);
                CellColor::Default
            }
            1 => {
                assert_eq!(packed & 0x00FF_FF00, 0, "palette bits 8-23 are zero");
                CellColor::Palette((packed & 0xFF) as u8)
            }
            2 => CellColor::Rgb(
                ((packed >> 16) & 0xFF) as u8,
                ((packed >> 8) & 0xFF) as u8,
                (packed & 0xFF) as u8,
            ),
            kind => panic!("unknown colour kind {kind}"),
        }
    }

    fn decode_cell(cell: &LunetermCell, text_pool: &[u8]) -> ViewportCell {
        let start = cell.text_offset as usize;
        let text = String::from_utf8(text_pool[start..start + cell.text_len as usize].to_vec())
            .expect("cell text is UTF-8");
        assert_eq!(cell.flags >> 8, 0, "flag bits 8-15 are zero");
        let bit = |n: u16| cell.flags & (1 << n) != 0;
        ViewportCell {
            text,
            width: cell.width,
            fg: unpack_color(cell.fg),
            bg: unpack_color(cell.bg),
            bold: bit(0),
            dim: bit(1),
            italic: bit(2),
            blink: bit(3),
            inverse: bit(4),
            invisible: bit(5),
            strikethrough: bit(6),
            overline: bit(7),
            underline: match cell.underline {
                0 => UnderlineStyle::None,
                1 => UnderlineStyle::Single,
                2 => UnderlineStyle::Double,
                3 => UnderlineStyle::Curly,
                4 => UnderlineStyle::Dotted,
                5 => UnderlineStyle::Dashed,
                other => panic!("unknown underline {other}"),
            },
            underline_color: unpack_color(cell.underline_color),
            link_id: cell.link_id,
        }
    }

    fn assert_viewport(vp: &LunetermViewport, expected: &Viewport, what: &str) {
        let (cols, rows) = (expected.cols, expected.rows);
        assert_eq!(
            (vp.cols as usize, vp.rows as usize),
            (cols, rows),
            "{what}: size"
        );
        assert_eq!(vp.cell_count as usize, cols * rows, "{what}: cell_count");
        let cells = slice(vp.cells, vp.cell_count);
        let text = slice(vp.text, vp.text_len);
        let mut text_used = 0usize;
        for (i, cell) in cells.iter().enumerate() {
            let (y, x) = (i / cols, i % cols);
            assert_eq!(
                decode_cell(cell, text),
                expected.cells[y][x],
                "{what}: cell ({x},{y})"
            );
            text_used += cell.text_len as usize;
        }
        assert_eq!(
            text_used,
            text.len(),
            "{what}: the text pool holds exactly the cells' text"
        );
        let link_data = slice(vp.link_data, vp.link_data_len);
        let mut links = BTreeMap::new();
        let mut link_used = 0usize;
        for link in slice(vp.links, vp.link_count) {
            assert_eq!(link.reserved, 0);
            let (start, len) = (link.offset as usize, link.len as usize);
            let uri = String::from_utf8(link_data[start..start + len].to_vec()).expect("uri");
            assert!(
                links.insert(link.id, uri).is_none(),
                "{what}: duplicate link id"
            );
            link_used += len;
        }
        assert_eq!(links, expected.links, "{what}: links");
        assert_eq!(
            link_used,
            link_data.len(),
            "{what}: the link pool holds exactly the uris"
        );
        assert!(
            slice(vp.links, vp.link_count)
                .windows(2)
                .all(|w| w[0].id < w[1].id),
            "{what}: links ascend by id"
        );
        assert_eq!(
            slice(vp.title, vp.title_len),
            expected.title.as_bytes(),
            "{what}: title"
        );
        assert_eq!(
            (vp.cursor_x as usize, vp.cursor_y as usize),
            (expected.cursor.x, expected.cursor.y),
            "{what}: cursor position"
        );
        assert_eq!(
            vp.cursor_visible,
            u8::from(expected.cursor.visible),
            "{what}: cursor visible"
        );
        assert_eq!(
            vp.cursor_style,
            match expected.cursor.style {
                CursorStyle::Block => 0,
                CursorStyle::Underline => 1,
                CursorStyle::Bar => 2,
            },
            "{what}: cursor style"
        );
        assert_eq!(
            vp.cursor_blink,
            u8::from(expected.cursor.blink),
            "{what}: cursor blink"
        );
        assert_eq!(
            (vp.viewport_top, vp.base_y, vp.total_lines),
            (
                expected.viewport_top as u64,
                expected.base_y as u64,
                expected.total_lines as u64
            ),
            "{what}: scroll position"
        );
        assert_eq!(
            vp.is_at_bottom,
            u8::from(expected.is_at_bottom),
            "{what}: is_at_bottom"
        );
        assert_eq!(
            vp.alt_screen,
            u8::from(expected.alt_screen),
            "{what}: alt_screen"
        );
        assert_eq!(vp.reserved, [0; 3]);
    }

    fn color_index(index: u32) -> ColorIndex {
        match index {
            0..=255 => ColorIndex::Palette(u8::try_from(index).expect("palette")),
            256 => ColorIndex::Foreground,
            257 => ColorIndex::Background,
            258 => ColorIndex::Cursor,
            other => panic!("unknown colour index {other}"),
        }
    }

    fn decode_events(ev: &LunetermEvents) -> Vec<HostEvent> {
        let data = slice(ev.data, ev.data_len);
        let colors = slice(ev.colors, ev.color_count);
        let bytes = |e: &LunetermEvent| {
            let start = e.offset as usize;
            data[start..start + e.len as usize].to_vec()
        };
        let text = |e: &LunetermEvent| String::from_utf8(bytes(e)).expect("event text is UTF-8");
        let mut data_used = 0usize;
        let mut colors_used = 0usize;
        let decoded: Vec<HostEvent> = slice(ev.events, ev.count)
            .iter()
            .map(|e| {
                let plain = |event| {
                    assert_eq!(
                        (e.value, e.value2, e.flags, e.offset, e.len),
                        (0, 0, 0, 0, 0)
                    );
                    event
                };
                match e.kind {
                    1 => {
                        data_used += e.len as usize;
                        HostEvent::Response(bytes(e))
                    }
                    2 => plain(HostEvent::Bell),
                    3 => plain(HostEvent::TitleChanged),
                    4 => {
                        let start = e.offset as usize;
                        let requests = &colors[start..start + e.len as usize];
                        colors_used += requests.len();
                        HostEvent::Color(
                            requests
                                .iter()
                                .map(|r| {
                                    assert_eq!(r.reserved, 0);
                                    match r.kind {
                                        0 => ColorRequest::Report(color_index(r.index)),
                                        1 => {
                                            ColorRequest::Set(color_index(r.index), [r.r, r.g, r.b])
                                        }
                                        2 => ColorRequest::Restore(Some(color_index(r.index))),
                                        3 => {
                                            assert_eq!(r.index, 0);
                                            ColorRequest::Restore(None)
                                        }
                                        other => panic!("unknown colour request {other}"),
                                    }
                                })
                                .collect(),
                        )
                    }
                    5 => plain(HostEvent::FocusReportRequested),
                    6 => HostEvent::WindowReport(match e.value {
                        0 => WindowReportKind::WinSizePixels,
                        1 => WindowReportKind::CellSizePixels,
                        other => panic!("unknown window report {other}"),
                    }),
                    7 => {
                        data_used += e.len as usize;
                        HostEvent::A11yText(text(e))
                    }
                    8 => HostEvent::A11yTab(usize::try_from(e.value).expect("tab count")),
                    9 => HostEvent::UnknownOsc {
                        ident: e.value,
                        len: usize::try_from(e.value2).expect("payload length"),
                    },
                    10 => {
                        data_used += e.len as usize;
                        HostEvent::ClipboardWrite(text(e))
                    }
                    11 => {
                        data_used += e.len as usize;
                        HostEvent::CwdChanged(text(e))
                    }
                    12 => HostEvent::Progress(lune_vt::Progress {
                        state: match e.value {
                            0 => Ps::None,
                            1 => Ps::Normal,
                            2 => Ps::Error,
                            3 => Ps::Indeterminate,
                            4 => Ps::Warning,
                            other => panic!("unknown progress state {other}"),
                        },
                        percent: u8::try_from(e.value2).expect("percent"),
                    }),
                    13 => HostEvent::CommandMark(lune_vt::CommandMark {
                        kind: match e.value {
                            0 => CommandMarkKind::PromptStart,
                            1 => CommandMarkKind::PromptEnd,
                            2 => CommandMarkKind::OutputStart,
                            3 => CommandMarkKind::CommandFinished,
                            other => panic!("unknown mark {other}"),
                        },
                        exit_code: (e.flags & 1 != 0).then_some(e.value2),
                    }),
                    14 => plain(HostEvent::SyncUpdateInCommand),
                    other => panic!("unknown event kind {other}"),
                }
            })
            .collect();
        assert_eq!(
            data_used,
            data.len(),
            "the data pool holds exactly the events' bytes"
        );
        assert_eq!(
            colors_used,
            colors.len(),
            "the colour pool holds exactly the requests"
        );
        decoded
    }

    fn decode_damage(d: &LunetermDamage) -> Damage {
        let rows = if d.has_rows != 0 {
            Some((d.first_row as usize, d.last_row as usize))
        } else {
            assert_eq!((d.first_row, d.last_row), (0, 0));
            None
        };
        Damage {
            rows,
            full: d.full != 0,
            cursor_moved: d.cursor_moved != 0,
            scrolled: d.scrolled != 0,
        }
    }

    fn decode_modes(m: &LunetermModes) -> Modes {
        let b = |v: u8| {
            assert!(v <= 1, "booleans are 0 or 1");
            v == 1
        };
        Modes {
            application_cursor_keys: b(m.application_cursor_keys),
            application_keypad: b(m.application_keypad),
            bracketed_paste: b(m.bracketed_paste),
            insert: b(m.insert),
            origin: b(m.origin),
            reverse_wraparound: b(m.reverse_wraparound),
            send_focus: b(m.send_focus),
            synchronized_output: b(m.synchronized_output),
            wraparound: b(m.wraparound),
            cursor_hidden: b(m.cursor_hidden),
            mouse_tracking: match m.mouse_tracking {
                0 => MouseTracking::None,
                1 => MouseTracking::X10,
                2 => MouseTracking::Vt200,
                3 => MouseTracking::Drag,
                4 => MouseTracking::Any,
                other => panic!("unknown mouse tracking {other}"),
            },
            mouse_encoding: match m.mouse_encoding {
                0 => MouseEncoding::Default,
                1 => MouseEncoding::Sgr,
                2 => MouseEncoding::SgrPixels,
                other => panic!("unknown mouse encoding {other}"),
            },
            kitty_keyboard_flags: m.kitty_keyboard_flags,
            modify_other_keys: m.modify_other_keys,
        }
    }

    // ---- the pair -----------------------------------------------------------------------------

    enum Step {
        Write(Vec<u8>),
        Scroll(i32),
        Top,
        Bottom,
        Resize(u32, u32),
    }

    fn w(bytes: impl AsRef<[u8]>) -> Step {
        Step::Write(bytes.as_ref().to_vec())
    }

    /// An FFI engine and a reference `Terminal` built the same way.
    struct Pair {
        engine: *mut LunetermEngine,
        reference: Terminal,
        /// Kinds of the events the reference produced (C `kind` numbers).
        kinds: BTreeSet<u32>,
    }

    impl Pair {
        fn new(cols: u32, rows: u32, scrollback: u32, unicode: u32, clipboard: bool) -> Pair {
            let engine = new_engine(&config(cols, rows, scrollback, unicode, clipboard));
            let mut reference = Terminal::new([
                OptionValue::Cols(cols.max(2) as usize),
                OptionValue::Rows(rows.max(1) as usize),
                OptionValue::Scrollback(scrollback as usize),
            ]);
            reference
                .install_standard_extensions(&StandardExtensions {
                    xtversion: None,
                    vs16: unicode == LUNETERM_UNICODE_11_VS16,
                    clipboard_write: clipboard,
                })
                .expect("extensions");
            if unicode == LUNETERM_UNICODE_11 {
                reference
                    .unicode_mut()
                    .set_active_version("11")
                    .expect("unicode 11");
            }
            Pair {
                engine,
                reference,
                kinds: BTreeSet::new(),
            }
        }

        fn option(&mut self, make: impl Fn() -> OptionValue) {
            with_inner(self.engine, |inner| {
                inner.term.set_option(make()).expect("option")
            });
            self.reference.set_option(make()).expect("option");
        }

        fn apply(&mut self, step: &Step) {
            // SAFETY (all arms): a live handle and live arguments.
            match step {
                Step::Write(bytes) => {
                    write_to(self.engine, bytes);
                    self.reference.write(bytes);
                }
                Step::Scroll(delta) => {
                    assert_eq!(
                        unsafe { luneterm_engine_scroll_lines(self.engine, *delta) },
                        LUNETERM_OK
                    );
                    self.reference.scroll_lines(i64::from(*delta));
                }
                Step::Top => {
                    assert_eq!(
                        unsafe { luneterm_engine_scroll_to_top(self.engine) },
                        LUNETERM_OK
                    );
                    self.reference.scroll_to_top();
                }
                Step::Bottom => {
                    assert_eq!(
                        unsafe { luneterm_engine_scroll_to_bottom(self.engine) },
                        LUNETERM_OK
                    );
                    self.reference.scroll_to_bottom();
                }
                Step::Resize(cols, rows) => {
                    assert_eq!(
                        unsafe { luneterm_engine_resize(self.engine, *cols, *rows) },
                        LUNETERM_OK
                    );
                    self.reference.resize(*cols as usize, *rows as usize);
                }
            }
        }

        /// Compares viewport, events, damage and modes with the reference (which consumes the
        /// reference's queued events and damage, as the FFI calls consume the engine's).
        fn check(&mut self, what: &str) {
            let vp = viewport_of(self.engine);
            assert_viewport(&vp, &self.reference.viewport(), what);
            let expected = self.reference.drain_events();
            for event in &expected {
                self.kinds.insert(kind_of(event));
            }
            let events = events_of(self.engine);
            assert_eq!(decode_events(&events), expected, "{what}: events");
            let damage = damage_of(self.engine);
            assert_eq!(
                decode_damage(&damage),
                self.reference.take_damage(),
                "{what}: damage"
            );
            let modes = modes_of(self.engine);
            assert_eq!(
                decode_modes(&modes),
                self.reference.modes(),
                "{what}: modes"
            );
        }

        fn run(&mut self, steps: &[Step], label: &str) {
            self.check(&format!("{label} initial"));
            for (i, step) in steps.iter().enumerate() {
                self.apply(step);
                self.check(&format!("{label} step {i}"));
            }
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            // SAFETY: a live handle, used for the last time.
            assert_eq!(unsafe { luneterm_engine_free(self.engine) }, LUNETERM_OK);
        }
    }

    fn kind_of(event: &HostEvent) -> u32 {
        match event {
            HostEvent::Response(_) => 1,
            HostEvent::Bell => 2,
            HostEvent::TitleChanged => 3,
            HostEvent::Color(_) => 4,
            HostEvent::FocusReportRequested => 5,
            HostEvent::WindowReport(_) => 6,
            HostEvent::A11yText(_) => 7,
            HostEvent::A11yTab(_) => 8,
            HostEvent::UnknownOsc { .. } => 9,
            HostEvent::ClipboardWrite(_) => 10,
            HostEvent::CwdChanged(_) => 11,
            HostEvent::Progress(_) => 12,
            HostEvent::CommandMark(_) => 13,
            HostEvent::SyncUpdateInCommand => 14,
        }
    }

    // ---- scripts ------------------------------------------------------------------------------

    fn clear() -> &'static str {
        "\x1b[0m\x1b[2J\x1b[H"
    }

    fn content_script(cols: u32) -> Vec<Step> {
        let long_uri = format!(
            "\x1b]8;;https://example.com/{}\x1b\\long\x1b]8;;\x1b\\",
            "u".repeat(65_536)
        );
        let big_title = format!("\x1b]2;{}\x07", "t".repeat(1 << 20));
        let mut steps = vec![
            w("hello"),
            // wide characters, one at the last column (it does not fit: wraps)
            w(format!(
                "{}\u{3042}\u{3044}\u{3046}\x1b[1;{cols}H\u{3042}x",
                clear()
            )),
            w(format!(
                "{}\u{2764}\u{FE0F}ab\u{2764}\u{FE0F}\u{1F600}",
                clear()
            )),
            // 70 combining marks on one base: past the cluster cap
            w(format!("{}e{}z", clear(), "\u{301}".repeat(70))),
            w(format!("{}e{}z", clear(), "\u{301}".repeat(63))),
            // colours of every kind on fg, bg and underline colour
            w(format!(
                "{}\x1b[31mA\x1b[0m\x1b[91mB\x1b[38;5;200mC\x1b[38;2;1;2;3mD\x1b[0;41mE\x1b[0;101mF\
                 \x1b[48;5;17mG\x1b[48;2;10;20;30mH\x1b[0;4;58;5;99mI\x1b[58;2;4;5;6mJ\x1b[59mK\x1b[0m",
                clear()
            )),
            // every attribute, alone and together, and every underline style
            w(format!(
                "{}\x1b[1mA\x1b[0m\x1b[2mB\x1b[0m\x1b[3mC\x1b[0m\x1b[5mD\x1b[0m\x1b[7mE\x1b[0m\x1b[8mF\x1b[0m\
                 \x1b[9mG\x1b[0m\x1b[53mH\x1b[0m\x1b[1;2;3;5;7;8;9;53mZ\x1b[0m",
                clear()
            )),
            w(format!(
                "{}\x1b[4mA\x1b[4:2mB\x1b[4:3mC\x1b[4:4mD\x1b[4:5mE\x1b[21mF\x1b[24mG\x1b[0m",
                clear()
            )),
            // links: shared and distinct ids, one without an id, one with a 64 KiB uri
            w(format!(
                "{}\x1b]8;id=a;https://e.com/a\x1b\\link\x1b]8;;\x1b\\ plain \x1b]8;id=b;https://e.com/b\x1b\\B\
                 \x1b]8;;\x1b\\ \x1b]8;id=a;https://e.com/a\x1b\\again\x1b]8;;\x1b\\ \
                 \x1b]8;;https://e.com/noid\x1b\\N\x1b]8;;\x1b\\",
                clear()
            )),
            w(format!("{}{long_uri}", clear())),
            // wrap pending: the cursor sits at x == cols
            w(format!("{}{}", clear(), "x".repeat(cols as usize))),
            w("\x1b[?25l"),
            w("\x1b[3 q"),
            w("\x1b[6 q"),
            w("\x1b[5 q\x1b[?25h"),
            w("\x1b[0 q"),
        ];
        // scrollback and scrolled-back views
        steps.push(w(clear()));
        steps.push(w("line\r\n".repeat(60)));
        steps.extend([
            Step::Scroll(-5),
            Step::Scroll(-1000),
            Step::Bottom,
            Step::Top,
            Step::Scroll(3),
            Step::Bottom,
        ]);
        // alt screen
        steps.extend([w("\x1b[?1049hALT"), w("more"), w("\x1b[?1049l")]);
        // titles: set, empty, and 1 MiB
        steps.extend([
            w("\x1b]2;title \u{e9}\x07"),
            w("\x1b]2;\x07"),
            w(big_title),
            w("\x1b]2;\x07"),
        ]);
        // resize, wide content across the new size
        steps.extend([
            Step::Resize(5, 3),
            w(format!("{}\u{3042}\u{3044}\u{3046}", clear())),
            Step::Resize(100, 30),
            Step::Resize(cols, 1),
            Step::Resize(0, 0),
            Step::Resize(cols, 24),
        ]);
        steps
    }

    #[test]
    fn viewport_events_damage_and_modes_match_the_reference_across_the_matrix() {
        for cols in [3, 80, 81] {
            for rows in [1, 24] {
                for unicode in [LUNETERM_UNICODE_6, LUNETERM_UNICODE_11_VS16] {
                    let mut pair = Pair::new(cols, rows, 100, unicode, false);
                    pair.run(
                        &content_script(cols),
                        &format!("{cols}x{rows} unicode {unicode}"),
                    );
                }
            }
        }
        let mut pair = Pair::new(80, 24, 100, LUNETERM_UNICODE_11, false);
        pair.run(&content_script(80), "80x24 unicode 11");
    }

    #[test]
    fn the_long_uri_and_the_big_title_are_really_exercised() {
        let mut pair = Pair::new(80, 24, 100, LUNETERM_UNICODE_6, false);
        pair.apply(&w(format!(
            "\x1b]8;;https://example.com/{}\x1b\\L\x1b]8;;\x1b\\\x1b]2;{}\x07",
            "u".repeat(65_536),
            "t".repeat(1 << 20)
        )));
        let vp = viewport_of(pair.engine);
        assert_eq!(vp.link_count, 1);
        assert_eq!(
            vp.link_data_len,
            65_536 + "https://example.com/".len() as u64
        );
        assert_eq!(vp.title_len, 1 << 20);
        pair.check("long uri and big title");
    }

    #[test]
    fn the_viewport_at_4096_by_2048_matches_the_reference() {
        let mut pair = Pair::new(4096, 2048, 0, LUNETERM_UNICODE_11_VS16, false);
        let mut content = String::new();
        for line in 0..40 {
            content.push_str(&format!(
                "\x1b[3{}m\x1b[48;5;{line}m{}\u{3042}\u{2764}\u{FE0F}\x1b[0m\r\n",
                line % 8,
                "line".repeat(line + 1)
            ));
        }
        pair.apply(&w(format!("{}{content}", clear())));
        pair.check("4096x2048");
    }

    #[test]
    fn known_answers_for_the_cell_packing() {
        let mut pair = Pair::new(20, 2, 0, LUNETERM_UNICODE_11, false);
        pair.apply(&w(
            "\x1b[1;38;5;5;48;2;1;2;3;58;5;7;4:3mA\x1b[0m\x1b[3;9mB\x1b[0;2;5;7;8;53mC\x1b[0m\u{3042}",
        ));
        let vp = viewport_of(pair.engine);
        let cells = slice(vp.cells, vp.cell_count);
        let text = slice(vp.text, vp.text_len);
        assert_eq!(text, "ABC\u{3042}".as_bytes());
        // A: bold, palette 5 fg, rgb bg, palette 7 underline colour, curly.
        assert_eq!(
            cells[0],
            LunetermCell {
                text_offset: 0,
                text_len: 1,
                fg: 0x0100_0005,
                bg: 0x0201_0203,
                underline_color: 0x0100_0007,
                link_id: 0,
                flags: 0x01,
                width: 1,
                underline: 3,
            }
        );
        // B: italic (bit 2) + strikethrough (bit 6); C: dim, blink, inverse, invisible, overline.
        assert_eq!((cells[1].flags, cells[1].fg, cells[1].bg), (0x44, 0, 0));
        assert_eq!(cells[2].flags, 0b1011_1010);
        assert_eq!((cells[2].text_offset, cells[2].text_len), (2, 1));
        // The wide character: width 2, then the empty second half.
        assert_eq!(
            (cells[3].width, cells[3].text_len, cells[3].text_offset),
            (2, 3, 3)
        );
        assert_eq!((cells[4].width, cells[4].text_len), (0, 0));
        // An empty cell: offset 0, length 0, width 1.
        assert_eq!(
            (cells[10].text_offset, cells[10].text_len, cells[10].width),
            (0, 0, 1)
        );
        // Cursor: blinking bar after `\x1b[5 q`; wrap-pending column is cols.
        pair.apply(&w("\x1b[5 q"));
        let vp = viewport_of(pair.engine);
        assert_eq!(
            (vp.cursor_style, vp.cursor_blink, vp.cursor_visible),
            (2, 1, 1)
        );
        pair.apply(&w("\x1b[2 q\x1b[?25l"));
        let vp = viewport_of(pair.engine);
        assert_eq!(
            (vp.cursor_style, vp.cursor_blink, vp.cursor_visible),
            (0, 0, 0)
        );
        pair.apply(&w(format!("{}{}", clear(), "x".repeat(20))));
        let vp = viewport_of(pair.engine);
        assert_eq!((vp.cursor_x, vp.cursor_y, vp.cols), (20, 0, 20));
        assert_eq!((vp.is_at_bottom, vp.alt_screen), (1, 0));
    }

    #[test]
    fn known_answers_for_links_and_titles() {
        let mut pair = Pair::new(20, 2, 0, LUNETERM_UNICODE_6, false);
        pair.apply(&w("\x1b]8;id=a;https://e.com/a\x1b\\A\x1b]8;id=b;https://e.com/b\x1b\\B\x1b]8;;\x1b\\\x1b]2;ti\x00tle\x07"));
        let vp = viewport_of(pair.engine);
        let links = slice(vp.links, vp.link_count);
        let pool = slice(vp.link_data, vp.link_data_len);
        assert_eq!(links.len(), 2);
        assert_eq!(pool, b"https://e.com/ahttps://e.com/b");
        assert_eq!((links[0].offset, links[0].len), (0, 15));
        assert_eq!((links[1].offset, links[1].len), (15, 15));
        let cells = slice(vp.cells, vp.cell_count);
        assert_eq!(
            (cells[0].link_id, cells[1].link_id, cells[2].link_id),
            (links[0].id, links[1].id, 0)
        );
        assert_ne!(links[0].id, 0);
        // The title is held, not copied: the pointer is the engine's own string.
        let own = with_inner(pair.engine, |inner| {
            inner.term.core().input_handler().window_title_shared()
        });
        assert_eq!(slice(vp.title, vp.title_len), own.as_bytes());
        assert_eq!(vp.title, own.as_ptr());
        // An empty title is NULL/0.
        pair.apply(&w("\x1b]2;\x07"));
        let vp = viewport_of(pair.engine);
        assert_eq!((vp.title, vp.title_len), (std::ptr::null(), 0));
    }

    fn event_script() -> Vec<Step> {
        vec![
            w("\x1b[5n"),
            w("\x07"),
            w("\x1b]2;t\x07"),
            // several requests of each kind in one event, and one per OSC
            w("\x1b]4;1;?;2;?;3;rgb:01/02/03;4;rgb:04/05/06\x07"),
            w("\x1b]104;1;2;3\x07"),
            w("\x1b]104\x07"),
            w("\x1b]10;?\x07\x1b]11;rgb:aa/bb/cc\x07\x1b]12;?\x07"),
            w("\x1b]110\x07\x1b]111\x07\x1b]112\x07"),
            w("\x1b[?1004h"),
            w("\x1b[14t"),
            w("\x1b[16t"),
            w("abc\tdef\t\t"),
            w(format!("\x1b]9999;{}\x07", "p".repeat(100_000))),
            w("\x1b]777;x\x07"),
            w("\x1b]52;c;aGk=\x07"),
            w("\x1b]7;file://host/tmp/x\x07"),
            w("\x1b]9;9;/tmp/y\x07"),
            w("\x1b]9;4;1;50\x07"),
            w("\x1b]9;4;2;100\x07"),
            w("\x1b]9;4;3\x07"),
            w("\x1b]9;4;4;10\x07"),
            w("\x1b]9;4;0\x07"),
            w("\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07"),
            w("\x1b]133;D;0\x07\x1b]133;D;137\x07\x1b]133;D\x07"),
            w("\x1b]133;C\x07\x1b[?2026h"),
            // one write, many events, in order
            w("\x07\x1b[5n\x1b]2;x\x07\x1b[c\x1b]133;D;1\x07\x1b]9999;z\x07\x07"),
        ]
    }

    #[test]
    fn every_host_event_variant_matches_the_reference_in_order() {
        let mut pair = Pair::new(80, 24, 100, LUNETERM_UNICODE_6, true);
        pair.option(|| OptionValue::ScreenReaderMode(true));
        pair.option(|| {
            OptionValue::WindowOptions(WindowOptions {
                get_win_size_pixels: true,
                get_cell_size_pixels: true,
                ..WindowOptions::default()
            })
        });
        pair.run(&event_script(), "events");
        assert_eq!(
            pair.kinds,
            (1..=14).collect::<BTreeSet<u32>>(),
            "every HostEvent variant was produced and compared"
        );
    }

    #[test]
    fn known_answers_for_the_event_encoding() {
        let mut pair = Pair::new(80, 24, 100, LUNETERM_UNICODE_6, true);
        pair.option(|| OptionValue::ScreenReaderMode(true));
        let run = |bytes: &str| {
            write_to(pair.engine, bytes.as_bytes());
            let ev = events_of(pair.engine);
            (
                slice(ev.events, ev.count).to_vec(),
                slice(ev.data, ev.data_len).to_vec(),
                slice(ev.colors, ev.color_count).to_vec(),
            )
        };
        let (events, data, _) = run("\x1b[5n");
        assert_eq!(
            events,
            [LunetermEvent {
                kind: 1,
                value: 0,
                value2: 0,
                flags: 0,
                offset: 0,
                len: 4
            }]
        );
        assert_eq!(data, b"\x1b[0n");
        let (events, data, _) = run("\x07\x1b[5n\x07");
        assert_eq!(events.iter().map(|e| e.kind).collect::<Vec<_>>(), [2, 1, 2]);
        assert_eq!((events[1].offset, events[1].len, data.len()), (0, 4, 4));
        let (events, _, colors) = run("\x1b]4;1;?;2;rgb:01/02/03\x07\x1b]104\x07\x1b]11;?\x07");
        assert_eq!(
            events
                .iter()
                .map(|e| (e.kind, e.offset, e.len))
                .collect::<Vec<_>>(),
            [(4, 0, 2), (4, 2, 1), (4, 3, 1)]
        );
        let packed = |r: &LunetermColorRequest| (r.kind, r.index, r.r, r.g, r.b);
        assert_eq!(
            colors.iter().map(packed).collect::<Vec<_>>(),
            [
                (0, 1, 0, 0, 0),
                (1, 2, 1, 2, 3),
                (3, 0, 0, 0, 0),
                (0, 257, 0, 0, 0)
            ]
        );
        let (events, _, _) = run("\x1b]9999;12345\x07");
        assert_eq!(
            (
                events[0].kind,
                events[0].value,
                events[0].value2,
                events[0].offset,
                events[0].len
            ),
            (9, 9999, 5, 0, 0)
        );
        let (events, _, _) = run("\x1b]9;4;2;77\x07");
        assert_eq!(
            (events[0].kind, events[0].value, events[0].value2),
            (12, 2, 77)
        );
        let (events, _, _) = run("\x1b]133;D;7\x07\x1b]133;D\x07\x1b]133;C\x07");
        assert_eq!(
            events
                .iter()
                .map(|e| (e.kind, e.value, e.value2, e.flags))
                .collect::<Vec<_>>(),
            [(13, 3, 7, 1), (13, 3, 0, 0), (13, 2, 0, 0)]
        );
        let (events, data, _) = run("\x1b]52;c;aGk=\x07");
        assert_eq!((events[0].kind, data.as_slice()), (10, b"hi".as_slice()));
        let (events, _, _) = run("a\tb");
        assert_eq!((events[0].kind, events[1].kind, events[2].kind), (7, 8, 7));
        assert!(events[1].value > 0);
        // A tab count past INT32_MAX saturates (checked on the converter: no input makes one).
        let mut slot = EventsSlot::default();
        let out = flatten_events(&[HostEvent::A11yTab(usize::MAX)], &mut slot);
        assert_eq!(slice(out.events, out.count)[0].value, i32::MAX);
        // An unknown-OSC payload length past INT32_MAX saturates in `value2`; offset and len stay 0.
        let out = flatten_events(
            &[HostEvent::UnknownOsc {
                ident: 7,
                len: usize::MAX,
            }],
            &mut slot,
        );
        let event = slice(out.events, out.count)[0];
        assert_eq!(
            (event.value, event.value2, event.offset, event.len),
            (7, i32::MAX, 0, 0)
        );
    }

    #[test]
    fn an_empty_drain_is_zero_with_null_pointers() {
        let pair = Pair::new(80, 24, 0, LUNETERM_UNICODE_6, false);
        let ev = events_of(pair.engine);
        assert!(ev.events.is_null() && ev.data.is_null() && ev.colors.is_null());
        assert_eq!((ev.count, ev.data_len, ev.color_count), (0, 0, 0));
        // And again after a write that queues nothing.
        write_to(pair.engine, b"plain text");
        let ev = events_of(pair.engine);
        assert_eq!((ev.count, ev.events.is_null()), (0, true));
    }

    #[test]
    fn known_answers_for_modes_and_damage() {
        let mut pair = Pair::new(80, 24, 0, LUNETERM_UNICODE_6, false);
        let modes = modes_of(pair.engine);
        assert_eq!(
            (
                modes.wraparound,
                modes.application_cursor_keys,
                modes.mouse_tracking,
                modes.mouse_encoding
            ),
            (1, 0, 0, 0)
        );
        let after = |pair: &mut Pair, bytes: &str| {
            write_to(pair.engine, bytes.as_bytes());
            modes_of(pair.engine)
        };
        assert_eq!(after(&mut pair, "\x1b[?1h").application_cursor_keys, 1);
        assert_eq!(after(&mut pair, "\x1b=").application_keypad, 1);
        assert_eq!(after(&mut pair, "\x1b[?2004h").bracketed_paste, 1);
        assert_eq!(after(&mut pair, "\x1b[4h").insert, 1);
        assert_eq!(after(&mut pair, "\x1b[?6h").origin, 1);
        assert_eq!(after(&mut pair, "\x1b[?45h").reverse_wraparound, 1);
        assert_eq!(after(&mut pair, "\x1b[?1004h").send_focus, 1);
        assert_eq!(after(&mut pair, "\x1b[?2026h").synchronized_output, 1);
        assert_eq!(after(&mut pair, "\x1b[?7l").wraparound, 0);
        assert_eq!(after(&mut pair, "\x1b[?25l").cursor_hidden, 1);
        for (set, tracking) in [
            ("\x1b[?9h", 1),
            ("\x1b[?1000h", 2),
            ("\x1b[?1002h", 3),
            ("\x1b[?1003h", 4),
        ] {
            assert_eq!(after(&mut pair, set).mouse_tracking, tracking, "{set:?}");
        }
        assert_eq!(after(&mut pair, "\x1b[?1006h").mouse_encoding, 1);
        assert_eq!(after(&mut pair, "\x1b[?1016h").mouse_encoding, 2);
        assert_eq!(after(&mut pair, "\x1b[>1u").kitty_keyboard_flags, 1);
        assert_eq!(after(&mut pair, "\x1b[>4;2m").modify_other_keys, 2);
        // Damage: a write dirties rows; the next take is empty; a resize is full.
        let _ = damage_of(pair.engine);
        write_to(pair.engine, b"a\r\n\r\nb");
        let damage = damage_of(pair.engine);
        assert_eq!(
            (damage.has_rows, damage.first_row, damage.last_row),
            (1, 0, 2)
        );
        assert_eq!(damage.cursor_moved, 1);
        assert_eq!(damage_of(pair.engine), LunetermDamage::default());
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { luneterm_engine_resize(pair.engine, 40, 10) },
            LUNETERM_OK
        );
        assert_eq!(damage_of(pair.engine).full, 1);
        // Run the full differential over the modes and the damage too.
        let mut pair = Pair::new(80, 24, 0, LUNETERM_UNICODE_6, false);
        pair.run(
            &[
                w("\x1b[?1h\x1b=\x1b[?2004h\x1b[4h"),
                w("\x1b[?6h\x1b[?45h\x1b[?7l"),
                w("\x1b[?1000h\x1b[?1006h\x1b[>1u\x1b[>4;2m"),
                w("\x1b[?1003h\x1b[?1016h\x1b[?2026h\x1b[?1004h\x1b[?25l"),
                w("\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[<u\x1b[4l"),
                w("\x1bc"),
            ],
            "modes",
        );
    }

    #[test]
    fn a_viewport_pointer_survives_until_the_next_viewport_call() {
        // The strict form of the contract, with no refill in between: the old pointers read the
        // old screen after write, drain, resize, scroll and the other getters.
        let mut pair = Pair::new(20, 5, 100, LUNETERM_UNICODE_6, false);
        pair.apply(&w(format!("{}OLD\x1b]2;old title\x07", clear())));
        let vp = viewport_of(pair.engine);
        let snapshot = (
            slice(vp.cells, vp.cell_count).to_vec(),
            slice(vp.text, vp.text_len).to_vec(),
            slice(vp.title, vp.title_len).to_vec(),
        );
        write_to(
            pair.engine,
            b"\x1b[2J\x1b[H\x1b]2;new title\x07NEW TEXT\x1b[5n",
        );
        let _ = events_of(pair.engine);
        let _ = damage_of(pair.engine);
        let _ = modes_of(pair.engine);
        // SAFETY: a live handle.
        unsafe {
            assert_eq!(luneterm_engine_resize(pair.engine, 40, 3), LUNETERM_OK);
            assert_eq!(luneterm_engine_scroll_lines(pair.engine, -1), LUNETERM_OK);
        }
        assert_eq!(slice(vp.cells, vp.cell_count), snapshot.0.as_slice());
        assert_eq!(slice(vp.text, vp.text_len), snapshot.1.as_slice());
        assert_eq!(slice(vp.title, vp.title_len), snapshot.2.as_slice());
        assert_eq!(snapshot.1, b"OLD");
        assert_eq!(snapshot.2, b"old title");
        // Only now is the slot refilled.
        let fresh = viewport_of(pair.engine);
        assert_eq!(slice(fresh.title, fresh.title_len), b"new title");
    }

    #[test]
    fn an_events_pointer_survives_until_the_next_drain() {
        let mut pair = Pair::new(20, 5, 100, LUNETERM_UNICODE_6, false);
        write_to(pair.engine, b"\x1b[5n");
        let ev = events_of(pair.engine);
        let before = (
            slice(ev.events, ev.count).to_vec(),
            slice(ev.data, ev.data_len).to_vec(),
        );
        write_to(pair.engine, b"\x1b[c\x07\x1b[5n");
        let _ = viewport_of(pair.engine);
        let _ = damage_of(pair.engine);
        let _ = modes_of(pair.engine);
        pair.apply(&Step::Resize(30, 10));
        assert_eq!(slice(ev.events, ev.count), before.0.as_slice());
        assert_eq!(slice(ev.data, ev.data_len), before.1.as_slice());
        assert_eq!(before.1, b"\x1b[0n");
    }

    #[test]
    fn text_offsets_that_overflow_uint32_fail_closed_with_the_out_param_zeroed() {
        let mut term = Terminal::new([OptionValue::Cols(10), OptionValue::Rows(2)]);
        term.write("abcdef".as_bytes());
        let mut slot = ViewportSlot::default();
        // Offsets 0..=5: a limit of 5 fits, a limit of 4 does not (the sixth cell's offset is 5).
        assert!(flatten_viewport(&term, &mut slot, 5).is_ok());
        assert_eq!(
            flatten_viewport(&term, &mut slot, 4).map(|_| ()),
            Err(LUNETERM_ERR_TOO_LARGE)
        );
        // A cell text longer than the limit.
        let mut term = Terminal::new([OptionValue::Cols(10), OptionValue::Rows(1)]);
        term.write(format!("e{}", "\u{301}".repeat(5)).as_bytes());
        let snapshot = term.viewport();
        assert_eq!(snapshot.cells[0][0].text.len(), 11);
        assert!(flatten_viewport(&term, &mut slot, 11).is_ok());
        assert_eq!(
            flatten_viewport(&term, &mut slot, 10).map(|_| ()),
            Err(LUNETERM_ERR_TOO_LARGE)
        );
        // Through the export body: garbage in the out-param is zeroed, the handle stays usable.
        let pair = Pair::new(10, 2, 0, LUNETERM_UNICODE_6, false);
        write_to(pair.engine, b"abcdef");
        let mut out = LunetermViewport {
            cols: 9,
            cell_count: 9,
            ..LunetermViewport::EMPTY
        };
        // SAFETY: a live handle and a live out-param.
        let code = unsafe { viewport_with_limit(pair.engine, &mut out, 4) };
        assert_eq!(code, LUNETERM_ERR_TOO_LARGE);
        assert_eq!((out.cols, out.cell_count), (0, 0));
        assert!(out.cells.is_null() && out.text.is_null() && out.title.is_null());
        let ok = viewport_of(pair.engine);
        assert_eq!(ok.cell_count, 20);
    }

    #[test]
    fn slots_give_back_capacity_beyond_twice_the_need_plus_the_floor() {
        let mut pair = Pair::new(80, 24, 100, LUNETERM_UNICODE_6, false);
        // A huge drain: 20 000 responses.
        write_to(pair.engine, "\x1b[5n".repeat(20_000).as_bytes());
        let ev = events_of(pair.engine);
        assert_eq!(ev.count, 20_000);
        let caps = |engine| {
            with_inner(engine, |i| {
                (
                    i.events.events.capacity(),
                    i.events.data.capacity(),
                    i.events.colors.capacity(),
                )
            })
        };
        let (events_cap, data_cap, _) = caps(pair.engine);
        assert!(
            events_cap >= 20_000 && data_cap >= 80_000,
            "the big drain is held while in use"
        );
        assert!(events_cap <= 2 * 20_000 + ARRAY_FLOOR && data_cap <= 2 * 80_000 + POOL_FLOOR);
        // The next, empty drain gives it back.
        let empty = events_of(pair.engine);
        assert_eq!(empty.count, 0);
        let (events_cap, data_cap, colors_cap) = caps(pair.engine);
        assert!(events_cap <= ARRAY_FLOOR && data_cap <= POOL_FLOOR && colors_cap <= ARRAY_FLOOR);
        // A small drain after a medium one: bounded by 2x its need plus the floor.
        write_to(pair.engine, "\x1b[5n".repeat(3_000).as_bytes());
        assert_eq!(events_of(pair.engine).count, 3_000);
        write_to(pair.engine, b"\x1b[5n");
        assert_eq!(events_of(pair.engine).count, 1);
        let (events_cap, data_cap, _) = caps(pair.engine);
        assert!(events_cap <= 2 + ARRAY_FLOOR && data_cap <= 8 + POOL_FLOOR);

        // The viewport slot: a 4096 x 100 screen, then 80 x 24.
        pair.apply(&Step::Resize(4096, 100));
        let big = viewport_of(pair.engine);
        assert_eq!(big.cell_count, 409_600);
        let cells_cap = |engine| {
            with_inner(engine, |i| {
                (i.viewport.cells.capacity(), i.viewport.text.capacity())
            })
        };
        assert!(cells_cap(pair.engine).0 >= 409_600);
        pair.apply(&Step::Resize(80, 24));
        let small = viewport_of(pair.engine);
        assert_eq!(small.cell_count, 1920);
        let (cells, text) = cells_cap(pair.engine);
        assert!(
            cells <= 2 * 1920 + ARRAY_FLOOR && text <= POOL_FLOOR,
            "cells {cells}, text {text}"
        );
    }

    #[test]
    fn output_pointers_that_are_null_or_misaligned_are_refused_and_the_handle_stays_usable() {
        let pair = Pair::new(80, 24, 0, LUNETERM_UNICODE_6, false);
        let engine = pair.engine;
        let mut buf = [0u64; 32];
        let bad = buf.as_mut_ptr().cast::<u8>().wrapping_add(1);
        // SAFETY: a live handle; NULL and misaligned out-params are refused before any write.
        unsafe {
            assert_eq!(
                luneterm_engine_viewport(engine, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_viewport(engine, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_drain_events(engine, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_drain_events(engine, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_take_damage(engine, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_take_damage(engine, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            // `LunetermModes` has alignment 1: it cannot be misaligned.
            assert_eq!(
                luneterm_engine_modes(engine, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
        }
        assert_eq!(buf, [0; 32], "a refused out-param is never written");
        // A refused call neither consumed the queue nor poisoned the handle.
        write_to(engine, b"\x1b[5n");
        // SAFETY: a live handle and NULL out-param.
        assert_eq!(
            unsafe { luneterm_engine_drain_events(engine, null_mut()) },
            LUNETERM_ERR_NULL_POINTER
        );
        assert_eq!(events_of(engine).count, 1);
        let _ = viewport_of(engine);
    }

    #[test]
    fn a_failing_call_in_a_null_handle_zeroes_the_garbage_it_was_given() {
        let mut vp = LunetermViewport {
            cols: 9,
            title_len: 9,
            ..LunetermViewport::EMPTY
        };
        // SAFETY: a NULL handle and a live out-param.
        assert_eq!(
            unsafe { luneterm_engine_viewport(null_mut(), &mut vp) },
            LUNETERM_ERR_NULL_POINTER
        );
        assert_eq!((vp.cols, vp.title_len), (0, 0));
    }
}
