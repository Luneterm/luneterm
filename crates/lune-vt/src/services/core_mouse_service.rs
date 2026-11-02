//! Port of xterm.js src/common/services/CoreMouseService.ts @ f447274 (tests:
//! CoreMouseService.test.ts), with the mouse types from `src/common/Types.ts`
//! (`CoreMouseButton`, `CoreMouseAction`, `ICoreMouseEvent`, `CoreMouseEventType`,
//! `ICoreMouseProtocol`, `CoreMouseEncoding`).
//!
//! Rust notes (see `docs/porting-xterm.md`, plan M6 D2):
//!  - `onProtocolChange` is a return value: [`CoreMouseService::set_active_protocol`] and
//!    [`CoreMouseService::reset`] return the new protocol's [`CoreMouseEventType`] (upstream
//!    fires on every successful assignment).
//!  - The `activeProtocol`/`activeEncoding` setters throw on unknown names; here they return
//!    [`MouseError`].
//!  - `triggerMouseEvent` calls `coreService.triggerBinaryEvent`/`triggerDataEvent(.., true)` and
//!    returns `true`/`false`. Here the caller routes the returned [`MouseReport`] through
//!    `CoreService`; `Some(_)` is upstream's `true`, `None` its `false`. A report the encoding
//!    left empty (DEFAULT beyond column/row 223) is `Some(MouseReport::Suppressed)`: upstream
//!    returns `true` and sends nothing. The event is taken by reference; upstream mutates the
//!    caller's object (1-based coordinates, X10 clearing the modifiers), which nothing reads
//!    afterwards, so the changes are applied to a copy that is stored as the last event.
//!  - The buffer size (`bufferService.cols/rows`) is passed in; `consume_wheel_event` also takes
//!    `rows` (the `DOM_DELTA_PAGE` multiplier) and the options.
//!  - `ICoreMouseEvent.ctrl/alt/shift` are optional upstream (`undefined` != `false` when two move
//!    events are compared); here they are plain `bool`s, `undefined` being `false`.
//!  - `consumeWheelEvent` returns a JS number, which is fractional in `DOM_DELTA_LINE` mode with a
//!    fractional sensitivity; the contract is an `i64`, so that result is truncated toward zero.
//!    `DOM_DELTA_PIXEL` results are already whole numbers.
//!  - Protocols and encodings are kept in registration order (a JS object's key order), and
//!    registering an existing name replaces it in place.

use crate::services::options_service::TerminalOptions;
use std::fmt;

/// `CoreMouseButton` (`Types.ts:288`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoreMouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    None = 3,
    Wheel = 4,
    Aux1 = 8,
    Aux2 = 9,
    Aux3 = 10,
    Aux4 = 11,
    Aux5 = 12,
    Aux6 = 13,
    Aux7 = 14,
    Aux8 = 15,
}

/// `CoreMouseAction` (`Types.ts:306`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoreMouseAction {
    /// Buttons, wheel.
    Up = 0,
    /// Buttons, wheel.
    Down = 1,
    /// Wheel only.
    Left = 2,
    /// Wheel only.
    Right = 3,
    /// Buttons only.
    Move = 32,
}

