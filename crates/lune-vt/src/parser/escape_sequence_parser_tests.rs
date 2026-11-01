//! Port of xterm.js src/common/parser/EscapeSequenceParser.test.ts @ f447274.
//!
//! Ported: "Parser init and methods", "state transitions and actions", "escape sequence
//! examples", "coverage tests", "set/clear handler" (with the custom-handler suites),
//! "function identifiers", and the sync half of "EscapeSequenceParser - async". Rust-only cases
//! cover the handler API (plan M4 step 4), the transition table against a dump of the upstream
//! table, and the parse loop's no-panic and read-ahead edges (plan M4 step 5).
//!
//! Adaptations (reason in each):
//!  - `parse(parser, 'str')` decodes the string's code points (`StringToUtf32` is replaced, plan
//!    D5); C1 controls are written `\u{80}`.
//!  - `MockOscPutParser` (a replacement OSC parser recording raw `put` data) is replaced by the
//!    real [`OscParser`] with a fallback handler that records the payload: the OSC parser is
//!    primed with `start` + `;` so it is in its payload state, then `parser.osc` is the
//!    recorded payload. End results read `"<id>;<payload>, success: <bool>"` as in upstream.
//!  - `{ prefix: '??' }` and `{ final: 'zz' }` are unrepresentable (`char`), covered by non-BMP
//!    cases instead.
//!  - `parser.currentState = 124` (reset test) is unrepresentable; a valid state stands in.
//!  - `trackStackSavesOnPause`, `parseP`, `evalStackSaves` and every `async handlers` case, plus
//!    "sync handlers keep being parsed in sync mode" / "correct result on async parse call" /
//!    "improper continuation should throw": replaced, there are no async handlers (plan D3,
//!    `write` is synchronous). The sync fall-through chain of "sync/async SGR mixed" is kept
//!    (`csi_fall_through_chain_triples`).
//!  - `assert.deepEqual(p.transitions, VT500_TRANSITION_TABLE)` compares tables by value.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::input::text_decoder::utf32_to_string;
use crate::parser::dcs_parser::DcsStringHandler;
use crate::parser::osc_parser::OscStringHandler;
use crate::parser::params::ParamsArrayItem;
use crate::parser::params::ParamsArrayItem::Param;

/// What the harness records (upstream `testTerminal.calls`).
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Print(String),
    Osc(String),
    Exe(u32),
    Csi(String, Vec<ParamsArrayItem>, String),
    Esc(String, String),
    DcsHook(Vec<ParamsArrayItem>),
    DcsPut(String),
    DcsUnhook(bool),
}

/// Error-handler snapshot (upstream `IParsingState`, owned).
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    position: usize,
    code: u32,
    current_state: ParserState,
    collect: u32,
    params: Vec<ParamsArrayItem>,
    abort: bool,
}

#[derive(Debug, Default)]
struct Host {
    join: u32,
    /// Free-form log of the Rust-only and set/clear tests.
    out: Vec<String>,
    /// `testTerminal.calls`.
    calls: Vec<Call>,
    /// The OSC payload recorded by the fallback (`parser.osc` upstream).
    osc: String,
    /// Each OSC `put` chunk seen by the fallback.
    osc_puts: Vec<String>,
    err: Option<Seen>,
}

impl ParserHost for Host {
    fn preceding_join_state(&mut self) -> &mut u32 {
        &mut self.join
    }
}

type P = EscapeSequenceParser<Host>;

const ALL_STATES: [ParserState; 14] = [
    ParserState::Ground,
    ParserState::Escape,
    ParserState::EscapeIntermediate,
    ParserState::CsiEntry,
    ParserState::CsiParam,
    ParserState::CsiIntermediate,
    ParserState::CsiIgnore,
    ParserState::SosPmApcString,
    ParserState::OscString,
    ParserState::DcsEntry,
    ParserState::DcsParam,
    ParserState::DcsIgnore,
    ParserState::DcsIntermediate,
    ParserState::DcsPassthrough,
];

fn split_ident(ident: u32) -> (String, String) {
    let id = ident_to_string(ident);
    let mut chars: Vec<char> = id.chars().collect();
    let last = chars.pop().map(String::from).unwrap_or_default();
    (chars.into_iter().collect(), last)
}

/// Upstream `testParser`: records everything through fallbacks into `Host::calls`.
fn test_parser() -> P {
    let mut p = P::new();
    p.set_print_handler(|h: &mut Host, data: &[u32]| {
        h.calls.push(Call::Print(utf32_to_string(data)));
    });
    p.set_csi_handler_fallback(|h: &mut Host, ident, params: &Params| {
        let (collect, flag) = split_ident(ident);
        h.calls.push(Call::Csi(collect, params.to_array(), flag));
    });
    p.set_esc_handler_fallback(|h: &mut Host, ident| {
        let (collect, flag) = split_ident(ident);
        h.calls.push(Call::Esc(collect, flag));
    });
    p.set_execute_handler_fallback(|h: &mut Host, code| h.calls.push(Call::Exe(code)));
    p.set_osc_handler_fallback(|h: &mut Host, id, action| match action {
        OscFallbackAction::Start => h.osc.clear(),
        OscFallbackAction::Put(data) => {
            let text = utf32_to_string(data);
            h.osc.push_str(&text);
            h.osc_puts.push(text);
        }
        OscFallbackAction::End(success) => {
            let payload = std::mem::take(&mut h.osc);
            h.calls
                .push(Call::Osc(format!("{id};{payload}, success: {success}")));
        }
    });
    p.set_dcs_handler_fallback(|h: &mut Host, _ident, action| match action {
        DcsFallbackAction::Hook(params) => h.calls.push(Call::DcsHook(params.to_array())),
        DcsFallbackAction::Put(data) => h.calls.push(Call::DcsPut(utf32_to_string(data))),
        DcsFallbackAction::Unhook(success) => h.calls.push(Call::DcsUnhook(success)),
    });
    p
}

fn cps(s: &str) -> Vec<u32> {
    s.chars().map(u32::from).collect()
}

/// `parse(parser, 'string')`.
fn parse(p: &mut P, h: &mut Host, s: &str) {
    p.parse(h, &cps(s));
}

fn parse_cp(p: &mut P, h: &mut Host, code: u32) {
    p.parse(h, &[code]);
}

fn chr(code: u32) -> String {
    char::from_u32(code).unwrap().to_string()
}

fn range(a: u32, b: u32) -> Vec<u32> {
    (a..b).collect()
}

/// C0 controls executed by the table: 0x00 - 0x17, 0x19, 0x1c - 0x1f.
fn exes() -> Vec<u32> {
    let mut v = range(0x00, 0x18);
    v.push(0x19);
    v.extend(range(0x1c, 0x20));
    v
}

fn params_of(values: &[i32]) -> Params {
    let items: Vec<ParamsArrayItem> = values.iter().map(|&v| Param(v)).collect();
    Params::from_array(&items).unwrap()
}

fn arr(values: &[i32]) -> Vec<ParamsArrayItem> {
    values.iter().map(|&v| Param(v)).collect()
}

fn set_collect(p: &mut P, s: &str) {
    p.collect = s
        .chars()
        .fold(0u32, |acc, c| acc.wrapping_shl(8) | u32::from(c));
}

fn collect_of(p: &P) -> String {
    ident_to_string(p.collect)
}

/// `parser.reset(); testTerminal.clear();`
fn reset_clear(p: &mut P, h: &mut Host) {
    p.reset(h);
    h.calls.clear();
    h.osc.clear();
    h.osc_puts.clear();
}

/// Put the OSC parser into its payload state (as `OSC_START` + `0;` would), silently.
fn prime_osc(p: &mut P, h: &mut Host) {
    p.osc_parser.start(h);
    p.osc_parser.put(h, &[0x3b]);
    h.calls.clear();
    h.osc.clear();
    h.osc_puts.clear();
}

/// Tagged entries of `Host::out` (`tag|payload`), without the tag.
fn entries(h: &Host, tag: &str) -> Vec<String> {
    let prefix = format!("{tag}|");
    h.out
        .iter()
        .filter_map(|e| e.strip_prefix(&prefix).map(str::to_owned))
        .collect()
}

