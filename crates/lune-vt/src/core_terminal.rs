//! Port of xterm.js src/common/CoreTerminal.ts @ f447274 (upstream has no test file;
//! `core_terminal_tests.rs` is written from the TypeScript behaviour).
//!
//! The parser and the [`InputHandler`] (which owns every service, plan M6 D1) side by side, and
//! the service wiring upstream does with event listeners.
//!
//! Rust shape (plan M6 D1, D2, D3, D6):
//!  - Each upstream listener is an explicit call at the point upstream's event fires:
//!    * `bufferService.onScroll` -> [`InputHandler::on_scroll`] after every `scroll_lines` that
//!      returns `Some` and after `BufferService::reset` (the buffer activation it fires);
//!      `InputHandler` calls it after its own `BufferService::scroll`s.
//!    * `bufferService.onResize` -> [`Damage::full`](crate::engine::Damage).
//!    * `coreService.onData` / `onBinary` -> [`HostEvent::Response`] (bytes of the string);
//!      `onRequestScrollToBottom` -> [`CoreTerminal::scroll_to_bottom`] inside
//!      [`CoreTerminal::input`]; `onUserInput` fed the `WriteBuffer`, which is not ported (D3).
//!    * `optionsService` changes (`set_option`): `windowsMode`/`windowsPty` -> the heuristics
//!      flag; `scrollback` -> `BufferSet::resize` and `tabStopWidth` -> `setupTabStops` (the
//!      `BufferSet` listeners); `logLevel` -> the `LogService` level.
//!    * `Marker.onDispose` -> `OscLinkService` ([`InputHandler::forward_disposed_markers`]) at
//!      the end of every public mutation.
//!  - `WriteBuffer` is replaced by a synchronous [`CoreTerminal::write`] (D3): no time slicing,
//!    callbacks, discard watermark or `writeSync`; flow control is the host's job. `onWriteParsed`
//!    and `onLineFeed` have no public consumer (D2) and are not ported; the public `onScroll`
//!    position is [`Damage::scrolled`](crate::engine::Damage) plus the buffer's `ydisp`.
//!  - Windows wrapping heuristics (D6): the CSI `H` handler is registered once, in
//!    [`CoreTerminal::new`], and runs only while the flag `_handleWindowsPtyOptionChange`
//!    computes is set; the `onLineFeed` half runs inside `InputHandler::line_feed`.
//!  - `resize` takes `usize`, so upstream's `isNaN` guard has no counterpart.

// No-panic gate (plan M6 D8).
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::string_slice,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

use crate::buffer::buffer_set::BufferSet;
use crate::engine::HostEvent;
use crate::ext::{self, ExtensionError, StandardExtensions};
use crate::extension::Extension;
use crate::input_handler::{CoreServices, InputHandler};
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::params::Params;
use crate::parser::types::{FunctionIdentifier, HandlerId, IdentifierError};
use crate::services::buffer_service::{MINIMUM_COLS, MINIMUM_ROWS};
use crate::services::options_service::{
    OptionError, OptionKey, OptionValue, OptionsService, PtyBackendKind, TerminalOptions,
};
use crate::windows_mode::update_windows_mode_wrapped_state;

/// The terminal core: the parser and the handler that owns the services.
#[derive(Debug)]
pub struct CoreTerminal {
    parser: EscapeSequenceParser<InputHandler>,
    input_handler: InputHandler,
}

impl CoreTerminal {
    /// A terminal over `options` (defaults for the rest). Services are built in upstream's
    /// order (`CoreTerminal.ts:106-121`), then the handlers are registered and the Windows
    /// heuristics set up (`_setup`).
    pub fn new(options: impl IntoIterator<Item = OptionValue>) -> Self {
        let mut input_handler = InputHandler::new(CoreServices::new(OptionsService::new(options)));
        let mut parser = EscapeSequenceParser::new();
        if let Err(error) = InputHandler::register_handlers(&mut parser) {
            // The identifiers are constants; this only fires on a bug in the registrations.
            input_handler
                .services
                .log
                .error(format_args!("registering handlers failed: {error:?}"));
        }
        register_windows_heuristics(&mut parser, &mut input_handler);
        handle_windows_pty_option_change(&mut input_handler);
        CoreTerminal {
            parser,
            input_handler,
        }
    }

    pub fn cols(&self) -> usize {
        self.input_handler.services.buffer.cols
    }

    pub fn rows(&self) -> usize {
        self.input_handler.services.buffer.rows
    }

    /// Both buffers.
    pub fn buffers(&self) -> &BufferSet {
        &self.input_handler.services.buffer.buffers
    }

    /// The current options.
    pub fn options(&self) -> &TerminalOptions {
        self.input_handler.services.options.options()
    }

    /// The handler, and through it every service.
    pub fn input_handler(&self) -> &InputHandler {
        &self.input_handler
    }

