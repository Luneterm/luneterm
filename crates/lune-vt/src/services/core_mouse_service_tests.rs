//! Tests for `core_mouse_service.rs`: the 16 tests of `CoreMouseService.test.ts` (a 300 x 100
//! terminal), plus the wheel and error paths.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use CoreMouseAction as A;
use CoreMouseButton as B;

const COLS: usize = 300;
const ROWS: usize = 100;

fn ev(col: i64, row: i64, button: B, action: A) -> CoreMouseEvent {
    CoreMouseEvent::new(col, row, button, action)
}

fn mods(button: B, action: A, ctrl: bool, alt: bool, shift: bool) -> CoreMouseEvent {
    CoreMouseEvent {
        ctrl,
        alt,
        shift,
        ..ev(0, 0, button, action)
    }
}

/// Collects the reports like the TS test's `reports` array (data and binary alike).
struct Fixture {
    cms: CoreMouseService,
    reports: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            cms: CoreMouseService::new(),
            reports: Vec::new(),
        }
    }

    /// `triggerMouseEvent`'s boolean; the report goes to `reports`.
    fn trigger(&mut self, e: CoreMouseEvent) -> bool {
        match self.cms.trigger_mouse_event(&e, COLS, ROWS) {
            None => false,
            Some(MouseReport::Data(s) | MouseReport::Binary(s)) => {
                self.reports.push(s);
                true
            }
            Some(MouseReport::Suppressed) => {
                self.reports.push(String::new());
                true
            }
        }
    }

    fn protocol(&mut self, name: &str) {
        self.cms.set_active_protocol(name).unwrap();
    }
}

fn to_bytes(s: Option<String>) -> Vec<u32> {
    s.map(|s| s.chars().map(u32::from).collect())
        .unwrap_or_default()
}

#[test]
fn init() {
    let cms = CoreMouseService::new();
    assert_eq!(cms.active_encoding(), "DEFAULT");
    assert_eq!(cms.active_protocol(), "NONE");
}

#[test]
fn default_protocols_none_x10_vt200_drag_any() {
    let cms = CoreMouseService::new();
    let names: Vec<&str> = cms.protocols.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["NONE", "X10", "VT200", "DRAG", "ANY"]);
}

#[test]
fn default_encodings_default_sgr() {
    let cms = CoreMouseService::new();
    let names: Vec<&str> = cms.encodings.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["DEFAULT", "SGR", "SGR_PIXELS"]);
}

#[test]
fn protocol_encoding_setter_reset() {
    let mut cms = CoreMouseService::new();
    cms.set_active_encoding("SGR").unwrap();
    cms.set_active_protocol("ANY").unwrap();
    assert_eq!(cms.active_encoding(), "SGR");
    assert_eq!(cms.active_protocol(), "ANY");
    cms.reset();
    assert_eq!(cms.active_encoding(), "DEFAULT");
    assert_eq!(cms.active_protocol(), "NONE");
    let err = cms.set_active_encoding("xyz").unwrap_err();
    assert_eq!(err.to_string(), "unknown encoding \"xyz\"");
    let err = cms.set_active_protocol("xyz").unwrap_err();
    assert_eq!(err.to_string(), "unknown protocol \"xyz\"");
    // a failed assignment leaves the active names alone
    assert_eq!(cms.active_encoding(), "DEFAULT");
    assert_eq!(cms.active_protocol(), "NONE");
}

#[test]
fn add_encoding() {
    let mut cms = CoreMouseService::new();
    cms.add_encoding("XYZ", Box::new(|_| String::new()));
    cms.set_active_encoding("XYZ").unwrap();
    assert_eq!(cms.active_encoding(), "XYZ");
}

#[test]
fn add_protocol() {
    let mut cms = CoreMouseService::new();
    cms.add_protocol(
        "XYZ",
        CoreMouseProtocol {
            events: CoreMouseEventType::NONE,
            restrict: Box::new(|_| false),
        },
    );
    cms.set_active_protocol("XYZ").unwrap();
    assert_eq!(cms.active_protocol(), "XYZ");
}