fn fmt_params(params: &Params) -> String {
    let items: Vec<String> = params
        .to_array()
        .iter()
        .map(|item| match item {
            Param(v) => v.to_string(),
            ParamsArrayItem::Sub(v) => format!("{v:?}"),
        })
        .collect();
    format!("[{}]", items.join(","))
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

// ---------------------------------------------------------------------------------------------
// describe('Parser init and methods')
// ---------------------------------------------------------------------------------------------

#[test]
fn constructor() {
    let p = P::new();
    assert_eq!(p.transitions, *VT500_TRANSITION_TABLE);
    let p = P::with_transitions(VT500_TRANSITION_TABLE.clone());
    assert_eq!(p.transitions, *VT500_TRANSITION_TABLE);
    let custom = TransitionTable::new(10);
    let p = P::with_transitions(custom.clone());
    assert_eq!(p.transitions, custom);
    assert_eq!(p.transitions.len(), 10);
}

#[test]
fn initial_states() {
    let p = test_parser();
    let h = Host::default();
    assert_eq!(p.initial_state, ParserState::Ground);
    assert_eq!(p.current_state, ParserState::Ground);
    assert_eq!(h.osc, "");
    assert_eq!(p.params.to_array(), arr(&[0]));
    assert_eq!(collect_of(&p), "");
}

#[test]
fn reset_states() {
    let mut p = test_parser();
    let mut h = Host::default();
    p.current_state = ParserState::CsiParam; // upstream: 124 (not a state)
    p.params = params_of(&[123]);
    set_collect(&mut p, "#");
    prime_osc(&mut p, &mut h);
    p.osc_parser.put(&mut h, &[0x78]);
    p.reset(&mut h);
    assert_eq!(p.current_state, ParserState::Ground);
    assert_eq!(p.params.to_array(), arr(&[0]));
    assert_eq!(collect_of(&p), "");
    // the OSC parser was reset too: its next sequence starts clean
    h.osc.clear();
    p.osc_parser.start(&mut h);
    p.osc_parser.put(&mut h, &cps("7;y"));
    p.osc_parser.end(&mut h, true);
    assert_eq!(h.calls, [Call::Osc("7;y, success: true".to_owned())]);
}

// ---------------------------------------------------------------------------------------------
// describe('state transitions and actions')
// ---------------------------------------------------------------------------------------------

/// Execute rules of `state`: every executable stays in `state` and runs the execute handler.
fn check_execute_rules(state: ParserState) {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in exes() {
        p.current_state = state;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, state, "code {c:#x}");
        assert_eq!(h.calls, [Call::Exe(c)], "code {c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

/// 0x7f is ignored in `state`.
fn check_ignore_7f(state: ParserState) {
    let mut p = test_parser();
    let mut h = Host::default();
    p.current_state = state;
    parse_cp(&mut p, &mut h, 0x7f);
    assert_eq!(p.current_state, state);
    assert!(h.calls.is_empty());
}

/// Intermediates 0x20 - 0x2f in `from` collect and move to `to`.
fn check_collect(from: ParserState, to: ParserState) {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x20, 0x30) {
        p.current_state = from;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, to, "code {c:#x}");
        assert_eq!(collect_of(&p), chr(c), "code {c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_ground_execute_action() {
    check_execute_rules(ParserState::Ground);
}

#[test]
fn state_ground_print_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x20, 0x7f) {
        // NOTE: DEL excluded
        p.current_state = ParserState::Ground;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::Ground);
        assert_eq!(h.calls, [Call::Print(chr(c))]);
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_anywhere_to_ground_with_actions() {
    let mut exes_any = vec![0x18, 0x1a];
    exes_any.extend(range(0x80, 0x90));
    exes_any.extend(range(0x91, 0x98));
    exes_any.extend([0x99, 0x9a]);
    let mut p = test_parser();
    let mut h = Host::default();
    for state in ALL_STATES {
        for &c in &exes_any {
            p.current_state = state;
            parse_cp(&mut p, &mut h, c);
            assert_eq!(p.current_state, ParserState::Ground, "{state:?} {c:#x}");
            let expected = match (state, c) {
                (ParserState::OscString, 0x18 | 0x1a) => vec![], // abort OSC_STRING
                (ParserState::DcsPassthrough, 0x18 | 0x1a) => vec![Call::DcsUnhook(false)],
                _ => vec![Call::Exe(c)],
            };
            assert_eq!(h.calls, expected, "{state:?} {c:#x}");
            reset_clear(&mut p, &mut h);
        }
        // Upstream quirk kept: `parser.reset()` above already put the parser back in GROUND, so
        // ST (0x9c) is only ever tried there.
        parse_cp(&mut p, &mut h, 0x9c);
        assert_eq!(p.current_state, ParserState::Ground);
        assert!(h.calls.is_empty());
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_anywhere_to_escape_with_clear() {
    let mut p = test_parser();
    let mut h = Host::default();
    p.reset(&mut h);
    for state in ALL_STATES {
        p.current_state = state;
        p.params = params_of(&[23]);
        set_collect(&mut p, "#");
        parse(&mut p, &mut h, "\x1b");
        assert_eq!(p.current_state, ParserState::Escape, "{state:?}");
        assert_eq!(p.params.to_array(), arr(&[0]), "{state:?}");
        assert_eq!(collect_of(&p), "", "{state:?}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_escape_execute_rules() {
    check_execute_rules(ParserState::Escape);
}

#[test]
fn state_escape_ignore() {
    check_ignore_7f(ParserState::Escape);
}

#[test]
fn trans_escape_to_ground_with_esc_dispatch_action() {
    let mut dispatches = range(0x30, 0x50);
    dispatches.extend(range(0x51, 0x58));
    dispatches.extend([0x59, 0x5a]); // excluded \x5c
    dispatches.extend(range(0x60, 0x7f));
    let mut p = test_parser();
    let mut h = Host::default();
    for c in dispatches {
        p.current_state = ParserState::Escape;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::Ground, "{c:#x}");
        assert_eq!(h.calls, [Call::Esc(String::new(), chr(c))], "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_escape_to_escape_intermediate_with_collect_action() {
    check_collect(ParserState::Escape, ParserState::EscapeIntermediate);
}

#[test]
fn state_escape_intermediate_execute_rules() {
    check_execute_rules(ParserState::EscapeIntermediate);
}

#[test]
fn state_escape_intermediate_ignore() {
    check_ignore_7f(ParserState::EscapeIntermediate);
}

#[test]
fn state_escape_intermediate_collect_action() {
    check_collect(
        ParserState::EscapeIntermediate,
        ParserState::EscapeIntermediate,
    );
}

#[test]
fn trans_escape_intermediate_to_ground_with_esc_dispatch_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x7f) {
        p.current_state = ParserState::EscapeIntermediate;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::Ground, "{c:#x}");
        // '\x5c' --> ESC + \ (7bit ST) parser does not expose this as it already got handled
        let expected = if c == 0x5c {
            vec![]
        } else {
            vec![Call::Esc(String::new(), chr(c))]
        };
        assert_eq!(h.calls, expected, "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_anywhere_escape_to_csi_entry_with_clear() {
    let mut p = test_parser();
    let mut h = Host::default();
    // C0
    p.current_state = ParserState::Escape;
    p.params = params_of(&[123]);
    set_collect(&mut p, "#");
    parse(&mut p, &mut h, "[");
    assert_eq!(p.current_state, ParserState::CsiEntry);
    assert_eq!(p.params.to_array(), arr(&[0]));
    assert_eq!(collect_of(&p), "");
    reset_clear(&mut p, &mut h);
    // C1
    for state in ALL_STATES {
        p.current_state = state;
        p.params = params_of(&[123]);
        set_collect(&mut p, "#");
        parse_cp(&mut p, &mut h, 0x9b);
        assert_eq!(p.current_state, ParserState::CsiEntry, "{state:?}");
        assert_eq!(p.params.to_array(), arr(&[0]), "{state:?}");
        assert_eq!(collect_of(&p), "", "{state:?}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_csi_entry_execute_rules() {
    check_execute_rules(ParserState::CsiEntry);
}

#[test]
fn state_csi_entry_ignore() {
    check_ignore_7f(ParserState::CsiEntry);
}

#[test]
fn trans_csi_entry_to_ground_with_csi_dispatch_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x40, 0x7f) {
        p.current_state = ParserState::CsiEntry;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::Ground, "{c:#x}");
        assert_eq!(
            h.calls,
            [Call::Csi(String::new(), arr(&[0]), chr(c))],
            "{c:#x}"
        );
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_csi_entry_to_csi_param_with_param_collect_actions() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x3a) {
        p.current_state = ParserState::CsiEntry;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::CsiParam);
        assert_eq!(p.params.to_array(), arr(&[i32::try_from(c).unwrap() - 48]));
        reset_clear(&mut p, &mut h);
    }
    p.current_state = ParserState::CsiEntry;
    parse(&mut p, &mut h, ";");
    assert_eq!(p.current_state, ParserState::CsiParam);
    assert_eq!(p.params.to_array(), arr(&[0, 0]));
    reset_clear(&mut p, &mut h);
    for c in [0x3c, 0x3d, 0x3e, 0x3f] {
        p.current_state = ParserState::CsiEntry;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::CsiParam);
        assert_eq!(collect_of(&p), chr(c));
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_csi_param_execute_rules() {
    check_execute_rules(ParserState::CsiParam);
}

#[test]
fn state_csi_param_param_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x3a) {
        p.current_state = ParserState::CsiParam;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::CsiParam);
        assert_eq!(p.params.to_array(), arr(&[i32::try_from(c).unwrap() - 48]));
        reset_clear(&mut p, &mut h);
    }
    p.current_state = ParserState::CsiParam;
    parse(&mut p, &mut h, ";");
    assert_eq!(p.current_state, ParserState::CsiParam);
    assert_eq!(p.params.to_array(), arr(&[0, 0]));
}

#[test]
fn state_csi_param_ignore() {
    check_ignore_7f(ParserState::CsiParam);
}

/// Dispatch of 0x40 - 0x7e from `state` with params `[0, 1]` set.
fn check_csi_dispatch_from(state: ParserState) {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x40, 0x7f) {
        p.current_state = state;
        p.params = params_of(&[0, 1]);
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::Ground, "{c:#x}");
        assert_eq!(
            h.calls,
            [Call::Csi(String::new(), arr(&[0, 1]), chr(c))],
            "{c:#x}"
        );
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_csi_param_to_ground_with_csi_dispatch_action() {
    check_csi_dispatch_from(ParserState::CsiParam);
}

#[test]
fn trans_csi_entry_to_csi_intermediate_with_collect_action() {
    check_collect(ParserState::CsiEntry, ParserState::CsiIntermediate);
}

#[test]
fn trans_csi_param_to_csi_intermediate_with_collect_action() {
    check_collect(ParserState::CsiParam, ParserState::CsiIntermediate);
}

#[test]
fn state_csi_intermediate_execute_rules() {
    check_execute_rules(ParserState::CsiIntermediate);
}

#[test]
fn state_csi_intermediate_collect() {
    check_collect(ParserState::CsiIntermediate, ParserState::CsiIntermediate);
}

#[test]
fn state_csi_intermediate_ignore() {
    check_ignore_7f(ParserState::CsiIntermediate);
}

#[test]
fn trans_csi_intermediate_to_ground_with_csi_dispatch_action() {
    check_csi_dispatch_from(ParserState::CsiIntermediate);
}

#[test]
fn trans_csi_entry_to_csi_param_for_colon() {
    let mut p = test_parser();
    let mut h = Host::default();
    p.current_state = ParserState::CsiEntry;
    parse(&mut p, &mut h, ":");
    assert_eq!(p.current_state, ParserState::CsiParam);
}

#[test]
fn trans_csi_param_to_csi_ignore() {
    let mut p = test_parser();
    let mut h = Host::default();
    // (upstream has this test twice, the second also asserts the params before each run)
    for c in [0x3c, 0x3d, 0x3e, 0x3f] {
        assert_eq!(p.params.to_array(), arr(&[0]));
        p.current_state = ParserState::CsiParam;
        parse(&mut p, &mut h, &format!(";{}", chr(c)));
        assert_eq!(p.current_state, ParserState::CsiIgnore);
        assert_eq!(p.params.to_array(), arr(&[0, 0]));
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_csi_intermediate_to_csi_ignore() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x40) {
        p.current_state = ParserState::CsiIntermediate;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::CsiIgnore, "{c:#x}");
        assert_eq!(p.params.to_array(), arr(&[0]));
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_csi_ignore_execute_rules() {
    check_execute_rules(ParserState::CsiIgnore);
}

#[test]
fn state_csi_ignore_ignore() {
    let mut p = test_parser();
    let mut h = Host::default();
    let mut ignored = range(0x20, 0x40);
    ignored.push(0x7f);
    for c in ignored {
        p.current_state = ParserState::CsiIgnore;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::CsiIgnore, "{c:#x}");
        assert!(h.calls.is_empty());
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_csi_ignore_to_ground() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x40, 0x7f) {
        p.current_state = ParserState::CsiIgnore;
        p.params = params_of(&[0, 1]);
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::Ground, "{c:#x}");
        assert!(h.calls.is_empty(), "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_anywhere_escape_to_sos_pm_apc_string() {
    let mut p = test_parser();
    let mut h = Host::default();
    // C0
    for c in [0x58, 0x5e, 0x5f] {
        parse(&mut p, &mut h, &format!("\x1b{}", chr(c)));
        assert_eq!(p.current_state, ParserState::SosPmApcString, "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
    // C1
    for state in ALL_STATES {
        p.current_state = state;
        for c in [0x98, 0x9e, 0x9f] {
            parse_cp(&mut p, &mut h, c);
            assert_eq!(p.current_state, ParserState::SosPmApcString, "{state:?}");
            reset_clear(&mut p, &mut h);
        }
    }
}

#[test]
fn state_sos_pm_apc_string_ignore_rules() {
    let mut p = test_parser();
    let mut h = Host::default();
    let mut ignored = exes();
    ignored.extend(range(0x20, 0x80));
    for c in ignored {
        p.current_state = ParserState::SosPmApcString;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::SosPmApcString, "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_anywhere_escape_to_osc_string() {
    let mut p = test_parser();
    let mut h = Host::default();
    // C0
    parse(&mut p, &mut h, "\x1b]");
    assert_eq!(p.current_state, ParserState::OscString);
    reset_clear(&mut p, &mut h);
    // C1
    for state in ALL_STATES {
        p.current_state = state;
        parse_cp(&mut p, &mut h, 0x9d);
        assert_eq!(p.current_state, ParserState::OscString, "{state:?}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_osc_string_ignore_rules() {
    let mut p = test_parser();
    let mut h = Host::default();
    // exes() without \x07 (OSC_END): 0x00 - 0x06, 0x08 - 0x17, 0x19, 0x1c - 0x1f
    let ignored: Vec<u32> = exes().into_iter().filter(|&c| c != 0x07).collect();
    for c in ignored {
        prime_osc(&mut p, &mut h);
        p.current_state = ParserState::OscString;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::OscString, "{c:#x}");
        assert_eq!(h.osc, "", "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_osc_string_put_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x20, 0x80) {
        prime_osc(&mut p, &mut h);
        p.current_state = ParserState::OscString;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::OscString, "{c:#x}");
        assert_eq!(h.osc, chr(c), "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_dcs_entry() {
    let mut p = test_parser();
    let mut h = Host::default();
    // C0
    parse(&mut p, &mut h, "\x1bP");
    assert_eq!(p.current_state, ParserState::DcsEntry);
    reset_clear(&mut p, &mut h);
    // C1
    for state in ALL_STATES {
        p.current_state = state;
        parse_cp(&mut p, &mut h, 0x90);
        assert_eq!(p.current_state, ParserState::DcsEntry, "{state:?}");
        reset_clear(&mut p, &mut h);
    }
}

/// 0x00 - 0x17, 0x19, 0x1c - 0x1f and 0x7f stay in `state` and produce nothing.
fn check_dcs_ignore_rules(state: ParserState) {
    let mut p = test_parser();
    let mut h = Host::default();
    let mut ignored = exes();
    ignored.push(0x7f);
    for c in ignored {
        p.current_state = state;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, state, "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_dcs_entry_ignore_rules() {
    check_dcs_ignore_rules(ParserState::DcsEntry);
}

#[test]
fn state_dcs_entry_to_dcs_param_with_param_collect_actions() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x3a) {
        p.current_state = ParserState::DcsEntry;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsParam);
        assert_eq!(p.params.to_array(), arr(&[i32::try_from(c).unwrap() - 48]));
        reset_clear(&mut p, &mut h);
    }
    p.current_state = ParserState::DcsEntry;
    parse(&mut p, &mut h, ";");
    assert_eq!(p.current_state, ParserState::DcsParam);
    assert_eq!(p.params.to_array(), arr(&[0, 0]));
    reset_clear(&mut p, &mut h);
    for c in [0x3c, 0x3d, 0x3e, 0x3f] {
        p.current_state = ParserState::DcsEntry;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsParam);
        assert_eq!(collect_of(&p), chr(c));
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_dcs_param_ignore_rules() {
    check_dcs_ignore_rules(ParserState::DcsParam);
}

#[test]
fn state_dcs_param_param_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x3a) {
        p.current_state = ParserState::DcsParam;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsParam);
        assert_eq!(p.params.to_array(), arr(&[i32::try_from(c).unwrap() - 48]));
        reset_clear(&mut p, &mut h);
    }
    p.current_state = ParserState::DcsParam;
    parse(&mut p, &mut h, ";");
    assert_eq!(p.current_state, ParserState::DcsParam);
    assert_eq!(p.params.to_array(), arr(&[0, 0]));
}

#[test]
fn trans_dcs_entry_to_dcs_param_for_colon() {
    let mut p = test_parser();
    let mut h = Host::default();
    p.current_state = ParserState::DcsEntry;
    parse(&mut p, &mut h, ":");
    assert_eq!(p.current_state, ParserState::DcsParam);
}

#[test]
fn trans_dcs_param_to_dcs_ignore() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in [0x3c, 0x3d, 0x3e, 0x3f] {
        p.current_state = ParserState::DcsParam;
        parse(&mut p, &mut h, &format!(";{}", chr(c)));
        assert_eq!(p.current_state, ParserState::DcsIgnore);
        assert_eq!(p.params.to_array(), arr(&[0, 0]));
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_dcs_intermediate_to_dcs_ignore() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x40) {
        p.current_state = ParserState::DcsIntermediate;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsIgnore, "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_dcs_ignore_ignore_rules() {
    let mut p = test_parser();
    let mut h = Host::default();
    let mut ignored = exes();
    ignored.push(0x7f);
    ignored.extend(range(0x20, 0x80));
    for c in ignored {
        p.current_state = ParserState::DcsIgnore;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsIgnore, "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_dcs_entry_to_dcs_intermediate_with_collect_action() {
    check_collect(ParserState::DcsEntry, ParserState::DcsIntermediate);
}

#[test]
fn trans_dcs_param_to_dcs_intermediate_with_collect_action() {
    check_collect(ParserState::DcsParam, ParserState::DcsIntermediate);
}

#[test]
fn state_dcs_intermediate_ignore_rules() {
    check_dcs_ignore_rules(ParserState::DcsIntermediate);
}

#[test]
fn state_dcs_intermediate_collect_action() {
    check_collect(ParserState::DcsIntermediate, ParserState::DcsIntermediate);
}

#[test]
fn trans_dcs_intermediate_to_dcs_ignore_keeps_collect() {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x30, 0x40) {
        p.current_state = ParserState::DcsIntermediate;
        parse(&mut p, &mut h, &format!("\x20{}", chr(c)));
        assert_eq!(p.current_state, ParserState::DcsIgnore, "{c:#x}");
        assert_eq!(collect_of(&p), "\x20");
        reset_clear(&mut p, &mut h);
    }
}

fn check_hook_from(state: ParserState) {
    let mut p = test_parser();
    let mut h = Host::default();
    for c in range(0x40, 0x7f) {
        p.current_state = state;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsPassthrough, "{c:#x}");
        assert_eq!(h.calls, [Call::DcsHook(arr(&[0]))], "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn trans_dcs_entry_to_dcs_passthrough_with_hook() {
    check_hook_from(ParserState::DcsEntry);
}

#[test]
fn trans_dcs_param_to_dcs_passthrough_with_hook() {
    check_hook_from(ParserState::DcsParam);
}

#[test]
fn trans_dcs_intermediate_to_dcs_passthrough_with_hook() {
    check_hook_from(ParserState::DcsIntermediate);
}

#[test]
fn state_dcs_passthrough_put_action() {
    let mut p = test_parser();
    let mut h = Host::default();
    let mut puts = exes();
    puts.extend(range(0x20, 0x7f));
    for c in puts {
        p.current_state = ParserState::DcsPassthrough;
        parse_cp(&mut p, &mut h, c);
        assert_eq!(p.current_state, ParserState::DcsPassthrough, "{c:#x}");
        assert_eq!(h.calls, [Call::DcsPut(chr(c))], "{c:#x}");
        reset_clear(&mut p, &mut h);
    }
}

#[test]
fn state_dcs_passthrough_ignore() {
    check_ignore_7f(ParserState::DcsPassthrough);
}

// ---------------------------------------------------------------------------------------------
// describe('escape sequence examples')
// ---------------------------------------------------------------------------------------------

/// `test(s, value, noReset)`.
fn example(p: &mut P, h: &mut Host, s: &str, expected: Vec<Call>, no_reset: bool) {
    if !no_reset {
        reset_clear(p, h);
    }
    parse(p, h, s);
    assert_eq!(h.calls, expected, "input {s:?}");
}

fn print(s: &str) -> Call {
    Call::Print(s.to_owned())
}

fn osc_call(s: &str) -> Call {
    Call::Osc(s.to_owned())
}

#[test]
fn example_csi_with_print_and_execute() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "\x1b[<31;5mHello World! öäü€\nabc",
        vec![
            Call::Csi("<".into(), arr(&[31, 5]), "m".into()),
            print("Hello World! öäü€"),
            Call::Exe(0x0a),
            print("abc"),
        ],
        false,
    );
}

#[test]
fn example_osc() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "\x1b]0;abc123€öäü\x07",
        vec![osc_call("0;abc123€öäü, success: true")],
        false,
    );
}

#[test]
fn example_single_dcs() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "\x1bP1;2;3+$aäbc;däe\u{9c}",
        vec![
            Call::DcsHook(arr(&[1, 2, 3])),
            Call::DcsPut("äbc;däe".into()),
            Call::DcsUnhook(true),
        ],
        false,
    );
}

