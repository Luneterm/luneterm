//! Port of xterm.js src/headless/Terminal.ts and src/headless/public/Terminal.ts @ f447274
//! (tests: headless/public/Terminal.test.ts), as one type (plan M6 D5): the core/public split
//! upstream exists only to hide JS internals.
//!
//! [`Terminal`] is the engine's public face: upstream's headless API, and [`VtEngine`] for
//! Lunegit's `IVtEngine` (D9).
//!
//! Rust shape (plan M6 D2, D3, D5, D9, D10):
//!  - Upstream's emitters are [`Terminal::drain_events`] (`onBell`, `onData`, `onBinary`,
//!    `onTitleChange`, `onA11yChar`, `onA11yTab`) and [`Terminal::take_damage`] (`onCursorMove`,
//!    `onScroll`, `onResize`). `onLineFeed` and `onWriteParsed` have no consumer (D2).
//!  - `write` is synchronous (D3): no callbacks. `writeln` is `write(data)` + `write("\r\n")` and
//!    is left to the caller.
//!  - The buffer API objects (`BufferNamespaceApi`, `BufferApiView`, `BufferLineApiView`) are
//!    [`Terminal::buffers`] and the viewport read API ([`Terminal::viewport`],
//!    [`Terminal::read_row`]); `ParserApi` is the `register_*` forwards; `UnicodeApi` is
//!    [`Terminal::unicode_mut`]; `AddonManager` is [`Terminal::add_extension`] (no JS addons).
//!  - `allowProposedApi` guards unstable JS API; the Rust API is versioned with the crate, so
//!    nothing is gated on it. `dispose` is `Drop`.
//!  - Integer arguments are typed, so `_verifyIntegers` has no counterpart.
//!  - D10: the Unicode 11 provider is registered at construction; version 6 stays active, as
//!    upstream without the addon loaded.

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

use std::collections::BTreeMap;

use crate::addons::unicode_v11::UnicodeV11;
use crate::buffer::attribute_data::ExtendedAttrs;
use crate::buffer::buffer_line::DEFAULT_ATTR_DATA;
use crate::buffer::buffer_set::BufferSet;
use crate::buffer::cell_data::CellData;
use crate::buffer::constants::{Attributes, BgFlags, FgFlags};
use crate::buffer::marker::MarkerId;
use crate::core_terminal::{CoreTerminal, core_reset, handle_windows_pty_option_change};
use crate::engine::{
    CellColor, Damage, HostEvent, Modes, MouseEncoding, MouseTracking, UnderlineStyle, Viewport,
    ViewportCell, ViewportCursor, VtEngine,
};
use crate::ext::{ExtensionError, StandardExtensions};
use crate::extension::Extension;
use crate::input::key_encoder::{KeyboardModes, encode_key};
use crate::input::key_event::{TerminalKey, TerminalKeyEvent, TerminalModifiers};
use crate::input::legacy_key_encoder::{legacy_char_input, legacy_key_input};
use crate::input::mouse_event::TerminalMouseEvent;
use crate::input::paste::paste_bytes;
use crate::input_handler::InputHandler;
use crate::parser::params::Params;
use crate::parser::types::{FunctionIdentifier, HandlerId, IdentifierError};
use crate::services::core_mouse_service::MouseReport;
use crate::services::log_service::Logger;
use crate::services::options_service::{OptionError, OptionKey, OptionValue, TerminalOptions};
use crate::services::unicode_service::UnicodeService;

/// A headless terminal: feed it PTY output, read the screen.
#[derive(Debug)]
pub struct Terminal {
    core: CoreTerminal,
}

impl Terminal {
    /// A terminal over `options` (defaults for the rest; a value that fails validation is
    /// dropped). `Cols`/`Rows` set the size.
    pub fn new(options: impl IntoIterator<Item = OptionValue>) -> Self {
        let mut core = CoreTerminal::new(options);
        core.input_handler_mut()
            .services
            .unicode
            .register(Box::new(UnicodeV11::new()));
        Terminal { core }
    }

