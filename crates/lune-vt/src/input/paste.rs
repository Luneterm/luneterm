//! The paste sanitizer (plan M7 step 6, D7). Port of xterm.js `browser/Clipboard.ts`
//! (`prepareTextForTerminal`, `bracketTextForPaste`) plus a strip of the control characters a
//! pasted text could use to leave the paste brackets (roadmap D9, refined by user decision Q22).
//!
//! The threat: clipboard text is untrusted. With bracketed paste on, the application reads
//! everything between `ESC [ 200 ~` and `ESC [ 201 ~` as inert text; a pasted `ESC [ 201 ~` ends
//! the paste early and the rest runs as typed input, and a C1 control such as U+009B (an 8-bit
//! CSI to programs that honour C1) can do the same without an ESC. So, bracketed:
//!
//! 1. line breaks `\r?\n` become `\r`;
//! 2. every `ESC [ 200 ~` and `ESC [ 201 ~` is removed, in one left-to-right pass;
//! 3. every C0 control except HT (0x09) and CR (0x0D) is dropped, so no ESC survives (this also
//!    covers a marker the removal in step 2 reassembled from fragments, such as
//!    `ESC [ 20 ESC [ 201 ~ 0 ~`), and so is every C1 control U+0080..=U+009F. DEL (0x7F) is not
//!    a C0 control and is kept;
//! 4. the result is wrapped in `ESC [ 200 ~ ... ESC [ 201 ~`.
//!
//! Invariant of the bracketed output: between the markers there is no `0x1B`, no byte below
//! `0x20` except `0x09` and `0x0D`, and no UTF-8 encoding of U+0080..=U+009F. Not bracketed there
//! is nothing to protect (the application has no paste boundary), so only step 1 applies
//! (roadmap D9). The output is never longer than the input plus twelve bytes.

const ESC: char = '\u{1b}';
const START: &str = "\u{1b}[200~";
const END: &str = "\u{1b}[201~";

/// The bytes to write to the PTY for pasting `text`; see the module notes.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = normalize_line_breaks(text);
    if !bracketed {
        return normalized.into_bytes();
    }
    let mut out = String::with_capacity(normalized.len() + START.len() + END.len());
    out.push_str(START);
    push_sanitized(&mut out, &normalized);
    out.push_str(END);
    out.into_bytes()
}

/// xterm.js `prepareTextForTerminal`: `\r?\n` -> `\r`, left to right.
fn normalize_line_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                out.push('\r');
            }
            '\n' => out.push('\r'),
            other => out.push(other),
        }
    }
    out
}

/// Steps 2 and 3 of the module notes.
fn push_sanitized(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == ESC {
            if let Some(after) = rest.strip_prefix(START).or_else(|| rest.strip_prefix(END)) {
                rest = after;
                continue;
            }
        }
        rest = rest.get(c.len_utf8()..).unwrap_or("");
        if is_dropped(c) {
            continue;
        }
        out.push(c);
    }
}

/// C0 controls other than HT and CR, and the C1 controls.
fn is_dropped(c: char) -> bool {
    match c {
        '\t' | '\r' => false,
        '\u{0}'..='\u{1f}' | '\u{80}'..='\u{9f}' => true,
        _ => false,
    }
}

#[cfg(test)]
#[path = "paste_tests.rs"]
mod tests;
