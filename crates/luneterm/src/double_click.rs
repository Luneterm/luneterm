//! What a double click on the title band does where the app, not the OS, sees it.
//!
//! macOS: the band is the app's own drawing under a transparent title bar, and a press there drags
//! the window by `drag_window`, so the OS never sees the double click of its own title bar. The app
//! does what System Settings > Desktop & Dock > "Double-click a window's title bar to" asks: the
//! user default `AppleActionOnDoubleClick` (`Maximize` = zoom, the default when unset; `Minimize`;
//! `None`/`Do Nothing`), or, without it, the older `AppleMiniaturizeOnDoubleClick` boolean.

use lune_ui::input::WindowCommand;

/// The command for the macOS setting's value (`action`, `AppleActionOnDoubleClick`) and the older
/// boolean (`miniaturize`, `AppleMiniaturizeOnDoubleClick`). `None`: the click does nothing.
pub fn mac_action(action: Option<&str>, miniaturize: bool) -> Option<WindowCommand> {
    match action {
        Some("Maximize") => Some(WindowCommand::ToggleMaximize),
        Some("Minimize") => Some(WindowCommand::Minimize),
        Some(_) => None,
        None if miniaturize => Some(WindowCommand::Minimize),
        None => Some(WindowCommand::ToggleMaximize),
    }
}

/// The OS setting, read on each double click (a change in System Settings applies at once).
#[cfg(target_os = "macos")]
pub fn system_action() -> Option<WindowCommand> {
    macos::system_action()
}

/// Only macOS asks the app; elsewhere the band's double click is the app's own maximize.
#[cfg(not(target_os = "macos"))]
pub fn system_action() -> Option<WindowCommand> {
    Some(WindowCommand::ToggleMaximize)
}

#[cfg(target_os = "macos")]
mod macos {
    use lune_ui::input::WindowCommand;
    use objc2_foundation::{NSString, NSUserDefaults};

    pub(super) fn system_action() -> Option<WindowCommand> {
        let defaults = NSUserDefaults::standardUserDefaults();
        let action = defaults
            .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
            .map(|s| s.to_string());
        let miniaturize = defaults.boolForKey(&NSString::from_str("AppleMiniaturizeOnDoubleClick"));
        super::mac_action(action.as_deref(), miniaturize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macos_setting_maps_to_a_window_command() {
        let max = Some(WindowCommand::ToggleMaximize);
        let min = Some(WindowCommand::Minimize);
        assert_eq!(mac_action(None, false), max, "unset = the default, zoom");
        assert_eq!(mac_action(None, true), min);
        assert_eq!(mac_action(Some("Maximize"), true), max, "the new key wins");
        assert_eq!(mac_action(Some("Minimize"), false), min);
        assert_eq!(mac_action(Some("None"), false), None);
        assert_eq!(mac_action(Some("Do Nothing"), false), None);
    }
}