    pub fn cols(&self) -> usize {
        self.core.cols()
    }

    pub fn rows(&self) -> usize {
        self.core.rows()
    }

    /// Parse PTY output (UTF-8; may end inside a character or a sequence).
    pub fn write(&mut self, data: &[u8]) {
        self.core.write(data);
    }

    /// [`Terminal::write`] of a string.
    pub fn write_str(&mut self, data: &str) {
        self.core.write(data.as_bytes());
    }

    /// Send `data` to the application as if typed: a [`HostEvent::Response`] (and a scroll to
    /// the bottom for user input, if `scroll_on_user_input`). Nothing with `disable_stdin`.
    pub fn input(&mut self, data: &str, was_user_input: bool) {
        self.core.input(data, was_user_input);
    }

    /// Resize to `x` columns and `y` rows (at least 2 x 1); nothing happens at the current size.
    pub fn resize(&mut self, x: usize, y: usize) {
        if x == self.cols() && y == self.rows() {
            return;
        }
        self.core.resize(x, y);
    }

    /// Scroll the display by `amount` lines (negative scrolls up).
    pub fn scroll_lines(&mut self, amount: i64) {
        self.core.scroll_lines(amount);
    }

    /// Scroll by `page_count` pages.
    pub fn scroll_pages(&mut self, page_count: i64) {
        self.core.scroll_pages(page_count);
    }

    pub fn scroll_to_top(&mut self) {
        self.core.scroll_to_top();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.core.scroll_to_bottom();
    }

    /// Scroll so that buffer line `line` is the first one shown.
    pub fn scroll_to_line(&mut self, line: usize) {
        self.core.scroll_to_line(line);
    }

    /// Clear the entire buffer, making the prompt line the new first line.
    pub fn clear(&mut self) {
        let rows = self.rows();
        let input_handler = self.core.input_handler_mut();
        let buffer = input_handler.services.buffer.buffer_mut();
        if buffer.ybase == 0 && buffer.y == 0 {
            // Don't clear if it's already clear
            return;
        }
        // upstream stores `undefined` at 0 when the line is missing
        let prompt = buffer
            .lines
            .get(buffer.ybase.saturating_add(buffer.y))
            .cloned();
        buffer.lines.set(0, prompt);
        buffer.lines.set_len(1);
        buffer.ydisp = 0;
        buffer.ybase = 0;
        buffer.y = 0;
        for _ in 1..rows {
            let blank = buffer.get_blank_line(&DEFAULT_ATTR_DATA, false);
            buffer.lines.push(blank);
        }
        // _onScroll.fire
        input_handler.damage_mut().scrolled = true;
        input_handler.forward_disposed_markers();
    }

    /// Reset the terminal (the size and the options are kept). Like upstream's, this does not
    /// reset the parser: a sequence split across writes continues after it. RIS (`ESC c`) is the
    /// in-band reset.
    pub fn reset(&mut self) {
        reset_terminal(self.core.input_handler_mut());
    }

    /// A marker on the cursor line plus `cursor_y_offset`, following its line; `None` on the
    /// alternate buffer.
    pub fn add_marker(&mut self, cursor_y_offset: i64) -> Option<MarkerId> {
        let buffers = &mut self.core.input_handler_mut().services.buffer.buffers;
        // Disallow markers on the alt buffer
        if buffers.is_alt_active() {
            return None;
        }
        let buffer = buffers.normal_mut();
        let y = to_i64(buffer.ybase)
            .saturating_add(to_i64(buffer.y))
            .saturating_add(cursor_y_offset);
        Some(buffer.add_marker(y))
    }

    pub fn options(&self) -> &TerminalOptions {
        self.core.options()
    }

    /// Set one option and run its side effects. `Cols`/`Rows` can only be set in
    /// [`Terminal::new`] ([`OptionError::ConstructorOnly`]; use [`Terminal::resize`]).
    pub fn set_option(&mut self, value: OptionValue) -> Result<(), OptionError> {
        let key = value.key();
        if key.is_one_of(&[OptionKey::Cols, OptionKey::Rows]) {
            return Err(OptionError::ConstructorOnly { key });
        }
        self.core.set_option(value)
    }

