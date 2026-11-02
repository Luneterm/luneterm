//! XTVERSION (`CSI > q`), Lunegit `driver.js` `registerCsiHandler({ prefix: '>', final: 'q' })`
//! (HEAD 33a1dd4). Not in xterm.js 6.0.0.
//!
//! The reply is `DCS > | <name> ST`, the name from the host (plan M7 D10). Only the bare query
//! is answered: `CSI > q` and `CSI > 0 q`; any other parameter, a second parameter or a
//! sub-parameter (`CSI > 0 : 1 q`) falls through, as does a terminal without a name. The reply
//! goes through `send_data`, so it is queued in stream order with a DA1 reply of the same write.

use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::{FunctionIdentifier, IdentifierError};

/// The longest name accepted, in bytes.
const MAX_NAME_BYTES: usize = 64;

/// Whether `name` may be written into the child's input: 1 to 64 bytes, each `0x20 ..= 0x7E`
/// (so no ESC, no ST, no control).
pub(super) fn is_valid_name(name: &str) -> bool {
    (1..=MAX_NAME_BYTES).contains(&name.len()) && name.bytes().all(|b| (0x20..=0x7E).contains(&b))
}

/// Register the CSI `> q` handler answering with `name` (already validated).
pub(super) fn register(
    parser: &mut EscapeSequenceParser<InputHandler>,
    name: &str,
) -> Result<(), IdentifierError> {
    let reply = format!("\u{1b}P>|{name}\u{1b}\\");
    parser.register_csi_handler(
        FunctionIdentifier::with_prefix('>', 'q'),
        move |h, params| {
            let bare = match params.length() {
                0 => true,
                1 => params.get(0) == Some(0) && !params.has_sub_params(0),
                _ => false,
            };
            if !bare {
                return false;
            }
            h.send_data(&reply);
            true
        },
    )?;
    Ok(())
}
