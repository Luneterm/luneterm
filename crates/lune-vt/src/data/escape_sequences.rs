//! Port of xterm.js src/common/data/EscapeSequences.ts @ f447274.
//!
//! C0 and C1 control codes as `char`s (upstream uses one-character strings).
//! See <https://en.wikipedia.org/wiki/C0_and_C1_control_codes>.

/// C0 control codes
/// See <https://en.wikipedia.org/wiki/C0_and_C1_control_codes>
#[allow(non_snake_case)]
pub mod C0 {
    /// Null (Caret = ^@, C = \0)
    pub const NUL: char = '\x00';
    /// Start of Heading (Caret = ^A)
    pub const SOH: char = '\x01';
    /// Start of Text (Caret = ^B)
    pub const STX: char = '\x02';
    /// End of Text (Caret = ^C)
    pub const ETX: char = '\x03';
    /// End of Transmission (Caret = ^D)
    pub const EOT: char = '\x04';
    /// Enquiry (Caret = ^E)
    pub const ENQ: char = '\x05';
    /// Acknowledge (Caret = ^F)
    pub const ACK: char = '\x06';
    /// Bell (Caret = ^G, C = \a)
    pub const BEL: char = '\x07';
    /// Backspace (Caret = ^H, C = \b)
    pub const BS: char = '\x08';
    /// Character Tabulation, Horizontal Tabulation (Caret = ^I, C = \t)
    pub const HT: char = '\x09';
    /// Line Feed (Caret = ^J, C = \n)
    pub const LF: char = '\x0a';
    /// Line Tabulation, Vertical Tabulation (Caret = ^K, C = \v)
    pub const VT: char = '\x0b';
    /// Form Feed (Caret = ^L, C = \f)
    pub const FF: char = '\x0c';
    /// Carriage Return (Caret = ^M, C = \r)
    pub const CR: char = '\x0d';
    /// Shift Out (Caret = ^N)
    pub const SO: char = '\x0e';
    /// Shift In (Caret = ^O)
    pub const SI: char = '\x0f';
    /// Data Link Escape (Caret = ^P)
    pub const DLE: char = '\x10';
    /// Device Control One (XON) (Caret = ^Q)
    pub const DC1: char = '\x11';
    /// Device Control Two (Caret = ^R)
    pub const DC2: char = '\x12';
    /// Device Control Three (XOFF) (Caret = ^S)
    pub const DC3: char = '\x13';
    /// Device Control Four (Caret = ^T)
    pub const DC4: char = '\x14';
    /// Negative Acknowledge (Caret = ^U)
    pub const NAK: char = '\x15';
    /// Synchronous Idle (Caret = ^V)
    pub const SYN: char = '\x16';
    /// End of Transmission Block (Caret = ^W)
    pub const ETB: char = '\x17';
    /// Cancel (Caret = ^X)
    pub const CAN: char = '\x18';
    /// End of Medium (Caret = ^Y)
    pub const EM: char = '\x19';
    /// Substitute (Caret = ^Z)
    pub const SUB: char = '\x1a';
    /// Escape (Caret = ^[, C = \e)
    pub const ESC: char = '\x1b';
    /// File Separator (Caret = ^\)
    pub const FS: char = '\x1c';
    /// Group Separator (Caret = ^])
    pub const GS: char = '\x1d';
    /// Record Separator (Caret = ^^)
    pub const RS: char = '\x1e';
    /// Unit Separator (Caret = ^_)
    pub const US: char = '\x1f';
    /// Space
    pub const SP: char = '\x20';
    /// Delete (Caret = ^?)
    pub const DEL: char = '\x7f';
}

/// C1 control codes
/// See <https://en.wikipedia.org/wiki/C0_and_C1_control_codes>
#[allow(non_snake_case)]
pub mod C1 {
    /// padding character
    pub const PAD: char = '\u{80}';
    /// High Octet Preset
    pub const HOP: char = '\u{81}';
    /// Break Permitted Here
    pub const BPH: char = '\u{82}';
    /// No Break Here
    pub const NBH: char = '\u{83}';
    /// Index
    pub const IND: char = '\u{84}';
    /// Next Line
    pub const NEL: char = '\u{85}';
    /// Start of Selected Area
    pub const SSA: char = '\u{86}';
    /// End of Selected Area
    pub const ESA: char = '\u{87}';
    /// Horizontal Tabulation Set
    pub const HTS: char = '\u{88}';
    /// Horizontal Tabulation With Justification
    pub const HTJ: char = '\u{89}';
    /// Vertical Tabulation Set
    pub const VTS: char = '\u{8a}';
    /// Partial Line Down
    pub const PLD: char = '\u{8b}';
    /// Partial Line Up
    pub const PLU: char = '\u{8c}';
    /// Reverse Index
    pub const RI: char = '\u{8d}';
    /// Single-Shift 2
    pub const SS2: char = '\u{8e}';
    /// Single-Shift 3
    pub const SS3: char = '\u{8f}';
    /// Device Control String
    pub const DCS: char = '\u{90}';
    /// Private Use 1
    pub const PU1: char = '\u{91}';
    /// Private Use 2
    pub const PU2: char = '\u{92}';
    /// Set Transmit State
    pub const STS: char = '\u{93}';
    /// Destructive backspace, intended to eliminate ambiguity about meaning of BS.
    pub const CCH: char = '\u{94}';
    /// Message Waiting
    pub const MW: char = '\u{95}';
    /// Start of Protected Area
    pub const SPA: char = '\u{96}';
    /// End of Protected Area
    pub const EPA: char = '\u{97}';
    /// Start of String
    pub const SOS: char = '\u{98}';
    /// Single Graphic Character Introducer
    pub const SGCI: char = '\u{99}';
    /// Single Character Introducer
    pub const SCI: char = '\u{9a}';
    /// Control Sequence Introducer
    pub const CSI: char = '\u{9b}';
    /// String Terminator
    pub const ST: char = '\u{9c}';
    /// Operating System Command
    pub const OSC: char = '\u{9d}';
    /// Privacy Message
    pub const PM: char = '\u{9e}';
    /// Application Program Command
    pub const APC: char = '\u{9f}';
}

/// Escaped control sequences (`ESC` followed by a final byte).
#[allow(non_snake_case)]
pub mod C1_ESCAPED {
    /// String Terminator as the 7-bit sequence `ESC \`.
    pub const ST: &str = "\x1b\\";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c0_codes_match_their_ascii_values() {
        assert_eq!(C0::NUL as u32, 0x00);
        assert_eq!(C0::BEL as u32, 0x07);
        assert_eq!(C0::ESC as u32, 0x1b);
        assert_eq!(C0::US as u32, 0x1f);
        assert_eq!(C0::SP as u32, 0x20);
        assert_eq!(C0::DEL as u32, 0x7f);
    }

    #[test]
    fn c1_codes_are_code_points_0x80_to_0x9f() {
        assert_eq!(C1::PAD as u32, 0x80);
        assert_eq!(C1::CSI as u32, 0x9b);
        assert_eq!(C1::ST as u32, 0x9c);
        assert_eq!(C1::OSC as u32, 0x9d);
        assert_eq!(C1::APC as u32, 0x9f);
    }

    #[test]
    fn c1_escaped_st_is_esc_backslash() {
        assert_eq!(C1_ESCAPED::ST, format!("{}\\", C0::ESC));
    }
}
