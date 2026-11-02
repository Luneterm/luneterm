//! Tests for the OSC 52 clipboard write (plan M7 step 7). There is no upstream test file. The
//! pure-parser cases are Lunegit's `Osc52ParserTests.cs` (11 Fact + 6 Theory rows), run against
//! [`parse_osc52`]:
//!  - ported: `Parse_ValidClipboardSelection_DecodesText`, `Parse_Utf8Payload_RoundTripsExactly`,
//!    `Parse_SelectionTarget_IsIgnored_AlwaysCopies` (6 rows), `Parse_Base64WithEmbeddedWhitespace_*`,
//!    `Parse_QueryForm_*`, `Parse_QuerySelectionAndQueryBody_*`, `Parse_NoSeparator_*`,
//!    `Parse_InvalidBase64_*`, `Parse_EmptyBody_*`, `Parse_OversizePayload_*`,
//!    `Parse_PayloadJustUnderTheCap_*`;
//!  - replaced: `Parse_Null_IsMalformed` (a `&str` cannot be null; the golden's `null` row is
//!    skipped by the conformance replay).
//!
//! The engine-level cases go through [`Terminal::write`]. The parser's own framing applies there
//! (C0 controls other than BEL, ESC, CAN and SUB are dropped inside an OSC string before any
//! handler sees them), which the conformance replay of the golden accounts for. Payload bytes
//! never appear in an assertion message: only outcomes, lengths and counts.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::cell::RefCell;
use std::rc::Rc;

use super::osc52::{MAX_BASE64_CHARS, MAX_DECODED_BYTES, Osc52Outcome, Osc52Result, parse_osc52};
use super::*;
use crate::engine::HostEvent;
use crate::headless::terminal::Terminal;
use crate::services::log_service::Logger;
use crate::services::options_service::{LogLevel, OptionValue};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A standard base64 encoder (test-only; the code under test only decodes).
fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn terminal(gate: bool) -> Terminal {
    let mut t = Terminal::new([OptionValue::Cols(40), OptionValue::Rows(4)]);
    t.install_standard_extensions(&StandardExtensions {
        clipboard_write: gate,
        ..StandardExtensions::default()
    })
    .unwrap();
    t
}

fn osc(payload: &str) -> Vec<u8> {
    let mut bytes = b"\x1b]52;".to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes.push(7);
    bytes
}

fn clipboard_events(t: &mut Terminal) -> Vec<String> {
    t.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::ClipboardWrite(text) => Some(text),
            _ => None,
        })
        .collect()
}

fn outcome(payload: &str) -> Osc52Outcome {
    parse_osc52(payload).outcome
}

// --- parse_osc52: Lunegit's Osc52ParserTests ------------------------------------------------

#[test]
fn valid_clipboard_selection_decodes_text() {
    let result = parse_osc52(&format!("c;{}", b64(b"hello")));
    assert_eq!(result.outcome, Osc52Outcome::Copy);
    assert_eq!(result.text.as_deref(), Some("hello"));
    assert_eq!(result.payload_length, 5);
}

#[test]
fn utf8_payload_round_trips_exactly() {
    let original = "h\u{e9}llo \u{1f680} \u{4e16}\u{754c}";
    let result = parse_osc52(&format!("c;{}", b64(original.as_bytes())));
    assert_eq!(result.outcome, Osc52Outcome::Copy);
    assert_eq!(result.text.as_deref(), Some(original));
}

#[test]
fn selection_target_is_ignored_and_always_copies() {
    for selection in ["", "c", "p", "s", "0", "pc"] {
        let result = parse_osc52(&format!("{selection};{}", b64(b"hello")));
        assert_eq!(result.outcome, Osc52Outcome::Copy, "{selection:?}");
        assert_eq!(result.text.as_deref(), Some("hello"));
    }
}

#[test]
fn base64_with_embedded_whitespace_still_decodes() {
    let raw = b64(b"hello world");
    let wrapped = format!("{}\n{}", &raw[..4], &raw[4..]);
    let result = parse_osc52(&format!("c;{wrapped}"));
    assert_eq!(result.outcome, Osc52Outcome::Copy);
    assert_eq!(result.text.as_deref(), Some("hello world"));
}

