//! The OS side of the tab-switch trace (`LUNETERM_TRACE_SWITCH=1`, plan D11, step 3 pass 3): how
//! long an input event waited between the moment the OS stamped it and the moment winit handed it
//! to the app. winit's `WindowEvent` carries no timestamp, so on macOS this reads the `NSEvent`
//! AppKit is dispatching (`NSApp.currentEvent`, the event winit's view is translating while the
//! app handles it) and compares its `timestamp` with `NSProcessInfo.systemUptime`: both are
//! seconds of the same uptime clock.
//!
//! The known-answer check is `SynthClicks`: a thread of its own posts left-button clicks into the
//! app's own event queue (`postEvent:atStart:`, no permission needed, nothing leaves the process)
//! stamped with the uptime at posting, and keeps the `Instant` it posted each one at. The age the
//! `NSEvent` timestamp gives and the age the `Instant` gives must agree; on an idle app both must
//! be small.
//!
//! Trace-only: nothing here runs when the variable is unset. Numbers only, never event content
//! (no positions, no keys). Other platforms read nothing (`None`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the app was handling when it asked for the event's age, so a mismatch with the event
/// AppKit is dispatching (an event winit queued and handed over later) reads as "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    /// A mouse button went down.
    Press,
    /// The pointer moved (or dragged; also the motion winit sends ahead of a press).
    Motion,
    /// A key went down.
    Key,
}

/// The age of the event being handled, as the OS stamped it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OsAge {
    /// ms between the OS timestamp and now.
    pub ms: f64,
    /// The event's OS timestamp (s of uptime): what tells a synthetic click from a real one.
    pub stamp: f64,
}

const RING: usize = 64;

/// The last `RING` posted clicks (stamp bits, posting `Instant`) and the next slot to write.
type Ring = ([Option<(u64, Instant)>; RING], usize);

/// The clicks posted by the trace's own thread: each one's timestamp and the `Instant` it was
/// posted at. (AppKit renumbers posted events, so the timestamp is the tag.)
#[derive(Debug)]
pub(crate) struct SynthClicks {
    posted: Mutex<Ring>,
}

impl SynthClicks {
    /// If `stamp` is a posted click's: ms since it was posted, by `Instant` (the second clock of
    /// the known-answer check).
    pub(crate) fn posted_ms_ago(&self, stamp: f64, now: Instant) -> Option<f64> {
        let ring = self.posted.lock().ok()?;
        let (_, at) = ring
            .0
            .iter()
            .flatten()
            // AppKit keeps the stamp in ns: equal within 1 us (a real event that close is unlikely).
            .find(|(s, _)| (f64::from_bits(*s) - stamp).abs() < 1e-6)?;
        Some(now.saturating_duration_since(*at).as_secs_f64() * 1e3)
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn record(&self, stamp: f64, at: Instant) {
        if let Ok(mut ring) = self.posted.lock() {
            let i = ring.1;
            ring.0[i] = Some((stamp.to_bits(), at));
            ring.1 = (i + 1) % RING;
        }
    }
}

/// The age of the event AppKit is dispatching, if it is the `kind` the app is handling. Must be
/// called on the main thread, from inside a winit callback; `None` elsewhere or off macOS.
pub(crate) fn current_age(kind: InputKind) -> Option<OsAge> {
    imp::current_age(kind)
}

/// Starts a thread that posts one left click (down, then up) every `every` into `window`, at the
/// centre of its upper half (the terminal area, below the tab strip). `None` off macOS or when
/// the window has no AppKit handle.
pub(crate) fn start_synth_clicks(
    window: &winit::window::Window,
    every: Duration,
) -> Option<Arc<SynthClicks>> {
    imp::start_synth_clicks(window, every)
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub(super) fn current_age(_kind: InputKind) -> Option<OsAge> {
        None
    }

    pub(super) fn start_synth_clicks(
        _window: &winit::window::Window,
        _every: Duration,
    ) -> Option<Arc<SynthClicks>> {
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    #![allow(unsafe_code)]

    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, class, msg_send};
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSView};
    use objc2_foundation::{NSPoint, NSProcessInfo};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    fn uptime() -> f64 {
        NSProcessInfo::processInfo().systemUptime()
    }

