//! Port of xterm.js src/common/parser/Constants.ts @ f447274.
//!
//! Discriminants are upstream's: the transition table packs `action << 4 | state` into one byte.

/// Internal states of the escape sequence parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ParserState {
    Ground = 0,
    Escape = 1,
    EscapeIntermediate = 2,
    CsiEntry = 3,
    CsiParam = 4,
    CsiIntermediate = 5,
    CsiIgnore = 6,
    SosPmApcString = 7,
    OscString = 8,
    DcsEntry = 9,
    DcsParam = 10,
    DcsIgnore = 11,
    DcsIntermediate = 12,
    DcsPassthrough = 13,
}

/// Internal actions of the escape sequence parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ParserAction {
    Ignore = 0,
    Error = 1,
    Print = 2,
    Execute = 3,
    OscStart = 4,
    OscPut = 5,
    OscEnd = 6,
    CsiDispatch = 7,
    Param = 8,
    Collect = 9,
    EscDispatch = 10,
    Clear = 11,
    DcsHook = 12,
    DcsPut = 13,
    DcsUnhook = 14,
}

/// Internal states of the OSC parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OscState {
    Start = 0,
    Id = 1,
    Payload = 2,
    Abort = 3,
}

/// Payload limit for OSC and DCS.
pub const PAYLOAD_LIMIT: usize = 10_000_000;