    /// The normal and the alternate buffer.
    pub fn buffers(&self) -> &BufferSet {
        self.core.buffers()
    }

    /// Fill `out` with viewport row `viewport_y` (`cols` cells, as [`Viewport::cells`]).
    /// `false` (and `out` untouched) when the row is outside the viewport.
    pub fn read_row(&self, viewport_y: usize, out: &mut Vec<ViewportCell>) -> bool {
        let services = self.core.input_handler().services();
        let cols = services.buffer.cols;
        if viewport_y >= services.buffer.rows {
            return false;
        }
        out.clear();
        let buffer = services.buffer.buffer();
        // a missing line (M5's scrollback-0 soft invariant) renders blank
        let Some(line) = buffer.lines.get(buffer.ydisp.saturating_add(viewport_y)) else {
            out.resize(cols, blank_cell());
            return true;
        };
        let mut cell = CellData::new();
        for x in 0..cols {
            if x >= line.len() {
                out.push(blank_cell());
                continue;
            }
            line.load_cell(x, &mut cell);
            out.push(viewport_cell(&cell));
        }
        true
    }

    /// The uri of link `id`, `None` once the link is freed.
    pub fn link_uri(&self, id: u32) -> Option<&str> {
        self.core
            .input_handler()
            .services()
            .osc_link
            .get_link_data(id)
            .map(|data| data.uri.as_str())
    }

    /// Run one batch of the deferred memory cleanup of each buffer that has one pending
    /// (upstream's `IdleTaskQueue`). Returns whether work remains; call again while it does,
    /// when the host is idle.
    pub fn idle(&mut self) -> bool {
        let buffers = &mut self.core.input_handler_mut().services.buffer.buffers;
        if buffers.normal().memory_cleanup_pending() {
            buffers.normal_mut().batched_memory_cleanup();
        }
        if buffers.alt().memory_cleanup_pending() {
            buffers.alt_mut().batched_memory_cleanup();
        }
        buffers.normal().memory_cleanup_pending() || buffers.alt().memory_cleanup_pending()
    }

    /// Where log messages go; `None` (the default) discards them.
    pub fn set_logger(&mut self, logger: Option<Box<dyn Logger>>) {
        self.core
            .input_handler_mut()
            .services
            .log
            .set_logger(logger);
    }

    /// The Unicode version registry (upstream `terminal.unicode`).
    pub fn unicode_mut(&mut self) -> &mut UnicodeService {
        &mut self.core.input_handler_mut().services.unicode
    }

    /// Add a handler for an ESC sequence (newest first; return `true` to consume it).
    pub fn register_esc_handler(
        &mut self,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        self.core.register_esc_handler(id, callback)
    }

