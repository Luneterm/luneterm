//! Port of xterm.js src/common/parser/DcsParser.test.ts @ f447274 (sync cases), plus Rust-only
//! cases for the handler API (plan M4 step 4).
//!
//! Upstream's handlers push into a closed-over `reports` array; here they push into the host
//! (`Reports`). The test-local `identifier` copy of `_identifier` is the shared
//! `escape_sequence_parser::identifier`. Async cases ("DcsParser - async tests") are replaced:
//! no async handlers (plan decision D3).

#![allow(clippy::unwrap_used)]

use super::*;
use crate::input::text_decoder::utf32_to_string;
use crate::parser::escape_sequence_parser::{CSI_FINAL_RANGE, identifier};
use crate::parser::params::ParamsArrayItem::Param;
use crate::parser::types::FunctionIdentifier;

#[derive(Debug, Default)]
struct Reports {
    out: Vec<String>,
}

fn to_utf32(s: &str) -> Vec<u32> {
    s.chars().map(u32::from).collect()
}

fn fmt_params(params: &Params) -> String {
    format!("{:?}", params.to_array())
}

/// `identifier({ intermediates: '+', final: 'p' })`.
fn plus_p() -> u32 {
    identifier(
        &FunctionIdentifier::with_intermediates("+", 'p'),
        CSI_FINAL_RANGE,
    )
    .unwrap()
}

fn params_123() -> Params {
    Params::from_array(&[Param(1), Param(2), Param(3)]).unwrap()
}

/// Upstream `TestHandler`.
struct TestHandler {
    msg: &'static str,
    return_false: bool,
}

fn th(msg: &'static str) -> TestHandler {
    TestHandler {
        msg,
        return_false: false,
    }
}

impl DcsHandler<Reports> for TestHandler {
    fn hook(&mut self, host: &mut Reports, params: &Params) {
        host.out
            .push(format!("{} HOOK {}", self.msg, fmt_params(params)));
    }
    fn put(&mut self, host: &mut Reports, data: &[u32]) {
        host.out
            .push(format!("{} PUT {}", self.msg, utf32_to_string(data)));
    }
    fn unhook(&mut self, host: &mut Reports, success: bool) -> bool {
        host.out.push(format!("{} UNHOOK {}", self.msg, success));
        !self.return_false
    }
}

fn parser() -> DcsParser<Reports> {
    let mut parser = DcsParser::new();
    parser.set_handler_fallback(|host: &mut Reports, id, action| {
        host.out.push(match action {
            DcsFallbackAction::Hook(params) => format!("{id} HOOK {}", fmt_params(params)),
            DcsFallbackAction::Put(data) => format!("{id} PUT {}", utf32_to_string(data)),
            DcsFallbackAction::Unhook(success) => format!("{id} UNHOOK {success}"),
        });
    });
    parser
}

/// `hook(+p, [1, 2, 3]); put(chunks...); unhook(success)`.
fn run(parser: &mut DcsParser<Reports>, host: &mut Reports, chunks: &[&str], success: bool) {
    parser.hook(host, plus_p(), &params_123());
    for chunk in chunks {
        parser.put(host, &to_utf32(chunk));
    }
    parser.unhook(host, success);
}

const P123: &str = "[Param(1), Param(2), Param(3)]";

// describe('handler registration')

#[test]
fn handler_registration_set_dcs_handler() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("th"));
    run(&mut p, &mut h, &["Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            format!("th HOOK {P123}"),
            "th PUT Here comes".to_owned(),
            "th PUT the mouse!".to_owned(),
            "th UNHOOK true".to_owned()
        ]
    );
}

#[test]
fn handler_registration_clear_dcs_handler() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("th"));
    p.clear_handler(plus_p());
    run(&mut p, &mut h, &["Here comes", "the mouse!"], true);
    let id = plus_p();
    assert_eq!(
        h.out,
        [
            format!("{id} HOOK {P123}"),
            format!("{id} PUT Here comes"),
            format!("{id} PUT the mouse!"),
            format!("{id} UNHOOK true")
        ]
    );
}

