//! Input encoder exports (M8 plan step 4): key, char, mouse, focus and paste, each writing the
//! bytes to send to the PTY into the handle's input slot (D3). The slot is replaced by the next
//! encoder call; no other call touches it. `len == 0` (and a NULL pointer) means nothing to send:
//! lune-vt returned `None` (no protocol reports it, `disable_stdin`, an invalid code point...).
//!
//! Enums cross as `uint32_t` and are mapped by the explicit tables below, never cast or
//! transmuted; modifiers go through `TerminalModifiers::from_bits`, which keeps unknown bits and
//! ignores them, as lune-vt does. A bad enum is `ERR_INVALID_ARGUMENT` with the out-param empty and
//! the engine untouched.

use std::ptr::null;

use lune_vt::{
    Terminal, TerminalKey, TerminalKeyEvent, TerminalModifiers, TerminalMouseButton,
    TerminalMouseEvent, TerminalMouseEventType,
};

use crate::guard::{
    LUNETERM_ERR_INVALID_ARGUMENT, LUNETERM_ERR_INVALID_UTF8, LUNETERM_OK, LunetermEngine,
    bytes_in, out_init, ref_in, with_engine,
};
use crate::types::{LunetermBytes, LunetermKeyEvent, LunetermMouseEvent};

/// Lunegit's `TerminalKey` ordinals (`TerminalInput.cs`), spelled out.
fn key_from(ordinal: u32) -> Option<TerminalKey> {
    Some(match ordinal {
        0 => TerminalKey::Enter,
        1 => TerminalKey::Tab,
        2 => TerminalKey::Backspace,
        3 => TerminalKey::Escape,
        4 => TerminalKey::Space,
        5 => TerminalKey::Up,
        6 => TerminalKey::Down,
        7 => TerminalKey::Right,
        8 => TerminalKey::Left,
        9 => TerminalKey::Home,
        10 => TerminalKey::End,
        11 => TerminalKey::PageUp,
        12 => TerminalKey::PageDown,
        13 => TerminalKey::Insert,
        14 => TerminalKey::Delete,
        15 => TerminalKey::F1,
        16 => TerminalKey::F2,
        17 => TerminalKey::F3,
        18 => TerminalKey::F4,
        19 => TerminalKey::F5,
        20 => TerminalKey::F6,
        21 => TerminalKey::F7,
        22 => TerminalKey::F8,
        23 => TerminalKey::F9,
        24 => TerminalKey::F10,
        25 => TerminalKey::F11,
        26 => TerminalKey::F12,
        27 => TerminalKey::Character,
        _ => return None,
    })
}

/// Lunegit's `TerminalMouseButton` ordinals.
fn button_from(ordinal: u32) -> Option<TerminalMouseButton> {
    Some(match ordinal {
        0 => TerminalMouseButton::None,
        1 => TerminalMouseButton::Left,
        2 => TerminalMouseButton::Middle,
        3 => TerminalMouseButton::Right,
        4 => TerminalMouseButton::WheelUp,
        5 => TerminalMouseButton::WheelDown,
        _ => return None,
    })
}

/// Lunegit's `TerminalMouseEventType` ordinals.
fn mouse_kind_from(ordinal: u32) -> Option<TerminalMouseEventType> {
    Some(match ordinal {
        0 => TerminalMouseEventType::Down,
        1 => TerminalMouseEventType::Up,
        2 => TerminalMouseEventType::Move,
        3 => TerminalMouseEventType::Drag,
        4 => TerminalMouseEventType::WheelUp,
        5 => TerminalMouseEventType::WheelDown,
        _ => return None,
    })
}

