//! Port of xterm.js src/common/buffer/Buffer.test.ts @ f447274, for the describes that do not
//! need column reflow: constructor, fillViewportRows, getWrappedRangeForLine, the non-reflow
//! `resize` describes, "buffer marked to have no scrollback", addMarker, translateBufferLineToString
//! and "memory cleanup after shrinking". The `reflow` describe (`:237-1053`, including its first
//! two cases "should not wrap empty lines" and "should shrink row length") is ported with the
//! reflow in plan M5 step 7.
//!
//! Adaptations (`adapted:`):
//!  - `MockOptionsService({ scrollback })` is a [`TerminalOptions`] value; `MockBufferService` is
//!    the pair `(INIT_COLS, INIT_ROWS)` passed to `Buffer::new`. No ported test here sets the mock
//!    service's cols apart from the buffer's.
//!  - `marker.isDisposed` / `buffer.markers.length` are `buffer.marker(id).is_none()` and
//!    `buffer.markers().len()`; `marker.onDispose` is the disposal log; `lines.onTrimEmitter.fire(n)`
//!    is `lines.notify_trim(n)`.
//!  - `buffer.lines.set(0, line)` on a buffer that was never filled writes into a JS array past
//!    `length`; here lines are `push`ed (a `CircularList` slot past `len()` is not addressable).
//!  - The memory cleanup test waits for `IdleTaskQueue`; here the idle loop is
//!    `while buffer.batched_memory_cleanup() {}`. `_data.buffer.byteLength` is observed through
//!    [`BufferLine::resize`] to the line's own length, which returns whether the allocation holds
//!    more than `CLEANUP_THRESHOLD` times the used memory.
//!
//! Added tests cover the pieces upstream leaves untested: tab stops, `clear` and markers, the
//! cursor and scrollback predicates, the Windows row-growth branch, `batched_memory_cleanup`'s
//! batching, trimming on a scrollback shrink and the guarded `get_wrapped_range_for_line`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

const INIT_COLS: usize = 80;
const INIT_ROWS: usize = 24;
const INIT_SCROLLBACK: usize = 1000;

fn options(scrollback: usize) -> TerminalOptions {
    TerminalOptions {
        scrollback,
        ..TerminalOptions::default()
    }
}

fn new_buffer() -> (Buffer, TerminalOptions) {
    let opts = options(INIT_SCROLLBACK);
    (Buffer::new(true, INIT_COLS, INIT_ROWS, &opts), opts)
}

fn char_data_at(buffer: &Buffer, line: usize, x: usize) -> crate::buffer::cell_data::CharData {
    buffer
        .lines
        .get(line)
        .unwrap()
        .load_cell(x, &mut CellData::new())
        .get_as_char_data()
}

fn set_char(buffer: &mut Buffer, line: usize, x: usize, c: &str) {
    let mut ch = char_data_at(buffer, line, x);
    ch.1 = c.to_string();
    buffer
        .lines
        .get_mut(line)
        .unwrap()
        .set_cell(x, &CellData::from_char_data(&ch));
}

// ---- constructor ----

#[test]
fn constructor_creates_a_list_of_rows_plus_scrollback() {
    let (buffer, _) = new_buffer();
    assert_eq!(buffer.lines.max_length(), INIT_ROWS + INIT_SCROLLBACK);
    assert!(buffer.lines.is_empty());
}

#[test]
fn constructor_sets_scroll_bottom_to_rows_minus_one() {
    let (buffer, _) = new_buffer();
    assert_eq!(buffer.scroll_bottom, INIT_ROWS - 1);
    assert_eq!(buffer.scroll_top, 0);
    assert_eq!(buffer.saved_charset, DEFAULT_CHARSET);
    assert_eq!(buffer.saved_cur_attr_data, DEFAULT_ATTR_DATA);
}

// ---- fillViewportRows ----

