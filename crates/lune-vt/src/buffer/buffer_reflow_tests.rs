//! Port of xterm.js src/common/buffer/BufferReflow.test.ts @ f447274.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::buffer::constants::{NULL_CELL_CHAR, NULL_CELL_CODE, NULL_CELL_WIDTH};

fn ch(c: &str, width: u32) -> crate::buffer::cell_data::CharData {
    let code = c.encode_utf16().next().map_or(0, u32::from);
    (0, c.to_string(), width, code)
}

fn wide_line(cols: usize) -> BufferLine {
    // 汉语 repeated over `cols` columns
    let mut line = BufferLine::new(cols, None, false);
    for i in (0..cols).step_by(4) {
        line.set(i, &ch("汉", 2));
        line.set(i + 2, &ch("语", 2));
    }
    for i in (1..cols).step_by(2) {
        line.set(i, &ch("", 0));
        line.set(i, &ch("", 0));
    }
    line
}

fn mixed_line(is_wrapped: bool) -> BufferLine {
    let mut line = BufferLine::new(6, None, is_wrapped);
    line.set(0, &ch("a", 1));
    line.set(1, &ch("汉", 2));
    line.set(2, &ch("", 0));
    line.set(3, &ch("语", 2));
    line.set(4, &ch("", 0));
    line.set(5, &ch("b", 1));
    line
}

#[test]
fn line_lengths_for_a_small_line_with_wide_characters() {
    let mut line = BufferLine::new(4, None, false);
    line.set(0, &ch("汉", 2));
    line.set(1, &ch("", 0));
    line.set(2, &ch("语", 2));
    line.set(3, &ch("", 0));
    assert_eq!(line.translate_to_string(true, None, None, None), "汉语");
    assert_eq!(reflow_smaller_get_new_line_lengths(&[&line], 4, 3), [2, 2]);
    assert_eq!(reflow_smaller_get_new_line_lengths(&[&line], 4, 2), [2, 2]);
}

#[test]
fn line_lengths_for_a_large_line_with_wide_characters() {
    let line = wide_line(12);
    assert_eq!(
        line.translate_to_string(false, None, None, None),
        "汉语汉语汉语"
    );
    let lengths = |new_cols| reflow_smaller_get_new_line_lengths(&[&line], 12, new_cols);
    assert_eq!(lengths(11), [10, 2], "line: 汉语汉语汉, 语");
    assert_eq!(lengths(10), [10, 2], "line: 汉语汉语汉, 语");
    assert_eq!(lengths(9), [8, 4], "line: 汉语汉语, 汉语");
    assert_eq!(lengths(8), [8, 4], "line: 汉语汉语, 汉语");
    assert_eq!(lengths(7), [6, 6], "line: 汉语汉, 语汉语");
    assert_eq!(lengths(6), [6, 6], "line: 汉语汉, 语汉语");
    assert_eq!(lengths(5), [4, 4, 4], "line: 汉语, 汉语, 汉语");
    assert_eq!(lengths(4), [4, 4, 4], "line: 汉语, 汉语, 汉语");
    assert_eq!(
        lengths(3),
        [2, 2, 2, 2, 2, 2],
        "line: 汉, 语, 汉, 语, 汉, 语"
    );
    assert_eq!(
        lengths(2),
        [2, 2, 2, 2, 2, 2],
        "line: 汉, 语, 汉, 语, 汉, 语"
    );
}

#[test]
fn line_lengths_for_a_string_with_wide_and_single_characters() {
    let line = mixed_line(false);
    assert_eq!(line.translate_to_string(false, None, None, None), "a汉语b");
    let lengths = |new_cols| reflow_smaller_get_new_line_lengths(&[&line], 6, new_cols);
    assert_eq!(lengths(5), [5, 1], "line: a汉语b");
    assert_eq!(lengths(4), [3, 3], "line: a汉, 语b");
    assert_eq!(lengths(3), [3, 3], "line: a汉, 语b");
    assert_eq!(lengths(2), [1, 2, 2, 1], "line: a, 汉, 语, b");
}

#[test]
fn line_lengths_for_a_wrapped_line_with_wide_and_single_characters() {
    let line1 = mixed_line(false);
    let line2 = mixed_line(true);
    assert_eq!(line1.translate_to_string(false, None, None, None), "a汉语b");
    assert_eq!(line2.translate_to_string(false, None, None, None), "a汉语b");
    let lengths = |new_cols| reflow_smaller_get_new_line_lengths(&[&line1, &line2], 6, new_cols);
    assert_eq!(lengths(5), [5, 4, 3], "lines: a汉语, ba汉, 语b");
    assert_eq!(lengths(4), [3, 4, 4, 1], "lines: a汉, 语ba, 汉语, b");
    assert_eq!(lengths(3), [3, 3, 3, 3], "lines: a汉, 语b, a汉, 语b");
    assert_eq!(
        lengths(2),
        [1, 2, 2, 2, 2, 2, 1],
        "lines: a, 汉, 语, ba, 汉, 语, b"
    );
}

