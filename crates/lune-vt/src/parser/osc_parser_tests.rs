//! Port of xterm.js src/common/parser/OscParser.test.ts @ f447274 (sync cases), plus Rust-only
//! cases for the handler API (plan M4 step 4).
//!
//! Upstream's handlers push into a closed-over `reports` array; here they push into the host
//! (`Reports`), which every call receives. Async cases ("OscParser - async tests") are
//! replaced: no async handlers (plan decision D3).

#![allow(clippy::unwrap_used)]

use super::*;
use crate::input::text_decoder::utf32_to_string;

#[derive(Debug, Default)]
struct Reports {
    out: Vec<String>,
}

fn to_utf32(s: &str) -> Vec<u32> {
    s.chars().map(u32::from).collect()
}

/// Upstream `TestHandler`.
struct TestHandler {
    id: i32,
    msg: &'static str,
    return_false: bool,
}

fn th(id: i32, msg: &'static str) -> TestHandler {
    TestHandler {
        id,
        msg,
        return_false: false,
    }
}

impl OscHandler<Reports> for TestHandler {
    fn start(&mut self, host: &mut Reports) {
        host.out.push(format!("{} {} START", self.msg, self.id));
    }
    fn put(&mut self, host: &mut Reports, data: &[u32]) {
        host.out.push(format!(
            "{} {} PUT {}",
            self.msg,
            self.id,
            utf32_to_string(data)
        ));
    }
    fn end(&mut self, host: &mut Reports, success: bool) -> bool {
        host.out
            .push(format!("{} {} END {}", self.msg, self.id, success));
        !self.return_false
    }
}

fn parser() -> OscParser<Reports> {
    let mut parser = OscParser::new();
    parser.set_handler_fallback(|host: &mut Reports, id, action| {
        host.out.push(match action {
            OscFallbackAction::Start => format!("{id} START"),
            OscFallbackAction::Put(data) => format!("{id} PUT {}", utf32_to_string(data)),
            OscFallbackAction::End(success) => format!("{id} END {success}"),
        });
    });
    parser
}

fn put(parser: &mut OscParser<Reports>, host: &mut Reports, s: &str) {
    parser.put(host, &to_utf32(s));
}

/// `parser.start(); put(chunks...); parser.end(success)`.
fn run(parser: &mut OscParser<Reports>, host: &mut Reports, chunks: &[&str], success: bool) {
    parser.start(host);
    for chunk in chunks {
        put(parser, host, chunk);
    }
    parser.end(host, success);
}

// describe('identifier parsing')

#[test]
fn identifier_parsing_no_report_for_illegal_ids() {
    let (mut p, mut h) = (parser(), Reports::default());
    put(&mut p, &mut h, "hello world!");
    p.end(&mut h, true);
    assert!(h.out.is_empty());
}

#[test]
fn identifier_parsing_no_payload() {
    let (mut p, mut h) = (parser(), Reports::default());
    run(&mut p, &mut h, &["12", "34"], true);
    assert_eq!(h.out, ["1234 START", "1234 END true"]);
}

#[test]
fn identifier_parsing_with_payload() {
    let (mut p, mut h) = (parser(), Reports::default());
    run(&mut p, &mut h, &["12", "34", ";h", "ello"], true);
    assert_eq!(
        h.out,
        ["1234 START", "1234 PUT h", "1234 PUT ello", "1234 END true"]
    );
}

// describe('handler registration')

#[test]
fn handler_registration_set_osc_handler() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, th(1234, "th"));
    run(&mut p, &mut h, &["1234;Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            "th 1234 START",
            "th 1234 PUT Here comes",
            "th 1234 PUT the mouse!",
            "th 1234 END true"
        ]
    );
}

#[test]
fn handler_registration_clear_osc_handler() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, th(1234, "th"));
    p.clear_handler(1234);
    run(&mut p, &mut h, &["1234;Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            "1234 START",
            "1234 PUT Here comes",
            "1234 PUT the mouse!",
            "1234 END true"
        ]
    );
}

#[test]
fn handler_registration_add_osc_handler() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, th(1234, "th1"));
    p.register_handler(1234, th(1234, "th2"));
    run(&mut p, &mut h, &["1234;Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            "th2 1234 START",
            "th1 1234 START",
            "th2 1234 PUT Here comes",
            "th1 1234 PUT Here comes",
            "th2 1234 PUT the mouse!",
            "th1 1234 PUT the mouse!",
            "th2 1234 END true",
            "th1 1234 END false" // false due being already handled by th2!
        ]
    );
}

