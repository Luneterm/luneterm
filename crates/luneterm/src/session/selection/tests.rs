//! The selection's tests: `SelectionModel.test.ts` ported, the word, line, wrapped-line and column
//! tests of `SelectionService.test.ts` ported onto a real `Terminal` (upstream builds lines cell by
//! cell, which this engine would not lay out the same way for emoji), and the engine-side rules
//! (trim, buffer switch, resize, drag scroll).

use lune_vt::Terminal;
use lune_vt::services::options_service::OptionValue;

use super::*;

const T0: Duration = Duration::ZERO;

// ------------------------------------------------------------------- SelectionModel.test.ts

fn model() -> SelectionModel {
    SelectionModel::default()
}

/// `MockBufferService(80, 2)`: 80 columns, `ybase` 0, 2 rows.
fn end_of(m: &SelectionModel) -> Option<Pos> {
    m.final_selection_end(80, 1)
}

#[test]
fn clear_selection_clears_the_final_selection() {
    let mut m = model();
    m.selection_start = Some([0, 0]);
    m.selection_end = Some([10, 2]);
    assert_eq!(m.final_selection_start(), Some([0, 0]));
    assert_eq!(end_of(&m), Some([10, 2]));
    m.clear_selection();
    assert_eq!(m.final_selection_start(), None);
    assert_eq!(end_of(&m), None);
}

#[test]
fn are_selection_values_reversed() {
    let mut m = model();
    m.selection_start = Some([1, 0]);
    m.selection_end = Some([0, 0]);
    assert!(m.are_selection_values_reversed());
    m.selection_start = Some([10, 2]);
    m.selection_end = Some([0, 0]);
    assert!(m.are_selection_values_reversed());
    m.selection_start = Some([0, 0]);
    m.selection_end = Some([1, 0]);
    assert!(!m.are_selection_values_reversed());
    m.selection_start = Some([0, 0]);
    m.selection_end = Some([10, 2]);
    assert!(!m.are_selection_values_reversed());
}

#[test]
fn on_trim_trims_a_portion_of_the_selection() {
    let mut m = model();
    m.selection_start = Some([0, 0]);
    m.selection_end = Some([10, 2]);
    m.handle_trim(1);
    assert_eq!(m.final_selection_start(), Some([0, 0]));
    assert_eq!(end_of(&m), Some([10, 1]));
    m.handle_trim(1);
    assert_eq!(m.final_selection_start(), Some([0, 0]));
    assert_eq!(end_of(&m), Some([10, 0]));
}

#[test]
fn on_trim_clears_a_selection_trimmed_in_its_entirety() {
    let mut m = model();
    m.selection_start = Some([0, 0]);
    m.selection_end = Some([10, 0]);
    assert!(m.handle_trim(1), "a refresh is needed");
    assert_eq!(m.final_selection_start(), None);
    assert_eq!(end_of(&m), None);
}

#[test]
fn final_selection_start_cases() {
    let mut m = model();
    m.is_select_all_active = true;
    assert_eq!(m.final_selection_start(), Some([0, 0]));
    let mut m = model();
    m.selection_start = Some([2, 2]);
    assert_eq!(m.final_selection_start(), Some([2, 2]), "no end: the start");
    m.selection_end = Some([3, 2]);
    assert_eq!(m.final_selection_start(), Some([2, 2]));
    m.selection_end = Some([1, 2]);
    assert_eq!(m.final_selection_start(), Some([1, 2]), "reversed: the end");
}