#[test]
fn add_protocol_replaces_in_place() {
    let mut cms = CoreMouseService::new();
    cms.add_protocol(
        "X10",
        CoreMouseProtocol {
            events: CoreMouseEventType::MOVE,
            restrict: Box::new(|_| true),
        },
    );
    assert_eq!(cms.protocols.len(), 5);
    assert_eq!(cms.protocols[1].0, "X10");
    assert_eq!(
        cms.set_active_protocol("X10").unwrap(),
        Some(CoreMouseEventType::MOVE)
    );
}

#[test]
fn on_protocol_change() {
    let mut cms = CoreMouseService::new();
    let mut wanted = Vec::new();
    wanted.push(cms.set_active_protocol("NONE").unwrap().unwrap());
    assert_eq!(wanted, [CoreMouseEventType::NONE]);
    wanted.push(cms.set_active_protocol("ANY").unwrap().unwrap());
    use CoreMouseEventType as T;
    assert_eq!(
        wanted,
        [T::NONE, T::DOWN | T::UP | T::WHEEL | T::DRAG | T::MOVE]
    );
    assert!(cms.are_mouse_events_active());
    // `reset` fires too
    assert_eq!(cms.reset(), T::NONE);
    assert!(!cms.are_mouse_events_active());
}

#[test]
fn explain_events() {
    let cms = CoreMouseService::new();
    use CoreMouseEventType as T;
    assert_eq!(
        cms.explain_events(T::DOWN | T::WHEEL),
        ExplainedEvents {
            down: true,
            up: false,
            drag: false,
            move_: false,
            wheel: true
        }
    );
    assert_eq!(
        cms.explain_events(T::NONE),
        ExplainedEvents {
            down: false,
            up: false,
            drag: false,
            move_: false,
            wheel: false
        }
    );
}

// triggerMouseEvent

#[test]
fn trigger_none() {
    let mut f = Fixture::new();
    assert!(!f.trigger(ev(0, 0, B::Left, A::Down)));
    assert!(!f.trigger(ev(0, 0, B::Left, A::Up)));
    assert!(!f.trigger(ev(0, 0, B::Left, A::Move)));
    assert!(!f.trigger(ev(0, 0, B::Middle, A::Down)));
    assert!(!f.trigger(ev(0, 0, B::Right, A::Down)));
    assert!(!f.trigger(ev(0, 0, B::Wheel, A::Up)));
    assert!(!f.trigger(ev(0, 0, B::None, A::Move)));
}

#[test]
fn trigger_x10() {
    let mut f = Fixture::new();
    f.protocol("X10");
    assert!(f.trigger(ev(0, 0, B::Left, A::Down)));
    assert!(!f.trigger(ev(0, 0, B::Left, A::Up)));
    assert!(!f.trigger(ev(0, 0, B::Left, A::Move)));
    assert!(f.trigger(ev(0, 0, B::Middle, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Right, A::Down)));
    assert!(!f.trigger(ev(0, 0, B::Wheel, A::Up)));
    assert!(!f.trigger(ev(0, 0, B::None, A::Move)));
}

#[test]
fn trigger_vt200() {
    let mut f = Fixture::new();
    f.protocol("VT200");
    assert!(f.trigger(ev(0, 0, B::Left, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Left, A::Up)));
    assert!(!f.trigger(ev(0, 0, B::Left, A::Move)));
    assert!(f.trigger(ev(0, 0, B::Middle, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Right, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Wheel, A::Up)));
    assert!(!f.trigger(ev(0, 0, B::None, A::Move)));
}

#[test]
fn trigger_drag() {
    let mut f = Fixture::new();
    f.protocol("DRAG");
    assert!(f.trigger(ev(0, 0, B::Left, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Left, A::Up)));
    assert!(f.trigger(ev(0, 0, B::Left, A::Move)));
    assert!(f.trigger(ev(0, 0, B::Middle, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Right, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Wheel, A::Up)));
}

