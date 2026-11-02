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
//!  - Step 5 (erase, insert/delete, scroll, REP, DECSCA, DECALN): the `regression tests`
//!    (`insertChars`, `deleteChars`, `eraseInLine`, `eraseInLine reflow`, both ED cases) call the
//!    handler methods with `Params::fromArray`; here too, through `params`. `MockBufferService`
//!    is a `test_handler` of the same size. `ED2 with scrollOnEraseInDisplay` builds its handler
//!    over the same `bufferService` with a mock option; here the option is passed to the
//!    constructor.
//!  - Step 8 (OSC, DCS): the `OSC` describe (12) reads the `onColor` stack as
//!    [`HostEvent::Color`] events and `inputHandler.curAttrData.extended.urlId` /
//!    `oscLinkService.getLinkData` as `cur_attr_data` / `services.osc_link`; `DECRQSS reports
//!    correct DECSCA state` reads `coreService.onData` as [`HostEvent::Response`].
//!  - replaced: `InputHandler - async handlers` (3 cases: `async CUP with CPR check`, `async OSC
//!    between`, `async DCS between`): the port has no async handlers (plan M4 D3).
//!  - `print`/`should not cause an infinite loop` calls `print` with the one-element slice
//!    (upstream passes a 10-element array with `start = 0, end = 1`).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::buffer::attribute_data::ExtendedAttrs;
use crate::buffer::constants::Attributes;
use crate::data::charsets::charset;
use crate::engine::{ColorIndex, ColorRequest, WindowReportKind};
use crate::input::unicode_v6::UnicodeV6;
use crate::parser::params::ParamsArrayItem;
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

    /// Upstream `bufferService.reset()`.
    fn reset_buffers(&mut self) {
        let services = &mut self.handler.services;
        services.buffer.reset(services.options.options());
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

/// A Unicode provider over V6 where `b` takes the width the preceding character did not have
/// (1 after a wide character, else 2): a unit whose repetitions alternate (review S7).
struct AlternatingB(UnicodeV6);

impl UnicodeVersionProvider for AlternatingB {
    fn version(&self) -> &str {
        "alternating-test"
    }

    fn wcwidth(&self, codepoint: u32) -> u32 {
        self.0.wcwidth(codepoint)
    }

    fn char_properties(&self, codepoint: u32, preceding: u32) -> u32 {
        if codepoint == u32::from('b') {
            let width = if UnicodeService::extract_width(preceding) == 2 {
                1
            } else {
                2
            };
            return UnicodeService::create_property_value(0, width, false);
        }
        self.0.char_properties(codepoint, preceding)
    }
}

fn with_alternating_b(t: &mut TestTerm) {
    let unicode = &mut t.handler.services.unicode;
    unicode.register(Box::new(AlternatingB(UnicodeV6::new())));
    unicode.set_active_version("alternating-test").unwrap();
}

fn with_vs16(t: &mut TestTerm) {
    let unicode = &mut t.handler.services.unicode;
    unicode.register(Box::new(Vs16Widens(UnicodeV6::new())));
    unicode.set_active_version("vs16-test").unwrap();
}

/// The shipped VS16 provider (`11-vs16`, plan M7 step 4) active, as
/// `StandardExtensions { vs16: true }` leaves it (M7 fix round 1).
fn with_11_vs16(t: &mut TestTerm) {
    let unicode = &mut t.handler.services.unicode;
    unicode.register(Box::new(crate::ext::vs16::UnicodeV11Vs16::new()));
    unicode.set_active_version("11-vs16").unwrap();
}

/// VS16 (`U+26A0 U+FE0F`: 2 cells under `11-vs16`), a keycap (`1 U+FE0F U+20E3`: a joiner
/// after the widening one) and an emoji with a ZWJ (a wide base and a joiner): the units of
/// M7 fix round 1.
const VS16: &str = "\u{26a0}\u{fe0f}";
const KEYCAP: &str = "1\u{fe0f}\u{20e3}";
const EMOJI_ZWJ: &str = "\u{1F468}\u{200D}";

/// Upstream `Params.fromArray([..])` of plain params.
fn params(values: &[i32]) -> Params {
    let items: Vec<ParamsArrayItem> = values.iter().map(|&v| ParamsArrayItem::Param(v)).collect();
    Params::from_array(&items).unwrap()
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
            // consecutive characters merge into one event (D2 amendment, Q10)
            HostEvent::A11yText("ab".into()),
            HostEvent::A11yTab(6),
            HostEvent::A11yText("￥".into()),
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

/// Reverse wrap-around with the mode set directly (the DECSET version is
/// `bs_with_reverse_wraparound_set_*`).
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
    // an unhandled OSC and DCS log their identifier and payload length, never the payload
    // (user decision 2026-11-01)
    t.parse_str("\x1b]52;c;c2VjcmV0\x07\x1bP1$zsecret\x1b\\");
    let messages = messages.borrow();
    let fallback = |m: &&String| m.starts_with("Unknown OSC") || m.starts_with("Unknown DCS");
    assert!(
        messages
            .iter()
            .filter(fallback)
            .all(|m| !m.contains("c2VjcmV0") && !m.contains("secret")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("identifier: 52") && m.contains("length: "))
    );
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

// --- upstream: InputHandler.test.ts, step 5 (erase, insert/delete, scroll, protect) ---

/// `describe('SL/SR/DECIC/DECDC')`: 5 x 5, scrollback 1, buffers reset.
fn columns_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(5, 5);
    t.set_option(OptionValue::Scrollback(1));
    t.reset_buffers();
    t
}

#[test]
fn sl_sr_decic_decdc_sl_scroll_left() {
    let mut t = columns_handler();
    t.parse_str(&"12345".repeat(6));
    t.parse_str("\x1b[ @");
    assert_eq!(
        t.lines(6),
        ["12345", "2345", "2345", "2345", "2345", "2345"]
    );
    t.parse_str("\x1b[0 @");
    assert_eq!(t.lines(6), ["12345", "345", "345", "345", "345", "345"]);
    t.parse_str("\x1b[2 @");
    assert_eq!(t.lines(6), ["12345", "5", "5", "5", "5", "5"]);
}

#[test]
fn sl_sr_decic_decdc_sr_scroll_right() {
    let mut t = columns_handler();
    t.parse_str(&"12345".repeat(6));
    t.parse_str("\x1b[ A");
    assert_eq!(
        t.lines(6),
        ["12345", " 1234", " 1234", " 1234", " 1234", " 1234"]
    );
    t.parse_str("\x1b[0 A");
    assert_eq!(
        t.lines(6),
        ["12345", "  123", "  123", "  123", "  123", "  123"]
    );
    t.parse_str("\x1b[2 A");
    assert_eq!(
        t.lines(6),
        ["12345", "    1", "    1", "    1", "    1", "    1"]
    );
}

/// The three runs of the DECIC / DECDC tests: no param, `1` and `2`, each on a fresh buffer.
fn columns_runs(final_byte: &str, expected: [[&str; 6]; 3]) {
    let mut t = columns_handler();
    for (param, expect) in ["", "1", "2"].into_iter().zip(expected) {
        t.reset_buffers();
        t.parse_str(&"12345".repeat(6));
        t.parse_str("\x1b[3;3H");
        t.parse_str(&format!("\x1b[{param}'{final_byte}"));
        assert_eq!(t.lines(6), expect, "param {param:?}");
    }
}

#[test]
fn sl_sr_decic_decdc_insert_columns_decic() {
    let row = ["12345", "12 34", "12 34", "12 34", "12 34", "12 34"];
    columns_runs(
        "}",
        [
            row,
            row,
            ["12345", "12  3", "12  3", "12  3", "12  3", "12  3"],
        ],
    );
}

#[test]
fn sl_sr_decic_decdc_delete_columns_decdc() {
    let row = ["12345", "1245", "1245", "1245", "1245", "1245"];
    columns_runs(
        "~",
        [row, row, ["12345", "125", "125", "125", "125", "125"]],
    );
}

fn eighty_by_thirty() -> TestTerm {
    test_handler(80, 30, [])
}

fn a_run(n: usize) -> String {
    "a".repeat(n)
}

#[test]
fn regression_tests_insert_chars() {
    let mut t = eighty_by_thirty();
    // insert some data in first and second line
    t.parse_str(&a_run(70));
    t.parse_str("1234567890");
    t.parse_str(&a_run(70));
    t.parse_str("1234567890");
    let line = |t: &TestTerm, trim| {
        t.buffer()
            .lines
            .get(0)
            .unwrap()
            .translate_to_string(trim, None, None, None)
    };
    assert_eq!(line(&t, false), a_run(70) + "1234567890");
    // insert one char from params = [0]
    for (param, tail) in [
        (0, " 123456789"),
        (1, "  12345678"),
        (2, "    123456"),
        (10, "          "),
    ] {
        t.set_cursor(70, 0);
        t.handler.insert_chars(&params(&[param]));
        assert_eq!(line(&t, false), a_run(70) + tail, "param {param}");
    }
    assert_eq!(line(&t, true), a_run(70));
}

#[test]
fn regression_tests_delete_chars() {
    let mut t = eighty_by_thirty();
    t.parse_str(&a_run(70));
    t.parse_str("1234567890");
    t.parse_str(&a_run(70));
    t.parse_str("1234567890");
    let line = |t: &TestTerm, trim| {
        t.buffer()
            .lines
            .get(0)
            .unwrap()
            .translate_to_string(trim, None, None, None)
    };
    assert_eq!(line(&t, false), a_run(70) + "1234567890");
    for (param, full, trimmed) in [
        (0, "234567890 ", "234567890"),
        (1, "34567890  ", "34567890"),
        (2, "567890    ", "567890"),
        (10, "          ", ""),
    ] {
        t.set_cursor(70, 0);
        t.handler.delete_chars(&params(&[param]));
        assert_eq!(line(&t, false), a_run(70) + full, "param {param}");
        assert_eq!(line(&t, true), a_run(70) + trimmed, "param {param}");
    }
}

#[test]
fn regression_tests_erase_in_line() {
    let mut t = eighty_by_thirty();
    // fill 6 lines to test 3 different states
    for _ in 0..3 {
        t.parse_str(&a_run(80));
    }
    let row = |t: &TestTerm, y| {
        t.buffer()
            .lines
            .get(y)
            .unwrap()
            .translate_to_string(false, None, None, None)
    };
    // params[0] - right erase
    t.set_cursor(70, 0);
    t.handler.erase_in_line(&params(&[0]), false);
    assert_eq!(row(&t, 0), a_run(70) + "          ");
    // params[1] - left erase
    t.set_cursor(70, 1);
    t.handler.erase_in_line(&params(&[1]), false);
    assert_eq!(row(&t, 1), " ".repeat(71) + "aaaaaaaaa");
    // params[2] - erase all
    t.set_cursor(70, 2);
    t.handler.erase_in_line(&params(&[2]), false);
    assert_eq!(row(&t, 2), " ".repeat(80));
}

#[test]
fn regression_tests_erase_in_line_reflow() {
    let mut t = eighty_by_thirty();
    fn reset_to_base_state(t: &mut TestTerm) {
        // reset and add a wrapped line
        t.set_cursor(0, 0);
        t.parse_str(&a_run(80)); // line 0
        t.parse_str(&a_run(89)); // line 1 and 2
        for _ in 3..30 {
            t.parse_str(&a_run(80));
        }
        // confirm precondition that line 2 is wrapped
        assert!(t.buffer().lines.get(2).unwrap().is_wrapped);
    }
    let wrapped = |t: &TestTerm| t.buffer().lines.get(2).unwrap().is_wrapped;

    // params[0] - erase from the cursor through the end of the row.
    reset_to_base_state(&mut t);
    t.set_cursor(40, 2);
    t.handler.erase_in_line(&params(&[0]), false);
    assert!(wrapped(&t));
    t.set_cursor(0, 2);
    t.handler.erase_in_line(&params(&[0]), false);
    assert!(!wrapped(&t));

    // params[1] - erase from the beginning of the line through the cursor
    reset_to_base_state(&mut t);
    t.set_cursor(40, 2);
    t.handler.erase_in_line(&params(&[1]), false);
    assert!(wrapped(&t));

    // params[2] - erase complete line
    reset_to_base_state(&mut t);
    t.set_cursor(40, 2);
    t.handler.erase_in_line(&params(&[2]), false);
    assert!(!wrapped(&t));
}

#[test]
fn regression_tests_ed2_with_scroll_on_erase_in_display_turned_on() {
    let mut t = test_handler(80, 30, [OptionValue::ScrollOnEraseInDisplay(true)]);
    let a_line = a_run(80);
    // add 2 full lines of text.
    t.parse_str(&a_line);
    t.parse_str(&a_line);

    t.handler.erase_in_display(&params(&[2]), false);
    // those 2 lines should have been pushed to scrollback.
    assert_eq!(30 + 2, t.buffer().lines.len());
    assert_eq!(t.buffer().ybase, 2);
    for i in 0..2 {
        let line = t.buffer().lines.get(i).unwrap();
        assert_eq!(line.translate_to_string(false, None, None, None), a_line);
    }

    // Move to last line and add more text.
    t.set_cursor(0, 29);
    t.parse_str(&a_line);
    t.handler.erase_in_display(&params(&[2]), false);
    // Screen should have been scrolled by a full screen size.
    assert_eq!(30 * 2 + 2, t.buffer().lines.len());
}

#[test]
fn regression_tests_erase_in_display() {
    let mut t = test_handler(80, 7, []);
    let rows = |t: &TestTerm, trim| -> Vec<String> {
        (0..7)
            .map(|i| {
                t.buffer()
                    .lines
                    .get(i)
                    .unwrap()
                    .translate_to_string(trim, None, None, None)
            })
            .collect()
    };
    let fill = |t: &mut TestTerm| {
        t.set_cursor(0, 0);
        for _ in 0..7 {
            t.parse_str(&a_run(80));
        }
    };
    let a = a_run(80);
    let blank = " ".repeat(80);

    // fill display with a's
    fill(&mut t);
    // params [0] - right and below erase
    t.set_cursor(40, 5);
    t.handler.erase_in_display(&params(&[0]), false);
    let partial = a_run(40) + &" ".repeat(40);
    assert_eq!(
        rows(&t, false),
        [&a, &a, &a, &a, &a, &partial, &blank].map(String::clone)
    );
    assert_eq!(
        rows(&t, true),
        [&a, &a, &a, &a, &a, &a_run(40), &String::new()].map(String::clone)
    );

    // params [1] - left and above
    fill(&mut t);
    t.set_cursor(40, 5);
    t.handler.erase_in_display(&params(&[1]), false);
    let partial = " ".repeat(41) + &a_run(39);
    assert_eq!(
        rows(&t, false),
        [&blank, &blank, &blank, &blank, &blank, &partial, &a].map(String::clone)
    );
    assert_eq!(
        rows(&t, true),
        ["", "", "", "", "", &partial, &a].map(str::to_owned)
    );

    // params [2] - whole screen
    fill(&mut t);
    t.set_cursor(40, 5);
    t.handler.erase_in_display(&params(&[2]), false);
    assert_eq!(rows(&t, false), vec![blank.clone(); 7]);
    assert_eq!(rows(&t, true), vec![String::new(); 7]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_dch() {
    clamp_case("\x1b[P", [9, 9], [0, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_dch_should_delete_last_cell() {
    let mut t = cursor_handler();
    t.parse_str("0123456789\x1b[P");
    assert_eq!(t.lines(1), ["012345678"]);
    let line = t.buffer().lines.get(0).unwrap();
    assert_eq!(
        line.translate_to_string(false, None, None, None),
        "012345678 "
    );
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_ech() {
    clamp_case("\x1b[X", [9, 9], [0, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_ech_should_delete_last_cell() {
    let mut t = cursor_handler();
    t.parse_str("0123456789\x1b[X");
    let line = t.buffer().lines.get(0).unwrap();
    assert_eq!(
        line.translate_to_string(false, None, None, None),
        "012345678 "
    );
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_ich() {
    clamp_case("\x1b[@", [9, 9], [0, 0]);
}

#[test]
fn cursor_positioning_should_clamp_cursor_into_addressible_range_ich_should_delete_last_cell() {
    let mut t = cursor_handler();
    t.parse_str("0123456789\x1b[@");
    let line = t.buffer().lines.get(0).unwrap();
    assert_eq!(
        line.translate_to_string(false, None, None, None),
        "012345678 "
    );
}

/// `describe('scroll margins')`: 10 x 10.
fn margins_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(10, 10);
    t
}

const TEN_LINES: &str = "0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9";

#[test]
fn scroll_margins_scroll_up() {
    let mut t = margins_handler();
    t.parse_str(&format!("{TEN_LINES}\x1b[2;4r\x1b[2Sm"));
    assert_eq!(t.screen(), ["m", "3", "", "", "4", "5", "6", "7", "8", "9"]);
}

#[test]
fn scroll_margins_scroll_down() {
    let mut t = margins_handler();
    t.parse_str(&format!("{TEN_LINES}\x1b[2;4r\x1b[2Tm"));
    assert_eq!(t.screen(), ["m", "", "", "1", "4", "5", "6", "7", "8", "9"]);
}

/// The "out of margins" cases of IL (`L`) and DL (`M`): the expected screens are the same.
fn lines_out_of_margins(final_byte: char) {
    let mut t = margins_handler();
    t.parse_str(&format!("{TEN_LINES}\x1b[3;6r"));
    assert_eq!(t.buffer().scroll_top, 2);
    assert_eq!(t.buffer().scroll_bottom, 5);
    t.parse_str(&format!("\x1b[2{final_byte}m"));
    assert_eq!(
        t.screen(),
        ["m", "1", "2", "3", "4", "5", "6", "7", "8", "9"]
    );
    t.parse_str(&format!("\x1b[2H\x1b[2{final_byte}n"));
    assert_eq!(
        t.screen(),
        ["m", "n", "2", "3", "4", "5", "6", "7", "8", "9"]
    );
    // skip below scrollbottom
    t.parse_str(&format!("\x1b[7H\x1b[2{final_byte}o"));
    assert_eq!(
        t.screen(),
        ["m", "n", "2", "3", "4", "5", "o", "7", "8", "9"]
    );
    t.parse_str(&format!("\x1b[8H\x1b[2{final_byte}p"));
    assert_eq!(
        t.screen(),
        ["m", "n", "2", "3", "4", "5", "o", "p", "8", "9"]
    );
    t.parse_str(&format!("\x1b[100H\x1b[2{final_byte}q"));
    assert_eq!(
        t.screen(),
        ["m", "n", "2", "3", "4", "5", "o", "p", "8", "q"]
    );
}

#[test]
fn scroll_margins_insert_lines_out_of_margins() {
    lines_out_of_margins('L');
}

#[test]
fn scroll_margins_insert_lines_within_margins() {
    let mut t = margins_handler();
    t.parse_str(&format!("{TEN_LINES}\x1b[3;6r"));
    assert_eq!(t.buffer().scroll_top, 2);
    assert_eq!(t.buffer().scroll_bottom, 5);
    t.parse_str("\x1b[3H\x1b[2Lm");
    assert_eq!(
        t.screen(),
        ["0", "1", "m", "", "2", "3", "6", "7", "8", "9"]
    );
    t.parse_str("\x1b[6H\x1b[2Ln");
    assert_eq!(
        t.screen(),
        ["0", "1", "m", "", "2", "n", "6", "7", "8", "9"]
    );
}

#[test]
fn scroll_margins_delete_lines_out_of_margins() {
    lines_out_of_margins('M');
}

#[test]
fn scroll_margins_delete_lines_within_margins() {
    let mut t = margins_handler();
    t.parse_str(&format!("{TEN_LINES}\x1b[3;6r"));
    assert_eq!(t.buffer().scroll_top, 2);
    assert_eq!(t.buffer().scroll_bottom, 5);
    t.parse_str("\x1b[6H\x1b[2Mm");
    assert_eq!(
        t.screen(),
        ["0", "1", "2", "3", "4", "m", "6", "7", "8", "9"]
    );
    t.parse_str("\x1b[3H\x1b[2Mn");
    assert_eq!(t.screen(), ["0", "1", "n", "m", "", "", "6", "7", "8", "9"]);
}

/// `describe('should correctly reset cells taken by wide chars')`: 10 x 5, scrollback 1, 20 `￥`.
fn wide_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(10, 5);
    t.set_option(OptionValue::Scrollback(1));
    t.parse_str(&"￥".repeat(20));
    t
}

#[test]
fn should_correctly_reset_cells_taken_by_wide_chars_el() {
    let mut t = wide_handler();
    t.parse_str("\x1b[1;6H\x1b[K#");
    assert_eq!(
        t.lines(5),
        ["￥￥ #", "￥￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[2;5H\x1b[1K");
    assert_eq!(
        t.lines(5),
        ["￥￥ #", "      ￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[3;6H\x1b[1K");
    assert_eq!(
        t.lines(5),
        ["￥￥ #", "      ￥￥", "      ￥￥", "￥￥￥￥￥", ""]
    );
}

#[test]
fn should_correctly_reset_cells_taken_by_wide_chars_ich() {
    let mut t = wide_handler();
    t.parse_str("\x1b[1;6H\x1b[@");
    assert_eq!(
        t.lines(5),
        ["￥￥   ￥", "￥￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[2;4H\x1b[2@");
    assert_eq!(
        t.lines(5),
        ["￥￥   ￥", "￥    ￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[3;4H\x1b[3@");
    assert_eq!(
        t.lines(5),
        ["￥￥   ￥", "￥    ￥￥", "￥     ￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[4;4H\x1b[4@");
    assert_eq!(
        t.lines(5),
        ["￥￥   ￥", "￥    ￥￥", "￥     ￥", "￥      ￥", ""]
    );
}

#[test]
fn should_correctly_reset_cells_taken_by_wide_chars_dch() {
    let mut t = wide_handler();
    t.parse_str("\x1b[1;6H\x1b[P");
    assert_eq!(
        t.lines(5),
        ["￥￥ ￥￥", "￥￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[2;6H\x1b[2P");
    assert_eq!(
        t.lines(5),
        ["￥￥ ￥￥", "￥￥  ￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[3;6H\x1b[3P");
    assert_eq!(
        t.lines(5),
        ["￥￥ ￥￥", "￥￥  ￥", "￥￥ ￥", "￥￥￥￥￥", ""]
    );
}

#[test]
fn should_correctly_reset_cells_taken_by_wide_chars_ech() {
    let mut t = wide_handler();
    t.parse_str("\x1b[1;6H\x1b[X");
    assert_eq!(
        t.lines(5),
        ["￥￥  ￥￥", "￥￥￥￥￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[2;6H\x1b[2X");
    assert_eq!(
        t.lines(5),
        ["￥￥  ￥￥", "￥￥    ￥", "￥￥￥￥￥", "￥￥￥￥￥", ""]
    );
    t.parse_str("\x1b[3;6H\x1b[3X");
    assert_eq!(
        t.lines(5),
        ["￥￥  ￥￥", "￥￥    ￥", "￥￥    ￥", "￥￥￥￥￥", ""]
    );
}

/// `describe('EL/ED cursor at buffer.cols')`: 10 x 5.
fn at_cols_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(10, 5);
    t
}

fn at_cols_case(final_seq: &str, expected_first_line: &str) {
    let mut t = at_cols_handler();
    t.parse_str(&format!("##########{final_seq}"));
    assert_eq!(t.buffer().x, 10);
    assert_eq!(t.screen(), [expected_first_line, "", "", "", ""]);
}

#[test]
fn el_ed_cursor_at_cols_el0() {
    at_cols_case("\x1b[0K", "##########");
}

#[test]
fn el_ed_cursor_at_cols_el1() {
    at_cols_case("\x1b[1K", "");
}

#[test]
fn el_ed_cursor_at_cols_el2() {
    at_cols_case("\x1b[2K", "");
}

#[test]
fn el_ed_cursor_at_cols_ed0() {
    at_cols_case("\x1b[0J", "##########");
}

#[test]
fn el_ed_cursor_at_cols_ed1() {
    at_cols_case("\x1b[1J", "");
}

#[test]
fn el_ed_cursor_at_cols_ed2() {
    at_cols_case("\x1b[2J", "");
}

#[test]
fn el_ed_cursor_at_cols_ed3() {
    at_cols_case("\x1b[3J", "##########");
}

#[test]
fn el_ed_cursor_at_cols_following_sequence_keeps_working_cursor_never_advances_beyond_cols() {
    let seqs = [
        "\x1b[10@",
        "\x1b[10 @",
        "\x1b[10A",
        "\x1b[10 A",
        "\x1b[10B",
        "\x1b[10C",
        "\x1b[10D",
        "\x1b[10E",
        "\x1b[10F",
        "\x1b[10G",
        "\x1b[10;10H",
        "\x1b[10I",
        "\x1b[10L",
        "\x1b[10M",
        "\x1b[10P",
        "\x1b[10S",
        "\x1b[10T",
        "\x1b[10X",
        "\x1b[10Z",
        "\x1b[10`",
        "\x1b[10a",
        "\x1b[10b",
        "\x1b[10d",
        "\x1b[10e",
        "\x1b[10;10f",
        "\x1b[0g",
        "\x1b[s",
        "\x1b[10'}",
        "\x1b[10'~",
    ];
    let mut t = at_cols_handler();
    for seq in seqs {
        t.parse_str(&format!("##########\x1b[2J{seq}"));
        assert!(t.buffer().x <= 10, "{seq:?}");
        t.handler.reset();
        t.reset_buffers();
    }
}

#[test]
fn decsca_and_decsed_decsel_default_is_unprotected() {
    let mut t = upstream_handler();
    t.parse_str("some text");
    t.parse_str("\x1b[?2K");
    assert_eq!(t.lines(2), ["", ""]);
    t.parse_str("some text");
    t.parse_str("\x1b[?2J");
    assert_eq!(t.lines(2), ["", ""]);
}

#[test]
fn decsca_and_decsed_decsel_decsca_1_with_decsel() {
    let mut t = upstream_handler();
    t.parse_str("###\x1b[1\"qlineerase\x1b[0\"q***");
    t.parse_str("\x1b[?2K");
    assert_eq!(t.lines(2), ["   lineerase", ""]);
    // normal EL works as before
    t.parse_str("\x1b[2K");
    assert_eq!(t.lines(2), ["", ""]);
}

#[test]
fn decsca_and_decsed_decsel_decsca_1_with_decsed() {
    let mut t = upstream_handler();
    t.parse_str("###\x1b[1\"qdisplayerase\x1b[0\"q***");
    t.parse_str("\x1b[?2J");
    assert_eq!(t.lines(2), ["   displayerase", ""]);
    // normal ED works as before
    t.parse_str("\x1b[2J");
    assert_eq!(t.lines(2), ["", ""]);
}

// --- added: oracle cases (expected values from @xterm/headless 6.0.0, V6 widths) ---

/// Replay `inputs` on a `cols` x `rows` terminal and compare every buffer line (`|` = row start,
/// `~` = wrapped row, right-trimmed) and `[x, y, ybase, ydisp]` with the oracle's.
fn oracle_case(
    cols: usize,
    rows: usize,
    scroll_on_erase: bool,
    inputs: &[&str],
    expect_lines: &[&str],
    expect_cursor: [usize; 4],
) {
    let options = if scroll_on_erase {
        vec![OptionValue::ScrollOnEraseInDisplay(true)]
    } else {
        Vec::new()
    };
    let mut t = test_handler(cols, rows, options);
    for input in inputs {
        t.parse_str(input);
    }
    let buffer = t.buffer();
    let lines: Vec<String> = (0..buffer.lines.len())
        .filter_map(|i| buffer.lines.get(i))
        .map(|line| {
            format!(
                "{}{}",
                if line.is_wrapped { '~' } else { '|' },
                line.translate_to_string(true, None, None, None)
            )
        })
        .collect();
    assert_eq!(lines, expect_lines);
    assert_eq!(
        [buffer.x, buffer.y, buffer.ybase, buffer.ydisp],
        expect_cursor
    );
}

#[test]
fn oracle_il_huge() {
    oracle_case(
        5,
        10,
        false,
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\r\nggggg\r\nhhhhh\r\niiiii\r\njjjjj\x1b[4;7r\x1b[6;1H\x1b[100000L",
        ],
        &[
            "|aaaaa", "|bbbbb", "|ccccc", "|ddddd", "|eeeee", "|", "|", "|hhhhh", "|iiiii",
            "|jjjjj",
        ],
        [0, 5, 0, 0],
    );
}

#[test]
fn oracle_dl_huge() {
    oracle_case(
        5,
        10,
        false,
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\r\nggggg\r\nhhhhh\r\niiiii\r\njjjjj\x1b[4;7r\x1b[5;1H\x1b[100000M",
        ],
        &[
            "|aaaaa", "|bbbbb", "|ccccc", "|ddddd", "|", "|", "|", "|hhhhh", "|iiiii", "|jjjjj",
        ],
        [0, 4, 0, 0],
    );
}

#[test]
fn oracle_su_huge() {
    oracle_case(
        5,
        10,
        false,
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\r\nggggg\r\nhhhhh\r\niiiii\r\njjjjj\x1b[4;7r\x1b[100000S",
        ],
        &[
            "|aaaaa", "|bbbbb", "|ccccc", "|", "|", "|", "|", "|hhhhh", "|iiiii", "|jjjjj",
        ],
        [0, 0, 0, 0],
    );
}

#[test]
fn oracle_sd_huge() {
    oracle_case(
        5,
        10,
        false,
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\r\nggggg\r\nhhhhh\r\niiiii\r\njjjjj\x1b[4;7r\x1b[100000T",
        ],
        &[
            "|aaaaa", "|bbbbb", "|ccccc", "|", "|", "|", "|", "|hhhhh", "|iiiii", "|jjjjj",
        ],
        [0, 0, 0, 0],
    );
}

#[test]
fn oracle_il_n() {
    oracle_case(
        5,
        10,
        false,
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\r\nggggg\r\nhhhhh\r\niiiii\r\njjjjj\x1b[4;7r\x1b[5;1H\x1b[2L",
        ],
        &[
            "|aaaaa", "|bbbbb", "|ccccc", "|ddddd", "|", "|", "|eeeee", "|hhhhh", "|iiiii",
            "|jjjjj",
        ],
        [0, 4, 0, 0],
    );
}

#[test]
fn oracle_rep_basic() {
    oracle_case(
        10,
        3,
        false,
        &["ab\x1b[3b"],
        &["|abbbb", "|", "|"],
        [5, 0, 0, 0],
    );
}

#[test]
fn oracle_rep_none_after_csi() {
    oracle_case(
        10,
        3,
        false,
        &["ab\x1b[m\x1b[3b"],
        &["|ab", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_rep_wide() {
    oracle_case(
        10,
        3,
        false,
        &["a中\x1b[2b"],
        &["|a中中中", "|", "|"],
        [7, 0, 0, 0],
    );
}

#[test]
fn oracle_rep_wrap() {
    oracle_case(
        5,
        3,
        false,
        &["abc\x1b[7b"],
        &["|abccc", "~ccccc", "|"],
        [5, 1, 0, 0],
    );
}

#[test]
fn oracle_rep_combining() {
    oracle_case(
        6,
        3,
        false,
        &["xe\u{301}\x1b[2b"],
        &["|xe\u{301}e\u{301}e\u{301}", "|", "|"],
        [4, 0, 0, 0],
    );
}

#[test]
fn oracle_rep_at_line_start() {
    oracle_case(
        4,
        3,
        false,
        &["abcd\x1b[2b"],
        &["|abcd", "~dd", "|"],
        [2, 1, 0, 0],
    );
}

#[test]
fn oracle_rep_wide_wrap_edge() {
    oracle_case(
        5,
        3,
        false,
        &["abc中\x1b[2b"],
        &["|abc中", "~中中", "|"],
        [4, 1, 0, 0],
    );
}

#[test]
fn oracle_ed3_scrollback() {
    oracle_case(
        5,
        3,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\x1b[3J"],
        &["|ddddd", "|eeeee", "|fffff"],
        [5, 2, 0, 0],
    );
}

#[test]
fn oracle_ed3_no_scrollback() {
    oracle_case(
        5,
        3,
        false,
        &["aaaaa\r\nbbbbb\x1b[3J"],
        &["|aaaaa", "|bbbbb", "|"],
        [5, 1, 0, 0],
    );
}

#[test]
fn oracle_ed1_last_col() {
    oracle_case(
        5,
        4,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\x1b[2;5H\x1b[1J"],
        &["|", "|", "|ccccc", "|ddddd"],
        [4, 1, 0, 0],
    );
}

#[test]
fn oracle_ed1_last_col_wrapped() {
    oracle_case(
        5,
        4,
        false,
        &["abcdefghijklmnopqrst\x1b[2;5H\x1b[1J"],
        &["|", "|", "|klmno", "~pqrst"],
        [4, 1, 0, 0],
    );
}

#[test]
fn oracle_ed1_at_ybase() {
    oracle_case(
        5,
        3,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\x1b[2;5H\x1b[1J"],
        &["|aaaaa", "|bbbbb", "|ccccc", "|", "|", "|fffff"],
        [4, 1, 3, 3],
    );
}

#[test]
fn oracle_ed1_abs_quirk() {
    oracle_case(
        5,
        3,
        false,
        &["aaaaabbbbbccccc\r\nddddd\r\neeeee\r\nfffff\x1b[2;5H\x1b[1J"],
        &["|aaaaa", "~bbbbb", "|ccccc", "|", "|", "|fffff"],
        [4, 1, 3, 3],
    );
}

#[test]
fn oracle_ed0_wrapped_x0() {
    oracle_case(
        5,
        4,
        false,
        &["abcdefghijklmnopqrst\x1b[2;1H\x1b[0J"],
        &["|abcde", "|", "|", "|"],
        [0, 1, 0, 0],
    );
}

#[test]
fn oracle_ed2_scroll_on_erase() {
    oracle_case(
        5,
        3,
        true,
        &["aaaaa\r\nbbbbb\x1b[2J"],
        &["|aaaaa", "|bbbbb", "|", "|", "|"],
        [5, 1, 2, 2],
    );
}

#[test]
fn oracle_ed2_scroll_on_erase_blank() {
    oracle_case(5, 3, true, &["\x1b[2J"], &["|", "|", "|"], [0, 0, 0, 0]);
}

#[test]
fn oracle_ed2_scroll_on_erase_full() {
    oracle_case(
        5,
        3,
        true,
        &["aaaaa\r\nbbbbb\r\nccccc\x1b[2J"],
        &["|aaaaa", "|bbbbb", "|ccccc", "|", "|", "|"],
        [5, 2, 3, 3],
    );
}

#[test]
fn oracle_decsed_protected() {
    oracle_case(
        6,
        3,
        false,
        &["ab\x1b[1\"qCD\x1b[0\"qef\r\ngh\x1b[?2J"],
        &["|  CD", "|", "|"],
        [2, 1, 0, 0],
    );
}

#[test]
fn oracle_decsel_1() {
    oracle_case(
        6,
        3,
        false,
        &["ab\x1b[1\"qCD\x1b[0\"qef\x1b[1;4H\x1b[?1K"],
        &["|  CDef", "|", "|"],
        [3, 0, 0, 0],
    );
}

#[test]
fn oracle_decsca_two_is_unprotect() {
    oracle_case(
        6,
        3,
        false,
        &["\x1b[1\"qA\x1b[2\"qB\x1b[?2K"],
        &["|A", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_decsca_no_param() {
    oracle_case(
        6,
        3,
        false,
        &["\x1b[1\"qA\x1b[\"qB\x1b[?2K"],
        &["|A", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_ech_wide() {
    oracle_case(
        6,
        3,
        false,
        &["a中b\x1b[1;3H\x1b[X"],
        &["|a  b", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_ech_huge() {
    oracle_case(
        6,
        3,
        false,
        &["abcdef\x1b[1;3H\x1b[2147483647X"],
        &["|ab", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_ich_huge() {
    oracle_case(
        6,
        3,
        false,
        &["abcdef\x1b[1;3H\x1b[2147483647@"],
        &["|ab", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_dch_huge() {
    oracle_case(
        6,
        3,
        false,
        &["abcdef\x1b[1;3H\x1b[2147483647P"],
        &["|ab", "|", "|"],
        [2, 0, 0, 0],
    );
}

#[test]
fn oracle_sl_sr_decic_decdc_margin_outside() {
    oracle_case(
        5,
        5,
        false,
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\x1b[2;4r\x1b[1;1H\x1b[2 @\x1b[2 A\x1b[2'}\x1b[2'~",
        ],
        &["|aaaaa", "|bbbbb", "|ccccc", "|ddddd", "|eeeee"],
        [0, 0, 0, 0],
    );
}

#[test]
fn oracle_sl_inside() {
    oracle_case(
        5,
        5,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\x1b[2;4r\x1b[3;1H\x1b[2 @"],
        &["|aaaaa", "|bbb", "|ccc", "|ddd", "|eeeee"],
        [0, 2, 0, 0],
    );
}

#[test]
fn oracle_sr_wrapped_flag() {
    oracle_case(
        5,
        3,
        false,
        &["abcdefghijkl\x1b[ A"],
        &["| abcd", "| fghi", "| kl"],
        [2, 2, 0, 0],
    );
}

#[test]
fn oracle_decaln() {
    oracle_case(
        4,
        3,
        false,
        &["ab\r\ncdefgh\x1b#8"],
        &["|EEEE", "|EEEE", "|EEEE"],
        [0, 0, 0, 0],
    );
}

#[test]
fn oracle_decaln_scrolled() {
    oracle_case(
        4,
        3,
        false,
        &["aaaa\r\nbbbb\r\ncccc\r\ndddd\r\neeee\x1b#8"],
        &["|aaaa", "|bbbb", "|EEEE", "|EEEE", "|EEEE"],
        [0, 0, 2, 2],
    );
}

#[test]
fn oracle_el_variants() {
    oracle_case(
        6,
        4,
        false,
        &["abcdef\r\nabcdef\r\nabcdef\x1b[1;3H\x1b[K\x1b[2;3H\x1b[1K\x1b[3;3H\x1b[2K"],
        &["|ab", "|   def", "|", "|"],
        [2, 2, 0, 0],
    );
}

#[test]
fn oracle_el_wrapped() {
    oracle_case(
        5,
        3,
        false,
        &["abcdefgh\x1b[2;1H\x1b[K"],
        &["|abcde", "|", "|"],
        [0, 1, 0, 0],
    );
}

#[test]
fn oracle_el_x0_unwrap() {
    oracle_case(
        5,
        3,
        false,
        &["abcdefgh\x1b[2;1H\x1b[0K"],
        &["|abcde", "|", "|"],
        [0, 1, 0, 0],
    );
}

#[test]
fn oracle_el_x_cols_pending() {
    oracle_case(
        5,
        3,
        false,
        &["abcde\x1b[1K"],
        &["|", "|", "|"],
        [5, 0, 0, 0],
    );
}

#[test]
fn oracle_ed_x_cols_pending() {
    oracle_case(
        5,
        3,
        false,
        &["abcde\r\nabcde\x1b[1;5H\x1b[0J"],
        &["|abcd", "|", "|"],
        [4, 0, 0, 0],
    );
}

#[test]
fn oracle_il_cursor_outside() {
    oracle_case(
        5,
        6,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\x1b[2;4r\x1b[6;3H\x1b[L"],
        &["|aaaaa", "|bbbbb", "|ccccc", "|ddddd", "|eeeee", "|fffff"],
        [2, 5, 0, 0],
    );
}

#[test]
fn oracle_il_sets_x0() {
    oracle_case(
        5,
        6,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\x1b[3;3H\x1b[L"],
        &["|aaaaa", "|bbbbb", "|", "|ccccc", "|ddddd", "|eeeee"],
        [0, 2, 0, 0],
    );
}

#[test]
fn oracle_dl_sets_x0() {
    oracle_case(
        5,
        6,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\x1b[3;3H\x1b[M"],
        &["|aaaaa", "|bbbbb", "|ddddd", "|eeeee", "|fffff", "|"],
        [0, 2, 0, 0],
    );
}

#[test]
fn oracle_su_scrollback() {
    oracle_case(
        5,
        4,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\x1b[2S"],
        &["|ccccc", "|ddddd", "|", "|"],
        [5, 3, 0, 0],
    );
}

#[test]
fn oracle_sd_simple() {
    oracle_case(
        5,
        4,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\x1b[2T"],
        &["|", "|", "|aaaaa", "|bbbbb"],
        [5, 3, 0, 0],
    );
}

#[test]
fn oracle_su_sd_zero_param() {
    oracle_case(
        5,
        4,
        false,
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\x1b[0S\x1b[0T\x1b[0L\x1b[0M"],
        &["|", "|bbbbb", "|ccccc", "|"],
        [0, 3, 0, 0],
    );
}

// --- added: step 5 (bounds, damage, attributes, markers, missing lines) ---

fn lettered(t: &mut TestTerm, rows: usize, cols: usize) {
    let text: Vec<String> = (0..rows)
        .map(|i| {
            char::from(b'a' + u8::try_from(i).unwrap())
                .to_string()
                .repeat(cols)
        })
        .collect();
    t.parse_str(&text.join("\r\n"));
}

/// IL, DL, SU and SD with a count far past the region equal the count of the region height
/// (upstream loops `Ps` times; the Rust loops stop at the height, `// LUNETERM:` in the
/// handlers). The count 100000 is the one the oracle cases above ran.
#[test]
fn huge_line_and_scroll_counts_equal_the_region_height() {
    for seq in ["L", "M", "S", "T"] {
        let run = |count: &str| {
            let mut t = test_handler(5, 10, []);
            lettered(&mut t, 10, 5);
            t.parse_str(&format!("\x1b[4;7r\x1b[5;1H\x1b[{count}{seq}"));
            t.snapshot()
        };
        assert_eq!(run("2147483647"), run("4"), "{seq}");
        assert_eq!(run("99999999999"), run("4"), "{seq} (past i32)");
    }
}

#[test]
fn huge_character_counts_are_clamped_by_the_buffer_line() {
    let mut t = test_handler(6, 3, []);
    t.parse_str("abcdef\x1b[1;3H\x1b[2147483647@");
    assert_eq!(t.lines(1), ["ab"]);
    t.parse_str("abcdef\x1b[1;3H\x1b[2147483647P");
    assert_eq!(t.lines(1), ["ab"]);
    t.parse_str("abcdef\x1b[1;3H\x1b[2147483647X");
    assert_eq!(t.lines(1), ["ab"]);
}

#[test]
fn rep_repeats_the_preceding_print_and_is_a_no_op_after_other_sequences() {
    let mut t = test_handler(10, 3, []);
    t.parse_str("ab\x1b[3b");
    assert_eq!(t.lines(1), ["abbbb"]);
    // any sequence in between clears the carried character
    let mut t = test_handler(10, 3, []);
    t.parse_str("ab\x1b[m\x1b[3b");
    assert_eq!(t.lines(1), ["ab"]);
    // a combined cluster and a wide char repeat as a whole
    let mut t = test_handler(6, 3, []);
    t.parse_str("xe\u{301}\x1b[2b");
    assert_eq!(t.lines(1), ["xe\u{301}e\u{301}e\u{301}"]);
    let mut t = test_handler(10, 3, []);
    t.parse_str("a中\x1b[2b");
    assert_eq!(t.lines(1), ["a中中中"]);
    // REP at the start of the buffer has nothing to repeat
    let mut t = test_handler(10, 3, []);
    t.parse_str("\x1b[3b");
    assert_eq!(t.lines(1), [""]);
}

#[test]
fn rep_with_a_huge_count_prints_a_bounded_amount() {
    let mut t = test_handler(80, 24, []);
    t.parse_str("a\x1b[2147483647b");
    let rows = t.viewport();
    assert!(rows.iter().all(|row| row.chars().all(|c| c == 'a')));
    // the cap, not the count: MAX_REP_CODE_POINTS characters after the first one
    let x = t.buffer().x;
    assert_eq!(x, (1 + MAX_REP_CODE_POINTS) % 80);
}

/// Everything REP can change, for comparing a capped REP with the uncapped print.
#[derive(Debug, PartialEq)]
struct RepState {
    lines: Vec<BufferLine>,
    cursor: [usize; 4],
    scroll: (usize, usize, bool),
    alt: bool,
    join: u32,
    damage: Damage,
    dirty: DirtyRowTracker,
    events: Vec<HostEvent>,
    links: (usize, usize),
    marker_lines: Vec<i64>,
}

fn rep_state(t: &mut TestTerm) -> RepState {
    t.handler.forward_disposed_markers();
    let events = t.handler.take_events();
    let h = &t.handler;
    let buffers = &h.services.buffer;
    let buffer = buffers.buffer();
    let mut marker_lines: Vec<i64> = buffers
        .buffers
        .normal()
        .markers()
        .iter()
        .chain(buffers.buffers.alt().markers().iter())
        .map(|m| m.line)
        .collect();
    marker_lines.sort_unstable();
    RepState {
        lines: (0..buffer.lines.len())
            .filter_map(|i| buffer.lines.get(i).cloned())
            .collect(),
        cursor: [buffer.x, buffer.y, buffer.ybase, buffer.ydisp],
        scroll: (
            buffer.scroll_top,
            buffer.scroll_bottom,
            buffers.is_user_scrolling,
        ),
        alt: buffers.buffers.is_alt_active(),
        join: h.preceding_join_state,
        damage: h.damage,
        dirty: h.dirty_row_tracker,
        events,
        links: (
            h.services.osc_link.len(),
            h.services.osc_link.marker_count(),
        ),
        marker_lines,
    }
}

/// Review S3: REP prints at most what can still change the state. For each setup, REP with a
/// count past the cut leaves exactly the state of printing all of the repetitions (the
/// uncapped path: `print` of the whole repeated data, what REP did before the cut). The cases
/// marked `fill` take the line-fill path (`print_repeated`, reviews S3 residual and S7), the
/// others `print`; each case checks which. Lines compare whole, side-table entries included.
#[test]
fn rep_cut_to_the_buffer_leaves_the_state_of_the_uncapped_print() {
    let cases: [RepCutCase; 40] = [
        ("plain", |_| {}, "a", true, true),
        ("wide, odd cols", |_| {}, "\u{4e2d}", true, true),
        ("cluster", |_| {}, "e\u{301}", true, true),
        ("wide cluster", |_| {}, "\u{4e2d}\u{301}", true, true),
        (
            "insert mode",
            |t| t.parse_str("xyz\r\n12345\x1b[H\x1b[4h"),
            "a",
            true,
            true,
        ),
        (
            "insert mode, wide cluster over clusters and wides",
            |t| {
                t.parse_str(
                    "e\u{301}\u{4e2d}e\u{301}x\r\n\u{4e2d}\u{301}e\u{301}e\u{301}\x1b[H\x1b[4h",
                )
            },
            "\u{4e2d}\u{301}",
            true,
            true,
        ),
        (
            "insert mode, cluster below the region",
            |t| t.parse_str("1\r\n2\r\n3\x1b[1;2r\x1b[3;2H\x1b[4h"),
            "e\u{301}",
            true,
            true,
        ),
        (
            "insert mode, underline style over link cells",
            |t| {
                t.parse_str(
                    "\x1b]8;;u\x07e\u{301}y\u{4e2d}z\r\n123\x1b]8;;\x07\x1b[4:3m\x1b[H\x1b[4h",
                )
            },
            "a",
            true,
            true,
        ),
        (
            "region, cursor inside",
            |t| t.parse_str("1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[2;3H"),
            "a",
            true,
            true,
        ),
        (
            "below the region",
            |t| t.parse_str("1\r\n2\r\n3\x1b[1;2r\x1b[3;2H"),
            "a",
            true,
            true,
        ),
        (
            "no wraparound",
            |t| t.parse_str("\x1b[?7l"),
            "a",
            true,
            false,
        ),
        (
            "no wraparound, wide",
            |t| t.parse_str("\x1b[?7l"),
            "\u{4e2d}",
            true,
            false,
        ),
        (
            "no wraparound, cluster",
            |t| t.parse_str("\x1b[?7l"),
            "e\u{301}",
            true,
            false,
        ),
        (
            "no wraparound, wide cluster",
            |t| t.parse_str("\x1b[?7l"),
            "\u{4e2d}\u{301}",
            true,
            false,
        ),
        (
            "no wraparound, insert mode, cluster",
            |t| t.parse_str("xyz\x1b[?7l\x1b[4h\x1b[H"),
            "e\u{301}",
            true,
            false,
        ),
        (
            "alt buffer",
            |t| t.parse_str("1\r\n2\x1b[?1049h"),
            "a",
            true,
            true,
        ),
        (
            "open link",
            |t| t.parse_str("\x1b]8;;u\x07"),
            "a",
            true,
            true,
        ),
        (
            "open link, insert mode, wide cluster",
            |t| t.parse_str("xyz\r\n12\x1b]8;;u\x07\x1b[H\x1b[4h"),
            "\u{4e2d}\u{301}",
            true,
            true,
        ),
        (
            "user scrolled",
            |t| {
                t.parse_str(&"x\r\n".repeat(9));
                t.handler.services.buffer.scroll_lines(-2, false);
            },
            "a",
            true,
            true,
        ),
        (
            "screen reader, DEC graphics",
            |t| {
                t.set_option(OptionValue::ScreenReaderMode(true));
                t.parse_str("\x1b(0");
            },
            "q",
            true,
            true,
        ),
        (
            "screen reader, cluster",
            |t| t.set_option(OptionValue::ScreenReaderMode(true)),
            "e\u{301}",
            true,
            true,
        ),
        // review S7 round 4: extended attributes, batched per line
        (
            "underline colour",
            |t| t.parse_str("\x1b[4:3;58:2::1:2:3m"),
            "a",
            true,
            true,
        ),
        (
            "underline colour, insert mode, open link, cluster",
            |t| t.parse_str("xyz\r\n12\x1b[4:3;58:5:9m\x1b]8;;u\x07\x1b[H\x1b[4h"),
            "e\u{301}",
            true,
            true,
        ),
        // (M7 fix round 1, review S1) a joiner that widens a narrow base: cut and filled
        ("joiner that widens", with_vs16, "a\u{fe0f}", true, true),
        (
            "joiner that widens, then a joiner",
            with_vs16,
            "a\u{fe0f}\u{301}",
            true,
            true,
        ),
        ("11-vs16, VS16", with_11_vs16, VS16, true, true),
        ("11-vs16, keycap", with_11_vs16, KEYCAP, true, true),
        ("11-vs16, emoji ZWJ", with_11_vs16, EMOJI_ZWJ, true, true),
        (
            "11-vs16, insert mode, VS16 over clusters and wides",
            |t| {
                with_11_vs16(t);
                t.parse_str("e\u{301}\u{4e2d}e\u{301}x\r\n\u{4e2d}\u{301}e\u{301}\u{26a0}\u{fe0f}\x1b[H\x1b[4h")
            },
            VS16,
            true,
            true,
        ),
        (
            "11-vs16, insert mode, keycap below the region",
            |t| {
                with_11_vs16(t);
                t.parse_str("1\r\n2\r\n3\x1b[1;2r\x1b[3;1H\x1b[4h")
            },
            KEYCAP,
            true,
            true,
        ),
        (
            "11-vs16, open link, VS16",
            |t| {
                with_11_vs16(t);
                t.parse_str("\x1b]8;;u\x07")
            },
            VS16,
            true,
            true,
        ),
        (
            "11-vs16, open link, insert mode, emoji ZWJ",
            |t| {
                with_11_vs16(t);
                t.parse_str("xyz\r\n12\x1b]8;;u\x07\x1b[H\x1b[4h")
            },
            EMOJI_ZWJ,
            true,
            true,
        ),
        (
            "11-vs16, screen reader, keycap",
            |t| {
                with_11_vs16(t);
                t.set_option(OptionValue::ScreenReaderMode(true));
            },
            KEYCAP,
            true,
            true,
        ),
        (
            "11-vs16, underline colour, insert mode, open link, VS16",
            |t| {
                with_11_vs16(t);
                t.parse_str("xyz\r\n12\x1b[4:3;58:5:9m\x1b]8;;u\x07\x1b[H\x1b[4h")
            },
            VS16,
            true,
            true,
        ),
        (
            "11-vs16, no wraparound, VS16",
            |t| {
                with_11_vs16(t);
                t.parse_str("\x1b[?7l")
            },
            VS16,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, keycap",
            |t| {
                with_11_vs16(t);
                t.parse_str("\x1b[?7l")
            },
            KEYCAP,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, insert mode, VS16",
            |t| {
                with_11_vs16(t);
                t.parse_str("xyz\x1b[?7l\x1b[4h\x1b[H")
            },
            VS16,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, insert mode, keycap",
            |t| {
                with_11_vs16(t);
                t.parse_str("x\u{4e2d}z\x1b[?7l\x1b[4h\x1b[H")
            },
            KEYCAP,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, emoji ZWJ",
            |t| {
                with_11_vs16(t);
                t.parse_str("\x1b[?7l")
            },
            EMOJI_ZWJ,
            true,
            false,
        ),
        // repetitions that alternate are not alike: no cut, no fill
        ("alternating widths", with_alternating_b, "b", false, false),
    ];
    // sizes (cols, rows, scrollback) and counts around and past the cut; at 300x100 counts that
    // fill, wrap and scroll the screen (the line-fill path of review S3)
    // (code review F10) by default the small sizes stop at 9008 and 300x100 runs a handful of
    // cases at three counts; with LUNE_VT_REP_MATRIX=full every case runs at every size and
    // count
    let full = std::env::var("LUNE_VT_REP_MATRIX").is_ok_and(|v| v == "full");
    let all_small = [40, 97, 211, 9_007, 9_008, 20_011];
    let small = if full {
        &all_small[..]
    } else {
        &all_small[..5]
    };
    let large = [97, 9_008, 26_000, 40_000];
    let sizes: [RepCutSize; 4] = [
        (7, 3, 4, small),
        (6, 4, 0, small),
        (2, 1, 0, small),
        (9, 2, 7, small),
    ];
    let mut stuck = Vec::new();
    let mut skipped = check_rep_cut(&cases, &sizes, CUT_AT, &mut stuck);
    // plan M7 step 13: only the no-wrap insert cases of a narrow base batch their stuck
    // inserts (M7 fix round 1: widened ones too), at every small size (at 300 columns only
    // with the full matrix, below 300 units not)
    let batching = [
        "no wraparound, insert mode, cluster",
        "11-vs16, no wraparound, insert mode, VS16",
        "11-vs16, no wraparound, insert mode, keycap",
    ];
    for (name, label, batches) in &stuck {
        // (the keycap's joiner stacks onto the cell left of the cursor until its string is
        // full, about 60 units, before the batch)
        let count: usize = label.rsplit(' ').next().unwrap().parse().unwrap();
        if name.ends_with("keycap") && batching.contains(name) && count <= 200 {
            assert!(*batches <= 1, "{label}: stuck path");
            continue;
        }
        let expected = batching.contains(name) && !label.contains(" 300x");
        if !batching.contains(name) || expected {
            assert_eq!(*batches, u64::from(expected), "{label}: stuck path");
        }
    }
    let large_cases = [
        "plain",
        "insert mode",
        "below the region",
        "open link",
        "underline colour",
    ];
    let (large_cases, large): (Vec<RepCutCase>, &[usize]) = if full {
        (cases.to_vec(), &large)
    } else {
        let cases = cases.iter().filter(|case| large_cases.contains(&case.0));
        (cases.copied().collect(), &[97, 9_008, 40_000])
    };
    assert_eq!(large_cases.len(), if full { 40 } else { 5 });
    skipped.extend(check_rep_cut(
        &large_cases,
        &[(300, 100, 5, large)],
        CUT_AT,
        &mut Vec::new(),
    ));
    // only the five-byte units at the largest count
    assert_eq!(skipped.len(), if full { 5 } else { 0 }, "{skipped:?}");
}

/// Where the REP cut tests check whether the cut applies: 7x3+4, REP 9007.
const CUT_AT: (usize, usize, usize, usize) = (7, 3, 4, 9_007);

/// A case of the REP cut tests: name, setup, the unit, whether the cut applies (checked at
/// one size and count) and whether REP takes the line-fill path.
type RepCutCase = (&'static str, fn(&mut TestTerm), &'static str, bool, bool);
/// Columns, rows, scrollback and the REP counts to compare at that size.
type RepCutSize<'a> = (usize, usize, usize, &'a [usize]);

/// For every case, size and count: REP leaves the state of the uncapped print of all of its
/// repetitions. Returns the labels whose uncapped print would not fit one `print` call, and
/// adds every run's case name, label and batched stuck inserts (plan M7 step 13) to `stuck`.
fn check_rep_cut(
    cases: &[RepCutCase],
    sizes: &[RepCutSize],
    cut_at: (usize, usize, usize, usize),
    stuck: &mut Vec<(&'static str, String, u64)>,
) -> Vec<String> {
    let mut skipped = Vec::new();
    for &(name, setup, unit, cut, fill) in cases {
        for &(cols, rows, scrollback, counts) in sizes {
            for &count in counts {
                let make = || {
                    let mut t = test_handler(cols, rows, [OptionValue::Scrollback(scrollback)]);
                    setup(&mut t);
                    t.parse_str(unit);
                    t
                };
                let label = format!("{name} at {cols}x{rows}+{scrollback}, REP {count}");
                // the cut REP
                let mut capped = make();
                let repeated = {
                    let buffer = capped.buffer();
                    let width = UnicodeService::extract_width(capped.handler.preceding_join_state);
                    let x = buffer.x.saturating_sub(width as usize);
                    buffer.lines.get(abs_y(buffer)).unwrap().get_string(x)
                };
                let units: Vec<u32> = repeated.chars().map(u32::from).collect();
                let effective = capped.handler.rep_equivalent_repeats(&units, count);
                assert!(effective <= count, "{label}");
                if (cols, rows, scrollback, count) == cut_at {
                    assert_eq!(effective < count, cut, "{label}: {effective} of {count}");
                }
                capped.parse_str(&format!("\x1b[{count}b"));
                assert_eq!(capped.handler.rep_fills, u64::from(fill), "{label}: path");
                stuck.push((name, label.clone(), capped.handler.rep_stuck_batches));
                // the uncapped print of every repetition: one `print` call with all of them
                // (one parse chunk), then a CSI with no handler, which ends the print as REP's
                // CSI does
                let mut uncapped = make();
                let text = repeated.repeat(count);
                if text.len() >= MAX_PARSEBUFFER_LENGTH {
                    // the uncapped print would not be one `print` call
                    skipped.push(label);
                    continue;
                }
                uncapped.parse_str(&format!("{text}\x1b[}}"));
                assert_eq!(rep_state(&mut capped), rep_state(&mut uncapped), "{label}");
            }
        }
    }
    skipped
}

/// Code review F9: every line of the buffer made `len` cells long (`kind` 0: `cols + 1`, 1:
/// `3 * cols`, 2: past what the buffer's own lines and rows shift out, `cols * (max_lines +
/// rows + 8) + 1`), as a column shrink without reflow leaves them; lettered by column so a
/// shift of the hidden tail shows, with a cluster, a wide character and an underline-coloured
/// cell past the screen's columns where they fit; then `modes`.
fn longer_lines(t: &mut TestTerm, kind: usize, modes: &str) {
    use crate::buffer::constants::BgFlags;
    let mut coloured = AttributeData::default();
    coloured.bg |= BgFlags::HAS_EXTENDED;
    coloured.extended = ExtendedAttrs::new(0x0040_0000, 0);
    let cols = t.handler.services.buffer.cols;
    let rows = t.handler.services.buffer.rows;
    let buffer = t.handler.services.buffer.buffer_mut();
    let len = match kind {
        0 => cols + 1,
        1 => 3 * cols,
        _ => cols * (buffer.lines.max_length() + rows + 8) + 1,
    };
    for y in 0..buffer.lines.len() {
        let mut line = BufferLine::new(len, None, false);
        for c in 0..len {
            let letter = u32::from(b'A') + (c % 26) as u32;
            line.set_cell_from_codepoint(c, letter, 1, &DEFAULT_ATTR_DATA);
        }
        line.add_codepoint_to_cell(cols, 0x301, 1);
        if len >= cols + 4 {
            line.set_cell_from_codepoint(cols + 1, u32::from('\u{4e2d}'), 2, &DEFAULT_ATTR_DATA);
            line.set_cell_from_codepoint(cols + 2, 0, 0, &DEFAULT_ATTR_DATA);
            line.set_cell_from_codepoint(len - 1, u32::from('z'), 1, &coloured);
        }
        buffer.lines.set(y, Some(line));
    }
    t.parse_str(modes);
}

/// Code review F9: the REP cut in insert mode on lines longer than `cols`, where every insert
/// shifts the hidden tail: below a scroll region (every wrap rewrites the last row) and
/// without wraparound (every unit inserts at the cursor), for one code point, a cluster, a
/// wide character and a wide cluster, and three line lengths; at the scroll bottom the
/// scrolled-in lines are `cols` long (control).
#[test]
fn rep_cut_on_lines_longer_than_cols_leaves_the_state_of_the_uncapped_print() {
    const BELOW: &str = "\x1b[1;2r\x1b[999;1H\x1b[4h";
    const NO_WRAP: &str = "\x1b[?7l\x1b[999;1H\x1b[4h";
    let cases: [RepCutCase; 21] = [
        (
            "below the region, len cols + 1",
            |t| longer_lines(t, 0, BELOW),
            "a",
            true,
            true,
        ),
        (
            "below the region, len 3 x cols",
            |t| longer_lines(t, 1, BELOW),
            "a",
            true,
            true,
        ),
        (
            "below the region, longest",
            |t| longer_lines(t, 2, BELOW),
            "a",
            true,
            true,
        ),
        (
            "below the region, cluster, longest",
            |t| longer_lines(t, 2, BELOW),
            "e\u{301}",
            true,
            true,
        ),
        (
            "below the region, wide, longest",
            |t| longer_lines(t, 2, BELOW),
            "\u{4e2d}",
            true,
            true,
        ),
        (
            "below the region, open link, wide cluster, 3 x cols",
            |t| longer_lines(t, 1, &format!("\x1b]8;;u\x07{BELOW}")),
            "\u{4e2d}\u{301}",
            true,
            true,
        ),
        (
            "below the region, underline colour, longest",
            |t| longer_lines(t, 2, &format!("\x1b[4:3;58:5:9m{BELOW}")),
            "a",
            true,
            true,
        ),
        (
            "no wraparound, len cols + 1",
            |t| longer_lines(t, 0, NO_WRAP),
            "a",
            true,
            false,
        ),
        (
            "no wraparound, len 3 x cols",
            |t| longer_lines(t, 1, NO_WRAP),
            "a",
            true,
            false,
        ),
        (
            "no wraparound, longest",
            |t| longer_lines(t, 2, NO_WRAP),
            "a",
            true,
            false,
        ),
        (
            "no wraparound, cluster from the second column, longest",
            |t| longer_lines(t, 2, "\x1b[?7l\x1b[4h\x1b[1;2H"),
            "e\u{301}",
            true,
            false,
        ),
        (
            "no wraparound, wide, longest",
            |t| longer_lines(t, 2, NO_WRAP),
            "\u{4e2d}",
            true,
            false,
        ),
        (
            "no wraparound, wide cluster, 3 x cols",
            |t| longer_lines(t, 1, NO_WRAP),
            "\u{4e2d}\u{301}",
            true,
            false,
        ),
        (
            "at the scroll bottom, longest",
            |t| longer_lines(t, 2, "\x1b[999;1H\x1b[4h"),
            "a",
            true,
            true,
        ),
        // M7 fix round 1: a joiner that widens a narrow base, and a wide base with a joiner
        (
            "11-vs16, below the region, VS16, longest",
            |t| {
                with_11_vs16(t);
                longer_lines(t, 2, BELOW)
            },
            VS16,
            true,
            true,
        ),
        (
            "11-vs16, below the region, open link, keycap, 3 x cols",
            |t| {
                with_11_vs16(t);
                longer_lines(t, 1, &format!("\x1b]8;;u\x07{BELOW}"))
            },
            KEYCAP,
            true,
            true,
        ),
        (
            "11-vs16, below the region, emoji ZWJ, longest",
            |t| {
                with_11_vs16(t);
                longer_lines(t, 2, BELOW)
            },
            EMOJI_ZWJ,
            true,
            true,
        ),
        (
            "11-vs16, no wraparound, VS16, longest",
            |t| {
                with_11_vs16(t);
                longer_lines(t, 2, NO_WRAP)
            },
            VS16,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, VS16 from the second column, len cols + 1",
            |t| {
                with_11_vs16(t);
                // (at 2 columns the second is the last, where VS16 does not fit)
                let column = if t.handler.services.buffer.cols > 2 {
                    2
                } else {
                    1
                };
                longer_lines(t, 0, &format!("\x1b[?7l\x1b[4h\x1b[1;{column}H"))
            },
            VS16,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, keycap, longest",
            |t| {
                with_11_vs16(t);
                longer_lines(t, 2, NO_WRAP)
            },
            KEYCAP,
            true,
            false,
        ),
        (
            "11-vs16, no wraparound, underline colour, emoji ZWJ, 3 x cols",
            |t| {
                with_11_vs16(t);
                longer_lines(t, 1, &format!("\x1b[4:3;58:5:9m{NO_WRAP}"))
            },
            EMOJI_ZWJ,
            true,
            false,
        ),
    ];
    // counts below and past the floors of the longest lines (at 7x3+4 below the region the
    // floor is 224, and was 91 before review F9)
    let counts = [40, 211, 1_009];
    let sizes: [RepCutSize; 5] = [
        (7, 3, 4, &counts),
        (6, 4, 0, &counts),
        (2, 1, 0, &counts),
        (9, 2, 7, &counts),
        (10, 3, 0, &counts),
    ];
    let mut stuck = Vec::new();
    let mut skipped = check_rep_cut(&cases, &sizes, (7, 3, 4, 1_009), &mut stuck);
    // the screen reader floor (2048) comes first
    let screen_reader: [RepCutCase; 1] = [(
        "no wraparound, screen reader, open link, longest",
        |t| {
            t.set_option(OptionValue::ScreenReaderMode(true));
            longer_lines(t, 2, &format!("\x1b]8;;u\x07{NO_WRAP}"));
        },
        "a",
        true,
        false,
    )];
    let sizes: [RepCutSize; 2] = [(7, 3, 4, &[2_503]), (10, 3, 0, &[2_503])];
    skipped.extend(check_rep_cut(
        &screen_reader,
        &sizes,
        (7, 3, 4, 2_503),
        &mut stuck,
    ));
    assert!(skipped.is_empty(), "{skipped:?}");
    // plan M7 step 13: which no-wrap insert cases batch their stuck inserts: the narrow units
    // (one code point and a cluster) at every size and count; a stuck wide unit is skipped
    // before its insert (nothing to batch) and the other cases wrap
    let batched = [
        "no wraparound, len cols + 1",
        "no wraparound, len 3 x cols",
        "no wraparound, longest",
        "no wraparound, cluster from the second column, longest",
        "no wraparound, screen reader, open link, longest",
        "11-vs16, no wraparound, VS16, longest",
        "11-vs16, no wraparound, VS16 from the second column, len cols + 1",
    ];
    let mut seen = 0;
    for (name, label, batches) in &stuck {
        if name.ends_with("keycap, longest") {
            // the keycap's last joiner joins the cell left of the cursor, which may take up to
            // about 60 units through `print` to settle (its string filling up) before the
            // batch
            let count: usize = label.rsplit(' ').next().unwrap().parse().unwrap();
            assert!(*batches <= 1, "{label}: stuck path");
            assert!(count <= 200 || *batches == 1, "{label}: stuck path");
            continue;
        }
        let expected = batched.contains(name);
        seen += usize::from(expected);
        assert_eq!(*batches, u64::from(expected), "{label}: stuck path");
    }
    assert_eq!(seen, 6 * 5 * 3 + 2, "batched runs");
}

/// Plan M7 step 13 (S7 residual 5), timing-free: a no-wrap insert REP of a narrow unit on a
/// line longer than `cols` shifts the line at most `cols + 3` times through `print` (the units
/// until the cursor sticks at the last column) and once batched, whatever the line's length;
/// a REP that starts stuck, twice. Before, every unit up to the F9 floor shifted it once:
/// `len - cols` more passes.
#[test]
fn rep_no_wrap_insert_on_a_long_line_makes_a_bounded_number_of_insert_passes() {
    use crate::buffer::buffer_line::insert_passes;
    const NO_WRAP: &str = "\x1b[?7l\x1b[999;1H\x1b[4h";
    for (cols, kind) in [(7, 0), (7, 1), (7, 2), (10, 2), (80, 1), (80, 2)] {
        // (M7 fix round 1) and a joiner that widens a narrow base, stuck at `cols - 1`
        for unit in ["a", "e\u{301}", VS16] {
            let label = format!("{unit:?} at {cols} cols, line kind {kind}");
            let mut t = test_handler(cols, 3, [OptionValue::Scrollback(4)]);
            with_11_vs16(&mut t);
            longer_lines(&mut t, kind, NO_WRAP);
            let len = t.buffer().lines.get(abs_y(t.buffer())).unwrap().len();
            assert!(len > cols, "{label}");
            t.parse_str(unit);
            let before = insert_passes();
            t.parse_str("\x1b[1048576b");
            let passes = insert_passes() - before;
            assert!(
                passes <= cols as u64 + 4,
                "{label}: {passes} passes over {len} cells"
            );
            assert_eq!(t.handler.rep_stuck_batches, 1, "{label}: stuck path");
            let stuck_x = if unit == VS16 { cols - 1 } else { cols };
            assert_eq!(t.buffer().x, stuck_x, "{label}");
            if unit == VS16 {
                // (a stuck VS16 leaves only its base in the last cell, with the join state of
                // two cells: a REP after it repeats the cell two columns back)
                continue;
            }
            // stuck already: one unit through `print`, the rest batched
            t.parse_str(unit);
            let before = insert_passes();
            t.parse_str("\x1b[1048576b");
            let passes = insert_passes() - before;
            assert!(passes <= 2, "{label}, stuck: {passes} passes");
            assert_eq!(t.handler.rep_stuck_batches, 2, "{label}: stuck path");
        }
    }
}

/// Review S7 (cases from the round-3 code review's probe, plus the S7 variants): the line-fill
/// path leaves the state of `print` over content already on screen (rows of wide chars at even
/// and odd offsets, links, colours), with non-ASCII charsets, a pending wrap,
/// insert mode, an open link, clusters, screen reader mode, lines longer than `cols` (insert
/// mode's fix-up at `cols - 1`, review S7 round 4) and extended attributes (batched per line,
/// round 4). Counts below the cut and one past a screen at the larger size.
#[test]
fn rep_fill_leaves_the_state_of_print_over_existing_content() {
    use crate::buffer::constants::BgFlags;
    type Setup = fn(&mut TestTerm, usize, usize);
    fn fill_wides(t: &mut TestTerm, cols: usize, rows: usize, pre: &str) {
        let mut s = String::new();
        for r in 0..rows {
            s.push_str(&format!("\x1b[{};1H{}", r + 1, pre));
            let mut w = pre.chars().count();
            while w + 2 <= cols {
                s.push('\u{4e2d}');
                w += 2;
            }
        }
        s.push_str("\x1b[H");
        t.parse_str(&s);
    }
    fn long_lines(t: &mut TestTerm, cols: usize, rows: usize) {
        fill_wides(t, cols, rows, "x");
        let buffer = t.handler.services.buffer.buffer_mut();
        for y in 0..rows {
            let mut line = BufferLine::new(cols + 2, None, false);
            line.copy_cells_from(buffer.lines.get(y).unwrap(), 0, 0, cols, false);
            line.set_cell_from_codepoint(cols, u32::from('\u{4e2d}'), 2, &DEFAULT_ATTR_DATA);
            buffer.lines.set(y, Some(line));
        }
        t.parse_str("\x1b[4h");
    }
    // (review S7 round 4) lines 5 cells longer than `cols`, as a column shrink without reflow
    // leaves them, with wides on screen after `pre` and, past the screen's columns, a wide,
    // a cluster and an underline-coloured wide; insert mode is left to the case
    fn long_lines_of(t: &mut TestTerm, cols: usize, rows: usize, pre: &str) {
        fill_wides(t, cols, rows, pre);
        let mut coloured = AttributeData::default();
        coloured.bg |= BgFlags::HAS_EXTENDED;
        coloured.extended = ExtendedAttrs::new(0x0040_0000, 0);
        let buffer = t.handler.services.buffer.buffer_mut();
        for y in 0..rows {
            let mut line = BufferLine::new(cols + 5, None, false);
            line.copy_cells_from(buffer.lines.get(y).unwrap(), 0, 0, cols, false);
            line.set_cell_from_codepoint(cols, u32::from('\u{4e2d}'), 2, &DEFAULT_ATTR_DATA);
            line.set_cell_from_codepoint(cols + 1, 0, 0, &DEFAULT_ATTR_DATA);
            line.set_cell_from_codepoint(cols + 2, u32::from('e'), 1, &coloured);
            line.add_codepoint_to_cell(cols + 2, 0x301, 1);
            line.set_cell_from_codepoint(cols + 3, u32::from('\u{4e2d}'), 2, &coloured);
            line.set_cell_from_codepoint(cols + 4, 0, 0, &coloured);
            buffer.lines.set(y, Some(line));
        }
    }
    let cases: [(&str, Setup, &str); 42] = [
        // (a sequence between the unit and REP ends the print: REP repeats nothing)
        ("uk charset", |t, _, _| t.parse_str("\x1b(A"), "#"),
        ("over even wides", |t, c, r| fill_wides(t, c, r, ""), "a"),
        ("over odd wides", |t, c, r| fill_wides(t, c, r, "x"), "a"),
        (
            "wide over odd wides",
            |t, c, r| fill_wides(t, c, r, "x"),
            "\u{4e2d}",
        ),
        (
            "wide over even wides",
            |t, c, r| fill_wides(t, c, r, ""),
            "\u{4e2d}",
        ),
        ("dec graphics", |t, _, _| t.parse_str("\x1b(0"), "q"),
        (
            "pending wrap",
            |t, c, _| t.parse_str(&"y".repeat(c - 1)),
            "a",
        ),
        (
            "bg colour",
            |t, c, r| {
                t.parse_str("\x1b[41m");
                fill_wides(t, c, r, "x")
            },
            "a",
        ),
        (
            "insert over odd wides",
            |t, c, r| {
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b[4h");
            },
            "a",
        ),
        (
            "insert wide over odd wides, bg colour",
            |t, c, r| {
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b[42m\x1b[4h");
            },
            "\u{4e2d}",
        ),
        (
            "insert cluster over even wides",
            |t, c, r| {
                fill_wides(t, c, r, "");
                t.parse_str("\x1b[4h");
            },
            "e\u{301}",
        ),
        (
            "link over odd wides",
            |t, c, r| {
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07");
            },
            "a",
        ),
        (
            "insert under a new link over link cells",
            |t, c, r| {
                t.parse_str("\x1b]8;;old\x07");
                fill_wides(t, c, r, "e\u{301}");
                t.parse_str("\x1b]8;;new\x07\x1b[4:3m\x1b[4h");
            },
            "\u{4e2d}\u{301}",
        ),
        (
            "link cluster, pending wrap",
            |t, c, _| t.parse_str(&format!("{}\x1b]8;;u\x07", "y".repeat(c - 1))),
            "e\u{301}",
        ),
        (
            "screen reader over odd wides",
            |t, c, r| {
                t.set_option(OptionValue::ScreenReaderMode(true));
                fill_wides(t, c, r, "x");
            },
            "a",
        ),
        (
            "link wide over odd wides",
            |t, c, r| {
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07");
            },
            "\u{4e2d}",
        ),
        ("insert into longer lines", long_lines, "a"),
        ("insert wide into longer lines", long_lines, "\u{4e2d}"),
        // review S7 round 4: lines longer than `cols` in insert mode take the batched
        // inserts, with `print`'s fix-up of a wide cell at `cols - 1`
        (
            "insert into longer lines with entries, odd wides",
            |t, c, r| {
                long_lines_of(t, c, r, "x");
                t.parse_str("\x1b[4h");
            },
            "a",
        ),
        (
            "insert wide into longer lines with entries, even wides",
            |t, c, r| {
                long_lines_of(t, c, r, "");
                t.parse_str("\x1b[4h");
            },
            "\u{4e2d}",
        ),
        (
            "insert cluster into longer lines, below the region",
            |t, c, r| {
                long_lines_of(t, c, r, "x");
                t.parse_str("\x1b[1;2r\x1b[999;1H\x1b[4h");
            },
            "e\u{301}",
        ),
        (
            "insert wide cluster into longer lines, underline colour, open link",
            |t, c, r| {
                long_lines_of(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07\x1b[4:3;58:5:9m\x1b[4h");
            },
            "\u{4e2d}\u{301}",
        ),
        (
            "insert into longer lines, underline colour, below the region",
            |t, c, r| {
                long_lines_of(t, c, r, "");
                t.parse_str("\x1b[4:3;58:2::1:2:3m\x1b[1;2r\x1b[999;1H\x1b[4h");
            },
            "a",
        ),
        // review S7 round 4: extended attributes take one batched insert per line
        (
            "underline colour over odd wides",
            |t, c, r| {
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b[4:3;58:5:9m");
            },
            "a",
        ),
        (
            "underline colour wide cluster under a link over coloured cells",
            |t, c, r| {
                t.parse_str("\x1b[58:5:3m");
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07\x1b[4:2m");
            },
            "\u{4e2d}\u{301}",
        ),
        (
            "insert, underline colour over coloured cells",
            |t, c, r| {
                t.parse_str("\x1b[4:3m");
                fill_wides(t, c, r, "e\u{301}");
                t.parse_str("\x1b[58:5:1m\x1b[4h");
            },
            "a",
        ),
        (
            "insert wide, underline colour, below the region over coloured cells",
            |t, c, r| {
                t.parse_str("\x1b[4:5m");
                fill_wides(t, c, r, "");
                t.parse_str("\x1b[58:5:2m\x1b[1;2r\x1b[999;1H\x1b[4h");
            },
            "\u{4e2d}",
        ),
        (
            "underline colour cluster, pending wrap",
            |t, c, _| t.parse_str(&format!("{}\x1b[4:3;58:5:9m", "y".repeat(c - 1))),
            "e\u{301}",
        ),
        // M7 fix round 1: a joiner that widens a narrow base (VS16 under `11-vs16`, alone and
        // followed by a joiner) and a wide base with a joiner (an emoji and a ZWJ)
        (
            "VS16 over odd wides",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "x")
            },
            VS16,
        ),
        (
            "VS16 over even wides",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "")
            },
            VS16,
        ),
        (
            "keycap over odd wides",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "x")
            },
            KEYCAP,
        ),
        (
            "emoji ZWJ over odd wides",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "x")
            },
            EMOJI_ZWJ,
        ),
        (
            "VS16, base on the last column",
            |t, c, r| {
                with_11_vs16(t);
                // (with one row and no scrollback, the wrap's carry finds the old row
                // recycled and drops the base: the REP unit is then no VS16 at all)
                if r > 1 {
                    t.parse_str(&"y".repeat(c - 1));
                }
            },
            VS16,
        ),
        (
            "insert VS16 over odd wides",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b[4h");
            },
            VS16,
        ),
        (
            "insert keycap over even wides, bg colour",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "");
                t.parse_str("\x1b[42m\x1b[4h");
            },
            KEYCAP,
        ),
        (
            "link VS16 over odd wides",
            |t, c, r| {
                with_11_vs16(t);
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07");
            },
            VS16,
        ),
        (
            "screen reader keycap over odd wides",
            |t, c, r| {
                with_11_vs16(t);
                t.set_option(OptionValue::ScreenReaderMode(true));
                fill_wides(t, c, r, "x");
            },
            KEYCAP,
        ),
        (
            "insert VS16 into longer lines with entries, odd wides",
            |t, c, r| {
                with_11_vs16(t);
                long_lines_of(t, c, r, "x");
                t.parse_str("\x1b[4h");
            },
            VS16,
        ),
        (
            "insert VS16 into longer lines, below the region",
            |t, c, r| {
                with_11_vs16(t);
                long_lines_of(t, c, r, "x");
                t.parse_str("\x1b[1;2r\x1b[999;1H\x1b[4h");
            },
            VS16,
        ),
        (
            "insert keycap into longer lines, underline colour, open link",
            |t, c, r| {
                with_11_vs16(t);
                long_lines_of(t, c, r, "");
                t.parse_str("\x1b]8;;u\x07\x1b[4:3;58:5:9m\x1b[4h");
            },
            KEYCAP,
        ),
        (
            "insert emoji ZWJ into longer lines, below the region, open link",
            |t, c, r| {
                with_11_vs16(t);
                long_lines_of(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07\x1b[1;2r\x1b[999;1H\x1b[4h");
            },
            EMOJI_ZWJ,
        ),
        (
            "underline colour VS16 under a link over coloured cells",
            |t, c, r| {
                with_11_vs16(t);
                t.parse_str("\x1b[58:5:3m");
                fill_wides(t, c, r, "x");
                t.parse_str("\x1b]8;;u\x07\x1b[4:2m");
            },
            VS16,
        ),
    ];
    // (3 columns hold one wide character per line)
    let small = [(7, 3, 4), (8, 3, 0), (2, 1, 0), (5, 2, 3), (3, 3, 2)];
    // (code review F10) by default the small sizes stop at 9008 (past every cut, the screen
    // reader floor included) and 300x100 runs a handful of cases at three counts; with
    // LUNE_VT_REP_MATRIX=full every case runs at every size and count
    let full = std::env::var("LUNE_VT_REP_MATRIX").is_ok_and(|v| v == "full");
    let all_counts: &[usize] = &[1, 2, 3, 5, 6, 7, 13, 40, 97, 9_008, 30_001];
    let counts = if full { all_counts } else { &all_counts[..10] };
    let large_cases = [
        "over odd wides",
        "insert over odd wides",
        "link over odd wides",
        "insert into longer lines with entries, odd wides",
        "underline colour over odd wides",
        "VS16 over odd wides",
        "insert VS16 over odd wides",
        "link VS16 over odd wides",
    ];
    let large_counts: &[usize] = if full {
        all_counts
    } else {
        &[97, 9_008, 30_001]
    };
    let (mut runs, mut filled) = (0, 0);
    for (name, setup, unit) in cases {
        let large = (full || large_cases.contains(&name)).then_some(((300, 100, 5), large_counts));
        let sizes = small.iter().map(|&size| (size, counts)).chain(large);
        for ((cols, rows, scrollback), counts) in sizes {
            for &count in counts {
                let make = || {
                    let mut t = test_handler(cols, rows, [OptionValue::Scrollback(scrollback)]);
                    setup(&mut t, cols, rows);
                    t.parse_str(unit);
                    t
                };
                let label = format!("{name} at {cols}x{rows}+{scrollback}, REP {count}");
                let mut fill = make();
                let fits = fill.handler.preceding_join_state != 0
                    && UnicodeService::extract_width(fill.handler.preceding_join_state) as usize
                        <= cols;
                fill.parse_str(&format!("\x1b[{count}b"));
                assert_eq!(fill.handler.rep_fills, u64::from(fits), "{label}: path");
                runs += 1;
                filled += usize::from(fits);
                let mut printed = make();
                let text = {
                    let buffer = printed.buffer();
                    let width = UnicodeService::extract_width(printed.handler.preceding_join_state);
                    let x = buffer.x.saturating_sub(width as usize);
                    buffer.lines.get(abs_y(buffer)).unwrap().get_string(x)
                };
                let text = text.repeat(count);
                if text.len() >= MAX_PARSEBUFFER_LENGTH {
                    continue;
                }
                printed.parse_str(&format!("{text}\x1b[}}"));
                assert_eq!(rep_state(&mut fill), rep_state(&mut printed), "{label}");
            }
        }
    }
    // every case takes the fill path, at every size and count of the matrix
    let expected = if full {
        42 * 6 * 11
    } else {
        42 * 5 * 10 + large_cases.len() * 3
    };
    assert_eq!(runs, expected, "matrix");
    assert_eq!(filled, runs, "fill path cases");
}

/// M7 fix round 1 (review S1 and the wide cluster REP), seeded: REP of wide and widened units
/// (VS16, a keycap, an emoji and a ZWJ, a wide cluster) and of narrow ones, after random
/// content and modes (insert, wraparound off, scroll regions, an open link, an underline
/// colour, screen reader, lines longer than `cols`), at random small sizes and counts
/// around and past the cuts, leaves the state of the uncapped print of all its repetitions.
/// `LUNE_VT_REP_DIFF_SEEDS=<n>` runs more seeds (default 1000), `LUNE_VT_REP_DIFF_SEED=<n>`
/// replays one.
#[test]
fn rep_of_wide_and_widened_units_leaves_the_state_of_the_uncapped_print_seeded() {
    const UNITS: [&str; 9] = [
        VS16,
        KEYCAP,
        EMOJI_ZWJ,
        "a\u{fe0f}",
        "\u{2764}\u{fe0f}\u{301}",
        "\u{4e2d}\u{301}",
        "\u{4e2d}",
        "e\u{301}",
        "a",
    ];
    const CONTENT: [&str; 22] = [
        "x",
        "\u{4e2d}",
        "e\u{301}",
        VS16,
        KEYCAP,
        EMOJI_ZWJ,
        "\r\n",
        "\x1b[4h",
        "\x1b[4l",
        "\x1b[?7l",
        "\x1b[?7h",
        "\x1b]8;;u\x07",
        "\x1b]8;;\x07",
        "\x1b[4:3;58:5:9m",
        "\x1b[0m",
        "\x1b[1;2r",
        "\x1b[r",
        "\x1b[999;1H",
        "\x1b[H",
        "\x1b[2;3H",
        "\x1b[1;2H",
        "yyyyyyy",
    ];
    let seeds: Vec<u64> = match std::env::var("LUNE_VT_REP_DIFF_SEED") {
        Ok(seed) => vec![seed.parse().unwrap()],
        Err(_) => {
            let n = std::env::var("LUNE_VT_REP_DIFF_SEEDS")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(1_000);
            (1..=n).collect()
        }
    };
    let (mut filled, mut batched, mut compared) = (0, 0, 0);
    for seed in seeds {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        let cols = 2 + next(11);
        let rows = 1 + next(5);
        let scrollback = next(7);
        let screen_reader = next(6) == 0;
        let long = next(4);
        let mut setup = String::new();
        for _ in 0..next(14) {
            setup.push_str(CONTENT[next(CONTENT.len())]);
        }
        let unit = UNITS[next(UNITS.len())];
        let count = match next(4) {
            0 => [97, 211, 1_009, 3_001, 9_008][next(5)],
            _ => 1 + next(60),
        };
        let make = || {
            let mut t = test_handler(cols, rows, [OptionValue::Scrollback(scrollback)]);
            with_11_vs16(&mut t);
            if screen_reader {
                t.set_option(OptionValue::ScreenReaderMode(true));
            }
            if long < 2 {
                longer_lines(&mut t, long, "");
            }
            t.parse_str(&setup);
            t.parse_str(unit);
            t
        };
        let label = format!(
            "seed {seed}: {cols}x{rows}+{scrollback}, long {long}, screen reader \
             {screen_reader}, {setup:?} then {unit:?}, REP {count}"
        );
        let mut capped = make();
        capped.parse_str(&format!("\x1b[{count}b"));
        filled += capped.handler.rep_fills;
        batched += capped.handler.rep_stuck_batches;
        // the uncapped print: what REP repeats (nothing after no print, or where the cell
        // would start left of the line), `count` times in one `print` call
        let mut uncapped = make();
        let text = {
            let join = uncapped.handler.preceding_join_state;
            let width = UnicodeService::extract_width(join) as usize;
            let buffer = uncapped.buffer();
            match buffer.x.checked_sub(width).filter(|_| join != 0) {
                Some(x) => buffer.lines.get(abs_y(buffer)).unwrap().get_string(x),
                None => String::new(),
            }
        };
        let text = text.repeat(count);
        assert!(text.len() < MAX_PARSEBUFFER_LENGTH, "{label}");
        uncapped.parse_str(&format!("{text}\x1b[}}"));
        assert_eq!(rep_state(&mut capped), rep_state(&mut uncapped), "{label}");
        compared += 1;
    }
    if std::env::var("LUNE_VT_REP_DIFF_SEED").is_err() {
        // the line fill and the batched stuck inserts both ran
        assert!(filled > compared as u64 / 4, "{filled} of {compared}");
        assert!(batched > 0, "{batched} of {compared}");
    }
}

#[test]
fn rep_with_the_cursor_column_past_the_line_does_not_panic() {
    let mut t = test_handler(5, 3, []);
    t.parse_str("abcde\x1b[3b");
    assert_eq!(t.lines(2), ["abcde", "eee"]);
}

#[test]
fn erase_handlers_fill_with_the_erase_attributes() {
    let mut t = test_handler(6, 3, []);
    t.parse_str("abcdef\r\nabcdef\r\nabcdef");
    t.handler.cur_attr_data.bg = BLUE;
    t.parse_str("\x1b[1;2H\x1b[2X\x1b[2;2H\x1b[K\x1b[3;3H\x1b[1K");
    assert_eq!(t.cell(0, 1), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(0, 2), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(0, 3).0, "d");
    assert_eq!(t.cell(1, 0).0, "a");
    assert_eq!(t.cell(1, 1), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(1, 5), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(2, 2), (String::new(), 1, 0, BLUE));
    assert_eq!(t.cell(2, 3).0, "d");
    // ED 2 and IL use them too
    t.parse_str("\x1b[2J");
    assert_eq!(t.cell(1, 0), (String::new(), 1, 0, BLUE));
    t.parse_str("x\x1b[L");
    assert_eq!(t.cell(0, 0), (String::new(), 1, 0, BLUE));
}

#[test]
fn scroll_down_blank_lines_use_the_default_attributes() {
    // upstream passes DEFAULT_ATTR_DATA for SD (not the erase attributes)
    let mut t = test_handler(4, 3, []);
    t.parse_str("a\r\nb\r\nc");
    t.handler.cur_attr_data.bg = BLUE;
    t.parse_str("\x1b[T");
    assert_eq!(t.cell(0, 0), (String::new(), 1, 0, 0));
    t.parse_str("\x1b[S");
    assert_eq!(t.cell(2, 0), (String::new(), 1, 0, BLUE));
}

#[test]
fn decsca_sets_and_clears_the_protected_flag() {
    let mut t = test_handler(10, 3, []);
    let protected = |t: &TestTerm| t.handler.get_attr_data().bg & BgFlags::PROTECTED;
    t.parse_str("\x1b[1\"q");
    assert_ne!(protected(&t), 0);
    t.parse_str("\x1b[0\"q");
    assert_eq!(protected(&t), 0);
    t.parse_str("\x1b[1\"q\x1b[2\"q");
    assert_eq!(protected(&t), 0);
    // another value changes nothing
    t.parse_str("\x1b[1\"q\x1b[7\"q");
    assert_ne!(protected(&t), 0);
}

#[test]
fn selective_erase_keeps_protected_cells_for_every_variant() {
    let mut t = test_handler(6, 3, []);
    t.parse_str("ab\x1b[1\"qCD\x1b[0\"qef\r\ngh\x1b[1\"qIJ\x1b[0\"q");
    // DECSEL 1, 0 and 2
    t.parse_str("\x1b[1;4H\x1b[?1K");
    assert_eq!(t.lines(1), ["  CDef"]);
    t.parse_str("\x1b[?0K");
    assert_eq!(t.lines(1), ["  CD"]);
    t.parse_str("\x1b[?2K");
    assert_eq!(t.lines(1), ["  CD"]);
    // DECSED 0 from the first row keeps CD and IJ
    t.parse_str("\x1b[1;1H\x1b[?0J");
    assert_eq!(t.lines(2), ["  CD", "  IJ"]);
    // DECSED 1 up to the second row
    t.parse_str("\x1b[2;6H\x1b[?1J");
    assert_eq!(t.lines(2), ["  CD", "  IJ"]);
    // DECSED 3 is the ordinary ED 3
    t.parse_str("\x1b[?3J");
    assert_eq!(t.lines(2), ["  CD", "  IJ"]);
    // plain ED ignores the flag
    t.parse_str("\x1b[2J");
    assert_eq!(t.lines(2), ["", ""]);
}

#[test]
fn ed_3_clears_the_scrollback_and_reports_scroll_damage() {
    let mut t = test_handler(5, 3, []);
    lettered(&mut t, 6, 5);
    t.handler.take_damage();
    assert_eq!((t.buffer().ybase, t.buffer().lines.len()), (3, 6));
    t.parse_str("\x1b[3J");
    assert_eq!(
        (t.buffer().ybase, t.buffer().ydisp, t.buffer().lines.len()),
        (0, 0, 3)
    );
    assert_eq!(t.viewport(), ["d", "e", "f"].map(|c| c.repeat(5)));
    assert!(t.handler.take_damage().scrolled);
    // with no scrollback there is nothing to clear and no scroll damage
    t.parse_str("\x1b[3J");
    assert!(!t.handler.take_damage().scrolled);
}

#[test]
fn ed_2_with_scroll_on_erase_in_display_reports_scroll_damage() {
    let mut t = test_handler(5, 3, [OptionValue::ScrollOnEraseInDisplay(true)]);
    t.parse_str("ab");
    t.handler.take_damage();
    t.parse_str("\x1b[2J");
    let damage = t.handler.take_damage();
    assert!(damage.scrolled);
    assert_eq!((t.buffer().ybase, t.buffer().lines.len()), (1, 4));
    // a blank screen scrolls nothing
    t.parse_str("\x1b[2J");
    assert_eq!((t.buffer().ybase, t.buffer().lines.len()), (1, 4));
}

#[test]
fn erasing_lines_disposes_their_markers() {
    let mut t = test_handler(5, 4, []);
    lettered(&mut t, 4, 5);
    let on_row_1 = t.buffer_mut().add_marker(1);
    let on_row_3 = t.buffer_mut().add_marker(3);
    t.parse_str("\x1b[3;1H\x1b[1J");
    // ED 1 resets rows 0 and 1; the marker on row 3 stays
    assert!(t.buffer().marker(on_row_1).is_none());
    assert_eq!(t.buffer().marker(on_row_3).map(|m| m.line), Some(3));
    // IL moves the marker below the cursor down
    t.parse_str("\x1b[1;1H\x1b[L");
    assert_eq!(
        t.buffer().marker(on_row_3).map(|m| m.line),
        None,
        "bottom line lost"
    );
}

#[test]
fn decaln_fills_the_viewport_with_e_in_the_current_colours() {
    let mut t = test_handler(4, 3, []);
    t.parse_str("ab\r\ncdefgh");
    assert!(
        t.buffer().lines.get(2).is_some_and(|line| line.is_wrapped),
        "precondition"
    );
    t.handler.cur_attr_data.fg = RED;
    t.handler.cur_attr_data.bg = BLUE;
    t.parse_str("\x1b#8");
    assert_eq!(t.viewport(), ["EEEE"; 3]);
    assert_eq!(t.cell(0, 0), ("E".into(), 1, RED, BLUE));
    assert_eq!(t.cell(2, 3), ("E".into(), 1, RED, BLUE));
    assert_eq!(t.cursor(), [0, 0]);
    // the wrapped flag of the continuation line is cleared
    assert!(t.buffer().lines.get(2).is_some_and(|line| !line.is_wrapped));
}

#[test]
fn decaln_follows_the_viewport_after_scrolling() {
    let mut t = test_handler(4, 3, []);
    lettered(&mut t, 5, 4);
    t.parse_str("\x1b#8");
    assert_eq!(t.lines(5), ["aaaa", "bbbb", "EEEE", "EEEE", "EEEE"]);
}

#[test]
fn new_handlers_survive_a_missing_cursor_line() {
    // the recipe of `missing_cursor_line_stops_the_handler_without_panic`
    let mut t = test_handler(97, 9, [OptionValue::Scrollback(0)]);
    for _ in 0..8 {
        t.parse_str(&"a".repeat(97));
        t.parse_str("\r\n");
    }
    t.parse_str(&"a".repeat(10));
    t.resize(23, 15);
    let buffer = t.buffer();
    assert!(buffer.lines.get(abs_y(buffer)).is_none(), "recipe");
    for seq in [
        "\x1b[@",
        "\x1b[P",
        "\x1b[X",
        "\x1b[K",
        "\x1b[1K",
        "\x1b[2K",
        "\x1b[J",
        "\x1b[1J",
        "\x1b[2J",
        "\x1b[3J",
        "\x1b[?J",
        "\x1b[?K",
        "\x1b[L",
        "\x1b[M",
        "\x1b[S",
        "\x1b[T",
        "\x1b[ @",
        "\x1b[ A",
        "\x1b['}",
        "\x1b['~",
        "\x1b[b",
        "\x1b#8",
        "\x1b[1\"q",
    ] {
        t.parse_str(&format!("\x1b[15;1H{seq}"));
    }
    t.parse_str("\x1b[1;1Hok");
    assert_eq!(
        t.viewport().first().and_then(|row| row.get(..2)),
        Some("ok")
    );
}

// --- step 6: SGR (CSI m) ---

/// `inputHandler.curAttrData` after parsing `\x1b[` + `seq` on a fresh handler.
fn sgr(seq: &str) -> TestTerm {
    let mut t = upstream_handler();
    t.parse_str(&format!("\x1b[{seq}"));
    t
}

impl TestTerm {
    fn attr(&self) -> AttributeData {
        self.handler.cur_attr_data
    }
}

// upstream: `text attributes`

#[test]
fn text_attributes_bold() {
    let mut t = sgr("1m");
    assert!(t.attr().is_bold() != 0);
    t.parse_str("\x1b[22m");
    assert!(t.attr().is_bold() == 0);
}

#[test]
fn text_attributes_dim() {
    let mut t = sgr("2m");
    assert!(t.attr().is_dim() != 0);
    t.parse_str("\x1b[22m");
    assert!(t.attr().is_dim() == 0);
}

#[test]
fn text_attributes_italic() {
    let mut t = sgr("3m");
    assert!(t.attr().is_italic() != 0);
    t.parse_str("\x1b[23m");
    assert!(t.attr().is_italic() == 0);
}

#[test]
fn text_attributes_underline() {
    let mut t = sgr("4m");
    assert!(t.attr().is_underline() != 0);
    t.parse_str("\x1b[24m");
    assert!(t.attr().is_underline() == 0);
}

#[test]
fn text_attributes_blink() {
    let mut t = sgr("5m");
    assert!(t.attr().is_blink() != 0);
    t.parse_str("\x1b[25m");
    assert!(t.attr().is_blink() == 0);
}

#[test]
fn text_attributes_inverse() {
    let mut t = sgr("7m");
    assert!(t.attr().is_inverse() != 0);
    t.parse_str("\x1b[27m");
    assert!(t.attr().is_inverse() == 0);
}

#[test]
fn text_attributes_invisible() {
    let mut t = sgr("8m");
    assert!(t.attr().is_invisible() != 0);
    t.parse_str("\x1b[28m");
    assert!(t.attr().is_invisible() == 0);
}

#[test]
fn text_attributes_strikethrough() {
    let mut t = sgr("9m");
    assert!(t.attr().is_strikethrough() != 0);
    t.parse_str("\x1b[29m");
    assert!(t.attr().is_strikethrough() == 0);
}

#[test]
fn text_attributes_overline() {
    let mut t = sgr("53m");
    assert!(t.attr().is_overline() != 0);
    t.parse_str("\x1b[55m");
    assert!(t.attr().is_overline() == 0);
}

#[test]
fn colormode_palette_16() {
    let mut t = upstream_handler();
    assert_eq!(t.attr().get_fg_color_mode(), 0);
    assert_eq!(t.attr().get_bg_color_mode(), 0);
    // lower 8 colors
    for i in 0..8 {
        t.parse_str(&format!("\x1b[{};{}m", i + 30, i + 40));
        assert_eq!(t.attr().get_fg_color_mode(), Attributes::CM_P16);
        assert_eq!(t.attr().get_fg_color(), i);
        assert_eq!(t.attr().get_bg_color_mode(), Attributes::CM_P16);
        assert_eq!(t.attr().get_bg_color(), i);
    }
    // reset to DEFAULT
    t.parse_str("\x1b[39;49m");
    assert_eq!(t.attr().get_fg_color_mode(), 0);
    assert_eq!(t.attr().get_bg_color_mode(), 0);
}

#[test]
fn colormode_palette_256() {
    let mut t = upstream_handler();
    assert_eq!(t.attr().get_fg_color_mode(), 0);
    assert_eq!(t.attr().get_bg_color_mode(), 0);
    for i in 0..256 {
        t.parse_str(&format!("\x1b[38;5;{i};48;5;{i}m"));
        assert_eq!(t.attr().get_fg_color_mode(), Attributes::CM_P256);
        assert_eq!(t.attr().get_fg_color(), i);
        assert_eq!(t.attr().get_bg_color_mode(), Attributes::CM_P256);
        assert_eq!(t.attr().get_bg_color(), i);
    }
    // reset to DEFAULT
    t.parse_str("\x1b[39;49m");
    assert_eq!(t.attr().get_fg_color_mode(), 0);
    assert_eq!(t.attr().get_fg_color(), -1);
    assert_eq!(t.attr().get_bg_color_mode(), 0);
    assert_eq!(t.attr().get_bg_color(), -1);
}

#[test]
fn colormode_rgb() {
    let mut t = upstream_handler();
    t.parse_str("\x1b[38;2;1;2;3;48;2;4;5;6m");
    assert_eq!(t.attr().get_fg_color_mode(), Attributes::CM_RGB);
    assert_eq!(t.attr().get_fg_color(), 1 << 16 | 2 << 8 | 3);
    assert_eq!(AttributeData::to_color_rgb(t.attr().fg), [1, 2, 3]);
    assert_eq!(t.attr().get_bg_color_mode(), Attributes::CM_RGB);
    assert_eq!(AttributeData::to_color_rgb(t.attr().bg), [4, 5, 6]);
    // reset to DEFAULT
    t.parse_str("\x1b[39;49m");
    assert_eq!(t.attr().get_fg_color_mode(), 0);
    assert_eq!(t.attr().get_fg_color(), -1);
    assert_eq!(t.attr().get_bg_color_mode(), 0);
    assert_eq!(t.attr().get_bg_color(), -1);
}

#[test]
fn colormode_transitions() {
    // RGB -> 256, RGB -> 16, 16 -> 256, 256 -> 16
    for (first, second, mode, color) in [
        (
            "\x1b[38;2;1;2;3;48;2;4;5;6m",
            "\x1b[38;5;255;48;5;255m",
            Attributes::CM_P256,
            255,
        ),
        (
            "\x1b[38;2;1;2;3;48;2;4;5;6m",
            "\x1b[37;47m",
            Attributes::CM_P16,
            7,
        ),
        (
            "\x1b[37;47m",
            "\x1b[38;5;255;48;5;255m",
            Attributes::CM_P256,
            255,
        ),
        (
            "\x1b[38;5;255;48;5;255m",
            "\x1b[37;47m",
            Attributes::CM_P16,
            7,
        ),
    ] {
        let mut t = upstream_handler();
        t.parse_str(first);
        t.parse_str(second);
        assert_eq!(t.attr().get_fg_color_mode(), mode);
        assert_eq!(t.attr().get_fg_color(), color);
        assert_eq!(t.attr().get_bg_color_mode(), mode);
        assert_eq!(t.attr().get_bg_color(), color);
    }
}

#[test]
fn should_zero_missing_rgb_values() {
    let mut t = upstream_handler();
    t.parse_str("\x1b[38;2;1;2;3m");
    t.parse_str("\x1b[38;2;5m");
    assert_eq!(AttributeData::to_color_rgb(t.attr().fg), [5, 0, 0]);
}

// upstream: `colon notation`

/// Upstream's two handlers with `fg = 0xFFFFFFFF`: `colon` and `semicolon` give the same `fg`,
/// and `fg & mask == expected`.
fn colon_equals_semicolon(colon: &str, semicolon: &str, mask: u32, expected: u32) {
    let mut a = upstream_handler();
    let mut b = upstream_handler();
    a.handler.cur_attr_data.fg = 0xFFFF_FFFF;
    b.handler.cur_attr_data.fg = 0xFFFF_FFFF;
    b.parse_str(&format!("\x1b[{semicolon}m"));
    a.parse_str(&format!("\x1b[{colon}m"));
    assert_eq!(b.attr().fg & mask, expected, "{semicolon}");
    assert_eq!(a.attr().fg, b.attr().fg, "{colon} vs {semicolon}");
}

#[test]
fn colon_notation_equals_semicolon() {
    let rgb = Attributes::RGB_MASK;
    colon_equals_semicolon(
        "38:2::50:100:150",
        "38;2;50;100;150",
        rgb,
        50 << 16 | 100 << 8 | 150,
    );
    colon_equals_semicolon("38:2::50:100:", "38;2;50;100;", rgb, 50 << 16 | 100 << 8);
    colon_equals_semicolon("38:2::50::", "38;2;50;;", rgb, 50 << 16);
    colon_equals_semicolon("38:2::::", "38;2;;;", rgb, 0);
    colon_equals_semicolon(
        "38;2::50:100:150",
        "38;2;50;100;150",
        rgb,
        50 << 16 | 100 << 8 | 150,
    );
    colon_equals_semicolon(
        "38;2;50:100:150",
        "38;2;50;100;150",
        rgb,
        50 << 16 | 100 << 8 | 150,
    );
    colon_equals_semicolon(
        "38;2;50;100:150",
        "38;2;50;100;150",
        rgb,
        50 << 16 | 100 << 8 | 150,
    );
    colon_equals_semicolon("38:5:50", "38;5;50", 0xFF, 50);
    colon_equals_semicolon("38:5:", "38;5;", 0xFF, 0);
    colon_equals_semicolon("38;5:50", "38;5;50", 0xFF, 50);
}

#[test]
fn colon_notation_fills_an_early_sequence_end_with_zero() {
    colon_equals_semicolon("38:2", "38;2", Attributes::RGB_MASK, 0);
    colon_equals_semicolon("38:5", "38;5", 0xFF, 0);
}

#[test]
fn colon_notation_does_not_interfere_with_leading_and_following_attributes() {
    for (colon, semicolon, rgb) in [
        (
            "1;38:2::50:100:150;4",
            "1;38;2;50;100;150;4",
            50 << 16 | 100 << 8 | 150,
        ),
        ("1;38:2::50:100:;4", "1;38;2;50;100;;4", 50 << 16 | 100 << 8),
        ("1;38:2::50:100;4", "1;38;2;50;100;;4", 50 << 16 | 100 << 8),
        ("1;38:2::;4", "1;38;2;;;;4", 0),
        ("1;38;2::;4", "1;38;2;;;;4", 0),
    ] {
        let a = sgr(&format!("{colon}m"));
        let b = sgr(&format!("{semicolon}m"));
        assert!(b.attr().is_bold() != 0 && b.attr().is_underline() != 0);
        assert_eq!(b.attr().fg & Attributes::RGB_MASK, rgb, "{semicolon}");
        assert_eq!(a.attr().fg, b.attr().fg, "{colon}");
    }
}

// upstream: `reset text attributes (SGR 0)`

#[test]
fn sgr0_resets_all_attributes_if_there_is_no_url() {
    let mut t = sgr("30m\x1b[40m\x1b[4m");
    assert_ne!(t.attr().fg, 0);
    assert_ne!(t.attr().bg, 0);
    assert!(!t.attr().extended.is_empty());
    t.parse_str("\x1b[m");
    assert_eq!(t.attr().fg, 0);
    assert_eq!(t.attr().bg, 0);
    assert!(t.attr().extended.is_empty());
}

#[test]
fn sgr0_resets_all_attributes_except_for_the_url() {
    let mut t = sgr("30m\x1b[40m\x1b[4m");
    t.parse_str("\x1b]8;;http://example.com\x1b\\");
    assert_ne!(t.attr().fg, 0);
    assert_ne!(t.attr().bg, 0);
    assert_ne!(t.attr().extended.ext(), 0);
    let url_id = t.attr().extended.url_id();
    assert_ne!(url_id, 0);
    t.parse_str("\x1b[m");
    assert_eq!(t.attr().fg, 0);
    assert_eq!(t.attr().bg, BgFlags::HAS_EXTENDED);
    let mut expected = ExtendedAttrs::default();
    expected.set_url_id(url_id);
    assert_eq!(t.attr().extended, expected);
    // also through the multi-param path
    t.parse_str("\x1b[1;4;0;3m");
    assert_eq!(t.attr().extended.url_id(), url_id);
    assert!(t.attr().is_italic() != 0 && t.attr().is_bold() == 0);
}

// upstream: `extended underline style support (SGR 4)`

#[test]
fn extended_underline_style_roundtrips() {
    let style = |t: &TestTerm| t.attr().get_underline_style();
    let mut t = upstream_handler();
    t.resize(10, 5);
    t.parse_str("\x1b[4m");
    assert_eq!(style(&t), UnderlineStyle::SINGLE);
    t.parse_str("\x1b[24m");
    assert_eq!(style(&t), UnderlineStyle::NONE);
    t.parse_str("\x1b[21m");
    assert_eq!(style(&t), UnderlineStyle::DOUBLE);
    t.parse_str("\x1b[24m");
    assert_eq!(style(&t), UnderlineStyle::NONE);
    for (n, expected) in [
        (1, UnderlineStyle::SINGLE),
        (2, UnderlineStyle::DOUBLE),
        (3, UnderlineStyle::CURLY),
        (4, UnderlineStyle::DOTTED),
        (5, UnderlineStyle::DASHED),
    ] {
        t.parse_str(&format!("\x1b[4:{n}m"));
        assert_eq!(style(&t), expected, "4:{n}");
        t.parse_str("\x1b[4:0m");
        assert_eq!(style(&t), UnderlineStyle::NONE, "4:{n} | 4:0");
        t.parse_str(&format!("\x1b[4:{n}m"));
        assert_eq!(style(&t), expected, "4:{n} again");
        t.parse_str("\x1b[24m");
        assert_eq!(style(&t), UnderlineStyle::NONE, "4:{n} | 24");
    }
}

#[test]
fn extended_underline_style_4_after_4_x_reverts_to_single() {
    let mut t = sgr("4:5m");
    assert_eq!(t.attr().get_underline_style(), UnderlineStyle::DASHED);
    t.parse_str("\x1b[4m");
    assert_eq!(t.attr().get_underline_style(), UnderlineStyle::SINGLE);
}

// upstream: `underline colors (SGR 58 & SGR 59)`

fn assert_underline_color_follows_fg(a: &AttributeData) {
    assert_eq!(a.get_underline_color(), a.get_fg_color());
    assert_eq!(a.get_underline_color_mode(), a.get_fg_color_mode());
    assert_eq!(a.is_underline_color_rgb(), a.is_fg_rgb());
    assert_eq!(a.is_underline_color_palette(), a.is_fg_palette());
    assert_eq!(a.is_underline_color_default(), a.is_fg_default());
}

fn assert_underline_color(a: &AttributeData, color: i32, mode: u32) {
    assert_eq!(a.get_underline_color(), color);
    assert_eq!(a.get_underline_color_mode(), mode);
    assert_eq!(a.is_underline_color_rgb(), mode == Attributes::CM_RGB);
    assert_eq!(a.is_underline_color_palette(), mode == Attributes::CM_P256);
    assert!(!a.is_underline_color_default());
}

#[test]
fn underline_colors_default_to_the_fg_color() {
    let mut t = upstream_handler();
    t.resize(10, 5);
    for s in ["", "\x1b[30m", "\x1b[38;510m", "\x1b[38;2;1;2;3m"] {
        t.parse_str(s);
        assert_underline_color_follows_fg(&t.attr());
    }
}

#[test]
fn underline_colors_set_p256_and_rgb() {
    let mut t = upstream_handler();
    t.resize(10, 5);
    t.parse_str("\x1b[4m");
    t.parse_str("\x1b[58;5;123m");
    assert_underline_color(&t.attr(), 123, Attributes::CM_P256);
    t.parse_str("\x1b[58;2::1:2:3m");
    assert_underline_color(&t.attr(), 1 << 16 | 2 << 8 | 3, Attributes::CM_RGB);
}

#[test]
fn underline_colors_persist_in_cells() {
    let mut t = upstream_handler();
    t.resize(10, 5);
    let mut cell = CellData::new();
    let mut load = |t: &TestTerm, x: usize| {
        t.buffer().lines.get(0).unwrap().load_cell(x, &mut cell);
        *cell
    };
    t.parse_str("\x1b[4m");
    t.parse_str("\x1b[58;5;123m");
    assert_underline_color(&t.attr(), 123, Attributes::CM_P256);
    t.parse_str("ab");
    assert_underline_color(&load(&t, 1), 123, Attributes::CM_P256);

    t.parse_str("\x1b[4:0m");
    assert_underline_color_follows_fg(&t.attr());
    t.parse_str("a");
    assert_underline_color(&load(&t, 1), 123, Attributes::CM_P256);
    assert_underline_color_follows_fg(&load(&t, 2));

    t.parse_str("\x1b[4m");
    t.parse_str("\x1b[58;2::1:2:3m");
    assert_underline_color(&t.attr(), 1 << 16 | 2 << 8 | 3, Attributes::CM_RGB);
    t.parse_str("a");
    t.parse_str("\x1b[24m");
    assert_underline_color(&load(&t, 1), 123, Attributes::CM_P256);
    assert_underline_color(&load(&t, 3), 1 << 16 | 2 << 8 | 3, Attributes::CM_RGB);

    // upstream compares object identity of the extended attrs of cells 0/1 and 1/3: values here
    assert_eq!(load(&t, 0).extended, load(&t, 1).extended);
    // no extended attrs were written for cell 2
    assert_eq!(load(&t, 2).bg & BgFlags::HAS_EXTENDED, 0);
    assert_ne!(load(&t, 1).extended, load(&t, 3).extended);
}

// added: SGR 59, 58 quirks and handler behaviour

#[test]
fn sgr_59_resets_the_underline_colour() {
    let mut t = sgr("4;58;5;9m");
    assert_underline_color(&t.attr(), 9, Attributes::CM_P256);
    t.parse_str("\x1b[59m");
    // upstream stores -1, which the setter masks to all colour bits: the colour then reads as
    // RGB 0xFFFFFF, not as "follow fg" (`~underlineColor` is non-zero for every stored value);
    // @xterm/headless 6.0.0 gives the same `_ext` (see `sgr_matches_the_oracle`).
    assert_eq!(t.attr().extended.underline_color(), 0x3FF_FFFF);
    assert_underline_color(&t.attr(), 0xFF_FFFF, Attributes::CM_RGB);
    assert_eq!(t.attr().get_underline_style(), UnderlineStyle::SINGLE);
}

#[test]
fn sgr_58_does_not_update_has_extended_by_itself() {
    // upstream's `_extractColor` writes the colour without `updateExtended` (kept)
    let t = sgr("58;5;9m");
    assert_eq!(t.attr().bg & BgFlags::HAS_EXTENDED, 0);
    assert_eq!(t.attr().extended.underline_color(), Attributes::CM_P256 | 9);
}

#[test]
fn single_sgr0_and_sgr0_in_a_list_agree() {
    let setup = "1;3;4:3;38;2;1;2;3;48;5;7;58;5;9";
    let mut a = sgr(&format!("{setup}m"));
    let mut b = sgr(&format!("{setup}m"));
    a.parse_str("\x1b[0m");
    b.parse_str("\x1b[0;0m");
    assert_eq!(a.attr(), b.attr());
    assert_eq!(a.attr(), AttributeData::default());
    a.parse_str("\x1b[m");
    assert_eq!(a.attr(), AttributeData::default());
}

#[test]
fn sgr_writes_nothing_but_the_current_attributes() {
    let mut t = upstream_handler();
    t.parse_str("ab");
    let before = t.snapshot();
    t.parse_str("\x1b[1;31;38;2;1;2;3;4:3;58;5;9;0m");
    assert_eq!(t.snapshot(), before);
}

#[test]
fn unknown_sgr_values_are_logged_and_ignored() {
    use crate::services::log_service::Logger;
    use crate::services::options_service::LogLevel;
    struct Sink(Rc<RefCell<Vec<String>>>);
    impl Logger for Sink {
        fn log(&self, _level: LogLevel, message: &str) {
            self.0.borrow_mut().push(message.to_owned());
        }
    }
    let messages = Rc::new(RefCell::new(Vec::new()));
    let mut t = upstream_handler();
    t.handler
        .services
        .log
        .set_logger(Some(Box::new(Sink(messages.clone()))));
    t.handler.services.log.set_level(LogLevel::Debug);
    t.parse_str("\x1b[10;56;60;108;200m");
    assert_eq!(t.attr(), AttributeData::default());
    let logged = messages.borrow().join("|");
    for n in [10, 56, 60, 108, 200] {
        assert!(
            logged.contains(&format!("Unknown SGR attribute: {n}.")),
            "{logged}"
        );
    }
}

/// Expected values from `@xterm/headless` 6.0.0: `[fg, bg, extended._ext]` of the current
/// attributes (as unsigned 32-bit values) after `ESC [ <seq>` on a fresh terminal. The run
/// exercises `_extractColor`'s index walk (semicolon / colon mixes, early ends, the sixth-param
/// edge, the 32-param limit), the underline style defaults and unknown values.
#[test]
fn sgr_matches_the_oracle() {
    #[rustfmt::skip]
    let cases: &[(&str, u32, u32, u32)] = &[
        ("1;31;42;3;4;5;7;8;9;2;53m", 0xFD000001, 0x5D000002, 0x04000000),
        ("38;2;1;2;3;48;5;9m", 0x03010203, 0x02000009, 0x00000000),
        ("38;5m", 0x02000000, 0x00000000, 0x00000000),
        ("38m", 0x00000000, 0x00000000, 0x00000000),
        ("48m", 0x00000000, 0x00000000, 0x00000000),
        ("38;2m", 0x03000000, 0x00000000, 0x00000000),
        ("38;2;1m", 0x03010000, 0x00000000, 0x00000000),
        ("38;2;1;2m", 0x03010200, 0x00000000, 0x00000000),
        ("38;5;1;2;3m", 0x02000001, 0x0C000000, 0x00000000),
        ("38:5:1:2:3m", 0x02000001, 0x00000000, 0x00000000),
        ("38:2:1:2:3:4m", 0x03020304, 0x00000000, 0x00000000),
        ("38:2::1:2:3:4m", 0x03010203, 0x00000000, 0x00000000),
        ("48:2:1:2:3:4m", 0x00000000, 0x03020304, 0x00000000),
        ("38;0;1;2;3;4;5m", 0x00000000, 0x00000000, 0x00000000),
        ("38;0;0;0;0;0:7m", 0x00000000, 0x00000000, 0x00000000),
        ("38;0;0;0;0;0:7:8:9m", 0x00000000, 0x00000000, 0x00000000),
        ("38;0;0;0;0;1m", 0x00000000, 0x00000000, 0x00000000),
        ("38;1;2;3;4;5;6m", 0x00000000, 0x00000000, 0x00000000),
        ("38;-1m", 0x00000000, 0x00000000, 0x00000000),
        ("38;5;300m", 0x0200002C, 0x00000000, 0x00000000),
        ("38:5:300m", 0x0200002C, 0x00000000, 0x00000000),
        ("38;2;300;400;500m", 0x032C90F4, 0x00000000, 0x00000000),
        ("38:2::300:400:500m", 0x032C90F4, 0x00000000, 0x00000000),
        ("38;5;1;48;5;2;58;5;3m", 0x02000001, 0x02000002, 0x02000003),
        ("58;5;1m", 0x00000000, 0x00000000, 0x02000001),
        ("58;2;1;2;3m", 0x00000000, 0x00000000, 0x03010203),
        ("58:2::1:2:3m", 0x00000000, 0x00000000, 0x03010203),
        ("58:5:9m", 0x00000000, 0x00000000, 0x02000009),
        ("4;58;5;9m", 0x10000000, 0x10000000, 0x06000009),
        ("4;58;5;9;59m", 0x10000000, 0x10000000, 0x07FFFFFF),
        ("58;5;9;24m", 0x00000000, 0x00000000, 0x02000009),
        ("58;5;9;4:0m", 0x00000000, 0x00000000, 0x02000009),
        ("4:3;58;2;9;8;7;0m", 0x00000000, 0x00000000, 0x00000000),
        ("4:9m", 0x10000000, 0x10000000, 0x04000000),
        ("4:6m", 0x10000000, 0x10000000, 0x04000000),
        ("4::m", 0x10000000, 0x10000000, 0x04000000),
        ("4:m", 0x10000000, 0x10000000, 0x04000000),
        ("4:;1m", 0x18000000, 0x10000000, 0x04000000),
        ("4:3:4m", 0x10000000, 0x10000000, 0x0C000000),
        ("4:0m", 0x00000000, 0x00000000, 0x00000000),
        ("4:5;4m", 0x10000000, 0x10000000, 0x04000000),
        ("21;24m", 0x00000000, 0x00000000, 0x00000000),
        ("4:2;24m", 0x00000000, 0x00000000, 0x00000000),
        ("4:3;21m", 0x10000000, 0x10000000, 0x08000000),
        ("10m", 0x00000000, 0x00000000, 0x00000000),
        ("56m", 0x00000000, 0x00000000, 0x00000000),
        ("60m", 0x00000000, 0x00000000, 0x00000000),
        ("100m", 0x00000000, 0x01000008, 0x00000000),
        ("107m", 0x00000000, 0x0100000F, 0x00000000),
        ("108m", 0x00000000, 0x00000000, 0x00000000),
        ("200m", 0x00000000, 0x00000000, 0x00000000),
        ("99999999999m", 0x00000000, 0x00000000, 0x00000000),
        ("m", 0x00000000, 0x00000000, 0x00000000),
        ("0m", 0x00000000, 0x00000000, 0x00000000),
        (";m", 0x00000000, 0x00000000, 0x00000000),
        ("0;1m", 0x08000000, 0x00000000, 0x00000000),
        ("1;0m", 0x00000000, 0x00000000, 0x00000000),
        ("1;;3m", 0x00000000, 0x04000000, 0x00000000),
        ("1;31;0;42m", 0x00000000, 0x01000002, 0x00000000),
        ("22m", 0x00000000, 0x00000000, 0x00000000),
        ("1;2;22m", 0x00000000, 0x00000000, 0x00000000),
        ("3;23m", 0x00000000, 0x00000000, 0x00000000),
        ("5;25m", 0x00000000, 0x00000000, 0x00000000),
        ("7;27m", 0x00000000, 0x00000000, 0x00000000),
        ("8;28m", 0x00000000, 0x00000000, 0x00000000),
        ("9;29m", 0x00000000, 0x00000000, 0x00000000),
        ("53;55m", 0x00000000, 0x00000000, 0x00000000),
        ("38;5;1;39m", 0x00000000, 0x00000000, 0x00000000),
        ("48;2;1;2;3;49m", 0x00000000, 0x00000000, 0x00000000),
        ("31;39;41;49m", 0x00000000, 0x00000000, 0x00000000),
        ("91;101m", 0x01000009, 0x01000009, 0x00000000),
        ("97;107m", 0x0100000F, 0x0100000F, 0x00000000),
        ("1;4:3;38:2::1:2:3;48;5;7;58:2:9:8:7:6m", 0x1B010203, 0x12000007, 0x0F080706),
        ("38;2;1;2;3;4;5;6;7m", 0x37010203, 0x10000000, 0x04000000),
        ("38:2:1:2:3:4:5:6:7:8:9:10m", 0x03020304, 0x00000000, 0x00000000),
        ("38;2;1;2;3;48;2;4;5;6;58;2;7;8;9m", 0x03010203, 0x03040506, 0x03070809),
        ("59m", 0x00000000, 0x00000000, 0x03FFFFFF),
        ("4;59m", 0x10000000, 0x10000000, 0x07FFFFFF),
        ("4;58;5;3;59m", 0x10000000, 0x10000000, 0x07FFFFFF),
        ("38;5;102;38;5;105;38;5;101;38;5;104;38;5;100;38;5;103;38;5;106;38;5;102;38;5;105;38;5;101;38;5;104;38;5;100;38;5;103;38m", 0x02000000, 0x00000000, 0x00000000),
        ("1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;31m", 0x08000000, 0x00000000, 0x00000000),
    ];
    for &(seq, fg, bg, ext) in cases {
        let t = sgr(seq);
        let a = t.attr();
        assert_eq!(
            (a.fg, a.bg, a.extended.ext(), a.extended.url_id()),
            (fg, bg, ext, 0),
            "ESC [ {seq}"
        );
    }
}

#[test]
fn sgr_params_past_the_stored_ones_are_not_read() {
    // 32 params are stored; the 33rd `31` is dropped, so the colour stays default
    let ones = vec!["1"; 40].join(";");
    let t = sgr(&format!("{ones};31m"));
    assert!(t.attr().is_bold() != 0);
    assert_eq!(t.attr().get_fg_color_mode(), 0);
}

#[test]
fn extract_color_survives_every_truncation_of_a_long_sequence() {
    // no panic for any prefix of a sequence that mixes 38/48/58 with sub params, and the
    // handler keeps running afterwards
    let full = "38;2;1;2;3;48:5:7;58:2::9:8:7;38;5;1:2:3:4:5:6:7;4:3;38:2:1:2:3:4:5:6:7:8:9";
    for end in 0..=full.len() {
        let mut t = upstream_handler();
        t.parse_str(&format!("\x1b[{}m", &full[..end]));
        t.parse_str("\x1b[0mok");
        assert_eq!(t.attr(), AttributeData::default());
        assert_eq!(t.screen().first().map(String::as_str), Some("ok"));
    }
}

// --- upstream: InputHandler.test.ts, step 7 (modes, reports, DECSTR, DECSCUSR, XTWINOPS) ---
//
// adapted (step 7):
//  - `windowOptions` `22/23 - PushTitle/PopTitle` (3 cases): `inputHandler.onTitleChange` is the
//    `HostEvent::Title` events. Step 7 set the title by calling `set_title`/`set_icon_name`
//    (OSC was not ported yet); step 8 runs the real `ESC ] 0 ; Pt BEL` (`osc_0_*` below and
//    `set_title_and_icon`). Titles coalesce to one per drain (D2 amendment, Q10), so the cases
//    drain after every write, as a host does (`titles_after`); the storm without draining is
//    `title_events_coalesce_to_the_latest_per_drain`.
//  - `windowOptions` `14` and `16`: upstream's test terminal has no renderer and reports nothing;
//    here they are the `HostEvent::WindowReport` events (the host answers them).
//  - `DECRQM`, `DECSTR` and `windowOptions` read replies from `coreService.onData`; here they
//    are the `HostEvent::Response` events.
//  - `setCursorStyle` resets `optionsService.options = clone(DEFAULT_OPTIONS)` between cases;
//    the DEC private modes are the only state the handler writes, so nothing is reset here.
//  - `BS with reverseWraparound` `:134` places the cursor by setting `buffer.x/y` as upstream.

/// The `Response` events since the last call, as strings.
fn responses(t: &mut TestTerm) -> Vec<String> {
    t.handler
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::Response(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            _ => None,
        })
        .collect()
}

fn set_window_options(t: &mut TestTerm, options: WindowOptions) {
    t.handler
        .services
        .options
        .set(OptionValue::WindowOptions(options))
        .unwrap();
}

#[test]
fn set_cursor_style_sets_the_dec_private_modes() {
    let mut t = upstream_handler();
    let state = |t: &TestTerm| {
        let modes = &t.handler.services.core.dec_private_modes;
        (modes.cursor_style, modes.cursor_blink)
    };
    t.handler.set_cursor_style(&params(&[0]));
    assert_eq!(state(&t), (None, None));
    for (param, style, blink) in [
        (1, CursorStyle::Block, true),
        (2, CursorStyle::Block, false),
        (3, CursorStyle::Underline, true),
        (4, CursorStyle::Underline, false),
        (5, CursorStyle::Bar, true),
        (6, CursorStyle::Bar, false),
    ] {
        t.handler.set_cursor_style(&params(&[param]));
        assert_eq!(state(&t), (Some(style), Some(blink)), "{param}");
    }
}

#[test]
fn set_mode_should_toggle_bracketed_paste_mode() {
    let mut t = test_handler(80, 30, []);
    // Set bracketed paste mode
    t.handler.set_mode_private(&params(&[2004]));
    assert!(
        t.handler
            .services
            .core
            .dec_private_modes
            .bracketed_paste_mode
    );
    // Reset bracketed paste mode
    t.handler.reset_mode_private(&params(&[2004]));
    assert!(
        !t.handler
            .services
            .core
            .dec_private_modes
            .bracketed_paste_mode
    );
}

#[test]
fn bs_with_reverse_wraparound_set_should_not_reverse_outside_of_scroll_margins() {
    let mut t = upstream_handler();
    t.resize(5, 5);
    t.set_option(OptionValue::Scrollback(1));
    t.reset_buffers();
    // prepare buffer content
    t.parse_str("#####abcdefghijklmnopqrstuvwxy");
    assert_eq!(
        t.lines(6),
        ["#####", "abcde", "fghij", "klmno", "pqrst", "uvwxy"]
    );
    assert_eq!(t.buffer().ydisp, 1);
    assert_eq!(t.buffer().x, 5);
    assert_eq!(t.buffer().y, 4);
    t.parse_str(&TTY_BS.repeat(100));
    assert_eq!(
        t.lines(6),
        ["#####", "abcde", "fghij", "klmno", "pqrst", "    y"]
    );

    t.parse_str("\x1b[?45h");
    t.parse_str("uvwxy");

    // set top/bottom to 1/3 (0-based)
    t.parse_str("\x1b[2;4r");
    // place cursor below scroll bottom
    t.set_cursor(5, 4);
    t.parse_str(&TTY_BS.repeat(100));
    assert_eq!(
        t.lines(6),
        ["#####", "abcde", "fghij", "klmno", "pqrst", "     "]
    );

    t.parse_str("uvwxy");
    // place cursor within scroll margins
    t.set_cursor(5, 3);
    t.parse_str(&TTY_BS.repeat(100));
    assert_eq!(
        t.lines(6),
        ["#####", "abcde", "     ", "     ", "     ", "uvwxy"]
    );
    assert_eq!(t.buffer().x, 0);
    assert_eq!(t.buffer().y, t.buffer().scroll_top); // stops at 0, scrollTop

    t.parse_str("fghijklmnopqrst");
    // place cursor above scroll top
    t.set_cursor(5, 0);
    t.parse_str(&TTY_BS.repeat(100));
    assert_eq!(
        t.lines(6),
        ["#####", "     ", "fghij", "klmno", "pqrst", "uvwxy"]
    );
}

#[test]
fn bs_with_reverse_wraparound_set_can_delete_last_cell() {
    let mut t = bs_handler();
    t.parse_str("\x1b[?45h");
    t.parse_str("12345");
    t.parse_str(TTY_BS);
    assert_eq!(t.lines(1), ["1234 "]);
    t.parse_str(&TTY_BS.repeat(7));
    assert_eq!(t.lines(1), ["     "]);
}

#[test]
fn bs_with_reverse_wraparound_set_can_access_prev_line_if_wrapped() {
    let mut t = bs_handler();
    t.parse_str("\x1b[?45h");
    t.parse_str(&"12345".repeat(2));
    t.parse_str(TTY_BS);
    assert_eq!(t.lines(2), ["12345", "1234 "]);
    t.parse_str(&TTY_BS.repeat(7));
    assert_eq!(t.lines(2), ["12   ", "     "]);
}

#[test]
fn bs_with_reverse_wraparound_set_should_lift_is_wrapped() {
    let mut t = bs_handler();
    t.parse_str("\x1b[?45h");
    t.parse_str(&"12345".repeat(2));
    assert!(t.buffer().lines.get(1).unwrap().is_wrapped);
    t.parse_str(&TTY_BS.repeat(7));
    assert!(!t.buffer().lines.get(1).unwrap().is_wrapped);
}

#[test]
fn bs_with_reverse_wraparound_set_stops_at_hard_nls() {
    let mut t = bs_handler();
    t.parse_str("\x1b[?45h");
    t.parse_str("12345\r\n");
    t.parse_str(&"12345".repeat(2));
    t.parse_str(&TTY_BS.repeat(50));
    assert_eq!(t.lines(3), ["12345", "     ", "     "]);
    assert_eq!(t.buffer().x, 0);
    assert_eq!(t.buffer().y, 1);
}

#[test]
fn bs_with_reverse_wraparound_set_handles_wide_chars_correctly() {
    let mut t = bs_handler();
    t.parse_str("\x1b[?45h");
    t.parse_str("￥￥￥");
    assert_eq!(t.lines(2), ["￥￥", "￥"]);
    for (lines, x) in [
        (["￥￥", "  "], 1),
        (["￥￥", "  "], 0),
        (["￥  ", "  "], 3), // x=4 skipped due to early wrap-around
        (["￥  ", "  "], 2),
        (["    ", "  "], 1),
        (["    ", "  "], 0),
    ] {
        t.parse_str(TTY_BS);
        assert_eq!(t.lines(2), lines);
        assert_eq!(t.buffer().x, x);
    }
}

/// `getFgColor()` / `getBgColor()` of a cell colour word: the palette index.
fn palette(colour: u32) -> u32 {
    colour & Attributes::PCOLOR_MASK
}

/// Upstream `describe('alt screen')`: a 80 x 30 handler.
fn alt_screen_handler() -> TestTerm {
    test_handler(80, 30, [])
}

#[test]
fn alt_screen_should_handle_decset_decrst_47() {
    let mut t = alt_screen_handler();
    t.parse_str("\x1b[?47h\r\n\x1b[31mJUNK\x1b[?47lTEST");
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(0, true, 0, None),
        ""
    );
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(1, true, 0, None),
        "    TEST"
    );
    // Text color of 'TEST' should be red
    assert_eq!(palette(t.cell(1, 4).2), 1);
}

#[test]
fn alt_screen_should_handle_decset_decrst_1047() {
    let mut t = alt_screen_handler();
    t.parse_str("\x1b[?1047h\r\n\x1b[31mJUNK\x1b[?1047lTEST");
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(0, true, 0, None),
        ""
    );
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(1, true, 0, None),
        "    TEST"
    );
    assert_eq!(palette(t.cell(1, 4).2), 1);
}

#[test]
fn alt_screen_should_handle_decset_decrst_1048() {
    let mut t = alt_screen_handler();
    t.parse_str("\x1b[?1048h\r\n\x1b[31mJUNK\x1b[?1048lTEST");
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(0, true, 0, None),
        "TEST"
    );
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(1, true, 0, None),
        "JUNK"
    );
    // Text color of 'TEST' should be default
    assert_eq!(
        t.buffer().lines.get(0).unwrap().get_fg(0),
        DEFAULT_ATTR_DATA.fg
    );
    // Text color of 'JUNK' should be red
    assert_eq!(palette(t.cell(1, 0).2), 1);
}

#[test]
fn alt_screen_should_handle_decset_decrst_1049() {
    let mut t = alt_screen_handler();
    t.parse_str("\x1b[?1049h\r\n\x1b[31mJUNK\x1b[?1049lTEST");
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(0, true, 0, None),
        "TEST"
    );
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(1, true, 0, None),
        ""
    );
    // Text color of 'TEST' should be default
    assert_eq!(
        t.buffer().lines.get(0).unwrap().get_fg(0),
        DEFAULT_ATTR_DATA.fg
    );
}

#[test]
fn alt_screen_1049_maintains_saved_cursor_for_alt_buffer() {
    let mut t = alt_screen_handler();
    t.parse_str("\x1b[?1049h\r\n\x1b[31m\x1b[s\x1b[?1049lTEST");
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(0, true, 0, None),
        "TEST"
    );
    // Text color of 'TEST' should be default
    assert_eq!(
        t.buffer().lines.get(0).unwrap().get_fg(0),
        DEFAULT_ATTR_DATA.fg
    );
    t.parse_str("\x1b[?1049h\x1b[uTEST");
    assert_eq!(
        t.buffer().translate_buffer_line_to_string(1, true, 0, None),
        "TEST"
    );
    // Text color of 'TEST' should be red
    assert_eq!(palette(t.cell(1, 0).2), 1);
}

#[test]
fn alt_screen_1049_clears_alt_buffer_with_erase_attributes() {
    let mut t = alt_screen_handler();
    t.parse_str("\x1b[42m\x1b[?1049h");
    // Buffer should be filled with green background
    assert_eq!(palette(t.cell(20, 10).3), 2);
}

#[test]
fn window_options_all_should_be_disabled_by_default_and_not_report() {
    let mut t = upstream_handler();
    t.resize(10, 10);
    for n in [14, 16, 18, 20, 21] {
        t.parse_str(&format!("\x1b[{n}t"));
    }
    assert!(t.handler.take_events().is_empty());
}

#[test]
fn window_options_14_get_win_size_pixels() {
    let mut t = upstream_handler();
    t.resize(10, 10);
    set_window_options(
        &mut t,
        WindowOptions {
            get_win_size_pixels: true,
            ..WindowOptions::default()
        },
    );
    t.parse_str("\x1b[14t");
    assert_eq!(
        t.handler.take_events(),
        [HostEvent::WindowReport(WindowReportKind::WinSizePixels)]
    );
    // `CSI 14 ; 2 t` (the window size, not the text area) is not reported
    t.parse_str("\x1b[14;2t");
    assert!(t.handler.take_events().is_empty());
}

#[test]
fn window_options_16_get_cell_size_pixels() {
    let mut t = upstream_handler();
    t.resize(10, 10);
    set_window_options(
        &mut t,
        WindowOptions {
            get_cell_size_pixels: true,
            ..WindowOptions::default()
        },
    );
    t.parse_str("\x1b[16t");
    assert_eq!(
        t.handler.take_events(),
        [HostEvent::WindowReport(WindowReportKind::CellSizePixels)]
    );
}

#[test]
fn window_options_18_get_win_size_chars() {
    let mut t = upstream_handler();
    t.resize(10, 10);
    set_window_options(
        &mut t,
        WindowOptions {
            get_win_size_chars: true,
            ..WindowOptions::default()
        },
    );
    t.parse_str("\x1b[18t");
    assert_eq!(responses(&mut t), ["\x1b[8;10;10t"]);
    t.resize(50, 20);
    t.parse_str("\x1b[18t");
    assert_eq!(responses(&mut t), ["\x1b[8;20;50t"]);
}

fn push_pop_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(10, 10);
    set_window_options(
        &mut t,
        WindowOptions {
            push_title: true,
            pop_title: true,
            ..WindowOptions::default()
        },
    );
    t
}

/// Upstream's `ESC ] 0 ; Pt BEL`: the title and the icon name.
fn set_title_and_icon(t: &mut TestTerm, name: &str) {
    t.parse_str(&format!("\x1b]0;{name}\x07"));
}

/// A title stack as string slices (the stacks hold shared `Arc<str>` titles).
fn strs(stack: &[std::sync::Arc<str>]) -> Vec<&str> {
    stack.iter().map(|title| &**title).collect()
}

/// Parse `input` and drain: the titles one write produced (the host drains after each write).
fn titles_after(t: &mut TestTerm, input: &str) -> Vec<String> {
    t.parse_str(input);
    titles(t)
}

/// The title after each `TitleChanged` of the drain (at most one since Q10/Q11).
fn titles(t: &mut TestTerm) -> Vec<String> {
    let events = t.handler.take_events();
    events
        .iter()
        .filter(|event| **event == HostEvent::TitleChanged)
        .map(|_| t.handler.window_title().to_owned())
        .collect()
}

#[test]
fn window_options_22_23_push_title_pop_title() {
    let mut t = push_pop_handler();
    let mut seen = Vec::new();
    for name in ["1", "2", "3"] {
        set_title_and_icon(&mut t, name);
        seen.extend(titles_after(&mut t, "\x1b[22t"));
    }
    assert_eq!(strs(&t.handler.window_title_stack), ["1", "2", "3"]);
    assert_eq!(strs(&t.handler.icon_name_stack), ["1", "2", "3"]);
    assert_eq!(seen, ["1", "2", "3"]);
    let mut seen = Vec::new();
    for _ in 0..4 {
        // the fourth one tests "overflow"
        seen.extend(titles_after(&mut t, "\x1b[23t"));
    }
    assert!(t.handler.window_title_stack.is_empty());
    assert!(t.handler.icon_name_stack.is_empty());
    assert_eq!(seen, ["3", "2", "1"]);
}

#[test]
fn window_options_22_23_push_title_pop_title_with_1() {
    let mut t = push_pop_handler();
    let mut seen = Vec::new();
    for name in ["1", "2", "3"] {
        set_title_and_icon(&mut t, name);
        seen.extend(titles_after(&mut t, "\x1b[22;1t"));
    }
    assert!(t.handler.window_title_stack.is_empty());
    assert_eq!(strs(&t.handler.icon_name_stack), ["1", "2", "3"]);
    assert_eq!(seen, ["1", "2", "3"]);
    for _ in 0..4 {
        t.parse_str("\x1b[23;1t");
    }
    assert!(t.handler.window_title_stack.is_empty());
    assert!(t.handler.icon_name_stack.is_empty());
    assert!(titles(&mut t).is_empty());
}

#[test]
fn window_options_22_23_push_title_pop_title_with_2() {
    let mut t = push_pop_handler();
    let mut seen = Vec::new();
    for name in ["1", "2", "3"] {
        set_title_and_icon(&mut t, name);
        seen.extend(titles_after(&mut t, "\x1b[22;2t"));
    }
    assert_eq!(strs(&t.handler.window_title_stack), ["1", "2", "3"]);
    assert!(t.handler.icon_name_stack.is_empty());
    assert_eq!(seen, ["1", "2", "3"]);
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.extend(titles_after(&mut t, "\x1b[23;2t"));
    }
    assert!(t.handler.window_title_stack.is_empty());
    assert!(t.handler.icon_name_stack.is_empty());
    assert_eq!(seen, ["3", "2", "1"]);
}

#[test]
fn window_options_deccolm_should_only_work_with_set_win_lines_enabled() {
    let mut t = upstream_handler();
    // disabled
    t.resize(10, 10);
    t.parse_str("\x1b[?3l");
    assert_eq!(t.handler.services.buffer.cols, 10);
    t.parse_str("\x1b[?3h");
    assert_eq!(t.handler.services.buffer.cols, 10);
    // enabled
    t.handler.reset();
    set_window_options(
        &mut t,
        WindowOptions {
            set_win_lines: true,
            ..WindowOptions::default()
        },
    );
    t.parse_str("\x1b[?3l");
    assert_eq!(t.handler.services.buffer.cols, 80);
    t.parse_str("\x1b[?3h");
    assert_eq!(t.handler.services.buffer.cols, 132);
}

/// Upstream `describe('DECSTR')`: 10 x 5, scrollback 1, `01234567890123` written.
fn decstr_handler() -> TestTerm {
    let mut t = upstream_handler();
    t.resize(10, 5);
    t.set_option(OptionValue::Scrollback(1));
    t.parse_str("01234567890123");
    t
}

#[test]
fn decstr_should_reset_irm() {
    let mut t = decstr_handler();
    t.parse_str("\x1b[4h");
    assert!(t.handler.services.core.modes.insert_mode);
    t.parse_str("\x1b[!p");
    assert!(!t.handler.services.core.modes.insert_mode);
}

#[test]
fn decstr_should_reset_cursor_visibility() {
    let mut t = decstr_handler();
    t.parse_str("\x1b[?25l");
    assert!(t.handler.services.core.is_cursor_hidden);
    t.parse_str("\x1b[!p");
    assert!(!t.handler.services.core.is_cursor_hidden);
}

#[test]
fn decstr_should_reset_scroll_margins() {
    let mut t = decstr_handler();
    t.parse_str("\x1b[2;4r");
    assert_eq!(t.buffer().scroll_top, 1);
    assert_eq!(t.buffer().scroll_bottom, 3);
    t.parse_str("\x1b[!p");
    assert_eq!(t.buffer().scroll_top, 0);
    assert_eq!(t.buffer().scroll_bottom, t.handler.services.buffer.rows - 1);
}

#[test]
fn decstr_should_reset_text_attributes() {
    let mut t = decstr_handler();
    t.parse_str("\x1b[1;2;32;43m");
    assert!(t.handler.cur_attr_data.is_bold() != 0);
    t.parse_str("\x1b[!p");
    assert!(t.handler.cur_attr_data.is_bold() == 0);
    assert_eq!(t.handler.cur_attr_data.fg, 0);
    assert_eq!(t.handler.cur_attr_data.bg, 0);
}

#[test]
fn decstr_should_reset_decsc_data() {
    let mut t = decstr_handler();
    t.parse_str("\x1b7");
    assert_eq!(t.buffer().saved_x, 4);
    assert_eq!(t.buffer().saved_y, 1);
    t.parse_str("\x1b[!p");
    assert_eq!(t.buffer().saved_x, 0);
    assert_eq!(t.buffer().saved_y, 0);
}

#[test]
fn decstr_should_reset_decom() {
    let mut t = decstr_handler();
    t.parse_str("\x1b[?6h");
    assert!(t.handler.services.core.dec_private_modes.origin);
    t.parse_str("\x1b[!p");
    assert!(!t.handler.services.core.dec_private_modes.origin);
}

/// Send `sequence` and return the one reply it produced.
fn reply(t: &mut TestTerm, sequence: &str) -> String {
    t.parse_str(sequence);
    let mut all = responses(t);
    assert_eq!(all.len(), 1, "replies to {sequence:?}: {all:?}");
    all.remove(0)
}

#[test]
fn decrqm_ansi_2_keyboard_action_mode() {
    let mut t = upstream_handler();
    assert_eq!(reply(&mut t, "\x1b[2$p"), "\x1b[2;4$y"); // always reset
}

#[test]
fn decrqm_ansi_4_insert_mode() {
    let mut t = upstream_handler();
    assert_eq!(reply(&mut t, "\x1b[4$p"), "\x1b[4;2$y"); // reset by default
    t.parse_str("\x1b[4h");
    assert_eq!(reply(&mut t, "\x1b[4$p"), "\x1b[4;1$y"); // now active
    t.parse_str("\x1b[4l");
    assert_eq!(reply(&mut t, "\x1b[4$p"), "\x1b[4;2$y"); // again reset
}

#[test]
fn decrqm_ansi_12_send_receive() {
    let mut t = upstream_handler();
    assert_eq!(reply(&mut t, "\x1b[12$p"), "\x1b[12;3$y"); // always set
}

#[test]
fn decrqm_ansi_20_newline_mode() {
    let mut t = upstream_handler();
    assert_eq!(reply(&mut t, "\x1b[20$p"), "\x1b[20;2$y"); // reset by default
    t.parse_str("\x1b[20h");
    assert_eq!(reply(&mut t, "\x1b[20$p"), "\x1b[20;1$y"); // now active
    t.parse_str("\x1b[20l");
    assert_eq!(reply(&mut t, "\x1b[20$p"), "\x1b[20;2$y"); // again reset
}

#[test]
fn decrqm_ansi_unknown() {
    let mut t = upstream_handler();
    assert_eq!(reply(&mut t, "\x1b[1234$p"), "\x1b[1234;0$y"); // not recognized
}

#[test]
fn decrqm_dec_privates_with_set_reset_semantic() {
    let mut t = upstream_handler();
    // initially reset
    for mode in [
        1, 6, 9, 12, 45, 66, 1000, 1002, 1003, 1004, 1006, 1016, 47, 1047, 1049, 2004, 2026,
    ] {
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};2$y")
        );
        t.parse_str(&format!("\x1b[?{mode}h"));
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};1$y")
        );
        t.parse_str(&format!("\x1b[?{mode}l"));
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};2$y")
        );
    }
    // initially set
    for mode in [7, 25] {
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};1$y")
        );
        t.parse_str(&format!("\x1b[?{mode}l"));
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};2$y")
        );
        t.parse_str(&format!("\x1b[?{mode}h"));
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};1$y")
        );
    }
}

#[test]
fn decrqm_dec_privates_perma_modes() {
    let mut t = upstream_handler();
    for (mode, value) in [(3, 0), (8, 3), (67, 4), (1005, 4), (1015, 4), (1048, 1)] {
        assert_eq!(
            reply(&mut t, &format!("\x1b[?{mode}$p")),
            format!("\x1b[?{mode};{value}$y")
        );
    }
}

// --- added: step 7, expected values from @xterm/headless 6.0.0 (the whole observable state after
// the same input; the scratch generator prints one `key=value;...` line per case) ---

/// The state string the oracle script prints: size, cursor and buffer geometry, attributes,
/// modes, margins, tab stops, saved cursor, mouse, charset, the two options the handlers write,
/// the replies and every buffer line (`|` row start, `~` wrapped row, right-trimmed).
fn state_of(t: &TestTerm, replies: &[String]) -> String {
    let services = &t.handler.services;
    let b = services.buffer.buffer();
    let flag = |v: bool| if v { '1' } else { '0' };
    let opt = |v: Option<bool>| match v {
        Some(v) => flag(v),
        None => '-',
    };
    let dm = &services.core.dec_private_modes;
    let style = match dm.cursor_style {
        Some(CursorStyle::Block) => "block",
        Some(CursorStyle::Underline) => "underline",
        Some(CursorStyle::Bar) => "bar",
        None => "-",
    };
    let tabs: Vec<String> = b.tabs.iter().map(usize::to_string).collect();
    let lines: Vec<String> = (0..b.lines.len())
        .filter_map(|i| b.lines.get(i))
        .map(|line| {
            format!(
                "{}{}",
                if line.is_wrapped { '~' } else { '|' },
                line.translate_to_string(true, None, None, None)
            )
        })
        .collect();
    let opts = services.options.options();
    format!(
        "size={}x{};cur={},{},{},{},{};alt={};attr={},{};ins={};dm={}{}{}{}{}{}{}{};cursor={},{};\
         hidden={};margins={},{};tabs={};saved={},{};mouse={}/{};g={},{};opt={}{};resp={};lines={}",
        services.buffer.cols,
        services.buffer.rows,
        b.x,
        b.y,
        b.ybase,
        b.ydisp,
        b.lines.len(),
        flag(services.buffer.buffers.is_alt_active()),
        t.handler.cur_attr_data.fg,
        t.handler.cur_attr_data.bg,
        flag(services.core.modes.insert_mode),
        flag(dm.application_cursor_keys),
        flag(dm.application_keypad),
        flag(dm.bracketed_paste_mode),
        flag(dm.origin),
        flag(dm.reverse_wraparound),
        flag(dm.send_focus),
        flag(dm.synchronized_output),
        flag(dm.wraparound),
        style,
        opt(dm.cursor_blink),
        flag(services.core.is_cursor_hidden),
        b.scroll_top,
        b.scroll_bottom,
        tabs.join(","),
        b.saved_x,
        b.saved_y,
        services.mouse.active_protocol(),
        services.mouse.active_encoding(),
        services.charset.glevel(),
        flag(services.charset.charset.is_some()),
        flag(opts.convert_eol),
        flag(opts.cursor_blink),
        replies.join("|").replace('\x1b', "\\e"),
        lines.join("\n"),
    )
}

/// Replay `inputs` (one `parse` each) and compare [`state_of`] with the oracle's.
fn state_case(
    cols: usize,
    rows: usize,
    options: Vec<OptionValue>,
    inputs: &[&str],
    expected: &str,
) {
    let mut t = test_handler(cols, rows, options);
    let mut replies = Vec::new();
    for input in inputs {
        t.parse_str(input);
        replies.extend(responses(&mut t));
    }
    let got = state_of(&t, &replies);
    // compare field by field first: the failing field is named
    for (g, e) in got.split(';').zip(expected.split(';')) {
        assert_eq!(g, e, "state field");
    }
    assert_eq!(got, expected);
}

#[test]
fn oracle_state_ris_then_csi_osc_text() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1b[1;31mcd\x1bc\x1b[2;3Hx\x1b]1000;zz\x07yz"],
        "size=10x4;cur=5,1,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|  xyz\n|\n|",
    );
}

#[test]
fn oracle_state_ris_at_chunk_end() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1b[31m", "\x1bc", "[1mQ"],
        "size=10x4;cur=4,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|[1mQ\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_then_split_csi() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1bc\x1b", "[1mQ"],
        "size=10x4;cur=1,0,0,0,4;alt=0;attr=134217728,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|Q\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_then_split_csi_params() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1bc\x1b[1;", "31mQ"],
        "size=10x4;cur=1,0,0,0,4;alt=0;attr=150994945,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|Q\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_during_pending_osc() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1b]1000;abc\x1bc", "xy"],
        "size=10x4;cur=2,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|xy\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_clears_rep() {
    state_case(
        10,
        4,
        vec![],
        &["a\x1bc\x1b[3b"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_resets_everything() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[4h\x1b[?6h\x1b[?7l\x1b[?45h\x1b[?25l\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b(0\x0e\x1b[1;2r\x1b[3g\x1b[5G\x1bH\x1b[2;4H\x1b7\x1b[?12h\x1b[20h\x1b[3 q\x1b[1;31mxyz",
            "\x1bc",
            "q",
        ],
        "size=10x4;cur=1,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=1;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=11;resp=;lines=|q\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_in_alt_screen() {
    state_case(
        10,
        4,
        vec![],
        &["x\x1b[?1049hy\x1bcz"],
        "size=10x4;cur=1,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|z\n|\n|\n|",
    );
}

#[test]
fn oracle_state_ris_with_scrollback() {
    state_case(
        5,
        3,
        vec![],
        &["aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff\r\nggggg\r\nhhhhh\x1bck"],
        "size=5x3;cur=1,0,0,0,3;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,2;tabs=0;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|k\n|\n|",
    );
}

#[test]
fn oracle_state_da_primary_secondary() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[c\x1b[>c\x1b[1c\x1b[>1c\x1b[0c\x1b[>0c\x1b[?c"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?1;2c|\\e[>0;276;0c|\\e[?1;2c|\\e[>0;276;0c;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_da_term_rxvt() {
    state_case(
        10,
        4,
        vec![OptionValue::TermName("rxvt-unicode-256color".to_owned())],
        &["\x1b[c\x1b[>c"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?1;2c|\\e[>85;95;0c;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_da_term_screen() {
    state_case(
        10,
        4,
        vec![OptionValue::TermName("screen-256color".to_owned())],
        &["\x1b[c\x1b[>c"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?1;2c|\\e[>83;40003;0c;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_da_term_linux() {
    state_case(
        10,
        4,
        vec![OptionValue::TermName("linux".to_owned())],
        &["\x1b[c\x1b[>c\x1b[>0c"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?6c|0c|0c;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_da_term_other() {
    state_case(
        10,
        4,
        vec![OptionValue::TermName("vt100".to_owned())],
        &["\x1b[c\x1b[>c"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_da_term_not_prefix() {
    state_case(
        10,
        4,
        vec![OptionValue::TermName("myxterm".to_owned())],
        &["\x1b[c\x1b[>c"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_dsr_all() {
    state_case(
        10,
        4,
        vec![],
        &[
            "ab\x1b[5n\x1b[6n\x1b[?6n\x1b[?15n\x1b[?25n\x1b[?26n\x1b[?53n\x1b[7n\x1b[n\x1b[1;2H\x1b[6n",
        ],
        "size=10x4;cur=1,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[0n|\\e[1;3R|\\e[?1;3R|\\e[1;2R;lines=|ab\n|\n|\n|",
    );
}

#[test]
fn oracle_state_dsr_origin() {
    state_case(
        10,
        5,
        vec![],
        &["\x1b[2;4r\x1b[?6h\x1b[2;3H\x1b[6n\x1b[?6n"],
        "size=10x5;cur=2,2,0,0,5;alt=0;attr=0,0;ins=0;dm=00010001;cursor=-,-;hidden=0;margins=1,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[3;3R|\\e[?3;3R;lines=|\n|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_dsr_wrap_pending() {
    state_case(
        5,
        3,
        vec![],
        &["abcde\x1b[6n"],
        "size=5x3;cur=5,0,0,0,3;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,2;tabs=0;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[1;6R;lines=|abcde\n|\n|",
    );
}

#[test]
fn oracle_state_sm_rm() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[4;20h\x1b[2;12h",
            "\x1b[4l",
            "\x1b[20l\x1b[1;2;3;4h\x1b[1;2;3;4l",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_all_known() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[?1;6;7;12;25;45;66;2004;2026h"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=11111011;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=01;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrst_all_known() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[?1;6;7;12;25;45;66;2004;2026h\x1b[?1;6;7;12;25;45;66;2004;2026l"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000000;cursor=-,-;hidden=1;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_mouse() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[?9h",
            "\x1b[?1000h",
            "\x1b[?1002h",
            "\x1b[?1003h\x1b[?1006h",
            "\x1b[?1016h",
            "\x1b[?1005h\x1b[?1015h",
            "\x1b[?1003l\x1b[?1006l",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_mouse_off_each() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[?1002h\x1b[?1016h\x1b[?9l\x1b[?1016l",
            "\x1b[?1002h\x1b[?1000l",
            "\x1b[?1003h\x1b[?1002l",
            "\x1b[?1000h\x1b[?1003l",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_focus() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[?1004h", "\x1b[?1004l"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_2() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b(0\x1b)0\x1b*0\x1b+0q\x1b[?2h\x1b[?2l", "q\x1bnq\x1boq"],
        "size=10x4;cur=4,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=3,0;opt=00;resp=;lines=|\u{2500}qqq\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_origin_cursor() {
    state_case(
        10,
        5,
        vec![],
        &["\x1b[2;4r\x1b[4;4H\x1b[?6h", "x\x1b[?6l", "y"],
        "size=10x5;cur=1,0,0,0,5;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=1,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|y\n|x\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decset_12_20_options() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[?12h\x1b[20h", "a\nb", "\x1b[?12l\x1b[20l", "c\nd"],
        "size=10x4;cur=3,2,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|a\n|bc\n|  d\n|",
    );
}

#[test]
fn oracle_state_decset_multi_mixed() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[?1;1049;2004;99999h", "\x1b[?1049;2004;7l"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=10000000;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_deccolm_off() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1b[?3h", "\x1b[?3l"],
        "size=10x4;cur=2,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|ab\n|\n|\n|",
    );
}

#[test]
fn oracle_state_deccolm_on() {
    state_case(
        10,
        4,
        vec![OptionValue::WindowOptions(WindowOptions {
            set_win_lines: true,
            ..WindowOptions::default()
        })],
        &["ab\x1b[?3h", "x", "\x1b[?3$p", "\x1b[?3l", "y\x1b[?3$p"],
        "size=80x4;cur=1,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8,16,24,32,40,48,56,64,72;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?3;1$y|\\e[?3;2$y;lines=|y\n|\n|\n|",
    );
}

#[test]
fn oracle_state_alt_47_1047() {
    state_case(
        10,
        4,
        vec![],
        &[
            "ab\x1b[?47hxy",
            "\x1b[?47l",
            "z\x1b[?1047hq",
            "\x1b[?1047lw",
        ],
        "size=10x4;cur=7,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|ab  z w\n|\n|\n|",
    );
}

#[test]
fn oracle_state_alt_1049() {
    state_case(
        10,
        4,
        vec![],
        &[
            "ab\x1b[2;3H\x1b[?1049hxy\x1b[3;3Hz",
            "\x1b[?1049lw",
            "\x1b[?1049h\x1b[?1049hv\x1b[?1049l\x1b[?1049l",
        ],
        "size=10x4;cur=3,1,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=3,1;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|ab\n|  w\n|\n|",
    );
}

#[test]
fn oracle_state_alt_1048() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1b[2;3H\x1b[?1048h\x1b[1;1H\x1b[31mxy\x1b[?1048lw"],
        "size=10x4;cur=3,1,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=2,1;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|xy\n|  w\n|\n|",
    );
}

#[test]
fn oracle_state_alt_erase_attrs() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[42m\x1b[?1049h"],
        "size=10x4;cur=0,0,0,0,4;alt=1;attr=0,16777218;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_alt_scrollback_viewport() {
    state_case(
        5,
        3,
        vec![],
        &[
            "aaaaa\r\nbbbbb\r\nccccc\r\nddddd\r\neeeee\r\nfffff",
            "\x1b[?1049h\x1b[Hm",
            "\x1b[?1049l",
        ],
        "size=5x3;cur=4,2,3,3,6;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,2;tabs=0;saved=5,5;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|aaaaa\n|bbbbb\n|ccccc\n|ddddd\n|eeeee\n|fffff",
    );
}

#[test]
fn oracle_state_alt_resize_flow() {
    state_case(
        10,
        4,
        vec![],
        &["ab\x1b[?1049hxy\x1b[?47l\x1b[?1047hz\x1b[?47h"],
        "size=10x4;cur=5,0,0,0,4;alt=1;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|    z\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrqm_ansi() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[2$p\x1b[4$p\x1b[12$p\x1b[20$p\x1b[1234$p\x1b[$p\x1b[4h\x1b[20h\x1b[4$p\x1b[20$p"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=1;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=10;resp=\\e[2;4$y|\\e[4;2$y|\\e[12;3$y|\\e[20;2$y|\\e[1234;0$y|\\e[0;0$y|\\e[4;1$y|\\e[20;1$y;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrqm_dec_reset() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[?1$p\x1b[?3$p\x1b[?6$p\x1b[?7$p\x1b[?8$p\x1b[?9$p\x1b[?12$p\x1b[?25$p\x1b[?45$p\x1b[?47$p\x1b[?66$p\x1b[?67$p\x1b[?1000$p\x1b[?1002$p\x1b[?1003$p\x1b[?1004$p\x1b[?1005$p\x1b[?1006$p\x1b[?1015$p\x1b[?1016$p\x1b[?1047$p\x1b[?1048$p\x1b[?1049$p\x1b[?2004$p\x1b[?2026$p\x1b[?5$p\x1b[?99999$p",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?1;2$y|\\e[?3;0$y|\\e[?6;2$y|\\e[?7;1$y|\\e[?8;3$y|\\e[?9;2$y|\\e[?12;2$y|\\e[?25;1$y|\\e[?45;2$y|\\e[?47;2$y|\\e[?66;2$y|\\e[?67;4$y|\\e[?1000;2$y|\\e[?1002;2$y|\\e[?1003;2$y|\\e[?1004;2$y|\\e[?1005;4$y|\\e[?1006;2$y|\\e[?1015;4$y|\\e[?1016;2$y|\\e[?1047;2$y|\\e[?1048;1$y|\\e[?1049;2$y|\\e[?2004;2$y|\\e[?2026;2$y|\\e[?5;0$y|\\e[?99999;0$y;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrqm_dec_set() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[?1h\x1b[?6h\x1b[?9h\x1b[?12h\x1b[?45h\x1b[?66h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h\x1b[?1016h\x1b[?2004h\x1b[?2026h\x1b[?1$p\x1b[?6$p\x1b[?9$p\x1b[?12$p\x1b[?45$p\x1b[?66$p\x1b[?1000$p\x1b[?1002$p\x1b[?1003$p\x1b[?1004$p\x1b[?1006$p\x1b[?1016$p\x1b[?2004$p\x1b[?2026$p\x1b[?25$p\x1b[?7$p",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=11111111;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=ANY/SGR_PIXELS;g=0,0;opt=01;resp=\\e[?1;1$y|\\e[?6;1$y|\\e[?9;2$y|\\e[?12;1$y|\\e[?45;1$y|\\e[?66;1$y|\\e[?1000;2$y|\\e[?1002;2$y|\\e[?1003;1$y|\\e[?1004;1$y|\\e[?1006;2$y|\\e[?1016;1$y|\\e[?2004;1$y|\\e[?2026;1$y|\\e[?25;1$y|\\e[?7;1$y;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrqm_alt() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b[?47$p\x1b[?1049h\x1b[?47$p\x1b[?1047$p\x1b[?1049$p\x1b[?1049l\x1b[?1049$p"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?47;2$y|\\e[?47;1$y|\\e[?1047;1$y|\\e[?1049;1$y|\\e[?1049;2$y;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrqm_deccolm() {
    state_case(
        10,
        4,
        vec![OptionValue::WindowOptions(WindowOptions {
            set_win_lines: true,
            ..WindowOptions::default()
        })],
        &["\x1b[?3$p\x1b[?3h\x1b[?3$p\x1b[?3l\x1b[?3$p"],
        "size=80x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8,16,24,32,40,48,56,64,72;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[?3;0$y|\\e[?3;1$y|\\e[?3;2$y;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decrqm_mouse_exclusive() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[?1000h\x1b[?1002h\x1b[?1000$p\x1b[?1002$p\x1b[?1006h\x1b[?1016h\x1b[?1006$p\x1b[?1016$p",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=DRAG/SGR_PIXELS;g=0,0;opt=00;resp=\\e[?1000;2$y|\\e[?1002;1$y|\\e[?1006;2$y|\\e[?1016;1$y;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decstr_all() {
    state_case(
        10,
        5,
        vec![],
        &[
            "\x1b[4h\x1b[?25l\x1b[2;4r\x1b[1;2;32;43m\x1b7\x1b[?6h\x1b(0\x1b[?1h\x1b[?7l\x1b[?1000h\x1b[3 q",
            "\x1b[!p",
            "q",
        ],
        "size=10x5;cur=1,1,0,0,5;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,4;tabs=0,8;saved=0,0;mouse=VT200/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|q\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decstr_saved_cursor() {
    state_case(
        10,
        5,
        vec![],
        &["012345678901234\x1b[1;31m\x1b7\x1b[!p\x1b8x"],
        "size=10x5;cur=1,0,0,0,5;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,4;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|x123456789\n~01234\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decstr_keeps_screen() {
    state_case(
        10,
        5,
        vec![],
        &["abc\x1b[2;5H\x1b[!p", "x"],
        "size=10x5;cur=5,1,0,0,5;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,4;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|abc\n|    x\n|\n|\n|",
    );
}

#[test]
fn oracle_state_decscusr() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[ q",
            "\x1b[2 q",
            "\x1b[3 q",
            "\x1b[4 q",
            "\x1b[5 q",
            "\x1b[6 q",
            "\x1b[7 q",
            "\x1b[0 q",
            "\x1b[3 q\x1b[0 q",
            "\x1b[9 q",
            "\x1b[ q\x1b[100000 q",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,0;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_xtwinops_defaults() {
    state_case(
        10,
        4,
        vec![],
        &[
            "\x1b[1t\x1b[2t\x1b[3t\x1b[4t\x1b[5t\x1b[6t\x1b[7t\x1b[8t\x1b[9t\x1b[10t\x1b[11t\x1b[12t\x1b[13t\x1b[14t\x1b[15t\x1b[16t\x1b[17t\x1b[18t\x1b[19t\x1b[20t\x1b[21t\x1b[22t\x1b[23t\x1b[24t\x1b[25t\x1b[26t\x1b[27t\x1b[28t\x1b[29t\x1b[30t\x1b[1000t\x1b[t\x1b[22;0t\x1b[23;0t",
        ],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_xtwinops_size_report() {
    state_case(
        7,
        3,
        vec![OptionValue::WindowOptions(WindowOptions {
            get_win_size_chars: true,
            ..WindowOptions::default()
        })],
        &["\x1b[18t\x1b[18;1;2t\x1b[14t\x1b[16t\x1b[19t\x1b[21t"],
        "size=7x3;cur=0,0,0,0,3;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,2;tabs=0;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[8;3;7t|\\e[8;3;7t;lines=|\n|\n|",
    );
}

#[test]
fn oracle_state_xtwinops_all_on() {
    state_case(
        7,
        3,
        vec![OptionValue::WindowOptions(WindowOptions {
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
        })],
        &[
            "\x1b[1t\x1b[2t\x1b[3t\x1b[4t\x1b[5t\x1b[6t\x1b[7t\x1b[8t\x1b[9t\x1b[10t\x1b[11t\x1b[12t\x1b[13t\x1b[14t\x1b[15t\x1b[16t\x1b[17t\x1b[18t\x1b[19t\x1b[20t\x1b[21t\x1b[22t\x1b[23t\x1b[24t\x1b[25t\x1b[26t\x1b[27t\x1b[28t\x1b[29t\x1b[30t\x1b[1000t\x1b[22;3t\x1b[23;3t\x1b[14;2t",
        ],
        "size=7x3;cur=0,0,0,0,3;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,2;tabs=0;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=\\e[8;3;7t;lines=|\n|\n|",
    );
}

#[test]
fn oracle_state_keypad() {
    state_case(
        10,
        4,
        vec![],
        &["\x1b=", "\x1b>", "\x1b=\x1b[?66l", "\x1b[?66h"],
        "size=10x4;cur=0,0,0,0,4;alt=0;attr=0,0;ins=0;dm=01000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_charsets_designate() {
    state_case(
        12,
        4,
        vec![],
        &[
            "\x1b(0q\x1b(Bq\x1b(Aq#\x1b(Kq{\x1b(Rq@\x1b(4q\x1b(Cq\x1b(5q\x1b(Qq\x1b(Yq\x1b(Eq\x1b(6q\x1b(Zq\x1b(Hq\x1b(7q\x1b(=q",
            "\r\n\x1b(0\x1b)0\x1b*0\x1b+0\x0eq\x0fq\x1bnq\x1boq\x1b|q\x1b}q\x1b~q",
        ],
        "size=12x4;cur=7,2,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=1,1;opt=00;resp=;lines=|\u{2500}qq\u{a3}q\u{e4}q\u{e0}qqqq\n~qqqqqqq\n|\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n|",
    );
}

#[test]
fn oracle_state_charsets_every_intermediate() {
    state_case(
        20,
        6,
        vec![],
        &[
            "\x1b(0\x1b)A\x1b*K\x1b+R\x1b-Q\x1b.Y\x1b/Zqq\x0eq\x1bnq\x1boq\x1b~q\x1b}q\x1b|q",
            "\r\n\x1b-0\x0e\x1b~q\x1b.0\x1bnq\x1b/0\x1bnq",
        ],
        "size=20x6;cur=3,1,0,0,6;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,5;tabs=0,8,16;saved=0,0;mouse=NONE/DEFAULT;g=2,1;opt=00;resp=;lines=|\u{2500}\u{2500}qqqqqq\n|\u{2500}\u{2500}\u{2500}\n|\n|\n|\n|",
    );
}

#[test]
fn oracle_state_charsets_default_select() {
    state_case(
        12,
        4,
        vec![],
        &[
            "\x1b(0\x1b)0\x0eq\x1b%@q\x1b(0\x0eq\x1b%Gq",
            "\x1b(0q\x1b(Xq\x1b(0q\x1b(Bq",
        ],
        "size=12x4;cur=8,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|\u{2500}q\u{2500}q\u{2500}\u{2500}\u{2500}q\n|\n|\n|",
    );
}

#[test]
fn oracle_state_charsets_save_restore() {
    state_case(
        12,
        4,
        vec![],
        &["\x1b(0\x1b7\x1b(Bq\x1b8q", "\x1b[!p\x1b8q"],
        "size=12x4;cur=1,0,0,0,4;alt=0;attr=0,0;ins=0;dm=00000001;cursor=-,-;hidden=0;margins=0,3;tabs=0,8;saved=0,0;mouse=NONE/DEFAULT;g=0,0;opt=00;resp=;lines=|q\n|\n|\n|",
    );
}

// --- added: step 7 (events, damage, options, bounds) ---

#[test]
fn replies_are_suppressed_by_disable_stdin() {
    let mut t = test_handler(10, 4, [OptionValue::DisableStdin(true)]);
    t.parse_str("\x1b[c\x1b[>c\x1b[5n\x1b[6n\x1b[?6n\x1b[4$p\x1b[?7$p");
    assert!(t.handler.take_events().is_empty());
    // with stdin enabled again the same input answers
    t.set_option(OptionValue::DisableStdin(false));
    t.parse_str("\x1b[5n");
    assert_eq!(responses(&mut t), ["\x1b[0n"]);
}

#[test]
fn replies_keep_their_order_and_are_not_merged() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b[5n\x1b[c\x1b[?7$p");
    assert_eq!(responses(&mut t), ["\x1b[0n", "\x1b[?1;2c", "\x1b[?7;1$y"]);
}

#[test]
fn da_does_not_see_the_first_param_of_the_previous_sequence() {
    // upstream reads `params.params[0]` of the shared typed array; the parser resets the first
    // slot to 0 for every sequence, so the 5 of `CSI 5 c` does not suppress the next answer
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b[5c\x1b[c");
    assert_eq!(responses(&mut t), ["\x1b[?1;2c"]);
}

fn state_without_stacks(t: &TestTerm) -> String {
    state_of(t, &[])
}

#[test]
fn window_options_default_consume_every_csi_t_without_effect() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("ab\x1b[2;3H");
    let before = state_without_stacks(&t);
    t.handler.take_events();
    t.handler.take_damage();
    for n in (1..=24).chain(25..=30).chain([1000, 65535]) {
        t.parse_str(&format!("\x1b[{n}t"));
        t.parse_str(&format!("\x1b[{n};1;2t"));
    }
    t.parse_str("\x1b[t");
    assert!(t.handler.take_events().is_empty());
    assert_eq!(state_without_stacks(&t), before);
    assert!(t.handler.window_title_stack.is_empty());
    assert!(t.handler.icon_name_stack.is_empty());
}

#[test]
fn window_options_enabled_act_only_for_14_16_18_22_23() {
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
    for n in (1..=30).chain([1000, 65535]) {
        let mut t = test_handler(7, 3, []);
        set_window_options(&mut t, all);
        t.parse_str("ab\x1b[2;3H");
        let before = state_without_stacks(&t);
        t.handler.take_events();
        t.parse_str(&format!("\x1b[{n}t"));
        let events = t.handler.take_events();
        match n {
            14 => assert_eq!(
                events,
                [HostEvent::WindowReport(WindowReportKind::WinSizePixels)]
            ),
            16 => assert_eq!(
                events,
                [HostEvent::WindowReport(WindowReportKind::CellSizePixels)]
            ),
            18 => assert_eq!(events, [HostEvent::Response(b"\x1b[8;3;7t".to_vec())]),
            _ => assert!(events.is_empty(), "{n}: {events:?}"),
        }
        assert_eq!(state_without_stacks(&t), before, "{n}");
        // push and pop change only the stacks (and pop sets the, empty, title back)
        let pushed = usize::from(n == 22);
        assert_eq!(t.handler.window_title_stack.len(), pushed, "{n}");
        assert_eq!(t.handler.icon_name_stack.len(), pushed, "{n}");
    }
}

#[test]
fn window_options_pop_title_sets_the_saved_title_and_notifies_the_host() {
    let mut t = push_pop_handler();
    t.handler.set_title("saved");
    t.handler.take_events();
    t.parse_str("\x1b[22t");
    t.handler.set_title("other");
    t.handler.take_events();
    t.parse_str("\x1b[23t");
    assert_eq!(t.handler.window_title(), "saved");
    assert_eq!(titles(&mut t), ["saved"]);
}

#[test]
fn window_options_title_stacks_keep_the_ten_newest() {
    let mut t = push_pop_handler();
    for i in 0..12 {
        set_title_and_icon(&mut t, &i.to_string());
        t.parse_str("\x1b[22t");
    }
    let expected: Vec<String> = (2..12).map(|i| i.to_string()).collect();
    assert_eq!(strs(&t.handler.window_title_stack), expected);
    assert_eq!(strs(&t.handler.icon_name_stack), expected);
    t.handler.take_events();
    let mut seen = Vec::new();
    for _ in 0..12 {
        seen.extend(titles_after(&mut t, "\x1b[23t"));
    }
    assert!(t.handler.window_title_stack.is_empty());
    let popped: Vec<String> = (2..12).rev().map(|i| i.to_string()).collect();
    assert_eq!(seen, popped);
    assert_eq!(t.handler.window_title(), "2");
}

#[test]
fn window_options_push_with_unknown_second_param_does_nothing() {
    let mut t = push_pop_handler();
    t.parse_str("\x1b[22;3t\x1b[22;7t\x1b[23;3t");
    assert!(t.handler.window_title_stack.is_empty());
    assert!(t.handler.icon_name_stack.is_empty());
    assert!(t.handler.take_events().is_empty());
}

#[test]
fn focus_report_is_requested_by_decset_1004_only() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b[?1004h");
    assert_eq!(t.handler.take_events(), [HostEvent::FocusReportRequested]);
    t.parse_str("\x1b[?1004h");
    assert_eq!(t.handler.take_events(), [HostEvent::FocusReportRequested]);
    t.parse_str("\x1b[?1004l");
    assert!(t.handler.take_events().is_empty());
    assert!(!t.handler.services.core.dec_private_modes.send_focus);
}

#[test]
fn mouse_modes_set_the_protocol_and_encoding_by_upstreams_names() {
    let mut t = test_handler(10, 4, []);
    for (mode, protocol) in [(9, "X10"), (1000, "VT200"), (1002, "DRAG"), (1003, "ANY")] {
        t.parse_str(&format!("\x1b[?{mode}h"));
        assert_eq!(t.handler.services.mouse.active_protocol(), protocol);
        // every protocol is switched off by any of the four resets
        for off in [9, 1000, 1002, 1003] {
            t.parse_str(&format!("\x1b[?{mode}h\x1b[?{off}l"));
            assert_eq!(t.handler.services.mouse.active_protocol(), "NONE");
        }
    }
    for (mode, encoding) in [(1006, "SGR"), (1016, "SGR_PIXELS")] {
        t.parse_str(&format!("\x1b[?{mode}h"));
        assert_eq!(t.handler.services.mouse.active_encoding(), encoding);
        t.parse_str(&format!("\x1b[?{mode}l"));
        assert_eq!(t.handler.services.mouse.active_encoding(), "DEFAULT");
    }
}

#[test]
fn utf8_and_urxvt_mouse_encodings_only_log() {
    use crate::services::log_service::Logger;
    use crate::services::options_service::LogLevel;

    struct Capture(Rc<RefCell<Vec<String>>>);
    impl Logger for Capture {
        fn log(&self, _level: LogLevel, message: &str) {
            self.0.borrow_mut().push(message.to_string());
        }
    }
    let mut t = test_handler(10, 4, [OptionValue::LogLevel(LogLevel::Debug)]);
    let messages = Rc::new(RefCell::new(Vec::new()));
    t.handler
        .services
        .log
        .set_logger(Some(Box::new(Capture(Rc::clone(&messages)))));
    t.parse_str("\x1b[?1006h");
    let before = state_without_stacks(&t);
    t.parse_str("\x1b[?1005h\x1b[?1015h\x1b[?1005l\x1b[?1015l");
    assert_eq!(state_without_stacks(&t), before);
    let messages = messages.borrow();
    for expected in [
        "DECSET 1005 not supported (see #2507)",
        "DECSET 1015 not supported (see #2507)",
        "DECRST 1005 not supported (see #2507)",
        "DECRST 1015 not supported (see #2507)",
    ] {
        assert!(messages.iter().any(|m| m == expected), "{expected}");
    }
}

#[test]
fn decset_12_and_20_write_the_options() {
    let mut t = test_handler(10, 4, []);
    assert!(!t.handler.services.options.options().cursor_blink);
    assert!(!t.handler.services.options.options().convert_eol);
    t.parse_str("\x1b[?12h\x1b[20h");
    assert!(t.handler.services.options.options().cursor_blink);
    assert!(t.handler.services.options.options().convert_eol);
    // LNM is observable: LF returns the carriage
    t.parse_str("ab\ncd");
    assert_eq!(t.lines(2), ["ab", "cd"]);
    t.parse_str("\x1b[?12l\x1b[20l");
    assert!(!t.handler.services.options.options().cursor_blink);
    assert!(!t.handler.services.options.options().convert_eol);
}

#[test]
fn alt_buffer_switches_report_scroll_and_full_damage() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("ab");
    t.handler.take_damage();
    t.parse_str("\x1b[?1049h");
    let damage = t.handler.take_damage();
    assert!(damage.full && damage.scrolled, "{damage:?}");
    assert_eq!(damage.rows, Some((0, 3)));
    // already on the alt buffer: no activation, but the rows are requested again
    t.parse_str("\x1b[?47h");
    let damage = t.handler.take_damage();
    assert!(!damage.full && !damage.scrolled, "{damage:?}");
    assert_eq!(damage.rows, Some((0, 3)));
    t.parse_str("\x1b[?1049l");
    let damage = t.handler.take_damage();
    assert!(damage.full && damage.scrolled, "{damage:?}");
    // already on the normal buffer: rows only
    t.parse_str("\x1b[?47l");
    let damage = t.handler.take_damage();
    assert!(!damage.full, "{damage:?}");
    assert_eq!(damage.rows, Some((0, 3)));
    // the synchronized output reset repaints too
    t.parse_str("\x1b[?2026h");
    assert!(
        t.handler
            .take_damage()
            .rows
            .is_some_and(|rows| rows == (0, 0))
    );
    t.parse_str("\x1b[?2026l");
    assert_eq!(t.handler.take_damage().rows, Some((0, 3)));
}

#[test]
fn alt_buffer_activation_marks_the_cursor_as_initialized() {
    let mut t = test_handler(10, 4, []);
    assert!(!t.handler.services.core.is_cursor_initialized);
    t.parse_str("\x1b[?47h");
    assert!(t.handler.services.core.is_cursor_initialized);
    t.handler.services.core.is_cursor_initialized = false;
    t.parse_str("\x1b[?47l");
    assert!(t.handler.services.core.is_cursor_initialized);
}

#[test]
fn alt_buffer_has_no_scrollback_and_normal_scrollback_survives() {
    let mut t = test_handler(5, 3, [OptionValue::Scrollback(10)]);
    t.parse_str("1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(t.buffer().ybase, 2);
    t.parse_str("\x1b[?1049h");
    t.parse_str("a\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(t.buffer().ybase, 0);
    assert_eq!(t.buffer().lines.len(), 3);
    t.parse_str("\x1b[?1049l");
    assert_eq!(t.buffer().ybase, 2);
    assert_eq!(t.viewport(), ["3", "4", "5"]);
}

#[test]
fn the_alt_buffer_markers_are_disposed_when_switching_back() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b[?1049h");
    let marker = t.handler.services.buffer.buffers.alt_mut().add_marker(1);
    t.parse_str("\x1b[?1049l");
    let alt = t.handler.services.buffer.buffers.alt_mut();
    assert!(alt.take_disposed_markers().contains(&marker));
}

#[test]
fn deccolm_resizes_resets_and_reports_full_damage() {
    let mut t = test_handler(10, 4, []);
    set_window_options(
        &mut t,
        WindowOptions {
            set_win_lines: true,
            ..WindowOptions::default()
        },
    );
    t.parse_str("abc\x1b[2;3H\x1b[?6h");
    t.handler.take_damage();
    t.parse_str("\x1b[?3h");
    assert_eq!(
        (
            t.handler.services.buffer.cols,
            t.handler.services.buffer.rows
        ),
        (132, 4)
    );
    // the terminal reset ran: screen, cursor and modes are default, the size stays
    assert_eq!(t.screen(), ["", "", "", ""]);
    assert_eq!(t.cursor(), [0, 0]);
    assert!(!t.handler.services.core.dec_private_modes.origin);
    assert!(t.handler.take_damage().full);
    assert_eq!(t.handler.services.options.options().cols, 132);
    assert_eq!(t.handler.services.options.options().rows, 4);
    t.parse_str("\x1b[?3l");
    assert_eq!(t.handler.services.buffer.cols, 80);
    assert_eq!(t.handler.services.options.options().cols, 80);
}

#[test]
fn keypad_modes_log_and_set_the_mode() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b=");
    assert!(t.handler.services.core.dec_private_modes.application_keypad);
    t.parse_str("\x1b>");
    assert!(!t.handler.services.core.dec_private_modes.application_keypad);
    t.parse_str("\x1b[?66h");
    assert!(t.handler.services.core.dec_private_modes.application_keypad);
    t.parse_str("\x1b[?66l");
    assert!(!t.handler.services.core.dec_private_modes.application_keypad);
}

#[test]
fn every_charset_designator_selects_its_set_for_the_level_of_its_intermediate() {
    for flag in [
        '0', 'A', 'B', '4', 'C', '5', 'R', 'Q', 'K', 'Y', 'E', '6', 'Z', 'H', '7', '=',
    ] {
        for (intermediate, level) in [('(', 0), (')', 1), ('*', 2), ('+', 3), ('-', 1), ('.', 2)] {
            let mut t = test_handler(10, 4, []);
            t.parse_str(&format!("\x1b{intermediate}{flag}"));
            let service = &t.handler.services.charset;
            // the designated set is current only when its level is
            let expected = if level == 0 { charset(flag) } else { None };
            assert_eq!(service.charset, expected, "ESC {intermediate} {flag}");
            t.handler.services.charset.set_g_level(level);
            assert_eq!(
                t.handler.services.charset.charset,
                charset(flag),
                "ESC {intermediate} {flag} at level {level}"
            );
        }
    }
}

#[test]
fn designating_with_slash_or_an_unknown_flag_leaves_the_sets() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b(0");
    let dec = t.handler.services.charset.charset;
    assert!(dec.is_some());
    // `ESC / C` is a no-op (upstream TODO)
    for flag in ['0', 'A', 'B', 'Z'] {
        t.parse_str(&format!("\x1b/{flag}"));
        assert_eq!(t.handler.services.charset.charset, dec);
    }
    // an unknown flag selects the default set (it has no handler, so nothing changes)
    t.parse_str("\x1b(X");
    assert_eq!(t.handler.services.charset.charset, dec);
    t.parse_str("\x1b(B");
    assert!(t.handler.services.charset.charset.is_none());
}

#[test]
fn ris_runs_the_terminal_reset_and_reports_full_damage() {
    let mut t = test_handler(10, 4, [OptionValue::Scrollback(5)]);
    t.parse_str("1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[1;31m\x1b[?7l");
    assert_eq!(t.buffer().ybase, 2);
    t.handler.take_damage();
    t.parse_str("\x1bc");
    assert_eq!(t.buffer().ybase, 0);
    assert_eq!(t.screen(), ["", "", "", ""]);
    assert_eq!(t.handler.cur_attr_data, DEFAULT_ATTR_DATA);
    assert!(t.handler.services.core.dec_private_modes.wraparound);
    let damage = t.handler.take_damage();
    assert!(damage.full && damage.scrolled);
}

#[test]
fn ris_with_an_active_link_frees_the_link_table() {
    use crate::services::osc_link_service::OscLinkData;

    let mut t = test_handler(10, 4, []);
    let services = &mut t.handler.services;
    let id = services.osc_link.register_link(
        &mut services.buffer,
        OscLinkData {
            id: None,
            uri: "https://a".to_owned(),
        },
    );
    assert!(t.handler.services.osc_link.get_link_data(id).is_some());
    t.parse_str("\x1bc");
    t.handler.forward_disposed_markers();
    assert!(t.handler.services.osc_link.get_link_data(id).is_none());
}

#[test]
fn soft_reset_keeps_the_screen_the_cursor_and_the_erase_attributes() {
    let mut t = test_handler(10, 5, []);
    t.parse_str("abc\x1b[2;5H\x1b[42m");
    t.parse_str("\x1b[!p");
    assert_eq!(t.lines(1), ["abc"]);
    assert_eq!(t.cursor(), [4, 1]);
    // upstream does not reset the erase attributes (`_eraseAttrDataInternal`), only the colours
    // of the text attributes: the erase takes the background of the (reset) attributes
    assert_eq!(t.handler.cur_attr_data, DEFAULT_ATTR_DATA);
}

#[test]
fn requests_for_an_unknown_mode_number_and_missing_params() {
    let mut t = test_handler(10, 4, []);
    t.parse_str("\x1b[$p\x1b[?$p\x1b[?65535$p\x1b[?2147483647$p");
    assert_eq!(
        responses(&mut t),
        [
            "\x1b[0;0$y",
            "\x1b[?0;0$y",
            "\x1b[?65535;0$y",
            "\x1b[?2147483647;0$y"
        ]
    );
}

#[test]
fn handlers_survive_extreme_params_and_many_params() {
    let mut t = test_handler(10, 4, []);
    let many = (0..40)
        .map(|i| (1000 + i).to_string())
        .collect::<Vec<_>>()
        .join(";");
    for body in [
        format!("\x1b[?{many}h"),
        format!("\x1b[?{many}l"),
        format!("\x1b[{many}h"),
        format!("\x1b[{many}l"),
        "\x1b[?2147483647h\x1b[?2147483647l\x1b[2147483647h".to_owned(),
        "\x1b[2147483647;2147483647 q\x1b[2147483647n\x1b[?2147483647n".to_owned(),
        "\x1b[2147483647t\x1b[2147483647;2147483647;2147483647t".to_owned(),
        "\x1b[1:2:3h\x1b[?1:2h\x1b[:h\x1b[;;h".to_owned(),
    ] {
        t.parse_str(&body);
        t.handler.take_events();
    }
    t.parse_str("\x1b[0mok");
    assert_eq!(t.lines(1), ["ok"]);
}

#[test]
fn decscusr_without_params_is_blinking_block_and_other_params_keep_the_style() {
    let mut t = test_handler(10, 4, []);
    // the parser always supplies a default param (0): a bare sequence resets to the option
    t.parse_str("\x1b[3 q\x1b[ q");
    let modes = &t.handler.services.core.dec_private_modes;
    assert_eq!((modes.cursor_style, modes.cursor_blink), (None, None));
    // only a handler call without any param takes the `length === 0` branch
    t.handler.set_cursor_style(&Params::new());
    let modes = &t.handler.services.core.dec_private_modes;
    assert_eq!(modes.cursor_style, Some(CursorStyle::Block));
    assert_eq!(modes.cursor_blink, Some(true));
    // 7 matches no style but still sets the blink by parity
    t.parse_str("\x1b[4 q\x1b[7 q");
    let modes = &t.handler.services.core.dec_private_modes;
    assert_eq!(modes.cursor_style, Some(CursorStyle::Underline));
    assert_eq!(modes.cursor_blink, Some(true));
}

// --- upstream: InputHandler.test.ts, step 8 (OSC, DECRQSS) ---

/// The `HostEvent::Color` events since the last call (upstream's `onColor` stack).
fn color_events(t: &mut TestTerm) -> Vec<Vec<ColorRequest>> {
    t.handler
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::Color(requests) => Some(requests),
            _ => None,
        })
        .collect()
}

use ColorIndex::{Background, Cursor, Foreground, Palette};
use ColorRequest::{Report, Restore, Set};

fn parse_osc(t: &mut TestTerm, body: &str) {
    t.parse_str(&format!("\x1b]{body}\x07"));
}

#[test]
fn osc_4_query_color_events() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "4;0;?");
    assert_eq!(color_events(&mut t), [vec![Report(Palette(0))]]);
    parse_osc(&mut t, "4;123;?");
    assert_eq!(color_events(&mut t), [vec![Report(Palette(123))]]);
    // multiple queries
    parse_osc(&mut t, "4;0;?;123;?");
    assert_eq!(
        color_events(&mut t),
        [vec![Report(Palette(0)), Report(Palette(123))]]
    );
}

#[test]
fn osc_4_set_color_events() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "4;0;rgb:01/02/03");
    assert_eq!(color_events(&mut t), [vec![Set(Palette(0), [1, 2, 3])]]);
    parse_osc(&mut t, "4;123;#aabbcc");
    assert_eq!(
        color_events(&mut t),
        [vec![Set(Palette(123), [170, 187, 204])]]
    );
    // multiple pairs
    parse_osc(&mut t, "4;0;rgb:aa/bb/cc;123;#001122");
    assert_eq!(
        color_events(&mut t),
        [vec![
            Set(Palette(0), [170, 187, 204]),
            Set(Palette(123), [0, 17, 34])
        ]]
    );
}

#[test]
fn osc_4_should_ignore_invalid_values() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "4;0;rgb:aa/bb/cc;45;rgb:1/22/333;123;#001122");
    assert_eq!(
        color_events(&mut t),
        [vec![
            Set(Palette(0), [170, 187, 204]),
            Set(Palette(123), [0, 17, 34])
        ]]
    );
}

#[test]
fn osc_8_hyperlink_with_id() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "8;id=100;http://localhost:3000");
    let url_id = t.attr().extended.url_id();
    assert_ne!(url_id, 0);
    assert_eq!(
        t.handler.services.osc_link.get_link_data(url_id),
        Some(&OscLinkData {
            id: Some("100".to_owned()),
            uri: "http://localhost:3000".to_owned(),
        })
    );
    parse_osc(&mut t, "8;;");
    assert_eq!(t.attr().extended.url_id(), 0);
}

#[test]
fn osc_8_hyperlink_with_semi_colon() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "8;;http://localhost:3000;abc=def");
    let url_id = t.attr().extended.url_id();
    assert_ne!(url_id, 0);
    assert_eq!(
        t.handler.services.osc_link.get_link_data(url_id),
        Some(&OscLinkData {
            id: None,
            uri: "http://localhost:3000;abc=def".to_owned(),
        })
    );
    parse_osc(&mut t, "8;;");
    assert_eq!(t.attr().extended.url_id(), 0);
}

#[test]
fn osc_104_restore_events() {
    let mut t = upstream_handler();
    t.parse_str("\x1b]104;0\x07\x1b]104;43\x07");
    assert_eq!(
        color_events(&mut t),
        [
            vec![Restore(Some(Palette(0)))],
            vec![Restore(Some(Palette(43)))]
        ]
    );
    // multiple in one command
    parse_osc(&mut t, "104;0;43");
    assert_eq!(
        color_events(&mut t),
        [vec![Restore(Some(Palette(0))), Restore(Some(Palette(43)))]]
    );
    // full ANSI table restore
    parse_osc(&mut t, "104");
    assert_eq!(color_events(&mut t), [vec![Restore(None)]]);
}

#[test]
fn osc_10_fg_set_and_query_events() {
    let mut t = upstream_handler();
    // single foreground query
    parse_osc(&mut t, "10;?");
    assert_eq!(color_events(&mut t), [vec![Report(Foreground)]]);
    // OSC with multiple values maps to OSC 10 & OSC 11 & OSC 12
    parse_osc(&mut t, "10;?;?;?;?");
    assert_eq!(
        color_events(&mut t),
        [
            vec![Report(Foreground)],
            vec![Report(Background)],
            vec![Report(Cursor)]
        ]
    );
    // set foreground color events
    parse_osc(&mut t, "10;rgb:01/02/03");
    assert_eq!(color_events(&mut t), [vec![Set(Foreground, [1, 2, 3])]]);
    parse_osc(&mut t, "10;#aabbcc");
    assert_eq!(
        color_events(&mut t),
        [vec![Set(Foreground, [170, 187, 204])]]
    );
    // set FG, BG and cursor color at once
    parse_osc(&mut t, "10;rgb:aa/bb/cc;#001122;rgb:12/34/56");
    assert_eq!(
        color_events(&mut t),
        [
            vec![Set(Foreground, [170, 187, 204])],
            vec![Set(Background, [0, 17, 34])],
            vec![Set(Cursor, [18, 52, 86])]
        ]
    );
}

#[test]
fn osc_110_restore_fg_color() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "110");
    assert_eq!(color_events(&mut t), [vec![Restore(Some(Foreground))]]);
}

#[test]
fn osc_11_bg_set_and_query_events() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "11;?");
    assert_eq!(color_events(&mut t), [vec![Report(Background)]]);
    // OSC 11 with multiple values creates only BG and cursor event
    parse_osc(&mut t, "11;?;?;?;?");
    assert_eq!(
        color_events(&mut t),
        [vec![Report(Background)], vec![Report(Cursor)]]
    );
    parse_osc(&mut t, "11;rgb:01/02/03");
    assert_eq!(color_events(&mut t), [vec![Set(Background, [1, 2, 3])]]);
    parse_osc(&mut t, "11;#aabbcc");
    assert_eq!(
        color_events(&mut t),
        [vec![Set(Background, [170, 187, 204])]]
    );
    // set BG and cursor color at once
    parse_osc(&mut t, "11;#001122;rgb:12/34/56");
    assert_eq!(
        color_events(&mut t),
        [
            vec![Set(Background, [0, 17, 34])],
            vec![Set(Cursor, [18, 52, 86])]
        ]
    );
}

#[test]
fn osc_111_restore_bg_color() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "111");
    assert_eq!(color_events(&mut t), [vec![Restore(Some(Background))]]);
}

#[test]
fn osc_12_cursor_color_set_and_query_events() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "12;?");
    assert_eq!(color_events(&mut t), [vec![Report(Cursor)]]);
    // OSC 12 with multiple values creates only cursor event
    parse_osc(&mut t, "12;?;?;?;?");
    assert_eq!(color_events(&mut t), [vec![Report(Cursor)]]);
    parse_osc(&mut t, "12;rgb:01/02/03");
    assert_eq!(color_events(&mut t), [vec![Set(Cursor, [1, 2, 3])]]);
    parse_osc(&mut t, "12;#aabbcc");
    assert_eq!(color_events(&mut t), [vec![Set(Cursor, [170, 187, 204])]]);
}

#[test]
fn osc_112_restore_cursor_color() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "112");
    assert_eq!(color_events(&mut t), [vec![Restore(Some(Cursor))]]);
}

#[test]
fn decrqss_reports_correct_decsca_state() {
    let mut t = upstream_handler();
    // DCS $ q " q ST
    t.parse_str("\x1bP$q\"q\x1b\\");
    // default - DECSCA unset (0 or 2)
    assert_eq!(responses(&mut t), ["\x1bP1$r0\"q\x1b\\"]);
    // DECSCA 1 - protected set
    t.parse_str("###\x1b[1\"q");
    t.parse_str("\x1bP$q\"q\x1b\\");
    assert_eq!(responses(&mut t), ["\x1bP1$r1\"q\x1b\\"]);
    // DECSCA 2 - protected reset (same as 0)
    t.parse_str("###\x1b[2\"q");
    t.parse_str("\x1bP$q\"q\x1b\\");
    assert_eq!(responses(&mut t), ["\x1bP1$r0\"q\x1b\\"]); // reported as DECSCA 0
}

// --- added: step 8 ---

#[test]
fn osc_0_sets_title_and_icon_name_osc_1_the_icon_name_only_osc_2_the_title_only() {
    let mut t = upstream_handler();
    parse_osc(&mut t, "0;both");
    assert_eq!(
        (t.handler.window_title(), &*t.handler.icon_name),
        ("both", "both")
    );
    assert_eq!(titles(&mut t), ["both"]);
    parse_osc(&mut t, "1;icon");
    assert_eq!(
        (t.handler.window_title(), &*t.handler.icon_name),
        ("both", "icon")
    );
    assert!(t.handler.take_events().is_empty(), "OSC 1 fires no event");
    parse_osc(&mut t, "2;title");
    assert_eq!(
        (t.handler.window_title(), &*t.handler.icon_name),
        ("title", "icon")
    );
    assert_eq!(titles(&mut t), ["title"]);
    // a title keeps its semicolons and its non-ASCII text; ST terminates like BEL
    t.parse_str("\x1b]2;a;b \u{65e5}\u{672c}\x1b\\");
    assert_eq!(titles(&mut t), ["a;b \u{65e5}\u{672c}"]);
}

#[test]
fn colour_requests_never_produce_a_response() {
    let mut t = upstream_handler();
    t.parse_str("\x1b]4;1;?\x07\x1b]10;?;?;?\x07\x1b]104\x07\x1b]110\x07");
    let events = t.handler.take_events();
    assert!(
        events.iter().all(|e| matches!(e, HostEvent::Color(_))),
        "{events:?}"
    );
    assert_eq!(events.len(), 1 + 3 + 1 + 1);
}

#[test]
fn osc_52_and_other_unregistered_oscs_produce_nothing() {
    let mut t = upstream_handler();
    t.parse_str("\x1b]52;c;aGk=\x07\x1b]52;c;?\x07\x1b]999;x\x07\x1b]7;file:///tmp\x07");
    assert!(t.handler.take_events().is_empty());
    t.parse_str("ok");
    assert_eq!(t.screen().first().map(String::as_str), Some("ok"));
}

#[test]
fn osc_8_stores_a_long_uri_untruncated() {
    let mut t = upstream_handler();
    let uri = format!("http://x/{}", "a".repeat(300_000));
    t.parse_str(&format!("\x1b]8;;{uri}\x07"));
    let url_id = t.attr().extended.url_id();
    assert_ne!(url_id, 0);
    assert_eq!(
        t.handler
            .services
            .osc_link
            .get_link_data(url_id)
            .map(|d| d.uri.len()),
        Some(uri.len())
    );
}

#[test]
fn osc_8_registers_after_forwarding_the_markers_disposed_earlier_in_the_parse() {
    // Upstream frees a link the moment the last marker of its lines is disposed, so an OSC 8
    // with the same id and uri afterwards creates a new link. Without the forward the stale
    // entry would be found and its id reused.
    let mut t = test_handler(10, 3, [OptionValue::Scrollback(2)]);
    t.parse_str("\x1b]8;id=a;u\x07x\x1b]8;;\x07");
    let first = t.attr().extended.url_id();
    assert_eq!(first, 0);
    t.parse_str("\n\n\n\n\n");
    t.parse_str("\x1b]8;id=a;u\x07");
    let second = t.attr().extended.url_id();
    assert_eq!(second, 2, "the first link was trimmed and freed");
    assert!(t.handler.services.osc_link.get_link_data(1).is_none());
    // control: without the trim the same id and uri is the same link
    let mut t = test_handler(10, 3, [OptionValue::Scrollback(2)]);
    t.parse_str("\x1b]8;id=a;u\x07x\x1b]8;;\x07\n");
    t.parse_str("\x1b]8;id=a;u\x07");
    assert_eq!(t.attr().extended.url_id(), 1);
}

#[test]
fn osc_8_print_adds_the_line_to_the_link() {
    let mut t = test_handler(5, 4, []);
    t.parse_str("\x1b]8;;u\x07ab\r\ncd\x1b]8;;\x07ef");
    let id = 1;
    assert_eq!(
        t.handler
            .services
            .osc_link
            .link_markers(id)
            .map(|m| m.len()),
        Some(2)
    );
    let cell_url = |t: &TestTerm, row: usize, x: usize| {
        let mut cell = CellData::new();
        t.buffer().lines.get(row).unwrap().load_cell(x, &mut cell);
        cell.extended.url_id()
    };
    assert_eq!((cell_url(&t, 0, 0), cell_url(&t, 1, 1)), (1, 1));
    assert_eq!(cell_url(&t, 1, 2), 0);
}

/// Review F1/S1: with a link open across the whole buffer (one marker per line), printing a
/// line inside it walks the link's markers once, when the cursor reaches the new line, not once
/// per character. Counts marker visits, not time.
#[test]
fn osc_8_print_inside_a_long_open_link_looks_up_markers_once_per_line() {
    let mut t = test_handler(80, 24, [OptionValue::Scrollback(1000)]);
    t.parse_str("\x1b]8;;http://x\x07");
    let line = format!("{}\r\n", "a".repeat(79));
    t.parse_str(&line.repeat(1100));
    let markers = t.handler.services.osc_link.marker_count();
    assert_eq!(markers, 1024, "one marker per live line");
    t.handler.services.osc_link.marker_visits = 0;
    t.parse_str(&"b".repeat(79));
    let visits = t.handler.services.osc_link.marker_visits;
    // the first character walks every marker (the new line holds none yet); the other 78 hit
    // the marker it added (upstream's walk: 79 x 1024)
    assert!(
        visits <= 2 + markers as u64 + 79,
        "{visits} marker visits for 79 characters over {markers} markers"
    );
    // and the line is in the link, once
    assert_eq!(t.handler.services.osc_link.marker_count(), 1024);
    // printing on a line the link already holds, after the cache moved elsewhere, still finds it
    t.parse_str("\x1b[1Ac\x1b[1Bd");
    assert_eq!(t.handler.services.osc_link.marker_count(), 1024);
}

/// Review S7: REP inside an open link adds each line it prints on once (per wrap), not once per
/// code point: the marker lookups of a REP over 20 lines stay at a few per line where the
/// per code point adds of `print` cost one per code point (12,000 here).
#[test]
fn rep_inside_an_open_link_looks_up_markers_once_per_line() {
    let mut t = test_handler(300, 5, [OptionValue::Scrollback(0)]);
    t.parse_str("\x1b]8;;http://x\x07e\u{301}");
    t.handler.services.osc_link.marker_visits = 0;
    let count = 300 * 20;
    t.parse_str(&format!("\x1b[{count}b"));
    assert_eq!(t.handler.rep_fills, 1);
    let visits = t.handler.services.osc_link.marker_visits;
    let markers = t.handler.services.osc_link.marker_count() as u64;
    let lines = (count / 300 + 1) as u64;
    // two adds per line at most, each the cached marker and at most one scan of the markers
    assert!(
        visits <= 2 * lines * (markers + 2),
        "{visits} marker visits for {lines} lines over {markers} markers"
    );
    assert!(
        (5..=6).contains(&markers),
        "every line of the buffer is in the link: {markers}"
    );
}

/// Review S7 residual 1: a REP that scrolls a full scrollback inside an open link visits a
/// few markers per printed line in total (link lookups plus marker-set events: one trimmed
/// marker and one added line each), where the walks of every marker made it O(lines x
/// markers). Counts visits, not time.
#[test]
fn rep_inside_an_open_link_over_a_full_scrollback_visits_few_markers_per_line() {
    let mut t = test_handler(80, 24, [OptionValue::Scrollback(1000)]);
    t.parse_str("\x1b]8;;http://x\x07");
    t.parse_str(&format!("{}\r\n", "a".repeat(79)).repeat(1100));
    t.parse_str("a");
    let markers_before = t.handler.services.osc_link.marker_count();
    assert_eq!(markers_before, 1024, "the link holds every line");
    let reset = |t: &mut TestTerm| {
        t.handler.services.osc_link.marker_visits = 0;
        let buffers = &mut t.handler.services.buffer.buffers;
        buffers.normal_mut().lines.observer_mut().visits = 0;
        buffers.alt_mut().lines.observer_mut().visits = 0;
    };
    reset(&mut t);
    let count = 1_048_576u64;
    t.parse_str(&format!("\x1b[{count}b"));
    let buffers = &t.handler.services.buffer.buffers;
    let visits = t.handler.services.osc_link.marker_visits
        + buffers.normal().markers().visits
        + buffers.alt().markers().visits;
    let lines = count / 80 + 1;
    assert!(
        visits <= 8 * lines,
        "{visits} marker visits for {lines} printed lines"
    );
    assert_eq!(t.handler.services.osc_link.marker_count(), 1024);
    assert_eq!(t.buffer().markers().len(), 1024);
}

/// Plan M7 step 12 (S7 residual 2): a narrow-cluster REP stores each line's combined strings
/// as at most two runs sharing one allocation, and allocates one string per line, not one per
/// cell (a `String` per cell before). Counts allocations and references, not time.
#[test]
fn rep_of_a_narrow_cluster_shares_one_string_per_line() {
    use crate::buffer::buffer_line::combined_allocations;
    use std::sync::Arc;
    let mut t = test_handler(80, 24, [OptionValue::Scrollback(1000)]);
    t.parse_str("e\u{301}");
    let before = combined_allocations();
    let k = 10;
    // the rest of line 0, k - 1 full lines and one cell of line k
    t.parse_str(&format!("\x1b[{}b", 80 * k));
    assert_eq!(t.handler.rep_fills, 1);
    let allocations = combined_allocations() - before;
    // the REP's head cell, then one string per line
    assert!(
        allocations <= k as u64 + 2,
        "{allocations} combined strings allocated for {} lines",
        k + 1
    );
    let buffer = t.buffer();
    for row in 0..=k {
        let line = buffer.lines.get(row).unwrap();
        let runs = line.combined_runs();
        assert!(
            (1..=2).contains(&runs.len()),
            "row {row}: {} runs",
            runs.len()
        );
        let shared = &runs[0].2;
        assert_eq!(&**shared, "e\u{301}");
        for x in (0..line.len()).filter(|&x| line.is_combined(x) != 0) {
            let run = runs
                .iter()
                .find(|&&(first, last, _)| first <= x && x <= last);
            assert!(
                run.is_some_and(|(_, _, chars)| Arc::ptr_eq(chars, shared)),
                "row {row} cell {x}"
            );
        }
        for (_, _, chars) in &runs {
            // held by the line's run and by this copy only, however many cells it covers
            assert_eq!(Arc::strong_count(chars), 2, "row {row}");
        }
    }
}

/// Review S2: links without an id repeated on one line (and overwritten) stay capped, with
/// their markers; the newest link still works and an evicted one's cells lose only the link.
#[test]
fn osc_8_links_repeated_on_one_line_stay_capped() {
    use crate::services::osc_link_service::{MAX_LINK_MARKERS, MAX_LINKS};
    let mut t = test_handler(80, 24, []);
    t.parse_str("\x1b]8;;first\x07x\r");
    t.parse_str(&"\x1b]8;;a\x07x\r".repeat(MAX_LINKS + 500));
    let osc = &t.handler.services.osc_link;
    assert_eq!(osc.len(), MAX_LINKS);
    assert!(osc.marker_count() <= MAX_LINK_MARKERS);
    assert_eq!(t.buffer().markers().len(), osc.marker_count());
    assert!(osc.get_link_data(1).is_none(), "the first link was evicted");
    let current = t.attr().extended.url_id();
    assert_eq!(
        osc.get_link_data(current).map(|d| d.uri.as_str()),
        Some("a")
    );
}

/// Review S4: combining marks printed onto one cell stop at the cap; the cursor is unaffected.
#[test]
fn print_caps_the_combining_marks_of_one_cell() {
    use crate::buffer::buffer_line::MAX_COMBINED_CODE_POINTS;
    let mut t = test_handler(10, 3, []);
    t.parse_str(&format!("a{}b", "\u{301}".repeat(10_000)));
    let text = t.cell(0, 0).0;
    assert_eq!(text.chars().count(), MAX_COMBINED_CODE_POINTS);
    assert_eq!(t.cell(0, 1).0, "b");
    assert_eq!(t.cursor(), [2, 0]);
}

#[test]
fn decrqss_unknown_requests_get_the_invalid_reply_and_stdin_can_be_disabled() {
    let mut t = upstream_handler();
    for body in ["x", "", "rr", "$", "\"q\"q", "1;2r", "0m", "  q"] {
        t.parse_str(&format!("\x1bP$q{body}\x1b\\"));
    }
    assert_eq!(responses(&mut t), ["\x1bP0$r\x1b\\"; 8]);
    // DECSCL always reports 61;1
    t.parse_str("\x1bP$q\"p\x1b\\");
    assert_eq!(responses(&mut t), ["\x1bP1$r61;1\"p\x1b\\"]);
    // not a DECRQSS: a DCS with another final byte or intermediates reaches the fallback only
    t.parse_str("\x1bP+q\x1b\\\x1bPq\x1b\\\x1bP$p\x1b\\");
    assert!(t.handler.take_events().is_empty());
    let mut t = test_handler(10, 4, [OptionValue::DisableStdin(true)]);
    t.parse_str("\x1bP$q\"q\x1b\\");
    assert!(t.handler.take_events().is_empty());
}

#[test]
fn decrqss_reports_the_cursor_style_option_and_the_margins() {
    use crate::services::options_service::CursorStyle as Style;
    for (style, blink, expected) in [
        (Style::Block, false, "2"),
        (Style::Block, true, "1"),
        (Style::Underline, false, "4"),
        (Style::Underline, true, "3"),
        (Style::Bar, false, "6"),
        (Style::Bar, true, "5"),
    ] {
        let mut t = test_handler(
            10,
            4,
            [
                OptionValue::CursorStyle(style),
                OptionValue::CursorBlink(blink),
            ],
        );
        t.parse_str("\x1bP$q q\x1b\\");
        assert_eq!(responses(&mut t), [format!("\x1bP1$r{expected} q\x1b\\")]);
    }
    let mut t = test_handler(10, 6, []);
    t.parse_str("\x1b[2;4r\x1bP$qr\x1b\\");
    assert_eq!(responses(&mut t), ["\x1bP1$r2;4r\x1b\\"]);
    t.parse_str("\x1b[1m\x1bP$qm\x1b\\");
    assert_eq!(responses(&mut t), ["\x1bP1$r0m\x1b\\"]);
}

#[test]
fn osc_and_dcs_split_at_every_byte_equal_the_unsplit_run() {
    let input = "\x1b]0;ti\u{65e5}tle\x07\x1b]4;1;?;2;rgb:1/2/3\x07\x1b]8;id=x;http://a;b\x07ab\x1b]8;;\x07\x1b]104\x07\
                 \x1bP$q\"q\x1b\\\x1b]11;?;#fff\x1b\\\x1b]2;end\x07";
    let bytes = input.as_bytes();
    let run = |chunks: &[&[u8]]| {
        let mut t = upstream_handler();
        for chunk in chunks {
            t.parse_bytes(chunk);
        }
        (
            t.handler.take_events(),
            t.handler.cur_attr_data.extended.url_id(),
            t.screen().first().cloned(),
        )
    };
    let expected = run(&[bytes]);
    assert!(!expected.0.is_empty());
    let singles: Vec<&[u8]> = bytes.chunks(1).collect();
    assert_eq!(run(&singles), expected);
    for cut in 1..bytes.len() {
        let (a, b) = bytes.split_at(cut);
        assert_eq!(run(&[a, b]), expected, "cut at {cut}");
    }
}

/// Expected values from `@xterm/headless` 6.0.0 (`_core._inputHandler.onColor` /
/// `onTitleChange`, `_curAttrData`, `_oscLinkService`, `coreService.onData`), one run per entry
/// (scratch script `s8/run.mjs`). The inputs spell ESC as `\e`, BEL as `\a`, ST as `\s`.
const OSC_ORACLE: &[(&[&str], &str)] = &[
    (
        &["\\e]4;0;?;123;?\\a"],
        "ev=C[R0,R123];url=0;links=;title=;icon=",
    ),
    (&["\\e]4;256;?\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]4;255;?\\a"], "ev=C[R255];url=0;links=;title=;icon="),
    (
        &["\\e]4;00;?;0001;?\\a"],
        "ev=C[R0,R1];url=0;links=;title=;icon=",
    ),
    (&["\\e]4;+1;?\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]4; 1;?\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]4;1\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]4;1;?;2\\a"], "ev=C[R1];url=0;links=;title=;icon="),
    (
        &["\\e]4;99999999999999999999999;?\\a"],
        "ev=;url=0;links=;title=;icon=",
    ),
    (
        &["\\e]4;1;rgb:ff/00/00;2;bad;3;#fff\\a"],
        "ev=C[S1=ff0000,S3=f0f0f0];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]4;255;?;;?\\a"],
        "ev=C[R255];url=0;links=;title=;icon=",
    ),
    (&["\\e]4;;?\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]4\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]4;\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\e]4;1;#abc;2;rgb:1/2/3;3;rgb:11/22/33;4;#aabbccdd\\a"],
        "ev=C[S1=a0b0c0,S2=112233,S3=112233];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]4;1;RGB:AA/BB/CC;2;#ABCDEF\\a"],
        "ev=C[S1=aabbcc,S2=abcdef];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]10;?;?;?;?\\a"],
        "ev=C[R256]|C[R257]|C[R258];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]11;?;bad;#fff\\a"],
        "ev=C[R257];url=0;links=;title=;icon=",
    ),
    (&["\\e]12;?;?\\a"], "ev=C[R258];url=0;links=;title=;icon="),
    (&["\\e]10;\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]10;;?\\a"], "ev=C[R257];url=0;links=;title=;icon="),
    (
        &["\\e]10;?;;?\\a"],
        "ev=C[R256]|C[R258];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]11;#102030;?;?\\a"],
        "ev=C[S257=102030]|C[R258];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]12;rgb:0/0/0\\a"],
        "ev=C[S258=000000];url=0;links=;title=;icon=",
    ),
    (&["\\e]104\\a"], "ev=C[X];url=0;links=;title=;icon="),
    (&["\\e]104;\\a"], "ev=C[X];url=0;links=;title=;icon="),
    (
        &["\\e]104;1;x;300;255;256;007\\a"],
        "ev=C[X1,X255,X7];url=0;links=;title=;icon=",
    ),
    (&["\\e]104;;1\\a"], "ev=C[X1];url=0;links=;title=;icon="),
    (&["\\e]104;x\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\e]110;foo\\a\\e]111;\\a\\e]112;1;2\\a"],
        "ev=C[X256]|C[X257]|C[X258];url=0;links=;title=;icon=",
    ),
    (
        &["\\e]0;a;b\\a"],
        "ev=T(a;b);url=0;links=;title=a;b;icon=a;b",
    ),
    (&["\\e]2;\\a"], "ev=T();url=0;links=;title=;icon="),
    (&["\\e]1;x\\a"], "ev=;url=0;links=;title=;icon=x"),
    (
        &["\\e]0;日本\\a"],
        "ev=T(\\u{65e5}\\u{672c});url=0;links=;title=\\u{65e5}\\u{672c};icon=\\u{65e5}\\u{672c}",
    ),
    (
        &["\\e]2;t1\\s\\e]0;t2\\a"],
        "ev=T(t1)|T(t2);url=0;links=;title=t2;icon=t2",
    ),
    (
        &["\\e]8;id=a;http://x\\a"],
        "ev=;url=1;links=1:a:http://x;title=;icon=",
    ),
    (
        &["\\e]8;a=b:id=c:d=e;uri\\a"],
        "ev=;url=1;links=1:c:uri;title=;icon=",
    ),
    (
        &["\\e]8;id=;uri\\a"],
        "ev=;url=1;links=1:-:uri;title=;icon=",
    ),
    (
        &["\\e]8;id=a:id=b;u\\a"],
        "ev=;url=1;links=1:a:u;title=;icon=",
    ),
    (&["\\e]8; id=a ;u\\a"], "ev=;url=1;links=1:a:u;title=;icon="),
    (&["\\e]8;x;\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\e]8;x;\\a\\e]8;;u\\a\\e]8;x;\\a"],
        "ev=;url=1;links=1:-:u;title=;icon=",
    ),
    (&["\\e]8;\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]8\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]8;;\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\e]8;;u;v;w\\a"],
        "ev=;url=1;links=1:-:u;v;w;title=;icon=",
    ),
    (
        &["\\e]8;id=a;u\\ahi\\e]8;id=a;u\\a"],
        "ev=;url=1;links=1:a:u;title=;icon=",
    ),
    (
        &["\\e]8;id=a;u\\ahi\\e]8;id=a;u2\\a"],
        "ev=;url=2;links=1:a:u,2:a:u2;title=;icon=",
    ),
    (
        &["\\e]8;;u1\\a\\e]8;;u2\\a"],
        "ev=;url=2;links=1:-:u1,2:-:u2;title=;icon=",
    ),
    (
        &["\\e]8;;u1\\ax\\e]8;;\\ay\\e]8;;u1\\az"],
        "ev=;url=2;links=1:-:u1,2:-:u1;title=;icon=",
    ),
    (&["\\e]8;:id=q;u\\a"], "ev=;url=1;links=1:q:u;title=;icon="),
    (&["\\e]8;ID=q;u\\a"], "ev=;url=1;links=1:-:u;title=;icon="),
    (
        &["\\e]8;id=q=r;u\\a"],
        "ev=;url=1;links=1:q=r:u;title=;icon=",
    ),
    (
        &["\\e]52;c;aGk=\\a\\e]52;c;?\\a"],
        "ev=;url=0;links=;title=;icon=",
    ),
    (&["\\e]999;x\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &[
            "\\eP$q\"q\\s",
            "\\eP$q\"p\\s",
            "\\eP$qr\\s",
            "\\eP$qm\\s",
            "\\eP$q q\\s",
            "\\eP$qx\\s",
            "\\eP$q\\s",
        ],
        "ev=D(\\u{1b}P1$r0\"q\\u{1b}\\u{5c})|D(\\u{1b}P1$r61;1\"p\\u{1b}\\u{5c})|D(\\u{1b}P1$r1;24r\\u{1b}\\u{5c})|D(\\u{1b}P1$r0m\\u{1b}\\u{5c})|D(\\u{1b}P1$r2 q\\u{1b}\\u{5c})|D(\\u{1b}P0$r\\u{1b}\\u{5c})|D(\\u{1b}P0$r\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\e[1\"q\\eP$q\"q\\s\\e[2\"q\\eP$q\"q\\s\\e[0\"q\\eP$q\"q\\s"],
        "ev=D(\\u{1b}P1$r1\"q\\u{1b}\\u{5c})|D(\\u{1b}P1$r0\"q\\u{1b}\\u{5c})|D(\\u{1b}P1$r0\"q\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\e[5;10r\\eP$qr\\s\\e[r\\eP$qr\\s\\e[3;3r\\eP$qr\\s"],
        "ev=D(\\u{1b}P1$r5;10r\\u{1b}\\u{5c})|D(\\u{1b}P1$r1;24r\\u{1b}\\u{5c})|D(\\u{1b}P1$r1;24r\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\e[1m\\eP$qm\\s\\e[0m\\eP$qm\\s"],
        "ev=D(\\u{1b}P1$r0m\\u{1b}\\u{5c})|D(\\u{1b}P1$r0m\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\e[3 q\\eP$q q\\s"],
        "ev=D(\\u{1b}P1$r2 q\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\eP$q q\\s\\eP1$q q\\s"],
        "ev=D(\\u{1b}P1$r2 q\\u{1b}\\u{5c})|D(\\u{1b}P1$r2 q\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\eP$qrr\\s", "\\eP$q\"q\"q\\s"],
        "ev=D(\\u{1b}P0$r\\u{1b}\\u{5c})|D(\\u{1b}P0$r\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\eP$q\"q", "\\s"],
        "ev=D(\\u{1b}P1$r0\"q\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (
        &["\\eP$q\"", "q\\s"],
        "ev=D(\\u{1b}P1$r0\"q\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (&["\\eP$q\"q\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\eP+q\\s\\eP$q\"q\\s"],
        "ev=D(\\u{1b}P1$r0\"q\\u{1b}\\u{5c});url=0;links=;title=;icon=",
    ),
    (&["\\e]8; id=a﻿;u\\a"], "ev=;url=1;links=1:a:u;title=;icon="),
    (
        &["\\e]8; id=a　;u\\a"],
        "ev=;url=1;links=1:a:u;title=;icon=",
    ),
    (&["\\e]8;id=a;u\\a"], "ev=Bell;url=0;links=;title=;icon="),
    (&["\\e]8; 　;\\a"], "ev=;url=0;links=;title=;icon="),
    (&["\\e]8;  ;\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\e]8;id=a;u\\a\\e]8; ;\\a"],
        "ev=;url=0;links=1:a:u;title=;icon=",
    ),
    (&["\\e]8;᠎;\\a"], "ev=;url=0;links=;title=;icon="),
    (
        &["\\e]8;id=a;u\\a\\e]8;;\\a"],
        "ev=Bell;url=1;links=1:a:u;title=;icon=",
    ),
];

fn oracle_escape(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if (' '..'\u{7f}').contains(&ch) && ch != '\\' {
            out.push(ch);
        } else {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(ch)));
        }
    }
    out
}

fn oracle_summary(t: &mut TestTerm) -> String {
    let events: Vec<String> = t
        .handler
        .take_events()
        .into_iter()
        .map(|event| match event {
            HostEvent::TitleChanged => format!("T({})", oracle_escape(t.handler.window_title())),
            HostEvent::Response(bytes) => {
                format!("D({})", oracle_escape(&String::from_utf8_lossy(&bytes)))
            }
            HostEvent::Color(requests) => {
                let items: Vec<String> = requests
                    .iter()
                    .map(|request| {
                        let index = |i: &ColorIndex| match i {
                            ColorIndex::Palette(n) => n.to_string(),
                            ColorIndex::Foreground => "256".to_owned(),
                            ColorIndex::Background => "257".to_owned(),
                            ColorIndex::Cursor => "258".to_owned(),
                        };
                        match request {
                            Report(i) => format!("R{}", index(i)),
                            Set(i, c) => {
                                format!("S{}={:02x}{:02x}{:02x}", index(i), c[0], c[1], c[2])
                            }
                            Restore(i) => format!("X{}", i.as_ref().map(index).unwrap_or_default()),
                        }
                    })
                    .collect();
                format!("C[{}]", items.join(","))
            }
            other => format!("{other:?}"),
        })
        .collect();
    let links: Vec<String> = (1..20)
        .filter_map(|id| {
            t.handler.services.osc_link.get_link_data(id).map(|d| {
                format!(
                    "{id}:{}:{}",
                    d.id.as_deref().map_or("-".to_owned(), oracle_escape),
                    oracle_escape(&d.uri)
                )
            })
        })
        .collect();
    format!(
        "ev={};url={};links={};title={};icon={}",
        events.join("|"),
        t.handler.cur_attr_data.extended.url_id(),
        links.join(","),
        oracle_escape(t.handler.window_title()),
        oracle_escape(&t.handler.icon_name),
    )
}

#[test]
fn osc_and_decrqss_equal_the_oracle() {
    for (inputs, expected) in OSC_ORACLE {
        let mut t = upstream_handler();
        t.resize(80, 24);
        for input in *inputs {
            let input = input
                .replace("\\e", "\x1b")
                .replace("\\a", "\x07")
                .replace("\\s", "\x1b\\");
            t.parse_str(&input);
        }
        let expected = coalesce_oracle_titles(expected);
        assert_eq!(oracle_summary(&mut t), expected, "inputs {inputs:?}");
    }
}

/// The oracle's events with the Q10/Q11 title rule applied (D2 amendment): one `T(..)` per
/// drain, at the first title's place; `oracle_summary` prints the title read at drain time
/// (`TitleChanged` carries none), which is the last one set. Upstream fires one `onTitleChange` per
/// change; the oracle rows keep that record. (No title in `OSC_ORACLE` contains `|`.)
fn coalesce_oracle_titles(expected: &str) -> String {
    let Some(rest) = expected.strip_prefix("ev=") else {
        return expected.to_owned();
    };
    let (events, tail) = rest.split_once(";url=").unwrap();
    let mut events: Vec<&str> = events.split('|').collect();
    let titles: Vec<usize> = (0..events.len())
        .filter(|&i| events[i].starts_with("T("))
        .collect();
    if let (Some(&first), Some(&last)) = (titles.first(), titles.last()) {
        events[first] = events[last];
        for &i in titles.iter().skip(1).rev() {
            events.remove(i);
        }
    }
    format!("ev={};url={tail}", events.join("|"))
}

#[test]
fn coalesce_oracle_titles_keeps_the_last_title_at_the_first_place() {
    assert_eq!(
        coalesce_oracle_titles("ev=T(a)|D(x)|T(b)|T(c);url=0;links=;title=c;icon="),
        "ev=T(c)|D(x);url=0;links=;title=c;icon="
    );
    assert_eq!(
        coalesce_oracle_titles("ev=D(x);url=0;links=;title=;icon="),
        "ev=D(x);url=0;links=;title=;icon="
    );
}

/// Review S6: IL, DL, SU and SD move the markers once per sequence, not once per shifted line
/// (one walk of the marker list instead of `2 * height`), and the markers end where the
/// per-line splices put them.
#[test]
fn line_insert_delete_and_scroll_walk_the_markers_once_per_sequence() {
    let rows = 20;
    let mut t = test_handler(10, rows, []);
    t.parse_str(&format!("\x1b[2;{}r", rows - 1));
    let markers = 300;
    let buffer = t.handler.services.buffer.buffer_mut();
    for i in 0..markers {
        buffer.add_marker(i64::from(i % rows as u32));
    }
    let before: Vec<i64> = t.buffer().markers().iter().map(|m| m.line).collect();
    for (seq, expect) in [
        // IL 99 at row 5 (index 4): 15 lines of the region shift down, the region is blanked
        // from the cursor; markers on rows 5..19 (indices 4..=18) are all disposed
        ("\x1b[5;1H\x1b[99L", 4..=18),
        ("\x1b[5;1H\x1b[99M", 4..=18),
        ("\x1b[99S", 1..=18),
        ("\x1b[99T", 1..=18),
    ] {
        let mut t2 = test_handler(10, rows, []);
        t2.parse_str(&format!("\x1b[2;{}r", rows - 1));
        for &line in &before {
            t2.handler.services.buffer.buffer_mut().add_marker(line);
        }
        t2.handler
            .services
            .buffer
            .buffer_mut()
            .lines
            .observer_mut()
            .visits = 0;
        t2.parse_str(seq);
        let set = t2.buffer().markers();
        assert!(
            set.visits <= markers as u64,
            "{seq:?}: {} visits for {markers} markers",
            set.visits
        );
        assert!(
            set.iter().all(|m| !expect.contains(&m.line)),
            "{seq:?}: a marker survived in the cleared rows"
        );
        let outside = before.iter().filter(|l| !(1..=18).contains(*l)).count();
        assert!(
            set.len() >= outside,
            "{seq:?}: markers outside the region survive"
        );
    }
}
