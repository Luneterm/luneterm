//! OSC 52 clipboard write: fail closed, write-only and gated (plan M7 step 7, D9). Not in
//! xterm.js 6.0.0 (its `ClipboardAddon` is a separate package); the rules are Lunegit's
//! `Osc52Parser.cs` (HEAD 33a1dd4), whose real outcomes are `goldens/lunegit-input/osc52.golden`.
//!
//! The payload is untrusted and the permission it asks for (the user's clipboard) is the host's
//! to give, so:
//!  - the request is **write-only**: the query (`OSC 52 ; c ; ?`) is recognised and consumed,
//!    never answered, never queued;
//!  - a write reaches the host only as [`HostEvent::ClipboardWrite`] with `1 ..= 1 MiB` decoded
//!    bytes, and only while the runtime gate is on ([`crate::Terminal::set_clipboard_write`]);
//!  - an oversize payload is discarded whole, never truncated;
//!  - the handler is streamed: it never holds more than [`MAX_BASE64_CHARS`] bytes (the cap, the
//!    payload of at most `1398104` base64 characters that could decode within the limit), the
//!    selection before the first `;` is ignored and not stored, and the first character that no
//!    base64 body may contain ends buffering (the outcome is then Malformed or Oversize whatever
//!    follows);
//!  - logs name the outcome and the length, never the payload.
//!
//! Differences from `Osc52Parser.cs`, all framing: the sequence is the parser's OSC (so CAN/SUB,
//! C1 ST and ESC inside the payload end or abort it as for every OSC, see `docs/porting-xterm.md`),
//! and a null payload cannot occur.

use crate::engine::HostEvent;
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::OscHandler;

/// The largest decoded payload accepted, in bytes (1 MiB). Larger is discarded entirely.
pub const MAX_DECODED_BYTES: usize = 1024 * 1024;

/// The most base64 characters (UTF-16 units after the first `;`, as .NET counts them) that can
/// decode within [`MAX_DECODED_BYTES`]: `4 * ceil(1 MiB / 3)`. Checked before decoding.
pub const MAX_BASE64_CHARS: usize = 4 * MAX_DECODED_BYTES.div_ceil(3);

/// OSC identifier.
const OSC_IDENT: i32 = 52;

/// What to do with a parsed OSC 52 payload (`Osc52Parser.Outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc52Outcome {
    /// A valid write; [`Osc52Result::text`] holds the decoded text.
    Copy,
    /// The read form (`;?`): refused by design, never answered.
    Query,
    /// No separator, or the body is not base64. Dropped.
    Malformed,
    /// Zero decoded bytes. Dropped: an empty write must never clear the clipboard.
    Empty,
    /// Over the cap. Dropped entirely.
    Oversize,
}

impl Osc52Outcome {
    /// The outcome's name, for logs.
    pub fn name(self) -> &'static str {
        match self {
            Osc52Outcome::Copy => "copy",
            Osc52Outcome::Query => "query",
            Osc52Outcome::Malformed => "malformed",
            Osc52Outcome::Empty => "empty",
            Osc52Outcome::Oversize => "oversize",
        }
    }
}

/// The result of [`parse_osc52`]. `text` is `Some` only for [`Osc52Outcome::Copy`];
/// `payload_length` is the diagnostic length (decoded bytes for a copy, base64 characters or
/// decoded bytes for an oversize rejection, 0 elsewhere).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc52Result {
    pub outcome: Osc52Outcome,
    pub text: Option<String>,
    pub payload_length: usize,
}

impl Osc52Result {
    fn dropped(outcome: Osc52Outcome, payload_length: usize) -> Self {
        Osc52Result {
            outcome,
            text: None,
            payload_length,
        }
    }
}

/// Parse an OSC 52 body (everything after `52;`, e.g. `c;aGVsbG8=`), `Osc52Parser.Parse` rule for
/// rule. Never panics, never allocates the decoded buffer for an oversize payload.
pub fn parse_osc52(payload: &str) -> Osc52Result {
    match payload.split_once(';') {
        Some((_selection, body)) => parse_body(body),
        None => Osc52Result::dropped(Osc52Outcome::Malformed, 0),
    }
}

