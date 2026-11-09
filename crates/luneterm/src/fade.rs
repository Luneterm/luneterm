//! The tab-switch crossfade: the terminal content of the tab you leave fades out while the one you
//! enter fades in. The glass and the chrome behind stay still (the strip's own notch slides
//! alongside, luneui-rs D291).
//!
//! Spec: `docs/glass-styling.md` ("Tab switch: 0.25 s sine ease-in-out crossfade") and Lunegit's
//! `TerminalPanelViewModel` ("Tab-switch crossfade (Phase 4)"), which this ports:
//!
//! - Two layers at once: the incoming tab's opacity goes to 1 and every other tab still on screen
//!   goes to 0, all with the same duration and easing, each **from the opacity it has now** (an
//!   Avalonia transition retargets from the current value), so a quick second switch continues
//!   from where the first was and never jumps. Lunegit's `_fadeGeneration` is the same rule in
//!   another shape: a newer switch owns every layer, and the older one's pending hide does
//!   nothing. Here there is no deferred hide to cancel; a switch rebases every animation.
//! - Duration by the motion level (`FadeDurationFor`): `Full` 250 ms, `Reduced` 180 ms (it still
//!   fades: it is opacity only), `None` 0, a hard cut.
//! - Workspace restore and the first tab do not fade: the caller does not start a fade for them.
//!
//! Everything here is pure (the caller passes the clock), so the rules are unit-tested.

use std::time::{Duration, Instant};

use crate::settings::Motion;
use crate::tabs::TabId;

/// The crossfade at the `Full` motion level (Lunegit `AnimDurationBase`).
pub const FULL: Duration = Duration::from_millis(250);
/// The crossfade at the `Reduced` level (Lunegit `AnimDurationFast`).
pub const REDUCED: Duration = Duration::from_millis(180);
/// Layers that may still fade out under the active one. Switching faster than the fade piles
/// layers up; beyond this the faintest is dropped (it is the one whose disappearance shows least).
pub const MAX_OUTGOING: usize = 2;

/// The crossfade duration for a motion level; zero is a hard cut.
pub fn duration(motion: Motion) -> Duration {
    match motion {
        Motion::Full => FULL,
        Motion::Reduced => REDUCED,
        Motion::None => Duration::ZERO,
    }
}

