//! Screen buffers, lines, cells and reflow. Port of xterm.js `src/common/buffer/`
//! plus `src/common/CircularList.ts`.
//!
//! Port order: `Constants.ts`, `AttributeData.ts`, `CellData.ts`, `BufferRange.ts`,
//! `BufferLine.ts`, `CircularList.ts`, `BufferReflow.ts`, `Buffer.ts`, `BufferSet.ts`,
//! `Marker.ts`.

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

pub mod attribute_data;
pub mod buffer_range;
pub mod cell_data;
pub mod constants;
