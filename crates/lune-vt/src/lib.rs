//! VT engine.
//!
//! A test-driven Rust port of xterm.js `src/common` at tag 6.0.0
//! (commit [`XTERM_JS_COMMIT`]). Module layout mirrors the xterm.js source tree so upstream
//! diffs map file-to-file; see `docs/porting-xterm.md`.
//!
//! This crate has no third-party dependencies and no `unsafe`. Everything Lunegit layered on
//! top of xterm.js (`driver.js`: kitty keyboard, XTVERSION, VS16 width, OSC 8/52 hooks) is
//! built as an [`extension::Extension`], not patched into the ported core.

#![forbid(unsafe_code)]

pub mod addons;
pub mod buffer;
pub mod circular_list;
pub mod color;
pub mod data;
pub mod engine;
pub mod extension;
pub mod input;
pub mod parser;
pub mod services;

pub use engine::{HostEvent, VtEngine};
pub use extension::Extension;

/// xterm.js release this crate ports.
pub const XTERM_JS_TAG: &str = "6.0.0";

/// Exact upstream commit of [`XTERM_JS_TAG`]; every ported file cites it.
pub const XTERM_JS_COMMIT: &str = "f447274f430fd22513f6adbf9862d19524471c04";