#[test]
fn example_multi_dcs() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "\x1bP1;2;3+$abc;de",
        vec![Call::DcsHook(arr(&[1, 2, 3])), Call::DcsPut("bc;de".into())],
        false,
    );
    h.calls.clear();
    example(
        &mut p,
        &mut h,
        "abc\u{9c}",
        vec![Call::DcsPut("abc".into()), Call::DcsUnhook(true)],
        true,
    );
}

#[test]
fn example_print_dcs_c1() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "abc\u{90}1;2;3+$abc;de\u{9c}",
        vec![
            print("abc"),
            Call::DcsHook(arr(&[1, 2, 3])),
            Call::DcsPut("bc;de".into()),
            Call::DcsUnhook(true),
        ],
        false,
    );
}

#[test]
fn example_print_pm_c1_print() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "abc\u{98}123tzf\u{9c}defg",
        vec![print("abc"), print("defg")],
        false,
    );
}

#[test]
fn example_print_osc_c1_print() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "abc\u{9d}123;tzf\u{9c}defg",
        vec![
            print("abc"),
            osc_call("123;tzf, success: true"),
            print("defg"),
        ],
        false,
    );
}

#[test]
fn example_error_recovery() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "\x1b[1€abcdefg\u{9b}<;c",
        vec![
            print("abcdefg"),
            Call::Csi("<".into(), arr(&[0, 0]), "c".into()),
        ],
        false,
    );
}

