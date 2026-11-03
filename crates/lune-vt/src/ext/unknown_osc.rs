//! The unknown-OSC report, Lunegit `driver.js` `setOscHandlerFallback` (HEAD 33a1dd4).
//!
//! Replaces the core's OSC fallback with one that still logs the sequence (identifier and
//! payload length, never the payload: the core fallback, called first) and also queues
//! [`HostEvent::UnknownOsc`] when the sequence ends: identifier and payload length in code
//! points, never the payload. An OSC with no identifier (`ESC ] BEL`, identifier -1) is not
//! reported. The queue keeps at most [`crate::input_handler::UNKNOWN_OSC_LIMIT`] per drain.
//!
//! The fallback runs only for identifiers without a registered handler, so every identifier an
//! extension handles drops out of the report on its own.

use crate::engine::HostEvent;
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::OscFallbackAction;

/// Install the fallback.
pub(super) fn register(parser: &mut EscapeSequenceParser<InputHandler>) {
    parser.set_osc_handler_fallback(fallback);
}

fn fallback(h: &mut InputHandler, ident: i32, action: OscFallbackAction<'_>) {
    InputHandler::log_unknown_osc(h, ident, action);
    match action {
        OscFallbackAction::Start => h.ext.unknown_osc_len = 0,
        OscFallbackAction::Put(data) => {
            h.ext.unknown_osc_len = h.ext.unknown_osc_len.saturating_add(data.len());
        }
        OscFallbackAction::End(_) => {
            // xterm leaves the identifier at -1 for an introducer with none
            if ident >= 0 {
                let len = h.ext.unknown_osc_len;
                h.push_event(HostEvent::UnknownOsc { ident, len });
            }
            h.ext.unknown_osc_len = 0;
        }
    }
}
