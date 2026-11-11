//! Frame pacing (plan render-performance D3-D5, D8): which present mode the window asks for and
//! how it is resolved, the frame grid's rate, which events are urgent, and the pacer that decides
//! when a frame asked for is drawn.
//!
//! The pacer has two lanes. A paced ask (PTY output, pointer moves, wheel, animations, the
//! crossfade, the blink, deadlines) starts at most one frame per slot of a grid at the display's
//! rate; an urgent ask (discrete input, and the one frame after a frame that took some) draws at
//! once and does not take the slot. It reads no clock: times are app time, the `Duration` since
//! the app's start, the origin of `RawInput.time` and `Repaint::At`.

use std::time::Duration;

use lune_ui::input::Event;
use lune_ui::motion::Cadence;

/// The environment variable that picks the present mode for a run (`fifo` or `novsync`).
pub const PRESENT_VAR: &str = "LUNETERM_PRESENT";

/// What the app asks of the surface: wait for the display, or do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentAsk {
    /// `Fifo`: presents wait for the display.
    Fifo,
    /// The first of `Immediate`, `Mailbox` the surface lists, else `Fifo`.
    NoVsync,
}

impl PresentAsk {
    /// The ask's name as the trace line prints it.
    pub fn name(self) -> &'static str {
        match self {
            PresentAsk::Fifo => "Fifo",
            PresentAsk::NoVsync => "NoVsync",
        }
    }
}

/// What the app asks for when `LUNETERM_PRESENT` is unset: no wait, on every OS (the user's
/// answer to D3, 2026-11-11). Measured on Metal only (luneui-rs D299); unmeasured on DX12, Vulkan
/// and GL, and X11 without a compositor may tear: `LUNETERM_PRESENT=fifo` is the escape.
pub const DEFAULT_PRESENT: PresentAsk = PresentAsk::NoVsync;

/// Reads [`PRESENT_VAR`]'s value: unset is `None` (the default), `fifo` and `novsync` are the two
/// asks. Anything else is an error that names the variable and nothing else.
pub fn present_from_env(value: Option<&str>) -> Result<Option<PresentAsk>, String> {
    match value {
        None => Ok(None),
        Some("fifo") => Ok(Some(PresentAsk::Fifo)),
        Some("novsync") => Ok(Some(PresentAsk::NoVsync)),
        Some(_) => Err(format!("{PRESENT_VAR} must be `fifo` or `novsync`")),
    }
}

/// The concrete present mode for `ask` on a surface that lists `supported`: `Fifo` for a `Fifo`
/// ask, else the first of `Immediate`, `Mailbox` the surface lists, else `Fifo` (every surface
/// offers it).
pub fn resolve(ask: PresentAsk, supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    match ask {
        PresentAsk::Fifo => Fifo,
        PresentAsk::NoVsync => [Immediate, Mailbox]
            .into_iter()
            .find(|m| supported.contains(m))
            .unwrap_or(Fifo),
    }
}

/// The frame grid's rate for a surface presenting in `mode` on a display of `refresh_mhz`
/// (D5): the display's whole Hz (60 when unknown), clamped to 30..=240, one below that when
/// presents queue (`Fifo`), so the grid never fills the queue and waits in acquire (D299).
pub fn grid_hz(mode: wgpu::PresentMode, refresh_mhz: Option<u32>) -> u16 {
    use wgpu::PresentMode::{AutoVsync, Fifo, FifoRelaxed};
    let hz = refresh_mhz.map_or(60, |m| m / 1000).clamp(30, 240) as u16;
    match mode {
        Fifo | FifoRelaxed | AutoVsync => hz - 1,
        _ => hz,
    }
}

