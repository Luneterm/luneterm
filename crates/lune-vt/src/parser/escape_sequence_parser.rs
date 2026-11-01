//! Port of xterm.js src/common/parser/EscapeSequenceParser.ts @ f447274.
//!
//! This parser implements the ANSI/DEC compatible parser described by Paul Williams
//! (<https://vt100.net/emu/dec_ansi_parser>). To implement custom ANSI compliant escape
//! sequences it is not needed to alter this parser, instead register a custom handler.
//!
//! The parser is hardcoded to operate in ZDM (Zero Default Mode) as suggested by the original
//! parser, thus empty parameters are set to 0. Sub parameters are supported in digital
//! parameters separated by colons; empty sub parameters are set to -1 (no ZDM for sub params).
//!
//! About prefix and intermediate bytes: only one prefix byte is allowed as first parameter
//! byte, range 0x3c .. 0x3f; at most two intermediates are respected, range 0x20 .. 0x2f.
//!
//! Rust shape (plan M4, decision D1; see `types.rs`):
//!  - `EscapeSequenceParser<C>` is generic over the host state; `parse` and `reset` borrow the
//!    host for one call and hand it to the handlers. Handlers are boxed `FnMut(&mut C, ..)`
//!    closures (CSI, ESC, print, execute, fallbacks) or [`OscHandler`] / [`DcsHandler`] trait
//!    objects; they own what they capture and never reference the host or the parser.
//!  - `precedingJoinState` is the host's ([`ParserHost`]); the parser zeroes it where upstream
//!    assigns `this.precedingJoinState = 0`.
//!  - Registration returns a [`HandlerId`]; [`EscapeSequenceParser::dispose_handler`] replaces
//!    `IDisposable.dispose` (idempotent). Upstream's `throw`s in `_identifier` are
//!    [`IdentifierError`] (plan decision D5).
//!  - No async handlers (plan decision D3): `_parseStack`, `_preserveStack` and the
//!    `promiseResult` resume path are not ported.

use std::collections::HashMap;
use std::fmt;

use super::constants::ParserState;
use super::dcs_parser::DcsParser;
use super::osc_parser::OscParser;
use super::params::Params;
use super::types::{
    CsiFallbackFn, CsiFn, DcsFallbackAction, DcsHandler, ErrorFn, EscFallbackFn, EscFn,
    ExecuteFallbackFn, ExecuteFn, FunctionIdentifier, HandlerId, IdentifierError,
    OscFallbackAction, OscHandler, ParserHost, ParsingState, PrintFn, remove_entry,
};

/// Final byte range of CSI and DCS function identifiers (upstream default `finalRange`).
pub const CSI_FINAL_RANGE: [u32; 2] = [0x40, 0x7e];
/// Final byte range of ESC function identifiers.
pub const ESC_FINAL_RANGE: [u32; 2] = [0x30, 0x7e];

/// Numeric function identifier of `id` (upstream `_identifier`): prefix, intermediates and
/// final byte packed one byte each, most significant first. Every upstream `throw` is an
/// `Err`.
pub fn identifier(id: &FunctionIdentifier, final_range: [u32; 2]) -> Result<u32, IdentifierError> {
    let mut res: u32 = 0;
    if let Some(prefix) = id.prefix {
        // `charCodeAt(0)`: the first UTF-16 unit
        res = u32::from(first_utf16_unit(prefix));
        // upstream: `res && 0x3c > res || res > 0x3f`, so a NUL prefix means "no prefix"
        if (res != 0 && 0x3c > res) || res > 0x3f {
            return Err(IdentifierError::PrefixRange);
        }
    }
    if !id.intermediates.is_empty() {
        // `intermediates.length` counts UTF-16 units
        if id.intermediates.encode_utf16().count() > 2 {
            return Err(IdentifierError::TooManyIntermediates);
        }
        for unit in id.intermediates.encode_utf16() {
            let intermediate = u32::from(unit);
            if !(0x20..=0x2f).contains(&intermediate) {
                return Err(IdentifierError::IntermediateRange);
            }
            res = res.wrapping_shl(8) | intermediate;
        }
    }
    let [lo, hi] = final_range;
    let final_code = u32::from(first_utf16_unit(id.final_byte));
    // a non-BMP final is two UTF-16 units upstream ("final must be a single byte"); its first
    // unit is a surrogate, outside every final range, so it is rejected here as well
    if id.final_byte.len_utf16() != 1 || lo > final_code || final_code > hi {
        return Err(IdentifierError::FinalRange { lo, hi });
    }
    Ok(res.wrapping_shl(8) | final_code)
}

