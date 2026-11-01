//! Port of xterm.js src/common/input/UnicodeV6.ts @ f447274 (tests: UnicodeV6.test.ts).
//!
//! The default width provider: Unicode 6 East Asian widths and combining marks. Widths below
//! U+10000 come from a 64 KiB lookup table built once per process (upstream: a module-level
//! `Uint8Array` filled by the first constructor); higher code points use a binary search.
//!
//! Rust notes (see `docs/porting-xterm.md`):
//!  - The table is a `OnceLock` shared by every [`UnicodeV6`]. Filling and reading use
//!    `get`/`get_mut`; nothing here indexes.
//!  - `wcwidth` takes a `u32`, so it covers every value upstream's `number` can hold that a
//!    code point can be, including values above U+10FFFF (width 1, as upstream).

use std::sync::OnceLock;

use crate::services::unicode_service::{UnicodeService, UnicodeVersionProvider};

const BMP_COMBINING: [(u32, u32); 129] = [
    (0x0300, 0x036F),
    (0x0483, 0x0486),
    (0x0488, 0x0489),
    (0x0591, 0x05BD),
    (0x05BF, 0x05BF),
    (0x05C1, 0x05C2),
    (0x05C4, 0x05C5),
    (0x05C7, 0x05C7),
    (0x0600, 0x0603),
    (0x0610, 0x0615),
    (0x064B, 0x065E),
    (0x0670, 0x0670),
    (0x06D6, 0x06E4),
    (0x06E7, 0x06E8),
    (0x06EA, 0x06ED),
    (0x070F, 0x070F),
    (0x0711, 0x0711),
    (0x0730, 0x074A),
    (0x07A6, 0x07B0),
    (0x07EB, 0x07F3),
    (0x0901, 0x0902),
    (0x093C, 0x093C),
    (0x0941, 0x0948),
    (0x094D, 0x094D),
    (0x0951, 0x0954),
    (0x0962, 0x0963),
    (0x0981, 0x0981),
    (0x09BC, 0x09BC),
    (0x09C1, 0x09C4),
    (0x09CD, 0x09CD),
    (0x09E2, 0x09E3),
    (0x0A01, 0x0A02),
    (0x0A3C, 0x0A3C),
    (0x0A41, 0x0A42),
    (0x0A47, 0x0A48),
    (0x0A4B, 0x0A4D),
    (0x0A70, 0x0A71),
    (0x0A81, 0x0A82),
    (0x0ABC, 0x0ABC),
    (0x0AC1, 0x0AC5),
    (0x0AC7, 0x0AC8),
    (0x0ACD, 0x0ACD),
    (0x0AE2, 0x0AE3),
    (0x0B01, 0x0B01),
    (0x0B3C, 0x0B3C),
    (0x0B3F, 0x0B3F),
    (0x0B41, 0x0B43),
    (0x0B4D, 0x0B4D),
    (0x0B56, 0x0B56),
    (0x0B82, 0x0B82),
    (0x0BC0, 0x0BC0),
    (0x0BCD, 0x0BCD),
    (0x0C3E, 0x0C40),
    (0x0C46, 0x0C48),
    (0x0C4A, 0x0C4D),
    (0x0C55, 0x0C56),
    (0x0CBC, 0x0CBC),
    (0x0CBF, 0x0CBF),
    (0x0CC6, 0x0CC6),
    (0x0CCC, 0x0CCD),
    (0x0CE2, 0x0CE3),
    (0x0D41, 0x0D43),
    (0x0D4D, 0x0D4D),
    (0x0DCA, 0x0DCA),
    (0x0DD2, 0x0DD4),
    (0x0DD6, 0x0DD6),
    (0x0E31, 0x0E31),
    (0x0E34, 0x0E3A),
    (0x0E47, 0x0E4E),
    (0x0EB1, 0x0EB1),
    (0x0EB4, 0x0EB9),
    (0x0EBB, 0x0EBC),
    (0x0EC8, 0x0ECD),
    (0x0F18, 0x0F19),
    (0x0F35, 0x0F35),
    (0x0F37, 0x0F37),
    (0x0F39, 0x0F39),
    (0x0F71, 0x0F7E),
    (0x0F80, 0x0F84),
    (0x0F86, 0x0F87),
    (0x0F90, 0x0F97),
    (0x0F99, 0x0FBC),
    (0x0FC6, 0x0FC6),
    (0x102D, 0x1030),
    (0x1032, 0x1032),
    (0x1036, 0x1037),
    (0x1039, 0x1039),
    (0x1058, 0x1059),
    (0x1160, 0x11FF),
    (0x135F, 0x135F),
    (0x1712, 0x1714),
    (0x1732, 0x1734),
    (0x1752, 0x1753),
    (0x1772, 0x1773),
    (0x17B4, 0x17B5),
    (0x17B7, 0x17BD),
    (0x17C6, 0x17C6),
    (0x17C9, 0x17D3),
    (0x17DD, 0x17DD),
    (0x180B, 0x180D),
    (0x18A9, 0x18A9),
    (0x1920, 0x1922),
    (0x1927, 0x1928),
    (0x1932, 0x1932),
    (0x1939, 0x193B),
    (0x1A17, 0x1A18),
    (0x1B00, 0x1B03),
    (0x1B34, 0x1B34),
    (0x1B36, 0x1B3A),
    (0x1B3C, 0x1B3C),
    (0x1B42, 0x1B42),
    (0x1B6B, 0x1B73),
    (0x1DC0, 0x1DCA),
    (0x1DFE, 0x1DFF),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2063),
    (0x206A, 0x206F),
    (0x20D0, 0x20EF),
    (0x302A, 0x302F),
    (0x3099, 0x309A),
    (0xA806, 0xA806),
    (0xA80B, 0xA80B),
    (0xA825, 0xA826),
    (0xFB1E, 0xFB1E),
    (0xFE00, 0xFE0F),
    (0xFE20, 0xFE23),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
];

