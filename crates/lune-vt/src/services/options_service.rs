//! Port of xterm.js src/common/services/OptionsService.ts @ f447274, with the option types from
//! `Services.ts:221-274` and `Types.ts:385-410` (tests: OptionsService.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D3):
//!  - Upstream's `ITerminalOptions` has 47 keys. Only the 21 the core reads are ported, as the
//!    typed [`TerminalOptions`]; renderer and browser options (fonts, theme, contrast, scroll
//!    animation, mac keyboard and selection flags, link handler, logger, ...) belong to
//!    `lune-termview`, the app settings and the LogService decision. Consumers that read
//!    `rawOptions` live take `&TerminalOptions` from [`OptionsService::options`].
//!  - A key is an [`OptionKey`] and a keyed value an [`OptionValue`]; there is no string-keyed
//!    access, so upstream's `No option with key "..."` error and the `key in defaultOptions`
//!    filter in the constructor cannot occur.
//!  - Upstream validates untyped JS values; most of that validation is the type system here.
//!    `cursorStyle` is a [`CursorStyle`] (no invalid state, no falsy-to-default), `scrollback`
//!    is unsigned (upstream's `< 0` error is unrepresentable) and `cols`/`rows` are numbers (the
//!    `must be numeric` error is unrepresentable). What remains is [`OptionError`].
//!  - `onOptionChange` becomes a return value: [`OptionsService::set`] returns `Some(key)`
//!    exactly when upstream fires, that is when the stored value changed. `onSpecificOptionChange`
//!    is `key == wanted` followed by a read of [`OptionsService::options`];
//!    `onMultipleOptionChange` is [`OptionKey::is_one_of`].
//!  - "Changed" is value equality. Upstream compares with `!==`, which for the object-valued
//!    `windowOptions` and `windowsPty` is reference identity (assigning an equal object fires);
//!    here an equal value is no change. Both keys only recompute derived state, so the extra
//!    event upstream fires for an equal object carries no information. For the `f64`
//!    sensitivities a NaN is never equal to itself, so it reports a change every time, as in JS.
//!  - `scrollback`, `tabStopWidth` and `cols`/`rows` are `usize`.
//!  - The constructor drops an invalid initial value and keeps the default (upstream catches the
//!    exception and `console.error`s it; this crate has no logger). No event is reported for
//!    constructor values.
//!  - `NaN` passes the `<= 0` sensitivity check in both implementations (comparisons with NaN
//!    are false); it is kept.
//!  - Upstream's dispose hook, which clears `linkHandler` and `documentOverride`, has nothing to
//!    clear: those options are not ported.

use std::fmt;

use crate::buffer::buffer::MAX_BUFFER_SIZE;

/// `Buffer.ts:19` `MAX_BUFFER_SIZE`, which `_sanitizeAndValidateOption` repeats as the literal
/// `4294967295` (`OptionsService.ts:185`): the largest accepted `scrollback`.
const MAX_SCROLLBACK: usize = MAX_BUFFER_SIZE;

/// `DEFAULT_OPTIONS.wordSeparator` (`OptionsService.ts:54`).
pub const DEFAULT_WORD_SEPARATOR: &str = " ()[]{}',\"`";

/// `CursorStyle` (`Types.ts:39`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    /// `'block'` (the default).
    #[default]
    Block,
    /// `'underline'`.
    Underline,
    /// `'bar'`.
    Bar,
}

/// `LogLevel` (`Services.ts:221`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogLevel {
    /// `'trace'`.
    Trace,
    /// `'debug'`.
    Debug,
    /// `'info'` (the default).
    #[default]
    Info,
    /// `'warn'`.
    Warn,
    /// `'error'`.
    Error,
    /// `'off'`.
    Off,
}

/// `IWindowsPty.backend` (xterm-headless.d.ts:340): the Windows pty emulation in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyBackendKind {
    /// `'conpty'`.
    Conpty,
    /// `'winpty'`.
    Winpty,
}

/// `IWindowsPty` (xterm-headless.d.ts:336-345). Both fields absent is the default (`{}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowsPty {
    /// Which pty backend is in use.
    pub backend: Option<PtyBackendKind>,
    /// The Windows build version (e.g. 19045).
    pub build_number: Option<u32>,
}