#[test]
fn handler_registration_add_dcs_handler() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("th1"));
    p.register_handler(plus_p(), th("th2"));
    run(&mut p, &mut h, &["Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            format!("th2 HOOK {P123}"),
            format!("th1 HOOK {P123}"),
            "th2 PUT Here comes".to_owned(),
            "th1 PUT Here comes".to_owned(),
            "th2 PUT the mouse!".to_owned(),
            "th1 PUT the mouse!".to_owned(),
            "th2 UNHOOK true".to_owned(),
            "th1 UNHOOK false".to_owned() // false due being already handled by th2!
        ]
    );
}

#[test]
fn handler_registration_add_dcs_handler_with_return_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("th1"));
    p.register_handler(
        plus_p(),
        TestHandler {
            return_false: true,
            ..th("th2")
        },
    );
    run(&mut p, &mut h, &["Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            format!("th2 HOOK {P123}"),
            format!("th1 HOOK {P123}"),
            "th2 PUT Here comes".to_owned(),
            "th1 PUT Here comes".to_owned(),
            "th2 PUT the mouse!".to_owned(),
            "th1 PUT the mouse!".to_owned(),
            "th2 UNHOOK true".to_owned(),
            "th1 UNHOOK true".to_owned() // true since th2 indicated to keep bubbling
        ]
    );
}

#[test]
fn handler_registration_dispose_handlers() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("th1"));
    let dispo = p.register_handler(
        plus_p(),
        TestHandler {
            return_false: true,
            ..th("th2")
        },
    );
    p.dispose_handler(dispo);
    run(&mut p, &mut h, &["Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            format!("th1 HOOK {P123}"),
            "th1 PUT Here comes".to_owned(),
            "th1 PUT the mouse!".to_owned(),
            "th1 UNHOOK true".to_owned()
        ]
    );
}

// describe('DcsHandlerFactory')

fn string_handler(tag: &'static str, ret: bool) -> DcsStringHandler<Reports> {
    DcsStringHandler::new(move |host: &mut Reports, data: &str, params: &Params| {
        host.out
            .push(format!("{tag} {} {data}", fmt_params(params)));
        ret
    })
}

#[test]
fn dcs_handler_factory_should_be_called_once_on_end_true() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), string_handler("-", true));
    run(&mut p, &mut h, &["Here comes", " the mouse!"], true);
    assert_eq!(h.out, [format!("- {P123} Here comes the mouse!")]);
}

#[test]
fn dcs_handler_factory_should_not_be_called_on_end_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), string_handler("-", true));
    run(&mut p, &mut h, &["Here comes", " the mouse!"], false);
    assert!(h.out.is_empty());
}

#[test]
fn dcs_handler_factory_should_be_disposable() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), string_handler("one", true));
    let dispo = p.register_handler(plus_p(), string_handler("two", true));
    run(&mut p, &mut h, &["Here comes", " the mouse!"], true);
    assert_eq!(h.out, [format!("two {P123} Here comes the mouse!")]);
    p.dispose_handler(dispo);
    run(&mut p, &mut h, &["some other", " data"], true);
    assert_eq!(
        h.out,
        [
            format!("two {P123} Here comes the mouse!"),
            format!("one {P123} some other data")
        ]
    );
}

#[test]
fn dcs_handler_factory_should_respect_return_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), string_handler("one", true));
    p.register_handler(plus_p(), string_handler("two", false));
    run(&mut p, &mut h, &["Here comes", " the mouse!"], true);
    assert_eq!(
        h.out,
        [
            format!("two {P123} Here comes the mouse!"),
            format!("one {P123} Here comes the mouse!")
        ]
    );
}

/// Records what reached a string handler without keeping a 10 MB report string.
#[derive(Debug, Default)]
struct Received {
    calls: Vec<(String, usize)>,
}

fn limit_parser() -> DcsParser<Received> {
    let mut p = DcsParser::new();
    p.register_handler(
        plus_p(),
        DcsStringHandler::new(|host: &mut Received, data: &str, params: &Params| {
            host.calls
                .push((fmt_params(params), data.encode_utf16().count()));
            true
        }),
    );
    p
}

