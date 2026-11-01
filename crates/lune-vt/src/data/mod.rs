//! Static data tables. Port of xterm.js `src/common/data/`.
//!
//! - `EscapeSequences.ts` → `escape_sequences.rs` (C0/C1 control codes).
//! - `Charsets.ts` → `charsets.rs` (VT100 national replacement character sets).

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

pub mod charsets;
pub mod escape_sequences;
