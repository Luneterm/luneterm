//! Tests for `headless/terminal.rs`: upstream `headless/public/Terminal.test.ts`, then
//! `input/WriteBuffer.test.ts` per plan M6 D3, then the viewport, modes and Lunegit `IVtEngine`
//! mapping written from the plan's D9.
//!
//! Terminal.test.ts:
//!  - adapted: the callback variants (`write with callback`, `write - bytes (UTF8) with
//!    callback`, `writeln with callback`) — `write` is synchronous (D3), so they are the plain
//!    cases; `writeln` is `write(data)` + `write("\r\n")` (not part of the Rust API).
//!  - adapted: `options` — `lineHeight`, `cursorWidth`, `fontSize`, `fontFamily` are renderer
//!    options the core does not hold; the same get/set checks use core options.
//!  - adapted: `Events` read the queue and the damage (`onCursorMove` -> `cursor_moved`,
//!    `onScroll` -> `scrolled` plus `ydisp`, `onResize` -> `full`).
//!  - adapted: `buffer` reads the buffers directly (`getLine(-1)` has no `usize` counterpart).
//!  - replaced: `Proposed API check` — nothing is gated on `allowProposedApi` (module notes).
//!  - replaced: `loadAddon` x3 — no JS addons (D5); `add_extension_handlers_run_first` covers
//!    the replacement.
//!  - replaced: `onLineFeed` — no public consumer (D2).
//!  - replaced: `dispose` — `Drop`.
//!  - ignored until their handlers land: `onData` (DSR, step 7), `onTitleChange` (OSC 2, step
//!    8), `active, normal, alternate` and the mode cases (DECSET/SM, step 7).
//!
//! WriteBuffer.test.ts (D3): `string`, `bytes`, `string/bytes mixed` and `write callback works
//! for empty chunks` are `Terminal::write` cases below; `writeSync`, `writeSync called from
//! action does not overflow callstack` and `writeSync maxSubsequentCalls argument` are
//! replaced: no async scheduling (there is no queue to flush or recurse into).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::buffer::attribute_data::AttributeData;
use crate::buffer::constants::UnderlineStyle as RawUnderline;
use crate::engine::CursorStyle;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::services::osc_link_service::OscLinkData;

fn term(options: impl IntoIterator<Item = OptionValue>) -> Terminal {
    Terminal::new(options)
}

fn sized(cols: usize, rows: usize) -> Terminal {
    term([OptionValue::Cols(cols), OptionValue::Rows(rows)])
}

/// `lineEquals`: the trimmed text of line `index` of the active buffer.
fn line(t: &Terminal, index: usize) -> String {
    t.buffers()
        .active()
        .translate_buffer_line_to_string(index, true, 0, None)
}

fn writeln(t: &mut Terminal, data: &[u8]) {
    t.write(data);
    t.write(b"\r\n");
}

fn cursor(t: &Terminal) -> (usize, usize) {
    let buffer = t.buffers().active();
    (buffer.x, buffer.y)
}

// --- Terminal.test.ts -----------------------------------------------------------------------

#[test]
fn default_options() {
    let t = term([]);
    assert_eq!((t.cols(), t.rows()), (80, 24));
}

#[test]
fn write() {
    let mut t = term([]);
    t.write_str("foo");
    t.write_str("bar");
    t.write_str("文");
    assert_eq!(line(&t, 0), "foobar文");
}

#[test]
fn write_bytes_utf8() {
    let mut t = term([]);
    t.write(&[102, 111, 111]); // foo
    t.write(&[98, 97, 114]); // bar
    t.write(&[230, 150, 135]); // 文
    assert_eq!(line(&t, 0), "foobar文");
}

#[test]
fn writeln_() {
    for bytes in [false, true] {
        let mut t = term([]);
        for text in ["foo", "bar", "文"] {
            if bytes {
                writeln(&mut t, text.as_bytes());
            } else {
                t.write_str(text);
                t.write_str("\r\n");
            }
        }
        assert_eq!(line(&t, 0), "foo");
        assert_eq!(line(&t, 1), "bar");
        assert_eq!(line(&t, 2), "文");
    }
}

