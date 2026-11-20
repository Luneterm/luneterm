//! Efficiency mode in the background: while the window is not focused or is minimized, Luneterm
//! throttles itself, and never the shells it starts (plan copy-select-efficiency, step 3).
//!
//! The [`Controller`] is a small state machine owned by the app on the UI thread (no global, no
//! mutex). It decides from four inputs (the setting, the window's focus, the time since a tab last
//! printed, and whether the OS mechanism slows the PTY path too) and calls an [`OsThrottle`] only
//! when the decision changes:
//!
//! - Windows: EcoQoS plus the idle priority class (Task Manager's own recipe, the green leaf).
//!   The class is process-wide, so it also slows the PTY reader: the throttle waits for [`QUIET`]
//!   with no output in any tab, and output lifts it (D6).
//! - macOS: the UI thread's QoS to utility (the PTY threads are not touched, so no gate).
//! - Elsewhere: nothing (the backend is never called).
//!
//! Invariants: never throttled while focused, while disabled, or inside [`Controller::lifted`]
//! (every child creation runs there: the idle class is inherited); OS calls only on a change of
//! state; an OS error leaves the process unthrottled and the controller off for the run.

use std::fmt;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// How long no tab may have printed before an OS mechanism that slows the PTY path is applied.
pub const QUIET: Duration = Duration::from_secs(2);

/// The OS mechanism picked for this build.
#[cfg(target_os = "macos")]
pub type HostOs = macos::UtilityQos;
/// The OS mechanism picked for this build.
#[cfg(windows)]
pub type HostOs = windows::EcoQos;
/// The OS mechanism picked for this build (none here: the controller never calls it).
#[cfg(not(any(windows, target_os = "macos")))]
pub type HostOs = NoOs;

/// A failed OS call: its name and code, for the one line printed when efficiency mode turns off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsError {
    pub call: &'static str,
    pub code: i64,
}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed ({})", self.call, self.code)
    }
}

/// What the OS offers to throttle Luneterm itself. `apply` and `lift` are idempotent; `lift`
/// restores what the first `apply` found.
pub trait OsThrottle {
    /// Whether this OS has a mechanism at all; when false the controller never calls it.
    const ACTIVE: bool;
    /// Whether throttling slows the PTY path (reader, engine and writer threads) too: Windows'
    /// priority class does (it is process-wide), macOS' UI-thread QoS does not. True: the quiet
    /// gate applies.
    const SLOWS_IO_PATH: bool;
    fn apply(&mut self) -> Result<(), OsError>;
    fn lift(&mut self) -> Result<(), OsError>;
}

/// The backend of an OS with no mechanism (Linux: nice and `SCHED_IDLE` cannot be undone without
/// privileges). Never called.
#[derive(Debug, Default)]
pub struct NoOs;

impl OsThrottle for NoOs {
    const ACTIVE: bool = false;
    const SLOWS_IO_PATH: bool = false;
    fn apply(&mut self) -> Result<(), OsError> {
        Ok(())
    }
    fn lift(&mut self) -> Result<(), OsError> {
        Ok(())
    }
}

/// Whether the throttle should be on: the setting is on, the window is not focused, and either no
/// tab printed for [`QUIET`] or the mechanism leaves the PTY path alone.
pub fn wanted(enabled: bool, focused: bool, quiet: bool, slows_io: bool) -> bool {
    enabled && !focused && (quiet || !slows_io)
}

/// The state machine; see the module docs. Starts focused and not applied.
#[derive(Debug)]
pub struct Controller<O: OsThrottle> {
    os: O,
    enabled: bool,
    focused: bool,
    last_output: Option<Instant>,
    applied: bool,
    /// An OS call failed: nothing more is called this run.
    broken: bool,
}

impl<O: OsThrottle> Controller<O> {
    pub fn new(os: O, enabled: bool) -> Self {
        Controller {
            os,
            enabled,
            focused: true,
            last_output: None,
            applied: false,
            broken: false,
        }
    }

    /// Whether no tab printed within [`QUIET`] of `now`.
    fn quiet(&self, now: Instant) -> bool {
        self.last_output
            .is_none_or(|t| now.saturating_duration_since(t) >= QUIET)
    }

