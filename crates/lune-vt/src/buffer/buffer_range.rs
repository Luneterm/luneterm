//! Port of xterm.js src/common/buffer/BufferRange.ts @ f447274 (tests: BufferRange.test.ts).
//!
//! `IBufferRange` comes from the public `@xterm/xterm` typings; [`BufferRange`] and
//! [`BufferPosition`] are its owned equivalents.

use std::fmt;

/// A position in the buffer (upstream `IBufferCellPosition`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferPosition {
    pub x: usize,
    pub y: usize,
}

/// A range of cells in the buffer (upstream `IBufferRange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferRange {
    pub start: BufferPosition,
    pub end: BufferPosition,
}

/// What [`get_range_length`] rejects where upstream throws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeError {
    pub range: BufferRange,
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Buffer range end ({}, {}) cannot be before start ({}, {})",
            self.range.end.x, self.range.end.y, self.range.start.x, self.range.start.y
        )
    }
}

impl std::error::Error for RangeError {}

/// Number of cells a range covers on a buffer `buffer_cols` wide.
///
/// `Err` where upstream throws (`start.y > end.y`).
// LUNETERM: upstream also returns a negative number for a same-row range whose end is left of
// its start (`(4, 1)..(1, 1)` gives -2); this returns `Err` for it too, since a length cannot be
// negative and the caller (selection code) never passes one.
pub fn get_range_length(range: &BufferRange, buffer_cols: usize) -> Result<usize, RangeError> {
    if range.start.y > range.end.y {
        return Err(RangeError { range: *range });
    }
    // `end.y >= start.y`, so the row count is a plain subtraction; the column term may be
    // negative in upstream's arithmetic, so the sum is formed with checked i128 math.
    let rows = (range.end.y - range.start.y) as i128;
    let columns = range.end.x as i128 - range.start.x as i128 + 1;
    let length = (buffer_cols as i128)
        .checked_mul(rows)
        .and_then(|cells| cells.checked_add(columns))
        .ok_or(RangeError { range: *range })?;
    usize::try_from(length).map_err(|_| RangeError { range: *range })
}

#[cfg(test)]
mod tests {
    //! Port of xterm.js src/common/buffer/BufferRange.test.ts @ f447274.
    use super::*;

    fn create_range(x1: usize, y1: usize, x2: usize, y2: usize) -> BufferRange {
        BufferRange {
            start: BufferPosition { x: x1, y: y1 },
            end: BufferPosition { x: x2, y: y2 },
        }
    }

    mod get_range_length {
        use super::*;

        #[test]
        fn should_get_range_for_single_line() {
            assert_eq!(get_range_length(&create_range(1, 1, 4, 1), 0), Ok(4));
        }

        #[test]
        fn should_throw_for_invalid_range() {
            assert!(get_range_length(&create_range(1, 3, 1, 1), 0).is_err());
        }

        #[test]
        fn should_get_range_multiple_lines() {
            assert_eq!(get_range_length(&create_range(1, 1, 4, 5), 5), Ok(24));
        }

        #[test]
        fn should_get_range_for_end_line_right_after_start_line() {
            assert_eq!(get_range_length(&create_range(1, 1, 7, 2), 5), Ok(12));
        }
    }

    // Rust-only.
    #[test]
    fn same_row_range_ending_left_of_its_start_is_an_error() {
        assert!(get_range_length(&create_range(4, 1, 1, 1), 80).is_err());
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        let r = create_range(0, 0, usize::MAX, usize::MAX);
        let _ = get_range_length(&r, usize::MAX);
    }
}
