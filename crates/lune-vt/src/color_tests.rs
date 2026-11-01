//! Port of xterm.js src/common/Color.test.ts @ f447274, plus Rust-only no-panic and
//! JS-compatibility cases for `css::to_color`.
//!
//! Upstream spells out 17 near-identical assertions per test (`0x00`, `0x10`, ... `0xf0`,
//! `0xff`); here they run as a loop over the same 17 values. Two upstream tests share the name
//! "should convert the #rgb format to an IColor" (the second one is the `#rgba` format); the
//! Rust names tell them apart.

#![allow(clippy::unwrap_used)]

use super::*;

/// The 17 channel values upstream's tests use: 0x00, 0x10, ..., 0xf0, 0xff.
fn steps() -> Vec<u32> {
    let mut values: Vec<u32> = (0..=0xf).map(|n| n * 0x10).collect();
    values.push(0xff);
    values
}

fn ic(css: &str, rgba: u32) -> IColor {
    IColor {
        css: css.to_string(),
        rgba,
    }
}

fn hex2(v: u32) -> String {
    format!("{v:02x}")
}

mod color_tests {
    use super::*;

    mod channels_tests {
        use super::*;

        mod to_css {
            use super::*;

            #[test]
            fn should_convert_an_rgb_array_to_css_hex_string() {
                for v in steps() {
                    let h = hex2(v);
                    assert_eq!(channels::to_css(v, v, v, None), format!("#{h}{h}{h}"));
                }
            }

            #[test]
            fn should_convert_an_rgba_array_to_css_hex_string() {
                for v in steps() {
                    let h = hex2(v);
                    assert_eq!(channels::to_css(v, v, v, Some(v)), format!("#{h}{h}{h}{h}"));
                }
            }
        }

        mod to_rgba {
            use super::*;

            #[test]
            fn should_convert_an_rgb_array_to_an_rgba_number() {
                for v in steps() {
                    let expected = (v << 24) | (v << 16) | (v << 8) | 0xff;
                    assert_eq!(channels::to_rgba(v, v, v, None), expected);
                }
                assert_eq!(channels::to_rgba(0x70, 0x70, 0x70, None), 0x707070ff);
                assert_eq!(channels::to_rgba(0xff, 0xff, 0xff, None), 0xffffffff);
            }

            #[test]
            fn should_convert_an_rgba_array_to_an_rgba_number() {
                for v in steps() {
                    let expected = (v << 24) | (v << 16) | (v << 8) | v;
                    assert_eq!(channels::to_rgba(v, v, v, Some(v)), expected);
                }
                assert_eq!(channels::to_rgba(0x10, 0x10, 0x10, Some(0x10)), 0x10101010);
                assert_eq!(channels::to_rgba(0xff, 0xff, 0xff, Some(0xff)), 0xffffffff);
            }
        }

        mod to_color {
            use super::*;

            #[test]
            fn should_convert_an_rgb_array_to_an_icolor() {
                for v in steps() {
                    let h = hex2(v);
                    assert_eq!(
                        channels::to_color(v, v, v, None),
                        ic(
                            &format!("#{h}{h}{h}"),
                            (v << 24) | (v << 16) | (v << 8) | 0xff
                        )
                    );
                }
                assert_eq!(
                    channels::to_color(0xa0, 0xa0, 0xa0, None),
                    ic("#a0a0a0", 0xa0a0a0ff)
                );
            }

            #[test]
            fn should_convert_an_rgba_array_to_an_icolor() {
                for v in steps() {
                    let h = hex2(v);
                    assert_eq!(
                        channels::to_color(v, v, v, Some(v)),
                        ic(
                            &format!("#{h}{h}{h}{h}"),
                            (v << 24) | (v << 16) | (v << 8) | v
                        )
                    );
                }
                assert_eq!(
                    channels::to_color(0xff, 0xff, 0xff, Some(0xff)),
                    ic("#ffffffff", 0xffffffff)
                );
            }
        }
    }

    mod blend {
        use super::*;