#[test]
fn clear() {
    let mut t = term([OptionValue::Rows(5)]);
    for i in 0..10 {
        t.write_str(&format!("\n\rtest{i}"));
    }
    t.take_damage();
    t.clear();
    assert_eq!(t.buffers().active().lines.len(), 5);
    assert_eq!(line(&t, 0), "test9");
    for i in 1..5 {
        assert_eq!(line(&t, i), "");
    }
    // onScroll
    assert!(t.take_damage().scrolled);
    let buffer = t.buffers().active();
    assert_eq!((buffer.ybase, buffer.ydisp, buffer.y), (0, 0, 0));

    // already clear: nothing happens
    t.clear();
    assert!(t.take_damage().is_empty());
}

#[test]
fn options_get_and_set() {
    let mut t = sized(80, 24);
    // get options (adapted: core options)
    assert_eq!(t.options().scrollback, 1000);
    assert_eq!(t.options().tab_stop_width, 8);
    // set options
    t.set_option(OptionValue::Scrollback(1)).unwrap();
    assert_eq!(t.options().scrollback, 1);
    t.set_option(OptionValue::CursorBlink(true)).unwrap();
    t.set_option(OptionValue::TermName("xterm-256color".to_owned()))
        .unwrap();
    assert!(t.options().cursor_blink);
    assert_eq!(t.options().term_name, "xterm-256color");
}

#[test]
fn cols_and_rows_can_only_be_set_in_the_constructor() {
    let mut t = sized(80, 24);
    for value in [OptionValue::Cols(10), OptionValue::Rows(10)] {
        let key = value.key();
        let error = t.set_option(value).unwrap_err();
        assert_eq!(error, OptionError::ConstructorOnly { key });
    }
    assert_eq!(
        OptionError::ConstructorOnly {
            key: OptionKey::Cols
        }
        .to_string(),
        "Option \"cols\" can only be set in the constructor"
    );
    assert_eq!((t.options().cols, t.cols()), (80, 80));
}

#[test]
fn add_extension_handlers_run_first() {
    struct SwallowBell;
    impl Extension<InputHandler> for SwallowBell {
        fn name(&self) -> &'static str {
            "swallow-bell"
        }
        fn register(
            &self,
            parser: &mut EscapeSequenceParser<InputHandler>,
        ) -> Result<(), IdentifierError> {
            parser.register_esc_handler(FunctionIdentifier::new('D'), |_| true)?;
            Ok(())
        }
    }
    let mut t = sized(10, 3);
    t.add_extension(&SwallowBell).unwrap();
    t.write(b"\x1bD");
    assert_eq!(cursor(&t), (0, 0));
}

#[test]
fn on_cursor_move() {
    let mut t = term([]);
    t.write_str("foo");
    assert!(t.take_damage().cursor_moved);
    t.write_str("bar");
    assert!(t.take_damage().cursor_moved);
    t.write_str("\x07");
    assert!(!t.take_damage().cursor_moved);
}

#[test]
#[ignore = "needs DSR (M6 step 7)"]
fn on_data() {
    let mut t = term([]);
    t.write_str("\x1b[5n"); // DSR Status Report
    assert_eq!(t.drain_events(), [HostEvent::Response(b"\x1b[0n".to_vec())]);
}

#[test]
fn on_scroll() {
    let mut t = term([OptionValue::Rows(5)]);
    t.take_damage();
    for _ in 0..4 {
        writeln(&mut t, b"foo");
    }
    assert!(!t.take_damage().scrolled);
    writeln(&mut t, b"bar");
    assert!(t.take_damage().scrolled);
    assert_eq!(t.buffers().active().ydisp, 1);
    writeln(&mut t, b"baz");
    assert!(t.take_damage().scrolled);
    assert_eq!(t.buffers().active().ydisp, 2);
}

#[test]
fn on_resize() {
    let mut t = term([]);
    t.take_damage();
    t.resize(10, 5);
    assert!(t.take_damage().full);
    assert_eq!((t.cols(), t.rows()), (10, 5));
    t.resize(20, 15);
    assert!(t.take_damage().full);
    assert_eq!((t.cols(), t.rows()), (20, 15));
    // headless resize returns early on the same size
    t.resize(20, 15);
    assert!(t.take_damage().is_empty());
}

