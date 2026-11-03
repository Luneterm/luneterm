//! Tests for the legacy generators (plan M7 step 5). The byte-for-byte expectations over every key
//! and modifier set are the Lunegit goldens (`lune-vt-conformance/tests/lunegit_input.rs`, real
//! `XtermVtInputEncoder` output); these are the readable tables and the properties around them.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

const N: TerminalModifiers = TerminalModifiers::NONE;
const S: TerminalModifiers = TerminalModifiers::SHIFT;
const A: TerminalModifiers = TerminalModifiers::ALT;
const C: TerminalModifiers = TerminalModifiers::CONTROL;

fn key(k: TerminalKey, m: TerminalModifiers, decckm: bool) -> String {
    legacy_key_input(k, m, decckm)
}

#[test]
fn unmodified_keys() {
    let cases = [
        (TerminalKey::Enter, "\r"),
        (TerminalKey::Tab, "\t"),
        (TerminalKey::Backspace, "\u{7f}"),
        (TerminalKey::Escape, "\u{1b}"),
        (TerminalKey::Space, " "),
        (TerminalKey::Up, "\u{1b}[A"),
        (TerminalKey::Down, "\u{1b}[B"),
        (TerminalKey::Right, "\u{1b}[C"),
        (TerminalKey::Left, "\u{1b}[D"),
        (TerminalKey::Home, "\u{1b}[H"),
        (TerminalKey::End, "\u{1b}[F"),
        (TerminalKey::PageUp, "\u{1b}[5~"),
        (TerminalKey::PageDown, "\u{1b}[6~"),
        (TerminalKey::Insert, "\u{1b}[2~"),
        (TerminalKey::Delete, "\u{1b}[3~"),
        (TerminalKey::F1, "\u{1b}OP"),
        (TerminalKey::F2, "\u{1b}OQ"),
        (TerminalKey::F3, "\u{1b}OR"),
        (TerminalKey::F4, "\u{1b}OS"),
        (TerminalKey::F5, "\u{1b}[15~"),
        (TerminalKey::F6, "\u{1b}[17~"),
        (TerminalKey::F7, "\u{1b}[18~"),
        (TerminalKey::F8, "\u{1b}[19~"),
        (TerminalKey::F9, "\u{1b}[20~"),
        (TerminalKey::F10, "\u{1b}[21~"),
        (TerminalKey::F11, "\u{1b}[23~"),
        (TerminalKey::F12, "\u{1b}[24~"),
    ];
    for (k, want) in cases {
        assert_eq!(key(k, N, false), want, "{k:?}");
    }
}

#[test]
fn decckm_only_changes_unmodified_arrows() {
    for (k, normal, app) in [
        (TerminalKey::Up, "\u{1b}[A", "\u{1b}OA"),
        (TerminalKey::Down, "\u{1b}[B", "\u{1b}OB"),
        (TerminalKey::Right, "\u{1b}[C", "\u{1b}OC"),
        (TerminalKey::Left, "\u{1b}[D", "\u{1b}OD"),
    ] {
        assert_eq!(key(k, N, false), normal);
        assert_eq!(key(k, N, true), app);
        assert_eq!(key(k, C, true), key(k, C, false));
    }
    assert_eq!(key(TerminalKey::Home, N, true), "\u{1b}[H");
    assert_eq!(key(TerminalKey::F1, N, true), "\u{1b}OP");
}

#[test]
fn modifier_parameter_is_one_plus_shift_two_alt_four_control() {
    for (m, p) in [
        (S, 2),
        (A, 3),
        (S | A, 4),
        (C, 5),
        (S | C, 6),
        (A | C, 7),
        (S | A | C, 8),
    ] {
        assert_eq!(key(TerminalKey::Up, m, false), format!("\u{1b}[1;{p}A"));
        assert_eq!(key(TerminalKey::Home, m, false), format!("\u{1b}[1;{p}H"));
        assert_eq!(key(TerminalKey::PageUp, m, false), format!("\u{1b}[5;{p}~"));
        assert_eq!(key(TerminalKey::Delete, m, false), format!("\u{1b}[3;{p}~"));
        assert_eq!(
            key(TerminalKey::Escape, m, false),
            format!("\u{1b}[27;{p}~")
        );
        assert_eq!(key(TerminalKey::F1, m, false), format!("\u{1b}[1;{p}P"));
        assert_eq!(key(TerminalKey::F3, m, false), format!("\u{1b}[1;{p}R"));
        assert_eq!(key(TerminalKey::F5, m, false), format!("\u{1b}[15;{p}~"));
        assert_eq!(key(TerminalKey::F12, m, false), format!("\u{1b}[24;{p}~"));
    }
}

#[test]
fn the_generator_itself_ignores_modifiers_on_enter_and_backspace_and_marks_shift_tab() {
    // The deviations (ESC CR, BS, ...) live in the encoder, not here.
    assert_eq!(key(TerminalKey::Enter, S | A | C, false), "\r");
    assert_eq!(key(TerminalKey::Backspace, S | A | C, false), "\u{7f}");
    assert_eq!(key(TerminalKey::Tab, S, false), "\u{1b}[Z");
    assert_eq!(key(TerminalKey::Tab, A | C, false), "\t");
    assert_eq!(key(TerminalKey::Space, C, false), "\0");
    assert_eq!(key(TerminalKey::Space, S | A, false), " ");
}

#[test]
fn a_character_key_has_no_key_sequence() {
    assert_eq!(key(TerminalKey::Character, N, false), "");
    assert_eq!(key(TerminalKey::Character, C, true), "");
}

#[test]
fn unknown_modifier_bits_are_ignored() {
    for k in TerminalKey::ALL {
        for bits in 0u8..=255 {
            assert_eq!(
                key(k, TerminalModifiers::from_bits(bits), true),
                key(k, TerminalModifiers::from_bits(bits & 7), true),
                "{k:?} {bits}"
            );
        }
    }
}

#[test]
fn char_input_plain_ctrl_alt() {
    let chars = |c: char, m: TerminalModifiers| legacy_char_input(c, m);
    assert_eq!(chars('a', N), "a");
    assert_eq!(chars('a', S), "a");
    assert_eq!(chars('a', C), "\u{1}");
    assert_eq!(chars('Z', C), "\u{1a}");
    assert_eq!(chars('z', C | A), "\u{1b}\u{1a}");
    assert_eq!(chars('a', A), "\u{1b}a");
    assert_eq!(chars(' ', C), "\0");
    assert_eq!(chars('@', C), "\0");
    assert_eq!(chars('[', C), "\u{1b}");
    assert_eq!(chars('\\', C), "\u{1c}");
    assert_eq!(chars(']', C), "\u{1d}");
    assert_eq!(chars('^', C), "\u{1e}");
    assert_eq!(chars('_', C), "\u{1f}");
    assert_eq!(chars('?', C), "\u{7f}");
    // Not in the old table: passes through (the encoder's kitty C0 table adds more).
    assert_eq!(chars('2', C), "2");
    assert_eq!(chars('/', C), "/");
    assert_eq!(chars('\u{4e2d}', C | A), "\u{1b}\u{4e2d}");
    assert_eq!(chars('\u{e4}', N), "\u{e4}");
}

#[test]
fn char_input_reads_only_control_and_alt() {
    for bits in 0u8..=255 {
        let m = TerminalModifiers::from_bits(bits);
        let reduced = TerminalModifiers::from_bits(bits & 6);
        assert_eq!(legacy_char_input('x', m), legacy_char_input('x', reduced));
    }
}
