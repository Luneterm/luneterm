//! Tests for the VS16 width provider (plan M7 step 4). Ported from Lunegit's
//! `XtermJsVtEngineVs16WidthTests.cs` (10 Fact; real xterm.js in V8 plus the `11-vs16` wrapper):
//!  - ported (10): the four `WideLeading_*` cases, `Vs16_WarningSign_OccupiesTwoCells_*`,
//!    `Vs16_BareWarningSign_StaysNarrow`, `AstralEmoji_Unchanged_StaysWide`,
//!    `Cjk_Unchanged_StaysWide`, `Vs16_AtRightEdge_WrapsWholeCluster`,
//!    `CombiningMark_Unchanged_StaysNarrow`. Lunegit's `Glyph` of a wide character's second
//!    cell is `" "`; the viewport here has `text: ""` and `width: 0` for it, and `width: 2` on
//!    the leading cell is Lunegit's `WideLeading`.
//!
//! Added: the provider-level equality with `UnicodeV11` over every code point, the install
//! switch (`vs16: false` leaves the active version alone), and chunk invariance.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::addons::unicode_v11::UnicodeV11;
use crate::engine::{Viewport, ViewportCell};
use crate::headless::terminal::Terminal;
use crate::services::options_service::OptionValue;
use crate::services::unicode_service::{UnicodeService, UnicodeVersionProvider};

fn terminal(cols: usize, vs16: bool) -> Terminal {
    let mut t = Terminal::new([OptionValue::Cols(cols), OptionValue::Rows(4)]);
    t.install_standard_extensions(&StandardExtensions {
        vs16,
        ..StandardExtensions::default()
    })
    .unwrap();
    t
}

fn view(cols: usize, input: &str) -> Viewport {
    let mut t = terminal(cols, true);
    t.write_str(input);
    t.viewport()
}

fn text(vp: &Viewport, row: usize, col: usize) -> &str {
    &vp.cells[row][col].text
}

fn cell(vp: &Viewport, row: usize, col: usize) -> &ViewportCell {
    &vp.cells[row][col]
}

fn wide_leading(vp: &Viewport, row: usize, col: usize) -> bool {
    cell(vp, row, col).width == 2
}

fn index_of(vp: &Viewport, row: usize, glyph: &str) -> usize {
    vp.cells[row].iter().position(|c| c.text == glyph).unwrap()
}

// --- XtermJsVtEngineVs16WidthTests.cs ----------------------------------------------------------

#[test]
fn wide_leading_set_on_leading_half_not_spacer_for_astral_emoji() {
    let vp = view(40, "\u{1F680}X");
    assert!(
        wide_leading(&vp, 0, 0),
        "leading half of the rocket is wide"
    );
    assert!(!wide_leading(&vp, 0, 1), "the spacer cell is not");
    assert_eq!(cell(&vp, 0, 1).width, 0);
    assert!(!wide_leading(&vp, 0, 2), "the narrow 'X' is not");
}

#[test]
fn wide_leading_not_set_on_narrow_ascii() {
    let vp = view(40, "A");
    assert!(!wide_leading(&vp, 0, 0));
}

#[test]
fn wide_leading_set_on_vs16_cluster() {
    let vp = view(40, "A\u{26A0}\u{FE0F}B");
    assert!(wide_leading(&vp, 0, 1), "the VS16-promoted cluster is wide");
    assert!(!wide_leading(&vp, 0, 2), "its spacer is not");
}

#[test]
fn wide_leading_set_on_cjk() {
    let vp = view(40, "\u{4F60}A");
    assert!(wide_leading(&vp, 0, 0));
    assert!(!wide_leading(&vp, 0, 1));
}