#[test]
fn line_lengths_work_on_lines_ending_in_null_space() {
    let mut line = BufferLine::new(5, None, false);
    line.set(0, &ch("汉", 2));
    line.set(1, &ch("", 0));
    line.set(2, &ch("语", 2));
    line.set(3, &ch("", 0));
    line.set(
        4,
        &(
            0,
            NULL_CELL_CHAR.to_string(),
            NULL_CELL_WIDTH,
            NULL_CELL_CODE,
        ),
    );
    assert_eq!(line.translate_to_string(true, None, None, None), "汉语");
    assert_eq!(line.translate_to_string(false, None, None, None), "汉语 ");
    assert_eq!(reflow_smaller_get_new_line_lengths(&[&line], 4, 3), [2, 2]);
    assert_eq!(reflow_smaller_get_new_line_lengths(&[&line], 4, 2), [2, 2]);
}

// ---- added: the layout functions and the trimmed length ----

#[test]
fn wrapped_line_trimmed_length_excludes_a_null_before_a_wide_start() {
    let mut first = BufferLine::new(4, None, false);
    first.set(0, &ch("a", 1));
    let second = wide_line(4);
    // a null last cell followed by a wide character counts cols - 1
    assert_eq!(get_wrapped_line_trimmed_length(&[&first, &second], 0, 4), 3);
    // the last row is its trimmed length
    assert_eq!(get_wrapped_line_trimmed_length(&[&first, &second], 1, 4), 4);
    assert_eq!(get_wrapped_line_trimmed_length(&[&first], 0, 4), 1);
    // a row that does not end in null counts every column
    assert_eq!(get_wrapped_line_trimmed_length(&[&second, &first], 0, 4), 4);
    // adapted: a missing row has length 0
    assert_eq!(get_wrapped_line_trimmed_length(&[&first], 3, 4), 0);
}

#[derive(Default)]
struct Log(Vec<String>);

impl ListObserver for Log {
    fn on_trim(&mut self, amount: usize) {
        self.0.push(format!("trim {amount}"));
    }
    fn on_insert(&mut self, e: crate::circular_list::InsertEvent) {
        self.0.push(format!("insert {} {}", e.index, e.amount));
    }
    fn on_delete(&mut self, e: crate::circular_list::DeleteEvent) {
        self.0.push(format!("delete {} {}", e.index, e.amount));
    }
}

#[test]
fn create_new_layout_skips_removed_runs_and_fires_adjusted_deletes() {
    let mut lines: CircularList<BufferLine, Log> = CircularList::with_observer(20, Log::default());
    for _ in 0..10 {
        lines.push(BufferLine::new(2, None, false));
    }
    let result = reflow_larger_create_new_layout(&mut lines, &[2, 2, 6, 3]);
    assert_eq!(result.layout, [0, 1, 4, 5, 9]);
    assert_eq!(result.count_removed, 5);
    assert_eq!(lines.observer().0, ["delete 2 2", "delete 4 3"]);
}

#[test]
fn apply_new_layout_moves_the_kept_lines_down_without_events() {
    let mut lines: CircularList<BufferLine, Log> = CircularList::with_observer(20, Log::default());
    for i in 0..5u8 {
        let mut line = BufferLine::new(1, None, false);
        line.set(0, &ch(&char::from(b'a' + i).to_string(), 1));
        lines.push(line);
    }
    reflow_larger_apply_new_layout(&mut lines, &[0, 2, 4]);
    assert_eq!(lines.len(), 3);
    let text: Vec<String> = (0..3)
        .map(|i| {
            lines
                .get(i)
                .unwrap()
                .translate_to_string(false, None, None, None)
        })
        .collect();
    assert_eq!(text, ["a", "c", "e"]);
    assert!(lines.observer().0.is_empty());
}

#[test]
fn get_lines_to_remove_unwraps_a_group_and_reports_the_empty_tail() {
    let mut lines: CircularList<BufferLine> = CircularList::new(20);
    let null = CellData::from_char_data(&(0, NULL_CELL_CHAR.to_string(), 1, NULL_CELL_CODE));
    for (i, text) in ["ab", "cd", "ef", "gh"].iter().enumerate() {
        let mut line = BufferLine::new(2, Some(&null), i % 2 == 1);
        for (x, c) in text.chars().enumerate() {
            line.set(x, &ch(&c.to_string(), 1));
        }
        line.resize(4, &null);
        lines.push(line);
    }
    // cursor elsewhere (row 10)
    let to_remove = reflow_larger_get_lines_to_remove(&mut lines, 2, 4, 10, &null, false);
    assert_eq!(to_remove, [1, 1, 3, 1]);
    assert_eq!(
        lines
            .get(0)
            .unwrap()
            .translate_to_string(false, None, None, None),
        "abcd"
    );
    assert_eq!(
        lines
            .get(2)
            .unwrap()
            .translate_to_string(false, None, None, None),
        "efgh"
    );
    // the cursor's group is left alone unless reflow_cursor_line
    let mut lines2: CircularList<BufferLine> = CircularList::new(20);
    for (i, text) in ["ab", "cd"].iter().enumerate() {
        let mut line = BufferLine::new(2, Some(&null), i == 1);
        for (x, c) in text.chars().enumerate() {
            line.set(x, &ch(&c.to_string(), 1));
        }
        line.resize(4, &null);
        lines2.push(line);
    }
    assert!(reflow_larger_get_lines_to_remove(&mut lines2, 2, 4, 1, &null, false).is_empty());
    assert_eq!(
        reflow_larger_get_lines_to_remove(&mut lines2, 2, 4, 1, &null, true),
        [1, 1]
    );
}
