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
//! Replaced: `WriteBuffer.ts` (the engine's `write` is synchronous and flow control is the
//! host's job, plan M6 D3) and `Keyboard.ts` (it maps DOM `KeyboardEvent`s, which the input API
//! rejects, roadmap D8; `key_encoder_tests.rs` lists how each of its 59 tests maps).
//!
//! Not xterm.js: modifyOtherKeys, the kitty keyboard protocol and the key encoders come from
//! Lunegit's `driver.js`, `TerminalKeyEncoder` and `XtermVtInputEncoder`:
//!
//! - `key_event.rs`: `TerminalKey`, `TerminalModifiers`, `TerminalKeyEvent` (Lunegit's
//!   `TerminalInput.cs`).
//! - `key_encoder.rs`: `encode_key` and `KeyboardModes` (`TerminalKeyEncoder.cs`): kitty,
//!   modifyOtherKeys, legacy.
//! - `legacy_key_encoder.rs`: `legacy_key_input`, `legacy_char_input` (the key and char parts of
//!   `XtermVtInputEncoder.cs`).

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

pub mod key_encoder;
pub mod key_event;
pub mod legacy_key_encoder;
pub mod text_decoder;
pub mod unicode_v6;
pub mod x_parse_color;
