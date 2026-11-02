//! The mouse event type the host passes in (plan M7 step 6, D5). Lunegit's `TerminalMouseButton`,
//! `TerminalMouseEventType` (`TerminalInput.cs`) mapped onto the ported upstream
//! `CoreMouseService`, which encodes from the engine's own DECSET protocol and encoding state
//! (X10 `?9`, VT200 `?1000`, drag `?1002`, any `?1003`; default, SGR `?1006`, SGR pixels `?1016`).
//!
//! Coordinates: `col` and `row` are zero-based cell positions (the top-left cell is 0, 0), as
//! Lunegit's `TerminalMouseEvent` and `CoreMouseService` both take them; the encoders add one
//! (the golden check is a left press at column 5, row 3 under SGR: `ESC [ < 0 ; 6 ; 4 M`).
//! `x` and `y` are pixel positions, read only by SGR pixels (`?1016`).
//!
//! Where the service differs from Lunegit's own encoder (kept, D5): a position outside the
//! terminal sends nothing (Lunegit clamped it); the default encoding sends nothing past column or
//! row 223 (Lunegit clamped); the held button of a drag comes from the event, not from state kept
//! across events.

use super::key_event::TerminalModifiers;
use crate::services::core_mouse_service::{CoreMouseAction, CoreMouseButton, CoreMouseEvent};

/// Mouse buttons (Lunegit's `TerminalMouseButton`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalMouseButton {
    None,
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// What the pointer did (Lunegit's `TerminalMouseEventType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalMouseEventType {
    /// A button went down (`button` names it).
    Down,
    /// A button went up (`button` names it).
    Up,
    /// Motion with no button held.
    Move,
    /// Motion with `button` held.
    Drag,
    /// Wheel up (`button` is ignored).
    WheelUp,
    /// Wheel down (`button` is ignored).
    WheelDown,
}

/// One pointer event, for [`crate::Terminal::mouse_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalMouseEvent {
    pub button: TerminalMouseButton,
    pub kind: TerminalMouseEventType,
    /// Zero-based column.
    pub col: i32,
    /// Zero-based row.
    pub row: i32,
    /// Pixel x (SGR pixels only).
    pub x: i32,
    /// Pixel y (SGR pixels only).
    pub y: i32,
    pub modifiers: TerminalModifiers,
}

impl TerminalMouseEvent {
    /// An event at a cell, with no pixel position and no modifiers.
    pub const fn new(
        button: TerminalMouseButton,
        kind: TerminalMouseEventType,
        col: i32,
        row: i32,
    ) -> TerminalMouseEvent {
        TerminalMouseEvent {
            button,
            kind,
            col,
            row,
            x: 0,
            y: 0,
            modifiers: TerminalModifiers::NONE,
        }
    }

    /// The service's event, or `None` for a combination that is nonsense: a press or release
    /// with no button or with a wheel as the button, and a drag with a wheel as the button.
    /// (A `Move` ignores `button`; a drag with no button is a plain move.)
    pub(crate) fn to_core(self) -> Option<CoreMouseEvent> {
        use TerminalMouseButton as B;
        use TerminalMouseEventType as K;
        let is_wheel_button = matches!(self.button, B::WheelUp | B::WheelDown);
        let held = match self.button {
            B::Left => CoreMouseButton::Left,
            B::Middle => CoreMouseButton::Middle,
            B::Right => CoreMouseButton::Right,
            B::None | B::WheelUp | B::WheelDown => CoreMouseButton::None,
        };
        let (button, action) = match self.kind {
            K::Down | K::Up if held == CoreMouseButton::None => return None,
            K::Drag if is_wheel_button => return None,
            K::Down => (held, CoreMouseAction::Down),
            K::Up => (held, CoreMouseAction::Up),
            K::Move => (CoreMouseButton::None, CoreMouseAction::Move),
            K::Drag => (held, CoreMouseAction::Move),
            K::WheelUp => (CoreMouseButton::Wheel, CoreMouseAction::Up),
            K::WheelDown => (CoreMouseButton::Wheel, CoreMouseAction::Down),
        };
        Some(CoreMouseEvent {
            col: i64::from(self.col),
            row: i64::from(self.row),
            x: i64::from(self.x),
            y: i64::from(self.y),
            button,
            action,
            ctrl: self.modifiers.contains(TerminalModifiers::CONTROL),
            alt: self.modifiers.contains(TerminalModifiers::ALT),
            shift: self.modifiers.contains(TerminalModifiers::SHIFT),
        })
    }
}

#[cfg(test)]
#[path = "mouse_event_tests.rs"]
mod tests;
