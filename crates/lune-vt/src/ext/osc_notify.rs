//! Notification requests: plain OSC 9 and `OSC 777;notify` (bell-notifications plan step 1, D4,
//! D5, D10). An extension of this engine, not in xterm.js 6.0.0. Registered only with
//! [`crate::StandardExtensions::notifications`] on; the result is [`HostEvent::Notification`].
//!
//! Grammar (the payload is the text after the identifier and its `;`):
//!  - **OSC 9**, payload `<message>`: a message unless its first field (up to the first `;`, or
//!    all of it) is one or more ASCII digits, the shape of every ConEmu sub-command (`9;4;...`
//!    progress, `9;9;...` cwd, `9;12`, a bare `9;404`). So `9;3 tests failed` and `9;v2;x` are
//!    messages and `9;4;1;50` is not. An empty payload is nothing. The event has an empty title
//!    and the whole payload (`;` included) as its body. The handler is registered before the
//!    cwd and progress handlers, so it sees only what they did not take; it returns `false` for
//!    a form it does not own.
//!  - **OSC 777**, payload `notify;<title>[;<body>]`: the first field must be exactly `notify`
//!    (case-sensitive); the title is the second field and the body is everything after the
//!    second `;` (it may hold `;`), empty when there is none. An event is emitted when the
//!    title or the body is non-empty. Every OSC 777 returns `true`: with this handler on, no
//!    `UnknownOsc` is reported for identifier 777 and nothing else is done with it.
//!
//! The text is untrusted program output. A payload over [`MAX_PAYLOAD_BYTES`] UTF-8 bytes is
//! discarded whole, never truncated; an aborted OSC (CAN, or `success == false`) emits nothing;
//! the queue keeps at most [`crate::input_handler::NOTIFICATION_LIMIT`] events per drain. The
//! text is never logged: the one debug line a discarded payload gets holds a byte count. The
//! host sanitises the text before it becomes UI text; this layer only bounds it.

use crate::engine::{HostEvent, Notification};
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::OscHandler;

use super::osc_cwd::BoundedPayload;

/// The longest payload kept (the text after the identifier), in UTF-8 bytes.
pub(super) const MAX_PAYLOAD_BYTES: usize = 4096;

/// The notification of an OSC 9 payload, `None` when it is not a message.
pub(super) fn parse_osc9(payload: &str) -> Option<Notification> {
    let first = payload.split(';').next().unwrap_or_default();
    if payload.is_empty() || (!first.is_empty() && first.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    Some(Notification {
        title: String::new(),
        body: payload.to_owned(),
    })
}

/// The notification of an OSC 777 payload, `None` when it is not `notify` or has no text.
pub(super) fn parse_osc777(payload: &str) -> Option<Notification> {
    let mut fields = payload.splitn(3, ';');
    if fields.next() != Some("notify") {
        return None;
    }
    let title = fields.next().unwrap_or_default();
    let body = fields.next().unwrap_or_default();
    if title.is_empty() && body.is_empty() {
        return None;
    }
    Some(Notification {
        title: title.to_owned(),
        body: body.to_owned(),
    })
}

/// One handler for either identifier: `parse` says what the payload means.
struct NotifyHandler {
    payload: BoundedPayload,
    ident: u16,
    parse: fn(&str) -> Option<Notification>,
    /// What the handler reports for a payload it does not turn into an event (OSC 9: `false`,
    /// pass it on; OSC 777: `true`, it is ours).
    owns_rest: bool,
}

impl OscHandler<InputHandler> for NotifyHandler {
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
                    "OSC {} notification discarded: over {MAX_PAYLOAD_BYTES} bytes",
                    self.ident
                ));
            }
            return self.owns_rest;
        };
        match (self.parse)(&text) {
            Some(notification) => {
                h.push_event(HostEvent::Notification(notification));
                true
            }
            None => self.owns_rest,
        }
    }
}

/// Register the OSC 9 and OSC 777 handlers on `parser`. Call before the cwd and progress
/// handlers (newer handlers run first).
pub(super) fn register(parser: &mut EscapeSequenceParser<InputHandler>) {
    parser.register_osc_handler(
        9,
        NotifyHandler {
            payload: BoundedPayload::new(MAX_PAYLOAD_BYTES),
            ident: 9,
            parse: parse_osc9,
            owns_rest: false,
        },
    );
    parser.register_osc_handler(
        777,
        NotifyHandler {
            payload: BoundedPayload::new(MAX_PAYLOAD_BYTES),
            ident: 777,
            parse: parse_osc777,
            owns_rest: true,
        },
    );
}
