//! Escape-sequence parser. Port of xterm.js `src/common/parser/`.
//!
//! Port order (tests first, one file at a time):
//! `Constants.ts` → `constants.rs`, `Params.ts` → `params.rs`,
//! `OscParser.ts` → `osc_parser.rs`, `DcsParser.ts` → `dcs_parser.rs`,
//! `EscapeSequenceParser.ts` → `escape_sequence_parser.rs`.

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

pub mod constants;
pub mod dcs_parser;
pub mod escape_sequence_parser;
pub mod osc_parser;
pub mod params;
pub mod types;
