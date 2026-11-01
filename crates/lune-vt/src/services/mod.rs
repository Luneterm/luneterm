//! Core services. Port of xterm.js `src/common/services/`.
//!
//! xterm.js wires these through a DI container (`InstantiationService`); the port uses plain
//! owned structs on the terminal instead. Port order: `OptionsService`, `UnicodeService`
//! (+ `@xterm/addon-unicode11` 0.9.0 tables), `CharsetService`, `BufferService`, `CoreService`,
//! `CoreMouseService`, `OscLinkService`.

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

pub mod buffer_service;
pub mod charset_service;
pub mod options_service;
pub mod unicode_service;