#[test]
fn fill_viewport_rows_fills_the_buffer_with_blank_lines() {
    let (mut buffer, _) = new_buffer();
    let blank_line_char = buffer
        .get_blank_line(&DEFAULT_ATTR_DATA, false)
        .load_cell(0, &mut CellData::new())
        .get_as_char_data();
    buffer.fill_viewport_rows(None);
    assert_eq!(buffer.lines.len(), INIT_ROWS);
    for y in 0..INIT_ROWS {
        assert_eq!(buffer.lines.get(y).unwrap().len(), INIT_COLS);
        for x in 0..INIT_COLS {
            assert_eq!(char_data_at(&buffer, y, x), blank_line_char);
        }
    }
}

#[test]
fn fill_viewport_rows_does_nothing_when_the_list_has_lines() {
    let (mut buffer, _) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.lines.pop();
    buffer.fill_viewport_rows(None);
    assert_eq!(buffer.lines.len(), INIT_ROWS - 1);
}

#[test]
fn fill_viewport_rows_uses_the_fill_attribute() {
    let (mut buffer, _) = new_buffer();
    let attr = AttributeData {
        fg: 0x0100_0001,
        bg: 0x0100_0002,
        extended: ExtendedAttrs::default(),
    };
    buffer.fill_viewport_rows(Some(&attr));
    let line = buffer.lines.get(3).unwrap();
    assert_eq!(line.get_fg(7), 0x0100_0001);
    assert_eq!(line.get_bg(7), 0x0100_0002);
}

// ---- getWrappedRangeForLine ----

fn filled() -> Buffer {
    let (mut buffer, _) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer
}

fn wrap(buffer: &mut Buffer, rows: &[usize]) {
    for row in rows {
        buffer.lines.get_mut(*row).unwrap().is_wrapped = true;
    }
}

#[test]
fn wrapped_range_non_wrapped_first_row() {
    assert_eq!(filled().get_wrapped_range_for_line(0), (0, 0));
}

#[test]
fn wrapped_range_non_wrapped_middle_row() {
    assert_eq!(filled().get_wrapped_range_for_line(12), (12, 12));
}

#[test]
fn wrapped_range_non_wrapped_last_row() {
    let buffer = filled();
    assert_eq!(
        buffer.get_wrapped_range_for_line(buffer.lines.len() - 1),
        (23, 23)
    );
}

#[test]
fn wrapped_range_first_row() {
    let mut buffer = filled();
    wrap(&mut buffer, &[1]);
    assert_eq!(buffer.get_wrapped_range_for_line(0), (0, 1));
}

#[test]
fn wrapped_range_middle_row_wrapping_upwards() {
    let mut buffer = filled();
    wrap(&mut buffer, &[12]);
    assert_eq!(buffer.get_wrapped_range_for_line(12), (11, 12));
}

#[test]
fn wrapped_range_middle_row_wrapping_downwards() {
    let mut buffer = filled();
    wrap(&mut buffer, &[13]);
    assert_eq!(buffer.get_wrapped_range_for_line(12), (12, 13));
}

#[test]
fn wrapped_range_middle_row_wrapping_both_ways() {
    let mut buffer = filled();
    wrap(&mut buffer, &[11, 12, 13, 14]);
    assert_eq!(buffer.get_wrapped_range_for_line(12), (10, 14));
}

#[test]
fn wrapped_range_last_row() {
    let mut buffer = filled();
    wrap(&mut buffer, &[23]);
    assert_eq!(
        buffer.get_wrapped_range_for_line(buffer.lines.len() - 1),
        (22, 23)
    );
}

#[test]
fn wrapped_range_row_that_wraps_upward_to_first_row() {
    let mut buffer = filled();
    wrap(&mut buffer, &[1]);
    assert_eq!(buffer.get_wrapped_range_for_line(1), (0, 1));
}

#[test]
fn wrapped_range_row_that_wraps_downward_to_last_row() {
    let mut buffer = filled();
    let last = buffer.lines.len() - 1;
    wrap(&mut buffer, &[last]);
    assert_eq!(buffer.get_wrapped_range_for_line(last - 1), (22, 23));
}