        #[test]
        fn should_blend_colors_based_on_the_alpha_channel() {
            let bg = ic("#000000", 0x000000ff);
            // alpha 0x00 .. 0xf0 on black: the result is the alpha as grey.
            for v in steps().into_iter().filter(|&v| v != 0xff) {
                let fg_css = format!("#FFFFFF{}", hex2(v).to_uppercase());
                let fg = ic(&fg_css, 0xffffff00 | v);
                let h = hex2(v);
                assert_eq!(
                    color::blend(&bg, &fg),
                    ic(
                        &format!("#{h}{h}{h}"),
                        (v << 24) | (v << 16) | (v << 8) | 0xff
                    )
                );
            }
            // 0xff is returned as is, css included.
            assert_eq!(
                color::blend(&bg, &ic("#FFFFFFFF", 0xffffffff)),
                ic("#FFFFFFFF", 0xffffffff)
            );
        }
    }

    mod blend_towards_darker {
        use super::*;

        // Rust-only: a foreground channel below the background gives a negative delta.
        #[test]
        fn should_blend_a_darker_foreground() {
            let white = ic("#ffffff", 0xffffffff);
            let half_black = ic("#00000080", 0x00000080);
            // 255 + round(-255 * 128/255) = 255 - 128 = 127
            assert_eq!(color::blend(&white, &half_black), ic("#7f7f7f", 0x7f7f7fff));
            assert_eq!(rgba::blend(0xffffffff, 0x00000080), 0x7f7f7fff);
        }
    }

    mod opaque {
        use super::*;

        #[test]
        fn should_make_the_color_opaque() {
            for v in steps() {
                let h = hex2(v);
                let input = ic(
                    &format!("#{h}{h}{h}{h}"),
                    (v << 24) | (v << 16) | (v << 8) | v,
                );
                assert_eq!(
                    color::opaque(&input),
                    ic(
                        &format!("#{h}{h}{h}"),
                        (v << 24) | (v << 16) | (v << 8) | 0xff
                    )
                );
            }
        }
    }

    mod is_opaque {
        use super::*;

        fn t(css_str: &str) -> IColor {
            css::to_color(css_str).unwrap()
        }

        #[test]
        fn should_return_true_for_opaque_colors() {
            for s in [
                "#000000",
                "#000000ff",
                "#808080",
                "#808080ff",
                "#ffffff",
                "#ffffffff",
            ] {
                assert!(color::is_opaque(&t(s)), "{s}");
            }
        }

        #[test]
        fn should_return_false_for_transparent_colors() {
            for s in [
                "#00000000",
                "#00000080",
                "#000000fe",
                "#80808000",
                "#80808080",
                "#808080fe",
                "#ffffff00",
                "#ffffff80",
                "#fffffffe",
            ] {
                assert!(!color::is_opaque(&t(s)), "{s}");
            }
        }
    }

    mod opacity {
        use super::*;

        #[test]
        fn should_make_the_color_transparent() {
            let black = css::to_color("#000000").unwrap();
            assert_eq!(color::opacity(&black, 0.0), ic("#00000000", 0x00000000));
            assert_eq!(color::opacity(&black, 0.25), ic("#00000040", 0x00000040));
            assert_eq!(color::opacity(&black, 0.5), ic("#00000080", 0x00000080));
            assert_eq!(color::opacity(&black, 0.75), ic("#000000bf", 0x000000bf));
            assert_eq!(color::opacity(&black, 1.0), ic("#000000ff", 0x000000ff));
        }
    }

    // Rust-only: functions with no upstream test.
    mod untested_upstream {
        use super::*;

        #[test]
        fn multiply_opacity_scales_the_alpha() {
            let half = css::to_color("#10203080").unwrap();
            assert_eq!(
                color::multiply_opacity(&half, 0.5),
                ic("#10203040", 0x10203040)
            );
            assert_eq!(
                color::multiply_opacity(&half, 1.0),
                ic("#10203080", 0x10203080)
            );
        }

