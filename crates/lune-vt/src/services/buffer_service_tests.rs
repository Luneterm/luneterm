//! Tests for `buffer_service.rs`, written from `BufferService.ts` (upstream has no test file for
//! it): the constructor clamp, `resize` and its event, `reset`, both branches of `scroll` plus the
//! in-place scroll region, and `scroll_lines`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::buffer::buffer_line::DEFAULT_ATTR_DATA;
use crate::buffer::cell_data::CellData;

fn options(scrollback: usize) -> TerminalOptions {
    TerminalOptions {
        scrollback,
        ..TerminalOptions::default()
    }
}

fn service(scrollback: usize) -> (BufferService, TerminalOptions) {
    let opts = options(scrollback);
    (BufferService::new(&opts), opts)
}

/// Writes `c` into column 0 of absolute line `line`.
fn mark(service: &mut BufferService, line: usize, c: char) {
    service.buffer_mut().lines.get_mut(line).unwrap().set_cell(
        0,
        &CellData::from_char_data(&(0, c.to_string(), 1, c as u32)),
    );
}

fn first_char(service: &BufferService, line: usize) -> String {
    service.buffer().lines.get(line).unwrap().get_string(0)
}

fn column_zero(service: &BufferService) -> String {
    (0..service.buffer().lines.len())
        .map(|i| {
            let s = first_char(service, i);
            if s.is_empty() {
                '.'
            } else {
                s.chars().next().unwrap()
            }
        })
        .collect()
}

// ---- constructor ----

#[test]
fn new_takes_the_size_from_the_options_and_fills_the_normal_buffer() {
    let (service, _) = service(1000);
    assert_eq!((service.cols, service.rows), (80, 24));
    assert_eq!(service.buffer().lines.len(), 24);
    assert!(!service.is_user_scrolling);
    assert!(!service.buffers.is_alt_active());
}

#[test]
fn new_clamps_the_size_to_the_minimum() {
    let opts = TerminalOptions {
        cols: 0,
        rows: 0,
        ..TerminalOptions::default()
    };
    let service = BufferService::new(&opts);
    assert_eq!((service.cols, service.rows), (MINIMUM_COLS, MINIMUM_ROWS));
    assert_eq!(service.buffer().lines.len(), MINIMUM_ROWS);
    assert_eq!(service.buffer().lines.get(0).unwrap().len(), MINIMUM_COLS);
    let one = TerminalOptions {
        cols: 1,
        rows: 1,
        ..TerminalOptions::default()
    };
    assert_eq!(BufferService::new(&one).cols, 2);
}

#[test]
fn buffer_follows_the_active_buffer() {
    let (mut service, _) = service(1000);
    assert_eq!(service.buffer().lines.max_length(), 24 + 1000);
    service.buffers.activate_alt_buffer(None);
    assert_eq!(service.buffer().lines.max_length(), 24);
    service.buffer_mut().x = 3;
    assert_eq!(service.buffers.alt().x, 3);
}

// ---- resize ----

#[test]
fn resize_reports_what_changed_and_resizes_the_buffers() {
    let (mut service, opts) = service(1000);
    assert_eq!(
        service.resize(100, 24, &opts),
        BufferResizeEvent {
            cols: 100,
            rows: 24,
            cols_changed: true,
            rows_changed: false
        }
    );
    assert_eq!(
        service.resize(100, 30, &opts),
        BufferResizeEvent {
            cols: 100,
            rows: 30,
            cols_changed: false,
            rows_changed: true
        }
    );
    assert_eq!(
        service.resize(100, 30, &opts),
        BufferResizeEvent {
            cols: 100,
            rows: 30,
            cols_changed: false,
            rows_changed: false
        }
    );
    assert_eq!((service.cols, service.rows), (100, 30));
    for buffer in [service.buffers.normal(), service.buffers.alt()] {
        assert_eq!((buffer.cols(), buffer.rows()), (100, 30));
    }
    assert_eq!(service.buffer().lines.len(), 30);
    assert_eq!(service.buffer().lines.get(0).unwrap().len(), 100);
}

