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
pub mod gpu;

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

/// Lunegit dark theme glass tokens (`Lunegit.Desktop/Themes/DarkTheme.axaml`).
pub mod lunegit_dark {
    use super::GlassLayer;
    use crate::color::Rgba;

    pub const TINT: Rgba = [0, 0, 0, 255];

    /// Base window layer (`GlassTintDarkOpacity` 1, `GlassMaterialDarkOpacity` 0.7).
    pub const WINDOW: GlassLayer = GlassLayer {
        tint: TINT,
        tint_opacity: 1.0,
        material_opacity: 0.7,
    };

    /// Panel layer (`GlassTintLightOpacity` 0.8, `GlassMaterialLightOpacity` 0.7).
    pub const PANEL: GlassLayer = GlassLayer {
        tint: TINT,
        tint_opacity: 0.8,
        material_opacity: 0.7,
    };

    /// Card and header-tint layer (tint 1, `GlassMaterialDarkHeaderOpacity` 0.8).
    pub const CARD: GlassLayer = GlassLayer {
        tint: TINT,
        tint_opacity: 1.0,
        material_opacity: 0.8,
    };

    /// Solid fill shown while the window is inactive (tint lightened 20%), replacing all
    /// acrylic layers. Lunegit works around AvaloniaUI/Avalonia#5440 this way.
    pub const INACTIVE_FALLBACK: Rgba = [0x33, 0x33, 0x33, 0xFF];

    pub const CARD_CORNER_RADIUS: f32 = 8.0;
    pub const TAB_HEADER_HEIGHT: f32 = 32.0;
    pub const BODY_PADDING: f32 = 10.0;
}
