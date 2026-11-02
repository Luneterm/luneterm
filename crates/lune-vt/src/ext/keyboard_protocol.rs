//! Keyboard protocol state: the kitty keyboard protocol (flags 1 and 4) and xterm
//! modifyOtherKeys, Lunegit `driver.js` `:80-116, :287-369` (HEAD 33a1dd4). Not in xterm.js
//! 6.0.0, which has no handler for any of these sequences.
//!
//! The handlers only keep state and answer queries; the encoders (plan M7 step 5) read the state
//! to shape key sequences. The output stream is untrusted, so everything is bounded: at most
//! [`KITTY_STACK_MAX`] entries per screen, stored flags masked to [`KITTY_SUPPORTED`],
//! modifyOtherKeys clamped to `0..=2`. Handler return values decide whether xterm's own handler
//! (or a fallback) still runs, as in `driver.js`: a sequence this extension does not own, or one
//! with a sub-parameter, returns `false` and changes nothing.
//!
//! Rust shape (plan M7 D1, D11):
//!  - The state is [`KeyboardProtocol`] in [`ExtState`](super::ExtState); the active screen is
//!    read per sequence (`CSI ? 1049 h` can arrive mid-write).
//!  - Resets: RIS and `Terminal::reset` reach [`KeyboardProtocol::reset`] through `core_reset`
//!    (a `// LUNETERM:` line there); DECSTR (`CSI ! p`) through the handler below, which returns
//!    `false` so `soft_reset` still runs. Alt-screen switches reset nothing.

use crate::input_handler::InputHandler;
use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::params::Params;
use crate::parser::types::{FunctionIdentifier, IdentifierError};

/// The kitty flags honoured: 1 (disambiguate escape codes) and 4 (report alternate keys).
/// 2, 8 and 16 are never stored, so a query never claims what the encoders do not do.
pub const KITTY_SUPPORTED: u8 = 5;

/// Most kitty flag entries per screen; the oldest is evicted on overflow (kitty `screen.h`).
pub const KITTY_STACK_MAX: usize = 8;

/// The highest modifyOtherKeys level.
pub const MODIFY_OTHER_KEYS_MAX: u8 = 2;

/// The keyboard protocol state of one terminal: a kitty flag stack per screen and the
/// modifyOtherKeys level (global, like xterm's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyboardProtocol {
    main: Vec<u8>,
    alt: Vec<u8>,
    modify_other_keys: u8,
}

impl KeyboardProtocol {
    fn stack_mut(&mut self, alt: bool) -> &mut Vec<u8> {
        if alt { &mut self.alt } else { &mut self.main }
    }

    /// The kitty flags in force on a screen: the top of its stack, 0 when empty. Always a subset
    /// of [`KITTY_SUPPORTED`].
    pub fn flags(&self, alt: bool) -> u8 {
        let stack = if alt { &self.alt } else { &self.main };
        stack.last().copied().unwrap_or(0)
    }

    /// The modifyOtherKeys level, `0..=2`.
    pub fn modify_other_keys(&self) -> u8 {
        self.modify_other_keys
    }

    /// The depth of the main and the alternate screen's kitty stack, each at most
    /// [`KITTY_STACK_MAX`] (for the host's diagnostics and the engine fuzz bounds).
    pub fn stack_depths(&self) -> (usize, usize) {
        (self.main.len(), self.alt.len())
    }

    /// Clear both stacks and the level (RIS, DECSTR, the host's reset hook).
    pub fn reset(&mut self) {
        self.main.clear();
        self.alt.clear();
        self.modify_other_keys = 0;
    }

    /// `CSI > flags u`: push, evicting the oldest entry when the stack is full.
    pub(crate) fn push(&mut self, alt: bool, flags: i32) {
        let stack = self.stack_mut(alt);
        if stack.len() >= KITTY_STACK_MAX {
            stack.remove(0);
        }
        stack.push(mask(flags));
    }

    /// `CSI < n u`: pop `n` entries; popping past empty just empties the stack.
    fn pop(&mut self, alt: bool, n: i32) {
        let stack = self.stack_mut(alt);
        let n = usize::try_from(n.max(1)).unwrap_or(1);
        stack.truncate(stack.len().saturating_sub(n));
    }

    /// `CSI = flags ; mode u` with `mode` 1 (set), 2 (or) or 3 (and-not); the current entry is
    /// created when the stack is empty.
    fn modify(&mut self, alt: bool, flags: i32, mode: i32) {
        let flags = mask(flags);
        let stack = self.stack_mut(alt);
        if stack.is_empty() {
            stack.push(0);
        }
        if let Some(top) = stack.last_mut() {
            *top = match mode {
                1 => flags,
                2 => *top | flags,
                _ => *top & !flags & KITTY_SUPPORTED,
            };
        }
    }
}

