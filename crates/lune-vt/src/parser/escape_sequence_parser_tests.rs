//! Port of xterm.js src/common/parser/EscapeSequenceParser.test.ts @ f447274.
//!
//! Ported so far: "function identifiers → registration limits" (test.ts:1584-1629), plus
//! Rust-only cases for the handler API (plan M4 step 4) that drive the dispatch helpers the
//! parse loop calls. JS `assert.throws` becomes an `Err(IdentifierError)` assertion; upstream's
//! `{ prefix: '??' }` and `{ final: 'zz' }` cases are unrepresentable (`prefix` and
//! `final_byte` are `char`s) and are covered by the non-BMP cases below instead.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::parser::dcs_parser::DcsStringHandler;
use crate::parser::osc_parser::OscStringHandler;
use crate::parser::params::ParamsArrayItem::Param;

#[derive(Debug, Default)]
struct Host {
    join: u32,
    out: Vec<String>,
}

impl ParserHost for Host {
    fn preceding_join_state(&mut self) -> &mut u32 {
        &mut self.join
    }
}

fn csi_ident(id: &FunctionIdentifier) -> Result<u32, IdentifierError> {
    identifier(id, CSI_FINAL_RANGE)
}

fn fi(prefix: Option<char>, intermediates: &str, final_byte: char) -> FunctionIdentifier {
    FunctionIdentifier {
        prefix,
        intermediates: intermediates.to_owned(),
        final_byte,
    }
}

// describe('function identifiers') → describe('registration limits')

#[test]
fn registration_limits_prefix_range_0x3c_0x3f_one_byte() {
    for i in 0x3c..=0x3f_u8 {
        let c = char::from(i);
        let ident = csi_ident(&FunctionIdentifier::with_prefix(c, 'z')).unwrap();
        assert_eq!(ident_to_string(ident), format!("{c}z"));
    }
    assert_eq!(
        csi_ident(&FunctionIdentifier::with_prefix('\x3b', 'z')),
        Err(IdentifierError::PrefixRange)
    );
    assert_eq!(
        csi_ident(&FunctionIdentifier::with_prefix('\x40', 'z')),
        Err(IdentifierError::PrefixRange)
    );
    // `{ prefix: '??' }`: unrepresentable; a non-BMP prefix (two UTF-16 units upstream) fails
    assert_eq!(
        csi_ident(&FunctionIdentifier::with_prefix('\u{1F600}', 'z')),
        Err(IdentifierError::PrefixRange)
    );
}

#[test]
fn registration_limits_intermediates_range_0x20_0x2f_up_to_two_bytes() {
    for i in 0x20..=0x2f_u8 {
        let c = char::from(i);
        let ident = csi_ident(&FunctionIdentifier::with_intermediates(
            &format!("{c}{c}"),
            'z',
        ))
        .unwrap();
        assert_eq!(ident_to_string(ident), format!("{c}{c}z"));
    }
    assert_eq!(
        csi_ident(&FunctionIdentifier::with_intermediates("\x1f", 'z')),
        Err(IdentifierError::IntermediateRange)
    );
    assert_eq!(
        csi_ident(&FunctionIdentifier::with_intermediates("\x30", 'z')),
        Err(IdentifierError::IntermediateRange)
    );
    assert_eq!(
        csi_ident(&FunctionIdentifier::with_intermediates("!!!", 'z')),
        Err(IdentifierError::TooManyIntermediates)
    );
}

#[test]
fn registration_limits_final_csi_dcs_range_0x40_0x7e_one_byte() {
    for i in 0x40..=0x7e_u8 {
        let c = char::from(i);
        let ident = csi_ident(&FunctionIdentifier::new(c)).unwrap();
        assert_eq!(ident_to_string(ident), c.to_string());
    }
    let range_err = IdentifierError::FinalRange { lo: 64, hi: 126 };
    let err = Err(range_err);
    assert_eq!(csi_ident(&FunctionIdentifier::new('\x3f')), err);
    assert_eq!(csi_ident(&FunctionIdentifier::new('\x7f')), err);
    // `{ final: 'zz' }`: unrepresentable; a non-BMP final (two UTF-16 units upstream) fails
    assert_eq!(csi_ident(&FunctionIdentifier::new('\u{1F600}')), err);
    assert_eq!(range_err.to_string(), "final must be in range 64 .. 126");
}

