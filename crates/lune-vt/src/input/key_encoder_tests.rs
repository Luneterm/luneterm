//! Tests for the key encoder (plan M7 step 5).
//!
//! Sources:
//!  - Lunegit `TerminalKeyEncoderTests.cs` (561 lines): every Fact and every InlineData / MemberData
//!    row is ported below, in its order, with its literals. The `Oracle` there (the untouched
//!    `XtermVtInputEncoder`) is [`legacy_key_input`] / [`legacy_char_input`] here. Not ported:
//!    nothing. `TerminalKey_Character_IsAppendedLast` becomes a check of [`TerminalKey::ALL`]'s
//!    order (Rust enums have no ordinal arithmetic), and the C# `Assert.All(off, ch < 0x80)` is a
//!    check on bytes.
//!  - Lunegit `XtermJsVtEngineKeyboardProtocolTests.GenerateKeyEvent_FollowsTheLiveProtocolState`
//!    and `GenerateKeyEvent_UsesTheActiveScreensFlags` (moved here from step 3), as engine tests
//!    through [`Terminal::key_event`].
//!  - xterm.js `Keyboard.test.ts` (59 `it` cases): `Keyboard.ts` is superseded (plan D4: it maps
//!    DOM `KeyboardEvent`s by `keyCode`, which `TerminalKeyEvent` does not carry). Each case is
//!    adapted to the key it names, with the shifted character the DOM `key` would carry. Where
//!    Lunegit's encoder disagrees, Lunegit's behaviour wins (it is what the FFI must reproduce)
//!    and the case is listed below. The 59, in file order, tested in the `keyboard_ts_*` tests:
//!
//!    1. unmodified keys (Backspace ... F12): adapted, equal
//!    2. ctrl+delete: adapted
//!    3. shift+delete: adapted
//!    4. alt+delete: adapted
//!    5. alt+enter: adapted (`alt_is_meta`)
//!    6. alt+esc: DISAGREES. Keyboard.ts sends `ESC ESC`; Lunegit sends `CSI 27;3~`
//!       (`XtermVtInputEncoder`'s Escape with modifiers). The test pins Lunegit.
//!    7. ctrl+left: adapted
//!    8. ctrl+right: adapted
//!    9. ctrl+up: adapted
//!    10. ctrl+down: adapted
//!    11. ctrl+backspace: adapted (a Lunegit deviation that agrees)
//!    12. alt+backspace: adapted (same)
//!    13. ctrl+alt+backspace: adapted (same)
//!    14. shift+delete (repeat of 3): adapted
//!    15. alt+delete (repeat of 4): adapted
//!    16. non-mac alt+left: adapted
//!    17. non-mac alt+right: adapted
//!    18. non-mac alt+up: adapted
//!    19. non-mac alt+down: adapted
//!    20. non-mac alt+a: adapted as a `Character` event
//!    21. non-mac alt+space: adapted as a `Character ' '` event. The `Space` KEY with Alt
//!        DISAGREES (Lunegit: a bare space, `Legacy_SpaceKey_StaysXtermNetSpace`); pinned too.
//!    22. non-mac ctrl+alt+space: adapted as `Character ' '`; the `Space` key sends NUL without
//!        ESC (DISAGREES, pinned)
//!    23. mac alt+left: adapted
//!    24. mac alt+right: adapted
//!    25. mac alt+up: adapted
//!    26. mac alt+down: adapted
//!    27. mac alt+a is `undefined`: replaced, DOM-only (`undefined` means "let the OS compose the
//!        character"). The Lunegit analogue is `alt_is_meta = false`: the character itself,
//!        no ESC; pinned.
//!    28. macOptionIsMeta alt+a: adapted (`alt_is_meta = true`)
//!    29. macOptionIsMeta alt+enter: adapted
//!    30. alt+up (repeat): adapted
//!    31. alt+down (repeat): adapted
//!    32. modified F1-F12 (shift, alt, ctrl; 36 assertions): adapted
//!    33. ctrl+alt+a: adapted
//!    34. alt+0 / alt+shift+0: adapted (with the shifted character `)`)
//!    35. alt+1
//!    36. alt+2
//!    37. alt+3
//!    38. alt+4
//!    39. alt+5
//!    40. alt+6
//!    41. alt+7
//!    42. alt+8
//!    43. alt+9
//!    44. alt+; : adapted
//!    45. alt+=
//!    46. alt+,
//!    47. alt+-
//!    48. alt+.
//!    49. alt+/
//!    50. alt+~ (the backtick key)
//!    51. alt+[
//!    52. alt+\
//!    53. alt+]
//!    54. alt+'
//!    55. mobile arrow events (`key: 'UIKeyInputUpArrow'`, `keyCode: 0`): replaced, DOM-only;
//!        the arrows themselves are covered by 7-10, 16-19 and the DECCKM tests
//!    56. lowercase letters: adapted
//!    57. uppercase letters: adapted
//!    58. alt+shift+letter / alt+letter: adapted
//!    59. ctrl+@: adapted
//!
//! Added here: `encode_key` never panics (seeded sweep over every field), invalid scalars,
//! `KeyboardModes` packing, and the engine tests (`disable_stdin`, scroll on user input).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::ext::StandardExtensions;
use crate::headless::terminal::Terminal;
use crate::input::legacy_key_encoder::{legacy_char_input, legacy_key_input};
use crate::services::options_service::OptionValue;

const E: &str = "\u{1b}";
const C: &str = "\u{1b}[";

type M = TerminalModifiers;
type Ev = TerminalKeyEvent;
const N: M = M::NONE;
const S: M = M::SHIFT;
const A: M = M::ALT;
const CT: M = M::CONTROL;

const LEGACY: KeyboardModes = km(0, 0, false);
const LEGACY_DECCKM: KeyboardModes = km(0, 0, true);
const MOK2: KeyboardModes = km(0, 2, false);
const KITTY1: KeyboardModes = km(1, 0, false);
const KITTY5: KeyboardModes = km(5, 0, false);

const fn km(
    kitty_flags: u8,
    modify_other_keys: u8,
    application_cursor_keys: bool,
) -> KeyboardModes {
    KeyboardModes {
        kitty_flags,
        modify_other_keys,
        application_cursor_keys,
    }
}

fn all_mods() -> impl Iterator<Item = M> {
    (0u8..8).map(M::from_bits)
}

fn functional_keys() -> impl Iterator<Item = TerminalKey> {
    TerminalKey::ALL
        .into_iter()
        .filter(|k| *k != TerminalKey::Character)
}

