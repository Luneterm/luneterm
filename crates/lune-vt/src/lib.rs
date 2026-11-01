//! VT engine.
//!
//! A test-driven Rust port of xterm.js `src/common` and `src/headless` at tag 6.0.0
//! (commit [`XTERM_JS_COMMIT`]). Module layout mirrors the xterm.js source tree so upstream
//! diffs map file-to-file; see `docs/porting-xterm.md` for the status of every file.
//!
//! Ported (through milestone M6): the parser (`parser`, `input::text_decoder`,
//! `input::x_parse_color`, `data`), the buffer (`buffer`, `circular_list`, `color`,
//! `input::unicode_v6`, `addons::unicode_v11`), every core service (`services`: options, unicode,
//! charset, buffer, OSC link, core, core mouse, log), `windows_mode`, the `InputHandler`
//! (`input_handler`: every handler upstream registers), `CoreTerminal` (`core_terminal`) and the
//! headless `Terminal` (`headless::terminal`). Replaced rather than ported: `WriteBuffer`
//! (`Terminal::write` is synchronous), the DI container, the renderer-side decoration service
//! and the JS buffer API objects. Not yet written: the keyboard encoders (`Keyboard.ts`) and
//! Lunegit's additions (M7).
//!
//! The public face is [`Terminal`], which implements [`VtEngine`] (`engine`): bytes in with
//! `write`, then `drain_events`, `take_damage` and `viewport` out. The engine has no callbacks,
//! timers or threads; `docs/architecture.md` ("lune-vt engine") says what the host still owns.
//!
//! This crate has no third-party dependencies and no `unsafe`. Everything Lunegit layered on
//! top of xterm.js (`driver.js`: kitty keyboard, XTVERSION, VS16 width, OSC 8/52 hooks) is
//! built as an [`extension::Extension`], not patched into the ported core.

#![forbid(unsafe_code)]

pub mod addons;
pub mod buffer;
pub mod circular_list;
pub mod color;
pub mod core_terminal;
pub mod data;
pub mod engine;
pub mod extension;
pub mod headless;
pub mod input;
pub mod input_handler;
pub mod parser;
pub mod services;
pub mod windows_mode;

pub use engine::{
    CellColor, ColorIndex, ColorRequest, CursorStyle, Damage, HostEvent, Modes, MouseEncoding,
    MouseTracking, UnderlineStyle, Viewport, ViewportCell, ViewportCursor, VtEngine,
    WindowReportKind,
};
pub use extension::Extension;
pub use headless::terminal::Terminal;

/// xterm.js release this crate ports.
pub const XTERM_JS_TAG: &str = "6.0.0";

/// Exact upstream commit of [`XTERM_JS_TAG`]; every ported file cites it.
pub const XTERM_JS_COMMIT: &str = "f447274f430fd22513f6adbf9862d19524471c04";