    /// Add a handler for a DCS sequence (the payload collected as a string).
    pub fn register_dcs_handler(
        &mut self,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler, &str, &Params) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        self.core.register_dcs_handler(id, callback)
    }

    /// Add a handler for a CSI sequence. A plain CSI `t` handler only runs for the window
    /// options the options allow.
    pub fn register_csi_handler(
        &mut self,
        id: FunctionIdentifier,
        callback: impl FnMut(&mut InputHandler, &Params) -> bool + 'static,
    ) -> Result<HandlerId, IdentifierError> {
        self.core.register_csi_handler(id, callback)
    }

    /// Add a handler for an OSC sequence (the payload collected as a string).
    pub fn register_osc_handler(
        &mut self,
        ident: i32,
        callback: impl FnMut(&mut InputHandler, &str) -> bool + 'static,
    ) -> HandlerId {
        self.core.register_osc_handler(ident, callback)
    }

    /// Remove a handler a `register_*` method returned.
    pub fn dispose_handler(&mut self, id: HandlerId) {
        self.core.dispose_handler(id);
    }

    /// Let `extension` register its handlers (they run before the built-in ones).
    pub fn add_extension(
        &mut self,
        extension: &dyn Extension<InputHandler>,
    ) -> Result<(), IdentifierError> {
        self.core.add_extension(extension)
    }

    /// Register Lunegit's terminal additions (plan M7 D2): XTVERSION when `config` names the
    /// terminal, the unknown-OSC report, and as later steps land the keyboard protocols and the
    /// rest. Call it once, right after [`Terminal::new`]; the second call is
    /// [`ExtensionError::AlreadyInstalled`]. A terminal that never calls it behaves exactly as
    /// xterm.js.
    pub fn install_standard_extensions(
        &mut self,
        config: &StandardExtensions,
    ) -> Result<(), ExtensionError> {
        self.core.install_standard_extensions(config)
    }

    /// The core (read access for the conformance dump).
    pub fn core(&self) -> &CoreTerminal {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut CoreTerminal {
        &mut self.core
    }

    /// The events produced since the last call, in fire order.
    pub fn drain_events(&mut self) -> Vec<HostEvent> {
        self.core.input_handler_mut().take_events()
    }

    /// The damage accumulated since the last call.
    pub fn take_damage(&mut self) -> Damage {
        self.core.input_handler_mut().take_damage()
    }

    /// A snapshot of the screen.
    pub fn viewport(&self) -> Viewport {
        let input_handler = self.core.input_handler();
        let services = input_handler.services();
        let (cols, rows) = (services.buffer.cols, services.buffer.rows);
        let buffer = services.buffer.buffer();

        let mut cells = Vec::with_capacity(rows);
        let mut links = BTreeMap::new();
        for y in 0..rows {
            let mut row = Vec::with_capacity(cols);
            self.read_row(y, &mut row);
            for cell in &row {
                if cell.link_id != 0 && !links.contains_key(&cell.link_id) {
                    if let Some(uri) = self.link_uri(cell.link_id) {
                        links.insert(cell.link_id, uri.to_owned());
                    }
                }
            }
            cells.push(row);
        }

        let cursor_line = buffer.ybase.saturating_add(buffer.y);
        let inside = cursor_line >= buffer.ydisp && cursor_line - buffer.ydisp < rows;
        let dec = &services.core.dec_private_modes;
        let opts = services.options.options();
        let cursor = ViewportCursor {
            x: buffer.x,
            y: if inside {
                cursor_line - buffer.ydisp
            } else {
                0
            },
            visible: inside && !services.core.is_cursor_hidden,
            style: dec.cursor_style.unwrap_or(opts.cursor_style),
            blink: dec.cursor_blink.unwrap_or(opts.cursor_blink),
        };

        Viewport {
            cols,
            rows,
            cells,
            cursor,
            viewport_top: buffer.ydisp,
            base_y: buffer.ybase,
            total_lines: buffer.lines.len(),
            is_at_bottom: buffer.ydisp >= buffer.ybase,
            title: input_handler.window_title_shared(),
            alt_screen: services.buffer.buffers.is_alt_active(),
            links,
        }
    }

    /// The modes the host needs for input encoding (upstream public `modes`, plus
    /// `cursor_hidden` and `mouse_encoding`).
    pub fn modes(&self) -> Modes {
        let services = self.core.input_handler().services();
        let m = &services.core.dec_private_modes;
        let mouse_tracking = match services.mouse.active_protocol() {
            "X10" => MouseTracking::X10,
            "VT200" => MouseTracking::Vt200,
            "DRAG" => MouseTracking::Drag,
            "ANY" => MouseTracking::Any,
            _ => MouseTracking::None,
        };
        let mouse_encoding = match services.mouse.active_encoding() {
            "SGR" => MouseEncoding::Sgr,
            "SGR_PIXELS" => MouseEncoding::SgrPixels,
            _ => MouseEncoding::Default,
        };
        Modes {
            application_cursor_keys: m.application_cursor_keys,
            application_keypad: m.application_keypad,
            bracketed_paste: m.bracketed_paste_mode,
            insert: services.core.modes.insert_mode,
            origin: m.origin,
            reverse_wraparound: m.reverse_wraparound,
            send_focus: m.send_focus,
            synchronized_output: m.synchronized_output,
            wraparound: m.wraparound,
            cursor_hidden: services.core.is_cursor_hidden,
            mouse_tracking,
            mouse_encoding,
            kitty_keyboard_flags: self
                .core
                .input_handler()
                .ext
                .keyboard
                .flags(services.buffer.buffers.is_alt_active()),
            modify_other_keys: self.core.input_handler().ext.keyboard.modify_other_keys(),
        }
    }

    /// Clear the kitty keyboard stacks of both screens and the modifyOtherKeys level, as RIS and
    /// DECSTR do. The host calls it when the application that set them is gone and the terminal
    /// is reused (Lunegit ledger row 11: a program that exits without popping its flags).
    pub fn reset_keyboard_protocol(&mut self) {
        self.core.input_handler_mut().ext.keyboard.reset();
    }

    /// Turn the OSC 52 clipboard-write gate on or off (initially
    /// [`crate::StandardExtensions::clipboard_write`]). Takes effect at the next OSC 52; with the
    /// gate off a valid write is consumed and dropped. Without the standard extensions installed
    /// it changes nothing: OSC 52 then reaches the core's unknown-OSC fallback only.
    pub fn set_clipboard_write(&mut self, enabled: bool) {
        self.core.input_handler_mut().ext.clipboard_write = enabled;
    }

    /// Whether the OSC 52 clipboard-write gate is on.
    pub fn clipboard_write(&self) -> bool {
        self.core.input_handler().ext.clipboard_write
    }

    /// The live keyboard modes the key encoder reads: the kitty flags of the active screen, the
    /// modifyOtherKeys level and DECCKM.
    pub fn keyboard_modes(&self) -> KeyboardModes {
        let handler = self.core.input_handler();
        let services = handler.services();
        KeyboardModes {
            kitty_flags: handler
                .ext
                .keyboard
                .flags(services.buffer.buffers.is_alt_active()),
            modify_other_keys: handler.ext.keyboard.modify_other_keys(),
            application_cursor_keys: services.core.dec_private_modes.application_cursor_keys,
        }
    }

    /// The bytes to write to the PTY for a keystroke, under the live keyboard protocol state
    /// (kitty, modifyOtherKeys, DECCKM; see [`encode_key`]). Lunegit's `GenerateKeyEvent`.
    ///
    /// Like upstream's user-input path it goes through `triggerDataEvent`: `None` with
    /// `disable_stdin`, and with `scroll_on_user_input` the display scrolls to the bottom first.
    /// `None` also when the key sends nothing (an invalid character code point), without
    /// scrolling. The bytes are returned, not queued as a [`HostEvent::Response`].
    pub fn key_event(&mut self, event: &TerminalKeyEvent) -> Option<Vec<u8>> {
        let bytes = encode_key(event, self.keyboard_modes());
        self.send_data(bytes, true)
    }

    /// Lunegit's `GenerateKeyInput`: the legacy bytes of a functional key with the live DECCKM
    /// state, whatever the keyboard protocol (same `None` rules as [`Terminal::key_event`]).
    pub fn key_input(&mut self, key: TerminalKey, modifiers: TerminalModifiers) -> Option<Vec<u8>> {
        let decckm = self.keyboard_modes().application_cursor_keys;
        self.send_data(legacy_key_input(key, modifiers, decckm).into_bytes(), true)
    }

    /// Lunegit's `GenerateCharInput`: Ctrl folds `c` to its C0 byte, Alt prefixes ESC (same `None`
    /// rules as [`Terminal::key_event`]).
    pub fn char_input(&mut self, c: char, modifiers: TerminalModifiers) -> Option<Vec<u8>> {
        self.send_data(legacy_char_input(c, modifiers).into_bytes(), true)
    }

    /// A pointer event as the bytes to write to the PTY, from the engine's own mouse state (the
    /// protocol and encoding the application selected with DECSET; see [`TerminalMouseEvent`] for
    /// the coordinate convention). `None` when no tracking is on, the protocol does not report
    /// this event, the position is outside the terminal, the default encoding cannot express the
    /// position, or `disable_stdin` is set. Reports other than the default encoding go through
    /// `triggerDataEvent(.., true)` (so they scroll on user input); the default encoding's bytes
    /// go through `triggerBinaryEvent` and are returned raw (a coordinate above 95 is one byte
    /// above 0x7F, not UTF-8).
    pub fn mouse_event(&mut self, event: &TerminalMouseEvent) -> Option<Vec<u8>> {
        let core_event = event.to_core()?;
        let services = &mut self.core.input_handler_mut().services;
        let (cols, rows) = (services.buffer.cols, services.buffer.rows);
        match services
            .mouse
            .trigger_mouse_event(&core_event, cols, rows)?
        {
            MouseReport::Data(report) => self.send_data(report.into_bytes(), true),
            MouseReport::Binary(report) => {
                let text = services.core.trigger_binary_event(
                    &report,
                    services.options.options(),
                    &services.log,
                )?;
                // One byte per character (the encoding only produces U+0000..=U+00FF).
                text.chars()
                    .map(|c| u8::try_from(u32::from(c)).ok())
                    .collect()
            }
            MouseReport::Suppressed => None,
        }
    }

    /// The focus report: `ESC [ I` (gained) or `ESC [ O` (lost), only while the application has
    /// focus reporting on (`CSI ? 1004 h`); `None` otherwise or with `disable_stdin`. As xterm.js's
    /// browser terminal it is sent through `triggerDataEvent` without the user-input flag, so it
    /// does not scroll.
    pub fn focus_event(&mut self, focused: bool) -> Option<Vec<u8>> {
        if !self
            .core
            .input_handler()
            .services()
            .core
            .dec_private_modes
            .send_focus
        {
            return None;
        }
        let report: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
        self.send_data(report.to_vec(), false)
    }

    /// The bytes to write to the PTY for pasting `text`: line breaks as `\r`, and with bracketed
    /// paste on (`CSI ? 2004 h`) the sanitized text between `ESC [ 200 ~` and `ESC [ 201 ~` (see
    /// [`paste_bytes`], which holds the invariant). `None` for an empty text or with
    /// `disable_stdin`. Like xterm.js's `paste` it goes through `triggerDataEvent(.., true)`. The
    /// text is never logged (the core logs lengths only).
    pub fn paste(&mut self, text: &str) -> Option<Vec<u8>> {
        if text.is_empty() {
            return None;
        }
        let bracketed = self
            .core
            .input_handler()
            .services()
            .core
            .dec_private_modes
            .bracketed_paste_mode;
        self.send_data(paste_bytes(text, bracketed), true)
    }

    /// `triggerDataEvent` (plan M7 D6): the bytes to send, `None` if there are none or stdin is
    /// disabled; with `was_user_input` and `scroll_on_user_input` the display scrolls to the
    /// bottom first. The encoders only produce UTF-8, so the text passes through as is.
    fn send_data(&mut self, bytes: Vec<u8>, was_user_input: bool) -> Option<Vec<u8>> {
        if bytes.is_empty() {
            return None;
        }
        let data = String::from_utf8(bytes).ok()?;
        let services = &mut self.core.input_handler_mut().services;
        let event = services.core.trigger_data_event(
            &data,
            was_user_input,
            services.buffer.buffer(),
            services.options.options(),
            &services.log,
        )?;
        if event.scroll_to_bottom {
            self.core.scroll_to_bottom();
        }
        Some(data.into_bytes())
    }
}

