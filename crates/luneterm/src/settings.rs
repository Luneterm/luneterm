//! The settings model. It mirrors Lunegit's `TerminalSettings` and keeps its shipped defaults; it
//! began as the glass UI spike's `Settings` (the spike was retired in M10 step 12) without its UI
//! plumbing and without serde (the file reader and writer are `persist`'s). Every field has a [`SettingId`] and
//! an [`Effect`] (plan M10 step 6): what changing it does, live or at the next spawn.

use lune_ui::material::Material;
use lune_ui::motion::MotionTier;
use lune_ui::near_black;
use lune_ui::text::TextRendering;
use lune_ui::widgets::tab_strip::BusyCue;

use crate::session::MAX_SCROLLBACK;

/// Font size bounds (Lunegit `JsonUserSettingsStore.cs:465,478`) and the default: 14 at the user's
/// request (Lunegit's is 13).
pub const MIN_FONT_SIZE: u32 = 8;
pub const MAX_FONT_SIZE: u32 = 32;
pub const DEFAULT_FONT_SIZE: u32 = 14;

/// The look: which layers and header tint the window is drawn with (Lunegit's `lunegit_dark` ones
/// or the near-black ones), under every [`GlassStyle`]. The persisted names are `luneterm` (the
/// default) and `lunegit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// The near-black glass, the look since M10 step 13.
    Luneterm,
    /// Lunegit's own look: the layers and the light vibrancy the app had before the near-black one.
    Lunegit,
}

/// The glass behind the window. The persisted name of the default is `default` (plan M10 D6), so
/// the change of what the default looks like (Lunegit's light vibrancy to the near-black look, step
/// 13) was a mapping and not a file format.
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

named!(Theme {
    Luneterm => "luneterm",
    Lunegit => "lunegit",
});
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
    pub theme: Theme,
    pub glass: GlassStyle,
    pub menu_blur: BlurLevel,
    pub motion: Motion,
    pub font: String,
    pub font_size: u32,
    /// Smoothed glyphs (the toolkit's `TextRendering::SMOOTH`, luneui-rs D296) in the UI text and
    /// the terminal grid. Off: the hinted look of before (`TextRendering::OFF`). A Luneterm-only
    /// setting: Lunegit has none.
    pub font_smoothing: bool,
    pub palette: String,
    pub animate_outline: bool,
    pub outline: OutlineAnim,
    pub outline_full_screen: bool,
    pub scrollback: u32,
    pub default_cwd: String,
    pub shell: String,
    pub shell_args: String,
    pub report_iterm2: bool,
    /// Install the shell-integration scripts and start the shell through them (OSC 7 and OSC 133
    /// marks). Off: the shell starts exactly as without them.
    pub shell_integration: bool,
    pub confirm_close: bool,
    pub osc_clipboard: bool,
    pub option_as_meta: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Luneterm,
            glass: GlassStyle::Default,
            menu_blur: BlurLevel::Medium,
            motion: Motion::Full,
            font: String::new(),
            font_size: DEFAULT_FONT_SIZE,
            font_smoothing: true,
            palette: DEFAULT_PALETTE.to_owned(),
            animate_outline: true,
            outline: OutlineAnim::Sweep,
            outline_full_screen: false,
            scrollback: 5000,
            default_cwd: String::new(),
            shell: String::new(),
            shell_args: String::new(),
            report_iterm2: false,
            shell_integration: true,
            confirm_close: true,
            osc_clipboard: true,
            option_as_meta: false,
        }
    }
}

/// Text a setting may hold: at most `max` bytes and no control character.
pub fn text_ok(s: &str, max: usize) -> bool {
    s.len() <= max && !s.chars().any(char::is_control)
}

impl Settings {
    /// The settings with every field inside what the app supports: scrollback at most
    /// [`MAX_SCROLLBACK`] lines, a font size outside 8..=32 reset to 14 (Lunegit resets rather than
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

