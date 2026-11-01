//! Port of xterm.js src/common/buffer/Constants.ts @ f447274.
//!
//! Rust notes: TypeScript's `const enum`s are empty enums with associated `u32` constants
//! (`Content::CODEPOINT_MASK`, read like upstream's `Content.CODEPOINT_MASK`). Every value is a
//! JS number that the bit operators treat as an int32; here they are `u32`, which holds the same
//! 32 bits (so `FgFlags::STRIKETHROUGH` is `0x8000_0000`, not a negative number).

pub const DEFAULT_COLOR: u32 = 0;
pub const DEFAULT_ATTR: u32 = (DEFAULT_COLOR << 9) | 256;
pub const DEFAULT_EXT: u32 = 0;

// Indices into a `CharData` (`buffer::cell_data::CharData` is a tuple, so these name its
// fields `.0` to `.3`; kept so upstream's constants all exist).
pub const CHAR_DATA_ATTR_INDEX: usize = 0;
pub const CHAR_DATA_CHAR_INDEX: usize = 1;
pub const CHAR_DATA_WIDTH_INDEX: usize = 2;
pub const CHAR_DATA_CODE_INDEX: usize = 3;

/// Null cell - a real empty cell (containing nothing).
/// Note that code should always be 0 for a null cell as
/// several test condition of the buffer line rely on this.
pub const NULL_CELL_CHAR: &str = "";
pub const NULL_CELL_WIDTH: u32 = 1;
pub const NULL_CELL_CODE: u32 = 0;

/// Whitespace cell.
/// This is meant as a replacement for empty cells when needed
/// during rendering lines to preserve correct aligment.
pub const WHITESPACE_CELL_CHAR: &str = " ";
pub const WHITESPACE_CELL_WIDTH: u32 = 1;
pub const WHITESPACE_CELL_CODE: u32 = 32;

/// Bitmasks for accessing data in `content`.
#[derive(Clone, Copy, Debug)]
pub enum Content {}

impl Content {
    /// bit 1..21    codepoint, max allowed in UTF32 is 0x10FFFF (21 bits taken)
    pub const CODEPOINT_MASK: u32 = 0x1F_FFFF;
    /// bit 22       flag indication whether a cell contains combined content
    pub const IS_COMBINED_MASK: u32 = 0x20_0000; // 1 << 21
    /// bit 1..22    mask to check whether a cell contains any string data
    pub const HAS_CONTENT_MASK: u32 = 0x3F_FFFF;
    /// bit 23..24   wcwidth value of cell, takes 2 bits (ranges from 0..2)
    pub const WIDTH_MASK: u32 = 0xC0_0000; // 3 << 22
    pub const WIDTH_SHIFT: u32 = 22;
}

#[derive(Clone, Copy, Debug)]
pub enum Attributes {}

impl Attributes {
    /// bit 1..8     blue in RGB, color in P256 and P16
    pub const BLUE_MASK: u32 = 0xFF;
    pub const BLUE_SHIFT: u32 = 0;
    pub const PCOLOR_MASK: u32 = 0xFF;
    pub const PCOLOR_SHIFT: u32 = 0;
    /// bit 9..16    green in RGB
    pub const GREEN_MASK: u32 = 0xFF00;
    pub const GREEN_SHIFT: u32 = 8;
    /// bit 17..24   red in RGB
    pub const RED_MASK: u32 = 0xFF_0000;
    pub const RED_SHIFT: u32 = 16;
    /// bit 25..26   color mode: DEFAULT (0) | P16 (1) | P256 (2) | RGB (3)
    pub const CM_MASK: u32 = 0x300_0000;
    pub const CM_DEFAULT: u32 = 0;
    pub const CM_P16: u32 = 0x100_0000;
    pub const CM_P256: u32 = 0x200_0000;
    pub const CM_RGB: u32 = 0x300_0000;
    /// bit 1..24  RGB room
    pub const RGB_MASK: u32 = 0xFF_FFFF;
}

/// bit 27..32
#[derive(Clone, Copy, Debug)]
pub enum FgFlags {}

impl FgFlags {
    pub const INVERSE: u32 = 0x400_0000;
    pub const BOLD: u32 = 0x800_0000;
    pub const UNDERLINE: u32 = 0x1000_0000;
    pub const BLINK: u32 = 0x2000_0000;
    pub const INVISIBLE: u32 = 0x4000_0000;
    pub const STRIKETHROUGH: u32 = 0x8000_0000;
}

/// bit 27..32 (upper 2 unused)
#[derive(Clone, Copy, Debug)]
pub enum BgFlags {}

impl BgFlags {
    pub const ITALIC: u32 = 0x400_0000;
    pub const DIM: u32 = 0x800_0000;
    pub const HAS_EXTENDED: u32 = 0x1000_0000;
    pub const PROTECTED: u32 = 0x2000_0000;
    pub const OVERLINE: u32 = 0x4000_0000;
}

#[derive(Clone, Copy, Debug)]
pub enum ExtFlags {}

impl ExtFlags {
    /// bit 27..29
    pub const UNDERLINE_STYLE: u32 = 0x1C00_0000;
    /// bit 30..32
    ///
    /// An optional variant for the glyph, this can be used for example to offset underlines by a
    /// number of pixels to create a perfect pattern.
    pub const VARIANT_OFFSET: u32 = 0xE000_0000;
}

/// Underline style as stored in `ExtFlags::UNDERLINE_STYLE` (3 bits, so 6 and 7 can occur in raw
/// data); `u32` constants rather than an enum because upstream's accessors return the raw number.
#[derive(Clone, Copy, Debug)]
pub enum UnderlineStyle {}

impl UnderlineStyle {
    pub const NONE: u32 = 0;
    pub const SINGLE: u32 = 1;
    pub const DOUBLE: u32 = 2;
    pub const CURLY: u32 = 3;
    pub const DOTTED: u32 = 4;
    pub const DASHED: u32 = 5;
}
