//! `#[repr(C)]` structs and constants of ABI 1 (M8 plan, Approach; D5, D6, D9).
//!
//! Layout rules (D6): fixed-width integers only; enums cross as `uint32_t` and are range-checked,
//! never transmuted; booleans cross as `uint8_t` (nonzero = true), never Rust `bool`; struct
//! lengths are `uint64_t`. Sizes and offsets are asserted at compile time below, and the same
//! numbers by `_Static_assert` in the C header.

use std::mem::{offset_of, size_of};

/// Largest accepted column count (D5). Over it: `LUNETERM_ERR_INVALID_ARGUMENT`.
pub const LUNETERM_MAX_COLS: u32 = 4096;
/// Largest accepted row count (D5).
pub const LUNETERM_MAX_ROWS: u32 = 2048;
/// Largest accepted scrollback, in lines (D5; Lunegit's own clamp).
pub const LUNETERM_MAX_SCROLLBACK: u32 = 100_000;

/// `LunetermConfig.unicode`: Unicode 6 widths (lune-vt's default, xterm.js's `"6"`).
pub const LUNETERM_UNICODE_6: u32 = 0;
/// `LunetermConfig.unicode`: Unicode 11 widths (`"11"`).
pub const LUNETERM_UNICODE_11: u32 = 1;
/// `LunetermConfig.unicode`: Unicode 11 with VS16 widening (`"11-vs16"`).
pub const LUNETERM_UNICODE_11_VS16: u32 = 2;

/// Engine construction parameters. All zero is the safe default: 2 x 1 (clamped), no
/// scrollback, Unicode 6, XTVERSION unanswered, clipboard writes off.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LunetermConfig {
    /// Columns; below 2 is clamped to 2, over [`LUNETERM_MAX_COLS`] is refused.
    pub cols: u32,
    /// Rows; below 1 is clamped to 1, over [`LUNETERM_MAX_ROWS`] is refused.
    pub rows: u32,
    /// Scrollback lines; over [`LUNETERM_MAX_SCROLLBACK`] is refused.
    pub scrollback: u32,
    /// One of `LUNETERM_UNICODE_*`; anything else is refused.
    pub unicode: u32,
    /// The XTVERSION name (UTF-8, not NUL-terminated), given to lune-vt as is: 1-64 bytes of
    /// `0x20..=0x7E`. A length of 0 (whatever the pointer): the query is not answered.
    pub xtversion: *const u8,
    /// Length of `xtversion` in bytes.
    pub xtversion_len: u64,
    /// Initial OSC 52 clipboard-write gate; nonzero = on.
    pub clipboard_write: u8,
    /// Ignored.
    pub reserved: [u8; 7],
}

const _: () = {
    assert!(size_of::<LunetermConfig>() == 40);
    assert!(offset_of!(LunetermConfig, cols) == 0);
    assert!(offset_of!(LunetermConfig, rows) == 4);
    assert!(offset_of!(LunetermConfig, scrollback) == 8);
    assert!(offset_of!(LunetermConfig, unicode) == 12);
    assert!(offset_of!(LunetermConfig, xtversion) == 16);
    assert!(offset_of!(LunetermConfig, xtversion_len) == 24);
    assert!(offset_of!(LunetermConfig, clipboard_write) == 32);
    assert!(offset_of!(LunetermConfig, reserved) == 33);
};

// ---- Viewport ------------------------------------------------------------------------------

/// `fg`/`bg`/`underline_color` kind (bits 24-31 of the packed colour): the theme's default; the
/// low bits are 0.
pub const LUNETERM_COLOR_DEFAULT: u32 = 0;
/// Colour kind: a palette entry, index in bits 0-7.
pub const LUNETERM_COLOR_PALETTE: u32 = 1;
/// Colour kind: true colour, `0xRRGGBB` in bits 0-23.
pub const LUNETERM_COLOR_RGB: u32 = 2;