#[test]
fn trigger_any() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    assert!(f.trigger(ev(0, 0, B::Left, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Left, A::Up)));
    assert!(f.trigger(ev(0, 0, B::Left, A::Move)));
    assert!(f.trigger(ev(0, 0, B::Middle, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Right, A::Down)));
    assert!(f.trigger(ev(0, 0, B::Wheel, A::Up)));
    assert!(f.trigger(ev(0, 0, B::None, A::Move)));
    // should not report in any case
    // invalid button + action combinations
    assert!(!f.trigger(ev(0, 0, B::Wheel, A::Move)));
    assert!(!f.trigger(ev(0, 0, B::None, A::Down)));
    assert!(!f.trigger(ev(0, 0, B::None, A::Up)));
    // invalid coords
    assert!(!f.trigger(ev(-1, 0, B::Left, A::Down)));
    assert!(!f.trigger(ev(500, 0, B::Left, A::Down)));
    assert!(!f.trigger(ev(0, -1, B::Left, A::Down)));
    assert!(!f.trigger(ev(0, 500, B::Left, A::Down)));
}

#[test]
fn coords_default_encoding() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    for i in 0..COLS as i64 {
        assert!(f.trigger(ev(i, 0, B::Left, A::Down)));
        if i > 222 {
            // supress mouse reports if we are out of addressible range (max. 222)
            assert_eq!(to_bytes(f.reports.pop()), Vec::<u32>::new());
        } else {
            assert_eq!(
                to_bytes(f.reports.pop()),
                [0x1b, 0x5b, 0x4d, 0x20, i as u32 + 33, 0x21]
            );
        }
    }
}

#[test]
fn coords_default_encoding_is_binary_and_out_of_range_is_suppressed() {
    let mut cms = CoreMouseService::new();
    cms.set_active_protocol("ANY").unwrap();
    assert_eq!(
        cms.trigger_mouse_event(&ev(0, 0, B::Left, A::Down), COLS, ROWS),
        Some(MouseReport::Binary("\x1b[M !!".to_string()))
    );
    assert_eq!(
        cms.trigger_mouse_event(&ev(250, 0, B::Left, A::Down), COLS, ROWS),
        Some(MouseReport::Suppressed)
    );
}

#[test]
fn coords_sgr_encoding() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    f.cms.set_active_encoding("SGR").unwrap();
    for i in 0..COLS as i64 {
        assert!(f.trigger(ev(i, 0, B::Left, A::Down)));
        assert_eq!(f.reports.pop().unwrap(), format!("\x1b[<0;{};1M", i + 1));
    }
    let report = f
        .cms
        .trigger_mouse_event(&ev(0, 0, B::Left, A::Down), COLS, ROWS);
    assert_eq!(report, Some(MouseReport::Data("\x1b[<0;1;1M".to_string())));
}

#[test]
fn coords_sgr_pixels_encoding() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    f.cms.set_active_encoding("SGR_PIXELS").unwrap();
    for i in 0..500 {
        let e = CoreMouseEvent {
            x: i,
            ..ev(0, 0, B::Left, A::Down)
        };
        assert!(f.trigger(e));
        assert_eq!(f.reports.pop().unwrap(), format!("\x1b[<0;{i};0M"));
    }
}

