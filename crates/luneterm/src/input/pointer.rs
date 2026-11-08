//! The mouse and the wheel (plan M9 D7): winit pointer events mapped to terminal mouse reports,
//! local scrolling and alt-screen arrows. Pure functions and two small accumulators; the winit
//! glue is [`pointer_event`] and [`wheel_delta`].
//!
//! Ported from Lunegit's `TerminalView` (wheel :1604-1719, Shift bypass :1363) and xterm.js's
//! `CoreMouseService.consumeWheelEvent` (:241-277). One deliberate difference from Lunegit (D7):
//! on the alt screen, or with no scrollback, with mouse tracking off, the wheel types an arrow
//! instead of doing nothing, as xterm.js does, so `less` and `man` scroll.

use lune_termview::GridLayout;
use lune_vt::{TerminalModifiers, TerminalMouseButton, TerminalMouseEvent, TerminalMouseEventType};
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

/// Wheel reports sent for one event at most (Lunegit `MaxWheelReportsPerEvent`).
pub const MAX_WHEEL_REPORTS: u32 = 32;
/// Scrollback lines per wheel notch (Lunegit `WheelLinesPerNotch`).
pub const LINES_PER_NOTCH: f64 = 3.0;
/// xterm.js scales a pixel delta under 50 px, a trackpad's, by this.
const TRACKPAD_SCALE: f64 = 0.3;
const TRACKPAD_LIMIT_PX: f64 = 50.0;
/// A line count past this is a garbage delta.
const MAX_LINES: f64 = 100_000.0;

/// What the pointer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvent {
    Down(TerminalMouseButton),
    Up(TerminalMouseButton),
    Move,
}

/// The glue for a button event: `None` for a button a terminal has no report for.
pub fn pointer_event(state: ElementState, button: MouseButton) -> Option<PointerEvent> {
    let b = match button {
        MouseButton::Left => TerminalMouseButton::Left,
        MouseButton::Middle => TerminalMouseButton::Middle,
        MouseButton::Right => TerminalMouseButton::Right,
        _ => return None,
    };
    Some(if state.is_pressed() {
        PointerEvent::Down(b)
    } else {
        PointerEvent::Up(b)
    })
}

/// Where the grid is, for mapping a pointer position to cells and pixels.
#[derive(Debug, Clone, Copy)]
pub struct MouseCtx {
    pub layout: GridLayout,
    pub cols: usize,
    pub rows: usize,
    /// The program asked for SGR-pixel reports (`?1016`): a move inside one cell is then still
    /// news.
    pub pixel_reports: bool,
}

/// What the mouse mapping remembers between events.
#[derive(Debug, Clone, Copy, Default)]
pub struct MouseState {
    last_cell: Option<(usize, usize)>,
    last_px: Option<(i32, i32)>,
    /// The button of the last reported press, until its release: motion is a drag meanwhile.
    held: Option<TerminalMouseButton>,
    /// A press with Shift held belongs to the app (selection, D7): so does its release.
    bypassed: bool,
}

/// The cell and the pixel (physical, relative to the grid origin) under a logical position, both
/// clamped into the grid.
fn locate(ctx: &MouseCtx, pos: (f32, f32)) -> ((usize, usize), (i32, i32)) {
    let (row, col) = ctx.layout.cell_at_clamped(pos, ctx.cols, ctx.rows);
    let m = &ctx.layout.metrics;
    let scale = if m.scale.is_finite() && m.scale > 0.0 {
        f64::from(m.scale)
    } else {
        1.0
    };
    let extent = |cells: usize, size: u32| {
        i64::try_from(cells)
            .unwrap_or(i64::MAX)
            .saturating_mul(i64::from(size))
            .saturating_sub(1)
            .max(0)
    };
    let px = |v: f32, origin: i32, cells: usize, size: u32| -> i32 {
        let rel = (f64::from(v) * scale).round() - f64::from(origin);
        let rel = if rel.is_finite() { rel as i64 } else { 0 };
        i32::try_from(rel.clamp(0, extent(cells, size))).unwrap_or(0)
    };
    (
        (row, col),
        (
            px(pos.0, ctx.layout.origin_px.0, ctx.cols, m.cell_w),
            px(pos.1, ctx.layout.origin_px.1, ctx.rows, m.cell_h),
        ),
    )
}

