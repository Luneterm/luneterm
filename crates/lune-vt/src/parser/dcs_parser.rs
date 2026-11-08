//! Port of xterm.js src/common/parser/DcsParser.ts @ f447274.
//!
//! Rust notes (no behaviour change; see `docs/porting-xterm.md` and plan M4 D1/D3):
//!  - Every call takes the host `&mut C`, which is handed on to the handlers.
//!  - `registerHandler` returns a [`HandlerId`]; `dispose()` of upstream's disposable is
//!    [`DcsParser::dispose_handler`].
//!  - `_active` is modelled as in `osc_parser.rs`: it names the ident while the list is in the
//!    registry and owns the list once `clear_handler` detached it mid-sequence.
//!  - No async handlers (D3): the `_stack` / `paused` / `fallThrough` branches are not ported.

use std::collections::HashMap;
use std::fmt;

use super::constants::PAYLOAD_LIMIT;
use super::osc_parser::append_utf32;
use super::params::Params;
use super::types::{
    DcsFallbackAction, DcsFallbackFn, DcsHandler, DcsStringFn, HandlerId, remove_entry,
};

type Entry<C> = (HandlerId, Box<dyn DcsHandler<C> + Send>);

/// The handlers of the running sequence (upstream `_active`).
enum Active<C> {
    /// `EMPTY_HANDLERS`: no registry list existed for the ident at hook.
    Empty,
    /// The registry list of this ident.
    Ident(u32),
    /// The list of this ident, removed from the registry by `clear_handler` mid-sequence.
    Detached(Vec<Entry<C>>),
}

/// DCS sub-parser (upstream `DcsParser`).
pub struct DcsParser<C> {
    handlers: HashMap<u32, Vec<Entry<C>>>,
    active: Active<C>,
    ident: u32,
    handler_fb: DcsFallbackFn<C>,
    next_id: u64,
}

impl<C> fmt::Debug for DcsParser<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DcsParser")
            .field("ident", &self.ident)
            .field("handler_idents", &self.handlers.len())
            .finish_non_exhaustive()
    }
}

impl<C> Default for DcsParser<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> DcsParser<C> {
    /// A parser with no handlers and a no-op fallback.
    pub fn new() -> Self {
        DcsParser {
            handlers: HashMap::new(),
            active: Active::Empty,
            ident: 0,
            handler_fb: Box::new(|_, _, _| {}),
            next_id: 0,
        }
    }