        #[test]
        fn to_color_rgb_drops_alpha() {
            assert_eq!(
                color::to_color_rgb(&ic("#10203040", 0x10203040)),
                [0x10, 0x20, 0x30]
            );
        }

        #[test]
        fn ensure_contrast_ratio_returns_an_opaque_icolor() {
            let bg = ic("#000000", 0x000000ff);
            let fg = ic("#606060", 0x606060ff);
            assert_eq!(color::ensure_contrast_ratio(&bg, &fg, 3.0), None);
            assert_eq!(
                color::ensure_contrast_ratio(&bg, &fg, 4.0),
                Some(ic("#707070", 0x707070ff))
            );
        }

        #[test]
        fn null_color_is_transparent_black() {
            assert_eq!(*NULL_COLOR, ic("#00000000", 0));
        }
    }
}

mod css_tests {
    use super::*;

    mod to_color {
        use super::*;

        #[test]
        fn should_convert_the_rgb_format_to_an_icolor() {
            for v in 0..=0xfu32 {
                let c = char::from_digit(v, 16).unwrap();
                let input = format!("#{c}{c}{c}");
                let h = hex2(v * 0x11);
                assert_eq!(
                    css::to_color(&input).unwrap(),
                    ic(&format!("#{h}{h}{h}"), ((v * 0x11) * 0x0101_0100) | 0xff)
                );
            }
        }

        // Upstream names this test the same as the one above.
        #[test]
        fn should_convert_the_rgba_format_to_an_icolor() {
            for v in 0..=0xfu32 {
                let c = char::from_digit(v, 16).unwrap();
                let input = format!("#{c}{c}{c}{c}");
                let h = hex2(v * 0x11);
                assert_eq!(
                    css::to_color(&input).unwrap(),
                    ic(&format!("#{h}{h}{h}{h}"), v * 0x11 * 0x0101_0101)
                );
            }
        }

        #[test]
        fn should_convert_the_rrggbb_format_to_an_icolor() {
            for v in steps() {
                let h = hex2(v);
                let input = format!("#{h}{h}{h}");
                assert_eq!(
                    css::to_color(&input).unwrap(),
                    ic(&input, (v << 24) | (v << 16) | (v << 8) | 0xff)
                );
            }
        }

        #[test]
        fn should_convert_the_rrggbbaa_format_to_an_icolor() {
            for v in steps() {
                let h = hex2(v);
                let input = format!("#{h}{h}{h}{h}");
                assert_eq!(
                    css::to_color(&input).unwrap(),
                    ic(&input, (v << 24) | (v << 16) | (v << 8) | v)
                );
            }
        }

        #[test]
        fn should_convert_the_rgb_function_format_to_an_icolor() {
            let cases = [
                ("rgb(0, 0, 0)", "#000000ff", 0x000000ff),
                ("rgb(80, 0, 0)", "#500000ff", 0x500000ff),
                ("rgb(0, 80, 0)", "#005000ff", 0x005000ff),
                ("rgb(0, 0, 80)", "#000050ff", 0x000050ff),
                ("rgb(255, 255, 255)", "#ffffffff", 0xffffffff),
            ];
            for (input, css_out, rgba_out) in cases {
                assert_eq!(
                    css::to_color(input).unwrap(),
                    ic(css_out, rgba_out),
                    "{input}"
                );
            }
        }

        #[test]
        fn should_convert_the_rgba_function_format_to_an_icolor() {
            let cases = [
                ("rgba(0, 0, 0, 0)", "#00000000", 0x00000000),
                ("rgba(80, 0, 0, 0.5)", "#50000080", 0x50000080),
                ("rgba(0, 80, 0, 0.5)", "#00500080", 0x00500080),
                ("rgba(0, 0, 80, 0.5)", "#00005080", 0x00005080),
                ("rgba(255, 255, 255, 1)", "#ffffffff", 0xffffffff),
            ];
            for (input, css_out, rgba_out) in cases {
                assert_eq!(
                    css::to_color(input).unwrap(),
                    ic(css_out, rgba_out),
                    "{input}"
                );
            }
        }
    }

