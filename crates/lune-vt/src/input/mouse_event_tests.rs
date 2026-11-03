//! Tests for `Terminal::mouse_event` and the event mapping (plan M7 step 6, D5). The encodings
//! themselves are upstream's `CoreMouseService` (16 tests in `services/core_mouse_service_tests.rs`);
//! these go through the public API after the DECSET sequences an application sends, and pin the
//! coordinate convention.
//!
//! Lunegit's `XtermJsEngineParityTests.MouseInput_SgrEncoding_MatchesXtermDotNet_AfterEnablingTrackingAndSgr`
//! (`?1002h ?1006h`, then press, drag, release) is adapted as `lunegit_sgr_parity_gesture`: its
//! expected bytes are what Lunegit's `GenerateSgrSequence` returns (the cell is zero-based, the
//! report one-based), computed from `XtermVtInputEncoder.cs:286-306`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::headless::terminal::Terminal;
use crate::services::options_service::OptionValue;

const ESC: &str = "\u{1b}";

type B = TerminalMouseButton;
type K = TerminalMouseEventType;
type Ev = TerminalMouseEvent;

fn term() -> Terminal {
    Terminal::new([OptionValue::Cols(80), OptionValue::Rows(24)])
}

fn with(sequences: &str) -> Terminal {
    let mut t = term();
    t.write_str(sequences);
    t
}

fn ev(button: B, kind: K, col: i32, row: i32) -> Ev {
    Ev::new(button, kind, col, row)
}

fn text(bytes: Option<Vec<u8>>) -> Option<String> {
    bytes.map(|b| String::from_utf8(b).unwrap())
}

fn mods(shift: bool, alt: bool, ctrl: bool) -> TerminalModifiers {
    let mut m = TerminalModifiers::NONE;
    if shift {
        m |= TerminalModifiers::SHIFT;
    }
    if alt {
        m |= TerminalModifiers::ALT;
    }
    if ctrl {
        m |= TerminalModifiers::CONTROL;
    }
    m
}

// ---- no tracking ------------------------------------------------------------------------------

#[test]
fn without_tracking_nothing_is_sent() {
    let mut t = term();
    for kind in [K::Down, K::Up, K::Move, K::Drag, K::WheelUp, K::WheelDown] {
        assert_eq!(t.mouse_event(&ev(B::Left, kind, 0, 0)), None, "{kind:?}");
    }
}

#[test]
fn tracking_off_again_sends_nothing() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    assert!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)).is_some());
    t.write_str(&format!("{ESC}[?1000l"));
    assert_eq!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)), None);
}

// ---- VT200 + SGR ------------------------------------------------------------------------------

#[test]
fn sgr_press_release_and_wheel() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    let sgr = |t: &mut Terminal, e: Ev| text(t.mouse_event(&e));
    assert_eq!(
        sgr(&mut t, ev(B::Left, K::Down, 0, 0)).as_deref(),
        Some("\u{1b}[<0;1;1M")
    );
    assert_eq!(
        sgr(&mut t, ev(B::Left, K::Up, 0, 0)).as_deref(),
        Some("\u{1b}[<0;1;1m")
    );
    assert_eq!(
        sgr(&mut t, ev(B::Middle, K::Down, 2, 1)).as_deref(),
        Some("\u{1b}[<1;3;2M")
    );
    assert_eq!(
        sgr(&mut t, ev(B::Right, K::Down, 79, 23)).as_deref(),
        Some("\u{1b}[<2;80;24M")
    );
    assert_eq!(
        sgr(&mut t, ev(B::None, K::WheelUp, 4, 4)).as_deref(),
        Some("\u{1b}[<64;5;5M")
    );
    assert_eq!(
        sgr(&mut t, ev(B::None, K::WheelDown, 4, 4)).as_deref(),
        Some("\u{1b}[<65;5;5M")
    );
    // The wheel kinds ignore `button`.
    assert_eq!(
        sgr(&mut t, ev(B::Left, K::WheelUp, 4, 4)).as_deref(),
        Some("\u{1b}[<64;5;5M")
    );
}

#[test]
fn the_coordinate_convention_is_zero_based_cells_reported_one_based() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    let report = text(t.mouse_event(&ev(B::Left, K::Down, 5, 3))).unwrap();
    assert_eq!(report, "\u{1b}[<0;6;4M");
}

