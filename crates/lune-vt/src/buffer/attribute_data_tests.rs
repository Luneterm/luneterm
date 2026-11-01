//! Port of the `AttributeData` describe of xterm.js src/common/buffer/BufferLine.test.ts
//! (`:27-136`) @ f447274, plus Rust-only checks of the bit-level behaviour the port must keep.

use super::*;
use crate::buffer::constants::{Attributes, BgFlags, FgFlags, UnderlineStyle};

mod attribute_data {
    use super::*;

    mod extended_attributes {
        use super::*;

        #[test]
        fn has_extended_attrs() {
            let mut attrs = AttributeData::default();
            assert!(attrs.has_extended_attrs() == 0);
            attrs.bg |= BgFlags::HAS_EXTENDED;
            assert!(attrs.has_extended_attrs() != 0);
        }

        #[test]
        fn get_underline_color_p256() {
            let mut attrs = AttributeData::default();
            // set a P256 color
            attrs.extended.set_underline_color(Attributes::CM_P256 | 45);

            // should use FG color if BgFlags.HAS_EXTENDED is not set
            assert_eq!(attrs.get_underline_color(), -1);

            // should use underlineColor if BgFlags.HAS_EXTENDED is set and underlineColor holds a value
            attrs.bg |= BgFlags::HAS_EXTENDED;
            assert_eq!(attrs.get_underline_color(), 45);

            // should use FG color if underlineColor holds no value
            attrs.extended.set_underline_color(0);
            attrs.fg |= Attributes::CM_P256 | 123;
            assert_eq!(attrs.get_underline_color(), 123);
        }

        #[test]
        fn get_underline_color_rgb() {
            let mut attrs = AttributeData::default();
            // set an RGB color
            attrs
                .extended
                .set_underline_color(Attributes::CM_RGB | (1 << 16) | (2 << 8) | 3);

            // should use FG color if BgFlags.HAS_EXTENDED is not set
            assert_eq!(attrs.get_underline_color(), -1);

            // should use underlineColor if BgFlags.HAS_EXTENDED is set and underlineColor holds a value
            attrs.bg |= BgFlags::HAS_EXTENDED;
            assert_eq!(attrs.get_underline_color(), (1 << 16) | (2 << 8) | 3);

            // should use FG color if underlineColor holds no value
            attrs.extended.set_underline_color(0);
            attrs.fg |= Attributes::CM_P256 | 123;
            assert_eq!(attrs.get_underline_color(), 123);
        }

        #[test]
        fn get_underline_color_mode_is_underline_color_rgb_palette_default() {
            let mut attrs = AttributeData::default();
            let modes = [
                Attributes::CM_DEFAULT,
                Attributes::CM_P16,
                Attributes::CM_P256,
                Attributes::CM_RGB,
            ];

            // should always return color mode of fg
            for mode in modes {
                attrs.extended.set_underline_color(mode);
                assert_eq!(attrs.get_underline_color_mode(), attrs.get_fg_color_mode());
                assert!(attrs.is_underline_color_default());
            }
            attrs.fg = Attributes::CM_RGB;
            for mode in modes {
                attrs.extended.set_underline_color(mode);
                assert_eq!(attrs.get_underline_color_mode(), attrs.get_fg_color_mode());
                assert!(!attrs.is_underline_color_default());
                assert!(attrs.is_underline_color_rgb());
            }

            // should return own mode
            attrs.bg |= BgFlags::HAS_EXTENDED;
            attrs.extended.set_underline_color(Attributes::CM_DEFAULT);
            assert_eq!(attrs.get_underline_color_mode(), Attributes::CM_DEFAULT);
            attrs.extended.set_underline_color(Attributes::CM_P16);
            assert_eq!(attrs.get_underline_color_mode(), Attributes::CM_P16);
            assert!(attrs.is_underline_color_palette());
            attrs.extended.set_underline_color(Attributes::CM_P256);
            assert_eq!(attrs.get_underline_color_mode(), Attributes::CM_P256);
            assert!(attrs.is_underline_color_palette());
            attrs.extended.set_underline_color(Attributes::CM_RGB);
            assert_eq!(attrs.get_underline_color_mode(), Attributes::CM_RGB);
            assert!(attrs.is_underline_color_rgb());
        }