    // Rust-only: input upstream's tests do not cover. Every expected value below was produced
    // by running the upstream expression in Node (v-any: plain JS number semantics).
    mod to_color_edge_cases {
        use super::*;

        #[test]
        fn non_ascii_input_is_an_error_not_a_panic() {
            for s in [
                "#€€€",
                "#abc€",
                "#ab€c",
                "€#abc",
                "#€€€€€€€",
                "#abcdef€",
                "rgb(€)",
                "rgba(1, 2, €)",
                "rgb(1,2,3€",
                "𝄞𝄞𝄞𝄞𝄞𝄞𝄞",
                "#𝄞𝄞𝄞",
                "",
                "#",
                "red",
            ] {
                assert_eq!(css::to_color(s), Err(UnsupportedCssFormat), "{s:?}");
            }
        }

        #[test]
        fn non_ascii_after_a_hex_prefix_reads_like_parse_int() {
            // 9 UTF-16 units: `parseInt("abcdef€1", 16)` stops at the euro sign.
            let s = "#abcdef€1";
            assert_eq!(css::to_color(s).unwrap(), ic(s, 0x00abcdef));
            // An astral char counts as two UTF-16 units: "#abc𝄞x" is 7 units.
            let s = "#abc𝄞x";
            assert_eq!(css::to_color(s).unwrap(), ic(s, 0x000abcff));
            // ... and "#abc𝄞𝄞" is 8, which no branch handles.
            assert_eq!(css::to_color("#abc𝄞𝄞"), Err(UnsupportedCssFormat));
        }

        #[test]
        fn unsupported_formats_are_errors() {
            for s in [
                "",
                "red",
                "#ab",
                "#abcdefabc",
                "hsl(0, 0%, 0%)",
                "rgb(1, 2)",
                "rgb(1000, 0, 0)",
                "rgba(1, 2, 3, 2)",
                "rgba(1, 2, 3, 10)",
            ] {
                assert_eq!(css::to_color(s), Err(UnsupportedCssFormat), "{s:?}");
            }
        }

        #[test]
        fn the_rgb_function_tolerates_whitespace_and_a_prefix() {
            assert_eq!(
                css::to_color("rgb(1,2,3)").unwrap(),
                ic("#010203ff", 0x010203ff)
            );
            assert_eq!(
                css::to_color("  rgb( 1 , 2 , 3 )  ").unwrap(),
                ic("#010203ff", 0x010203ff)
            );
            assert_eq!(
                css::to_color("color: rgba( 255 ,0,0 , .5 ) !important").unwrap(),
                ic("#ff000080", 0xff000080)
            );
            assert_eq!(
                css::to_color("rgba(1, 2, 3, 0)").unwrap(),
                ic("#01020300", 0x01020300)
            );
            assert_eq!(
                css::to_color("rgba(1, 2, 3, 1)").unwrap(),
                ic("#010203ff", 0x010203ff)
            );
        }

        #[test]
        fn the_rgb_function_does_not_clamp_channels() {
            // `\d{1,3}` allows 999; upstream formats it as is ("3e7").
            assert_eq!(css::to_color("rgb(999, 0, 0)").unwrap().css, "#3e70000ff");
        }

        #[test]
        fn the_rgba_alpha_alternation_matches_zero_one_or_a_decimal() {
            // `0|1|\d?\.(\d+)`: "9.5" is a valid alpha upstream, scaled past 255.
            // 9.5 * 255 = 2422.5, Math.round -> 2423 = 0x977.
            assert_eq!(
                css::to_color("rgba(1, 2, 3, 9.5)").unwrap().css,
                "#010203977"
            );
        }

