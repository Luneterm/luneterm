//! Port of the `BufferLine` describe of xterm.js src/common/buffer/BufferLine.test.ts
//! (`:163-810`) @ f447274, plus Rust-only cases for the typed-array edge semantics (plan M5 D7).
//!
//! All 49 test cases of that file are accounted for between this file, `attribute_data_tests.rs`
//! (the 6 `AttributeData` cases, `:27-136`) and `cell_data_tests.rs` (`CellData`, `:137-161`):
//! `ctor`, `insertCells`, `deleteCells`, `replaceCells`, `fill`, `clone`, `copyFrom`, `should
//! support combining chars`, 5 `resize`, 5 `getTrimLength`, 8 `translateToString`, 3
//! `addCharToCell`, 5 `correct fullwidth handling` and 8 `extended attributes` cases.
//!
//! `adapted:` the `TestBufferLine` subclass's `combined` getter and `toArray` become direct field
//! access and [`to_array`]; `(line as any)._extendedAttrs[i]` becomes `line.extended_attrs`.
//! Upstream's reference-identity assertions (`assert.equal(ext[1], ext[2])` /
//! `assert.notEqual(ext[1], ext[3])`) become value (in)equality: `ExtendedAttrs` is a `Copy`
//! value here and the two compared values differ in underline style.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::buffer::constants::{
    BgFlags, Content, DEFAULT_ATTR, NULL_CELL_CHAR, NULL_CELL_CODE, NULL_CELL_WIDTH, UnderlineStyle,
};

fn cd(attr: u32, chars: &str, width: u32, code: u32) -> CharData {
    (attr, chars.to_string(), width, code)
}

/// `'x'.charCodeAt(index)`.
fn unit(s: &str, index: usize) -> u32 {
    u32::from(s.encode_utf16().nth(index).unwrap())
}

/// `'x'.charCodeAt(0)`.
fn cc(s: &str) -> u32 {
    unit(s, 0)
}

fn cell(attr: u32, chars: &str, width: u32, code: u32) -> CellData {
    CellData::from_char_data(&cd(attr, chars, width, code))
}

/// `CellData.fromCharData([1, 'a', 0, 'a'.charCodeAt(0)])`-style cell: `ch` with its own code.
fn cell_of(attr: u32, ch: &str, width: u32) -> CellData {
    cell(attr, ch, width, cc(ch))
}

fn null_cell() -> CellData {
    cell(
        DEFAULT_ATTR,
        NULL_CELL_CHAR,
        NULL_CELL_WIDTH,
        NULL_CELL_CODE,
    )
}

/// `new TestBufferLine(cols, CellData.fromCharData([DEFAULT_ATTR, NULL_CELL_CHAR, NULL_CELL_WIDTH,
/// NULL_CELL_CODE]), false)`.
fn null_line(cols: usize) -> BufferLine {
    BufferLine::new(cols, Some(&null_cell()), false)
}

/// `new TestBufferLine(cols, CellData.fromCharData([DEFAULT_ATTR, NULL_CELL_CHAR, 0,
/// NULL_CELL_CODE]), false)`.
fn zero_width_line(cols: usize) -> BufferLine {
    let fill = cell(DEFAULT_ATTR, NULL_CELL_CHAR, 0, NULL_CELL_CODE);
    BufferLine::new(cols, Some(&fill), false)
}

fn to_array(line: &BufferLine) -> Vec<CharData> {
    (0..line.len())
        .map(|i| {
            let mut c = CellData::new();
            line.load_cell(i, &mut c);
            c.get_as_char_data()
        })
        .collect()
}

fn text(line: &BufferLine) -> String {
    line.translate_to_string(false, None, None, None)
}

fn tts(
    line: &BufferLine,
    trim_right: bool,
    start: Option<usize>,
    end: Option<usize>,
    columns: &mut Vec<usize>,
) -> String {
    line.translate_to_string(trim_right, start, end, Some(columns))
}

fn a_cell() -> CellData {
    cell_of(1, "a", 1)
}

mod buffer_line {
    use super::*;

    #[test]
    fn ctor() {
        let mut line = BufferLine::new(0, None, false);
        assert_eq!(line.len(), 0);
        assert!(!line.is_wrapped);
        line = BufferLine::new(10, None, false);
        assert_eq!(line.len(), 10);
        let mut c = CellData::new();
        assert_eq!(
            line.load_cell(0, &mut c).get_as_char_data(),
            cd(0, NULL_CELL_CHAR, NULL_CELL_WIDTH, NULL_CELL_CODE)
        );
        assert!(!line.is_wrapped);
        line = BufferLine::new(10, None, true);
        assert_eq!(line.len(), 10);
        assert_eq!(
            line.load_cell(0, &mut c).get_as_char_data(),
            cd(0, NULL_CELL_CHAR, NULL_CELL_WIDTH, NULL_CELL_CODE)
        );
        assert!(line.is_wrapped);
        line = BufferLine::new(10, Some(&cell_of(123, "a", 456)), true);
        assert_eq!(line.len(), 10);
        assert_eq!(
            line.load_cell(0, &mut c).get_as_char_data(),
            cd(123, "a", 456, cc("a"))
        );
        assert!(line.is_wrapped);
    }

    #[test]
    fn insert_cells() {
        let mut line = BufferLine::new(3, None, false);
        line.set_cell(0, &cell_of(1, "a", 0));
        line.set_cell(1, &cell_of(2, "b", 0));
        line.set_cell(2, &cell_of(3, "c", 0));
        line.insert_cells(1, 3, &cell_of(4, "d", 0));
        assert_eq!(
            to_array(&line),
            vec![
                cd(1, "a", 0, cc("a")),
                cd(4, "d", 0, cc("d")),
                cd(4, "d", 0, cc("d")),
            ]
        );
    }

