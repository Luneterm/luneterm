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

/// The glass behind the window. The persisted name of the default is `default` (plan M10 D6), so
/// a later change of what the default looks like (the dark preset, step 13) changes a mapping and
/// not the file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassStyle {
    Default,
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

/// Enums that are saved by name, never by position: reordering or adding a variant cannot remap a
/// saved value.
macro_rules! named {
    ($t:ident { $($v:ident => $name:literal),+ $(,)? }) => {
        impl $t {
            /// Every variant, in the order a settings page lists them.
            pub const ALL: &'static [$t] = &[$($t::$v),+];

            /// The name the file stores.
            pub fn name(self) -> &'static str {
                match self { $($t::$v => $name),+ }
            }

            /// The variant of a stored name, `None` for any other text.
            pub fn from_name(name: &str) -> Option<$t> {
                match name { $($name => Some($t::$v),)+ _ => None }
            }
        }
    };
}

named!(GlassStyle {
    Default => "default",
    LiquidClear => "liquid-clear",
    LiquidRegular => "liquid-regular",
    Off => "off",
});
named!(Motion {
    Full => "full",
    Reduced => "reduced",
    None => "none",
});
named!(BlurLevel {
    Low => "low",
    Medium => "medium",
    High => "high",
});
named!(OutlineAnim {
    Sweep => "sweep",
    Comet => "comet",
    Breath => "breath",
});

/// Longest saved font or palette name, in bytes.
pub const MAX_NAME_BYTES: usize = 64;
/// Longest saved shell program and its arguments, in bytes.
pub const MAX_SHELL_BYTES: usize = 1024;

/// The palette used until one is chosen.
pub const DEFAULT_PALETTE: &str = "One Dark";

/// Defaults follow Lunegit's shipped defaults (`TerminalSettings.Default`).
///
/// `font` and `palette` are names, never positions in a list (D5). `font` is empty until a font is
/// chosen: the app's own terminal font then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub glass: GlassStyle,
    pub menu_blur: BlurLevel,
    pub motion: Motion,
    pub font: String,
    pub font_size: u32,
    pub palette: String,
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
            glass: GlassStyle::Default,
            menu_blur: BlurLevel::Medium,
            motion: Motion::Full,
            font: String::new(),
            font_size: DEFAULT_FONT_SIZE,
            palette: DEFAULT_PALETTE.to_owned(),
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

/// Text a setting may hold: at most `max` bytes and no control character.
fn text_ok(s: &str, max: usize) -> bool {
    s.len() <= max && !s.chars().any(char::is_control)
}

impl Settings {
    /// The settings with every field inside what the app supports: scrollback at most
    /// [`MAX_SCROLLBACK`] lines, a font size outside 8..=32 reset to 13 (Lunegit resets rather than
    /// clamps), and text fields (names, shell, directory) of the allowed length with no control
    /// character, else reset to their default. Whatever read the settings (a file, a page) passes
    /// through here.
    #[must_use]
    pub fn clamped(mut self) -> Settings {
        let defaults = Settings::default();
        let cap = u32::try_from(MAX_SCROLLBACK).unwrap_or(u32::MAX);
        self.scrollback = self.scrollback.min(cap);
        if !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&self.font_size) {
            self.font_size = DEFAULT_FONT_SIZE;
        }
        let fit = |field: &mut String, max: usize, default: &str| {
            if !text_ok(field, max) {
                default.clone_into(field);
            }
        };
        fit(&mut self.font, MAX_NAME_BYTES, &defaults.font);
        fit(&mut self.palette, MAX_NAME_BYTES, &defaults.palette);
        if self.palette.is_empty() {
            self.palette = defaults.palette;
        }
        fit(&mut self.shell, MAX_SHELL_BYTES, "");
        fit(&mut self.shell_args, MAX_SHELL_BYTES, "");
        fit(&mut self.default_cwd, crate::cwd::MAX_CWD_BYTES, "");
        self
    }

    /// The OS material the glass style asks for. The toolkit's glass does not depend on the
    /// window's active state, so Lunegit's "material follows the window" has no solid fallback to
    /// go with it here (and `keep_glass_inactive` is gone: the glass is always active, D8).
    pub fn material(&self) -> Material {
        match self.glass {
            GlassStyle::Default => Material::Vibrancy,
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
        assert_eq!(s.glass, GlassStyle::Default);
        assert_eq!((s.font_size, s.scrollback), (13, 5000));
        assert!(s.osc_clipboard && s.confirm_close);
        assert!(!s.report_iterm2 && !s.option_as_meta);
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
        assert_eq!(with(GlassStyle::Default).material(), Material::Vibrancy);
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

    #[test]
    fn enum_names_round_trip_and_are_stable() {
        for (&g, name) in
            GlassStyle::ALL
                .iter()
                .zip(["default", "liquid-clear", "liquid-regular", "off"])
        {
            assert_eq!(g.name(), name);
            assert_eq!(GlassStyle::from_name(name), Some(g));
        }
        for m in Motion::ALL {
            assert_eq!(Motion::from_name(m.name()), Some(*m));
        }
        for b in BlurLevel::ALL {
            assert_eq!(BlurLevel::from_name(b.name()), Some(*b));
        }
        for o in OutlineAnim::ALL {
            assert_eq!(OutlineAnim::from_name(o.name()), Some(*o));
        }
        // Text that is not a name is no variant, whatever its case or padding.
        for bad in ["", "Default", " default", "default ", "0", "liquid_clear"] {
            assert_eq!(GlassStyle::from_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn clamped_resets_text_that_is_too_long_or_has_a_control_character() {
        let ok = Settings {
            font: "x".repeat(MAX_NAME_BYTES),
            shell: "y".repeat(MAX_SHELL_BYTES),
            ..Settings::default()
        };
        assert_eq!(ok.clone().clamped(), ok);
        let bad = Settings {
            font: "x".repeat(MAX_NAME_BYTES + 1),
            palette: "Nord\n".into(),
            shell: "a\u{1b}[31m".into(),
            shell_args: "z".repeat(MAX_SHELL_BYTES + 1),
            default_cwd: "/tmp\0x".into(),
            ..Settings::default()
        }
        .clamped();
        assert_eq!(bad, Settings::default());
        // An empty palette name is the default palette.
        let empty = Settings {
            palette: String::new(),
            ..Settings::default()
        }
        .clamped();
        assert_eq!(empty.palette, DEFAULT_PALETTE);
    }
}
