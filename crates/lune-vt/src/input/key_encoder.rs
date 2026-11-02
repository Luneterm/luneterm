//! The protocol-aware key encoder. Port of Lunegit's
//! `Lunegit.Infrastructure/Terminal/TerminalKeyEncoder.cs` and `KeyboardModes` (plan M7 step 5).
//!
//! Precedence: the kitty keyboard protocol (flag 1 set; flag 4 adds alternate keys) -> xterm
//! modifyOtherKeys (levels 1 and 2) -> legacy. The legacy branch hands every case it does not
//! deliberately change to [`legacy_key_input`], so legacy bytes stay identical to it except the
//! listed deviations: Shift+Enter and Alt(meta)+Enter send `ESC CR`; Ctrl+Backspace sends BS;
//! Alt(meta)+Backspace sends `ESC DEL`; Alt(meta)+Tab is ESC-prefixed; Ctrl+digit and punctuation
//! go through the kitty C0 table.
//!
//! Stateless: the caller supplies the live [`KeyboardModes`] ([`crate::Terminal::key_event`]
//! reads them from the engine).
//!
//! `encode_key` never panics. A `Character` event whose `codepoint` is not a Unicode scalar value
//! (0, a surrogate, above `0x10FFFF`) sends nothing in any mode, as Lunegit does. Two cases
//! Lunegit does not cover also send nothing (an empty `Vec`) rather than a malformed sequence: a
//! modifyOtherKeys event whose shifted code point is not a scalar, and a kitty flag-4 event whose
//! base-layout code point is not a scalar. For every valid scalar the bytes are Lunegit's.

use super::key_event::{TerminalKey, TerminalKeyEvent, TerminalModifiers};
use super::legacy_key_encoder::legacy_key_input;

const ESC: char = '\u{1b}';
const DEL: &str = "\u{7f}";
const BS: &str = "\u{8}";

/// The engine's live keyboard modes. Lunegit packs them into one int (see [`KeyboardModes::pack`])
/// so its V8 driver can hand them over in one call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardModes {
    /// Kitty keyboard flags of the active screen.
    pub kitty_flags: u8,
    /// xterm modifyOtherKeys level; 2 and above behave as level 2.
    pub modify_other_keys: u8,
    /// DECCKM (application cursor keys).
    pub application_cursor_keys: bool,
}

impl KeyboardModes {
    const KITTY_MASK: u32 = 0xFF;
    const MOK_SHIFT: u32 = 8;
    const MOK_MASK: u32 = 0x3;
    const DECCKM_BIT: u32 = 1 << 16;

    /// Lunegit's `KeyboardModes.Unpack`: kitty flags in bits 0-7, modifyOtherKeys in bits 8-9
    /// (so at most 3), DECCKM in bit 16.
    pub const fn unpack(packed: u32) -> KeyboardModes {
        KeyboardModes {
            kitty_flags: (packed & Self::KITTY_MASK) as u8,
            modify_other_keys: ((packed >> Self::MOK_SHIFT) & Self::MOK_MASK) as u8,
            application_cursor_keys: packed & Self::DECCKM_BIT != 0,
        }
    }

    /// Lunegit's `KeyboardModes.Pack`; the modifyOtherKeys level keeps its low two bits.
    pub const fn pack(self) -> u32 {
        (self.kitty_flags as u32 & Self::KITTY_MASK)
            | ((self.modify_other_keys as u32 & Self::MOK_MASK) << Self::MOK_SHIFT)
            | if self.application_cursor_keys {
                Self::DECCKM_BIT
            } else {
                0
            }
    }
}

/// The bytes to write to the PTY for `event` under `modes`; empty when nothing is sent.
pub fn encode_key(event: &TerminalKeyEvent, modes: KeyboardModes) -> Vec<u8> {
    encode_key_string(event, modes).into_bytes()
}