#[test]
fn handler_registration_add_osc_handler_with_return_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, th(1234, "th1"));
    p.register_handler(
        1234,
        TestHandler {
            return_false: true,
            ..th(1234, "th2")
        },
    );
    run(&mut p, &mut h, &["1234;Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            "th2 1234 START",
            "th1 1234 START",
            "th2 1234 PUT Here comes",
            "th1 1234 PUT Here comes",
            "th2 1234 PUT the mouse!",
            "th1 1234 PUT the mouse!",
            "th2 1234 END true",
            "th1 1234 END true" // true since th2 indicated to keep bubbling
        ]
    );
}

#[test]
fn handler_registration_dispose_handlers() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, th(1234, "th1"));
    let dispo = p.register_handler(
        1234,
        TestHandler {
            return_false: true,
            ..th(1234, "th2")
        },
    );
    assert!(p.dispose_handler(dispo));
    run(&mut p, &mut h, &["1234;Here comes", "the mouse!"], true);
    assert_eq!(
        h.out,
        [
            "th1 1234 START",
            "th1 1234 PUT Here comes",
            "th1 1234 PUT the mouse!",
            "th1 1234 END true"
        ]
    );
}

// describe('OscHandlerFactory')

fn string_handler(tag: &'static str, ret: bool) -> OscStringHandler<Reports> {
    OscStringHandler::new(move |host: &mut Reports, data: &str| {
        host.out.push(format!("{tag} {data}"));
        ret
    })
}

#[test]
fn osc_handler_factory_should_be_called_once_on_end_true() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, string_handler("1234", true));
    run(&mut p, &mut h, &["1234;Here comes", " the mouse!"], true);
    assert_eq!(h.out, ["1234 Here comes the mouse!"]);
}

#[test]
fn osc_handler_factory_should_not_be_called_on_end_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, string_handler("1234", true));
    run(&mut p, &mut h, &["1234;Here comes", " the mouse!"], false);
    assert!(h.out.is_empty());
}

#[test]
fn osc_handler_factory_should_be_disposable() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, string_handler("one", true));
    let dispo = p.register_handler(1234, string_handler("two", true));
    run(&mut p, &mut h, &["1234;Here comes", " the mouse!"], true);
    assert_eq!(h.out, ["two Here comes the mouse!"]);
    p.dispose_handler(dispo);
    run(&mut p, &mut h, &["1234;some other", " data"], true);
    assert_eq!(h.out, ["two Here comes the mouse!", "one some other data"]);
}

#[test]
fn osc_handler_factory_should_respect_return_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1234, string_handler("one", true));
    p.register_handler(1234, string_handler("two", false));
    run(&mut p, &mut h, &["1234;Here comes", " the mouse!"], true);
    assert_eq!(
        h.out,
        ["two Here comes the mouse!", "one Here comes the mouse!"]
    );
}

/// Records what reached a string handler without keeping a 10 MB report string.
#[derive(Debug, Default)]
struct Received {
    calls: Vec<(usize, String)>,
}

fn limit_parser() -> OscParser<Received> {
    let mut p = OscParser::new();
    p.register_handler(
        1234,
        OscStringHandler::new(|host: &mut Received, data: &str| {
            let first = data.chars().next();
            assert!(data.chars().all(|c| Some(c) == first));
            let first = first.map(String::from).unwrap_or_default();
            host.calls.push((data.encode_utf16().count(), first));
            true
        }),
    );
    p
}

#[test]
fn osc_handler_factory_should_work_up_to_payload_limit() {
    let (mut p, mut h) = (limit_parser(), Received::default());
    p.start(&mut h);
    p.put(&mut h, &to_utf32("1234;"));
    let data = to_utf32(&"A".repeat(1000));
    for _ in (0..PAYLOAD_LIMIT).step_by(1000) {
        p.put(&mut h, &data);
    }
    p.end(&mut h, true);
    assert_eq!(h.calls, [(PAYLOAD_LIMIT, "A".to_owned())]);
}

#[test]
fn osc_handler_factory_should_abort_for_payload_limit_plus_1() {
    let (mut p, mut h) = (limit_parser(), Received::default());
    p.start(&mut h);
    p.put(&mut h, &to_utf32("1234;"));
    let data = to_utf32(&"A".repeat(1000));
    for _ in (0..PAYLOAD_LIMIT).step_by(1000) {
        p.put(&mut h, &data);
    }
    p.put(&mut h, &to_utf32("A"));
    p.end(&mut h, true);
    assert!(h.calls.is_empty());
}

// Rust-only cases.