/// The part of a base64 character, `None` for anything else.
fn sextet(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// ASCII whitespace the .NET decoder ignores anywhere: SP, HT, CR, LF (not VT or FF).
fn is_ignored_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// A character a buffered body may contain: the base64 alphabet, `=`, `?` (the query) and the
/// ignored whitespace. Anything else makes the payload Malformed (or Oversize) outright.
fn is_body_char(ch: char) -> bool {
    matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' | '=' | '?' | ' ' | '\t' | '\r' | '\n')
}

/// Everything after the first `;`.
fn parse_body(body: &str) -> Osc52Result {
    if body == "?" {
        return Osc52Result::dropped(Osc52Outcome::Query, 0);
    }
    if body.is_empty() {
        return Osc52Result::dropped(Osc52Outcome::Empty, 0);
    }
    let units = body.encode_utf16().count();
    if units > MAX_BASE64_CHARS {
        return Osc52Result::dropped(Osc52Outcome::Oversize, units);
    }
    // Validate: alphabet characters, then at most two `=`, ignoring whitespace; the count of
    // non-whitespace characters must be a multiple of 4.
    let mut data = 0usize;
    let mut pad = 0usize;
    for byte in body.bytes() {
        if is_ignored_whitespace(byte) {
            continue;
        }
        if byte == b'=' {
            pad += 1;
            if pad > 2 {
                return Osc52Result::dropped(Osc52Outcome::Malformed, 0);
            }
        } else if sextet(byte).is_some() && pad == 0 {
            data += 1;
        } else {
            return Osc52Result::dropped(Osc52Outcome::Malformed, 0);
        }
    }
    let significant = data + pad;
    if significant % 4 != 0 {
        return Osc52Result::dropped(Osc52Outcome::Malformed, 0);
    }
    if significant == 0 {
        return Osc52Result::dropped(Osc52Outcome::Empty, 0);
    }
    let decoded = significant / 4 * 3 - pad;
    if decoded > MAX_DECODED_BYTES {
        return Osc52Result::dropped(Osc52Outcome::Oversize, decoded);
    }
    let mut bytes = Vec::with_capacity(decoded);
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    for value in body.bytes().filter_map(sextet) {
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((accumulator >> bits) & 0xFF).unwrap_or(0));
            accumulator &= (1 << bits) - 1;
        }
    }
    Osc52Result {
        outcome: Osc52Outcome::Copy,
        payload_length: bytes.len(),
        // `from_utf8_lossy` never fails: invalid sequences become U+FFFD, as in .NET.
        text: Some(String::from_utf8_lossy(&bytes).into_owned()),
    }
}

/// The streaming state of the OSC 52 being received (a field of [`super::ExtState`]).
#[derive(Debug, Default)]
pub(crate) struct Osc52State {
    /// The first `;` after the identifier arrived: what follows is the body.
    seen_separator: bool,
    /// The body so far, ASCII only (the characters a body may contain), at most
    /// [`MAX_BASE64_CHARS`] bytes; empty once `invalid` or past the cap.
    body: String,
    /// The body's length in UTF-16 units, counted past the cap (saturating) for the log.
    length: usize,
    /// A character no base64 body contains arrived: the outcome is Malformed (or Oversize).
    invalid: bool,
}

impl Osc52State {
    /// A reset (RIS, `Terminal::reset`) while the sequence may be in flight: free the buffer and
    /// make whatever follows end as Malformed (the part already received is gone, so the rest
    /// must not be read as a payload).
    pub(crate) fn abort(&mut self) {
        *self = Osc52State {
            seen_separator: true,
            body: String::new(),
            length: 0,
            invalid: true,
        };
    }

    fn push(&mut self, ch: char) {
        self.length = self.length.saturating_add(ch.len_utf16());
        if self.length > MAX_BASE64_CHARS {
            // oversize: nothing more is kept
            self.body = String::new();
            return;
        }
        if self.invalid {
            return;
        }
        if is_body_char(ch) {
            self.reserve_one();
            self.body.push(ch);
        } else {
            self.invalid = true;
            self.body = String::new();
        }
    }

    /// Grow by doubling but never past the cap, so the buffer stays within it.
    fn reserve_one(&mut self) {
        let len = self.body.len();
        if self.body.capacity() > len {
            return;
        }
        let target = len
            .saturating_mul(2)
            .max(256)
            .max(len + 1)
            .min(MAX_BASE64_CHARS);
        self.body.reserve_exact(target - len);
    }

    #[cfg(test)]
    pub(crate) fn buffered(&self) -> (usize, usize) {
        (self.body.len(), self.body.capacity())
    }
}

struct Osc52Handler;

impl OscHandler<InputHandler> for Osc52Handler {
    fn start(&mut self, h: &mut InputHandler) {
        h.ext.osc52 = Osc52State::default();
    }

    fn put(&mut self, h: &mut InputHandler, data: &[u32]) {
        let state = &mut h.ext.osc52;
        for &code in data {
            if !state.seen_separator {
                // the selection target is ignored and not stored
                state.seen_separator = code == u32::from(b';');
                continue;
            }
            state.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
    }

    fn end(&mut self, h: &mut InputHandler, success: bool) -> bool {
        let state = std::mem::take(&mut h.ext.osc52);
        if !success {
            return false;
        }
        let result = if !state.seen_separator {
            Osc52Result::dropped(Osc52Outcome::Malformed, 0)
        } else if state.length > MAX_BASE64_CHARS {
            Osc52Result::dropped(Osc52Outcome::Oversize, state.length)
        } else if state.invalid {
            Osc52Result::dropped(Osc52Outcome::Malformed, 0)
        } else {
            parse_body(&state.body)
        };
        match result.outcome {
            Osc52Outcome::Oversize => h.services.log.warn(format_args!(
                "OSC 52 payload of {} exceeds the cap ({} base64 characters, {} decoded bytes), discarded",
                result.payload_length, MAX_BASE64_CHARS, MAX_DECODED_BYTES
            )),
            outcome => h.services.log.debug(format_args!(
                "OSC 52 {}: length {}",
                outcome.name(),
                result.payload_length
            )),
        }
        if let (Osc52Outcome::Copy, Some(text)) = (result.outcome, result.text) {
            if h.ext.clipboard_write {
                h.push_event(HostEvent::ClipboardWrite(text));
            }
        }
        // consumed whatever the outcome: nothing falls through to the unknown-OSC report
        true
    }
}

/// Register the handler on `parser`.
pub(super) fn register(parser: &mut EscapeSequenceParser<InputHandler>) {
    parser.register_osc_handler(OSC_IDENT, Osc52Handler);
}
