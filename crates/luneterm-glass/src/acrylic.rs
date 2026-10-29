//! Avalonia `ExperimentalAcrylicMaterial` colour math, ported so Luneterm's glass layers produce the
//! same pixels as Lunegit's.
//!
//! Source: Avalonia 12.1.2 `src/Avalonia.Base/Media/ExperimentalAcrylicMaterial.cs`
//! (`GetEffectiveTintColor`, `GetTintOpacityModifier`, `GetLuminosityColor`) and
//! `src/Skia/Avalonia.Skia/DrawingContextImpl.cs` (`CreateAcrylicPaint`). MIT, see
//! `licenses/avalonia-LICENSE`.
//!
//! Skia paints an acrylic layer as `noise OVER (tint OVER luminosity)` with blend mode `Src`
//! (for `BackgroundSource=Digger`), so each layer *replaces* what is under it rather than stacking.

use crate::{GlassLayer, Rgba};

/// Opacity Avalonia applies to its 256×256 noise texture.
pub const NOISE_OPACITY: f32 = 0.0225;

/// The two solid colours Avalonia composes for one acrylic layer (before noise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcrylicColors {
    /// Tint with its suppressed alpha (`GetEffectiveTintColor`).
    pub tint: Rgba,
    /// Grey luminosity backdrop (`GetLuminosityColor`).
    pub luminosity: Rgba,
}

impl GlassLayer {
    /// Avalonia's effective tint and luminosity colours for this layer
    /// (`PlatformTransparencyCompensationLevel` = 0, Avalonia's default).
    pub fn acrylic_colors(&self) -> AcrylicColors {
        let Rgba(r, g, b, a) = self.tint;
        let modifier = tint_opacity_modifier(self.tint);
        let tint_alpha =
            (255.0 * ((255.0 / f64::from(a)) * f64::from(self.tint_opacity)) * modifier) as u8;

        let max = f64::from(r.max(g).max(b)) / 255.0;
        let min = f64::from(r.min(g).min(b)) / 255.0;
        let material = f64::from(self.material_opacity);
        let mut lightness = (max + min) / 2.0;
        lightness = 1.0 - ((1.0 - lightness) * material);
        lightness = 0.13 + lightness * 0.74;
        let l = trim(lightness);
        let lum_alpha = (255.0 * material.clamp(0.0, 1.0)) as u8;

        AcrylicColors {
            tint: Rgba(r, g, b, tint_alpha),
            luminosity: Rgba(l, l, l, lum_alpha),
        }
    }

    /// `tint OVER luminosity`, premultiplied, channels in 0..=1. Noise is added by the renderer.
    pub fn premultiplied(&self) -> [f32; 4] {
        let c = self.acrylic_colors();
        let (t, l) = (to_f32(c.tint), to_f32(c.luminosity));
        let inv = 1.0 - t[3];
        [
            t[0] * t[3] + l[0] * l[3] * inv,
            t[1] * t[3] + l[1] * l[3] * inv,
            t[2] * t[3] + l[2] * l[3] * inv,
            t[3] + l[3] * inv,
        ]
    }
}

fn to_f32(Rgba(r, g, b, a): Rgba) -> [f32; 4] {
    [r, g, b, a].map(|v| f32::from(v) / 255.0)
}

fn trim(value: f64) -> u8 {
    (value * 256.0).floor().clamp(0.0, 255.0) as u8
}

fn tint_opacity_modifier(Rgba(r, g, b, _): Rgba) -> f64 {
    const MID_POINT: f64 = 0.5;
    const WHITE_MAX_OPACITY: f64 = 0.2;
    const MID_POINT_MAX_OPACITY: f64 = 0.45;
    const BLACK_MAX_OPACITY: f64 = 0.45;

    let max = f64::from(r.max(g).max(b)) / 255.0;
    let min = f64::from(r.min(g).min(b)) / 255.0;
    let (v, s) = (max, if max == 0.0 { 0.0 } else { (max - min) / max });

    if v == MID_POINT {
        return MID_POINT_MAX_OPACITY;
    }
    let (lowest, max_deviation) = if v > MID_POINT {
        (WHITE_MAX_OPACITY, 1.0 - MID_POINT)
    } else {
        (BLACK_MAX_OPACITY, MID_POINT)
    };
    let mut suppression = MID_POINT_MAX_OPACITY - lowest;
    let normalized = (v - MID_POINT).abs() / max_deviation;
    if s > 0.0 {
        suppression *= (1.0 - s * 2.0).max(0.0);
    }
    MID_POINT_MAX_OPACITY - suppression * normalized
}

#[cfg(test)]
mod tests {
    use crate::lunegit_dark::{CARD, PANEL, WINDOW};
    use crate::{GlassLayer, Rgba};

    // Expected values computed independently from Avalonia 12.1.2's formulas.

    #[test]
    fn window_layer_matches_avalonia() {
        let c = WINDOW.acrylic_colors();
        assert_eq!(c.tint, Rgba(0, 0, 0, 114));
        assert_eq!(c.luminosity, Rgba(90, 90, 90, 178));
    }

    #[test]
    fn panel_layer_matches_avalonia() {
        let c = PANEL.acrylic_colors();
        assert_eq!(c.tint, Rgba(0, 0, 0, 91));
        assert_eq!(c.luminosity, Rgba(90, 90, 90, 178));
    }

    #[test]
    fn card_layer_matches_avalonia() {
        let c = CARD.acrylic_colors();
        assert_eq!(c.tint, Rgba(0, 0, 0, 114));
        assert_eq!(c.luminosity, Rgba(71, 71, 71, 204));
        let p = CARD.premultiplied();
        assert!((p[3] - 0.8894).abs() < 1e-3, "alpha {}", p[3]);
        assert!((p[0] - 0.1232).abs() < 1e-3, "red {}", p[0]);
    }

    #[test]
    fn white_tint_is_suppressed_to_twenty_percent() {
        let white = GlassLayer {
            tint: Rgba(255, 255, 255, 255),
            tint_opacity: 1.0,
            material_opacity: 0.0,
        };
        assert_eq!(white.acrylic_colors().tint.3, 51);
    }
}
