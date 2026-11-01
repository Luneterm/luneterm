//! Port of xterm.js src/common/parser/Params.test.ts @ f447274, plus Rust-only no-panic cases.
//!
//! JS `assert.throws` becomes an `Err(ParamsError)` assertion. Test names follow the upstream
//! `describe`/`it` text.

#![allow(clippy::unwrap_used)]

use super::ParamsArrayItem::{Param, Sub};
use super::*;

fn s(values: &[i32]) -> ParamsArrayItem {
    Sub(values.to_vec())
}

/// `Params` parser shim from the upstream test: one chunk list, `;` `:` or a digit per char.
///
/// Upstream drives the same per-character switch from a `do { } while` loop whose bound reads
/// `s.length` (the chunk-array length); for the digit/`;`/`:` input the tests use that is
/// equivalent to this plain per-character loop.
fn parse(params: &mut Params, chunks: &[&str]) {
    params.reset();
    params.add_param(0).unwrap();
    for chunk in chunks {
        for ch in chunk.chars() {
            match ch {
                ';' => params.add_param(0).unwrap(),
                ':' => params.add_sub_param(-1).unwrap(),
                _ => params.add_digit(ch as i32 - 48),
            }
        }
    }
}

fn to_array(p: &Params) -> Vec<ParamsArrayItem> {
    p.to_array()
}

fn ints(values: &[i32]) -> Vec<ParamsArrayItem> {
    values.iter().map(|&v| Param(v)).collect()
}

#[test]
fn should_respect_ctor_args() {
    let params = Params::with_limits(12, 23).unwrap();
    assert_eq!(params.params.len(), 12);
    assert_eq!(params.sub_params.len(), 23);
    assert_eq!(to_array(&params), vec![]);
}

#[test]
fn add_param() {
    let mut params = Params::new();
    params.add_param(1).unwrap();
    assert_eq!(params.length(), 1);
    assert_eq!(params.values(), &[1]);
    assert_eq!(to_array(&params), ints(&[1]));
    params.add_param(23).unwrap();
    assert_eq!(params.length(), 2);
    assert_eq!(params.values(), &[1, 23]);
    assert_eq!(to_array(&params), ints(&[1, 23]));
    assert_eq!(params.sub_params_length, 0);
}

#[test]
fn add_sub_param() {
    let mut params = Params::new();
    params.add_param(1).unwrap();
    params.add_sub_param(2).unwrap();
    params.add_sub_param(3).unwrap();
    assert_eq!(params.length(), 1);
    assert_eq!(params.sub_params_length, 2);
    assert_eq!(to_array(&params), vec![Param(1), s(&[2, 3])]);
    params.add_param(12345).unwrap();
    params.add_sub_param(-1).unwrap();
    assert_eq!(params.length(), 2);
    assert_eq!(params.sub_params_length, 3);
    assert_eq!(
        to_array(&params),
        vec![Param(1), s(&[2, 3]), Param(12345), s(&[-1])]
    );
}

#[test]
fn should_not_add_sub_params_without_previous_param() {
    let mut params = Params::new();
    params.add_sub_param(2).unwrap();
    params.add_sub_param(3).unwrap();
    assert_eq!(params.length(), 0);
    assert_eq!(params.sub_params_length, 0);
    assert_eq!(to_array(&params), vec![]);
    params.add_param(1).unwrap();
    params.add_sub_param(2).unwrap();
    params.add_sub_param(3).unwrap();
    assert_eq!(params.length(), 1);
    assert_eq!(params.sub_params_length, 2);
    assert_eq!(to_array(&params), vec![Param(1), s(&[2, 3])]);
}

#[test]
fn reset() {
    let mut params = Params::new();
    params.add_param(1).unwrap();
    params.add_sub_param(2).unwrap();
    params.add_sub_param(3).unwrap();
    params.add_param(12345).unwrap();
    params.add_sub_param(-1).unwrap();
    params.reset();
    assert_eq!(params.length(), 0);
    assert_eq!(params.sub_params_length, 0);
    assert_eq!(to_array(&params), vec![]);
    params.add_param(1).unwrap();
    params.add_sub_param(2).unwrap();
    params.add_sub_param(3).unwrap();
    params.add_param(12345).unwrap();
    params.add_sub_param(-1).unwrap();
    assert_eq!(params.length(), 2);
    assert_eq!(params.sub_params_length, 3);
    assert_eq!(
        to_array(&params),
        vec![Param(1), s(&[2, 3]), Param(12345), s(&[-1])]
    );
}

