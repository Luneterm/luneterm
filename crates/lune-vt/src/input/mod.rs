//! Input-side helpers. Port of xterm.js `src/common/input/`, plus the keyboard protocols Lunegit
//! added outside xterm.js.
//!
//! Ported:
//!
//! - `UnicodeV6.ts` → `unicode_v6.rs` (default width provider).
//! - `XParseColor.ts` → `x_parse_color.rs` (OSC 4/10/11 color specs).
//! - `TextDecoder.ts` → `text_decoder.rs` (streaming UTF-8; keeps the split-multibyte
//!   regression from Lunegit's `PtyTerminalSessionDecodeRegressionTests`).
//!
//! Pending: `Keyboard.ts` (legacy xterm key encoding, M7). `WriteBuffer.ts` is replaced: the
//! engine's `write` is synchronous and flow control is the host's job (plan M6 D3).
//!
//! Not xterm.js: modifyOtherKeys, the kitty keyboard protocol and mouse encoding come from
//! Lunegit's `driver.js` and `XtermVtInputEncoder`, and are built as extensions or here.

// No-panic gate (plan M4): ported non-test code must not index, unwrap or panic. Tests are
// exempt; every other lint stays at the workspace level.
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

pub mod text_decoder;
pub mod unicode_v6;
pub mod x_parse_color;
