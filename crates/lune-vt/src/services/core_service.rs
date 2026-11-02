//! Port of xterm.js src/common/services/CoreService.ts @ f447274 (tests: CoreService.test.ts),
//! with `IModes` and `IDecPrivateModes` from `src/common/Types.ts`.
//!
//! Rust notes (see `docs/porting-xterm.md`, plan M6 D2, D4):
//!  - Upstream's `onData`, `onBinary`, `onUserInput` and `onRequestScrollToBottom` emitters are
//!    return values (M5 D1): [`CoreService::trigger_data_event`] returns the [`DataEvent`] that
//!    carries `onData`'s payload plus whether upstream would also have fired
//!    `onRequestScrollToBottom` and `onUserInput`; [`CoreService::trigger_binary_event`] returns
//!    `onBinary`'s payload. `None` is "nothing fired" (`disableStdin`).
//!  - The service reads the buffer, the options and the log service upstream through injected
//!    services; here the caller passes `&Buffer`, `&TerminalOptions` and `&LogService`.
//!  - Strings are Rust `String`s: the binary event's characters are all `<= U+00FF`, one per byte.
//!  - `// LUNETERM:` (user decision 2026-11-02) the data and binary events log their length at
//!    debug, never the data (upstream logs it at debug and its codes at trace): it is user input.
//!  - `clone(DEFAULT_MODES)` is a plain `Default`/`Clone` (`Clone.ts` is replaced by `Clone`).

use crate::buffer::buffer::Buffer;
use crate::services::log_service::LogService;
use crate::services::options_service::{CursorStyle, TerminalOptions};

/// `IModes` (`Types.ts:263`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modes {
    pub insert_mode: bool,
}

/// `IDecPrivateModes` (`Types.ts:267`). `cursor_blink` and `cursor_style` are `undefined` until
/// the application sets them (`None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecPrivateModes {
    pub application_cursor_keys: bool,
    pub application_keypad: bool,
    pub bracketed_paste_mode: bool,
    pub cursor_blink: Option<bool>,
    pub cursor_style: Option<CursorStyle>,
    pub origin: bool,
    pub reverse_wraparound: bool,
    pub send_focus: bool,
    pub synchronized_output: bool,
    /// Defaults: xterm - true, vt100 - false.
    pub wraparound: bool,
}

impl Default for DecPrivateModes {
    /// `DEFAULT_DEC_PRIVATE_MODES`.
    fn default() -> Self {
        DecPrivateModes {
            application_cursor_keys: false,
            application_keypad: false,
            bracketed_paste_mode: false,
            cursor_blink: None,
            cursor_style: None,
            origin: false,
            reverse_wraparound: false,
            send_focus: false,
            synchronized_output: false,
            wraparound: true,
        }
    }
}

/// What [`CoreService::trigger_data_event`] fires: `onData(data)`, preceded by
/// `onRequestScrollToBottom` when `scroll_to_bottom` and `onUserInput` when `user_input`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataEvent {
    pub data: String,
    /// Upstream fired `onRequestScrollToBottom`.
    pub scroll_to_bottom: bool,
    /// Upstream fired `onUserInput`.
    pub user_input: bool,
}

/// Modes, the cursor flags, and the data/binary events towards the pty.
#[derive(Debug, Default)]
pub struct CoreService {
    pub is_cursor_initialized: bool,
    pub is_cursor_hidden: bool,
    pub modes: Modes,
    pub dec_private_modes: DecPrivateModes,
}

impl CoreService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores the default modes. Does not touch `is_cursor_initialized` or `is_cursor_hidden`
    /// (upstream's test pins the first).
    pub fn reset(&mut self) {
        self.modes = Modes::default();
        self.dec_private_modes = DecPrivateModes::default();
    }

    /// Upstream `triggerDataEvent`. `None` when `disableStdin` is set.
    pub fn trigger_data_event(
        &mut self,
        data: &str,
        was_user_input: bool,
        buffer: &Buffer,
        opts: &TerminalOptions,
        log: &LogService,
    ) -> Option<DataEvent> {
        // Prevents all events to pty process if stdin is disabled
        if opts.disable_stdin {
            return None;
        }

        // Input is being sent to the terminal, the terminal should focus the prompt.
        let scroll_to_bottom =
            was_user_input && opts.scroll_on_user_input && buffer.ybase != buffer.ydisp;

        // LUNETERM: (user decision 2026-11-02, review S5/F8) upstream logs the data at debug and
        // its codes at trace; the data is user input (keystrokes, a typed password, a paste),
        // so only its length is logged and the trace line is dropped.
        log.debug(format_args!("sending data ({} bytes)", data.len()));
        Some(DataEvent {
            data: data.to_string(),
            scroll_to_bottom,
            user_input: was_user_input,
        })
    }

    /// Upstream `triggerBinaryEvent`: the `onBinary` payload, `None` when `disableStdin` is set.
    pub fn trigger_binary_event(
        &mut self,
        data: &str,
        opts: &TerminalOptions,
        log: &LogService,
    ) -> Option<String> {
        if opts.disable_stdin {
            return None;
        }
        // LUNETERM: length only, as in `trigger_data_event` (one byte per character)
        log.debug(format_args!(
            "sending binary ({} bytes)",
            data.chars().count()
        ));
        Some(data.to_string())
    }
}

#[cfg(test)]
#[path = "core_service_tests.rs"]
mod tests;