#[test]
fn final_selection_end_cases() {
    let mut m = model();
    m.is_select_all_active = true;
    assert_eq!(end_of(&m), Some([80, 1]));

    let mut m = model();
    assert_eq!(end_of(&m), None);
    m.selection_end = Some([1, 2]);
    assert_eq!(end_of(&m), None, "no start");

    // start + length with no end, or reversed, or an end inside the start selection
    let mut m = model();
    m.selection_start = Some([2, 2]);
    m.selection_start_length = 2;
    assert_eq!(end_of(&m), Some([4, 2]));
    m.selection_end = Some([2, 1]);
    assert_eq!(end_of(&m), Some([4, 2]));
    m.selection_end = Some([3, 2]);
    assert_eq!(end_of(&m), Some([4, 2]));
    // an end after start + length
    m.selection_end = Some([5, 2]);
    assert_eq!(end_of(&m), Some([5, 2]));

    // start + length overflowing onto a following row, without and with an end in between
    let mut m = model();
    m.selection_start = Some([78, 2]);
    m.selection_start_length = 4;
    assert_eq!(end_of(&m), Some([2, 3]));
    m.selection_end = Some([79, 2]);
    assert_eq!(end_of(&m), Some([2, 3]));

    // no trailing EOL when the selection ends at the end of a line
    let mut m = model();
    m.selection_start = Some([0, 0]);
    m.selection_start_length = 80;
    assert_eq!(end_of(&m), Some([80, 0]));
    m.selection_start_length = 160;
    assert_eq!(end_of(&m), Some([80, 1]));
}

// ------------------------------------------------------------------ SelectionService.test.ts

fn term(cols: usize, rows: usize, scrollback: usize) -> Terminal {
    Terminal::new([
        OptionValue::Cols(cols),
        OptionValue::Rows(rows),
        OptionValue::Scrollback(scrollback),
    ])
}

/// The tests' terminal: 20 x 20 as upstream's `MockBufferService(20, 20)`.
fn t20() -> Terminal {
    term(20, 20, 100)
}

fn put(t: &mut Terminal, text: &str) {
    t.write_str(text);
}

/// A click (`clicks` 1, 2 or 3) on cell (row, col) of an unscrolled viewport.
fn click(sel: &mut Selection, t: &mut Terminal, row: usize, col: usize, clicks: u8) {
    sel.apply(
        t,
        SelectCmd::Start {
            cell: (row, col),
            top: 0,
            clicks,
            extend: false,
            column: false,
        },
        T0,
    );
}

fn text(sel: &Selection, t: &Terminal) -> String {
    sel.text(t, MAX_COPY_BYTES)
        .expect("a selection")
        .expect("within the cap")
}

/// `selectWordAt([x, y])` then `selectionText`, on a fresh selection each time.
fn word(t: &mut Terminal, x: usize, y: usize) -> String {
    let mut sel = Selection::new();
    click(&mut sel, t, y, x, 2);
    text(&sel, t)
}

#[test]
fn select_word_at_expands_for_normal_width_chars() {
    let mut t = t20();
    put(&mut t, "foo bar");
    for (x, want) in [
        (0, "foo"),
        (1, "foo"),
        (2, "foo"),
        (3, " "),
        (4, "bar"),
        (5, "bar"),
        (6, "bar"),
    ] {
        assert_eq!(word(&mut t, x, 0), want, "x {x}");
    }
}

#[test]
fn select_word_at_expands_for_whitespace() {
    let mut t = t20();
    put(&mut t, "a   b");
    for (x, want) in [(0, "a"), (1, "   "), (2, "   "), (3, "   "), (4, "b")] {
        assert_eq!(word(&mut t, x, 0), want, "x {x}");
    }
}

#[test]
fn select_word_at_expands_for_wide_characters() {
    let mut t = t20();
    // 中文 a中文b foo: the wide characters take two columns each.
    put(&mut t, "中文 a中文b foo");
    let cases: [(usize, &str); 15] = [
        (0, "中文"),
        (1, "中文"),
        (2, "中文"),
        (3, "中文"),
        (4, " "),
        (5, "a中文b"),
        (6, "a中文b"),
        (7, "a中文b"),
        (8, "a中文b"),
        (9, "a中文b"),
        (10, "a中文b"),
        (11, " "),
        (12, "foo"),
        (13, "foo"),
        (14, "foo"),
    ];
    for (x, want) in cases {
        assert_eq!(word(&mut t, x, 0), want, "x {x}");
    }
}