/// `LunetermCell.flags` bit: bold.
pub const LUNETERM_CELL_BOLD: u16 = 1 << 0;
/// `LunetermCell.flags` bit: dim.
pub const LUNETERM_CELL_DIM: u16 = 1 << 1;
/// `LunetermCell.flags` bit: italic.
pub const LUNETERM_CELL_ITALIC: u16 = 1 << 2;
/// `LunetermCell.flags` bit: blink.
pub const LUNETERM_CELL_BLINK: u16 = 1 << 3;
/// `LunetermCell.flags` bit: inverse.
pub const LUNETERM_CELL_INVERSE: u16 = 1 << 4;
/// `LunetermCell.flags` bit: invisible.
pub const LUNETERM_CELL_INVISIBLE: u16 = 1 << 5;
/// `LunetermCell.flags` bit: strikethrough.
pub const LUNETERM_CELL_STRIKETHROUGH: u16 = 1 << 6;
/// `LunetermCell.flags` bit: overline.
pub const LUNETERM_CELL_OVERLINE: u16 = 1 << 7;

/// `LunetermCell.underline`: none.
pub const LUNETERM_UNDERLINE_NONE: u8 = 0;
/// `LunetermCell.underline`: single.
pub const LUNETERM_UNDERLINE_SINGLE: u8 = 1;
/// `LunetermCell.underline`: double.
pub const LUNETERM_UNDERLINE_DOUBLE: u8 = 2;
/// `LunetermCell.underline`: curly.
pub const LUNETERM_UNDERLINE_CURLY: u8 = 3;
/// `LunetermCell.underline`: dotted.
pub const LUNETERM_UNDERLINE_DOTTED: u8 = 4;
/// `LunetermCell.underline`: dashed.
pub const LUNETERM_UNDERLINE_DASHED: u8 = 5;

/// `LunetermViewport.cursor_style`: block.
pub const LUNETERM_CURSOR_BLOCK: u8 = 0;
/// `LunetermViewport.cursor_style`: underline.
pub const LUNETERM_CURSOR_UNDERLINE: u8 = 1;
/// `LunetermViewport.cursor_style`: bar.
pub const LUNETERM_CURSOR_BAR: u8 = 2;

/// One screen cell of a [`LunetermViewport`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermCell {
    /// Offset of the cell's text in `LunetermViewport.text`.
    pub text_offset: u32,
    /// Length of the cell's text in bytes; 0 for an empty cell and for the second half of a
    /// wide character.
    pub text_len: u32,
    /// Packed colour: bits 24-31 kind (`LUNETERM_COLOR_*`), the rest as the kind says.
    pub fg: u32,
    /// Packed colour, as `fg`.
    pub bg: u32,
    /// Packed colour, as `fg`; `DEFAULT` = follow the foreground.
    pub underline_color: u32,
    /// OSC 8 link id (0 = none); the uri is in `LunetermViewport.links`.
    pub link_id: u32,
    /// `LUNETERM_CELL_*` bits.
    pub flags: u16,
    /// 1 normal, 2 first half of a wide character, 0 its second half.
    pub width: u8,
    /// `LUNETERM_UNDERLINE_*`.
    pub underline: u8,
}

/// An OSC 8 link used by a cell of a [`LunetermViewport`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermLink {
    /// The id cells carry in `link_id`.
    pub id: u32,
    /// Zero.
    pub reserved: u32,
    /// Offset of the uri in `LunetermViewport.link_data`.
    pub offset: u64,
    /// Length of the uri in bytes.
    pub len: u64,
}