        #[test]
        fn hex_runs_embedded_in_longer_strings_follow_parse_int() {
            // The `#[\da-f]{3,8}` test is unanchored; then `parseInt(css.slice(1), 16)` reads the
            // longest hex prefix. Values from `(parseInt(css.slice(1), 16) << 8 | 0xff) >>> 0`.
            let cases = [
                ("#abcdeg", 0x0abcdeff_u32),
                ("ab#abcd", 0x00000bff),
                ("x-1#abc", 0xffffffff),
                ("xx#abcd", 0x000000ff),
                ("  #abcd", 0x000000ff),
            ];
            for (input, rgba) in cases {
                assert_eq!(css::to_color(input).unwrap(), ic(input, rgba), "{input:?}");
            }
            // length 9: `parseInt(css.slice(1), 16) >>> 0`
            assert_eq!(
                css::to_color("#abcdefg1").unwrap(),
                ic("#abcdefg1", 0x00abcdef)
            );
            assert_eq!(
                css::to_color("#abcdef12").unwrap(),
                ic("#abcdef12", 0xabcdef12)
            );
        }

        #[test]
        fn short_forms_with_a_non_hex_digit_are_errors() {
            // LUNETERM: upstream builds NaN channels here ("#NaNNaNNaN").
            assert!(css::to_color("x#abc").is_err());
            assert!(css::to_color("#abc").is_ok());
        }
    }
}

mod rgb_tests {
    use super::*;

    mod relative_luminance {
        use super::*;

        #[test]
        fn should_calculate_the_relative_luminance_of_the_color() {
            assert_eq!(rgb::relative_luminance(0x000000), 0.0);
            let expected = [
                (0x101010, "0.0052"),
                (0x202020, "0.0144"),
                (0x303030, "0.0296"),
                (0x404040, "0.0513"),
                (0x505050, "0.0802"),
                (0x606060, "0.1170"),
                (0x707070, "0.1620"),
                (0x808080, "0.2159"),
                (0x909090, "0.2789"),
                (0xa0a0a0, "0.3515"),
                (0xb0b0b0, "0.4342"),
                (0xc0c0c0, "0.5271"),
                (0xd0d0d0, "0.6308"),
                (0xe0e0e0, "0.7454"),
                (0xf0f0f0, "0.8714"),
            ];
            for (value, text) in expected {
                assert_eq!(
                    format!("{:.4}", rgb::relative_luminance(value)),
                    text,
                    "{value:#x}"
                );
            }
            assert_eq!(rgb::relative_luminance(0xffffff), 1.0);
        }
    }
}

mod rgba_tests {
    use super::*;

    mod blend {
        use super::*;

        #[test]
        fn should_blend_colors_based_on_the_alpha_channel() {
            for v in steps().into_iter().filter(|&v| v != 0xff) {
                assert_eq!(
                    rgba::blend(0x000000ff, 0xffffff00 | v),
                    (v << 24) | (v << 16) | (v << 8) | 0xff
                );
            }
            assert_eq!(rgba::blend(0x000000ff, 0xffffffff), 0xffffffff);
        }
    }

    mod ensure_contrast_ratio {
        use super::*;

        #[test]
        fn should_return_undefined_if_the_color_already_meets_the_contrast_ratio_black_bg() {
            assert_eq!(
                rgba::ensure_contrast_ratio(0x000000ff, 0x606060ff, 1.0),
                None
            );
            assert_eq!(
                rgba::ensure_contrast_ratio(0x000000ff, 0x606060ff, 2.0),
                None
            );
            assert_eq!(
                rgba::ensure_contrast_ratio(0x000000ff, 0x606060ff, 3.0),
                None
            );
        }

        #[test]
        fn should_return_a_color_that_meets_the_contrast_ratio_black_bg() {
            let expected = [
                (4.0, 0x707070ff_u32),
                (5.0, 0x7f7f7fff),
                (6.0, 0x8c8c8cff),
                (7.0, 0x989898ff),
                (8.0, 0xa3a3a3ff),
                (9.0, 0xadadadff),
                (10.0, 0xb6b6b6ff),
                (11.0, 0xbebebeff),
                (12.0, 0xc5c5c5ff),
                (13.0, 0xd1d1d1ff),
                (14.0, 0xd6d6d6ff),
                (15.0, 0xdbdbdbff),
                (16.0, 0xe3e3e3ff),
                (17.0, 0xe9e9e9ff),
                (18.0, 0xeeeeeeff),
                (19.0, 0xf4f4f4ff),
                (20.0, 0xfafafaff),
                (21.0, 0xffffffff),
            ];
            for (ratio, rgba_out) in expected {
                assert_eq!(
                    rgba::ensure_contrast_ratio(0x000000ff, 0x606060ff, ratio),
                    Some(rgba_out),
                    "ratio {ratio}"
                );
            }
        }