#[test]
fn wrapped_range_past_the_end_and_over_an_empty_list_does_not_panic() {
    // upstream dereferences `undefined` and throws; the missing line is not wrapped
    let (empty, _) = new_buffer();
    assert_eq!(empty.get_wrapped_range_for_line(0), (0, 0));
    assert_eq!(empty.get_wrapped_range_for_line(500), (500, 500));
    let mut buffer = filled();
    wrap(&mut buffer, &[23]);
    assert_eq!(buffer.get_wrapped_range_for_line(100), (100, 100));
    assert_eq!(
        buffer.get_wrapped_range_for_line(usize::MAX),
        (usize::MAX, usize::MAX)
    );
}

// ---- resize ----

#[test]
fn resize_columns_reduced_trims_the_data() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(INIT_COLS / 2, INIT_ROWS, &opts);
    assert_eq!(buffer.lines.len(), INIT_ROWS);
    for i in 0..INIT_ROWS {
        assert_eq!(buffer.lines.get(i).unwrap().len(), INIT_COLS / 2);
    }
}

#[test]
fn resize_columns_increased_adds_pad_columns() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(INIT_COLS + 10, INIT_ROWS, &opts);
    assert_eq!(buffer.lines.len(), INIT_ROWS);
    for i in 0..INIT_ROWS {
        assert_eq!(buffer.lines.get(i).unwrap().len(), INIT_COLS + 10);
    }
}

#[test]
fn resize_rows_reduced_trims_blank_lines_from_the_end() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(INIT_COLS, INIT_ROWS - 10, &opts);
    assert_eq!(buffer.lines.len(), INIT_ROWS - 10);
}

#[test]
fn resize_rows_reduced_moves_the_viewport_down_when_at_the_end() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    // Set cursor y to have 5 blank lines below it
    buffer.y = INIT_ROWS - 5 - 1;
    buffer.resize(INIT_COLS, INIT_ROWS - 10, &opts);
    // Trim 5 rows
    assert_eq!(buffer.lines.len(), INIT_ROWS - 5);
    // Shift the viewport down 5 rows
    assert_eq!(buffer.ydisp, 5);
    assert_eq!(buffer.ybase, 5);
}

#[test]
fn resize_rows_reduced_without_scrollback_trims_from_the_top_when_the_cursor_reaches_the_bottom() {
    let opts = options(0);
    let mut buffer = Buffer::new(true, INIT_COLS, INIT_ROWS, &opts);
    assert_eq!(buffer.lines.max_length(), INIT_ROWS);
    buffer.y = INIT_ROWS - 1;
    buffer.fill_viewport_rows(None);
    set_char(&mut buffer, 5, 0, "a");
    set_char(&mut buffer, INIT_ROWS - 1, 0, "b");
    buffer.resize(INIT_COLS, INIT_ROWS - 5, &opts);
    assert_eq!(char_data_at(&buffer, 0, 0).1, "a");
    assert_eq!(char_data_at(&buffer, INIT_ROWS - 1 - 5, 0).1, "b");
}

#[test]
fn resize_rows_increased_empty_buffer_adds_blank_lines_to_the_end() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    assert_eq!(buffer.ydisp, 0);
    buffer.resize(INIT_COLS, INIT_ROWS + 10, &opts);
    assert_eq!(buffer.ydisp, 0);
    assert_eq!(buffer.lines.len(), INIT_ROWS + 10);
}

#[test]
fn resize_rows_increased_filled_buffer_shows_more_of_the_buffer_above() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    // Create 10 extra blank lines
    for _ in 0..10 {
        let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
        buffer.lines.push(line);
    }
    // Set cursor to the bottom of the buffer
    buffer.y = INIT_ROWS - 1;
    // Scroll down 10 lines
    buffer.ybase = 10;
    buffer.ydisp = 10;
    assert_eq!(buffer.lines.len(), INIT_ROWS + 10);
    buffer.resize(INIT_COLS, INIT_ROWS + 5, &opts);
    // Should be should 5 more lines
    assert_eq!(buffer.ydisp, 5);
    assert_eq!(buffer.ybase, 5);
    // Should not trim the buffer
    assert_eq!(buffer.lines.len(), INIT_ROWS + 10);
}

