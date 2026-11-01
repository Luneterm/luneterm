//! Ported xterm.js addons that the engine core needs. Port of the `src` of the matching
//! packages under xterm.js `addons/`.
//!
//! - `addon-unicode11/src/UnicodeV11.ts` → `unicode_v11.rs` (0.9.0; the addon shell
//!   `Unicode11Addon.ts` is replaced by `UnicodeService::register`).

// No-panic gate (plan M5, extending M4's with `string_slice`): ported non-test code must not
// index, slice a string, unwrap or panic. Tests are exempt; every other lint stays at the
// workspace level.
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

pub mod unicode_v11;
