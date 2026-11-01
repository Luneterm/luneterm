//! Port of xterm.js src/common/parser/Types.ts @ f447274.
//!
//! Rust shape of the handler API (plan M4, decision D1):
//!  - The parser is generic over the host state `C` and borrows it only for the duration of one
//!    call (`parse`, `reset`, a sub-parser `put`/`end`). Handlers receive `&mut C`; they never
//!    hold a reference back to the host or the parser.
//!  - Upstream's `precedingJoinState` lives on the host behind [`ParserHost`], because `print`
//!    and REP read and write it while the parser is dispatching.
//!  - `IDisposable` becomes a [`HandlerId`] passed to `dispose_handler`.
//!  - No async handlers (plan decision D3): handlers return `bool`, never a promise, so
//!    `ParserStackType`, `IParserStackState`, `ISubParserStackState` and
//!    `ResumableHandlersType` are not ported.
//!  - `IParams` / `IParamsConstructor` are the concrete [`Params`]; `ISubParser`, `IOscParser`,
//!    `IDcsParser` and `IEscapeSequenceParser` are the concrete parser types.

use std::fmt;

use super::constants::ParserState;
use super::params::{Params, ParamsArrayItem};

/// Sequence params serialized to arrays (upstream `ParamsArray`).
pub type ParamsArray = Vec<ParamsArrayItem>;

/// Host state the escape sequence parser dispatches into.
pub trait ParserHost {
    /// xterm.js `EscapeSequenceParser.precedingJoinState` (`UnicodeJoinProperties`).
    ///
    /// Preceding grapheme-join-state, used for joining grapheme clusters across calls to
    /// `print` and by REP to check whether repeating a character is allowed. The parser resets
    /// it to 0 for any valid sequence besides text, and in `reset`.
    fn preceding_join_state(&mut self) -> &mut u32;
}

/// Internal state of the escape sequence parser, passed to the error handler for introspection
/// on parse errors (upstream `IParsingState`).
///
/// LUNETERM: upstream's handler returns the (possibly altered) state and the parser only reads
/// `abort` back; here the handler returns `abort` directly and the state is read-only.
#[derive(Debug)]
pub struct ParsingState<'a> {
    /// Position in the parsed chunk.
    pub position: usize,
    /// Current code point.
    pub code: u32,
    /// Current parser state.
    pub current_state: ParserState,
    /// Collect buffer with intermediate characters.
    pub collect: u32,
    /// Params buffer.
    pub params: &'a Params,
    /// Should abort (always `false` when handed to the handler).
    pub abort: bool,
}

/// Identifies one registered handler; replaces upstream's `IDisposable` return value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandlerId(pub(crate) u64);

/// DCS handler (upstream `IDcsHandler`).
pub trait DcsHandler<C> {
    /// Called when a DCS command starts. Prepare needed data structures here.
    /// Note: `params` is borrowed.
    fn hook(&mut self, host: &mut C, params: &Params);
    /// Incoming payload chunk (upstream `put(data, start, end)` as one slice).
    fn put(&mut self, host: &mut C, data: &[u32]);
    /// End of DCS command. `success` indicates whether the command finished normally or got
    /// aborted, thus final execution of the command should depend on `success`. To save memory
    /// also clean up data structures here. Return `true` to stop older handlers from running.
    fn unhook(&mut self, host: &mut C, success: bool) -> bool;
}

/// OSC handler (upstream `IOscHandler`).
pub trait OscHandler<C> {
    /// Announces start of this OSC command. Prepare needed data structures here.
    fn start(&mut self, host: &mut C);
    /// Incoming data chunk. Note: data is borrowed.
    fn put(&mut self, host: &mut C, data: &[u32]);
    /// End of OSC command. `success` indicates whether the command finished normally or got
    /// aborted, thus final execution of the command should depend on `success`. To save memory
    /// also clean up data structures here. Return `true` to stop older handlers from running.
    fn end(&mut self, host: &mut C, success: bool) -> bool;
}

/// Action passed to the OSC fallback handler (upstream `'START' | 'PUT' | 'END'` + payload).
///
/// LUNETERM: upstream converts the `PUT` payload to a string; the code points are passed as
/// is here (no allocation per chunk). `input::text_decoder::utf32_to_string` converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscFallbackAction<'a> {
    /// `'START'`.
    Start,
    /// `'PUT'` with the payload chunk.
    Put(&'a [u32]),
    /// `'END'` with `success`.
    End(bool),
}

/// Action passed to the DCS fallback handler (upstream `'HOOK' | 'PUT' | 'UNHOOK'` + payload).
///
/// LUNETERM: the `PUT` payload is passed as code points, as for [`OscFallbackAction`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DcsFallbackAction<'a> {
    /// `'HOOK'` with the borrowed params.
    Hook(&'a Params),
    /// `'PUT'` with the payload chunk.
    Put(&'a [u32]),
    /// `'UNHOOK'` with `success`.
    Unhook(bool),
}

/// Denotes a specific ESC, CSI or DCS handler slot (upstream `IFunctionIdentifier`).
///
/// The values are turned into an integer representation during handler registration, so the
/// parser can look handlers up quickly. `prefix` and `final_byte` are single `char`s here, so
/// upstream's "only one byte as prefix" and "final must be a single byte" errors cannot occur.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionIdentifier {
    /// Optional prefix byte, 0x3C ..= 0x3F (`<`, `=`, `>`, `?`).
    pub prefix: Option<char>,
    /// Up to two intermediate bytes, each 0x20 ..= 0x2F.
    pub intermediates: String,
    /// The final byte.
    pub final_byte: char,
}