#[test]
fn registration_limits_final_esc_range_0x30_0x7e_one_byte() {
    let mut parser = EscapeSequenceParser::<Host>::new();
    for i in 0x30..=0x7e_u8 {
        let id = parser
            .register_esc_handler(FunctionIdentifier::new(char::from(i)), |_| true)
            .unwrap();
        parser.dispose_handler(id);
    }
    let err = Err(IdentifierError::FinalRange { lo: 48, hi: 126 });
    assert_eq!(
        parser.register_esc_handler(FunctionIdentifier::new('\x2f'), |_| true),
        err
    );
    assert_eq!(
        parser.register_esc_handler(FunctionIdentifier::new('\x7f'), |_| true),
        err
    );
}

#[test]
fn registration_limits_id_calculation_should_stack_prefix_intermediate_final() {
    let s = |id: FunctionIdentifier| ident_to_string(csi_ident(&id).unwrap());
    assert_eq!(s(fi(None, "", 'z')), "z");
    assert_eq!(s(fi(Some('?'), "", 'z')), "?z");
    assert_eq!(s(fi(None, "!", 'z')), "!z");
    assert_eq!(s(fi(Some('?'), "!", 'z')), "?!z");
    assert_eq!(s(fi(Some('?'), "!!", 'z')), "?!!z");
}

// Rust-only cases.

/// Upstream `if (id.prefix)` + `res && ...`: a NUL prefix packs to 0, i.e. no prefix.
#[test]
fn nul_prefix_is_no_prefix_as_upstream() {
    assert_eq!(
        csi_ident(&fi(Some('\0'), "", 'm')),
        csi_ident(&FunctionIdentifier::new('m'))
    );
}

/// Every constructor builds the same identifier as the struct literal.
#[test]
fn function_identifier_constructors() {
    assert_eq!(FunctionIdentifier::new('m'), fi(None, "", 'm'));
    assert_eq!(
        FunctionIdentifier::with_prefix('?', 'h'),
        fi(Some('?'), "", 'h')
    );
    assert_eq!(
        FunctionIdentifier::with_intermediates(" ", 'q'),
        fi(None, " ", 'q')
    );
    assert_eq!(
        FunctionIdentifier::with_prefix_and_intermediates('>', "$", 'p'),
        fi(Some('>'), "$", 'p')
    );
}

#[test]
fn ident_to_string_terminates_for_every_u32_shape() {
    assert_eq!(ident_to_string(0), "");
    assert_eq!(ident_to_string(0x6d), "m");
    assert_eq!(ident_to_string(0x3f21_216d), "?!!m");
    // high bit set: upstream's arithmetic shift would loop forever on this (negative) int32
    assert_eq!(ident_to_string(u32::MAX), "\u{ff}".repeat(4));
    assert_eq!(ident_to_string(0x8000_0000), "\u{80}\0\0\0");
}

fn recording_parser() -> EscapeSequenceParser<Host> {
    let mut p = EscapeSequenceParser::<Host>::new();
    p.set_csi_handler_fallback(|h: &mut Host, ident, params: &Params| {
        h.out.push(format!(
            "csi-fb {} {:?}",
            ident_to_string(ident),
            params.to_array()
        ));
    });
    p.set_esc_handler_fallback(|h: &mut Host, ident| {
        h.out.push(format!("esc-fb {}", ident_to_string(ident)));
    });
    p.set_execute_handler_fallback(|h: &mut Host, code| {
        h.out.push(format!("exe-fb {code}"));
    });
    p
}