#[test]
#[ignore = "needs OSC 2 (M6 step 8)"]
fn on_title_change() {
    let mut t = term([]);
    t.write_str("\x1b]2;foo\u{9c}");
    assert_eq!(t.drain_events(), [HostEvent::Title("foo".to_owned())]);
}

#[test]
fn on_bell() {
    let mut t = term([]);
    assert!(t.drain_events().is_empty());
    t.write_str("\x07");
    assert_eq!(t.drain_events(), [HostEvent::Bell]);
}

#[test]
fn buffer_cursor_x_cursor_y() {
    let mut t = sized(5, 5);
    assert_eq!(cursor(&t), (0, 0));
    t.write_str("foo");
    assert_eq!(cursor(&t), (3, 0));
    t.write_str("\n");
    assert_eq!(cursor(&t), (3, 1));
    t.write_str("\r");
    assert_eq!(cursor(&t), (0, 1));
    t.write_str("abcde");
    assert_eq!(cursor(&t), (5, 1));
    t.write_str("\n\r\n\n\n\n\n");
    assert_eq!(cursor(&t), (0, 4));
}

#[test]
fn buffer_viewport_y_and_base_y() {
    let mut t = term([OptionValue::Rows(5)]);
    let pos = |t: &Terminal| (t.buffers().active().ydisp, t.buffers().active().ybase);
    assert_eq!(pos(&t), (0, 0));
    t.write_str("\n\n\n\n");
    assert_eq!(pos(&t), (0, 0));
    t.write_str("\n");
    assert_eq!(pos(&t), (1, 1));
    t.write_str("\n\n\n\n");
    assert_eq!(pos(&t), (5, 5));
    t.scroll_lines(-1);
    assert_eq!(pos(&t), (4, 5));
    t.scroll_to_top();
    assert_eq!(pos(&t), (0, 5));
}

#[test]
fn buffer_length() {
    let mut t = term([OptionValue::Rows(5)]);
    assert_eq!(t.buffers().active().lines.len(), 5);
    t.write_str("\n\n\n\n");
    assert_eq!(t.buffers().active().lines.len(), 5);
    t.write_str("\n");
    assert_eq!(t.buffers().active().lines.len(), 6);
    t.write_str("\n\n\n\n");
    assert_eq!(t.buffers().active().lines.len(), 10);
}

#[test]
fn buffer_get_line_invalid_index() {
    let t = term([OptionValue::Rows(5)]);
    assert!(t.buffers().active().lines.get(5).is_none());
}

#[test]
fn buffer_get_line_is_wrapped() {
    let mut t = term([OptionValue::Cols(5)]);
    let wrapped = |t: &Terminal, i: usize| t.buffers().active().lines.get(i).unwrap().is_wrapped;
    assert!(!wrapped(&t, 0) && !wrapped(&t, 1));
    t.write_str("abcde");
    assert!(!wrapped(&t, 0) && !wrapped(&t, 1));
    t.write_str("f");
    assert!(!wrapped(&t, 0) && wrapped(&t, 1));
}

#[test]
fn buffer_get_line_translate_to_string() {
    let mut t = term([OptionValue::Cols(5)]);
    let text = |t: &Terminal, i: usize, trim: bool, start: usize, end: Option<usize>| {
        t.buffers()
            .active()
            .translate_buffer_line_to_string(i, trim, start, end)
    };
    assert_eq!(text(&t, 0, false, 0, None), "     ");
    assert_eq!(text(&t, 0, true, 0, None), "");
    t.write_str("foo");
    assert_eq!(text(&t, 0, false, 0, None), "foo  ");
    assert_eq!(text(&t, 0, true, 0, None), "foo");
    t.write_str("bar");
    assert_eq!(text(&t, 0, false, 0, None), "fooba");
    assert_eq!(text(&t, 0, true, 0, None), "fooba");
    assert_eq!(text(&t, 1, true, 0, None), "r");
    assert_eq!(text(&t, 0, false, 1, None), "ooba");
    assert_eq!(text(&t, 0, false, 1, Some(3)), "oo");
}