/// `ICoreMouseEvent` (`Types.ts:314`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreMouseEvent {
    /// Column (zero based).
    pub col: i64,
    /// Row (zero based).
    pub row: i64,
    /// Pixel position.
    pub x: i64,
    pub y: i64,
    pub button: CoreMouseButton,
    pub action: CoreMouseAction,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl CoreMouseEvent {
    /// An event at the given cell with no pixel position and no modifiers.
    pub fn new(col: i64, row: i64, button: CoreMouseButton, action: CoreMouseAction) -> Self {
        CoreMouseEvent {
            col,
            row,
            x: 0,
            y: 0,
            button,
            action,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
}

/// `CoreMouseEventType` (`Types.ts:346`): a bit set of the events a protocol wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoreMouseEventType(pub u8);

impl CoreMouseEventType {
    pub const NONE: CoreMouseEventType = CoreMouseEventType(0);
    /// Any mousedown event.
    pub const DOWN: CoreMouseEventType = CoreMouseEventType(1);
    /// Any mouseup event.
    pub const UP: CoreMouseEventType = CoreMouseEventType(2);
    /// Any mousemove event while a button is held.
    pub const DRAG: CoreMouseEventType = CoreMouseEventType(4);
    /// Any mousemove event without a button.
    pub const MOVE: CoreMouseEventType = CoreMouseEventType(8);
    /// Any wheel event.
    pub const WHEEL: CoreMouseEventType = CoreMouseEventType(16);

    /// Whether every bit of `other` is set.
    pub fn contains(self, other: CoreMouseEventType) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for CoreMouseEventType {
    type Output = CoreMouseEventType;
    fn bitor(self, rhs: CoreMouseEventType) -> CoreMouseEventType {
        CoreMouseEventType(self.0 | rhs.0)
    }
}

/// `explainEvents`' result: which events a [`CoreMouseEventType`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplainedEvents {
    pub down: bool,
    pub up: bool,
    pub drag: bool,
    pub move_: bool,
    pub wheel: bool,
}

/// `ICoreMouseProtocol` (`Types.ts:368`).
pub struct CoreMouseProtocol {
    /// The events the protocol wants, a hint for the host.
    pub events: CoreMouseEventType,
    /// Applies protocol restrictions (may adjust the event); `false` drops the event.
    pub restrict: Box<dyn Fn(&mut CoreMouseEvent) -> bool>,
}

impl fmt::Debug for CoreMouseProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreMouseProtocol")
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

/// `CoreMouseEncoding` (`Types.ts:382`): an empty string suppresses the report.
pub type CoreMouseEncoding = Box<dyn Fn(&CoreMouseEvent) -> String>;

/// The throw sites of the `activeProtocol`/`activeEncoding` setters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseError {
    /// `unknown protocol "<name>"`.
    UnknownProtocol(String),
    /// `unknown encoding "<name>"`.
    UnknownEncoding(String),
}

impl fmt::Display for MouseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MouseError::UnknownProtocol(name) => write!(f, "unknown protocol \"{name}\""),
            MouseError::UnknownEncoding(name) => write!(f, "unknown encoding \"{name}\""),
        }
    }
}

impl std::error::Error for MouseError {}

/// What a passed mouse event asks the host to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseReport {
    /// `triggerDataEvent(report, true)`.
    Data(String),
    /// `triggerBinaryEvent(report)` (the DEFAULT encoding).
    Binary(String),
    /// The event passed but the encoding produced no report.
    Suppressed,
}

/// `WheelEvent.DOM_DELTA_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDeltaMode {
    Pixel,
    Line,
    Page,
}

/// The fields of a DOM `WheelEvent` that `consumeWheelEvent` reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelEvent {
    pub delta_y: f64,
    pub delta_mode: WheelDeltaMode,
    pub alt_key: bool,
    pub ctrl_key: bool,
    pub shift_key: bool,
}

const MOD_SHIFT: i64 = 4;
const MOD_ALT: i64 = 8;
const MOD_CTRL: i64 = 16;

/// Helper for the default encoders to generate the event code.
fn event_code(e: &CoreMouseEvent, is_sgr: bool) -> i64 {
    let mut code = (if e.ctrl { MOD_CTRL } else { 0 })
        | (if e.shift { MOD_SHIFT } else { 0 })
        | (if e.alt { MOD_ALT } else { 0 });
    let button = i64::from(e.button as u8);
    if e.button == CoreMouseButton::Wheel {
        code |= 64;
        code |= i64::from(e.action as u8);
    } else {
        code |= button & 3;
        if button & 4 != 0 {
            code |= 64;
        }
        if button & 8 != 0 {
            code |= 128;
        }
        if e.action == CoreMouseAction::Move {
            code |= i64::from(CoreMouseAction::Move as u8);
        } else if e.action == CoreMouseAction::Up && !is_sgr {
            // special case - only SGR can report button on release
            // all others have to go with NONE
            code |= i64::from(CoreMouseButton::None as u8);
        }
    }
    code
}

/// `String.fromCharCode` for a value the DEFAULT encoding has checked to be at most 255.
fn byte_char(value: i64) -> char {
    u8::try_from(value).map(char::from).unwrap_or('\u{0}')
}