#[test]
fn select_word_at_stops_at_non_path_characters_commonly_adjacent_to_paths() {
    let mut t = t20();
    put(&mut t, "(cd)[ef]{gh}'ij\"");
    let cases: [(usize, &str); 16] = [
        (0, "(cd"),
        (1, "cd"),
        (2, "cd"),
        (3, "cd)"),
        (4, "[ef"),
        (5, "ef"),
        (6, "ef"),
        (7, "ef]"),
        (8, "{gh"),
        (9, "gh"),
        (10, "gh"),
        (11, "gh}"),
        (12, "'ij"),
        (13, "ij"),
        (14, "ij"),
        (15, "ij\""),
    ];
    for (x, want) in cases {
        assert_eq!(word(&mut t, x, 0), want, "x {x}");
    }
}

#[test]
fn the_word_separators_are_the_engine_option() {
    let mut t = t20();
    put(&mut t, "a-b c");
    assert_eq!(word(&mut t, 0, 0), "a-b", "- is no separator by default");
    t.set_option(OptionValue::WordSeparator("- ".into()))
        .expect("option");
    assert_eq!(word(&mut t, 0, 0), "a");
    assert_eq!(word(&mut t, 2, 0), "b");
}

#[test]
fn select_word_at_expands_upwards_or_downwards_for_wrapped_lines() {
    let mut t = t20();
    // 17 spaces, foo, bar: "bar" wraps onto the second line.
    put(&mut t, &format!("{}foobar", " ".repeat(17)));
    assert!(t.buffers().active().lines.get(1).expect("line").is_wrapped);
    assert_eq!(word(&mut t, 1, 1), "foobar");
    assert_eq!(word(&mut t, 18, 0), "foobar");
}

#[test]
fn select_word_at_expands_over_many_wrapped_lines() {
    let mut t = t20();
    let expected = format!(
        "foo{}{}{}bar",
        "a".repeat(20),
        "b".repeat(20),
        "c".repeat(20)
    );
    put(
        &mut t,
        &format!(
            "{}foo{}{}{}bar",
            " ".repeat(17),
            "a".repeat(20),
            "b".repeat(20),
            "c".repeat(20)
        ),
    );
    for (x, y) in [(18, 0), (10, 1), (10, 2), (10, 3), (1, 4)] {
        assert_eq!(word(&mut t, x, y), expected, "({x}, {y})");
    }
}

#[test]
fn astral_characters_count_their_utf16_units_as_upstream_does() {
    // U+1F601 is two UTF-16 units; whatever width this engine's Unicode version gives it, a word
    // around it is selected whole, and the characters on both sides come with it.
    let mut t = t20();
    put(&mut t, " \u{1F601}ab cd\u{1F601} ef\u{1F601}gh x");
    let line = t
        .buffers()
        .active()
        .translate_buffer_line_to_string(0, true, 0, None);
    let col_of = |needle: &str| -> usize {
        // Columns: wide characters count two, so walk the cells.
        let l = t.buffers().active().lines.get(0).expect("line");
        (0..l.len())
            .find(|&i| l.get_string(i).starts_with(needle))
            .expect("the character is on the line")
    };
    let (a, c, e) = (col_of("a"), col_of("c"), col_of("e"));
    assert_eq!(word(&mut t, a, 0), "\u{1F601}ab", "{line:?}");
    assert_eq!(word(&mut t, c, 0), "cd\u{1F601}");
    assert_eq!(word(&mut t, e, 0), "ef\u{1F601}gh");
}

#[test]
fn select_line_at_selects_the_entire_line() {
    let mut t = t20();
    put(&mut t, "foo bar");
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 0, 3, 3);
    assert_eq!(text(&sel, &t), "foo bar");
    assert_eq!(sel.model.selection_start, Some([0, 0]));
    assert_eq!(sel.model.selection_end, None);
    assert_eq!(sel.model.selection_start_length, 20);
    assert_eq!(sel.model.final_selection_start(), Some([0, 0]));
    assert_eq!(sel.model.final_selection_end(20, 19), Some([20, 0]));
}

#[test]
fn select_line_at_selects_the_entire_wrapped_line() {
    let mut t = t20();
    put(&mut t, &format!("{}bar", "x".repeat(20)));
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 1, 0, 3);
    assert_eq!(text(&sel, &t), format!("{}bar", "x".repeat(20)));
    assert_eq!(sel.model.selection_start, Some([0, 0]));
    assert_eq!(sel.model.selection_start_length, 40);
    assert_eq!(sel.model.final_selection_end(20, 19), Some([20, 1]));
}