        #[test]
        fn should_return_undefined_if_the_color_already_meets_the_contrast_ratio_white_bg() {
            for ratio in 1..=6 {
                assert_eq!(
                    rgba::ensure_contrast_ratio(0xffffffff, 0x606060ff, f64::from(ratio)),
                    None,
                    "ratio {ratio}"
                );
            }
        }

        #[test]
        fn should_return_a_color_that_meets_the_contrast_ratio_white_bg() {
            let expected = [
                (7.0, 0x565656ff_u32),
                (8.0, 0x4d4d4dff),
                (9.0, 0x454545ff),
                (10.0, 0x3e3e3eff),
                (11.0, 0x373737ff),
                (12.0, 0x313131ff),
                (13.0, 0x313131ff),
                (14.0, 0x272727ff),
                (15.0, 0x232323ff),
                (16.0, 0x1f1f1fff),
                (17.0, 0x1b1b1bff),
                (18.0, 0x151515ff),
                (19.0, 0x101010ff),
                (20.0, 0x080808ff),
                (21.0, 0x000000ff),
            ];
            for (ratio, rgba_out) in expected {
                assert_eq!(
                    rgba::ensure_contrast_ratio(0xffffffff, 0x606060ff, ratio),
                    Some(rgba_out),
                    "ratio {ratio}"
                );
            }
        }

        // Rust-only: the luminance loops must terminate for any ratio.
        #[test]
        fn impossible_ratios_terminate() {
            assert!(rgba::ensure_contrast_ratio(0x808080ff, 0x808080ff, 1000.0).is_some());
            assert!(rgba::ensure_contrast_ratio(0x808080ff, 0x808080ff, f64::INFINITY).is_some());
            assert_eq!(
                rgba::ensure_contrast_ratio(0x808080ff, 0x000000ff, f64::NAN),
                None
            );
        }
    }

    mod to_channels {
        use super::*;

        #[test]
        fn should_convert_an_rgba_number_to_an_rgba_array() {
            for v in steps() {
                assert_eq!(
                    rgba::to_channels((v << 24) | (v << 16) | (v << 8) | v),
                    [v, v, v, v]
                );
            }
            assert_eq!(rgba::to_channels(0xffffffff), [0xff, 0xff, 0xff, 0xff]);
        }
    }
}

mod to_padded_hex {
    use super::*;

    #[test]
    fn should_convert_numbers_to_2_digit_hex_values() {
        for v in steps() {
            assert_eq!(to_padded_hex(v), hex2(v));
        }
        assert_eq!(to_padded_hex(0x00), "00");
        assert_eq!(to_padded_hex(0xa0), "a0");
        assert_eq!(to_padded_hex(0xff), "ff");
        // Wider values print in full, as `Number.prototype.toString(16)`.
        assert_eq!(to_padded_hex(0x1ff), "1ff");
        assert_eq!(to_padded_hex(0x5), "05");
    }
}

mod contrast_ratio {
    use super::*;

    #[test]
    fn should_calculate_the_relative_luminance_of_the_color() {
        assert_eq!(contrast_ratio(0.0, 0.0), 1.0);
        assert_eq!(contrast_ratio(0.0, 0.5), 11.0);
        assert_eq!(contrast_ratio(0.0, 1.0), 21.0);
    }

    #[test]
    fn should_work_regardless_of_the_parameter_order() {
        assert_eq!(contrast_ratio(0.0, 1.0), 21.0);
        assert_eq!(contrast_ratio(1.0, 0.0), 21.0);
    }
}