    fn abcde() -> BufferLine {
        let mut line = BufferLine::new(5, None, false);
        for (i, ch) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            line.set_cell(i, &cell_of(i as u32 + 1, ch, 0));
        }
        line
    }

    #[test]
    fn delete_cells() {
        let mut line = abcde();
        line.delete_cells(1, 2, &cell_of(6, "f", 0));
        assert_eq!(
            to_array(&line),
            vec![
                cd(1, "a", 0, cc("a")),
                cd(4, "d", 0, cc("d")),
                cd(5, "e", 0, cc("e")),
                cd(6, "f", 0, cc("f")),
                cd(6, "f", 0, cc("f")),
            ]
        );
    }

    #[test]
    fn replace_cells() {
        let mut line = abcde();
        line.replace_cells(2, 4, &cell_of(6, "f", 0), false);
        assert_eq!(
            to_array(&line),
            vec![
                cd(1, "a", 0, cc("a")),
                cd(2, "b", 0, cc("b")),
                cd(6, "f", 0, cc("f")),
                cd(6, "f", 0, cc("f")),
                cd(5, "e", 0, cc("e")),
            ]
        );
    }

    #[test]
    fn fill() {
        let mut line = abcde();
        line.fill(&cell_of(123, "z", 0), false);
        assert_eq!(to_array(&line), vec![cd(123, "z", 0, cc("z")); 5]);
    }

    #[test]
    fn clone() {
        let mut line = BufferLine::new(5, None, true);
        for (i, ch) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            line.set_cell(i, &cell_of(i as u32 + 1, ch, 0));
        }
        let line2 = line.clone();
        assert_eq!(to_array(&line2), to_array(&line));
        assert_eq!(line2.len(), line.len());
        assert_eq!(line2.is_wrapped, line.is_wrapped);
    }

    #[test]
    fn copy_from() {
        let line = abcde();
        let mut line2 = BufferLine::new(5, Some(&cell_of(1, "a", 0)), true);
        line2.copy_from(&line);
        assert_eq!(to_array(&line2), to_array(&line));
        assert_eq!(line2.len(), line.len());
        assert_eq!(line2.is_wrapped, line.is_wrapped);
    }

    #[test]
    fn should_support_combining_chars() {
        // CHAR_DATA_CODE_INDEX resembles current behavior in InputHandler.print
        // --> set code to the last charCodeAt value of the string
        let acute = "e\u{301}";
        let line = BufferLine::new(2, Some(&cell(1, acute, 0, cc("\u{301}"))), false);
        assert_eq!(
            to_array(&line),
            vec![
                cd(1, acute, 0, cc("\u{301}")),
                cd(1, acute, 0, cc("\u{301}"))
            ]
        );
        let mut line2 = BufferLine::new(5, Some(&cell(1, "a", 0, cc("\u{301}"))), true);
        line2.copy_from(&line);
        assert_eq!(to_array(&line2), to_array(&line));
        let line3 = line.clone();
        assert_eq!(to_array(&line3), to_array(&line));
    }

    mod resize {
        use super::*;

        #[test]
        fn enlarge_false() {
            let mut line = BufferLine::new(5, Some(&cell_of(1, "a", 0)), false);
            line.resize(10, &cell_of(1, "a", 0));
            assert_eq!(to_array(&line), vec![cd(1, "a", 0, cc("a")); 10]);
        }

        #[test]
        fn enlarge_true() {
            let mut line = BufferLine::new(5, Some(&cell_of(1, "a", 0)), false);
            line.resize(10, &cell_of(1, "a", 0));
            assert_eq!(to_array(&line), vec![cd(1, "a", 0, cc("a")); 10]);
        }

        #[test]
        fn shrink_true_should_apply_new_size() {
            let mut line = BufferLine::new(10, Some(&cell_of(1, "a", 0)), false);
            line.resize(5, &cell_of(1, "a", 0));
            assert_eq!(to_array(&line), vec![cd(1, "a", 0, cc("a")); 5]);
        }

        #[test]
        fn shrink_to_0_length() {
            let mut line = BufferLine::new(10, Some(&cell_of(1, "a", 0)), false);
            line.resize(0, &cell_of(1, "a", 0));
            assert_eq!(to_array(&line), Vec::<CharData>::new());
        }

        #[test]
        fn should_remove_combining_data_on_replaced_cells_after_shrinking_then_enlarging() {
            let mut line = BufferLine::new(10, Some(&cell_of(1, "a", 0)), false);
            line.set(2, &cd(0, "😁", 1, cc("😁")));
            line.set(9, &cd(0, "😁", 1, cc("😁")));
            assert_eq!(text(&line), "aa😁aaaaaa😁");
            assert_eq!(line.combined.len(), 2);
            line.resize(5, &cell_of(1, "a", 0));
            assert_eq!(text(&line), "aa😁aa");
            line.resize(10, &cell_of(1, "a", 0));
            assert_eq!(text(&line), "aa😁aaaaaaa");
            assert_eq!(line.combined.len(), 1);
        }
    }

    mod get_trim_length {
        use super::*;

        #[test]
        fn empty_line() {
            assert_eq!(null_line(10).get_trimmed_length(), 0);
        }

        #[test]
        fn ascii() {
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            line.set_cell(2, &a_cell());
            assert_eq!(line.get_trimmed_length(), 3);
        }

        #[test]
        fn surrogate() {
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            line.set_cell(2, &cell(1, "𝄞", 1, cc("𝄞")));
            assert_eq!(line.get_trimmed_length(), 3);
        }

        #[test]
        fn combining() {
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            line.set_cell(2, &cell(1, "e\u{301}", 1, cc("\u{301}")));
            assert_eq!(line.get_trimmed_length(), 3);
        }

        #[test]
        fn fullwidth() {
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            line.set_cell(2, &cell(1, "１", 2, cc("１")));
            line.set_cell(3, &cell(0, "", 0, 0));
            assert_eq!(line.get_trimmed_length(), 4); // also counts null cell after fullwidth
        }
    }

    mod translate_to_string_with_and_without_trimming {
        use super::*;

        #[test]
        fn empty_line() {
            let mut columns = Vec::new();
            let line = null_line(10);
            assert_eq!(tts(&line, false, None, None, &mut columns), "          ");
            assert_eq!(columns, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
            assert_eq!(tts(&line, true, None, None, &mut columns), "");
            assert_eq!(columns, vec![0]);
        }

        #[test]
        fn ascii() {
            let mut columns = Vec::new();
            let mut line = null_line(10);
            for i in [0, 2, 4, 5] {
                line.set_cell(i, &a_cell());
            }
            assert_eq!(tts(&line, false, None, None, &mut columns), "a a aa    ");
            assert_eq!(columns, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
            assert_eq!(tts(&line, true, None, None, &mut columns), "a a aa");
            assert_eq!(columns, vec![0, 1, 2, 3, 4, 5, 6]);
            for trim_right in [true, false] {
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(5), &mut columns),
                    "a a a"
                );
                assert_eq!(columns, vec![0, 1, 2, 3, 4, 5]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(4), &mut columns),
                    "a a "
                );
                assert_eq!(columns, vec![0, 1, 2, 3, 4]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(3), &mut columns),
                    "a a"
                );
                assert_eq!(columns, vec![0, 1, 2, 3]);
            }
        }

        #[test]
        fn surrogate() {
            let mut columns = Vec::new();
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            for i in [2, 4, 5] {
                line.set_cell(i, &cell(1, "𝄞", 1, cc("𝄞")));
            }
            assert_eq!(tts(&line, false, None, None, &mut columns), "a 𝄞 𝄞𝄞    ");
            // `out_columns` index UTF-16 code units: the astral char takes two entries.
            assert_eq!(columns, vec![0, 1, 2, 2, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10]);
            assert_eq!(tts(&line, true, None, None, &mut columns), "a 𝄞 𝄞𝄞");
            assert_eq!(columns, vec![0, 1, 2, 2, 3, 4, 4, 5, 5, 6]);
            for trim_right in [true, false] {
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(5), &mut columns),
                    "a 𝄞 𝄞"
                );
                assert_eq!(columns, vec![0, 1, 2, 2, 3, 4, 4, 5]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(4), &mut columns),
                    "a 𝄞 "
                );
                assert_eq!(columns, vec![0, 1, 2, 2, 3, 4]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(3), &mut columns),
                    "a 𝄞"
                );
                assert_eq!(columns, vec![0, 1, 2, 2, 3]);
            }
        }

        #[test]
        fn combining() {
            let mut columns = Vec::new();
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            for i in [2, 4, 5] {
                line.set_cell(i, &cell(1, "e\u{301}", 1, cc("\u{301}")));
            }
            assert_eq!(
                tts(&line, false, None, None, &mut columns),
                "a e\u{301} e\u{301}e\u{301}    "
            );
            assert_eq!(columns, vec![0, 1, 2, 2, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10]);
            assert_eq!(
                tts(&line, true, None, None, &mut columns),
                "a e\u{301} e\u{301}e\u{301}"
            );
            assert_eq!(columns, vec![0, 1, 2, 2, 3, 4, 4, 5, 5, 6]);
            for trim_right in [true, false] {
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(5), &mut columns),
                    "a e\u{301} e\u{301}"
                );
                assert_eq!(columns, vec![0, 1, 2, 2, 3, 4, 4, 5]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(4), &mut columns),
                    "a e\u{301} "
                );
                assert_eq!(columns, vec![0, 1, 2, 2, 3, 4]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(3), &mut columns),
                    "a e\u{301}"
                );
                assert_eq!(columns, vec![0, 1, 2, 2, 3]);
            }
        }

        #[test]
        fn fullwidth() {
            let mut columns = Vec::new();
            let mut line = null_line(10);
            line.set_cell(0, &a_cell());
            for i in [2, 5, 7] {
                line.set_cell(i, &cell(1, "１", 2, cc("１")));
                line.set_cell(i + 1, &cell(0, "", 0, 0));
            }
            assert_eq!(tts(&line, false, None, None, &mut columns), "a １ １１ ");
            assert_eq!(columns, vec![0, 1, 2, 4, 5, 7, 9, 10]);
            assert_eq!(tts(&line, true, None, None, &mut columns), "a １ １１");
            assert_eq!(columns, vec![0, 1, 2, 4, 5, 7, 9]);
            for trim_right in [true, false] {
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(7), &mut columns),
                    "a １ １"
                );
                assert_eq!(columns, vec![0, 1, 2, 4, 5, 7]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(6), &mut columns),
                    "a １ １"
                );
                assert_eq!(columns, vec![0, 1, 2, 4, 5, 7]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(5), &mut columns),
                    "a １ "
                );
                assert_eq!(columns, vec![0, 1, 2, 4, 5]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(4), &mut columns),
                    "a １"
                );
                assert_eq!(columns, vec![0, 1, 2, 4]);
                assert_eq!(
                    tts(&line, trim_right, Some(0), Some(3), &mut columns),
                    "a １"
                );
                assert_eq!(columns, vec![0, 1, 2, 4]);
                assert_eq!(tts(&line, trim_right, Some(0), Some(2), &mut columns), "a ");
                assert_eq!(columns, vec![0, 1, 2]);
            }
        }

        #[test]
        fn space_at_end() {
            let mut columns = Vec::new();
            let mut line = null_line(10);
            for i in [0, 2, 4, 5] {
                line.set_cell(i, &a_cell());
            }
            line.set_cell(6, &cell(1, " ", 1, cc(" ")));
            assert_eq!(tts(&line, false, None, None, &mut columns), "a a aa    ");
            assert_eq!(columns, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
            assert_eq!(tts(&line, true, None, None, &mut columns), "a a aa ");
            assert_eq!(columns, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        }

        #[test]
        fn should_always_return_some_sane_value() {
            let mut columns = Vec::new();
            // sanity check - broken line with invalid out of bound null width cells
            let line = zero_width_line(10);
            assert_eq!(tts(&line, false, None, None, &mut columns), "          ");
            assert_eq!(columns, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
            assert_eq!(tts(&line, true, None, None, &mut columns), "");
            assert_eq!(columns, vec![0]);
        }

        #[test]
        fn should_work_with_end_col_0() {
            let mut columns = Vec::new();
            let mut line = zero_width_line(10);
            line.set_cell(0, &a_cell());
            assert_eq!(tts(&line, true, Some(0), Some(0), &mut columns), "");
            assert_eq!(columns, vec![0]);
        }
    }

    mod add_char_to_cell {
        use super::*;

        #[test]
        fn should_set_width_to_1_for_empty_cell() {
            let mut line = null_line(3);
            line.add_codepoint_to_cell(0, cc("\u{301}"), 0);
            let mut c = CellData::new();
            line.load_cell(0, &mut c);
            // chars contains single combining char, width is set to 1
            assert_eq!(c.get_as_char_data(), cd(DEFAULT_ATTR, "\u{301}", 1, 0x0301));
            // do not account a single combining char as combined
            assert_eq!(c.is_combined(), 0);
        }

        #[test]
        fn should_add_char_to_combining_string_in_cell() {
            let mut line = null_line(3);
            let mut c = CellData::new();
            line.load_cell(0, &mut c);
            c.set_from_char_data(&cd(123, "e\u{301}", 1, unit("e\u{301}", 1)));
            line.set_cell(0, &c);
            line.add_codepoint_to_cell(0, cc("\u{301}"), 0);
            line.load_cell(0, &mut c);
            // chars contains 3 chars, width is set to 1
            assert_eq!(c.get_as_char_data(), cd(123, "e\u{301}\u{301}", 1, 0x0301));
            assert_eq!(c.is_combined(), Content::IS_COMBINED_MASK);
        }

        #[test]
        fn should_create_combining_string_on_taken_cell() {
            let mut line = null_line(3);
            let mut c = CellData::new();
            line.load_cell(0, &mut c);
            // `'e'.charCodeAt(1)` is NaN upstream: the code argument is ignored for a single char.
            c.set_from_char_data(&cd(123, "e", 1, 0));
            line.set_cell(0, &c);
            line.add_codepoint_to_cell(0, cc("\u{301}"), 0);
            line.load_cell(0, &mut c);
            // chars contains 2 chars, width is set to 1
            assert_eq!(c.get_as_char_data(), cd(123, "e\u{301}", 1, 0x0301));
            assert_eq!(c.is_combined(), Content::IS_COMBINED_MASK);
        }
    }

    mod correct_fullwidth_handling {
        use super::*;

        fn populate(line: &mut BufferLine) {
            let c = cell(1, "￥", 2, cc("￥"));
            for i in (0..line.len()).step_by(2) {
                line.set_cell(i, &c);
            }
        }

        fn a() -> CellData {
            cell(1, "a", 1, cc("a"))
        }

        #[test]
        fn insert_wide_char_at_pos() {
            let mut line = zero_width_line(10);
            populate(&mut line);
            line.insert_cells(9, 1, &a());
            assert_eq!(text(&line), "￥￥￥￥ a");
            line.insert_cells(8, 1, &a());
            assert_eq!(text(&line), "￥￥￥￥a ");
            line.insert_cells(1, 1, &a());
            assert_eq!(text(&line), " a ￥￥￥a");
        }

        #[test]
        fn insert_wide_char_at_end() {
            let mut line = zero_width_line(10);
            populate(&mut line);
            line.insert_cells(0, 3, &a());
            assert_eq!(text(&line), "aaa￥￥￥ ");
            line.insert_cells(4, 1, &a());
            assert_eq!(text(&line), "aaa a ￥￥");
            line.insert_cells(4, 1, &a());
            assert_eq!(text(&line), "aaa aa ￥ ");
        }

        #[test]
        fn delete() {
            let mut line = zero_width_line(10);
            populate(&mut line);
            line.delete_cells(0, 1, &a());
            assert_eq!(text(&line), " ￥￥￥￥a");
            line.delete_cells(5, 2, &a());
            assert_eq!(text(&line), " ￥￥￥aaa");
            line.delete_cells(0, 2, &a());
            assert_eq!(text(&line), " ￥￥aaaaa");
        }

        fn replaced(start: usize, end: usize) -> String {
            let mut line = zero_width_line(10);
            populate(&mut line);
            line.replace_cells(start, end, &a(), false);
            text(&line)
        }

        #[test]
        fn replace_start_at_0() {
            assert_eq!(replaced(0, 1), "a ￥￥￥￥");
            assert_eq!(replaced(0, 2), "aa￥￥￥￥");
            assert_eq!(replaced(0, 3), "aaa ￥￥￥");
            assert_eq!(replaced(0, 8), "aaaaaaaa￥");
            assert_eq!(replaced(0, 9), "aaaaaaaaa ");
            assert_eq!(replaced(0, 10), "aaaaaaaaaa");
        }

        #[test]
        fn replace_start_at_1() {
            assert_eq!(replaced(1, 2), " a￥￥￥￥");
            assert_eq!(replaced(1, 3), " aa ￥￥￥");
            assert_eq!(replaced(1, 4), " aaa￥￥￥");
            assert_eq!(replaced(1, 8), " aaaaaaa￥");
            assert_eq!(replaced(1, 9), " aaaaaaaa ");
            assert_eq!(replaced(1, 10), " aaaaaaaaa");
        }
    }

    mod extended_attributes {
        use super::*;

        fn curly_cell() -> CellData {
            let mut c = cell(1, "a", 0, cc("a"));
            c.extended.set_underline_style(UnderlineStyle::CURLY);
            c.bg |= BgFlags::HAS_EXTENDED;
            c
        }

        fn style(line: &BufferLine, index: usize) -> u32 {
            line.extended_attrs.get(&index).unwrap().underline_style()
        }

        /// The five-cell sequence of upstream's `setCells`, `loadCell`, `clone` and `copyFrom`.
        fn five_cells() -> BufferLine {
            let mut line = BufferLine::new(5, None, false);
            let mut c = cell(1, "a", 0, cc("a"));
            // no eAttrs
            line.set_cell(0, &c);
            // some underline style
            c.extended.set_underline_style(UnderlineStyle::CURLY);
            c.bg |= BgFlags::HAS_EXTENDED;
            line.set_cell(1, &c);
            // same eAttr, different codepoint
            c.content = 65; // 'A'
            line.set_cell(2, &c);
            // different eAttr
            c.extended.set_underline_style(UnderlineStyle::DOTTED);
            line.set_cell(3, &c);
            // no eAttrs again
            c.bg &= !BgFlags::HAS_EXTENDED;
            line.set_cell(4, &c);
            line
        }

        #[test]
        fn set_cells() {
            let line = five_cells();
            assert_eq!(
                to_array(&line),
                vec![
                    cd(1, "a", 0, cc("a")),
                    cd(1, "a", 0, cc("a")),
                    cd(1, "A", 0, cc("A")),
                    cd(1, "A", 0, cc("A")),
                    cd(1, "A", 0, cc("A")),
                ]
            );
            assert_eq!(line.extended_attrs.get(&0), None);
            assert_eq!(style(&line, 1), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 2), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 3), UnderlineStyle::DOTTED);
            assert_eq!(line.extended_attrs.get(&4), None);
            // adapted: upstream asserts the same / a different object
            assert_eq!(line.extended_attrs.get(&1), line.extended_attrs.get(&2));
            assert_ne!(line.extended_attrs.get(&1), line.extended_attrs.get(&3));
        }

        #[test]
        fn load_cell() {
            let line = five_cells();
            let load = |i: usize| {
                let mut c = CellData::new();
                line.load_cell(i, &mut c);
                c
            };
            let (c0, c1, c2, c3, c4) = (load(0), load(1), load(2), load(3), load(4));
            assert_eq!(c0.extended.underline_style(), UnderlineStyle::NONE);
            assert_eq!(c1.extended.underline_style(), UnderlineStyle::CURLY);
            assert_eq!(c2.extended.underline_style(), UnderlineStyle::CURLY);
            assert_eq!(c3.extended.underline_style(), UnderlineStyle::DOTTED);
            assert_eq!(c4.extended.underline_style(), UnderlineStyle::NONE);
            // adapted: value equality for upstream's reference identity
            assert_eq!(c1.extended, c2.extended);
            assert_ne!(c2.extended, c3.extended);
        }

        #[test]
        fn fill() {
            let mut line = BufferLine::new(3, None, false);
            line.fill(&curly_cell(), false);
            for i in 0..3 {
                assert_eq!(style(&line, i), UnderlineStyle::CURLY);
            }
        }

        #[test]
        fn insert_cells() {
            let mut line = BufferLine::new(5, None, false);
            let mut c = curly_cell();
            line.insert_cells(1, 3, &c);
            assert_eq!(style(&line, 1), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 2), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 3), UnderlineStyle::CURLY);
            assert_eq!(line.extended_attrs.get(&4), None);
            c.extended.set_underline_style(UnderlineStyle::DOTTED);
            line.insert_cells(2, 2, &c);
            assert_eq!(style(&line, 1), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 2), UnderlineStyle::DOTTED);
            assert_eq!(style(&line, 3), UnderlineStyle::DOTTED);
            assert_eq!(style(&line, 4), UnderlineStyle::CURLY);
        }

        #[test]
        fn delete_cells() {
            let mut line = BufferLine::new(5, None, false);
            let mut fill = curly_cell();
            line.fill(&fill, false);
            fill.extended.set_underline_style(UnderlineStyle::DOUBLE);
            line.delete_cells(1, 3, &fill);
            assert_eq!(style(&line, 0), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 1), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 2), UnderlineStyle::DOUBLE);
            assert_eq!(style(&line, 3), UnderlineStyle::DOUBLE);
            assert_eq!(style(&line, 4), UnderlineStyle::DOUBLE);
        }

        #[test]
        fn replace_cells() {
            let mut line = BufferLine::new(5, None, false);
            let mut fill = curly_cell();
            line.fill(&fill, false);
            fill.extended.set_underline_style(UnderlineStyle::DOUBLE);
            line.replace_cells(1, 3, &fill, false);
            assert_eq!(style(&line, 0), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 1), UnderlineStyle::DOUBLE);
            assert_eq!(style(&line, 2), UnderlineStyle::DOUBLE);
            assert_eq!(style(&line, 3), UnderlineStyle::CURLY);
            assert_eq!(style(&line, 4), UnderlineStyle::CURLY);
        }

        #[test]
        fn clone() {
            let line = five_cells();
            let n_line = line.clone();
            for i in 0..5 {
                assert_eq!(n_line.extended_attrs.get(&i), line.extended_attrs.get(&i));
            }
        }

        #[test]
        fn copy_from() {
            let initial = five_cells();
            let mut line = BufferLine::new(5, None, false);
            line.fill(&cell(1, "b", 0, cc("b")), false);
            line.copy_from(&initial);
            for i in 0..5 {
                assert_eq!(line.extended_attrs.get(&i), initial.extended_attrs.get(&i));
            }
        }
    }
}