fn encode_default(e: &CoreMouseEvent) -> String {
    let params = [event_code(e, false) + 32, e.col + 32, e.row + 32];
    // supress mouse report if we exceed addressible range
    // Note this is handled differently by emulators
    // - xterm:         sends 0;0 coords instead
    // - vte, konsole:  no report
    if params.iter().any(|&p| p > 255) {
        return String::new();
    }
    let mut report = String::from("\x1b[M");
    for p in params {
        report.push(byte_char(p));
    }
    report
}

fn sgr_final(e: &CoreMouseEvent) -> char {
    if e.action == CoreMouseAction::Up && e.button != CoreMouseButton::Wheel {
        'm'
    } else {
        'M'
    }
}

fn encode_sgr(e: &CoreMouseEvent) -> String {
    format!(
        "\x1b[<{};{};{}{}",
        event_code(e, true),
        e.col,
        e.row,
        sgr_final(e)
    )
}

fn encode_sgr_pixels(e: &CoreMouseEvent) -> String {
    format!(
        "\x1b[<{};{};{}{}",
        event_code(e, true),
        e.x,
        e.y,
        sgr_final(e)
    )
}

fn default_protocols() -> Vec<(String, CoreMouseProtocol)> {
    use CoreMouseEventType as T;
    vec![
        // NONE. Events: none. Modifiers: none.
        (
            "NONE".to_string(),
            CoreMouseProtocol {
                events: T::NONE,
                restrict: Box::new(|_| false),
            },
        ),
        // X10. Events: mousedown. Modifiers: none.
        (
            "X10".to_string(),
            CoreMouseProtocol {
                events: T::DOWN,
                restrict: Box::new(|e| {
                    // no wheel, no move, no up
                    if e.button == CoreMouseButton::Wheel || e.action != CoreMouseAction::Down {
                        return false;
                    }
                    // no modifiers
                    e.ctrl = false;
                    e.alt = false;
                    e.shift = false;
                    true
                }),
            },
        ),
        // VT200. Events: mousedown / mouseup / wheel. Modifiers: all.
        (
            "VT200".to_string(),
            CoreMouseProtocol {
                events: T::DOWN | T::UP | T::WHEEL,
                // no move
                restrict: Box::new(|e| e.action != CoreMouseAction::Move),
            },
        ),
        // DRAG. Events: mousedown / mouseup / wheel / mousedrag. Modifiers: all.
        (
            "DRAG".to_string(),
            CoreMouseProtocol {
                events: T::DOWN | T::UP | T::WHEEL | T::DRAG,
                // no move without button
                restrict: Box::new(|e| {
                    !(e.action == CoreMouseAction::Move && e.button == CoreMouseButton::None)
                }),
            },
        ),
        // ANY. Events: all mouse related events. Modifiers: all.
        (
            "ANY".to_string(),
            CoreMouseProtocol {
                events: T::DOWN | T::UP | T::WHEEL | T::DRAG | T::MOVE,
                restrict: Box::new(|_| true),
            },
        ),
    ]
}

fn default_encodings() -> Vec<(String, CoreMouseEncoding)> {
    vec![
        // DEFAULT - CSI M Pb Px Py. Single byte encoding for coords and event code.
        // Can encode values up to 223 (1-based).
        ("DEFAULT".to_string(), Box::new(encode_default)),
        // SGR - CSI < Pb ; Px ; Py M|m. No encoding limitation.
        // Can report button on release and works with a well formed sequence.
        ("SGR".to_string(), Box::new(encode_sgr)),
        ("SGR_PIXELS".to_string(), Box::new(encode_sgr_pixels)),
    ]
}

/// Mouse tracking reports with different protocols and encodings.
///
/// Protocols: NONE (default), X10, VT200, DRAG, ANY. Encodings: DEFAULT, SGR, SGR_PIXELS. Custom
/// ones are added with [`add_protocol`](Self::add_protocol) / [`add_encoding`](Self::add_encoding)
/// and activated by name.
pub struct CoreMouseService {
    protocols: Vec<(String, CoreMouseProtocol)>,
    encodings: Vec<(String, CoreMouseEncoding)>,
    active_protocol: String,
    active_encoding: String,
    last_event: Option<CoreMouseEvent>,
    wheel_partial_scroll: f64,
}

