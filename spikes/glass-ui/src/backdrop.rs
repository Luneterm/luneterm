//! OS glass behind the window: the app's glass modes, mapped onto `lune_ui::material`.

use lune_ui::material::{self, Material};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassMode {
    /// Lunegit's material (NSVisualEffectMaterialLight), kept active when unfocused.
    Vibrancy,
    /// Exactly Lunegit: material follows window state; app shows solid #333333 when unfocused.
    VibrancyLunegit,
    LiquidClear,
    LiquidRegular,
    /// No OS glass: what Linux gets.
    Opaque,
}

impl GlassMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vibrancy => "Vibrancy (stays on when unfocused)",
            Self::VibrancyLunegit => "Vibrancy (Lunegit-exact)",
            Self::LiquidClear => "Liquid Glass · clear",
            Self::LiquidRegular => "Liquid Glass · regular",
            Self::Opaque => "Opaque (Linux fallback)",
        }
    }

    fn material(self) -> Material {
        match self {
            Self::Vibrancy => Material::Vibrancy,
            Self::VibrancyLunegit => Material::VibrancyFollowsWindow,
            Self::LiquidClear => Material::LiquidClear,
            Self::LiquidRegular => Material::LiquidRegular,
            Self::Opaque => Material::Opaque,
        }
    }
}

pub fn apply(window: &winit::window::Window, mode: GlassMode) -> Result<(), String> {
    material::apply(window, mode.material())
}
