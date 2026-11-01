//! Port of xterm.js src/common/buffer/AttributeData.ts @ f447274.
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D7):
//!  - JS `number` bit results are `u32` (the same 32 bits an int32 holds). Flag getters return
//!    the masked bits, truthy when non-zero, exactly as upstream; `is_strikethrough` is
//!    `0x8000_0000` where JS yields a negative number.
//!  - Colour accessors that upstream defaults to `-1` return `i32`.
//!  - [`ExtendedAttrs`] is a `Copy` value. Upstream shares references, but every in-place
//!    `.extended.*` mutation in `src/common` clones first, so a copy is equivalent.
//!  - `IAttributeData` and `IExtendedAttrs` (Types.ts) are replaced: the structs are the types.

use crate::buffer::constants::{Attributes, BgFlags, ExtFlags, FgFlags, UnderlineStyle};
use crate::color::ColorRgb;

/// Colour, flag and extended attributes of a cell (upstream `AttributeData`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttributeData {
    pub fg: u32,
    pub bg: u32,
    pub extended: ExtendedAttrs,
}

impl AttributeData {
    pub fn to_color_rgb(value: u32) -> ColorRgb {
        [
            (value >> Attributes::RED_SHIFT) & 255,
            (value >> Attributes::GREEN_SHIFT) & 255,
            value & 255,
        ]
    }

    pub fn from_color_rgb(value: ColorRgb) -> u32 {
        ((value[0] & 255) << Attributes::RED_SHIFT)
            | ((value[1] & 255) << Attributes::GREEN_SHIFT)
            | (value[2] & 255)
    }

    // flags
    pub fn is_inverse(&self) -> u32 {
        self.fg & FgFlags::INVERSE
    }
    pub fn is_bold(&self) -> u32 {
        self.fg & FgFlags::BOLD
    }
    pub fn is_underline(&self) -> u32 {
        if self.has_extended_attrs() != 0 && self.extended.underline_style() != UnderlineStyle::NONE
        {
            return 1;
        }
        self.fg & FgFlags::UNDERLINE
    }
    pub fn is_blink(&self) -> u32 {
        self.fg & FgFlags::BLINK
    }
    pub fn is_invisible(&self) -> u32 {
        self.fg & FgFlags::INVISIBLE
    }
    pub fn is_italic(&self) -> u32 {
        self.bg & BgFlags::ITALIC
    }
    pub fn is_dim(&self) -> u32 {
        self.bg & BgFlags::DIM
    }
    pub fn is_strikethrough(&self) -> u32 {
        self.fg & FgFlags::STRIKETHROUGH
    }
    pub fn is_protected(&self) -> u32 {
        self.bg & BgFlags::PROTECTED
    }
    pub fn is_overline(&self) -> u32 {
        self.bg & BgFlags::OVERLINE
    }

    // color modes
    pub fn get_fg_color_mode(&self) -> u32 {
        self.fg & Attributes::CM_MASK
    }
    pub fn get_bg_color_mode(&self) -> u32 {
        self.bg & Attributes::CM_MASK
    }
    pub fn is_fg_rgb(&self) -> bool {
        (self.fg & Attributes::CM_MASK) == Attributes::CM_RGB
    }
    pub fn is_bg_rgb(&self) -> bool {
        (self.bg & Attributes::CM_MASK) == Attributes::CM_RGB
    }
    pub fn is_fg_palette(&self) -> bool {
        (self.fg & Attributes::CM_MASK) == Attributes::CM_P16
            || (self.fg & Attributes::CM_MASK) == Attributes::CM_P256
    }
    pub fn is_bg_palette(&self) -> bool {
        (self.bg & Attributes::CM_MASK) == Attributes::CM_P16
            || (self.bg & Attributes::CM_MASK) == Attributes::CM_P256
    }
    pub fn is_fg_default(&self) -> bool {
        (self.fg & Attributes::CM_MASK) == 0
    }
    pub fn is_bg_default(&self) -> bool {
        (self.bg & Attributes::CM_MASK) == 0
    }
    pub fn is_attribute_default(&self) -> bool {
        self.fg == 0 && self.bg == 0
    }

    // colors
    /// The foreground colour index or RGB value; -1 for the default colour.
    pub fn get_fg_color(&self) -> i32 {
        color_of(self.fg)
    }
    /// The background colour index or RGB value; -1 for the default colour.
    pub fn get_bg_color(&self) -> i32 {
        color_of(self.bg)
    }

    // extended attrs
    pub fn has_extended_attrs(&self) -> u32 {
        self.bg & BgFlags::HAS_EXTENDED
    }
    pub fn update_extended(&mut self) {
        if self.extended.is_empty() {
            self.bg &= !BgFlags::HAS_EXTENDED;
        } else {
            self.bg |= BgFlags::HAS_EXTENDED;
        }
    }

    /// Whether the underline colour is stored in the extended attributes. Upstream tests
    /// `(bg & HAS_EXTENDED) && ~underlineColor`; the second operand is non-zero for every value
    /// `underlineColor` can hold (it is masked to 26 bits), kept for fidelity.
    fn has_own_underline_color(&self) -> bool {
        self.bg & BgFlags::HAS_EXTENDED != 0 && !self.extended.underline_color() != 0
    }