#[test]
fn event_codes_with_modifiers_default_encoding() {
    // TODO (upstream): implement AUX button tests
    let mut f = Fixture::new();
    f.protocol("ANY");
    f.cms.set_active_encoding("DEFAULT").unwrap();
    let take = |f: &mut Fixture| std::mem::take(&mut f.reports);

    // all buttons + down + no modifer
    for b in [B::Left, B::Middle, B::Right, B::Wheel] {
        assert!(f.trigger(mods(b, A::Down, false, false, false)));
    }
    assert_eq!(
        take(&mut f),
        ["\x1b[M !!", "\x1b[M!!!", "\x1b[M\"!!", "\x1b[Ma!!"]
    );

    // all buttons + up + no modifier
    for b in [B::Left, B::Middle, B::Right, B::Wheel] {
        assert!(f.trigger(mods(b, A::Up, false, false, false)));
    }
    assert_eq!(
        take(&mut f),
        ["\x1b[M#!!", "\x1b[M#!!", "\x1b[M#!!", "\x1b[M`!!"]
    );

    // all buttons + move + no modifier
    for b in [B::Left, B::Middle, B::Right, B::None] {
        assert!(f.trigger(mods(b, A::Move, false, false, false)));
    }
    assert_eq!(
        take(&mut f),
        ["\x1b[M@!!", "\x1b[MA!!", "\x1b[MB!!", "\x1b[MC!!"]
    );

    // button none + move + modifiers
    for (ctrl, alt, shift) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (true, true, false),
        (false, true, true),
        (true, true, true),
    ] {
        assert!(f.trigger(mods(B::None, A::Move, ctrl, alt, shift)));
    }
    assert_eq!(
        take(&mut f),
        [
            "\x1b[MS!!",
            "\x1b[MK!!",
            "\x1b[MG!!",
            "\x1b[M[!!",
            "\x1b[MO!!",
            "\x1b[M_!!"
        ]
    );
}

#[test]
fn identical_move_events_are_debounced() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    assert!(f.trigger(ev(3, 4, B::None, A::Move)));
    assert!(!f.trigger(ev(3, 4, B::None, A::Move)));
    assert!(f.trigger(ev(4, 4, B::None, A::Move)));
    // a non-move event is never debounced
    assert!(f.trigger(ev(4, 4, B::Left, A::Down)));
    assert!(f.trigger(ev(4, 4, B::Left, A::Down)));
    // SGR_PIXELS compares pixels, not cells
    f.cms.set_active_encoding("SGR_PIXELS").unwrap();
    let at = |x| CoreMouseEvent {
        x,
        ..ev(4, 4, B::None, A::Move)
    };
    assert!(f.trigger(at(1)));
    assert!(!f.trigger(at(1)));
    assert!(f.trigger(at(2)));
    // reset forgets the last event
    f.cms.reset();
    f.protocol("ANY");
    assert!(f.trigger(at(2)));
}

#[test]
fn x10_clears_modifiers() {
    let mut f = Fixture::new();
    f.protocol("X10");
    assert!(f.trigger(mods(B::Left, A::Down, true, true, true)));
    assert_eq!(f.reports.pop().unwrap(), "\x1b[M !!");
}

#[test]
fn wheel_left_right_actions_need_the_wheel_button() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    assert!(!f.trigger(ev(0, 0, B::Left, A::Left)));
    assert!(!f.trigger(ev(0, 0, B::Left, A::Right)));
    assert!(f.trigger(ev(0, 0, B::Wheel, A::Left)));
}

#[test]
fn sgr_release_reports_lowercase_m_except_wheel() {
    let mut f = Fixture::new();
    f.protocol("ANY");
    f.cms.set_active_encoding("SGR").unwrap();
    assert!(f.trigger(ev(0, 0, B::Right, A::Up)));
    assert!(f.trigger(ev(0, 0, B::Wheel, A::Up)));
    assert_eq!(f.reports, ["\x1b[<2;1;1m", "\x1b[<64;1;1M"]);
}

// consumeWheelEvent

fn wheel(delta_y: f64, mode: WheelDeltaMode) -> WheelEvent {
    WheelEvent {
        delta_y,
        delta_mode: mode,
        alt_key: false,
        ctrl_key: false,
        shift_key: false,
    }
}

