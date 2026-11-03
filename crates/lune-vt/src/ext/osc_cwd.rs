//! OSC 7 and OSC 9;9: the shell's working directory (plan M7 step 8, D3, D8). Not in xterm.js
//! 6.0.0. Lunegit reads both from the raw PTY bytes in `OscCwdScanner.cs` (HEAD 33a1dd4); here
//! they are OSC handlers on the parser, so the framing is the parser's, and the result is
//! [`HostEvent::CwdChanged`] (last wins within a drain).
//!
//! The text is untrusted and a later milestone opens tabs in the directory, so beyond the
//! scanner's rules (which `goldens/lunegit-input/osc-cwd.golden` pins row by row):
//!  - a payload over [`MAX_PAYLOAD_BYTES`] UTF-8 bytes is discarded whole (Lunegit's 4096-byte
//!    body, less the `7;` / `9;` the parser has consumed), never truncated;
//!  - **tightening over Lunegit:** a path that contains a C0 control, DEL or a C1 control after
//!    decoding (`%00`, `%0A`, `%C2%85`, ...) is dropped, as is an empty one: no event;
//!  - nothing is logged but lengths.
//!
//! The file URI rules follow `OscCwdScanner.ParseFileUri` as .NET's `System.Uri` behaves on
//! Unix (probed against .NET 10 and, for the rows the golden holds, pinned by it):
//!  - the scheme is `file:` (any case) followed by `//`; `file:/x` and `file:x` are no path;
//!  - the authority (host, `user@`, `:port`) is dropped; a drive authority (`file://C:/x`) is
//!    the drive; four or more slashes (`file:////host/x`) are a UNC host, also dropped;
//!  - the path loses its query and fragment, its `.` and `..` segments are compressed
//!    (`%2e` counts as `.`), and `%XX` escapes are decoded where they form UTF-8 (an escape run
//!    that is not valid UTF-8 stays as written);
//!  - `/C:/x` becomes `C:/x`; a backslash is an ordinary character;
//!  - a text starting with `/` is a path as it is (no decoding, no query), after the same dot
//!    compression; `//host/x` is a UNC host form;
//!  - an authority with a character outside `A-Za-z0-9._~-` (a port, a user, a space) makes
//!    `System.Uri` refuse the URI, and Lunegit then strips scheme and host leniently: the path
//!    from the first `/`, escapes decoded, nothing else; the same here.
//!
//! OSC 9;9 takes the text after `9;9;` (trimmed) verbatim unless it starts with `file:` (any
//! case), when it is parsed as above. Differences from the scanner that are not framing are the
//! tightening above and the cases the host cannot tell apart; they are listed in the test
//! headers.

use crate::engine::HostEvent;
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::OscHandler;

/// Lunegit's cap on a cwd body (`7;file://...` or `9;9;...`), in bytes.
const MAX_BODY_BYTES: usize = 4096;

/// What the parser has already consumed of the body: the identifier digit and the `;`.
const IDENT_BYTES: usize = 2;

/// The longest payload (the text after `7;` or `9;`) kept, in UTF-8 bytes.
pub(super) const MAX_PAYLOAD_BYTES: usize = MAX_BODY_BYTES - IDENT_BYTES;

/// A payload buffered up to a cap in UTF-8 bytes; past it the payload is dropped whole and not
/// buffered any further. The OSC handlers of this module share it.
#[derive(Debug)]
pub(super) struct BoundedPayload {
    text: String,
    bytes: usize,
    cap: usize,
    overflow: bool,
}

impl BoundedPayload {
    pub(super) fn new(cap: usize) -> Self {
        BoundedPayload {
            text: String::new(),
            bytes: 0,
            cap,
            overflow: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.text = String::new();
        self.bytes = 0;
        self.overflow = false;
    }

    /// Append code points; a non-scalar becomes U+FFFD as everywhere in the parser.
    pub(super) fn push_all(&mut self, data: &[u32]) {
        for &code in data {
            if self.overflow {
                return;
            }
            let ch = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
            self.bytes = self.bytes.saturating_add(ch.len_utf8());
            if self.bytes > self.cap {
                self.overflow = true;
                self.text = String::new();
                return;
            }
            self.text.push(ch);
        }
    }

    /// The payload, `None` when it went over the cap; the buffer is cleared either way.
    pub(super) fn take(&mut self) -> Option<String> {
        let result = (!self.overflow).then(|| std::mem::take(&mut self.text));
        self.clear();
        result
    }

    /// Whether the payload went over the cap (for the log).
    pub(super) fn overflowed(&self) -> bool {
        self.overflow
    }
}

// --- path parsing ----------------------------------------------------------------------------

/// Whether a decoded path may become an event: non-empty, within the cap, and free of C0, DEL
/// and C1 controls.
fn is_safe_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= MAX_BODY_BYTES && !path.chars().any(char::is_control)
}