        #[test]
        fn get_underline_style() {
            let mut attrs = AttributeData::default();

            // defaults to no underline style
            assert_eq!(attrs.get_underline_style(), UnderlineStyle::NONE);

            // should return NONE if UNDERLINE is not set
            attrs.extended.set_underline_style(UnderlineStyle::CURLY);
            assert_eq!(attrs.get_underline_style(), UnderlineStyle::NONE);

            // should return SINGLE style if UNDERLINE is set and HAS_EXTENDED is false
            attrs.fg |= FgFlags::UNDERLINE;
            assert_eq!(attrs.get_underline_style(), UnderlineStyle::SINGLE);

            // should return correct style if both is set
            attrs.bg |= BgFlags::HAS_EXTENDED;
            assert_eq!(attrs.get_underline_style(), UnderlineStyle::CURLY);

            // should return NONE if UNDERLINE is not set, but HAS_EXTENDED is true
            attrs.fg &= !FgFlags::UNDERLINE;
            assert_eq!(attrs.get_underline_style(), UnderlineStyle::NONE);
        }

        #[test]
        fn get_underline_variant_offset() {
            let mut attrs = AttributeData::default();

            // defaults to no offset
            assert_eq!(attrs.get_underline_variant_offset(), 0);

            // should return 0 - 7
            for i in 0..8 {
                attrs.extended.set_underline_variant_offset(i);
                assert_eq!(attrs.get_underline_variant_offset(), i);
            }
        }
    }
}

// Rust-only: behaviour of the pieces the upstream tests do not reach.
mod bit_level {
    use super::*;

    #[test]
    fn to_and_from_color_rgb() {
        assert_eq!(AttributeData::to_color_rgb(0x0012_3456), [0x12, 0x34, 0x56]);
        // Only the low 24 bits are read, as upstream's `& 255` masks.
        assert_eq!(AttributeData::to_color_rgb(0xFF12_3456), [0x12, 0x34, 0x56]);
        assert_eq!(
            AttributeData::from_color_rgb([0x12, 0x34, 0x56]),
            0x0012_3456
        );
        // Out-of-range channels are masked, not carried into the next one.
        assert_eq!(
            AttributeData::from_color_rgb([0x1FF, 0x100, 0x1AB]),
            0x00FF_00AB
        );
    }

    #[test]
    fn flag_getters_return_the_masked_bits() {
        let attrs = AttributeData {
            fg: FgFlags::INVERSE | FgFlags::BOLD | FgFlags::BLINK | FgFlags::STRIKETHROUGH,
            bg: BgFlags::ITALIC | BgFlags::PROTECTED,
            extended: ExtendedAttrs::default(),
        };
        assert_eq!(attrs.is_inverse(), FgFlags::INVERSE);
        assert_eq!(attrs.is_bold(), FgFlags::BOLD);
        assert_eq!(attrs.is_blink(), FgFlags::BLINK);
        assert_eq!(attrs.is_invisible(), 0);
        // The top bit: upstream yields a negative int32, truthy; here the same 32 bits.
        assert_eq!(attrs.is_strikethrough(), 0x8000_0000);
        assert_eq!(attrs.is_italic(), BgFlags::ITALIC);
        assert_eq!(attrs.is_dim(), 0);
        assert_eq!(attrs.is_protected(), BgFlags::PROTECTED);
        assert_eq!(attrs.is_overline(), 0);
        assert_eq!(attrs.is_underline(), 0);
    }

    #[test]
    fn is_underline_uses_the_extended_style_when_present() {
        let mut attrs = AttributeData::default();
        attrs.extended.set_underline_style(UnderlineStyle::DOTTED);
        // HAS_EXTENDED unset: the extended style is ignored.
        assert_eq!(attrs.is_underline(), 0);
        attrs.bg |= BgFlags::HAS_EXTENDED;
        assert_eq!(attrs.is_underline(), 1);
    }

    #[test]
    fn color_modes_and_colors() {
        let mut attrs = AttributeData::default();
        assert!(attrs.is_fg_default() && attrs.is_bg_default());
        assert!(attrs.is_attribute_default());
        assert_eq!(attrs.get_fg_color(), -1);
        assert_eq!(attrs.get_bg_color(), -1);

        attrs.fg = Attributes::CM_P16 | 9;
        attrs.bg = Attributes::CM_P256 | 200;
        assert!(attrs.is_fg_palette() && attrs.is_bg_palette());
        assert!(!attrs.is_fg_rgb() && !attrs.is_bg_rgb());
        assert_eq!(attrs.get_fg_color_mode(), Attributes::CM_P16);
        assert_eq!(attrs.get_bg_color_mode(), Attributes::CM_P256);
        assert_eq!(attrs.get_fg_color(), 9);
        assert_eq!(attrs.get_bg_color(), 200);

        attrs.fg = Attributes::CM_RGB | 0x00AB_CDEF | FgFlags::BOLD;
        assert!(attrs.is_fg_rgb());
        assert_eq!(attrs.get_fg_color(), 0x00AB_CDEF);
        assert!(!attrs.is_attribute_default());
    }