fn event(
    ctx: &MouseCtx,
    pos: (f32, f32),
    button: TerminalMouseButton,
    kind: TerminalMouseEventType,
    mods: TerminalModifiers,
) -> TerminalMouseEvent {
    let ((row, col), (x, y)) = locate(ctx, pos);
    TerminalMouseEvent {
        button,
        kind,
        col: i32::try_from(col).unwrap_or(i32::MAX),
        row: i32::try_from(row).unwrap_or(i32::MAX),
        x,
        y,
        modifiers: mods,
    }
}

/// A terminal mouse report for a pointer event at `pos` (logical px in the window), or `None` when
/// the app keeps it: Shift bypasses reporting (D7), and a move that did not leave its cell is not
/// news (unless the program asked for pixels). Cells are zero-based and clamped into the grid.
/// The caller applies the mode gate (tracking on); the engine drops what its protocol excludes.
pub fn mouse_to_terminal(
    state: &mut MouseState,
    ev: PointerEvent,
    pos: (f32, f32),
    mods: TerminalModifiers,
    ctx: &MouseCtx,
) -> Option<TerminalMouseEvent> {
    use TerminalMouseEventType as K;
    let shift = mods.contains(TerminalModifiers::SHIFT);
    match ev {
        PointerEvent::Down(button) => {
            if shift {
                state.bypassed = true;
                return None;
            }
            state.bypassed = false;
            state.held = Some(button);
            let out = event(ctx, pos, button, K::Down, mods);
            state.last_cell = Some((out.row as usize, out.col as usize));
            state.last_px = Some((out.x, out.y));
            Some(out)
        }
        PointerEvent::Up(button) => {
            if std::mem::take(&mut state.bypassed) {
                return None;
            }
            state.held = None;
            // The release reports at its position even when Shift is down now: the press was ours.
            Some(event(ctx, pos, button, K::Up, mods))
        }
        PointerEvent::Move => {
            if shift || state.bypassed {
                return None;
            }
            let out = match state.held {
                Some(b) => event(ctx, pos, b, K::Drag, mods),
                None => event(ctx, pos, TerminalMouseButton::None, K::Move, mods),
            };
            let cell = (out.row as usize, out.col as usize);
            let moved = state.last_cell != Some(cell);
            let pixel_moved = ctx.pixel_reports && state.last_px != Some((out.x, out.y));
            if !moved && !pixel_moved {
                return None;
            }
            state.last_cell = Some(cell);
            state.last_px = Some((out.x, out.y));
            Some(out)
        }
    }
}

/// One wheel report at `pos`: up or down, with the modifiers held.
pub fn wheel_report(
    ctx: &MouseCtx,
    pos: (f32, f32),
    up: bool,
    mods: TerminalModifiers,
) -> TerminalMouseEvent {
    use TerminalMouseEventType as K;
    if up {
        event(ctx, pos, TerminalMouseButton::WheelUp, K::WheelUp, mods)
    } else {
        event(ctx, pos, TerminalMouseButton::WheelDown, K::WheelDown, mods)
    }
}

/// A wheel delta; positive is up, toward history (winit: the content moves down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelDelta {
    /// Notches (or fractions of one).
    Lines(f64),
    /// Physical pixels.
    Pixels(f64),
}

/// The glue: the vertical part of winit's delta, or `None` for no vertical movement.
pub fn wheel_delta(delta: MouseScrollDelta) -> Option<WheelDelta> {
    let d = match delta {
        MouseScrollDelta::LineDelta(_, y) => WheelDelta::Lines(f64::from(y)),
        MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels(p.y),
    };
    match d {
        WheelDelta::Lines(v) | WheelDelta::Pixels(v) if v == 0.0 || !v.is_finite() => None,
        d => Some(d),
    }
}

