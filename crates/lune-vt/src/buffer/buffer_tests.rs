//! Port of xterm.js src/common/buffer/Buffer.test.ts @ f447274: every describe, all 56 cases
//! (the `reflow` describe, `:237-1053`, with the reflow in plan M5 step 7).
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
//!  - Upstream's tests call `buffer.resize` without resizing the `MockBufferService`, so its
//!    `getBlankLine` keeps making 80-column lines (in the spliced rows and the rows reflow adds);
//!    here they have the buffer's columns. Every such line is resized to the buffer's columns
//!    before a test reads it, so no assertion differs.
//!  - "should wrap and unwrap lines" and "should discard parts of wrapped lines" resize to one
//!    column as upstream does; without wide characters that does not lock up (plan D8).
//!
//! The added reflow tests (plan M5 step 7 (a)-(d)) take their expected values from
//! @xterm/headless 6.0.0 driven through `Terminal.resize`.
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

// ---- resize: reflow (Buffer.test.ts:237-1053) ----

fn cd(c: &str, width: u32) -> crate::buffer::cell_data::CharData {
    (
        0,
        c.to_string(),
        width,
        c.encode_utf16().next().map_or(0, u32::from),
    )
}

/// `line.set(from + i, [0, c, 1, code])` for every char of `text`.
fn set_text(buffer: &mut Buffer, row: usize, from: usize, text: &str) {
    let line = buffer.lines.get_mut(row).unwrap();
    for (i, c) in text.chars().enumerate() {
        line.set(from + i, &cd(&c.to_string(), 1));
    }
}

/// `translateToString()` (no trim) of a row.
fn text(buffer: &Buffer, row: usize) -> String {
    buffer
        .lines
        .get(row)
        .unwrap()
        .translate_to_string(false, None, None, None)
}

fn trimmed(buffer: &Buffer, row: usize) -> String {
    buffer
        .lines
        .get(row)
        .unwrap()
        .translate_to_string(true, None, None, None)
}

fn assert_wrapped(buffer: &Buffer, wrapped: &[usize]) {
    for i in 0..buffer.lines.len() {
        assert_eq!(
            buffer.lines.get(i).unwrap().is_wrapped,
            wrapped.contains(&i),
            "line {i} isWrapped must equal {}",
            wrapped.contains(&i)
        );
    }
}

/// Rows as the oracle script printed them: `+` for a wrapped row, ` ` otherwise, then the text.
fn rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.lines.len())
        .map(|i| {
            let line = buffer.lines.get(i).unwrap();
            format!(
                "{}{}",
                if line.is_wrapped { '+' } else { ' ' },
                line.translate_to_string(false, None, None, None)
            )
        })
        .collect()
}

#[test]
fn reflow_should_not_wrap_empty_lines() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    assert_eq!(buffer.lines.len(), INIT_ROWS);
    buffer.resize(INIT_COLS - 5, INIT_ROWS, &opts);
    assert_eq!(buffer.lines.len(), INIT_ROWS);
}

#[test]
fn reflow_should_shrink_row_length() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(5, 10, &opts);
    assert_eq!(buffer.lines.len(), 10);
    for i in 0..10 {
        assert_eq!(buffer.lines.get(i).unwrap().len(), 5);
    }
}

#[test]
fn reflow_should_wrap_and_unwrap_lines() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(5, 10, &opts);
    set_text(&mut buffer, 0, 0, "abcde");
    buffer.y = 1;
    assert_eq!(buffer.lines.get(0).unwrap().len(), 5);
    assert_eq!(text(&buffer, 0), "abcde");
    // upstream resizes to one column here; no wide character, so no lock-up (plan D8)
    buffer.resize(1, 10, &opts);
    assert_eq!(buffer.lines.len(), 10);
    for (i, c) in ["a", "b", "c", "d", "e", " ", " ", " ", " ", " "]
        .iter()
        .enumerate()
    {
        assert_eq!(text(&buffer, i), *c);
    }
    buffer.resize(5, 10, &opts);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(text(&buffer, 0), "abcde");
    for i in 1..10 {
        assert_eq!(text(&buffer, i), "     ");
    }
}

#[test]
fn reflow_should_discard_parts_of_wrapped_lines_that_go_out_of_the_scrollback() {
    let (mut buffer, mut opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    opts.scrollback = 1;
    buffer.resize(10, 5, &opts);
    set_text(&mut buffer, 3, 0, "abcdefghij");
    assert_eq!(buffer.lines.len(), 5);
    buffer.y = 4;
    buffer.resize(2, 5, &opts);
    assert_eq!(buffer.y, 4);
    assert_eq!(buffer.ybase, 1);
    assert_eq!(buffer.lines.len(), 6);
    for (i, t) in ["ab", "cd", "ef", "gh", "ij", "  "].iter().enumerate() {
        assert_eq!(text(&buffer, i), *t);
    }
    buffer.resize(1, 5, &opts);
    assert_eq!(buffer.y, 4);
    assert_eq!(buffer.ybase, 1);
    assert_eq!(buffer.lines.len(), 6);
    for (i, t) in ["f", "g", "h", "i", "j", " "].iter().enumerate() {
        assert_eq!(text(&buffer, i), *t);
    }
    buffer.resize(10, 5, &opts);
    assert_eq!(buffer.y, 1);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 5);
    assert_eq!(text(&buffer, 0), "fghij     ");
    for i in 1..5 {
        assert_eq!(text(&buffer, i), "          ");
    }
}

