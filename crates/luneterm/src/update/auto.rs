//! The two rules the app keeps around the update worker (auto-update plan step 4, D7), as plain
//! data with no clock of their own and no UI, so they are tested without a window:
//!
//! - [`AutoCheck`]: the 24 h timer. Runs only while "Check for updates automatically" is on and
//!   the copy can update itself; switching it off drops the deadline.
//! - [`ToastRule`]: when a state earns the glass toast. Only a check the timer started (`auto`)
//!   that found a version, and each version once per run; a manual check never toasts.

use std::time::{Duration, Instant};

use super::UpdateState;

/// How often the timer checks.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The automatic check's schedule: a check at once when it is switched on (at launch, or when the
/// setting is turned on), then one every [`CHECK_EVERY`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoCheck {
    next: Option<Instant>,
}

impl AutoCheck {
    /// Switches the timer on (the first check is due at `now`) or off (no deadline). Switching on
    /// a timer that already runs keeps its schedule.
    pub fn set_enabled(&mut self, on: bool, now: Instant) {
        match (on, self.next) {
            (true, None) => self.next = Some(now),
            (false, _) => self.next = None,
            (true, Some(_)) => {}
        }
    }

    /// Whether the timer runs.
    pub fn enabled(&self) -> bool {
        self.next.is_some()
    }

    /// When the next check is due; `None` while the timer is off.
    pub fn deadline(&self) -> Option<Instant> {
        self.next
    }

    /// Whether a check is due at `now`; when it is, the next one is [`CHECK_EVERY`] later.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.next {
            Some(t) if t <= now => {
                self.next = Some(now + CHECK_EVERY);
                true
            }
            _ => false,
        }
    }
}

/// Which versions have had their toast in this run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToastRule {
    shown: Vec<String>,
}

impl ToastRule {
    /// The version `state` earns a toast for: an automatic check's find that has not had one.
    /// Nothing is recorded: [`ToastRule::shown`] does that once the toast is up.
    pub fn wants<'a>(&self, state: &'a UpdateState) -> Option<&'a str> {
        match state {
            UpdateState::Available {
                version,
                auto: true,
            } if !self.shown.iter().any(|v| v == version) => Some(version),
            _ => None,
        }
    }

    /// The toast for `version` is up: it gets no second one in this run.
    pub fn shown(&mut self, version: &str) {
        if !self.shown.iter().any(|v| v == version) {
            self.shown.push(version.to_owned());
        }
    }
}

/// Whether the Restart toast a toast-started download wants (`restart_toast_pending`) survives
/// `state`: only while that download runs or has staged. Any other state means it ended without
/// staging (stale feed, cancel, failure) or never started.
pub fn keeps_restart_toast(state: &UpdateState) -> bool {
    matches!(state, UpdateState::Downloading(_) | UpdateState::Ready(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(version: &str, auto: bool) -> UpdateState {
        UpdateState::Available {
            version: version.to_owned(),
            auto,
        }
    }

    #[test]
    fn a_download_that_ends_without_staging_drops_the_restart_toast() {
        assert!(keeps_restart_toast(&UpdateState::Downloading(0)));
        assert!(keeps_restart_toast(&UpdateState::Ready("0.9.0".into())));
        for state in [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
            available("0.9.0", false),
            UpdateState::Failed("x".into()),
            UpdateState::Unavailable(crate::update::Unavailable::NotInstalled),
        ] {
            assert!(!keeps_restart_toast(&state), "{state:?}");
        }
    }

    #[test]
    fn the_timer_is_off_until_switched_on_and_then_due_at_once() {
        let t0 = Instant::now();
        let mut timer = AutoCheck::default();
        assert!(!timer.enabled() && timer.deadline().is_none());
        assert!(!timer.take_due(t0), "an off timer is never due");
        timer.set_enabled(true, t0);
        assert_eq!(timer.deadline(), Some(t0));
        assert!(timer.take_due(t0), "the launch check");
        assert!(!timer.take_due(t0), "once per period");
    }

    #[test]
    fn the_timer_repeats_every_24_hours() {
        let t0 = Instant::now();
        let mut timer = AutoCheck::default();
        timer.set_enabled(true, t0);
        assert!(timer.take_due(t0));
        assert_eq!(timer.deadline(), Some(t0 + CHECK_EVERY));
        let almost = t0 + CHECK_EVERY - Duration::from_secs(1);
        assert!(!timer.take_due(almost));
        let on_time = t0 + CHECK_EVERY;
        assert!(timer.take_due(on_time));
        // A late tick (the machine slept): the next check is a period after it, not a burst.
        let late = on_time + CHECK_EVERY * 3;
        assert!(timer.take_due(late));
        assert!(!timer.take_due(late + Duration::from_secs(1)));
        assert_eq!(timer.deadline(), Some(late + CHECK_EVERY));
    }

    #[test]
    fn switching_off_cancels_the_timer_and_switching_on_again_checks_at_once() {
        let t0 = Instant::now();
        let mut timer = AutoCheck::default();
        timer.set_enabled(true, t0);
        assert!(timer.take_due(t0));
        timer.set_enabled(false, t0 + Duration::from_secs(5));
        assert!(!timer.enabled() && timer.deadline().is_none());
        assert!(!timer.take_due(t0 + CHECK_EVERY * 2), "off stays off");
        let later = t0 + Duration::from_secs(60);
        timer.set_enabled(true, later);
        assert!(timer.take_due(later));
    }

    #[test]
    fn switching_on_a_running_timer_keeps_its_schedule() {
        let t0 = Instant::now();
        let mut timer = AutoCheck::default();
        timer.set_enabled(true, t0);
        assert!(timer.take_due(t0));
        let deadline = timer.deadline();
        timer.set_enabled(true, t0 + Duration::from_secs(10));
        assert_eq!(timer.deadline(), deadline);
    }

    #[test]
    fn only_an_automatic_find_earns_a_toast() {
        let rule = ToastRule::default();
        assert_eq!(rule.wants(&available("0.9.0", true)), Some("0.9.0"));
        assert_eq!(
            rule.wants(&available("0.9.0", false)),
            None,
            "a manual check never toasts"
        );
        for other in [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
            UpdateState::Downloading(10),
            UpdateState::Ready("0.9.0".into()),
            UpdateState::Failed("x".into()),
            UpdateState::Unavailable(super::super::Unavailable::NotInstalled),
        ] {
            assert_eq!(rule.wants(&other), None, "{other:?}");
        }
    }

    #[test]
    fn each_version_gets_one_toast_per_run() {
        let mut rule = ToastRule::default();
        let a = available("0.9.0", true);
        assert_eq!(rule.wants(&a), Some("0.9.0"));
        // Asking does not record: a toast that could not be shown is asked for again.
        assert_eq!(rule.wants(&a), Some("0.9.0"));
        rule.shown("0.9.0");
        assert_eq!(rule.wants(&a), None, "the same version, a day later");
        // A newer version is another toast.
        assert_eq!(rule.wants(&available("0.9.1", true)), Some("0.9.1"));
        // A manual find of a version that was toasted stays quiet too.
        assert_eq!(rule.wants(&available("0.9.0", false)), None);
        rule.shown("0.9.0");
        assert_eq!(rule, {
            let mut r = ToastRule::default();
            r.shown("0.9.0");
            r
        });
    }
}