    /// The OS material the glass style asks for; the default style takes the theme's: the
    /// near-black material for Luneterm, Lunegit's light vibrancy for Lunegit. The toolkit's glass
    /// does not depend on the window's active state, so Lunegit's "material follows the window" has
    /// no solid fallback to go with it here (and `keep_glass_inactive` is gone: the glass is always
    /// active, D8).
    pub fn material(&self) -> Material {
        match self.glass {
            GlassStyle::Default => match self.theme {
                Theme::Luneterm => near_black::MATERIAL,
                Theme::Lunegit => Material::Vibrancy,
            },
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

    /// How the toolkit and the terminal grid rasterize glyphs.
    pub fn text_rendering(&self) -> TextRendering {
        if self.font_smoothing {
            TextRendering::SMOOTH
        } else {
            TextRendering::OFF
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

    /// The toolkit's outline cue for the running-command outline, `None` while the setting (or
    /// the motion level) keeps the outline static.
    pub fn busy_cue(&self) -> Option<BusyCue> {
        self.effective_outline().map(|o| match o {
            OutlineAnim::Sweep => BusyCue::Sweep,
            OutlineAnim::Comet => BusyCue::Comet,
            OutlineAnim::Breath => BusyCue::Breath,
        })
    }

    /// The fields that differ between `self` and `other`, in the order of [`SettingId::ALL`].
    /// The destructuring is exhaustive: a field added to [`Settings`] without an id does not
    /// compile.
    pub fn changed(&self, other: &Settings) -> Vec<SettingId> {
        let Settings {
            theme,
            glass,
            menu_blur,
            motion,
            font,
            font_size,
            font_smoothing,
            palette,
            animate_outline,
            outline,
            outline_full_screen,
            scrollback,
            default_cwd,
            shell,
            shell_args,
            report_iterm2,
            shell_integration,
            confirm_close,
            osc_clipboard,
            option_as_meta,
        } = self;
        let flags = [
            (SettingId::Theme, *theme != other.theme),
            (SettingId::Glass, *glass != other.glass),
            (SettingId::MenuBlur, *menu_blur != other.menu_blur),
            (SettingId::Motion, *motion != other.motion),
            (SettingId::Font, *font != other.font),
            (SettingId::FontSize, *font_size != other.font_size),
            (
                SettingId::FontSmoothing,
                *font_smoothing != other.font_smoothing,
            ),
            (SettingId::Palette, *palette != other.palette),
            (
                SettingId::AnimateOutline,
                *animate_outline != other.animate_outline,
            ),
            (SettingId::Outline, *outline != other.outline),
            (
                SettingId::OutlineFullScreen,
                *outline_full_screen != other.outline_full_screen,
            ),
            (SettingId::Scrollback, *scrollback != other.scrollback),
            (SettingId::DefaultCwd, *default_cwd != other.default_cwd),
            (SettingId::Shell, *shell != other.shell),
            (SettingId::ShellArgs, *shell_args != other.shell_args),
            (
                SettingId::ReportIterm2,
                *report_iterm2 != other.report_iterm2,
            ),
            (
                SettingId::ShellIntegration,
                *shell_integration != other.shell_integration,
            ),
            (
                SettingId::ConfirmClose,
                *confirm_close != other.confirm_close,
            ),
            (
                SettingId::OscClipboard,
                *osc_clipboard != other.osc_clipboard,
            ),
            (
                SettingId::OptionAsMeta,
                *option_as_meta != other.option_as_meta,
            ),
        ];
        flags
            .into_iter()
            .filter_map(|(id, differs)| differs.then_some(id))
            .collect()
    }
}

/// One setting of the page: a [`Settings`] field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingId {
    Theme,
    Glass,
    MenuBlur,
    Motion,
    Font,
    FontSize,
    FontSmoothing,
    Palette,
    AnimateOutline,
    Outline,
    OutlineFullScreen,
    Scrollback,
    DefaultCwd,
    Shell,
    ShellArgs,
    ReportIterm2,
    ShellIntegration,
    ConfirmClose,
    OscClipboard,
    OptionAsMeta,
}

impl SettingId {
    /// Every setting, in the order of the [`Settings`] fields.
    pub const ALL: [SettingId; 20] = [
        SettingId::Theme,
        SettingId::Glass,
        SettingId::MenuBlur,
        SettingId::Motion,
        SettingId::Font,
        SettingId::FontSize,
        SettingId::FontSmoothing,
        SettingId::Palette,
        SettingId::AnimateOutline,
        SettingId::Outline,
        SettingId::OutlineFullScreen,
        SettingId::Scrollback,
        SettingId::DefaultCwd,
        SettingId::Shell,
        SettingId::ShellArgs,
        SettingId::ReportIterm2,
        SettingId::ShellIntegration,
        SettingId::ConfirmClose,
        SettingId::OscClipboard,
        SettingId::OptionAsMeta,
    ];

    /// What changing this setting does (plan M10 step 6's table).
    pub fn effect(self) -> Effect {
        match self {
            SettingId::Theme | SettingId::Glass => Effect::Glass,
            SettingId::MenuBlur => Effect::MenuBlur,
            SettingId::Motion => Effect::Motion,
            SettingId::Font | SettingId::FontSize => Effect::Font,
            SettingId::FontSmoothing => Effect::TextRendering,
            SettingId::Palette => Effect::Palette,
            SettingId::AnimateOutline | SettingId::Outline | SettingId::OutlineFullScreen => {
                Effect::Outline
            }
            SettingId::Scrollback => Effect::Scrollback,
            SettingId::OscClipboard => Effect::OscClipboard,
            SettingId::OptionAsMeta => Effect::OptionAsMeta,
            SettingId::ConfirmClose => Effect::ConfirmClose,
            SettingId::DefaultCwd
            | SettingId::Shell
            | SettingId::ShellArgs
            | SettingId::ReportIterm2
            | SettingId::ShellIntegration => Effect::NextSpawn,
        }
    }
}

/// What the app does when a setting changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    /// `material::apply` on the window, and the theme's layers and header tint (or the opaque
    /// ones): the Theme and the Glass style both do it.
    Glass,
    /// The renderer's menu and dialog blur.
    MenuBlur,
    /// The renderer's motion tier.
    Motion,
    /// The renderer's terminal fonts, and the geometry sent again to every tab.
    Font,
    /// The renderer's text setting: the UI text and every terminal view, on the next frame (no
    /// restart).
    TextRendering,
    /// Every tab's colours (`Command::SetTheme`); a colour the child set itself stays.
    Palette,
    /// Read each frame: the outline gate and the strip's `BusyCue`.
    Outline,
    /// `Command::SetScrollback` to every tab.
    Scrollback,
    /// `Command::SetClipboardWrite` to every tab.
    OscClipboard,
    /// `set_option_as_alt` on the window (macOS), and the key router's flag.
    OptionAsMeta,
    /// Read when a tab closes (`plan_close`).
    ConfirmClose,
    /// Tabs opened from now on; the page says so (its restart notice).
    NextSpawn,
}

/// The effects `old` to `new` has, each once, in the order of the changed settings.
pub fn effects(old: &Settings, new: &Settings) -> Vec<Effect> {
    let mut out = Vec::new();
    for id in old.changed(new) {
        let e = id.effect();
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_lunegits() {
        let s = Settings::default();
        assert_eq!((s.theme, s.glass), (Theme::Luneterm, GlassStyle::Default));
        assert_eq!((s.font_size, s.scrollback), (14, 5000));
        assert!(s.osc_clipboard && s.confirm_close && s.font_smoothing);
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
            assert_eq!(at(0, bad).font_size, 14, "size {bad} resets");
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
        assert_eq!(with(GlassStyle::Default).material(), near_black::MATERIAL);
        assert_eq!(with(GlassStyle::Off).material(), Material::Opaque);
        assert_eq!(
            with(GlassStyle::LiquidClear).material(),
            Material::LiquidClear
        );
        assert_eq!(
            with(GlassStyle::LiquidRegular).material(),
            Material::LiquidRegular
        );
        // The default is the near-black material, a glass the app applies (not `Opaque`).
        assert!(with(GlassStyle::Default).material().is_glass());
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

    /// Every (style, theme) to its OS material: only the default style follows the theme.
    #[test]
    fn the_theme_picks_the_material_of_the_default_style_only() {
        let material = |glass, theme| {
            Settings {
                glass,
                theme,
                ..Settings::default()
            }
            .material()
        };
        let table = [
            (GlassStyle::Default, Theme::Luneterm, near_black::MATERIAL),
            (GlassStyle::Default, Theme::Lunegit, Material::Vibrancy),
            (
                GlassStyle::LiquidClear,
                Theme::Luneterm,
                Material::LiquidClear,
            ),
            (
                GlassStyle::LiquidClear,
                Theme::Lunegit,
                Material::LiquidClear,
            ),
            (
                GlassStyle::LiquidRegular,
                Theme::Luneterm,
                Material::LiquidRegular,
            ),
            (
                GlassStyle::LiquidRegular,
                Theme::Lunegit,
                Material::LiquidRegular,
            ),
            (GlassStyle::Off, Theme::Luneterm, Material::Opaque),
            (GlassStyle::Off, Theme::Lunegit, Material::Opaque),
        ];
        assert_eq!(table.len(), GlassStyle::ALL.len() * Theme::ALL.len());
        for (glass, theme, want) in table {
            assert_eq!(material(glass, theme), want, "{glass:?} {theme:?}");
        }
        assert_ne!(near_black::MATERIAL, Material::Vibrancy);
        // Changing the theme alone applies like a glass change: the window's material and the
        // renderer's layers, with no restart.
        let lunegit = Settings {
            theme: Theme::Lunegit,
            ..Settings::default()
        };
        assert_eq!(effects(&Settings::default(), &lunegit), vec![Effect::Glass]);
    }

    #[test]
    fn enum_names_round_trip_and_are_stable() {
        for (&t, name) in Theme::ALL.iter().zip(["luneterm", "lunegit"]) {
            assert_eq!(t.name(), name);
            assert_eq!(Theme::from_name(name), Some(t));
        }
        for bad in ["", "Luneterm", " lunegit", "dark", "default"] {
            assert_eq!(Theme::from_name(bad), None, "{bad:?}");
        }
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

    /// The step 6 table, one literal row per setting (not derived from `effect`).
    #[test]
    fn every_setting_has_the_effect_the_table_gives_it() {
        use Effect as E;
        use SettingId as S;
        let table = [
            (S::Theme, E::Glass),
            (S::Glass, E::Glass),
            (S::MenuBlur, E::MenuBlur),
            (S::Motion, E::Motion),
            (S::Font, E::Font),
            (S::FontSize, E::Font),
            (S::FontSmoothing, E::TextRendering),
            (S::Palette, E::Palette),
            (S::AnimateOutline, E::Outline),
            (S::Outline, E::Outline),
            (S::OutlineFullScreen, E::Outline),
            (S::Scrollback, E::Scrollback),
            (S::DefaultCwd, E::NextSpawn),
            (S::Shell, E::NextSpawn),
            (S::ShellArgs, E::NextSpawn),
            (S::ReportIterm2, E::NextSpawn),
            (S::ShellIntegration, E::NextSpawn),
            (S::ConfirmClose, E::ConfirmClose),
            (S::OscClipboard, E::OscClipboard),
            (S::OptionAsMeta, E::OptionAsMeta),
        ];
        assert_eq!(table.len(), SettingId::ALL.len());
        for (id, want) in table {
            assert_eq!(id.effect(), want, "{id:?}");
            assert!(SettingId::ALL.contains(&id));
        }
    }

    /// Each field, changed alone, is exactly one id: no field is dead and no two share an id.
    #[test]
    fn changing_one_field_changes_exactly_its_id() {
        let d = Settings::default();
        let cases: [(SettingId, Settings); 20] = [
            (
                SettingId::Theme,
                Settings {
                    theme: Theme::Lunegit,
                    ..d.clone()
                },
            ),
            (
                SettingId::Glass,
                Settings {
                    glass: GlassStyle::Off,
                    ..d.clone()
                },
            ),
            (
                SettingId::MenuBlur,
                Settings {
                    menu_blur: BlurLevel::High,
                    ..d.clone()
                },
            ),
            (
                SettingId::Motion,
                Settings {
                    motion: Motion::None,
                    ..d.clone()
                },
            ),
            (
                SettingId::Font,
                Settings {
                    font: "x".into(),
                    ..d.clone()
                },
            ),
            (
                SettingId::FontSize,
                Settings {
                    font_size: 20,
                    ..d.clone()
                },
            ),
            (
                SettingId::FontSmoothing,
                Settings {
                    font_smoothing: false,
                    ..d.clone()
                },
            ),
            (
                SettingId::Palette,
                Settings {
                    palette: "Nord".into(),
                    ..d.clone()
                },
            ),
            (
                SettingId::AnimateOutline,
                Settings {
                    animate_outline: false,
                    ..d.clone()
                },
            ),
            (
                SettingId::Outline,
                Settings {
                    outline: OutlineAnim::Comet,
                    ..d.clone()
                },
            ),
            (
                SettingId::OutlineFullScreen,
                Settings {
                    outline_full_screen: true,
                    ..d.clone()
                },
            ),
            (
                SettingId::Scrollback,
                Settings {
                    scrollback: 1,
                    ..d.clone()
                },
            ),
            (
                SettingId::DefaultCwd,
                Settings {
                    default_cwd: "/".into(),
                    ..d.clone()
                },
            ),
            (
                SettingId::Shell,
                Settings {
                    shell: "sh".into(),
                    ..d.clone()
                },
            ),
            (
                SettingId::ShellArgs,
                Settings {
                    shell_args: "-l".into(),
                    ..d.clone()
                },
            ),
            (
                SettingId::ReportIterm2,
                Settings {
                    report_iterm2: true,
                    ..d.clone()
                },
            ),
            (
                SettingId::ShellIntegration,
                Settings {
                    shell_integration: false,
                    ..d.clone()
                },
            ),
            (
                SettingId::ConfirmClose,
                Settings {
                    confirm_close: false,
                    ..d.clone()
                },
            ),
            (
                SettingId::OscClipboard,
                Settings {
                    osc_clipboard: false,
                    ..d.clone()
                },
            ),
            (
                SettingId::OptionAsMeta,
                Settings {
                    option_as_meta: true,
                    ..d.clone()
                },
            ),
        ];
        let mut seen = Vec::new();
        for (id, changed) in cases {
            assert_eq!(d.changed(&changed), vec![id], "{id:?}");
            assert_eq!(effects(&d, &changed), vec![id.effect()]);
            seen.push(id);
        }
        assert_eq!(seen, SettingId::ALL);
        assert!(effects(&d, &d).is_empty());
        // Two fields with one effect give it once; two effects give both, in field order.
        let both = Settings {
            font_size: 9,
            font: "y".into(),
            ..d.clone()
        };
        assert_eq!(effects(&d, &both), vec![Effect::Font]);
        let two = Settings {
            shell: "sh".into(),
            scrollback: 3,
            ..d
        };
        assert_eq!(
            effects(&Settings::default(), &two),
            vec![Effect::Scrollback, Effect::NextSpawn]
        );
    }

    #[test]
    fn font_smoothing_picks_the_text_rendering_and_applies_live() {
        let on = Settings::default();
        let off = Settings {
            font_smoothing: false,
            ..on.clone()
        };
        assert_eq!(on.text_rendering(), TextRendering::SMOOTH);
        assert_eq!(off.text_rendering(), TextRendering::OFF);
        // Its own effect, not the next-spawn one: no restart notice, the renderer takes it live.
        assert_eq!(effects(&on, &off), vec![Effect::TextRendering]);
        assert_eq!(effects(&off, &on), vec![Effect::TextRendering]);
        assert_ne!(SettingId::FontSmoothing.effect(), Effect::NextSpawn);
        // Smoothing alone never changes the clamped value.
        assert_eq!(off.clone().clamped(), off);
    }

    #[test]
    fn the_outline_settings_pick_the_busy_cue() {
        use lune_ui::widgets::tab_strip::BusyCue;
        let with = |animate_outline, outline, motion| Settings {
            animate_outline,
            outline,
            motion,
            ..Settings::default()
        };
        let full = Motion::Full;
        assert_eq!(
            with(true, OutlineAnim::Sweep, full).busy_cue(),
            Some(BusyCue::Sweep)
        );
        assert_eq!(
            with(true, OutlineAnim::Comet, full).busy_cue(),
            Some(BusyCue::Comet)
        );
        assert_eq!(
            with(true, OutlineAnim::Breath, full).busy_cue(),
            Some(BusyCue::Breath)
        );
        // Off wins; Reduced forces the breath; None keeps it static.
        assert_eq!(with(false, OutlineAnim::Comet, full).busy_cue(), None);
        assert_eq!(
            with(true, OutlineAnim::Comet, Motion::Reduced).busy_cue(),
            Some(BusyCue::Breath)
        );
        assert_eq!(
            with(true, OutlineAnim::Comet, Motion::None).busy_cue(),
            None
        );
    }
}
