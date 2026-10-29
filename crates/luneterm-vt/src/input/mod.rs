//! Input-side helpers. Port of xterm.js `src/common/input/`, plus the keyboard protocols Lunegit
//! added outside xterm.js.
//!
//! - `Keyboard.ts` → `keyboard.rs` (legacy xterm key encoding).
//! - `UnicodeV6.ts` → `unicode_v6.rs` (default width provider).
//! - `XParseColor.ts` → `x_parse_color.rs` (OSC 4/10/11 color specs).
//! - `WriteBuffer.ts` → `write_buffer.rs` (chunked writes / flow control).
//! - `TextDecoder.ts` → `text_decoder.rs` (streaming UTF-8; keep the split-multibyte
//!   regression from Lunegit's `PtyTerminalSessionDecodeRegressionTests`).
//!
//! Not xterm.js: modifyOtherKeys, the kitty keyboard protocol and mouse encoding come from
//! Lunegit's `driver.js` and `XtermVtInputEncoder`, and are built as extensions or here.
