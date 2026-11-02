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
        assert_eq!(
            line.combined.get(&9).map(|chars| &**chars),
            Some("e\u{301}")
        );
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
        assert_eq!(
            dest.combined.get(&0).map(|chars| &**chars),
            Some("e\u{301}")
        );
        assert_eq!(
            dest.combined.get(&3).map(|chars| &**chars),
            Some("o\u{308}")
        );
        // keys below src_col are not copied
        let mut dest = null_line(6);
        dest.copy_cells_from(&src, 2, 0, 1, false);
        assert_eq!(dest.combined.len(), 1);
        assert_eq!(
            dest.combined.get(&2).map(|chars| &**chars),
            Some("o\u{308}")
        );
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
        let entries = line.combined.entries();
        let combined: Vec<(usize, &str)> = entries.iter().map(|(k, v)| (*k, &**v)).collect();
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

    /// LUNETERM: (review S4) a cell holds at most `MAX_COMBINED_CODE_POINTS` code points; the
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

    /// LUNETERM: (review S7) `insert_cells_repeatedly` leaves exactly what the loop of
    /// `insert_cells` calls leaves: cells and both side tables, stale entries included, over
    /// random lines (narrow, wide, combined and extended cells, stale entries at unflagged
    /// cells), fills (plain, extended, combined, wide) and positions, including calls outside
    /// the fast path's conditions.
    #[test]
    fn inserting_repeatedly_equals_the_loop_of_inserts() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        let ext = |v: usize| ExtendedAttrs::new(v as u32 + 1, v as u32);
        let (mut fast, mut fast_with_edge, mut fast_fired) = (0, 0, 0);
        for _ in 0..50_000 {
            let len = 1 + next(12);
            let mut line = null_line(len);
            let mut c = 0;
            while c < len {
                let mut cell = CellData::new();
                let kind = next(5);
                let width = if kind == 1 && c + 1 < len { 2 } else { 1 };
                cell.content = match kind {
                    0 => NULL_CELL_CODE | (NULL_CELL_WIDTH << Content::WIDTH_SHIFT),
                    2 => {
                        cell.combined_data =
                            format!("e{}", char::from_u32(0x300 + c as u32).unwrap());
                        Content::IS_COMBINED_MASK | (width << Content::WIDTH_SHIFT)
                    }
                    _ => (u32::from('a') + c as u32) | (width << Content::WIDTH_SHIFT),
                };
                if next(3) == 0 {
                    cell.bg |= BgFlags::HAS_EXTENDED;
                    cell.extended = ext(next(4));
                }
                line.set_cell(c, &cell);
                if next(4) == 0 {
                    // a stale entry at a cell without the flag
                    line.combined.insert(c, format!("stale{c}").into());
                    line.extended_attrs.insert(c, ext(9));
                }
                if width == 2 {
                    line.set_cell_from_codepoint(c + 1, 0, 0, &DEFAULT_ATTR_DATA);
                }
                c += width as usize;
            }
            let mut fill = null_cell();
            match next(6) {
                0 | 1 => {
                    fill.bg |= BgFlags::HAS_EXTENDED;
                    fill.extended = ext(7);
                }
                2 => {
                    fill.content = Content::IS_COMBINED_MASK | (1 << Content::WIDTH_SHIFT);
                    fill.combined_data = "fill".into();
                }
                3 => fill.content = u32::from('w') | (2 << Content::WIDTH_SHIFT),
                _ => {}
            }
            let pos = next(len + 1);
            let n = next(4);
            // mostly within the line (the fast path), sometimes one step past it
            let times = next((len - pos) / n.max(1) + 2);
            // the edge fix-up's column (`print`'s `cols - 1`): mostly at or right of the last
            // written cell and left of the last cell, as REP passes it on a line longer than
            // `cols`; sometimes anywhere, at the last cell or past it
            let reach = (pos + n * times).saturating_sub(1);
            let edge = match next(4) {
                0 => next(len + 3),
                1 => len - 1,
                _ => reach + next(len.saturating_sub(reach + 1).max(1)),
            };
            let mut edge_attrs = AttributeData {
                fg: 0x0300_0011,
                ..AttributeData::default()
            };
            if next(2) == 0 {
                edge_attrs.bg |= BgFlags::HAS_EXTENDED;
                edge_attrs.extended = ext(5);
            }
            let fast_path = fill.get_width() != 2
                && n != 0
                && times != 0
                && pos + n * times <= len
                && (edge + 1 >= len || edge >= reach);
            if fast_path {
                fast += 1;
                if edge + 1 < len {
                    fast_with_edge += 1;
                }
            }
            let mut looped = line.clone();
            let mut fired = false;
            for j in 0..times {
                looped.insert_cells(pos + j * n, n, &fill);
                if looped.get_width(edge) == 2 {
                    looped.set_cell_from_codepoint(
                        edge,
                        NULL_CELL_CODE,
                        NULL_CELL_WIDTH,
                        &edge_attrs,
                    );
                    fired = true;
                }
            }
            if fast_path && fired {
                fast_fired += 1;
            }
            let mut once = line.clone();
            once.insert_cells_repeatedly(pos, n, times, &fill, edge, &edge_attrs);
            assert_eq!(
                once, looped,
                "{line:?} pos {pos} n {n} times {times} fill {fill:?} edge {edge} {edge_attrs:?}"
            );
        }
        assert!(fast > 8000, "{fast}");
        assert!(fast_with_edge > 3000, "{fast_with_edge}");
        assert!(fast_fired > 600, "{fast_fired}");
    }

    /// LUNETERM: (review S7 round 4) `set_cells_repeatedly` leaves exactly what its loop of
    /// `set_cell` calls leaves, over random lines with entries, heads and stubs with and
    /// without combined strings and extended attributes, widths 0 to 3 and runs past the end
    /// (whose map entries are still stored); the runs are long enough to take the batched
    /// insert.
    #[test]
    fn setting_cells_repeatedly_equals_the_loop_of_set_cell() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        let ext = |v: usize| ExtendedAttrs::new(v as u32 + 1, v as u32);
        let cell_of_kind = |kind: usize, tag: usize| {
            let mut cell = CellData::new();
            cell.content = (u32::from('a') + tag as u32) | (1 << Content::WIDTH_SHIFT);
            cell.fg = tag as u32;
            if kind & 1 != 0 {
                cell.content = Content::IS_COMBINED_MASK | (1 << Content::WIDTH_SHIFT);
                cell.combined_data = format!("c{tag}\u{301}");
            }
            if kind & 2 != 0 {
                cell.bg |= BgFlags::HAS_EXTENDED;
                cell.extended = ext(tag);
            }
            cell
        };
        let mut batched = 0;
        for round in 0..20_000 {
            let len = 1 + next(80);
            let mut line = null_line(len);
            for c in 0..len {
                if next(3) == 0 {
                    line.set_cell(c, &cell_of_kind(next(4), c));
                }
                if next(5) == 0 {
                    line.combined.insert(c, format!("stale{c}").into());
                }
                if next(5) == 0 {
                    line.extended_attrs.insert(c, ext(9));
                }
            }
            let head = cell_of_kind(next(4), 100 + round % 7);
            let stub = cell_of_kind(next(4), 200 + round % 5);
            let w = next(4);
            let pos = next(len + 4);
            let times = next(len + 8);
            if w > 0 && times * w >= 16 {
                batched += 1;
            }
            let mut looped = line.clone();
            for r in 0..times {
                looped.set_cell(pos + r * w, &head);
                for i in 1..w {
                    looped.set_cell(pos + r * w + i, &stub);
                }
            }
            let mut once = line.clone();
            once.set_cells_repeatedly(pos, w, times, &head, &stub);
            assert_eq!(
                once, looped,
                "{line:?} pos {pos} w {w} times {times} head {head:?} stub {stub:?}"
            );
        }
        assert!(batched > 8000, "{batched}");
    }
}

/// LUNETERM: (review S7 round 4) `RunMap` answers as the per-key `BTreeMap` it replaces, and
/// keeps one form per set of entries (so the derived equality of `BufferLine` compares
/// entries): random inserts, removals, range assignments, truncations, clears and shifted
/// copies from another map, near 0 and near `usize::MAX`, against a `BTreeMap` model. Plan M7
/// step 12: with `u8` values and with `Arc<str>` values made fresh for every write (equal
/// strings in distinct allocations, which must merge as equal values do).
#[test]
fn run_map_holds_the_entries_of_a_per_key_map() {
    check_run_map(|v| v as u8);
    check_run_map(|v| -> Arc<str> { Arc::from(format!("c{v}\u{301}")) });
}