    #[test]
    fn update_extended_follows_the_extended_attrs() {
        let mut attrs = AttributeData::default();
        attrs.extended.set_url_id(7);
        attrs.update_extended();
        assert!(attrs.has_extended_attrs() != 0);
        attrs.extended.set_url_id(0);
        attrs.update_extended();
        assert_eq!(attrs.has_extended_attrs(), 0);
    }

    #[test]
    fn url_id_forces_the_dashed_underline_style() {
        let mut ext = ExtendedAttrs::default();
        ext.set_underline_style(UnderlineStyle::CURLY);
        assert_eq!(ext.underline_style(), UnderlineStyle::CURLY);
        assert_eq!(ext.ext(), UnderlineStyle::CURLY << 26);
        assert!(!ext.is_empty());

        ext.set_url_id(3);
        assert_eq!(ext.url_id(), 3);
        assert_eq!(ext.underline_style(), UnderlineStyle::DASHED);
        // The `ext` getter folds the style in; the raw value is untouched.
        assert_eq!(ext.ext(), UnderlineStyle::DASHED << 26);

        // A url id alone makes the object non-empty, even without a style.
        let mut only_url = ExtendedAttrs::default();
        assert!(only_url.is_empty());
        only_url.set_url_id(1);
        assert!(!only_url.is_empty());
        assert_eq!(only_url.underline_style(), UnderlineStyle::DASHED);
    }

    #[test]
    fn underline_color_and_style_share_the_ext_word() {
        let mut ext = ExtendedAttrs::new(0, 0);
        ext.set_underline_color(Attributes::CM_RGB | 0x0012_3456);
        ext.set_underline_style(UnderlineStyle::DOUBLE);
        ext.set_underline_variant_offset(5);
        assert_eq!(ext.underline_color(), Attributes::CM_RGB | 0x0012_3456);
        assert_eq!(ext.underline_style(), UnderlineStyle::DOUBLE);
        assert_eq!(ext.underline_variant_offset(), 5);
        // Setting a colour keeps only the colour bits of its argument.
        ext.set_underline_color(0xFFFF_FFFF);
        assert_eq!(ext.underline_color(), 0x03FF_FFFF);
        assert_eq!(ext.underline_style(), UnderlineStyle::DOUBLE);
        assert_eq!(ext.underline_variant_offset(), 5);
        // Styles and offsets wider than their fields are masked.
        ext.set_underline_style(0xFF);
        assert_eq!(ext.underline_style(), 7);
        ext.set_underline_variant_offset(0xFF);
        assert_eq!(ext.underline_variant_offset(), 7);
        assert_eq!(ext.underline_color(), 0x03FF_FFFF);
    }

    #[test]
    fn extended_attrs_are_a_copy_value() {
        let mut a = AttributeData::default();
        a.extended.set_url_id(9);
        let mut b = a;
        b.extended.set_url_id(10);
        assert_eq!(a.extended.url_id(), 9);
        assert_eq!(b.extended.url_id(), 10);
    }

    #[test]
    fn constants_match_upstream() {
        use crate::buffer::constants::{Content, ExtFlags};
        assert_eq!(crate::buffer::constants::DEFAULT_ATTR, 256);
        assert_eq!(Content::IS_COMBINED_MASK, 1 << 21);
        assert_eq!(Content::WIDTH_MASK, 3 << 22);
        assert_eq!(
            Content::HAS_CONTENT_MASK,
            Content::CODEPOINT_MASK | Content::IS_COMBINED_MASK
        );
        assert_eq!(Attributes::CM_RGB, Attributes::CM_P16 | Attributes::CM_P256);
        assert_eq!(
            Attributes::RGB_MASK,
            Attributes::RED_MASK | Attributes::GREEN_MASK | Attributes::BLUE_MASK
        );
        assert_eq!(FgFlags::STRIKETHROUGH, 1 << 31);
        assert_eq!(FgFlags::INVERSE, 1 << 26);
        assert_eq!(BgFlags::OVERLINE, 1 << 30);
        assert_eq!(ExtFlags::UNDERLINE_STYLE, 0b111 << 26);
        assert_eq!(ExtFlags::VARIANT_OFFSET, 0b111 << 29);
    }
}