#[test]
fn example_7bit_st_should_be_swallowed() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "abc\u{9d}123;tzf\x1b\\defg",
        vec![
            print("abc"),
            osc_call("123;tzf, success: true"),
            print("defg"),
        ],
        false,
    );
}

#[test]
fn example_colon_notation_in_csi_params() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "\x1b[<31;5::123:;8mHello World! öäü€\nabc",
        vec![
            Call::Csi(
                "<".into(),
                vec![
                    Param(31),
                    Param(5),
                    ParamsArrayItem::Sub(vec![-1, 123, -1]),
                    Param(8),
                ],
                "m".into(),
            ),
            print("Hello World! öäü€"),
            Call::Exe(0x0a),
            print("abc"),
        ],
        false,
    );
}

fn colon_dcs_params() -> Vec<ParamsArrayItem> {
    vec![
        Param(1),
        Param(2),
        ParamsArrayItem::Sub(vec![-1, 55]),
        Param(3),
    ]
}

#[test]
fn example_colon_notation_in_dcs_params() {
    let (mut p, mut h) = (test_parser(), Host::default());
    example(
        &mut p,
        &mut h,
        "abc\u{90}1;2::55;3+$abc;de\u{9c}",
        vec![
            print("abc"),
            Call::DcsHook(colon_dcs_params()),
            Call::DcsPut("bc;de".into()),
            Call::DcsUnhook(true),
        ],
        false,
    );
}

#[test]
fn example_can_and_sub_abort_dcs() {
    for abort in ["\x18", "\x1a"] {
        let (mut p, mut h) = (test_parser(), Host::default());
        example(
            &mut p,
            &mut h,
            &format!("abc\u{90}1;2::55;3+$abc;de{abort}"),
            vec![
                print("abc"),
                Call::DcsHook(colon_dcs_params()),
                Call::DcsPut("bc;de".into()),
                Call::DcsUnhook(false), // false for abort
            ],
            false,
        );
    }
}

#[test]
fn example_can_and_sub_abort_osc() {
    for abort in ["\x18", "\x1a"] {
        let (mut p, mut h) = (test_parser(), Host::default());
        example(
            &mut p,
            &mut h,
            &format!("\x1b]0;abc123€öäü{abort}"),
            vec![osc_call("0;abc123€öäü, success: false")],
            false,
        );
    }
}

// ---------------------------------------------------------------------------------------------
// describe('coverage tests')
// ---------------------------------------------------------------------------------------------

#[test]
fn coverage_csi_ignore_error() {
    let (mut p, mut h) = (test_parser(), Host::default());
    p.current_state = ParserState::CsiIgnore;
    parse(&mut p, &mut h, "€öäü");
    assert_eq!(p.current_state, ParserState::CsiIgnore);
    assert!(h.calls.is_empty());
}

#[test]
fn coverage_dcs_ignore_error() {
    let (mut p, mut h) = (test_parser(), Host::default());
    p.current_state = ParserState::DcsIgnore;
    parse(&mut p, &mut h, "€öäü");
    assert_eq!(p.current_state, ParserState::DcsIgnore);
    assert!(h.calls.is_empty());
}

#[test]
fn coverage_dcs_passthrough_error() {
    let (mut p, mut h) = (test_parser(), Host::default());
    p.current_state = ParserState::DcsPassthrough;
    parse(&mut p, &mut h, "\u{90}1;2;3+$a€öäü");
    assert_eq!(p.current_state, ParserState::DcsPassthrough);
    assert_eq!(
        h.calls,
        [Call::DcsHook(arr(&[1, 2, 3])), Call::DcsPut("€öäü".into())]
    );
}

#[test]
fn coverage_error_else_of_if_code_gt_159() {
    let (mut p, mut h) = (test_parser(), Host::default());
    p.current_state = ParserState::Ground;
    parse(&mut p, &mut h, "\u{9c}");
    assert_eq!(p.current_state, ParserState::Ground);
    assert!(h.calls.is_empty());
}

// ---------------------------------------------------------------------------------------------
// describe('set/clear handler')
// ---------------------------------------------------------------------------------------------

const INPUT: &str = "\x1b[1;31mhello \x1b%Gwor\x1bEld!\x1b[0m\r\n$>\x1b]1;foo=bar\x1b\\";

/// A fresh parser without fallbacks (`parser2`).
fn parser2() -> P {
    P::new()
}

fn esc_pusher(tag: &'static str, ret: bool) -> impl FnMut(&mut Host) -> bool + 'static {
    move |h: &mut Host| {
        h.out.push(format!("{tag}|"));
        ret
    }
}

fn csi_pusher(tag: &'static str, ret: bool) -> impl FnMut(&mut Host, &Params) -> bool + 'static {
    move |h: &mut Host, params: &Params| {
        h.out.push(format!("{tag}|m {}", fmt_params(params)));
        ret
    }
}

fn osc_pusher(tag: &'static str, ret: bool) -> OscStringHandler<Host> {
    OscStringHandler::new(move |h: &mut Host, data: &str| {
        h.out.push(format!("{tag}|1 {data}"));
        ret
    })
}

fn dcs_pusher(tag: &'static str, ret: bool) -> DcsStringHandler<Host> {
    DcsStringHandler::new(move |h: &mut Host, data: &str, params: &Params| {
        h.out.push(format!("{tag}|{} {data}", fmt_params(params)));
        ret
    })
}

fn pct_g() -> FunctionIdentifier {
    FunctionIdentifier {
        prefix: None,
        intermediates: "%".to_owned(),
        final_byte: 'G',
    }
}

#[test]
fn set_clear_print_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    let mut printed = String::new();
    p.set_print_handler(|h: &mut Host, data: &[u32]| {
        h.out.push(format!("p|{}", utf32_to_string(data)));
    });
    parse(&mut p, &mut h, INPUT);
    for e in entries(&h, "p") {
        printed.push_str(&e);
    }
    assert_eq!(printed, "hello world!$>");
    p.clear_print_handler();
    p.clear_print_handler(); // should not throw
    h.out.clear();
    parse(&mut p, &mut h, INPUT);
    assert!(h.out.is_empty());
}

#[test]
fn set_clear_esc_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    p.register_esc_handler(pct_g(), esc_pusher("%G", true))
        .unwrap();
    p.register_esc_handler(FunctionIdentifier::new('E'), esc_pusher("E", true))
        .unwrap();
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["%G|", "E|"]);
    p.clear_esc_handler(pct_g()).unwrap();
    p.clear_esc_handler(pct_g()).unwrap(); // should not throw
    h.out.clear();
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["E|"]);
    p.clear_esc_handler(FunctionIdentifier::new('E')).unwrap();
    h.out.clear();
    parse(&mut p, &mut h, INPUT);
    assert!(h.out.is_empty());
}