/// Runs an encoder under the guard and publishes its bytes in the input slot. `None` from
/// lune-vt is OK with `len == 0`. `*out` is written only on `OK`.
///
/// # Safety
///
/// `engine` as for every handle export.
unsafe fn encode(
    engine: *mut LunetermEngine,
    out: &mut LunetermBytes,
    produce: impl FnOnce(&mut Terminal) -> Option<Vec<u8>>,
) -> i32 {
    // SAFETY: forwarded from the caller's contract on `engine`.
    let result = unsafe {
        with_engine(engine, |inner| {
            inner.input = produce(&mut inner.term).unwrap_or_default();
            let slot = &inner.input;
            Ok(LunetermBytes {
                ptr: if slot.is_empty() {
                    null()
                } else {
                    slot.as_ptr()
                },
                len: slot.len() as u64,
            })
        })
    };
    match result {
        Ok(bytes) => {
            *out = bytes;
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

/// Encodes a keystroke under the live keyboard protocol state (kitty, modifyOtherKeys, DECCKM).
/// `event->key` outside 0..=27 is `ERR_INVALID_ARGUMENT`. `*out` is zeroed first; its pointer
/// stays valid until the next encoder call on this handle (or `luneterm_engine_free`).
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `event` is NULL, misaligned, or readable;
/// `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_key_event(
    engine: *mut LunetermEngine,
    event: *const LunetermKeyEvent,
    out: *mut LunetermBytes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermBytes::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `event`.
    let event = match unsafe { ref_in(event) } {
        Ok(event) => event,
        Err(code) => return code,
    };
    let LunetermKeyEvent {
        key,
        codepoint,
        shifted_codepoint,
        base_layout_codepoint,
        modifiers,
        is_keypad,
        alt_is_meta,
        reserved: _,
    } = event;
    let Some(key) = key_from(key) else {
        return LUNETERM_ERR_INVALID_ARGUMENT;
    };
    let event = TerminalKeyEvent {
        key,
        modifiers: TerminalModifiers::from_bits(modifiers),
        codepoint,
        shifted_codepoint,
        base_layout_codepoint,
        is_keypad: is_keypad != 0,
        alt_is_meta: alt_is_meta != 0,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    unsafe { encode(engine, out, |term| term.key_event(&event)) }
}

/// Legacy bytes of a functional key with the live DECCKM state, whatever the keyboard protocol.
/// `key` outside 0..=27 is `ERR_INVALID_ARGUMENT`. Output as for `luneterm_engine_key_event`.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_key_input(
    engine: *mut LunetermEngine,
    key: u32,
    modifiers: u8,
    out: *mut LunetermBytes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermBytes::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    let Some(key) = key_from(key) else {
        return LUNETERM_ERR_INVALID_ARGUMENT;
    };
    let modifiers = TerminalModifiers::from_bits(modifiers);
    // SAFETY: forwarded from this function's contract on `engine`.
    unsafe { encode(engine, out, |term| term.key_input(key, modifiers)) }
}

/// Legacy bytes of a character (Ctrl folds to its C0 byte, Alt prefixes ESC). A `codepoint` that
/// is not a Unicode scalar value (a surrogate, above `0x10FFFF`) is OK with `len == 0`.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_char_input(
    engine: *mut LunetermEngine,
    codepoint: u32,
    modifiers: u8,
    out: *mut LunetermBytes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermBytes::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    let character = char::from_u32(codepoint);
    let modifiers = TerminalModifiers::from_bits(modifiers);
    // SAFETY: forwarded from this function's contract on `engine`.
    unsafe {
        encode(engine, out, |term| {
            character.and_then(|c| term.char_input(c, modifiers))
        })
    }
}

/// Encodes a pointer event from the engine's own mouse protocol and encoding state. `button` or
/// `kind` outside 0..=5 is `ERR_INVALID_ARGUMENT`. The default encoding's bytes are raw (a
/// coordinate above 95 is one byte above 0x7F, not UTF-8).
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `event` is NULL, misaligned, or readable;
/// `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_mouse_event(
    engine: *mut LunetermEngine,
    event: *const LunetermMouseEvent,
    out: *mut LunetermBytes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermBytes::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `event`.
    let event = match unsafe { ref_in(event) } {
        Ok(event) => event,
        Err(code) => return code,
    };
    let LunetermMouseEvent {
        button,
        kind,
        col,
        row,
        x,
        y,
        modifiers,
        reserved: _,
    } = event;
    let (Some(button), Some(kind)) = (button_from(button), mouse_kind_from(kind)) else {
        return LUNETERM_ERR_INVALID_ARGUMENT;
    };
    let event = TerminalMouseEvent {
        button,
        kind,
        col,
        row,
        x,
        y,
        modifiers: TerminalModifiers::from_bits(modifiers),
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    unsafe { encode(engine, out, |term| term.mouse_event(&event)) }
}

/// The focus report (`ESC [ I` / `ESC [ O`), only while the application has focus reporting on;
/// `focused` nonzero = gained.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_focus_event(
    engine: *mut LunetermEngine,
    focused: u8,
    out: *mut LunetermBytes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermBytes::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    unsafe { encode(engine, out, |term| term.focus_event(focused != 0)) }
}

/// The bytes to write for pasting `text` (UTF-8): line breaks as `\r`, and with bracketed paste
/// on the sanitized text between `ESC [ 200 ~` and `ESC [ 201 ~`. The sanitizing is the engine's;
/// the host cannot bypass it. Invalid UTF-8 is `ERR_INVALID_UTF8` and nothing is produced (no
/// lossy replacement). Empty text is OK with `len == 0`.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `text` is readable for `len` bytes when
/// `len > 0`; `out` is NULL, misaligned, or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_paste(
    engine: *mut LunetermEngine,
    text: *const u8,
    len: usize,
    out: *mut LunetermBytes,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, LunetermBytes::EMPTY) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `text`.
    let text = match unsafe { bytes_in(text, len) } {
        Ok(text) => text,
        Err(code) => return code,
    };
    let Ok(text) = std::str::from_utf8(text) else {
        return LUNETERM_ERR_INVALID_UTF8;
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    unsafe { encode(engine, out, |term| term.paste(text)) }
}

