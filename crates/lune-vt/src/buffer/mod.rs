//! Screen buffers, lines, cells and reflow. Port of xterm.js `src/common/buffer/`
//! plus `src/common/CircularList.ts`.
//!
//! Ported (one Rust file per upstream file, `docs/porting-xterm.md` has the status table):
//!
//! - `Constants.ts` → `constants.rs`, `AttributeData.ts` → `attribute_data.rs`,
//!   `CellData.ts` → `cell_data.rs`, `BufferRange.ts` → `buffer_range.rs`
//! - `BufferLine.ts` → `buffer_line.rs`
//! - `BufferReflow.ts` → `buffer_reflow.rs` (the column reflow itself is in `Buffer`)
//! - `Buffer.ts` → `buffer.rs`, `BufferSet.ts` → `buffer_set.rs`
//! - `Marker.ts` and the marker list of `Buffer.ts` → `marker.rs` (`MarkerId`, `MarkerSet`)
//!
//! `Types.ts` is replaced (the structs are the types). `CircularList.ts` lives one level up, in
//! `crate::circular_list`, as upstream's `src/common/CircularList.ts`. `docs/architecture.md`
//! ("lune-vt buffer") explains the observer, the events-as-return-values rule and the host duties.

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
#[allow(clippy::module_inception)] // `Buffer.ts` in `buffer/`: one TS file, one Rust file
pub mod buffer;
pub mod buffer_line;
pub mod buffer_range;
pub mod buffer_reflow;
pub mod buffer_set;
pub mod cell_data;
pub mod constants;
pub mod marker;