#[test]
fn reflow_should_remove_the_correct_amount_of_rows_when_reflowing_larger() {
    // This is a regression test to ensure that successive wrapped lines that are getting 3+
    // lines removed on a reflow actually remove the right lines
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(10, 10, &opts);
    buffer.y = 2;
    set_text(&mut buffer, 0, 0, "abcdefghij");
    set_text(&mut buffer, 1, 0, "0123456789");
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(text(&buffer, 0), "abcdefghij");
    assert_eq!(text(&buffer, 1), "0123456789");
    for i in 2..10 {
        assert_eq!(text(&buffer, i), "          ");
    }
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.ybase, 1);
    assert_eq!(buffer.lines.len(), 11);
    for (i, t) in [
        "ab", "cd", "ef", "gh", "ij", "01", "23", "45", "67", "89", "  ",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(text(&buffer, i), *t);
    }
    buffer.resize(10, 10, &opts);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(text(&buffer, 0), "abcdefghij");
    assert_eq!(text(&buffer, 1), "0123456789");
    for i in 2..10 {
        assert_eq!(text(&buffer, i), "          ");
    }
}

#[test]
fn reflow_should_transfer_combined_char_data_over_to_reflowed_lines() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(4, 3, &opts);
    buffer.y = 2;
    {
        let line = buffer.lines.get_mut(0).unwrap();
        line.set(0, &cd("a", 1));
        line.set(1, &cd("b", 1));
        line.set(2, &cd("c", 1));
        line.set(3, &cd("😁", 1));
    }
    assert_eq!(buffer.lines.len(), 3);
    assert_eq!(text(&buffer, 0), "abc😁");
    assert_eq!(text(&buffer, 1), "    ");
    buffer.resize(2, 3, &opts);
    assert_eq!(text(&buffer, 0), "ab");
    assert_eq!(text(&buffer, 1), "c😁");
}

fn three_rows(buffer: &mut Buffer) {
    set_text(buffer, 0, 0, "abcdefghij");
    set_text(buffer, 1, 0, "0123456789");
    set_text(buffer, 2, 0, "klmnopqrst");
}

#[test]
fn reflow_should_adjust_markers_when_reflowing() {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(10, 16, &opts);
    three_rows(&mut buffer);
    buffer.y = 3;
    let first = buffer.add_marker(0);
    let second = buffer.add_marker(1);
    let third = buffer.add_marker(2);
    assert_eq!(text(&buffer, 0), "abcdefghij");
    assert_eq!(text(&buffer, 1), "0123456789");
    assert_eq!(text(&buffer, 2), "klmnopqrst");
    let line = |buffer: &Buffer, id| buffer.marker(id).unwrap().line;
    assert_eq!(line(&buffer, first), 0);
    assert_eq!(line(&buffer, second), 1);
    assert_eq!(line(&buffer, third), 2);
    buffer.resize(2, 16, &opts);
    for (i, t) in [
        "ab", "cd", "ef", "gh", "ij", "01", "23", "45", "67", "89", "kl", "mn", "op", "qr", "st",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(text(&buffer, i), *t);
    }
    assert_eq!(
        line(&buffer, first),
        0,
        "first marker should remain unchanged"
    );
    assert_eq!(
        line(&buffer, second),
        5,
        "second marker should be shifted since the first line wrapped"
    );
    assert_eq!(
        line(&buffer, third),
        10,
        "third marker should be shifted since the first and second lines wrapped"
    );
    buffer.resize(10, 16, &opts);
    assert_eq!(text(&buffer, 0), "abcdefghij");
    assert_eq!(text(&buffer, 1), "0123456789");
    assert_eq!(text(&buffer, 2), "klmnopqrst");
    assert_eq!(
        line(&buffer, first),
        0,
        "first marker should remain unchanged"
    );
    assert_eq!(
        line(&buffer, second),
        1,
        "second marker should be restored to it's original line"
    );
    assert_eq!(
        line(&buffer, third),
        2,
        "third marker should be restored to it's original line"
    );
    assert!(buffer.marker(first).is_some());
    assert!(buffer.marker(second).is_some());
    assert!(buffer.marker(third).is_some());
}