fn esc_custom_setup(first: bool, second: Option<bool>, third: Option<bool>) -> (P, Host) {
    let (mut p, h) = (parser2(), Host::default());
    p.register_esc_handler(pct_g(), esc_pusher("default - %G", first))
        .unwrap();
    if let Some(ret) = second {
        p.register_esc_handler(pct_g(), esc_pusher("custom - %G", ret))
            .unwrap();
    }
    if let Some(ret) = third {
        p.register_esc_handler(pct_g(), esc_pusher("custom2 - %G", ret))
            .unwrap();
    }
    (p, h)
}

#[test]
fn esc_custom_handlers_prevent_fallback() {
    let (mut p, mut h) = esc_custom_setup(true, Some(true), None);
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["custom - %G|"]);
}

#[test]
fn esc_custom_handlers_allow_fallback() {
    let (mut p, mut h) = esc_custom_setup(true, Some(false), None);
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["custom - %G|", "default - %G|"]);
}

#[test]
fn esc_custom_handlers_multiple_fallback_once() {
    let (mut p, mut h) = esc_custom_setup(true, Some(true), Some(false));
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["custom2 - %G|", "custom - %G|"]);
}

#[test]
fn esc_custom_handlers_multiple_no_fallback() {
    let (mut p, mut h) = esc_custom_setup(true, Some(true), Some(true));
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["custom2 - %G|"]);
}

#[test]
fn esc_custom_handlers_execution_order() {
    let (mut p, mut h) = (parser2(), Host::default());
    for (n, ret) in [(1, true), (2, false), (3, false)] {
        p.register_esc_handler(pct_g(), move |h: &mut Host| {
            h.out.push(n.to_string());
            ret
        })
        .unwrap();
    }
    parse(&mut p, &mut h, "\x1b%G");
    assert_eq!(h.out, ["3", "2", "1"]);
}

#[test]
fn esc_custom_handlers_dispose_works_and_twice() {
    for times in [1, 2] {
        let (mut p, mut h) = (parser2(), Host::default());
        p.register_esc_handler(pct_g(), esc_pusher("default - %G", true))
            .unwrap();
        let dispo = p
            .register_esc_handler(pct_g(), esc_pusher("custom - %G", true))
            .unwrap();
        for _ in 0..times {
            p.dispose_handler(dispo);
        }
        parse(&mut p, &mut h, INPUT);
        assert_eq!(h.out, ["default - %G|"]);
    }
}

#[test]
fn set_clear_csi_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    let m = || FunctionIdentifier::new('m');
    p.register_csi_handler(m(), csi_pusher("csi", true))
        .unwrap();
    parse(&mut p, &mut h, INPUT);
    assert_eq!(entries(&h, "csi"), ["m [1,31]", "m [0]"]);
    p.clear_csi_handler(m()).unwrap();
    p.clear_csi_handler(m()).unwrap(); // should not throw
    h.out.clear();
    parse(&mut p, &mut h, INPUT);
    assert!(h.out.is_empty());
}

/// `csi` (default), `custom`, `custom2` handlers in registration order.
fn csi_custom_setup(rets: &[bool]) -> (P, Host) {
    let (mut p, h) = (parser2(), Host::default());
    for (tag, &ret) in ["csi", "custom", "custom2"].into_iter().zip(rets) {
        p.register_csi_handler(FunctionIdentifier::new('m'), csi_pusher(tag, ret))
            .unwrap();
    }
    (p, h)
}

const SGR_CALLS: [&str; 2] = ["m [1,31]", "m [0]"];

#[test]
fn csi_custom_handlers_prevent_fallback() {
    let (mut p, mut h) = csi_custom_setup(&[true, true]);
    parse(&mut p, &mut h, INPUT);
    assert!(entries(&h, "csi").is_empty(), "should not fall back");
    assert_eq!(entries(&h, "custom"), SGR_CALLS);
}

#[test]
fn csi_custom_handlers_allow_fallback() {
    let (mut p, mut h) = csi_custom_setup(&[true, false]);
    parse(&mut p, &mut h, INPUT);
    assert_eq!(entries(&h, "csi"), SGR_CALLS, "should fall back");
    assert_eq!(entries(&h, "custom"), SGR_CALLS);
}

#[test]
fn csi_custom_handlers_multiple_fallback_once() {
    let (mut p, mut h) = csi_custom_setup(&[true, true, false]);
    parse(&mut p, &mut h, INPUT);
    assert!(entries(&h, "csi").is_empty());
    assert_eq!(entries(&h, "custom"), SGR_CALLS);
    assert_eq!(entries(&h, "custom2"), SGR_CALLS);
}

#[test]
fn csi_custom_handlers_multiple_no_fallback() {
    let (mut p, mut h) = csi_custom_setup(&[true, true, true]);
    parse(&mut p, &mut h, INPUT);
    assert!(entries(&h, "csi").is_empty());
    assert!(entries(&h, "custom").is_empty());
    assert_eq!(entries(&h, "custom2"), SGR_CALLS);
}

#[test]
fn csi_custom_handlers_execution_order() {
    let (mut p, mut h) = (parser2(), Host::default());
    for (n, ret) in [(1, true), (2, false), (3, false)] {
        p.register_csi_handler(FunctionIdentifier::new('m'), move |h: &mut Host, _| {
            h.out.push(n.to_string());
            ret
        })
        .unwrap();
    }
    parse(&mut p, &mut h, "\x1b[0m");
    assert_eq!(h.out, ["3", "2", "1"]);
}

#[test]
fn csi_custom_handlers_dispose_works_and_twice() {
    for times in [1, 2] {
        let (mut p, mut h) = csi_custom_setup(&[true]);
        let custom = p
            .register_csi_handler(FunctionIdentifier::new('m'), csi_pusher("custom", true))
            .unwrap();
        for _ in 0..times {
            p.dispose_handler(custom);
        }
        parse(&mut p, &mut h, INPUT);
        assert_eq!(entries(&h, "csi"), SGR_CALLS);
        assert!(entries(&h, "custom").is_empty(), "disposed handler ran");
    }
}

/// Mirrors the sync half of "sync/async SGR mixed": a chain of fall-through handlers calls
/// newest first, once each, ending at the original.
#[test]
fn csi_fall_through_chain_triples() {
    let (mut p, mut h) = csi_custom_setup(&[true, false, false]);
    parse(&mut p, &mut h, INPUT);
    let order: Vec<&str> = h.out.iter().map(|e| e.split_once('|').unwrap().0).collect();
    assert_eq!(
        order,
        ["custom2", "custom", "csi", "custom2", "custom", "csi"]
    );
}

#[test]
fn set_clear_execute_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    p.set_execute_handler('\n', |h: &mut Host| h.out.push("\n".to_owned()));
    p.set_execute_handler('\r', |h: &mut Host| h.out.push("\r".to_owned()));
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["\r", "\n"]);
    p.clear_execute_handler('\r');
    p.clear_execute_handler('\r'); // should not throw
    h.out.clear();
    parse(&mut p, &mut h, INPUT);
    assert_eq!(h.out, ["\n"]);
}

#[test]
fn set_clear_osc_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    p.register_osc_handler(1, osc_pusher("osc", true));
    parse(&mut p, &mut h, INPUT);
    assert_eq!(entries(&h, "osc"), ["1 foo=bar"]);
    p.clear_osc_handler(1);
    p.clear_osc_handler(1); // should not throw
    h.out.clear();
    parse(&mut p, &mut h, INPUT);
    assert!(h.out.is_empty());
}

fn osc_custom_setup(rets: &[bool]) -> (P, Host) {
    let (mut p, h) = (parser2(), Host::default());
    for (tag, &ret) in ["osc", "custom", "custom2"].into_iter().zip(rets) {
        p.register_osc_handler(1, osc_pusher(tag, ret));
    }
    (p, h)
}

#[test]
fn osc_custom_handlers_prevent_fallback() {
    let (mut p, mut h) = osc_custom_setup(&[true, true]);
    parse(&mut p, &mut h, INPUT);
    assert!(entries(&h, "osc").is_empty(), "should not fall back");
    assert_eq!(entries(&h, "custom"), ["1 foo=bar"]);
}

#[test]
fn osc_custom_handlers_allow_fallback() {
    let (mut p, mut h) = osc_custom_setup(&[true, false]);
    parse(&mut p, &mut h, INPUT);
    assert_eq!(entries(&h, "osc"), ["1 foo=bar"], "should fall back");
    assert_eq!(entries(&h, "custom"), ["1 foo=bar"]);
}

#[test]
fn osc_custom_handlers_multiple_fallback_once() {
    let (mut p, mut h) = osc_custom_setup(&[true, true, false]);
    parse(&mut p, &mut h, INPUT);
    assert!(entries(&h, "osc").is_empty());
    assert_eq!(entries(&h, "custom"), ["1 foo=bar"]);
    assert_eq!(entries(&h, "custom2"), ["1 foo=bar"]);
}

#[test]
fn osc_custom_handlers_multiple_no_fallback() {
    let (mut p, mut h) = osc_custom_setup(&[true, true, true]);
    parse(&mut p, &mut h, INPUT);
    assert!(entries(&h, "osc").is_empty());
    assert!(entries(&h, "custom").is_empty());
    assert_eq!(entries(&h, "custom2"), ["1 foo=bar"]);
}