#[test]
fn has_selection_needs_a_start_and_a_different_end() {
    let t = t20();
    let mut sel = Selection::new();
    sel.model.selection_start = Some([0, 0]);
    sel.model.selection_start_length = 0;
    assert!(!sel.has_selection(&t));
    sel.model.selection_end = Some([0, 0]);
    assert!(!sel.has_selection(&t));
    for end in [[1, 0], [0, 1], [1, 1]] {
        sel.model.selection_end = Some(end);
        assert!(sel.has_selection(&t), "{end:?}");
    }
}

fn column_text(t: &Terminal, start: Pos, end: Pos) -> String {
    let mut sel = Selection::new();
    sel.mode = SelectionMode::Column;
    sel.model.selection_start = Some(start);
    sel.model.selection_end = Some(end);
    text(&sel, t)
}

#[test]
fn column_selection_selects_a_column_of_text() {
    let mut t = t20();
    put(&mut t, "abcdefghij\r\nklmnopqrst\r\nuvwxyz");
    assert_eq!(column_text(&t, [2, 0], [4, 2]), "cd\nmn\nwx");
    // The corners swap independently.
    assert_eq!(column_text(&t, [4, 2], [2, 0]), "cd\nmn\nwx");
    assert_eq!(column_text(&t, [4, 0], [2, 2]), "cd\nmn\nwx");
}

#[test]
fn column_selection_does_not_chop_wide_characters() {
    let mut t = t20();
    put(&mut t, "a\r\n語\r\nb");
    assert_eq!(column_text(&t, [0, 0], [1, 2]), "a\n語\nb");
}

#[test]
fn column_selection_with_a_single_character_emoji() {
    let mut t = t20();
    put(&mut t, "a\r\n☃\r\nc");
    assert_eq!(column_text(&t, [0, 0], [1, 2]), "a\n☃\nc");
}

#[test]
fn a_zero_width_column_selection_is_empty() {
    let mut t = t20();
    put(&mut t, "abc\r\ndef");
    assert_eq!(column_text(&t, [1, 0], [1, 1]), "");
}

// ----------------------------------------------------------- the text of a selection

fn drag(sel: &mut Selection, t: &mut Terminal, from: (usize, usize), to: (usize, usize)) {
    click(sel, t, from.0, from.1, 1);
    sel.apply(
        t,
        SelectCmd::Drag {
            cell: to,
            top: 0,
            scroll: 0,
        },
        T0,
    );
    sel.apply(t, SelectCmd::End, T0);
}

#[test]
fn a_drag_selects_across_lines_and_trims_trailing_blanks() {
    let mut t = t20();
    put(&mut t, "hello world\r\nsecond line\r\nthird");
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (0, 6), (2, 3));
    assert_eq!(text(&sel, &t), "world\nsecond line\nthi");
    // A drag the other way selects the same text.
    let mut back = Selection::new();
    drag(&mut back, &mut t, (2, 3), (0, 6));
    assert_eq!(text(&back, &t), "world\nsecond line\nthi");
}

#[test]
fn a_wrapped_line_copies_without_a_break_inside_it() {
    let mut t = t20();
    put(&mut t, &format!("{}tail\r\nnext", "x".repeat(20)));
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (0, 0), (2, 4));
    assert_eq!(text(&sel, &t), format!("{}tail\nnext", "x".repeat(20)));
}

#[test]
fn a_click_without_a_drag_selects_nothing_and_copies_nothing() {
    let mut t = t20();
    put(&mut t, "hello");
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 0, 2, 1);
    assert!(!sel.has_selection(&t));
    assert_eq!(sel.view(&t), None);
}