fn check_run_map<V: Clone + PartialEq + std::fmt::Debug>(value_of: impl Fn(usize) -> V) {
    use std::collections::BTreeMap;
    let mut seed: u64 = 0x1234_5678_9abc_def1;
    let mut next = |bound: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % bound as u64) as usize
    };
    for round in 0..3_000 {
        let base = if round % 3 == 0 { usize::MAX - 40 } else { 0 };
        let mut map = RunMap::<V>::default();
        let mut model = BTreeMap::<usize, V>::new();
        let mut other = RunMap::<V>::default();
        let mut other_model = BTreeMap::<usize, V>::new();
        for _ in 0..60 {
            let key = base + next(41);
            let value = value_of(next(3));
            match next(12) {
                0..=4 => {
                    map.insert(key, value.clone());
                    model.insert(key, value);
                }
                5 | 6 => {
                    map.remove(&key);
                    model.remove(&key);
                }
                7 | 8 => {
                    let last = key.saturating_add(next(12)).min(base + 40);
                    let value = (next(4) != 0).then_some(value);
                    map.assign(key, last, value.clone());
                    for k in key..=last {
                        match &value {
                            Some(value) => model.insert(k, value.clone()),
                            None => model.remove(&k),
                        };
                    }
                }
                9 => {
                    // the other map, the source of the shifted copies
                    let last = key.saturating_add(next(6)).min(base + 40);
                    other.assign(key, last, Some(value.clone()));
                    for k in key..=last {
                        other_model.insert(k, value.clone());
                    }
                }
                10 => {
                    // `copy_cells_from`'s loop: every source key from `src_col` on, ascending,
                    // to its saturating shift
                    let src_col = base + next(41);
                    let dest_col = match next(3) {
                        0 => usize::MAX - next(8),
                        _ => base + next(41),
                    };
                    map.copy_shifted_from(&other, src_col, dest_col);
                    for (&k, v) in other_model.range(src_col..) {
                        model.insert((k - src_col).saturating_add(dest_col), v.clone());
                    }
                }
                11 if next(2) == 0 => {
                    // ascending distinct keys, sometimes long enough for the bulk merge, in
                    // and away from the existing runs
                    let mut k = base + next(41);
                    let mut updates = Vec::new();
                    while k <= base + 40 && updates.len() < 40 {
                        updates.push((k, value_of(next(2))));
                        match k.checked_add(1 + next(2)) {
                            Some(after) => k = after,
                            None => break,
                        }
                    }
                    for (k, v) in &updates {
                        model.insert(*k, v.clone());
                    }
                    map.insert_ascending(updates);
                }
                _ if next(4) == 0 => {
                    map.clear();
                    model.clear();
                }
                _ => {
                    map.truncate(key);
                    model.retain(|&k, _| k < key);
                }
            }
            let keys = model.keys().copied();
            let probes: Vec<usize> = (base..=base + 40)
                .chain(keys)
                .chain([usize::MAX - 1, usize::MAX])
                .collect();
            for k in probes {
                assert_eq!(map.get(&k), model.get(&k), "key {k}: {map:?} vs {model:?}");
                assert_eq!(map.contains_key(&k), model.contains_key(&k));
            }
            assert_eq!(map.len(), model.len());
            let model_entries: Vec<(usize, V)> =
                model.iter().map(|(&k, v)| (k, v.clone())).collect();
            assert_eq!(map.entries(), model_entries);
            let from = base + next(41);
            let model_keys: Vec<usize> = model.range(from..).map(|(&k, _)| k).collect();
            assert_eq!(map.keys_from(from), model_keys);
            // one form: the map built key by key from the model is equal, and no two runs
            // that touch hold the same value
            let mut rebuilt = RunMap::default();
            for (&k, v) in &model {
                rebuilt.insert(k, v.clone());
            }
            assert_eq!(map, rebuilt);
            let runs: Vec<_> = map.runs.iter().map(|(&f, (l, v))| (f, *l, v)).collect();
            for pair in runs.windows(2) {
                let ((_, last, value), (first, _, next_value)) = (&pair[0], &pair[1]);
                assert!(last < first, "{runs:?}");
                assert!(
                    last.checked_add(1) != Some(*first) || value != next_value,
                    "{runs:?}"
                );
            }
        }
    }
}

// ---- ReferenceBufferLine: the line with a per-key combined map, as shipped before plan M7 step 12 ----
//
// Moved verbatim from `buffer_line.rs` (`insert_sorted` and `BufferLine`, only `BufferLine`
// renamed to `ReferenceBufferLine`), so the differential below compares the line whose combined
// strings are a `RunMap<Arc<str>>` against the per-key `BTreeMap<usize, String>` it replaces
// (S7 residual 2).

#[allow(dead_code, unreachable_pub, clippy::indexing_slicing)]
mod reference {
    use super::*;

    /// LUNETERM: (review S7) `map.insert(k, v)` for every `(k, v)` of `updates`, whose keys are
    /// ascending and distinct. A batch that is large next to the map rebuilds it in one ordered
    /// merge (`collect` of a sorted sequence builds the tree in bulk, O(entries)) instead of one
    /// tree insert per entry; the map ends with the same entries either way, and nothing reads a
    /// map's node layout.
    fn insert_sorted<V>(map: &mut BTreeMap<usize, V>, updates: Vec<(usize, V)>) {
        debug_assert!(updates.windows(2).all(|pair| match pair {
            [(a, _), (b, _)] => a < b,
            _ => true,
        }));
        if updates.len() < 16 || updates.len().saturating_mul(4) < map.len() {
            for (key, value) in updates {
                map.insert(key, value);
            }
            return;
        }
        let old = std::mem::take(map);
        let mut merged = Vec::with_capacity(old.len() + updates.len());
        let mut old = old.into_iter().peekable();
        for (key, value) in updates {
            while let Some(entry) = old.next_if(|(old_key, _)| *old_key < key) {
                merged.push(entry);
            }
            // an entry at the same key is replaced
            old.next_if(|(old_key, _)| *old_key == key);
            merged.push((key, value));
        }
        merged.extend(old);
        *map = merged.into_iter().collect();
    }

    /// Typed array based bufferline implementation.
    ///
    /// There are 2 ways to insert data into the cell buffer:
    /// - `set_cell_from_codepoint` + `add_codepoint_to_cell`
    ///   Use these for data that is already UTF32.
    ///   Used during normal input in `InputHandler` for faster buffer access.
    /// - `set_cell`
    ///   This method takes a `CellData` object and stores the data in the buffer.
    ///   Use `CellData::from_char_data` to create the `CellData` object (e.g. from a string).
    ///
    /// To retrieve data from the buffer use either one of the primitive methods
    /// (if only one particular value is needed) or `load_cell`. For `load_cell` in a loop
    /// allocations can be avoided by reusing the `CellData` object.
    ///
    /// Upstream memory layout:
    ///
    /// ```text
    ///   |             uint32_t             |        uint32_t         |        uint32_t         |
    ///   |             `content`            |          `FG`           |          `BG`           |
    ///   | wcwidth(2) comb(1) codepoint(21) | flags(8) R(8) G(8) B(8) | flags(8) R(8) G(8) B(8) |
    /// ```
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ReferenceBufferLine {
        data: Vec<u32>,
        combined: BTreeMap<usize, String>,
        extended_attrs: RunMap<ExtendedAttrs>,
        length: usize,
        pub is_wrapped: bool,
    }

    impl ReferenceBufferLine {
        /// A line of `cols` cells, each a copy of `fill_cell_data` (default: the null cell).
        ///
        /// `cols` must be a sane terminal width (the caller clamps it, as `CoreTerminal` does);
        /// upstream throws a `RangeError` from `new Uint32Array` for an absurd one.
        pub fn new(cols: usize, fill_cell_data: Option<&CellData>, is_wrapped: bool) -> Self {
            let mut line = ReferenceBufferLine {
                data: vec![0; cols.saturating_mul(CELL_SIZE)],
                combined: BTreeMap::new(),
                extended_attrs: RunMap::default(),
                length: cols,
                is_wrapped,
            };
            let default_cell;
            let cell = match fill_cell_data {
                Some(cell) => cell,
                None => {
                    default_cell = CellData::from_char_data(&(
                        0,
                        NULL_CELL_CHAR.to_string(),
                        NULL_CELL_WIDTH,
                        NULL_CELL_CODE,
                    ));
                    &default_cell
                }
            };
            for i in 0..cols {
                line.set_cell(i, cell);
            }
            line
        }

        /// Number of cells (upstream `length`).
        pub fn len(&self) -> usize {
            self.length
        }

        pub fn is_empty(&self) -> bool {
            self.length == 0
        }

        /// The number of combined strings held (upstream `Object.keys(_combined).length`); for
        /// bounding the side tables in tests.
        pub fn combined_len(&self) -> usize {
            self.combined.len()
        }

        /// The number of extended attribute entries held (upstream `Object.keys(_extendedAttrs).length`).
        pub fn extended_attrs_len(&self) -> usize {
            self.extended_attrs.len()
        }