#[test]
fn osc_custom_handlers_execution_order() {
    let (mut p, mut h) = (parser2(), Host::default());
    for (n, ret) in [(1, true), (2, false), (3, false)] {
        p.register_osc_handler(
            1,
            OscStringHandler::new(move |h: &mut Host, _: &str| {
                h.out.push(n.to_string());
                ret
            }),
        );
    }
    parse(&mut p, &mut h, "\x1b]1;foo=bar\x1b\\");
    assert_eq!(h.out, ["3", "2", "1"]);
}

#[test]
fn osc_custom_handlers_dispose_works_and_twice() {
    for times in [1, 2] {
        let (mut p, mut h) = osc_custom_setup(&[true]);
        let custom = p.register_osc_handler(1, osc_pusher("custom", true));
        for _ in 0..times {
            p.dispose_handler(custom);
        }
        parse(&mut p, &mut h, INPUT);
        assert_eq!(entries(&h, "osc"), ["1 foo=bar"]);
        assert!(entries(&h, "custom").is_empty(), "disposed handler ran");
    }
}

/// Raw `DcsHandler` (upstream's object literal with `hook` / `put` / `unhook`).
struct RawDcs;

impl DcsHandler<Host> for RawDcs {
    fn hook(&mut self, host: &mut Host, params: &Params) {
        host.out.push(format!("hook {}", fmt_params(params)));
    }
    fn put(&mut self, host: &mut Host, data: &[u32]) {
        host.out.push(format!("put {}", utf32_to_string(data)));
    }
    fn unhook(&mut self, host: &mut Host, _success: bool) -> bool {
        host.out.push("unhook".to_owned());
        true
    }
}

#[test]
fn set_clear_dcs_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    let id = || FunctionIdentifier::with_intermediates("+", 'p');
    p.register_dcs_handler(id(), RawDcs).unwrap();
    parse(&mut p, &mut h, "\x1bP1;2;3+pabc");
    parse(&mut p, &mut h, ";de\u{9c}");
    assert_eq!(h.out, ["hook [1,2,3]", "put abc", "put ;de", "unhook"]);
    p.clear_dcs_handler(id()).unwrap();
    p.clear_dcs_handler(id()).unwrap(); // should not throw
    h.out.clear();
    parse(&mut p, &mut h, "\x1bP1;2;3+pabc");
    parse(&mut p, &mut h, ";de\u{9c}");
    assert!(h.out.is_empty());
}

const DCS_INPUT: &str = "\x1bP1;2;3+pabc\x1b\\";

fn dcs_custom_setup(rets: &[bool]) -> (P, Host) {
    let (mut p, h) = (parser2(), Host::default());
    for (tag, &ret) in ["A", "B", "C"].into_iter().zip(rets) {
        p.register_dcs_handler(
            FunctionIdentifier::with_intermediates("+", 'p'),
            dcs_pusher(tag, ret),
        )
        .unwrap();
    }
    (p, h)
}

#[test]
fn dcs_custom_handlers_prevent_fallback() {
    let (mut p, mut h) = dcs_custom_setup(&[true, true]);
    parse(&mut p, &mut h, DCS_INPUT);
    assert_eq!(h.out, ["B|[1,2,3] abc"]);
}

#[test]
fn dcs_custom_handlers_allow_fallback() {
    let (mut p, mut h) = dcs_custom_setup(&[true, false]);
    parse(&mut p, &mut h, DCS_INPUT);
    assert_eq!(h.out, ["B|[1,2,3] abc", "A|[1,2,3] abc"]);
}

#[test]
fn dcs_custom_handlers_multiple_fallback_once() {
    let (mut p, mut h) = dcs_custom_setup(&[true, true, false]);
    parse(&mut p, &mut h, DCS_INPUT);
    assert_eq!(h.out, ["C|[1,2,3] abc", "B|[1,2,3] abc"]);
}

#[test]
fn dcs_custom_handlers_multiple_no_fallback() {
    let (mut p, mut h) = dcs_custom_setup(&[true, true, true]);
    parse(&mut p, &mut h, DCS_INPUT);
    assert_eq!(h.out, ["C|[1,2,3] abc"]);
}

#[test]
fn dcs_custom_handlers_execution_order() {
    let (mut p, mut h) = (parser2(), Host::default());
    for (n, ret) in [(1, true), (2, false), (3, false)] {
        p.register_dcs_handler(
            FunctionIdentifier::with_intermediates("+", 'p'),
            DcsStringHandler::new(move |h: &mut Host, _: &str, _: &Params| {
                h.out.push(n.to_string());
                ret
            }),
        )
        .unwrap();
    }
    parse(&mut p, &mut h, DCS_INPUT);
    assert_eq!(h.out, ["3", "2", "1"]);
}

#[test]
fn dcs_custom_handlers_dispose_works_and_twice() {
    for times in [1, 2] {
        let (mut p, mut h) = dcs_custom_setup(&[true]);
        let dispo = p
            .register_dcs_handler(
                FunctionIdentifier::with_intermediates("+", 'p'),
                dcs_pusher("B", true),
            )
            .unwrap();
        for _ in 0..times {
            p.dispose_handler(dispo);
        }
        parse(&mut p, &mut h, DCS_INPUT);
        assert_eq!(h.out, ["A|[1,2,3] abc"]);
    }
}

#[test]
fn set_clear_error_handler() {
    let (mut p, mut h) = (parser2(), Host::default());
    p.set_error_handler(|h: &mut Host, state: &ParsingState<'_>| {
        h.err = Some(Seen {
            position: state.position,
            code: state.code,
            current_state: state.current_state,
            collect: state.collect,
            params: state.params.to_array(),
            abort: state.abort,
        });
        false
    });
    parse(&mut p, &mut h, "\x1b[1;2;€;3m"); // faulty escape sequence
    assert_eq!(
        h.err,
        Some(Seen {
            position: 6,
            code: 0x20ac,
            current_state: ParserState::CsiParam,
            collect: 0,
            params: arr(&[1, 2, 0]), // extra zero here
            abort: false,
        })
    );
    p.clear_error_handler();
    p.clear_error_handler(); // should not throw
    h.err = None;
    parse(&mut p, &mut h, "\x1b[1;2;a;3m");
    assert_eq!(h.err, None);
}

// ---------------------------------------------------------------------------------------------
// describe('function identifiers') → describe('identifier invocation')
// ---------------------------------------------------------------------------------------------

#[test]
fn identifier_invocation_esc() {
    let (mut p, mut h) = (test_parser(), Host::default());
    let ids: Vec<HandlerId> = [("z", ""), ("!z", "!"), ("!!z", "!!")]
        .into_iter()
        .map(|(tag, inter)| {
            p.register_esc_handler(
                FunctionIdentifier::with_intermediates(inter, 'z'),
                move |h: &mut Host| {
                    h.out.push(tag.to_owned());
                    true
                },
            )
            .unwrap()
        })
        .collect();
    parse(&mut p, &mut h, "\x1bz\x1b!z\x1b!!z");
    for id in ids {
        p.dispose_handler(id);
    }
    parse(&mut p, &mut h, "\x1bz\x1b!z\x1b!!z");
    assert_eq!(h.out, ["z", "!z", "!!z"]);
}

fn six_identifiers() -> [(&'static str, Option<char>, &'static str); 6] {
    [
        ("z", None, ""),
        ("!z", None, "!"),
        ("!!z", None, "!!"),
        ("?z", Some('?'), ""),
        ("?!z", Some('?'), "!"),
        ("?!!z", Some('?'), "!!"),
    ]
}

#[test]
fn identifier_invocation_csi() {
    let (mut p, mut h) = (test_parser(), Host::default());
    let mut ids = Vec::new();
    for (tag, prefix, inter) in six_identifiers() {
        ids.push(
            p.register_csi_handler(
                fi(prefix, inter, 'z'),
                move |h: &mut Host, params: &Params| {
                    h.out.push(format!("{tag} {}", fmt_params(params)));
                    true
                },
            )
            .unwrap(),
        );
    }
    let input = "\x1b[1;z\x1b[1;!z\x1b[1;!!z\x1b[?1;z\x1b[?1;!z\x1b[?1;!!z";
    parse(&mut p, &mut h, input);
    for id in ids {
        p.dispose_handler(id);
    }
    parse(&mut p, &mut h, input);
    assert_eq!(
        h.out,
        [
            "z [1,0]",
            "!z [1,0]",
            "!!z [1,0]",
            "?z [1,0]",
            "?!z [1,0]",
            "?!!z [1,0]"
        ]
    );
}

