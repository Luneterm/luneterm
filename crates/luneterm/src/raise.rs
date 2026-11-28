//! Bringing the app itself forward for a `ctl tab focus` or `tab new --focus`, next to the window
//! calls the app makes.
//!
//! macOS needs it: winit's `focus_window` does nothing for a window that is not visible (the app is
//! hidden, Cmd+H) or is still miniaturized (the way back animates), and `set_minimized(false)` only
//! asks for the way back. Neither unhides nor activates the app. Elsewhere the window calls are all
//! there is.

/// Unhides and activates the app, from the UI thread. The OS may decline.
#[cfg(target_os = "macos")]
pub(crate) fn app_to_front() {
    macos::app_to_front();
}

/// Nothing to do: the window calls restore and focus the window.
#[cfg(not(target_os = "macos"))]
pub(crate) fn app_to_front() {}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    pub(super) fn app_to_front() {
        // AppKit is the main thread's, where the UI thread is; from another nothing is done.
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.unhide(None);
        // The newer `activate` waits for the app in front to yield, which it does not do for a
        // request that came from another process. This is the call winit's `focus_window` makes.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}