const HIGH_COMBINING: [(u32, u32); 13] = [
    (0x10A01, 0x10A03),
    (0x10A05, 0x10A06),
    (0x10A0C, 0x10A0F),
    (0x10A38, 0x10A3A),
    (0x10A3F, 0x10A3F),
    (0x1D167, 0x1D169),
    (0x1D173, 0x1D182),
    (0x1D185, 0x1D18B),
    (0x1D1AA, 0x1D1AD),
    (0x1D242, 0x1D244),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
    (0xE0100, 0xE01EF),
];

/// BMP lookup table: one width (0, 1 or 2) per code point below U+10000.
static TABLE: OnceLock<Box<[u8]>> = OnceLock::new();

/// `table.fill(value, start, end)`.
fn fill(table: &mut [u8], value: u8, start: usize, end: usize) {
    if let Some(range) = table.get_mut(start..end) {
        range.fill(value);
    }
}

/// `table[index] = value`.
fn set(table: &mut [u8], index: usize, value: u8) {
    if let Some(cell) = table.get_mut(index) {
        *cell = value;
    }
}

fn build_table() -> Box<[u8]> {
    let mut table = vec![1u8; 65536];
    set(&mut table, 0, 0);
    // control chars
    fill(&mut table, 0, 1, 32);
    fill(&mut table, 0, 0x7f, 0xa0);

    // apply wide char rules first
    // wide chars
    fill(&mut table, 2, 0x1100, 0x1160);
    set(&mut table, 0x2329, 2);
    set(&mut table, 0x232a, 2);
    fill(&mut table, 2, 0x2e80, 0xa4d0);
    set(&mut table, 0x303f, 1); // wrongly in last line

    fill(&mut table, 2, 0xac00, 0xd7a4);
    fill(&mut table, 2, 0xf900, 0xfb00);
    fill(&mut table, 2, 0xfe10, 0xfe1a);
    fill(&mut table, 2, 0xfe30, 0xfe70);
    fill(&mut table, 2, 0xff00, 0xff61);
    fill(&mut table, 2, 0xffe0, 0xffe7);

    // apply combining last to ensure we overwrite
    // wrongly wide set chars:
    //    the original algo evals combining first and falls
    //    through to wide check so we simply do here the opposite
    // combining 0
    for (start, end) in BMP_COMBINING {
        fill(&mut table, 0, start as usize, end as usize + 1);
    }
    table.into_boxed_slice()
}

/// Binary search for `ucs` in sorted, disjoint `[first, last]` ranges.
pub(crate) fn bisearch(ucs: u32, data: &[(u32, u32)]) -> bool {
    let (Some(first), Some(last)) = (data.first(), data.last()) else {
        return false;
    };
    if ucs < first.0 || ucs > last.1 {
        return false;
    }
    let mut min: i64 = 0;
    let mut max: i64 = data.len() as i64 - 1;
    while max >= min {
        let mid = (min + max) >> 1;
        let Some(range) = usize::try_from(mid).ok().and_then(|i| data.get(i)) else {
            return false;
        };
        if ucs > range.1 {
            min = mid + 1;
        } else if ucs < range.0 {
            max = mid - 1;
        } else {
            return true;
        }
    }
    false
}

/// Unicode 6 widths (`UnicodeV6`), registered under version `"6"`.
#[derive(Debug, Clone, Copy)]
pub struct UnicodeV6 {
    _private: (),
}

impl Default for UnicodeV6 {
    fn default() -> Self {
        Self::new()
    }
}

impl UnicodeV6 {
    /// The version name this provider registers under.
    pub const VERSION: &'static str = "6";

    /// A provider; builds the shared lookup table on first use.
    pub fn new() -> Self {
        TABLE.get_or_init(build_table);
        Self { _private: () }
    }

    fn table_width(num: u32) -> u32 {
        TABLE
            .get_or_init(build_table)
            .get(num as usize)
            .copied()
            .map_or(1, u32::from)
    }
}

impl UnicodeVersionProvider for UnicodeV6 {
    fn version(&self) -> &str {
        Self::VERSION
    }

    fn wcwidth(&self, num: u32) -> u32 {
        if num < 32 {
            return 0;
        }
        if num < 127 {
            return 1;
        }
        if num < 65536 {
            return Self::table_width(num);
        }
        if bisearch(num, &HIGH_COMBINING) {
            return 0;
        }
        if (0x20000..=0x2fffd).contains(&num) || (0x30000..=0x3fffd).contains(&num) {
            return 2;
        }
        1
    }

    fn char_properties(&self, codepoint: u32, preceding: u32) -> u32 {
        let mut width = self.wcwidth(codepoint);
        let mut should_join = width == 0 && preceding != 0;
        if should_join {
            let old_width = UnicodeService::extract_width(preceding);
            if old_width == 0 {
                should_join = false;
            } else if old_width > width {
                width = old_width;
            }
        }
        UnicodeService::create_property_value(0, width, should_join)
    }
}

#[cfg(test)]
#[path = "unicode_v6_tests.rs"]
mod tests;