/// `IWindowOptions` (`Types.ts:387-410`): which `CSI Ps ; ... t` window operations the terminal
/// honours. Every flag defaults to off (`windowOptions: {}`, `OptionsService.ts:51`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(missing_docs)] // one flag per upstream property, same names in snake case
pub struct WindowOptions {
    pub restore_win: bool,
    pub minimize_win: bool,
    pub set_win_position: bool,
    pub set_win_size_pixels: bool,
    pub raise_win: bool,
    pub lower_win: bool,
    pub refresh_win: bool,
    pub set_win_size_chars: bool,
    pub maximize_win: bool,
    pub fullscreen_win: bool,
    pub get_win_state: bool,
    pub get_win_position: bool,
    pub get_win_size_pixels: bool,
    pub get_screen_size_pixels: bool,
    pub get_cell_size_pixels: bool,
    pub get_win_size_chars: bool,
    pub get_screen_size_chars: bool,
    pub get_icon_title: bool,
    pub get_win_title: bool,
    pub push_title: bool,
    pub pop_title: bool,
    pub set_win_lines: bool,
}

/// The options the core reads (plan M5 D3), with `DEFAULT_OPTIONS` values (`OptionsService.ts`
/// line per field). Read it through [`OptionsService::options`].
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalOptions {
    /// Columns (`:13`, 80).
    pub cols: usize,
    /// Rows (`:14`, 24).
    pub rows: usize,
    /// Scrollback lines, at most 2^32 - 1 (`:34`, 1000).
    pub scrollback: usize,
    /// Tab stop interval, at least 1 (`:46`, 8).
    pub tab_stop_width: usize,
    /// Reflow the cursor line on resize (`:48`, false).
    pub reflow_cursor_line: bool,
    /// Windows pty mode (`:52`, false).
    pub windows_mode: bool,
    /// Windows pty backend and build (`:53`, `{}`).
    pub windows_pty: WindowsPty,
    /// Treat LF as CR LF (`:56`, false).
    pub convert_eol: bool,
    /// Terminal name (`:57`, `"xterm"`).
    pub term_name: String,
    /// Characters that delimit words, never empty (`:54`, [`DEFAULT_WORD_SEPARATOR`]).
    pub word_separator: String,
    /// Push the screen into scrollback on erase in display (`:35`, false).
    pub scroll_on_erase_in_display: bool,
    /// Scroll to the bottom on user input (`:36`, true).
    pub scroll_on_user_input: bool,
    /// Ignore keyboard input (`:43`, false).
    pub disable_stdin: bool,
    /// Blinking cursor (`:15`, false).
    pub cursor_blink: bool,
    /// Cursor shape (`:16`, block).
    pub cursor_style: CursorStyle,
    /// Screen reader mode (`:38`, false).
    pub screen_reader_mode: bool,
    /// Which window operations are honoured (`:51`, all off).
    pub window_options: WindowOptions,
    /// Log verbosity (`:32`, info).
    pub log_level: LogLevel,
    /// Allow proposed API (`:44`, false).
    pub allow_proposed_api: bool,
    /// Scroll multiplier while the fast-scroll modifier is held, above 0 (`:23`, 5).
    pub fast_scroll_sensitivity: f64,
    /// Scroll multiplier, above 0 (`:37`, 1).
    pub scroll_sensitivity: f64,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback: 1000,
            tab_stop_width: 8,
            reflow_cursor_line: false,
            windows_mode: false,
            windows_pty: WindowsPty::default(),
            convert_eol: false,
            term_name: "xterm".to_owned(),
            word_separator: DEFAULT_WORD_SEPARATOR.to_owned(),
            scroll_on_erase_in_display: false,
            scroll_on_user_input: true,
            disable_stdin: false,
            cursor_blink: false,
            cursor_style: CursorStyle::Block,
            screen_reader_mode: false,
            window_options: WindowOptions::default(),
            log_level: LogLevel::Info,
            allow_proposed_api: false,
            fast_scroll_sensitivity: 5.0,
            scroll_sensitivity: 1.0,
        }
    }
}