#[test]
fn reflow_should_dispose_markers_whose_rows_are_trimmed_during_a_reflow() {
    let (mut buffer, mut opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    opts.scrollback = 1;
    buffer.resize(10, 11, &opts);
    three_rows(&mut buffer);
    buffer.y = 10;
    let first = buffer.add_marker(0);
    let second = buffer.add_marker(1);
    let third = buffer.add_marker(2);
    buffer.y = 3;
    assert_eq!(text(&buffer, 0), "abcdefghij");
    assert_eq!(text(&buffer, 1), "0123456789");
    assert_eq!(text(&buffer, 2), "klmnopqrst");
    let line = |buffer: &Buffer, id| buffer.marker(id).unwrap().line;
    assert_eq!(line(&buffer, first), 0);
    assert_eq!(line(&buffer, second), 1);
    assert_eq!(line(&buffer, third), 2);
    buffer.resize(2, 11, &opts);
    for (i, t) in [
        "ij", "01", "23", "45", "67", "89", "kl", "mn", "op", "qr", "st",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(text(&buffer, i), *t);
    }
    assert_eq!(
        line(&buffer, second),
        1,
        "second marker should remain the same as it was shifted 4 and trimmed 4"
    );
    assert_eq!(
        line(&buffer, third),
        6,
        "third marker should be shifted since the first and second lines wrapped"
    );
    assert!(buffer.marker(first).is_none(), "first marker was trimmed");
    assert!(buffer.marker(second).is_some());
    assert!(buffer.marker(third).is_some());
    buffer.resize(10, 11, &opts);
    assert_eq!(text(&buffer, 0), "ij        ");
    assert_eq!(text(&buffer, 1), "0123456789");
    assert_eq!(text(&buffer, 2), "klmnopqrst");
    assert_eq!(line(&buffer, second), 1, "second marker should be restored");
    assert_eq!(line(&buffer, third), 2, "third marker should be restored");
}

fn ab_cd_wrapped(buffer: &mut Buffer, opts: &TerminalOptions) {
    buffer.fill_viewport_rows(None);
    buffer.resize(4, 10, opts);
    buffer.y = 2;
    set_text(buffer, 0, 0, "ab");
    set_text(buffer, 1, 0, "cd");
    buffer.lines.get_mut(1).unwrap().is_wrapped = true;
    // Buffer:
    // "ab  " (wrapped)
    // "cd"
}

#[test]
fn reflow_larger_wrapped_lines_that_end_in_0_space_via_tab_char() {
    let (mut buffer, opts) = new_buffer();
    ab_cd_wrapped(&mut buffer, &opts);
    buffer.resize(5, 10, &opts);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(trimmed(&buffer, 0), "ab  c");
    assert_eq!(text(&buffer, 1), "d    ");
    buffer.resize(6, 10, &opts);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(trimmed(&buffer, 0), "ab  cd");
    assert_eq!(text(&buffer, 1), "      ");
}

fn wide_rows(buffer: &mut Buffer, opts: &TerminalOptions) {
    buffer.fill_viewport_rows(None);
    buffer.resize(12, 10, opts);
    buffer.y = 2;
    for row in 0..2 {
        let line = buffer.lines.get_mut(row).unwrap();
        for i in (0..12).step_by(4) {
            line.set(i, &cd("汉", 2));
        }
        for i in (2..12).step_by(4) {
            line.set(i, &cd("语", 2));
        }
        for i in (1..12).step_by(2) {
            line.set(i, &cd("", 0));
        }
    }
    buffer.lines.get_mut(1).unwrap().is_wrapped = true;
    // Buffer:
    // 汉语汉语汉语 (wrapped)
    // 汉语汉语汉语
    assert_eq!(trimmed(buffer, 0), "汉语汉语汉语");
    assert_eq!(trimmed(buffer, 1), "汉语汉语汉语");
}

#[test]
fn reflow_should_wrap_wide_characters_correctly_when_reflowing_larger() {
    let (mut buffer, opts) = new_buffer();
    wide_rows(&mut buffer, &opts);
    buffer.resize(13, 10, &opts);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(trimmed(&buffer, 0), "汉语汉语汉语");
    assert_eq!(text(&buffer, 0), "汉语汉语汉语 ");
    assert_eq!(trimmed(&buffer, 1), "汉语汉语汉语");
    assert_eq!(text(&buffer, 1), "汉语汉语汉语 ");
    buffer.resize(14, 10, &opts);
    assert_eq!(trimmed(&buffer, 0), "汉语汉语汉语汉");
    assert_eq!(text(&buffer, 0), "汉语汉语汉语汉");
    assert_eq!(trimmed(&buffer, 1), "语汉语汉语");
    assert_eq!(text(&buffer, 1), "语汉语汉语    ");
}

#[test]
fn reflow_smaller_wrapped_lines_that_end_in_0_space_via_tab_char() {
    let (mut buffer, opts) = new_buffer();
    ab_cd_wrapped(&mut buffer, &opts);
    buffer.resize(3, 10, &opts);
    assert_eq!(buffer.y, 2);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(text(&buffer, 0), "ab ");
    assert_eq!(text(&buffer, 1), " cd");
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.y, 3);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_eq!(text(&buffer, 0), "ab");
    assert_eq!(text(&buffer, 1), "  ");
    assert_eq!(text(&buffer, 2), "cd");
}