        /// `_data[index * CELL_SIZE + field]` with `Uint32Array` read semantics: 0 out of range.
        fn read(&self, index: usize, field: usize) -> u32 {
            index
                .checked_mul(CELL_SIZE)
                .and_then(|slot| slot.checked_add(field))
                .and_then(|slot| self.data.get(slot))
                .copied()
                .unwrap_or(0)
        }

        /// `_data[index * CELL_SIZE + field] = value` with `Uint32Array` write semantics: ignored
        /// out of range.
        fn write(&mut self, index: usize, field: usize, value: u32) {
            let slot = index
                .checked_mul(CELL_SIZE)
                .and_then(|slot| slot.checked_add(field));
            if let Some(entry) = slot.and_then(|slot| self.data.get_mut(slot)) {
                *entry = value;
            }
        }

        /// Get cell data CharData.
        /// @deprecated (upstream)
        pub fn get(&self, index: usize) -> CharData {
            let content = self.read(index, CELL_CONTENT);
            let cp = content & Content::CODEPOINT_MASK;
            let is_combined = content & Content::IS_COMBINED_MASK != 0;
            let chars = if is_combined {
                self.combined.get(&index).cloned().unwrap_or_default()
            } else if cp != 0 {
                string_from_code_point(cp)
            } else {
                String::new()
            };
            let code = if is_combined {
                last_code_unit(&chars)
            } else {
                cp
            };
            (
                self.read(index, CELL_FG),
                chars,
                content >> Content::WIDTH_SHIFT,
                code,
            )
        }

        /// Set cell data from CharData.
        /// @deprecated (upstream)
        ///
        /// Upstream decides on the string's UTF-16 length (`.length > 1`), so an astral single
        /// character is stored as a combined string here, unlike [`CellData::set_from_char_data`].
        /// The combined cell's code point slot holds the *index* (`index | IS_COMBINED_MASK | ...`),
        /// as upstream writes it.
        pub fn set(&mut self, index: usize, value: &CharData) {
            self.write(index, CELL_FG, value.0);
            let width_bits = value.2 << Content::WIDTH_SHIFT;
            if value.1.encode_utf16().count() > 1 {
                self.combined.insert(index, value.1.clone());
                // `index | ...` is an int32 operation: only the low 32 bits survive.
                let content = (index as u32) | Content::IS_COMBINED_MASK | width_bits;
                self.write(index, CELL_CONTENT, content);
            } else {
                // `''.charCodeAt(0)` is NaN, and `NaN | x` is `x`.
                let unit = value.1.encode_utf16().next().map_or(0, u32::from);
                self.write(index, CELL_CONTENT, unit | width_bits);
            }
        }

        /// primitive getters
        /// use these when only one value is needed, otherwise use `load_cell`
        pub fn get_width(&self, index: usize) -> u32 {
            self.read(index, CELL_CONTENT) >> Content::WIDTH_SHIFT
        }

        /// Test whether content has width.
        pub fn has_width(&self, index: usize) -> u32 {
            self.read(index, CELL_CONTENT) & Content::WIDTH_MASK
        }

        /// Get FG cell component.
        pub fn get_fg(&self, index: usize) -> u32 {
            self.read(index, CELL_FG)
        }

        /// Get BG cell component.
        pub fn get_bg(&self, index: usize) -> u32 {
            self.read(index, CELL_BG)
        }

        /// Test whether contains any chars.
        /// Basically an empty has no content, but other cells might differ in FG/BG
        /// from real empty cells.
        pub fn has_content(&self, index: usize) -> u32 {
            self.read(index, CELL_CONTENT) & Content::HAS_CONTENT_MASK
        }

        /// Get codepoint of the cell.
        /// To be in line with `code` in CharData this either returns
        /// a single UTF32 codepoint or, for a combined string, the **last UTF-16 code unit** of the
        /// string (`charCodeAt(length - 1)`; a low surrogate when the last char is astral).
        pub fn get_code_point(&self, index: usize) -> u32 {
            let content = self.read(index, CELL_CONTENT);
            if content & Content::IS_COMBINED_MASK != 0 {
                return self
                    .combined
                    .get(&index)
                    .map_or(0, |chars| last_code_unit(chars));
            }
            content & Content::CODEPOINT_MASK
        }

        /// Test whether the cell contains a combined string.
        pub fn is_combined(&self, index: usize) -> u32 {
            self.read(index, CELL_CONTENT) & Content::IS_COMBINED_MASK
        }

        /// Returns the string content of the cell.
        pub fn get_string(&self, index: usize) -> String {
            let content = self.read(index, CELL_CONTENT);
            if content & Content::IS_COMBINED_MASK != 0 {
                return self.combined.get(&index).cloned().unwrap_or_default();
            }
            if content & Content::CODEPOINT_MASK != 0 {
                return string_from_code_point(content & Content::CODEPOINT_MASK);
            }
            // return empty string for empty cells
            String::new()
        }

        /// Get state of protected flag.
        pub fn is_protected(&self, index: usize) -> u32 {
            self.read(index, CELL_BG) & BgFlags::PROTECTED
        }