fn encode_key_string(event: &TerminalKeyEvent, modes: KeyboardModes) -> String {
    let mut e = *event;
    e.modifiers = e.modifiers.masked();
    let kitty = modes.kitty_flags & 1 != 0;

    if (kitty || modes.modify_other_keys >= 1) && e.key == TerminalKey::Space {
        e.key = TerminalKey::Character;
        e.codepoint = ' ' as u32;
        e.shifted_codepoint = ' ' as u32;
        e.base_layout_codepoint = 0;
    }

    if e.key == TerminalKey::Character && !is_scalar(e.codepoint) {
        // Defensive: a character event without a valid character sends nothing in any mode
        // (never a lone ESC prefix or a CSI with a bogus key number).
        return String::new();
    }

    if kitty {
        return encode_kitty(&e, modes.kitty_flags);
    }
    if modes.modify_other_keys >= 1 {
        return encode_modify_other_keys(&e, modes);
    }
    encode_legacy(&e, modes)
}

// ---- shared ---------------------------------------------------------------------------------

fn has(mods: TerminalModifiers, flag: TerminalModifiers) -> bool {
    mods.intersects(flag)
}

fn is_ascii_letter(cp: u32) -> bool {
    matches!(cp, 0x41..=0x5A | 0x61..=0x7A)
}

/// Whether `cp` is a Unicode scalar value other than NUL (Lunegit's `IsScalar`).
fn is_scalar(cp: u32) -> bool {
    cp != 0 && char::from_u32(cp).is_some()
}

/// Base (unshifted) code point: lowercase for ASCII letters, else `codepoint`.
fn base_of(e: &TerminalKeyEvent) -> u32 {
    if is_ascii_letter(e.codepoint) {
        e.codepoint | 0x20
    } else {
        e.codepoint
    }
}

/// Shifted code point: the given value, else uppercase for ASCII letters, else 0 (unknown).
fn shifted_of(e: &TerminalKeyEvent) -> u32 {
    if e.shifted_codepoint != 0 {
        e.shifted_codepoint
    } else if is_ascii_letter(e.codepoint) {
        e.codepoint & !0x20
    } else {
        0
    }
}

/// The character the key produces: the shifted one when Shift is held and known.
fn produced(e: &TerminalKeyEvent) -> u32 {
    let shifted = shifted_of(e);
    if has(e.modifiers, TerminalModifiers::SHIFT) && shifted != 0 {
        shifted
    } else {
        e.codepoint
    }
}

/// Text passthrough: empty for a code point that is not a valid scalar.
fn text(cp: u32) -> String {
    if is_scalar(cp) {
        char::from_u32(cp).map(String::from).unwrap_or_default()
    } else {
        String::new()
    }
}

fn from_u8(b: u8) -> String {
    char::from(b).to_string()
}

// ---- legacy ---------------------------------------------------------------------------------

fn encode_legacy(e: &TerminalKeyEvent, modes: KeyboardModes) -> String {
    let mods = e.modifiers;
    let alt_esc = has(mods, TerminalModifiers::ALT) && e.alt_is_meta;
    let esc_if = |on: bool| if on { ESC.to_string() } else { String::new() };

    match e.key {
        // Shift+Enter -> ESC CR is Lunegit's pre-existing deliberate hack, for any combination
        // containing Shift.
        TerminalKey::Enter => {
            if has(mods, TerminalModifiers::SHIFT) || alt_esc {
                format!("{ESC}\r")
            } else {
                "\r".to_string()
            }
        }
        TerminalKey::Backspace => {
            let c = if has(mods, TerminalModifiers::CONTROL) {
                BS
            } else {
                DEL
            };
            format!("{}{c}", esc_if(alt_esc))
        }
        TerminalKey::Tab => {
            let t = if has(mods, TerminalModifiers::SHIFT) {
                format!("{ESC}[Z")
            } else {
                "\t".to_string()
            };
            format!("{}{t}", esc_if(alt_esc))
        }
        TerminalKey::Character => encode_legacy_char(e, alt_esc),
        key => legacy_key_input(key, mods, modes.application_cursor_keys),
    }
}