/// Sine ease-in-out of `t` in 0..=1 (clamped), the curve of the spike and of Lunegit's transition.
pub fn ease(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

/// How far a fade of `total` has come after `elapsed`, eased, 0..=1. A zero duration is done.
pub fn progress(elapsed: Duration, total: Duration) -> f32 {
    if total.is_zero() {
        return 1.0;
    }
    ease(elapsed.as_secs_f32() / total.as_secs_f32())
}

/// One opacity moving from `from` to `to`, started at `start`.
#[derive(Debug, Clone, Copy)]
struct Anim {
    from: f32,
    to: f32,
    start: Instant,
}

impl Anim {
    fn value(&self, now: Instant, total: Duration) -> f32 {
        let p = progress(now.saturating_duration_since(self.start), total);
        self.from + (self.to - self.from) * p
    }

    fn done(&self, now: Instant, total: Duration) -> bool {
        now.saturating_duration_since(self.start) >= total
    }
}

/// The opacities to draw one frame with.
#[derive(Debug, Clone, PartialEq)]
pub struct Alphas {
    /// The active tab's grid.
    pub incoming: f32,
    /// The grids still fading out under it, by tab.
    pub outgoing: Vec<(TabId, f32)>,
}

impl Alphas {
    /// Nothing fades: the active tab at full opacity.
    pub fn settled() -> Alphas {
        Alphas {
            incoming: 1.0,
            outgoing: Vec::new(),
        }
    }
}

/// The running crossfade, if any. Idle is the default and costs nothing.
#[derive(Debug, Default)]
pub struct Fade {
    total: Duration,
    incoming: Option<Anim>,
    outgoing: Vec<(TabId, Anim)>,
}

impl Fade {
    /// The active tab changed to `new` at `now`; `old` is the tab that was active if it is still
    /// there (a tab that was closed has nothing to fade out). `total` is the level's duration;
    /// zero cuts every running fade short.
    pub fn switch(&mut self, now: Instant, total: Duration, old: Option<TabId>, new: TabId) {
        let at_now = self.sample(now);
        if total.is_zero() {
            *self = Fade::default();
            return;
        }
        let mut outgoing = at_now.outgoing;
        // Going back to a tab that is still fading out: it continues from the opacity it has.
        let back = outgoing
            .iter()
            .position(|(id, _)| *id == new)
            .map(|i| outgoing.remove(i).1);
        if let Some(old) = old.filter(|o| *o != new) {
            outgoing.retain(|(id, _)| *id != old);
            outgoing.push((old, at_now.incoming));
        }
        outgoing.retain(|(_, v)| *v > 0.0);
        while outgoing.len() > MAX_OUTGOING {
            let faintest = outgoing
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
                .map(|(i, _)| i);
            if let Some(i) = faintest {
                outgoing.remove(i);
            }
        }
        self.total = total;
        self.incoming = Some(Anim {
            from: back.unwrap_or(0.0),
            to: 1.0,
            start: now,
        });
        self.outgoing = outgoing
            .into_iter()
            .map(|(id, v)| {
                (
                    id,
                    Anim {
                        from: v,
                        to: 0.0,
                        start: now,
                    },
                )
            })
            .collect();
    }

    fn sample(&self, now: Instant) -> Alphas {
        Alphas {
            incoming: self.incoming.map_or(1.0, |a| a.value(now, self.total)),
            outgoing: self
                .outgoing
                .iter()
                .map(|(id, a)| (*id, a.value(now, self.total)))
                .collect(),
        }
    }

    /// The opacities for a frame at `now`, with the layers that are gone (`alive` says a tab
    /// still exists) or finished dropped. Also reports whether another frame is needed: `true`
    /// until every animation has reached its end, so an idle window asks for no redraw.
    pub fn tick(&mut self, now: Instant, alive: impl Fn(TabId) -> bool) -> (Alphas, bool) {
        self.outgoing.retain(|(id, _)| alive(*id));
        let alphas = self.sample(now);
        let total = self.total;
        let running = self.incoming.is_some_and(|a| !a.done(now, total))
            || self.outgoing.iter().any(|(_, a)| !a.done(now, total));
        let outgoing = alphas
            .outgoing
            .into_iter()
            .filter(|(_, v)| *v > 0.0)
            .collect();
        if !running {
            *self = Fade::default();
        }
        (
            Alphas {
                incoming: alphas.incoming,
                outgoing,
            },
            running,
        )
    }

    /// Whether a fade is in progress (as of the last tick or switch).
    pub fn is_running(&self) -> bool {
        self.incoming.is_some() || !self.outgoing.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    const ALL: fn(TabId) -> bool = |_| true;

    #[test]
    fn the_curve_is_sine_in_out_and_progress_is_clamped() {
        assert_eq!(ease(0.0), 0.0);
        assert!(close(ease(0.5), 0.5));
        assert!(close(ease(1.0), 1.0));
        assert_eq!(ease(-3.0), 0.0);
        assert!(close(ease(9.0), 1.0));
        assert!(ease(0.25) < 0.25, "slow start");
        assert!(ease(0.75) > 0.75, "slow end");
        assert!(close(progress(Duration::from_millis(125), FULL), 0.5));
        assert_eq!(progress(Duration::from_millis(5), Duration::ZERO), 1.0);
    }

    #[test]
    fn the_duration_follows_the_motion_level() {
        assert_eq!(duration(Motion::Full), Duration::from_millis(250));
        assert_eq!(duration(Motion::Reduced), Duration::from_millis(180));
        assert_eq!(duration(Motion::None), Duration::ZERO);
    }

    #[test]
    fn a_switch_crosses_over_and_then_goes_idle() {
        let t0 = Instant::now();
        let mut f = Fade::default();
        assert!(!f.is_running());
        f.switch(t0, FULL, Some(1), 2);
        let (a, running) = f.tick(t0, ALL);
        assert!(running);
        assert!(close(a.incoming, 0.0));
        assert_eq!(a.outgoing.len(), 1);
        assert_eq!(a.outgoing[0].0, 1);
        assert!(close(a.outgoing[0].1, 1.0));
        let (a, running) = f.tick(at(t0, 125), ALL);
        assert!(running);
        assert!(close(a.incoming, 0.5));
        assert!(close(a.outgoing[0].1, 0.5));
        let (a, running) = f.tick(at(t0, 250), ALL);
        assert!(!running, "done at the end: no more frames wanted");
        assert!(close(a.incoming, 1.0));
        assert!(a.outgoing.is_empty());
        assert!(!f.is_running());
        // Idle afterwards.
        let (a, running) = f.tick(at(t0, 900), ALL);
        assert_eq!(a, Alphas::settled());
        assert!(!running);
    }

    #[test]
    fn a_second_switch_continues_from_where_the_first_was() {
        let t0 = Instant::now();
        let mut f = Fade::default();
        f.switch(t0, FULL, Some(1), 2);
        // Half way: tab 2 is at 0.5 in, tab 1 at 0.5 out. Now 2 -> 3.
        f.switch(at(t0, 125), FULL, Some(2), 3);
        let (a, running) = f.tick(at(t0, 125), ALL);
        assert!(running);
        assert!(close(a.incoming, 0.0), "tab 3 starts from nothing");
        // -1 when the tab is not among the fading ones.
        let of = |a: &Alphas, id| {
            a.outgoing
                .iter()
                .find(|(i, _)| *i == id)
                .map_or(-1.0, |(_, v)| *v)
        };
        assert!(close(of(&a, 2), 0.5), "tab 2 leaves from 0.5");
        assert!(close(of(&a, 1), 0.5), "tab 1 is rebased, not dropped");
        // Both fade out together to nothing over a fresh duration.
        let (a, _) = f.tick(at(t0, 125 + 125), ALL);
        assert!(close(a.incoming, 0.5));
        assert!(close(of(&a, 2), 0.25));
        let (a, running) = f.tick(at(t0, 125 + 250), ALL);
        assert!(!running);
        assert_eq!(a.outgoing, vec![]);
        assert!(close(a.incoming, 1.0));
    }

    #[test]
    fn switching_back_to_a_fading_tab_resumes_its_opacity() {
        let t0 = Instant::now();
        let mut f = Fade::default();
        f.switch(t0, FULL, Some(1), 2);
        // At 0.5, 2 -> 1: tab 1 was leaving at 0.5 and comes back from there.
        f.switch(at(t0, 125), FULL, Some(2), 1);
        let (a, _) = f.tick(at(t0, 125), ALL);
        assert!(close(a.incoming, 0.5), "{a:?}");
        assert!(a.outgoing.iter().all(|(id, _)| *id != 1));
        assert!(a.outgoing.iter().any(|(id, v)| *id == 2 && close(*v, 0.5)));
    }

    #[test]
    fn the_pile_is_bounded_and_the_faintest_layer_goes() {
        let t0 = Instant::now();
        let mut f = Fade::default();
        // A rapid run of switches with no time between them: each old tab enters at the
        // incoming's value (0 at the same instant, so nothing to pile up).
        for (old, new) in [(1, 2), (2, 3), (3, 4), (4, 5)] {
            f.switch(t0, FULL, Some(old), new);
            let (a, _) = f.tick(t0, ALL);
            assert!(a.outgoing.len() <= MAX_OUTGOING, "{a:?}");
        }
        // With time between them the layers have opacity and the cap shows.
        let mut f = Fade::default();
        f.switch(t0, FULL, Some(1), 2);
        f.switch(at(t0, 100), FULL, Some(2), 3);
        f.switch(at(t0, 140), FULL, Some(3), 4);
        f.switch(at(t0, 160), FULL, Some(4), 5);
        let (a, _) = f.tick(at(t0, 160), ALL);
        assert_eq!(a.outgoing.len(), MAX_OUTGOING, "{a:?}");
        let faintest_kept = a.outgoing.iter().map(|(_, v)| *v).fold(f32::MAX, f32::min);
        assert!(faintest_kept > 0.0);
    }

    #[test]
    fn a_closed_tab_is_not_drawn_and_a_hard_cut_has_no_fade() {
        let t0 = Instant::now();
        let mut f = Fade::default();
        f.switch(t0, FULL, Some(1), 2);
        // Tab 1 was closed during the fade: nothing to draw for it.
        let (a, running) = f.tick(at(t0, 50), |id| id != 1);
        assert!(a.outgoing.is_empty());
        assert!(running, "the incoming one is still arriving");
        // The level is None: a switch cuts any running fade and starts none.
        f.switch(at(t0, 60), Duration::ZERO, Some(2), 3);
        assert!(!f.is_running());
        let (a, running) = f.tick(at(t0, 60), ALL);
        assert_eq!(a, Alphas::settled());
        assert!(!running);
    }

    #[test]
    fn a_tab_that_replaced_a_closed_active_one_fades_in_alone() {
        // Lunegit: the incoming opacity animates from its hidden value 0 whether or not a tab is
        // still on screen to fade out.
        let t0 = Instant::now();
        let mut f = Fade::default();
        f.switch(t0, FULL, None, 2);
        let (a, running) = f.tick(t0, ALL);
        assert!(running);
        assert!(close(a.incoming, 0.0));
        assert!(a.outgoing.is_empty());
    }
}