    fn matches(kind: InputKind, ty: NSEventType) -> bool {
        match kind {
            InputKind::Press => [
                NSEventType::LeftMouseDown,
                NSEventType::RightMouseDown,
                NSEventType::OtherMouseDown,
            ]
            .contains(&ty),
            InputKind::Motion => [
                NSEventType::MouseMoved,
                NSEventType::LeftMouseDragged,
                NSEventType::RightMouseDragged,
                NSEventType::OtherMouseDragged,
                NSEventType::LeftMouseDown,
                NSEventType::RightMouseDown,
                NSEventType::OtherMouseDown,
                NSEventType::LeftMouseUp,
                NSEventType::RightMouseUp,
                NSEventType::OtherMouseUp,
            ]
            .contains(&ty),
            InputKind::Key => ty == NSEventType::KeyDown,
        }
    }

    pub(super) fn current_age(kind: InputKind) -> Option<OsAge> {
        let mtm = MainThreadMarker::new()?;
        let event = NSApplication::sharedApplication(mtm).currentEvent()?;
        let ty = event.r#type();
        if !matches(kind, ty) {
            return None;
        }
        let stamp = event.timestamp();
        Some(OsAge {
            ms: (uptime() - stamp) * 1e3,
            stamp,
        })
    }

    pub(super) fn start_synth_clicks(
        window: &winit::window::Window,
        every: Duration,
    ) -> Option<Arc<SynthClicks>> {
        let _mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: on the main thread (`_mtm`), `ns_view` is the live `NSView` of `window`, which
        // the borrow keeps alive for this block; it is only read here.
        let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
        let ns_window = view.window()?;
        let number = ns_window.windowNumber();
        let frame = view.frame();
        let at = view.convertPoint_toView(
            NSPoint::new(frame.size.width / 2.0, frame.size.height * 0.5),
            None,
        );
        let clicks = Arc::new(SynthClicks {
            posted: Mutex::new(([None; RING], 0)),
        });
        let ring = Arc::clone(&clicks);
        std::thread::Builder::new()
            .name("luneterm-trace-click".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(every);
                    let now = uptime();
                    let make = |ty| {
                        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                            ty,
                            at,
                            NSEventModifierFlags::empty(),
                            now,
                            number,
                            None,
                            0,
                            1,
                            1.0,
                        )
                    };
                    let (Some(down), Some(up)) =
                        (make(NSEventType::LeftMouseDown), make(NSEventType::LeftMouseUp))
                    else {
                        return;
                    };
                    ring.record(now, Instant::now());
                    // SAFETY: `NSApp` exists (winit made it before the loop ran) and is never
                    // released; `postEvent:atStart:` is documented as callable from subthreads
                    // and only appends to the app's own event queue. The object is used as a
                    // plain `NSObject` here, never through a main-thread-only method.
                    unsafe {
                        let app: Option<Retained<AnyObject>> =
                            msg_send![class!(NSApplication), sharedApplication];
                        let Some(app) = app else { return };
                        let _: () = msg_send![&*app, postEvent: &*down, atStart: false];
                        let _: () = msg_send![&*app, postEvent: &*up, atStart: false];
                    }
                }
            })
            .ok()?;
        Some(clicks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A posted click is found by its stamp to within AppKit's ns rounding, never by a stamp
    /// 2 us away; the ring keeps the last `RING` clicks.
    #[test]
    fn a_synthetic_click_is_told_by_its_stamp() {
        let clicks = SynthClicks {
            posted: Mutex::new(([None; RING], 0)),
        };
        let at = Instant::now();
        clicks.record(1234.5, at);
        let later = at + Duration::from_millis(7);
        let ms = clicks
            .posted_ms_ago(1234.5 + 4e-7, later)
            .expect("same click");
        assert!((ms - 7.0).abs() < 1e-6, "{ms}");
        assert_eq!(clicks.posted_ms_ago(1234.5 + 2e-6, later), None);
        for i in 0..RING {
            clicks.record(2000.0 + i as f64, at);
        }
        assert_eq!(clicks.posted_ms_ago(1234.5, later), None, "overwritten");
        assert!(clicks.posted_ms_ago(2000.0, later).is_some());
    }

    /// Off the main thread there is no event being dispatched: no age.
    #[test]
    fn no_age_off_the_main_thread() {
        assert_eq!(current_age(InputKind::Press), None);
    }
}