/// `flags & KITTY_SUPPORTED`.
fn mask(flags: i32) -> u8 {
    u8::try_from(flags & i32::from(KITTY_SUPPORTED)).unwrap_or(0)
}

/// Parameter `idx`, non-negative (an omitted first parameter arrives as 0).
fn param(params: &Params, idx: usize) -> i32 {
    params.get(idx).unwrap_or(0).max(0)
}

/// `__kbdHasSub`: no parameters, or a sub-parameter on any of them.
fn has_sub(params: &Params) -> bool {
    params.length() == 0 || (0..params.length()).any(|i| params.has_sub_params(i))
}

fn active_is_alt(h: &InputHandler) -> bool {
    h.services.buffer.buffers.is_alt_active()
}

/// Register the handlers on `parser`.
pub(super) fn register(
    parser: &mut EscapeSequenceParser<InputHandler>,
) -> Result<(), IdentifierError> {
    // CSI ? u: query. Only the bare form; reply with the active screen's flags.
    parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'u'), |h, p| {
        if has_sub(p) || p.length() != 1 || param(p, 0) != 0 {
            return false;
        }
        let flags = h.ext.keyboard.flags(active_is_alt(h));
        h.send_data(&format!("\u{1b}[?{flags}u"));
        true
    })?;

    // CSI > flags u: push onto the active screen's stack.
    parser.register_csi_handler(FunctionIdentifier::with_prefix('>', 'u'), |h, p| {
        if has_sub(p) {
            return false;
        }
        let alt = active_is_alt(h);
        h.ext.keyboard.push(alt, param(p, 0));
        true
    })?;

    // CSI < n u: pop n entries (0 or omitted: 1).
    parser.register_csi_handler(FunctionIdentifier::with_prefix('<', 'u'), |h, p| {
        if has_sub(p) {
            return false;
        }
        let alt = active_is_alt(h);
        h.ext.keyboard.pop(alt, param(p, 0));
        true
    })?;

    // CSI = flags ; mode u: set (1, also 0 or omitted), or (2), and-not (3) on the current
    // entry. An unknown mode is consumed without a change.
    parser.register_csi_handler(FunctionIdentifier::with_prefix('=', 'u'), |h, p| {
        if has_sub(p) {
            return false;
        }
        let mode = if p.length() > 1 && param(p, 1) != 0 {
            param(p, 1)
        } else {
            1
        };
        if !(1..=3).contains(&mode) {
            return true;
        }
        let alt = active_is_alt(h);
        h.ext.keyboard.modify(alt, param(p, 0), mode);
        true
    })?;

    // CSI > Pp ; Pv m: XTMODKEYS. Only resource 4 (modifyOtherKeys) is tracked; `CSI > m`
    // arrives as `[0]` and resets the level, as xterm does for an omitted resource.
    parser.register_csi_handler(FunctionIdentifier::with_prefix('>', 'm'), |h, p| {
        if has_sub(p) {
            return false;
        }
        if p.length() == 1 && param(p, 0) == 0 {
            h.ext.keyboard.modify_other_keys = 0;
            return true;
        }
        if param(p, 0) != 4 {
            return false;
        }
        let value = if p.length() > 1 { param(p, 1) } else { 0 };
        h.ext.keyboard.modify_other_keys =
            u8::try_from(value.min(i32::from(MODIFY_OTHER_KEYS_MAX))).unwrap_or(0);
        true
    })?;

    // CSI > Ps n: disable a modify-keys resource. Only resource 4 is ours.
    parser.register_csi_handler(FunctionIdentifier::with_prefix('>', 'n'), |h, p| {
        if has_sub(p) || param(p, 0) != 4 {
            return false;
        }
        h.ext.keyboard.modify_other_keys = 0;
        true
    })?;

    // CSI ? Pp m: XTQMODKEYS. One `CSI > 4 ; level m` per parameter equal to 4; other resources
    // get no reply. Bounded by the parser's parameter limit.
    parser.register_csi_handler(FunctionIdentifier::with_prefix('?', 'm'), |h, p| {
        if has_sub(p) {
            return false;
        }
        let mut answered = false;
        for i in 0..p.length() {
            if param(p, i) == 4 {
                let level = h.ext.keyboard.modify_other_keys;
                h.send_data(&format!("\u{1b}[>4;{level}m"));
                answered = true;
            }
        }
        answered
    })?;

    // DECSTR clears both stacks and the level. It returns false so `soft_reset` still runs
    // (returning true would disable DECSTR). RIS does the same through `core_reset`.
    parser.register_csi_handler(FunctionIdentifier::with_intermediates("!", 'p'), |h, _| {
        h.ext.keyboard.reset();
        false
    })?;
    Ok(())
}