        /// Load data at `index` into `cell`. This is used to access cells in a way that's more
        /// friendly to allocation as it significantly reduces the amount of new objects needed.
        ///
        /// As upstream, `cell.combined_data` is only touched when the cell is combined and
        /// `cell.extended` only when the cell has extended attributes (a missing map entry reads as
        /// the empty string / default attributes).
        pub fn load_cell<'a>(&self, index: usize, cell: &'a mut CellData) -> &'a mut CellData {
            cell.content = self.read(index, CELL_CONTENT);
            cell.fg = self.read(index, CELL_FG);
            cell.bg = self.read(index, CELL_BG);
            if cell.content & Content::IS_COMBINED_MASK != 0 {
                cell.combined_data = self.combined.get(&index).cloned().unwrap_or_default();
            }
            if cell.bg & BgFlags::HAS_EXTENDED != 0 {
                cell.extended = self.extended_attrs.get(&index).copied().unwrap_or_default();
            }
            cell
        }

        /// Set data at `index` to `cell`.
        pub fn set_cell(&mut self, index: usize, cell: &CellData) {
            if cell.content & Content::IS_COMBINED_MASK != 0 {
                self.combined.insert(index, cell.combined_data.clone());
            }
            if cell.bg & BgFlags::HAS_EXTENDED != 0 {
                self.extended_attrs.insert(index, cell.extended);
            }
            self.write(index, CELL_CONTENT, cell.content);
            self.write(index, CELL_FG, cell.fg);
            self.write(index, CELL_BG, cell.bg);
        }

        /// Set cell data from input handler.
        /// Since the input handler see the incoming chars as UTF32 codepoints,
        /// it gets an optimized access method.
        pub fn set_cell_from_codepoint(
            &mut self,
            index: usize,
            code_point: u32,
            width: u32,
            attrs: &AttributeData,
        ) {
            if attrs.bg & BgFlags::HAS_EXTENDED != 0 {
                self.extended_attrs.insert(index, attrs.extended);
            }
            self.write(
                index,
                CELL_CONTENT,
                code_point | (width << Content::WIDTH_SHIFT),
            );
            self.write(index, CELL_FG, attrs.fg);
            self.write(index, CELL_BG, attrs.bg);
        }

        /// Add a codepoint to a cell from input handler.
        /// During input stage combining chars with a width of 0 follow and stack
        /// onto a leading char. Since we already set the attrs
        /// by the previous `set_cell_from_codepoint` call, we can omit it here.
        pub fn add_codepoint_to_cell(&mut self, index: usize, code_point: u32, width: u32) {
            let mut content = self.read(index, CELL_CONTENT);
            if content & Content::IS_COMBINED_MASK != 0 {
                // we already have a combined string, simply add
                let combined = self.combined.entry(index).or_default();
                // LUNETERM: capped (module notes); the width below still applies
                if combined.chars().count() < MAX_COMBINED_CODE_POINTS {
                    combined.push_str(&string_from_code_point(code_point));
                }
            } else if content & Content::CODEPOINT_MASK != 0 {
                // normal case for combining chars:
                //  - move current leading char + new one into combined string
                //  - set combined flag
                let mut chars = string_from_code_point(content & Content::CODEPOINT_MASK);
                chars.push_str(&string_from_code_point(code_point));
                self.combined.insert(index, chars);
                content &= !Content::CODEPOINT_MASK; // set codepoint in buffer to 0
                content |= Content::IS_COMBINED_MASK;
            } else {
                // should not happen - we actually have no data in the cell yet
                // simply set the data in the cell buffer with a width of 1
                content = code_point | (1 << Content::WIDTH_SHIFT);
            }
            if width != 0 {
                content &= !Content::WIDTH_MASK;
                content |= width << Content::WIDTH_SHIFT;
            }
            self.write(index, CELL_CONTENT, content);
        }

        pub fn insert_cells(&mut self, pos: usize, n: usize, fill_cell_data: &CellData) {
            if self.length == 0 {
                return;
            }
            let pos = pos % self.length;

            // handle fullwidth at pos: reset cell one to the left if pos is second cell of a wide char
            if pos != 0 && self.get_width(pos - 1) == 2 {
                self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
            }

            if n < self.length - pos {
                let mut cell = CellData::new();
                for i in (0..self.length - pos - n).rev() {
                    self.load_cell(pos + i, &mut cell);
                    self.set_cell(pos + n + i, &cell);
                }
                // LUNETERM: (review S7) `for i in 0..n { self.set_cell(pos + i, fill) }`, batched
                self.set_cells_repeatedly(pos, 1, n, fill_cell_data, fill_cell_data);
            } else {
                // LUNETERM: (review S7) `for i in pos..self.length { self.set_cell(i, fill) }`
                self.set_cells_repeatedly(
                    pos,
                    1,
                    self.length - pos,
                    fill_cell_data,
                    fill_cell_data,
                );
            }

            // handle fullwidth at line end: reset last cell if it is first cell of a wide char
            if self.get_width(self.length - 1) == 2 {
                self.set_cell_from_codepoint(self.length - 1, 0, 1, fill_cell_data);
            }
        }

        /// LUNETERM: (review S7) `times` repetitions of a `w`-cell unit from `pos`: for each
        /// `p = pos + r × w`, `set_cell(p, head)` and then `set_cell(p + i, stub)` for `i` in
        /// `1..w`. The same cells and entries (data writes past the end are ignored, map entries
        /// are stored at any key, as `set_cell` does), with the cell data written in one pass, the
        /// combined strings inserted as one sorted batch and extended attributes the cells share
        /// as one run, instead of a map insert per cell (review S7 round 4: an underline colour
        /// made every printed cell a `BTreeMap` insert). REP's line fill and `insert_cells`' fill
        /// use it.
        pub fn set_cells_repeatedly(
            &mut self,
            pos: usize,
            w: usize,
            times: usize,
            head: &CellData,
            stub: &CellData,
        ) {
            let end = w
                .checked_mul(times)
                .and_then(|cells| pos.checked_add(cells));
            let Some(end) = end.filter(|_| w != 0) else {
                for r in 0..times {
                    let p = pos.saturating_add(r.saturating_mul(w));
                    self.set_cell(p, head);
                    for i in 1..w {
                        self.set_cell(p.saturating_add(i), stub);
                    }
                }
                return;
            };
            let head_slots = [head.content, head.fg, head.bg];
            let stub_slots = [stub.content, stub.fg, stub.bg];
            let (lo, hi) = (pos.min(self.length), end.min(self.length));
            if let Some(slots) = self.data.get_mut(lo * CELL_SIZE..hi * CELL_SIZE) {
                if w == 1 {
                    for cell in slots.chunks_exact_mut(CELL_SIZE) {
                        cell.copy_from_slice(&head_slots);
                    }
                } else {
                    for (i, cell) in slots.chunks_exact_mut(CELL_SIZE).enumerate() {
                        cell.copy_from_slice(if i % w == 0 { &head_slots } else { &stub_slots });
                    }
                }
            }
            // the entries, in key order: per cell, the head's or the stub's where flagged
            let is_head = |p: usize| (p - pos) % w == 0;
            let head_combined = head.content & Content::IS_COMBINED_MASK != 0;
            let stub_combined = stub.content & Content::IS_COMBINED_MASK != 0 && w > 1;
            if head_combined || stub_combined {
                let updates = (pos..end)
                    .filter_map(|p| match (is_head(p), head_combined, stub_combined) {
                        (true, true, _) => Some((p, head.combined_data.clone())),
                        (false, _, true) => Some((p, stub.combined_data.clone())),
                        _ => None,
                    })
                    .collect();
                insert_sorted(&mut self.combined, updates);
            }
            let head_extended = head.bg & BgFlags::HAS_EXTENDED != 0;
            let stub_extended = stub.bg & BgFlags::HAS_EXTENDED != 0 && w > 1;
            if head_extended && (w == 1 || stub_extended && stub.extended == head.extended) {
                // every cell takes the same attributes: one run
                if let Some(last) = end.checked_sub(1).filter(|&last| last >= pos) {
                    self.extended_attrs.assign(pos, last, Some(head.extended));
                }
            } else if head_extended || stub_extended {
                for p in pos..end {
                    match (is_head(p), head_extended, stub_extended) {
                        (true, true, _) => self.extended_attrs.insert(p, head.extended),
                        (false, _, true) => self.extended_attrs.insert(p, stub.extended),
                        _ => {}
                    }
                }
            }
        }

        /// LUNETERM: (review S7) the insert-mode shifts of printing `times` characters of `n`
        /// cells from `pos` (`InputHandler::print`'s insert path, `edge` = its `cols - 1`), in one
        /// pass:
        ///
        /// ```text
        /// for j in 0..times {
        ///     self.insert_cells(pos + j * n, n, fill);
        ///     if self.get_width(edge) == 2 {
        ///         self.set_cell_from_codepoint(edge, NULL_CELL_CODE, NULL_CELL_WIDTH, edge_attrs);
        ///     }
        /// }
        /// ```
        ///
        /// It leaves the same cells and the same `combined` and `extended_attrs` entries,
        /// including the stale ones each step's shift leaves behind (`copy_cells_from` can carry a
        /// stale combined string back into view, so they are state). Costs O(len) plus the entries
        /// written, where the loop costs O(times × len).
        ///
        /// Why it is the loop: with `pos + times × n <= len`, step `j` inserts at
        /// `pos_j = pos + j × n`. At its start the cells from `pos_j` on are the old cells from
        /// `pos` on, shifted by `j × n` (a step's line-end fix-up rewrites the last cell, which the
        /// next step shifts out), so the cells end as one insert of `times × n` (one call below
        /// does that, with the same fix-ups at `pos - 1` and at the end). Each step's shift copies
        /// old cell `c`'s entries to `c + (j + 1) × n`, so key `k` was last written by the
        /// largest step, the smallest `c` on `k`'s residue class in the window the steps reach
        /// (`k - J × n ..= k - n`, `J` = the steps whose shift reaches `k`), that carries the
        /// entry's flag; the fill of a later step at `k` comes after (`k < pos + times × n`), and
        /// the last cell also takes the line-end fix-up's entry at every step that shifts a wide
        /// cell onto it. The fill cell must not be wide: a wide fill would fire the `pos - 1`
        /// fix-up of every later step.
        ///
        /// The `edge` fix-up never fires at the last cell or past it (the insert's own fix-up
        /// leaves no wide last cell; past the end the width reads 0). Left of the last cell (a
        /// line longer than `cols`, left by a column shrink without reflow) with
        /// `edge + 1 >= pos + times × n`, the cell at `edge` after step `j`'s shift is old cell
        /// `s = edge - (j + 1) × n` or a fill (never wide, and only at the last step), and no
        /// other step touches a cell after it has moved past `edge`. So the fix-up rewrites each
        /// wide old cell on `edge`'s residue class in `edge - times × n ..= edge - n` once, as it
        /// passes `edge`, and that cell moves on as a narrow cell with no combined string and the
        /// edge attributes' extended entry. Hence: key `edge` takes the fix-up's entry after its
        /// shift, as the last cell takes the line-end fix-up's; every key right of `edge` sees the
        /// sources as rewritten (a source that reaches it on `edge`'s class has passed `edge`, and
        /// the rewritten ones are only on that class), keys left of it see the old ones; and a
        /// rewritten cell ends as the fix-up's cell. Outside these conditions this runs the loop
        /// itself.
        pub fn insert_cells_repeatedly(
            &mut self,
            pos: usize,
            n: usize,
            times: usize,
            fill_cell_data: &CellData,
            edge: usize,
            edge_attrs: &AttributeData,
        ) {
            let len = self.length;
            // whether the edge fix-up can fire: only left of the last cell
            let edge_fix = edge.checked_add(1).is_some_and(|next| next < len);
            let total = n.checked_mul(times);
            let end = total.and_then(|total| pos.checked_add(total));
            let Some((total, end)) = total.zip(end).filter(|&(total, end)| {
                total != 0
                    && end <= len
                    && fill_cell_data.get_width() != 2
                    && (!edge_fix || edge + 1 >= end)
            }) else {
                for j in 0..times {
                    self.insert_cells(pos.saturating_add(j.saturating_mul(n)), n, fill_cell_data);
                    if self.get_width(edge) == 2 {
                        self.set_cell_from_codepoint(
                            edge,
                            NULL_CELL_CODE,
                            NULL_CELL_WIDTH,
                            edge_attrs,
                        );
                    }
                }
                return;
            };
            // the old cells a step can shift: `pos..sources_end` (n <= total <= len - pos)
            let sources_end = len - n;
            let fill_extended = fill_cell_data.bg & BgFlags::HAS_EXTENDED != 0;
            let (mut any_combined, mut any_extended, mut any_wide) = (false, false, false);
            for c in pos..sources_end {
                let content = self.read(c, CELL_CONTENT);
                any_combined |= content & Content::IS_COMBINED_MASK != 0;
                any_extended |= self.read(c, CELL_BG) & BgFlags::HAS_EXTENDED != 0;
                any_wide |= content >> Content::WIDTH_SHIFT == 2;
            }
            // Whether a step's shift or fix-up leaves an entry the one insert does not: a combined
            // string (no fill removes one); an extended entry or a wide cell's fix-ups when keys
            // past the fills keep shifted cells (`end < len`; the edge fix-up needs that too); an
            // extended entry under fills that write none. With `end == len` every shifted key is
            // filled later, and a fill with extended attributes overwrites the entry.
            let leaves_entries = any_combined
                || (end < len && (any_extended || any_wide))
                || (any_extended && !fill_extended);
            if !leaves_entries {
                // only the fills write entries, at the keys the one insert fills; its cells are
                // the loop's (the fix-ups' cells are shifted out or filled)
                self.insert_cells(pos, total, fill_cell_data);
                return;
            }
            // the sources the edge fix-up rewrites as they pass `edge`, as indexes: the wide ones
            // on its residue class in `edge - times × n ..= edge - n`
            let edge_extended =
                (edge_attrs.bg & BgFlags::HAS_EXTENDED != 0).then_some(edge_attrs.extended);
            let mut rewritten = Vec::new();
            if edge_fix {
                let mut source = edge.checked_sub(n);
                for _ in 0..times {
                    let Some(s) = source.filter(|&s| s >= pos) else {
                        break;
                    };
                    if self.read(s, CELL_CONTENT) >> Content::WIDTH_SHIFT == 2 {
                        rewritten.push(s - pos);
                    }
                    source = s.checked_sub(n);
                }
            }
            let fill_combined = fill_cell_data.content & Content::IS_COMBINED_MASK != 0;
            if !any_combined
                && !any_extended
                && !fill_combined
                && !fill_extended
                && edge_extended.is_none()
            {
                // no shift copies an entry and no fill or fix-up writes one: the entries stay, and
                // the cells are the one insert's with each rewritten source ending as the
                // fix-up's cell
                self.insert_cells(pos, total, fill_cell_data);
                self.write_edge_fix_ups(pos + total, &rewritten, edge_attrs);
                return;
            }
            // per source `pos + i`, as `load_cell` reads them: its combined string and extended
            // attributes where flagged, and whether it is wide
            let mut combined = Vec::with_capacity(sources_end - pos);
            let mut extended = Vec::with_capacity(sources_end - pos);
            let mut wide = Vec::with_capacity(sources_end - pos);
            for c in pos..sources_end {
                let content = self.read(c, CELL_CONTENT);
                let bg = self.read(c, CELL_BG);
                combined.push(
                    (content & Content::IS_COMBINED_MASK != 0)
                        .then(|| self.combined.get(&c).cloned().unwrap_or_default()),
                );
                extended.push(
                    (bg & BgFlags::HAS_EXTENDED != 0)
                        .then(|| self.extended_attrs.get(&c).copied().unwrap_or_default()),
                );
                wide.push(content >> Content::WIDTH_SHIFT == 2);
            }

            // the sources as the keys right of `edge` see them
            let rewritten_sources = (!rewritten.is_empty()).then(|| {
                let (mut c, mut e, mut w) = (combined.clone(), extended.clone(), wide.clone());
                for &i in &rewritten {
                    if let Some(slot) = c.get_mut(i) {
                        *slot = None;
                    }
                    if let Some(slot) = e.get_mut(i) {
                        *slot = edge_extended;
                    }
                    if let Some(slot) = w.get_mut(i) {
                        *slot = false;
                    }
                }
                (c, e, w)
            });

            // The one insert copies a rewritten source's old entries to its final key, and the
            // line-end fix-up's entry when that key is the last cell; the loop writes neither
            // there (a final key right of `edge`; at `edge` itself the loop's last shift copies
            // the old entries too). Those keys get their entries back before the pass below.
            let restore: Vec<(usize, Option<String>, Option<ExtendedAttrs>)> = rewritten
                .iter()
                .map(|&i| pos + i + total)
                .filter(|&k| k > edge && k < len)
                .map(|k| {
                    (
                        k,
                        self.combined.get(&k).cloned(),
                        self.extended_attrs.get(&k).copied(),
                    )
                })
                .collect();

            self.insert_cells(pos, total, fill_cell_data);

            for (k, chars, attrs) in restore {
                match chars {
                    Some(chars) => self.combined.insert(k, chars),
                    None => self.combined.remove(&k),
                };
                match attrs {
                    Some(attrs) => self.extended_attrs.insert(k, attrs),
                    None => self.extended_attrs.remove(&k),
                };
            }
            self.write_edge_fix_ups(pos + total, &rewritten, edge_attrs);

            // per source `pos + i`: the first source at or after it on its residue class (mod n)
            // that holds an entry, as an index
            let next_held = |held: &dyn Fn(usize) -> bool| {
                let mut next = vec![usize::MAX; sources_end - pos];
                for i in (0..next.len()).rev() {
                    let found = if held(i) {
                        i
                    } else {
                        next.get(i + n).copied().unwrap_or(usize::MAX)
                    };
                    if let Some(slot) = next.get_mut(i) {
                        *slot = found;
                    }
                }
                next
            };
            // (empty where no source holds such an entry: every lookup then finds none)
            let held_combined = |c: &[Option<String>]| match any_combined {
                true => next_held(&|i| c.get(i).is_some_and(Option::is_some)),
                false => Vec::new(),
            };
            let held_extended = |e: &[Option<ExtendedAttrs>], any: bool| match any {
                true => next_held(&|i| e.get(i).is_some_and(Option::is_some)),
                false => Vec::new(),
            };
            let next_combined = held_combined(&combined);
            let next_extended = held_extended(&extended, any_extended);
            let rewritten_next = rewritten_sources.as_ref().map(|(c, e, _)| {
                (
                    held_combined(c),
                    held_extended(e, any_extended || edge_extended.is_some()),
                )
            });
            // A filled key (`k < end`, left of every rewritten source's key) takes no entry from
            // the shifts when the fill writes its own or no source holds one; only the edge
            // fix-up may still write there.
            let skip_filled = (fill_combined || !any_combined) && (fill_extended || !any_extended);
            let mut combined_updates = Vec::new();
            for k in pos + n..len {
                if k < end && skip_filled && !(edge_fix && k == edge) {
                    continue;
                }
                // the steps whose shift reaches key k, and the window of their sources: indexes
                // `low..=high` on k's residue class
                let steps = times.min((k - pos) / n);
                let (low, high) = (k - steps * n - pos, k - n - pos);
                let in_fill = k < end;
                let (combined, extended, wide, next_combined, next_extended) =
                    match (&rewritten_sources, &rewritten_next) {
                        (Some((c, e, w)), Some((nc, ne))) if k > edge => (c, e, w, nc, ne),
                        _ => (&combined, &extended, &wide, &next_combined, &next_extended),
                    };
                let last = |next: &[usize]| next.get(low).copied().filter(|&i| i <= high);
                if !(in_fill && fill_combined) {
                    if let Some(Some(chars)) = last(next_combined).and_then(|i| combined.get(i)) {
                        // (an equal entry is left in place: the same state without a copy)
                        if self.combined.get(&k) != Some(chars) {
                            combined_updates.push((k, chars.clone()));
                        }
                    }
                }
                if in_fill && fill_extended {
                    continue;
                }
                // the window's last entry where a fix-up after each shift writes `fix` over a
                // wide cell (with no fix-up entry, the shifts' last entry)
                let fixed_up = |fix: Option<ExtendedAttrs>| match fix {
                    Some(fix) => (low..=high).step_by(n).find_map(|i| {
                        if wide.get(i) == Some(&true) {
                            Some(fix)
                        } else {
                            extended.get(i).copied().flatten()
                        }
                    }),
                    None => last(next_extended).and_then(|i| extended.get(i).copied().flatten()),
                };
                let value = if k == len - 1 && !in_fill {
                    // the line-end fix-up also writes here, after the shift, when it is wide
                    fixed_up(fill_extended.then_some(fill_cell_data.extended))
                } else if edge_fix && k == edge {
                    // and the edge fix-up here
                    fixed_up(edge_extended)
                } else {
                    last(next_extended).and_then(|i| extended.get(i).copied().flatten())
                };
                if let Some(value) = value {
                    self.extended_attrs.insert(k, value);
                }
            }
            insert_sorted(&mut self.combined, combined_updates);
        }

        /// The cells of `insert_cells_repeatedly`'s rewritten sources (indexes from `pos`, moved
        /// to `end + i`, at `edge` or right of it): the edge fix-up's cell.
        fn write_edge_fix_ups(
            &mut self,
            end: usize,
            rewritten: &[usize],
            edge_attrs: &AttributeData,
        ) {
            for &i in rewritten {
                let k = end.saturating_add(i);
                self.write(
                    k,
                    CELL_CONTENT,
                    NULL_CELL_CODE | (NULL_CELL_WIDTH << Content::WIDTH_SHIFT),
                );
                self.write(k, CELL_FG, edge_attrs.fg);
                self.write(k, CELL_BG, edge_attrs.bg);
            }
        }

        pub fn delete_cells(&mut self, pos: usize, n: usize, fill_cell_data: &CellData) {
            if self.length == 0 {
                return;
            }
            let pos = pos % self.length;
            if n < self.length - pos {
                let mut cell = CellData::new();
                for i in 0..self.length - pos - n {
                    self.load_cell(pos + n + i, &mut cell);
                    self.set_cell(pos + i, &cell);
                }
                for i in self.length - n..self.length {
                    self.set_cell(i, fill_cell_data);
                }
            } else {
                for i in pos..self.length {
                    self.set_cell(i, fill_cell_data);
                }
            }

            // handle fullwidth at pos:
            // - reset pos-1 if wide char
            // - reset pos if width==0 (previous second cell of a wide char)
            if pos != 0 && self.get_width(pos - 1) == 2 {
                self.set_cell_from_codepoint(pos - 1, 0, 1, fill_cell_data);
            }
            if self.get_width(pos) == 0 && self.has_content(pos) == 0 {
                self.set_cell_from_codepoint(pos, 0, 1, fill_cell_data);
            }
        }

        /// `getWidth(end - 1)`; `end == 0` reads index -1 upstream: undefined, width 0. Evaluated
        /// where upstream evaluates it, i.e. after the start-side reset above may have changed it.
        fn width_before(&self, end: usize) -> u32 {
            end.checked_sub(1).map_or(0, |i| self.get_width(i))
        }

        pub fn replace_cells(
            &mut self,
            start: usize,
            end: usize,
            fill_cell_data: &CellData,
            respect_protect: bool,
        ) {
            let mut start = start;

            // full branching on respectProtect==true, hopefully getting fast JIT for standard case
            if respect_protect {
                if start != 0 && self.get_width(start - 1) == 2 && self.is_protected(start - 1) == 0
                {
                    self.set_cell_from_codepoint(start - 1, 0, 1, fill_cell_data);
                }
                if end < self.length && self.width_before(end) == 2 && self.is_protected(end) == 0 {
                    self.set_cell_from_codepoint(end, 0, 1, fill_cell_data);
                }
                while start < end && start < self.length {
                    if self.is_protected(start) == 0 {
                        self.set_cell(start, fill_cell_data);
                    }
                    start += 1;
                }
                return;
            }

            // handle fullwidth at start: reset cell one to the left if start is second cell of a
            // wide char
            if start != 0 && self.get_width(start - 1) == 2 {
                self.set_cell_from_codepoint(start - 1, 0, 1, fill_cell_data);
            }
            // handle fullwidth at last cell + 1: reset to empty cell if it is second part of a wide
            // char
            if end < self.length && self.width_before(end) == 2 {
                self.set_cell_from_codepoint(end, 0, 1, fill_cell_data);
            }

            while start < end && start < self.length {
                self.set_cell(start, fill_cell_data);
                start += 1;
            }
        }

        /// Resize ReferenceBufferLine to `cols` filling excess cells with `fill_cell_data`.
        /// The underlying array buffer will not change if there is still enough space
        /// to hold the new buffer line data.
        /// Returns a boolean indicating, whether a `cleanup_memory` call would free
        /// excess memory (true after shrinking > CLEANUP_THRESHOLD).
        ///
        /// "Buffer" is the `Vec`'s capacity (see the module notes). Growing past the capacity
        /// allocates exactly `cols * CELL_SIZE` slots, as the fresh `Uint32Array` does.
        pub fn resize(&mut self, cols: usize, fill_cell_data: &CellData) -> bool {
            if cols == self.length {
                return self.excess_buffer(self.data.len());
            }
            let uint32_cells = cols.saturating_mul(CELL_SIZE);
            if cols > self.length {
                if self.data.capacity() >= uint32_cells {
                    // optimization: avoid alloc and data copy if buffer has enough room
                    self.data.resize(uint32_cells, 0);
                } else {
                    // slow path: new alloc and full data copy
                    let mut data = Vec::with_capacity(uint32_cells);
                    data.extend_from_slice(&self.data);
                    data.resize(uint32_cells, 0);
                    self.data = data;
                }
                for i in self.length..cols {
                    self.set_cell(i, fill_cell_data);
                }
            } else {
                // optimization: just shrink the view on existing buffer
                self.data.truncate(uint32_cells);
                // Remove any cut off combined data
                self.combined.retain(|&key, _| key < cols);
                // remove any cut off extended attributes
                self.extended_attrs.truncate(cols);
            }
            self.length = cols;
            self.excess_buffer(uint32_cells)
        }

        /// `used * 4 * CLEANUP_THRESHOLD < buffer.byteLength` (in `u32` slots: `used` slots against
        /// the capacity).
        fn excess_buffer(&self, used: usize) -> bool {
            used.saturating_mul(CLEANUP_THRESHOLD) < self.data.capacity()
        }

        /// Cleanup underlying array buffer.
        /// A cleanup will be triggered if the array buffer exceeds the actual used
        /// memory by a factor of CLEANUP_THRESHOLD.
        /// Returns 0 or 1 indicating whether a cleanup happened.
        pub fn cleanup_memory(&mut self) -> u32 {
            if self.excess_buffer(self.data.len()) {
                let mut data = Vec::with_capacity(self.data.len());
                data.extend_from_slice(&self.data);
                self.data = data;
                return 1;
            }
            0
        }

        /// fill a line with fill_cell_data
        pub fn fill(&mut self, fill_cell_data: &CellData, respect_protect: bool) {
            // full branching on respectProtect==true, hopefully getting fast JIT for standard case
            if respect_protect {
                for i in 0..self.length {
                    if self.is_protected(i) == 0 {
                        self.set_cell(i, fill_cell_data);
                    }
                }
                return;
            }
            self.combined.clear();
            self.extended_attrs.clear();
            for i in 0..self.length {
                self.set_cell(i, fill_cell_data);
            }
        }

        /// alter to a full copy of line
        pub fn copy_from(&mut self, line: &ReferenceBufferLine) {
            if self.length != line.length {
                self.data = line.data.clone();
            } else {
                // reuse the buffer if lengths are equal
                self.data.clear();
                self.data.extend_from_slice(&line.data);
            }
            self.length = line.length;
            self.combined = line.combined.clone();
            self.extended_attrs = line.extended_attrs.clone();
            self.is_wrapped = line.is_wrapped;
        }

        // `clone` is the derived `Clone`: a fresh exact-size buffer, both maps, `is_wrapped`.

        pub fn get_trimmed_length(&self) -> usize {
            for i in (0..self.length).rev() {
                let content = self.read(i, CELL_CONTENT);
                if content & Content::HAS_CONTENT_MASK != 0 {
                    return i + (content >> Content::WIDTH_SHIFT) as usize;
                }
            }
            0
        }

        pub fn get_no_bg_trimmed_length(&self) -> usize {
            for i in (0..self.length).rev() {
                let content = self.read(i, CELL_CONTENT);
                if content & Content::HAS_CONTENT_MASK != 0
                    || self.read(i, CELL_BG) & Attributes::CM_MASK != 0
                {
                    return i + (content >> Content::WIDTH_SHIFT) as usize;
                }
            }
            0
        }

        /// Copy `len` cells of `src` starting at `src_col` to `self` starting at `dest_col`.
        ///
        /// Out-of-range reads of `src` copy 0, out-of-range writes to `self` are ignored (typed
        /// array semantics). Extended attributes are copied per cell; **combined strings are copied
        /// for every key `>= src_col` of `src`** (ascending), not only for the copied range, as
        /// upstream does.
        pub fn copy_cells_from(
            &mut self,
            src: &ReferenceBufferLine,
            src_col: usize,
            dest_col: usize,
            len: usize,
            apply_in_reverse: bool,
        ) {
            if apply_in_reverse {
                for cell in (0..len).rev() {
                    self.copy_cell_from(src, src_col, dest_col, cell);
                }
            } else {
                for cell in 0..len {
                    self.copy_cell_from(src, src_col, dest_col, cell);
                }
            }

            // Move any combined data over as needed, FIXME: repeat for extended attrs
            for (&key, value) in src.combined.range(src_col..) {
                self.combined
                    .insert((key - src_col).saturating_add(dest_col), value.clone());
            }
        }

        fn copy_cell_from(
            &mut self,
            src: &ReferenceBufferLine,
            src_col: usize,
            dest_col: usize,
            cell: usize,
        ) {
            let src_index = src_col.saturating_add(cell);
            let dest_index = dest_col.saturating_add(cell);
            for field in 0..CELL_SIZE {
                self.write(dest_index, field, src.read(src_index, field));
            }
            if src.read(src_index, CELL_BG) & BgFlags::HAS_EXTENDED != 0 {
                match src.extended_attrs.get(&src_index) {
                    Some(&attrs) => {
                        self.extended_attrs.insert(dest_index, attrs);
                    }
                    None => {
                        self.extended_attrs.remove(&dest_index);
                    }
                }
            }
        }

        /// [`copy_cells_from`](Self::copy_cells_from) with `src` being this line itself (upstream
        /// `line.copyCellsFrom(line, ...)`, used by reflow): the same loop over the line's own data
        /// and maps. The cell copy reads cells it has already overwritten when the ranges overlap and
        /// `apply_in_reverse` does not match the direction, and the combined strings are read live
        /// from a snapshot of the keys taken up front, so a forward overlapping shift cascades
        /// (`copy_cells_within(1, 2, 2, false)` over combined cells at 1 and 2 leaves cell 1's string
        /// at 1, 2 and 3). Keys written by the loop are not visited.
        pub fn copy_cells_within(
            &mut self,
            src_col: usize,
            dest_col: usize,
            len: usize,
            apply_in_reverse: bool,
        ) {
            if apply_in_reverse {
                for cell in (0..len).rev() {
                    self.copy_cell_within(src_col, dest_col, cell);
                }
            } else {
                for cell in 0..len {
                    self.copy_cell_within(src_col, dest_col, cell);
                }
            }

            // Move any combined data over as needed, FIXME: repeat for extended attrs
            let keys: Vec<usize> = self
                .combined
                .range(src_col..)
                .map(|(&key, _)| key)
                .collect();
            for key in keys {
                if let Some(value) = self.combined.get(&key).cloned() {
                    self.combined
                        .insert((key - src_col).saturating_add(dest_col), value);
                }
            }
        }

        fn copy_cell_within(&mut self, src_col: usize, dest_col: usize, cell: usize) {
            let src_index = src_col.saturating_add(cell);
            let dest_index = dest_col.saturating_add(cell);
            for field in 0..CELL_SIZE {
                let value = self.read(src_index, field);
                self.write(dest_index, field, value);
            }
            if self.read(src_index, CELL_BG) & BgFlags::HAS_EXTENDED != 0 {
                match self.extended_attrs.get(&src_index).copied() {
                    Some(attrs) => {
                        self.extended_attrs.insert(dest_index, attrs);
                    }
                    None => {
                        self.extended_attrs.remove(&dest_index);
                    }
                }
            }
        }

        /// Translates the buffer line to a string.
        ///
        /// * `trim_right` - Whether to trim any empty cells on the right.
        /// * `start_col` - The column to start the string (0-based inclusive), default 0.
        /// * `end_col` - The column to end the string (0-based exclusive), default `len()`.
        /// * `out_columns` - if specified, this vector is cleared and filled with column numbers such
        ///   that the UTF-16 code unit `i` of the returned string is displayed at column
        ///   `out_columns[i]`; `out_columns[units]` (`units` = the string's UTF-16 length) is where
        ///   the character following the string will be displayed.
        ///
        /// **`out_columns` is indexed by UTF-16 code units, not by `char`s or bytes** (upstream
        /// contract): when a single cell is translated to multiple code units (e.g. a surrogate pair
        /// for an astral char) the corresponding entries have the same column number.
        ///
        /// The cost is `O(end_col - start_col)` even past `len()` (cells there read as spaces).
        pub fn translate_to_string(
            &self,
            trim_right: bool,
            start_col: Option<usize>,
            end_col: Option<usize>,
            out_columns: Option<&mut Vec<usize>>,
        ) -> String {
            let mut start_col = start_col.unwrap_or(0);
            let mut end_col = end_col.unwrap_or(self.length);
            let mut out_columns = out_columns;
            if trim_right {
                end_col = end_col.min(self.get_trimmed_length());
            }
            if let Some(columns) = out_columns.as_deref_mut() {
                columns.clear();
            }
            let mut result = String::new();
            while start_col < end_col {
                let content = self.read(start_col, CELL_CONTENT);
                let cp = content & Content::CODEPOINT_MASK;
                let units = if content & Content::IS_COMBINED_MASK != 0 {
                    let chars = self
                        .combined
                        .get(&start_col)
                        .map_or("", |chars| chars.as_str());
                    result.push_str(chars);
                    chars.encode_utf16().count()
                } else if cp != 0 {
                    // as `string_from_code_point`
                    let ch = char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER);
                    result.push(ch);
                    ch.len_utf16()
                } else {
                    result.push_str(WHITESPACE_CELL_CHAR);
                    WHITESPACE_CELL_CHAR.len()
                };
                if let Some(columns) = out_columns.as_deref_mut() {
                    for _ in 0..units {
                        columns.push(start_col);
                    }
                }
                // always advance by at least 1
                start_col =
                    start_col.saturating_add(((content >> Content::WIDTH_SHIFT) as usize).max(1));
            }
            if let Some(columns) = out_columns {
                columns.push(start_col);
            }
            result
        }
    }

    /// Plan M7 step 12 (S7 residual 2): every read of a `BufferLine` equals the reference's (the
    /// per-key `BTreeMap<usize, String>` combined map), stale keys and keys past `len()` included,
    /// after every operation of a seeded random sequence over lines with combined content.
    /// `LUNE_VT_COMBINED_DIFF_SEEDS=<n>` runs more seeds; `LUNE_VT_COMBINED_DIFF_SEED=<n>` replays
    /// one.
    #[test]
    fn buffer_line_equals_the_reference_combined_map() {
        if let Some(seed) = std::env::var("LUNE_VT_COMBINED_DIFF_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            run_combined_differential(seed, 200);
            return;
        }
        let seeds: u64 = std::env::var("LUNE_VT_COMBINED_DIFF_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(200);
        for seed in 0..seeds {
            run_combined_differential(seed, 200);
        }
    }

    /// A line under test and its reference, driven through the same operations.
    struct Pair {
        new: BufferLine,
        old: ReferenceBufferLine,
    }

    /// Every key the line's combined map holds, in order, with its string.
    fn combined_entries(line: &BufferLine) -> Vec<(usize, String)> {
        line.combined
            .entries()
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect()
    }

    fn assert_same_reads(pair: &Pair, context: &str) {
        let (new, old) = (&pair.new, &pair.old);
        assert_eq!(new.length, old.length, "{context}: len");
        assert_eq!(new.is_wrapped, old.is_wrapped, "{context}: is_wrapped");
        assert_eq!(new.data, old.data, "{context}: data");
        assert_eq!(
            new.extended_attrs, old.extended_attrs,
            "{context}: extended"
        );
        let old_entries: Vec<(usize, String)> =
            old.combined.iter().map(|(&k, v)| (k, v.clone())).collect();
        assert_eq!(combined_entries(new), old_entries, "{context}: combined");
        assert_eq!(
            new.combined_len(),
            old.combined_len(),
            "{context}: combined_len"
        );
        let probes = (0..new.len() + 6).chain([usize::MAX - 1, usize::MAX]);
        for i in probes {
            assert_eq!(new.get(i), old.get(i), "{context}: get({i})");
            assert_eq!(
                new.get_string(i),
                old.get_string(i),
                "{context}: get_string({i})"
            );
            assert_eq!(
                new.is_combined(i),
                old.is_combined(i),
                "{context}: is_combined({i})"
            );
            assert_eq!(
                new.get_code_point(i),
                old.get_code_point(i),
                "{context}: get_code_point({i})"
            );
            // a cell that already holds data: `load_cell` touches the string only when combined
            let mut seeded = CellData::new();
            seeded.combined_data = "prior".into();
            seeded.extended = ExtendedAttrs::new(3, 4);
            let (mut a, mut b) = (seeded.clone(), seeded);
            new.load_cell(i, &mut a);
            old.load_cell(i, &mut b);
            assert_eq!(a, b, "{context}: load_cell({i})");
        }
        for trim in [false, true] {
            let (mut a, mut b) = (Vec::new(), Vec::new());
            assert_eq!(
                new.translate_to_string(trim, None, None, Some(&mut a)),
                old.translate_to_string(trim, None, None, Some(&mut b)),
                "{context}: translate_to_string({trim})"
            );
            assert_eq!(a, b, "{context}: out_columns({trim})");
        }
        assert_eq!(
            new.get_trimmed_length(),
            old.get_trimmed_length(),
            "{context}: trimmed length"
        );
    }

    fn run_combined_differential(seed: u64, ops: usize) {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound.max(1) as u64) as usize
        };
        let ext = |v: usize| ExtendedAttrs::new(v as u32 + 1, v as u32);
        // a few distinct cluster strings, so neighbouring cells sometimes hold equal ones
        let cluster = |v: usize| -> String {
            match v % 4 {
                0 => "e\u{301}".into(),
                1 => "o\u{308}".into(),
                2 => "\u{1F469}\u{200D}".into(),
                _ => format!("{}\u{301}", char::from(b'a' + (v % 26) as u8)),
            }
        };
        let mut lines: Vec<Pair> = (0..2)
            .map(|_| {
                let len = next(13);
                Pair {
                    new: BufferLine::new(len, None, false),
                    old: ReferenceBufferLine::new(len, None, false),
                }
            })
            .collect();
        for op in 0..ops {
            let cell_of = |next: &mut dyn FnMut(usize) -> usize| {
                let mut cell = CellData::new();
                let width = [1, 1, 1, 2, 0][next(5)];
                match next(4) {
                    0 => cell.content = NULL_CELL_CODE | (NULL_CELL_WIDTH << Content::WIDTH_SHIFT),
                    1 => {
                        cell.content =
                            (u32::from('a') + next(26) as u32) | (width << Content::WIDTH_SHIFT)
                    }
                    _ => {
                        cell.content = Content::IS_COMBINED_MASK | (width << Content::WIDTH_SHIFT);
                        cell.combined_data = cluster(next(8));
                    }
                }
                cell.fg = next(3) as u32;
                if next(3) == 0 {
                    cell.bg |= BgFlags::HAS_EXTENDED;
                    cell.extended = ext(next(3));
                }
                if next(8) == 0 {
                    cell.bg |= BgFlags::PROTECTED;
                }
                cell
            };
            let target = next(4).min(1);
            let other = 1 - target;
            let len = lines[target].new.len();
            let index = |next: &mut dyn FnMut(usize) -> usize| match next(16) {
                0 => usize::MAX - next(3),
                _ => next(len + 4),
            };
            let kind = next(17);
            let label;
            match kind {
                0 => {
                    let i = index(&mut next);
                    let cell = cell_of(&mut next);
                    label = format!("set_cell({i}, {cell:?})");
                    lines[target].new.set_cell(i, &cell);
                    lines[target].old.set_cell(i, &cell);
                }
                1 => {
                    let i = index(&mut next);
                    let attrs = *cell_of(&mut next);
                    let cp = u32::from('a') + next(26) as u32;
                    let w = next(3) as u32;
                    label = format!("set_cell_from_codepoint({i}, {cp}, {w})");
                    lines[target].new.set_cell_from_codepoint(i, cp, w, &attrs);
                    lines[target].old.set_cell_from_codepoint(i, cp, w, &attrs);
                }
                2 => {
                    let i = index(&mut next);
                    let w = next(3) as u32;
                    // sometimes enough to reach the code point cap
                    let times = if next(6) == 0 { 70 } else { 1 + next(3) };
                    label = format!("add_codepoint_to_cell({i}, x{times}, {w})");
                    for t in 0..times {
                        let cp = [0x301, 0x308, 0x200D, 0xFE0F][(t + i % 4) % 4];
                        lines[target].new.add_codepoint_to_cell(i, cp, w);
                        lines[target].old.add_codepoint_to_cell(i, cp, w);
                    }
                }
                3 => {
                    let (pos, n) = (next(len + 2), next(len + 2));
                    let fill = cell_of(&mut next);
                    label = format!("insert_cells({pos}, {n})");
                    lines[target].new.insert_cells(pos, n, &fill);
                    lines[target].old.insert_cells(pos, n, &fill);
                }
                4 => {
                    let (pos, n) = (next(len + 2), next(len + 2));
                    let fill = cell_of(&mut next);
                    label = format!("delete_cells({pos}, {n})");
                    lines[target].new.delete_cells(pos, n, &fill);
                    lines[target].old.delete_cells(pos, n, &fill);
                }
                5 => {
                    let (start, end) = (next(len + 2), next(len + 3));
                    let fill = cell_of(&mut next);
                    let protect = next(2) == 0;
                    label = format!("replace_cells({start}, {end}, {protect})");
                    lines[target].new.replace_cells(start, end, &fill, protect);
                    lines[target].old.replace_cells(start, end, &fill, protect);
                }
                6 => {
                    let cols = next(16);
                    let fill = cell_of(&mut next);
                    label = format!("resize({cols})");
                    let a = lines[target].new.resize(cols, &fill);
                    let b = lines[target].old.resize(cols, &fill);
                    assert_eq!(a, b, "seed {seed} op {op}: {label} result");
                }
                7 => {
                    let fill = cell_of(&mut next);
                    let protect = next(2) == 0;
                    label = format!("fill({protect})");
                    lines[target].new.fill(&fill, protect);
                    lines[target].old.fill(&fill, protect);
                }
                8 => {
                    let src_col = next(lines[other].new.len() + 3);
                    let dest_col = index(&mut next);
                    let n = next(len + 3);
                    let reverse = next(2) == 0;
                    label = format!(
                        "copy_cells_from(src_col {src_col}, dest_col {dest_col}, {n}, {reverse})"
                    );
                    let (src_new, src_old) = (lines[other].new.clone(), lines[other].old.clone());
                    lines[target]
                        .new
                        .copy_cells_from(&src_new, src_col, dest_col, n, reverse);
                    lines[target]
                        .old
                        .copy_cells_from(&src_old, src_col, dest_col, n, reverse);
                }
                9 => {
                    let (src_col, dest_col) = (next(len + 3), next(len + 3));
                    let n = next(len + 3);
                    let reverse = next(2) == 0;
                    label = format!("copy_cells_within({src_col}, {dest_col}, {n}, {reverse})");
                    lines[target]
                        .new
                        .copy_cells_within(src_col, dest_col, n, reverse);
                    lines[target]
                        .old
                        .copy_cells_within(src_col, dest_col, n, reverse);
                }
                10 | 11 => {
                    let pos = next(len + 4);
                    let w = next(4);
                    let times = next(len + 20);
                    let (head, stub) = (cell_of(&mut next), cell_of(&mut next));
                    label =
                        format!("set_cells_repeatedly({pos}, {w}, {times}, {head:?}, {stub:?})");
                    lines[target]
                        .new
                        .set_cells_repeatedly(pos, w, times, &head, &stub);
                    lines[target]
                        .old
                        .set_cells_repeatedly(pos, w, times, &head, &stub);
                }
                12 | 13 => {
                    let pos = next(len + 1);
                    let n = next(3);
                    let times = next((len.saturating_sub(pos)) / n.max(1) + 2);
                    let edge = match next(3) {
                        0 => next(len + 3),
                        _ => len.saturating_sub(1),
                    };
                    let fill = cell_of(&mut next);
                    let edge_attrs = *cell_of(&mut next);
                    label = format!("insert_cells_repeatedly({pos}, {n}, {times}, edge {edge})");
                    lines[target].new.insert_cells_repeatedly(
                        pos,
                        n,
                        times,
                        &fill,
                        edge,
                        &edge_attrs,
                    );
                    lines[target].old.insert_cells_repeatedly(
                        pos,
                        n,
                        times,
                        &fill,
                        edge,
                        &edge_attrs,
                    );
                }
                14 => {
                    label = "cleanup_memory".into();
                    let a = lines[target].new.cleanup_memory();
                    let b = lines[target].old.cleanup_memory();
                    assert_eq!(a, b, "seed {seed} op {op}: {label} result");
                }
                15 => {
                    label = "copy_from".into();
                    let (src_new, src_old) = (lines[other].new.clone(), lines[other].old.clone());
                    lines[target].new.copy_from(&src_new);
                    lines[target].old.copy_from(&src_old);
                }
                _ => {
                    let i = index(&mut next);
                    let chars = match next(3) {
                        0 => String::new(),
                        1 => "x".into(),
                        _ => cluster(next(8)),
                    };
                    let value = (next(3) as u32, chars, next(3) as u32, 0);
                    label = format!("set({i}, {value:?})");
                    lines[target].new.set(i, &value);
                    lines[target].old.set(i, &value);
                }
            }
            let context = format!("seed {seed} op {op} line {target}: {label}");
            assert_same_reads(&lines[target], &context);
            // a clone reads as its source
            let cloned = Pair {
                new: lines[target].new.clone(),
                old: lines[target].old.clone(),
            };
            assert_same_reads(&cloned, &format!("{context} (clone)"));
        }
    }
}