#[test]
fn buffer_get_line_get_cell() {
    let mut t = term([OptionValue::Cols(5)]);
    let mut row = Vec::new();
    t.read_row(0, &mut row);
    assert_eq!((row[0].text.as_str(), row[0].width), ("", 1));
    t.write_str("a文");
    t.read_row(0, &mut row);
    assert_eq!((row[0].text.as_str(), row[0].width), ("a", 1));
    assert_eq!((row[1].text.as_str(), row[1].width), ("文", 2));
    assert_eq!((row[2].text.as_str(), row[2].width), ("", 0));
    assert_eq!(row.len(), 5);
}

#[test]
#[ignore = "needs DECSET 47 (M6 step 7)"]
fn buffer_active_normal_alternate() {
    let mut t = term([OptionValue::Cols(5)]);
    let normal = |t: &Terminal| {
        t.buffers()
            .normal()
            .translate_buffer_line_to_string(0, false, 0, None)
    };
    assert!(!t.buffers().is_alt_active());
    t.write_str("norm ");
    assert_eq!(normal(&t), "norm ");
    assert!(t.buffers().alt().lines.get(0).is_none());
    t.write_str("\x1b[?47h\r"); // use alternate screen buffer
    assert!(t.buffers().is_alt_active());
    assert_eq!(line(&t, 0), "");
    t.write_str("alt  ");
    assert_eq!(line(&t, 0), "alt");
    assert_eq!(normal(&t), "norm ");
    t.write_str("\x1b[?47l\r"); // use normal screen buffer
    assert!(!t.buffers().is_alt_active());
    assert_eq!(normal(&t), "norm ");
    assert!(t.buffers().alt().lines.get(0).is_none());
}

#[test]
fn modes_defaults() {
    let t = term([]);
    assert_eq!(
        t.modes(),
        Modes {
            application_cursor_keys: false,
            application_keypad: false,
            bracketed_paste: false,
            insert: false,
            origin: false,
            reverse_wraparound: false,
            send_focus: false,
            synchronized_output: false,
            wraparound: true,
            cursor_hidden: false,
            mouse_tracking: MouseTracking::None,
            mouse_encoding: MouseEncoding::Default,
        }
    );
}

#[test]
#[ignore = "needs SM/RM and DECSET/DECRST (M6 step 7)"]
fn modes_set_and_reset() {
    type Get = fn(&Modes) -> bool;
    let cases: [(&str, Get); 8] = [
        ("?1", |m| m.application_cursor_keys),
        ("?66", |m| m.application_keypad),
        ("?2004", |m| m.bracketed_paste),
        ("4", |m| m.insert),
        ("?6", |m| m.origin),
        ("?45", |m| m.reverse_wraparound),
        ("?1004", |m| m.send_focus),
        ("?7", |m| m.wraparound),
    ];
    let mut t = term([]);
    for (mode, get) in cases {
        t.write_str(&format!("\x1b[{mode}h"));
        assert!(get(&t.modes()), "{mode} h");
        t.write_str(&format!("\x1b[{mode}l"));
        assert!(!get(&t.modes()), "{mode} l");
    }
    for (mode, tracking) in [
        ("9", MouseTracking::X10),
        ("1000", MouseTracking::Vt200),
        ("1002", MouseTracking::Drag),
        ("1003", MouseTracking::Any),
    ] {
        t.write_str(&format!("\x1b[?{mode}h"));
        assert_eq!(t.modes().mouse_tracking, tracking);
        t.write_str(&format!("\x1b[?{mode}l"));
        assert_eq!(t.modes().mouse_tracking, MouseTracking::None);
    }
}

// --- WriteBuffer.test.ts (D3) ---------------------------------------------------------------

#[test]
fn write_input_string_bytes_and_mixed() {
    let chunks = ["a._", "b.x", "c._", "d.x", "e"];
    for mode in 0..3 {
        let mut t = term([]);
        for (i, chunk) in chunks.iter().enumerate() {
            // 0: strings, 1: bytes, 2: two strings then bytes
            if mode == 0 || (mode == 2 && i < 2) {
                t.write_str(chunk);
            } else {
                t.write(chunk.as_bytes());
            }
        }
        assert_eq!(line(&t, 0), "a._b.xc._d.xe", "mode {mode}");
    }
}