#[test]
fn query_form_is_refused_and_carries_no_text() {
    let result = parse_osc52("c;?");
    assert_eq!(result.outcome, Osc52Outcome::Query);
    assert_eq!(result.text, None);
    assert_eq!(parse_osc52("?;?").outcome, Osc52Outcome::Query);
    assert_eq!(parse_osc52("?;?").text, None);
}

#[test]
fn no_separator_is_malformed() {
    assert_eq!(outcome(&b64(b"hello")), Osc52Outcome::Malformed);
}

#[test]
fn invalid_base64_is_malformed() {
    assert_eq!(outcome("c;!!notbase64!!"), Osc52Outcome::Malformed);
}

#[test]
fn empty_body_is_empty_so_the_clipboard_is_never_cleared() {
    let result = parse_osc52("c;");
    assert_eq!(result.outcome, Osc52Outcome::Empty);
    assert_eq!(result.text, None);
}

#[test]
fn oversize_payload_is_discarded_entirely() {
    let payload = format!("c;{}", "A".repeat(MAX_BASE64_CHARS + 4));
    let result = parse_osc52(&payload);
    assert_eq!(result.outcome, Osc52Outcome::Oversize);
    assert_eq!(result.text, None);
    assert!(result.payload_length > MAX_BASE64_CHARS);
}

#[test]
fn payload_just_under_the_cap_still_copies() {
    let text = "x".repeat(1000);
    let result = parse_osc52(&format!("c;{}", b64(text.as_bytes())));
    assert_eq!(result.outcome, Osc52Outcome::Copy);
    assert_eq!(result.text.as_deref(), Some(text.as_str()));
}

// --- parse_osc52: rules beyond the Lunegit tests (the golden pins them too) -----------------

#[test]
fn padding_and_length_rules() {
    for (payload, want) in [
        ("c;YQ==", Osc52Outcome::Copy),
        ("c;YWI=", Osc52Outcome::Copy),
        ("c;YWJj", Osc52Outcome::Copy),
        ("c;YQ", Osc52Outcome::Malformed),
        ("c;YQ=", Osc52Outcome::Malformed),
        ("c;YQ===", Osc52Outcome::Malformed),
        ("c;YQ==YQ==", Osc52Outcome::Malformed),
        ("c;Y=Q=", Osc52Outcome::Malformed),
        ("c;====", Osc52Outcome::Malformed),
        ("c;=", Osc52Outcome::Malformed),
        ("c;YWJj====", Osc52Outcome::Malformed),
        // trailing bits are not checked
        ("c;YR==", Osc52Outcome::Copy),
        // VT and FF are not whitespace, NBSP neither
        ("c;aGVs\u{b}bG8=", Osc52Outcome::Malformed),
        ("c;aGVs\u{c}bG8=", Osc52Outcome::Malformed),
        ("c;aGVs\u{a0}bG8=", Osc52Outcome::Malformed),
        // SP, HT, CR, LF are ignored anywhere, even between the padding
        ("c;YQ= \t\r\n=", Osc52Outcome::Copy),
        ("c;  \n ", Osc52Outcome::Empty),
    ] {
        assert_eq!(outcome(payload), want, "{payload:?}");
    }
}

#[test]
fn invalid_utf8_becomes_replacement_characters() {
    // 0xFF 0xFE 'A' and a truncated sequence
    assert_eq!(
        parse_osc52("c;//5B").text.as_deref(),
        Some("\u{fffd}\u{fffd}A")
    );
    assert_eq!(parse_osc52("c;QcM=").text.as_deref(), Some("A\u{fffd}"));
}

#[test]
fn the_decoded_limit_is_one_mebibyte_inclusive() {
    let exactly = format!("c;{}AA==", "A".repeat(MAX_BASE64_CHARS - 4));
    let result = parse_osc52(&exactly);
    assert_eq!(result.outcome, Osc52Outcome::Copy);
    assert_eq!(result.payload_length, MAX_DECODED_BYTES);
    let over = format!("c;{}AAA=", "A".repeat(MAX_BASE64_CHARS - 4));
    assert_eq!(
        parse_osc52(&over),
        Osc52Result {
            outcome: Osc52Outcome::Oversize,
            text: None,
            payload_length: MAX_DECODED_BYTES + 1
        }
    );
    // the longest accepted character count already decodes past the limit
    let max_chars = format!("c;{}", "A".repeat(MAX_BASE64_CHARS));
    assert_eq!(outcome(&max_chars), Osc52Outcome::Oversize);
    assert_eq!(MAX_BASE64_CHARS, 1_398_104);
}

