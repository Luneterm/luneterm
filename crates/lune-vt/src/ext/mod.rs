//! Lunegit's terminal additions to the ported xterm.js core, as parser extensions (plan M7).
//!
//! Lunegit's `driver.js` layers kitty keyboard flags, modifyOtherKeys, XTVERSION, VS16 width, an
//! OSC 52 write path and an unknown-OSC report on top of xterm.js; the OSC scanners and input
//! encoders around it add the cwd, progress and command marks. Here each is an extension of the
//! [`Extension`] kind: handlers on the same parser registry the built-in handlers use, none of
//! them patched into a ported file.
//!
//! Rust shape (plan M7 D1, D2):
//!  - The handlers must queue events and replies ([`InputHandler::push_event`],
//!    [`InputHandler::send_data`], both `pub(crate)`), so the extensions live in this crate. Their
//!    state lives in [`ExtState`], a field of [`InputHandler`] (`// LUNETERM:`), so the parser
//!    closures (`&mut InputHandler`) and the encoders (`&Terminal`) see the same state without
//!    a reference back to the host.
//!  - They are opt-in: [`crate::Terminal::install_standard_extensions`] registers the standard
//!    set once, so [`crate::Terminal::new`] (and every upstream-equal test and golden) keeps
//!    plain xterm.js behaviour.
//!  - The host supplies anything that names it: the XTVERSION name is a parameter, `lune-vt`
//!    carries no identity.

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

use std::fmt;

use crate::extension::Extension;
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::IdentifierError;

mod unknown_osc;
mod xtversion;

#[cfg(test)]
mod ext_tests;

/// What [`crate::Terminal::install_standard_extensions`] registers, beyond the always-on
/// handlers (the unknown-OSC report).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StandardExtensions {
    /// The terminal name XTVERSION (`CSI > q`) reports, as `DCS > | <name> ST`. `None`: the
    /// query is not answered. At install it must be 1 to 64 bytes, each in `0x20 ..= 0x7E`: the
    /// name is written into the child's input, so anything else is refused
    /// ([`ExtensionError::InvalidXtversion`]).
    pub xtversion: Option<String>,
}

/// Why [`crate::Terminal::install_standard_extensions`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionError {
    /// A handler's function identifier was invalid (a bug in the registrations).
    Identifier(IdentifierError),
    /// [`StandardExtensions::xtversion`] is empty, longer than 64 bytes or holds a byte outside
    /// `0x20 ..= 0x7E`. Nothing was registered.
    InvalidXtversion,
    /// The standard extensions are already installed on this terminal.
    AlreadyInstalled,
}

impl fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtensionError::Identifier(error) => write!(f, "invalid function identifier: {error}"),
            ExtensionError::InvalidXtversion => {
                f.write_str("the XTVERSION name must be 1 to 64 bytes of printable ASCII")
            }
            ExtensionError::AlreadyInstalled => {
                f.write_str("the standard extensions are already installed")
            }
        }
    }
}

impl std::error::Error for ExtensionError {}

impl From<IdentifierError> for ExtensionError {
    fn from(error: IdentifierError) -> Self {
        ExtensionError::Identifier(error)
    }
}

/// The state of the standard extensions (plan M7 D1): a field of [`InputHandler`], so parser
/// closures and encoders read and write the same value. Everything here is bounded whatever the
/// PTY sends.
#[derive(Debug, Default)]
pub struct ExtState {
    /// [`install`] ran.
    pub(crate) installed: bool,
    /// Payload code points of the unknown OSC being received (reset at its start).
    pub(crate) unknown_osc_len: usize,
}

/// The extension [`install`] registers: every handler of the standard set on one parser.
struct StandardSet {
    xtversion: Option<String>,
}

impl Extension<InputHandler> for StandardSet {
    fn name(&self) -> &'static str {
        "standard"
    }

    fn register(
        &self,
        parser: &mut EscapeSequenceParser<InputHandler>,
    ) -> Result<(), IdentifierError> {
        if let Some(name) = &self.xtversion {
            xtversion::register(parser, name)?;
        }
        unknown_osc::register(parser);
        Ok(())
    }
}

/// Register the standard set on `parser` (once per terminal). Checks everything before it
/// registers anything; a failure after that leaves the terminal marked installed, so a retry
/// cannot register a handler twice.
pub(crate) fn install(
    parser: &mut EscapeSequenceParser<InputHandler>,
    input_handler: &mut InputHandler,
    config: &StandardExtensions,
) -> Result<(), ExtensionError> {
    if input_handler.ext.installed {
        return Err(ExtensionError::AlreadyInstalled);
    }
    if let Some(name) = &config.xtversion {
        if !xtversion::is_valid_name(name) {
            return Err(ExtensionError::InvalidXtversion);
        }
    }
    input_handler.ext.installed = true;
    StandardSet {
        xtversion: config.xtversion.clone(),
    }
    .register(parser)?;
    Ok(())
}