#[test]
fn write_works_for_empty_chunks() {
    let mut t = term([]);
    t.write_str("a");
    t.write_str("");
    t.write(b"c");
    t.write(&[]);
    t.write_str("e");
    assert_eq!(line(&t, 0), "ace");
}

// --- reset, add_marker, idle -----------------------------------------------------------------

#[test]
fn reset_keeps_the_size_and_options_and_clears_the_screen() {
    let mut t = term([
        OptionValue::Cols(10),
        OptionValue::Rows(4),
        OptionValue::Scrollback(7),
    ]);
    t.resize(12, 5);
    t.write_str("hello\r\nworld");
    t.reset();
    assert_eq!((t.cols(), t.rows()), (12, 5));
    assert_eq!((t.options().cols, t.options().rows), (12, 5));
    assert_eq!(t.options().scrollback, 7);
    assert_eq!(line(&t, 0), "");
    assert_eq!(cursor(&t), (0, 0));
    assert!(t.take_damage().full);
}

#[test]
fn add_marker_follows_the_cursor_line_and_is_refused_on_the_alt_buffer() {
    let mut t = sized(10, 3);
    t.write_str("\n\n");
    let marker = t.add_marker(-1).unwrap();
    assert_eq!(t.buffers().normal().marker(marker).unwrap().line, 1);
    t.core_mut()
        .input_handler_mut()
        .services
        .buffer
        .buffers
        .activate_alt_buffer(None)
        .unwrap();
    assert!(t.add_marker(0).is_none());
}

#[test]
fn idle_finishes_the_memory_cleanup_after_a_column_shrink() {
    let mut t = term([
        OptionValue::Cols(80),
        OptionValue::Rows(10),
        OptionValue::Scrollback(500),
    ]);
    for i in 0..400 {
        t.write_str(&format!("line {i}\r\n"));
    }
    assert!(!t.idle(), "nothing pending before the resize");
    t.resize(20, 10);
    assert!(t.buffers().normal().memory_cleanup_pending());
    let mut calls = 0;
    while t.idle() {
        calls += 1;
        assert!(calls < 1000, "idle loop does not terminate");
    }
    assert!(calls > 0);
    assert!(!t.buffers().normal().memory_cleanup_pending());
    // everything was freed: a further cleanup finds nothing
    let normal = t
        .core_mut()
        .input_handler_mut()
        .services
        .buffer
        .buffers
        .normal_mut();
    for i in 0..normal.lines.len() {
        assert_eq!(
            normal.lines.get_mut(i).unwrap().cleanup_memory(),
            0,
            "line {i}"
        );
    }
}

// --- viewport and read_row ----------------------------------------------------------------

/// Writes `cell` (code point, width, attributes) into the active buffer at `(x, y)`.
fn put(t: &mut Terminal, x: usize, y: usize, code: u32, attr: &AttributeData) {
    let buffer = t
        .core_mut()
        .input_handler_mut()
        .services
        .buffer
        .buffer_mut();
    buffer
        .lines
        .get_mut(y)
        .unwrap()
        .set_cell_from_codepoint(x, code, 1, attr);
}

fn attr(fg: u32, bg: u32) -> AttributeData {
    AttributeData {
        fg,
        bg,
        ..DEFAULT_ATTR_DATA
    }
}

#[test]
fn viewport_cells_carry_colours_and_flags() {
    let mut t = sized(6, 2);
    put(
        &mut t,
        0,
        0,
        'a' as u32,
        &attr(Attributes::CM_P16 | 3, Attributes::CM_P256 | 200),
    );
    put(
        &mut t,
        1,
        0,
        'b' as u32,
        &attr(
            Attributes::CM_RGB | 0x11_2233,
            Attributes::CM_RGB | 0xA0_B0C0,
        ),
    );
    let flags = FgFlags::BOLD
        | FgFlags::BLINK
        | FgFlags::INVERSE
        | FgFlags::INVISIBLE
        | FgFlags::STRIKETHROUGH;
    put(
        &mut t,
        2,
        0,
        'c' as u32,
        &attr(flags, BgFlags::DIM | BgFlags::ITALIC | BgFlags::OVERLINE),
    );
    let v = t.viewport();
    assert_eq!(
        (v.cols, v.rows, v.cells.len(), v.cells[0].len()),
        (6, 2, 2, 6)
    );
    let a = &v.cells[0][0];
    assert_eq!(
        (a.fg, a.bg),
        (CellColor::Palette(3), CellColor::Palette(200))
    );
    let b = &v.cells[0][1];
    assert_eq!(
        (b.fg, b.bg),
        (
            CellColor::Rgb(0x11, 0x22, 0x33),
            CellColor::Rgb(0xA0, 0xB0, 0xC0)
        )
    );
    let c = &v.cells[0][2];
    assert!(c.bold && c.blink && c.inverse && c.invisible && c.strikethrough);
    assert!(c.dim && c.italic && c.overline);
    assert_eq!((c.fg, c.bg), (CellColor::Default, CellColor::Default));
    assert_eq!(c.underline, UnderlineStyle::None);
    // a null cell
    let empty = &v.cells[1][5];
    assert_eq!(*empty, blank_cell());
}