#[test]
fn modifiers_add_shift_4_alt_8_ctrl_16() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    for (shift, alt, ctrl, code) in [
        (true, false, false, 4),
        (false, true, false, 8),
        (false, false, true, 16),
        (true, true, false, 12),
        (true, false, true, 20),
        (false, true, true, 24),
        (true, true, true, 28),
    ] {
        let mut e = ev(B::Left, K::Down, 0, 0);
        e.modifiers = mods(shift, alt, ctrl);
        assert_eq!(
            text(t.mouse_event(&e)).unwrap(),
            format!("\u{1b}[<{code};1;1M")
        );
    }
    // Unknown modifier bits are ignored.
    let mut e = ev(B::Left, K::Down, 0, 0);
    e.modifiers = TerminalModifiers::from_bits(0xF8);
    assert_eq!(text(t.mouse_event(&e)).unwrap(), "\u{1b}[<0;1;1M");
}

#[test]
fn motion_is_reported_only_under_1002_and_1003() {
    let drag = ev(B::Left, K::Drag, 1, 1);
    let mv = ev(B::None, K::Move, 1, 1);

    // VT200 (1000): no motion at all.
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    assert_eq!(t.mouse_event(&drag), None);
    assert_eq!(t.mouse_event(&mv), None);

    // Drag (1002): motion with a button only.
    let mut t = with(&format!("{ESC}[?1002h{ESC}[?1006h"));
    assert_eq!(text(t.mouse_event(&drag)).unwrap(), "\u{1b}[<32;2;2M");
    assert_eq!(t.mouse_event(&mv), None);

    // Any (1003): both. A move with no button is code 35 (32 + 3).
    let mut t = with(&format!("{ESC}[?1003h{ESC}[?1006h"));
    assert_eq!(text(t.mouse_event(&drag)).unwrap(), "\u{1b}[<32;2;2M");
    assert_eq!(text(t.mouse_event(&mv)).unwrap(), "\u{1b}[<35;2;2M");
}

#[test]
fn lunegit_sgr_parity_gesture() {
    let mut t = with(&format!("{ESC}[?1002h{ESC}[?1006h"));
    assert_eq!(
        text(t.mouse_event(&ev(B::Left, K::Down, 5, 3))).unwrap(),
        "\u{1b}[<0;6;4M"
    );
    assert_eq!(
        text(t.mouse_event(&ev(B::Left, K::Drag, 6, 3))).unwrap(),
        "\u{1b}[<32;7;4M"
    );
    assert_eq!(
        text(t.mouse_event(&ev(B::Left, K::Up, 6, 3))).unwrap(),
        "\u{1b}[<0;7;4m"
    );
    assert_eq!(t.modes().mouse_tracking, crate::engine::MouseTracking::Drag);
}

// ---- other protocols and encodings ------------------------------------------------------------

#[test]
fn x10_reports_presses_only_and_clears_modifiers() {
    let mut t = with(&format!("{ESC}[?9h{ESC}[?1006h"));
    let mut press = ev(B::Left, K::Down, 0, 0);
    press.modifiers = mods(true, true, true);
    assert_eq!(text(t.mouse_event(&press)).unwrap(), "\u{1b}[<0;1;1M");
    assert_eq!(t.mouse_event(&ev(B::Left, K::Up, 0, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::None, K::WheelUp, 0, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::Left, K::Drag, 0, 0)), None);
}

#[test]
fn default_encoding_returns_raw_bytes() {
    let mut t = Terminal::new([OptionValue::Cols(200), OptionValue::Rows(24)]);
    t.write_str(&format!("{ESC}[?1000h"));
    // CSI M, then 32 + code, 32 + col + 1, 32 + row + 1.
    assert_eq!(
        t.mouse_event(&ev(B::Left, K::Down, 0, 0)).unwrap(),
        [0x1b, b'[', b'M', 32, 33, 33]
    );
    // A release without SGR carries button 3.
    assert_eq!(
        t.mouse_event(&ev(B::Left, K::Up, 0, 0)).unwrap(),
        [0x1b, b'[', b'M', 35, 33, 33]
    );
    // Column 100 is 32 + 101 = 133: one byte above 0x7F, not its UTF-8 encoding.
    assert_eq!(
        t.mouse_event(&ev(B::Left, K::Down, 100, 0)).unwrap(),
        [0x1b, b'[', b'M', 32, 133, 33]
    );
}

#[test]
fn default_encoding_sends_nothing_past_column_or_row_222() {
    let mut t = Terminal::new([OptionValue::Cols(300), OptionValue::Rows(300)]);
    t.write_str(&format!("{ESC}[?1000h"));
    assert!(t.mouse_event(&ev(B::Left, K::Down, 222, 0)).is_some());
    assert_eq!(t.mouse_event(&ev(B::Left, K::Down, 223, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::Left, K::Down, 0, 223)), None);
    // SGR has no limit.
    t.write_str(&format!("{ESC}[?1006h"));
    assert_eq!(
        text(t.mouse_event(&ev(B::Left, K::Down, 299, 299))).unwrap(),
        "\u{1b}[<0;300;300M"
    );
}

