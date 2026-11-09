//! Input: winit events mapped to what the session takes (plan M9 step 5, decisions D4, D5, D7).
//!
//! Everything that decides is a pure function over plain data and is table-tested; the winit glue
//! (one function per event kind) only copies fields. Keys are in [`keys`], the app's own shortcuts
//! in [`shortcuts`], the mouse and the wheel in [`pointer`].

pub mod keys;
pub mod pointer;
pub mod shortcuts;

pub use keys::{KeyInput, KeyRoute, RouteCtx, key_input, route_key, terminal_mods, toolkit_mods};
pub use pointer::{
    ClickCounter, MouseCtx, MouseState, PointerEvent, WheelAction, WheelCtx, WheelDelta,
    WheelState, mouse_to_terminal, pointer_event, selection_position, selection_scroll, wheel,
    wheel_delta, wheel_report,
};
pub use shortcuts::{AppCommand, AppShortcuts};

/// The `OptionAsAlt` a window needs for the "Use Option as Meta" setting (D5 deviation 1): `Both`
/// when on, so winit hands the key over as Alt+key instead of feeding it to the dead-key input
/// method, and `None` when off, so Option composes text. The window code applies it with
/// `WindowExtMacOS::set_option_as_alt` at window creation and whenever the setting changes.
#[cfg(target_os = "macos")]
pub fn option_as_alt(option_as_meta: bool) -> winit::platform::macos::OptionAsAlt {
    use winit::platform::macos::OptionAsAlt;
    if option_as_meta {
        OptionAsAlt::Both
    } else {
        OptionAsAlt::None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use winit::platform::macos::OptionAsAlt;

    #[test]
    fn the_setting_picks_the_option_as_alt_mode() {
        assert_eq!(option_as_alt(true), OptionAsAlt::Both);
        assert_eq!(option_as_alt(false), OptionAsAlt::None);
    }
}
