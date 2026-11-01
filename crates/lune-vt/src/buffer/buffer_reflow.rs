//! Port of xterm.js src/common/buffer/BufferReflow.ts @ f447274 (tests: BufferReflow.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1, D2, D8, D9):
//!  - Upstream holds the lines of a wrapped group by reference while it rewrites them in place.
//!    Here the group is taken out of the [`CircularList`] (its slots are `Option`s), rewritten as
//!    an owned `Vec<BufferLine>` and put back in the same slots; no event fires for that, as
//!    upstream fires none. A copy whose source and destination are the same line uses
//!    [`BufferLine::copy_cells_within`] (upstream's `line.copyCellsFrom(line, ...)`).
//!  - `lines.onDeleteEmitter.fire` is [`CircularList::notify_delete`].
//!  - `toRemove[k]` past the end is `undefined` upstream and never equals an index; here the
//!    lookup is an `Option` with the same outcome.
//!  - `adapted:` reads upstream would make of a missing line (`undefined`, then a throw) treat the
//!    line as absent: an empty slot ends a group, a missing line has width 0 and trimmed length 0.
//!    No valid buffer reaches them.
//!  - `new_cols >= 2` is the caller's precondition (D8): [`reflow_smaller_get_new_line_lengths`]
//!    locks up at 1 column on a line with a wide character, as upstream does.

use std::borrow::Borrow;

use crate::buffer::buffer_line::BufferLine;
use crate::buffer::cell_data::CellData;
use crate::circular_list::{CircularList, ListObserver};

/// Upstream `INewLayoutResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLayoutResult {
    pub layout: Vec<usize>,
    pub count_removed: usize,
}

/// Copies cells between two lines of `group` (or within one line when `dest == src`), as
/// upstream's `group[dest].copyCellsFrom(group[src], ...)`. A missing index copies nothing.
pub(crate) fn copy_cells_in_group(
    group: &mut [BufferLine],
    dest: usize,
    src: usize,
    src_col: usize,
    dest_col: usize,
    len: usize,
    apply_in_reverse: bool,
) {
    if dest == src {
        if let Some(line) = group.get_mut(dest) {
            line.copy_cells_within(src_col, dest_col, len, apply_in_reverse);
        }
    } else if dest < src {
        if let Some((head, tail)) = group.split_at_mut_checked(src) {
            if let (Some(d), Some(s)) = (head.get_mut(dest), tail.first()) {
                d.copy_cells_from(s, src_col, dest_col, len, apply_in_reverse);
            }
        }
    } else if let Some((head, tail)) = group.split_at_mut_checked(dest) {
        if let (Some(s), Some(d)) = (head.get(src), tail.first_mut()) {
            d.copy_cells_from(s, src_col, dest_col, len, apply_in_reverse);
        }
    }
}

/// Takes the lines `start..start + count` out of `lines` (their slots are left empty).
pub(crate) fn take_group<O: ListObserver>(
    lines: &mut CircularList<BufferLine, O>,
    start: usize,
    count: usize,
) -> Vec<BufferLine> {
    (start..start.saturating_add(count))
        .filter_map(|i| lines.take(i))
        .collect()
}

/// Puts `group` back into the slots from `start` on.
pub(crate) fn put_group<O: ListObserver>(
    lines: &mut CircularList<BufferLine, O>,
    start: usize,
    group: Vec<BufferLine>,
) {
    for (i, line) in group.into_iter().enumerate() {
        lines.set(start.saturating_add(i), Some(line));
    }
}

