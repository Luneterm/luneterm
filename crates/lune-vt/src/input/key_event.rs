//! The key types the encoders take. Port of Lunegit's `Lunegit.Core/Terminal/TerminalInput.cs`
//! (`TerminalKey`, `TerminalKeyEvent`, `TerminalModifiers`; plan M7 step 5, D4).
//!
//! These are library-agnostic: the host maps its toolkit's key data onto them. They replace the DOM
//! `KeyboardEvent` fields (`keyCode`, `key`, `code`) that xterm.js's `Keyboard.ts` reads (roadmap
//! D8). The mouse types live in [`super::mouse_event`].

use std::ops::{BitAnd, BitOr, BitOrAssign};

/// A special key, or [`TerminalKey::Character`] for a text-producing key described by
/// [`TerminalKeyEvent::codepoint`]. The order is Lunegit's (`Character` last).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TerminalKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Space,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    /// A text-producing key (letter, digit, punctuation, keypad character).
    Character,
}

impl TerminalKey {
    /// Every key, in Lunegit's enum order.
    pub const ALL: [TerminalKey; 28] = [
        TerminalKey::Enter,
        TerminalKey::Tab,
        TerminalKey::Backspace,
        TerminalKey::Escape,
        TerminalKey::Space,
        TerminalKey::Up,
        TerminalKey::Down,
        TerminalKey::Right,
        TerminalKey::Left,
        TerminalKey::Home,
        TerminalKey::End,
        TerminalKey::PageUp,
        TerminalKey::PageDown,
        TerminalKey::Insert,
        TerminalKey::Delete,
        TerminalKey::F1,
        TerminalKey::F2,
        TerminalKey::F3,
        TerminalKey::F4,
        TerminalKey::F5,
        TerminalKey::F6,
        TerminalKey::F7,
        TerminalKey::F8,
        TerminalKey::F9,
        TerminalKey::F10,
        TerminalKey::F11,
        TerminalKey::F12,
        TerminalKey::Character,
    ];