/// String of the numeric function identifier `ident` (upstream `identToString`), for
/// debugging output in fallback handlers. Each byte becomes the Latin-1 character of that
/// value. A full back translation to [`FunctionIdentifier`] is not implemented.
///
/// LUNETERM: logical shifts on `u32`, so this terminates for every value (upstream's `>>=`
/// loops forever on a negative int32).
pub fn ident_to_string(ident: u32) -> String {
    let mut res = Vec::new();
    let mut ident = ident;
    while ident != 0 {
        res.push(char::from((ident & 0xff) as u8));
        ident >>= 8;
    }
    res.iter().rev().collect()
}

fn first_utf16_unit(ch: char) -> u16 {
    let mut buf = [0u16; 2];
    ch.encode_utf16(&mut buf).first().copied().unwrap_or(0)
}

/// First UTF-16 unit of `flag` (upstream `flag.charCodeAt(0)`), the execute handler key.
fn execute_key(flag: char) -> u32 {
    u32::from(first_utf16_unit(flag))
}

type CsiEntry<C> = (HandlerId, CsiFn<C>);
type EscEntry<C> = (HandlerId, EscFn<C>);

/// Escape sequence parser (upstream `EscapeSequenceParser`).
pub struct EscapeSequenceParser<C: ParserHost> {
    initial_state: ParserState,
    current_state: ParserState,

    // buffers over several parse calls
    params: Params,
    collect: u32,

    // handler lookup containers
    print_handler: PrintFn<C>,
    execute_handlers: HashMap<u32, ExecuteFn<C>>,
    csi_handlers: HashMap<u32, Vec<CsiEntry<C>>>,
    esc_handlers: HashMap<u32, Vec<EscEntry<C>>>,
    osc_parser: OscParser<C>,
    dcs_parser: DcsParser<C>,
    error_handler: ErrorFn<C>,

    // fallback handlers (`_printHandlerFb` and `_errorHandlerFb` are the defaults restored by
    // `clear_print_handler` / `clear_error_handler`)
    execute_handler_fb: ExecuteFallbackFn<C>,
    csi_handler_fb: CsiFallbackFn<C>,
    esc_handler_fb: EscFallbackFn<C>,

    /// Next [`HandlerId`]; one counter for every table, so ids never collide.
    next_handler_id: u64,
}

impl<C: ParserHost> fmt::Debug for EscapeSequenceParser<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EscapeSequenceParser")
            .field("current_state", &self.current_state)
            .field("params", &self.params)
            .field("collect", &self.collect)
            .field("osc_parser", &self.osc_parser)
            .field("dcs_parser", &self.dcs_parser)
            .finish_non_exhaustive()
    }
}

impl<C: ParserHost + 'static> Default for EscapeSequenceParser<C> {
    fn default() -> Self {
        Self::new()
    }
}

fn default_print_handler<C>() -> PrintFn<C> {
    Box::new(|_, _| {})
}

fn default_error_handler<C>() -> ErrorFn<C> {
    // upstream `_errorHandlerFb` returns the state unchanged: `abort` stays false
    Box::new(|_, _| false)
}

fn zdm_params(params: &mut Params) {
    params.reset();
    // 0 is a valid param value, so this cannot fail.
    let _ = params.add_param(0); // ZDM
}