/// Evaluates and returns indexes to be removed after a reflow larger occurs. Lines will be removed
/// when a wrapped line unwraps. The result is a flat list of `(index, count)` pairs.
///
/// * `lines` - The buffer lines.
/// * `old_cols` - The columns before resize
/// * `new_cols` - The columns after resize.
/// * `buffer_absolute_y` - The absolute y position of the cursor (baseY + cursorY).
/// * `null_cell` - The cell data to use when filling in empty cells.
/// * `reflow_cursor_line` - Whether to reflow the line containing the cursor.
pub fn reflow_larger_get_lines_to_remove<O: ListObserver>(
    lines: &mut CircularList<BufferLine, O>,
    old_cols: usize,
    new_cols: usize,
    buffer_absolute_y: usize,
    null_cell: &CellData,
    reflow_cursor_line: bool,
) -> Vec<usize> {
    // Gather all BufferLines that need to be removed from the Buffer here so that they can be
    // batched up and only committed once
    let mut to_remove: Vec<usize> = Vec::new();
    let is_wrapped = |lines: &CircularList<BufferLine, O>, i: usize| {
        lines.get(i).is_some_and(|line| line.is_wrapped)
    };

    let mut y = 0usize;
    while y + 1 < lines.len() {
        // Check if this row is wrapped
        let mut i = y + 1;
        if !is_wrapped(lines, i) {
            y += 1;
            continue;
        }

        // Check how many lines it's wrapped for
        while i < lines.len() && is_wrapped(lines, i) {
            i += 1;
        }
        let group_len = i - y;

        if !reflow_cursor_line {
            // If these lines contain the cursor don't touch them, the program will handle fixing
            // up wrapped lines with the cursor
            if buffer_absolute_y >= y && buffer_absolute_y < i {
                y += group_len;
                continue;
            }
        }

        let mut wrapped_lines = take_group(lines, y, group_len);

        // Copy buffer data to new locations
        let mut dest_line_index = 0usize;
        let mut dest_col = wrapped_line_trimmed_length(&wrapped_lines, dest_line_index, old_cols);
        let mut src_line_index = 1usize;
        let mut src_col = 0usize;
        while src_line_index < wrapped_lines.len() {
            let src_trimmed_line_length =
                wrapped_line_trimmed_length(&wrapped_lines, src_line_index, old_cols);
            let src_remaining_cells = src_trimmed_line_length.saturating_sub(src_col);
            let dest_remaining_cells = new_cols.saturating_sub(dest_col);
            let cells_to_copy = src_remaining_cells.min(dest_remaining_cells);

            copy_cells_in_group(
                &mut wrapped_lines,
                dest_line_index,
                src_line_index,
                src_col,
                dest_col,
                cells_to_copy,
                false,
            );

            dest_col += cells_to_copy;
            if dest_col == new_cols {
                dest_line_index += 1;
                dest_col = 0;
            }
            src_col += cells_to_copy;
            if src_col == src_trimmed_line_length {
                src_line_index += 1;
                src_col = 0;
            }

            // Make sure the last cell isn't wide, if it is copy it to the current dest
            if dest_col == 0 && dest_line_index != 0 {
                let last_col = new_cols.saturating_sub(1);
                let prev = dest_line_index - 1;
                if wrapped_lines
                    .get(prev)
                    .is_some_and(|line| line.get_width(last_col) == 2)
                {
                    copy_cells_in_group(
                        &mut wrapped_lines,
                        dest_line_index,
                        prev,
                        last_col,
                        dest_col,
                        1,
                        false,
                    );
                    dest_col += 1;
                    // Null out the end of the last row
                    if let Some(line) = wrapped_lines.get_mut(prev) {
                        line.set_cell(last_col, null_cell);
                    }
                }
            }
        }

        // Clear out remaining cells or fragments could remain;
        if let Some(line) = wrapped_lines.get_mut(dest_line_index) {
            line.replace_cells(dest_col, new_cols, null_cell, false);
        }

        // Work backwards and remove any rows at the end that only contain null cells
        let mut count_to_remove = 0usize;
        for (k, line) in wrapped_lines.iter().enumerate().skip(1).rev() {
            if k > dest_line_index || line.get_trimmed_length() == 0 {
                count_to_remove += 1;
            } else {
                break;
            }
        }

        let wrapped_len = wrapped_lines.len();
        put_group(lines, y, wrapped_lines);

        if count_to_remove > 0 {
            to_remove.push(y + wrapped_len - count_to_remove); // index
            to_remove.push(count_to_remove);
        }

        y += wrapped_len;
    }
    to_remove
}

/// Creates and return the new layout for lines given an array of indexes to be removed. Fires the
/// list's delete event for each removed run (markers follow).
///
/// * `lines` - The buffer lines.
/// * `to_remove` - The indexes to remove.
pub fn reflow_larger_create_new_layout<O: ListObserver>(
    lines: &mut CircularList<BufferLine, O>,
    to_remove: &[usize],
) -> NewLayoutResult {
    let mut layout: Vec<usize> = Vec::new();
    // First iterate through the list and get the actual indexes to use for rows
    let mut next_to_remove_index = 0usize;
    let mut next_to_remove_start = to_remove.get(next_to_remove_index).copied();
    let mut count_removed_so_far = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        if next_to_remove_start == Some(i) {
            next_to_remove_index += 1;
            let count_to_remove = to_remove.get(next_to_remove_index).copied().unwrap_or(0);

            // Tell markers that there was a deletion
            lines.notify_delete(i.saturating_sub(count_removed_so_far), count_to_remove);

            // `i += countToRemove - 1`, then the loop's `i++`
            i = i.saturating_add(count_to_remove);
            count_removed_so_far = count_removed_so_far.saturating_add(count_to_remove);
            next_to_remove_index += 1;
            next_to_remove_start = to_remove.get(next_to_remove_index).copied();
        } else {
            layout.push(i);
            i += 1;
        }
    }
    NewLayoutResult {
        layout,
        count_removed: count_removed_so_far,
    }
}

