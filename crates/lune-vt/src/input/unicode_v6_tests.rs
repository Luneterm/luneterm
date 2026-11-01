//! Port of xterm.js src/common/input/UnicodeV6.test.ts @ f447274: the new lookup-table
//! implementation must match the old `wcwidth.c`-derived implementation (reproduced here,
//! including its packed 2-bit table) on every BMP code point. Plus spot checks read from
//! `UnicodeV6.ts` and the no-panic range checks.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

// Old implementation, extracted upstream from
// https://www.cl.cam.ac.uk/%7Emgk25/ucs/wcwidth.c
// combining characters
const COMBINING_BMP: [(u32, u32); 129] = [
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

const COMBINING_HIGH: [(u32, u32); 13] = [
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

/// Options of the old implementation: `{nul: 0, control: 0}`.
const NUL: u32 = 0;
const CONTROL: u32 = 0;

fn wcwidth_bmp(ucs: u32) -> u32 {
    // test for 8-bit control characters
    if ucs == 0 {
        return NUL;
    }
    if ucs < 32 || (0x7f..0xa0).contains(&ucs) {
        return CONTROL;
    }
    // binary search in table of non-spacing characters
    if bisearch(ucs, &COMBINING_BMP) {
        return 0;
    }
    // if we arrive here, ucs is not a combining or C0/C1 control character
    if is_wide_bmp(ucs) {
        return 2;
    }
    1
}

fn is_wide_bmp(ucs: u32) -> bool {
    ucs >= 0x1100
        && (ucs <= 0x115f // Hangul Jamo init. consonants
            || ucs == 0x2329
            || ucs == 0x232a
            || ((0x2e80..=0xa4cf).contains(&ucs) && ucs != 0x303f) // CJK..Yi
            || (0xac00..=0xd7a3).contains(&ucs) // Hangul Syllables
            || (0xf900..=0xfaff).contains(&ucs) // CJK Compat Ideographs
            || (0xfe10..=0xfe19).contains(&ucs) // Vertical forms
            || (0xfe30..=0xfe6f).contains(&ucs) // CJK Compat Forms
            || (0xff00..=0xff60).contains(&ucs) // Fullwidth Forms
            || (0xffe0..=0xffe6).contains(&ucs))
}

fn wcwidth_high(ucs: u32) -> u32 {
    if bisearch(ucs, &COMBINING_HIGH) {
        return 0;
    }
    if (0x20000..=0x2fffd).contains(&ucs) || (0x30000..=0x3fffd).contains(&ucs) {
        return 2;
    }
    1
}

/// The old implementation's packed table: 16 two-bit widths per `u32`.
fn init_table() -> Vec<u32> {
    const CODEPOINTS: u32 = 65536; // BMP holds 65536 codepoints
    const BITWIDTH: u32 = 2; // a codepoint can have a width of 0, 1 or 2
    const ITEMSIZE: u32 = 32; // using uint32_t
    const CONTAINERSIZE: u32 = CODEPOINTS * BITWIDTH / ITEMSIZE;
    const CODEPOINTS_PER_ITEM: u32 = ITEMSIZE / BITWIDTH;
    let mut table = Vec::new();
    for i in 0..CONTAINERSIZE {
        let mut num: u32 = 0;
        let mut pos = CODEPOINTS_PER_ITEM;
        while pos > 0 {
            pos -= 1;
            num = (num << 2) | wcwidth_bmp(CODEPOINTS_PER_ITEM * i + pos);
        }
        table.push(num);
    }
    table
}

fn wcwidth_old(table: &[u32], num: u32) -> u32 {
    if num < 32 {
        return CONTROL;
    }
    if num < 127 {
        return 1;
    }
    if num < 65536 {
        return table[(num >> 4) as usize] >> ((num & 15) << 1) & 3;
    }
    // do a full search for high codepoints
    wcwidth_high(num)
}

#[test]
fn wcwidth_should_match_all_values_from_the_old_implementation() {
    // new implementation
    let new = UnicodeV6::new();
    // old implementation
    let table = init_table();
    // test full BMP range old vs new implementation
    for i in 0..65536u32 {
        assert_eq!(
            new.wcwidth(i),
            wcwidth_old(&table, i),
            "mismatch for i: {i}"
        );
    }
}

#[test]
fn wcwidth_high_code_points_match_the_old_implementation() {
    // Not in the upstream test (which stops at the BMP); the high path is the same code in the
    // old implementation, so compare every code point to U+40000 plus values past the Unicode
    // range.
    let new = UnicodeV6::new();
    let table = init_table();
    for i in 65536..0x40000u32 {
        assert_eq!(
            new.wcwidth(i),
            wcwidth_old(&table, i),
            "mismatch for i: {i}"
        );
    }
    for i in [0x10ffff, 0x110000, 0x7fff_ffff, 0x8000_0000, u32::MAX] {
        assert_eq!(
            new.wcwidth(i),
            wcwidth_old(&table, i),
            "mismatch for i: {i}"
        );
        assert_eq!(new.wcwidth(i), 1);
    }
}

#[test]
fn version_is_6() {
    assert_eq!(UnicodeV6::new().version(), "6");
}

/// Spot checks, each read from the tables in `UnicodeV6.ts` (not from the Rust code).
#[test]
fn spot_checks_from_the_typescript_tables() {
    let v6 = UnicodeV6::new();
    // table.fill(2, 0x2e80, 0xa4d0) (UnicodeV6.ts:102): CJK U+4E00 is wide.
    assert_eq!(v6.wcwidth(0x4e00), 2);
    // BMP_COMBINING [0x0300, 0x036F] (UnicodeV6.ts:9): U+0301 combining acute is zero width.
    assert_eq!(v6.wcwidth(0x0301), 0);
    // No wide rule covers U+1F600 and HIGH_COMBINING does not either (UnicodeV6.ts:128 falls
    // through to `return 1`): emoji are narrow in V6.
    assert_eq!(v6.wcwidth(0x1f600), 1);
    // BMP_COMBINING [0x200B, 0x200F] (UnicodeV6.ts:47): U+200B zero width space.
    assert_eq!(v6.wcwidth(0x200b), 0);
    // `(num >= 0x20000 && num <= 0x2fffd)` (UnicodeV6.ts:128): U+20000 is wide.
    assert_eq!(v6.wcwidth(0x20000), 2);
    // HIGH_COMBINING [0xE0100, 0xE01EF] (UnicodeV6.ts:58): variation selector supplement.
    assert_eq!(v6.wcwidth(0xe0100), 0);
    // table[0x303f] = 1, "wrongly in last line" (UnicodeV6.ts:103).
    assert_eq!(v6.wcwidth(0x303f), 1);
}

#[test]
fn char_properties_join_rules() {
    let v6 = UnicodeV6::new();
    // A narrow character: width 1, no join.
    let a = v6.char_properties(u32::from('a'), 0);
    assert_eq!(UnicodeService::extract_width(a), 1);
    assert!(!UnicodeService::extract_should_join(a));
    // A combining mark after a narrow character joins it and takes its width.
    let joined = v6.char_properties(0x0301, a);
    assert_eq!(UnicodeService::extract_width(joined), 1);
    assert!(UnicodeService::extract_should_join(joined));
    // A combining mark after a zero-width character does not join.
    let zero = v6.char_properties(0x0301, 0);
    assert_eq!(UnicodeService::extract_width(zero), 0);
    assert!(!UnicodeService::extract_should_join(zero));
    let not_joined = v6.char_properties(0x0301, zero);
    assert!(!UnicodeService::extract_should_join(not_joined));
}
