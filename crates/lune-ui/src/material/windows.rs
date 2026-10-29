//! Windows 11 DWM system backdrops. Avalonia builds its acrylic from the WinUI composition recipe
//! (host backdrop → blur → saturation → luminosity/tint); DWM's acrylic is the system version of
//! the same recipe.

use window_vibrancy::{
    apply_acrylic, apply_mica, apply_tabbed, clear_acrylic, clear_mica, clear_tabbed,
};

use super::Material;

pub(super) fn apply(window: &winit::window::Window, material: Material) -> Result<(), String> {
    // Clearing a backdrop that was never applied is harmless.
    let _ = clear_acrylic(window);
    let _ = clear_mica(window);
    let _ = clear_tabbed(window);
    let result = match material {
        Material::Vibrancy | Material::VibrancyFollowsWindow => apply_acrylic(window, None),
        Material::LiquidClear => apply_mica(window, Some(true)),
        Material::LiquidRegular => apply_tabbed(window, Some(true)),
        Material::Opaque => Ok(()),
    };
    result.map_err(|e| e.to_string())
}