/// Rust-only cases: the typed-array edge semantics of D7 and the return values of `resize`.
mod edge_semantics {
    use super::*;

    fn combined_cell(chars: &str) -> CellData {
        cell(1, chars, 1, 0)
    }

    #[test]
    fn set_cell_past_the_end_leaves_the_data_alone() {
        let mut line = null_line(4);
        let before = line.data.clone();
        line.set_cell(9, &cell_of(7, "x", 1));
        line.set_cell_from_codepoint(9, 65, 1, &AttributeData::default());
        line.add_codepoint_to_cell(9, 0x301, 0);
        assert_eq!(line.data, before);
        assert_eq!(line.len(), 4);
        assert_eq!(line.get_width(9), 0);
        assert_eq!(line.get_width(usize::MAX), 0);
        assert_eq!(line.get_fg(9), 0);
        assert_eq!(line.get_string(9), "");
        assert_eq!(line.get_code_point(9), 0);
        assert_eq!(line.get(9), cd(0, "", 0, 0));
    }

    #[test]
    fn a_combined_set_cell_past_the_end_still_writes_the_map_like_a_js_object() {
        let mut line = null_line(4);
        let before = line.data.clone();
        line.set_cell(9, &combined_cell("e\u{301}"));
        assert_eq!(line.data, before);
        assert_eq!(line.combined.get(&9).map(String::as_str), Some("e\u{301}"));
        // Nothing reads it: it is not part of any translation within the line.
        assert_eq!(text(&line), "    ");
        // ... and resize drops it again only if it is cut off.
        line.resize(2, &null_cell());
        assert_eq!(line.combined.len(), 0);
    }

