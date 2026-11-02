//! OSC 9;4: the ConEmu / Windows Terminal progress report (plan M7 step 8, D3, D8). Not in
//! xterm.js 6.0.0. Lunegit reads it from the raw PTY bytes in `Osc94ProgressScanner.cs` (HEAD
//! 33a1dd4); here it is an OSC 9 handler, so the framing is the parser's, and the result is
//! [`HostEvent::Progress`] (last wins within a drain).
//!
//! `9;4;<state>[;<percent>[;...]]`: the state is ASCII digits that fit an `i32` (anything else:
//! the sequence is ignored), 0 to 4 meaning none, normal, error, indeterminate and warning and
//! any other value none; the percent is the next field, ASCII digits that fit an `i32`, else 0,
//! clamped to 0 to 100; later fields are ignored. A payload over [`MAX_PAYLOAD_BYTES`] bytes is
//! discarded whole (Lunegit's 64-byte body, less the `9;` the parser has consumed).
//!
//! The other OSC 9 forms belong to the cwd handler (`9;9;`) or are consumed without effect.

use crate::engine::{HostEvent, Progress, ProgressState};
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::OscHandler;

use super::osc_cwd::BoundedPayload;

/// Lunegit's cap on a progress body (`9;4;...`), in bytes.
const MAX_BODY_BYTES: usize = 64;

/// The longest payload (the text after `9;`) kept, in UTF-8 bytes.
pub(super) const MAX_PAYLOAD_BYTES: usize = MAX_BODY_BYTES - 2;

/// `TryParseInt`: ASCII digits only, at least one, fitting an `i32` (leading zeros allowed).
fn parse_int(token: &str) -> Option<i32> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse::<i32>().ok()
}

/// The report of an OSC 9 payload (the text after `9;`), `None` when it is not a well-formed
/// 9;4 sequence.
pub(super) fn parse_progress(payload: &str) -> Option<Progress> {
    let mut fields = payload.split(';');
    if fields.next() != Some("4") {
        return None;
    }
    let state = match parse_int(fields.next()?)? {
        1 => ProgressState::Normal,
        2 => ProgressState::Error,
        3 => ProgressState::Indeterminate,
        4 => ProgressState::Warning,
        // 0, and any other value: cleared
        _ => ProgressState::None,
    };
    let percent = fields
        .next()
        .and_then(parse_int)
        .map_or(0, |value| value.clamp(0, 100));
    Some(Progress {
        state,
        percent: u8::try_from(percent).unwrap_or(0),
    })
}

struct ProgressHandler {
    payload: BoundedPayload,
}

impl OscHandler<InputHandler> for ProgressHandler {
    fn start(&mut self, _h: &mut InputHandler) {
        self.payload.clear();
    }

    fn put(&mut self, _h: &mut InputHandler, data: &[u32]) {
        self.payload.push_all(data);
    }

    fn end(&mut self, h: &mut InputHandler, success: bool) -> bool {
        let payload = self.payload.take();
        if !success {
            return false;
        }
        match payload.as_deref().map(parse_progress) {
            Some(Some(progress)) => {
                h.push_event(HostEvent::Progress(progress));
                true
            }
            // not a 9;4 form, or over the cap: not handled here; `false` passes it on to the other OSC 9 handler (9;9 cwd)
            _ => false,
        }
    }
}

/// Register the OSC 9;4 handler on `parser` (identifier 9, alongside the cwd handler).
pub(super) fn register(parser: &mut EscapeSequenceParser<InputHandler>) {
    parser.register_osc_handler(
        9,
        ProgressHandler {
            payload: BoundedPayload::new(MAX_PAYLOAD_BYTES),
        },
    );
}