/// The kind of callback an ask comes from, as far as pacing cares (D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    MouseButton,
    Key,
    Ime,
    Focus,
    Resized,
    ScaleFactorChanged,
    /// `Occluded(false)`.
    Unoccluded,
    /// `Occluded(true)`: asks for nothing.
    Occluded,
    DroppedFile,
    CloseRequested,
    /// `UserEvent::TraceSwitch`.
    TraceSwitch,
    CursorMoved,
    CursorEntered,
    CursorLeft,
    Wheel,
    ModifiersChanged,
    HoveredFile,
    HoveredFileCancelled,
    Moved,
    /// `UserEvent::Wake`: a session's Update.
    Wake,
    /// `RedrawRequested`: the frame sets its own post-frame urgency.
    Redraw,
    /// A deadline came due (`StartCause::ResumeTimeReached`).
    Deadline,
    /// Anything else (touch, gestures, theme changes): paced.
    Other,
}

/// Whether asks made while handling `kind` draw at once (D4): discrete input only. Every event
/// that can come at the pointer's or the PTY's rate is paced, or a flood draws at event rate.
pub fn is_urgent(kind: EventKind) -> bool {
    use EventKind::*;
    match kind {
        MouseButton | Key | Ime | Focus | Resized | ScaleFactorChanged | Unoccluded
        | DroppedFile | CloseRequested | TraceSwitch => true,
        Occluded | CursorMoved | CursorEntered | CursorLeft | Wheel | ModifiersChanged
        | HoveredFile | HoveredFileCancelled | Moved | Wake | Redraw | Deadline | Other => false,
    }
}

/// Whether the post-frame section of a frame that took `events` is urgent (D4's follow-up): the
/// frame took discrete input, so what it asks for next (the click's tab switch, a menu opening)
/// draws at once. Only that one frame: the next frame's asks are paced again.
pub fn follow_up(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            Event::PointerButton { .. }
                | Event::Key { .. }
                | Event::Text(_)
                | Event::Ime(_)
                | Event::Focus(_)
                | Event::Access(_)
                | Event::ClipboardText(_)
                | Event::ClipboardUnavailable
        )
    })
}

/// When the frames asked for are drawn (D4). Pure: every time is app time passed in.
#[derive(Debug, Clone)]
pub struct Pacer {
    rate: Cadence,
    /// When the last paced frame started; `None` before the first.
    last_paced: Option<Duration>,
    /// The earliest deferred paced ask.
    due: Option<Duration>,
    /// Since the last frame start: a paced ask was served at once.
    paced_now: bool,
    /// Since the last frame start: an urgent ask was made.
    urgent_now: bool,
}

impl Pacer {
    /// A pacer on a grid of `rate`.
    pub fn new(rate: Cadence) -> Pacer {
        Pacer {
            rate,
            last_paced: None,
            due: None,
            paced_now: false,
            urgent_now: false,
        }
    }

    /// The first grid tick after the last paced frame start (0 before any frame); the start
    /// itself under [`Cadence::EveryFrame`].
    fn free_at(&self) -> Duration {
        self.last_paced
            .map_or(Duration::ZERO, |l| self.rate.next_tick(l).unwrap_or(l))
    }

    /// A frame is asked for at `now`, to be drawn no earlier than `not_before`. Returns whether
    /// to request a redraw now; when it does not, the ask is in [`Pacer::deadline`]. An urgent
    /// ask is always served at once and does not take the grid slot.
    pub fn ask(&mut self, now: Duration, not_before: Duration, urgent: bool) -> bool {
        if urgent {
            self.urgent_now = true;
            return true;
        }
        let at = not_before.max(self.free_at());
        if at <= now {
            self.paced_now = true;
            // A deadline that has come is served by the frame asked for now: dropped, so the
            // control flow does not wait for a moment already past while the redraw is pending.
            if self.due.is_some_and(|d| d <= now) {
                self.due = None;
            }
            true
        } else {
            self.due = Some(self.due.map_or(at, |d| d.min(at)));
            false
        }
    }