#[test]
fn reflow_should_wrap_wide_characters_correctly_when_reflowing_smaller() {
    let (mut buffer, opts) = new_buffer();
    wide_rows(&mut buffer, &opts);
    let check = |buffer: &Buffer, expected: &[&str]| {
        for (i, t) in expected.iter().enumerate() {
            assert_eq!(trimmed(buffer, i), *t, "row {i}");
        }
    };
    buffer.resize(11, 10, &opts);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    check(&buffer, &["汉语汉语汉", "语汉语汉语", "汉语"]);
    buffer.resize(10, 10, &opts);
    check(&buffer, &["汉语汉语汉", "语汉语汉语", "汉语"]);
    buffer.resize(9, 10, &opts);
    check(&buffer, &["汉语汉语", "汉语汉语", "汉语汉语"]);
    buffer.resize(8, 10, &opts);
    check(&buffer, &["汉语汉语", "汉语汉语", "汉语汉语"]);
    buffer.resize(7, 10, &opts);
    check(&buffer, &["汉语汉", "语汉语", "汉语汉", "语汉语"]);
    buffer.resize(6, 10, &opts);
    check(&buffer, &["汉语汉", "语汉语", "汉语汉", "语汉语"]);
}

// ---- reflowLarger cases ----

/// 'ab', 'cd' (wrapped), 'ef', 'gh' (wrapped), 'ij', 'kl' (wrapped), then blank rows; 2 x 10.
fn larger_setup() -> (Buffer, TerminalOptions) {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(2, 10, &opts);
    for (row, t) in ["ab", "cd", "ef", "gh", "ij", "kl"].iter().enumerate() {
        set_text(&mut buffer, row, 0, t);
    }
    for row in [1, 3, 5] {
        buffer.lines.get_mut(row).unwrap().is_wrapped = true;
    }
    (buffer, opts)
}

/// `buffer.lines.splice(0, 0, buffer.getBlankLine(DEFAULT_ATTR_DATA))` ten times.
fn prepend_ten_blank_rows(buffer: &mut Buffer) {
    for _ in 0..10 {
        let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
        buffer.lines.splice(0, 0, vec![line]);
    }
}

fn assert_larger_content(buffer: &Buffer, top_blank: usize) {
    for i in 0..top_blank {
        assert_eq!(text(buffer, i), "    ");
    }
    assert_eq!(text(buffer, top_blank), "abcd");
    assert_eq!(text(buffer, top_blank + 1), "efgh");
    assert_eq!(text(buffer, top_blank + 2), "ijkl");
    for i in top_blank + 3..buffer.lines.len() {
        assert_eq!(text(buffer, i), "    ");
    }
    assert_wrapped(buffer, &[]);
}

#[test]
fn reflow_larger_viewport_not_yet_filled_moves_the_cursor_up_and_adds_empty_lines() {
    let (mut buffer, opts) = larger_setup();
    buffer.y = 6;
    buffer.resize(4, 10, &opts);
    assert_eq!(buffer.y, 3);
    assert_eq!(buffer.ydisp, 0);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_larger_content(&buffer, 0);
}

#[test]
fn reflow_larger_scrollback_remaining_ybase_0_moves_the_cursor_up_and_adds_empty_lines() {
    let (mut buffer, opts) = larger_setup();
    buffer.y = 9;
    buffer.resize(4, 10, &opts);
    assert_eq!(buffer.y, 6);
    assert_eq!(buffer.ydisp, 0);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_larger_content(&buffer, 0);
}

#[test]
fn reflow_larger_scrollback_remaining_ydisp_eq_ybase_adjusts_the_viewport_and_keeps_ydisp_eq_ybase()
{
    let (mut buffer, opts) = larger_setup();
    buffer.y = 9;
    prepend_ten_blank_rows(&mut buffer);
    buffer.ybase = 10;
    buffer.ydisp = 10;
    buffer.resize(4, 10, &opts);
    assert_eq!(buffer.y, 9);
    assert_eq!(buffer.ydisp, 7);
    assert_eq!(buffer.ybase, 7);
    assert_eq!(buffer.lines.len(), 17);
    assert_larger_content(&buffer, 10);
}

#[test]
fn reflow_larger_scrollback_remaining_ydisp_ne_ybase_keeps_ydisp_at_the_same_value() {
    let (mut buffer, opts) = larger_setup();
    buffer.y = 9;
    prepend_ten_blank_rows(&mut buffer);
    buffer.ybase = 10;
    buffer.ydisp = 5;
    buffer.resize(4, 10, &opts);
    assert_eq!(buffer.y, 9);
    assert_eq!(buffer.ydisp, 5);
    assert_eq!(buffer.ybase, 7);
    assert_eq!(buffer.lines.len(), 17);
    assert_larger_content(&buffer, 10);
}

fn larger_no_scrollback_setup() -> (Buffer, TerminalOptions) {
    let (mut buffer, mut opts) = larger_setup();
    opts.scrollback = 10;
    prepend_ten_blank_rows(&mut buffer);
    buffer.y = 9;
    buffer.ybase = 10;
    (buffer, opts)
}

#[test]
fn reflow_larger_no_scrollback_remaining_ydisp_eq_ybase_trims_lines_and_keeps_ydisp_eq_ybase() {
    let (mut buffer, opts) = larger_no_scrollback_setup();
    buffer.ydisp = 10;
    buffer.resize(4, 10, &opts);
    assert_eq!(buffer.y, 9);
    assert_eq!(buffer.ydisp, 7);
    assert_eq!(buffer.ybase, 7);
    assert_eq!(buffer.lines.len(), 17);
    assert_larger_content(&buffer, 10);
}