#[test]
fn a_shift_click_extends_the_selection_from_its_start() {
    let mut t = t20();
    put(&mut t, "hello world");
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 0, 0, 1);
    sel.apply(&mut t, SelectCmd::End, T0);
    sel.apply(
        &mut t,
        SelectCmd::Start {
            cell: (0, 5),
            top: 0,
            clicks: 1,
            extend: true,
            column: false,
        },
        T0,
    );
    assert_eq!(text(&sel, &t), "hello");
    // With no selection start an incremental click does nothing.
    let mut none = Selection::new();
    none.apply(
        &mut t,
        SelectCmd::Start {
            cell: (0, 4),
            top: 0,
            clicks: 1,
            extend: true,
            column: false,
        },
        T0,
    );
    assert_eq!(none.view(&t), None);
}

#[test]
fn a_double_click_then_a_drag_selects_whole_words() {
    let mut t = t20();
    put(&mut t, "one two three");
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 0, 5, 2); // "two"
    sel.apply(
        &mut t,
        SelectCmd::Drag {
            cell: (0, 9),
            top: 0,
            scroll: 0,
        },
        T0,
    );
    assert_eq!(text(&sel, &t), "two three");
}

#[test]
fn the_second_half_of_a_wide_character_selects_the_whole_character() {
    let mut t = t20();
    put(&mut t, "a中b");
    let mut sel = Selection::new();
    // Press on the right half (column 2), drag to the end of the line.
    drag(&mut sel, &mut t, (0, 2), (0, 5));
    assert!(text(&sel, &t).starts_with('b') || text(&sel, &t).starts_with('中'));
    let mut left = Selection::new();
    drag(&mut left, &mut t, (0, 1), (0, 4));
    assert!(text(&left, &t).starts_with('中'));
}

#[test]
fn non_breaking_spaces_are_copied_as_spaces() {
    let mut t = t20();
    put(&mut t, "a\u{a0}b");
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 0, 0, 3);
    assert_eq!(text(&sel, &t), "a b");
}

#[test]
fn a_text_over_the_cap_is_refused_whole() {
    let mut t = t20();
    put(&mut t, "hello world\r\nsecond");
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (0, 0), (1, 6));
    assert!(sel.text(&t, 100).expect("selection").is_ok());
    assert_eq!(
        sel.text(&t, 10).expect("selection"),
        Err(CopyError::TooLarge)
    );
    // The cap counts the line breaks: the exact size is accepted, one under is not.
    let exact = "hello world".len() + EOL.len() + "second".len();
    assert!(sel.text(&t, exact).expect("selection").is_ok());
    assert!(sel.text(&t, exact - 1).expect("selection").is_err());
}

// ------------------------------------------------------- trim, buffer switch, resize, input

/// 10 columns, 3 rows, 4 lines of scrollback: 7 lines at most.
fn small() -> Terminal {
    term(10, 3, 4)
}

fn lines(t: &mut Terminal, from: usize, to: usize) {
    for i in from..to {
        put(t, &format!("l{i}\r\n"));
    }
}

#[test]
fn the_selection_shifts_with_a_trim_and_clears_when_its_anchor_is_trimmed() {
    let mut t = small();
    lines(&mut t, 0, 3); // l0..l2 on lines 0..2, the cursor on line 3
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (1, 0), (2, 1)); // l1 .. "l" of l2: lines 1 and 2
    assert_eq!(text(&sel, &t), "l1\nl");
    let anchor_before = sel.view(&t).expect("selected").start.1;
    assert_eq!(anchor_before, 1);

    // Fill the buffer (7 lines), then one more line trims one.
    lines(&mut t, 3, 7);
    sel.sync(&mut t);
    let after = sel.view(&t).expect("still selected");
    let total = t.buffers().active().lines.len();
    assert_eq!(total, 7, "the buffer is full");
    assert!(
        after.start.1 < anchor_before,
        "moved up with its text: {after:?}"
    );
    assert_eq!(text(&sel, &t), "l1\nl", "the same text");

    // Trim past the anchor: gone.
    lines(&mut t, 8, 14);
    sel.sync(&mut t);
    assert_eq!(sel.view(&t), None);
    assert!(sel.model.selection_start.is_none());
}