/// An option name: the `keyof ITerminalOptions` of the options the core reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptionKey {
    /// `cols`.
    Cols,
    /// `rows`.
    Rows,
    /// `scrollback`.
    Scrollback,
    /// `tabStopWidth`.
    TabStopWidth,
    /// `reflowCursorLine`.
    ReflowCursorLine,
    /// `windowsMode`.
    WindowsMode,
    /// `windowsPty`.
    WindowsPty,
    /// `convertEol`.
    ConvertEol,
    /// `termName`.
    TermName,
    /// `wordSeparator`.
    WordSeparator,
    /// `scrollOnEraseInDisplay`.
    ScrollOnEraseInDisplay,
    /// `scrollOnUserInput`.
    ScrollOnUserInput,
    /// `disableStdin`.
    DisableStdin,
    /// `cursorBlink`.
    CursorBlink,
    /// `cursorStyle`.
    CursorStyle,
    /// `screenReaderMode`.
    ScreenReaderMode,
    /// `windowOptions`.
    WindowOptions,
    /// `logLevel`.
    LogLevel,
    /// `allowProposedApi`.
    AllowProposedApi,
    /// `fastScrollSensitivity`.
    FastScrollSensitivity,
    /// `scrollSensitivity`.
    ScrollSensitivity,
}

impl OptionKey {
    /// Every key, once (21: the options the core reads).
    pub const ALL: [OptionKey; 21] = [
        OptionKey::Cols,
        OptionKey::Rows,
        OptionKey::Scrollback,
        OptionKey::TabStopWidth,
        OptionKey::ReflowCursorLine,
        OptionKey::WindowsMode,
        OptionKey::WindowsPty,
        OptionKey::ConvertEol,
        OptionKey::TermName,
        OptionKey::WordSeparator,
        OptionKey::ScrollOnEraseInDisplay,
        OptionKey::ScrollOnUserInput,
        OptionKey::DisableStdin,
        OptionKey::CursorBlink,
        OptionKey::CursorStyle,
        OptionKey::ScreenReaderMode,
        OptionKey::WindowOptions,
        OptionKey::LogLevel,
        OptionKey::AllowProposedApi,
        OptionKey::FastScrollSensitivity,
        OptionKey::ScrollSensitivity,
    ];

    /// The upstream property name (used in error text).
    pub fn name(self) -> &'static str {
        match self {
            OptionKey::Cols => "cols",
            OptionKey::Rows => "rows",
            OptionKey::Scrollback => "scrollback",
            OptionKey::TabStopWidth => "tabStopWidth",
            OptionKey::ReflowCursorLine => "reflowCursorLine",
            OptionKey::WindowsMode => "windowsMode",
            OptionKey::WindowsPty => "windowsPty",
            OptionKey::ConvertEol => "convertEol",
            OptionKey::TermName => "termName",
            OptionKey::WordSeparator => "wordSeparator",
            OptionKey::ScrollOnEraseInDisplay => "scrollOnEraseInDisplay",
            OptionKey::ScrollOnUserInput => "scrollOnUserInput",
            OptionKey::DisableStdin => "disableStdin",
            OptionKey::CursorBlink => "cursorBlink",
            OptionKey::CursorStyle => "cursorStyle",
            OptionKey::ScreenReaderMode => "screenReaderMode",
            OptionKey::WindowOptions => "windowOptions",
            OptionKey::LogLevel => "logLevel",
            OptionKey::AllowProposedApi => "allowProposedApi",
            OptionKey::FastScrollSensitivity => "fastScrollSensitivity",
            OptionKey::ScrollSensitivity => "scrollSensitivity",
        }
    }

    /// Whether this key is in `keys`: the filter of `onMultipleOptionChange`.
    pub fn is_one_of(self, keys: &[OptionKey]) -> bool {
        keys.contains(&self)
    }
}