#[test]
fn resize_rows_increased_filled_buffer_shows_more_below_when_the_viewport_is_at_the_top() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    for _ in 0..10 {
        let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
        buffer.lines.push(line);
    }
    buffer.y = INIT_ROWS - 1;
    buffer.ybase = 10;
    buffer.ydisp = 0;
    assert_eq!(buffer.lines.len(), INIT_ROWS + 10);
    buffer.resize(INIT_COLS, INIT_ROWS + 5, &opts);
    // The viewport should remain at the top
    assert_eq!(buffer.ydisp, 0);
    // The buffer ybase should move up 5 lines
    assert_eq!(buffer.ybase, 5);
    // Should not trim the buffer
    assert_eq!(buffer.lines.len(), INIT_ROWS + 10);
}

#[test]
fn resize_rows_and_columns_increased() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(INIT_COLS + 5, INIT_ROWS + 5, &opts);
    assert_eq!(buffer.lines.len(), INIT_ROWS + 5);
    for i in 0..INIT_ROWS + 5 {
        assert_eq!(buffer.lines.get(i).unwrap().len(), INIT_COLS + 5);
    }
}

// Added: the rest of `resize` that the upstream describes leave to the reflow cases.

#[test]
fn resize_keeps_the_cursor_on_screen_and_resets_the_scroll_region() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.x = 70;
    buffer.saved_x = 75;
    buffer.scroll_top = 3;
    buffer.resize(40, INIT_ROWS, &opts);
    assert_eq!((buffer.x, buffer.saved_x), (39, 39));
    assert_eq!(buffer.scroll_top, 0);
    assert_eq!(buffer.scroll_bottom, INIT_ROWS - 1);
    assert_eq!((buffer.cols(), buffer.rows()), (40, INIT_ROWS));
}

#[test]
fn resize_on_an_unfilled_buffer_only_updates_sizes() {
    let (mut buffer, opts) = new_buffer();
    buffer.y = 10;
    buffer.resize(100, 10, &opts);
    assert!(buffer.lines.is_empty());
    assert_eq!(buffer.y, 10, "the cursor is only clamped once lines exist");
    assert_eq!(buffer.scroll_bottom, 9);
    // as upstream, the maximum length shrinks only in the filled branch
    assert_eq!(buffer.lines.max_length(), INIT_ROWS + INIT_SCROLLBACK);
}

#[test]
fn resize_rows_increased_adds_lines_on_windows_instead_of_scrolling_up() {
    for windows_pty in [
        crate::services::options_service::WindowsPty {
            backend: Some(PtyBackendKind::Winpty),
            build_number: None,
        },
        crate::services::options_service::WindowsPty {
            backend: None,
            build_number: Some(0),
        },
    ] {
        let mut opts = options(INIT_SCROLLBACK);
        opts.windows_pty = windows_pty;
        let mut buffer = Buffer::new(true, INIT_COLS, INIT_ROWS, &opts);
        buffer.fill_viewport_rows(None);
        for _ in 0..10 {
            let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
            buffer.lines.push(line);
        }
        buffer.y = INIT_ROWS - 1;
        buffer.ybase = 10;
        buffer.ydisp = 10;
        buffer.resize(INIT_COLS, INIT_ROWS + 5, &opts);
        // `lines.len() (34) < new_rows + ybase (39)`: five lines are pushed, ybase stays
        assert_eq!((buffer.ybase, buffer.ydisp), (10, 10));
        assert_eq!(buffer.lines.len(), INIT_ROWS + 10 + 5);
        assert_eq!(buffer.lines.get(INIT_ROWS + 10).unwrap().len(), INIT_COLS);
    }
}

#[test]
fn resize_trims_from_the_top_when_the_scrollback_shrinks() {
    let (mut buffer, _) = new_buffer();
    buffer.fill_viewport_rows(None);
    for _ in 0..30 {
        let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
        buffer.lines.push(line);
    }
    buffer.y = INIT_ROWS - 1;
    buffer.ybase = 30;
    buffer.ydisp = 30;
    buffer.saved_y = 40;
    let marker = buffer.add_marker(10);
    let kept = buffer.add_marker(50);
    let smaller = options(20);
    buffer.resize(INIT_COLS, INIT_ROWS, &smaller);
    // new max length 44; 54 lines -> trim 10
    assert_eq!(buffer.lines.max_length(), INIT_ROWS + 20);
    assert_eq!(buffer.lines.len(), INIT_ROWS + 20);
    assert_eq!((buffer.ybase, buffer.ydisp, buffer.saved_y), (20, 20, 30));
    assert_eq!(
        buffer.marker(marker).unwrap().line,
        0,
        "10 - 10 is 0: not below 0, kept"
    );
    assert_eq!(buffer.marker(kept).unwrap().line, 40);
}