    #[test]
    fn insert_delete_replace_past_the_end_do_not_panic() {
        // insert/delete wrap `pos` modulo the length, like upstream.
        let mut line = null_line(4);
        for (i, ch) in ["a", "b", "c", "d"].into_iter().enumerate() {
            line.set_cell(i, &cell_of(1, ch, 1));
        }
        let mut a = line.clone();
        a.insert_cells(4 + 1, 1, &cell_of(2, "z", 1)); // pos 1
        assert_eq!(text(&a), "azbc");
        let mut b = line.clone();
        b.delete_cells(4 + 1, 1, &cell_of(2, "z", 1)); // pos 1
        assert_eq!(text(&b), "acdz");
        let mut c = line.clone();
        c.delete_cells(6, 100, &cell_of(2, "z", 1)); // pos 2, n beyond the end
        assert_eq!(text(&c), "abzz");
        // replace past the end is bounded by the length.
        let mut d = line.clone();
        d.replace_cells(2, 40, &cell_of(2, "z", 1), false);
        assert_eq!(text(&d), "abzz");
        let mut e = line.clone();
        e.replace_cells(4, 40, &cell_of(2, "z", 1), false);
        e.replace_cells(40, 2, &cell_of(2, "z", 1), true);
        e.replace_cells(0, 0, &cell_of(2, "z", 1), true);
        e.replace_cells(0, 0, &cell_of(2, "z", 1), false);
        assert_eq!(text(&e), "abcd");
        // a zero-length line ignores insert/delete instead of dividing by zero.
        let mut empty = BufferLine::new(0, None, false);
        empty.insert_cells(3, 2, &cell_of(2, "z", 1));
        empty.delete_cells(3, 2, &cell_of(2, "z", 1));
        empty.replace_cells(0, 3, &cell_of(2, "z", 1), false);
        assert_eq!(empty.len(), 0);
        assert!(empty.data.is_empty());
    }