#[test]
fn vs16_warning_sign_occupies_two_cells_reported_symptom() {
    let vp = view(40, "A\u{26A0}\u{FE0F}B|\r\nAxB|");

    // row 0: A, warning sign + VS16, spacer, B, |
    assert_eq!(text(&vp, 0, 0), "A");
    assert_eq!(text(&vp, 0, 1), "\u{26A0}\u{FE0F}");
    assert_eq!(cell(&vp, 0, 1).width, 2);
    assert_eq!(text(&vp, 0, 2), "");
    assert_eq!(cell(&vp, 0, 2).width, 0);
    assert_eq!(text(&vp, 0, 3), "B");
    assert_eq!(text(&vp, 0, 4), "|");

    // row 1: A x B |
    assert_eq!(text(&vp, 1, 0), "A");
    assert_eq!(text(&vp, 1, 1), "x");
    assert_eq!(text(&vp, 1, 2), "B");
    assert_eq!(text(&vp, 1, 3), "|");

    // the '|' on row 0 sits exactly one column right of row 1's
    assert_eq!(index_of(&vp, 1, "|") + 1, index_of(&vp, 0, "|"));
}

#[test]
fn vs16_bare_warning_sign_stays_narrow() {
    let vp = view(40, "A\u{26A0}B|");
    assert_eq!(text(&vp, 0, 0), "A");
    assert_eq!(text(&vp, 0, 1), "\u{26A0}");
    assert_eq!(cell(&vp, 0, 1).width, 1);
    assert_eq!(text(&vp, 0, 2), "B");
    assert_eq!(text(&vp, 0, 3), "|");
}

#[test]
fn astral_emoji_unchanged_stays_wide() {
    let vp = view(40, "\u{1F680}X");
    assert_eq!(text(&vp, 0, 0), "\u{1F680}");
    assert_eq!(text(&vp, 0, 1), "");
    assert_eq!(text(&vp, 0, 2), "X");
}

#[test]
fn cjk_unchanged_stays_wide() {
    let vp = view(40, "\u{4F60}\u{597D}AB");
    assert_eq!(text(&vp, 0, 0), "\u{4F60}");
    assert_eq!(text(&vp, 0, 1), "");
    assert_eq!(text(&vp, 0, 2), "\u{597D}");
    assert_eq!(text(&vp, 0, 3), "");
    assert_eq!(text(&vp, 0, 4), "A");
}

#[test]
fn vs16_at_right_edge_wraps_whole_cluster() {
    let vp = view(6, "ABCDE\u{26A0}\u{FE0F}XY");
    // the width-1 col 5 cannot hold a width-2 cluster: it is left blank and the cluster wraps
    assert_eq!(text(&vp, 0, 5), "");
    assert_eq!(text(&vp, 1, 0), "\u{26A0}\u{FE0F}");
    assert_eq!(cell(&vp, 1, 0).width, 2);
    assert_eq!(text(&vp, 1, 1), "");
    assert_eq!(text(&vp, 1, 2), "X");
    assert_eq!(text(&vp, 1, 3), "Y");
}

#[test]
fn combining_mark_unchanged_stays_narrow() {
    let vp = view(40, "a\u{0301}B");
    assert_eq!(text(&vp, 0, 0), "a\u{0301}");
    assert_eq!(cell(&vp, 0, 0).width, 1);
    assert_eq!(text(&vp, 0, 1), "B");
}

// --- added ------------------------------------------------------------------------------------

/// Without the install switch the behaviour is Unicode 6 (the terminal default) or whatever the
/// host chose: VS16 does not widen anything.
#[test]
fn vs16_false_registers_nothing_and_leaves_the_active_version() {
    let mut t = terminal(40, false);
    assert_eq!(t.unicode_mut().active_version(), "6");
    assert_eq!(t.unicode_mut().versions(), ["6", "11"]);
    t.unicode_mut().set_active_version("11").unwrap();
    t.write_str("A\u{26A0}\u{FE0F}B");
    let vp = t.viewport();
    assert_eq!(cell(&vp, 0, 1).width, 1, "plain Unicode 11 keeps one cell");
    assert_eq!(text(&vp, 0, 2), "B");
}

#[test]
fn vs16_true_registers_and_activates_the_version_and_the_host_can_switch_away() {
    let mut t = terminal(40, true);
    assert_eq!(t.unicode_mut().active_version(), "11-vs16");
    assert_eq!(t.unicode_mut().versions(), ["6", "11", "11-vs16"]);
    t.unicode_mut().set_active_version("11").unwrap();
    t.write_str("\u{26A0}\u{FE0F}");
    assert_eq!(cell(&t.viewport(), 0, 0).width, 1);
}