// ---- buffer marked to have no scrollback ----

#[test]
fn a_buffer_without_scrollback_always_has_a_scrollback_of_0() {
    let opts = options(1000);
    // Test size on initialization
    let mut buffer = Buffer::new(false, INIT_COLS, INIT_ROWS, &opts);
    buffer.fill_viewport_rows(None);
    assert_eq!(buffer.lines.max_length(), INIT_ROWS);
    // Test size on buffer increase
    buffer.resize(INIT_COLS, INIT_ROWS * 2, &opts);
    assert_eq!(buffer.lines.max_length(), INIT_ROWS * 2);
    // Test size on buffer decrease
    buffer.resize(INIT_COLS, INIT_ROWS / 2, &opts);
    assert_eq!(buffer.lines.max_length(), INIT_ROWS / 2);
}

// ---- addMarker ----

fn marker_buffer() -> Buffer {
    let opts = options(0);
    let mut buffer = Buffer::new(true, INIT_COLS, INIT_ROWS, &opts);
    buffer.fill_viewport_rows(None);
    buffer
}

#[test]
fn add_marker_adjusts_a_marker_line_when_the_buffer_is_trimmed() {
    let mut buffer = marker_buffer();
    let last = i64::try_from(buffer.lines.len()).unwrap() - 1;
    let marker = buffer.add_marker(last);
    assert_eq!(buffer.marker(marker).unwrap().line, last);
    buffer.lines.notify_trim(1);
    assert_eq!(buffer.marker(marker).unwrap().line, last - 1);
}

#[test]
fn add_marker_disposes_a_marker_trimmed_off_the_buffer() {
    let mut buffer = marker_buffer();
    assert_eq!(buffer.markers().len(), 0);
    let marker = buffer.add_marker(0);
    assert!(buffer.marker(marker).is_some());
    assert_eq!(buffer.markers().len(), 1);
    buffer.lines.notify_trim(1);
    assert!(buffer.marker(marker).is_none());
    assert_eq!(buffer.markers().len(), 0);
}

#[test]
fn add_marker_reports_the_disposal() {
    let mut buffer = marker_buffer();
    assert_eq!(buffer.markers().len(), 0);
    let marker = buffer.add_marker(0);
    assert!(buffer.marker(marker).is_some());
    assert_eq!(buffer.markers().len(), 1);
    buffer.lines.notify_trim(1);
    assert!(buffer.marker(marker).is_none());
    assert_eq!(buffer.markers().len(), 0);
    assert_eq!(buffer.take_disposed_markers(), vec![marker]);
    assert!(buffer.take_disposed_markers().is_empty());
}

#[test]
fn markers_follow_pushes_into_a_full_list() {
    let mut buffer = marker_buffer();
    let marker = buffer.add_marker(5);
    // the list is full (scrollback 0): a push trims one line
    let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
    buffer.lines.push(line);
    assert_eq!(buffer.marker(marker).unwrap().line, 4);
}

#[test]
fn clear_markers_disposes_only_the_given_line() {
    let mut buffer = marker_buffer();
    let a = buffer.add_marker(3);
    let b = buffer.add_marker(4);
    let c = buffer.add_marker(3);
    buffer.clear_markers(3);
    assert!(buffer.marker(a).is_none() && buffer.marker(c).is_none());
    assert!(buffer.marker(b).is_some());
    assert_eq!(buffer.take_disposed_markers(), vec![a, c]);
    buffer.clear_all_markers();
    assert!(buffer.marker(b).is_none());
    assert_eq!(buffer.take_disposed_markers(), vec![b]);
}