    #[test]
    fn replace_cells_reads_the_width_before_end_after_the_start_side_reset() {
        // start == end on a second half of a wide char: upstream resets cell start-1 first and
        // then reads getWidth(end - 1), which is that same (now width 1) cell.
        let mut line = zero_width_line(4);
        line.set_cell(0, &cell(1, "￥", 2, cc("￥")));
        line.replace_cells(1, 1, &cell(1, "a", 1, cc("a")), false);
        assert_eq!(line.get_width(0), 1);
        assert_eq!(line.get_width(1), 0); // not reset: the width read was already 1
    }

    #[test]
    fn copy_cells_from_beyond_either_end() {
        let mut src = null_line(3);
        for (i, ch) in ["a", "b", "c"].into_iter().enumerate() {
            src.set_cell(i, &cell_of(1, ch, 1));
        }
        // source range runs 2 cells past the end: those cells copy as zeros
        let mut dest = BufferLine::new(5, Some(&cell_of(9, "z", 1)), false);
        dest.copy_cells_from(&src, 1, 0, 4, false);
        assert_eq!(dest.get_string(0), "b");
        assert_eq!(dest.get_string(1), "c");
        assert_eq!(dest.get(2), cd(0, "", 0, 0));
        assert_eq!(dest.get(3), cd(0, "", 0, 0));
        assert_eq!(dest.get_string(4), "z");
        // destination range runs past the end: ignored
        let mut small = null_line(2);
        let before = small.data.clone();
        small.copy_cells_from(&src, 0, 5, 3, true);
        assert_eq!(small.data, before);
        let mut small = null_line(2);
        small.copy_cells_from(&src, 0, 1, 3, false);
        assert_eq!(small.get_string(0), "");
        assert_eq!(small.get_string(1), "a");
        assert_eq!(small.len(), 2);
    }