    /// Whether the throttle is on now.
    pub fn applied(&self) -> bool {
        self.applied
    }

    /// The setting changed.
    pub fn set_enabled(&mut self, enabled: bool, now: Instant) {
        self.enabled = enabled;
        self.sync(now);
    }

    /// The process is ending: lifts the throttle (if on) and keeps it off, so the last writes
    /// (workspace, settings) never run at the idle class.
    pub fn release_for_exit(&mut self, now: Instant) {
        self.set_enabled(false, now);
    }

    /// The window gained or lost focus (a minimized window is not focused).
    pub fn set_focused(&mut self, focused: bool, now: Instant) {
        self.focused = focused;
        self.sync(now);
    }

    /// A tab parsed PTY bytes (`Update.output`).
    pub fn output(&mut self, now: Instant) {
        self.last_output = Some(now);
        self.sync(now);
    }

    /// The loop woke: applies the throttle when the quiet period has passed.
    pub fn tick(&mut self, now: Instant) {
        self.sync(now);
    }

    /// When the loop must wake to apply after a quiet period; `None` when nothing waits. Call
    /// [`Self::tick`] first in every callback: then a returned instant is in the future (or the
    /// very next `tick` consumes it), so arming it cannot spin the loop.
    pub fn deadline(&self) -> Option<Instant> {
        (O::ACTIVE
            && O::SLOWS_IO_PATH
            && self.enabled
            && !self.focused
            && !self.applied
            && !self.broken)
            .then_some(self.last_output)
            .flatten()
            .map(|t| t + QUIET)
    }

    /// Runs `f` with the throttle lifted, then re-evaluates. Every child creation goes through
    /// here: the idle class is inherited, so a child made while throttled would start throttled.
    /// `f` must not re-enter the controller.
    pub fn lifted<R>(&mut self, now: Instant, f: impl FnOnce() -> R) -> R {
        if self.applied {
            self.do_lift();
        }
        let r = f();
        self.sync(now);
        r
    }

    fn sync(&mut self, now: Instant) {
        if self.broken || !O::ACTIVE {
            return;
        }
        let want = wanted(
            self.enabled,
            self.focused,
            self.quiet(now),
            O::SLOWS_IO_PATH,
        );
        match (want, self.applied) {
            (true, false) => match self.os.apply() {
                Ok(()) => self.applied = true,
                Err(e) => {
                    // Half a recipe may be on: lift what can be lifted, then stop for the run.
                    let _ = self.os.lift();
                    self.turn_off(&e);
                }
            },
            (false, true) => self.do_lift(),
            _ => {}
        }
    }

    fn do_lift(&mut self) {
        self.applied = false;
        if let Err(e) = self.os.lift() {
            self.turn_off(&e);
        }
    }