#[cfg(test)]
mod tests {
    //! The encoder exports against the same `Terminal` methods on a twin terminal in the same
    //! state (every state set by writing its DECSET/CSI sequence to both), plus known answers with
    //! literal ordinals and bytes, so a wrong table fails even where twin and export agree.
    //!
    //! Axes: all 28 keys x modifier values 0..=7, 0x08, 0xFF x {no protocol, kitty 1, kitty 1|4,
    //! modifyOtherKeys 1 and 2} x DECCKM on and off; characters incl. non-BMP, a surrogate and
    //! 0x110000; mouse button x kind x tracking mode x encoding x positions at the 223/224 edge
    //! and -1 x pixels x modifiers; focus with 1004 on and off; paste bracketed on and off with
    //! embedded end markers, C0/C1 controls and CRLF; every bad enum.

    use std::ptr::{null, null_mut};

    use lune_vt::services::options_service::OptionValue;
    use lune_vt::{StandardExtensions, Terminal};

    use super::*;
    use crate::guard::{LUNETERM_ERR_NULL_POINTER, LunetermEngine};
    use crate::{
        LunetermConfig, LunetermDamage, LunetermEvents, LunetermModes, LunetermViewport,
        luneterm_engine_drain_events, luneterm_engine_free, luneterm_engine_modes,
        luneterm_engine_new, luneterm_engine_resize, luneterm_engine_take_damage,
        luneterm_engine_viewport, luneterm_engine_write,
    };

    /// An FFI engine and a reference terminal built the same way.
    struct Twin {
        engine: *mut LunetermEngine,
        reference: Terminal,
    }

    impl Twin {
        fn new(cols: u32, rows: u32) -> Twin {
            let config = LunetermConfig {
                cols,
                rows,
                scrollback: 100,
                unicode: 0,
                xtversion: null(),
                xtversion_len: 0,
                clipboard_write: 0,
                reserved: [0; 7],
            };
            let mut engine = null_mut();
            // SAFETY: a live config and out-param.
            assert_eq!(
                unsafe { luneterm_engine_new(&config, &mut engine) },
                LUNETERM_OK
            );
            let mut reference = Terminal::new([
                OptionValue::Cols(cols as usize),
                OptionValue::Rows(rows as usize),
                OptionValue::Scrollback(100),
            ]);
            reference
                .install_standard_extensions(&StandardExtensions {
                    xtversion: None,
                    vs16: false,
                    clipboard_write: false,
                })
                .expect("extensions");
            Twin { engine, reference }
        }

        /// A twin with `setup` written to both.
        fn with(cols: u32, rows: u32, setup: &str) -> Twin {
            let mut twin = Twin::new(cols, rows);
            twin.write(setup.as_bytes());
            twin
        }

        fn write(&mut self, bytes: &[u8]) {
            // SAFETY: a live handle and a live slice.
            let code = unsafe { luneterm_engine_write(self.engine, bytes.as_ptr(), bytes.len()) };
            assert_eq!(code, LUNETERM_OK);
            self.reference.write(bytes);
        }

        /// The modes both sides agree on (so the setup really took effect on both).
        fn modes_agree(&mut self) -> lune_vt::Modes {
            let mut out = LunetermModes::default();
            // SAFETY: a live handle and out-param.
            assert_eq!(
                unsafe { luneterm_engine_modes(self.engine, &mut out) },
                LUNETERM_OK
            );
            let expected = self.reference.modes();
            assert_eq!(out.kitty_keyboard_flags, expected.kitty_keyboard_flags);
            assert_eq!(out.modify_other_keys, expected.modify_other_keys);
            assert_eq!(
                out.application_cursor_keys,
                u8::from(expected.application_cursor_keys)
            );
            expected
        }
    }

    impl Drop for Twin {
        fn drop(&mut self) {
            // SAFETY: a live handle, used for the last time.
            assert_eq!(unsafe { luneterm_engine_free(self.engine) }, LUNETERM_OK);
        }
    }

    /// The bytes behind an out-param: NULL exactly when empty.
    fn read(out: &LunetermBytes) -> Vec<u8> {
        if out.len == 0 {
            assert!(out.ptr.is_null(), "empty input bytes must be NULL");
            return Vec::new();
        }
        assert!(!out.ptr.is_null());
        // SAFETY: `ptr`/`len` come from the input slot, which the test does not refill while it
        // reads.
        unsafe { std::slice::from_raw_parts(out.ptr, out.len as usize) }.to_vec()
    }

    fn garbage() -> LunetermBytes {
        LunetermBytes {
            ptr: std::ptr::dangling(),
            len: 9,
        }
    }

    fn key_event(engine: *mut LunetermEngine, event: &LunetermKeyEvent) -> (i32, Vec<u8>) {
        let mut out = garbage();
        // SAFETY: a live handle, event and out-param.
        let code = unsafe { luneterm_engine_key_event(engine, event, &mut out) };
        (code, read(&out))
    }