    /// Lunegit's name for the key (`Enter`, `PageUp`, `F5`, ...), as the goldens spell it.
    pub fn name(self) -> &'static str {
        match self {
            TerminalKey::Enter => "Enter",
            TerminalKey::Tab => "Tab",
            TerminalKey::Backspace => "Backspace",
            TerminalKey::Escape => "Escape",
            TerminalKey::Space => "Space",
            TerminalKey::Up => "Up",
            TerminalKey::Down => "Down",
            TerminalKey::Right => "Right",
            TerminalKey::Left => "Left",
            TerminalKey::Home => "Home",
            TerminalKey::End => "End",
            TerminalKey::PageUp => "PageUp",
            TerminalKey::PageDown => "PageDown",
            TerminalKey::Insert => "Insert",
            TerminalKey::Delete => "Delete",
            TerminalKey::F1 => "F1",
            TerminalKey::F2 => "F2",
            TerminalKey::F3 => "F3",
            TerminalKey::F4 => "F4",
            TerminalKey::F5 => "F5",
            TerminalKey::F6 => "F6",
            TerminalKey::F7 => "F7",
            TerminalKey::F8 => "F8",
            TerminalKey::F9 => "F9",
            TerminalKey::F10 => "F10",
            TerminalKey::F11 => "F11",
            TerminalKey::F12 => "F12",
            TerminalKey::Character => "Character",
        }
    }

    /// The key whose [`TerminalKey::name`] is `name`.
    pub fn from_name(name: &str) -> Option<TerminalKey> {
        TerminalKey::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Modifier flags (Shift 1, Alt 2, Control 4), as Lunegit's `TerminalModifiers`. Any `u8` is
/// accepted; the encoders look at the three known bits only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TerminalModifiers(u8);

impl TerminalModifiers {
    pub const NONE: TerminalModifiers = TerminalModifiers(0);
    pub const SHIFT: TerminalModifiers = TerminalModifiers(1);
    pub const ALT: TerminalModifiers = TerminalModifiers(2);
    pub const CONTROL: TerminalModifiers = TerminalModifiers(4);

    /// The three bits the encoders honour.
    const KNOWN: u8 = 7;

    /// The raw bits, unknown ones kept.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// From raw bits (any value; unknown bits are kept and ignored by the encoders).
    pub const fn from_bits(bits: u8) -> TerminalModifiers {
        TerminalModifiers(bits)
    }

    /// Only Shift, Alt and Control (what `TerminalKeyEncoder.Encode` does first).
    pub const fn masked(self) -> TerminalModifiers {
        TerminalModifiers(self.0 & Self::KNOWN)
    }

    /// Whether every bit of `flag` is set.
    pub const fn contains(self, flag: TerminalModifiers) -> bool {
        self.0 & flag.0 == flag.0
    }

    /// Whether any bit of `flags` is set.
    pub const fn intersects(self, flags: TerminalModifiers) -> bool {
        self.0 & flags.0 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `1 + Shift + 2 Alt + 4 Control`: the modifier parameter of the CSI forms.
    pub(crate) const fn param(self) -> u32 {
        1 + (if self.intersects(Self::SHIFT) { 1 } else { 0 })
            + (if self.intersects(Self::ALT) { 2 } else { 0 })
            + (if self.intersects(Self::CONTROL) { 4 } else { 0 })
    }
}

impl BitOr for TerminalModifiers {
    type Output = TerminalModifiers;

    fn bitor(self, rhs: TerminalModifiers) -> TerminalModifiers {
        TerminalModifiers(self.0 | rhs.0)
    }
}

impl BitOrAssign for TerminalModifiers {
    fn bitor_assign(&mut self, rhs: TerminalModifiers) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for TerminalModifiers {
    type Output = TerminalModifiers;

    fn bitand(self, rhs: TerminalModifiers) -> TerminalModifiers {
        TerminalModifiers(self.0 & rhs.0)
    }
}

/// One keystroke for the protocol-aware key encoder. Fields as Lunegit's `TerminalKeyEvent`
/// (code points are `u32` here: a value above `0x10FFFF` or a surrogate is "not a scalar value"
/// and sends nothing, as Lunegit's negative or out-of-range `int`s do).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalKeyEvent {
    /// The key. [`TerminalKey::Character`] means the key is described by `codepoint`.
    pub key: TerminalKey,
    /// Shift/Alt/Control held.
    pub modifiers: TerminalModifiers,
    /// For `Character`: the unshifted base character (lowercase for letters), or the keypad
    /// character when `is_keypad`.
    pub codepoint: u32,
    /// The character the key produces with Shift, 0 = unknown. For an ASCII-letter `codepoint`,
    /// 0 means "the uppercase letter".
    pub shifted_codepoint: u32,
    /// The key's character on the US-QWERTY base layout (kitty alternate-keys flag), 0 = unknown.
    pub base_layout_codepoint: u32,
    /// The key is on the numeric keypad.
    pub is_keypad: bool,
    /// Whether Alt acts as Meta. Gates only the legacy ESC prefix; protocol encodings always
    /// carry the Alt modifier bit.
    pub alt_is_meta: bool,
}

impl TerminalKeyEvent {
    /// A functional key (anything but `Character`); `alt_is_meta` is on, as Lunegit's default.
    pub const fn for_key(key: TerminalKey, modifiers: TerminalModifiers) -> TerminalKeyEvent {
        TerminalKeyEvent {
            key,
            modifiers,
            codepoint: 0,
            shifted_codepoint: 0,
            base_layout_codepoint: 0,
            is_keypad: false,
            alt_is_meta: true,
        }
    }

    /// A character that is already what the key produced: sets both `codepoint` and
    /// `shifted_codepoint` to `c`, so with no protocol active the encoding matches the plain char
    /// path (Shift never re-maps the character).
    pub const fn for_char(c: char, modifiers: TerminalModifiers) -> TerminalKeyEvent {
        TerminalKeyEvent {
            key: TerminalKey::Character,
            modifiers,
            codepoint: c as u32,
            shifted_codepoint: c as u32,
            base_layout_codepoint: 0,
            is_keypad: false,
            alt_is_meta: true,
        }
    }
}
