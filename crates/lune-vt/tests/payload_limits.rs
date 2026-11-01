//! Payload and parameter limits of the escape sequence parser, through the OSC/DCS string
//! handlers: exactly `PAYLOAD_LIMIT` UTF-16 units is delivered, one more is dropped (the
//! handler is not called and the parser recovers); params and sub-params stay within 32;
//! absurdly long digit runs neither panic nor grow anything.
//!
//! Payload sizes are 10 million units, so these run in the default (debug) `cargo test` in a
//! second or two each; input is fed in 64 KiB chunks like a PTY read.

use lune_vt::input::text_decoder::Utf8ToUtf32;
use lune_vt::parser::constants::PAYLOAD_LIMIT;
use lune_vt::parser::dcs_parser::DcsStringHandler;
use lune_vt::parser::escape_sequence_parser::EscapeSequenceParser;
use lune_vt::parser::osc_parser::OscStringHandler;
use lune_vt::parser::params::{MAX_VALUE, Params, ParamsArrayItem};
use lune_vt::parser::types::{FunctionIdentifier, OscFallbackAction, ParserHost};

const CHUNK: usize = 64 * 1024;

#[derive(Debug, Default)]
struct Host {
    join: u32,
    /// `(UTF-16 units, first char, last char)` of each payload a string handler received.
    delivered: Vec<(usize, char, char)>,
    /// `(id, success)` of each OSC that reached the fallback.
    osc_fallback_ends: Vec<(i32, bool)>,
    /// Params seen by CSI handlers: (length, array).
    csi: Vec<(usize, Vec<ParamsArrayItem>)>,
    /// Params seen by the DCS string handler.
    dcs_params: Vec<Vec<ParamsArrayItem>>,
}

impl ParserHost for Host {
    fn preceding_join_state(&mut self) -> &mut u32 {
        &mut self.join
    }
}

fn units(payload: &str) -> usize {
    payload.chars().map(char::len_utf16).sum()
}

fn note(host: &mut Host, payload: &str) {
    let first = payload.chars().next().unwrap_or('\0');
    let last = payload.chars().next_back().unwrap_or('\0');
    host.delivered.push((units(payload), first, last));
}

/// OSC 0 and DCS `$ q` through the string handlers.
fn parser() -> EscapeSequenceParser<Host> {
    let mut p = EscapeSequenceParser::<Host>::new();
    p.register_osc_handler(
        0,
        OscStringHandler::new(|h: &mut Host, payload: &str| {
            note(h, payload);
            true
        }),
    );
    let registered = p.register_dcs_handler(
        FunctionIdentifier::with_intermediates("$", 'q'),
        DcsStringHandler::new(|h: &mut Host, payload: &str, params: &Params| {
            note(h, payload);
            h.dcs_params.push(params.to_array());
            true
        }),
    );
    assert!(registered.is_ok());
    let m = p.register_csi_handler(
        FunctionIdentifier::new('m'),
        |h: &mut Host, params: &Params| {
            h.csi.push((params.length(), params.to_array()));
            true
        },
    );
    assert!(m.is_ok());
    p.set_osc_handler_fallback(|h: &mut Host, id, action| {
        if let OscFallbackAction::End(success) = action {
            h.osc_fallback_ends.push((id, success));
        }
    });
    p
}

fn cps(bytes: &[u8]) -> Vec<u32> {
    bytes.iter().map(|&b| u32::from(b)).collect()
}

