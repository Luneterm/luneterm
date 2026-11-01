//! Port of xterm.js src/common/services/UnicodeService.test.ts @ f447274, plus tests for the
//! Rust-side contract (`Object.keys` ordering of versions, error text, property-value layout,
//! upstream's stale-active-provider quirk).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

struct DummyProvider;

impl UnicodeVersionProvider for DummyProvider {
    fn version(&self) -> &str {
        "123"
    }
    fn wcwidth(&self, _n: u32) -> u32 {
        2
    }
    fn char_properties(&self, codepoint: u32, _preceding: u32) -> u32 {
        UnicodeService::create_property_value(0, self.wcwidth(codepoint), false)
    }
}

/// A provider with a configurable name and constant width.
struct Named(&'static str, u32);

impl UnicodeVersionProvider for Named {
    fn version(&self) -> &str {
        self.0
    }
    fn wcwidth(&self, _n: u32) -> u32 {
        self.1
    }
    fn char_properties(&self, codepoint: u32, _preceding: u32) -> u32 {
        UnicodeService::create_property_value(0, self.wcwidth(codepoint), false)
    }
}

mod unicode_provider {
    use super::*;

    #[test]
    fn default_to_v6() {
        let mut us = UnicodeService::new();
        assert_eq!(us.active_version(), "6");
        assert_eq!(us.versions(), vec!["6"]);
        // assert.doesNotThrow(() => { us.activeVersion = '6'; })
        assert!(us.set_active_version("6").is_ok());
        assert_eq!(us.get_string_cell_width("hello"), 5);
    }

    #[test]
    fn activate_should_throw_for_unknown_version() {
        let mut us = UnicodeService::new();
        let err = us.set_active_version("55").unwrap_err();
        assert_eq!(err.to_string(), "unknown Unicode version \"55\"");
        // The failed switch leaves the active version alone.
        assert_eq!(us.active_version(), "6");
    }

    #[test]
    fn should_notify_about_version_change() {
        let mut us = UnicodeService::new();
        let mut notes: Vec<String> = Vec::new();
        us.register(Box::new(DummyProvider));
        // `onChange` is the return value of the setter.
        notes.push(us.set_active_version("123").unwrap().version);
        assert_eq!(notes, vec!["123".to_owned()]);
    }

    #[test]
    fn correctly_changes_provider_impl() {
        let mut us = UnicodeService::new();
        assert_eq!(us.get_string_cell_width("hello"), 5);
        us.register(Box::new(DummyProvider));
        us.set_active_version("123").unwrap();
        assert_eq!(us.get_string_cell_width("hello"), 2 * 5);
    }

    #[test]
    fn wcwidth_v6_emoji_test() {
        let us = UnicodeService::new();
        let width_v6 = us.get_string_cell_width("🤣🤣🤣🤣🤣🤣🤣🤣🤣🤣");
        assert_eq!(width_v6, 10);
    }
}

#[test]
fn register_alone_does_not_activate() {
    let mut us = UnicodeService::new();
    us.register(Box::new(DummyProvider));
    assert_eq!(us.active_version(), "6");
    assert_eq!(us.get_string_cell_width("hello"), 5);
}

#[test]
fn versions_follow_object_keys_order() {
    // Integer-like keys ascend numerically ("6" < "11" < "123"), the rest keep registration
    // order and come after them; "06" and "-1" are not array indices.
    let mut us = UnicodeService::new();
    us.register(Box::new(Named("zeta", 1)));
    us.register(Box::new(Named("123", 1)));
    us.register(Box::new(Named("11", 1)));
    us.register(Box::new(Named("06", 1)));
    us.register(Box::new(Named("alpha", 1)));
    us.register(Box::new(Named("-1", 1)));
    assert_eq!(
        us.versions(),
        vec!["6", "11", "123", "zeta", "06", "alpha", "-1"]
    );
}

#[test]
fn registering_an_existing_version_replaces_it_in_place() {
    let mut us = UnicodeService::new();
    us.register(Box::new(Named("a", 1)));
    us.register(Box::new(Named("b", 1)));
    us.register(Box::new(Named("a", 2)));
    assert_eq!(us.versions(), vec!["6", "a", "b"]);
    us.set_active_version("a").unwrap();
    assert_eq!(us.wcwidth(0x41), 2);
}

#[test]
fn registering_over_the_active_version_keeps_the_old_provider_until_reactivated() {
    // Upstream's `_activeProvider` is only reassigned by the `activeVersion` setter.
    let mut us = UnicodeService::new();
    us.register(Box::new(Named("x", 1)));
    us.set_active_version("x").unwrap();
    us.register(Box::new(Named("x", 2)));
    assert_eq!(us.wcwidth(0x41), 1);
    us.set_active_version("x").unwrap();
    assert_eq!(us.wcwidth(0x41), 2);
}

#[test]
fn property_value_layout() {
    // (state & 0xffffff) << 3 | (width & 3) << 1 | join
    assert_eq!(UnicodeService::create_property_value(0, 0, false), 0);
    assert_eq!(UnicodeService::create_property_value(0, 1, false), 0b10);
    assert_eq!(UnicodeService::create_property_value(0, 2, true), 0b101);
    assert_eq!(UnicodeService::create_property_value(0, 3, true), 0b111);
    // width is masked to two bits, state to 24 bits
    assert_eq!(UnicodeService::create_property_value(0, 4, false), 0);
    assert_eq!(
        UnicodeService::create_property_value(0x1ff_ffff, 0, false),
        0xff_ffff << 3
    );
    let v = UnicodeService::create_property_value(5, 2, true);
    assert_eq!(UnicodeService::extract_char_kind(v), 5);
    assert_eq!(UnicodeService::extract_width(v), 2);
    assert!(UnicodeService::extract_should_join(v));
    assert!(!UnicodeService::extract_should_join(
        UnicodeService::create_property_value(5, 2, false)
    ));
}

#[test]
fn string_cell_width_joins_zero_width_marks() {
    let us = UnicodeService::new();
    // 'e' + combining acute = one cell; the mark joins and adds nothing.
    assert_eq!(us.get_string_cell_width("e\u{301}"), 1);
    // CJK is two cells; a following combining mark joins it and adds nothing.
    assert_eq!(us.get_string_cell_width("\u{4e00}\u{301}"), 2);
    // A mark with nothing before it is zero wide, not joined.
    assert_eq!(us.get_string_cell_width("\u{301}"), 0);
    assert_eq!(us.get_string_cell_width(""), 0);
    // Astral characters are read as one code point (surrogate pair in upstream).
    assert_eq!(us.get_string_cell_width("\u{20000}"), 2);
}

#[test]
fn wcwidth_delegates_to_the_active_provider_for_any_code_point() {
    let us = UnicodeService::new();
    assert_eq!(us.wcwidth(u32::MAX), 1);
    assert_eq!(us.wcwidth(0x4e00), 2);
}

#[test]
fn a_provider_that_joins_narrower_cannot_make_the_width_panic() {
    // A custom provider can say "join" with a width below the preceding one; upstream's sum can
    // then go negative. The result clamps at 0.
    struct Odd;
    impl UnicodeVersionProvider for Odd {
        fn version(&self) -> &str {
            "odd"
        }
        fn wcwidth(&self, _n: u32) -> u32 {
            0
        }
        fn char_properties(&self, _cp: u32, preceding: u32) -> u32 {
            if preceding == 0 {
                UnicodeService::create_property_value(0, 1, false)
            } else {
                UnicodeService::create_property_value(0, 0, true)
            }
        }
    }
    let mut us = UnicodeService::new();
    us.register(Box::new(Odd));
    us.set_active_version("odd").unwrap();
    // 'a' = 1, then a joiner of width 0: 0 - 1 = -1 → total 0.
    assert_eq!(us.get_string_cell_width("aa"), 0);
}