#[test]
fn identifier_invocation_dcs() {
    let (mut p, mut h) = (test_parser(), Host::default());
    let mut ids = Vec::new();
    for (tag, prefix, inter) in six_identifiers() {
        ids.push(
            p.register_dcs_handler(
                fi(prefix, inter, 'z'),
                DcsStringHandler::new(move |h: &mut Host, data: &str, params: &Params| {
                    h.out.push(format!("{tag} {} {data}", fmt_params(params)));
                    true
                }),
            )
            .unwrap(),
        );
    }
    let input = "\x1bP1;zAB\x1b\\\x1bP1;!zAB\x1b\\\x1bP1;!!zAB\x1b\\\
                 \x1bP?1;zAB\x1b\\\x1bP?1;!zAB\x1b\\\x1bP?1;!!zAB\x1b\\";
    parse(&mut p, &mut h, input);
    for id in ids {
        p.dispose_handler(id);
    }
    parse(&mut p, &mut h, input);
    assert_eq!(
        h.out,
        [
            "z [1,0] AB",
            "!z [1,0] AB",
            "!!z [1,0] AB",
            "?z [1,0] AB",
            "?!z [1,0] AB",
            "?!!z [1,0] AB"
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// describe('EscapeSequenceParser - async') → 'sync handlers should behave as before'
// ---------------------------------------------------------------------------------------------

/// "correct result on sync parse call". The other two cases of the group and every async case
/// are replaced (module docs: plan D3).
#[test]
fn sync_handlers_correct_result_on_sync_parse_call() {
    let input = "\x1b[1;31mhello \x1b%Gwor\x1bEld!\x1b[0m\r\n$>\x1bP1;2axyz\x1b\\\
                 \x1b]1;foo=bar\x1b\\FIN";
    let (mut p, mut h) = (P::new(), Host::default());
    p.reset(&mut h);
    p.set_print_handler(|h: &mut Host, data: &[u32]| {
        h.out.push(format!("PRINT {}", utf32_to_string(data)));
    });
    let sgr = |h: &mut Host, params: &Params| {
        h.out.push(format!("SGR {}", fmt_params(params)));
        true
    };
    p.register_csi_handler(FunctionIdentifier::new('m'), sgr)
        .unwrap();
    p.register_esc_handler(pct_g(), |h: &mut Host| {
        h.out.push("ESC %G".to_owned());
        true
    })
    .unwrap();
    p.register_esc_handler(FunctionIdentifier::new('E'), |h: &mut Host| {
        h.out.push("ESC E".to_owned());
        true
    })
    .unwrap();
    p.set_execute_handler('\r', |h: &mut Host| h.out.push("EXE \r".to_owned()));
    p.set_execute_handler('\n', |h: &mut Host| h.out.push("EXE \n".to_owned()));
    p.register_osc_handler(
        1,
        OscStringHandler::new(|h: &mut Host, data: &str| {
            h.out.push(format!("OSC 1 {data}"));
            true
        }),
    );
    p.register_dcs_handler(
        FunctionIdentifier::new('a'),
        DcsStringHandler::new(|h: &mut Host, data: &str, params: &Params| {
            h.out.push(format!("DCS a {data} {}", fmt_params(params)));
            true
        }),
    )
    .unwrap();
    parse(&mut p, &mut h, input);
    assert_eq!(
        h.out,
        [
            "SGR [1,31]",
            "PRINT hello ",
            "ESC %G",
            "PRINT wor",
            "ESC E",
            "PRINT ld!",
            "SGR [0]",
            "EXE \r",
            "EXE \n",
            "PRINT $>",
            "DCS a xyz [1,2]",
            "OSC 1 foo=bar",
            "PRINT FIN",
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// Rust-only: transition table, parse loop edges, no-panic
// ---------------------------------------------------------------------------------------------

/// `VT500_TRANSITION_TABLE` of xterm.js 6.0.0, dumped from a fresh upstream `EscapeSequenceParser`
/// (`_transitions.table`, via `tools/xterm-oracle/oracle.mjs` `parserInternals()`): one row per
/// state in `ParserState` order, two hex digits (`action << 4 | next_state`) per code 0x00 ..=
/// 0xA0 (0xA0 is NON_ASCII_PRINTABLE). Every other entry upstream is the default 0x10
/// (ERROR, GROUND); the table is 4095 long.
const UPSTREAM_TABLE: [&str; 14] = [
    // GROUND
    "303030303030303030303030303030303030303030303030303030b13030303020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020201030303030303030303030303030303030b930303030303030073030b30048070720",
    // ESCAPE
    "313131313131313131313131313131313131313131313131303130b13131313192929292929292929292929292929292a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0b9a0a0a0a0a0a0a007a0a0b3a0480707a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a00130303030303030303030303030303030b930303030303030073030b30048070710",
    // ESCAPE_INTERMEDIATE
    "323232323232323232323232323232323232323232323232303230b13232323292929292929292929292929292929292a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a00230303030303030303030303030303030b930303030303030073030b30048070710",
    // CSI_ENTRY
    "333333333333333333333333333333333333333333333333303330b13333333395959595959595959595959595959595848484848484848484848484949494947070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070700330303030303030303030303030303030b930303030303030073030b30048070710",
    // CSI_PARAM
    "343434343434343434343434343434343434343434343434303430b13434343495959595959595959595959595959595848484848484848484848484060606067070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070700430303030303030303030303030303030b930303030303030073030b30048070710",
    // CSI_INTERMEDIATE
    "353535353535353535353535353535353535353535353535303530b13535353595959595959595959595959595959595060606060606060606060606060606067070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070700530303030303030303030303030303030b930303030303030073030b30048070710",
    // CSI_IGNORE
    "363636363636363636363636363636363636363636363636303630b13636363606060606060606060606060606060606060606060606060606060606060606060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000630303030303030303030303030303030b930303030303030073030b30048070706",
    // SOS_PM_APC_STRING
    "070707070707070707070707070707070707070707070707300730b10707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070730303030303030303030303030303030b930303030303030073030b30048070710",
    // OSC_STRING
    "080808080808086008080808080808080808080808080808600860600808080858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585858585830303030303030303030303030303030b930303030303030073030b36048070758",
    // DCS_ENTRY
    "090909090909090909090909090909090909090909090909300930b1090909099c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c8a8a8a8a8a8a8a8a8a8a8a8a9a9a9a9acdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd0930303030303030303030303030303030b930303030303030073030b30048070710",
    // DCS_PARAM
    "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a300a30b10a0a0a0a9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c8a8a8a8a8a8a8a8a8a8a8a8a0b0b0b0bcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd0a30303030303030303030303030303030b930303030303030073030b30048070710",
    // DCS_IGNORE
    "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b300b30b10b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b30303030303030303030303030303030b930303030303030073030b3004807070b",
    // DCS_INTERMEDIATE
    "0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c300c30b10c0c0c0c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0bcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd0c30303030303030303030303030303030b930303030303030073030b30048070710",
    // DCS_PASSTHROUGH
    "dddddddddddddddddddddddddddddddddddddddddddddddde0dde0e0dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd0d30303030303030303030303030303030b930303030303030073030b3e0480707dd",
];

#[test]
fn transition_table_matches_upstream_dump() {
    let table = &*VT500_TRANSITION_TABLE;
    assert_eq!(table.len(), 4095);
    assert_eq!(UPSTREAM_TABLE.len(), ALL_STATES.len());
    let mut checked = 0;
    for (state, row) in ALL_STATES.iter().zip(UPSTREAM_TABLE) {
        assert_eq!(row.len(), 161 * 2, "row {state:?}");
        for code in 0..=0xa0_usize {
            let expected = u8::from_str_radix(&row[code * 2..code * 2 + 2], 16).unwrap();
            let actual = table.table[(*state as usize) << 8 | code];
            assert_eq!(actual, expected, "state {state:?} code {code:#x}");
            checked += 1;
        }
    }
    assert_eq!(checked, 14 * 161);
    // everything not covered by the dump is the default entry (ERROR, GROUND)
    let covered = |i: usize| i < 14 * 256 && (i & 0xff) <= 0xa0;
    for (i, &value) in table.table.iter().enumerate() {
        if !covered(i) {
            assert_eq!(value, 0x10, "index {i}");
        }
    }
}

#[test]
fn transition_table_api_matches_upstream_semantics() {
    let mut t = TransitionTable::new(4);
    assert_eq!(t.table, [0, 0, 0, 0]);
    t.set_default(ParserAction::Error, ParserState::Ground);
    assert_eq!(t.table, [0x10; 4]);
    t.add(
        1,
        ParserState::Ground,
        ParserAction::Print,
        ParserState::Escape,
    );
    t.add_many(
        &[2, 3],
        ParserState::Ground,
        ParserAction::Execute,
        ParserState::CsiParam,
    );
    assert_eq!(t.table, [0x10, 0x21, 0x34, 0x34]);
    // out of range writes are dropped, as a Uint8Array drops them
    t.add(
        0,
        ParserState::DcsPassthrough,
        ParserAction::Print,
        ParserState::Ground,
    );
    t.add(
        u32::MAX,
        ParserState::Ground,
        ParserAction::Print,
        ParserState::Ground,
    );
    assert_eq!(t.table, [0x10, 0x21, 0x34, 0x34]);
    assert!(!t.is_empty());
    assert!(TransitionTable::new(0).is_empty());
}

#[test]
fn print_read_ahead_stops_at_controls_del_and_c1() {
    let (mut p, mut h) = (test_parser(), Host::default());
    // SP is printable; DEL (0x7f) and 0x80 - 0x9f end the run; code >= 0xa0 continues it
    p.parse(
        &mut h,
        &[
            0x61, 0x20, 0x62, 0x7f, 0x63, 0x85, 0x64, 0xa0, 0xe9, 0x1f, 0x65,
        ],
    );
    assert_eq!(
        h.calls,
        [
            print("a b"),
            // DEL in GROUND is an ERROR (default error handler ignores it), then GROUND resumes
            print("c"),
            Call::Exe(0x85),
            print("d\u{a0}é"),
            Call::Exe(0x1f),
            print("e"),
        ]
    );
}

#[test]
fn osc_put_read_ahead_includes_del_and_stops_at_controls_and_c1() {
    let (mut p, mut h) = (test_parser(), Host::default());
    parse(&mut p, &mut h, "\x1b]0;a\x7fb c\u{a0}d\x01e\x07");
    // one chunk up to the \x01 control (which the table ignores in OSC_STRING)
    assert_eq!(h.osc_puts, ["a\x7fb c\u{a0}d", "e"]);
    assert_eq!(h.calls, [osc_call("0;a\x7fb c\u{a0}de, success: true")]);
    // 0x85 ends the OSC (C1 executes in every state)
    let (mut p, mut h) = (test_parser(), Host::default());
    p.parse(&mut h, &[0x1b, 0x5d, 0x30, 0x3b, 0x61, 0x62, 0x85, 0x63]);
    assert_eq!(h.osc_puts, ["ab"]);
    assert_eq!(h.calls, [Call::Exe(0x85), print("c")]);
}

#[test]
fn dcs_put_read_ahead_includes_del_and_stops_at_cancel_esc_and_c1() {
    let (mut p, mut h) = (test_parser(), Host::default());
    // DEL is in the chunk (the table ignores it only as a chunk's first code point); 0x85 is a
    // C1 execute that leaves DCS_PASSTHROUGH
    parse(&mut p, &mut h, "\x1bPqab\x7fcd\u{85}ef");
    assert_eq!(
        h.calls,
        [
            Call::DcsHook(arr(&[0])),
            Call::DcsPut("ab\x7fcd".into()),
            Call::Exe(0x85),
            print("ef"),
        ]
    );
    // CAN, SUB and ESC end a chunk and unhook; ESC continues in ESCAPE
    for (end, success) in [("\x18", false), ("\x1a", false), ("\x1b", true)] {
        let (mut p, mut h) = (test_parser(), Host::default());
        parse(&mut p, &mut h, &format!("\x1bPqxy{end}"));
        assert_eq!(
            h.calls,
            [
                Call::DcsHook(arr(&[0])),
                Call::DcsPut("xy".into()),
                Call::DcsUnhook(success)
            ]
        );
        let expected = if end == "\x1b" {
            ParserState::Escape
        } else {
            ParserState::Ground
        };
        assert_eq!(p.current_state, expected);
    }
}

#[test]
fn esc_after_osc_or_dcs_end_enters_escape_and_clears() {
    let (mut p, mut h) = (test_parser(), Host::default());
    parse(&mut p, &mut h, "\x1b]0;x\x1b[4;5H");
    assert_eq!(
        h.calls,
        [
            osc_call("0;x, success: true"),
            Call::Csi(String::new(), arr(&[4, 5]), "H".into())
        ]
    );
    let (mut p, mut h) = (test_parser(), Host::default());
    parse(&mut p, &mut h, "\x1bPqx\x1b[7m");
    assert_eq!(
        h.calls.last(),
        Some(&Call::Csi(String::new(), arr(&[7]), "m".into()))
    );
}

#[test]
fn join_state_is_zeroed_where_upstream_zeroes_it() {
    // zeroed: EXECUTE, CSI_DISPATCH, ESC_DISPATCH, OSC_END, DCS_UNHOOK
    // kept:   PRINT, PARAM, COLLECT, CLEAR, OSC_START/PUT, DCS_HOOK/PUT
    for (input, zeroed) in [
        ("a", false),
        ("\x1b", false),
        ("\x1b[1;2", false),
        ("\x1b[?", false),
        ("\x1b]0;x", false),
        ("\x1bPqx", false),
        ("\n", true),
        ("\x1b[m", true),
        ("\x1bE", true),
        ("\x1b]0;x\x07", true),
        ("\x1bPqx\u{9c}", true),
    ] {
        let (mut p, mut h) = (test_parser(), Host::default());
        h.join = 99;
        parse(&mut p, &mut h, input);
        assert_eq!(h.join == 0, zeroed, "input {input:?}");
    }
}

#[test]
fn error_handler_abort_returns_from_parse_without_changing_state() {
    let (mut p, mut h) = (test_parser(), Host::default());
    p.set_error_handler(|h: &mut Host, state: &ParsingState<'_>| {
        h.out.push(format!("{} {:x}", state.position, state.code));
        true
    });
    // DEL is an error in GROUND; the rest of the chunk is dropped
    parse(&mut p, &mut h, "ab\x7fcd");
    assert_eq!(h.calls, [print("ab")]);
    assert_eq!(h.out, ["2 7f"]);
    assert_eq!(p.current_state, ParserState::Ground);
    // inside a sequence the state before the error is kept (no transition is applied)
    let (mut p, mut h) = (test_parser(), Host::default());
    p.set_error_handler(|_: &mut Host, _: &ParsingState<'_>| true);
    parse(&mut p, &mut h, "\x1b[1\u{20ac}2m");
    assert_eq!(p.current_state, ParserState::CsiParam);
    assert!(h.calls.is_empty());
    // a handler that does not abort lets the parse go on in GROUND
    let (mut p, mut h) = (test_parser(), Host::default());
    parse(&mut p, &mut h, "\x1b[1\u{20ac}2m");
    assert_eq!(h.calls, [print("2m")]);
}

/// Any `u32` is a valid code: 0xA0 and up (including > 0x10FFFF) use the NON_ASCII column.
#[test]
fn any_u32_code_is_handled_in_every_state() {
    let codes = [
        0u32,
        0x1f,
        0x7f,
        0x80,
        0x9f,
        0xa0,
        0xa1,
        0xff,
        0x100,
        0xd800,
        0x10_ffff,
        0x11_0000,
        0x7fff_ffff,
        0x8000_0000,
        u32::MAX,
    ];
    for state in ALL_STATES {
        for code in codes {
            let (mut p, mut h) = (test_parser(), Host::default());
            p.current_state = state;
            p.parse(&mut h, &[code, code, 0x41]);
            p.reset(&mut h);
        }
    }
    // in GROUND a large code is printed like any non-ASCII printable
    let (mut p, mut h) = (P::new(), Host::default());
    p.set_print_handler(|h: &mut Host, data: &[u32]| h.out.push(format!("{data:x?}")));
    p.parse(&mut h, &[u32::MAX, 0x11_0000, 0x41]);
    assert_eq!(h.out, ["[ffffffff, 110000, 41]"]);
}

#[test]
fn many_and_huge_params_do_not_panic_and_stay_bounded() {
    let (mut p, mut h) = (P::new(), Host::default());
    p.register_csi_handler(
        FunctionIdentifier::new('m'),
        |h: &mut Host, params: &Params| {
            h.out.push(format!(
                "{} {} {:?}",
                params.length(),
                params.length() <= params.max_length(),
                params.get(0)
            ));
            true
        },
    )
    .unwrap();
    let digits = "9".repeat(5000);
    parse(&mut p, &mut h, &format!("\x1b[{digits}m"));
    let many = "1;".repeat(1000);
    parse(&mut p, &mut h, &format!("\x1b[{many}m"));
    let subs = "1:".repeat(1000);
    parse(&mut p, &mut h, &format!("\x1b[1;{subs}m"));
    assert_eq!(h.out.len(), 3);
    assert_eq!(h.out[0], "1 true Some(2147483647)");
    assert!(h.out[1].starts_with("32 true"), "{}", h.out[1]);
    // the intermediate collect shifts bits out instead of overflowing
    let (mut p, mut h) = (test_parser(), Host::default());
    parse(&mut p, &mut h, "\x1b[ !\"#$%&'()*+,-./m");
    assert_eq!(h.calls.len(), 1);
}

#[test]
fn custom_table_edges_do_not_panic() {
    // too short: every lookup reads 0 (IGNORE, GROUND)
    let mut p = P::with_transitions(TransitionTable::new(10));
    let mut h = Host::default();
    p.set_print_handler(|h: &mut Host, _: &[u32]| h.out.push("print".to_owned()));
    p.current_state = ParserState::CsiParam;
    p.parse(&mut h, &cps("abc\x1b[1m\u{20ac}"));
    assert!(h.out.is_empty());
    assert_eq!(p.current_state, ParserState::Ground);
    // values past the defined actions / states: no action, GROUND
    let mut t = TransitionTable::new(4095);
    t.table.fill(0xff);
    let mut p = P::with_transitions(t);
    p.parse(&mut h, &cps("abc"));
    assert!(h.out.is_empty());
    assert_eq!(p.current_state, ParserState::Ground);
    // PARAM / COLLECT with a code far outside the digit range saturate instead of overflowing
    let mut t = TransitionTable::new(4095);
    t.add(
        NON_ASCII_PRINTABLE,
        ParserState::Ground,
        ParserAction::Param,
        ParserState::Ground,
    );
    t.add(
        0x41,
        ParserState::Ground,
        ParserAction::Collect,
        ParserState::Ground,
    );
    let mut p = P::with_transitions(t);
    p.parse(&mut h, &[u32::MAX, 0x41, 0x0, 0x41, 0x41, 0x41, 0x41]);
    // digit = saturating(u32::MAX as i32 clamp) - 48, added to the ZDM 0 of an untouched parser
    assert_eq!(p.params.to_array(), arr(&[i32::MAX - 48]));
    assert_eq!(p.collect, 0x4141_4141);
}

#[test]
fn parse_is_incremental_across_chunks() {
    // state, params and collect live in the parser between parse calls
    let whole = "\x1b[<31;5mhi\x1b]0;t\x07\x1bP1;2+qdata\u{9c}";
    let (mut p, mut h) = (test_parser(), Host::default());
    parse(&mut p, &mut h, whole);
    let expected = h.calls.clone();
    let codes = cps(whole);
    for split in 0..=codes.len() {
        let (mut p, mut h) = (test_parser(), Host::default());
        let (a, b) = codes.split_at(split);
        p.parse(&mut h, a);
        p.parse(&mut h, b);
        // print / put chunks may split; compare the concatenated text per kind
        let joined = |calls: &[Call]| {
            let mut out: Vec<Call> = Vec::new();
            for call in calls {
                match (out.last_mut(), call) {
                    (Some(Call::Print(a)), Call::Print(b)) => a.push_str(b),
                    (Some(Call::DcsPut(a)), Call::DcsPut(b)) => a.push_str(b),
                    _ => out.push(call.clone()),
                }
            }
            out
        };
        assert_eq!(joined(&h.calls), joined(&expected), "split at {split}");
    }
}