#[test]
fn wheel_ignores_zero_delta_shift_and_missing_metrics() {
    let mut cms = CoreMouseService::new();
    let opts = TerminalOptions::default();
    let px = WheelDeltaMode::Pixel;
    assert_eq!(
        cms.consume_wheel_event(&wheel(0.0, px), Some(20.0), Some(1.0), 24, &opts),
        0
    );
    let shift = WheelEvent {
        shift_key: true,
        ..wheel(100.0, px)
    };
    assert_eq!(
        cms.consume_wheel_event(&shift, Some(20.0), Some(1.0), 24, &opts),
        0
    );
    assert_eq!(
        cms.consume_wheel_event(&wheel(100.0, px), None, Some(1.0), 24, &opts),
        0
    );
    assert_eq!(
        cms.consume_wheel_event(&wheel(100.0, px), Some(20.0), None, 24, &opts),
        0
    );
}

#[test]
fn wheel_pixel_mode_accumulates_partial_scrolls() {
    let mut cms = CoreMouseService::new();
    let opts = TerminalOptions::default();
    let px = WheelDeltaMode::Pixel;
    // a mouse wheel notch (>= 50 px): 100 / 20 = 5 lines
    assert_eq!(
        cms.consume_wheel_event(&wheel(100.0, px), Some(20.0), Some(1.0), 24, &opts),
        5
    );
    assert_eq!(
        cms.consume_wheel_event(&wheel(-100.0, px), Some(20.0), Some(1.0), 24, &opts),
        -5
    );
    // trackpad (< 50 px): 10 / 20 * 0.3 = 0.15 per event, a whole line after 7 events
    let mut total = 0;
    for _ in 0..6 {
        total += cms.consume_wheel_event(&wheel(10.0, px), Some(20.0), Some(1.0), 24, &opts);
    }
    assert_eq!(total, 0);
    assert_eq!(
        cms.consume_wheel_event(&wheel(10.0, px), Some(20.0), Some(1.0), 24, &opts),
        1
    );
    // dpr divides the cell height
    let mut cms = CoreMouseService::new();
    assert_eq!(
        cms.consume_wheel_event(&wheel(100.0, px), Some(20.0), Some(2.0), 24, &opts),
        10
    );
}

#[test]
fn wheel_reset_clears_the_partial_scroll() {
    let mut cms = CoreMouseService::new();
    let opts = TerminalOptions::default();
    let px = WheelDeltaMode::Pixel;
    cms.consume_wheel_event(&wheel(10.0, px), Some(20.0), Some(1.0), 24, &opts);
    assert!(cms.wheel_partial_scroll > 0.0);
    cms.reset();
    assert_eq!(cms.wheel_partial_scroll, 0.0);
}

#[test]
fn wheel_line_and_page_modes_and_sensitivity() {
    let mut cms = CoreMouseService::new();
    let opts = TerminalOptions::default();
    assert_eq!(
        cms.consume_wheel_event(
            &wheel(3.0, WheelDeltaMode::Line),
            Some(20.0),
            Some(1.0),
            24,
            &opts
        ),
        3
    );
    assert_eq!(
        cms.consume_wheel_event(
            &wheel(-2.0, WheelDeltaMode::Page),
            Some(20.0),
            Some(1.0),
            24,
            &opts
        ),
        -48
    );
    // modifier keys multiply by fast_scroll_sensitivity (5) x scroll_sensitivity
    let alt = WheelEvent {
        alt_key: true,
        ..wheel(2.0, WheelDeltaMode::Line)
    };
    assert_eq!(
        cms.consume_wheel_event(&alt, Some(20.0), Some(1.0), 24, &opts),
        10
    );
    let opts = TerminalOptions {
        scroll_sensitivity: 2.0,
        ..TerminalOptions::default()
    };
    assert_eq!(
        cms.consume_wheel_event(
            &wheel(3.0, WheelDeltaMode::Line),
            Some(20.0),
            Some(1.0),
            24,
            &opts
        ),
        6
    );
}