fn encode_legacy_char(e: &TerminalKeyEvent, alt_esc: bool) -> String {
    let mods = e.modifiers;
    let produced = produced(e);

    if has(mods, TerminalModifiers::CONTROL) {
        if is_ascii_letter(e.codepoint) {
            // Today's bytes: Ctrl+letter keeps the ESC prefix whenever Alt is held, regardless of
            // alt_is_meta (macOS Ctrl+Option+letter has always sent it).
            let c0 = from_u8((base_of(e) - 'a' as u32 + 1) as u8);
            return if has(mods, TerminalModifiers::ALT) {
                format!("{ESC}{c0}")
            } else {
                c0
            };
        }
        let mapped = ctrl_map(produced);
        return if alt_esc {
            format!("{ESC}{mapped}")
        } else {
            mapped
        };
    }

    if alt_esc {
        return format!("{ESC}{}", text(produced));
    }
    // Defensive: the router never sends a plain/Shift-only character here.
    text(produced)
}

/// The kitty spec's legacy Ctrl table for non-letters; anything unlisted passes through.
fn ctrl_map(k: u32) -> String {
    match char::from_u32(k) {
        Some(' ' | '2' | '@') => "\0".to_string(),
        Some('3' | '[') => from_u8(0x1b),
        Some('4' | '\\') => from_u8(0x1c),
        Some('5' | ']') => from_u8(0x1d),
        Some('6' | '^' | '~') => from_u8(0x1e),
        Some('7' | '/' | '_') => from_u8(0x1f),
        Some('8' | '?') => DEL.to_string(),
        _ => text(k),
    }
}

// ---- modifyOtherKeys ------------------------------------------------------------------------

fn encode_modify_other_keys(e: &TerminalKeyEvent, modes: KeyboardModes) -> String {
    let mods = e.modifiers;
    let level2 = modes.modify_other_keys >= 2;
    let has_mods = !mods.is_empty();
    let mp = mods.param();
    let without_ctrl = mods.bits() & !TerminalModifiers::CONTROL.bits();

    match e.key {
        TerminalKey::Enter if has_mods => mok(mp, 13),
        // Shift-only Tab stays CSI Z (xterm's actual behaviour) at both levels: legacy.
        TerminalKey::Tab if has_mods && mods != TerminalModifiers::SHIFT => mok(mp, 9),
        TerminalKey::Escape if level2 && has_mods => mok(mp, 27),
        TerminalKey::Backspace if level2 && without_ctrl != 0 => mok(mp, 127),
        TerminalKey::Character
            if level2
                && (has(mods, TerminalModifiers::CONTROL) || has(mods, TerminalModifiers::ALT)) =>
        {
            let shifted = shifted_of(e);
            let cp = if has(mods, TerminalModifiers::SHIFT) && shifted != 0 {
                shifted
            } else {
                base_of(e)
            };
            if is_scalar(cp) {
                mok(mp, cp)
            } else {
                String::new()
            }
        }
        _ => encode_legacy(e, modes),
    }
}

fn mok(modifier_param: u32, code: u32) -> String {
    format!("{ESC}[27;{modifier_param};{code}~")
}

// ---- kitty ----------------------------------------------------------------------------------

