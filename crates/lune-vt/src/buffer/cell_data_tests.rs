//! Port of the `CellData` describe of xterm.js src/common/buffer/BufferLine.test.ts
//! (`:137-162`) @ f447274, plus Rust-only cases for the paths the single upstream test skips.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::buffer::constants::{BgFlags, Content};

/// `'x'.charCodeAt(index)`.
fn unit(s: &str, index: usize) -> u32 {
    u32::from(s.encode_utf16().nth(index).unwrap())
}

fn cd(attr: u32, chars: &str, width: u32, code: u32) -> CharData {
    (attr, chars.to_string(), width, code)
}

mod cell_data {
    use super::*;

    #[test]
    fn char_data_cell_data_equality() {
        let mut cell = CellData::new();
        // ASCII
        cell.set_from_char_data(&cd(123, "a", 1, u32::from(b'a')));
        assert_eq!(cell.get_as_char_data(), cd(123, "a", 1, u32::from(b'a')));
        assert_eq!(cell.is_combined(), 0);
        // combining
        let combining = "e\u{301}";
        cell.set_from_char_data(&cd(123, combining, 1, unit("\u{301}", 0)));
        assert_eq!(
            cell.get_as_char_data(),
            cd(123, combining, 1, unit("\u{301}", 0))
        );
        assert_eq!(cell.is_combined(), Content::IS_COMBINED_MASK);
        // surrogate
        cell.set_from_char_data(&cd(123, "𝄞", 1, 0x1D11E));
        assert_eq!(cell.get_as_char_data(), cd(123, "𝄞", 1, 0x1D11E));
        assert_eq!(cell.is_combined(), 0);
        // surrogate + combining
        let astral_combining = "𓂀\u{301}";
        cell.set_from_char_data(&cd(123, astral_combining, 1, unit(astral_combining, 2)));
        assert_eq!(
            cell.get_as_char_data(),
            cd(123, astral_combining, 1, unit(astral_combining, 2))
        );
        assert_eq!(cell.is_combined(), Content::IS_COMBINED_MASK);
        // wide char
        cell.set_from_char_data(&cd(123, "１", 2, unit("１", 0)));
        assert_eq!(cell.get_as_char_data(), cd(123, "１", 2, unit("１", 0)));
        assert_eq!(cell.is_combined(), 0);
    }
}

// Rust-only.
mod set_from_char_data {
    use super::*;

    #[test]
    fn from_char_data_builds_the_same_cell() {
        let value = cd(5, "x", 1, u32::from(b'x'));
        let mut a = CellData::new();
        a.set_from_char_data(&value);
        assert_eq!(CellData::from_char_data(&value), a);
    }

    #[test]
    fn resets_bg_and_takes_fg_from_the_attr() {
        let mut cell = CellData::new();
        cell.bg = BgFlags::ITALIC;
        cell.set_from_char_data(&cd(77, "a", 1, 97));
        assert_eq!(cell.fg, 77);
        assert_eq!(cell.bg, 0);
    }

    #[test]
    fn content_packs_codepoint_and_width() {
        let mut cell = CellData::new();
        cell.set_from_char_data(&cd(0, "１", 2, 0xFF11));
        assert_eq!(cell.content, 0xFF11 | (2 << Content::WIDTH_SHIFT));
        assert_eq!(cell.get_width(), 2);
        assert_eq!(cell.get_code(), 0xFF11);
        assert_eq!(cell.get_chars(), "１");
    }

    #[test]
    fn empty_string_is_an_empty_cell_with_the_given_width() {
        let mut cell = CellData::new();
        // A previous combined cell must not leak into the new one.
        cell.set_from_char_data(&cd(0, "e\u{301}", 1, 0x301));
        cell.set_from_char_data(&cd(0, "", 1, 0));
        assert_eq!(cell.content, 1 << Content::WIDTH_SHIFT);
        assert_eq!(cell.is_combined(), 0);
        assert_eq!(cell.get_chars(), "");
        assert_eq!(cell.get_code(), 0);
    }

    #[test]
    fn two_bmp_chars_are_combined() {
        let mut cell = CellData::new();
        cell.set_from_char_data(&cd(0, "ab", 1, 98));
        assert_eq!(cell.is_combined(), Content::IS_COMBINED_MASK);
        assert_eq!(cell.get_chars(), "ab");
        assert_eq!(cell.get_code(), 98);
    }

    #[test]
    fn two_astral_chars_are_combined_and_code_is_the_last_utf16_unit() {
        let mut cell = CellData::new();
        let s = "𝄞𓂀"; // four UTF-16 units
        cell.set_from_char_data(&cd(0, s, 1, 0));
        assert_eq!(cell.is_combined(), Content::IS_COMBINED_MASK);
        assert_eq!(cell.get_chars(), s);
        // A low surrogate despite the doc comment upstream (`charCodeAt(length - 1)`).
        assert_eq!(cell.get_code(), unit(s, 3));
        assert!((0xDC00..=0xDFFF).contains(&cell.get_code()));
    }

    #[test]
    fn combined_cell_with_empty_data_has_code_zero() {
        let mut cell = CellData::new();
        cell.content = Content::IS_COMBINED_MASK;
        assert_eq!(cell.get_code(), 0);
        assert_eq!(cell.get_chars(), "");
    }

    #[test]
    fn non_scalar_content_reads_as_the_replacement_char() {
        let mut cell = CellData::new();
        cell.content = 0xD800; // a lone surrogate cannot be a Rust char
        assert_eq!(cell.get_chars(), "\u{FFFD}");
        cell.content = 0x1F_FFFF; // above U+10FFFF
        assert_eq!(cell.get_chars(), "\u{FFFD}");
    }

    #[test]
    fn attribute_methods_are_reachable_through_the_cell() {
        let mut cell = CellData::new();
        cell.fg = crate::buffer::constants::FgFlags::BOLD;
        assert_eq!(cell.is_bold(), crate::buffer::constants::FgFlags::BOLD);
        let attrs: &AttributeData = &cell;
        assert_eq!(attrs.fg, cell.fg);
    }
}