#[test]
fn resize_does_not_clamp() {
    // the caller (`CoreTerminal`) clamps; the service reports and applies what it is given
    let (mut service, opts) = service(1000);
    let event = service.resize(5, 3, &opts);
    assert_eq!((event.cols, event.rows), (5, 3));
    assert_eq!((service.cols, service.rows), (5, 3));
}

// ---- reset ----

#[test]
fn reset_rebuilds_the_buffers_and_stops_user_scrolling() {
    let (mut service, opts) = service(1000);
    for _ in 0..5 {
        service.scroll(&DEFAULT_ATTR_DATA, false);
    }
    service.scroll_lines(-2, false);
    assert!(service.is_user_scrolling);
    service.buffers.activate_alt_buffer(None);
    service.resize(40, 10, &opts);

    assert_eq!(
        service.reset(&opts),
        0,
        "the onScroll payload: the new buffer's ydisp"
    );
    assert!(!service.is_user_scrolling);
    assert!(!service.buffers.is_alt_active());
    assert_eq!((service.buffer().ybase, service.buffer().ydisp), (0, 0));
    // fresh buffers at the service's current size
    assert_eq!(service.buffer().lines.len(), 10);
    assert_eq!(service.buffer().lines.get(0).unwrap().len(), 40);
    assert_eq!(service.buffers.alt().lines.len(), 0);
}

// ---- scroll: scroll region at the top ----

#[test]
fn scroll_appends_a_line_and_moves_ybase_and_ydisp() {
    let (mut service, _) = service(1000);
    mark(&mut service, 0, 'a');
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 1);
    let buffer = service.buffer();
    assert_eq!((buffer.ybase, buffer.ydisp), (1, 1));
    assert_eq!(buffer.lines.len(), 25);
    assert_eq!(
        first_char(&service, 0),
        "a",
        "nothing is trimmed while the list has room"
    );
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 2);
    assert_eq!(service.buffer().lines.len(), 26);
}

#[test]
fn scroll_keeps_ydisp_when_the_user_has_scrolled_up() {
    let (mut service, _) = service(1000);
    for _ in 0..4 {
        service.scroll(&DEFAULT_ATTR_DATA, false);
    }
    assert_eq!(service.scroll_lines(-3, false), Some(1));
    assert!(service.is_user_scrolling);
    // `onScroll` carries ydisp, which did not follow ybase
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 1);
    assert_eq!((service.buffer().ybase, service.buffer().ydisp), (5, 1));
}

#[test]
fn scroll_recycles_the_oldest_line_when_the_list_is_full() {
    // scrollback 0: the list holds exactly the 24 rows
    let (mut service, _) = service(0);
    assert!(service.buffer().lines.is_full());
    mark(&mut service, 0, 'a');
    mark(&mut service, 1, 'b');
    mark(&mut service, 23, 'z');
    let marker = service.buffer_mut().add_marker(1);
    let gone = service.buffer_mut().add_marker(0);
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 0);
    let buffer = service.buffer();
    assert_eq!(buffer.lines.len(), 24);
    assert_eq!(
        (buffer.ybase, buffer.ydisp),
        (0, 0),
        "a trimmed buffer does not move ybase"
    );
    assert_eq!(first_char(&service, 0), "b");
    assert_eq!(first_char(&service, 22), "z");
    assert_eq!(first_char(&service, 23), "", "the recycled line is blank");
    assert_eq!(
        service.buffer().marker(marker).unwrap().line,
        0,
        "trim shifted the marker"
    );
    assert!(
        service.buffer().marker(gone).is_none(),
        "line 0 was trimmed off"
    );
}

#[test]
fn scroll_on_a_full_list_while_the_user_scrolls_keeps_the_text_stable() {
    let (mut service, _) = service(5);
    // fill the 29-line list: 5 scrolls
    for _ in 0..5 {
        service.scroll(&DEFAULT_ATTR_DATA, false);
    }
    assert!(service.buffer().lines.is_full());
    assert_eq!((service.buffer().ybase, service.buffer().ydisp), (5, 5));
    service.scroll_lines(-2, false);
    assert_eq!(service.buffer().ydisp, 3);
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 2);
    assert_eq!((service.buffer().ybase, service.buffer().ydisp), (5, 2));
    // at the very top it stays at 0
    service.scroll_lines(-2, false);
    assert_eq!(service.buffer().ydisp, 0);
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 0);
    assert_eq!(service.buffer().ydisp, 0);
}