impl fmt::Debug for CoreMouseService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreMouseService")
            .field("active_protocol", &self.active_protocol)
            .field("active_encoding", &self.active_encoding)
            .field("last_event", &self.last_event)
            .field("wheel_partial_scroll", &self.wheel_partial_scroll)
            .finish_non_exhaustive()
    }
}

impl Default for CoreMouseService {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreMouseService {
    /// A service with the default protocols and encodings, reset (NONE / DEFAULT).
    pub fn new() -> Self {
        let mut service = CoreMouseService {
            protocols: default_protocols(),
            encodings: default_encodings(),
            active_protocol: String::new(),
            active_encoding: String::new(),
            last_event: None,
            wheel_partial_scroll: 0.0,
        };
        // call reset to set defaults
        service.reset();
        service
    }

    pub fn add_protocol(&mut self, name: &str, protocol: CoreMouseProtocol) {
        match self.protocols.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = protocol,
            None => self.protocols.push((name.to_string(), protocol)),
        }
    }

    pub fn add_encoding(&mut self, name: &str, encoding: CoreMouseEncoding) {
        match self.encodings.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = encoding,
            None => self.encodings.push((name.to_string(), encoding)),
        }
    }

    fn protocol(&self, name: &str) -> Option<&CoreMouseProtocol> {
        self.protocols
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p)
    }

    fn encoding(&self, name: &str) -> Option<&CoreMouseEncoding> {
        self.encodings
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, e)| e)
    }

    pub fn active_protocol(&self) -> &str {
        &self.active_protocol
    }

    /// Whether the active protocol wants any events.
    pub fn are_mouse_events_active(&self) -> bool {
        self.protocol(&self.active_protocol)
            .is_some_and(|p| p.events != CoreMouseEventType::NONE)
    }

    /// Activates a protocol. On success returns `Some(events)`, the `onProtocolChange` payload
    /// (upstream fires on every successful assignment, so this is always `Some`).
    pub fn set_active_protocol(
        &mut self,
        name: &str,
    ) -> Result<Option<CoreMouseEventType>, MouseError> {
        let events = self
            .protocol(name)
            .map(|p| p.events)
            .ok_or_else(|| MouseError::UnknownProtocol(name.to_string()))?;
        self.active_protocol = name.to_string();
        Ok(Some(events))
    }

    pub fn active_encoding(&self) -> &str {
        &self.active_encoding
    }

    pub fn set_active_encoding(&mut self, name: &str) -> Result<(), MouseError> {
        if self.encoding(name).is_none() {
            return Err(MouseError::UnknownEncoding(name.to_string()));
        }
        self.active_encoding = name.to_string();
        Ok(())
    }

    /// Back to NONE / DEFAULT. Returns the `onProtocolChange` payload (the NONE events).
    pub fn reset(&mut self) -> CoreMouseEventType {
        self.active_protocol = "NONE".to_string();
        self.active_encoding = "DEFAULT".to_string();
        self.last_event = None;
        self.wheel_partial_scroll = 0.0;
        self.protocol("NONE")
            .map_or(CoreMouseEventType::NONE, |p| p.events)
    }

    /// Processes a wheel event, accounting for partial scrolls for trackpad, mouse scrolls.
    /// This prevents hyper-sensitive scrolling in alt buffer. `rows` is the terminal height.
    pub fn consume_wheel_event(
        &mut self,
        ev: &WheelEvent,
        cell_height: Option<f64>,
        dpr: Option<f64>,
        rows: usize,
        opts: &TerminalOptions,
    ) -> i64 {
        // Do nothing if it's not a vertical scroll event
        if ev.delta_y == 0.0 || ev.shift_key {
            return 0;
        }

        let (Some(cell_height), Some(dpr)) = (cell_height, dpr) else {
            return 0;
        };

        let target_wheel_event_pixels = cell_height / dpr;
        let mut amount = apply_scroll_modifier(ev.delta_y, ev, opts);

        match ev.delta_mode {
            WheelDeltaMode::Pixel => {
                amount /= target_wheel_event_pixels;

                let is_likely_trackpad = ev.delta_y.abs() < 50.0;
                if is_likely_trackpad {
                    amount *= 0.3;
                }

                self.wheel_partial_scroll += amount;
                amount = self.wheel_partial_scroll.abs().floor()
                    * if self.wheel_partial_scroll > 0.0 {
                        1.0
                    } else {
                        -1.0
                    };
                self.wheel_partial_scroll %= 1.0;
            }
            WheelDeltaMode::Page => {
                amount *= rows as f64;
            }
            WheelDeltaMode::Line => {}
        }
        // A float-to-int `as` saturates and maps NaN to 0.
        amount as i64
    }

    /// Triggers a mouse event to be sent. Returns `None` when the event was filtered (upstream
    /// `false`), otherwise the report to send (module notes). `cols`/`rows` are the terminal size.
    pub fn trigger_mouse_event(
        &mut self,
        event: &CoreMouseEvent,
        cols: usize,
        rows: usize,
    ) -> Option<MouseReport> {
        let mut e = *event;

        // range check for col/row
        if e.col < 0
            || e.col >= i64::try_from(cols).unwrap_or(i64::MAX)
            || e.row < 0
            || e.row >= i64::try_from(rows).unwrap_or(i64::MAX)
        {
            return None;
        }

        // filter nonsense combinations of button + action
        if e.button == CoreMouseButton::Wheel && e.action == CoreMouseAction::Move {
            return None;
        }
        if e.button == CoreMouseButton::None && e.action != CoreMouseAction::Move {
            return None;
        }
        if e.button != CoreMouseButton::Wheel
            && (e.action == CoreMouseAction::Left || e.action == CoreMouseAction::Right)
        {
            return None;
        }

        // report 1-based coords
        e.col += 1;
        e.row += 1;

        // debounce move events at grid or pixel level
        if e.action == CoreMouseAction::Move {
            if let Some(last) = &self.last_event {
                if equal_events(last, &e, self.active_encoding == "SGR_PIXELS") {
                    return None;
                }
            }
        }

        // apply protocol restrictions
        let protocol = self.protocol(&self.active_protocol)?;
        if !(protocol.restrict)(&mut e) {
            return None;
        }

        // encode report and send
        let encoding = self.encoding(&self.active_encoding)?;
        let report = encoding(&e);
        let result = if report.is_empty() {
            MouseReport::Suppressed
        } else if self.active_encoding == "DEFAULT" {
            // always send DEFAULT as binary data
            MouseReport::Binary(report)
        } else {
            MouseReport::Data(report)
        };

        self.last_event = Some(e);

        Some(result)
    }

    /// Which events `events` asks for.
    pub fn explain_events(&self, events: CoreMouseEventType) -> ExplainedEvents {
        ExplainedEvents {
            down: events.contains(CoreMouseEventType::DOWN),
            up: events.contains(CoreMouseEventType::UP),
            drag: events.contains(CoreMouseEventType::DRAG),
            move_: events.contains(CoreMouseEventType::MOVE),
            wheel: events.contains(CoreMouseEventType::WHEEL),
        }
    }
}

/// Multiply the scroll speed when the modifier key is pressed.
fn apply_scroll_modifier(amount: f64, ev: &WheelEvent, opts: &TerminalOptions) -> f64 {
    if ev.alt_key || ev.ctrl_key || ev.shift_key {
        return amount * opts.fast_scroll_sensitivity * opts.scroll_sensitivity;
    }
    amount * opts.scroll_sensitivity
}

fn equal_events(e1: &CoreMouseEvent, e2: &CoreMouseEvent, pixels: bool) -> bool {
    if pixels {
        if e1.x != e2.x || e1.y != e2.y {
            return false;
        }
    } else if e1.col != e2.col || e1.row != e2.row {
        return false;
    }
    e1.button == e2.button
        && e1.action == e2.action
        && e1.ctrl == e2.ctrl
        && e1.alt == e2.alt
        && e1.shift == e2.shift
}

#[cfg(test)]
#[path = "core_mouse_service_tests.rs"]
mod tests;
