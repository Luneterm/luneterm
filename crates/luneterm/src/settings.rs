//! The settings model. It mirrors Lunegit's `TerminalSettings` and keeps its shipped defaults; it
//! is the spike's `Settings` (`spikes/glass-ui/src/state.rs`) without the spike's UI plumbing and
//! without serde (a settings file is M10's). M9 reads only what a running session needs.

use lune_ui::material::Material;
use lune_ui::motion::MotionTier;

use crate::session::MAX_SCROLLBACK;

/// Font size bounds and the default (Lunegit `JsonUserSettingsStore.cs:465,478`).
pub const MIN_FONT_SIZE: u32 = 8;
pub const MAX_FONT_SIZE: u32 = 32;
pub const DEFAULT_FONT_SIZE: u32 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassStyle {
    Lunegit,
    LiquidClear,
    LiquidRegular,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Full,
    Reduced,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlurLevel {
    Low,
    Medium,
    High,
}

impl BlurLevel {
    /// Gaussian sigma in logical px. Medium is the value approved on 2026-10-29.
    pub fn sigma(self) -> f32 {
        match self {
            Self::Low => 4.0,
            Self::Medium => 8.0,
            Self::High => 14.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineAnim {
    Sweep,
    Comet,
    Breath,
}

/// Defaults follow Lunegit's shipped defaults (`TerminalSettings.Default`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub glass: GlassStyle,
    pub keep_glass_inactive: bool,
    pub menu_blur: BlurLevel,
    pub motion: Motion,
    pub font: usize,
    pub font_size: u32,
    pub palette: usize,
    pub animate_outline: bool,
    pub outline: OutlineAnim,
    pub outline_full_screen: bool,
    pub scrollback: u32,
    pub default_cwd: String,
    pub shell: String,
    pub shell_args: String,
    pub report_iterm2: bool,
    pub confirm_close: bool,
    pub osc_clipboard: bool,
    pub option_as_meta: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            glass: GlassStyle::Lunegit,
            keep_glass_inactive: false,
            menu_blur: BlurLevel::Medium,
            motion: Motion::Full,
            font: 0,
            font_size: DEFAULT_FONT_SIZE,
            palette: 0,
            animate_outline: true,
            outline: OutlineAnim::Sweep,
            outline_full_screen: false,
            scrollback: 5000,
            default_cwd: String::new(),
            shell: String::new(),
            shell_args: String::new(),
            report_iterm2: false,
            confirm_close: true,
            osc_clipboard: true,
            option_as_meta: false,
        }
    }
}

impl Settings {
    /// The settings with their numbers inside what the app supports: scrollback at most
    /// [`MAX_SCROLLBACK`] lines, a font size outside 8..=32 reset to 13 (Lunegit resets rather than
    /// clamps).
    #[must_use]
    pub fn clamped(mut self) -> Settings {
        let cap = u32::try_from(MAX_SCROLLBACK).unwrap_or(u32::MAX);
        self.scrollback = self.scrollback.min(cap);
        if !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&self.font_size) {
            self.font_size = DEFAULT_FONT_SIZE;
        }
        self
    }

    /// The OS material the glass style asks for. The toolkit's glass does not depend on the
    /// window's active state, so Lunegit's "material follows the window" has no solid fallback to
    /// go with it here and maps to the same material as "keep glass when inactive".
    pub fn material(&self) -> Material {
        match self.glass {
            GlassStyle::Lunegit => Material::Vibrancy,
            GlassStyle::LiquidClear => Material::LiquidClear,
            GlassStyle::LiquidRegular => Material::LiquidRegular,
            GlassStyle::Off => Material::Opaque,
        }
    }

    /// The motion tier the toolkit plays.
    pub fn tier(&self) -> MotionTier {
        match self.motion {
            Motion::Full => MotionTier::Full,
            Motion::Reduced => MotionTier::Reduced,
            Motion::None => MotionTier::Off,
        }
    }

    /// Lunegit's `TerminalOutlineAnimationResolver`: the app-wide motion level wins.
    pub fn effective_outline(&self) -> Option<OutlineAnim> {
        if !self.animate_outline {
            return None;
        }
        match self.motion {
            Motion::Full => Some(self.outline),
            Motion::Reduced => Some(OutlineAnim::Breath),
            Motion::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_lunegits() {
        let s = Settings::default();
        assert_eq!(s.glass, GlassStyle::Lunegit);
        assert_eq!((s.font_size, s.scrollback), (13, 5000));
        assert!(s.osc_clipboard && s.confirm_close);
        assert!(!s.report_iterm2 && !s.option_as_meta && !s.keep_glass_inactive);
        assert_eq!(s.clone().clamped(), s);
    }

    #[test]
    fn clamped_caps_scrollback_and_resets_a_font_size_outside_8_to_32() {
        let at = |scrollback, font_size| {
            Settings {
                scrollback,
                font_size,
                ..Settings::default()
            }
            .clamped()
        };
        assert_eq!(at(0, 8).scrollback, 0);
        assert_eq!(at(100_000, 8).scrollback, 100_000);
        assert_eq!(at(100_001, 8).scrollback, 100_000);
        assert_eq!(at(u32::MAX, 8).scrollback, 100_000);
        for ok in [8, 13, 32] {
            assert_eq!(at(0, ok).font_size, ok, "size {ok} stays");
        }
        for bad in [0, 7, 33, 1000, u32::MAX] {
            assert_eq!(at(0, bad).font_size, 13, "size {bad} resets");
        }
        // The other fields are left alone.
        let s = Settings {
            osc_clipboard: false,
            shell: "zsh".into(),
            ..Settings::default()
        };
        assert_eq!(s.clone().clamped(), s);
    }

    #[test]
    fn glass_style_and_motion_map_to_the_toolkit() {
        let with = |glass| Settings {
            glass,
            ..Settings::default()
        };
        assert_eq!(with(GlassStyle::Lunegit).material(), Material::Vibrancy);
        assert_eq!(with(GlassStyle::Off).material(), Material::Opaque);
        assert_eq!(
            with(GlassStyle::LiquidClear).material(),
            Material::LiquidClear
        );
        let motion = |motion| Settings {
            motion,
            ..Settings::default()
        };
        assert_eq!(motion(Motion::Reduced).tier(), MotionTier::Reduced);
        assert_eq!(motion(Motion::None).tier(), MotionTier::Off);
        assert_eq!(
            motion(Motion::Reduced).effective_outline(),
            Some(OutlineAnim::Breath)
        );
        assert_eq!(motion(Motion::None).effective_outline(), None);
    }
}
