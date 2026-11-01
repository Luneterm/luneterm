//! Port of xterm.js src/common/input/XParseColor.test.ts @ f447274, plus Rust-only no-panic
//! cases (non-ASCII input must return `None`, never slice inside a character).

#![allow(clippy::unwrap_used)]

use super::*;

fn p(s: &str) -> Option<[u8; 3]> {
    parse_color(s)
}

mod parse_color_tests {
    use super::*;

    #[test]
    fn rgb_scheme_in_4_8_12_16_bit() {
        // 4 bit
        assert_eq!(p("rgb:0/0/0"), Some([0, 0, 0]));
        assert_eq!(p("rgb:f/f/f"), Some([255, 255, 255]));
        assert_eq!(p("rgb:1/2/3"), Some([17, 34, 51]));
        // 8 bit
        assert_eq!(p("rgb:00/00/00"), Some([0, 0, 0]));
        assert_eq!(p("rgb:ff/ff/ff"), Some([255, 255, 255]));
        assert_eq!(p("rgb:11/22/33"), Some([17, 34, 51]));
        // 12 bit
        assert_eq!(p("rgb:000/000/000"), Some([0, 0, 0]));
        assert_eq!(p("rgb:fff/fff/fff"), Some([255, 255, 255]));
        assert_eq!(p("rgb:111/222/333"), Some([17, 34, 51]));
        // 16 bit
        assert_eq!(p("rgb:0000/0000/0000"), Some([0, 0, 0]));
        assert_eq!(p("rgb:ffff/ffff/ffff"), Some([255, 255, 255]));
        assert_eq!(p("rgb:1111/2222/3333"), Some([17, 34, 51]));
    }

    #[test]
    fn hash_rgb_scheme_in_4_8_12_16_bit() {
        // 4 bit
        assert_eq!(p("#000"), Some([0, 0, 0]));
        assert_eq!(p("#fff"), Some([240, 240, 240]));
        assert_eq!(p("#123"), Some([16, 32, 48]));
        // 8 bit
        assert_eq!(p("#000000"), Some([0, 0, 0]));
        assert_eq!(p("#ffffff"), Some([255, 255, 255]));
        assert_eq!(p("#112233"), Some([17, 34, 51]));
        // 12 bit
        assert_eq!(p("#000000000"), Some([0, 0, 0]));
        assert_eq!(p("#fffffffff"), Some([255, 255, 255]));
        assert_eq!(p("#111222333"), Some([17, 34, 51]));
        // 16 bit
        assert_eq!(p("#000000000000"), Some([0, 0, 0]));
        assert_eq!(p("#ffffffffffff"), Some([255, 255, 255]));
        assert_eq!(p("#111122223333"), Some([17, 34, 51]));
    }

    #[test]
    fn supports_upper_case() {
        assert_eq!(p("RGB:0/A/F"), Some([0, 170, 255]));
        assert_eq!(p("#FFF"), Some([240, 240, 240]));
    }

    #[test]
    fn does_not_parse_illegal_combinations() {
        // shifting bit width
        assert_eq!(p("rgb:0/11/222"), None);
        // unsupported scheme
        assert_eq!(p("rgbi:00/11/22"), None);
        // broken # specifier
        assert_eq!(p("#aabbbcc"), None);
        // out of range
        assert_eq!(p("#aabbgg"), None);
        assert_eq!(p("rgb:aa/bb/gg"), None);
    }

    /// Rust-only: the regexes anchor both ends; empty and partial specs are `None`.
    #[test]
    fn malformed_shapes_are_none() {
        for s in [
            "",
            "rgb:",
            "rgb:1",
            "rgb:1/2",
            "rgb:1/2/3/4",
            "rgb:/1/2",
            "rgb:1//2",
            "rgb:12345/1/2",
            "rgb:1/2/3 ",
            " rgb:1/2/3",
            "rgb:1/2/3\n",
            "#",
            "#12",
            "#1234",
            "#1234567890123",
            "#123\n",
            "red",
            "rgb",
            "rgb:0x1/0x2/0x3",
            "rgb:-1/0/0",
            "rgb:+1/0/0",
        ] {
            assert_eq!(p(s), None, "{s:?}");
        }
    }

