//! The legacy (xterm) key and character generators. Port of the key and char parts of Lunegit's
//! `Lunegit.Infrastructure/Terminal/XtermVtInputEncoder.cs` (`GenerateKeyInput`,
//! `GenerateCharInput`; plan M7 step 5), itself a byte-for-byte port of XTerm.NET's
//! `KeyboardInputGenerator`. Headless xterm.js has no input encoder, so these are what Lunegit
//! sends; they are not xterm.js `Keyboard.ts` (see `key_encoder_tests.rs` for how its tests map).
//!
//! Mouse is not here: [`crate::services::core_mouse_service`] encodes it from the engine's own
//! mode state (plan D5).

use super::key_event::{TerminalKey, TerminalModifiers};

const ESC: char = '\u{1b}';

/// Lunegit's `GenerateKeyInput`: the legacy bytes of a functional key, with the live DECCKM state.
/// `TerminalKey::Character` yields an empty string (it has no legacy key sequence). Modifier bits
/// other than Shift, Alt and Control are ignored (Lunegit masks them before calling this).
pub fn legacy_key_input(
    key: TerminalKey,
    modifiers: TerminalModifiers,
    application_cursor_keys: bool,
) -> String {
    let modifiers = modifiers.masked();
    // XTerm.NET returns NUL for Ctrl+Space before the main switch.
    if modifiers.contains(TerminalModifiers::CONTROL) && key == TerminalKey::Space {
        return "\0".to_string();
    }
    match key {
        TerminalKey::Enter => "\r".to_string(),
        TerminalKey::Tab => {
            if modifiers.contains(TerminalModifiers::SHIFT) {
                format!("{ESC}[Z")
            } else {
                "\t".to_string()
            }
        }
        TerminalKey::Backspace => "\u{7f}".to_string(),
        TerminalKey::Escape => {
            if modifiers.is_empty() {
                ESC.to_string()
            } else {
                modified_sequence("27", '~', modifiers)
            }
        }
        TerminalKey::Space => " ".to_string(),
        TerminalKey::Up => arrow_key('A', modifiers, application_cursor_keys),
        TerminalKey::Down => arrow_key('B', modifiers, application_cursor_keys),
        TerminalKey::Right => arrow_key('C', modifiers, application_cursor_keys),
        TerminalKey::Left => arrow_key('D', modifiers, application_cursor_keys),
        TerminalKey::Home => navigation_key('H', modifiers),
        TerminalKey::End => navigation_key('F', modifiers),
        TerminalKey::PageUp => modified_sequence("5", '~', modifiers),
        TerminalKey::PageDown => modified_sequence("6", '~', modifiers),
        TerminalKey::Insert => modified_sequence("2", '~', modifiers),
        TerminalKey::Delete => modified_sequence("3", '~', modifiers),
        TerminalKey::F1 => function_key(1, modifiers),
        TerminalKey::F2 => function_key(2, modifiers),
        TerminalKey::F3 => function_key(3, modifiers),
        TerminalKey::F4 => function_key(4, modifiers),
        TerminalKey::F5 => function_key(5, modifiers),
        TerminalKey::F6 => function_key(6, modifiers),
        TerminalKey::F7 => function_key(7, modifiers),
        TerminalKey::F8 => function_key(8, modifiers),
        TerminalKey::F9 => function_key(9, modifiers),
        TerminalKey::F10 => function_key(10, modifiers),
        TerminalKey::F11 => function_key(11, modifiers),
        TerminalKey::F12 => function_key(12, modifiers),
        TerminalKey::Character => String::new(),
    }
}

/// Lunegit's `GenerateCharInput`: Ctrl folds letters (and `@ [ \ ] ^ _ ? space`) to their C0
/// byte, Alt prefixes ESC. Unlike [`super::key_encoder::encode_key`] it takes any `char` and the
/// modifiers as given (only the Control and Alt bits are read).
pub fn legacy_char_input(c: char, modifiers: TerminalModifiers) -> String {
    let ctrl = modifiers.contains(TerminalModifiers::CONTROL);
    let alt = modifiers.contains(TerminalModifiers::ALT);

    let text = if !ctrl {
        c.to_string()
    } else {
        match c {
            'a'..='z' => from_u8(c as u8 - b'a' + 1),
            'A'..='Z' => from_u8(c as u8 - b'A' + 1),
            ' ' | '@' => "\0".to_string(),
            '[' => from_u8(0x1b),
            '\\' => from_u8(0x1c),
            ']' => from_u8(0x1d),
            '^' => from_u8(0x1e),
            '_' => from_u8(0x1f),
            '?' => from_u8(0x7f),
            other => other.to_string(),
        }
    };

    if alt { format!("{ESC}{text}") } else { text }
}

fn from_u8(b: u8) -> String {
    char::from(b).to_string()
}

fn arrow_key(
    direction: char,
    modifiers: TerminalModifiers,
    application_cursor_keys: bool,
) -> String {
    if modifiers.is_empty() {
        return if application_cursor_keys {
            format!("{ESC}O{direction}")
        } else {
            format!("{ESC}[{direction}")
        };
    }
    format!("{ESC}[1;{}{direction}", modifiers.param())
}

fn navigation_key(key: char, modifiers: TerminalModifiers) -> String {
    if modifiers.is_empty() {
        format!("{ESC}[{key}")
    } else {
        format!("{ESC}[1;{}{key}", modifiers.param())
    }
}

/// `ESC [ <prefix> <final>` or, with modifiers, `ESC [ <prefix> ; <mod> <final>`.
fn modified_sequence(prefix: &str, final_byte: char, modifiers: TerminalModifiers) -> String {
    if modifiers.is_empty() {
        format!("{ESC}[{prefix}{final_byte}")
    } else {
        format!("{ESC}[{prefix};{}{final_byte}", modifiers.param())
    }
}

fn function_key(number: u32, modifiers: TerminalModifiers) -> String {
    // F1-F4: SS3 (ESC O P..S), CSI 1;m P..S with modifiers.
    if (1..=4).contains(&number) {
        let value = char::from(b'P' + (number - 1) as u8);
        return if modifiers.is_empty() {
            format!("{ESC}O{value}")
        } else {
            format!("{ESC}[1;{}{value}", modifiers.param())
        };
    }
    // F5-F12: CSI <n> ~ (with an optional modifier).
    let num = match number {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return String::new(),
    };
    modified_sequence(&num.to_string(), '~', modifiers)
}

#[cfg(test)]
#[path = "legacy_key_encoder_tests.rs"]
mod tests;
