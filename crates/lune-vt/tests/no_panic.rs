//! Seeded no-panic fuzz of the decoder + escape sequence parser, with a chunk-invariance check.
//!
//! Per seed a biased random byte stream (escape-sequence building blocks, digit runs, C1 as
//! UTF-8, invalid and valid UTF-8, now and then a payload burst up to 64 KiB) is cut into random
//! chunks of 1 to 64 bytes and fed to a parser with a full set of registered handlers while
//! handlers are disposed and re-registered between chunks. Each seed runs under `catch_unwind`;
//! a failure reports the seed (and for a trace mismatch the line index and clipped lines, never
//! the input). The same input then goes through the fallback-only trace parser chunked and
//! unsplit; the normalised traces must be equal.
//!
//! Debug builds keep integer-overflow checks on, so arithmetic overflow in the parser is a panic
//! here.
//!
//! `LUNE_VT_FUZZ_SEEDS=<n>` runs seeds `0..n` (default 1000); `LUNE_VT_FUZZ_SEED=<s>` replays one.

// `lune-vt` cannot dev-depend on `lune-vt-conformance` (it depends on `lune-vt`), so the trace
// runner is included by path: one implementation of the normalisation for this test and for
// `lune-vt-conformance/tests/parser_trace.rs`.
#[path = "../../lune-vt-conformance/src/escape.rs"]
mod escape;
#[path = "../../lune-vt-conformance/src/trace.rs"]
mod trace;

use std::panic::{AssertUnwindSafe, catch_unwind};

use lune_vt::input::text_decoder::Utf8ToUtf32;
use lune_vt::parser::constants::PAYLOAD_LIMIT;
use lune_vt::parser::dcs_parser::DcsStringHandler;
use lune_vt::parser::escape_sequence_parser::EscapeSequenceParser;
use lune_vt::parser::osc_parser::OscStringHandler;
use lune_vt::parser::params::Params;
use lune_vt::parser::types::{
    DcsFallbackAction, FunctionIdentifier, HandlerId, OscFallbackAction, ParserHost,
};

use trace::{Tracer, clip, first_difference};

const DEFAULT_SEEDS: u64 = 1000;
const INPUT_BYTES: usize = 4096;
/// Default `Params` limits (upstream: 32 params, 32 sub-params).
const PARAM_LIMIT: usize = 32;
/// `interim` of the decoder holds at most lead + two continuation bytes.
const INTERIM_LIMIT: usize = 3;

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish in `0..n` (`n > 0`).
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// Uniform-ish in `lo..=hi`.
    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        let index = usize::try_from(self.below(items.len() as u64)).ok()?;
        items.get(index)
    }

    fn byte_of(&mut self, items: &[u8]) -> u8 {
        self.pick(items).copied().unwrap_or(0)
    }
}