fn encode_kitty(e: &TerminalKeyEvent, flags: u8) -> String {
    let mods = e.modifiers;
    let has_mods = !mods.is_empty();
    let mp = mods.param();

    // CSI <final> / CSI 1;m <final> (no "1;" without modifiers).
    let letter = |final_byte: char| {
        if has_mods {
            format!("{ESC}[1;{mp}{final_byte}")
        } else {
            format!("{ESC}[{final_byte}")
        }
    };
    // CSI n <final> / CSI n;m <final>.
    let numbered = |n: u32, final_byte: char| {
        if has_mods {
            format!("{ESC}[{n};{mp}{final_byte}")
        } else {
            format!("{ESC}[{n}{final_byte}")
        }
    };
    let unicode = |code: u32| numbered(code, 'u');
    // A keypad key sends its KP_* code as a `u` sequence; otherwise `plain`.
    let keypad_or = |code: u32, plain: String| {
        if e.is_keypad { unicode(code) } else { plain }
    };

    match e.key {
        TerminalKey::Enter => {
            if e.is_keypad {
                unicode(57414)
            } else if has_mods {
                unicode(13)
            } else {
                "\r".to_string()
            }
        }
        TerminalKey::Tab => {
            if has_mods {
                unicode(9)
            } else {
                "\t".to_string()
            }
        }
        TerminalKey::Backspace => {
            if has_mods {
                unicode(127)
            } else {
                DEL.to_string()
            }
        }
        TerminalKey::Escape => unicode(27),
        TerminalKey::Up => keypad_or(57419, letter('A')),
        TerminalKey::Down => keypad_or(57420, letter('B')),
        TerminalKey::Right => keypad_or(57418, letter('C')),
        TerminalKey::Left => keypad_or(57417, letter('D')),
        TerminalKey::Home => keypad_or(57423, letter('H')),
        TerminalKey::End => keypad_or(57424, letter('F')),
        TerminalKey::PageUp => keypad_or(57421, numbered(5, '~')),
        TerminalKey::PageDown => keypad_or(57422, numbered(6, '~')),
        TerminalKey::Insert => keypad_or(57425, numbered(2, '~')),
        TerminalKey::Delete => keypad_or(57426, numbered(3, '~')),
        TerminalKey::F1 => letter('P'),
        TerminalKey::F2 => letter('Q'),
        // Not CSI R: that collides with the cursor-position report.
        TerminalKey::F3 => numbered(13, '~'),
        TerminalKey::F4 => letter('S'),
        TerminalKey::F5 => numbered(15, '~'),
        TerminalKey::F6 => numbered(17, '~'),
        TerminalKey::F7 => numbered(18, '~'),
        TerminalKey::F8 => numbered(19, '~'),
        TerminalKey::F9 => numbered(20, '~'),
        TerminalKey::F10 => numbered(21, '~'),
        TerminalKey::F11 => numbered(23, '~'),
        TerminalKey::F12 => numbered(24, '~'),
        TerminalKey::Character => encode_kitty_char(e, flags, mp),
        // Space was turned into a Character before this point.
        TerminalKey::Space => String::new(),
    }
}

fn encode_kitty_char(e: &TerminalKeyEvent, flags: u8, mp: u32) -> String {
    let mods = e.modifiers;
    if !has(mods, TerminalModifiers::CONTROL) && !has(mods, TerminalModifiers::ALT) {
        // Plain/Shift text, as in legacy.
        return text(produced(e));
    }

    let key = if e.is_keypad {
        keypad_code(e.codepoint)
    } else {
        base_of(e)
    };
    let mut alternates = String::new();
    if flags & 4 != 0 && !e.is_keypad {
        // Shifted key only for letters (uppercase is layout-independent); never for
        // digits/punctuation, whose shifted form depends on the layout.
        let shifted = if has(mods, TerminalModifiers::SHIFT) && is_ascii_letter(e.codepoint) {
            key & !0x20
        } else {
            0
        };
        let base_layout = if e.base_layout_codepoint != 0 && e.base_layout_codepoint != key {
            e.base_layout_codepoint
        } else {
            0
        };
        if base_layout != 0 && !is_scalar(base_layout) {
            // Not covered by Lunegit's tests: a garbage alternate key would be written to the
            // child as a number. Send nothing instead.
            return String::new();
        }
        if shifted != 0 || base_layout != 0 {
            alternates.push(':');
            if shifted != 0 {
                alternates.push_str(&shifted.to_string());
            }
            if base_layout != 0 {
                alternates.push(':');
                alternates.push_str(&base_layout.to_string());
            }
        }
    }

    format!("{ESC}[{key}{alternates};{mp}u")
}

/// kitty KP_* functional codes for keypad characters; unknown ones keep the code point.
fn keypad_code(cp: u32) -> u32 {
    match char::from_u32(cp) {
        Some(c @ '0'..='9') => 57399 + (c as u32 - '0' as u32),
        Some('.') => 57409,
        Some('/') => 57410,
        Some('*') => 57411,
        Some('-') => 57412,
        Some('+') => 57413,
        Some('=') => 57415,
        Some(',') => 57416,
        _ => cp,
    }
}

#[cfg(test)]
#[path = "key_encoder_tests.rs"]
mod tests;