/// Feeds `payload` code points in 64 KiB chunks.
fn feed(p: &mut EscapeSequenceParser<Host>, h: &mut Host, data: &[u32]) {
    for chunk in data.chunks(CHUNK) {
        p.parse(h, chunk);
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Osc,
    Dcs,
}

/// `OSC 0 ; payload ST` or `DCS $ q payload ST`, payload built from `body` code points.
fn string_sequence(kind: Kind, body: &[u32]) -> Vec<u32> {
    let mut data = match kind {
        Kind::Osc => cps(b"\x1b]0;"),
        Kind::Dcs => cps(b"\x1bP$q"),
    };
    data.extend_from_slice(body);
    data.extend(cps(b"\x1b\\"));
    data
}

const BMP: u32 = 0x4E2D; // one UTF-16 unit
const ASTRAL: u32 = 0x1F600; // two UTF-16 units

/// Runs one sequence, then a short normal one, and returns the host.
fn run(kind: Kind, body: &[u32]) -> Host {
    let mut p = parser();
    let mut h = Host::default();
    feed(&mut p, &mut h, &string_sequence(kind, body));
    // The parser must be usable afterwards.
    feed(&mut p, &mut h, &string_sequence(kind, &cps(b"after")));
    h
}

fn expect_delivered(h: &Host, first: (usize, char, char)) {
    assert_eq!(
        h.delivered.len(),
        2,
        "limit payload and the follow-up both delivered"
    );
    assert_eq!(h.delivered.first().copied(), Some(first));
    assert_eq!(h.delivered.get(1).copied(), Some((5, 'a', 'r')));
}

fn expect_dropped(h: &Host) {
    assert_eq!(
        h.delivered.len(),
        1,
        "over-limit payload dropped, follow-up delivered: {:?}",
        h.delivered
    );
    assert_eq!(h.delivered.first().copied(), Some((5, 'a', 'r')));
}

fn bmp_body(n: usize) -> Vec<u32> {
    vec![BMP; n]
}

/// `PAYLOAD_LIMIT / 2` astral chars plus `extra_bmp` BMP chars: `PAYLOAD_LIMIT + extra_bmp` units.
fn astral_body(extra_bmp: usize) -> Vec<u32> {
    let mut body = vec![ASTRAL; PAYLOAD_LIMIT / 2];
    body.extend(std::iter::repeat_n(BMP, extra_bmp));
    body
}

const BMP_CHAR: char = '\u{4E2D}';
const ASTRAL_CHAR: char = '\u{1F600}';

#[test]
fn limit_is_ten_million_utf16_units() {
    assert_eq!(PAYLOAD_LIMIT, 10_000_000);
    assert_eq!(
        PAYLOAD_LIMIT % 2,
        0,
        "the astral cases assume an even limit"
    );
}

#[test]
fn osc_bmp_at_limit_is_delivered() {
    let h = run(Kind::Osc, &bmp_body(PAYLOAD_LIMIT));
    expect_delivered(&h, (PAYLOAD_LIMIT, BMP_CHAR, BMP_CHAR));
}

#[test]
fn osc_bmp_over_limit_is_dropped() {
    let h = run(Kind::Osc, &bmp_body(PAYLOAD_LIMIT + 1));
    expect_dropped(&h);
}

#[test]
fn osc_astral_at_limit_is_delivered() {
    let h = run(Kind::Osc, &astral_body(0));
    expect_delivered(&h, (PAYLOAD_LIMIT, ASTRAL_CHAR, ASTRAL_CHAR));
}

#[test]
fn osc_astral_one_unit_over_limit_is_dropped() {
    // PAYLOAD_LIMIT / 2 astral chars + 1 BMP char = PAYLOAD_LIMIT + 1 units.
    let h = run(Kind::Osc, &astral_body(1));
    expect_dropped(&h);
}

#[test]
fn osc_astral_two_units_over_limit_is_dropped() {
    let mut body = astral_body(0);
    body.push(ASTRAL);
    let h = run(Kind::Osc, &body);
    expect_dropped(&h);
}

#[test]
fn dcs_bmp_at_limit_is_delivered() {
    let h = run(Kind::Dcs, &bmp_body(PAYLOAD_LIMIT));
    expect_delivered(&h, (PAYLOAD_LIMIT, BMP_CHAR, BMP_CHAR));
}

#[test]
fn dcs_bmp_over_limit_is_dropped() {
    let h = run(Kind::Dcs, &bmp_body(PAYLOAD_LIMIT + 1));
    expect_dropped(&h);
}

#[test]
fn dcs_astral_at_limit_is_delivered() {
    let h = run(Kind::Dcs, &astral_body(0));
    expect_delivered(&h, (PAYLOAD_LIMIT, ASTRAL_CHAR, ASTRAL_CHAR));
}

#[test]
fn dcs_astral_one_unit_over_limit_is_dropped() {
    let h = run(Kind::Dcs, &astral_body(1));
    expect_dropped(&h);
}

#[test]
fn dcs_astral_two_units_over_limit_is_dropped() {
    let mut body = astral_body(0);
    body.push(ASTRAL);
    let h = run(Kind::Dcs, &body);
    expect_dropped(&h);
}

/// The same limit through the UTF-8 decoder in 64 KiB byte chunks (the path a PTY read takes),
/// with multi-byte characters straddling the chunk boundaries.
#[test]
fn osc_limit_through_the_decoder_with_straddling_chars() {
    for (extra, delivered) in [(0usize, true), (1, false)] {
        let mut bytes = b"\x1b]0;".to_vec();
        let mut body = String::new();
        body.extend(std::iter::repeat_n(BMP_CHAR, PAYLOAD_LIMIT + extra));
        bytes.extend_from_slice(body.as_bytes());
        bytes.extend_from_slice(b"\x1b\\");
        let mut p = parser();
        let mut h = Host::default();
        let mut decoder = Utf8ToUtf32::new();
        let mut scratch = Vec::new();
        for chunk in bytes.chunks(CHUNK) {
            decoder.decode(chunk, &mut scratch);
            p.parse(&mut h, &scratch);
        }
        assert_eq!(h.delivered.len(), usize::from(delivered), "extra {extra}");
        if delivered {
            assert_eq!(
                h.delivered.first().copied(),
                Some((PAYLOAD_LIMIT, BMP_CHAR, BMP_CHAR))
            );
        }
    }
}

/// Dropping the payload does not leak state into the next sequence: after an over-limit OSC the
/// handler is idle and a following OSC (with its own start) works, and an OSC with no handler at
/// the same time still reaches the fallback.
#[test]
fn over_limit_osc_leaves_the_parser_usable() {
    let mut p = parser();
    let mut h = Host::default();
    feed(
        &mut p,
        &mut h,
        &string_sequence(Kind::Osc, &bmp_body(PAYLOAD_LIMIT + 5)),
    );
    feed(&mut p, &mut h, &cps(b"\x1b]7;file\x07\x1b]0;ok\x07"));
    assert_eq!(h.delivered, vec![(2, 'o', 'k')]);
    assert_eq!(h.osc_fallback_ends, vec![(7, true)]);
}

fn csi_params(input: &[u8]) -> Host {
    let mut p = parser();
    let mut h = Host::default();
    feed(&mut p, &mut h, &cps(input));
    h
}

#[test]
fn params_beyond_the_limit_are_dropped() {
    let mut input = b"\x1b[".to_vec();
    for i in 0..100 {
        if i > 0 {
            input.push(b';');
        }
        input.extend_from_slice(i.to_string().as_bytes());
    }
    input.push(b'm');
    let h = csi_params(&input);
    let (length, array) = h.csi.first().cloned().unwrap_or_default();
    assert_eq!(h.csi.len(), 1);
    assert_eq!(length, 32);
    let expected: Vec<ParamsArrayItem> = (0..32).map(ParamsArrayItem::Param).collect();
    assert_eq!(array, expected);
}

#[test]
fn sub_params_beyond_the_limit_are_dropped() {
    // `1:2:3:...:300;7m`: 300 sub-params on the first param, then a second param.
    let mut input = b"\x1b[1".to_vec();
    for i in 2..=300 {
        input.push(b':');
        input.extend_from_slice(i.to_string().as_bytes());
    }
    input.extend_from_slice(b";7m");
    let h = csi_params(&input);
    assert_eq!(h.csi.len(), 1);
    let (length, array) = h.csi.first().cloned().unwrap_or_default();
    assert_eq!(length, 2);
    // Only the first 32 sub-params are stored.
    assert_eq!(array.len(), 3, "{array:?}");
    match array.get(1) {
        Some(ParamsArrayItem::Sub(subs)) => {
            assert_eq!(subs.len(), 32);
            assert_eq!(subs.first().copied(), Some(2));
        }
        other => panic!("expected sub-params, got {other:?}"),
    }
    assert_eq!(array.get(2), Some(&ParamsArrayItem::Param(7)));
}

#[test]
fn one_mebibyte_digit_run_in_one_csi() {
    // Param value saturates at i32::MAX; nothing grows.
    let mut input = b"\x1b[".to_vec();
    input.extend(std::iter::repeat_n(b'9', 1 << 20));
    input.push(b'm');
    let h = csi_params(&input);
    assert_eq!(h.csi.len(), 1);
    let (length, array) = h.csi.first().cloned().unwrap_or_default();
    assert_eq!(length, 1);
    assert_eq!(array, vec![ParamsArrayItem::Param(MAX_VALUE)]);

    // The same digit run as a sub-param value, and as a DCS param.
    let mut input = b"\x1b[1:".to_vec();
    input.extend(std::iter::repeat_n(b'8', 1 << 20));
    input.push(b'm');
    let h = csi_params(&input);
    let (_, array) = h.csi.first().cloned().unwrap_or_default();
    assert_eq!(
        array,
        vec![
            ParamsArrayItem::Param(1),
            ParamsArrayItem::Sub(vec![MAX_VALUE])
        ]
    );

    let mut input = b"\x1bP".to_vec();
    input.extend(std::iter::repeat_n(b'7', 1 << 20));
    input.extend_from_slice(b"$qx\x1b\\");
    let h = csi_params(&input);
    assert_eq!(h.dcs_params, vec![vec![ParamsArrayItem::Param(MAX_VALUE)]]);
    assert_eq!(h.delivered, vec![(1, 'x', 'x')]);
}

#[test]
fn a_million_digit_run_split_in_64k_chunks() {
    let mut p = parser();
    let mut h = Host::default();
    feed(&mut p, &mut h, &cps(b"\x1b["));
    feed(&mut p, &mut h, &vec![u32::from(b'3'); 1 << 20]);
    feed(&mut p, &mut h, &cps(b";4m"));
    let (length, array) = h.csi.first().cloned().unwrap_or_default();
    assert_eq!(length, 2);
    assert_eq!(
        array,
        vec![ParamsArrayItem::Param(MAX_VALUE), ParamsArrayItem::Param(4)]
    );
}

#[test]
fn osc_id_of_ten_thousand_digits_saturates() {
    let mut p = parser();
    let mut h = Host::default();
    let mut input = b"\x1b]".to_vec();
    input.extend(std::iter::repeat_n(b'9', 10_000));
    input.extend_from_slice(b";x\x07");
    feed(&mut p, &mut h, &cps(&input));
    // No handler at i32::MAX: fallback, with the saturated id.
    assert_eq!(h.osc_fallback_ends, vec![(i32::MAX, true)]);
    assert!(h.delivered.is_empty());

    // Leading zeros do not grow the id; the value is what the digits say.
    let mut input = b"\x1b]".to_vec();
    input.extend(std::iter::repeat_n(b'0', 10_000));
    input.extend_from_slice(b"0;hi\x07");
    feed(&mut p, &mut h, &cps(&input));
    assert_eq!(h.delivered, vec![(2, 'h', 'i')]);
}