/// The byte of the escape `%XX` at `index`, if there is one.
fn escape_at(bytes: &[u8], index: usize) -> Option<u8> {
    if bytes.get(index) != Some(&b'%') {
        return None;
    }
    let digit = |offset: usize| {
        let byte = *bytes.get(index + offset)?;
        char::from(byte).to_digit(16)
    };
    u8::try_from(digit(1)? * 16 + digit(2)?).ok()
}

/// `%XX` decoding as `Uri.UnescapeDataString`: an escape that is part of a valid UTF-8 sequence
/// becomes its character; a byte that is not stays as written (`%ff`, a cut `%E0%A4`); `%` not
/// followed by two hex digits is literal. One pass: `%2541` is `%41`.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        // the run of escapes starting here
        let mut run: Vec<u8> = Vec::new();
        let mut end = index;
        while let Some(byte) = escape_at(bytes, end) {
            run.push(byte);
            end += 3;
        }
        if run.is_empty() {
            let Some(c) = text.get(index..).and_then(|rest| rest.chars().next()) else {
                return out;
            };
            out.push(c);
            index += c.len_utf8();
        } else {
            push_decoded_run(&mut out, &run, text.get(index..end).unwrap_or(""));
            index = end;
        }
    }
    out
}

/// Append the escape run `bytes` (written as `source`, three characters per byte) as text:
/// valid UTF-8 as characters, each invalid byte as its source escape.
fn push_decoded_run(out: &mut String, bytes: &[u8], source: &str) {
    let mut start = 0;
    while let Some(rest) = bytes.get(start..) {
        if rest.is_empty() {
            return;
        }
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                return;
            }
            Err(error) => {
                let good = error.valid_up_to();
                if let Some(valid) = rest.get(..good).and_then(|b| std::str::from_utf8(b).ok()) {
                    out.push_str(valid);
                }
                let bad = error.error_len().unwrap_or(rest.len() - good);
                let from = (start + good) * 3;
                out.push_str(source.get(from..from + bad * 3).unwrap_or(""));
                start += good + bad;
            }
        }
    }
}

/// Whether a segment is `.` or `..` once `%2e` is read as `.`: 1 and 2, else 0.
fn dot_segments(segment: &str) -> usize {
    match segment.replace("%2e", ".").replace("%2E", ".").as_str() {
        "." => 1,
        ".." => 2,
        _ => 0,
    }
}

/// RFC 3986 `remove_dot_segments` on an absolute path (`/a/./b/../c` is `/a/c`; `/a/b/..` is
/// `/a/`; `..` never climbs above the root).
fn compress_dots(path: &str) -> String {
    let Some(body) = path.strip_prefix('/') else {
        return path.to_owned();
    };
    let segments: Vec<&str> = body.split('/').collect();
    let mut out: Vec<&str> = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        match dot_segments(segment) {
            1 => {
                if last {
                    out.push("");
                }
            }
            2 => {
                out.pop();
                if last {
                    out.push("");
                }
            }
            _ => out.push(segment),
        }
    }
    format!("/{}", out.join("/"))
}

/// `/C:/x` becomes `C:/x` (a letter, then a colon).
fn dos_fix(path: String) -> String {
    let mut chars = path.chars();
    let dos = chars.next() == Some('/')
        && chars.next().is_some_and(char::is_alphabetic)
        && chars.next() == Some(':');
    if dos {
        path.chars().skip(1).collect()
    } else {
        path
    }
}

/// The path of a URI with an accepted authority: query and fragment cut, dots compressed,
/// escapes decoded, DOS fix. `""` is `/`.
fn uri_path(path_part: &str) -> String {
    let end = path_part.find(['?', '#']).unwrap_or(path_part.len());
    let path = path_part.get(..end).unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    dos_fix(percent_decode(&compress_dots(path)))
}

/// A host `System.Uri` accepts in a `file:` URI (a name or an address, no port, no user).
fn is_plain_host(authority: &str) -> bool {
    authority
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
}

/// A drive authority: one letter and a colon.
fn is_drive(authority: &str) -> bool {
    let mut chars = authority.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && chars.next().is_none()
}

/// UNC form: the leading slashes are dropped, the host runs to the next `/`; no host is no path.
fn unc_path(after_slashes: &str, decode: bool) -> Option<String> {
    let (host, path) = match after_slashes.find('/') {
        Some(index) => (
            after_slashes.get(..index).unwrap_or(""),
            after_slashes.get(index..).unwrap_or("/"),
        ),
        None => (after_slashes, ""),
    };
    if host.is_empty() {
        return None;
    }
    Some(if decode {
        uri_path(path)
    } else if path.is_empty() {
        "/".to_owned()
    } else {
        dos_fix(compress_dots(path))
    })
}