/// An option with its new value: one variant per [`OptionKey`].
#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    /// `cols`.
    Cols(usize),
    /// `rows`.
    Rows(usize),
    /// `scrollback`; values above 2^32 - 1 are clamped to it.
    Scrollback(usize),
    /// `tabStopWidth`; below 1 is an error.
    TabStopWidth(usize),
    /// `reflowCursorLine`.
    ReflowCursorLine(bool),
    /// `windowsMode`.
    WindowsMode(bool),
    /// `windowsPty`; `None` stores the default (`value ?? {}`).
    WindowsPty(Option<WindowsPty>),
    /// `convertEol`.
    ConvertEol(bool),
    /// `termName`.
    TermName(String),
    /// `wordSeparator`; empty stores the default.
    WordSeparator(String),
    /// `scrollOnEraseInDisplay`.
    ScrollOnEraseInDisplay(bool),
    /// `scrollOnUserInput`.
    ScrollOnUserInput(bool),
    /// `disableStdin`.
    DisableStdin(bool),
    /// `cursorBlink`.
    CursorBlink(bool),
    /// `cursorStyle`.
    CursorStyle(CursorStyle),
    /// `screenReaderMode`.
    ScreenReaderMode(bool),
    /// `windowOptions`.
    WindowOptions(WindowOptions),
    /// `logLevel`.
    LogLevel(LogLevel),
    /// `allowProposedApi`.
    AllowProposedApi(bool),
    /// `fastScrollSensitivity`; at or below 0 is an error.
    FastScrollSensitivity(f64),
    /// `scrollSensitivity`; at or below 0 is an error.
    ScrollSensitivity(f64),
}

impl OptionValue {
    /// The key this value is for.
    pub fn key(&self) -> OptionKey {
        match self {
            OptionValue::Cols(_) => OptionKey::Cols,
            OptionValue::Rows(_) => OptionKey::Rows,
            OptionValue::Scrollback(_) => OptionKey::Scrollback,
            OptionValue::TabStopWidth(_) => OptionKey::TabStopWidth,
            OptionValue::ReflowCursorLine(_) => OptionKey::ReflowCursorLine,
            OptionValue::WindowsMode(_) => OptionKey::WindowsMode,
            OptionValue::WindowsPty(_) => OptionKey::WindowsPty,
            OptionValue::ConvertEol(_) => OptionKey::ConvertEol,
            OptionValue::TermName(_) => OptionKey::TermName,
            OptionValue::WordSeparator(_) => OptionKey::WordSeparator,
            OptionValue::ScrollOnEraseInDisplay(_) => OptionKey::ScrollOnEraseInDisplay,
            OptionValue::ScrollOnUserInput(_) => OptionKey::ScrollOnUserInput,
            OptionValue::DisableStdin(_) => OptionKey::DisableStdin,
            OptionValue::CursorBlink(_) => OptionKey::CursorBlink,
            OptionValue::CursorStyle(_) => OptionKey::CursorStyle,
            OptionValue::ScreenReaderMode(_) => OptionKey::ScreenReaderMode,
            OptionValue::WindowOptions(_) => OptionKey::WindowOptions,
            OptionValue::LogLevel(_) => OptionKey::LogLevel,
            OptionValue::AllowProposedApi(_) => OptionKey::AllowProposedApi,
            OptionValue::FastScrollSensitivity(_) => OptionKey::FastScrollSensitivity,
            OptionValue::ScrollSensitivity(_) => OptionKey::ScrollSensitivity,
        }
    }
}

/// A value `_sanitizeAndValidateOption` rejects (`OptionsService.ts:149-207`).
#[derive(Debug, Clone, PartialEq)]
pub enum OptionError {
    /// `<key> cannot be less than 1, value: <value>` (`tabStopWidth`).
    LessThanOne {
        /// The rejected option.
        key: OptionKey,
        /// The rejected value.
        value: usize,
    },
    /// `<key> cannot be less than or equal to 0, value: <value>` (the scroll sensitivities).
    NotPositive {
        /// The rejected option.
        key: OptionKey,
        /// The rejected value.
        value: f64,
    },
}

impl fmt::Display for OptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OptionError::LessThanOne { key, value } => {
                write!(f, "{} cannot be less than 1, value: {}", key.name(), value)
            }
            OptionError::NotPositive { key, value } => {
                write!(
                    f,
                    "{} cannot be less than or equal to 0, value: {}",
                    key.name(),
                    value
                )
            }
        }
    }
}

