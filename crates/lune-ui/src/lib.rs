//! App-agnostic glass UI toolkit: an immediate-mode painter on wgpu, widgets, OS backdrop
//! materials and window helpers, styled after Lunegit (see `docs/glass-styling.md`).
//!
//! lune-ui knows nothing about the app that uses it. The app owns the winit event loop and passes
//! in everything else: fonts, the [`theme::Theme`], hit values (the painter and widgets are
//! generic over the app's hit type), sizes, scale and time. The crate reads neither the clock nor
//! the environment, so frames are deterministic.
//!
//! Modules:
//! - [`color`], [`theme`] (the `Theme` tokens and the [`lunegit_dark`] preset with its glass
//!   layers), [`acrylic`] (Avalonia acrylic colour math);
//! - [`paint`] (the `Painter`), [`text`], [`icon`], [`widgets`] (controls, menus, dropdowns,
//!   dialogs), [`chrome`] (tab outline geometry);
//! - [`gpu`], [`blur`], [`frame`] (wgpu setup, backdrop blur, the scene → blur → target pass
//!   sequence), [`dump`] (offscreen targets and PNG readback);
//! - [`material`] (OS backdrop per platform, [`GlassLayer`] and [`Backdrop`] describe the layers
//!   and capability levels), [`window`] (glass window attributes, edge resize, double click,
//!   caption buttons).
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
pub mod material;
pub mod paint;
pub mod text;
pub mod theme;
pub mod widgets;
pub mod window;

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