#[test]
fn params_from_array_to_array() {
    let round_trip = |data: Vec<ParamsArrayItem>| {
        assert_eq!(Params::from_array(&data).unwrap().to_array(), data);
    };
    round_trip(vec![]);
    round_trip(vec![Param(1), s(&[2, 3]), Param(12345), s(&[-1])]);
    round_trip(ints(&[38, 2, 50, 100, 150]));
    round_trip(vec![Param(38), Param(2), Param(50), Param(100), s(&[150])]);
    round_trip(vec![Param(38), s(&[2, 50, 100, 150])]);
    // strip empty sub params
    let data = vec![Param(38), s(&[2, 50, 100, 150]), Param(5), s(&[]), Param(6)];
    assert_eq!(
        Params::from_array(&data).unwrap().to_array(),
        vec![Param(38), s(&[2, 50, 100, 150]), Param(5), Param(6)]
    );
}

#[test]
fn from_array_skips_leading_sub_params() {
    let data = vec![s(&[7, 8]), Param(1), s(&[2])];
    assert_eq!(
        Params::from_array(&data).unwrap().to_array(),
        vec![Param(1), s(&[2])]
    );
}

#[test]
fn clone() {
    let params = Params::from_array(&[
        Param(38),
        s(&[2, 50, 100, 150]),
        Param(5),
        s(&[]),
        Param(6),
        Param(1),
        s(&[2, 3]),
        Param(12345),
        s(&[-1]),
    ])
    .unwrap();
    assert_eq!(params.clone(), params);
}

#[test]
fn has_sub_params_get_sub_params() {
    let params =
        Params::from_array(&[Param(38), s(&[2, 50, 100, 150]), Param(5), s(&[]), Param(6)])
            .unwrap();
    assert!(params.has_sub_params(0));
    assert_eq!(params.get_sub_params(0), Some(&[2, 50, 100, 150][..]));
    assert!(!params.has_sub_params(1));
    assert_eq!(params.get_sub_params(1), None);
    assert!(!params.has_sub_params(2));
    assert_eq!(params.get_sub_params(2), None);
}

#[test]
fn get_sub_params_all() {
    let params =
        Params::from_array(&[Param(1), s(&[2, 3]), Param(7), Param(12345), s(&[-1])]).unwrap();
    assert_eq!(
        params.get_sub_params_all(),
        vec![(0, vec![2, 3]), (2, vec![-1])]
    );
}

mod parse_tests {
    use super::*;

    #[test]
    fn param_defaults_to_0_zdm_zero_default_mode() {
        let mut params = Params::new();
        parse(&mut params, &[""]);
        assert_eq!(to_array(&params), ints(&[0]));
    }

    #[test]
    fn sub_param_defaults_to_minus_1() {
        let mut params = Params::new();
        parse(&mut params, &[":"]);
        assert_eq!(to_array(&params), vec![Param(0), s(&[-1])]);
    }

    #[test]
    fn should_correctly_reset_on_new_sequence() {
        let mut params = Params::new();
        parse(&mut params, &["1;2;3"]);
        assert_eq!(to_array(&params), ints(&[1, 2, 3]));
        parse(&mut params, &["4"]);
        assert_eq!(to_array(&params), ints(&[4]));
        parse(&mut params, &["4::123:5;6;7"]);
        assert_eq!(
            to_array(&params),
            vec![Param(4), s(&[-1, 123, 5]), Param(6), Param(7)]
        );
        parse(&mut params, &[""]);
        assert_eq!(to_array(&params), ints(&[0]));
    }

    #[test]
    fn should_handle_length_restrictions_correctly() {
        // restrict to 3 params and 3 sub params
        let mut params = Params::with_limits(3, 3).unwrap();
        parse(&mut params, &["1;2;3"]);
        assert_eq!(to_array(&params), ints(&[1, 2, 3]));
        parse(&mut params, &["4"]);
        assert_eq!(to_array(&params), ints(&[4]));
        parse(&mut params, &["4::123:5;6;7"]);
        assert_eq!(
            to_array(&params),
            vec![Param(4), s(&[-1, 123, 5]), Param(6), Param(7)]
        );
        parse(&mut params, &[""]);
        assert_eq!(to_array(&params), ints(&[0]));
        // overlong params
        parse(&mut params, &["1;2;3;4;5;6;7"]);
        assert_eq!(to_array(&params), ints(&[1, 2, 3]));
        // overlong sub params
        parse(&mut params, &["4;38:2::50:100:150;48:5:22"]);
        assert_eq!(
            to_array(&params),
            vec![Param(4), Param(38), s(&[2, -1, 50]), Param(48)]
        );
    }