    #[test]
    fn copy_cells_from_copies_every_combined_key_from_src_col_on() {
        let mut src = null_line(6);
        src.set_cell(1, &combined_cell("e\u{301}"));
        src.set_cell(4, &combined_cell("o\u{308}"));
        let mut dest = null_line(6);
        // copy a single cell (index 1); the combined string at index 4 comes along anyway.
        dest.copy_cells_from(&src, 1, 0, 1, false);
        assert_eq!(dest.combined.get(&0).map(String::as_str), Some("e\u{301}"));
        assert_eq!(dest.combined.get(&3).map(String::as_str), Some("o\u{308}"));
        // keys below src_col are not copied
        let mut dest = null_line(6);
        dest.copy_cells_from(&src, 2, 0, 1, false);
        assert_eq!(dest.combined.len(), 1);
        assert_eq!(dest.combined.get(&2).map(String::as_str), Some("o\u{308}"));
    }

    #[test]
    fn copy_cells_within_forward_shift_cascades_like_upstream() {
        // Worked from the TS loop (node, upstream keys snapshot [1, 2, 5], values read live):
        //  data: cell 2 <- cell 1, then cell 3 <- (new) cell 2, i.e. cells 1, 2, 3 all hold A.
        //  combined: key 1 (A) -> [2] = A; key 2 now reads A -> [3] = A (not B: the cascade);
        //  key 5 (X, outside the copied range) -> [6] = X, one past the end of the line.
        let a = "e\u{301}";
        let b = "o\u{308}";
        let x = "x\u{300}";
        let mut line = null_line(6);
        line.set_cell(1, &combined_cell(a));
        line.set_cell(2, &combined_cell(b));
        line.set_cell(5, &combined_cell(x));
        line.copy_cells_within(1, 2, 2, false);
        let combined: Vec<(usize, &str)> = line
            .combined
            .iter()
            .map(|(&k, v)| (k, v.as_str()))
            .collect();
        assert_eq!(combined, vec![(1, a), (2, a), (3, a), (5, x), (6, x)]);
        assert_eq!(line.get_string(1), a);
        assert_eq!(line.get_string(2), a);
        assert_eq!(line.get_string(3), a);
        assert_eq!(line.get_string(4), "");
        // The reverse direction copies the cell data without the cascade (cell 3 <- 2 first),
        // but the combined strings are always moved by the forward key loop, so they cascade
        // regardless of direction: cell 3 keeps B's content word while its string is A.
        let mut line = null_line(6);
        line.set_cell(1, &combined_cell(a));
        line.set_cell(2, &combined_cell(b));
        let b_content = line.read(2, CELL_CONTENT);
        line.copy_cells_within(1, 2, 2, true);
        assert_eq!(line.read(3, CELL_CONTENT), b_content);
        assert_eq!(line.get_string(2), a);
        assert_eq!(line.get_string(3), a);
    }