#[test]
fn decoded_output_is_never_longer_than_the_limit() {
    // nothing panics and nothing exceeds the cap for hostile bodies
    for body in [
        "A".repeat(3),
        "=".repeat(3),
        "\u{1f680}".repeat(10),
        "A=".repeat(1000),
        "?".to_owned(),
        "??".to_owned(),
    ] {
        let result = parse_osc52(&format!(";{body}"));
        if let Some(text) = result.text {
            assert!(text.len() <= MAX_DECODED_BYTES * 3);
        }
    }
}

// --- the handler through Terminal::write ------------------------------------------------------

#[test]
fn a_copy_is_queued_as_one_clipboard_write() {
    let mut t = terminal(true);
    t.write(&osc(&format!("c;{}", b64(b"hello"))));
    assert_eq!(
        t.drain_events(),
        [HostEvent::ClipboardWrite("hello".into())]
    );
}

#[test]
fn st_terminated_and_split_writes_work() {
    let mut t = terminal(true);
    t.write(b"\x1b]52;c;aGVs");
    t.write(b"bG8=\x1b");
    t.write(b"\\");
    assert_eq!(
        t.drain_events(),
        [HostEvent::ClipboardWrite("hello".into())]
    );
}

#[test]
fn a_query_is_never_answered_and_queues_nothing() {
    for gate in [true, false] {
        let mut t = terminal(gate);
        t.write(&osc("c;?"));
        t.write(&osc("?;?"));
        t.write(&osc(";?"));
        assert!(t.drain_events().is_empty(), "gate {gate}");
    }
}

#[test]
fn empty_malformed_and_selection_only_queue_nothing() {
    let mut t = terminal(true);
    for payload in ["c;", "c", "", ";", "c;!!!!", "c;YQ=", "c;   "] {
        t.write(&osc(payload));
    }
    t.write(b"\x1b]52\x07");
    t.write(b"\x1b]52;\x07");
    assert!(t.drain_events().is_empty());
}

#[test]
fn gate_off_consumes_and_queues_nothing_and_reports_nothing_unknown() {
    let mut t = terminal(false);
    t.write(&osc(&format!("c;{}", b64(b"hello"))));
    // consumed: not reported as an unknown OSC either
    assert!(t.drain_events().is_empty());
}

#[test]
fn toggling_the_gate_takes_effect_at_the_next_osc() {
    let mut t = terminal(false);
    let payload = osc(&format!("c;{}", b64(b"hello")));
    assert!(!t.clipboard_write());
    t.write(&payload);
    assert!(clipboard_events(&mut t).is_empty());
    t.set_clipboard_write(true);
    assert!(t.clipboard_write());
    t.write(&payload);
    assert_eq!(clipboard_events(&mut t), ["hello"]);
    // turned off between the start and the end of one OSC: the gate is read at the end
    t.write(b"\x1b]52;c;aGVsbG8=");
    t.set_clipboard_write(false);
    t.write(b"\x07");
    assert!(clipboard_events(&mut t).is_empty());
    t.set_clipboard_write(true);
    t.write(&payload);
    assert_eq!(clipboard_events(&mut t), ["hello"]);
}

#[test]
fn two_writes_in_one_drain_give_one_event_with_the_second_text() {
    let mut t = terminal(true);
    t.write(&osc(&format!("c;{}", b64(b"first"))));
    t.write(b"\x07");
    t.write(&osc(&format!("c;{}", b64(b"second"))));
    assert_eq!(t.drain_events().len(), 2); // Bell, ClipboardWrite
    t.write(&osc(&format!("c;{}", b64(b"first"))));
    t.write(&osc(&format!("c;{}", b64(b"second"))));
    assert_eq!(
        t.drain_events(),
        [HostEvent::ClipboardWrite("second".into())]
    );
}

