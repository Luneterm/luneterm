//! Port of xterm.js src/common/WindowsMode.ts @ f447274 (upstream has no test file;
//! `windows_mode_tests.rs` is written from the TypeScript behaviour).
//!
//! Rust notes (see plan M6 D6, D8): upstream's `lines.get(..)` returns `undefined` for a missing
//! line, and `bufferService.buffer.ybase + y - 1` can be `-1`; both leave the state untouched
//! here. The listener wiring (`CoreTerminal` registers the CSI `H` handler and runs this on every
//! line feed while `windowsMode` is on) lives in `core_terminal.rs`.

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

use crate::buffer::constants::{NULL_CELL_CODE, WHITESPACE_CELL_CODE};
use crate::services::buffer_service::BufferService;

/// Marks the cursor line as wrapped when the last cell of the line above holds a character.
pub fn update_windows_mode_wrapped_state(buffers: &mut BufferService) {
    // Winpty does not support wraparound mode which means that lines will never
    // be marked as wrapped. This causes issues for things like copying a line
    // retaining the wrapped new line characters or if consumers are listening
    // in on the data stream.
    //
    // The workaround for this is to listen to every incoming line feed and mark
    // the line as wrapped if the last character in the previous line is not a
    // space. This is certainly not without its problems, but generally on
    // Windows when text reaches the end of the terminal it's likely going to be
    // wrapped.
    let last_col = buffers.cols.saturating_sub(1);
    let buffer = buffers.buffer_mut();
    let cursor_line = buffer.ybase + buffer.y;

    let last_char = cursor_line
        .checked_sub(1)
        .and_then(|index| buffer.lines.get(index))
        .map(|line| line.get(last_col));

    if let (Some(last_char), Some(next_line)) = (last_char, buffer.lines.get_mut(cursor_line)) {
        // `lastChar[CHAR_DATA_CODE_INDEX]`: the code is the fourth `CharData` element.
        let code = last_char.3;
        next_line.is_wrapped = code != NULL_CELL_CODE && code != WHITESPACE_CELL_CODE;
    }
}

#[cfg(test)]
#[path = "windows_mode_tests.rs"]
mod tests;