/// `OscCwdScanner.ParseFileUri`: the path of `text`, or `None`.
fn parse_file_uri(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    if text.starts_with('/') {
        // an implicit path: literal, a UNC host form from two slashes
        let slashes = text.bytes().take_while(|b| *b == b'/').count();
        if slashes >= 2 {
            return unc_path(text.trim_start_matches('/'), false);
        }
        return Some(dos_fix(compress_dots(text)));
    }
    let scheme = text.get(..5)?;
    if !scheme.eq_ignore_ascii_case("file:") {
        return None;
    }
    let after = text.get(5..)?.strip_prefix("//")?;
    let slashes = after.bytes().take_while(|b| *b == b'/').count();
    if slashes >= 2 {
        return unc_path(after.trim_start_matches('/'), true);
    }
    if slashes == 1 {
        return Some(uri_path(after));
    }
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = after.get(..end).unwrap_or("");
    let path_part = after.get(end..).unwrap_or("");
    if is_plain_host(authority) {
        return Some(uri_path(path_part));
    }
    if is_drive(authority) {
        let path = if path_part.is_empty() {
            String::new()
        } else {
            uri_path(path_part)
        };
        return Some(format!("{authority}{path}"));
    }
    // `System.Uri` refused the authority: the lenient strip of scheme and host
    match after.find('/') {
        Some(index) => {
            let path = after.get(index..)?;
            Some(percent_decode(&dos_fix(path.to_owned())))
        }
        None => None,
    }
}

/// The cwd of an OSC 7 payload (the text after `7;`).
pub(super) fn osc7_path(payload: &str) -> Option<String> {
    parse_file_uri(payload.trim()).filter(|path| is_safe_path(path))
}

/// The cwd of an OSC 9;9 text (after `9;9;`): a `file:` URI is parsed, anything else is the path.
pub(super) fn osc99_path(text: &str) -> Option<String> {
    let raw = text.trim();
    let is_uri = raw
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"));
    let path = if is_uri {
        parse_file_uri(raw)?
    } else {
        raw.to_owned()
    };
    Some(path).filter(|path| is_safe_path(path))
}

// --- handlers --------------------------------------------------------------------------------

struct Osc7Handler {
    payload: BoundedPayload,
}

impl OscHandler<InputHandler> for Osc7Handler {
    fn start(&mut self, _h: &mut InputHandler) {
        self.payload.clear();
    }

    fn put(&mut self, _h: &mut InputHandler, data: &[u32]) {
        self.payload.push_all(data);
    }

    fn end(&mut self, h: &mut InputHandler, success: bool) -> bool {
        let overflowed = self.payload.overflowed();
        let payload = self.payload.take();
        if !success {
            return false;
        }
        match payload {
            None => debug_assert!(overflowed),
            Some(text) => {
                if let Some(path) = osc7_path(&text) {
                    h.push_event(HostEvent::CwdChanged(path));
                }
            }
        }
        if overflowed {
            h.services.log.debug(format_args!(
                "OSC 7 discarded: over {MAX_PAYLOAD_BYTES} bytes"
            ));
        }
        true
    }
}

/// OSC 9 whose payload starts `9;`: the ConEmu / Windows Terminal cwd. Any other OSC 9 form is
/// left to the other OSC 9 handlers (9;4 progress); the unknown-OSC report does not run for a
/// registered identifier, so forms nobody owns produce nothing.
struct Osc99Handler {
    payload: BoundedPayload,
}

impl OscHandler<InputHandler> for Osc99Handler {
    fn start(&mut self, _h: &mut InputHandler) {
        self.payload.clear();
    }

    fn put(&mut self, _h: &mut InputHandler, data: &[u32]) {
        self.payload.push_all(data);
    }

    fn end(&mut self, h: &mut InputHandler, success: bool) -> bool {
        let overflowed = self.payload.overflowed();
        let payload = self.payload.take();
        if !success {
            return false;
        }
        let Some(text) = payload else {
            if overflowed {
                h.services.log.debug(format_args!(
                    "OSC 9 discarded: over {MAX_PAYLOAD_BYTES} bytes"
                ));
            }
            return false;
        };
        let Some(rest) = text.strip_prefix("9;") else {
            return false;
        };
        if let Some(path) = osc99_path(rest) {
            h.push_event(HostEvent::CwdChanged(path));
        }
        true
    }
}

/// Register the OSC 7 and OSC 9;9 handlers on `parser`.
pub(super) fn register(parser: &mut EscapeSequenceParser<InputHandler>) {
    parser.register_osc_handler(
        7,
        Osc7Handler {
            payload: BoundedPayload::new(MAX_PAYLOAD_BYTES),
        },
    );
    parser.register_osc_handler(
        9,
        Osc99Handler {
            payload: BoundedPayload::new(MAX_PAYLOAD_BYTES),
        },
    );
}
