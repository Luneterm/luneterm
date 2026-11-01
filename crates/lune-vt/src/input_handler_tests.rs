//! Tests for `input_handler.rs`: the cases of upstream `InputHandler.test.ts` whose handlers are
//! ported (plan M6 step 2; the full describe -> step map is the `ih-tests:` table in the task's
//! `notes.md`), then cases written from `InputHandler.ts`.
//!
//! Upstream builds the handler with mocks for the charset, mouse and Unicode services
//! (`TestUtils.test.ts`); these tests use the real services with V6 active, which is what
//! `MockUnicodeService` computes. Upstream's `beforeEach` (a `BufferService` over the default
//! options, resized to 80 x 30) is [`upstream_handler`]. Every case parses through a parser
//! registered by [`InputHandler::register_handlers`], the function `CoreTerminal` calls.
//!
//! adapted:
//!  - `should clamp cursor into addressible range`: the buffer's cursor is `usize`, so the
//!    halves that set `x = y = -10000` set them to 0, which is what upstream's restriction makes
//!    of -10000 before every handler reads them (`_restrictCursor` / `_moveCursor`).
//!  - `should parse big chunks in smaller subchunks` replaces `_parser.parse` with a recorder;
//!    here the print handler records the length of each run it receives (one run per parser
//!    call for printable input). The parse buffer's capacity is not observable.
//!  - `print`/`should not cause an infinite loop` calls `print` with the one-element slice
//!    (upstream passes a 10-element array with `start = 0, end = 1`).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::buffer::constants::Attributes;
use crate::data::charsets::charset;
use crate::input::unicode_v6::UnicodeV6;
use crate::services::options_service::{OptionValue, WindowOptions};
use crate::services::unicode_service::UnicodeVersionProvider;

struct TestTerm {
    parser: EscapeSequenceParser<InputHandler>,
    handler: InputHandler,
}

fn test_handler(
    cols: usize,
    rows: usize,
    options: impl IntoIterator<Item = OptionValue>,
) -> TestTerm {
    let mut values = vec![OptionValue::Cols(cols), OptionValue::Rows(rows)];
    values.extend(options);
    let handler = InputHandler::new(CoreServices::new(OptionsService::new(values)));
    let mut parser = EscapeSequenceParser::new();
    InputHandler::register_handlers(&mut parser).unwrap();
    TestTerm { parser, handler }
}

/// Upstream's `beforeEach`: a buffer service over the default options, resized to 80 x 30.
fn upstream_handler() -> TestTerm {
    let mut t = test_handler(80, 24, []);
    t.resize(80, 30);
    t
}

impl TestTerm {
    fn parse_str(&mut self, data: &str) {
        self.handler.parse(&mut self.parser, data.as_bytes());
    }

    fn parse_bytes(&mut self, data: &[u8]) {
        self.handler.parse(&mut self.parser, data);
    }

    fn buffer(&self) -> &Buffer {
        self.handler.services.buffer.buffer()
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        self.handler.services.buffer.buffer_mut()
    }

    /// Upstream `getCursor`.
    fn cursor(&self) -> [usize; 2] {
        [self.buffer().x, self.buffer().y]
    }

    fn set_cursor(&mut self, x: usize, y: usize) {
        let buffer = self.buffer_mut();
        buffer.x = x;
        buffer.y = y;
    }

    /// Upstream `getLines(bufferService, limit)`.
    fn lines(&self, limit: usize) -> Vec<String> {
        (0..limit)
            .filter_map(|i| self.buffer().lines.get(i))
            .map(|line| line.translate_to_string(true, None, None, None))
            .collect()
    }

    /// Upstream `getLines(bufferService)` (limit = rows).
    fn screen(&self) -> Vec<String> {
        self.lines(self.handler.services.buffer.rows)
    }

    /// The rows from `ybase` (the viewport at the bottom), right-trimmed.
    fn viewport(&self) -> Vec<String> {
        let buffer = self.buffer();
        (buffer.ybase..buffer.ybase + self.handler.services.buffer.rows)
            .filter_map(|i| buffer.lines.get(i))
            .map(|line| line.translate_to_string(true, None, None, None))
            .collect()
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        let services = &mut self.handler.services;
        services
            .buffer
            .resize(cols, rows, services.options.options());
    }

    fn set_option(&mut self, value: OptionValue) {
        self.handler.services.options.set(value).unwrap();
    }

    /// `(chars, width, fg colour, bg colour)` of a cell, colours without flag bits.
    fn cell(&self, row: usize, x: usize) -> (String, u32, u32, u32) {
        let line = self.buffer().lines.get(row).unwrap();
        let colour = Attributes::CM_MASK | Attributes::RGB_MASK;
        (
            line.get_string(x),
            line.get_width(x),
            line.get_fg(x) & colour,
            line.get_bg(x) & colour,
        )
    }

    /// Every line of the buffer with its wrapped flag, the cursor and the scroll position.
    fn snapshot(&self) -> (Vec<(String, bool)>, [usize; 4]) {
        let buffer = self.buffer();
        let lines = (0..buffer.lines.len())
            .filter_map(|i| buffer.lines.get(i))
            .map(|line| {
                (
                    line.translate_to_string(false, None, None, None),
                    line.is_wrapped,
                )
            })
            .collect();
        (lines, [buffer.x, buffer.y, buffer.ybase, buffer.ydisp])
    }
}

/// A Unicode provider over V6 where VS16 (U+FE0F) joins a width-1 character and widens it to
/// 2, so `print` takes its "combining character widens 1 column to 2" wrap path. The same
/// provider is registered in the oracle runs that produced the expected cells below.
struct Vs16Widens(UnicodeV6);

impl UnicodeVersionProvider for Vs16Widens {
    fn version(&self) -> &str {
        "vs16-test"
    }

    fn wcwidth(&self, codepoint: u32) -> u32 {
        if codepoint == 0xFE0F {
            0
        } else {
            self.0.wcwidth(codepoint)
        }
    }

