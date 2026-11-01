//! Port of xterm.js src/common/parser/OscParser.ts @ f447274.
//!
//! Rust notes (no behaviour change; see `docs/porting-xterm.md` and plan M4 D1/D3):
//!  - Every call takes the host `&mut C`, which is handed on to the handlers.
//!  - `registerHandler` returns a [`HandlerId`]; `dispose()` of upstream's disposable is
//!    [`OscParser::dispose_handler`].
//!  - Upstream's `_active` is a reference to the registry's handler array, so a dispose between
//!    chunks of one sequence affects it and `clearHandler` detaches it. `Active` reproduces
//!    both: it names the ident while the list is in the registry and owns the list once it was
//!    cleared mid-sequence.
//!  - No async handlers (D3): the `_stack` / `paused` / `fallThrough` branches are not ported.
//!  - `_id` grows with saturating `i32` arithmetic: an id longer than 10 digits sticks at
//!    `i32::MAX` instead of growing without bound (upstream JS number), so it can only match a
//!    handler registered at `i32::MAX`.

use std::collections::HashMap;
use std::fmt;

use super::constants::{OscState, PAYLOAD_LIMIT};
use super::types::{
    HandlerId, OscFallbackAction, OscFallbackFn, OscHandler, OscStringFn, remove_entry,
};

type Entry<C> = (HandlerId, Box<dyn OscHandler<C>>);

/// The handlers of the running sequence (upstream `_active`).
enum Active<C> {
    /// `EMPTY_HANDLERS`: no registry list existed for the id at start.
    Empty,
    /// The registry list of this id.
    Ident(i32),
    /// The list of this id, removed from the registry by `clear_handler` mid-sequence.
    Detached(Vec<Entry<C>>),
}

/// OSC sub-parser (upstream `OscParser`).
pub struct OscParser<C> {
    state: OscState,
    active: Active<C>,
    id: i32,
    handlers: HashMap<i32, Vec<Entry<C>>>,
    handler_fb: OscFallbackFn<C>,
    next_id: u64,
}

impl<C> fmt::Debug for OscParser<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OscParser")
            .field("state", &self.state)
            .field("id", &self.id)
            .field("handler_idents", &self.handlers.len())
            .finish_non_exhaustive()
    }
}

impl<C> Default for OscParser<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> OscParser<C> {
    /// A parser with no handlers and a no-op fallback.
    pub fn new() -> Self {
        OscParser {
            state: OscState::Start,
            active: Active::Empty,
            id: -1,
            handlers: HashMap::new(),
            handler_fb: Box::new(|_, _, _| {}),
            next_id: 0,
        }
    }

    /// Register `handler` for OSC `ident`; newer handlers run first.
    pub fn register_handler(
        &mut self,
        ident: i32,
        handler: impl OscHandler<C> + 'static,
    ) -> HandlerId {
        let id = HandlerId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.register_handler_with_id(ident, id, Box::new(handler));
        id
    }

    /// Registration with an id allocated by the owning `EscapeSequenceParser`, so ids are
    /// unique across all of its handler tables.
    pub(crate) fn register_handler_with_id(
        &mut self,
        ident: i32,
        id: HandlerId,
        handler: Box<dyn OscHandler<C>>,
    ) {
        self.handlers.entry(ident).or_default().push((id, handler));
    }

    /// Remove the handler `id` (upstream `IDisposable.dispose`). Returns whether it was found;
    /// a second call is a no-op.
    pub fn dispose_handler(&mut self, id: HandlerId) -> bool {
        if let Active::Detached(list) = &mut self.active {
            if remove_entry(list, id) {
                return true;
            }
        }
        self.handlers
            .values_mut()
            .any(|list| remove_entry(list, id))
    }

    /// Remove every handler of `ident`.
    pub fn clear_handler(&mut self, ident: i32) {
        if let Some(list) = self.handlers.remove(&ident) {
            if matches!(self.active, Active::Ident(active) if active == ident) {
                self.active = Active::Detached(list);
            }
        }
    }