#[test]
fn reflow_larger_no_scrollback_remaining_ydisp_ne_ybase_trims_lines_and_does_not_change_ydisp() {
    let (mut buffer, opts) = larger_no_scrollback_setup();
    buffer.ydisp = 5;
    buffer.resize(4, 10, &opts);
    assert_eq!(buffer.y, 9);
    assert_eq!(buffer.ydisp, 5);
    assert_eq!(buffer.ybase, 7);
    assert_eq!(buffer.lines.len(), 17);
    assert_larger_content(&buffer, 10);
}

// ---- reflowSmaller cases ----

/// 'abcd', 'efgh' (not wrapped), 'ijkl', then blank rows; 4 x 10.
fn smaller_setup() -> (Buffer, TerminalOptions) {
    let (mut buffer, opts) = new_buffer();
    buffer.fill_viewport_rows(None);
    buffer.resize(4, 10, &opts);
    for (row, t) in ["abcd", "efgh", "ijkl"].iter().enumerate() {
        set_text(&mut buffer, row, 0, t);
    }
    (buffer, opts)
}

fn assert_smaller_content(buffer: &Buffer, top_blank: usize) {
    for i in 0..top_blank {
        assert_eq!(text(buffer, i), "  ");
    }
    for (k, t) in ["ab", "cd", "ef", "gh", "ij", "kl"].iter().enumerate() {
        assert_eq!(text(buffer, top_blank + k), *t);
    }
    for i in top_blank + 6..buffer.lines.len() {
        assert_eq!(text(buffer, i), "  ");
    }
    assert_wrapped(buffer, &[top_blank + 1, top_blank + 3, top_blank + 5]);
}

#[test]
fn reflow_smaller_viewport_not_yet_filled_moves_the_cursor_down() {
    let (mut buffer, opts) = smaller_setup();
    buffer.y = 3;
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.y, 6);
    assert_eq!(buffer.ydisp, 0);
    assert_eq!(buffer.ybase, 0);
    assert_eq!(buffer.lines.len(), 10);
    assert_smaller_content(&buffer, 0);
}

#[test]
fn reflow_smaller_scrollback_remaining_ybase_0_trims_the_top() {
    let (mut buffer, opts) = smaller_setup();
    buffer.y = 9;
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.y, 9);
    assert_eq!(buffer.ydisp, 3);
    assert_eq!(buffer.ybase, 3);
    assert_eq!(buffer.lines.len(), 13);
    assert_smaller_content(&buffer, 0);
}

#[test]
fn reflow_smaller_scrollback_remaining_ydisp_eq_ybase_adjusts_the_viewport_and_keeps_ydisp_eq_ybase()
 {
    let (mut buffer, opts) = smaller_setup();
    buffer.y = 9;
    prepend_ten_blank_rows(&mut buffer);
    buffer.ybase = 10;
    buffer.ydisp = 10;
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.ydisp, 13);
    assert_eq!(buffer.ybase, 13);
    assert_eq!(buffer.lines.len(), 23);
    assert_smaller_content(&buffer, 10);
}

#[test]
fn reflow_smaller_scrollback_remaining_ydisp_ne_ybase_keeps_ydisp_at_the_same_value() {
    let (mut buffer, opts) = smaller_setup();
    buffer.y = 9;
    prepend_ten_blank_rows(&mut buffer);
    buffer.ybase = 10;
    buffer.ydisp = 5;
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.ydisp, 5);
    assert_eq!(buffer.ybase, 13);
    assert_eq!(buffer.lines.len(), 23);
    assert_smaller_content(&buffer, 10);
}

fn smaller_no_scrollback_setup() -> (Buffer, TerminalOptions) {
    let (mut buffer, mut opts) = smaller_setup();
    opts.scrollback = 10;
    prepend_ten_blank_rows(&mut buffer);
    buffer.ybase = 10;
    (buffer, opts)
}

#[test]
fn reflow_smaller_no_scrollback_remaining_ydisp_eq_ybase_trims_lines_and_keeps_ydisp_eq_ybase() {
    let (mut buffer, opts) = smaller_no_scrollback_setup();
    buffer.ydisp = 10;
    buffer.y = 13;
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.ydisp, 10);
    assert_eq!(buffer.ybase, 10);
    assert_eq!(buffer.lines.len(), 20);
    assert_smaller_content(&buffer, 7);
}

#[test]
fn reflow_smaller_no_scrollback_remaining_ydisp_ne_ybase_trims_lines_and_does_not_change_ydisp() {
    let (mut buffer, opts) = smaller_no_scrollback_setup();
    buffer.ydisp = 5;
    buffer.y = 13;
    buffer.resize(2, 10, &opts);
    assert_eq!(buffer.ydisp, 5);
    assert_eq!(buffer.ybase, 10);
    assert_eq!(buffer.lines.len(), 20);
    assert_smaller_content(&buffer, 7);
}

