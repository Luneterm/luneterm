//! Tests for addons/addon-unicode11/src/UnicodeV11.ts @ f447274. The upstream test
//! (`addons/addon-unicode11/test/Unicode11Addon.test.ts`) drives a browser through Playwright;
//! its assertions are ported here as unit tests on `UnicodeService`, plus spot checks read from
//! the `UnicodeV11.ts` tables.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::services::unicode_service::UnicodeService;

const TEN_ROLLING_ON_THE_FLOOR: &str = "🤣🤣🤣🤣🤣🤣🤣🤣🤣🤣";

/// `window.unicode11 = new Unicode11Addon(); window.term.loadAddon(window.unicode11)`.
fn load_addon(us: &mut UnicodeService) {
    us.register(Box::new(UnicodeV11::new()));
}

#[test]
fn wcwidth_v11_emoji_test() {
    let mut us = UnicodeService::new();
    load_addon(&mut us);
    // should have loaded '11'
    assert!(us.versions().contains(&"11"));
    // switch should not throw
    us.set_active_version("11").unwrap();
    assert_eq!(us.active_version(), "11");
    // v6: 10, V11: 20
    assert_eq!(us.get_string_cell_width(TEN_ROLLING_ON_THE_FLOOR), 20);
}

#[test]
fn v6_is_ten_before_activation() {
    let mut us = UnicodeService::new();
    load_addon(&mut us);
    assert_eq!(us.get_string_cell_width(TEN_ROLLING_ON_THE_FLOOR), 10);
}

#[test]
fn version_is_11_and_registered_after_6() {
    assert_eq!(UnicodeV11::new().version(), "11");
    let mut us = UnicodeService::new();
    load_addon(&mut us);
    assert_eq!(us.versions(), vec!["6", "11"]);
}

/// Spot checks, each read from the tables in `UnicodeV11.ts` (not from the Rust code).
#[test]
fn spot_checks_from_the_typescript_tables() {
    let v11 = UnicodeV11::new();
    // BMP_WIDE [0x4E00, 0xA48C] (UnicodeV11.ts:143): CJK U+4E00 is wide.
    assert_eq!(v11.wcwidth(0x4e00), 2);
    // BMP_COMBINING [0x0300, 0x036F] (UnicodeV11.ts:11): U+0301 combining acute.
    assert_eq!(v11.wcwidth(0x0301), 0);
    // HIGH_WIDE [0x1F5FB, 0x1F64F] (UnicodeV11.ts:161): U+1F600 grinning face is wide.
    assert_eq!(v11.wcwidth(0x1f600), 2);
    // BMP_COMBINING [0x200B, 0x200F] (UnicodeV11.ts:66): U+200B zero width space.
    assert_eq!(v11.wcwidth(0x200b), 0);
    // HIGH_WIDE [0x20000, 0x2FFFD] (UnicodeV11.ts:167): U+20000 is wide.
    assert_eq!(v11.wcwidth(0x20000), 2);
    // HIGH_COMBINING [0xE0100, 0xE01EF] (UnicodeV11.ts:124): variation selector supplement.
    assert_eq!(v11.wcwidth(0xe0100), 0);
}

#[test]
fn control_and_ascii_ranges() {
    let v11 = UnicodeV11::new();
    assert_eq!(v11.wcwidth(0), 0);
    assert_eq!(v11.wcwidth(31), 0);
    assert_eq!(v11.wcwidth(32), 1);
    assert_eq!(v11.wcwidth(126), 1);
    assert_eq!(v11.wcwidth(0x7f), 0);
    assert_eq!(v11.wcwidth(0x9f), 0);
    assert_eq!(v11.wcwidth(0xa0), 1);
}

#[test]
fn any_code_point_is_handled_without_panic() {
    let v11 = UnicodeV11::new();
    for cp in [0x10ffff, 0x110000, 0x7fff_ffff, 0x8000_0000, u32::MAX] {
        assert_eq!(v11.wcwidth(cp), 1, "cp {cp:#x}");
    }
    // Sweep the full range around the table boundaries.
    for cp in 0..0x40000u32 {
        assert!(v11.wcwidth(cp) <= 2, "cp {cp:#x}");
    }
}

#[test]
fn emoji_modifier_joins_to_preceding_wide_character() {
    // A zero-width mark after a wide character joins it and keeps its width (the shared
    // `charProperties` logic; the cell width of the pair is 2, not 2 + 0).
    let mut us = UnicodeService::new();
    load_addon(&mut us);
    us.set_active_version("11").unwrap();
    assert_eq!(us.get_string_cell_width("\u{1f600}\u{fe0f}"), 2);
}