impl<C: ParserHost + 'static> EscapeSequenceParser<C> {
    /// A parser in GROUND state with default (no-op) handlers. Swallows 7-bit ST (`ESC \`).
    pub fn new() -> Self {
        let mut params = Params::new(); // defaults to 32 storable params/subparams
        zdm_params(&mut params);
        let mut parser = EscapeSequenceParser {
            initial_state: ParserState::Ground,
            current_state: ParserState::Ground,
            params,
            collect: 0,
            print_handler: default_print_handler(),
            execute_handlers: HashMap::new(),
            csi_handlers: HashMap::new(),
            esc_handlers: HashMap::new(),
            osc_parser: OscParser::new(),
            dcs_parser: DcsParser::new(),
            error_handler: default_error_handler(),
            execute_handler_fb: Box::new(|_, _| {}),
            csi_handler_fb: Box::new(|_, _, _| {}),
            esc_handler_fb: Box::new(|_, _| {}),
            next_handler_id: 0,
        };
        // swallow 7bit ST (ESC+\)
        let ident = u32::from(b'\\');
        let id = parser.next_id();
        parser
            .esc_handlers
            .entry(ident)
            .or_default()
            .push((id, Box::new(|_| true)));
        parser
    }

    fn next_id(&mut self) -> HandlerId {
        let id = HandlerId(self.next_handler_id);
        self.next_handler_id = self.next_handler_id.wrapping_add(1);
        id
    }

    /// Set the print handler (`data` is a run of printable code points).
    pub fn set_print_handler(&mut self, handler: impl FnMut(&mut C, &[u32]) + 'static) {
        self.print_handler = Box::new(handler);
    }

    /// Restore the default (no-op) print handler.
    pub fn clear_print_handler(&mut self) {
        self.print_handler = default_print_handler();
    }

    /// Register an ESC handler; newer handlers run first, `false` falls through to older ones
    /// and finally to the fallback. Final byte range 0x30 ..= 0x7e.
    pub fn register_esc_handler(
        &mut self,
        id: FunctionIdentifier,
        handler: impl FnMut(&mut C) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        let ident = identifier(&id, ESC_FINAL_RANGE)?;
        let handler_id = self.next_id();
        self.esc_handlers
            .entry(ident)
            .or_default()
            .push((handler_id, Box::new(handler)));
        Ok(handler_id)
    }

    /// Remove every ESC handler of `id`.
    pub fn clear_esc_handler(&mut self, id: FunctionIdentifier) -> Result<(), IdentifierError> {
        let ident = identifier(&id, ESC_FINAL_RANGE)?;
        self.esc_handlers.remove(&ident);
        Ok(())
    }

    /// Set the handler for ESC sequences without a handler that returned `true`.
    pub fn set_esc_handler_fallback(&mut self, handler: impl FnMut(&mut C, u32) + 'static) {
        self.esc_handler_fb = Box::new(handler);
    }

    /// Set the handler of the C0/C1 control `flag` (one handler per flag; replaces).
    pub fn set_execute_handler(&mut self, flag: char, handler: impl FnMut(&mut C) + 'static) {
        self.execute_handlers
            .insert(execute_key(flag), Box::new(handler));
    }

    /// Remove the handler of the control `flag`.
    pub fn clear_execute_handler(&mut self, flag: char) {
        self.execute_handlers.remove(&execute_key(flag));
    }

    /// Set the handler for controls without a handler.
    pub fn set_execute_handler_fallback(&mut self, handler: impl FnMut(&mut C, u32) + 'static) {
        self.execute_handler_fb = Box::new(handler);
    }

    /// Register a CSI handler; newer handlers run first, `false` falls through to older ones
    /// and finally to the fallback. `params` is borrowed.
    pub fn register_csi_handler(
        &mut self,
        id: FunctionIdentifier,
        handler: impl FnMut(&mut C, &Params) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        let ident = identifier(&id, CSI_FINAL_RANGE)?;
        let handler_id = self.next_id();
        self.csi_handlers
            .entry(ident)
            .or_default()
            .push((handler_id, Box::new(handler)));
        Ok(handler_id)
    }

    /// Remove every CSI handler of `id`.
    pub fn clear_csi_handler(&mut self, id: FunctionIdentifier) -> Result<(), IdentifierError> {
        let ident = identifier(&id, CSI_FINAL_RANGE)?;
        self.csi_handlers.remove(&ident);
        Ok(())
    }

    /// Set the handler for CSI sequences without a handler that returned `true`.
    pub fn set_csi_handler_fallback(
        &mut self,
        handler: impl FnMut(&mut C, u32, &Params) + 'static,
    ) {
        self.csi_handler_fb = Box::new(handler);
    }

    /// Register a DCS handler; newer handlers run first (see [`DcsParser`]).
    pub fn register_dcs_handler(
        &mut self,
        id: FunctionIdentifier,
        handler: impl DcsHandler<C> + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        let ident = identifier(&id, CSI_FINAL_RANGE)?;
        let handler_id = self.next_id();
        self.dcs_parser
            .register_handler_with_id(ident, handler_id, Box::new(handler));
        Ok(handler_id)
    }

    /// Remove every DCS handler of `id`.
    pub fn clear_dcs_handler(&mut self, id: FunctionIdentifier) -> Result<(), IdentifierError> {
        let ident = identifier(&id, CSI_FINAL_RANGE)?;
        self.dcs_parser.clear_handler(ident);
        Ok(())
    }

    /// Set the handler for DCS sequences without registered handlers.
    pub fn set_dcs_handler_fallback(
        &mut self,
        handler: impl FnMut(&mut C, u32, DcsFallbackAction<'_>) + 'static,
    ) {
        self.dcs_parser.set_handler_fallback(handler);
    }

    /// Register an OSC handler for the numeric OSC `ident`; newer handlers run first (see
    /// [`OscParser`]).
    pub fn register_osc_handler(
        &mut self,
        ident: i32,
        handler: impl OscHandler<C> + 'static,
    ) -> HandlerId {
        let handler_id = self.next_id();
        self.osc_parser
            .register_handler_with_id(ident, handler_id, Box::new(handler));
        handler_id
    }

    /// Remove every OSC handler of `ident`.
    pub fn clear_osc_handler(&mut self, ident: i32) {
        self.osc_parser.clear_handler(ident);
    }

    /// Set the handler for OSC commands without registered handlers.
    pub fn set_osc_handler_fallback(
        &mut self,
        handler: impl FnMut(&mut C, i32, OscFallbackAction<'_>) + 'static,
    ) {
        self.osc_parser.set_handler_fallback(handler);
    }

    /// Set the parse error handler; it returns whether to abort the current `parse` call.
    pub fn set_error_handler(
        &mut self,
        handler: impl FnMut(&mut C, &ParsingState<'_>) -> bool + 'static,
    ) {
        self.error_handler = Box::new(handler);
    }

    /// Restore the default error handler (never aborts).
    pub fn clear_error_handler(&mut self) {
        self.error_handler = default_error_handler();
    }

    /// Remove the handler `id` from whichever table holds it (upstream `IDisposable.dispose`).
    /// Disposing an unknown or already disposed id is a no-op.
    pub fn dispose_handler(&mut self, id: HandlerId) {
        let _ = self
            .csi_handlers
            .values_mut()
            .any(|list| remove_entry(list, id))
            || self
                .esc_handlers
                .values_mut()
                .any(|list| remove_entry(list, id))
            || self.osc_parser.dispose_handler(id)
            || self.dcs_parser.dispose_handler(id);
    }

    /// Reset the parser to its initial state (handlers are kept). Running OSC/DCS handlers get
    /// their `end(false)` / `unhook(false)`; the host's join state is zeroed.
    pub fn reset(&mut self, host: &mut C) {
        self.current_state = self.initial_state;
        self.osc_parser.reset(host);
        self.dcs_parser.reset(host);
        zdm_params(&mut self.params);
        self.collect = 0;
        *host.preceding_join_state() = 0;
    }

    /// Parse UTF-32 code points in `data`, dispatching to the registered handlers.
    pub fn parse(&mut self, host: &mut C, data: &[u32]) {
        // The transition table and parse loop are ported in a later step of this milestone.
        let _ = (host, data);
    }

    // Dispatch of one action of the parse loop. Each matches one `case` of upstream's
    // `parse` switch (minus the async stack save) and touches only the fields it needs, so the
    // loop can call it without holding other borrows of `self`.

    /// `ParserAction.PRINT`.
    #[allow(dead_code, reason = "called by the parse loop, ported in M4 step 5")]
    fn print(&mut self, host: &mut C, data: &[u32]) {
        (self.print_handler)(host, data);
    }

    /// `ParserAction.EXECUTE`.
    #[allow(dead_code, reason = "called by the parse loop, ported in M4 step 5")]
    fn execute(&mut self, host: &mut C, code: u32) {
        match self.execute_handlers.get_mut(&code) {
            Some(handler) => handler(host),
            None => (self.execute_handler_fb)(host, code),
        }
        *host.preceding_join_state() = 0;
    }

    /// `ParserAction.CSI_DISPATCH` for the final byte `code`.
    #[allow(dead_code, reason = "called by the parse loop, ported in M4 step 5")]
    fn csi_dispatch(&mut self, host: &mut C, code: u32) {
        // JS: `this._collect << 8 | code` (bits shifted out of 32 are lost, as in int32)
        let ident = self.collect.wrapping_shl(8) | code;
        let mut handled = false;
        if let Some(list) = self.csi_handlers.get_mut(&ident) {
            for (_, handler) in list.iter_mut().rev() {
                if handler(host, &self.params) {
                    handled = true;
                    break;
                }
            }
        }
        if !handled {
            (self.csi_handler_fb)(host, ident, &self.params);
        }
        *host.preceding_join_state() = 0;
    }

    /// `ParserAction.ESC_DISPATCH` for the final byte `code`.
    #[allow(dead_code, reason = "called by the parse loop, ported in M4 step 5")]
    fn esc_dispatch(&mut self, host: &mut C, code: u32) {
        let ident = self.collect.wrapping_shl(8) | code;
        let mut handled = false;
        if let Some(list) = self.esc_handlers.get_mut(&ident) {
            for (_, handler) in list.iter_mut().rev() {
                if handler(host) {
                    handled = true;
                    break;
                }
            }
        }
        if !handled {
            (self.esc_handler_fb)(host, ident);
        }
        *host.preceding_join_state() = 0;
    }

    /// `ParserAction.ERROR`: returns whether to abort the `parse` call.
    #[allow(dead_code, reason = "called by the parse loop, ported in M4 step 5")]
    fn error(&mut self, host: &mut C, position: usize, code: u32) -> bool {
        let state = ParsingState {
            position,
            code,
            current_state: self.current_state,
            collect: self.collect,
            params: &self.params,
            abort: false,
        };
        (self.error_handler)(host, &state)
    }
}

#[cfg(test)]
#[path = "escape_sequence_parser_tests.rs"]
mod tests;
