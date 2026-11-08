//! Keys (plan M9 D4, D5): a winit key event becomes an app shortcut, a [`TerminalKeyEvent`] for the
//! protocol-aware encoder, plain text for the PTY, or nothing.
//!
//! [`route_key`] is a port of Lunegit's `TerminalKeyRouter.TryRoute`
//! (`Lunegit.Desktop/Controls/Terminal/TerminalKeyRouter.cs`, cited as `R<line>` below). Like the
//! original it does not look at the engine's keyboard modes: the same keystrokes route the same way
//! in every mode and the encoder alone decides the bytes. It differs in three places (D5): Option
//! as Alt needs winit's `OptionAsAlt` (set by the window code), Ctrl+Alt+letter that produced
//! printable text is AltGr text, and `Process`, dead keys, synthetic presses and releases are
//! dropped.
//!
//! The function takes a [`KeyInput`], not winit's `KeyEvent`, which cannot be built outside winit;
//! [`key_input`] is the one glue function.

use lune_ui::input::{Key as UiKey, Keymap, Modifiers, NamedKey as UiNamed};
use lune_vt::{TerminalKey, TerminalKeyEvent, TerminalModifiers};
use winit::event::KeyEvent;
use winit::keyboard::{Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use super::shortcuts::{AppCommand, AppShortcuts};

/// One key event, reduced to what routing reads.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyInput {
    /// The layout's key with modifiers applied (`KeyEvent::logical_key`).
    pub logical: Key,
    /// The key's position (`KeyEvent::physical_key`).
    pub physical: PhysicalKey,
    /// The text the press produced, modifiers applied (`KeyEvent::text`).
    pub text: Option<String>,
    /// The layout's key with no modifier applied (`key_without_modifiers`).
    pub unmodified: Option<Key>,
    pub location: KeyLocation,
    pub pressed: bool,
    pub repeat: bool,
    /// Made by the window system on focus change, not by the user.
    pub synthetic: bool,
    pub mods: ModifiersState,
}

/// The glue: a winit key event, the modifiers held and whether it is synthetic.
pub fn key_input(event: &KeyEvent, mods: ModifiersState, synthetic: bool) -> KeyInput {
    KeyInput {
        logical: event.logical_key.clone(),
        physical: event.physical_key,
        text: event.text.as_ref().map(|t| t.to_string()),
        unmodified: Some(event.key_without_modifiers()),
        location: event.location,
        pressed: event.state.is_pressed(),
        repeat: event.repeat,
        synthetic,
        mods,
    }
}

/// What routing needs besides the key.
#[derive(Debug, Clone)]
pub struct RouteCtx<'a> {
    pub macos: bool,
    /// "Use Option as Meta" (macOS): Option+key is Alt+key instead of composed text.
    pub option_as_meta: bool,
    pub keymap: Keymap,
    pub shortcuts: &'a AppShortcuts,
}

/// Where a key goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRoute {
    /// An app command; the key goes no further.
    Shortcut(AppCommand),
    /// Encode with the protocol-aware encoder.
    Terminal(TerminalKeyEvent),
    /// Send as typed text.
    Text(String),
    Ignore,
}

/// Shift, Alt and Control as the encoder's flags; Logo is never sent (R:71-79).
pub fn terminal_mods(mods: ModifiersState) -> TerminalModifiers {
    let mut m = TerminalModifiers::NONE;
    if mods.shift_key() {
        m |= TerminalModifiers::SHIFT;
    }
    if mods.alt_key() {
        m |= TerminalModifiers::ALT;
    }
    if mods.control_key() {
        m |= TerminalModifiers::CONTROL;
    }
    m
}

/// The modifiers as the toolkit's chords read them.
pub fn toolkit_mods(mods: ModifiersState) -> Modifiers {
    Modifiers {
        shift: mods.shift_key(),
        control: mods.control_key(),
        alt: mods.alt_key(),
        logo: mods.super_key(),
    }
}

/// The toolkit key of a winit key, for `Chord::matches`.
fn toolkit_key(key: &Key) -> UiKey {
    let named = match key {
        Key::Character(s) => return UiKey::Character(s.to_string()),
        Key::Named(n) => n,
        _ => return UiKey::Unidentified,
    };
    let n = match named {
        NamedKey::Enter => UiNamed::Enter,
        NamedKey::Tab => UiNamed::Tab,
        NamedKey::Space => UiNamed::Space,
        NamedKey::Backspace => UiNamed::Backspace,
        NamedKey::Delete => UiNamed::Delete,
        NamedKey::Insert => UiNamed::Insert,
        NamedKey::Escape => UiNamed::Escape,
        NamedKey::Home => UiNamed::Home,
        NamedKey::End => UiNamed::End,
        NamedKey::PageUp => UiNamed::PageUp,
        NamedKey::PageDown => UiNamed::PageDown,
        NamedKey::ArrowLeft => UiNamed::ArrowLeft,
        NamedKey::ArrowRight => UiNamed::ArrowRight,
        NamedKey::ArrowUp => UiNamed::ArrowUp,
        NamedKey::ArrowDown => UiNamed::ArrowDown,
        _ => return UiKey::Unidentified,
    };
    UiKey::Named(n)
}

