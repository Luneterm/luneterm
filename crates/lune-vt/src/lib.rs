//! VT engine.
//!
//! A test-driven Rust port of xterm.js `src/common` and `src/headless` at tag 6.0.0
//! (commit [`XTERM_JS_COMMIT`]). Module layout mirrors the xterm.js source tree so upstream
//! diffs map file-to-file; see `docs/porting-xterm.md` for the status of every file.
//!
//! Ported (through milestone M7): the parser (`parser`, `input::text_decoder`,
//! `input::x_parse_color`, `data`), the buffer (`buffer`, `circular_list`, `color`,
//! `input::unicode_v6`, `addons::unicode_v11`), every core service (`services`: options, unicode,
//! charset, buffer, OSC link, core, core mouse, log), `windows_mode`, the `InputHandler`
//! (`input_handler`: every handler upstream registers), `CoreTerminal` (`core_terminal`) and the
//! headless `Terminal` (`headless::terminal`). Replaced rather than ported: `WriteBuffer`
//! (`Terminal::write` is synchronous), the DI container, the renderer-side decoration service
//! and the JS buffer API objects. `Keyboard.ts` is superseded by the key encoders of
//! `input::key_encoder` (Lunegit's `TerminalKeyEncoder`; plan M7 D4). Lunegit's additions (M7)
//! are in `ext` (the parser extensions, installed by [`Terminal::install_standard_extensions`])
//! and `input` (the key, mouse, focus and paste encoders, which return the bytes the host writes
//! to the PTY); `docs/porting-xterm.md` ("Ported from Lunegit") lists each file.
//!
//! The public face is [`Terminal`], which implements [`VtEngine`] (`engine`): bytes in with
//! `write`, then `drain_events`, `take_damage` and `viewport` out. The engine has no callbacks,
//! timers or threads; `docs/architecture.md` ("lune-vt engine") says what the host still owns.
//!
//! This crate has no third-party dependencies and no `unsafe`. Everything Lunegit layered on
//! top of xterm.js (`driver.js`: kitty keyboard, modifyOtherKeys, XTVERSION, VS16 width, the
//! OSC 52 write, the unknown-OSC report; and its OSC 7, 9;9, 9;4 and 133 scanners) is built as
//! parser extensions (`ext`, registered through [`extension::Extension`]'s mechanism), not
//! patched into the ported core.

#![forbid(unsafe_code)]

pub mod addons;
pub mod buffer;
pub mod circular_list;
pub mod color;
pub mod core_terminal;
pub mod data;
pub mod engine;
pub mod ext;
pub mod extension;
pub mod headless;
pub mod input;
pub mod input_handler;
pub mod parser;
pub mod services;
pub mod windows_mode;

pub use engine::{
    CellColor, ColorIndex, ColorRequest, CommandMark, CommandMarkKind, CursorStyle, Damage,
    HostEvent, Modes, MouseEncoding, MouseTracking, Progress, ProgressState, UnderlineStyle,
    Viewport, ViewportCell, ViewportCursor, VtEngine, WindowReportKind,
};
pub use ext::{ExtensionError, StandardExtensions};
pub use extension::Extension;
pub use headless::terminal::Terminal;
pub use input::key_encoder::{KeyboardModes, encode_key};
pub use input::key_event::{TerminalKey, TerminalKeyEvent, TerminalModifiers};
pub use input::mouse_event::{TerminalMouseButton, TerminalMouseEvent, TerminalMouseEventType};
pub use input::paste::paste_bytes;

/// xterm.js release this crate ports.
pub const XTERM_JS_TAG: &str = "6.0.0";

/// Exact upstream commit of [`XTERM_JS_TAG`]; every ported file cites it.
pub const XTERM_JS_COMMIT: &str = "f447274f430fd22513f6adbf9862d19524471c04";