#[test]
fn trimming_a_line_below_the_anchor_leaves_the_selection() {
    let mut t = small();
    lines(&mut t, 0, 3);
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (1, 0), (2, 1));
    // No trim yet (7 lines fit): the view is untouched.
    lines(&mut t, 3, 5);
    sel.sync(&mut t);
    assert_eq!(sel.view(&t).expect("selected").start, (0, 1));
}

#[test]
fn a_new_click_moves_the_anchor_marker_instead_of_adding_one() {
    let mut t = small();
    lines(&mut t, 0, 3);
    let mut sel = Selection::new();
    for _ in 0..50 {
        click(&mut sel, &mut t, 1, 0, 1);
    }
    assert_eq!(t.buffers().normal().markers().len(), 1);
    sel.clear(&mut t);
    assert_eq!(t.buffers().normal().markers().len(), 0);
}

#[test]
fn switching_to_the_alternate_buffer_clears_the_selection() {
    let mut t = small();
    lines(&mut t, 0, 2);
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (0, 0), (1, 1));
    assert!(sel.has_selection(&t));
    put(&mut t, "\x1b[?1049h");
    sel.sync(&mut t);
    assert!(!sel.has_selection(&t));
    // And back: still nothing, and a selection can be made on the alternate buffer.
    put(&mut t, "\x1b[Halt text");
    let mut alt = Selection::new();
    drag(&mut alt, &mut t, (0, 0), (0, 3));
    assert_eq!(text(&alt, &t), "alt");
    put(&mut t, "\x1b[?1049l");
    alt.sync(&mut t);
    assert!(!alt.has_selection(&t));
}

#[test]
fn a_vertical_resize_clears_and_a_user_input_clears() {
    let mut t = small();
    lines(&mut t, 0, 2);
    let mut sel = Selection::new();
    drag(&mut sel, &mut t, (0, 0), (1, 1));
    sel.rows_changed(&mut t);
    assert!(!sel.has_selection(&t));
    drag(&mut sel, &mut t, (0, 0), (1, 1));
    sel.user_input(&mut t);
    assert!(!sel.has_selection(&t));
    // Input with nothing selected leaves it nothing.
    sel.user_input(&mut t);
    assert_eq!(sel.view(&t), None);
}

// ------------------------------------------------------------------- drag scroll, mapping

#[test]
fn drag_scroll_amount_follows_the_pointer_beyond_the_viewport() {
    // Inside (inclusive of both edges): nothing.
    assert_eq!(drag_scroll_amount(0.0, 300.0), 0);
    assert_eq!(drag_scroll_amount(150.0, 300.0), 0);
    assert_eq!(drag_scroll_amount(300.0, 300.0), 0);
    // Above: negative; the first pixel is a line, the threshold is the top speed.
    assert_eq!(drag_scroll_amount(-1.0, 300.0), -1);
    assert_eq!(drag_scroll_amount(-50.0, 300.0), -15);
    assert_eq!(drag_scroll_amount(-500.0, 300.0), -15);
    // Below.
    assert_eq!(drag_scroll_amount(301.0, 300.0), 1);
    assert_eq!(drag_scroll_amount(350.0, 300.0), 15);
    assert_eq!(drag_scroll_amount(10_000.0, 300.0), 15);
    // Not a number: nothing.
    assert_eq!(drag_scroll_amount(f32::NAN, 300.0), 0);
}