// ---- added reflow tests (plan M5 step 7 (a)-(d)); expected values from @xterm/headless 6.0.0
// driven through `Terminal.resize` (scratch oracle script, recorded in the task notes) ----

#[test]
fn added_markers_before_inside_after_and_trimmed_follow_both_reflow_directions() {
    let opts = options(1);
    let mut buffer = Buffer::new(true, 10, 5, &opts);
    buffer.fill_viewport_rows(None);
    for (row, t) in ["top", "b", "0123456789", "abcdefghij", "after"]
        .iter()
        .enumerate()
    {
        set_text(&mut buffer, row, 0, t);
    }
    buffer.lines.get_mut(3).unwrap().is_wrapped = true;
    buffer.y = 4;
    let top = buffer.add_marker(0);
    let before = buffer.add_marker(1);
    let in0 = buffer.add_marker(2);
    let in1 = buffer.add_marker(3);
    let after = buffer.add_marker(4);
    let line = |buffer: &Buffer, id| buffer.marker(id).map(|m| m.line);

    // shrink: the group grows by two rows, one row trims off the top
    buffer.resize(5, 5, &opts);
    assert_eq!(
        rows(&buffer),
        [" b    ", " 01234", "+56789", "+abcde", "+fghij", " after"]
    );
    assert_eq!(
        (buffer.ybase, buffer.ydisp, buffer.y, buffer.saved_y),
        (1, 1, 4, 2)
    );
    assert_eq!(line(&buffer, top), None);
    assert_eq!(line(&buffer, before), Some(0));
    assert_eq!(line(&buffer, in0), Some(1));
    assert_eq!(line(&buffer, in1), Some(2));
    assert_eq!(line(&buffer, after), Some(5));
    assert_eq!(buffer.take_disposed_markers(), vec![top]);

    // grow: a marker on every row, the two removed rows dispose theirs
    let g: Vec<MarkerId> = (0..6).map(|i| buffer.add_marker(i)).collect();
    buffer.resize(10, 5, &opts);
    assert_eq!(
        rows(&buffer),
        [
            " b         ",
            " 0123456789",
            "+abcdefghij",
            " after     ",
            "           "
        ]
    );
    assert_eq!(
        (buffer.ybase, buffer.ydisp, buffer.y, buffer.saved_y),
        (0, 0, 3, 0)
    );
    assert_eq!(line(&buffer, before), Some(0));
    assert_eq!(line(&buffer, in0), Some(1));
    assert_eq!(line(&buffer, in1), Some(2));
    assert_eq!(line(&buffer, after), Some(3));
    let g_lines: Vec<Option<i64>> = g.iter().map(|&id| line(&buffer, id)).collect();
    assert_eq!(g_lines, [Some(0), Some(1), Some(2), None, None, Some(3)]);
    assert_eq!(buffer.take_disposed_markers(), vec![g[3], g[4]]);
}

/// Line 0 as `abe\u{301}` + `CSI 4:3 m` + `OSC 8 ;; https://x.test` + `LINK` + `OSC 8 ;;` +
/// `CSI 24 m` + `z` leaves it at 8 columns in @xterm/headless 6.0.0 (raw `_data`, `_combined`,
/// `_extendedAttrs` dumped by the oracle script).
fn oracle_link_line(buffer: &mut Buffer) {
    const DATA: [u32; 24] = [
        4194401, 0, 0, 4194402, 0, 0, 6291456, 0, 0, 4194380, 268435456, 268435456, 4194377,
        268435456, 268435456, 4194382, 268435456, 268435456, 4194379, 268435456, 268435456,
        4194426, 0, 0,
    ];
    let line = buffer.lines.get_mut(0).unwrap();
    for x in 0..8 {
        let mut cell = CellData::new();
        cell.content = DATA[x * 3];
        cell.fg = DATA[x * 3 + 1];
        cell.bg = DATA[x * 3 + 2];
        if x == 2 {
            cell.combined_data = "e\u{301}".to_string();
        }
        if (3..=6).contains(&x) {
            cell.extended = ExtendedAttrs::new(201326592, 1);
        }
        line.set_cell(x, &cell);
    }
}

/// `[chars, width, combined, has_extended, url_id, raw extended attrs]` per cell of a row.
fn cell_dump(buffer: &Buffer, row: usize) -> Vec<(String, u32, bool, bool, u32, ExtendedAttrs)> {
    let line = buffer.lines.get(row).unwrap();
    (0..line.len())
        .map(|x| {
            let mut cell = CellData::new();
            line.load_cell(x, &mut cell);
            let has_ext = cell.has_extended_attrs() != 0;
            (
                cell.get_chars(),
                cell.get_width(),
                cell.is_combined() != 0,
                has_ext,
                if has_ext { cell.extended.url_id() } else { 0 },
                if has_ext {
                    cell.extended
                } else {
                    ExtendedAttrs::default()
                },
            )
        })
        .collect()
}

