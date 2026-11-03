//! VS16 (U+FE0F) emoji-presentation width, Lunegit `driver.js` `:179-214` (HEAD 33a1dd4). Not in
//! xterm.js 6.0.0.
//!
//! The Unicode 11 provider gives `U+26A0 U+FE0F` (warning sign + VS16) one cell, as it has no
//! emoji-presentation promotion; tmux, iTerm2, VS Code, macOS Terminal.app and Python's
//! `wcwidth` give it two. [`UnicodeV11Vs16`] wraps [`UnicodeV11`] and promotes a U+FE0F that
//! joins a width-1 character to width 2; every other result is the base provider's, so astral
//! emoji, CJK and combining marks are unchanged.
//!
//! Lunegit validates the base provider's packed layout at startup and skips the install if it
//! moved; here the layout is [`UnicodeService`]'s own (`create_property_value`), so the
//! promotion is written with those functions and the provider-level test pins it against the
//! base for every code point.

use crate::addons::unicode_v11::UnicodeV11;
use crate::services::unicode_service::{UnicodeService, UnicodeVersionProvider};

/// The variation selector-16.
const VS16: u32 = 0xFE0F;

/// Unicode 11 widths with VS16 promotion, registered under version `"11-vs16"`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnicodeV11Vs16 {
    base: UnicodeV11,
}

impl UnicodeV11Vs16 {
    /// The version name this provider registers under.
    pub const VERSION: &'static str = "11-vs16";

    /// A provider over a fresh [`UnicodeV11`].
    pub fn new() -> Self {
        Self::default()
    }
}

impl UnicodeVersionProvider for UnicodeV11Vs16 {
    fn version(&self) -> &str {
        Self::VERSION
    }

    fn wcwidth(&self, codepoint: u32) -> u32 {
        self.base.wcwidth(codepoint)
    }

    fn char_properties(&self, codepoint: u32, preceding: u32) -> u32 {
        let value = self.base.char_properties(codepoint, preceding);
        if codepoint == VS16
            && UnicodeService::extract_should_join(value)
            && UnicodeService::extract_width(value) == 1
        {
            return UnicodeService::create_property_value(
                UnicodeService::extract_char_kind(value),
                2,
                true,
            );
        }
        value
    }
}