/// A snapshot of the screen. Every pointer points into the handle's viewport slot and stays valid
/// until the next `luneterm_engine_viewport` on that handle (or `luneterm_engine_free`).
/// Pointers of an empty array or string are NULL.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LunetermViewport {
    /// Columns.
    pub cols: u32,
    /// Rows.
    pub rows: u32,
    /// `cell_count` cells, row-major.
    pub cells: *const LunetermCell,
    /// `cols * rows`.
    pub cell_count: u64,
    /// Cell text pool (UTF-8).
    pub text: *const u8,
    /// Length of `text` in bytes.
    pub text_len: u64,
    /// The links the cells use, ascending by id.
    pub links: *const LunetermLink,
    /// Number of links.
    pub link_count: u64,
    /// Link uri pool (UTF-8).
    pub link_data: *const u8,
    /// Length of `link_data` in bytes.
    pub link_data_len: u64,
    /// The window title (UTF-8, may contain NUL).
    pub title: *const u8,
    /// Length of the title in bytes.
    pub title_len: u64,
    /// First buffer line shown.
    pub viewport_top: u64,
    /// First line of the bottom page.
    pub base_y: u64,
    /// Lines in the active buffer, scrollback included.
    pub total_lines: u64,
    /// Cursor column, `0..=cols + 1`: `cols` while a wrap is pending, `cols + 1` after a
    /// zero-width character (such as U+0301) printed there, as @xterm/headless 6.0.0 does. Not
    /// clamped: a host indexing `cells` with it must clamp first.
    pub cursor_x: u32,
    /// Cursor viewport row.
    pub cursor_y: u32,
    /// Nonzero when the cursor is shown.
    pub cursor_visible: u8,
    /// `LUNETERM_CURSOR_*`.
    pub cursor_style: u8,
    /// Nonzero when the cursor blinks.
    pub cursor_blink: u8,
    /// Nonzero when the viewport shows the bottom page.
    pub is_at_bottom: u8,
    /// Nonzero while the alternate screen is active.
    pub alt_screen: u8,
    /// Zero.
    pub reserved: [u8; 3],
}

impl LunetermViewport {
    /// All zero, with NULL pointers: what an out-param holds after an error.
    pub(crate) const EMPTY: LunetermViewport = LunetermViewport {
        cols: 0,
        rows: 0,
        cells: std::ptr::null(),
        cell_count: 0,
        text: std::ptr::null(),
        text_len: 0,
        links: std::ptr::null(),
        link_count: 0,
        link_data: std::ptr::null(),
        link_data_len: 0,
        title: std::ptr::null(),
        title_len: 0,
        viewport_top: 0,
        base_y: 0,
        total_lines: 0,
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: 0,
        cursor_style: 0,
        cursor_blink: 0,
        is_at_bottom: 0,
        alt_screen: 0,
        reserved: [0; 3],
    };
}

// ---- Events --------------------------------------------------------------------------------

/// Event kind: bytes to send back to the application (`data`).
pub const LUNETERM_EVENT_RESPONSE: u32 = 1;
/// Event kind: BEL.
pub const LUNETERM_EVENT_BELL: u32 = 2;
/// Event kind: the title changed; read it from the viewport.
pub const LUNETERM_EVENT_TITLE_CHANGED: u32 = 3;
/// Event kind: colour requests (`offset`/`len` index `colors`).
pub const LUNETERM_EVENT_COLOR: u32 = 4;
/// Event kind: the application turned focus reporting on.
pub const LUNETERM_EVENT_FOCUS_REPORT_REQUESTED: u32 = 5;
/// Event kind: a window report is wanted (`value`: 0 window size in pixels, 1 cell size).
pub const LUNETERM_EVENT_WINDOW_REPORT: u32 = 6;
/// Event kind: screen-reader text (`data`).
pub const LUNETERM_EVENT_A11Y_TEXT: u32 = 7;
/// Event kind: HT moved the cursor (`value` = cells, saturated to `INT32_MAX`).
pub const LUNETERM_EVENT_A11Y_TAB: u32 = 8;
/// Event kind: an OSC nobody handled (`value` = ident, `len` = payload length, no data).
pub const LUNETERM_EVENT_UNKNOWN_OSC: u32 = 9;
/// Event kind: OSC 52 clipboard write (`data`, UTF-8).
pub const LUNETERM_EVENT_CLIPBOARD_WRITE: u32 = 10;
/// Event kind: the working directory changed (`data`, UTF-8).
pub const LUNETERM_EVENT_CWD_CHANGED: u32 = 11;
/// Event kind: progress report (`value` state 0-4, `value2` percent).
pub const LUNETERM_EVENT_PROGRESS: u32 = 12;
/// Event kind: OSC 133 mark (`value` 0 A, 1 B, 2 C, 3 D; `value2` exit code when `flags` has
/// `LUNETERM_EVENT_HAS_EXIT_CODE`).
pub const LUNETERM_EVENT_COMMAND_MARK: u32 = 13;
/// Event kind: synchronized output was turned on while a command runs.
pub const LUNETERM_EVENT_SYNC_UPDATE_IN_COMMAND: u32 = 14;