    fn key_input(engine: *mut LunetermEngine, key: u32, modifiers: u8) -> (i32, Vec<u8>) {
        let mut out = garbage();
        // SAFETY: a live handle and out-param.
        let code = unsafe { luneterm_engine_key_input(engine, key, modifiers, &mut out) };
        (code, read(&out))
    }

    fn char_input(engine: *mut LunetermEngine, codepoint: u32, modifiers: u8) -> (i32, Vec<u8>) {
        let mut out = garbage();
        // SAFETY: a live handle and out-param.
        let code = unsafe { luneterm_engine_char_input(engine, codepoint, modifiers, &mut out) };
        (code, read(&out))
    }

    fn mouse_event(engine: *mut LunetermEngine, event: &LunetermMouseEvent) -> (i32, Vec<u8>) {
        let mut out = garbage();
        // SAFETY: a live handle, event and out-param.
        let code = unsafe { luneterm_engine_mouse_event(engine, event, &mut out) };
        (code, read(&out))
    }

    fn focus_event(engine: *mut LunetermEngine, focused: u8) -> (i32, Vec<u8>) {
        let mut out = garbage();
        // SAFETY: a live handle and out-param.
        let code = unsafe { luneterm_engine_focus_event(engine, focused, &mut out) };
        (code, read(&out))
    }

    fn paste(engine: *mut LunetermEngine, text: &[u8]) -> (i32, Vec<u8>) {
        let mut out = garbage();
        // SAFETY: a live handle, a live slice and out-param.
        let code = unsafe { luneterm_engine_paste(engine, text.as_ptr(), text.len(), &mut out) };
        (code, read(&out))
    }