/// Applies a new layout to the buffer. This essentially does the same as many splice calls but
/// it's done all at once in a single iteration through the list since splice is very expensive.
/// Fires no events.
///
/// * `lines` - The buffer lines.
/// * `new_layout` - The new layout to apply.
pub fn reflow_larger_apply_new_layout<O: ListObserver>(
    lines: &mut CircularList<BufferLine, O>,
    new_layout: &[usize],
) {
    // Record original lines so they don't get overridden when we rearrange the list. The layout
    // is strictly increasing (each index once), so every line moves out of its slot.
    let new_layout_lines: Vec<Option<BufferLine>> =
        new_layout.iter().map(|&i| lines.take(i)).collect();

    // Rearrange the list
    lines.set_len(new_layout.len());
    for (i, line) in new_layout_lines.into_iter().enumerate() {
        lines.set(i, line);
    }
}

/// Gets the new line lengths for a given wrapped line. The purpose of this function it to
/// pre-compute the wrapping points since wide characters may need to be wrapped onto the
/// following line. This function will return an array of numbers of where each line wraps to,
/// the resulting array will only contain the values `new_cols` (when the line does not end with
/// a wide character) and `new_cols - 1` (when the line does end with a wide character), except
/// for the last value which will contain the remaining items to fill the line.
///
/// Calling this with a `new_cols` value of `1` will lock up.
///
/// * `wrapped_lines` - The wrapped lines to evaluate.
/// * `old_cols` - The columns before resize.
/// * `new_cols` - The columns after resize.
pub fn reflow_smaller_get_new_line_lengths(
    wrapped_lines: &[&BufferLine],
    old_cols: usize,
    new_cols: usize,
) -> Vec<usize> {
    let mut new_line_lengths: Vec<usize> = Vec::new();
    let cells_needed: usize = (0..wrapped_lines.len())
        .map(|i| wrapped_line_trimmed_length(wrapped_lines, i, old_cols))
        .sum();

    // Use src_col and src_line to find the new wrapping point, use that to get the
    // cells_available and lines_needed
    let mut src_col = 0usize;
    let mut src_line = 0usize;
    let mut cells_available = 0usize;
    while cells_available < cells_needed {
        if cells_needed - cells_available < new_cols {
            // Add the final line and exit the loop
            new_line_lengths.push(cells_needed - cells_available);
            break;
        }
        src_col += new_cols;
        let old_trimmed_length = wrapped_line_trimmed_length(wrapped_lines, src_line, old_cols);
        if src_col > old_trimmed_length {
            src_col -= old_trimmed_length;
            src_line += 1;
        }
        let ends_with_wide = wrapped_lines.get(src_line).is_some_and(|line| {
            src_col
                .checked_sub(1)
                .is_some_and(|col| line.get_width(col) == 2)
        });
        if ends_with_wide {
            src_col -= 1;
        }
        let line_length = if ends_with_wide {
            new_cols - 1
        } else {
            new_cols
        };
        new_line_lengths.push(line_length);
        cells_available += line_length;
    }

    new_line_lengths
}

/// The length of row `i` of a wrapped group: the trimmed length for the last row, otherwise
/// `cols`, or `cols - 1` when the row ends in a null cell that a wide character at the start of
/// the next row did not fit into.
pub fn get_wrapped_line_trimmed_length(lines: &[&BufferLine], i: usize, cols: usize) -> usize {
    wrapped_line_trimmed_length(lines, i, cols)
}

/// [`get_wrapped_line_trimmed_length`] over owned or borrowed lines.
pub(crate) fn wrapped_line_trimmed_length<L: Borrow<BufferLine>>(
    lines: &[L],
    i: usize,
    cols: usize,
) -> usize {
    let Some(line) = lines.get(i).map(Borrow::borrow) else {
        return 0;
    };
    // If this is the last row in the wrapped line, get the actual trimmed length
    if i + 1 == lines.len() {
        return line.get_trimmed_length();
    }
    // Detect whether the following line starts with a wide character and the end of the current
    // line is null, if so then we can be pretty sure the null character should be excluded from
    // the line length
    let last_col = cols.saturating_sub(1);
    let ends_in_null = line.has_content(last_col) == 0 && line.get_width(last_col) == 1;
    let following_line_starts_with_wide = lines
        .get(i + 1)
        .map(Borrow::borrow)
        .is_some_and(|next| next.get_width(0) == 2);
    if ends_in_null && following_line_starts_with_wide {
        return cols.saturating_sub(1);
    }
    cols
}

#[cfg(test)]
#[path = "buffer_reflow_tests.rs"]
mod tests;