/// A random valid UTF-8 sequence of 2, 3 or 4 bytes.
fn utf8_sequence(rng: &mut Rng, out: &mut Vec<u8>) {
    let (lo, hi) = match rng.below(3) {
        0 => (0x80, 0x7FF),
        1 => (0x800, 0xFFFF),
        _ => (0x1_0000, 0x10_FFFF),
    };
    let cp = u32::try_from(rng.between(lo, hi)).unwrap_or(0xE9);
    let ch = char::from_u32(cp).unwrap_or('\u{E9}');
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

/// An OSC or DCS with a long payload, 1 to 64 KiB.
fn payload_burst(rng: &mut Rng, out: &mut Vec<u8>) {
    let osc = rng.below(2) == 0;
    if osc {
        out.extend_from_slice(b"\x1b]");
        const IDS: [&[u8]; 7] = [b"0", b"2", b"8", b"52", b"104", b"x", b""];
        out.extend_from_slice(rng.pick(&IDS).copied().unwrap_or(b"0"));
        out.push(b';');
    } else {
        out.extend_from_slice(b"\x1bP");
        if rng.below(2) == 0 {
            out.extend_from_slice(b"1;2");
        }
        out.extend_from_slice(b"$q");
    }
    let length = rng.between(1, 65536);
    let multibyte = rng.below(3) == 0;
    while (out.len() as u64) < length {
        if multibyte && rng.below(8) == 0 {
            utf8_sequence(rng, out);
        } else {
            out.push(0x20 + u8::try_from(rng.below(95)).unwrap_or(0));
        }
    }
    match rng.below(5) {
        0 => out.push(0x07),
        1 | 2 => out.extend_from_slice(b"\x1b\\"),
        3 => out.extend_from_slice(&[0xC2, 0x9C]),
        _ => {}
    }
}

/// Biased random stream of about `target` bytes; one seed in 25 also carries a payload burst.
fn generate(rng: &mut Rng, target: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(target + 128);
    while out.len() < target {
        match rng.below(100) {
            0..=9 => out.push(0x1b),
            10..=17 => out.push(rng.byte_of(b"[]P\\^_X")),
            18..=29 => {
                let run = if rng.below(8) == 0 {
                    rng.between(5, 60)
                } else {
                    rng.between(1, 4)
                };
                for _ in 0..run {
                    out.push(b'0' + u8::try_from(rng.below(10)).unwrap_or(0));
                }
            }
            30..=37 => out.push(rng.byte_of(b";:?><=$!\"'")),
            38..=44 => out.push(0x20 + u8::try_from(rng.below(16)).unwrap_or(0)),
            45..=54 => out.push(0x40 + u8::try_from(rng.below(63)).unwrap_or(0)),
            55..=58 => out.push(rng.byte_of(&[0x07, 0x18, 0x1a, 0x7f, 0x00, 0x08, 0x0a, 0x0d])),
            59..=62 => {
                out.push(0xC2);
                out.push(0x80 + u8::try_from(rng.below(32)).unwrap_or(0));
            }
            63..=68 => out.push(0x80 + u8::try_from(rng.below(128)).unwrap_or(0)),
            69..=78 => utf8_sequence(rng, &mut out),
            _ => out.push(0x20 + u8::try_from(rng.below(95)).unwrap_or(0)),
        }
    }
    if rng.below(25) == 0 {
        let mut burst = Vec::new();
        payload_burst(rng, &mut burst);
        let at = usize::try_from(rng.below(out.len() as u64 + 1)).unwrap_or(0);
        out.splice(at..at, burst);
    }
    out
}

/// Chunk lengths 1..=64 covering `total` bytes.
fn chunk_lengths(rng: &mut Rng, total: usize) -> Vec<usize> {
    let mut lengths = Vec::new();
    let mut left = total;
    while left > 0 {
        let len = usize::try_from(rng.between(1, 64)).unwrap_or(1).min(left);
        lengths.push(len);
        left -= len;
    }
    lengths
}

// ------------------------------------------------------------------------------------------
// Parser with handlers registered.

#[derive(Debug, Default)]
struct Host {
    join: u32,
    events: u64,
}

impl ParserHost for Host {
    fn preceding_join_state(&mut self) -> &mut u32 {
        &mut self.join
    }
}

/// What a CSI/DCS handler may rely on: params stay within the limits and read without panic.
fn check_params(params: &Params) {
    assert!(params.length() <= PARAM_LIMIT, "params.length() > 32");
    assert_eq!(params.values().len(), params.length());
    for i in 0..40 {
        if let Some(subs) = params.get_sub_params(i) {
            assert!(subs.len() <= PARAM_LIMIT, "sub-params > 32");
        }
    }
    let array = params.to_array();
    assert!(array.len() <= 2 * PARAM_LIMIT, "to_array() too long");
}

/// A string handler's payload, as the OSC/DCS string handlers keep it: at most
/// `PAYLOAD_LIMIT` UTF-16 units reach the callback.
fn check_payload(payload: &str) {
    let units: usize = payload.chars().map(char::len_utf16).sum();
    assert!(units <= PAYLOAD_LIMIT, "payload over the limit");
}

fn csi_base(p: &mut EscapeSequenceParser<Host>) {
    let ids = [
        FunctionIdentifier::new('m'),
        FunctionIdentifier::new('H'),
        FunctionIdentifier::new('c'),
        FunctionIdentifier::with_prefix('?', 'h'),
        FunctionIdentifier::with_prefix('?', 'l'),
        FunctionIdentifier::with_intermediates("$", 'p'),
        FunctionIdentifier::with_prefix_and_intermediates('?', "$", 'p'),
    ];
    for (i, id) in ids.into_iter().enumerate() {
        // Every third handler returns false so the fall-through chain is exercised.
        let handled = i % 3 != 0;
        let _ = p.register_csi_handler(id, move |h: &mut Host, params: &Params| {
            h.events += 1;
            check_params(params);
            handled
        });
    }
}

fn osc_handler() -> OscStringHandler<Host> {
    OscStringHandler::new(|h: &mut Host, payload: &str| {
        h.events += 1;
        check_payload(payload);
        true
    })
}

fn build_parser() -> EscapeSequenceParser<Host> {
    let mut p = EscapeSequenceParser::<Host>::new();
    p.set_print_handler(|h: &mut Host, data: &[u32]| {
        h.events += 1;
        *h.preceding_join_state() = u32::try_from(data.len() & 3).unwrap_or(0);
    });
    for flag in ['\u{7}', '\u{8}', '\n', '\r'] {
        p.set_execute_handler(flag, |h: &mut Host| h.events += 1);
    }
    p.set_execute_handler_fallback(|h: &mut Host, _| h.events += 1);
    csi_base(&mut p);
    p.set_csi_handler_fallback(|h: &mut Host, _, params: &Params| {
        h.events += 1;
        check_params(params);
    });
    for id in [
        FunctionIdentifier::new('7'),
        FunctionIdentifier::new('c'),
        FunctionIdentifier::with_intermediates("(", 'B'),
    ] {
        let _ = p.register_esc_handler(id, |h: &mut Host| {
            h.events += 1;
            true
        });
    }
    p.set_esc_handler_fallback(|h: &mut Host, _| h.events += 1);
    for ident in [0, 2, 8, 52, 104] {
        p.register_osc_handler(ident, osc_handler());
    }
    p.set_osc_handler_fallback(|h: &mut Host, _, action| {
        h.events += 1;
        if let OscFallbackAction::Put(data) = action {
            assert!(data.len() <= PAYLOAD_LIMIT);
        }
    });
    let _ = p.register_dcs_handler(
        FunctionIdentifier::with_intermediates("$", 'q'),
        DcsStringHandler::new(|h: &mut Host, payload: &str, params: &Params| {
            h.events += 1;
            check_payload(payload);
            check_params(params);
            true
        }),
    );
    p.set_dcs_handler_fallback(|h: &mut Host, _, action| {
        h.events += 1;
        if let DcsFallbackAction::Hook(params) = action {
            check_params(params);
        }
    });
    p.set_error_handler(|h: &mut Host, state| {
        h.events += 1;
        check_params(state.params);
        // Abort now and then: the rest of the chunk is dropped, parsing resumes with the next.
        state.code & 0xF == 0xF
    });
    p
}

/// Feeds the chunks to a parser with handlers, changing its registrations between chunks.
fn run_with_handlers(rng: &mut Rng, input: &[u8], lengths: &[usize]) {
    let mut parser = build_parser();
    let mut decoder = Utf8ToUtf32::new();
    let mut host = Host::default();
    let mut scratch: Vec<u32> = Vec::new();
    let mut csi_extra: Option<HandlerId> = None;
    let mut osc_extra: Option<HandlerId> = None;
    let mut at = 0;
    for &len in lengths {
        let chunk = input.get(at..at + len).unwrap_or(&[]);
        at += len;
        decoder.decode(chunk, &mut scratch);
        assert!(
            decoder.interim().len() <= INTERIM_LIMIT,
            "decoder interim over 3 bytes"
        );
        parser.parse(&mut host, &scratch);
        match rng.below(8) {
            0 => match csi_extra.take() {
                Some(id) => parser.dispose_handler(id),
                None => {
                    csi_extra = parser
                        .register_csi_handler(
                            FunctionIdentifier::new('m'),
                            |h: &mut Host, params: &Params| {
                                h.events += 1;
                                check_params(params);
                                true
                            },
                        )
                        .ok();
                }
            },
            1 => {
                let _ = parser.clear_csi_handler(FunctionIdentifier::new('m'));
                csi_extra = None;
                let _ = parser.register_csi_handler(
                    FunctionIdentifier::new('m'),
                    |h: &mut Host, params: &Params| {
                        h.events += 1;
                        check_params(params);
                        false
                    },
                );
            }
            2 => match osc_extra.take() {
                Some(id) => parser.dispose_handler(id),
                None => osc_extra = Some(parser.register_osc_handler(0, osc_handler())),
            },
            3 => {
                parser.clear_osc_handler(2);
                parser.register_osc_handler(2, osc_handler());
            }
            4 => {
                let _ = parser.clear_dcs_handler(FunctionIdentifier::with_intermediates("$", 'q'));
            }
            5 if rng.below(8) == 0 => parser.reset(&mut host),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------------------------------

/// Upstream xterm.js 6.0.0 is not chunk-invariant for DEL inside a DCS payload: in
/// DCS_PASSTHROUGH the table maps 0x7F to IGNORE, but the DCS_PUT read-ahead loop includes it,
/// so a DEL that starts a chunk is dropped and one in the middle of a chunk is kept (verified
/// against the oracle: `ESC P $ q a DEL b` gives `a DEL b`, split before DEL gives `a b`). The
/// port is faithful, so the invariance check removes DEL from `dcs put` lines on both sides.
/// `dcs_del_chunk_dependence_is_upstreams` pins the quirk; delete this function and that test
/// together if the parser ever stops having it.
fn without_dcs_del(lines: Vec<String>) -> Vec<String> {
    const PREFIX: &str = "dcs put \"";
    const DEL: &str = "\\u{7F}";
    lines
        .into_iter()
        .filter_map(|line| match line.strip_prefix(PREFIX) {
            Some(rest) => {
                let kept = rest.replace(DEL, "");
                (kept != "\"").then(|| format!("{PREFIX}{kept}"))
            }
            None => Some(line),
        })
        .collect()
}

fn run_seed(seed: u64) {
    let mut rng = Rng(seed);
    let input = generate(&mut rng, INPUT_BYTES);
    let lengths = chunk_lengths(&mut rng, input.len());

    run_with_handlers(&mut rng, &input, &lengths);

    // Chunk invariance across decoder and parser.
    let mut chunked = Tracer::new();
    let mut at = 0;
    for &len in &lengths {
        chunked.feed(input.get(at..at + len).unwrap_or(&[]));
        at += len;
    }
    let mut unsplit = Tracer::new();
    unsplit.feed(&input);
    let (a, b) = (
        without_dcs_del(chunked.lines()),
        without_dcs_del(unsplit.lines()),
    );
    if let Some(index) = first_difference(&a, &b) {
        panic!(
            "chunked and unsplit traces differ at line {index}\n  chunked: {}\n  unsplit: {}",
            clip(a.get(index), 120),
            clip(b.get(index), 120)
        );
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<non-string panic>".to_string()
    }
}

#[test]
fn random_streams_never_panic_and_chunking_changes_nothing() {
    let seeds: Vec<u64> = match env_u64("LUNE_VT_FUZZ_SEED") {
        Some(one) => vec![one],
        None => (0..env_u64("LUNE_VT_FUZZ_SEEDS").unwrap_or(DEFAULT_SEEDS)).collect(),
    };
    let mut failures: Vec<String> = Vec::new();
    for &seed in &seeds {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| run_seed(seed))) {
            failures.push(format!("seed {seed}: {}", panic_text(payload.as_ref())));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} seeds failed (replay with LUNE_VT_FUZZ_SEED=<seed>):\n{}",
        failures.len(),
        seeds.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The upstream quirk `without_dcs_del` hides, pinned so it is noticed if it goes away.
#[test]
fn dcs_del_chunk_dependence_is_upstreams() {
    let head = b"\x1bP$qa";
    let tail = b"\x7fb\x1b\\";
    let mut whole = head.to_vec();
    whole.extend_from_slice(tail);
    let mut unsplit = Tracer::new();
    unsplit.feed(&whole);
    let mut split = Tracer::new();
    split.feed(head);
    split.feed(tail);
    assert!(
        unsplit
            .lines()
            .contains(&"dcs put \"a\\u{7F}b\"".to_string()),
        "{:?}",
        unsplit.lines()
    );
    assert!(
        split.lines().contains(&"dcs put \"ab\"".to_string()),
        "{:?}",
        split.lines()
    );
    // And the normalisation makes them equal.
    assert_eq!(
        without_dcs_del(unsplit.lines()),
        without_dcs_del(split.lines())
    );
}

/// The generator itself: deterministic per seed, and it does produce the shapes the fuzz relies
/// on (an instrument check: a generator that emitted only ASCII would pass everything).
#[test]
fn generator_is_deterministic_and_varied() {
    let a = generate(&mut Rng(7), INPUT_BYTES);
    let b = generate(&mut Rng(7), INPUT_BYTES);
    let c = generate(&mut Rng(8), INPUT_BYTES);
    assert_eq!(a, b);
    assert_ne!(a, c);
    let mut esc = 0;
    let mut high = 0;
    let mut digits = 0;
    for seed in 0..50 {
        for &byte in &generate(&mut Rng(seed), INPUT_BYTES) {
            esc += usize::from(byte == 0x1b);
            high += usize::from(byte >= 0x80);
            digits += usize::from(byte.is_ascii_digit());
        }
    }
    assert!(
        esc > 5000 && high > 5000 && digits > 5000,
        "{esc} {high} {digits}"
    );
    // Some seed in the first hundred carries a long payload burst.
    assert!(
        (0..100).any(|seed| generate(&mut Rng(seed), INPUT_BYTES).len() > 3 * INPUT_BYTES),
        "no payload burst in 100 seeds"
    );
}