    const MODIFIERS: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 0x08, 0xFF];

    /// Protocol states x DECCKM, as the sequences that set them.
    fn key_states() -> Vec<(String, String)> {
        let mut states = Vec::new();
        for (name, protocol) in [
            ("none", ""),
            ("kitty1", "\x1b[>1u"),
            ("kitty1|4", "\x1b[>5u"),
            ("mok1", "\x1b[>4;1m"),
            ("mok2", "\x1b[>4;2m"),
        ] {
            for (decckm_name, decckm) in [("", ""), ("+decckm", "\x1b[?1h")] {
                states.push((
                    format!("{name}{decckm_name}"),
                    format!("{protocol}{decckm}"),
                ));
            }
        }
        states
    }

    fn key_event_for(
        key: usize,
        modifiers: u8,
        (codepoint, shifted, base): (u32, u32, u32),
        keypad: bool,
        meta: bool,
    ) -> (LunetermKeyEvent, TerminalKeyEvent) {
        (
            LunetermKeyEvent {
                key: key as u32,
                codepoint,
                shifted_codepoint: shifted,
                base_layout_codepoint: base,
                modifiers,
                is_keypad: u8::from(keypad),
                alt_is_meta: u8::from(meta),
                reserved: 0,
            },
            TerminalKeyEvent {
                key: TerminalKey::ALL[key],
                modifiers: TerminalModifiers::from_bits(modifiers),
                codepoint,
                shifted_codepoint: shifted,
                base_layout_codepoint: base,
                is_keypad: keypad,
                alt_is_meta: meta,
            },
        )
    }

    #[test]
    fn key_event_matches_the_terminal_for_every_key_modifier_and_protocol_state() {
        let mut nonempty = 0usize;
        let mut distinct_outputs = std::collections::BTreeSet::new();
        for (name, setup) in key_states() {
            let mut twin = Twin::with(80, 24, &setup);
            if name.starts_with("kitty1|4") {
                assert_eq!(twin.modes_agree().kitty_keyboard_flags, 5, "{name}");
            }
            if name.starts_with("mok2") {
                assert_eq!(twin.modes_agree().modify_other_keys, 2, "{name}");
            }
            for key in 0..28usize {
                // (codepoint, shifted, base) variants; functional keys ignore them.
                let characters: &[(u32, u32, u32)] = if key == 27 {
                    &[
                        (0x61, 0, 0),
                        (0x61, 0x41, 0x61),
                        (0x5A, 0, 0),
                        (0x31, 0x21, 0x31),
                        (0x20, 0, 0),
                        (0xE9, 0, 0x65),
                        (0x1F600, 0, 0),
                        (0xD800, 0, 0),
                        (0x110000, 0, 0),
                        (0, 0, 0),
                        (u32::MAX, 0, 0),
                    ]
                } else {
                    &[(0, 0, 0)]
                };
                for &chars in characters {
                    for (keypad, meta) in [(false, true), (false, false), (true, true)] {
                        for modifiers in MODIFIERS {
                            let (c_event, event) =
                                key_event_for(key, modifiers, chars, keypad, meta);
                            let expected = twin.reference.key_event(&event).unwrap_or_default();
                            let (code, got) = key_event(twin.engine, &c_event);
                            assert_eq!(code, LUNETERM_OK);
                            assert_eq!(
                                got, expected,
                                "{name} key {key} mods {modifiers:#x} {chars:?}"
                            );
                            if !got.is_empty() {
                                nonempty += 1;
                                distinct_outputs.insert(got);
                            }
                        }
                    }
                }
            }
        }
        assert!(nonempty > 5000, "the matrix produced output ({nonempty})");
        assert!(
            distinct_outputs.len() > 300,
            "and varied output ({})",
            distinct_outputs.len()
        );
    }

    #[test]
    fn key_input_and_char_input_match_the_terminal() {
        for (name, setup) in key_states() {
            let mut twin = Twin::with(80, 24, &setup);
            for key in 0..28usize {
                for modifiers in MODIFIERS {
                    let expected = twin
                        .reference
                        .key_input(
                            TerminalKey::ALL[key],
                            TerminalModifiers::from_bits(modifiers),
                        )
                        .unwrap_or_default();
                    let (code, got) = key_input(twin.engine, key as u32, modifiers);
                    assert_eq!(
                        (code, got),
                        (LUNETERM_OK, expected),
                        "{name} key {key} mods {modifiers:#x}"
                    );
                }
            }
            for codepoint in [
                0x61,
                0x5A,
                0x20,
                0x5B,
                0x40,
                0x7F,
                0xE9,
                0x1F600,
                0,
                0xD800,
                0xDFFF,
                0x110000,
                u32::MAX,
            ] {
                for modifiers in MODIFIERS {
                    let expected = char::from_u32(codepoint)
                        .and_then(|c| {
                            twin.reference
                                .char_input(c, TerminalModifiers::from_bits(modifiers))
                        })
                        .unwrap_or_default();
                    let (code, got) = char_input(twin.engine, codepoint, modifiers);
                    assert_eq!(
                        (code, got),
                        (LUNETERM_OK, expected),
                        "{name} char {codepoint:#x} mods {modifiers:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn mouse_event_matches_the_terminal_for_every_button_kind_mode_and_encoding() {
        let mut nonempty = 0usize;
        let mut raw_bytes = false;
        for (tracking_name, tracking) in [
            ("off", ""),
            ("x10", "\x1b[?9h"),
            ("vt200", "\x1b[?1000h"),
            ("drag", "\x1b[?1002h"),
            ("any", "\x1b[?1003h"),
        ] {
            for (encoding_name, encoding) in [
                ("default", ""),
                ("sgr", "\x1b[?1006h"),
                ("sgr-pixels", "\x1b[?1016h"),
            ] {
                let mut twin = Twin::with(300, 300, &format!("{tracking}{encoding}"));
                let modes = twin.reference.modes();
                assert_eq!(
                    modes.mouse_tracking == lune_vt::MouseTracking::None,
                    tracking.is_empty()
                );
                for button in 0..6u32 {
                    for kind in 0..6u32 {
                        for (col, row) in [
                            (0, 0),
                            (222, 222),
                            (223, 223),
                            (224, 224),
                            (-1, -1),
                            (0, 224),
                            (224, 0),
                            (5, 3),
                        ] {
                            for modifiers in [0u8, 7, 0x38] {
                                let (x, y) = (col * 7 + 3, row * 13 + 2);
                                let c_event = LunetermMouseEvent {
                                    button,
                                    kind,
                                    col,
                                    row,
                                    x,
                                    y,
                                    modifiers,
                                    reserved: [0; 3],
                                };
                                let event = TerminalMouseEvent {
                                    button: [
                                        TerminalMouseButton::None,
                                        TerminalMouseButton::Left,
                                        TerminalMouseButton::Middle,
                                        TerminalMouseButton::Right,
                                        TerminalMouseButton::WheelUp,
                                        TerminalMouseButton::WheelDown,
                                    ][button as usize],
                                    kind: [
                                        TerminalMouseEventType::Down,
                                        TerminalMouseEventType::Up,
                                        TerminalMouseEventType::Move,
                                        TerminalMouseEventType::Drag,
                                        TerminalMouseEventType::WheelUp,
                                        TerminalMouseEventType::WheelDown,
                                    ][kind as usize],
                                    col,
                                    row,
                                    x,
                                    y,
                                    modifiers: TerminalModifiers::from_bits(modifiers),
                                };
                                let expected =
                                    twin.reference.mouse_event(&event).unwrap_or_default();
                                let (code, got) = mouse_event(twin.engine, &c_event);
                                assert_eq!(
                                    (code, &got),
                                    (LUNETERM_OK, &expected),
                                    "{tracking_name}/{encoding_name} button {button} kind {kind} at ({col},{row}) mods {modifiers:#x}"
                                );
                                nonempty += usize::from(!got.is_empty());
                                raw_bytes |= got.iter().any(|b| *b >= 0x80);
                            }
                        }
                    }
                }
            }
        }
        assert!(
            nonempty > 1000,
            "mouse matrix produced reports ({nonempty})"
        );
        assert!(
            raw_bytes,
            "the default encoding's raw bytes above 0x7F were exercised"
        );
    }

    #[test]
    fn focus_event_matches_the_terminal_with_reporting_on_and_off() {
        for setup in ["", "\x1b[?1004h", "\x1b[?1004h\x1b[?1004l"] {
            let mut twin = Twin::with(80, 24, setup);
            for focused in [0u8, 1, 2, 255] {
                let expected = twin.reference.focus_event(focused != 0).unwrap_or_default();
                assert_eq!(
                    focus_event(twin.engine, focused),
                    (LUNETERM_OK, expected),
                    "{setup:?} {focused}"
                );
            }
        }
    }

    #[test]
    fn paste_matches_the_terminal_bracketed_and_not() {
        let big = "ab\n".repeat(300_000);
        let texts: [&str; 11] = [
            "plain",
            "line one\r\nline two\nline three\rend",
            "x\x1b[201~y",
            "\x1b[200~inner\x1b[201~",
            "c0 \x01\x02\x03\x07\x08\x1b[31m\x7f end",
            "c1 \u{85}\u{9b}31m end",
            "emoji \u{1F600} \u{e9} \u{3042}",
            "\n",
            "\r\n\r\n",
            "",
            &big,
        ];
        for setup in ["", "\x1b[?2004h", "\x1b[?2004h\x1b[?2004l"] {
            let mut twin = Twin::with(80, 24, setup);
            for text in texts {
                let expected = twin.reference.paste(text).unwrap_or_default();
                let (code, got) = paste(twin.engine, text.as_bytes());
                assert_eq!(
                    code,
                    LUNETERM_OK,
                    "{setup:?} {:?}",
                    &text[..text.len().min(20)]
                );
                assert!(
                    got == expected,
                    "{setup:?} {:?}",
                    &text[..text.len().min(20)]
                );
            }
        }
        // The engine's sanitizing cannot be bypassed: one end marker, at the end.
        let twin = Twin::with(80, 24, "\x1b[?2004h");
        let (_, got) = paste(twin.engine, b"x\x1b[201~y\x1b[200~z");
        let got = String::from_utf8(got).expect("utf-8");
        assert!(got.starts_with("\x1b[200~") && got.ends_with("\x1b[201~"));
        assert_eq!(got.matches("\x1b[201~").count(), 1, "{got:?}");
        assert_eq!(got.matches("\x1b[200~").count(), 1, "{got:?}");
    }

    #[test]
    fn paste_input_edge_cases() {
        let mut twin = Twin::with(80, 24, "\x1b[?2004h");
        // Invalid UTF-8: refused, nothing produced, no lossy replacement, the engine untouched.
        for bad in [
            &b"\xff"[..],
            b"\xe3\x81",
            b"\xed\xa0\x80",
            b"ok\xc0\xaf",
            b"\xf4\x90\x80\x80",
        ] {
            assert_eq!(
                paste(twin.engine, bad),
                (LUNETERM_ERR_INVALID_UTF8, Vec::new()),
                "{bad:?}"
            );
        }
        let expected = twin.reference.paste("after").unwrap_or_default();
        assert_eq!(paste(twin.engine, b"after"), (LUNETERM_OK, expected));
        // (NULL, 0) is empty text; (NULL, n) is refused; a huge length is refused.
        let mut out = garbage();
        // SAFETY: a live handle and out-param; the NULL/huge text is refused before any read.
        unsafe {
            assert_eq!(
                luneterm_engine_paste(twin.engine, null(), 0, &mut out),
                LUNETERM_OK
            );
            assert_eq!((out.len, out.ptr), (0, null()));
            assert_eq!(
                luneterm_engine_paste(twin.engine, null(), 1, &mut out),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(out.len, 0);
            let huge = (isize::MAX as usize) + 1;
            assert_eq!(
                luneterm_engine_paste(twin.engine, b"x".as_ptr(), huge, &mut out),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
        }
        // A zero-length non-NULL text is empty too.
        assert_eq!(paste(twin.engine, b""), (LUNETERM_OK, Vec::new()));
    }

    #[test]
    fn out_of_range_enums_are_invalid_arguments_with_an_empty_output() {
        let twin = Twin::new(80, 24);
        // A previous result in the slot must survive a refused call.
        let (_, enter) = key_input(twin.engine, 0, 0);
        assert_eq!(enter, b"\r");
        let mut kept = garbage();
        // SAFETY: a live handle and out-param.
        assert_eq!(
            unsafe { luneterm_engine_key_input(twin.engine, 0, 0, &mut kept) },
            LUNETERM_OK
        );
        for key in [28u32, 29, 1000, u32::MAX] {
            let event = LunetermKeyEvent {
                key,
                ..LunetermKeyEvent::default()
            };
            assert_eq!(
                key_event(twin.engine, &event),
                (LUNETERM_ERR_INVALID_ARGUMENT, Vec::new()),
                "key {key}"
            );
            assert_eq!(
                key_input(twin.engine, key, 0),
                (LUNETERM_ERR_INVALID_ARGUMENT, Vec::new()),
                "key_input {key}"
            );
        }
        for value in [6u32, 7, 1000, u32::MAX] {
            for (button, kind) in [(value, 0), (0, value), (value, value)] {
                let event = LunetermMouseEvent {
                    button,
                    kind,
                    ..LunetermMouseEvent::default()
                };
                assert_eq!(
                    mouse_event(twin.engine, &event),
                    (LUNETERM_ERR_INVALID_ARGUMENT, Vec::new()),
                    "button {button} kind {kind}"
                );
            }
        }
        assert_eq!(read(&kept), b"\r", "a refused call leaves the slot alone");
        // Still usable.
        assert_eq!(key_input(twin.engine, 0, 0), (LUNETERM_OK, b"\r".to_vec()));
    }

    #[test]
    fn known_answers_with_literal_ordinals_and_bytes() {
        let mut twin = Twin::new(80, 24);
        let key = |twin: &Twin, key: u32, modifiers: u8| key_input(twin.engine, key, modifiers).1;
        // Lunegit ordinals: Enter 0, Tab 1, Backspace 2, Escape 3, Space 4, Up 5 ... F1 15, F12 26.
        assert_eq!(key(&twin, 0, 0), b"\r");
        assert_eq!(key(&twin, 1, 0), b"\t");
        assert_eq!(key(&twin, 2, 0), b"\x7f");
        assert_eq!(key(&twin, 3, 0), b"\x1b");
        assert_eq!(key(&twin, 4, 0), b" ");
        assert_eq!(key(&twin, 5, 0), b"\x1b[A");
        assert_eq!(key(&twin, 6, 0), b"\x1b[B");
        assert_eq!(key(&twin, 7, 0), b"\x1b[C");
        assert_eq!(key(&twin, 8, 0), b"\x1b[D");
        assert_eq!(key(&twin, 15, 0), b"\x1bOP");
        assert_eq!(key(&twin, 26, 0), b"\x1b[24~");
        assert_eq!(key(&twin, 5, 1), b"\x1b[1;2A", "Shift is bit 1");
        assert_eq!(key(&twin, 5, 4), b"\x1b[1;5A", "Control is bit 4");
        assert_eq!(key(&twin, 5, 2), b"\x1b[1;3A", "Alt is bit 2");
        twin.write(b"\x1b[?1h");
        assert_eq!(key(&twin, 5, 0), b"\x1bOA", "DECCKM");
        // key_event: Enter, and a character under kitty flags 1.
        let enter = LunetermKeyEvent {
            key: 0,
            alt_is_meta: 1,
            ..LunetermKeyEvent::default()
        };
        assert_eq!(
            key_event(twin.engine, &enter),
            (LUNETERM_OK, b"\r".to_vec())
        );
        let a = LunetermKeyEvent {
            key: 27,
            codepoint: 0x61,
            shifted_codepoint: 0x61,
            alt_is_meta: 1,
            ..LunetermKeyEvent::default()
        };
        assert_eq!(key_event(twin.engine, &a), (LUNETERM_OK, b"a".to_vec()));
        twin.write(b"\x1b[>1u");
        let ctrl_a = LunetermKeyEvent { modifiers: 4, ..a };
        assert_eq!(
            key_event(twin.engine, &ctrl_a),
            (LUNETERM_OK, b"\x1b[97;5u".to_vec())
        );
        // char_input: Ctrl folds to C0, Alt prefixes ESC, non-scalars send nothing.
        assert_eq!(
            char_input(twin.engine, 0x61, 4),
            (LUNETERM_OK, b"\x01".to_vec())
        );
        assert_eq!(
            char_input(twin.engine, 0x61, 2),
            (LUNETERM_OK, b"\x1ba".to_vec())
        );
        assert_eq!(
            char_input(twin.engine, 0xE9, 0),
            (LUNETERM_OK, "\u{e9}".as_bytes().to_vec())
        );
        assert_eq!(
            char_input(twin.engine, 0xD800, 0),
            (LUNETERM_OK, Vec::new())
        );
        assert_eq!(
            char_input(twin.engine, 0x110000, 0),
            (LUNETERM_OK, Vec::new())
        );
        // Mouse: Left 1, Down 0 / Up 1, WheelUp 4 / kind 4, at (5, 3) under VT200 + SGR.
        let mut twin = Twin::with(80, 24, "\x1b[?1000h\x1b[?1006h");
        let at = |button, kind| LunetermMouseEvent {
            button,
            kind,
            col: 5,
            row: 3,
            ..LunetermMouseEvent::default()
        };
        assert_eq!(mouse_event(twin.engine, &at(1, 0)).1, b"\x1b[<0;6;4M");
        assert_eq!(mouse_event(twin.engine, &at(1, 1)).1, b"\x1b[<0;6;4m");
        assert_eq!(mouse_event(twin.engine, &at(3, 0)).1, b"\x1b[<2;6;4M");
        assert_eq!(mouse_event(twin.engine, &at(0, 4)).1, b"\x1b[<64;6;4M");
        assert_eq!(mouse_event(twin.engine, &at(0, 5)).1, b"\x1b[<65;6;4M");
        // Down with no button is nonsense: nothing sent, OK.
        assert_eq!(
            mouse_event(twin.engine, &at(0, 0)),
            (LUNETERM_OK, Vec::new())
        );
        // Focus.
        assert_eq!(
            focus_event(twin.engine, 1),
            (LUNETERM_OK, Vec::new()),
            "reporting is off"
        );
        twin.write(b"\x1b[?1004h");
        assert_eq!(focus_event(twin.engine, 1).1, b"\x1b[I");
        assert_eq!(focus_event(twin.engine, 0).1, b"\x1b[O");
        assert_eq!(
            focus_event(twin.engine, 7).1,
            b"\x1b[I",
            "any nonzero byte is true"
        );
        // Paste.
        assert_eq!(paste(twin.engine, b"hi\nthere").1, b"hi\rthere");
        twin.write(b"\x1b[?2004h");
        assert_eq!(
            paste(twin.engine, b"hi\nthere").1,
            b"\x1b[200~hi\rthere\x1b[201~"
        );
    }

    #[test]
    fn the_input_slot_is_untouched_by_other_calls_and_replaced_by_the_next_encoder() {
        let mut twin = Twin::new(20, 5);
        let mut out = LunetermBytes::EMPTY;
        // SAFETY: a live handle and out-param.
        assert_eq!(
            unsafe { luneterm_engine_key_input(twin.engine, 5, 0, &mut out) },
            LUNETERM_OK
        );
        assert_eq!(read(&out), b"\x1b[A");
        twin.write(b"hello\x1b[5n\x1b]2;t\x07");
        let (mut viewport, mut events, mut damage) = (
            LunetermViewport::EMPTY,
            LunetermEvents::EMPTY,
            LunetermDamage::default(),
        );
        // SAFETY: a live handle and out-params.
        unsafe {
            assert_eq!(
                luneterm_engine_viewport(twin.engine, &mut viewport),
                LUNETERM_OK
            );
            assert_eq!(
                luneterm_engine_drain_events(twin.engine, &mut events),
                LUNETERM_OK
            );
            assert_eq!(
                luneterm_engine_take_damage(twin.engine, &mut damage),
                LUNETERM_OK
            );
            assert_eq!(luneterm_engine_resize(twin.engine, 30, 8), LUNETERM_OK);
        }
        assert_eq!(
            read(&out),
            b"\x1b[A",
            "viewport, drain, damage, write and resize leave the input slot alone"
        );
        // The next encoder call replaces it (even by an empty result).
        let mut next = LunetermBytes::EMPTY;
        // SAFETY: a live handle and out-param.
        assert_eq!(
            unsafe { luneterm_engine_focus_event(twin.engine, 1, &mut next) },
            LUNETERM_OK
        );
        assert_eq!(read(&next), b"");
        // And a result is replaced by a result.
        // SAFETY: a live handle and out-param.
        assert_eq!(
            unsafe { luneterm_engine_key_input(twin.engine, 0, 0, &mut next) },
            LUNETERM_OK
        );
        assert_eq!(read(&next), b"\r");
    }

    #[test]
    fn null_and_misaligned_pointers_are_refused_and_the_engine_stays_usable() {
        let twin = Twin::new(80, 24);
        let engine = twin.engine;
        let key = LunetermKeyEvent::default();
        let mouse = LunetermMouseEvent::default();
        let mut out = garbage();
        let mut buf = [0u64; 16];
        let bad = buf.as_mut_ptr().cast::<u8>().wrapping_add(1);
        // SAFETY: a live handle; NULL and misaligned pointers are refused before any access, the
        // rest are live.
        unsafe {
            assert_eq!(
                luneterm_engine_key_event(engine, null(), &mut out),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(out.len, 0);
            assert_eq!(
                luneterm_engine_key_event(engine, bad.cast(), &mut out),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_key_event(engine, &key, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_key_event(engine, &key, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_mouse_event(engine, null(), &mut out),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_mouse_event(engine, bad.cast(), &mut out),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_mouse_event(engine, &mouse, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_key_input(engine, 0, 0, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_char_input(engine, 97, 0, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_focus_event(engine, 1, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(
                luneterm_engine_paste(engine, b"x".as_ptr(), 1, bad.cast()),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
        }
        assert_eq!(buf, [0; 16], "a refused pointer is never written");
        assert_eq!(key_input(engine, 0, 0), (LUNETERM_OK, b"\r".to_vec()));
    }
}