    #[test]
    fn copy_cells_within_copies_extended_attrs_per_cell() {
        let mut line = null_line(5);
        let mut c = cell_of(1, "a", 1);
        c.extended.set_underline_style(UnderlineStyle::CURLY);
        c.bg |= BgFlags::HAS_EXTENDED;
        line.set_cell(0, &c);
        line.copy_cells_within(0, 3, 1, false);
        assert_eq!(
            line.extended_attrs.get(&3).unwrap().underline_style(),
            UnderlineStyle::CURLY
        );
        assert_eq!(line.get_string(3), "a");
    }

    #[test]
    fn a_missing_map_entry_reads_as_empty_not_a_panic() {
        // content says "combined" but the map has no entry (a corrupt copy); upstream reads
        // `undefined` and throws on `.charCodeAt`; here it is the empty string.
        let mut line = null_line(2);
        line.write(0, CELL_CONTENT, Content::IS_COMBINED_MASK | (1 << 22));
        assert_eq!(line.get_string(0), "");
        assert_eq!(line.get_code_point(0), 0);
        assert_eq!(line.get(0), cd(DEFAULT_ATTR, "", 1, 0));
        assert_eq!(text(&line), " ");
        line.add_codepoint_to_cell(0, 0x301, 0);
        assert_eq!(line.get_string(0), "\u{301}");
        let mut c = CellData::new();
        line.write(1, CELL_BG, BgFlags::HAS_EXTENDED);
        line.load_cell(1, &mut c);
        assert_eq!(c.extended, ExtendedAttrs::default());
    }

    #[test]
    fn get_code_point_of_a_combined_cell_is_the_last_utf16_unit() {
        let mut line = null_line(2);
        line.set_cell(0, &cell(1, "e𓂀", 1, 0)); // ends in an astral char
        let low = unit("e𓂀", 2);
        assert!((0xDC00..0xE000).contains(&low));
        assert_eq!(line.get_code_point(0), low);
        assert_eq!(line.get(0).3, low);
    }

    #[test]
    fn set_stores_an_astral_single_char_as_combined_with_the_index_as_code_point() {
        // upstream decides on `.length > 1` (UTF-16 units)
        let mut line = null_line(4);
        line.set(3, &cd(5, "😁", 1, 0));
        assert_eq!(line.is_combined(3), Content::IS_COMBINED_MASK);
        assert_eq!(line.read(3, CELL_CONTENT) & Content::CODEPOINT_MASK, 3);
        assert_eq!(line.get_string(3), "😁");
        assert_eq!(line.get_fg(3), 5);
        // a BMP char is not combined, an empty string is code 0
        line.set(0, &cd(0, "x", 1, 0));
        line.set(1, &cd(0, "", 1, 0));
        assert_eq!(line.is_combined(0), 0);
        assert_eq!(line.get_code_point(0), u32::from(b'x'));
        assert_eq!(line.get_code_point(1), 0);
    }