impl std::error::Error for OptionError {}

/// Replace `slot` with `value`; `true` iff it differed.
fn assign<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

/// Holds the validated options. See the module notes for how upstream's events map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptionsService {
    options: TerminalOptions,
}

impl OptionsService {
    /// Defaults, then each of `initial` in order; a value that fails validation is dropped and
    /// the default kept (`:73-99`).
    pub fn new(initial: impl IntoIterator<Item = OptionValue>) -> Self {
        let mut service = Self::default();
        for value in initial {
            // Upstream logs the exception and moves on.
            let _ = service.set(value);
        }
        service
    }

    /// The current options (`rawOptions`). Read it again after each [`OptionsService::set`]:
    /// upstream's readers see changes live.
    pub fn options(&self) -> &TerminalOptions {
        &self.options
    }

    /// Validate `value` as `_sanitizeAndValidateOption` and store it (the `options` setter,
    /// `:119-147`). `Ok(Some(key))` when the stored value changed (upstream fires
    /// `onOptionChange` then), `Ok(None)` when it was already that value, and `Err` (nothing
    /// stored) for a rejected value.
    pub fn set(&mut self, value: OptionValue) -> Result<Option<OptionKey>, OptionError> {
        let key = value.key();
        let opts = &mut self.options;
        let changed = match value {
            OptionValue::Cols(v) => assign(&mut opts.cols, v),
            OptionValue::Rows(v) => assign(&mut opts.rows, v),
            OptionValue::Scrollback(v) => assign(&mut opts.scrollback, v.min(MAX_SCROLLBACK)),
            OptionValue::TabStopWidth(v) => {
                if v < 1 {
                    return Err(OptionError::LessThanOne { key, value: v });
                }
                assign(&mut opts.tab_stop_width, v)
            }
            OptionValue::ReflowCursorLine(v) => assign(&mut opts.reflow_cursor_line, v),
            OptionValue::WindowsMode(v) => assign(&mut opts.windows_mode, v),
            OptionValue::WindowsPty(v) => assign(&mut opts.windows_pty, v.unwrap_or_default()),
            OptionValue::ConvertEol(v) => assign(&mut opts.convert_eol, v),
            OptionValue::TermName(v) => assign(&mut opts.term_name, v),
            OptionValue::WordSeparator(v) => {
                let v = if v.is_empty() {
                    DEFAULT_WORD_SEPARATOR.to_owned()
                } else {
                    v
                };
                assign(&mut opts.word_separator, v)
            }
            OptionValue::ScrollOnEraseInDisplay(v) => {
                assign(&mut opts.scroll_on_erase_in_display, v)
            }
            OptionValue::ScrollOnUserInput(v) => assign(&mut opts.scroll_on_user_input, v),
            OptionValue::DisableStdin(v) => assign(&mut opts.disable_stdin, v),
            OptionValue::CursorBlink(v) => assign(&mut opts.cursor_blink, v),
            OptionValue::CursorStyle(v) => assign(&mut opts.cursor_style, v),
            OptionValue::ScreenReaderMode(v) => assign(&mut opts.screen_reader_mode, v),
            OptionValue::WindowOptions(v) => assign(&mut opts.window_options, v),
            OptionValue::LogLevel(v) => assign(&mut opts.log_level, v),
            OptionValue::AllowProposedApi(v) => assign(&mut opts.allow_proposed_api, v),
            OptionValue::FastScrollSensitivity(v) => {
                if v <= 0.0 {
                    return Err(OptionError::NotPositive { key, value: v });
                }
                assign(&mut opts.fast_scroll_sensitivity, v)
            }
            OptionValue::ScrollSensitivity(v) => {
                if v <= 0.0 {
                    return Err(OptionError::NotPositive { key, value: v });
                }
                assign(&mut opts.scroll_sensitivity, v)
            }
        };
        Ok(changed.then_some(key))
    }
}

#[cfg(test)]
#[path = "options_service_tests.rs"]
mod tests;