/// `LunetermEvent.flags` bit for `COMMAND_MARK`: `value2` holds an exit code.
pub const LUNETERM_EVENT_HAS_EXIT_CODE: u32 = 1;

/// `LunetermColorRequest.kind`: report the current colour.
pub const LUNETERM_COLOR_REQUEST_REPORT: u32 = 0;
/// `LunetermColorRequest.kind`: set the colour to `r`, `g`, `b`.
pub const LUNETERM_COLOR_REQUEST_SET: u32 = 1;
/// `LunetermColorRequest.kind`: restore one colour.
pub const LUNETERM_COLOR_REQUEST_RESTORE: u32 = 2;
/// `LunetermColorRequest.kind`: restore the whole palette (`index` is 0).
pub const LUNETERM_COLOR_REQUEST_RESTORE_ALL: u32 = 3;

/// `LunetermColorRequest.index`: the default foreground.
pub const LUNETERM_COLOR_INDEX_FOREGROUND: u32 = 256;
/// `LunetermColorRequest.index`: the default background.
pub const LUNETERM_COLOR_INDEX_BACKGROUND: u32 = 257;
/// `LunetermColorRequest.index`: the cursor colour.
pub const LUNETERM_COLOR_INDEX_CURSOR: u32 = 258;

/// One event of a [`LunetermEvents`] drain.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermEvent {
    /// `LUNETERM_EVENT_*`.
    pub kind: u32,
    /// Kind specific.
    pub value: i32,
    /// Kind specific.
    pub value2: i32,
    /// Kind specific bits.
    pub flags: u32,
    /// Offset into `LunetermEvents.data` (or the first request in `colors` for `COLOR`).
    pub offset: u64,
    /// Length in bytes (or the number of requests for `COLOR`).
    pub len: u64,
}

/// One colour request of a `COLOR` event.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermColorRequest {
    /// `LUNETERM_COLOR_REQUEST_*`.
    pub kind: u32,
    /// 0-255 palette entry, or `LUNETERM_COLOR_INDEX_*`; 0 for `RESTORE_ALL`.
    pub index: u32,
    /// Red of a `SET` request.
    pub r: u8,
    /// Green of a `SET` request.
    pub g: u8,
    /// Blue of a `SET` request.
    pub b: u8,
    /// Zero.
    pub reserved: u8,
}

/// The events drained from the engine, in order. Every pointer points into the handle's events
/// slot and stays valid until the next `luneterm_engine_drain_events` on that handle (or
/// `luneterm_engine_free`). Pointers of an empty array or string are NULL.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LunetermEvents {
    /// `count` events.
    pub events: *const LunetermEvent,
    /// Number of events.
    pub count: u64,
    /// Pool for the events' byte payloads.
    pub data: *const u8,
    /// Length of `data` in bytes.
    pub data_len: u64,
    /// Pool for the `COLOR` events' requests.
    pub colors: *const LunetermColorRequest,
    /// Number of colour requests.
    pub color_count: u64,
}

impl LunetermEvents {
    /// All zero, with NULL pointers: what an out-param holds after an error.
    pub(crate) const EMPTY: LunetermEvents = LunetermEvents {
        events: std::ptr::null(),
        count: 0,
        data: std::ptr::null(),
        data_len: 0,
        colors: std::ptr::null(),
        color_count: 0,
    };
}

// ---- Damage and modes ----------------------------------------------------------------------

/// What changed on screen since the last `luneterm_engine_take_damage`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermDamage {
    /// Nonzero when `first_row..=last_row` is set.
    pub has_rows: u8,
    /// Nonzero when everything must be redrawn.
    pub full: u8,
    /// Nonzero when the cursor moved.
    pub cursor_moved: u8,
    /// Nonzero when the buffer scrolled.
    pub scrolled: u8,
    /// First viewport row to redraw (0 when `has_rows` is 0).
    pub first_row: u32,
    /// Last viewport row to redraw, inclusive (0 when `has_rows` is 0).
    pub last_row: u32,
}