    /// A frame starts at `now`: it serves every ask made so far (each frame re-derives its
    /// time-based asks, I3), so the deadline goes. The frame takes the grid slot unless every
    /// ask it serves was urgent; a frame with no ask (the OS asked) is paced.
    pub fn frame_started(&mut self, now: Duration) {
        let deadline_frame = self.due.is_some_and(|d| d <= now);
        let only_urgent = self.urgent_now && !self.paced_now && !deadline_frame;
        if !only_urgent {
            self.last_paced = Some(now);
        }
        self.due = None;
        self.paced_now = false;
        self.urgent_now = false;
    }

    /// When the earliest deferred ask is due, if any.
    pub fn deadline(&self) -> Option<Duration> {
        self.due
    }

    /// Drops the deferred asks (the window is hidden, or the surface has nothing to draw on).
    pub fn clear(&mut self) {
        self.due = None;
    }

    /// The grid's rate (the window moved to another display). The surface is not touched.
    pub fn set_rate(&mut self, rate: Cadence) {
        self.rate = rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::PresentMode::{AutoNoVsync, Fifo, FifoRelaxed, Immediate, Mailbox};

    #[test]
    fn env_unset_is_the_default() {
        assert_eq!(present_from_env(None), Ok(None));
    }

    #[test]
    fn env_knows_two_words() {
        assert_eq!(present_from_env(Some("fifo")), Ok(Some(PresentAsk::Fifo)));
        assert_eq!(
            present_from_env(Some("novsync")),
            Ok(Some(PresentAsk::NoVsync))
        );
    }

    #[test]
    fn env_anything_else_is_an_error_naming_only_the_variable() {
        for bad in ["", "FIFO", "mailbox", "fifo ", "secret-value"] {
            let e = present_from_env(Some(bad)).expect_err("rejected");
            assert!(e.contains(PRESENT_VAR), "{e}");
        }
        let e = present_from_env(Some("secret-value")).expect_err("rejected");
        assert!(!e.contains("secret-value"), "{e}");
    }

    #[test]
    fn novsync_takes_immediate_then_mailbox_then_fifo() {
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo, Immediate]), Immediate);
        assert_eq!(
            resolve(PresentAsk::NoVsync, &[Fifo, Mailbox, Immediate]),
            Immediate
        );
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo, Mailbox]), Mailbox);
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo]), Fifo);
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo, FifoRelaxed]), Fifo);
        assert_eq!(resolve(PresentAsk::NoVsync, &[]), Fifo);
    }

    #[test]
    fn fifo_ask_is_fifo_whatever_the_surface_lists() {
        for caps in [&[Fifo][..], &[Fifo, Immediate, Mailbox], &[Immediate], &[]] {
            assert_eq!(resolve(PresentAsk::Fifo, caps), Fifo);
        }
        // Never an Auto* mode: the result is always concrete.
        assert_ne!(
            resolve(PresentAsk::NoVsync, &[AutoNoVsync, Fifo]),
            AutoNoVsync
        );
    }

    // ------------------------------------------------------------------ the pacer (step 3)

    const MS: Duration = Duration::from_millis(1);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A simulated event loop, 1 µs steps over one second: `asks(t)` says whether a callback at
    /// `t` asks for a frame and whether it is urgent; a requested or due frame starts when the
    /// previous one is done and costs `cost`. Returns the frame starts.
    fn simulate(rate: Cadence, cost: Duration, every: Duration, urgent: bool) -> Vec<Duration> {
        let mut p = Pacer::new(rate);
        let mut starts = Vec::new();
        let (mut requested, mut busy_until) = (false, Duration::ZERO);
        let mut next_ask = Duration::ZERO;
        let step = Duration::from_micros(10);
        let mut t = Duration::ZERO;
        while t < Duration::from_secs(1) {
            if t >= next_ask {
                if p.ask(t, Duration::ZERO, urgent) {
                    requested = true;
                }
                next_ask += every;
            }
            if t >= busy_until && (requested || p.deadline().is_some_and(|d| d <= t)) {
                p.frame_started(t);
                starts.push(t);
                requested = false;
                busy_until = t + cost;
            }
            t += step;
        }
        starts
    }

    #[test]
    fn asks_every_ms_draw_one_frame_per_slot() {
        // (a) 60 Hz, frames of 2 ms.
        let n = simulate(Cadence::hz(60), 2 * MS, MS, false).len();
        assert!((59..=60).contains(&n), "{n}");
        // (b) 30 Hz.
        let n = simulate(Cadence::hz(30), 2 * MS, MS, false).len();
        assert!((29..=30).contains(&n), "{n}");
    }

    #[test]
    fn urgent_asks_do_not_take_the_slot() {
        // (c)
        // A paced frame at 16 ms (its slot ends at the tick 16.67 ms), an urgent ask 1 ms later,
        // a paced ask 1 ms after that frame: the slot is still free, as if the urgent frame had
        // not been (counted as paced it would hold the ask until 33.3 ms).
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(16));
        assert!(p.ask(ms(17), Duration::ZERO, true), "urgent: at once");
        p.frame_started(ms(17));
        assert!(
            p.ask(ms(18), Duration::ZERO, false),
            "the urgent frame took no slot"
        );
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(100));
        assert!(p.ask(ms(101), Duration::ZERO, true));
        p.frame_started(ms(101));
        assert!(!p.ask(ms(102), Duration::ZERO, false));
        assert_eq!(p.deadline(), Cadence::hz(60).next_tick(ms(100)));
    }

    #[test]
    fn a_paced_ask_after_a_paced_frame_waits_for_the_next_tick() {
        // (d)
        let mut p = Pacer::new(Cadence::hz(60));
        assert!(p.ask(ms(0), Duration::ZERO, false));
        p.frame_started(ms(0));
        assert!(!p.ask(ms(0), Duration::ZERO, false));
        assert_eq!(p.deadline(), Some(Duration::from_nanos(16_666_667)));
    }

    #[test]
    fn not_before_holds_a_free_slot() {
        // (e)
        let mut p = Pacer::new(Cadence::hz(60));
        assert!(!p.ask(ms(0), ms(100), false));
        assert_eq!(p.deadline(), Some(ms(100)));
        // At its time the ask is served.
        assert!(p.ask(ms(100), ms(100), false));
    }

    #[test]
    fn several_deferred_asks_keep_the_earliest() {
        // (f)
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(0));
        assert!(!p.ask(ms(1), ms(600), false));
        assert!(!p.ask(ms(2), Duration::ZERO, false));
        assert!(!p.ask(ms(3), ms(300), false));
        assert_eq!(p.deadline(), Some(Duration::from_nanos(16_666_667)));
        let mut p = Pacer::new(Cadence::hz(60));
        assert!(!p.ask(ms(1), ms(600), false));
        assert!(!p.ask(ms(2), ms(300), false));
        assert_eq!(p.deadline(), Some(ms(300)));
    }

    #[test]
    fn a_long_frame_frees_the_slot_it_overran() {
        // (g)
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(0));
        assert!(p.ask(ms(20), Duration::ZERO, false));
    }

    #[test]
    fn every_frame_serves_every_ask() {
        // (h)
        let mut p = Pacer::new(Cadence::EveryFrame);
        for t in 0..100 {
            assert!(p.ask(ms(t), Duration::ZERO, false));
            p.frame_started(ms(t));
        }
        assert_eq!(p.deadline(), None);
    }

    #[test]
    fn pointer_moves_are_paced() {
        // (i) a second of cursor moves every 2 ms, frames of 2 ms.
        let urgent = is_urgent(EventKind::CursorMoved);
        assert!(!urgent);
        let n = simulate(Cadence::hz(60), 2 * MS, 2 * MS, urgent).len();
        assert!(n <= 60, "{n}");
        // Urgent asks at that rate would draw one frame per ask: what pacing prevents.
        let n = simulate(Cadence::hz(60), MS, 2 * MS, true).len();
        assert!(n > 400, "{n}");
    }

    #[test]
    fn the_grid_follows_the_display() {
        // (j)
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        assert_eq!(grid_hz(Immediate, Some(60_000)), 60);
        assert_eq!(grid_hz(Fifo, Some(60_000)), 59);
        assert_eq!(grid_hz(Immediate, Some(59_940)), 59);
        assert_eq!(grid_hz(Immediate, None), 60);
        assert_eq!(grid_hz(Immediate, Some(1_000)), 30);
        assert_eq!(grid_hz(Immediate, Some(500_000)), 240);
        assert_eq!(grid_hz(Fifo, Some(120_000)), 119);
        assert_eq!(grid_hz(Mailbox, Some(120_000)), 120);
        assert_eq!(grid_hz(Fifo, None), 59);
    }

    #[test]
    fn a_frame_of_urgent_asks_only_keeps_the_slot() {
        // (k)
        let tick = Duration::from_nanos(16_666_667);
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(0));
        assert!(p.ask(ms(5), Duration::ZERO, true));
        p.frame_started(ms(5));
        // `free_at` is still the tick after 0.
        assert!(!p.ask(ms(6), Duration::ZERO, false));
        assert_eq!(p.deadline(), Some(tick));
        // An urgent frame once that deadline is due counts as paced: the slot moves to the tick
        // after 20 ms.
        assert!(p.ask(ms(20), Duration::ZERO, true));
        p.frame_started(ms(20));
        assert!(!p.ask(ms(21), Duration::ZERO, false));
        assert_eq!(p.deadline(), Cadence::hz(60).next_tick(ms(20)));
        // A frame with no ask (the OS asked) is paced too.
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(40));
        assert!(!p.ask(ms(41), Duration::ZERO, false));
        assert_eq!(p.deadline(), Cadence::hz(60).next_tick(ms(40)));
    }

    #[test]
    fn clear_drops_the_deadline() {
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(0));
        assert!(!p.ask(ms(1), Duration::ZERO, false));
        p.clear();
        assert_eq!(p.deadline(), None);
    }

    #[test]
    fn only_discrete_input_is_urgent() {
        use EventKind::*;
        let table = [
            (MouseButton, true),
            (Key, true),
            (Ime, true),
            (Focus, true),
            (Resized, true),
            (ScaleFactorChanged, true),
            (Unoccluded, true),
            (DroppedFile, true),
            (CloseRequested, true),
            (TraceSwitch, true),
            (Occluded, false),
            (CursorMoved, false),
            (CursorEntered, false),
            (CursorLeft, false),
            (Wheel, false),
            (ModifiersChanged, false),
            (HoveredFile, false),
            (HoveredFileCancelled, false),
            (Moved, false),
            (Wake, false),
            (Redraw, false),
            (Deadline, false),
            (Other, false),
        ];
        for (kind, urgent) in table {
            assert_eq!(is_urgent(kind), urgent, "{kind:?}");
        }
    }

    #[test]
    fn a_frame_that_took_discrete_input_is_followed_up_at_once() {
        use lune_ui::input::{Modifiers, PointerButton};
        let press = Event::PointerButton {
            pos: (1.0, 1.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::default(),
        };
        let moved = Event::PointerMoved { pos: (2.0, 2.0) };
        assert!(follow_up(&[moved.clone(), press]));
        assert!(follow_up(&[Event::Text("a".into())]));
        assert!(follow_up(&[Event::Focus(true)]));
        assert!(follow_up(&[Event::ClipboardUnavailable]));
        assert!(follow_up(&[Event::ClipboardText("x".into())]));
        assert!(!follow_up(&[moved, Event::PointerLeft]));
        assert!(!follow_up(&[Event::ModifiersChanged(Modifiers::default())]));
        assert!(!follow_up(&[]));
    }
}