#[test]
fn clear_leaves_existing_markers_behind_and_new_ones_follow_the_new_list() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    let old = buffer.add_marker(5);
    buffer.clear(&opts);
    assert!(buffer.lines.is_empty());
    assert_eq!(
        (buffer.ydisp, buffer.ybase, buffer.y, buffer.x),
        (0, 0, 0, 0)
    );
    buffer.fill_viewport_rows(None);
    let new = buffer.add_marker(5);
    buffer.lines.notify_trim(2);
    // the old marker stays in the set but is not on the new list; the new one moved
    assert_eq!(buffer.marker(old).unwrap().line, 5);
    assert_eq!(buffer.marker(new).unwrap().line, 3);
    buffer.clear_all_markers();
    assert!(buffer.marker(old).is_none() && buffer.marker(new).is_none());
    assert_eq!(buffer.take_disposed_markers(), vec![old, new]);
}

// ---- translateBufferLineToString ----

fn buffer_with_line(line: BufferLine) -> Buffer {
    let (mut buffer, _) = new_buffer();
    buffer.lines.push(line);
    buffer
}

fn cell(chars: &str, width: u32, code: u32) -> CellData {
    CellData::from_char_data(&(0, chars.to_string(), width, code))
}

#[test]
fn translate_handles_selecting_a_section_of_ascii_text() {
    let mut line = BufferLine::new(4, None, false);
    line.set_cell(0, &cell("a", 1, 'a' as u32));
    line.set_cell(1, &cell("b", 1, 'b' as u32));
    line.set_cell(2, &cell("c", 1, 'c' as u32));
    line.set_cell(3, &cell("d", 1, 'd' as u32));
    let buffer = buffer_with_line(line);
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(2)),
        "ab"
    );
}

#[test]
fn translate_includes_a_cut_off_double_width_character() {
    let mut line = BufferLine::new(3, None, false);
    line.set_cell(0, &cell("語", 2, 35486));
    line.set_cell(1, &cell("", 0, 0));
    line.set_cell(2, &cell("a", 1, 'a' as u32));
    let buffer = buffer_with_line(line);
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(1)),
        "語"
    );
}

#[test]
fn translate_skips_a_zero_width_character_in_the_middle_of_the_string() {
    let mut line = BufferLine::new(3, None, false);
    line.set_cell(0, &cell("語", 2, '語' as u32));
    line.set_cell(1, &cell("", 0, 0));
    line.set_cell(2, &cell("a", 1, 'a' as u32));
    let buffer = buffer_with_line(line);
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(1)),
        "語"
    );
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(2)),
        "語"
    );
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(3)),
        "語a"
    );
}

#[test]
fn translate_handles_single_width_emojis() {
    let mut line = BufferLine::new(2, None, false);
    line.set_cell(0, &cell("😁", 1, 0xD83D));
    line.set_cell(1, &cell("a", 1, 'a' as u32));
    let buffer = buffer_with_line(line);
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(1)),
        "😁"
    );
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(2)),
        "😁a"
    );
}

#[test]
fn translate_handles_double_width_emojis() {
    let mut line = BufferLine::new(2, None, false);
    line.set_cell(0, &cell("😁", 2, 0xD83D));
    line.set_cell(1, &cell("", 0, 0));
    let mut buffer = buffer_with_line(line);
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(1)),
        "😁"
    );
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(2)),
        "😁"
    );

    let mut line2 = BufferLine::new(3, None, false);
    line2.set_cell(0, &cell("😁", 2, 0xD83D));
    line2.set_cell(1, &cell("", 0, 0));
    line2.set_cell(2, &cell("a", 1, 'a' as u32));
    buffer.lines.set(0, Some(line2));
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, true, 0, Some(3)),
        "😁a"
    );
}

#[test]
fn translate_of_a_missing_line_is_empty() {
    let (buffer, _) = new_buffer();
    assert_eq!(
        buffer.translate_buffer_line_to_string(0, false, 0, None),
        ""
    );
    assert_eq!(
        buffer.translate_buffer_line_to_string(9, false, 3, Some(1)),
        ""
    );
}

// ---- memory cleanup after shrinking ----