    /// Set the handler called for idents without registered handlers.
    pub fn set_handler_fallback(
        &mut self,
        handler: impl FnMut(&mut C, i32, OscFallbackAction<'_>) + 'static,
    ) {
        self.handler_fb = Box::new(handler);
    }

    /// The handlers of the running sequence, newest last.
    fn active_list(&mut self) -> &mut [Entry<C>] {
        match &mut self.active {
            Active::Empty => &mut [],
            Active::Ident(ident) => self
                .handlers
                .get_mut(ident)
                .map_or(&mut [], Vec::as_mut_slice),
            Active::Detached(list) => list,
        }
    }

    /// Abort a running sequence: handlers that already saw a payload get `end(false)`.
    pub fn reset(&mut self, host: &mut C) {
        // force cleanup handlers if payload was already sent
        if self.state == OscState::Payload {
            for (_, handler) in self.active_list().iter_mut().rev() {
                handler.end(host, false);
            }
        }
        self.active = Active::Empty;
        self.id = -1;
        self.state = OscState::Start;
    }

    fn start_handlers(&mut self, host: &mut C) {
        self.active = if self.handlers.contains_key(&self.id) {
            Active::Ident(self.id)
        } else {
            Active::Empty
        };
        let id = self.id;
        let list = self.active_list();
        if list.is_empty() {
            (self.handler_fb)(host, id, OscFallbackAction::Start);
        } else {
            for (_, handler) in list.iter_mut().rev() {
                handler.start(host);
            }
        }
    }

    fn put_handlers(&mut self, host: &mut C, data: &[u32]) {
        let id = self.id;
        let list = self.active_list();
        if list.is_empty() {
            (self.handler_fb)(host, id, OscFallbackAction::Put(data));
        } else {
            for (_, handler) in list.iter_mut().rev() {
                handler.put(host, data);
            }
        }
    }

    /// Start a new OSC command (resets leftover handlers).
    pub fn start(&mut self, host: &mut C) {
        // always reset leftover handlers
        self.reset(host);
        self.state = OscState::Id;
    }

    /// Put data to the current OSC command.
    ///
    /// Expects the identifier of the OSC command in the form `OSC id ; payload ST/BEL`.
    /// Payload chunks are not further processed and get directly passed to the handlers.
    pub fn put(&mut self, host: &mut C, data: &[u32]) {
        if self.state == OscState::Abort {
            return;
        }
        let mut rest = data;
        if self.state == OscState::Id {
            while let Some((&code, tail)) = rest.split_first() {
                rest = tail;
                if code == 0x3b {
                    self.state = OscState::Payload;
                    self.start_handlers(host);
                    break;
                }
                if !(0x30..=0x39).contains(&code) {
                    self.state = OscState::Abort;
                    return;
                }
                if self.id == -1 {
                    self.id = 0;
                }
                // LUNETERM: saturating, see the module docs (upstream grows unbounded).
                let digit = i32::try_from(code - 48).unwrap_or(0);
                self.id = self.id.saturating_mul(10).saturating_add(digit);
            }
        }
        if self.state == OscState::Payload && !rest.is_empty() {
            self.put_handlers(host, rest);
        }
    }

    /// Indicates the end of an OSC command. Whether the OSC got aborted or finished normally is
    /// indicated by `success`.
    pub fn end(&mut self, host: &mut C, success: bool) {
        if self.state == OscState::Start {
            return;
        }
        // do nothing if command was faulty
        if self.state != OscState::Abort {
            // if we are still in ID state and get an early end means that the command has no
            // payload thus we still have to announce START and send END right after
            if self.state == OscState::Id {
                self.start_handlers(host);
            }
            let id = self.id;
            let list = self.active_list();
            if list.is_empty() {
                (self.handler_fb)(host, id, OscFallbackAction::End(success));
            } else {
                // newest first until one returns true, then cleanup left over handlers: we
                // always have to call `end` for proper cleanup, here `success` false tells a
                // handler not to execute
                let mut handled = false;
                for (_, handler) in list.iter_mut().rev() {
                    if handled {
                        handler.end(host, false);
                    } else {
                        handled = handler.end(host, success);
                    }
                }
            }
        }
        self.active = Active::Empty;
        self.id = -1;
        self.state = OscState::Start;
    }
}

/// Convenience OSC handler from a string callback (upstream class `OscHandler`).
///
/// Buffers the payload as a `String` and calls the callback once on `end(true)`. The payload is
/// limited to [`PAYLOAD_LIMIT`] UTF-16 code units, upstream's `string.length`; a longer payload
/// is dropped (the callback is not called) and buffering stops for the rest of the sequence.
pub struct OscStringHandler<C> {
    data: String,
    /// `data.length` in UTF-16 code units.
    utf16_len: usize,
    hit_limit: bool,
    handler: OscStringFn<C>,
}

impl<C> fmt::Debug for OscStringHandler<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OscStringHandler")
            .field("utf16_len", &self.utf16_len)
            .field("hit_limit", &self.hit_limit)
            .finish_non_exhaustive()
    }
}

impl<C> OscStringHandler<C> {
    /// Wrap `handler(host, payload) -> handled`.
    pub fn new(handler: impl FnMut(&mut C, &str) -> bool + 'static) -> Self {
        OscStringHandler {
            data: String::new(),
            utf16_len: 0,
            hit_limit: false,
            handler: Box::new(handler),
        }
    }
}

impl<C> OscHandler<C> for OscStringHandler<C> {
    fn start(&mut self, _host: &mut C) {
        self.data = String::new();
        self.utf16_len = 0;
        self.hit_limit = false;
    }

    fn put(&mut self, _host: &mut C, data: &[u32]) {
        if self.hit_limit {
            return;
        }
        append_utf32(&mut self.data, &mut self.utf16_len, data);
        if self.utf16_len > PAYLOAD_LIMIT {
            self.data = String::new();
            self.utf16_len = 0;
            self.hit_limit = true;
        }
    }

    fn end(&mut self, host: &mut C, success: bool) -> bool {
        let ret = if self.hit_limit {
            false
        } else if success {
            (self.handler)(host, &self.data)
        } else {
            false
        };
        self.data = String::new();
        self.utf16_len = 0;
        self.hit_limit = false;
        ret
    }
}

/// `data += utf32ToString(chunk)`, counting UTF-16 units into `utf16_len`. Same mapping as
/// `input::text_decoder::utf32_to_string` (non-scalars become U+FFFD).
pub(crate) fn append_utf32(data: &mut String, utf16_len: &mut usize, chunk: &[u32]) {
    for &code in chunk {
        let ch = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
        data.push(ch);
        *utf16_len = utf16_len.saturating_add(ch.len_utf16());
    }
}

#[cfg(test)]
#[path = "osc_parser_tests.rs"]
mod tests;
