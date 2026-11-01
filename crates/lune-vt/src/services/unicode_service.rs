//! Port of xterm.js src/common/services/UnicodeService.ts @ f447274, with the
//! `IUnicodeService` / `IUnicodeVersionProvider` interfaces from `Services.ts:347-375`
//! (tests: UnicodeService.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1):
//!  - `onChange` becomes a return value: [`UnicodeService::set_active_version`] returns the
//!    [`UnicodeChange`] that upstream fires, exactly when upstream fires it. The `activeVersion`
//!    setter is that method; the `throw` is [`UnicodeError`].
//!  - `UnicodeCharWidth` (`0 | 1 | 2`) and `UnicodeCharProperties` (a JS number) are `u32`.
//!  - Providers are trait objects ([`UnicodeVersionProvider`]). The trait requires `Send + Sync`
//!    so a terminal owning the service can move between threads.
//!  - `versions` follows `Object.keys` order on upstream's `Object.create(null)` map: keys that
//!    are canonical array indices ("6", "11") first in ascending numeric order, then the other
//!    keys in registration order. Registering an existing version replaces its provider in place.
//!  - `register` over the version that is already active does not change the active provider
//!    (upstream keeps `_activeProvider` until `activeVersion` is set again); this port does the
//!    same.
//!  - `getStringCellWidth` walks `chars()` where upstream walks UTF-16 code units: a `&str`
//!    cannot contain a lone surrogate, so upstream's lone-surrogate branches never apply and each
//!    surrogate pair is one `char`. The sum is computed in `i64` (upstream subtracts the
//!    preceding width for a joining character, which a custom provider can make negative) and a
//!    negative total is returned as 0; the built-in providers never produce one.

use std::fmt;
use std::sync::Arc;

use crate::input::unicode_v6::UnicodeV6;

/// A Unicode version's width and grapheme-join rules (`IUnicodeVersionProvider`).
pub trait UnicodeVersionProvider: Send + Sync {
    /// The version name this provider is registered under, e.g. `"6"` or `"11"`.
    fn version(&self) -> &str;
    /// Cell width of `codepoint`: 0, 1 or 2 for the built-in providers.
    fn wcwidth(&self, codepoint: u32) -> u32;
    /// Width and join state of `codepoint`, packed as
    /// [`UnicodeService::create_property_value`]. `preceding` is the previous character's
    /// result (0 if none); the result says whether the two characters should be joined.
    fn char_properties(&self, codepoint: u32, preceding: u32) -> u32;
}

/// What `onChange` fires when the active version changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnicodeChange {
    /// The newly active version.
    pub version: String,
}

/// Upstream throws `unknown Unicode version "<v>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnicodeError {
    /// No provider is registered under this version.
    UnknownVersion(String),
}

impl fmt::Display for UnicodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnicodeError::UnknownVersion(version) => {
                write!(f, "unknown Unicode version \"{version}\"")
            }
        }
    }
}

impl std::error::Error for UnicodeError {}

/// Registered Unicode version providers and the active one.
pub struct UnicodeService {
    /// Registration order; `versions()` derives the `Object.keys` order from it.
    providers: Vec<(String, Arc<dyn UnicodeVersionProvider>)>,
    active: String,
    active_provider: Arc<dyn UnicodeVersionProvider>,
}

impl fmt::Debug for UnicodeService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnicodeService")
            .field("versions", &self.versions())
            .field("active", &self.active)
            .finish()
    }
}

impl Default for UnicodeService {
    fn default() -> Self {
        Self::new()
    }
}

/// `true` for a canonical array-index key: decimal digits, no leading zero (except "0"), and
/// below 2^32 - 1. `Object.keys` lists these first, in ascending numeric order.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if key.len() > 1 && key.starts_with('0') {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

impl UnicodeService {
    /// Whether `value`'s join bit is set (`extractShouldJoin`).
    pub fn extract_should_join(value: u32) -> bool {
        (value & 1) != 0
    }