fn csi_handler(tag: &'static str, ret: bool) -> impl FnMut(&mut Host, &Params) -> bool + 'static {
    move |h: &mut Host, params: &Params| {
        h.out.push(format!("{tag} {:?}", params.to_array()));
        ret
    }
}

/// Newest-first: the handler registered last runs first; `false` falls through to the older
/// one; `true` stops; nothing handled → fallback. Every CSI dispatch zeroes the join state.
#[test]
fn csi_newest_first_false_falls_through_then_fallback() {
    let mut p = recording_parser();
    let mut h = Host::default();
    let m = || FunctionIdentifier::new('m');
    p.register_csi_handler(m(), csi_handler("old", true))
        .unwrap();
    p.register_csi_handler(m(), csi_handler("new", false))
        .unwrap();
    p.params = Params::from_array(&[Param(1), Param(31)]).unwrap();
    h.join = 7;
    p.csi_dispatch(&mut h, u32::from('m'));
    assert_eq!(
        h.out,
        ["new [Param(1), Param(31)]", "old [Param(1), Param(31)]"]
    );
    assert_eq!(h.join, 0);

    // both decline → fallback
    let mut p = recording_parser();
    let mut h = Host::default();
    p.register_csi_handler(m(), csi_handler("a", false))
        .unwrap();
    p.register_csi_handler(m(), csi_handler("b", false))
        .unwrap();
    p.csi_dispatch(&mut h, u32::from('m'));
    assert_eq!(
        h.out,
        ["b [Param(0)]", "a [Param(0)]", "csi-fb m [Param(0)]"]
    );

    // newest returns true → older and fallback never run
    let mut p = recording_parser();
    let mut h = Host::default();
    p.register_csi_handler(m(), csi_handler("a", true)).unwrap();
    p.register_csi_handler(m(), csi_handler("b", true)).unwrap();
    p.csi_dispatch(&mut h, u32::from('m'));
    assert_eq!(h.out, ["b [Param(0)]"]);
}

/// Upstream "Execution order should go from latest handler down to the original" (ESC).
#[test]
fn esc_execution_order_latest_to_original() {
    let mut p = recording_parser();
    let mut h = Host::default();
    let id = || FunctionIdentifier::with_intermediates("%", 'G');
    for (n, ret) in [(1, true), (2, false), (3, false)] {
        p.register_esc_handler(id(), move |h: &mut Host| {
            h.out.push(n.to_string());
            ret
        })
        .unwrap();
    }
    p.collect = u32::from(b'%');
    h.join = 3;
    p.esc_dispatch(&mut h, u32::from('G'));
    assert_eq!(h.out, ["3", "2", "1"]);
    assert_eq!(h.join, 0);
}

/// The constructor swallows 7-bit ST (`ESC \`): its dispatch never reaches the fallback.
#[test]
fn esc_backslash_is_swallowed_by_default() {
    let mut p = recording_parser();
    let mut h = Host::default();
    p.esc_dispatch(&mut h, u32::from('\\'));
    p.esc_dispatch(&mut h, u32::from('E'));
    assert_eq!(h.out, ["esc-fb E"]);
}

/// Dispose removes exactly one handler from whichever table holds it; a second dispose (and a
/// dispose after `clear`) is a no-op and leaves the other handlers alone.
#[test]
fn dispose_twice_is_a_no_op_across_tables() {
    let mut p = recording_parser();
    let mut h = Host::default();
    let m = || FunctionIdentifier::new('m');
    p.register_csi_handler(m(), csi_handler("default", true))
        .unwrap();
    let custom = p
        .register_csi_handler(m(), csi_handler("custom", true))
        .unwrap();
    let esc = p
        .register_esc_handler(FunctionIdentifier::new('E'), |h: &mut Host| {
            h.out.push("E".to_owned());
            true
        })
        .unwrap();
    let osc = p.register_osc_handler(1, OscStringHandler::new(|_: &mut Host, _: &str| true));
    let dcs = p
        .register_dcs_handler(
            FunctionIdentifier::new('q'),
            DcsStringHandler::new(|_: &mut Host, _: &str, _: &Params| true),
        )
        .unwrap();
    // ids are unique across tables
    let ids = [custom, esc, osc, dcs];
    for (i, a) in ids.iter().enumerate() {
        for b in ids.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }
    p.dispose_handler(custom);
    p.dispose_handler(custom);
    p.csi_dispatch(&mut h, u32::from('m'));
    assert_eq!(h.out, ["default [Param(0)]"]);

    p.clear_esc_handler(FunctionIdentifier::new('E')).unwrap();
    p.dispose_handler(esc);
    p.dispose_handler(osc);
    p.dispose_handler(osc);
    p.dispose_handler(dcs);
    p.dispose_handler(dcs);
    p.dispose_handler(HandlerId(u64::MAX));
    h.out.clear();
    p.esc_dispatch(&mut h, u32::from('E'));
    p.csi_dispatch(&mut h, u32::from('m'));
    assert_eq!(h.out, ["esc-fb E", "default [Param(0)]"]);
}

#[test]
fn clear_handlers_report_identifier_errors() {
    let mut p = recording_parser();
    let bad = || FunctionIdentifier::new('\x7f');
    assert!(p.clear_csi_handler(bad()).is_err());
    assert!(p.clear_dcs_handler(bad()).is_err());
    assert!(p.clear_esc_handler(bad()).is_err());
    assert!(p.register_csi_handler(bad(), |_, _| true).is_err());
    assert!(
        p.register_dcs_handler(
            bad(),
            DcsStringHandler::new(|_: &mut Host, _: &str, _: &Params| true)
        )
        .is_err()
    );
    // clearing twice is fine
    assert!(p.clear_csi_handler(FunctionIdentifier::new('m')).is_ok());
    assert!(p.clear_csi_handler(FunctionIdentifier::new('m')).is_ok());
}

#[test]
fn execute_handler_set_clear_and_fallback() {
    let mut p = recording_parser();
    let mut h = Host::default();
    p.set_execute_handler('\n', |h: &mut Host| h.out.push("LF".to_owned()));
    h.join = 1;
    p.execute(&mut h, 0x0a);
    assert_eq!(h.join, 0);
    p.execute(&mut h, 0x0d);
    p.clear_execute_handler('\n');
    p.clear_execute_handler('\n'); // should not throw
    p.execute(&mut h, 0x0a);
    assert_eq!(h.out, ["LF", "exe-fb 13", "exe-fb 10"]);
}

#[test]
fn print_handler_set_and_clear() {
    let mut p = recording_parser();
    let mut h = Host::default();
    p.set_print_handler(|h: &mut Host, data: &[u32]| {
        h.out
            .push(crate::input::text_decoder::utf32_to_string(data));
    });
    p.print(&mut h, &[0x68, 0x69]);
    p.clear_print_handler();
    p.clear_print_handler(); // should not throw
    p.print(&mut h, &[0x68]);
    assert_eq!(h.out, ["hi"]);
}

#[test]
fn error_handler_sees_state_and_returns_abort() {
    let mut p = recording_parser();
    let mut h = Host::default();
    assert!(!p.error(&mut h, 0, 0x7f)); // default never aborts
    p.set_error_handler(|h: &mut Host, state: &ParsingState<'_>| {
        h.out.push(format!(
            "{} {:x} {:?} {} {:?} {}",
            state.position,
            state.code,
            state.current_state,
            state.collect,
            state.params.to_array(),
            state.abort
        ));
        true
    });
    p.params = Params::from_array(&[Param(1), Param(2), Param(0)]).unwrap();
    p.current_state = ParserState::CsiParam;
    assert!(p.error(&mut h, 6, 0x20ac));
    assert_eq!(
        h.out,
        ["6 20ac CsiParam 0 [Param(1), Param(2), Param(0)] false"]
    );
    p.clear_error_handler();
    p.clear_error_handler(); // should not throw
    assert!(!p.error(&mut h, 0, 0));
    assert_eq!(h.out.len(), 1);
}