#[test]
fn memory_is_realigned_by_the_idle_cleanup() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    assert!(!buffer.memory_cleanup_pending());

    // shrink more than 2 times to trigger lazy memory cleanup
    buffer.resize(INIT_COLS / 2 - 1, INIT_ROWS, &opts);
    assert!(buffer.memory_cleanup_pending());

    let null_cell = buffer.get_null_cell(None);
    let probe = |buffer: &mut Buffer| -> Vec<bool> {
        (0..INIT_ROWS)
            .map(|i| {
                let line = buffer.lines.get_mut(i).unwrap();
                assert_eq!(line.len(), INIT_COLS / 2 - 1);
                // resize to the same length: does the allocation hold more than twice the use?
                line.resize(INIT_COLS / 2 - 1, &null_cell)
            })
            .collect()
    };
    // line memory is still at the old size from initialization
    assert!(probe(&mut buffer).iter().all(|excess| *excess));

    while buffer.batched_memory_cleanup() {}
    assert!(!buffer.memory_cleanup_pending());

    // cleanup realigned memory with the exact length
    assert!(probe(&mut buffer).iter().all(|excess| !*excess));
}

#[test]
fn memory_cleanup_is_batched_to_about_100_lines_and_stays_pending_until_done() {
    let opts = options(1000);
    let mut buffer = Buffer::new(true, 80, 300, &opts);
    buffer.fill_viewport_rows(None);
    buffer.resize(10, 300, &opts);
    assert!(buffer.memory_cleanup_pending());
    // 300 lines to clean at one per line: the first batch stops after the 101st
    assert!(buffer.batched_memory_cleanup());
    assert!(buffer.memory_cleanup_pending());
    let mut batches = 1;
    while buffer.batched_memory_cleanup() {
        batches += 1;
        assert!(batches < 20, "the cleanup must end");
    }
    // batches of 101, 101 and 98 lines ask for another call; the rescan finds nothing and ends
    assert_eq!(batches, 3);
    assert!(!buffer.memory_cleanup_pending());
    // nothing left to clean: a further call ends at once
    assert!(!buffer.batched_memory_cleanup());
}

#[test]
fn a_small_shrink_does_not_schedule_a_cleanup() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    // 80 -> 79 columns: the allocation is far from twice the use
    buffer.resize(INIT_COLS - 1, INIT_ROWS, &opts);
    assert!(!buffer.memory_cleanup_pending());
}

// ---- null and whitespace cells, predicates ----

#[test]
fn null_and_whitespace_cells_carry_the_attributes_asked_for() {
    let (buffer, _) = new_buffer();
    let attr = AttributeData {
        fg: 5,
        bg: 6,
        extended: ExtendedAttrs::new(7, 8),
    };
    let null = buffer.get_null_cell(Some(&attr));
    assert_eq!((null.fg, null.bg, null.extended), (5, 6, attr.extended));
    assert_eq!(null.get_as_char_data().1, "");
    assert_eq!(null.get_width(), 1);
    let space = buffer.get_whitespace_cell(Some(&attr));
    assert_eq!(space.get_as_char_data().1, " ");
    assert_eq!(space.get_code(), 32);
    // no attribute: back to the defaults (the shared upstream cell is reset)
    let plain = buffer.get_null_cell(None);
    assert_eq!(
        (plain.fg, plain.bg, plain.extended),
        (0, 0, ExtendedAttrs::default())
    );
}

#[test]
fn blank_lines_use_the_buffer_columns_and_wrap_flag() {
    let (buffer, _) = new_buffer();
    let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, true);
    assert_eq!(line.len(), INIT_COLS);
    assert!(line.is_wrapped);
}

#[test]
fn has_scrollback_needs_the_flag_and_a_list_longer_than_the_viewport() {
    let (buffer, _) = new_buffer();
    assert!(buffer.has_scrollback());
    let none = Buffer::new(true, INIT_COLS, INIT_ROWS, &options(0));
    assert!(!none.has_scrollback());
    let alt = Buffer::new(false, INIT_COLS, INIT_ROWS, &options(1000));
    assert!(!alt.has_scrollback());
}