#[test]
fn scroll_splices_when_there_are_lines_below_the_scroll_region() {
    // more lines than the viewport shows: the bottom row is not the last line
    let (mut service, _) = service(1000);
    for _ in 0..3 {
        let line = service.buffer().get_blank_line(&DEFAULT_ATTR_DATA, false);
        service.buffer_mut().lines.push(line);
    }
    mark(&mut service, 23, 'x');
    mark(&mut service, 24, 'y');
    assert_eq!(service.buffer().lines.len(), 27);
    let marker_above = service.buffer_mut().add_marker(23);
    let marker_below = service.buffer_mut().add_marker(24);
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 1);
    let buffer = service.buffer();
    assert_eq!(buffer.lines.len(), 28);
    assert_eq!((buffer.ybase, buffer.ydisp), (1, 1));
    // the new line went in after the bottom row (line 24 = ybase + scroll_bottom + 1 - 1 + 1)
    assert_eq!(first_char(&service, 23), "x");
    assert_eq!(first_char(&service, 24), "");
    assert_eq!(first_char(&service, 25), "y");
    assert_eq!(service.buffer().marker(marker_above).unwrap().line, 23);
    assert_eq!(service.buffer().marker(marker_below).unwrap().line, 25);
}

#[test]
fn scroll_blank_line_has_the_erase_attributes_and_wrap_flag() {
    let (mut service, _) = service(1000);
    let attr = AttributeData {
        fg: 0x0100_0001,
        bg: 0x0100_0002,
        ..DEFAULT_ATTR_DATA
    };
    service.scroll(&attr, true);
    let line = service.buffer().lines.get(24).unwrap();
    assert!(line.is_wrapped);
    assert_eq!((line.get_fg(0), line.get_bg(0)), (0x0100_0001, 0x0100_0002));
    assert_eq!(line.len(), 80);
    // a second line with the same attributes (the cached blank line is reused) is a separate
    // copy: changing its wrap flag leaves the first one alone
    service.scroll(&attr, false);
    assert!(service.buffer().lines.get(24).unwrap().is_wrapped);
    assert!(!service.buffer().lines.get(25).unwrap().is_wrapped);
    // different attributes get a new blank line
    service.scroll(&DEFAULT_ATTR_DATA, false);
    let last = service.buffer().lines.get(26).unwrap();
    assert_eq!((last.get_fg(0), last.get_bg(0)), (0, 0));
}

#[test]
fn scroll_reuses_the_cached_line_only_while_the_width_matches() {
    let (mut service, opts) = service(1000);
    service.scroll(&DEFAULT_ATTR_DATA, false);
    service.resize(40, 24, &opts);
    service.scroll(&DEFAULT_ATTR_DATA, false);
    let last = service.buffer().lines.len() - 1;
    assert_eq!(service.buffer().lines.get(last).unwrap().len(), 40);
}

// ---- scroll: scroll region below the top ----

#[test]
fn scroll_shifts_the_inner_region_in_place() {
    let (mut service, _) = service(1000);
    for (i, c) in "abcdefghijklmnopqrstuvwx".chars().enumerate() {
        mark(&mut service, i, c);
    }
    service.buffer_mut().scroll_top = 5; // f
    service.buffer_mut().scroll_bottom = 10; // k
    assert_eq!(service.scroll(&DEFAULT_ATTR_DATA, false), 0);
    // rows 5..=10 take the content of rows 6..=10, row 10 is blank; everything else stays
    assert_eq!(column_zero(&service), "abcdeghijk.lmnopqrstuvwx");
    let buffer = service.buffer();
    assert_eq!((buffer.ybase, buffer.ydisp), (0, 0));
    assert_eq!(buffer.lines.len(), 24, "no line enters the scrollback");
}