/// What the wheel is routed by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelCtx {
    /// A mouse tracking mode is on.
    pub tracking: bool,
    pub shift: bool,
    /// The alt screen is showing, or there is no scrollback: nothing to scroll locally.
    pub no_scrollback: bool,
    /// One cell's height in the pixels of [`WheelDelta::Pixels`] (physical).
    pub cell_h: f32,
}

/// What a wheel event does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelAction {
    /// Send `n` wheel reports (1..=32).
    Reports(TerminalMouseButton, u32),
    /// Scroll the viewport by lines; negative is up, into history.
    Scroll(i64),
    /// Type one arrow key (the engine picks the sequence by DECCKM).
    Arrows {
        up: bool,
    },
    None,
}

/// The remainders the wheel keeps between events, one per path so a mode switch does not leak
/// one path's movement into another.
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelState {
    reports: f64,
    scroll: f64,
    arrows: f64,
}

/// Takes the whole part of `acc` (truncating toward zero) and keeps the rest.
fn take_whole(acc: &mut f64) -> f64 {
    let whole = acc.trunc();
    *acc -= whole;
    whole
}

/// Maps one wheel event (D7). In order:
/// - tracking on and no Shift: wheel reports, one per whole notch (a pixel delta counts a cell
///   height as one), at most 32;
/// - no scrollback (or the alt screen): one arrow key when xterm.js's line count is non-zero;
///   Shift counts zero, a pixel delta is divided by the cell height and scaled by 0.3 when under
///   50 px (a trackpad), the remainder is kept;
/// - otherwise a local scroll: 3 lines per notch, a pixel delta divided by the cell height, the
///   remainder kept.
pub fn wheel(state: &mut WheelState, delta: WheelDelta, ctx: &WheelCtx) -> WheelAction {
    let cell_h = f64::from(ctx.cell_h);
    if matches!(delta, WheelDelta::Pixels(_)) && !(cell_h.is_finite() && cell_h > 0.0) {
        return WheelAction::None;
    }
    let up = match delta {
        WheelDelta::Lines(v) | WheelDelta::Pixels(v) => v > 0.0,
    };
    // The delta in whole-notch units (a cell height of pixels is one).
    let notches = match delta {
        WheelDelta::Lines(v) => v,
        WheelDelta::Pixels(v) => v / cell_h,
    };

    if ctx.tracking && !ctx.shift {
        state.reports += notches;
        let n = take_whole(&mut state.reports)
            .abs()
            .min(f64::from(MAX_WHEEL_REPORTS)) as u32;
        if n == 0 {
            return WheelAction::None;
        }
        let button = if up {
            TerminalMouseButton::WheelUp
        } else {
            TerminalMouseButton::WheelDown
        };
        return WheelAction::Reports(button, n);
    }

    if ctx.no_scrollback {
        if ctx.shift {
            return WheelAction::None;
        }
        let amount = match delta {
            WheelDelta::Lines(v) => v,
            WheelDelta::Pixels(v) => {
                let a = v / cell_h;
                if v.abs() < TRACKPAD_LIMIT_PX {
                    a * TRACKPAD_SCALE
                } else {
                    a
                }
            }
        };
        state.arrows += amount;
        let whole = take_whole(&mut state.arrows);
        return if whole == 0.0 {
            WheelAction::None
        } else {
            WheelAction::Arrows { up: whole > 0.0 }
        };
    }

    let lines = match delta {
        WheelDelta::Lines(v) => v * LINES_PER_NOTCH,
        WheelDelta::Pixels(_) => notches,
    };
    state.scroll += lines;
    let whole = take_whole(&mut state.scroll).clamp(-MAX_LINES, MAX_LINES);
    if whole == 0.0 {
        WheelAction::None
    } else {
        // Positive deltas scroll up, which is negative lines.
        WheelAction::Scroll(-(whole as i64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lune_termview::CellMetrics;

    const NONE: TerminalModifiers = TerminalModifiers::NONE;
    const SHIFT: TerminalModifiers = TerminalModifiers::SHIFT;

    /// 8 x 17 cells at scale 1, the grid at (20, 44), 100 x 30.
    fn ctx_at(scale: f32) -> MouseCtx {
        let m = CellMetrics {
            cell_w: (8.0 * scale) as u32,
            cell_h: (17.0 * scale) as u32,
            char_h: 15,
            char_top: 1,
            baseline: 13,
            font_size: 13.0,
            scale,
        };
        MouseCtx {
            layout: GridLayout::new((20.0, 44.0), m),
            cols: 100,
            rows: 30,
            pixel_reports: false,
        }
    }

    fn cell(
        col: i32,
        row: i32,
        button: TerminalMouseButton,
        kind: TerminalMouseEventType,
    ) -> (i32, i32, TerminalMouseButton, TerminalMouseEventType) {
        (col, row, button, kind)
    }

    fn parts(e: TerminalMouseEvent) -> (i32, i32, TerminalMouseButton, TerminalMouseEventType) {
        (e.col, e.row, e.button, e.kind)
    }

    use TerminalMouseButton as B;
    use TerminalMouseEventType as K;

    #[test]
    fn a_press_reports_its_zero_based_cell_and_pixel() {
        let ctx = ctx_at(1.0);
        let mut st = MouseState::default();
        // (20 + 8*5 + 3, 44 + 17*3 + 4) is column 5, row 3, pixel (43, 55).
        let e = mouse_to_terminal(
            &mut st,
            PointerEvent::Down(B::Left),
            (63.0, 99.0),
            NONE,
            &ctx,
        )
        .expect("press");
        assert_eq!(parts(e), cell(5, 3, B::Left, K::Down));
        assert_eq!((e.x, e.y), (43, 55));
        // At scale 2 the pixel is physical.
        let ctx2 = ctx_at(2.0);
        let e = mouse_to_terminal(
            &mut MouseState::default(),
            PointerEvent::Down(B::Right),
            (63.0, 99.0),
            NONE,
            &ctx2,
        )
        .expect("press");
        assert_eq!(parts(e), cell(5, 3, B::Right, K::Down));
        assert_eq!((e.x, e.y), (86, 110));
        let up = mouse_to_terminal(&mut st, PointerEvent::Up(B::Left), (63.0, 99.0), NONE, &ctx)
            .expect("release");
        assert_eq!(parts(up), cell(5, 3, B::Left, K::Up));
    }

    #[test]
    fn shift_bypasses_reporting_for_the_whole_gesture() {
        let ctx = ctx_at(1.0);
        let mut st = MouseState::default();
        let at = (63.0, 99.0);
        assert_eq!(
            mouse_to_terminal(&mut st, PointerEvent::Down(B::Left), at, SHIFT, &ctx),
            None
        );
        // Motion and the release of a bypassed press stay with the app, Shift still down or not.
        assert_eq!(
            mouse_to_terminal(&mut st, PointerEvent::Move, (200.0, 99.0), NONE, &ctx),
            None
        );
        assert_eq!(
            mouse_to_terminal(&mut st, PointerEvent::Up(B::Left), at, NONE, &ctx),
            None
        );
        // The next gesture reports again.
        assert!(mouse_to_terminal(&mut st, PointerEvent::Down(B::Left), at, NONE, &ctx).is_some());
        // Motion with Shift held is the app's.
        assert_eq!(
            mouse_to_terminal(&mut st, PointerEvent::Move, (200.0, 99.0), SHIFT, &ctx),
            None
        );
    }

    #[test]
    fn a_move_is_reported_only_when_the_cell_changes() {
        let ctx = ctx_at(1.0);
        let mut st = MouseState::default();
        let mv = |st: &mut MouseState, x: f32| {
            mouse_to_terminal(st, PointerEvent::Move, (x, 60.0), NONE, &ctx).map(parts)
        };
        assert_eq!(mv(&mut st, 21.0), Some(cell(0, 0, B::None, K::Move)));
        assert_eq!(mv(&mut st, 25.0), None, "same cell");
        assert_eq!(mv(&mut st, 27.9), None, "same cell");
        assert_eq!(mv(&mut st, 28.0), Some(cell(1, 0, B::None, K::Move)));
        assert_eq!(
            mv(&mut st, 21.0),
            Some(cell(0, 0, B::None, K::Move)),
            "back"
        );
        // With a button held the motion is a drag of that button.
        mouse_to_terminal(
            &mut st,
            PointerEvent::Down(B::Middle),
            (21.0, 60.0),
            NONE,
            &ctx,
        );
        assert_eq!(mv(&mut st, 21.0), None, "the press set the cell");
        assert_eq!(mv(&mut st, 40.0), Some(cell(2, 0, B::Middle, K::Drag)));
        mouse_to_terminal(
            &mut st,
            PointerEvent::Up(B::Middle),
            (40.0, 60.0),
            NONE,
            &ctx,
        );
        assert_eq!(mv(&mut st, 60.0), Some(cell(5, 0, B::None, K::Move)));
    }

    #[test]
    fn under_sgr_pixels_a_move_inside_a_cell_is_still_reported() {
        let ctx = MouseCtx {
            pixel_reports: true,
            ..ctx_at(1.0)
        };
        let mut st = MouseState::default();
        let mv = |st: &mut MouseState, x: f32| {
            mouse_to_terminal(st, PointerEvent::Move, (x, 60.0), NONE, &ctx).map(|e| e.x)
        };
        assert_eq!(mv(&mut st, 21.0), Some(1));
        assert_eq!(mv(&mut st, 21.0), None, "no movement");
        assert_eq!(mv(&mut st, 22.0), Some(2), "same cell, new pixel");
    }

    #[test]
    fn positions_outside_the_grid_clamp_into_it() {
        let ctx = ctx_at(1.0);
        let mut st = MouseState::default();
        let far = mouse_to_terminal(
            &mut st,
            PointerEvent::Down(B::Left),
            (5000.0, 5000.0),
            NONE,
            &ctx,
        )
        .expect("press");
        assert_eq!((far.col, far.row), (99, 29));
        assert_eq!((far.x, far.y), (799, 509), "the last pixel of the grid");
        let near = mouse_to_terminal(
            &mut st,
            PointerEvent::Down(B::Left),
            (-50.0, 0.0),
            NONE,
            &ctx,
        )
        .expect("press");
        assert_eq!((near.col, near.row, near.x, near.y), (0, 0, 0, 0));
        let nan = mouse_to_terminal(
            &mut st,
            PointerEvent::Down(B::Left),
            (f32::NAN, 60.0),
            NONE,
            &ctx,
        )
        .expect("press");
        assert_eq!((nan.col, nan.x), (0, 0));
        // Modifiers pass through to the report.
        let ctrl = mouse_to_terminal(
            &mut st,
            PointerEvent::Down(B::Left),
            (30.0, 60.0),
            TerminalModifiers::CONTROL,
            &ctx,
        )
        .expect("press");
        assert_eq!(ctrl.modifiers, TerminalModifiers::CONTROL);
    }

    #[test]
    fn buttons_a_terminal_has_no_report_for_are_dropped() {
        assert_eq!(
            pointer_event(ElementState::Pressed, MouseButton::Left),
            Some(PointerEvent::Down(B::Left))
        );
        assert_eq!(
            pointer_event(ElementState::Released, MouseButton::Right),
            Some(PointerEvent::Up(B::Right))
        );
        assert_eq!(
            pointer_event(ElementState::Pressed, MouseButton::Back),
            None
        );
        assert_eq!(
            pointer_event(ElementState::Pressed, MouseButton::Other(7)),
            None
        );
    }

    fn wctx(tracking: bool, shift: bool, no_scrollback: bool) -> WheelCtx {
        WheelCtx {
            tracking,
            shift,
            no_scrollback,
            cell_h: 17.0,
        }
    }

    fn run(deltas: &[WheelDelta], ctx: &WheelCtx) -> Vec<WheelAction> {
        let mut st = WheelState::default();
        deltas.iter().map(|d| wheel(&mut st, *d, ctx)).collect()
    }

    use WheelDelta::{Lines, Pixels};

    #[test]
    fn tracking_sends_one_report_per_notch_and_at_most_32() {
        let up = TerminalMouseButton::WheelUp;
        let down = TerminalMouseButton::WheelDown;
        let c = wctx(true, false, false);
        assert_eq!(
            run(&[Lines(1.0), Lines(1.0), Lines(1.0)], &c),
            vec![WheelAction::Reports(up, 1); 3]
        );
        assert_eq!(run(&[Lines(3.0)], &c), vec![WheelAction::Reports(up, 3)]);
        assert_eq!(run(&[Lines(-2.0)], &c), vec![WheelAction::Reports(down, 2)]);
        assert_eq!(run(&[Lines(100.0)], &c), vec![WheelAction::Reports(up, 32)]);
        assert_eq!(
            run(&[Lines(-1.0e9)], &c),
            vec![WheelAction::Reports(down, 32)]
        );
        // A sub-notch delta waits for the rest; a pixel delta counts a cell height as a notch.
        assert_eq!(
            run(&[Lines(0.5), Lines(0.5)], &c),
            vec![WheelAction::None, WheelAction::Reports(up, 1)]
        );
        assert_eq!(
            run(&[Pixels(8.5), Pixels(8.5)], &c),
            vec![WheelAction::None, WheelAction::Reports(up, 1)]
        );
        // The same events with Shift scroll locally (D7), 3 lines a notch, up is negative.
        let shifted = wctx(true, true, false);
        assert_eq!(run(&[Lines(1.0)], &shifted), vec![WheelAction::Scroll(-3)]);
    }

    #[test]
    fn the_alt_screen_types_one_arrow_per_event() {
        let c = wctx(false, false, true);
        assert_eq!(
            run(&[Lines(1.0), Lines(-1.0), Lines(5.0)], &c),
            vec![
                WheelAction::Arrows { up: true },
                WheelAction::Arrows { up: false },
                WheelAction::Arrows { up: true },
            ]
        );
        // Shift counts zero lines.
        assert_eq!(
            run(&[Lines(1.0)], &wctx(false, true, true)),
            vec![WheelAction::None]
        );
        // A mouse-tracking program on the alt screen gets reports, not arrows.
        assert_eq!(
            run(&[Lines(2.0)], &wctx(true, false, true)),
            vec![WheelAction::Reports(TerminalMouseButton::WheelUp, 2)]
        );
        // The arrow's sequence follows DECCKM in the engine (`session::Command::WheelArrows`,
        // `session_text_and_wheel_arrows_are_typed_input`); here only the direction is decided.
    }

    #[test]
    fn a_trackpad_is_scaled_down_in_the_arrow_path_only() {
        // 20 px at 17 px cells is 1.18 lines, x 0.3 = 0.35 of an arrow: three events make one.
        let arrows = run(&[Pixels(20.0); 6], &wctx(false, false, true));
        let up = WheelAction::Arrows { up: true };
        assert_eq!(
            arrows,
            vec![
                WheelAction::None,
                WheelAction::None,
                up,
                WheelAction::None,
                WheelAction::None,
                up
            ]
        );
        // A pixel delta of 50 or more is a mouse wheel: no scaling, 100 px is 5.9 lines, one arrow.
        assert_eq!(run(&[Pixels(100.0)], &wctx(false, false, true)), vec![up]);
        assert_eq!(
            run(&[Pixels(-100.0)], &wctx(false, false, true)),
            vec![WheelAction::Arrows { up: false }]
        );
        // The local scroll takes the same 20 px events unscaled (next test).
    }

    #[test]
    fn local_scrolling_takes_three_lines_a_notch_and_keeps_the_pixel_remainder() {
        let c = wctx(false, false, false);
        assert_eq!(run(&[Lines(1.0)], &c), vec![WheelAction::Scroll(-3)]);
        assert_eq!(run(&[Lines(-2.0)], &c), vec![WheelAction::Scroll(6)]);
        // Five events of 20 px at 17 px cells: 1.18 lines each, the .18 carried: 1,1,1,1,1, then 2.
        let got = run(&[Pixels(20.0); 6], &c);
        assert_eq!(
            got,
            vec![
                WheelAction::Scroll(-1),
                WheelAction::Scroll(-1),
                WheelAction::Scroll(-1),
                WheelAction::Scroll(-1),
                WheelAction::Scroll(-1),
                WheelAction::Scroll(-2),
            ]
        );
        // A sub-line pixel delta is kept until it adds up, in either direction.
        assert_eq!(
            run(&[Pixels(-10.0), Pixels(-10.0)], &c),
            vec![WheelAction::None, WheelAction::Scroll(1)]
        );
        // Shift does not change the local scroll.
        assert_eq!(
            run(&[Lines(1.0)], &wctx(false, true, false)),
            vec![WheelAction::Scroll(-3)]
        );
    }

    #[test]
    fn the_remainders_are_per_path() {
        let mut st = WheelState::default();
        let scroll = wctx(false, false, false);
        let arrows = wctx(false, false, true);
        assert_eq!(wheel(&mut st, Pixels(10.0), &scroll), WheelAction::None);
        // The half line kept for scrolling does not tip the arrow path.
        assert_eq!(wheel(&mut st, Pixels(10.0), &arrows), WheelAction::None);
        assert_eq!(
            wheel(&mut st, Pixels(10.0), &scroll),
            WheelAction::Scroll(-1)
        );
    }

    #[test]
    fn nonsense_deltas_do_nothing() {
        assert_eq!(wheel_delta(MouseScrollDelta::LineDelta(1.0, 0.0)), None);
        assert_eq!(
            wheel_delta(MouseScrollDelta::LineDelta(0.0, f32::NAN)),
            None
        );
        assert_eq!(
            wheel_delta(MouseScrollDelta::LineDelta(0.0, 2.0)),
            Some(Lines(2.0))
        );
        assert_eq!(
            wheel_delta(MouseScrollDelta::PixelDelta(
                winit::dpi::PhysicalPosition::new(0.0, -7.5)
            )),
            Some(Pixels(-7.5))
        );
        let mut st = WheelState::default();
        let bad_cell = WheelCtx {
            cell_h: 0.0,
            ..wctx(false, false, false)
        };
        assert_eq!(wheel(&mut st, Pixels(20.0), &bad_cell), WheelAction::None);
        // A garbage line delta is bounded.
        let big = wheel(&mut st, Lines(1.0e300), &wctx(false, false, false));
        assert_eq!(big, WheelAction::Scroll(-100_000));
    }

    #[test]
    fn a_wheel_report_carries_the_cell_and_modifiers() {
        let ctx = ctx_at(1.0);
        let e = wheel_report(&ctx, (63.0, 99.0), true, TerminalModifiers::CONTROL);
        assert_eq!(parts(e), cell(5, 3, B::WheelUp, K::WheelUp));
        assert_eq!(e.modifiers, TerminalModifiers::CONTROL);
        let d = wheel_report(&ctx, (63.0, 99.0), false, NONE);
        assert_eq!(parts(d), cell(5, 3, B::WheelDown, K::WheelDown));
    }
}
