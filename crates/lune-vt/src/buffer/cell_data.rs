//! Port of xterm.js src/common/buffer/CellData.ts @ f447274.
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D7):
//!  - `CellData extends AttributeData` becomes composition plus `Deref`/`DerefMut` to the
//!    embedded [`AttributeData`], so `cell.fg`, `cell.extended` and every attribute method read
//!    as they do upstream.
//!  - A string is *combined* iff it holds more than one code point (upstream: more than one
//!    UTF-16 unit, except a valid surrogate pair). A lone surrogate cannot exist in a Rust
//!    `String`, so upstream's "high surrogate not followed by a low one" case does not arise.
//!  - [`CharData`] (Types.ts) is a tuple `(attr, chars, width, code)`; the code is the last
//!    UTF-16 code unit of a combined string, see [`CellData::get_code`].

use std::ops::{Deref, DerefMut};

use crate::buffer::attribute_data::AttributeData;
use crate::buffer::constants::Content;
use crate::input::text_decoder::string_from_code_point;

/// `[attr, chars, width, code]` (upstream `CharData`).
pub type CharData = (u32, String, u32, u32);

/// CellData - represents a single Cell in the terminal buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellData {
    /// Primitives from terminal buffer.
    pub content: u32,
    pub combined_data: String,
    attrs: AttributeData,
}

impl Deref for CellData {
    type Target = AttributeData;
    fn deref(&self) -> &AttributeData {
        &self.attrs
    }
}

impl DerefMut for CellData {
    fn deref_mut(&mut self) -> &mut AttributeData {
        &mut self.attrs
    }
}

impl CellData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Helper to create CellData from CharData.
    pub fn from_char_data(value: &CharData) -> CellData {
        let mut obj = CellData::new();
        obj.set_from_char_data(value);
        obj
    }

    /// Whether cell contains a combined string.
    pub fn is_combined(&self) -> u32 {
        self.content & Content::IS_COMBINED_MASK
    }

    /// Width of the cell.
    pub fn get_width(&self) -> u32 {
        self.content >> Content::WIDTH_SHIFT
    }

    /// The string of the content.
    pub fn get_chars(&self) -> String {
        if self.content & Content::IS_COMBINED_MASK != 0 {
            return self.combined_data.clone();
        }
        if self.content & Content::CODEPOINT_MASK != 0 {
            return string_from_code_point(self.content & Content::CODEPOINT_MASK);
        }
        String::new()
    }

    /// Code of the cell: the code point of a single char; for a combined string the **last
    /// UTF-16 code unit** of the string (`charCodeAt(length - 1)`), which is a low surrogate when
    /// the last char is astral, despite upstream's doc comment. 0 for an empty combined string
    /// (upstream: `NaN`).
    pub fn get_code(&self) -> u32 {
        if self.is_combined() != 0 {
            self.combined_data
                .encode_utf16()
                .last()
                .map_or(0, u32::from)
        } else {
            self.content & Content::CODEPOINT_MASK
        }
    }

    /// Set data from CharData.
    pub fn set_from_char_data(&mut self, value: &CharData) {
        self.attrs.fg = value.0;
        self.attrs.bg = 0;
        let width_bits = value.2 << Content::WIDTH_SHIFT;
        let mut chars = value.1.chars();
        match (chars.next(), chars.next()) {
            // `''.charCodeAt(0)` is NaN, and `NaN | x` is `x`.
            (None, _) => self.content = width_bits,
            (Some(c), None) => self.content = u32::from(c) | width_bits,
            _ => {
                self.combined_data = value.1.clone();
                self.content = Content::IS_COMBINED_MASK | width_bits;
            }
        }
    }

    /// Get data as CharData.
    pub fn get_as_char_data(&self) -> CharData {
        (
            self.attrs.fg,
            self.get_chars(),
            self.get_width(),
            self.get_code(),
        )
    }
}

#[cfg(test)]
#[path = "cell_data_tests.rs"]
mod tests;