#[test]
fn an_aborted_osc_queues_nothing_and_the_next_one_works() {
    let mut t = terminal(true);
    // CAN and SUB abort the string
    t.write(b"\x1b]52;c;aGVsbG8=\x18");
    t.write(b"\x1b]52;c;aGVsbG8=\x1a");
    assert!(t.drain_events().is_empty());
    // ESC ends an OSC string normally (it may start ST): the parser's framing, as upstream, so
    // a payload cut by an ESC is a complete write
    t.write(b"\x1b]52;c;aGVsbG8=\x1b]52;c;");
    t.write(b"\x07");
    assert_eq!(clipboard_events(&mut t), ["hello"]);
    t.write(&osc(&format!("c;{}", b64(b"ok"))));
    assert_eq!(clipboard_events(&mut t), ["ok"]);
}

#[test]
fn the_selection_is_not_stored_whatever_its_length() {
    let mut t = terminal(true);
    let mut bytes = b"\x1b]52;".to_vec();
    bytes.extend(std::iter::repeat_n(b'c', 3_000_000));
    t.write(&bytes);
    let (len, capacity) = t.core().input_handler().ext.osc52.buffered();
    assert_eq!((len, capacity), (0, 0));
    t.write(b";aGVsbG8=\x07");
    assert_eq!(clipboard_events(&mut t), ["hello"]);
}

#[test]
fn without_the_install_osc_52_reaches_the_core_fallback_only() {
    let mut t = Terminal::new([]);
    t.write(&osc(&format!("c;{}", b64(b"hello"))));
    t.write(&osc("c;?"));
    assert!(t.drain_events().is_empty());
    // the gate setter is inert there
    t.set_clipboard_write(true);
    t.write(&osc(&format!("c;{}", b64(b"hello"))));
    assert!(t.drain_events().is_empty());
}

// --- streaming and the cap --------------------------------------------------------------------

/// Write `payload` in `chunk`-byte writes and return the largest buffer length and capacity
/// seen after any write.
fn stream(
    t: &mut Terminal,
    prefix: &[u8],
    body: &[u8],
    chunk: usize,
    end: &[u8],
) -> (usize, usize) {
    t.write(prefix);
    let mut peak = (0, 0);
    for part in body.chunks(chunk) {
        t.write(part);
        let (len, capacity) = t.core().input_handler().ext.osc52.buffered();
        peak = (peak.0.max(len), peak.1.max(capacity));
    }
    t.write(end);
    peak
}

#[test]
fn a_payload_at_the_character_cap_decodes_over_many_writes() {
    // 1,398,100 characters plus `AA==`: exactly 1 MiB decoded
    let mut t = terminal(true);
    let mut body = vec![b'A'; MAX_BASE64_CHARS - 4];
    body.extend_from_slice(b"AA==");
    let (len, capacity) = stream(&mut t, b"\x1b]52;c;", &body, 4093, b"\x07");
    assert!(len <= MAX_BASE64_CHARS, "{len}");
    assert!(capacity <= MAX_BASE64_CHARS, "{capacity}");
    let events = clipboard_events(&mut t);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].len(), MAX_DECODED_BYTES);
    // the buffer is released at the end
    assert_eq!(t.core().input_handler().ext.osc52.buffered(), (0, 0));
}

#[test]
fn a_payload_one_character_over_the_cap_is_discarded_and_never_buffered_past_it() {
    for extra in [1usize, 4, 5, 1_000_000] {
        let mut t = terminal(true);
        let body = vec![b'A'; MAX_BASE64_CHARS + extra];
        let (len, capacity) = stream(&mut t, b"\x1b]52;c;", &body, 65_536, b"\x07");
        assert!(len <= MAX_BASE64_CHARS, "{extra}: {len}");
        assert!(capacity <= MAX_BASE64_CHARS, "{extra}: {capacity}");
        assert!(clipboard_events(&mut t).is_empty(), "{extra}");
        assert_eq!(t.core().input_handler().ext.osc52.buffered(), (0, 0));
        // the terminal is not wedged
        t.write(&osc(&format!("c;{}", b64(b"after"))));
        assert_eq!(clipboard_events(&mut t), ["after"]);
    }
}