impl FunctionIdentifier {
    /// `{ final }`.
    pub fn new(final_byte: char) -> Self {
        FunctionIdentifier {
            prefix: None,
            intermediates: String::new(),
            final_byte,
        }
    }

    /// `{ prefix, final }`.
    pub fn with_prefix(prefix: char, final_byte: char) -> Self {
        FunctionIdentifier {
            prefix: Some(prefix),
            intermediates: String::new(),
            final_byte,
        }
    }

    /// `{ intermediates, final }`.
    pub fn with_intermediates(intermediates: &str, final_byte: char) -> Self {
        FunctionIdentifier {
            prefix: None,
            intermediates: intermediates.to_owned(),
            final_byte,
        }
    }

    /// `{ prefix, intermediates, final }`.
    pub fn with_prefix_and_intermediates(
        prefix: char,
        intermediates: &str,
        final_byte: char,
    ) -> Self {
        FunctionIdentifier {
            prefix: Some(prefix),
            intermediates: intermediates.to_owned(),
            final_byte,
        }
    }
}

/// What upstream's `_identifier` throws (plan decision D5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentifierError {
    /// `prefix must be in range 0x3c .. 0x3f`.
    PrefixRange,
    /// `only two bytes as intermediates are supported`.
    TooManyIntermediates,
    /// `intermediate must be in range 0x20 .. 0x2f`.
    IntermediateRange,
    /// `final must be in range <lo> .. <hi>`.
    FinalRange {
        /// Lowest allowed final byte.
        lo: u32,
        /// Highest allowed final byte.
        hi: u32,
    },
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentifierError::PrefixRange => f.write_str("prefix must be in range 0x3c .. 0x3f"),
            IdentifierError::TooManyIntermediates => {
                f.write_str("only two bytes as intermediates are supported")
            }
            IdentifierError::IntermediateRange => {
                f.write_str("intermediate must be in range 0x20 .. 0x2f")
            }
            IdentifierError::FinalRange { lo, hi } => {
                write!(f, "final must be in range {lo} .. {hi}")
            }
        }
    }
}

impl std::error::Error for IdentifierError {}

// Boxed handler shapes. All are `'static`: a handler owns what it captures; host state lives in
// `C` and is passed in per call.

/// `PrintHandlerType` (`data, start, end` as one slice).
pub(crate) type PrintFn<C> = Box<dyn FnMut(&mut C, &[u32])>;
/// `ExecuteHandlerType` (upstream's `boolean` return is never read by the parser).
pub(crate) type ExecuteFn<C> = Box<dyn FnMut(&mut C)>;
/// `ExecuteFallbackHandlerType`.
pub(crate) type ExecuteFallbackFn<C> = Box<dyn FnMut(&mut C, u32)>;
/// `CsiHandlerType`.
pub(crate) type CsiFn<C> = Box<dyn FnMut(&mut C, &Params) -> bool>;
/// `CsiFallbackHandlerType`.
pub(crate) type CsiFallbackFn<C> = Box<dyn FnMut(&mut C, u32, &Params)>;
/// `EscHandlerType`.
pub(crate) type EscFn<C> = Box<dyn FnMut(&mut C) -> bool>;
/// `EscFallbackHandlerType`.
pub(crate) type EscFallbackFn<C> = Box<dyn FnMut(&mut C, u32)>;
/// `OscFallbackHandlerType`.
pub(crate) type OscFallbackFn<C> = Box<dyn FnMut(&mut C, i32, OscFallbackAction<'_>)>;
/// `DcsFallbackHandlerType`.
pub(crate) type DcsFallbackFn<C> = Box<dyn FnMut(&mut C, u32, DcsFallbackAction<'_>)>;
/// Callback of `OscStringHandler` (upstream `OscHandler` constructor argument).
pub(crate) type OscStringFn<C> = Box<dyn FnMut(&mut C, &str) -> bool>;
/// Callback of `DcsStringHandler` (upstream `DcsHandler` constructor argument).
pub(crate) type DcsStringFn<C> = Box<dyn FnMut(&mut C, &str, &Params) -> bool>;
/// Error handler: returns `abort`.
pub(crate) type ErrorFn<C> = Box<dyn FnMut(&mut C, &ParsingState<'_>) -> bool>;

/// Remove the entry `id` from `list` (upstream's disposable: `splice(indexOf(handler), 1)`).
/// Returns whether it was there.
pub(crate) fn remove_entry<H>(list: &mut Vec<(HandlerId, H)>, id: HandlerId) -> bool {
    match list.iter().position(|(entry, _)| *entry == id) {
        Some(index) => {
            list.remove(index);
            true
        }
        None => false,
    }
}