    fn char_properties(&self, codepoint: u32, preceding: u32) -> u32 {
        let mut width = self.wcwidth(codepoint);
        let mut should_join = width == 0 && preceding != 0;
        if should_join {
            let old_width = UnicodeService::extract_width(preceding);
            if old_width == 0 {
                should_join = false;
            } else if codepoint == 0xFE0F && old_width == 1 {
                width = 2;
            } else if old_width > width {
                width = old_width;
            }
        }
        UnicodeService::create_property_value(0, width, should_join)
    }
}

fn with_vs16(t: &mut TestTerm) {
    let unicode = &mut t.handler.services.unicode;
    unicode.register(Box::new(Vs16Widens(UnicodeV6::new())));
    unicode.set_active_version("vs16-test").unwrap();
}

const RED: u32 = Attributes::CM_P16 | 1;
const BLUE: u32 = Attributes::CM_P16 | 4;

// --- upstream: InputHandler.test.ts ---

#[test]
fn save_and_restore_cursor() {
    let mut t = upstream_handler();
    t.buffer_mut().x = 1;
    t.buffer_mut().y = 2;
    t.buffer_mut().ybase = 0;
    t.handler.cur_attr_data.fg = 3;
    // Save cursor position
    t.handler.save_cursor();
    assert_eq!(t.buffer().x, 1);
    assert_eq!(t.buffer().y, 2);
    assert_eq!(t.handler.cur_attr_data.fg, 3);
    // Change cursor position
    t.buffer_mut().x = 10;
    t.buffer_mut().y = 20;
    t.handler.cur_attr_data.fg = 30;
    // Restore cursor position
    t.handler.restore_cursor();
    assert_eq!(t.buffer().x, 1);
    assert_eq!(t.buffer().y, 2);
    assert_eq!(t.handler.cur_attr_data.fg, 3);
}

#[test]
fn print_should_not_cause_an_infinite_loop_regression_test() {
    let mut t = test_handler(80, 30, []);
    t.handler.print(&[0x200B]);
}

#[test]
fn print_should_clear_cells_to_the_right_on_early_wrap_around() {
    let mut t = upstream_handler();
    t.resize(5, 5);
    t.set_option(OptionValue::Scrollback(1));
    t.parse_str("12345");
    t.buffer_mut().x = 0;
    t.parse_str("￥￥￥");
    assert_eq!(t.lines(2), ["￥￥", "￥"]);
}

/// Upstream `describe('cursor positioning')`: `beforeEach` resizes to 10 x 10.
fn cursor_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(10, 10);
    t
}

#[test]
fn cursor_positioning_cursor_forward_cuf() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[C");
    assert_eq!(t.cursor(), [1, 0]);
    t.parse_str("\x1b[1C");
    assert_eq!(t.cursor(), [2, 0]);
    t.parse_str("\x1b[4C");
    assert_eq!(t.cursor(), [6, 0]);
    t.parse_str("\x1b[100C");
    assert_eq!(t.cursor(), [9, 0]);
    // should not change y
    t.set_cursor(8, 4);
    t.parse_str("\x1b[C");
    assert_eq!(t.cursor(), [9, 4]);
}

#[test]
fn cursor_positioning_cursor_backward_cub() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[D");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[1D");
    assert_eq!(t.cursor(), [0, 0]);
    // place cursor at end of first line
    t.parse_str("\x1b[100C");
    t.parse_str("\x1b[D");
    assert_eq!(t.cursor(), [8, 0]);
    t.parse_str("\x1b[1D");
    assert_eq!(t.cursor(), [7, 0]);
    t.parse_str("\x1b[4D");
    assert_eq!(t.cursor(), [3, 0]);
    t.parse_str("\x1b[100D");
    assert_eq!(t.cursor(), [0, 0]);
    // should not change y
    t.set_cursor(4, 4);
    t.parse_str("\x1b[D");
    assert_eq!(t.cursor(), [3, 4]);
}

#[test]
fn cursor_positioning_cursor_down_cud() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[B");
    assert_eq!(t.cursor(), [0, 1]);
    t.parse_str("\x1b[1B");
    assert_eq!(t.cursor(), [0, 2]);
    t.parse_str("\x1b[4B");
    assert_eq!(t.cursor(), [0, 6]);
    t.parse_str("\x1b[100B");
    assert_eq!(t.cursor(), [0, 9]);
    // should not change x
    t.set_cursor(8, 0);
    t.parse_str("\x1b[B");
    assert_eq!(t.cursor(), [8, 1]);
}

#[test]
fn cursor_positioning_cursor_up_cuu() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[A");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[1A");
    assert_eq!(t.cursor(), [0, 0]);
    // place cursor at beginning of last row
    t.parse_str("\x1b[100B");
    t.parse_str("\x1b[A");
    assert_eq!(t.cursor(), [0, 8]);
    t.parse_str("\x1b[1A");
    assert_eq!(t.cursor(), [0, 7]);
    t.parse_str("\x1b[4A");
    assert_eq!(t.cursor(), [0, 3]);
    t.parse_str("\x1b[100A");
    assert_eq!(t.cursor(), [0, 0]);
    // should not change x
    t.set_cursor(8, 9);
    t.parse_str("\x1b[A");
    assert_eq!(t.cursor(), [8, 8]);
}

#[test]
fn cursor_positioning_cursor_next_line_cnl() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[E");
    assert_eq!(t.cursor(), [0, 1]);
    t.parse_str("\x1b[1E");
    assert_eq!(t.cursor(), [0, 2]);
    t.parse_str("\x1b[4E");
    assert_eq!(t.cursor(), [0, 6]);
    t.parse_str("\x1b[100E");
    assert_eq!(t.cursor(), [0, 9]);
    // should reset x to zero
    t.set_cursor(8, 0);
    t.parse_str("\x1b[E");
    assert_eq!(t.cursor(), [0, 1]);
}