    /// The cell width stored in `value` (`extractWidth`).
    pub fn extract_width(value: u32) -> u32 {
        (value >> 1) & 0x3
    }

    /// The character kind (grapheme state) stored in `value` (`extractCharKind`).
    pub fn extract_char_kind(value: u32) -> u32 {
        value >> 3
    }

    /// Pack `state` (24 bits), `width` (2 bits) and the join flag (`createPropertyValue`).
    pub fn create_property_value(state: u32, width: u32, should_join: bool) -> u32 {
        ((state & 0xff_ffff) << 3) | ((width & 3) << 1) | u32::from(should_join)
    }

    /// A service with the default V6 provider registered and active.
    pub fn new() -> Self {
        let default_provider: Arc<dyn UnicodeVersionProvider> = Arc::new(UnicodeV6::new());
        let version = default_provider.version().to_owned();
        Self {
            providers: vec![(version.clone(), Arc::clone(&default_provider))],
            active: version,
            active_provider: default_provider,
        }
    }

    /// Registered versions, in `Object.keys` order (see the module notes).
    pub fn versions(&self) -> Vec<&str> {
        let mut indexed: Vec<(u32, &str)> = Vec::new();
        let mut named: Vec<&str> = Vec::new();
        for (version, _) in &self.providers {
            match array_index(version) {
                Some(index) => indexed.push((index, version.as_str())),
                None => named.push(version.as_str()),
            }
        }
        indexed.sort_by_key(|(index, _)| *index);
        indexed
            .into_iter()
            .map(|(_, version)| version)
            .chain(named)
            .collect()
    }

    /// The currently active version.
    pub fn active_version(&self) -> &str {
        &self.active
    }

    /// Activate a registered version (the `activeVersion` setter). `Ok` carries what upstream's
    /// `onChange` fires; `Err` is where upstream throws, leaving the active version unchanged.
    pub fn set_active_version(&mut self, version: &str) -> Result<UnicodeChange, UnicodeError> {
        let provider = self
            .providers
            .iter()
            .find(|(key, _)| key == version)
            .map(|(_, provider)| Arc::clone(provider))
            .ok_or_else(|| UnicodeError::UnknownVersion(version.to_owned()))?;
        self.active = version.to_owned();
        self.active_provider = provider;
        Ok(UnicodeChange {
            version: version.to_owned(),
        })
    }

    /// Register a provider under its [`version`](UnicodeVersionProvider::version), replacing
    /// any provider already registered for it.
    pub fn register(&mut self, provider: Box<dyn UnicodeVersionProvider>) {
        let provider: Arc<dyn UnicodeVersionProvider> = Arc::from(provider);
        let version = provider.version().to_owned();
        match self.providers.iter_mut().find(|(key, _)| *key == version) {
            Some(entry) => entry.1 = provider,
            None => self.providers.push((version, provider)),
        }
    }

    /// Cell width of `codepoint` under the active provider.
    pub fn wcwidth(&self, codepoint: u32) -> u32 {
        self.active_provider.wcwidth(codepoint)
    }

    /// Total cell width of `s` under the active provider, joining zero-width characters to the
    /// character before them as the provider says.
    pub fn get_string_cell_width(&self, s: &str) -> u32 {
        let mut result: i64 = 0;
        let mut preceding_info: u32 = 0;
        for ch in s.chars() {
            let current_info = self.char_properties(u32::from(ch), preceding_info);
            let mut ch_width = i64::from(Self::extract_width(current_info));
            if Self::extract_should_join(current_info) {
                ch_width -= i64::from(Self::extract_width(preceding_info));
            }
            result += ch_width;
            preceding_info = current_info;
        }
        u32::try_from(result.max(0)).unwrap_or(u32::MAX)
    }

    /// Width and join state of `codepoint` given the `preceding` character's result.
    pub fn char_properties(&self, codepoint: u32, preceding: u32) -> u32 {
        self.active_provider.char_properties(codepoint, preceding)
    }
}

#[cfg(test)]
#[path = "unicode_service_tests.rs"]
mod tests;