#[test]
fn added_reflow_round_trip_keeps_combined_marks_and_linked_curly_underlines() {
    let opts = options(10);
    let mut buffer = Buffer::new(true, 8, 3, &opts);
    buffer.fill_viewport_rows(None);
    oracle_link_line(&mut buffer);
    buffer.y = 2;
    let none = ExtendedAttrs::default();
    let plain = |c: &str| (c.to_string(), 1, false, false, 0, none);
    // curly underline (`_ext` 3 << 26) with url id 1
    let link = |c: &str| {
        (
            c.to_string(),
            1,
            false,
            true,
            1,
            ExtendedAttrs::new(201326592, 1),
        )
    };
    let combined = ("e\u{301}".to_string(), 1, true, false, 0, none);

    buffer.resize(5, 3, &opts);
    assert_eq!(
        (buffer.ybase, buffer.ydisp, buffer.y, buffer.saved_y),
        (1, 1, 2, 1)
    );
    assert_eq!(buffer.lines.len(), 4);
    assert_eq!(text(&buffer, 0), "abe\u{301}LI");
    assert_eq!(text(&buffer, 1), "NKz  ");
    assert!(buffer.lines.get(1).unwrap().is_wrapped);
    assert_eq!(
        cell_dump(&buffer, 0),
        [
            plain("a"),
            plain("b"),
            combined.clone(),
            link("L"),
            link("I")
        ]
    );
    assert_eq!(
        cell_dump(&buffer, 1),
        [link("N"), link("K"), plain("z"), plain(""), plain("")]
    );

    buffer.resize(8, 3, &opts);
    assert_eq!(
        (buffer.ybase, buffer.ydisp, buffer.y, buffer.saved_y),
        (0, 0, 2, 0)
    );
    assert_eq!(buffer.lines.len(), 3);
    assert_eq!(text(&buffer, 0), "abe\u{301}LINKz");
    assert_eq!(
        cell_dump(&buffer, 0),
        [
            plain("a"),
            plain("b"),
            combined,
            link("L"),
            link("I"),
            link("N"),
            link("K"),
            plain("z")
        ]
    );
}

#[test]
fn added_reflow_cursor_line_decides_whether_the_cursor_group_reflows() {
    for reflow_cursor_line in [false, true] {
        let opts = TerminalOptions {
            scrollback: 10,
            reflow_cursor_line,
            ..TerminalOptions::default()
        };
        // shrink: `0123456789abcde` written at 10 columns, cursor after `e`
        let mut buffer = Buffer::new(true, 10, 5, &opts);
        buffer.fill_viewport_rows(None);
        set_text(&mut buffer, 0, 0, "0123456789");
        set_text(&mut buffer, 1, 0, "abcde");
        buffer.lines.get_mut(1).unwrap().is_wrapped = true;
        buffer.y = 1;
        buffer.x = 5;
        buffer.resize(5, 5, &opts);
        if reflow_cursor_line {
            assert_eq!(rows(&buffer)[..3], [" 01234", "+56789", "+abcde"]);
            assert_eq!((buffer.y, buffer.x, buffer.saved_y), (2, 4, 1));
        } else {
            assert_eq!(rows(&buffer)[..3], [" 01234", "+abcde", "      "]);
            assert_eq!((buffer.y, buffer.x, buffer.saved_y), (1, 4, 0));
        }
        assert_eq!(buffer.lines.len(), 5);

        // grow: `0123456789ab` written at 5 columns, cursor after `b`
        let mut buffer = Buffer::new(true, 5, 5, &opts);
        buffer.fill_viewport_rows(None);
        set_text(&mut buffer, 0, 0, "01234");
        set_text(&mut buffer, 1, 0, "56789");
        set_text(&mut buffer, 2, 0, "ab");
        buffer.lines.get_mut(1).unwrap().is_wrapped = true;
        buffer.lines.get_mut(2).unwrap().is_wrapped = true;
        buffer.y = 2;
        buffer.x = 2;
        buffer.resize(10, 5, &opts);
        if reflow_cursor_line {
            assert_eq!(rows(&buffer)[..2], [" 0123456789", "+ab        "]);
            assert_eq!(buffer.y, 1);
        } else {
            assert_eq!(
                rows(&buffer)[..3],
                [" 01234     ", "+56789     ", "+ab        "]
            );
            assert_eq!(buffer.y, 2);
        }
        assert_eq!(buffer.lines.len(), 5);
    }
}

#[test]
fn added_windows_mode_and_conpty_builds_decide_whether_reflow_runs() {
    let cases = [
        (true, None, None, false),
        (false, Some(PtyBackendKind::Conpty), Some(21375), false),
        (false, Some(PtyBackendKind::Conpty), Some(21376), true),
        (false, None, None, true),
    ];
    for (windows_mode, backend, build_number, reflows) in cases {
        let mut opts = options(10);
        opts.windows_mode = windows_mode;
        opts.windows_pty.backend = backend;
        opts.windows_pty.build_number = build_number;
        let mut buffer = Buffer::new(true, 4, 5, &opts);
        buffer.fill_viewport_rows(None);
        set_text(&mut buffer, 0, 0, "abcd");
        buffer.y = 4;
        buffer.resize(2, 5, &opts);
        let label = format!("{windows_mode} {backend:?} {build_number:?}");
        if reflows {
            assert_eq!(text(&buffer, 0), "ab", "{label}");
            assert_eq!(text(&buffer, 1), "cd", "{label}");
            assert_eq!(buffer.lines.len(), 6, "{label}");
        } else {
            // upstream trims shrunk lines only inside the reflow branch: the rows keep 4 columns
            assert_eq!(text(&buffer, 0), "abcd", "{label}");
            assert_eq!(text(&buffer, 1), "    ", "{label}");
            assert_eq!(buffer.lines.len(), 5, "{label}");
        }
    }
}