#[test]
fn viewport_underline_style_colour_and_links() {
    let mut t = sized(6, 2);
    let link_id = {
        let services = &mut t.core_mut().input_handler_mut().services;
        services.osc_link.register_link(
            &mut services.buffer,
            OscLinkData {
                id: None,
                uri: "https://example.com".to_owned(),
            },
        )
    };
    let extended = |style: u32, url: u32, colour: u32| {
        let mut a = attr(FgFlags::UNDERLINE, BgFlags::HAS_EXTENDED);
        a.extended.set_underline_style(style);
        a.extended.set_url_id(url);
        a.extended.set_underline_color(colour);
        a
    };
    // SGR 4 without extended attributes: single
    put(&mut t, 0, 0, 'a' as u32, &attr(FgFlags::UNDERLINE, 0));
    // curly with an RGB underline colour
    put(
        &mut t,
        1,
        0,
        'b' as u32,
        &extended(RawUnderline::CURLY, 0, Attributes::CM_RGB | 0xFF_0000),
    );
    // dotted and linked: the link does not change the style
    put(
        &mut t,
        2,
        0,
        'c' as u32,
        &extended(RawUnderline::DOTTED, link_id, Attributes::CM_P256 | 9),
    );
    // linked without SGR 4: no underline, link kept
    let mut linked = attr(0, BgFlags::HAS_EXTENDED);
    linked.extended.set_url_id(link_id);
    put(&mut t, 3, 0, 'd' as u32, &linked);

    let v = t.viewport();
    let row = &v.cells[0];
    assert_eq!(
        (row[0].underline, row[0].underline_color),
        (UnderlineStyle::Single, CellColor::Default)
    );
    assert_eq!(
        (row[1].underline, row[1].underline_color, row[1].link_id),
        (UnderlineStyle::Curly, CellColor::Rgb(255, 0, 0), 0)
    );
    assert_eq!(
        (row[2].underline, row[2].underline_color, row[2].link_id),
        (UnderlineStyle::Dotted, CellColor::Palette(9), link_id)
    );
    assert_eq!(
        (row[3].underline, row[3].link_id),
        (UnderlineStyle::None, link_id)
    );
    assert_eq!(
        v.links.into_iter().collect::<Vec<_>>(),
        [(link_id, "https://example.com".to_owned())]
    );
    assert_eq!(t.link_uri(link_id), Some("https://example.com"));
    assert_eq!(t.link_uri(link_id + 1), None);
}

#[test]
fn viewport_rows_follow_ydisp_and_read_row_matches() {
    let mut t = term([OptionValue::Cols(4), OptionValue::Rows(2)]);
    t.write_str("l0\r\nl1\r\nl2\r\nl3");
    t.scroll_lines(-1);
    let v = t.viewport();
    assert_eq!((v.viewport_top, v.base_y, v.total_lines), (1, 2, 4));
    assert!(!v.is_at_bottom);
    assert_eq!(v.cells[0][0].text, "l");
    assert_eq!(v.cells[0][1].text, "1");
    assert_eq!(v.cells[1][1].text, "2");
    let mut row = Vec::new();
    for y in 0..2 {
        assert!(t.read_row(y, &mut row));
        assert_eq!(row, v.cells[y]);
    }
    row = vec![blank_cell()];
    assert!(!t.read_row(2, &mut row));
    assert_eq!(row, [blank_cell()], "untouched outside the viewport");
    t.scroll_to_bottom();
    assert!(t.viewport().is_at_bottom);
}