#[test]
fn scroll_shifts_the_inner_region_relative_to_ybase() {
    let (mut service, _) = service(1000);
    for _ in 0..3 {
        service.scroll(&DEFAULT_ATTR_DATA, false);
    }
    assert_eq!(service.buffer().ybase, 3);
    for (i, c) in "abcdefghijklmnopqrstuvwxyz0".chars().enumerate() {
        mark(&mut service, i, c);
    }
    service.buffer_mut().scroll_top = 2;
    service.buffer_mut().scroll_bottom = 4;
    service.scroll(&DEFAULT_ATTR_DATA, true);
    // absolute rows 5..=7 are the region: f g h -> g h .
    assert_eq!(column_zero(&service), "abcdegh.ijklmnopqrstuvwxyz0");
    assert!(service.buffer().lines.get(7).unwrap().is_wrapped);
    assert_eq!(service.buffer().lines.len(), 27);
}

#[test]
fn scroll_of_a_one_row_region_just_blanks_it() {
    let (mut service, _) = service(1000);
    for (i, c) in "abcdefghijklmnopqrstuvwx".chars().enumerate() {
        mark(&mut service, i, c);
    }
    service.buffer_mut().scroll_top = 5;
    service.buffer_mut().scroll_bottom = 5;
    service.scroll(&DEFAULT_ATTR_DATA, false);
    assert_eq!(column_zero(&service), "abcde.ghijklmnopqrstuvwx");
}

// ---- scroll_lines ----

fn scrolled(n: usize) -> BufferService {
    let (mut service, _) = service(1000);
    for _ in 0..n {
        service.scroll(&DEFAULT_ATTR_DATA, false);
    }
    service
}

#[test]
fn scroll_lines_up_marks_the_user_as_scrolling_and_reports_the_move() {
    let mut service = scrolled(10);
    assert_eq!(service.buffer().ydisp, 10);
    assert_eq!(service.scroll_lines(-3, false), Some(7));
    assert!(service.is_user_scrolling);
    assert_eq!(service.buffer().ydisp, 7);
}

#[test]
fn scroll_lines_clamps_to_the_buffer() {
    let mut service = scrolled(10);
    assert_eq!(service.scroll_lines(-100, false), Some(0));
    assert_eq!(service.buffer().ydisp, 0);
    assert_eq!(service.scroll_lines(1000, false), Some(10));
    assert_eq!(service.buffer().ydisp, 10);
}

#[test]
fn scroll_lines_up_at_the_top_does_nothing() {
    let mut service = scrolled(10);
    service.scroll_lines(-100, false);
    service.is_user_scrolling = false;
    assert_eq!(service.scroll_lines(-1, false), None);
    assert!(
        !service.is_user_scrolling,
        "the early return leaves the flag alone"
    );
}

#[test]
fn scroll_lines_down_to_the_bottom_ends_user_scrolling() {
    let mut service = scrolled(10);
    service.scroll_lines(-5, false);
    assert!(service.is_user_scrolling);
    assert_eq!(service.scroll_lines(2, false), Some(7));
    assert!(service.is_user_scrolling, "still above the bottom");
    assert_eq!(service.scroll_lines(3, false), Some(10));
    assert!(!service.is_user_scrolling);
}

#[test]
fn scroll_lines_without_a_change_fires_nothing() {
    let mut service = scrolled(10);
    // already at the bottom
    assert_eq!(service.scroll_lines(4, false), None);
    assert_eq!(service.scroll_lines(0, false), None);
    assert!(!service.is_user_scrolling);
    // a zero scroll while scrolled up reaches the same ydisp and does not change the flag
    service.scroll_lines(-5, false);
    assert_eq!(service.scroll_lines(0, false), None);
    assert!(service.is_user_scrolling);
}

#[test]
fn scroll_lines_can_suppress_the_event_but_still_scrolls() {
    let mut service = scrolled(10);
    assert_eq!(service.scroll_lines(-4, true), None);
    assert_eq!(service.buffer().ydisp, 6);
    assert!(service.is_user_scrolling);
    assert_eq!(service.scroll_lines(1, false), Some(7));
}

#[test]
fn scroll_lines_with_extreme_values_does_not_overflow() {
    let mut service = scrolled(10);
    assert_eq!(service.scroll_lines(i64::MIN, false), Some(0));
    assert_eq!(service.scroll_lines(i64::MAX, false), Some(10));
}