/// The limit counts UTF-16 units, like upstream's `string.length`: `PAYLOAD_LIMIT / 2` astral
/// characters (4 UTF-8 bytes, 2 UTF-16 units each) are exactly at the limit and reach the
/// handler; a byte count would have dropped them at a quarter of that.
#[test]
fn payload_limit_counts_utf16_units_with_astral_chars() {
    let astral = to_utf32(&"\u{1F600}".repeat(1000)); // 2000 units per chunk
    let run_astral = |extra: &str| {
        let (mut p, mut h) = (limit_parser(), Received::default());
        p.start(&mut h);
        p.put(&mut h, &to_utf32("1234;"));
        for _ in (0..PAYLOAD_LIMIT).step_by(2000) {
            p.put(&mut h, &astral);
        }
        p.put(&mut h, &to_utf32(extra));
        p.end(&mut h, true);
        h.calls
    };
    assert_eq!(run_astral(""), [(PAYLOAD_LIMIT, "\u{1F600}".to_owned())]);
    assert!(run_astral("A").is_empty());
}

/// After the limit is hit the buffer is released and stays empty for the rest of the
/// sequence; the next sequence starts fresh.
#[test]
fn payload_limit_releases_buffer_and_next_sequence_works() {
    let mut handler = OscStringHandler::new(|host: &mut Received, data: &str| {
        host.calls.push((data.len(), data.to_owned()));
        true
    });
    let mut h = Received::default();
    handler.start(&mut h);
    let chunk = to_utf32(&"A".repeat(PAYLOAD_LIMIT + 1));
    handler.put(&mut h, &chunk);
    assert!(handler.hit_limit);
    assert_eq!(handler.data.capacity(), 0);
    handler.put(&mut h, &to_utf32("more"));
    assert_eq!(handler.data.capacity(), 0);
    assert!(!handler.end(&mut h, true));
    assert!(h.calls.is_empty());
    handler.start(&mut h);
    handler.put(&mut h, &to_utf32("ok"));
    assert!(handler.end(&mut h, true));
    assert_eq!(h.calls, [(2, "ok".to_owned())]);
}

/// A 400-digit id saturates at `i32::MAX` without overflowing (debug builds check overflow).
#[test]
fn osc_id_saturates_without_panic() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(i32::MAX - 1, th(0, "never"));
    let digits = "9".repeat(400);
    run(&mut p, &mut h, &[&digits, ";x"], true);
    let max = i32::MAX;
    assert_eq!(
        h.out,
        [
            format!("{max} START"),
            format!("{max} PUT x"),
            format!("{max} END true")
        ]
    );
    // only a handler registered at i32::MAX matches
    p.register_handler(i32::MAX, th(max, "max"));
    h.out.clear();
    run(&mut p, &mut h, &[&digits, ";x"], true);
    assert_eq!(
        h.out,
        [
            format!("max {max} START"),
            format!("max {max} PUT x"),
            format!("max {max} END true")
        ]
    );
}

#[test]
fn dispose_twice_is_a_no_op() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1, th(1, "a"));
    let id = p.register_handler(1, th(1, "b"));
    assert!(p.dispose_handler(id));
    assert!(!p.dispose_handler(id));
    run(&mut p, &mut h, &["1;x"], true);
    assert_eq!(h.out, ["a 1 START", "a 1 PUT x", "a 1 END true"]);
}

/// `reset` during a payload ends every active handler with `false` (upstream `reset`).
#[test]
fn reset_during_payload_ends_handlers_with_false() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1, th(1, "a"));
    p.register_handler(1, th(1, "b"));
    p.start(&mut h);
    put(&mut p, &mut h, "1;x");
    p.reset(&mut h);
    assert_eq!(
        h.out,
        [
            "b 1 START",
            "a 1 START",
            "b 1 PUT x",
            "a 1 PUT x",
            "b 1 END false",
            "a 1 END false"
        ]
    );
}

/// Upstream's `_active` is the registry array itself; `clearHandler` mid-sequence deletes the
/// registry entry but the running sequence keeps its handlers until `end`.
#[test]
fn clear_mid_sequence_keeps_running_handlers_until_end() {
    let (mut p, mut h) = (parser(), Reports::default());
    p.register_handler(1, th(1, "a"));
    p.start(&mut h);
    put(&mut p, &mut h, "1;x");
    p.clear_handler(1);
    put(&mut p, &mut h, "y");
    p.end(&mut h, true);
    run(&mut p, &mut h, &["1;z"], true);
    assert_eq!(
        h.out,
        [
            "a 1 START",
            "a 1 PUT x",
            "a 1 PUT y",
            "a 1 END true",
            "1 START",
            "1 PUT z",
            "1 END true"
        ]
    );
}