#[test]
fn a_missing_line_renders_blank() {
    let mut t = sized(3, 2);
    t.write_str("abc");
    t.core_mut()
        .input_handler_mut()
        .services
        .buffer
        .buffer_mut()
        .lines
        .set(0, None);
    let v = t.viewport();
    assert_eq!(v.cells[0], vec![blank_cell(); 3]);
}

#[test]
fn viewport_cursor_visibility_style_and_blink() {
    let mut t = term([OptionValue::Cols(5), OptionValue::Rows(2)]);
    t.write_str("ab");
    let c = t.viewport().cursor;
    assert_eq!((c.x, c.y, c.visible), (2, 0, true));
    assert_eq!((c.style, c.blink), (CursorStyle::Block, false));

    t.set_option(OptionValue::CursorStyle(CursorStyle::Bar))
        .unwrap();
    t.set_option(OptionValue::CursorBlink(true)).unwrap();
    let c = t.viewport().cursor;
    assert_eq!((c.style, c.blink), (CursorStyle::Bar, true));
    {
        let dec = &mut t
            .core_mut()
            .input_handler_mut()
            .services
            .core
            .dec_private_modes;
        dec.cursor_style = Some(CursorStyle::Underline);
        dec.cursor_blink = Some(false);
    }
    let c = t.viewport().cursor;
    assert_eq!((c.style, c.blink), (CursorStyle::Underline, false));

    // hidden
    t.core_mut()
        .input_handler_mut()
        .services
        .core
        .is_cursor_hidden = true;
    assert!(!t.viewport().cursor.visible);
    assert!(t.modes().cursor_hidden);
    t.core_mut()
        .input_handler_mut()
        .services
        .core
        .is_cursor_hidden = false;

    // scrolled out of the viewport
    t.write_str("\r\n\r\n\r\nxyz");
    assert_eq!(t.viewport().cursor.y, 1);
    t.scroll_to_top();
    let c = t.viewport().cursor;
    assert!(!c.visible);
    assert_eq!(c.y, 0);

    // pending wrap: x = cols
    t.scroll_to_bottom();
    t.write_str("de");
    assert_eq!(t.viewport().cursor.x, 5);
    assert_eq!(t.viewport().title, "");
    assert!(!t.viewport().alt_screen);
}

#[test]
fn modes_report_the_mouse_protocol_and_encoding() {
    let mut t = term([]);
    let services = &mut t.core_mut().input_handler_mut().services;
    services.mouse.set_active_protocol("DRAG").unwrap();
    services.mouse.set_active_encoding("SGR_PIXELS").unwrap();
    let m = t.modes();
    assert_eq!(
        (m.mouse_tracking, m.mouse_encoding),
        (MouseTracking::Drag, MouseEncoding::SgrPixels)
    );
    let services = &mut t.core_mut().input_handler_mut().services;
    services.mouse.set_active_encoding("SGR").unwrap();
    assert_eq!(t.modes().mouse_encoding, MouseEncoding::Sgr);
}

// --- HostEvent ---------------------------------------------------------------------------------

#[test]
fn osc_52_produces_no_event_and_no_response() {
    // ledger row 13: the core never answers or forwards clipboard requests
    let mut t = term([]);
    t.write(b"\x1b]52;c;?\x07");
    t.write(b"\x1b]52;c;aGk=\x07");
    assert!(t.drain_events().is_empty());
}

#[test]
fn events_keep_fire_order_and_responses_are_not_merged() {
    let mut t = term([OptionValue::ScreenReaderMode(true)]);
    t.write(b"a\x07\tb");
    t.input("x", true);
    t.input("y", true);
    assert_eq!(
        t.drain_events(),
        [
            HostEvent::A11yChar("a".to_owned()),
            HostEvent::Bell,
            HostEvent::A11yTab(7),
            HostEvent::A11yChar("b".to_owned()),
            HostEvent::Response(b"x".to_vec()),
            HostEvent::Response(b"y".to_vec()),
        ]
    );
    assert!(t.drain_events().is_empty());
}