#[test]
fn added_reflow_smaller_reads_popped_rows_again_as_upstream_does() {
    // Upstream's `lines.pop()` leaves the row in its slot and the same loop reads it again
    // (row 7 here, after the group at rows 8-9 popped eight rows). Expected values from
    // @xterm/headless 6.0.0 (`Terminal.resize(2, 10)`), including its lost rows and the
    // markers it leaves past the end.
    let opts = options(100);
    let mut buffer = Buffer::new(true, 10, 10, &opts);
    buffer.fill_viewport_rows(None);
    set_text(&mut buffer, 1, 0, "top");
    set_text(&mut buffer, 7, 0, "XYZ");
    set_text(&mut buffer, 8, 0, "0123456789");
    set_text(&mut buffer, 9, 0, "abcdefghij");
    buffer.lines.get_mut(9).unwrap().is_wrapped = true;
    buffer.y = 0;
    let markers: Vec<MarkerId> = [0, 1, 7, 8, 9].map(|y| buffer.add_marker(y)).to_vec();
    buffer.resize(2, 10, &opts);
    assert_eq!(
        rows(&buffer),
        [
            "   ", "+p ", "+Z ", "+45", "+67", "+89", "+ab", "+cd", "+ef", "+gh", "+ij"
        ]
    );
    assert_eq!(
        (buffer.ybase, buffer.ydisp, buffer.y, buffer.saved_y),
        (1, 1, 9, 10)
    );
    let lines: Vec<Option<i64>> = markers
        .iter()
        .map(|&id| buffer.marker(id).map(|m| m.line))
        .collect();
    assert_eq!(lines, [Some(0), Some(11), Some(17), Some(18), Some(19)]);
}

#[test]
fn added_reflow_smaller_drops_trimmed_new_rows_instead_of_overwriting_the_last_row() {
    // adapted (plan D2): upstream's ring (start index 1 here) receives the trimmed new row `cd`
    // through `lines.set(-1)` in the last row's slot (@xterm/headless 6.0.0 ends with
    // `..., "9", "cd"`); the list here has no ring, so the last row keeps `ZZ`.
    let opts = options(5);
    let mut buffer = Buffer::new(true, 6, 5, &opts);
    buffer.fill_viewport_rows(None);
    for _ in 0..5 {
        let line = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
        buffer.lines.push(line);
    }
    for (row, t) in ["abcdef", "2", "3", "4", "5", "6", "7", "8", "9", "ZZ"]
        .iter()
        .enumerate()
    {
        set_text(&mut buffer, row, 0, t);
    }
    buffer.ybase = 5;
    buffer.ydisp = 5;
    buffer.y = 4;
    buffer.resize(2, 5, &opts);
    let text: Vec<String> = (0..buffer.lines.len())
        .map(|i| trimmed(&buffer, i))
        .collect();
    assert_eq!(text, ["ef", "2", "3", "4", "5", "6", "7", "8", "9", "ZZ"]);
    assert_eq!((buffer.ybase, buffer.y), (5, 4));
}

// ---- reflow over an empty slot ----

#[test]
fn reflow_larger_leaves_lines_in_their_slots_when_a_group_starts_on_an_empty_slot() {
    let opts = options(0);
    let mut buffer = Buffer::new(true, 4, 3, &opts);
    buffer.fill_viewport_rows(None);
    for (x, c) in ["a", "b", "c", "d"].iter().enumerate() {
        set_char(&mut buffer, 1, x, c);
    }
    set_char(&mut buffer, 2, 0, "x");
    set_char(&mut buffer, 2, 1, "y");
    wrap(&mut buffer, &[1]);
    buffer.lines.set(0, None);
    buffer.y = 2;
    let at_empty = buffer.add_marker(0);
    let at_wrapped = buffer.add_marker(1);
    let after = buffer.add_marker(2);

    buffer.resize(8, 3, &opts);

    assert!(buffer.lines.get(0).is_none(), "the empty slot stays empty");
    assert_eq!(
        buffer.translate_buffer_line_to_string(1, true, 0, None),
        "abcd"
    );
    assert!(buffer.lines.get(1).unwrap().is_wrapped);
    assert_eq!(
        buffer.translate_buffer_line_to_string(2, true, 0, None),
        "xy"
    );
    assert_eq!(buffer.lines.len(), 3);
    assert_eq!(buffer.marker(at_empty).unwrap().line, 0);
    assert_eq!(buffer.marker(at_wrapped).unwrap().line, 1);
    assert_eq!(buffer.marker(after).unwrap().line, 2);
}
