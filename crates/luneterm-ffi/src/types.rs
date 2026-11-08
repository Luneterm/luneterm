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