impl VtEngine for Terminal {
    fn columns(&self) -> u16 {
        u16::try_from(self.cols()).unwrap_or(u16::MAX)
    }

    fn rows(&self) -> u16 {
        u16::try_from(Terminal::rows(self)).unwrap_or(u16::MAX)
    }

    fn write(&mut self, data: &[u8]) {
        Terminal::write(self, data);
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        Terminal::resize(self, usize::from(columns), usize::from(rows));
    }

    fn scroll_lines(&mut self, delta: i32) {
        Terminal::scroll_lines(self, i64::from(delta));
    }

    fn scroll_to_top(&mut self) {
        Terminal::scroll_to_top(self);
    }

    fn scroll_to_bottom(&mut self) {
        Terminal::scroll_to_bottom(self);
    }

    fn drain_events(&mut self) -> Vec<HostEvent> {
        Terminal::drain_events(self)
    }

    fn take_damage(&mut self) -> Damage {
        Terminal::take_damage(self)
    }

    fn viewport(&self) -> Viewport {
        Terminal::viewport(self)
    }

    fn modes(&self) -> Modes {
        Terminal::modes(self)
    }
}

/// Headless `reset` on the handler alone (`Terminal.ts:129-139`), so the RIS handler (inside a
/// parse) can run it: keep the size in the options, rerun `_setup`, then `CoreTerminal.reset`.
pub(crate) fn reset_terminal(input_handler: &mut InputHandler) {
    let buffer = &input_handler.services.buffer;
    let (cols, rows) = (buffer.cols, buffer.rows);
    let options = &mut input_handler.services.options;
    // Both values are always valid.
    let _ = options.set(OptionValue::Rows(rows));
    let _ = options.set(OptionValue::Cols(cols));
    handle_windows_pty_option_change(input_handler);
    core_reset(input_handler);
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// An empty cell: no text, width 1, default colours.
fn blank_cell() -> ViewportCell {
    ViewportCell {
        width: 1,
        ..ViewportCell::default()
    }
}

/// A colour word (`fg`, `bg` or the underline colour) by its colour mode.
fn cell_color(value: u32) -> CellColor {
    let byte = |shift: u32| u8::try_from((value >> shift) & 0xFF).unwrap_or(0);
    match value & Attributes::CM_MASK {
        Attributes::CM_P16 | Attributes::CM_P256 => CellColor::Palette(byte(0)),
        Attributes::CM_RGB => CellColor::Rgb(byte(16), byte(8), byte(0)),
        _ => CellColor::Default,
    }
}

/// The underline the application set: the raw style bits (a url id makes upstream's accessor
/// read DASHED; the link is reported through `link_id` instead). 6 and 7, which no sequence
/// produces, read as single.
fn underline_style(cell: &CellData, extended: Option<ExtendedAttrs>) -> UnderlineStyle {
    if cell.fg & FgFlags::UNDERLINE == 0 {
        return UnderlineStyle::None;
    }
    let Some(mut extended) = extended else {
        return UnderlineStyle::Single;
    };
    extended.set_url_id(0);
    match extended.underline_style() {
        0 => UnderlineStyle::None,
        2 => UnderlineStyle::Double,
        3 => UnderlineStyle::Curly,
        4 => UnderlineStyle::Dotted,
        5 => UnderlineStyle::Dashed,
        _ => UnderlineStyle::Single,
    }
}

fn viewport_cell(cell: &CellData) -> ViewportCell {
    // `load_cell` leaves `extended` stale unless the cell has extended attributes
    let extended = (cell.bg & BgFlags::HAS_EXTENDED != 0).then_some(cell.extended);
    ViewportCell {
        text: cell.get_chars(),
        width: u8::try_from(cell.get_width()).unwrap_or(0),
        fg: cell_color(cell.fg),
        bg: cell_color(cell.bg),
        bold: cell.is_bold() != 0,
        dim: cell.is_dim() != 0,
        italic: cell.is_italic() != 0,
        blink: cell.is_blink() != 0,
        inverse: cell.is_inverse() != 0,
        invisible: cell.is_invisible() != 0,
        strikethrough: cell.is_strikethrough() != 0,
        overline: cell.is_overline() != 0,
        underline: underline_style(cell, extended),
        underline_color: extended.map_or(CellColor::Default, |e| cell_color(e.underline_color())),
        link_id: extended.map_or(0, |e| e.url_id()),
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