/// `collect << 8 | code` drops bits shifted past 32, as JS int32 does; no overflow panic.
#[test]
fn ident_arithmetic_wraps() {
    let mut p = recording_parser();
    let mut h = Host::default();
    p.collect = 0xffff_ffff;
    p.csi_dispatch(&mut h, 0x6d);
    p.esc_dispatch(&mut h, 0x45);
    assert_eq!(
        h.out,
        [
            "csi-fb \u{ff}\u{ff}\u{ff}m [Param(0)]",
            "esc-fb \u{ff}\u{ff}\u{ff}E"
        ]
    );
}

/// `reset` zeroes the host's join state, ends running OSC/DCS handlers with `false`, returns
/// to GROUND and restores ZDM params; handlers are kept.
#[test]
fn reset_zeroes_join_state_and_aborts_sub_parsers() {
    let mut p = recording_parser();
    let mut h = Host::default();
    p.register_osc_handler(
        1,
        OscStringHandler::new(|h: &mut Host, data: &str| {
            h.out.push(format!("osc {data}"));
            true
        }),
    );
    p.register_dcs_handler(
        FunctionIdentifier::new('q'),
        DcsStringHandler::new(|h: &mut Host, data: &str, _: &Params| {
            h.out.push(format!("dcs {data}"));
            true
        }),
    )
    .unwrap();
    p.osc_parser.start(&mut h);
    p.osc_parser.put(&mut h, &[0x31, 0x3b, 0x78]);
    p.dcs_parser.hook(&mut h, u32::from('q'), &p.params);
    p.dcs_parser.put(&mut h, &[0x79]);
    p.params = Params::from_array(&[Param(5)]).unwrap();
    p.collect = 0x24;
    p.current_state = ParserState::CsiParam;
    h.join = 9;
    p.reset(&mut h);
    assert_eq!(h.join, 0);
    assert!(
        h.out.is_empty(),
        "aborted handlers must not run: {:?}",
        h.out
    );
    assert_eq!(p.current_state, ParserState::Ground);
    assert_eq!(p.collect, 0);
    assert_eq!(p.params.to_array(), [Param(0)]);

    // handlers survive reset
    p.osc_parser.start(&mut h);
    p.osc_parser.put(&mut h, &[0x31, 0x3b, 0x7a]);
    p.osc_parser.end(&mut h, true);
    assert_eq!(h.out, ["osc z"]);
}
