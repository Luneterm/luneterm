//! OSC 133 command marks and the synchronized-update latch (plan M7 step 8, D3, D8, D13). Not in
//! xterm.js 6.0.0. Lunegit reads them from the raw PTY bytes in `TerminalCommandScanner.cs` (HEAD
//! 33a1dd4); here 133 is an OSC handler, so the framing is the parser's.
//!
//! `133;<A|B|C|D>[;...]`: the mark is exactly one character, anything else is ignored. Each mark
//! is a [`HostEvent::CommandMark`] (never merged). `D;<digits>` carries the exit status when it
//! fits an `i32` (an original addition: Lunegit ignores the code). A payload over
//! [`MAX_PAYLOAD_BYTES`] bytes is discarded whole (Lunegit's 64-byte body, less `133;`).
//!
//! The latch (`TerminalCommandScanner.SawSynchronizedUpdateSinceCommandStart`) is set by a
//! DECSET (`CSI ? ... h`) whose parameters include 2026 as a plain number (a list `?1;2026h`
//! counts, `?2026:1h` does not), and cleared by 133 A, C and D. Its false to true transition
//! while a command is running (between a C and the next A or D) queues
//! [`HostEvent::SyncUpdateInCommand`]: once per command, because C clears the latch. The latch
//! also sets while no command runs (as Lunegit's flag does) but queues nothing then. The observer
//! returns `false`, so the core's DECSET handler still runs.

use crate::engine::{CommandMark, CommandMarkKind, HostEvent};
use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::{FunctionIdentifier, IdentifierError, OscHandler};

use super::osc_cwd::BoundedPayload;

/// Lunegit's cap on a 133 body (`133;C;...`), in bytes.
const MAX_BODY_BYTES: usize = 64;

/// The longest payload (the text after `133;`) kept, in UTF-8 bytes.
pub(super) const MAX_PAYLOAD_BYTES: usize = MAX_BODY_BYTES - 4;

/// The DEC private mode of synchronized output.
const SYNC_UPDATE_MODE: i32 = 2026;

/// The command and latch state (a field of [`super::ExtState`]).
#[derive(Debug, Default)]
pub(crate) struct CommandState {
    /// A C was seen and no A or D since.
    running: bool,
    /// The 2026 latch.
    sync_latch: bool,
}

impl CommandState {
    /// Whether a command is running (test hook).
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.running
    }

    /// The latch (test hook).
    #[cfg(test)]
    pub(crate) fn sync_latch(&self) -> bool {
        self.sync_latch
    }
}

/// The mark of an OSC 133 payload (the text after `133;`): `None` unless the first field is
/// exactly one of `A`, `B`, `C`, `D`.
pub(super) fn parse_mark(payload: &str) -> Option<CommandMark> {
    let mut fields = payload.split(';');
    let kind = match fields.next()? {
        "A" => CommandMarkKind::PromptStart,
        "B" => CommandMarkKind::PromptEnd,
        "C" => CommandMarkKind::OutputStart,
        "D" => CommandMarkKind::CommandFinished,
        _ => return None,
    };
    let exit_code = match kind {
        CommandMarkKind::CommandFinished => fields
            .next()
            .filter(|token| !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|token| token.parse::<i32>().ok()),
        _ => None,
    };
    Some(CommandMark { kind, exit_code })
}

struct CommandHandler {
    payload: BoundedPayload,
}

impl OscHandler<InputHandler> for CommandHandler {
    fn start(&mut self, _h: &mut InputHandler) {
        self.payload.clear();
    }

    fn put(&mut self, _h: &mut InputHandler, data: &[u32]) {
        self.payload.push_all(data);
    }

    fn end(&mut self, h: &mut InputHandler, success: bool) -> bool {
        let payload = self.payload.take();
        if !success {
            return false;
        }
        let Some(mark) = payload.as_deref().and_then(parse_mark) else {
            return true;
        };
        match mark.kind {
            CommandMarkKind::OutputStart => {
                h.ext.command.running = true;
                h.ext.command.sync_latch = false;
            }
            CommandMarkKind::PromptStart | CommandMarkKind::CommandFinished => {
                h.ext.command.running = false;
                h.ext.command.sync_latch = false;
            }
            CommandMarkKind::PromptEnd => {}
        }
        h.push_event(HostEvent::CommandMark(mark));
        true
    }
}

/// Register the OSC 133 handler and the DECSET observer on `parser`.
pub(super) fn register(
    parser: &mut EscapeSequenceParser<InputHandler>,
) -> Result<(), IdentifierError> {
    parser.register_osc_handler(
        133,
        CommandHandler {
            payload: BoundedPayload::new(MAX_PAYLOAD_BYTES),
        },
    );
    parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'h'), |h, params| {
        let sets_sync_update = (0..params.length())
            .any(|i| params.get(i) == Some(SYNC_UPDATE_MODE) && !params.has_sub_params(i));
        if sets_sync_update && !h.ext.command.sync_latch {
            h.ext.command.sync_latch = true;
            if h.ext.command.running {
                h.push_event(HostEvent::SyncUpdateInCommand);
            }
        }
        // an observer: the core's DECSET handler still runs
        false
    })?;
    Ok(())
}