    /// Rust-only: non-ASCII input returns `None` and never slices inside a character. The `#`
    /// cases have byte lengths 3/6/9/12 on purpose.
    #[test]
    fn non_ascii_input_is_none_without_panicking() {
        for s in [
            "rgb:€/0/0",
            "rgb:0/€/0",
            "rgb:0/0/€",
            "rgb:€€€/000/000",
            "rgb:\u{e9}/1/2",
            "#\u{e9}",  // 2 bytes + '#'
            "#\u{e9}a", // 3 bytes of digits
            "#€",       // 3 bytes
            "#€€",      // 6 bytes
            "#€€€",     // 9 bytes
            "#€€€€",    // 12 bytes
            "#\u{1F600}\u{1F600}\u{1F600}",
            "#12\u{e9}1",
            "\u{212A}", // Kelvin sign lower-cases to ASCII 'k' in JS
            "RGB\u{FF1A}1/2/3",
            "\u{1F600}",
        ] {
            assert_eq!(p(s), None, "{s:?}");
        }
    }
}

mod to_rgb_string_tests {
    use super::*;

    fn rt(spec: &str, bits: u32) -> String {
        to_rgb_string(parse_color(spec).unwrap(), bits)
    }

    #[test]
    fn rgb_scheme_in_4_8_12_16_bit() {
        // 4 bit
        assert_eq!(rt("rgb:0/0/0", 4), "rgb:0/0/0");
        assert_eq!(rt("rgb:f/f/f", 4), "rgb:f/f/f");
        assert_eq!(rt("rgb:1/2/3", 4), "rgb:1/2/3");
        // 8 bit
        assert_eq!(rt("rgb:00/00/00", 8), "rgb:00/00/00");
        assert_eq!(rt("rgb:ff/ff/ff", 8), "rgb:ff/ff/ff");
        assert_eq!(rt("rgb:11/22/33", 8), "rgb:11/22/33");
        // 12 bit
        assert_eq!(rt("rgb:000/000/000", 12), "rgb:000/000/000");
        assert_eq!(rt("rgb:fff/fff/fff", 12), "rgb:fff/fff/fff");
        assert_eq!(rt("rgb:111/222/333", 12), "rgb:111/222/333");
        // 16 bit
        assert_eq!(rt("rgb:0000/0000/0000", 16), "rgb:0000/0000/0000");
        assert_eq!(rt("rgb:ffff/ffff/ffff", 16), "rgb:ffff/ffff/ffff");
        assert_eq!(rt("rgb:1111/2222/3333", 16), "rgb:1111/2222/3333");
    }

    #[test]
    fn defaults_to_16_bit_output() {
        let d = DEFAULT_RGB_BITS;
        assert_eq!(rt("rgb:1/2/3", d), "rgb:1111/2222/3333");
        assert_eq!(rt("rgb:11/22/33", d), "rgb:1111/2222/3333");
        assert_eq!(rt("rgb:111/222/333", d), "rgb:1111/2222/3333");
        assert_eq!(rt("rgb:123/123/123", d), "rgb:1212/1212/1212");
    }

    #[test]
    fn reduces_colors_to_8_bit_resolution() {
        assert_eq!(rt("rgb:123/123/123", 12), "rgb:121/121/121");
        assert_eq!(rt("rgb:1234/1234/1234", 16), "rgb:1212/1212/1212");
    }

    /// Rust-only: the 4 bit output is the first digit of the unpadded hex (upstream `s[0]`),
    /// so a value below 0x10 keeps its single digit; unknown widths fall to 16 bit.
    #[test]
    fn quirks_and_unknown_widths() {
        assert_eq!(to_rgb_string([1, 0xa, 0xff], 4), "rgb:1/a/f");
        assert_eq!(to_rgb_string([1, 0xa, 0xff], 8), "rgb:01/0a/ff");
        assert_eq!(to_rgb_string([1, 0xa, 0xff], 12), "rgb:010/0a0/fff");
        assert_eq!(to_rgb_string([1, 0xa, 0xff], 0), "rgb:0101/0a0a/ffff");
        assert_eq!(to_rgb_string([1, 0xa, 0xff], 7), "rgb:0101/0a0a/ffff");
    }
}