/// R:81-115: the functional keys, always routed.
fn functional_key(key: &Key) -> Option<TerminalKey> {
    let Key::Named(n) = key else { return None };
    Some(match n {
        NamedKey::Enter => TerminalKey::Enter,
        NamedKey::Tab => TerminalKey::Tab,
        NamedKey::Backspace => TerminalKey::Backspace,
        NamedKey::Escape => TerminalKey::Escape,
        NamedKey::ArrowUp => TerminalKey::Up,
        NamedKey::ArrowDown => TerminalKey::Down,
        NamedKey::ArrowLeft => TerminalKey::Left,
        NamedKey::ArrowRight => TerminalKey::Right,
        NamedKey::Home => TerminalKey::Home,
        NamedKey::End => TerminalKey::End,
        NamedKey::PageUp => TerminalKey::PageUp,
        NamedKey::PageDown => TerminalKey::PageDown,
        NamedKey::Insert => TerminalKey::Insert,
        NamedKey::Delete => TerminalKey::Delete,
        NamedKey::F1 => TerminalKey::F1,
        NamedKey::F2 => TerminalKey::F2,
        NamedKey::F3 => TerminalKey::F3,
        NamedKey::F4 => TerminalKey::F4,
        NamedKey::F5 => TerminalKey::F5,
        NamedKey::F6 => TerminalKey::F6,
        NamedKey::F7 => TerminalKey::F7,
        NamedKey::F8 => TerminalKey::F8,
        NamedKey::F9 => TerminalKey::F9,
        NamedKey::F10 => TerminalKey::F10,
        NamedKey::F11 => TerminalKey::F11,
        NamedKey::F12 => TerminalKey::F12,
        _ => return None,
    })
}

/// A letter's US-QWERTY character from its physical key.
fn physical_letter(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return None,
    })
}

/// R:173-184: digits, Space and punctuation, US-QWERTY positional: (unshifted, shifted).
fn main_key(code: KeyCode) -> Option<(char, char)> {
    Some(match code {
        KeyCode::Digit0 => ('0', ')'),
        KeyCode::Digit1 => ('1', '!'),
        KeyCode::Digit2 => ('2', '@'),
        KeyCode::Digit3 => ('3', '#'),
        KeyCode::Digit4 => ('4', '$'),
        KeyCode::Digit5 => ('5', '%'),
        KeyCode::Digit6 => ('6', '^'),
        KeyCode::Digit7 => ('7', '&'),
        KeyCode::Digit8 => ('8', '*'),
        KeyCode::Digit9 => ('9', '('),
        KeyCode::Space => (' ', ' '),
        KeyCode::Minus => ('-', '_'),
        KeyCode::Equal => ('=', '+'),
        KeyCode::BracketLeft => ('[', '{'),
        KeyCode::BracketRight => (']', '}'),
        KeyCode::Backslash => ('\\', '|'),
        KeyCode::Semicolon => (';', ':'),
        KeyCode::Quote => ('\'', '"'),
        KeyCode::Backquote => ('`', '~'),
        KeyCode::Comma => (',', '<'),
        KeyCode::Period => ('.', '>'),
        KeyCode::Slash => ('/', '?'),
        _ => return None,
    })
}

/// R:162-171: the keypad's characters.
fn keypad_key(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::Numpad0 => '0',
        KeyCode::Numpad1 => '1',
        KeyCode::Numpad2 => '2',
        KeyCode::Numpad3 => '3',
        KeyCode::Numpad4 => '4',
        KeyCode::Numpad5 => '5',
        KeyCode::Numpad6 => '6',
        KeyCode::Numpad7 => '7',
        KeyCode::Numpad8 => '8',
        KeyCode::Numpad9 => '9',
        KeyCode::NumpadDecimal => '.',
        KeyCode::NumpadDivide => '/',
        KeyCode::NumpadMultiply => '*',
        KeyCode::NumpadSubtract => '-',
        KeyCode::NumpadAdd => '+',
        KeyCode::NumpadEqual => '=',
        KeyCode::NumpadComma => ',',
        _ => return None,
    })
}

/// A character key as the encoder wants it (R:120-160).
struct CharKey {
    codepoint: char,
    shifted: u32,
    base_layout: u32,
    is_keypad: bool,
    is_letter: bool,
}