    #[test]
    fn translate_to_string_past_the_end_reads_spaces() {
        let line = null_line(3);
        let mut columns = Vec::new();
        assert_eq!(tts(&line, false, Some(2), Some(5), &mut columns), "   ");
        assert_eq!(columns, vec![2, 3, 4, 5]);
        // inverted range: empty, one column entry
        assert_eq!(tts(&line, false, Some(4), Some(1), &mut columns), "");
        assert_eq!(columns, vec![4]);
        // invalid code point bits become U+FFFD instead of panicking
        let mut line = null_line(1);
        line.write(0, CELL_CONTENT, Content::CODEPOINT_MASK | (1 << 22));
        assert_eq!(text(&line), "\u{FFFD}");
    }

    #[test]
    fn trimmed_lengths() {
        let mut line = null_line(6);
        assert_eq!(line.get_no_bg_trimmed_length(), 0);
        let mut bg = cell(1, "", 1, 0);
        bg.bg = Attributes::CM_RGB | 0x0012_3456;
        line.set_cell(4, &bg);
        // a coloured background counts for the no-bg variant only
        assert_eq!(line.get_trimmed_length(), 0);
        assert_eq!(line.get_no_bg_trimmed_length(), 5);
    }

    #[test]
    fn protected_cells_survive_respect_protect() {
        let mut line = null_line(5);
        let mut protected = cell(1, "p", 1, cc("p"));
        protected.bg |= BgFlags::PROTECTED;
        line.set_cell(1, &protected);
        let z = cell(2, "z", 1, cc("z"));
        line.fill(&z, true);
        assert_eq!(text(&line), "zpzzz");
        line.replace_cells(0, 5, &cell(3, "y", 1, cc("y")), true);
        assert_eq!(text(&line), "ypyyy");
        assert_eq!(line.is_protected(1), BgFlags::PROTECTED);
        assert_eq!(line.is_protected(0), 0);
    }

    #[test]
    fn resize_reuses_and_reports_the_buffer_like_a_typed_array() {
        let fill = null_cell();
        // 10 cells = 30 slots, capacity exactly 30 (upstream byteLength 120)
        let mut line = null_line(10);
        assert_eq!(line.data.capacity(), 30);
        // resizing to the same length: used * 2 < capacity?
        assert!(!line.resize(10, &fill));
        // shrink to 5 (15 slots): 30 < 30 is false; capacity kept
        assert!(!line.resize(5, &fill));
        assert_eq!(line.data.len(), 15);
        assert_eq!(line.data.capacity(), 30);
        assert_eq!(line.cleanup_memory(), 0);
        // shrink to 4 (12 slots): 24 < 30 -> a cleanup would free memory
        assert!(line.resize(4, &fill));
        assert!(line.resize(4, &fill));
        // growing within the kept capacity reuses the buffer
        assert!(!line.resize(10, &fill));
        assert_eq!(line.data.capacity(), 30);
        assert!(line.resize(4, &fill));
        assert_eq!(line.cleanup_memory(), 1);
        assert_eq!(line.data.capacity(), 12);
        assert_eq!(line.cleanup_memory(), 0);
        assert_eq!(text(&line), "    ");
        // growing past the capacity allocates exactly the new size
        let mut small = null_line(3);
        assert_eq!(small.data.capacity(), 9);
        assert!(!small.resize(10, &fill));
        assert_eq!(small.data.capacity(), 30);
        assert_eq!(small.len(), 10);
        // growth keeps the old cells
        let mut line = null_line(2);
        line.set_cell(1, &cell_of(4, "q", 1));
        line.resize(8, &fill);
        assert_eq!(line.get_string(1), "q");
        assert_eq!(line.get_fg(1), 4);
    }

    #[test]
    fn resize_drops_cut_off_extended_attrs() {
        let mut line = null_line(4);
        let mut c = cell_of(1, "a", 1);
        c.extended.set_underline_style(UnderlineStyle::DOUBLE);
        c.bg |= BgFlags::HAS_EXTENDED;
        line.set_cell(1, &c);
        line.set_cell(3, &c);
        line.resize(2, &null_cell());
        assert_eq!(line.extended_attrs.len(), 1);
        assert!(line.extended_attrs.contains_key(&1));
    }

    #[test]
    fn copy_from_with_a_different_length_takes_the_exact_size() {
        let mut big = null_line(8);
        let small = null_line(3);
        big.copy_from(&small);
        assert_eq!(big.len(), 3);
        assert_eq!(big.data.len(), 9);
        assert_eq!(big, small);
        let mut same = null_line(3);
        same.set_cell(0, &cell_of(1, "x", 1));
        same.copy_from(&small);
        assert_eq!(same, small);
    }

    #[test]
    fn default_attr_data_is_all_zero() {
        assert_eq!(DEFAULT_ATTR_DATA, AttributeData::default());
        assert_eq!(CELL_SIZE, 3);
    }

    /// LUNETERM (review S4): a cell holds at most `MAX_COMBINED_CODE_POINTS` code points; the
    /// rest are dropped, but a width they ask for still applies.
    #[test]
    fn combining_code_points_past_the_cap_are_dropped_and_their_width_still_applies() {
        let mut line = null_line(3);
        line.set_cell_from_codepoint(0, u32::from('a'), 1, &DEFAULT_ATTR_DATA);
        for _ in 0..1000 {
            line.add_codepoint_to_cell(0, 0x301, 0);
        }
        let s = line.get_string(0);
        assert_eq!(s.chars().count(), MAX_COMBINED_CODE_POINTS);
        assert!(s.starts_with("a\u{301}"));
        assert_eq!(line.get_width(0), 1);
        line.add_codepoint_to_cell(0, 0xFE0F, 2);
        assert_eq!(line.get_string(0).chars().count(), MAX_COMBINED_CODE_POINTS);
        assert_eq!(line.get_width(0), 2);
    }
}