    /// The handler, mutably.
    pub fn input_handler_mut(&mut self) -> &mut InputHandler {
        &mut self.input_handler
    }

    /// Set one option (upstream's `options` setter) and run what upstream's listeners on it do.
    /// A rejected value leaves everything unchanged.
    pub fn set_option(&mut self, value: OptionValue) -> Result<(), OptionError> {
        let changed = self.input_handler.services.options.set(value)?;
        if let Some(key) = changed {
            let services = &mut self.input_handler.services;
            let opts = services.options.options();
            match key {
                // BufferSet: onSpecificOptionChange('scrollback')
                OptionKey::Scrollback => {
                    let (cols, rows) = (services.buffer.cols, services.buffer.rows);
                    services.buffer.buffers.resize(cols, rows, opts);
                }
                // BufferSet: onSpecificOptionChange('tabStopWidth')
                OptionKey::TabStopWidth => services.buffer.buffers.setup_tab_stops(None, opts),
                // LogService: onSpecificOptionChange('logLevel')
                OptionKey::LogLevel => services.log.set_level(opts.log_level),
                // CoreTerminal: onMultipleOptionChange(['windowsMode', 'windowsPty'])
                OptionKey::WindowsMode | OptionKey::WindowsPty => {
                    handle_windows_pty_option_change(&mut self.input_handler);
                }
                _ => {}
            }
        }
        self.input_handler.forward_disposed_markers();
        Ok(())
    }

    /// Parse `data` (UTF-8, may end inside a character or a sequence) synchronously (D3).
    pub fn write(&mut self, data: &[u8]) {
        self.input_handler.parse(&mut self.parser, data);
        self.input_handler.forward_disposed_markers();
    }

    /// Send `data` to the application as if typed (upstream `input` ->
    /// `coreService.triggerDataEvent`): a [`HostEvent::Response`], preceded by a scroll to the
    /// bottom when user input should scroll.
    pub fn input(&mut self, data: &str, was_user_input: bool) {
        let services = &mut self.input_handler.services;
        let event = services.core.trigger_data_event(
            data,
            was_user_input,
            services.buffer.buffer(),
            services.options.options(),
            &services.log,
        );
        let Some(event) = event else {
            return;
        };
        if event.scroll_to_bottom {
            self.scroll_to_bottom();
        }
        self.input_handler
            .push_event(HostEvent::Response(event.data.into_bytes()));
    }

    /// Resize to `x` columns and `y` rows, clamped to at least [`MINIMUM_COLS`] x
    /// [`MINIMUM_ROWS`] before the buffers see them (M5 D8).
    pub fn resize(&mut self, x: usize, y: usize) {
        let x = x.max(MINIMUM_COLS);
        let y = y.max(MINIMUM_ROWS);
        let services = &mut self.input_handler.services;
        services.buffer.resize(x, y, services.options.options());
        // onResize
        self.input_handler.damage_mut().full = true;
        self.input_handler.forward_disposed_markers();
    }

    /// Scroll the display by `disp` lines (negative scrolls up).
    pub fn scroll_lines(&mut self, disp: i64) {
        if let Some(ydisp) = self.input_handler.services.buffer.scroll_lines(disp, false) {
            self.input_handler.on_scroll(ydisp);
        }
        self.input_handler.forward_disposed_markers();
    }

    /// Scroll by `page_count` pages of `rows - 1` lines.
    pub fn scroll_pages(&mut self, page_count: i64) {
        let page = i64::try_from(self.rows()).unwrap_or(i64::MAX) - 1;
        self.scroll_lines(page_count.saturating_mul(page));
    }

    pub fn scroll_to_top(&mut self) {
        let ydisp = self.input_handler.services.buffer.buffer().ydisp;
        self.scroll_lines(-to_i64(ydisp));
    }

    pub fn scroll_to_bottom(&mut self) {
        let buffer = self.input_handler.services.buffer.buffer();
        let disp = to_i64(buffer.ybase) - to_i64(buffer.ydisp);
        self.scroll_lines(disp);
    }

    /// Scroll so that buffer line `line` is the first one shown.
    pub fn scroll_to_line(&mut self, line: usize) {
        let ydisp = self.input_handler.services.buffer.buffer().ydisp;
        let scroll_amount = to_i64(line) - to_i64(ydisp);
        if scroll_amount != 0 {
            self.scroll_lines(scroll_amount);
        }
        self.input_handler.forward_disposed_markers();
    }

    /// Add a handler for an ESC sequence (newest first; return `true` to consume it).
    pub fn register_esc_handler(
        &mut self,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        InputHandler::register_esc_handler(&mut self.parser, id, callback)
    }