    #[test]
    fn typical_sequences() {
        let mut params = Params::new();
        // SGR with semicolon syntax
        parse(&mut params, &["0;4;38;2;50;100;150;48;5;22"]);
        assert_eq!(
            to_array(&params),
            ints(&[0, 4, 38, 2, 50, 100, 150, 48, 5, 22])
        );
        // SGR mixed style (partly wrong)
        parse(&mut params, &["0;4;38;2;50:100:150;48;5:22"]);
        assert_eq!(
            to_array(&params),
            vec![
                Param(0),
                Param(4),
                Param(38),
                Param(2),
                Param(50),
                s(&[100, 150]),
                Param(48),
                Param(5),
                s(&[22]),
            ]
        );
        // SGR colon style
        parse(&mut params, &["0;4;38:2::50:100:150;48:5:22"]);
        assert_eq!(
            to_array(&params),
            vec![
                Param(0),
                Param(4),
                Param(38),
                s(&[2, -1, 50, 100, 150]),
                Param(48),
                s(&[5, 22]),
            ]
        );
    }
}

mod should_not_overflow_to_negative {
    use super::*;

    #[test]
    fn reject_params_lesser_minus_1() {
        let mut params = Params::new();
        params.add_param(-1).unwrap();
        let err = params.add_param(-2).unwrap_err();
        assert_eq!(err, ParamsError::ValueTooSmall);
        assert_eq!(err.to_string(), "values lesser than -1 are not allowed");
    }

    #[test]
    fn reject_subparams_lesser_minus_1() {
        let mut params = Params::new();
        params.add_param(-1).unwrap();
        params.add_sub_param(-1).unwrap();
        let err = params.add_sub_param(-2).unwrap_err();
        assert_eq!(err, ParamsError::ValueTooSmall);
        assert_eq!(err.to_string(), "values lesser than -1 are not allowed");
        assert_eq!(to_array(&params), vec![Param(-1), s(&[-1])]);
    }

    #[test]
    fn clamp_parsed_params() {
        let mut params = Params::new();
        parse(&mut params, &["2147483648"]);
        assert_eq!(to_array(&params), ints(&[0x7FFF_FFFF]));
    }

    #[test]
    fn clamp_parsed_subparams() {
        let mut params = Params::new();
        parse(&mut params, &[":2147483648"]);
        assert_eq!(to_array(&params), vec![Param(0), s(&[0x7FFF_FFFF])]);
    }
}

mod issue_2389 {
    use super::*;

    #[test]
    fn should_cancel_subdigits_if_beyond_params_limit() {
        let mut params = Params::new();
        parse(
            &mut params,
            &[";;;;;;;;;10;;;;;;;;;;20;;;;;;;;;;30;31;32;33;34;35::::::::"],
        );
        assert_eq!(
            to_array(&params),
            ints(&[
                0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 30, 31, 32
            ])
        );
    }

    #[test]
    fn should_carry_forward_is_sub_state() {
        let mut params = Params::new();
        parse(&mut params, &["1:22:33", "44"]);
        assert_eq!(to_array(&params), vec![Param(1), s(&[22, 3344])]);
    }
}

// Rust-only cases: the no-panic contract (M4 step 2).
mod no_panic {
    use super::*;

    #[test]
    fn forty_params_keep_32() {
        let mut params = Params::new();
        for i in 0..40 {
            params.add_param(i).unwrap();
        }
        assert_eq!(params.length(), 32);
        assert_eq!(params.values(), (0..32).collect::<Vec<_>>().as_slice());
    }

    #[test]
    fn forty_sub_params_keep_32() {
        let mut params = Params::new();
        params.add_param(1).unwrap();
        for i in 0..40 {
            params.add_sub_param(i).unwrap();
        }
        assert_eq!(params.sub_params_length, 32);
        assert_eq!(
            to_array(&params),
            vec![Param(1), Sub((0..32).collect::<Vec<_>>())]
        );
    }

