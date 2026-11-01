//! Port of xterm.js src/common/services/CharsetService.ts @ f447274 (interface:
//! `Services.ts:114-134`). Upstream has no tests; `charset_service_tests.rs` is written from
//! the TypeScript behaviour.
//!
//! Rust notes (see `docs/porting-xterm.md`):
//!  - `ICharset | undefined` is `Option<&'static Charset>` (M4's [`crate::data::charsets`]).
//!  - Upstream's `_charsets` is a JS array, so an out-of-range `g` reads `undefined` and a write
//!    extends the array. The port keeps four slots (G0..G3, the only levels the escape sequences
//!    `ESC ( ) * +` and the SO/SI/LS2/LS3 shifts can name): a read at `g >= 4` is `None`, and a
//!    write at `g >= 4` is dropped from the slot table but still sets [`CharsetService::charset`]
//!    when `g` is the current level, as upstream does. This differs from upstream only when a
//!    level above 3 is written and later read back, which no caller does. `glevel` is not
//!    clamped.

use crate::data::charsets::Charset;

/// Number of designatable character sets (G0..G3).
const SLOTS: usize = 4;

/// The designated character sets and the active G level.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CharsetService {
    /// The character set in effect (the one designated for the current G level).
    pub charset: Option<&'static Charset>,
    glevel: usize,
    charsets: [Option<&'static Charset>; SLOTS],
}

impl CharsetService {
    /// A service with no sets designated and G0 active.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current G level.
    pub fn glevel(&self) -> usize {
        self.glevel
    }

    /// Forget all designations and select G0.
    pub fn reset(&mut self) {
        self.charset = None;
        self.charsets = [None; SLOTS];
        self.glevel = 0;
    }

    /// Select G level `g` and make its designated set current.
    pub fn set_g_level(&mut self, g: usize) {
        self.glevel = g;
        self.charset = self.charsets.get(g).copied().flatten();
    }

    /// Designate `charset` for G level `g`; if `g` is the current level it also becomes current.
    pub fn set_g_charset(&mut self, g: usize, charset: Option<&'static Charset>) {
        if let Some(slot) = self.charsets.get_mut(g) {
            *slot = charset;
        }
        if self.glevel == g {
            self.charset = charset;
        }
    }
}

#[cfg(test)]
#[path = "charset_service_tests.rs"]
mod tests;