#[test]
fn cursor_positioning_cursor_previous_line_cpl() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[F");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[1F");
    assert_eq!(t.cursor(), [0, 0]);
    // place cursor at beginning of last row
    t.parse_str("\x1b[100E");
    t.parse_str("\x1b[F");
    assert_eq!(t.cursor(), [0, 8]);
    t.parse_str("\x1b[1F");
    assert_eq!(t.cursor(), [0, 7]);
    t.parse_str("\x1b[4F");
    assert_eq!(t.cursor(), [0, 3]);
    t.parse_str("\x1b[100F");
    assert_eq!(t.cursor(), [0, 0]);
    // should reset x to zero
    t.set_cursor(8, 9);
    t.parse_str("\x1b[F");
    assert_eq!(t.cursor(), [0, 8]);
}

#[test]
fn cursor_positioning_cursor_character_absolute_cha() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[G");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[1G");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[2G");
    assert_eq!(t.cursor(), [1, 0]);
    t.parse_str("\x1b[5G");
    assert_eq!(t.cursor(), [4, 0]);
    t.parse_str("\x1b[100G");
    assert_eq!(t.cursor(), [9, 0]);
}

#[test]
fn cursor_positioning_cursor_position_cup() {
    let mut t = cursor_handler();
    for (input, expected) in [
        ("\x1b[H", [0, 0]),
        ("\x1b[1H", [0, 0]),
        ("\x1b[1;1H", [0, 0]),
        ("\x1b[8H", [0, 7]),
        ("\x1b[;8H", [7, 0]),
        ("\x1b[100;100H", [9, 9]),
    ] {
        t.set_cursor(5, 5);
        t.parse_str(input);
        assert_eq!(t.cursor(), expected, "{input:?}");
    }
}

#[test]
fn cursor_positioning_horizontal_position_absolute_hpa() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[`");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[1`");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[2`");
    assert_eq!(t.cursor(), [1, 0]);
    t.parse_str("\x1b[5`");
    assert_eq!(t.cursor(), [4, 0]);
    t.parse_str("\x1b[100`");
    assert_eq!(t.cursor(), [9, 0]);
}

#[test]
fn cursor_positioning_horizontal_position_relative_hpr() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[a");
    assert_eq!(t.cursor(), [1, 0]);
    t.parse_str("\x1b[1a");
    assert_eq!(t.cursor(), [2, 0]);
    t.parse_str("\x1b[4a");
    assert_eq!(t.cursor(), [6, 0]);
    t.parse_str("\x1b[100a");
    assert_eq!(t.cursor(), [9, 0]);
    // should not change y
    t.set_cursor(8, 4);
    t.parse_str("\x1b[a");
    assert_eq!(t.cursor(), [9, 4]);
}

#[test]
fn cursor_positioning_vertical_position_absolute_vpa() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[d");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[1d");
    assert_eq!(t.cursor(), [0, 0]);
    t.parse_str("\x1b[2d");
    assert_eq!(t.cursor(), [0, 1]);
    t.parse_str("\x1b[5d");
    assert_eq!(t.cursor(), [0, 4]);
    t.parse_str("\x1b[100d");
    assert_eq!(t.cursor(), [0, 9]);
    // should not change x
    t.set_cursor(8, 4);
    t.parse_str("\x1b[d");
    assert_eq!(t.cursor(), [8, 0]);
}

#[test]
fn cursor_positioning_vertical_position_relative_vpr() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[e");
    assert_eq!(t.cursor(), [0, 1]);
    t.parse_str("\x1b[1e");
    assert_eq!(t.cursor(), [0, 2]);
    t.parse_str("\x1b[4e");
    assert_eq!(t.cursor(), [0, 6]);
    t.parse_str("\x1b[100e");
    assert_eq!(t.cursor(), [0, 9]);
    // should not change x
    t.set_cursor(8, 4);
    t.parse_str("\x1b[e");
    assert_eq!(t.cursor(), [8, 5]);
}