/// `LunetermModes.mouse_tracking`: off.
pub const LUNETERM_MOUSE_TRACKING_NONE: u8 = 0;
/// `LunetermModes.mouse_tracking`: X10.
pub const LUNETERM_MOUSE_TRACKING_X10: u8 = 1;
/// `LunetermModes.mouse_tracking`: VT200.
pub const LUNETERM_MOUSE_TRACKING_VT200: u8 = 2;
/// `LunetermModes.mouse_tracking`: button-event (drag).
pub const LUNETERM_MOUSE_TRACKING_DRAG: u8 = 3;
/// `LunetermModes.mouse_tracking`: any-event.
pub const LUNETERM_MOUSE_TRACKING_ANY: u8 = 4;

/// `LunetermModes.mouse_encoding`: the default (X10 bytes).
pub const LUNETERM_MOUSE_ENCODING_DEFAULT: u8 = 0;
/// `LunetermModes.mouse_encoding`: SGR.
pub const LUNETERM_MOUSE_ENCODING_SGR: u8 = 1;
/// `LunetermModes.mouse_encoding`: SGR pixels.
pub const LUNETERM_MOUSE_ENCODING_SGR_PIXELS: u8 = 2;

/// The modes the host needs for input encoding. Booleans are 0 or 1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermModes {
    /// DECCKM.
    pub application_cursor_keys: u8,
    /// DECKPAM.
    pub application_keypad: u8,
    /// Bracketed paste.
    pub bracketed_paste: u8,
    /// Insert mode.
    pub insert: u8,
    /// DECOM.
    pub origin: u8,
    /// Reverse wraparound.
    pub reverse_wraparound: u8,
    /// Focus reporting.
    pub send_focus: u8,
    /// Synchronized output.
    pub synchronized_output: u8,
    /// DECAWM.
    pub wraparound: u8,
    /// DECTCEM off.
    pub cursor_hidden: u8,
    /// `LUNETERM_MOUSE_TRACKING_*`.
    pub mouse_tracking: u8,
    /// `LUNETERM_MOUSE_ENCODING_*`.
    pub mouse_encoding: u8,
    /// Kitty keyboard flags in force.
    pub kitty_keyboard_flags: u8,
    /// The modifyOtherKeys level, 0-2.
    pub modify_other_keys: u8,
}

// ---- Input ---------------------------------------------------------------------------------

/// Bytes the engine hands out (an encoder result). `ptr` points into the handle's input slot and
/// stays valid until the next encoder call on that handle (or `luneterm_engine_free`); it is NULL
/// when `len == 0`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LunetermBytes {
    /// The bytes to write to the PTY.
    pub ptr: *const u8,
    /// Number of bytes; 0 = nothing to send.
    pub len: u64,
}

impl LunetermBytes {
    /// NULL/0: what an out-param holds after an error and when there is nothing to send.
    pub(crate) const EMPTY: LunetermBytes = LunetermBytes {
        ptr: std::ptr::null(),
        len: 0,
    };
}

/// Modifier bit (`uint8_t modifiers`): Shift.
pub const LUNETERM_MOD_SHIFT: u8 = 1;
/// Modifier bit: Alt.
pub const LUNETERM_MOD_ALT: u8 = 2;
/// Modifier bit: Control.
pub const LUNETERM_MOD_CONTROL: u8 = 4;

/// One keystroke for `luneterm_engine_key_event`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermKeyEvent {
    /// Lunegit's `TerminalKey` ordinal, 0..=27; anything else is refused: Enter 0, Tab 1,
    /// Backspace 2, Escape 3, Space 4, Up 5, Down 6, Right 7, Left 8, Home 9, End 10, PageUp 11,
    /// PageDown 12, Insert 13, Delete 14, F1 15 ... F12 26, Character 27.
    pub key: u32,
    /// For `Character`: the unshifted base character (a Unicode scalar value, else nothing is
    /// sent), or the keypad character when `is_keypad`.
    pub codepoint: u32,
    /// What the key produces with Shift; 0 = unknown.
    pub shifted_codepoint: u32,
    /// The key's character on the US-QWERTY layout; 0 = unknown.
    pub base_layout_codepoint: u32,
    /// `LUNETERM_MOD_*` bits; unknown bits are kept and ignored.
    pub modifiers: u8,
    /// Nonzero when the key is on the numeric keypad.
    pub is_keypad: u8,
    /// Nonzero when Alt acts as Meta.
    pub alt_is_meta: u8,
    /// Ignored.
    pub reserved: u8,
}