    /// Add a handler for a DCS sequence (the payload collected as a string).
    pub fn register_dcs_handler(
        &mut self,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler, &str, &Params) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        InputHandler::register_dcs_handler(&mut self.parser, id, callback)
    }

    /// Add a handler for a CSI sequence. A plain CSI `t` handler only runs for the window
    /// options the options allow.
    pub fn register_csi_handler(
        &mut self,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler, &Params) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        InputHandler::register_csi_handler(&mut self.parser, id, callback)
    }

    /// Add a handler for an OSC sequence (the payload collected as a string).
    pub fn register_osc_handler(
        &mut self,
        ident: i32,
        callback: impl FnMut(&mut InputHandler, &str) -> bool + 'static,
    ) -> HandlerId {
        InputHandler::register_osc_handler(&mut self.parser, ident, callback)
    }

    /// Remove a handler the `register_*` methods returned (upstream `IDisposable.dispose`).
    pub fn dispose_handler(&mut self, id: HandlerId) {
        self.parser.dispose_handler(id);
    }

    /// Let `extension` register its handlers on the parser (they run before the built-in ones).
    /// Extensions register on the parser directly, without the CSI `t` guard of
    /// [`CoreTerminal::register_csi_handler`].
    pub fn add_extension(
        &mut self,
        extension: &dyn Extension<InputHandler>,
    ) -> Result<(), IdentifierError> {
        extension.register(&mut self.parser)
    }

    /// Register the standard extensions (plan M7 D2): once per terminal.
    pub fn install_standard_extensions(
        &mut self,
        config: &StandardExtensions,
    ) -> Result<(), ExtensionError> {
        ext::install(&mut self.parser, &mut self.input_handler, config)
    }

    /// Upstream `_setup`: everything a full terminal creation sets up that `reset` reruns.
    pub fn setup(&mut self) {
        handle_windows_pty_option_change(&mut self.input_handler);
    }

    /// Reset the handler and the services (upstream `reset`, `CoreTerminal.ts:244-250`).
    pub fn reset(&mut self) {
        core_reset(&mut self.input_handler);
        self.input_handler.forward_disposed_markers();
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// `CoreTerminal.reset` on the handler alone, so the RIS handler (inside a parse) can run it.
pub(crate) fn core_reset(input_handler: &mut InputHandler) {
    input_handler.reset();
    // LUNETERM (plan M7 D11): RIS clears the kitty keyboard stacks and modifyOtherKeys, as
    // Lunegit's `ESC c` handler does; this is the one path RIS and `Terminal::reset` share.
    input_handler.ext.keyboard.reset();
    let services = &mut input_handler.services;
    // LUNETERM: upstream drops the old buffers without disposing their markers, so an OSC 8
    // link alive at the reset stays in the OscLinkService forever (and a later link with the
    // same id and uri reuses its id). Disposing the markers first frees those links (the plan's
    // invariant: no link entry whose markers are all gone). Without a reset, the caps in
    // `osc_link_service` (MAX_LINKS, MAX_LINK_MARKERS) bound the table.
    services.buffer.buffers.normal_mut().clear_all_markers();
    services.buffer.buffers.alt_mut().clear_all_markers();
    // before the buffers (and their disposal logs) are replaced
    input_handler.forward_disposed_markers();
    let services = &mut input_handler.services;
    let ydisp = services.buffer.reset(services.options.options());
    services.charset.reset();
    services.core.reset();
    services.mouse.reset();
    // onBufferActivate -> bufferService.onScroll
    input_handler.on_scroll(ydisp);
    // the buffers were replaced
    input_handler.damage_mut().full = true;
    input_handler.forward_disposed_markers();
}

/// `_handleWindowsPtyOptionChange`: whether the Windows wrapping heuristics run.
pub(crate) fn handle_windows_pty_option_change(input_handler: &mut InputHandler) {
    let opts = input_handler.services.options.options();
    let windows_pty = &opts.windows_pty;
    input_handler.windows_heuristics = match windows_pty.build_number {
        Some(build_number) => {
            windows_pty.backend == Some(PtyBackendKind::Conpty) && build_number < 21376
        }
        None => opts.windows_mode,
    };
}

/// The CSI `H` half of `_enableWindowsWrappingHeuristics`, registered once (D6). It returns
/// `false` so `cursorPosition` still runs.
///
/// LUNETERM: upstream registers it when the heuristics turn on, so a CSI `H` handler added
/// before that runs after it. Here it is registered at construction (a handler cannot register
/// from inside RIS, which reruns `_setup`), so every later CSI `H` handler runs before it. No
/// built-in extension registers CSI `H`.
fn register_windows_heuristics(
    parser: &mut EscapeSequenceParser<InputHandler>,
    input_handler: &mut InputHandler,
) {
    let result =
        InputHandler::register_csi_handler(parser, FunctionIdentifier::new('H'), |h, _| {
            if h.windows_heuristics {
                update_windows_mode_wrapped_state(&mut h.services.buffer);
            }
            false
        });
    if let Err(error) = result {
        input_handler.services.log.error(format_args!(
            "registering the Windows heuristics failed: {error:?}"
        ));
    }
}

#[cfg(test)]
#[path = "core_terminal_tests.rs"]
mod tests;