// --- Lunegit IVtEngine mapping (one test per member with an M6 equivalent) -------------------

fn engine(cols: u16, rows: u16) -> Terminal {
    sized(usize::from(cols), usize::from(rows))
}

#[test]
fn lunegit_columns_and_rows() {
    let t = engine(30, 7);
    assert_eq!((VtEngine::columns(&t), VtEngine::rows(&t)), (30, 7));
}

#[test]
fn lunegit_bracketed_paste_mode() {
    let mut t = engine(10, 3);
    assert!(!VtEngine::modes(&t).bracketed_paste);
    t.core_mut()
        .input_handler_mut()
        .services
        .core
        .dec_private_modes
        .bracketed_paste_mode = true;
    assert!(VtEngine::modes(&t).bracketed_paste);
}

#[test]
fn lunegit_is_mouse_tracking_enabled() {
    let mut t = engine(10, 3);
    assert_eq!(VtEngine::modes(&t).mouse_tracking, MouseTracking::None);
    let services = &mut t.core_mut().input_handler_mut().services;
    services.mouse.set_active_protocol("VT200").unwrap();
    assert_ne!(VtEngine::modes(&t).mouse_tracking, MouseTracking::None);
}

#[test]
fn lunegit_host_response_generated() {
    let mut t = engine(10, 3);
    t.input("\x1b[I", false);
    assert_eq!(
        VtEngine::drain_events(&mut t),
        [HostEvent::Response(b"\x1b[I".to_vec())]
    );
}

#[test]
fn lunegit_clipboard_osc_received_is_never_produced_by_the_core() {
    let mut t = engine(10, 3);
    VtEngine::write(&mut t, b"\x1b]52;c;aGk=\x07");
    assert!(VtEngine::drain_events(&mut t).is_empty());
}

#[test]
fn lunegit_write() {
    let mut t = engine(10, 3);
    VtEngine::write(&mut t, "h\u{e9}".as_bytes().get(..2).unwrap());
    VtEngine::write(&mut t, "h\u{e9}".as_bytes().get(2..).unwrap());
    assert_eq!(line(&t, 0), "hé");
}

#[test]
fn lunegit_resize() {
    let mut t = engine(10, 3);
    VtEngine::resize(&mut t, 0, 0);
    assert_eq!((VtEngine::columns(&t), VtEngine::rows(&t)), (2, 1));
    VtEngine::resize(&mut t, 40, 12);
    assert_eq!((t.cols(), t.rows()), (40, 12));
    assert!(VtEngine::take_damage(&mut t).full);
}

#[test]
fn lunegit_scroll_lines_scroll_to_top_scroll_to_bottom() {
    let mut t = engine(10, 3);
    VtEngine::write(&mut t, &[b'\n'; 10]);
    assert_eq!(t.buffers().active().ydisp, 8);
    VtEngine::scroll_lines(&mut t, -2);
    assert_eq!(t.buffers().active().ydisp, 6);
    VtEngine::scroll_to_top(&mut t);
    assert_eq!(t.buffers().active().ydisp, 0);
    VtEngine::scroll_to_bottom(&mut t);
    assert_eq!(t.buffers().active().ydisp, 8);
    assert!(VtEngine::take_damage(&mut t).scrolled);
}

#[test]
fn lunegit_read_viewport() {
    let mut t = engine(4, 2);
    VtEngine::write(&mut t, "ab文".as_bytes());
    let v = VtEngine::viewport(&t);
    let texts: Vec<_> = v.cells[0].iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["a", "b", "文", ""]);
    assert_eq!((v.cursor.x, v.cursor.y, v.cursor.visible), (4, 0, true));
    assert_eq!(v, t.viewport());
}

#[test]
fn engine_is_usable_as_a_trait_object() {
    let calls = Rc::new(RefCell::new(0));
    let mut t = engine(10, 3);
    let sink = Rc::clone(&calls);
    t.register_osc_handler(7, move |_, _| {
        *sink.borrow_mut() += 1;
        true
    });
    let mut boxed: Box<dyn VtEngine> = Box::new(t);
    boxed.write(b"\x1b]7;file:///tmp\x07");
    assert_eq!(*calls.borrow(), 1);
}
