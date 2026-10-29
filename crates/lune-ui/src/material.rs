//! OS backdrop material behind a transparent window.
//!
//! The effect is applied by the platform: `NSVisualEffectView` / `NSGlassEffectView` on macOS,
//! DWM system backdrops on Windows, nothing elsewhere. The platform code lives in `#[cfg]`
//! submodules; this file holds the request type and the shared entry point.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod other;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
use macos::apply as apply_os;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use other::apply as apply_os;
#[cfg(target_os = "windows")]
use windows::apply as apply_os;

/// The material requested for the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Light vibrancy (macOS) or acrylic (Windows), kept active when the window is unfocused.
    Vibrancy,
    /// Like [`Material::Vibrancy`], but the material follows the window's active state (macOS;
    /// acrylic on Windows).
    VibrancyFollowsWindow,
    /// Clear liquid glass (macOS) or Mica (Windows).
    LiquidClear,
    /// Regular liquid glass (macOS) or tabbed Mica (Windows).
    LiquidRegular,
    /// No OS material.
    Opaque,
}

impl Material {
    /// True for every material except [`Material::Opaque`].
    pub fn is_glass(self) -> bool {
        self != Self::Opaque
    }
}

/// Applies `material` to `window`, first clearing any material applied earlier.
///
/// `Opaque` is `Ok` everywhere. A glass material on a platform without OS glass is an `Err`.
pub fn apply(window: &winit::window::Window, material: Material) -> Result<(), String> {
    apply_os(window, material)
}