/// One pointer event for `luneterm_engine_mouse_event`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LunetermMouseEvent {
    /// Lunegit's `TerminalMouseButton` ordinal, 0..=5, anything else is refused: None 0, Left 1,
    /// Middle 2, Right 3, WheelUp 4, WheelDown 5.
    pub button: u32,
    /// Lunegit's `TerminalMouseEventType` ordinal, 0..=5, anything else is refused: Down 0, Up 1,
    /// Move 2, Drag 3, WheelUp 4, WheelDown 5.
    pub kind: u32,
    /// Zero-based column.
    pub col: i32,
    /// Zero-based row.
    pub row: i32,
    /// Pixel x (SGR pixels only).
    pub x: i32,
    /// Pixel y (SGR pixels only).
    pub y: i32,
    /// `LUNETERM_MOD_*` bits; unknown bits are kept and ignored.
    pub modifiers: u8,
    /// Ignored.
    pub reserved: [u8; 3],
}

#[cfg(test)]
mod output_layout {
    use super::*;

    macro_rules! layout {
        ($ty:ty, $size:expr, $align:expr, { $($field:ident : $offset:expr),+ $(,)? }) => {
            assert_eq!(size_of::<$ty>(), $size, concat!("size of ", stringify!($ty)));
            assert_eq!(align_of::<$ty>(), $align, concat!("align of ", stringify!($ty)));
            $(assert_eq!(
                offset_of!($ty, $field),
                $offset,
                concat!("offset of ", stringify!($ty), ".", stringify!($field))
            );)+
        };
    }

    #[test]
    fn output_structs_have_the_abi_1_layout() {
        layout!(LunetermCell, 28, 4, {
            text_offset: 0, text_len: 4, fg: 8, bg: 12, underline_color: 16, link_id: 20,
            flags: 24, width: 26, underline: 27,
        });
        layout!(LunetermLink, 24, 8, { id: 0, reserved: 4, offset: 8, len: 16 });
        layout!(LunetermViewport, 128, 8, {
            cols: 0, rows: 4, cells: 8, cell_count: 16, text: 24, text_len: 32, links: 40,
            link_count: 48, link_data: 56, link_data_len: 64, title: 72, title_len: 80,
            viewport_top: 88, base_y: 96, total_lines: 104, cursor_x: 112, cursor_y: 116,
            cursor_visible: 120, cursor_style: 121, cursor_blink: 122, is_at_bottom: 123,
            alt_screen: 124, reserved: 125,
        });
        layout!(LunetermEvent, 32, 8, {
            kind: 0, value: 4, value2: 8, flags: 12, offset: 16, len: 24,
        });
        layout!(LunetermColorRequest, 12, 4, {
            kind: 0, index: 4, r: 8, g: 9, b: 10, reserved: 11,
        });
        layout!(LunetermEvents, 48, 8, {
            events: 0, count: 8, data: 16, data_len: 24, colors: 32, color_count: 40,
        });
        layout!(LunetermDamage, 12, 4, {
            has_rows: 0, full: 1, cursor_moved: 2, scrolled: 3, first_row: 4, last_row: 8,
        });
        layout!(LunetermBytes, 16, 8, { ptr: 0, len: 8 });
        layout!(LunetermKeyEvent, 20, 4, {
            key: 0, codepoint: 4, shifted_codepoint: 8, base_layout_codepoint: 12, modifiers: 16,
            is_keypad: 17, alt_is_meta: 18, reserved: 19,
        });
        layout!(LunetermMouseEvent, 28, 4, {
            button: 0, kind: 4, col: 8, row: 12, x: 16, y: 20, modifiers: 24, reserved: 25,
        });
        layout!(LunetermModes, 14, 1, {
            application_cursor_keys: 0, application_keypad: 1, bracketed_paste: 2, insert: 3,
            origin: 4, reverse_wraparound: 5, send_focus: 6, synchronized_output: 7,
            wraparound: 8, cursor_hidden: 9, mouse_tracking: 10, mouse_encoding: 11,
            kitty_keyboard_flags: 12, modify_other_keys: 13,
        });
    }
}