    pub fn get_underline_color(&self) -> i32 {
        if self.has_own_underline_color() {
            let underline_color = self.extended.underline_color();
            return match underline_color & Attributes::CM_MASK {
                Attributes::CM_P16 | Attributes::CM_P256 => {
                    (underline_color & Attributes::PCOLOR_MASK) as i32
                }
                Attributes::CM_RGB => (underline_color & Attributes::RGB_MASK) as i32,
                _ => self.get_fg_color(),
            };
        }
        self.get_fg_color()
    }
    pub fn get_underline_color_mode(&self) -> u32 {
        if self.has_own_underline_color() {
            self.extended.underline_color() & Attributes::CM_MASK
        } else {
            self.get_fg_color_mode()
        }
    }
    pub fn is_underline_color_rgb(&self) -> bool {
        if self.has_own_underline_color() {
            (self.extended.underline_color() & Attributes::CM_MASK) == Attributes::CM_RGB
        } else {
            self.is_fg_rgb()
        }
    }
    pub fn is_underline_color_palette(&self) -> bool {
        if self.has_own_underline_color() {
            (self.extended.underline_color() & Attributes::CM_MASK) == Attributes::CM_P16
                || (self.extended.underline_color() & Attributes::CM_MASK) == Attributes::CM_P256
        } else {
            self.is_fg_palette()
        }
    }
    pub fn is_underline_color_default(&self) -> bool {
        if self.has_own_underline_color() {
            (self.extended.underline_color() & Attributes::CM_MASK) == 0
        } else {
            self.is_fg_default()
        }
    }
    pub fn get_underline_style(&self) -> u32 {
        if self.fg & FgFlags::UNDERLINE != 0 {
            if self.bg & BgFlags::HAS_EXTENDED != 0 {
                self.extended.underline_style()
            } else {
                UnderlineStyle::SINGLE
            }
        } else {
            UnderlineStyle::NONE
        }
    }
    pub fn get_underline_variant_offset(&self) -> u32 {
        self.extended.underline_variant_offset()
    }
}

/// Shared body of `getFgColor` / `getBgColor`.
fn color_of(value: u32) -> i32 {
    match value & Attributes::CM_MASK {
        // Both masks are at most 24 bits, so the casts keep the value.
        Attributes::CM_P16 | Attributes::CM_P256 => (value & Attributes::PCOLOR_MASK) as i32,
        Attributes::CM_RGB => (value & Attributes::RGB_MASK) as i32,
        _ => -1, // CM_DEFAULT defaults to -1
    }
}

/// Extended attributes for a cell.
/// Holds information about different underline styles and color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtendedAttrs {
    /// Upstream `_ext`.
    ext: u32,
    /// Upstream `_urlId`.
    url_id: u32,
}

impl ExtendedAttrs {
    pub const fn new(ext: u32, url_id: u32) -> Self {
        Self { ext, url_id }
    }

    /// The `ext` getter: with a url id set, the underline style bits read DASHED.
    pub fn ext(&self) -> u32 {
        if self.url_id != 0 {
            return (self.ext & !ExtFlags::UNDERLINE_STYLE) | (self.underline_style() << 26);
        }
        self.ext
    }
    /// The `ext` setter (stores the raw value).
    pub fn set_ext(&mut self, value: u32) {
        self.ext = value;
    }

    pub fn underline_style(&self) -> u32 {
        // Always return the URL style if it has one
        if self.url_id != 0 {
            return UnderlineStyle::DASHED;
        }
        (self.ext & ExtFlags::UNDERLINE_STYLE) >> 26
    }
    pub fn set_underline_style(&mut self, value: u32) {
        self.ext &= !ExtFlags::UNDERLINE_STYLE;
        self.ext |= (value << 26) & ExtFlags::UNDERLINE_STYLE;
    }

    pub fn underline_color(&self) -> u32 {
        self.ext & (Attributes::CM_MASK | Attributes::RGB_MASK)
    }
    pub fn set_underline_color(&mut self, value: u32) {
        self.ext &= !(Attributes::CM_MASK | Attributes::RGB_MASK);
        self.ext |= value & (Attributes::CM_MASK | Attributes::RGB_MASK);
    }

    pub fn url_id(&self) -> u32 {
        self.url_id
    }
    pub fn set_url_id(&mut self, value: u32) {
        self.url_id = value;
    }

    /// The offset 0..=7. Upstream reads the top three bits with a signed shift and flips the
    /// sign extension back off (`val ^ 0xFFFFFFF8`), which is the unsigned value of those bits.
    pub fn underline_variant_offset(&self) -> u32 {
        (self.ext & ExtFlags::VARIANT_OFFSET) >> 29
    }
    pub fn set_underline_variant_offset(&mut self, value: u32) {
        self.ext &= !ExtFlags::VARIANT_OFFSET;
        self.ext |= (value << 29) & ExtFlags::VARIANT_OFFSET;
    }

    /// Convenient method to indicate whether the object holds no additional information,
    /// that needs to be persistant in the buffer.
    pub fn is_empty(&self) -> bool {
        self.underline_style() == UnderlineStyle::NONE && self.url_id == 0
    }
}

#[cfg(test)]
#[path = "attribute_data_tests.rs"]
mod tests;
