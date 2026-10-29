//! Glass styling.
//!
//! Lunegit's look is two things stacked (see `docs/glass-styling.md`):
//! 1. an OS backdrop blur behind a transparent window ([`Backdrop`]: NSVisualEffectView on
//!    macOS, DWM acrylic/Mica on Windows, compositor blur where Linux offers one), and
//! 2. tint/material layers drawn by the app itself ([`GlassLayer`]), which the renderer
//!    composites over the backdrop.
//!
//! Values in [`lunegit_dark`] are copied from Lunegit's `DarkTheme.axaml` so the two apps match.

pub mod acrylic;
pub mod blur;
pub mod chrome;
pub mod color;
pub mod dump;
pub mod frame;
pub mod gpu;
pub mod icon;
pub mod paint;
pub mod text;
pub mod theme;
pub mod widgets;

pub use theme::lunegit_dark;

use color::Rgba;

/// One app-drawn acrylic layer, in Avalonia `ExperimentalAcrylicMaterial` terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassLayer {
    pub tint: Rgba,
    pub tint_opacity: f32,
    pub material_opacity: f32,
}

/// OS backdrop material requested for the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropKind {
    /// Strongest blur the platform offers (macOS vibrancy, Windows acrylic, KDE blur).
    Blur,
    /// Transparent window without blur (Linux compositors without a blur protocol).
    Transparent,
    /// Opaque fallback (no compositor, or the user turned glass off).
    Opaque,
}

/// Applies an OS backdrop to a native window. One implementation per platform.
pub trait Backdrop {
    /// Best kind this platform/session supports; callers fall back down
    /// `Blur → Transparent → Opaque` like Lunegit's `TransparencyLevelHint`.
    fn best_supported(&self) -> BackdropKind;
}