#[test]
fn a_drag_beyond_the_viewport_scrolls_on_the_timer_and_keeps_the_end_at_the_edge() {
    let mut t = small();
    lines(&mut t, 0, 6); // scrollback to look at
    let mut sel = Selection::new();
    click(&mut sel, &mut t, 1, 0, 1);
    let ms = Duration::from_millis;
    let top = t.buffers().active().ydisp;
    sel.apply(
        &mut t,
        SelectCmd::Drag {
            cell: (0, 5),
            top,
            scroll: -1,
        },
        ms(0),
    );
    assert!(!sel.drag_due(ms(49)));
    assert_eq!(sel.drag_deadline(), Some(ms(50)));
    assert!(sel.drag_due(ms(50)));
    let y0 = t.buffers().active().ydisp;
    sel.drag_scroll(&mut t, ms(50));
    let y1 = t.buffers().active().ydisp;
    assert_eq!(y1 + 1, y0, "scrolled one line up");
    assert_eq!(
        sel.model.selection_end,
        Some([0, i64::try_from(y1).expect("a line number")])
    );
    assert_eq!(sel.drag_deadline(), Some(ms(100)), "and again in 50 ms");
    // Back inside the viewport: the timer stops; a release stops it too.
    sel.apply(
        &mut t,
        SelectCmd::Drag {
            cell: (0, 5),
            top: y1,
            scroll: 0,
        },
        ms(60),
    );
    assert_eq!(sel.drag_deadline(), None);
    sel.apply(
        &mut t,
        SelectCmd::Drag {
            cell: (0, 5),
            top: y1,
            scroll: 3,
        },
        ms(70),
    );
    assert!(sel.drag_deadline().is_some());
    sel.apply(&mut t, SelectCmd::End, ms(80));
    assert_eq!(sel.drag_deadline(), None);
}

fn sv(start: (usize, usize), end: (usize, usize), column: bool) -> SelectionView {
    SelectionView { start, end, column }
}

#[test]
fn the_pointer_selects_the_nearest_cell_edge() {
    // 10 px cells: the left half of a cell is its left edge, the right half the next one.
    assert_eq!(edge_of(0.0, 10.0, 80), 0);
    assert_eq!(edge_of(4.9, 10.0, 80), 0);
    assert_eq!(edge_of(5.0, 10.0, 80), 0);
    assert_eq!(edge_of(5.1, 10.0, 80), 1);
    assert_eq!(edge_of(14.9, 10.0, 80), 1);
    assert_eq!(edge_of(15.1, 10.0, 80), 2);
    // The right end of the last cell is the grid's width; beyond it and before it clamp.
    assert_eq!(edge_of(799.0, 10.0, 80), 80);
    assert_eq!(edge_of(5000.0, 10.0, 80), 80);
    assert_eq!(edge_of(-30.0, 10.0, 80), 0);
    assert_eq!(edge_of(f32::NAN, 10.0, 80), 0);
    assert_eq!(edge_of(5.1, 0.0, 80), 0);
}

#[test]
fn a_selection_maps_to_the_viewport_rows() {
    use lune_termview::SelectionKind::{Block, Linear};
    // Buffer lines 10..30 are in view (top 10, 20 rows, 80 columns).
    let at = |v: SelectionView| view_range(&v, 10, 20, 80).map(|r| (r.start, r.end, r.kind));
    // Inside: the end column is exclusive, so the inclusive end is one before it.
    assert_eq!(
        at(sv((3, 12), (8, 14), false)),
        Some(((2, 3), (4, 7), Linear))
    );
    // One row.
    assert_eq!(
        at(sv((3, 12), (8, 12), false)),
        Some(((2, 3), (2, 7), Linear))
    );
    // An end at column 0 ends the row before, at its last cell.
    assert_eq!(
        at(sv((3, 12), (0, 14), false)),
        Some(((2, 3), (3, 79), Linear))
    );
    // The whole line (an end at the width).
    assert_eq!(
        at(sv((0, 12), (80, 12), false)),
        Some(((2, 0), (2, 79), Linear))
    );
    // Starting above the view: from the first cell; ending below it: to the last.
    assert_eq!(
        at(sv((5, 2), (4, 12), false)),
        Some(((0, 0), (2, 3), Linear))
    );
    assert_eq!(
        at(sv((5, 25), (4, 40), false)),
        Some(((15, 5), (19, 79), Linear))
    );
    // Wholly out of view, either side.
    assert_eq!(at(sv((5, 2), (4, 9), false)), None);
    assert_eq!(at(sv((5, 30), (4, 40), false)), None);
    // Ending at the top of the first visible line's column 0: only lines above: nothing.
    assert_eq!(at(sv((5, 2), (0, 10), false)), None);
    // A block takes its columns from both corners, either order.
    assert_eq!(
        at(sv((9, 12), (4, 15), true)),
        Some(((2, 4), (5, 8), Block))
    );
    assert_eq!(at(sv((4, 12), (4, 15), true)), None, "zero width");
}