    fn turn_off(&mut self, e: &OsError) {
        self.broken = true;
        self.applied = false;
        eprintln!("luneterm: efficiency mode off: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that records its calls and fails on demand.
    #[derive(Debug, Default)]
    struct Fake {
        calls: Vec<&'static str>,
        fail_apply: bool,
        fail_lift: bool,
        on: bool,
    }

    macro_rules! fake_os {
        ($name:ident, $slows:expr) => {
            #[derive(Debug, Default)]
            struct $name(Fake);
            impl OsThrottle for $name {
                const ACTIVE: bool = true;
                const SLOWS_IO_PATH: bool = $slows;
                fn apply(&mut self) -> Result<(), OsError> {
                    self.0.calls.push("apply");
                    if self.0.fail_apply {
                        self.0.on = true; // half a recipe
                        return Err(OsError {
                            call: "apply",
                            code: 5,
                        });
                    }
                    self.0.on = true;
                    Ok(())
                }
                fn lift(&mut self) -> Result<(), OsError> {
                    self.0.calls.push("lift");
                    self.0.on = false;
                    if self.0.fail_lift {
                        return Err(OsError {
                            call: "lift",
                            code: 6,
                        });
                    }
                    Ok(())
                }
            }
        };
    }
    fake_os!(Gated, true);
    fake_os!(Plain, false);

    fn at(ms: u64, base: Instant) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn wanted_is_the_documented_rule() {
        for enabled in [false, true] {
            for focused in [false, true] {
                for quiet in [false, true] {
                    for slows in [false, true] {
                        assert_eq!(
                            wanted(enabled, focused, quiet, slows),
                            enabled && !focused && (quiet || !slows)
                        );
                    }
                }
            }
        }
        assert!(wanted(true, false, false, false), "macOS: output is fine");
        assert!(!wanted(true, false, false, true), "Windows: not while busy");
    }

    #[test]
    fn starts_focused_and_not_applied() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), true);
        assert!(!c.applied());
        c.tick(t0);
        assert!(c.os.0.calls.is_empty());
    }

    #[test]
    fn focus_loss_applies_once_and_focus_gain_lifts_once() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), true);
        c.set_focused(false, t0);
        c.set_focused(false, t0);
        c.tick(at(10, t0));
        assert_eq!(c.os.0.calls, ["apply"], "repeated loss: one apply");
        assert!(c.applied());
        c.set_focused(true, at(20, t0));
        c.set_focused(true, at(30, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"]);
        assert!(!c.applied());
    }

    #[test]
    fn disabled_never_applies_and_disabling_lifts() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), false);
        c.set_focused(false, t0);
        assert!(c.os.0.calls.is_empty());
        c.set_enabled(true, at(1, t0));
        assert_eq!(c.os.0.calls, ["apply"]);
        c.set_enabled(false, at(2, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"]);
        assert!(!c.applied());
    }

    #[test]
    fn release_for_exit_lifts_when_applied_and_stays_off() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), true);
        c.set_focused(false, t0);
        assert_eq!(c.os.0.calls, ["apply"]);
        c.release_for_exit(at(1, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"]);
        assert!(!c.applied());
        c.tick(at(10_000, t0));
        c.set_focused(false, at(10_001, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"], "no re-apply after exit");

        // Windows-like backend (slows_io): applied after the quiet period.
        let mut g = Controller::new(Gated::default(), true);
        g.set_focused(false, t0);
        g.tick(at(3_000, t0));
        assert!(g.applied());
        g.release_for_exit(at(3_001, t0));
        assert_eq!(g.os.0.calls, ["apply", "lift"]);
        assert!(!g.applied());

        // Not applied: no OS call.
        let mut u = Controller::new(Plain::default(), true);
        u.release_for_exit(t0);
        assert!(u.os.0.calls.is_empty());
    }

    #[test]
    fn lifted_lifts_first_and_reapplies_after() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), true);
        c.set_focused(false, t0);
        assert_eq!(c.lifted(at(5, t0), || 7), 7);
        assert_eq!(c.os.0.calls, ["apply", "lift", "apply"]);
        assert!(c.applied(), "re-applied after");
    }

    #[test]
    fn f_runs_unthrottled() {
        // The fake flips `on` with its calls; read it from inside `f` through a raw cell shared
        // with the backend.
        use std::cell::Cell;
        use std::rc::Rc;
        #[derive(Debug)]
        struct Shared(Rc<Cell<bool>>);
        impl OsThrottle for Shared {
            const ACTIVE: bool = true;
            const SLOWS_IO_PATH: bool = false;
            fn apply(&mut self) -> Result<(), OsError> {
                self.0.set(true);
                Ok(())
            }
            fn lift(&mut self) -> Result<(), OsError> {
                self.0.set(false);
                Ok(())
            }
        }
        let t0 = Instant::now();
        let on = Rc::new(Cell::new(false));
        let mut c = Controller::new(Shared(on.clone()), true);
        c.set_focused(false, t0);
        assert!(on.get());
        let seen = c.lifted(at(1, t0), || on.get());
        assert!(!seen, "`f` saw the process unthrottled");
        assert!(on.get(), "re-applied after");
    }

    #[test]
    fn lifted_while_unapplied_makes_no_os_call() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), true);
        c.lifted(t0, || ());
        assert!(c.os.0.calls.is_empty());
        // Focused (not wanted) after `f`: still nothing.
        let mut c = Controller::new(Gated::default(), true);
        c.output(t0);
        c.lifted(at(10, t0), || ());
        assert!(c.os.0.calls.is_empty());
    }

    #[test]
    fn the_quiet_gate_waits_for_the_quiet_period_and_output_lifts() {
        let t0 = Instant::now();
        let mut c = Controller::new(Gated::default(), true);
        c.output(t0);
        c.set_focused(false, at(100, t0));
        assert!(c.os.0.calls.is_empty(), "output within QUIET: no apply");
        assert_eq!(c.deadline(), Some(t0 + QUIET));
        c.tick(at(1999, t0));
        assert!(c.os.0.calls.is_empty());
        c.tick(t0 + QUIET);
        assert_eq!(c.os.0.calls, ["apply"], "tick at the deadline applies");
        assert_eq!(c.deadline(), None, "nothing waits once applied");
        c.output(at(2500, t0));
        assert_eq!(
            c.os.0.calls,
            ["apply", "lift"],
            "output while applied lifts"
        );
        assert_eq!(c.deadline(), Some(at(2500, t0) + QUIET));
        // More output while lifted only moves the deadline.
        c.output(at(3000, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"]);
        assert_eq!(c.deadline(), Some(at(3000, t0) + QUIET));
    }

    #[test]
    fn no_deadline_when_nothing_waits() {
        let t0 = Instant::now();
        // Never printed: applies at focus loss, no deadline.
        let mut c = Controller::new(Gated::default(), true);
        c.set_focused(false, t0);
        assert_eq!(c.os.0.calls, ["apply"]);
        assert_eq!(c.deadline(), None);
        // Focused, or disabled: no deadline even with recent output.
        let mut c = Controller::new(Gated::default(), true);
        c.output(t0);
        assert_eq!(c.deadline(), None, "focused");
        c.set_focused(false, t0);
        c.set_enabled(false, t0);
        assert_eq!(c.deadline(), None, "disabled");
        // A mechanism that leaves the PTY path alone has no gate.
        let mut c = Controller::new(Plain::default(), true);
        c.output(t0);
        c.set_focused(false, t0);
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn without_the_gate_output_does_not_lift() {
        let t0 = Instant::now();
        let mut c = Controller::new(Plain::default(), true);
        c.set_focused(false, t0);
        c.output(at(1, t0));
        c.output(at(2, t0));
        assert_eq!(c.os.0.calls, ["apply"]);
        assert!(c.applied());
    }

    #[test]
    fn an_apply_error_lifts_then_stops_for_the_run() {
        let t0 = Instant::now();
        let mut c = Controller::new(
            Plain(Fake {
                fail_apply: true,
                ..Fake::default()
            }),
            true,
        );
        c.set_focused(false, t0);
        assert_eq!(c.os.0.calls, ["apply", "lift"], "half a recipe is lifted");
        assert!(!c.os.0.on && !c.applied());
        c.set_focused(true, at(1, t0));
        c.set_focused(false, at(2, t0));
        c.lifted(at(3, t0), || ());
        c.set_enabled(false, at(4, t0));
        c.set_enabled(true, at(5, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"], "no further OS call");
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn a_lift_error_stops_for_the_run_and_never_reapplies() {
        let t0 = Instant::now();
        let mut c = Controller::new(
            Plain(Fake {
                fail_lift: true,
                ..Fake::default()
            }),
            true,
        );
        c.set_focused(false, t0);
        c.set_focused(true, at(1, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"]);
        assert!(!c.applied());
        c.set_focused(false, at(2, t0));
        assert_eq!(c.os.0.calls, ["apply", "lift"], "off for the run");
    }

    #[test]
    fn a_backend_with_no_mechanism_is_never_called() {
        let t0 = Instant::now();
        let mut c = Controller::new(NoOs, true);
        c.set_focused(false, t0);
        c.output(t0);
        c.tick(at(5000, t0));
        c.lifted(at(5001, t0), || ());
        assert!(!c.applied());
        assert_eq!(c.deadline(), None);
    }
}
