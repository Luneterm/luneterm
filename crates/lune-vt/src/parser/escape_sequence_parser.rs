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
use std::sync::LazyLock;

use super::constants::{ParserAction, ParserState};
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

/// Shift of the action in a packed table value (`action << 4 | next_state`).
const TRANSITION_ACTION_SHIFT: u8 = 4;
/// Mask of the next state in a packed table value.
const TRANSITION_STATE_MASK: u8 = 15;
/// Shift of the state in a table index (`state << 8 | code`).
const INDEX_STATE_SHIFT: usize = 8;

/// Pseudo-character placeholder for printable non-ASCII characters (unicode).
const NON_ASCII_PRINTABLE: u32 = 0xa0;

/// Transition table of the [`EscapeSequenceParser`] (upstream `TransitionTable`).
///
/// Values are `action << 4 | next_state`, indexed `current_state << 8 | code`.
///
/// Upstream writes through a `Uint8Array`, where an out-of-range index is silently dropped; here
/// [`TransitionTable::add`] does the same (no panic).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionTable {
    /// The packed values.
    pub table: Vec<u8>,
}

impl TransitionTable {
    /// A table of `length` entries, all `0` (IGNORE, GROUND).
    pub fn new(length: usize) -> Self {
        TransitionTable {
            table: vec![0; length],
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    fn pack(action: ParserAction, next: ParserState) -> u8 {
        (action as u8) << TRANSITION_ACTION_SHIFT | next as u8
    }

    fn index(state: ParserState, code: u32) -> usize {
        // `state << 8 | code`; a `code` above 255 overlaps the state bits, as in upstream
        (state as usize) << INDEX_STATE_SHIFT | code as usize
    }

    /// Set the default transition.
    pub fn set_default(&mut self, action: ParserAction, next: ParserState) {
        self.table.fill(Self::pack(action, next));
    }

    /// Add a transition for `code` in `state`.
    pub fn add(&mut self, code: u32, state: ParserState, action: ParserAction, next: ParserState) {
        if let Some(slot) = self.table.get_mut(Self::index(state, code)) {
            *slot = Self::pack(action, next);
        }
    }

    /// Add transitions for multiple input character codes.
    pub fn add_many(
        &mut self,
        codes: &[u32],
        state: ParserState,
        action: ParserAction,
        next: ParserState,
    ) {
        for &code in codes {
            self.add(code, state, action, next);
        }
    }

    /// The packed value for `state` and `code`; `0` (IGNORE, GROUND) outside the table, which is
    /// what a read past a `Uint8Array` yields once shifted and masked.
    fn lookup(&self, state: ParserState, code: u32) -> u8 {
        self.table
            .get(Self::index(state, code))
            .copied()
            .unwrap_or(0)
    }
}

/// `blueprint.slice(start, end)`: the code points `start .. end`.
fn r(start: u32, end: u32) -> Vec<u32> {
    (start..end).collect()
}

fn build_vt500_transition_table() -> TransitionTable {
    use ParserAction as A;
    use ParserState as S;
    let mut table = TransitionTable::new(4095);

    // Default definitions.
    let printables = r(0x20, 0x7f); // 0x20 (SP) included, 0x7F (DEL) excluded
    let mut executables = r(0x00, 0x18);
    executables.push(0x19);
    executables.extend(r(0x1c, 0x20));

    let states = [
        S::Ground,
        S::Escape,
        S::EscapeIntermediate,
        S::CsiEntry,
        S::CsiParam,
        S::CsiIntermediate,
        S::CsiIgnore,
        S::SosPmApcString,
        S::OscString,
        S::DcsEntry,
        S::DcsParam,
        S::DcsIgnore,
        S::DcsIntermediate,
        S::DcsPassthrough,
    ];

    // set default transition
    table.set_default(A::Error, S::Ground);
    // printables
    table.add_many(&printables, S::Ground, A::Print, S::Ground);
    // global anywhere rules
    for state in states {
        table.add_many(&[0x18, 0x1a, 0x99, 0x9a], state, A::Execute, S::Ground);
        table.add_many(&r(0x80, 0x90), state, A::Execute, S::Ground);
        table.add_many(&r(0x90, 0x98), state, A::Execute, S::Ground);
        table.add(0x9c, state, A::Ignore, S::Ground); // ST as terminator
        table.add(0x1b, state, A::Clear, S::Escape); // ESC
        table.add(0x9d, state, A::OscStart, S::OscString); // OSC
        table.add_many(&[0x98, 0x9e, 0x9f], state, A::Ignore, S::SosPmApcString);
        table.add(0x9b, state, A::Clear, S::CsiEntry); // CSI
        table.add(0x90, state, A::Clear, S::DcsEntry); // DCS
    }
    // rules for executables and 7f
    table.add_many(&executables, S::Ground, A::Execute, S::Ground);
    table.add_many(&executables, S::Escape, A::Execute, S::Escape);
    table.add(0x7f, S::Escape, A::Ignore, S::Escape);
    table.add_many(&executables, S::OscString, A::Ignore, S::OscString);
    table.add_many(&executables, S::CsiEntry, A::Execute, S::CsiEntry);
    table.add(0x7f, S::CsiEntry, A::Ignore, S::CsiEntry);
    table.add_many(&executables, S::CsiParam, A::Execute, S::CsiParam);
    table.add(0x7f, S::CsiParam, A::Ignore, S::CsiParam);
    table.add_many(&executables, S::CsiIgnore, A::Execute, S::CsiIgnore);
    table.add_many(
        &executables,
        S::CsiIntermediate,
        A::Execute,
        S::CsiIntermediate,
    );
    table.add(0x7f, S::CsiIntermediate, A::Ignore, S::CsiIntermediate);
    table.add_many(
        &executables,
        S::EscapeIntermediate,
        A::Execute,
        S::EscapeIntermediate,
    );
    table.add(
        0x7f,
        S::EscapeIntermediate,
        A::Ignore,
        S::EscapeIntermediate,
    );
    // osc
    table.add(0x5d, S::Escape, A::OscStart, S::OscString);
    table.add_many(&printables, S::OscString, A::OscPut, S::OscString);
    table.add(0x7f, S::OscString, A::OscPut, S::OscString);
    table.add_many(
        &[0x9c, 0x1b, 0x18, 0x1a, 0x07],
        S::OscString,
        A::OscEnd,
        S::Ground,
    );
    table.add_many(&r(0x1c, 0x20), S::OscString, A::Ignore, S::OscString);
    // sos/pm/apc does nothing
    table.add_many(&[0x58, 0x5e, 0x5f], S::Escape, A::Ignore, S::SosPmApcString);
    table.add_many(&printables, S::SosPmApcString, A::Ignore, S::SosPmApcString);
    table.add_many(
        &executables,
        S::SosPmApcString,
        A::Ignore,
        S::SosPmApcString,
    );
    table.add(0x9c, S::SosPmApcString, A::Ignore, S::Ground);
    table.add(0x7f, S::SosPmApcString, A::Ignore, S::SosPmApcString);
    // csi entries
    table.add(0x5b, S::Escape, A::Clear, S::CsiEntry);
    table.add_many(&r(0x40, 0x7f), S::CsiEntry, A::CsiDispatch, S::Ground);
    table.add_many(&r(0x30, 0x3c), S::CsiEntry, A::Param, S::CsiParam);
    table.add_many(
        &[0x3c, 0x3d, 0x3e, 0x3f],
        S::CsiEntry,
        A::Collect,
        S::CsiParam,
    );
    table.add_many(&r(0x30, 0x3c), S::CsiParam, A::Param, S::CsiParam);
    table.add_many(&r(0x40, 0x7f), S::CsiParam, A::CsiDispatch, S::Ground);
    table.add_many(
        &[0x3c, 0x3d, 0x3e, 0x3f],
        S::CsiParam,
        A::Ignore,
        S::CsiIgnore,
    );
    table.add_many(&r(0x20, 0x40), S::CsiIgnore, A::Ignore, S::CsiIgnore);
    table.add(0x7f, S::CsiIgnore, A::Ignore, S::CsiIgnore);
    table.add_many(&r(0x40, 0x7f), S::CsiIgnore, A::Ignore, S::Ground);
    table.add_many(&r(0x20, 0x30), S::CsiEntry, A::Collect, S::CsiIntermediate);
    table.add_many(
        &r(0x20, 0x30),
        S::CsiIntermediate,
        A::Collect,
        S::CsiIntermediate,
    );
    table.add_many(&r(0x30, 0x40), S::CsiIntermediate, A::Ignore, S::CsiIgnore);
    table.add_many(
        &r(0x40, 0x7f),
        S::CsiIntermediate,
        A::CsiDispatch,
        S::Ground,
    );
    table.add_many(&r(0x20, 0x30), S::CsiParam, A::Collect, S::CsiIntermediate);
    // esc_intermediate
    table.add_many(&r(0x20, 0x30), S::Escape, A::Collect, S::EscapeIntermediate);
    table.add_many(
        &r(0x20, 0x30),
        S::EscapeIntermediate,
        A::Collect,
        S::EscapeIntermediate,
    );
    table.add_many(
        &r(0x30, 0x7f),
        S::EscapeIntermediate,
        A::EscDispatch,
        S::Ground,
    );
    table.add_many(&r(0x30, 0x50), S::Escape, A::EscDispatch, S::Ground);
    table.add_many(&r(0x51, 0x58), S::Escape, A::EscDispatch, S::Ground);
    table.add_many(&[0x59, 0x5a, 0x5c], S::Escape, A::EscDispatch, S::Ground);
    table.add_many(&r(0x60, 0x7f), S::Escape, A::EscDispatch, S::Ground);
    // dcs entry
    table.add(0x50, S::Escape, A::Clear, S::DcsEntry);
    table.add_many(&executables, S::DcsEntry, A::Ignore, S::DcsEntry);
    table.add(0x7f, S::DcsEntry, A::Ignore, S::DcsEntry);
    table.add_many(&r(0x1c, 0x20), S::DcsEntry, A::Ignore, S::DcsEntry);
    table.add_many(&r(0x20, 0x30), S::DcsEntry, A::Collect, S::DcsIntermediate);
    table.add_many(&r(0x30, 0x3c), S::DcsEntry, A::Param, S::DcsParam);
    table.add_many(
        &[0x3c, 0x3d, 0x3e, 0x3f],
        S::DcsEntry,
        A::Collect,
        S::DcsParam,
    );
    table.add_many(&executables, S::DcsIgnore, A::Ignore, S::DcsIgnore);
    table.add_many(&r(0x20, 0x80), S::DcsIgnore, A::Ignore, S::DcsIgnore);
    table.add_many(&r(0x1c, 0x20), S::DcsIgnore, A::Ignore, S::DcsIgnore);
    table.add_many(&executables, S::DcsParam, A::Ignore, S::DcsParam);
    table.add(0x7f, S::DcsParam, A::Ignore, S::DcsParam);
    table.add_many(&r(0x1c, 0x20), S::DcsParam, A::Ignore, S::DcsParam);
    table.add_many(&r(0x30, 0x3c), S::DcsParam, A::Param, S::DcsParam);
    table.add_many(
        &[0x3c, 0x3d, 0x3e, 0x3f],
        S::DcsParam,
        A::Ignore,
        S::DcsIgnore,
    );
    table.add_many(&r(0x20, 0x30), S::DcsParam, A::Collect, S::DcsIntermediate);
    table.add_many(
        &executables,
        S::DcsIntermediate,
        A::Ignore,
        S::DcsIntermediate,
    );
    table.add(0x7f, S::DcsIntermediate, A::Ignore, S::DcsIntermediate);
    table.add_many(
        &r(0x1c, 0x20),
        S::DcsIntermediate,
        A::Ignore,
        S::DcsIntermediate,
    );
    table.add_many(
        &r(0x20, 0x30),
        S::DcsIntermediate,
        A::Collect,
        S::DcsIntermediate,
    );
    table.add_many(&r(0x30, 0x40), S::DcsIntermediate, A::Ignore, S::DcsIgnore);
    table.add_many(
        &r(0x40, 0x7f),
        S::DcsIntermediate,
        A::DcsHook,
        S::DcsPassthrough,
    );
    table.add_many(&r(0x40, 0x7f), S::DcsParam, A::DcsHook, S::DcsPassthrough);
    table.add_many(&r(0x40, 0x7f), S::DcsEntry, A::DcsHook, S::DcsPassthrough);
    table.add_many(
        &executables,
        S::DcsPassthrough,
        A::DcsPut,
        S::DcsPassthrough,
    );
    table.add_many(&printables, S::DcsPassthrough, A::DcsPut, S::DcsPassthrough);
    table.add(0x7f, S::DcsPassthrough, A::Ignore, S::DcsPassthrough);
    table.add_many(
        &[0x1b, 0x9c, 0x18, 0x1a],
        S::DcsPassthrough,
        A::DcsUnhook,
        S::Ground,
    );
    // special handling of unicode chars
    table.add(NON_ASCII_PRINTABLE, S::Ground, A::Print, S::Ground);
    table.add(NON_ASCII_PRINTABLE, S::OscString, A::OscPut, S::OscString);
    table.add(NON_ASCII_PRINTABLE, S::CsiIgnore, A::Ignore, S::CsiIgnore);
    table.add(NON_ASCII_PRINTABLE, S::DcsIgnore, A::Ignore, S::DcsIgnore);
    table.add(
        NON_ASCII_PRINTABLE,
        S::DcsPassthrough,
        A::DcsPut,
        S::DcsPassthrough,
    );
    table
}

/// VT500 compatible transition table, taken from <https://vt100.net/emu/dec_ansi_parser>.
pub static VT500_TRANSITION_TABLE: LazyLock<TransitionTable> =
    LazyLock::new(build_vt500_transition_table);

/// The action of a packed table value; `None` for a value outside [`ParserAction`] (only a
/// custom table can hold one), which upstream's `switch` treats as no action.
fn action_of(transition: u8) -> Option<ParserAction> {
    Some(match transition >> TRANSITION_ACTION_SHIFT {
        0 => ParserAction::Ignore,
        1 => ParserAction::Error,
        2 => ParserAction::Print,
        3 => ParserAction::Execute,
        4 => ParserAction::OscStart,
        5 => ParserAction::OscPut,
        6 => ParserAction::OscEnd,
        7 => ParserAction::CsiDispatch,
        8 => ParserAction::Param,
        9 => ParserAction::Collect,
        10 => ParserAction::EscDispatch,
        11 => ParserAction::Clear,
        12 => ParserAction::DcsHook,
        13 => ParserAction::DcsPut,
        14 => ParserAction::DcsUnhook,
        _ => return None,
    })
}

/// The next state of a packed table value. Values 14 and 15 are not states; only a custom
/// table can hold them, and they map to GROUND (upstream would index the table's default
/// entries, which are `ERROR -> GROUND`).
fn state_of(transition: u8) -> ParserState {
    match transition & TRANSITION_STATE_MASK {
        1 => ParserState::Escape,
        2 => ParserState::EscapeIntermediate,
        3 => ParserState::CsiEntry,
        4 => ParserState::CsiParam,
        5 => ParserState::CsiIntermediate,
        6 => ParserState::CsiIgnore,
        7 => ParserState::SosPmApcString,
        8 => ParserState::OscString,
        9 => ParserState::DcsEntry,
        10 => ParserState::DcsParam,
        11 => ParserState::DcsIgnore,
        12 => ParserState::DcsIntermediate,
        13 => ParserState::DcsPassthrough,
        _ => ParserState::Ground,
    }
}

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
    transitions: TransitionTable,

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
    /// A parser in GROUND state with default (no-op) handlers and the VT500 transition table.
    /// Swallows 7-bit ST (`ESC \`).
    pub fn new() -> Self {
        Self::with_transitions(VT500_TRANSITION_TABLE.clone())
    }

    /// Like [`EscapeSequenceParser::new`] with a custom transition table (upstream's optional
    /// `transitions` constructor argument). A value outside the table reads as `0` (no action,
    /// GROUND).
    pub fn with_transitions(transitions: TransitionTable) -> Self {
        let mut params = Params::new(); // defaults to 32 storable params/subparams
        zdm_params(&mut params);
        let mut parser = EscapeSequenceParser {
            initial_state: ParserState::Ground,
            current_state: ParserState::Ground,
            transitions,
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
    pub fn set_print_handler(&mut self, handler: impl FnMut(&mut C, &[u32]) + Send + 'static) {
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
        handler: impl FnMut(&mut C) -> bool + Send + 'static,
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
    pub fn set_esc_handler_fallback(&mut self, handler: impl FnMut(&mut C, u32) + Send + 'static) {
        self.esc_handler_fb = Box::new(handler);
    }

    /// Set the handler of the C0/C1 control `flag` (one handler per flag; replaces).
    pub fn set_execute_handler(
        &mut self,
        flag: char,
        handler: impl FnMut(&mut C) + Send + 'static,
    ) {
        self.execute_handlers
            .insert(execute_key(flag), Box::new(handler));
    }

    /// Remove the handler of the control `flag`.
    pub fn clear_execute_handler(&mut self, flag: char) {
        self.execute_handlers.remove(&execute_key(flag));
    }

    /// Set the handler for controls without a handler.
    pub fn set_execute_handler_fallback(
        &mut self,
        handler: impl FnMut(&mut C, u32) + Send + 'static,
    ) {
        self.execute_handler_fb = Box::new(handler);
    }

    /// Register a CSI handler; newer handlers run first, `false` falls through to older ones
    /// and finally to the fallback. `params` is borrowed.
    pub fn register_csi_handler(
        &mut self,
        id: FunctionIdentifier,
        handler: impl FnMut(&mut C, &Params) -> bool + Send + 'static,
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
        handler: impl FnMut(&mut C, u32, &Params) + Send + 'static,
    ) {
        self.csi_handler_fb = Box::new(handler);
    }

    /// Register a DCS handler; newer handlers run first (see [`DcsParser`]).
    pub fn register_dcs_handler(
        &mut self,
        id: FunctionIdentifier,
        handler: impl DcsHandler<C> + Send + 'static,
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
        handler: impl FnMut(&mut C, u32, DcsFallbackAction<'_>) + Send + 'static,
    ) {
        self.dcs_parser.set_handler_fallback(handler);
    }

    /// Register an OSC handler for the numeric OSC `ident`; newer handlers run first (see
    /// [`OscParser`]).
    pub fn register_osc_handler(
        &mut self,
        ident: i32,
        handler: impl OscHandler<C> + Send + 'static,
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
        handler: impl FnMut(&mut C, i32, OscFallbackAction<'_>) + Send + 'static,
    ) {
        self.osc_parser.set_handler_fallback(handler);
    }

    /// Set the parse error handler; it returns whether to abort the current `parse` call.
    pub fn set_error_handler(
        &mut self,
        handler: impl FnMut(&mut C, &ParsingState<'_>) -> bool + Send + 'static,
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
    ///
    /// Handlers are synchronous (plan decision D3), so one call parses all of `data` unless the
    /// error handler asks to abort. Any `u32` is accepted: values from 0xA0 up (including those
    /// above U+10FFFF) look up the NON_ASCII_PRINTABLE column of the table.
    pub fn parse(&mut self, host: &mut C, data: &[u32]) {
        let mut i = 0;
        while let Some(&first) = data.get(i) {
            let mut code = first;
            // normal transition & action lookup
            let lookup = if code < NON_ASCII_PRINTABLE {
                code
            } else {
                NON_ASCII_PRINTABLE
            };
            let mut transition = self.transitions.lookup(self.current_state, lookup);
            // LUNETERM: DEL (0x7F) inside a DCS payload is kept wherever it falls. Upstream's table
            // maps it to IGNORE in DCS_PASSTHROUGH, but its DCS_PUT read-ahead loop does not stop
            // at it (the comment there says it should), so the same DEL was dropped when it was
            // the first code after the hook or the first of a `parse` call and kept anywhere
            // later in a run; the payload then depended on how the host split its writes (see
            // ledger row 18 and `dcs_del_is_chunk_invariant`). Routing the table's IGNORE
            // for this one entry to DCS_PUT makes the lookup agree with the read-ahead loop. An
            // entry that is not IGNORE (a custom table) is left alone. Every other read-ahead
            // loop (print, OSC put, params) already matches its table entries code for code.
            if code == 0x7f
                && self.current_state == ParserState::DcsPassthrough
                && action_of(transition) == Some(ParserAction::Ignore)
            {
                transition =
                    TransitionTable::pack(ParserAction::DcsPut, ParserState::DcsPassthrough);
            }
            match action_of(transition) {
                Some(ParserAction::Print) => {
                    // read ahead: 0x20 (SP) is included, 0x7F (DEL) and 0x80 - 0x9F are excluded
                    // (upstream unrolls this loop four times; same exit condition)
                    let mut j = i + 1;
                    while data
                        .get(j)
                        .is_some_and(|&c| c >= 0x20 && !(c > 0x7e && c < NON_ASCII_PRINTABLE))
                    {
                        j += 1;
                    }
                    if let Some(chunk) = data.get(i..j) {
                        self.print(host, chunk);
                    }
                    i = j - 1;
                }
                Some(ParserAction::Execute) => self.execute(host, code),
                Some(ParserAction::Ignore) | None => {}
                Some(ParserAction::Error) => {
                    if self.error(host, i, code) {
                        return;
                    }
                    // inject values: currently not implemented
                }
                Some(ParserAction::CsiDispatch) => self.csi_dispatch(host, code),
                Some(ParserAction::Param) => {
                    // inner loop: digits (0x30 - 0x39) and ; (0x3b) and : (0x3a)
                    let mut j = i;
                    loop {
                        match code {
                            0x3b => {
                                let _ = self.params.add_param(0); // ZDM
                            }
                            0x3a => {
                                let _ = self.params.add_sub_param(-1);
                            }
                            // 0x30 - 0x39 (saturating: a custom table may route any code here)
                            _ => self.params.add_digit(
                                i32::try_from(code).unwrap_or(i32::MAX).saturating_sub(48),
                            ),
                        }
                        j += 1;
                        match data.get(j) {
                            Some(&next) if next > 0x2f && next < 0x3c => code = next,
                            _ => break,
                        }
                    }
                    i = j - 1;
                }
                Some(ParserAction::Collect) => {
                    self.collect = self.collect.wrapping_shl(8) | code;
                }
                Some(ParserAction::EscDispatch) => self.esc_dispatch(host, code),
                Some(ParserAction::Clear) => {
                    zdm_params(&mut self.params);
                    self.collect = 0;
                }
                Some(ParserAction::DcsHook) => {
                    let ident = self.collect.wrapping_shl(8) | code;
                    self.dcs_parser.hook(host, ident, &self.params);
                }
                Some(ParserAction::DcsPut) => {
                    // inner loop - exit DCS_PUT: 0x18, 0x1a, 0x1b, 0x7f, 0x80 - 0x9f
                    // unhook triggered by: 0x1b, 0x9c (success) and 0x18, 0x1a (abort)
                    let mut j = i + 1;
                    while data.get(j).is_some_and(|&c| {
                        !(c == 0x18
                            || c == 0x1a
                            || c == 0x1b
                            || (c > 0x7f && c < NON_ASCII_PRINTABLE))
                    }) {
                        j += 1;
                    }
                    if let Some(chunk) = data.get(i..j) {
                        self.dcs_parser.put(host, chunk);
                    }
                    i = j - 1;
                }
                Some(ParserAction::DcsUnhook) => {
                    self.dcs_parser.unhook(host, code != 0x18 && code != 0x1a);
                    if code == 0x1b {
                        transition |= ParserState::Escape as u8;
                    }
                    zdm_params(&mut self.params);
                    self.collect = 0;
                    *host.preceding_join_state() = 0;
                }
                Some(ParserAction::OscStart) => self.osc_parser.start(host),
                Some(ParserAction::OscPut) => {
                    // inner loop: 0x20 (SP) included, 0x7F (DEL) included
                    let mut j = i + 1;
                    while data
                        .get(j)
                        .is_some_and(|&c| c >= 0x20 && !(c > 0x7f && c < NON_ASCII_PRINTABLE))
                    {
                        j += 1;
                    }
                    if let Some(chunk) = data.get(i..j) {
                        self.osc_parser.put(host, chunk);
                    }
                    i = j - 1;
                }
                Some(ParserAction::OscEnd) => {
                    self.osc_parser.end(host, code != 0x18 && code != 0x1a);
                    if code == 0x1b {
                        transition |= ParserState::Escape as u8;
                    }
                    zdm_params(&mut self.params);
                    self.collect = 0;
                    *host.preceding_join_state() = 0;
                }
            }
            self.current_state = state_of(transition);
            i += 1;
        }
    }

    // Dispatch of one action of the parse loop. Each matches one `case` of upstream's
    // `parse` switch (minus the async stack save) and touches only the fields it needs, so the
    // loop can call it without holding other borrows of `self`.

    /// `ParserAction.PRINT`.
    fn print(&mut self, host: &mut C, data: &[u32]) {
        (self.print_handler)(host, data);
    }

    /// `ParserAction.EXECUTE`.
    fn execute(&mut self, host: &mut C, code: u32) {
        match self.execute_handlers.get_mut(&code) {
            Some(handler) => handler(host),
            None => (self.execute_handler_fb)(host, code),
        }
        *host.preceding_join_state() = 0;
    }

    /// `ParserAction.CSI_DISPATCH` for the final byte `code`.
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