fn ascii_letter(key: &Key) -> Option<char> {
    match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn character_key(k: &KeyInput) -> Option<CharKey> {
    let code = match k.physical {
        PhysicalKey::Code(c) => Some(c),
        PhysicalKey::Unidentified(_) => None,
    };
    // R:135-141: letters follow the layout (the unmodified key first, then the modified one, then
    // the key's position, so a Cyrillic layout still gets Ctrl+C), with the US-QWERTY key at the
    // position as the kitty base-layout alternate.
    let letter = k
        .unmodified
        .as_ref()
        .and_then(ascii_letter)
        .or_else(|| ascii_letter(&k.logical))
        .or_else(|| code.and_then(physical_letter));
    if let Some(c) = letter {
        return Some(CharKey {
            codepoint: c,
            shifted: 0,
            base_layout: code.and_then(physical_letter).map_or(0, u32::from),
            is_keypad: false,
            is_letter: true,
        });
    }
    // R:143-148: the keypad, from its physical key.
    if let Some(c) = code.and_then(keypad_key) {
        return Some(CharKey {
            codepoint: c,
            shifted: 0,
            base_layout: 0,
            is_keypad: true,
            is_letter: false,
        });
    }
    // R:150-155: digits, Space and punctuation, from the physical key, US-QWERTY positional.
    if let Some((plain, shifted)) = code.and_then(main_key) {
        return Some(CharKey {
            codepoint: plain,
            shifted: u32::from(shifted),
            base_layout: 0,
            is_keypad: false,
            is_letter: false,
        });
    }
    // A Space whose position the platform did not give.
    (k.logical == Key::Named(NamedKey::Space)).then_some(CharKey {
        codepoint: ' ',
        shifted: u32::from(' '),
        base_layout: 0,
        is_keypad: false,
        is_letter: false,
    })
}

/// Text that is safe to type: no control characters (named keys cover those) and none of macOS's
/// private-use function-key characters (U+F700 to U+F8FF, which an unmapped F13 or media key
/// would otherwise type).
fn typable(text: &str) -> bool {
    !text.is_empty()
        && !text
            .chars()
            .any(|c| c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c))
}