#[test]
fn dcs_handler_factory_should_work_up_to_payload_limit() {
    let (mut p, mut h) = (limit_parser(), Received::default());
    p.hook(&mut h, plus_p(), &params_123());
    let data = to_utf32(&"A".repeat(1000));
    for _ in (0..PAYLOAD_LIMIT).step_by(1000) {
        p.put(&mut h, &data);
    }
    p.unhook(&mut h, true);
    assert_eq!(h.calls, [(P123.to_owned(), PAYLOAD_LIMIT)]);
}

#[test]
fn dcs_handler_factory_should_abort_for_payload_limit_plus_1() {
    let (mut p, mut h) = (limit_parser(), Received::default());
    p.hook(&mut h, plus_p(), &params_123());
    let data = to_utf32(&"A".repeat(1000));
    for _ in (0..PAYLOAD_LIMIT).step_by(1000) {
        p.put(&mut h, &data);
    }
    p.put(&mut h, &to_utf32("A"));
    p.unhook(&mut h, true);
    assert!(h.calls.is_empty());
}

// Rust-only cases.

/// The limit counts UTF-16 units (upstream `string.length`), not bytes.
#[test]
fn payload_limit_counts_utf16_units_with_astral_chars() {
    let astral = to_utf32(&"\u{10348}".repeat(1000)); // 2000 units, 4000 bytes per chunk
    for (extra, expected) in [("", Some(PAYLOAD_LIMIT)), ("A", None)] {
        let (mut p, mut h) = (limit_parser(), Received::default());
        p.hook(&mut h, plus_p(), &params_123());
        for _ in (0..PAYLOAD_LIMIT).step_by(2000) {
            p.put(&mut h, &astral);
        }
        p.put(&mut h, &to_utf32(extra));
        p.unhook(&mut h, true);
        let got = h.calls.first().map(|(_, len)| *len);
        assert_eq!(got, expected, "extra {extra:?}");
    }
}

/// The string handler keeps the params it saw at hook time, although the parser reuses and
/// changes its params before `unhook`.
#[test]
fn string_handler_sees_hook_time_params_after_parser_params_change() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), string_handler("-", true));
    let mut params = params_123();
    p.hook(&mut h, plus_p(), &params);
    params.reset();
    params.add_param(0).unwrap();
    params.add_param(99).unwrap();
    p.put(&mut h, &to_utf32("abc"));
    p.unhook(&mut h, true);
    assert_eq!(h.out, [format!("- {P123} abc")]);
}

/// Empty params (`[0]`, ZDM) take upstream's `EMPTY_PARAMS` path; the handler still sees `[0]`.
#[test]
fn string_handler_empty_params_are_zdm_zero() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), string_handler("-", true));
    let params = Params::from_array(&[Param(0)]).unwrap();
    p.hook(&mut h, plus_p(), &params);
    p.unhook(&mut h, true);
    assert_eq!(h.out, ["- [Param(0)] "]);
}

#[test]
fn dispose_twice_is_a_no_op() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("a"));
    let id = p.register_handler(plus_p(), th("b"));
    assert!(p.dispose_handler(id));
    assert!(!p.dispose_handler(id));
    run(&mut p, &mut h, &["x"], true);
    assert_eq!(
        h.out,
        [
            format!("a HOOK {P123}"),
            "a PUT x".to_owned(),
            "a UNHOOK true".to_owned()
        ]
    );
}

/// A new hook unhooks leftover handlers with `false` first (upstream `hook` calls `reset`).
#[test]
fn hook_resets_leftover_handlers() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(plus_p(), th("a"));
    p.hook(&mut h, plus_p(), &params_123());
    p.hook(&mut h, plus_p(), &params_123());
    assert_eq!(
        h.out,
        [
            format!("a HOOK {P123}"),
            "a UNHOOK false".to_owned(),
            format!("a HOOK {P123}")
        ]
    );
}