#[test]
fn sgr_pixels_reports_the_pixel_position() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1016h"));
    let mut e = ev(B::Left, K::Down, 1, 1);
    e.x = 123;
    e.y = 45;
    assert_eq!(text(t.mouse_event(&e)).unwrap(), "\u{1b}[<0;123;45M");
    e.kind = K::Up;
    assert_eq!(text(t.mouse_event(&e)).unwrap(), "\u{1b}[<0;123;45m");
    assert_eq!(
        t.modes().mouse_encoding,
        crate::engine::MouseEncoding::SgrPixels
    );
}

#[test]
fn a_position_outside_the_terminal_sends_nothing() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    assert!(t.mouse_event(&ev(B::Left, K::Down, 79, 23)).is_some());
    for (col, row) in [
        (80, 0),
        (0, 24),
        (-1, 0),
        (0, -1),
        (i32::MAX, 0),
        (0, i32::MIN),
    ] {
        assert_eq!(
            t.mouse_event(&ev(B::Left, K::Down, col, row)),
            None,
            "{col},{row}"
        );
    }
}

#[test]
fn nonsense_combinations_send_nothing() {
    let mut t = with(&format!("{ESC}[?1003h{ESC}[?1006h"));
    assert_eq!(t.mouse_event(&ev(B::None, K::Down, 0, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::None, K::Up, 0, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::WheelUp, K::Down, 0, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::WheelDown, K::Up, 0, 0)), None);
    assert_eq!(t.mouse_event(&ev(B::WheelUp, K::Drag, 0, 0)), None);
}

#[test]
fn to_core_maps_every_kind() {
    use crate::services::core_mouse_service::{CoreMouseAction as A, CoreMouseButton as C};
    let core = |b, k| ev(b, k, 3, 4).to_core().map(|c| (c.button, c.action));
    assert_eq!(core(B::Left, K::Down), Some((C::Left, A::Down)));
    assert_eq!(core(B::Middle, K::Up), Some((C::Middle, A::Up)));
    assert_eq!(core(B::Right, K::Drag), Some((C::Right, A::Move)));
    assert_eq!(core(B::None, K::Drag), Some((C::None, A::Move)));
    assert_eq!(core(B::Left, K::Move), Some((C::None, A::Move)));
    assert_eq!(core(B::None, K::WheelUp), Some((C::Wheel, A::Up)));
    assert_eq!(core(B::None, K::WheelDown), Some((C::Wheel, A::Down)));
    assert_eq!(core(B::None, K::Down), None);
    assert_eq!(core(B::WheelUp, K::Up), None);
}

// ---- user-input semantics ---------------------------------------------------------------------

#[test]
fn disable_stdin_sends_nothing_for_both_encodings() {
    let mut t = with(&format!("{ESC}[?1000h"));
    t.set_option(OptionValue::DisableStdin(true)).unwrap();
    assert_eq!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)), None); // default (binary)
    t.write_str(&format!("{ESC}[?1006h"));
    assert_eq!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)), None); // SGR (data)
    t.set_option(OptionValue::DisableStdin(false)).unwrap();
    assert!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)).is_some());
}

fn scrolled_up_with_sgr_tracking() -> Terminal {
    let mut t = Terminal::new([OptionValue::Cols(20), OptionValue::Rows(4)]);
    for i in 0..30 {
        t.write_str(&format!("line {i}\r\n"));
    }
    t.write_str(&format!("{ESC}[?1000h{ESC}[?1006h"));
    t.scroll_lines(-10);
    assert!(!t.viewport().is_at_bottom);
    t
}

#[test]
fn an_sgr_report_scrolls_to_the_bottom_like_user_input() {
    let mut t = scrolled_up_with_sgr_tracking();
    assert!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)).is_some());
    assert!(t.viewport().is_at_bottom);
}

#[test]
fn a_default_encoding_report_does_not_scroll() {
    // Upstream sends it through triggerBinaryEvent, which has no scroll.
    let mut t = scrolled_up_with_sgr_tracking();
    t.write_str(&format!("{ESC}[?1006l"));
    assert!(t.mouse_event(&ev(B::Left, K::Down, 0, 0)).is_some());
    assert!(!t.viewport().is_at_bottom);
}

#[test]
fn mouse_reports_do_not_queue_host_events() {
    let mut t = with(&format!("{ESC}[?1000h{ESC}[?1006h"));
    t.drain_events();
    t.mouse_event(&ev(B::Left, K::Down, 0, 0)).unwrap();
    assert!(t.drain_events().is_empty());
}