#[test]
fn vs16_is_chunk_invariant() {
    let input = "A\u{26A0}\u{FE0F}B\u{4F60}\u{FE0F}C\u{1F680}\u{FE0F}\r\n\u{26A0}\u{FE0F}\u{FE0F}x";
    let whole = view(7, input);
    let mut t = terminal(7, true);
    for byte in input.bytes() {
        t.write(&[byte]);
    }
    assert_eq!(t.viewport(), whole);
}

#[test]
fn vs16_after_wide_or_zero_width_does_nothing_extra() {
    // CJK + VS16 and a leading VS16 render as plain Unicode 11 renders them
    for input in ["\u{4F60}\u{FE0F}A", "\u{FE0F}A", "\u{1F680}\u{FE0F}A"] {
        let mut plain = terminal(40, false);
        plain.unicode_mut().set_active_version("11").unwrap();
        plain.write_str(input);
        assert_eq!(view(40, input), plain.viewport(), "{input:?}");
    }
    assert_eq!(cell(&view(40, "\u{4F60}\u{FE0F}A"), 0, 0).width, 2);
}

/// The provider equals `UnicodeV11` for every code point and every preceding state, except a
/// U+FE0F that joins a width-1 character, which becomes width 2.
#[test]
fn provider_equals_unicode_v11_except_vs16_after_a_width_one_character() {
    let base = UnicodeV11::new();
    let provider = UnicodeV11Vs16::new();
    assert_eq!(provider.version(), "11-vs16");

    // instrument: the base reports U+FE0F as width 0, so the exception below is exercised
    assert_eq!(base.wcwidth(0xFE0F), 0);
    // the corpus contains the interesting classes
    let width_of = |cp| base.wcwidth(cp);
    assert_eq!(
        (
            width_of('A' as u32),
            width_of(0x4F60),
            width_of(0x1F680),
            width_of(0x0301),
            width_of(0x26A0)
        ),
        (1, 2, 2, 0, 1)
    );

    let preceding_states: Vec<u32> = [0u32, 0x26A0, 0x4F60, 0x1F680, 0x0301, 0x41, 0xFE0F]
        .iter()
        .map(|&cp| base.char_properties(cp, 0))
        .chain([0])
        .collect();
    let mut promoted = 0u32;
    for cp in 0..0x11_0000u32 {
        assert_eq!(provider.wcwidth(cp), base.wcwidth(cp), "wcwidth {cp:#x}");
        for &preceding in &preceding_states {
            let expected = base.char_properties(cp, preceding);
            let actual = provider.char_properties(cp, preceding);
            if cp == 0xFE0F
                && UnicodeService::extract_should_join(expected)
                && UnicodeService::extract_width(expected) == 1
            {
                promoted += 1;
                assert_eq!(UnicodeService::extract_width(actual), 2, "{preceding:#x}");
                assert!(UnicodeService::extract_should_join(actual));
                assert_eq!(
                    UnicodeService::extract_char_kind(actual),
                    UnicodeService::extract_char_kind(expected)
                );
            } else {
                assert_eq!(actual, expected, "{cp:#x} after {preceding:#x}");
            }
        }
    }
    // FE0F after the width-1 states (U+26A0 and 'A') were promoted; none of the others
    assert_eq!(promoted, 2);
}

/// U+FE0F after every code point: promoted exactly when the base character is one cell wide.
#[test]
fn vs16_after_each_code_point_is_promoted_exactly_when_the_base_is_one_cell_wide() {
    let base = UnicodeV11::new();
    let provider = UnicodeV11Vs16::new();
    let mut promoted_bases = 0u32;
    for cp in 0..0x11_0000u32 {
        let preceding = base.char_properties(cp, 0);
        let expected = base.char_properties(0xFE0F, preceding);
        let actual = provider.char_properties(0xFE0F, preceding);
        match base.wcwidth(cp) {
            1 => {
                promoted_bases += 1;
                assert_eq!(UnicodeService::extract_width(actual), 2, "{cp:#x}");
                assert!(UnicodeService::extract_should_join(actual), "{cp:#x}");
            }
            _ => assert_eq!(actual, expected, "{cp:#x}"),
        }
    }
    assert!(promoted_bases > 100_000, "{promoted_bases}");
}