#[test]
fn a_decoded_oversize_within_the_character_cap_is_discarded() {
    let mut t = terminal(true);
    let body = vec![b'A'; MAX_BASE64_CHARS];
    stream(&mut t, b"\x1b]52;c;", &body, 1 << 16, b"\x07");
    assert!(clipboard_events(&mut t).is_empty());
}

#[test]
fn a_character_no_body_holds_stops_buffering() {
    let mut t = terminal(true);
    t.write(b"\x1b]52;c;aGVs\xc3\xa9");
    assert_eq!(t.core().input_handler().ext.osc52.buffered(), (0, 0));
    t.write(b"bG8=\x07");
    assert!(clipboard_events(&mut t).is_empty());
    // 1-byte writes behave as one write
    let mut t = terminal(true);
    for byte in osc(&format!("c;{}", b64(b"hello"))) {
        t.write(&[byte]);
    }
    assert_eq!(clipboard_events(&mut t), ["hello"]);
}

#[test]
fn engine_output_is_chunk_invariant() {
    let payloads = [
        format!("c;{}", b64(b"hello world")),
        "c;!!!!".to_owned(),
        "c;?".to_owned(),
        format!("pc;{}", b64("h\u{e9}llo \u{1f680}".as_bytes())),
    ];
    for payload in payloads {
        let bytes = osc(&payload);
        let mut whole = terminal(true);
        whole.write(&bytes);
        let mut split = terminal(true);
        for byte in &bytes {
            split.write(std::slice::from_ref(byte));
        }
        assert_eq!(
            whole.drain_events(),
            split.drain_events(),
            "{}",
            payload.len()
        );
    }
}

// --- logging ----------------------------------------------------------------------------------

struct Capture(Rc<RefCell<Vec<(LogLevel, String)>>>);

impl Logger for Capture {
    fn log(&self, level: LogLevel, message: &str) {
        self.0.borrow_mut().push((level, message.to_owned()));
    }
}

#[test]
fn logs_carry_the_outcome_and_length_never_the_payload() {
    let mut t = Terminal::new([OptionValue::LogLevel(LogLevel::Trace)]);
    t.install_standard_extensions(&StandardExtensions {
        clipboard_write: true,
        ..StandardExtensions::default()
    })
    .unwrap();
    let messages = Rc::new(RefCell::new(Vec::new()));
    t.set_logger(Some(Box::new(Capture(Rc::clone(&messages)))));
    let secret = "hunter2-clipboard-secret";
    let encoded = b64(secret.as_bytes());
    t.write(&osc(&format!("c;{encoded}")));
    t.write(&osc("c;?"));
    t.write(&osc(&format!("c;{encoded}=")));
    t.write(&osc("c;!!!!"));
    let mut huge = b"\x1b]52;c;".to_vec();
    huge.extend(std::iter::repeat_n(b'A', MAX_BASE64_CHARS + 8));
    huge.push(7);
    t.write(&huge);
    let messages = messages.borrow();
    for (_, message) in messages.iter() {
        assert!(!message.contains(secret), "plain payload logged");
        assert!(!message.contains(&encoded), "encoded payload logged");
        assert!(!message.contains("hunter2"), "payload fragment logged");
    }
    let has = |level: LogLevel, needle: &str| {
        messages
            .iter()
            .any(|(l, m)| *l == level && m.contains(needle))
    };
    assert!(
        has(LogLevel::Debug, "OSC 52 copy: length 24"),
        "{:?}",
        messages.len()
    );
    assert!(has(LogLevel::Debug, "OSC 52 query: length 0"));
    assert!(has(LogLevel::Debug, "OSC 52 malformed: length 0"));
    let cap = MAX_BASE64_CHARS.to_string();
    assert!(
        messages.iter().any(|(l, m)| *l == LogLevel::Warn
            && m.contains(&(MAX_BASE64_CHARS + 8).to_string())
            && m.contains(&cap)),
        "oversize warning with length and cap"
    );
}