/// Routes one key event (the order of the steps is the order of the rules in plan step 5).
pub fn route_key(k: &KeyInput, ctx: &RouteCtx<'_>) -> KeyRoute {
    // 1. Releases, synthetic presses, IME key events and dead keys are not keystrokes.
    if !k.pressed
        || k.synthetic
        || matches!(k.logical, Key::Named(NamedKey::Process) | Key::Dead(_))
    {
        return KeyRoute::Ignore;
    }
    // 2. App shortcuts, before the key router. A held paste chord would paste again and again.
    if let Some(cmd) =
        ctx.shortcuts
            .matching(&toolkit_key(&k.logical), toolkit_mods(k.mods), &ctx.keymap)
    {
        if cmd.once_per_press() && k.repeat {
            return KeyRoute::Ignore;
        }
        return KeyRoute::Shortcut(cmd);
    }
    // 3. Any other chord with Cmd/Win held is the OS's or the app's, never the shell's (R:71,
    //    "Meta never maps"; Lunegit encodes Cmd chords as bare keys, which M9 does not).
    if k.mods.super_key() {
        return KeyRoute::Ignore;
    }
    let mods = terminal_mods(k.mods);
    let alt_is_meta = !ctx.macos || ctx.option_as_meta;
    // 4. R:39-45: functional keys are always routed, with any modifiers. A keypad Enter, or a
    //    keypad navigation key with NumLock off, comes with its position.
    if let Some(key) = functional_key(&k.logical) {
        let mut ev = TerminalKeyEvent::for_key(key, mods);
        ev.is_keypad = k.location == KeyLocation::Numpad;
        ev.alt_is_meta = alt_is_meta;
        return KeyRoute::Terminal(ev);
    }
    // 5. R:48-68: character keys, only for chords the platform's text path cannot carry.
    if let Some(c) = character_key(k) {
        let (ctrl, alt) = (k.mods.control_key(), k.mods.alt_key());
        // D5 deviation 2: Ctrl+Alt+letter whose text is a printable character other than the
        // letter is AltGr composition (Polish AltGr+a is "ą"); Lunegit would encode it.
        let altgr_text =
            !ctx.macos && c.is_letter && k.text.as_deref().is_some_and(|t| {
                let mut chars = t.chars();
                matches!(
                    (chars.next(), chars.next()),
                    (Some(ch), None) if !ch.is_control() && !ch.eq_ignore_ascii_case(&c.codepoint)
                )
            });
        // Lunegit's three cases: Ctrl without Alt; Ctrl+Alt on a letter (less AltGr text); Alt
        // without Ctrl when Alt is Meta. The first two are one test on Ctrl.
        let ctrl_chord = ctrl && (!alt || (c.is_letter && !altgr_text));
        let routed = ctrl_chord || (alt && !ctrl && alt_is_meta);
        if routed {
            return KeyRoute::Terminal(TerminalKeyEvent {
                key: TerminalKey::Character,
                modifiers: mods,
                codepoint: u32::from(c.codepoint),
                shifted_codepoint: c.shifted,
                base_layout_codepoint: c.base_layout,
                is_keypad: c.is_keypad,
                alt_is_meta,
            });
        }
    }
    // 6. The rest is text, as Lunegit's `OnTextInput` sends it (TerminalView.cs:1089-1111).
    match &k.text {
        Some(t) if typable(t) => KeyRoute::Text(t.clone()),
        _ => KeyRoute::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Scroll;

    const NONE: ModifiersState = ModifiersState::empty();
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const ALT: ModifiersState = ModifiersState::ALT;
    const SUPER: ModifiersState = ModifiersState::SUPER;

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn named(n: NamedKey) -> Key {
        Key::Named(n)
    }

    /// A press of `logical` at `physical` with `mods`; `text` is what it types.
    fn press(
        logical: Key,
        physical: KeyCode,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> KeyInput {
        KeyInput {
            unmodified: Some(match &logical {
                Key::Character(s) => Key::Character(s.to_lowercase().as_str().into()),
                other => other.clone(),
            }),
            logical,
            physical: PhysicalKey::Code(physical),
            text: text.map(str::to_owned),
            location: KeyLocation::Standard,
            pressed: true,
            repeat: false,
            synthetic: false,
            mods,
        }
    }

    fn numpad(mut k: KeyInput) -> KeyInput {
        k.location = KeyLocation::Numpad;
        k
    }

    fn ctx(macos: bool, option_as_meta: bool, shortcuts: &AppShortcuts) -> RouteCtx<'_> {
        RouteCtx {
            macos,
            option_as_meta,
            keymap: if macos { Keymap::MAC } else { Keymap::PC },
            shortcuts,
        }
    }

    /// A character-key terminal event.
    fn chr(c: char, shifted: char, base: char, mods: TerminalModifiers, meta: bool) -> KeyRoute {
        KeyRoute::Terminal(TerminalKeyEvent {
            key: TerminalKey::Character,
            modifiers: mods,
            codepoint: u32::from(c),
            shifted_codepoint: if shifted == '\0' {
                0
            } else {
                u32::from(shifted)
            },
            base_layout_codepoint: if base == '\0' { 0 } else { u32::from(base) },
            is_keypad: false,
            alt_is_meta: meta,
        })
    }

    fn func(key: TerminalKey, mods: TerminalModifiers, keypad: bool, meta: bool) -> KeyRoute {
        let mut ev = TerminalKeyEvent::for_key(key, mods);
        ev.is_keypad = keypad;
        ev.alt_is_meta = meta;
        KeyRoute::Terminal(ev)
    }

    fn text(s: &str) -> KeyRoute {
        KeyRoute::Text(s.to_owned())
    }

    const S: TerminalModifiers = TerminalModifiers::SHIFT;
    const A: TerminalModifiers = TerminalModifiers::ALT;
    const C: TerminalModifiers = TerminalModifiers::CONTROL;

    fn or(a: TerminalModifiers, b: TerminalModifiers) -> TerminalModifiers {
        a | b
    }

    #[test]
    fn windows_and_linux_keys() {
        let sc = AppShortcuts::standard(false);
        let win = ctx(false, false, &sc);
        let cases: Vec<(&str, KeyInput, KeyRoute)> = vec![
            // Plain and shifted text goes to the platform text path (R:48-62).
            (
                "a",
                press(ch("a"), KeyCode::KeyA, Some("a"), NONE),
                text("a"),
            ),
            (
                "Shift+a",
                press(ch("A"), KeyCode::KeyA, Some("A"), SHIFT),
                text("A"),
            ),
            (
                "Shift+1",
                press(ch("!"), KeyCode::Digit1, Some("!"), SHIFT),
                text("!"),
            ),
            (
                "space",
                press(named(NamedKey::Space), KeyCode::Space, Some(" "), NONE),
                text(" "),
            ),
            (
                "Ctrl+a",
                press(ch("a"), KeyCode::KeyA, Some("\u{1}"), CTRL),
                chr('a', '\0', 'a', C, true),
            ),
            (
                "Ctrl+Shift+a",
                press(ch("A"), KeyCode::KeyA, Some("\u{1}"), CTRL | SHIFT),
                chr('a', '\0', 'a', or(S, C), true),
            ),
            // Alt is meta on Windows and Linux (R:37).
            (
                "Alt+a",
                press(ch("a"), KeyCode::KeyA, Some("a"), ALT),
                chr('a', '\0', 'a', A, true),
            ),
            (
                "Alt+Shift+a",
                press(ch("A"), KeyCode::KeyA, Some("A"), ALT | SHIFT),
                chr('a', '\0', 'a', or(S, A), true),
            ),
            // D13: Alt+1..9 select a tab (it was meta+digit before the tabs).
            (
                "Alt+1",
                press(ch("1"), KeyCode::Digit1, Some("1"), ALT),
                KeyRoute::Shortcut(AppCommand::SelectTab(0)),
            ),
            // Ctrl+Alt+letter on a US layout is a chord (R:59).
            (
                "Ctrl+Alt+a US",
                press(ch("a"), KeyCode::KeyA, Some("a"), CTRL | ALT),
                chr('a', '\0', 'a', or(A, C), true),
            ),
            (
                "Ctrl+Alt+a, control text",
                press(ch("a"), KeyCode::KeyA, Some("\u{1}"), CTRL | ALT),
                chr('a', '\0', 'a', or(A, C), true),
            ),
            // D5 deviation 2: Polish AltGr+a is the text "ą".
            (
                "Polish AltGr+a",
                press(ch("ą"), KeyCode::KeyA, Some("ą"), CTRL | ALT),
                text("ą"),
            ),
            // Ctrl+Alt+non-letter is AltGr composition, left to text (R:59).
            (
                "AltGr+7 (German {)",
                press(ch("{"), KeyCode::Digit7, Some("{"), CTRL | ALT),
                text("{"),
            ),
            // Punctuation by physical key, unshifted with the shifted twin.
            (
                "Ctrl+Space",
                press(named(NamedKey::Space), KeyCode::Space, Some(" "), CTRL),
                chr(' ', ' ', '\0', C, true),
            ),
            (
                "Ctrl+[",
                press(ch("["), KeyCode::BracketLeft, Some("\u{1b}"), CTRL),
                chr('[', '{', '\0', C, true),
            ),
            (
                "Ctrl+/",
                press(ch("/"), KeyCode::Slash, Some("/"), CTRL),
                chr('/', '?', '\0', C, true),
            ),
            (
                "Ctrl+Shift+2",
                press(ch("@"), KeyCode::Digit2, Some("@"), CTRL | SHIFT),
                chr('2', '@', '\0', or(S, C), true),
            ),
            (
                "Ctrl+\\",
                press(ch("\\"), KeyCode::Backslash, Some("\u{1c}"), CTRL),
                chr('\\', '|', '\0', C, true),
            ),
            (
                "Ctrl+`",
                press(ch("`"), KeyCode::Backquote, Some("`"), CTRL),
                chr('`', '~', '\0', C, true),
            ),
            // Functional keys always route (R:39-45).
            (
                "Enter",
                press(named(NamedKey::Enter), KeyCode::Enter, Some("\r"), NONE),
                func(TerminalKey::Enter, TerminalModifiers::NONE, false, true),
            ),
            (
                "Shift+Enter",
                press(named(NamedKey::Enter), KeyCode::Enter, Some("\r"), SHIFT),
                func(TerminalKey::Enter, S, false, true),
            ),
            (
                "Numpad Enter",
                numpad(press(
                    named(NamedKey::Enter),
                    KeyCode::NumpadEnter,
                    Some("\r"),
                    NONE,
                )),
                func(TerminalKey::Enter, TerminalModifiers::NONE, true, true),
            ),
            (
                "Tab",
                press(named(NamedKey::Tab), KeyCode::Tab, Some("\t"), NONE),
                func(TerminalKey::Tab, TerminalModifiers::NONE, false, true),
            ),
            (
                "Shift+Tab",
                press(named(NamedKey::Tab), KeyCode::Tab, None, SHIFT),
                func(TerminalKey::Tab, S, false, true),
            ),
            (
                "Backspace",
                press(
                    named(NamedKey::Backspace),
                    KeyCode::Backspace,
                    Some("\u{8}"),
                    NONE,
                ),
                func(TerminalKey::Backspace, TerminalModifiers::NONE, false, true),
            ),
            (
                "Escape",
                press(
                    named(NamedKey::Escape),
                    KeyCode::Escape,
                    Some("\u{1b}"),
                    NONE,
                ),
                func(TerminalKey::Escape, TerminalModifiers::NONE, false, true),
            ),
            (
                "F5",
                press(named(NamedKey::F5), KeyCode::F5, None, NONE),
                func(TerminalKey::F5, TerminalModifiers::NONE, false, true),
            ),
            (
                "Ctrl+F5",
                press(named(NamedKey::F5), KeyCode::F5, None, CTRL),
                func(TerminalKey::F5, C, false, true),
            ),
            (
                "Alt+Left",
                press(named(NamedKey::ArrowLeft), KeyCode::ArrowLeft, None, ALT),
                func(TerminalKey::Left, A, false, true),
            ),
            (
                "PageUp",
                press(named(NamedKey::PageUp), KeyCode::PageUp, None, NONE),
                func(TerminalKey::PageUp, TerminalModifiers::NONE, false, true),
            ),
            (
                "Delete",
                press(named(NamedKey::Delete), KeyCode::Delete, None, NONE),
                func(TerminalKey::Delete, TerminalModifiers::NONE, false, true),
            ),
            // Keypad navigation with NumLock off carries its position.
            (
                "Numpad Home (NumLock off)",
                numpad(press(named(NamedKey::Home), KeyCode::Numpad7, None, NONE)),
                func(TerminalKey::Home, TerminalModifiers::NONE, true, true),
            ),
            // The keypad's characters: text plain, a chord with Ctrl (R:143-148).
            (
                "Numpad 5",
                numpad(press(ch("5"), KeyCode::Numpad5, Some("5"), NONE)),
                text("5"),
            ),
            (
                "Ctrl+Numpad 5",
                numpad(press(ch("5"), KeyCode::Numpad5, Some("5"), CTRL)),
                KeyRoute::Terminal(TerminalKeyEvent {
                    key: TerminalKey::Character,
                    modifiers: C,
                    codepoint: u32::from('5'),
                    shifted_codepoint: 0,
                    base_layout_codepoint: 0,
                    is_keypad: true,
                    alt_is_meta: true,
                }),
            ),
            (
                "Alt+keypad +",
                numpad(press(ch("+"), KeyCode::NumpadAdd, Some("+"), ALT)),
                KeyRoute::Terminal(TerminalKeyEvent {
                    key: TerminalKey::Character,
                    modifiers: A,
                    codepoint: u32::from('+'),
                    shifted_codepoint: 0,
                    base_layout_codepoint: 0,
                    is_keypad: true,
                    alt_is_meta: true,
                }),
            ),
            // App shortcuts and what is not ours.
            (
                "Shift+PageUp",
                press(named(NamedKey::PageUp), KeyCode::PageUp, None, SHIFT),
                KeyRoute::Shortcut(AppCommand::Scroll(Scroll::Pages(-1))),
            ),
            (
                "Shift+PageDown",
                press(named(NamedKey::PageDown), KeyCode::PageDown, None, SHIFT),
                KeyRoute::Shortcut(AppCommand::Scroll(Scroll::Pages(1))),
            ),
            (
                "Shift+Home",
                press(named(NamedKey::Home), KeyCode::Home, None, SHIFT),
                KeyRoute::Shortcut(AppCommand::Scroll(Scroll::Top)),
            ),
            (
                "Shift+End",
                press(named(NamedKey::End), KeyCode::End, None, SHIFT),
                KeyRoute::Shortcut(AppCommand::Scroll(Scroll::Bottom)),
            ),
            (
                "Ctrl+Shift+V",
                press(ch("V"), KeyCode::KeyV, Some("\u{16}"), CTRL | SHIFT),
                KeyRoute::Shortcut(AppCommand::Paste),
            ),
            // Plain Ctrl+V is the shell's literal-next (TerminalView.cs:1807).
            (
                "Ctrl+V",
                press(ch("v"), KeyCode::KeyV, Some("\u{16}"), CTRL),
                chr('v', '\0', 'v', C, true),
            ),
            (
                "Win+a",
                press(ch("a"), KeyCode::KeyA, Some("a"), SUPER),
                KeyRoute::Ignore,
            ),
            (
                "Win+Enter",
                press(named(NamedKey::Enter), KeyCode::Enter, None, SUPER),
                KeyRoute::Ignore,
            ),
        ];
        assert!(cases.len() >= 40, "{} cases", cases.len());
        for (name, input, want) in cases {
            assert_eq!(route_key(&input, &win), want, "{name}");
        }
    }

    #[test]
    fn macos_keys() {
        let sc = AppShortcuts::standard(true);
        let off = ctx(true, false, &sc);
        let on = ctx(true, true, &sc);
        let a_option = || press(ch("å"), KeyCode::KeyA, Some("å"), ALT);
        // Option composes text unless the setting is on (D5 deviation 1: the window sets
        // `OptionAsAlt::Both`, so with it on winit hands over the plain "a").
        assert_eq!(route_key(&a_option(), &off), text("å"));
        let plain_a = press(ch("a"), KeyCode::KeyA, Some("a"), ALT);
        assert_eq!(
            route_key(&plain_a, &on),
            chr('a', '\0', 'a', A, true),
            "Option+a as meta"
        );
        // alt_is_meta is false on the events macOS routes with the setting off.
        let ctrl_a = press(ch("a"), KeyCode::KeyA, Some("\u{1}"), CTRL);
        assert_eq!(route_key(&ctrl_a, &off), chr('a', '\0', 'a', C, false));
        assert_eq!(route_key(&ctrl_a, &on), chr('a', '\0', 'a', C, true));
        // The unmodified key gives the letter when the logical key is a composed one.
        let mut composed = press(ch("å"), KeyCode::KeyA, Some("å"), ALT);
        composed.unmodified = Some(ch("a"));
        assert_eq!(route_key(&composed, &on), chr('a', '\0', 'a', A, true));
        // Ctrl+Option+letter is a chord on macOS (no AltGr there).
        let ctrl_opt = press(ch("a"), KeyCode::KeyA, Some("å"), CTRL | ALT);
        assert_eq!(
            route_key(&ctrl_opt, &off),
            chr('a', '\0', 'a', or(A, C), false)
        );
        // Option+digit composes text with the setting off, is meta with it on.
        let opt1 = press(ch("¡"), KeyCode::Digit1, Some("¡"), ALT);
        assert_eq!(route_key(&opt1, &off), text("¡"));
        let opt1_meta = press(ch("1"), KeyCode::Digit1, Some("1"), ALT);
        assert_eq!(route_key(&opt1_meta, &on), chr('1', '!', '\0', A, true));
        // Cmd+V pastes; Cmd+C and other Cmd chords are not the shell's in M9.
        let cmd_v = press(ch("v"), KeyCode::KeyV, Some("v"), SUPER);
        assert_eq!(
            route_key(&cmd_v, &off),
            KeyRoute::Shortcut(AppCommand::Paste)
        );
        let cmd_c = press(ch("c"), KeyCode::KeyC, Some("c"), SUPER);
        assert_eq!(route_key(&cmd_c, &off), KeyRoute::Ignore);
        let cmd_shift_v = press(ch("V"), KeyCode::KeyV, Some("V"), SUPER | SHIFT);
        assert_eq!(route_key(&cmd_shift_v, &off), KeyRoute::Ignore);
        // Ctrl+V on macOS is the shell's.
        let ctrl_v = press(ch("v"), KeyCode::KeyV, Some("\u{16}"), CTRL);
        assert_eq!(route_key(&ctrl_v, &off), chr('v', '\0', 'v', C, false));
        // Functional keys carry the meta flag of the setting.
        let up = press(named(NamedKey::ArrowUp), KeyCode::ArrowUp, None, NONE);
        assert_eq!(
            route_key(&up, &off),
            func(TerminalKey::Up, TerminalModifiers::NONE, false, false)
        );
        assert_eq!(
            route_key(&up, &on),
            func(TerminalKey::Up, TerminalModifiers::NONE, false, true)
        );
    }

    #[test]
    fn things_that_are_not_keystrokes() {
        let sc = AppShortcuts::standard(false);
        let c = ctx(false, false, &sc);
        let a = press(ch("a"), KeyCode::KeyA, Some("a"), NONE);
        let mut release = a.clone();
        release.pressed = false;
        assert_eq!(route_key(&release, &c), KeyRoute::Ignore, "release");
        let mut synthetic = a.clone();
        synthetic.synthetic = true;
        assert_eq!(route_key(&synthetic, &c), KeyRoute::Ignore, "synthetic");
        let process = press(named(NamedKey::Process), KeyCode::KeyA, None, NONE);
        assert_eq!(route_key(&process, &c), KeyRoute::Ignore, "Process");
        let dead = press(Key::Dead(Some('´')), KeyCode::Quote, None, NONE);
        assert_eq!(route_key(&dead, &c), KeyRoute::Ignore, "dead key");
        let mut ctrl_dead = dead.clone();
        ctrl_dead.mods = CTRL;
        assert_eq!(route_key(&ctrl_dead, &c), KeyRoute::Ignore, "Ctrl+dead key");
        // A modifier key alone, and a key with no mapping and no text.
        let shift = press(named(NamedKey::Shift), KeyCode::ShiftLeft, None, SHIFT);
        assert_eq!(route_key(&shift, &c), KeyRoute::Ignore, "Shift alone");
        let f13 = press(named(NamedKey::F13), KeyCode::F13, None, NONE);
        assert_eq!(route_key(&f13, &c), KeyRoute::Ignore, "F13 without text");
        let media = press(
            named(NamedKey::MediaPlayPause),
            KeyCode::MediaPlayPause,
            None,
            NONE,
        );
        assert_eq!(route_key(&media, &c), KeyRoute::Ignore, "media key");
        // macOS reports unmapped function keys as private-use text: never typed.
        let f13_text = press(named(NamedKey::F13), KeyCode::F13, Some("\u{f710}"), NONE);
        assert_eq!(
            route_key(&f13_text, &c),
            KeyRoute::Ignore,
            "F13 private-use text"
        );
        // A released paste chord, and a held one, are not pastes.
        let paste = press(ch("V"), KeyCode::KeyV, None, CTRL | SHIFT);
        let mut held = paste.clone();
        held.repeat = true;
        assert_eq!(route_key(&paste, &c), KeyRoute::Shortcut(AppCommand::Paste));
        assert_eq!(route_key(&held, &c), KeyRoute::Ignore, "held paste chord");
        // A held plain key repeats.
        let mut repeat = a.clone();
        repeat.repeat = true;
        assert_eq!(route_key(&repeat, &c), text("a"));
        // Text with a control character is not typed.
        let ctrl_text = press(ch("x"), KeyCode::KeyX, Some("\u{18}"), NONE);
        assert_eq!(route_key(&ctrl_text, &c), KeyRoute::Ignore);
    }

    #[test]
    fn layouts_and_positions() {
        let sc = AppShortcuts::standard(false);
        let c = ctx(false, false, &sc);
        // Cyrillic layout: Ctrl+С at the C position is Ctrl+C.
        let mut cyr = press(ch("с"), KeyCode::KeyC, Some("\u{3}"), CTRL);
        cyr.unmodified = Some(ch("с"));
        assert_eq!(route_key(&cyr, &c), chr('c', '\0', 'c', C, true));
        // Dvorak: the key at the S position types "o": Ctrl+O, with S as the base layout.
        let dvorak = press(ch("o"), KeyCode::KeyS, Some("\u{f}"), CTRL);
        assert_eq!(route_key(&dvorak, &c), chr('o', '\0', 's', C, true));
        // A letter with no position still routes by its layout key.
        let mut unpositioned = press(ch("a"), KeyCode::KeyA, Some("\u{1}"), CTRL);
        unpositioned.physical =
            PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        assert_eq!(route_key(&unpositioned, &c), chr('a', '\0', '\0', C, true));
        // A Space with no position.
        let mut space = press(named(NamedKey::Space), KeyCode::Space, Some(" "), CTRL);
        space.physical = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        assert_eq!(route_key(&space, &c), chr(' ', ' ', '\0', C, true));
        // Text from an unpositioned, unmapped key is still text.
        let mut odd = press(ch("€"), KeyCode::KeyE, Some("€"), NONE);
        odd.physical = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        assert_eq!(route_key(&odd, &c), text("€"));
        // Multi-character text (a compose result) is typed whole.
        let compose = press(ch("æ"), KeyCode::KeyA, Some("æ"), NONE);
        assert_eq!(route_key(&compose, &c), text("æ"));
    }

    #[test]
    fn modifiers_map_to_shift_alt_control_and_never_logo() {
        let all = ModifiersState::all();
        assert_eq!(terminal_mods(all), or(or(S, A), C));
        assert_eq!(terminal_mods(SUPER), TerminalModifiers::NONE);
        assert_eq!(terminal_mods(NONE), TerminalModifiers::NONE);
        assert_eq!(S.bits(), 1);
        assert_eq!(A.bits(), 2);
        assert_eq!(C.bits(), 4);
    }

    /// The US-QWERTY table, key by key: the unshifted and shifted character of each position.
    #[test]
    fn the_position_table_is_us_qwerty() {
        let sc = AppShortcuts::standard(false);
        let c = ctx(false, false, &sc);
        let table = [
            (KeyCode::Digit0, '0', ')'),
            (KeyCode::Digit1, '1', '!'),
            (KeyCode::Digit2, '2', '@'),
            (KeyCode::Digit3, '3', '#'),
            (KeyCode::Digit4, '4', '$'),
            (KeyCode::Digit5, '5', '%'),
            (KeyCode::Digit6, '6', '^'),
            (KeyCode::Digit7, '7', '&'),
            (KeyCode::Digit8, '8', '*'),
            (KeyCode::Digit9, '9', '('),
            (KeyCode::Minus, '-', '_'),
            (KeyCode::Equal, '=', '+'),
            (KeyCode::BracketLeft, '[', '{'),
            (KeyCode::BracketRight, ']', '}'),
            (KeyCode::Backslash, '\\', '|'),
            (KeyCode::Semicolon, ';', ':'),
            (KeyCode::Quote, '\'', '"'),
            (KeyCode::Backquote, '`', '~'),
            (KeyCode::Comma, ',', '<'),
            (KeyCode::Period, '.', '>'),
            (KeyCode::Slash, '/', '?'),
        ];
        for (code, plain, shifted) in table {
            // The layout key does not matter for a position key: use something unrelated.
            let k = press(ch("\u{2022}"), code, None, CTRL);
            assert_eq!(
                route_key(&k, &c),
                chr(plain, shifted, '\0', C, true),
                "{code:?}"
            );
        }
        for (code, want) in [
            (KeyCode::Numpad0, '0'),
            (KeyCode::Numpad9, '9'),
            (KeyCode::NumpadDecimal, '.'),
            (KeyCode::NumpadDivide, '/'),
            (KeyCode::NumpadMultiply, '*'),
            (KeyCode::NumpadSubtract, '-'),
            (KeyCode::NumpadAdd, '+'),
            (KeyCode::NumpadEqual, '='),
            (KeyCode::NumpadComma, ','),
        ] {
            let k = numpad(press(ch("\u{2022}"), code, None, CTRL));
            let KeyRoute::Terminal(ev) = route_key(&k, &c) else {
                panic!("{code:?} not routed");
            };
            assert_eq!(
                (ev.codepoint, ev.is_keypad),
                (u32::from(want), true),
                "{code:?}"
            );
        }
    }
}