    /// Register `handler` for the numeric function identifier `ident`; newer handlers run
    /// first.
    pub fn register_handler(
        &mut self,
        ident: u32,
        handler: impl DcsHandler<C> + Send + 'static,
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
        ident: u32,
        id: HandlerId,
        handler: Box<dyn DcsHandler<C> + Send>,
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
    pub fn clear_handler(&mut self, ident: u32) {
        if let Some(list) = self.handlers.remove(&ident) {
            if matches!(self.active, Active::Ident(active) if active == ident) {
                self.active = Active::Detached(list);
            }
        }
    }

    /// Set the handler called for idents without registered handlers.
    pub fn set_handler_fallback(
        &mut self,
        handler: impl FnMut(&mut C, u32, DcsFallbackAction<'_>) + Send + 'static,
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

    /// Abort a running sequence: every active handler gets `unhook(false)`.
    pub fn reset(&mut self, host: &mut C) {
        // force cleanup leftover handlers
        for (_, handler) in self.active_list().iter_mut().rev() {
            handler.unhook(host, false);
        }
        self.active = Active::Empty;
        self.ident = 0;
    }

    /// Start the DCS command `ident` (resets leftover handlers). `params` is borrowed.
    pub fn hook(&mut self, host: &mut C, ident: u32, params: &Params) {
        // always reset leftover handlers
        self.reset(host);
        self.ident = ident;
        self.active = if self.handlers.contains_key(&ident) {
            Active::Ident(ident)
        } else {
            Active::Empty
        };
        let list = self.active_list();
        if list.is_empty() {
            (self.handler_fb)(host, ident, DcsFallbackAction::Hook(params));
        } else {
            for (_, handler) in list.iter_mut().rev() {
                handler.hook(host, params);
            }
        }
    }

    /// Payload chunk of the current DCS command.
    pub fn put(&mut self, host: &mut C, data: &[u32]) {
        let ident = self.ident;
        let list = self.active_list();
        if list.is_empty() {
            (self.handler_fb)(host, ident, DcsFallbackAction::Put(data));
        } else {
            for (_, handler) in list.iter_mut().rev() {
                handler.put(host, data);
            }
        }
    }

    /// End of the current DCS command; `success` tells whether it finished normally.
    pub fn unhook(&mut self, host: &mut C, success: bool) {
        let ident = self.ident;
        let list = self.active_list();
        if list.is_empty() {
            (self.handler_fb)(host, ident, DcsFallbackAction::Unhook(success));
        } else {
            // newest first until one returns true, then cleanup left over handlers
            let mut handled = false;
            for (_, handler) in list.iter_mut().rev() {
                if handled {
                    handler.unhook(host, false);
                } else {
                    handled = handler.unhook(host, success);
                }
            }
        }
        self.active = Active::Empty;
        self.ident = 0;
    }
}

/// Set `params` to upstream's `EMPTY_PARAMS` (`[0]`, ZDM).
fn set_empty_params(params: &mut Params) {
    params.reset();
    // 0 is a valid param value, so this cannot fail.
    let _ = params.add_param(0);
}

/// Convenience DCS handler from a single callback (upstream class `DcsHandler`).
///
/// Buffers the payload as a `String` and calls the callback once on `unhook(true)` with the
/// params cloned at `hook` (the parser's params are only borrowed and change before `unhook`).
/// The payload is limited to [`PAYLOAD_LIMIT`] UTF-16 code units, upstream's `string.length`;
/// a longer payload is dropped (the callback is not called) and buffering stops for the rest
/// of the sequence.
pub struct DcsStringHandler<C> {
    data: String,
    /// `data.length` in UTF-16 code units.
    utf16_len: usize,
    params: Params,
    hit_limit: bool,
    handler: DcsStringFn<C>,
}

impl<C> fmt::Debug for DcsStringHandler<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DcsStringHandler")
            .field("utf16_len", &self.utf16_len)
            .field("params", &self.params)
            .field("hit_limit", &self.hit_limit)
            .finish_non_exhaustive()
    }
}

impl<C> DcsStringHandler<C> {
    /// Wrap `handler(host, payload, params) -> handled`.
    pub fn new(handler: impl FnMut(&mut C, &str, &Params) -> bool + Send + 'static) -> Self {
        let mut params = Params::new();
        set_empty_params(&mut params);
        DcsStringHandler {
            data: String::new(),
            utf16_len: 0,
            params,
            hit_limit: false,
            handler: Box::new(handler),
        }
    }
}

impl<C> DcsHandler<C> for DcsStringHandler<C> {
    fn hook(&mut self, _host: &mut C, params: &Params) {
        // since we need to preserve params until `unhook`, we have to clone it (only borrowed
        // from parser and spans multiple parser states); clone only if we have non empty
        // params, otherwise stick with the default
        if params.length() > 1 || params.get(0).unwrap_or(0) != 0 {
            self.params.clone_from(params);
        } else {
            set_empty_params(&mut self.params);
        }
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

    fn unhook(&mut self, host: &mut C, success: bool) -> bool {
        let ret = if self.hit_limit {
            false
        } else if success {
            (self.handler)(host, &self.data, &self.params)
        } else {
            false
        };
        set_empty_params(&mut self.params);
        self.data = String::new();
        self.utf16_len = 0;
        self.hit_limit = false;
        ret
    }
}

#[cfg(test)]
#[path = "dcs_parser_tests.rs"]
mod tests;