    #[test]
    fn add_digit_on_12_nines_stays_max_value() {
        let mut params = Params::new();
        params.add_param(0).unwrap();
        for _ in 0..12 {
            params.add_digit(9);
        }
        assert_eq!(to_array(&params), ints(&[0x7FFF_FFFF]));
        // and again once saturated, in the sub-param store too
        params.add_sub_param(0).unwrap();
        for _ in 0..12 {
            params.add_digit(9);
        }
        assert_eq!(
            to_array(&params),
            vec![Param(0x7FFF_FFFF), s(&[0x7FFF_FFFF])]
        );
    }

    #[test]
    fn add_digit_with_an_out_of_range_digit_does_not_overflow() {
        let mut params = Params::new();
        params.add_param(i32::MAX).unwrap();
        params.add_digit(i32::MAX);
        params.add_digit(i32::MIN);
        params.add_param(1).unwrap();
        params.add_digit(i32::MAX);
        assert_eq!(params.values().first(), Some(&i32::MAX));
    }

    #[test]
    fn get_sub_params_out_of_range_is_none() {
        let mut params = Params::new();
        params.add_param(1).unwrap();
        params.add_sub_param(2).unwrap();
        assert_eq!(params.get_sub_params(1000), None);
        assert_eq!(params.get_sub_params(usize::MAX), None);
        assert!(!params.has_sub_params(1000));
        assert!(!params.has_sub_params(usize::MAX));
        assert_eq!(params.get(1000), None);
    }

    #[test]
    fn with_limits_rejects_more_than_256_sub_params() {
        assert!(Params::with_limits(3, 257).is_err());
        assert_eq!(
            Params::with_limits(3, 257).unwrap_err().to_string(),
            "maxSubParamsLength must not be greater than 256"
        );
        assert!(Params::with_limits(3, 256).is_ok());
        assert!(Params::with_limits(0, 0).is_ok());
    }

    #[test]
    fn zero_limits_store_nothing() {
        let mut params = Params::with_limits(0, 0).unwrap();
        params.add_param(1).unwrap();
        params.add_sub_param(2).unwrap();
        params.add_digit(3);
        assert_eq!(params.length(), 0);
        assert_eq!(to_array(&params), vec![]);
    }

    #[test]
    fn over_limit_values_below_minus_1_are_dropped_not_errors() {
        // Upstream checks the length limit before the value check.
        let mut params = Params::with_limits(1, 1).unwrap();
        params.add_param(1).unwrap();
        assert_eq!(params.add_param(-5), Ok(()));
        assert_eq!(params.add_sub_param(1), Ok(()));
        assert_eq!(params.add_sub_param(-5), Ok(()));
        assert_eq!(params.add_param(-5), Ok(()));
    }

    #[test]
    fn max_sub_params_use_the_u16_index_exactly_like_upstream() {
        // With 256 sub params the packed u16 index wraps (JS Uint16Array does the same); the
        // observable result is that the entry reads as having no sub params. Must not panic.
        let mut params = Params::with_limits(2, 256).unwrap();
        params.add_param(1).unwrap();
        for i in 0..300 {
            params.add_sub_param(i).unwrap();
        }
        assert_eq!(params.sub_params_length, 256);
        assert!(!params.has_sub_params(0));
        assert_eq!(params.get_sub_params(0), None);
        params.add_param(2).unwrap();
        let _ = params.to_array();
        let _ = params.get_sub_params_all();
        let _ = params.clone();
    }

    #[test]
    fn stale_values_survive_reset_like_the_js_typed_array() {
        let mut params = Params::new();
        params.add_param(7).unwrap();
        params.reset();
        assert_eq!(params.length(), 0);
        assert_eq!(params.get(0), Some(7));
        assert_eq!(params.get(32), None);
    }

    #[test]
    fn arbitrary_digit_streams_never_panic() {
        // Deterministic LCG; every op, with small limits so the reject paths fire.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u32
        };
        let mut params = Params::with_limits(4, 5).unwrap();
        for _ in 0..20_000 {
            match next() % 8 {
                0 => {
                    let _ = params.add_param((next() % 7) as i32 - 2);
                }
                1 => {
                    let _ = params.add_sub_param((next() % 7) as i32 - 2);
                }
                2..=4 => params.add_digit((next() % 10) as i32),
                5 => params.reset(),
                6 => {
                    let idx = next() as usize % 9;
                    let _ = params.get_sub_params(idx);
                    let _ = params.has_sub_params(idx);
                }
                _ => {
                    let _ = params.to_array();
                    let _ = params.get_sub_params_all();
                    let _ = params.clone();
                }
            }
        }
    }
}