#[test]
fn is_cursor_in_viewport_compares_the_absolute_cursor_row_to_the_viewport() {
    let (mut buffer, _) = new_buffer();
    buffer.ybase = 10;
    buffer.ydisp = 10;
    buffer.y = 20;
    assert!(buffer.is_cursor_in_viewport());
    buffer.ydisp = 0;
    assert!(
        !buffer.is_cursor_in_viewport(),
        "viewport scrolled above the cursor"
    );
    buffer.ydisp = 31;
    assert!(!buffer.is_cursor_in_viewport(), "relative row is negative");
    buffer.ydisp = 0;
    buffer.ybase = 0;
    buffer.y = INIT_ROWS - 1;
    assert!(buffer.is_cursor_in_viewport());
}

#[test]
fn the_list_length_is_capped_at_max_buffer_size() {
    let opts = options(usize::MAX);
    let buffer = Buffer::new(true, 80, 24, &opts);
    assert_eq!(buffer.lines.max_length(), MAX_BUFFER_SIZE);
    assert_eq!(MAX_BUFFER_SIZE, 4_294_967_295);
    // allocation does not follow the maximum
    let mut filled = buffer;
    filled.fill_viewport_rows(None);
    assert_eq!(filled.lines.len(), 24);
}

// ---- tab stops ----

#[test]
fn tab_stops_are_every_tab_stop_width_columns() {
    let (buffer, _) = new_buffer();
    let stops: Vec<usize> = buffer.tabs.iter().copied().collect();
    assert_eq!(stops, (0..80).step_by(8).collect::<Vec<_>>());
}

#[test]
fn setup_tab_stops_rebuilds_from_scratch_and_honours_the_width() {
    let (mut buffer, mut opts) = new_buffer();
    buffer.tabs.insert(3);
    opts.tab_stop_width = 4;
    buffer.setup_tab_stops(None, &opts);
    let stops: Vec<usize> = buffer.tabs.iter().copied().collect();
    assert_eq!(stops, (0..80).step_by(4).collect::<Vec<_>>());
}

#[test]
fn setup_tab_stops_from_a_column_keeps_earlier_stops_and_adds_the_rest() {
    let (mut buffer, opts) = new_buffer();
    buffer.resize(100, INIT_ROWS, &opts);
    // 80, 88 and 96 are not stops yet (setup ran for 80 columns; resize does not extend them)
    assert!(!buffer.tabs.contains(&88));
    buffer.setup_tab_stops(Some(100), &opts);
    // 100 is no stop: back to the previous stop (72), then every 8
    assert!(buffer.tabs.contains(&72) && buffer.tabs.contains(&88) && buffer.tabs.contains(&96));
    assert!(!buffer.tabs.contains(&100));
}

#[test]
fn a_zero_tab_stop_width_does_not_hang() {
    let (mut buffer, mut opts) = new_buffer();
    opts.tab_stop_width = 0;
    buffer.setup_tab_stops(None, &opts);
    assert_eq!(buffer.tabs.len(), INIT_COLS);
}

#[test]
fn prev_stop_and_next_stop_walk_the_stops_and_clamp() {
    let (mut buffer, _) = new_buffer();
    assert_eq!(buffer.next_stop(Some(0)), 8);
    assert_eq!(buffer.next_stop(Some(7)), 8);
    assert_eq!(buffer.next_stop(Some(8)), 16);
    assert_eq!(
        buffer.next_stop(Some(72)),
        79,
        "no stop at 80: clamped to the last column"
    );
    assert_eq!(buffer.next_stop(Some(79)), 79);
    assert_eq!(buffer.next_stop(Some(500)), 79);
    assert_eq!(buffer.prev_stop(Some(9)), 8);
    assert_eq!(buffer.prev_stop(Some(8)), 0);
    assert_eq!(buffer.prev_stop(Some(1)), 0);
    assert_eq!(buffer.prev_stop(Some(0)), 0);
    assert_eq!(
        buffer.prev_stop(Some(500)),
        72,
        "walks down to the last stop"
    );
    // default: the cursor column
    buffer.x = 20;
    assert_eq!(buffer.next_stop(None), 24);
    assert_eq!(buffer.prev_stop(None), 16);
}
