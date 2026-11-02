//! Tests for `windows_mode.rs`, written from `WindowsMode.ts` (upstream has no test file): the
//! last character of the line above decides whether the cursor line is marked wrapped.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::buffer::attribute_data::AttributeData;
use crate::buffer::cell_data::CellData;
use crate::services::options_service::TerminalOptions;

fn service() -> BufferService {
    BufferService::new(&TerminalOptions {
        cols: 5,
        rows: 3,
        scrollback: 10,
        ..TerminalOptions::default()
    })
}

/// Writes `c` into the last column of absolute line `line`.
fn put_last(service: &mut BufferService, line: usize, c: char) {
    let last = service.cols - 1;
    service.buffer_mut().lines.get_mut(line).unwrap().set_cell(
        last,
        &CellData::from_char_data(&(0, c.to_string(), 1, c as u32)),
    );
}

fn wrapped(service: &BufferService, line: usize) -> bool {
    service.buffer().lines.get(line).unwrap().is_wrapped
}

#[test]
fn last_char_other_marks_the_cursor_line_wrapped() {
    let mut s = service();
    put_last(&mut s, 0, 'a');
    s.buffer_mut().y = 1;
    update_windows_mode_wrapped_state(&mut s);
    assert!(wrapped(&s, 1));
    assert!(!wrapped(&s, 0));
    assert!(!wrapped(&s, 2));
}

#[test]
fn last_char_whitespace_clears_the_flag() {
    let mut s = service();
    put_last(&mut s, 0, ' ');
    s.buffer_mut().y = 1;
    s.buffer_mut().lines.get_mut(1).unwrap().is_wrapped = true;
    update_windows_mode_wrapped_state(&mut s);
    assert!(!wrapped(&s, 1));
}

#[test]
fn last_char_null_clears_the_flag() {
    let mut s = service();
    s.buffer_mut().y = 1;
    s.buffer_mut().lines.get_mut(1).unwrap().is_wrapped = true;
    update_windows_mode_wrapped_state(&mut s);
    assert!(!wrapped(&s, 1));
}

#[test]
fn only_the_last_column_counts() {
    let mut s = service();
    s.buffer_mut().lines.get_mut(0).unwrap().set_cell(
        0,
        &CellData::from_char_data(&(0, "a".to_string(), 1, 'a' as u32)),
    );
    s.buffer_mut().y = 1;
    update_windows_mode_wrapped_state(&mut s);
    assert!(!wrapped(&s, 1));
}

#[test]
fn uses_ybase_plus_y() {
    let mut s = service();
    for _ in 0..2 {
        s.scroll(&AttributeData::default(), false);
    }
    assert_eq!(s.buffer().ybase, 2);
    s.buffer_mut().y = 1;
    put_last(&mut s, 2, 'z');
    update_windows_mode_wrapped_state(&mut s);
    assert!(wrapped(&s, 3));
}

#[test]
fn no_line_above_is_a_no_op() {
    let mut s = service();
    s.buffer_mut().lines.get_mut(0).unwrap().is_wrapped = true;
    update_windows_mode_wrapped_state(&mut s); // ybase + y == 0: upstream reads line -1
    assert!(wrapped(&s, 0));
}

#[test]
fn no_cursor_line_is_a_no_op() {
    let mut s = service();
    let len = s.buffer().lines.len();
    put_last(&mut s, len - 1, 'a');
    s.buffer_mut().y = len; // the cursor line does not exist
    update_windows_mode_wrapped_state(&mut s);
    for i in 0..len {
        assert!(!wrapped(&s, i));
    }
}