/// Upstream `describe('should clamp cursor into addressible range')`: the cursor at 10000,
/// 10000 and then at -10000, -10000 (0, 0 here: see the module notes).
fn clamp_case(input: &str, from_far: [usize; 2], from_negative: [usize; 2]) {
    let mut t = cursor_handler();
    t.set_cursor(10000, 10000);
    t.parse_str(input);
    assert_eq!(t.cursor(), from_far, "{input:?} from 10000");
    t.set_cursor(0, 0);
    t.parse_str(input);
    assert_eq!(t.cursor(), from_negative, "{input:?} from -10000");
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cuf() {
    clamp_case("\x1b[C", [9, 9], [1, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cub() {
    clamp_case("\x1b[D", [8, 9], [0, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cud() {
    clamp_case("\x1b[B", [9, 9], [0, 1]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cuu() {
    clamp_case("\x1b[A", [9, 8], [0, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cnl() {
    clamp_case("\x1b[E", [0, 9], [0, 1]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cpl() {
    clamp_case("\x1b[F", [0, 8], [0, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cha() {
    clamp_case("\x1b[5G", [4, 9], [4, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_cup() {
    clamp_case("\x1b[5;5H", [4, 4], [4, 4]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_hpa() {
    clamp_case("\x1b[5`", [4, 9], [4, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_hpr() {
    clamp_case("\x1b[a", [9, 9], [1, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_vpa() {
    clamp_case("\x1b[5d", [9, 4], [0, 4]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_vpr() {
    clamp_case("\x1b[e", [9, 9], [0, 1]);
}

#[test]
fn decstbm_scroll_margins_should_default_to_whole_viewport() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[r");
    assert_eq!(t.buffer().scroll_top, 0);
    assert_eq!(t.buffer().scroll_bottom, 9);
    t.parse_str("\x1b[3;7r");
    assert_eq!(t.buffer().scroll_top, 2);
    assert_eq!(t.buffer().scroll_bottom, 6);
    t.parse_str("\x1b[0;0r");
    assert_eq!(t.buffer().scroll_top, 0);
    assert_eq!(t.buffer().scroll_bottom, 9);
}

#[test]
fn decstbm_scroll_margins_should_clamp_bottom() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[3;1000r");
    assert_eq!(t.buffer().scroll_top, 2);
    assert_eq!(t.buffer().scroll_bottom, 9);
}

#[test]
fn decstbm_scroll_margins_should_only_apply_for_top_lt_bottom() {
    let mut t = cursor_handler();
    t.parse_str("\x1b[7;2r");
    assert_eq!(t.buffer().scroll_top, 0);
    assert_eq!(t.buffer().scroll_bottom, 9);
}

#[test]
fn decstbm_scroll_margins_should_home_cursor() {
    let mut t = cursor_handler();
    t.set_cursor(10000, 10000);
    t.parse_str("\x1b[2;7r");
    assert_eq!(t.cursor(), [0, 0]);
}

#[test]
fn should_parse_big_chunks_in_smaller_subchunks() {
    // max single chunk size is hardcoded as 131072
    let mut t = upstream_handler();
    t.resize(10, 10);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&calls);
    t.parser
        .set_print_handler(move |_, data| recorder.borrow_mut().push(data.len()));
    t.parse_str("12345");
    t.parse_str(&"a".repeat(10000));
    t.parse_str(&"a".repeat(200_000));
    t.parse_str(&"a".repeat(300_000));
    assert_eq!(
        *calls.borrow(),
        [
            5,
            10000,
            131_072,
            200_000 - 131_072,
            131_072,
            131_072,
            300_000 - 131_072 - 131_072
        ]
    );
}

#[test]
fn should_correctly_reset_cells_taken_by_wide_chars_print() {
    let mut t = upstream_handler();
    t.resize(10, 5);
    t.set_option(OptionValue::Scrollback(1));
    t.parse_str(&"￥".repeat(20));
    t.parse_str("\x1b[H#");
    assert_eq!(
        t.screen(),
        ["# ￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[1;6H######");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "# ￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("#");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "##￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("#");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "### ￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[3;9H#");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "### ￥￥￥", "￥￥￥￥#", "￥￥￥￥￥", ""]
    );
    t.parse_str("#");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "### ￥￥￥", "￥￥￥￥##", "￥￥￥￥￥", ""]
    );
    t.parse_str("#");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "### ￥￥￥", "￥￥￥￥##", "# ￥￥￥￥", ""]
    );
    t.parse_str("\x1b[4;10H#");
    assert_eq!(
        t.screen(),
        ["# ￥ #####", "### ￥￥￥", "￥￥￥￥##", "# ￥￥￥ #", ""]
    );
}

/// tty ICANON sends <BS SP BS> on pressing BS.
const TTY_BS: &str = "\x08 \x08";

/// Upstream `describe('BS with reverseWraparound set/unset')` at `:1645`: 5 x 5.
fn bs_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(5, 5);
    t.set_option(OptionValue::Scrollback(1));
    t
}

#[test]
fn bs_with_reverse_wraparound_unset_cannot_delete_last_cell() {
    let mut t = bs_handler();
    t.parse_str("12345");
    t.parse_str(TTY_BS);
    assert_eq!(t.lines(1), ["123 5"]);
    t.parse_str(&TTY_BS.repeat(10));
    assert_eq!(t.lines(1), ["    5"]);
}

#[test]
fn bs_with_reverse_wraparound_unset_cannot_access_prev_line() {
    let mut t = bs_handler();
    t.parse_str(&"12345".repeat(2));
    t.parse_str(TTY_BS);
    assert_eq!(t.lines(2), ["12345", "123 5"]);
    t.parse_str(&TTY_BS.repeat(10));
    assert_eq!(t.lines(2), ["12345", "    5"]);
}

// --- added: print's `oldRow` across the scroll (expected cells from @xterm/headless 6.0.0 with
// the same provider and attributes; the attributes are set with SGR 31 / 44 there) ---

/// (a) One row, no scrollback: the scroll recycles the old row as the new row, so upstream's
/// `oldRow` *is* the new row. VS16 after `abcde` widens `e` to 2 at x = 5: wrap with
/// `oldCol = 4`. The copy reads the recycled (blank) row, so `e` is lost; the tail clear writes
/// cell 4 of the same row with the current attributes. Oracle: `["\u{FE0F}"/2 fg default bg
/// blue, ""/0 red/blue, ""/1 default/blue, ""/1 default/blue, ""/1 red/blue]`, wrapped, x = 2.
#[test]
fn print_wrap_with_one_row_and_no_scrollback_aliases_the_old_row() {
    let mut t = test_handler(5, 1, [OptionValue::Scrollback(0)]);
    with_vs16(&mut t);
    t.handler.cur_attr_data.fg = RED;
    t.handler.cur_attr_data.bg = BLUE;
    t.parse_str("abcde");
    t.parse_str("\u{FE0F}");
    assert_eq!(t.buffer().lines.len(), 1);
    assert_eq!(t.cell(0, 0), ("\u{FE0F}".to_string(), 2, 0, BLUE));
    assert_eq!(t.cell(0, 1), (String::new(), 0, RED, BLUE));
    assert_eq!(t.cell(0, 2), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(0, 3), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(0, 4), (String::new(), 1, RED, BLUE));
    assert!(t.buffer().lines.get(0).unwrap().is_wrapped);
    assert_eq!(t.cursor(), [2, 0]);
}

/// (a) with an early-wrapping wide char (`old_width` 0): `￥` at x = 4 of 5 wraps with
/// `oldCol = 4`. Oracle: `["￥"/2 red/blue, ""/0 red/blue, ""/1 default/blue, ""/1
/// default/blue, ""/1 red/blue]`, wrapped, x = 2.
#[test]
fn print_early_wrap_with_one_row_and_no_scrollback_clears_the_tail_of_the_new_row() {
    let mut t = test_handler(5, 1, [OptionValue::Scrollback(0)]);
    t.handler.cur_attr_data.fg = RED;
    t.handler.cur_attr_data.bg = BLUE;
    t.parse_str("abcd");
    t.parse_str("￥");
    assert_eq!(t.cell(0, 0), ("￥".to_string(), 2, RED, BLUE));
    assert_eq!(t.cell(0, 1), (String::new(), 0, RED, BLUE));
    assert_eq!(t.cell(0, 2), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(0, 3), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(0, 4), (String::new(), 1, RED, BLUE));
    assert!(t.buffer().lines.get(0).unwrap().is_wrapped);
    assert_eq!(t.cursor(), [2, 0]);
}

/// (b) Inner scroll region (rows 2-3 of 4): the wrap at the region bottom shifts the region up,
/// so the old row is now row 1. `d` (x = 3) moves to the new row 2 with VS16; row 1 keeps `abc`
/// and its cell 3 is cleared with red. Oracle: row 0 `0000`; row 1 `a b c` red + `""`/1 red;
/// row 2 (wrapped) `"d\u{FE0F}"`/2 red, `""`/0 red, two default blanks; row 3 `3333`;
/// cursor (2, 2), ybase 0.
#[test]
fn print_wrap_inside_a_scroll_region_follows_the_old_row_up() {
    let mut t = test_handler(4, 4, []);
    with_vs16(&mut t);
    t.parse_str("0000\r\n1111\r\n2222\r\n3333\x1b[2;3r\x1b[3;1H");
    t.handler.cur_attr_data.fg = RED;
    t.parse_str("abcd");
    t.parse_str("\u{FE0F}");
    assert_eq!(t.lines(4), ["0000", "abc", "d\u{FE0F}", "3333"]);
    for x in 0..3 {
        assert_eq!(t.cell(1, x).2, RED);
    }
    assert_eq!(t.cell(1, 3), (String::new(), 1, RED, 0));
    assert_eq!(t.cell(2, 0), ("d\u{FE0F}".to_string(), 2, RED, 0));
    assert_eq!(t.cell(2, 1), (String::new(), 0, RED, 0));
    assert_eq!(t.cell(2, 2), (String::new(), 1, 0, 0));
    assert_eq!(t.cell(2, 3), (String::new(), 1, 0, 0));
    assert!(!t.buffer().lines.get(1).unwrap().is_wrapped);
    assert!(t.buffer().lines.get(2).unwrap().is_wrapped);
    assert_eq!(t.cursor(), [2, 2]);
    assert_eq!(t.buffer().ybase, 0);
}

/// (c) Full scrollback (2 rows + 2 lines): the scroll recycles the oldest line, so the old row
/// moves from index 3 to 2. Oracle: lines `22`, `33`, `abc` red + `""`/1 red, (wrapped)
/// `"d\u{FE0F}"`/2 red, `""`/0 red, two default blanks; cursor (2, 1), ybase 2, length 4.
#[test]
fn print_wrap_with_full_scrollback_follows_the_old_row_into_the_recycled_list() {
    let mut t = test_handler(4, 2, [OptionValue::Scrollback(2)]);
    with_vs16(&mut t);
    t.parse_str("11\r\n22\r\n33\r\n");
    t.handler.cur_attr_data.fg = RED;
    t.parse_str("abcd");
    assert!(t.buffer().lines.is_full());
    t.parse_str("\u{FE0F}");
    assert_eq!(t.lines(4), ["22", "33", "abc", "d\u{FE0F}"]);
    assert_eq!(t.cell(2, 3), (String::new(), 1, RED, 0));
    assert_eq!(t.cell(3, 0), ("d\u{FE0F}".to_string(), 2, RED, 0));
    assert_eq!(t.cell(3, 1), (String::new(), 0, RED, 0));
    assert_eq!(t.cell(3, 2), (String::new(), 1, 0, 0));
    assert!(t.buffer().lines.get(3).unwrap().is_wrapped);
    assert_eq!(t.cursor(), [2, 1]);
    assert_eq!(t.buffer().ybase, 2);
    assert_eq!(t.buffer().lines.len(), 4);
}

#[test]
fn old_row_after_scroll_covers_every_scroll_branch() {
    // push: list not full, region at the bottom
    let t = test_handler(4, 3, [OptionValue::Scrollback(10)]);
    assert_eq!(old_row_after_scroll(t.buffer(), 2), Some(2));
    // recycle: full list; the oldest line becomes the new last line
    let mut t = test_handler(4, 1, [OptionValue::Scrollback(0)]);
    assert!(t.buffer().lines.is_full());
    assert_eq!(old_row_after_scroll(t.buffer(), 0), Some(0));
    t = test_handler(4, 2, [OptionValue::Scrollback(0)]);
    assert_eq!(old_row_after_scroll(t.buffer(), 1), Some(0));
    // splice above the last line (scroll_top 0, region bottom above the viewport bottom)
    let mut t = test_handler(4, 3, [OptionValue::Scrollback(0)]);
    t.buffer_mut().scroll_bottom = 0;
    assert_eq!(old_row_after_scroll(t.buffer(), 0), None);
    assert_eq!(old_row_after_scroll(t.buffer(), 2), Some(2));
    let mut t = test_handler(4, 3, [OptionValue::Scrollback(10)]);
    t.buffer_mut().scroll_bottom = 1;
    assert_eq!(old_row_after_scroll(t.buffer(), 1), Some(1));
    assert_eq!(old_row_after_scroll(t.buffer(), 2), Some(3));
    // inner region: rows in the region move up one, the top row leaves the list
    let mut t = test_handler(4, 5, []);
    t.buffer_mut().scroll_top = 1;
    t.buffer_mut().scroll_bottom = 3;
    assert_eq!(old_row_after_scroll(t.buffer(), 3), Some(2));
    assert_eq!(old_row_after_scroll(t.buffer(), 1), None);
    assert_eq!(old_row_after_scroll(t.buffer(), 4), Some(4));
    assert_eq!(old_row_after_scroll(t.buffer(), 0), Some(0));
}

/// The splice branch with a full list and the old row at index 0 (reachable only with a
/// hand-set margin): upstream writes to a detached line, so nothing visible changes beyond the
/// new line, and nothing panics.
#[test]
fn print_wrap_when_the_old_row_leaves_the_list_writes_nothing_to_it() {
    let mut t = test_handler(4, 3, [OptionValue::Scrollback(0)]);
    with_vs16(&mut t);
    t.parse_str("\x1b[2;1Hxx\x1b[3;1Hyy\x1b[1;1Habcd");
    t.buffer_mut().scroll_bottom = 0;
    t.parse_str("\u{FE0F}");
    // the old row 0 was trimmed; the new row (spliced at 1) carries `d` + VS16
    assert_eq!(t.lines(3), ["d\u{FE0F}", "xx", "yy"]);
}

// --- added: parse ---

#[test]
fn a_300000_byte_write_equals_the_same_bytes_in_one_byte_writes() {
    let mut data = "a".repeat(131_071).into_bytes();
    data.extend_from_slice("é".as_bytes()); // spans the 131072-byte chunk boundary
    let pattern = "héllo\r\n\x1b[2Cwörld ￥\x1b[A\n\t\x08x\x1b[3;7r\x1bM\x1b[r";
    while data.len() < 300_000 {
        data.extend_from_slice(pattern.as_bytes());
    }
    data.truncate(300_000);
    assert_eq!(data[131_071], 0xC3);

    let mut whole = test_handler(10, 5, []);
    whole.parse_bytes(&data);
    let mut split = test_handler(10, 5, []);
    for byte in &data {
        split.parse_bytes(std::slice::from_ref(byte));
    }
    assert_eq!(whole.snapshot(), split.snapshot());
}

#[test]
fn parse_reports_cursor_moves_and_dirty_rows_as_damage() {
    let mut t = test_handler(10, 5, []);
    t.parse_str("ab\r\ncd");
    let damage = t.handler.take_damage();
    assert!(damage.cursor_moved);
    assert_eq!(damage.rows, Some((0, 1)));
    assert!(!damage.scrolled);
    assert!(t.handler.take_damage().is_empty());
    // no cursor move: the cursor row is still refreshed
    t.parse_str("\x1b[s\x1b[u");
    let damage = t.handler.take_damage();
    assert!(!damage.cursor_moved);
    assert_eq!(damage.rows, Some((1, 1)));
    // a line feed at the bottom scrolls
    t.parse_str("\n\n\n\n");
    let damage = t.handler.take_damage();
    assert!(damage.scrolled);
    assert_eq!(damage.rows, Some((0, 4)));
}

// --- added: D8 (missing lines, params) ---

/// Scrollback 0 + a reflow to fewer columns leaves `ybase + y` past the list (M5's soft
/// invariant, `reflow_without_scrollback_matches_xterm_js`). Upstream throws in `print`; the
/// handler stops instead and later sequences keep working.
#[test]
fn missing_cursor_line_stops_the_handler_without_panic() {
    let mut t = test_handler(97, 9, [OptionValue::Scrollback(0)]);
    for _ in 0..8 {
        t.parse_str(&"a".repeat(97));
        t.parse_str("\r\n");
    }
    t.parse_str(&"a".repeat(10));
    t.resize(23, 15);
    let buffer = t.buffer();
    assert_eq!(
        (buffer.ybase, buffer.lines.len(), buffer.y),
        (1, 15, 14),
        "recipe"
    );
    assert!(buffer.lines.get(abs_y(buffer)).is_none());

    let before = t.snapshot();
    t.parse_str("xyz");
    assert_eq!(t.snapshot(), before, "print writes nothing");

    t.parse_str("\x08\t\x1b[A\x1bM\x1bD\x1b7\x1b8\x1bE\x1bH\x1b[3g\x1b[I\x1b[Z\x0e\x0f");
    t.parse_str("\r\n\x0b\x0cmore text\r\n\x1b[1;1Hok");
    assert_eq!(t.cursor(), [2, 0]);
}

#[test]
fn huge_params_do_not_overflow_and_tab_loops_end() {
    // Values from @xterm/headless 6.0.0 (CUU..VPR, CUP, DECSTBM). Upstream's CHT/CBT loop Ps
    // times: its run with Ps = 2^31 - 1 did not finish within 120 s; here the loops stop at the
    // fixed point with the same cursor.
    let mut t = test_handler(10, 5, []);
    for seq in ["A", "B", "C", "D", "E", "F", "G", "a", "d", "e", "I", "Z"] {
        t.parse_str(&format!("\x1b[2147483647{seq}"));
        t.parse_str(&format!("\x1b[99999999999{seq}"));
    }
    t.parse_str("\x1b[2147483647;2147483647H");
    assert_eq!(t.cursor(), [9, 4]);
    t.parse_str("\x1b[2147483647;2147483647r");
    assert_eq!(t.buffer().scroll_bottom, 4);
    t.parse_str("\x1b[3;2147483647r");
    assert_eq!((t.buffer().scroll_top, t.buffer().scroll_bottom), (2, 4));
}

// --- added: execute, ESC and CSI handlers ---

#[test]
fn bell_queues_a_bell_event() {
    let mut t = test_handler(10, 5, []);
    t.parse_str("a\x07b\x07");
    assert_eq!(t.handler.take_events(), [HostEvent::Bell, HostEvent::Bell]);
    assert!(t.handler.take_events().is_empty());
}

#[test]
fn screen_reader_mode_fires_a11y_char_and_tab_events() {
    let mut t = test_handler(20, 5, []);
    t.parse_str("a\tb");
    assert!(t.handler.take_events().is_empty(), "off by default");

    let mut t = test_handler(20, 5, [OptionValue::ScreenReaderMode(true)]);
    t.parse_str("ab\t￥\t");
    assert_eq!(
        t.handler.take_events(),
        [
            HostEvent::A11yChar("a".into()),
            HostEvent::A11yChar("b".into()),
            HostEvent::A11yTab(6),
            HostEvent::A11yChar("￥".into()),
            HostEvent::A11yTab(6),
        ]
    );
}

#[test]
fn line_feed_scrolls_at_the_bottom_and_honours_convert_eol() {
    let mut t = test_handler(5, 3, []);
    t.parse_str("1\n2\n3\n4");
    assert_eq!(t.viewport(), [" 2", "  3", "   4"]);
    assert_eq!(t.buffer().ybase, 1);

    let mut t = test_handler(5, 3, [OptionValue::ConvertEol(true)]);
    t.parse_str("1\n2\x0b3\x0c4");
    assert_eq!(t.viewport(), ["2", "3", "4"]);
    assert_eq!(t.cursor(), [1, 2]);

    // at the right edge LF keeps the cursor on the last column
    let mut t = test_handler(5, 3, []);
    t.parse_str("12345\n");
    assert_eq!(t.cursor(), [4, 1]);
    // an explicit LF clears the wrapped flag of the row it moves to
    t.buffer_mut().lines.get_mut(2).unwrap().is_wrapped = true;
    t.parse_str("\n");
    assert!(!t.buffer().lines.get(2).unwrap().is_wrapped);
}

#[test]
fn line_feed_runs_the_windows_heuristic_only_when_enabled() {
    let mut t = test_handler(5, 3, []);
    t.parse_str("abc\r\n");
    assert!(!t.buffer().lines.get(1).unwrap().is_wrapped);
    t.handler.windows_heuristics = true;
    t.parse_str("\x1b[1;1Habcde\x1b[1;5H\n");
    assert!(t.buffer().lines.get(1).unwrap().is_wrapped);
}

#[test]
fn index_next_line_and_reverse_index() {
    let mut t = test_handler(5, 3, []);
    t.parse_str("a\x1bDb\u{84}c");
    assert_eq!(t.viewport(), ["a", " b", "  c"]);
    t.parse_str("\x1bEd");
    assert_eq!(t.viewport(), [" b", "  c", "d"]);
    t.parse_str("\u{85}e");
    assert_eq!(t.viewport(), ["  c", "d", "e"]);
    assert_eq!((t.cursor(), t.buffer().ybase), ([1, 2], 2));

    // RI at the top margin scrolls the region down with the erase colour
    let mut t = test_handler(5, 4, []);
    t.parse_str("0\r\n1\r\n2\r\n3\x1b[2;3r\x1b[2;1H");
    t.handler.cur_attr_data.bg = BLUE;
    t.parse_str("\x1bM");
    assert_eq!(t.screen(), ["0", "", "1", "3"]);
    assert_eq!(t.cell(1, 0).3, BLUE);
    assert_eq!(t.cursor(), [0, 1]);
    // RI below the top margin moves up; at the top margin (row 0 now) it scrolls
    t.parse_str("\x1b[r\x1b[2;3H\x1bM");
    assert_eq!(t.cursor(), [2, 0]);
    assert_eq!(t.screen(), ["0", "", "1", "3"]);
    t.parse_str("\x1bM\x1bM");
    assert_eq!(t.cursor(), [2, 0]);
    assert_eq!(t.screen(), ["", "", "0", ""]);
}

#[test]
fn tab_stops_set_clear_and_move() {
    let mut t = test_handler(20, 3, []);
    t.parse_str("\t");
    assert_eq!(t.cursor(), [8, 0]);
    t.parse_str("\x1b[3G\x1bH\x1b[G\t");
    assert_eq!(t.cursor(), [2, 0]);
    t.parse_str("\u{88}\x1b[2I");
    assert_eq!(t.cursor(), [16, 0]);
    t.parse_str("\x1b[Z");
    assert_eq!(t.cursor(), [8, 0]);
    t.parse_str("\x1b[g\x1b[G\x1b[2I");
    assert_eq!(t.cursor(), [16, 0]);
    t.parse_str("\x1b[3g\x1b[G\t");
    assert_eq!(t.cursor(), [19, 0]);
    t.parse_str("\x1b[9Z");
    assert_eq!(t.cursor(), [0, 0]);
}

#[test]
fn save_and_restore_cursor_restore_the_charset() {
    let mut t = test_handler(10, 3, []);
    let dec = charset('0');
    t.handler.services.charset.set_g_charset(0, dec);
    t.parse_str("\x1b[2;3H\x1b7");
    t.handler.services.charset.set_g_charset(0, None);
    t.parse_str("\x1b[H\x1b8");
    assert_eq!(t.cursor(), [2, 1]);
    assert_eq!(t.handler.services.charset.charset, dec);
    // `q` prints as the DEC line drawing character
    t.parse_str("q");
    assert_eq!(t.lines(2)[1], "  ─");
    // nothing saved: CSI u clears the charset
    let mut t = test_handler(10, 3, []);
    t.handler.services.charset.set_g_charset(0, dec);
    t.parse_str("\x1b[u");
    assert_eq!(t.handler.services.charset.charset, None);
}

#[test]
fn shift_out_and_shift_in_switch_the_g_level() {
    let mut t = test_handler(10, 3, []);
    t.handler.services.charset.set_g_charset(1, charset('0'));
    t.parse_str("q\x0eq\x0fq");
    assert_eq!(t.lines(1), ["q─q"]);
}

#[test]
fn origin_mode_keeps_the_cursor_inside_the_margins() {
    let mut t = test_handler(10, 10, []);
    t.parse_str("\x1b[3;6r");
    t.handler.services.core.dec_private_modes.origin = true;
    t.parse_str("\x1b[1;1H");
    assert_eq!(t.cursor(), [0, 2]);
    t.parse_str("\x1b[100;100H");
    assert_eq!(t.cursor(), [9, 5]);
    t.parse_str("\x1b[100A");
    // upstream adds the top margin to the relative target again (`_moveCursor` passes the
    // absolute row to `_setCursor`): 5 - 3 + 2 = 4
    assert_eq!(t.cursor(), [9, 4]);
}

/// Reverse wrap-around (the mode is set directly until DECSET is ported, plan M6 step 7).
#[test]
fn backspace_with_reverse_wraparound_lifts_the_soft_wrap() {
    let mut t = test_handler(5, 5, []);
    t.handler.services.core.dec_private_modes.reverse_wraparound = true;
    t.parse_str(&"12345".repeat(2));
    assert!(t.buffer().lines.get(1).unwrap().is_wrapped);
    t.parse_str(&TTY_BS.repeat(8));
    assert_eq!(t.lines(2), ["12   ", "     "]);
    assert!(!t.buffer().lines.get(1).unwrap().is_wrapped);
    // an empty cell left by an early wrapping wide char is skipped
    let mut t = test_handler(5, 5, []);
    t.handler.services.core.dec_private_modes.reverse_wraparound = true;
    t.parse_str("￥￥￥\x08\x08\x08");
    assert_eq!(t.cursor(), [3, 0]);
}

#[test]
fn register_csi_handler_guards_t_by_the_window_options() {
    let mut t = test_handler(10, 3, []);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&calls);
    InputHandler::register_csi_handler(&mut t.parser, FunctionIdentifier::new('t'), move |_, p| {
        seen.borrow_mut().push(p.get(0));
        true
    })
    .unwrap();
    t.parse_str("\x1b[18t\x1b[8t");
    assert!(calls.borrow().is_empty(), "all window options are off");
    t.handler
        .services
        .options
        .set(OptionValue::WindowOptions(WindowOptions {
            get_win_size_chars: true,
            ..WindowOptions::default()
        }))
        .unwrap();
    t.parse_str("\x1b[18t\x1b[8t");
    assert_eq!(*calls.borrow(), [Some(18)]);
    // other identifiers are not guarded
    let other = Rc::new(RefCell::new(0));
    let count = Rc::clone(&other);
    InputHandler::register_csi_handler(
        &mut t.parser,
        FunctionIdentifier::with_intermediates(" ", 't'),
        move |_, _| {
            *count.borrow_mut() += 1;
            true
        },
    )
    .unwrap();
    t.parse_str("\x1b[8 t");
    assert_eq!(*other.borrow(), 1);
}

#[test]
fn param_to_window_option_maps_every_param() {
    let all = WindowOptions {
        restore_win: true,
        minimize_win: true,
        set_win_position: true,
        set_win_size_pixels: true,
        raise_win: true,
        lower_win: true,
        refresh_win: true,
        set_win_size_chars: true,
        maximize_win: true,
        fullscreen_win: true,
        get_win_state: true,
        get_win_position: true,
        get_win_size_pixels: true,
        get_screen_size_pixels: true,
        get_cell_size_pixels: true,
        get_win_size_chars: true,
        get_screen_size_chars: true,
        get_icon_title: true,
        get_win_title: true,
        push_title: true,
        pop_title: true,
        set_win_lines: true,
    };
    let enabled: Vec<i32> = (-1..30)
        .filter(|&n| param_to_window_option(Some(n), &all))
        .collect();
    let mut expected: Vec<i32> = (1..=11).chain(13..=16).chain(18..=24).collect();
    expected.extend(25..30);
    assert_eq!(enabled, expected);
    assert!(!param_to_window_option(None, &all));
    for n in -1..30 {
        assert!(!param_to_window_option(Some(n), &WindowOptions::default()));
    }
}

#[test]
fn fallbacks_log_unknown_sequences() {
    use crate::services::log_service::Logger;
    use crate::services::options_service::LogLevel;

    struct Capture(Rc<RefCell<Vec<String>>>);
    impl Logger for Capture {
        fn log(&self, _level: LogLevel, message: &str) {
            self.0.borrow_mut().push(message.to_string());
        }
    }
    let mut t = test_handler(10, 3, [OptionValue::LogLevel(LogLevel::Debug)]);
    t.handler.services.log.set_level(LogLevel::Debug);
    let messages = Rc::new(RefCell::new(Vec::new()));
    t.handler
        .services
        .log
        .set_logger(Some(Box::new(Capture(Rc::clone(&messages)))));
    t.parse_str("\x1b[1;2y\x1b(z\x01\x1b]777;hi\x07\x1bP1$zab\x1b\\ok");
    let messages = messages.borrow();
    for prefix in [
        "Unknown CSI code",
        "Unknown ESC code",
        "Unknown EXECUTE code",
        "Unknown OSC code",
        "Unknown DCS code",
    ] {
        assert!(
            messages.iter().any(|m| m.starts_with(prefix)),
            "{prefix} missing"
        );
    }
    assert_eq!(t.lines(1), ["ok"]);
}

/// `print` forwards disposed markers before `addLineToLink`: upstream freed the link the moment
/// its line was trimmed, so printing with its url id afterwards adds nothing.
#[test]
fn print_with_a_link_trimmed_earlier_in_the_same_parse_does_not_revive_it() {
    use crate::buffer::constants::BgFlags;
    use crate::services::osc_link_service::OscLinkData;

    let mut t = test_handler(10, 3, [OptionValue::Scrollback(2)]);
    let services = &mut t.handler.services;
    let id = services.osc_link.register_link(
        &mut services.buffer,
        OscLinkData {
            id: None,
            uri: "https://a".to_owned(),
        },
    );
    t.handler.cur_attr_data.bg |= BgFlags::HAS_EXTENDED;
    t.handler.cur_attr_data.extended.set_url_id(id);
    // the fifth line feed trims line 0 (3 rows + 2 scrollback); nothing forwards in between
    // except print itself
    t.parse_str("\n\n\n\n\nb");
    assert!(t.handler.services.osc_link.get_link_data(id).is_none());
    assert_eq!(t.handler.services.osc_link.marker_count(), 0);

    // control: without the trim the printed line joins the link
    let mut t = test_handler(10, 3, [OptionValue::Scrollback(2)]);
    let services = &mut t.handler.services;
    let id = services.osc_link.register_link(
        &mut services.buffer,
        OscLinkData {
            id: None,
            uri: "https://a".to_owned(),
        },
    );
    t.handler.cur_attr_data.bg |= BgFlags::HAS_EXTENDED;
    t.handler.cur_attr_data.extended.set_url_id(id);
    t.parse_str("\nb");
    assert_eq!(
        t.handler.services.osc_link.link_markers(id).unwrap().len(),
        2
    );
}