/// A router-shaped character event: unshifted base plus optional shifted char (`'\0'` = none).
fn ch_full(c: char, mods: M, shifted: char, base_layout: char, keypad: bool, meta: bool) -> Ev {
    Ev {
        key: TerminalKey::Character,
        modifiers: mods,
        codepoint: c as u32,
        shifted_codepoint: shifted as u32,
        base_layout_codepoint: base_layout as u32,
        is_keypad: keypad,
        alt_is_meta: meta,
    }
}

fn ch(c: char, mods: M) -> Ev {
    ch_full(c, mods, '\0', '\0', false, true)
}

fn chs(c: char, mods: M, shifted: char) -> Ev {
    ch_full(c, mods, shifted, '\0', false, true)
}

fn ch_meta(c: char, mods: M, meta: bool) -> Ev {
    ch_full(c, mods, '\0', '\0', false, meta)
}

fn ch_keypad(c: char, mods: M) -> Ev {
    ch_full(c, mods, '\0', '\0', true, true)
}

fn key(k: TerminalKey, mods: M) -> Ev {
    Ev::for_key(k, mods)
}

fn key_pad(k: TerminalKey, mods: M) -> Ev {
    Ev {
        is_keypad: true,
        ..Ev::for_key(k, mods)
    }
}

fn key_meta(k: TerminalKey, mods: M, meta: bool) -> Ev {
    Ev {
        alt_is_meta: meta,
        ..Ev::for_key(k, mods)
    }
}

/// The encoding as text (the encoders only produce UTF-8).
fn enc(e: &Ev, modes: KeyboardModes) -> String {
    String::from_utf8(encode_key(e, modes)).unwrap()
}

fn legacy(e: &Ev) -> String {
    enc(e, LEGACY)
}

// ---- 1. legacy functional-key equivalence -----------------------------------------------------

/// The explicit, complete list of legacy functional-key combinations whose bytes deliberately
/// differ from [`legacy_key_input`]. Expected bytes are independent of DECCKM.
fn deviation_rows() -> Vec<(TerminalKey, M, bool, String)> {
    let esc_cr = format!("{E}\r");
    let tab_back = format!("{E}{C}Z");
    let k = TerminalKey::Enter;
    let b = TerminalKey::Backspace;
    let t = TerminalKey::Tab;
    vec![
        // Enter + Shift (any combination, Alt-as-Meta irrelevant) -> ESC CR.
        (k, S, true, esc_cr.clone()),
        (k, S, false, esc_cr.clone()),
        (k, S | A, true, esc_cr.clone()),
        (k, S | A, false, esc_cr.clone()),
        (k, S | CT, true, esc_cr.clone()),
        (k, S | CT, false, esc_cr.clone()),
        (k, S | A | CT, true, esc_cr.clone()),
        (k, S | A | CT, false, esc_cr.clone()),
        // Enter + Alt(meta) -> ESC CR.
        (k, A, true, esc_cr.clone()),
        (k, A | CT, true, esc_cr),
        // Backspace + Ctrl -> BS (ESC-prefixed when Alt is Meta).
        (b, CT, true, "\u{8}".into()),
        (b, CT, false, "\u{8}".into()),
        (b, S | CT, true, "\u{8}".into()),
        (b, S | CT, false, "\u{8}".into()),
        (b, A | CT, true, format!("{E}\u{8}")),
        (b, A | CT, false, "\u{8}".into()),
        (b, S | A | CT, true, format!("{E}\u{8}")),
        (b, S | A | CT, false, "\u{8}".into()),
        // Backspace + Alt(meta) -> ESC DEL.
        (b, A, true, format!("{E}\u{7f}")),
        (b, S | A, true, format!("{E}\u{7f}")),
        // Tab + Alt(meta) -> ESC-prefixed Tab / back-tab.
        (t, A, true, format!("{E}\t")),
        (t, S | A, true, tab_back.clone()),
        (t, A | CT, true, format!("{E}\t")),
        (t, S | A | CT, true, tab_back),
    ]
}

fn is_deliberate_deviation(k: TerminalKey, m: M, meta: bool) -> bool {
    deviation_rows()
        .iter()
        .any(|(rk, rm, rmeta, _)| *rk == k && *rm == m && *rmeta == meta)
}

#[test]
fn legacy_functional_keys_are_byte_identical_to_the_generator_except_listed_deviations() {
    let (mut compared, mut skipped) = (0, 0);
    let mut mismatches = Vec::new();
    for k in functional_keys() {
        for m in all_mods() {
            for decckm in [false, true] {
                for meta in [false, true] {
                    if is_deliberate_deviation(k, m, meta) {
                        skipped += 1;
                        continue;
                    }
                    compared += 1;
                    let want = legacy_key_input(k, m, decckm);
                    let got = enc(&key_meta(k, m, meta), km(0, 0, decckm));
                    if want != got {
                        mismatches.push(format!(
                            "{k:?} {m:?} decckm={decckm} meta={meta}: want {want:?} got {got:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    // 27 functional keys x 8 modifier sets x DECCKM {off,on} x AltIsMeta {off,on}.
    assert_eq!(functional_keys().count(), 27);
    assert_eq!(compared + skipped, 864);
    // 24 deviation rows x 2 DECCKM values.
    assert_eq!(deviation_rows().len(), 24);
    assert_eq!(skipped, 48);
    assert_eq!(compared, 816);
}

#[test]
fn legacy_deliberate_deviation_has_explicit_bytes() {
    for (k, m, meta, want) in deviation_rows() {
        let e = key_meta(k, m, meta);
        assert_eq!(enc(&e, LEGACY), want, "{k:?} {m:?} meta={meta}");
        assert_eq!(
            enc(&e, LEGACY_DECCKM),
            want,
            "{k:?} {m:?} meta={meta} decckm"
        );
        assert_ne!(
            legacy_key_input(k, m, false),
            want,
            "{k:?} {m:?} meta={meta}"
        );
    }
}

#[test]
fn legacy_keypad_enter_follows_enter_rules() {
    assert_eq!(legacy(&key_pad(TerminalKey::Enter, N)), "\r");
    assert_eq!(legacy(&key_pad(TerminalKey::Enter, CT)), "\r");
    assert_eq!(legacy(&key_pad(TerminalKey::Enter, S)), format!("{E}\r"));
}

#[test]
fn legacy_ctrl_enter_and_ctrl_tab_stay_indistinguishable() {
    // Only a protocol can distinguish these; legacy must not type garbage into a shell prompt.
    assert_eq!(legacy(&key(TerminalKey::Enter, CT)), "\r");
    assert_eq!(legacy(&key(TerminalKey::Tab, CT)), "\t");
}

// ---- 2. legacy character tables ---------------------------------------------------------------

#[test]
fn legacy_character_chords() {
    // (char, mods, shifted, altIsMeta, expected): Lunegit's InlineData rows, in order.
    let rows: [(char, M, char, bool, &str); 37] = [
        ('2', CT, '@', true, "\0"),
        ('3', CT, '#', true, "\u{1b}"),
        ('4', CT, '$', true, "\u{1c}"),
        ('5', CT, '%', true, "\u{1d}"),
        ('6', CT, '^', true, "\u{1e}"),
        ('7', CT, '&', true, "\u{1f}"),
        ('8', CT, '*', true, "\u{7f}"),
        ('1', CT, '!', true, "1"),
        ('9', CT, '(', true, "9"),
        ('0', CT, ')', true, "0"),
        (' ', CT, ' ', true, "\0"),
        ('/', CT, '?', true, "\u{1f}"),
        ('/', CT | S, '?', true, "\u{7f}"),
        ('-', CT, '_', true, "-"),
        ('-', CT | S, '_', true, "\u{1f}"),
        ('[', CT, '{', true, "\u{1b}"),
        ('\\', CT, '|', true, "\u{1c}"),
        (']', CT, '}', true, "\u{1d}"),
        ('`', CT, '~', true, "`"),
        ('`', CT | S, '~', true, "\u{1e}"),
        ('6', CT | S, '^', true, "\u{1e}"),
        ('2', CT | S, '@', true, "\0"),
        ('2', CT | A, '@', true, "\u{1b}\0"),
        ('2', CT | A, '@', false, "\0"),
        ('a', CT, '\0', true, "\u{1}"),
        ('a', CT | S, '\0', true, "\u{1}"),
        ('z', CT, '\0', true, "\u{1a}"),
        ('a', CT | A, '\0', true, "\u{1b}\u{1}"),
        ('a', CT | A, '\0', false, "\u{1b}\u{1}"),
        ('a', A, '\0', true, "\u{1b}a"),
        ('a', A | S, '\0', true, "\u{1b}A"),
        ('1', A, '!', true, "\u{1b}1"),
        ('1', A | S, '!', true, "\u{1b}!"),
        ('[', A, '{', true, "\u{1b}["),
        ('.', A, '>', true, "\u{1b}."),
        ('v', A, '\0', false, "v"),
        (' ', A, ' ', true, "\u{1b} "),
    ];
    for (c, mods, shifted, meta, want) in rows {
        let e = ch_full(c, mods, shifted, '\0', false, meta);
        assert_eq!(legacy(&e), want, "{c:?} {mods:?} meta={meta}");
    }
}

#[test]
fn legacy_keypad_character_with_alt_meta_sends_keypad_char() {
    assert_eq!(legacy(&ch_keypad('1', A)), format!("{E}1"));
    // Keypad chars carry no shifted form: Shift keeps the keypad char.
    assert_eq!(legacy(&ch_keypad('1', A | S)), format!("{E}1"));
    assert_eq!(legacy(&ch_keypad('+', A)), format!("{E}+"));
}

#[test]
fn legacy_space_key_stays_the_generators_space() {
    assert_eq!(legacy(&key(TerminalKey::Space, CT)), "\0");
    assert_eq!(legacy(&key(TerminalKey::Space, A)), " ");
    assert_eq!(legacy(&key(TerminalKey::Space, N)), " ");
}

#[test]
fn invalid_or_missing_codepoint_produces_no_text() {
    let ev = |mods: M, cp: u32| Ev {
        codepoint: cp,
        ..Ev::for_key(TerminalKey::Character, mods)
    };
    assert_eq!(legacy(&ev(A, 0)), "");
    assert_eq!(legacy(&ev(A, 0xD800)), "");
    assert_eq!(enc(&ev(CT, u32::MAX), MOK2), ""); // C#: -1
    assert_eq!(enc(&ev(CT, 0), KITTY1), "");
    assert_eq!(enc(&ev(N, 0x110000), KITTY1), "");
}

// ---- 3. ForChar parity with the old char path -------------------------------------------------

/// Ctrl+char whose old mapping was a passthrough and the kitty C0 table now maps.
const CTRL_TABLE_ADDITIONS: [(char, char); 9] = [
    ('2', '\0'),
    ('3', '\u{1b}'),
    ('4', '\u{1c}'),
    ('5', '\u{1d}'),
    ('6', '\u{1e}'),
    ('7', '\u{1f}'),
    ('8', '\u{7f}'),
    ('/', '\u{1f}'),
    ('~', '\u{1e}'),
];

#[test]
fn legacy_for_char_matches_old_char_input_except_ctrl_table_additions() {
    let (mut compared, mut excepted) = (0, 0);
    let mut mismatches = Vec::new();
    for i in 0x20u32..=0x7E {
        let c = char::from_u32(i).unwrap();
        for m in all_mods() {
            let actual = legacy(&Ev::for_char(c, m));
            let addition = CTRL_TABLE_ADDITIONS.iter().find(|(k, _)| *k == c);
            if let (true, Some((_, c0))) = (m.contains(CT), addition) {
                excepted += 1;
                let want = format!("{}{c0}", if m.contains(A) { E } else { "" });
                if want != actual {
                    mismatches.push(format!(
                        "addition {c:?} {m:?}: want {want:?} got {actual:?}"
                    ));
                }
                if legacy_char_input(c, m) == actual {
                    mismatches.push(format!(
                        "addition {c:?} {m:?} unexpectedly equals the old bytes"
                    ));
                }
                continue;
            }
            compared += 1;
            let want = legacy_char_input(c, m);
            if want != actual {
                mismatches.push(format!("{c:?} {m:?}: want {want:?} got {actual:?}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    // 95 printable ASCII chars x 8 modifier sets; 9 additions x 4 Ctrl-containing sets.
    assert_eq!(compared + excepted, 760);
    assert_eq!(excepted, 36);
    assert_eq!(compared, 724);
}

#[test]
fn for_char_sets_codepoint_and_shifted_to_the_char_as_given() {
    let ev = Ev::for_char('a', S);
    assert_eq!(ev.key, TerminalKey::Character);
    assert_eq!(ev.codepoint, 'a' as u32);
    assert_eq!(ev.shifted_codepoint, 'a' as u32);
    assert!(ev.alt_is_meta);
    assert!(!ev.is_keypad);
    assert_eq!(ev.base_layout_codepoint, 0);
    assert_eq!(
        Ev {
            key: TerminalKey::Up,
            modifiers: CT,
            codepoint: 0,
            shifted_codepoint: 0,
            base_layout_codepoint: 0,
            is_keypad: false,
            alt_is_meta: true,
        },
        Ev::for_key(TerminalKey::Up, CT)
    );
}

#[test]
fn terminal_key_character_is_last_and_all_is_in_order() {
    assert_eq!(TerminalKey::ALL.last(), Some(&TerminalKey::Character));
    assert_eq!(TerminalKey::ALL.iter().max(), Some(&TerminalKey::Character));
    assert!(TerminalKey::ALL.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        TerminalKey::ALL[TerminalKey::F12 as usize + 1],
        TerminalKey::Character
    );
    for k in TerminalKey::ALL {
        assert_eq!(TerminalKey::from_name(k.name()), Some(k));
    }
    assert_eq!(TerminalKey::from_name("f1"), None);
}

// ---- 4. modifyOtherKeys -----------------------------------------------------------------------

#[test]
fn modify_other_keys_table() {
    #[rustfmt::skip]
    let cases: Vec<(u8, Ev, String)> = vec![
        // Level 2.
        (2, key(TerminalKey::Enter, CT), format!("{C}27;5;13~")),
        (2, key(TerminalKey::Enter, S), format!("{C}27;2;13~")),
        (2, key(TerminalKey::Enter, A | CT), format!("{C}27;7;13~")),
        (2, key_pad(TerminalKey::Enter, CT), format!("{C}27;5;13~")),
        (2, key(TerminalKey::Enter, N), "\r".into()),
        (2, key(TerminalKey::Tab, S), format!("{C}Z")),
        (2, key(TerminalKey::Tab, CT), format!("{C}27;5;9~")),
        (2, key(TerminalKey::Tab, S | CT), format!("{C}27;6;9~")),
        (2, key(TerminalKey::Tab, N), "\t".into()),
        (2, key(TerminalKey::Escape, N), E.into()),
        (2, key(TerminalKey::Escape, CT), format!("{C}27;5;27~")),
        (2, key(TerminalKey::Backspace, A), format!("{C}27;3;127~")),
        (2, key(TerminalKey::Backspace, S), format!("{C}27;2;127~")),
        (2, key(TerminalKey::Backspace, S | CT), format!("{C}27;6;127~")),
        (2, key(TerminalKey::Backspace, CT), "\u{8}".into()),
        (2, key(TerminalKey::Backspace, N), "\u{7f}".into()),
        (2, ch('a', CT), format!("{C}27;5;97~")),
        (2, ch('a', CT | S), format!("{C}27;6;65~")),
        (2, chs('[', A, '{'), format!("{C}27;3;91~")),
        (2, chs('1', CT | S, '!'), format!("{C}27;6;33~")),
        (2, ch_meta('a', A, false), format!("{C}27;3;97~")),
        (2, ch_keypad('1', CT), format!("{C}27;5;49~")),
        (2, ch('a', S), "A".into()),
        (2, key(TerminalKey::Space, CT), format!("{C}27;5;32~")),
        (2, key(TerminalKey::Space, N), " ".into()),
        (2, key(TerminalKey::Up, CT), format!("{C}1;5A")),
        (2, key(TerminalKey::F5, N), format!("{C}15~")),
        // Level 1: only modified Enter and non-Shift-only modified Tab change.
        (1, key(TerminalKey::Enter, CT), format!("{C}27;5;13~")),
        (1, key(TerminalKey::Enter, S), format!("{C}27;2;13~")),
        (1, key(TerminalKey::Tab, CT), format!("{C}27;5;9~")),
        (1, key(TerminalKey::Tab, S), format!("{C}Z")),
        (1, key(TerminalKey::Escape, CT), format!("{C}27;5~")),
        (1, key(TerminalKey::Backspace, A), format!("{E}\u{7f}")),
        (1, ch('a', CT), "\u{1}".into()),
        (1, chs('[', A, '{'), format!("{E}[")),
        (1, key(TerminalKey::Space, CT), "\0".into()),
    ];
    for (level, ev, want) in cases {
        assert_eq!(enc(&ev, km(0, level, false)), want, "level {level} {ev:?}");
    }
}

#[test]
fn modify_other_keys_level_3_behaves_as_level_2() {
    assert_eq!(enc(&ch('a', CT), km(0, 3, false)), format!("{C}27;5;97~"));
    // Past Lunegit's two packed bits: still level 2.
    assert_eq!(enc(&ch('a', CT), km(0, 200, false)), format!("{C}27;5;97~"));
}

#[test]
fn modify_other_keys_unmodified_arrow_honours_decckm() {
    assert_eq!(
        enc(&key(TerminalKey::Up, N), km(0, 2, true)),
        format!("{E}OA")
    );
    assert_eq!(enc(&key(TerminalKey::Up, N), MOK2), format!("{C}A"));
}

#[test]
fn modify_other_keys_a_shifted_code_point_that_is_not_a_scalar_sends_nothing() {
    // Not covered by Lunegit's tests (documented in the module header).
    for shifted in [0xD800, 0x110000, u32::MAX] {
        let ev = Ev {
            shifted_codepoint: shifted,
            ..ch('a', CT | S)
        };
        assert_eq!(enc(&ev, MOK2), "", "{shifted:#x}");
    }
}

// ---- 5. kitty flag 1 --------------------------------------------------------------------------

#[test]
fn kitty_flag1_table() {
    #[rustfmt::skip]
    let cases: Vec<(Ev, String)> = vec![
        (key(TerminalKey::Enter, CT), format!("{C}13;5u")),
        (key(TerminalKey::Enter, N), "\r".into()),
        (key(TerminalKey::Enter, S), format!("{C}13;2u")),
        (key(TerminalKey::Enter, S | CT), format!("{C}13;6u")),
        (key_meta(TerminalKey::Enter, A, false), format!("{C}13;3u")),
        (key_pad(TerminalKey::Enter, N), format!("{C}57414u")),
        (key_pad(TerminalKey::Enter, CT), format!("{C}57414;5u")),
        (key(TerminalKey::Tab, N), "\t".into()),
        (key(TerminalKey::Tab, S), format!("{C}9;2u")),
        (key(TerminalKey::Tab, CT), format!("{C}9;5u")),
        (key(TerminalKey::Backspace, N), "\u{7f}".into()),
        (key(TerminalKey::Backspace, A), format!("{C}127;3u")),
        (key(TerminalKey::Backspace, CT), format!("{C}127;5u")),
        (key(TerminalKey::Escape, N), format!("{C}27u")),
        (key(TerminalKey::Escape, CT), format!("{C}27;5u")),
        (key(TerminalKey::Up, N), format!("{C}A")),
        (key(TerminalKey::Up, CT), format!("{C}1;5A")),
        (key(TerminalKey::Down, S), format!("{C}1;2B")),
        (key(TerminalKey::Right, N), format!("{C}C")),
        (key(TerminalKey::Left, N), format!("{C}D")),
        (key_pad(TerminalKey::Left, N), format!("{C}57417u")),
        (key_pad(TerminalKey::Right, N), format!("{C}57418u")),
        (key_pad(TerminalKey::Up, N), format!("{C}57419u")),
        (key_pad(TerminalKey::Down, CT), format!("{C}57420;5u")),
        (key(TerminalKey::Home, N), format!("{C}H")),
        (key(TerminalKey::End, S), format!("{C}1;2F")),
        (key_pad(TerminalKey::Home, N), format!("{C}57423u")),
        (key_pad(TerminalKey::End, N), format!("{C}57424u")),
        (key(TerminalKey::PageUp, N), format!("{C}5~")),
        (key(TerminalKey::PageDown, CT), format!("{C}6;5~")),
        (key(TerminalKey::Insert, N), format!("{C}2~")),
        (key(TerminalKey::Delete, CT), format!("{C}3;5~")),
        (key_pad(TerminalKey::PageUp, N), format!("{C}57421u")),
        (key_pad(TerminalKey::PageDown, N), format!("{C}57422u")),
        (key_pad(TerminalKey::Insert, N), format!("{C}57425u")),
        (key_pad(TerminalKey::Delete, N), format!("{C}57426u")),
        (key(TerminalKey::F1, N), format!("{C}P")),
        (key(TerminalKey::F1, CT), format!("{C}1;5P")),
        (key(TerminalKey::F2, N), format!("{C}Q")),
        (key(TerminalKey::F3, N), format!("{C}13~")),
        (key(TerminalKey::F3, CT), format!("{C}13;5~")),
        (key(TerminalKey::F4, N), format!("{C}S")),
        (key(TerminalKey::F5, N), format!("{C}15~")),
        (key(TerminalKey::F6, N), format!("{C}17~")),
        (key(TerminalKey::F7, N), format!("{C}18~")),
        (key(TerminalKey::F8, N), format!("{C}19~")),
        (key(TerminalKey::F9, N), format!("{C}20~")),
        (key(TerminalKey::F10, N), format!("{C}21~")),
        (key(TerminalKey::F11, N), format!("{C}23~")),
        (key(TerminalKey::F12, S), format!("{C}24;2~")),
        (ch('c', CT), format!("{C}99;5u")),
        (ch('c', CT | S), format!("{C}99;6u")),
        (ch_meta('a', A, false), format!("{C}97;3u")),
        (ch('a', N), "a".into()),
        (ch('a', S), "A".into()),
        (chs('1', CT, '!'), format!("{C}49;5u")),
        (ch_keypad('1', CT), format!("{C}57400;5u")),
        (ch_keypad('0', CT), format!("{C}57399;5u")),
        (ch_keypad('9', A), format!("{C}57408;3u")),
        (ch_keypad('.', CT), format!("{C}57409;5u")),
        (ch_keypad('/', CT), format!("{C}57410;5u")),
        (ch_keypad('*', CT), format!("{C}57411;5u")),
        (ch_keypad('-', CT), format!("{C}57412;5u")),
        (ch_keypad('+', CT), format!("{C}57413;5u")),
        (ch_keypad('=', CT), format!("{C}57415;5u")),
        (ch_keypad(',', CT), format!("{C}57416;5u")),
        (key(TerminalKey::Space, CT), format!("{C}32;5u")),
        (key(TerminalKey::Space, N), " ".into()),
    ];
    assert_eq!(cases.len(), 68);
    for (ev, want) in cases {
        assert_eq!(enc(&ev, KITTY1), want, "{ev:?}");
    }
}

#[test]
fn kitty_up_with_decckm_is_csi_not_ss3() {
    assert_eq!(
        enc(&key(TerminalKey::Up, N), km(1, 0, true)),
        format!("{C}A")
    );
}

#[test]
fn kitty_never_emits_ss3_ignores_decckm_and_output_is_ascii() {
    let mut checked = 0;
    for k in functional_keys() {
        for m in all_mods() {
            for keypad in [false, true] {
                let ev = Ev {
                    is_keypad: keypad,
                    ..key(k, m)
                };
                let off = encode_key(&ev, km(1, 0, false));
                let on = encode_key(&ev, km(1, 0, true));
                assert_eq!(off, on, "{ev:?}");
                let text = String::from_utf8(off.clone()).unwrap();
                assert!(!text.contains(&format!("{E}O")), "{ev:?}");
                assert!(!off.is_empty(), "{ev:?}");
                assert!(off.iter().all(|b| *b < 0x80), "{ev:?}");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 27 * 8 * 2);
}

// ---- 6. kitty flags 5 (alternate keys) --------------------------------------------------------

#[test]
fn kitty_flags5_alternate_keys() {
    // (char, mods, shifted, base layout, keypad, expected tail)
    let rows: [(char, M, char, char, bool, &str); 9] = [
        ('a', CT | S, '\0', '\0', false, "97:65;6u"),
        ('a', CT, '\0', 'q', false, "97::113;5u"),
        ('a', CT | S, '\0', 'q', false, "97:65:113;6u"),
        ('a', CT, '\0', 'a', false, "97;5u"),
        ('a', CT, '\0', '\0', false, "97;5u"),
        ('1', CT | S, '!', '\0', false, "49;6u"),
        ('1', CT | S, '!', '1', false, "49;6u"),
        ('1', CT, '\0', '\0', true, "57400;5u"),
        ('1', CT, '\0', '1', true, "57400;5u"),
    ];
    for (c, mods, shifted, base_layout, keypad, tail) in rows {
        let ev = ch_full(c, mods, shifted, base_layout, keypad, true);
        assert_eq!(enc(&ev, KITTY5), format!("{C}{tail}"), "{ev:?}");
    }
}

#[test]
fn kitty_flag1_only_emits_no_alternates() {
    let ev = ch_full('a', CT | S, '\0', 'q', false, true);
    assert_eq!(enc(&ev, KITTY1), format!("{C}97;6u"));
}

#[test]
fn kitty_letter_key_field_is_always_lowercase() {
    assert_eq!(
        enc(&Ev::for_char('A', CT | S), KITTY5),
        format!("{C}97:65;6u")
    );
}

#[test]
fn kitty_flag4_alone_does_not_enable_kitty_encoding() {
    assert_eq!(enc(&key(TerminalKey::Enter, CT), km(4, 0, false)), "\r");
}

#[test]
fn kitty_a_base_layout_that_is_not_a_scalar_sends_nothing() {
    // Not covered by Lunegit's tests (documented in the module header).
    for base in [0xD800, 0x110000, u32::MAX] {
        let ev = Ev {
            base_layout_codepoint: base,
            ..ch('a', CT)
        };
        assert_eq!(enc(&ev, KITTY5), "", "{base:#x}");
        // Without flag 4 the field is never read.
        assert_eq!(enc(&ev, KITTY1), format!("{C}97;5u"), "{base:#x}");
    }
}

// ---- 7. precedence ----------------------------------------------------------------------------

#[test]
fn precedence_kitty_flag1_beats_modify_other_keys() {
    let both = km(5, 2, false);
    assert_eq!(enc(&key(TerminalKey::Enter, CT), both), format!("{C}13;5u"));
    assert_eq!(enc(&ch('a', CT | S), both), format!("{C}97:65;6u"));
}

#[test]
fn precedence_kitty_without_flag1_falls_through_to_modify_other_keys_or_legacy() {
    let flag4_mok = km(4, 2, false);
    assert_eq!(
        enc(&key(TerminalKey::Enter, CT), flag4_mok),
        format!("{C}27;5;13~")
    );
    assert_eq!(enc(&ch('a', CT | S), flag4_mok), format!("{C}27;6;65~"));

    let flag4_only = km(4, 0, false);
    assert_eq!(enc(&key(TerminalKey::Enter, CT), flag4_only), "\r");
    assert_eq!(
        enc(&key(TerminalKey::Up, N), km(4, 0, true)),
        format!("{E}OA")
    );
}

#[test]
fn modifiers_are_masked_to_shift_alt_control() {
    for bits in 0u8..=255 {
        for modes in [LEGACY, KITTY5, MOK2] {
            for ev in [
                key(TerminalKey::Up, M::from_bits(bits)),
                ch('a', M::from_bits(bits)),
            ] {
                let reduced = Ev {
                    modifiers: M::from_bits(bits & 7),
                    ..ev
                };
                assert_eq!(
                    encode_key(&ev, modes),
                    encode_key(&reduced, modes),
                    "{bits} {ev:?}"
                );
            }
        }
    }
}

// ---- 8. KeyboardModes packing -----------------------------------------------------------------

#[test]
fn keyboard_modes_pack_unpack_round_trips() {
    for (flags, mok, decckm) in [
        (0, 0, false),
        (1, 0, false),
        (5, 2, true),
        (4, 1, false),
        (31, 2, true),
        (255, 3, true),
    ] {
        let modes = km(flags, mok, decckm);
        assert_eq!(KeyboardModes::unpack(modes.pack()), modes);
    }
}

#[test]
fn keyboard_modes_layout_is_flags_low_byte_mok_bits_8_to_9_decckm_bit_16() {
    assert_eq!(
        KeyboardModes::unpack(5 | (2 << 8) | (1 << 16)),
        km(5, 2, true)
    );
    assert_eq!(km(5, 2, true).pack(), 5 | (2 << 8) | (1 << 16));
    assert_eq!(KeyboardModes::unpack(0), km(0, 0, false));
    // Bits outside the layout are dropped.
    assert_eq!(KeyboardModes::unpack(0xFFFF_FFFF), km(255, 3, true));
    assert_eq!(km(0, 7, false).pack(), 3 << 8);
}

// ---- 9. added: encode_key never panics --------------------------------------------------------

/// xorshift64*: a seeded stream, so a failure is replayable from the seed in the message.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn encode_key_never_panics_for_any_field_values() {
    // Code points that matter: ASCII letters / digits / punctuation, NUL, the surrogate range and
    // its edges, the last scalar, the first invalid values, and random u32s.
    let special: [u32; 16] = [
        0,
        0x20,
        0x41,
        0x5A,
        0x61,
        0x7A,
        0xD7FF,
        0xD800,
        0xDBFF,
        0xDFFF,
        0xE000,
        0x10FFFF,
        0x110000,
        0x7FFF_FFFF,
        0x8000_0000,
        u32::MAX,
    ];
    for seed in 1..=20u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let pick = |rng: &mut Rng| -> u32 {
            match rng.below(3) {
                0 => special[rng.below(special.len() as u64) as usize],
                1 => rng.below(0x80) as u32,
                _ => rng.next() as u32,
            }
        };
        for _ in 0..10_000 {
            let k = TerminalKey::ALL[rng.below(TerminalKey::ALL.len() as u64) as usize];
            let ev = Ev {
                key: k,
                modifiers: M::from_bits(rng.below(256) as u8),
                codepoint: pick(&mut rng),
                shifted_codepoint: pick(&mut rng),
                base_layout_codepoint: pick(&mut rng),
                is_keypad: rng.below(2) == 1,
                alt_is_meta: rng.below(2) == 1,
            };
            let modes = KeyboardModes {
                kitty_flags: rng.below(256) as u8,
                modify_other_keys: rng.below(256) as u8,
                application_cursor_keys: rng.below(2) == 1,
            };
            let out = encode_key(&ev, modes);
            assert!(
                out.len() <= 64,
                "seed {seed}: {} bytes for {ev:?} {modes:?}",
                out.len()
            );
            assert!(
                std::str::from_utf8(&out).is_ok(),
                "seed {seed}: {ev:?} {modes:?}"
            );
            let cp_ok = ev.codepoint != 0 && char::from_u32(ev.codepoint).is_some();
            if k == TerminalKey::Character && !cp_ok {
                assert!(out.is_empty(), "seed {seed}: {ev:?} {modes:?}");
            }
        }
    }
}

// ---- 10. engine tests: the live protocol state ------------------------------------------------

fn engine() -> Terminal {
    let mut term = Terminal::new([OptionValue::Cols(40), OptionValue::Rows(4)]);
    term.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    term
}

fn event_text(term: &mut Terminal, ev: &Ev) -> String {
    String::from_utf8(term.key_event(ev).unwrap()).unwrap()
}

#[test]
fn generate_key_event_follows_the_live_protocol_state() {
    let mut t = engine();
    let up = key(TerminalKey::Up, N);
    let ctrl_enter = key(TerminalKey::Enter, CT);

    t.write_str(&format!("{E}[?1h{E}[>5u{E}[>4;2m"));
    assert_eq!(event_text(&mut t, &up), format!("{E}[A")); // kitty ignores DECCKM
    assert_eq!(event_text(&mut t, &ctrl_enter), format!("{E}[13;5u"));

    t.write_str(&format!("{E}[<u"));
    assert_eq!(event_text(&mut t, &up), format!("{E}OA")); // legacy DECCKM
    assert_eq!(event_text(&mut t, &ctrl_enter), format!("{E}[27;5;13~")); // modifyOtherKeys 2

    t.write_str(&format!("{E}[>4m"));
    assert_eq!(event_text(&mut t, &ctrl_enter), "\r"); // legacy
}

#[test]
fn generate_key_event_uses_the_active_screens_flags() {
    let mut t = engine();
    let ctrl_enter = key(TerminalKey::Enter, CT);

    t.write_str(&format!("{E}[?1049h{E}[>1u"));
    assert_eq!(event_text(&mut t, &ctrl_enter), format!("{E}[13;5u"));

    t.write_str(&format!("{E}[?1049l"));
    assert_eq!(event_text(&mut t, &ctrl_enter), "\r");
}

#[test]
fn ris_resets_decckm_for_the_key_encoder_too() {
    let mut t = engine();
    let up = key(TerminalKey::Up, N);
    t.write_str(&format!("{E}[?1h"));
    assert_eq!(event_text(&mut t, &up), format!("{E}OA"));
    t.write_str(&format!("{E}c"));
    assert_eq!(event_text(&mut t, &up), format!("{E}[A"));
}

#[test]
fn keyboard_modes_reads_the_live_state() {
    let mut t = engine();
    assert_eq!(t.keyboard_modes(), LEGACY);
    t.write_str(&format!("{E}[?1h{E}[>5u{E}[>4;2m"));
    assert_eq!(t.keyboard_modes(), km(5, 2, true));
    assert_eq!(t.keyboard_modes().pack(), 5 | (2 << 8) | (1 << 16));
    let modes = t.modes();
    assert_eq!(modes.kitty_keyboard_flags, 5);
    assert_eq!(modes.modify_other_keys, 2);
}

#[test]
fn key_input_and_char_input_are_the_legacy_generators_with_live_decckm() {
    let mut t = engine();
    t.write_str(&format!("{E}[?1h{E}[>1u{E}[>4;2m"));
    // The protocol state is ignored; DECCKM is not.
    assert_eq!(
        t.key_input(TerminalKey::Up, N).unwrap(),
        format!("{E}OA").into_bytes()
    );
    assert_eq!(t.key_input(TerminalKey::Enter, CT).unwrap(), b"\r");
    assert_eq!(t.char_input('a', CT | A).unwrap(), b"\x1b\x01");
    assert_eq!(t.char_input('\u{4e2d}', N).unwrap(), "\u{4e2d}".as_bytes());
    // A character key has no key sequence: nothing is sent.
    assert_eq!(t.key_input(TerminalKey::Character, N), None);
}

#[test]
fn disable_stdin_sends_nothing() {
    let mut t = engine();
    t.set_option(OptionValue::DisableStdin(true)).unwrap();
    assert_eq!(t.key_event(&key(TerminalKey::Enter, N)), None);
    assert_eq!(t.key_event(&ch('a', N)), None);
    assert_eq!(t.key_input(TerminalKey::Up, N), None);
    assert_eq!(t.char_input('a', N), None);
    t.set_option(OptionValue::DisableStdin(false)).unwrap();
    assert_eq!(t.key_event(&key(TerminalKey::Enter, N)).unwrap(), b"\r");
}

fn scrolled_up() -> Terminal {
    let mut t = Terminal::new([OptionValue::Cols(20), OptionValue::Rows(4)]);
    for i in 0..30 {
        t.write_str(&format!("line {i}\r\n"));
    }
    t.scroll_lines(-10);
    assert!(!t.viewport().is_at_bottom);
    t
}

#[test]
fn user_input_scrolls_to_the_bottom_when_the_option_is_on() {
    let mut t = scrolled_up();
    assert_eq!(t.key_event(&ch('a', N)).unwrap(), b"a");
    assert!(t.viewport().is_at_bottom);

    let mut t = scrolled_up();
    t.set_option(OptionValue::ScrollOnUserInput(false)).unwrap();
    assert_eq!(t.key_event(&ch('a', N)).unwrap(), b"a");
    assert!(!t.viewport().is_at_bottom);
}

#[test]
fn a_key_that_sends_nothing_neither_returns_bytes_nor_scrolls() {
    let mut t = scrolled_up();
    let invalid = Ev {
        codepoint: 0xD800,
        ..key(TerminalKey::Character, N)
    };
    assert_eq!(t.key_event(&invalid), None);
    assert!(!t.viewport().is_at_bottom);
}

#[test]
fn encoders_do_not_queue_host_events() {
    let mut t = engine();
    t.key_event(&key(TerminalKey::Enter, N)).unwrap();
    assert!(t.drain_events().is_empty());
}

// ---- 11. xterm.js Keyboard.test.ts, adapted (module header lists all 59) -----------------------

/// (key, unmodified bytes) for `Keyboard.test.ts` case 1, in key-code order.
#[test]
fn keyboard_ts_1_unmodified_keys() {
    let cases = [
        (TerminalKey::Backspace, "\u{7f}"),
        (TerminalKey::Tab, "\t"),
        (TerminalKey::Enter, "\r"),
        (TerminalKey::Escape, "\u{1b}"),
        (TerminalKey::PageUp, "\u{1b}[5~"),
        (TerminalKey::PageDown, "\u{1b}[6~"),
        (TerminalKey::End, "\u{1b}[F"),
        (TerminalKey::Home, "\u{1b}[H"),
        (TerminalKey::Left, "\u{1b}[D"),
        (TerminalKey::Up, "\u{1b}[A"),
        (TerminalKey::Right, "\u{1b}[C"),
        (TerminalKey::Down, "\u{1b}[B"),
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
        assert_eq!(legacy(&key(k, N)), want, "{k:?}");
    }
}

#[test]
fn keyboard_ts_2_to_5_delete_and_alt_enter() {
    assert_eq!(legacy(&key(TerminalKey::Delete, CT)), format!("{C}3;5~"));
    assert_eq!(legacy(&key(TerminalKey::Delete, S)), format!("{C}3;2~"));
    assert_eq!(legacy(&key(TerminalKey::Delete, A)), format!("{C}3;3~"));
    assert_eq!(legacy(&key(TerminalKey::Enter, A)), format!("{E}\r"));
    // 14, 15: repeats of 3 and 4.
    assert_eq!(legacy(&key(TerminalKey::Delete, S)), format!("{C}3;2~"));
    assert_eq!(legacy(&key(TerminalKey::Delete, A)), format!("{C}3;3~"));
}

#[test]
fn keyboard_ts_6_alt_esc_disagrees_and_lunegit_wins() {
    // Keyboard.ts: ESC ESC. Lunegit (XtermVtInputEncoder): CSI 27;3 ~.
    assert_eq!(legacy(&key(TerminalKey::Escape, A)), format!("{C}27;3~"));
    assert_ne!(legacy(&key(TerminalKey::Escape, A)), format!("{E}{E}"));
}

#[test]
fn keyboard_ts_7_to_13_ctrl_arrows_and_backspace_chords() {
    assert_eq!(legacy(&key(TerminalKey::Left, CT)), format!("{C}1;5D"));
    assert_eq!(legacy(&key(TerminalKey::Right, CT)), format!("{C}1;5C"));
    assert_eq!(legacy(&key(TerminalKey::Up, CT)), format!("{C}1;5A"));
    assert_eq!(legacy(&key(TerminalKey::Down, CT)), format!("{C}1;5B"));
    assert_eq!(legacy(&key(TerminalKey::Backspace, CT)), "\u{8}");
    assert_eq!(
        legacy(&key(TerminalKey::Backspace, A)),
        format!("{E}\u{7f}")
    );
    assert_eq!(
        legacy(&key(TerminalKey::Backspace, CT | A)),
        format!("{E}\u{8}")
    );
}

#[test]
fn keyboard_ts_16_to_19_and_23_to_26_and_30_31_alt_arrows() {
    // The platform (isMac) does not matter to a TerminalKeyEvent: alt+arrow is the same CSI form,
    // with Alt as Meta or not.
    for meta in [true, false] {
        for (k, f) in [
            (TerminalKey::Left, 'D'),
            (TerminalKey::Right, 'C'),
            (TerminalKey::Up, 'A'),
            (TerminalKey::Down, 'B'),
        ] {
            assert_eq!(
                legacy(&key_meta(k, A, meta)),
                format!("{C}1;3{f}"),
                "{k:?} {meta}"
            );
        }
    }
}

#[test]
fn keyboard_ts_20_to_22_27_to_29_alt_letters_and_space() {
    // 20, 28: alt+a with Alt as Meta (non-mac, or mac with macOptionIsMeta).
    assert_eq!(legacy(&ch('a', A)), format!("{E}a"));
    // 29: macOptionIsMeta alt+enter.
    assert_eq!(legacy(&key(TerminalKey::Enter, A)), format!("{E}\r"));
    // 27: mac alt+a is `undefined` in Keyboard.ts (the OS composes the character). Here the
    // host sets alt_is_meta = false and the character itself is sent, with no ESC.
    assert_eq!(legacy(&ch_meta('a', A, false)), "a");
    // 21, 22: alt+space and ctrl+alt+space as Character events.
    assert_eq!(legacy(&ch(' ', A)), format!("{E} "));
    assert_eq!(legacy(&ch(' ', A | CT)), format!("{E}\0"));
    // The Space KEY disagrees with Keyboard.ts (Lunegit keeps XTerm.NET's bytes): pinned.
    assert_eq!(legacy(&key(TerminalKey::Space, A)), " ");
    assert_eq!(legacy(&key(TerminalKey::Space, A | CT)), "\0");
}

#[test]
fn keyboard_ts_32_modified_function_keys() {
    let fkeys = [
        (TerminalKey::F1, "1;{}P"),
        (TerminalKey::F2, "1;{}Q"),
        (TerminalKey::F3, "1;{}R"),
        (TerminalKey::F4, "1;{}S"),
        (TerminalKey::F5, "15;{}~"),
        (TerminalKey::F6, "17;{}~"),
        (TerminalKey::F7, "18;{}~"),
        (TerminalKey::F8, "19;{}~"),
        (TerminalKey::F9, "20;{}~"),
        (TerminalKey::F10, "21;{}~"),
        (TerminalKey::F11, "23;{}~"),
        (TerminalKey::F12, "24;{}~"),
    ];
    for (m, p) in [(S, 2), (A, 3), (CT, 5)] {
        for (k, template) in fkeys {
            let want = format!("{C}{}", template.replace("{}", &p.to_string()));
            assert_eq!(legacy(&key(k, m)), want, "{k:?} {m:?}");
        }
    }
}

#[test]
fn keyboard_ts_33_ctrl_alt_a() {
    assert_eq!(legacy(&ch('a', A | CT)), format!("{E}\u{1}"));
}

/// Cases 34-54: (unshifted char, shifted char), the DOM `key` for the key code with and without
/// Shift on a US layout.
#[test]
fn keyboard_ts_34_to_54_alt_digits_and_punctuation() {
    let keys = [
        ('0', ')'),
        ('1', '!'),
        ('2', '@'),
        ('3', '#'),
        ('4', '$'),
        ('5', '%'),
        ('6', '^'),
        ('7', '&'),
        ('8', '*'),
        ('9', '('),
        (';', ':'),
        ('=', '+'),
        (',', '<'),
        ('-', '_'),
        ('.', '>'),
        ('/', '?'),
        ('`', '~'),
        ('[', '{'),
        ('\\', '|'),
        (']', '}'),
        ('\'', '"'),
    ];
    assert_eq!(keys.len(), 21);
    for (base, shifted) in keys {
        assert_eq!(
            legacy(&chs(base, A, shifted)),
            format!("{E}{base}"),
            "alt+{base}"
        );
        assert_eq!(
            legacy(&chs(base, A | S, shifted)),
            format!("{E}{shifted}"),
            "alt+shift+{base}"
        );
    }
}

#[test]
fn keyboard_ts_55_mobile_arrow_events_are_replaced_by_decckm_arrows() {
    // `key: 'UIKeyInputUpArrow'` with keyCode 0 is DOM-only. The arrows' DECCKM forms:
    for (k, normal, app) in [
        (TerminalKey::Up, 'A', "OA"),
        (TerminalKey::Left, 'D', "OD"),
        (TerminalKey::Right, 'C', "OC"),
        (TerminalKey::Down, 'B', "OB"),
    ] {
        assert_eq!(enc(&key(k, N), LEGACY), format!("{C}{normal}"));
        assert_eq!(enc(&key(k, N), LEGACY_DECCKM), format!("{E}{app}"));
    }
}

#[test]
fn keyboard_ts_56_to_59_letters_and_ctrl_at() {
    // 56 lowercase letters
    assert_eq!(legacy(&ch('a', N)), "a");
    assert_eq!(legacy(&ch('-', N)), "-");
    // 57 uppercase letters
    assert_eq!(legacy(&Ev::for_char('A', S)), "A");
    assert_eq!(legacy(&chs('1', S, '!')), "!");
    // 58 alt+shift+letter and alt+letter
    for (lower, upper) in [('a', 'A'), ('h', 'H'), ('z', 'Z')] {
        assert_eq!(legacy(&ch(lower, A | S)), format!("{E}{upper}"));
        assert_eq!(legacy(&ch(lower, A)), format!("{E}{lower}"));
    }
    // 59 ctrl+@ (ctrl+shift+2)
    assert_eq!(legacy(&chs('2', CT | S, '@')), "\0");
}
