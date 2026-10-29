//! Lunegit design tokens (`DarkTheme.axaml`, `App.axaml`) and terminal palettes
//! (`TerminalPalettePresets.cs`). Colours are straight-alpha RGBA.

pub use lune_ui::color::{Rgba, over, premul, with_alpha};

pub use lune_ui::icon;
use lune_ui::lunegit_dark::THEME;

// Tokens the toolkit widgets read live in `lunegit_dark::THEME`; these names alias the fields
// the app's own views also use.
pub const FG_PRIMARY: Rgba = THEME.fg_primary;
pub const FG_SECONDARY: Rgba = THEME.fg_secondary;
pub const FG_TERTIARY: Rgba = THEME.fg_tertiary;
pub const SURFACE_HOVER: Rgba = THEME.surface_hover;
pub const SURFACE_SELECTED: Rgba = THEME.surface_selected;
pub const BORDER: Rgba = THEME.border;
pub const BORDER_SUBTLE: Rgba = THEME.border_subtle;
pub const ACCENT: Rgba = THEME.accent;
pub const FONT_S: f32 = THEME.font_s;
pub const FONT_BASE: f32 = THEME.font_base;
pub const FONT_M: f32 = THEME.font_m;
pub const ITEM_RADIUS: f32 = THEME.item_radius;

// App-only tokens.
pub const FG_MUTED: Rgba = [0xB0, 0xB0, 0xB0, 0xFF];
pub const SELECTION: Rgba = [0x76, 0x7D, 0x85, 0x33];
pub const CURSOR: Rgba = [0xCC, 0xCC, 0xCC, 0x80];
/// Lunegit's window fallback (tint lightened 20%), shown while the window is inactive.
pub const INACTIVE_SOLID: Rgba = [0x33, 0x33, 0x33, 0xFF];
/// Base under the glass layers when there is no OS glass (Linux).
pub const OPAQUE_BASE: Rgba = [0x1F, 0x1F, 0x1F, 0xFF];
pub const FONT_L: f32 = 18.0;

pub const UI_FONT: &str = "RznSans";
/// Lunegit's bundled terminal font. RznMono replaces it once it exists.
pub const MONO_FONT: &str = "RobotoMono Nerd Font Mono";

/// Codicon for the "Duplicate Tab" menu item (the toolkit has no app-specific glyphs).
pub const ICON_TERMINAL: &str = "\u{ea85}";

#[derive(Debug)]
pub struct Palette {
    pub name: &'static str,
    pub ansi: [u32; 16],
    pub fg: u32,
}

impl Palette {
    pub fn rgb(&self, ink: crate::mock::Ink) -> [u8; 3] {
        let v = match ink {
            crate::mock::Ink::Fg => self.fg,
            crate::mock::Ink::Ansi(i) => self.ansi[usize::from(i & 15)],
        };
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }
}

pub const PALETTES: [Palette; 10] = [
    Palette {
        name: "One Dark",
        ansi: [
            0x282C34, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xABB2BF,
            0x5C6370, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xFFFFFF,
        ],
        fg: 0xABB2BF,
    },
    Palette {
        name: "xterm",
        ansi: [
            0x000000, 0xCD0000, 0x00CD00, 0xCDCD00, 0x0000EE, 0xCD00CD, 0x00CDCD, 0xE5E5E5,
            0x7F7F7F, 0xFF0000, 0x00FF00, 0xFFFF00, 0x5C5CFF, 0xFF00FF, 0x00FFFF, 0xFFFFFF,
        ],
        fg: 0xCCCCCC,
    },
    Palette {
        name: "Solarized Dark",
        ansi: [
            0x073642, 0xDC322F, 0x859900, 0xB58900, 0x268BD2, 0xD33682, 0x2AA198, 0xEEE8D5,
            0x002B36, 0xCB4B16, 0x586E75, 0x657B83, 0x839496, 0x6C71C4, 0x93A1A1, 0xFDF6E3,
        ],
        fg: 0x839496,
    },
    Palette {
        name: "Gruvbox Dark",
        ansi: [
            0x282828, 0xCC241D, 0x98971A, 0xD79921, 0x458588, 0xB16286, 0x689D6A, 0xA89984,
            0x928374, 0xFB4934, 0xB8BB26, 0xFABD2F, 0x83A598, 0xD3869B, 0x8EC07C, 0xEBDBB2,
        ],
        fg: 0xEBDBB2,
    },
    Palette {
        name: "Dracula",
        ansi: [
            0x21222C, 0xFF5555, 0x50FA7B, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x8BE9FD, 0xF8F8F2,
            0x6272A4, 0xFF6E6E, 0x69FF94, 0xFFFFA5, 0xD6ACFF, 0xFF92DF, 0xA4FFFF, 0xFFFFFF,
        ],
        fg: 0xF8F8F2,
    },
    Palette {
        name: "Nord",
        ansi: [
            0x3B4252, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x88C0D0, 0xE5E9F0,
            0x4C566A, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x8FBCBB, 0xECEFF4,
        ],
        fg: 0xD8DEE9,
    },
    Palette {
        name: "Tokyo Night",
        ansi: [
            0x15161E, 0xF7768E, 0x9ECE6A, 0xE0AF68, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xA9B1D6,
            0x414868, 0xF7768E, 0x9ECE6A, 0xE0AF68, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xC0CAF5,
        ],
        fg: 0xC0CAF5,
    },
    Palette {
        name: "Catppuccin Mocha",
        ansi: [
            0x45475A, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xBAC2DE,
            0x585B70, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xA6ADC8,
        ],
        fg: 0xCDD6F4,
    },
    Palette {
        name: "Monokai",
        ansi: [
            0x272822, 0xF92672, 0xA6E22E, 0xF4BF75, 0x66D9EF, 0xAE81FF, 0xA1EFE4, 0xF8F8F2,
            0x75715E, 0xF92672, 0xA6E22E, 0xF4BF75, 0x66D9EF, 0xAE81FF, 0xA1EFE4, 0xF9F8F5,
        ],
        fg: 0xF8F8F2,
    },
    Palette {
        name: "Campbell",
        ansi: [
            0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC,
            0x767676, 0xE74856, 0x16C60C, 0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
        ],
        fg: 0xCCCCCC,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The aliases keep the literals they replaced when the tokens moved into the toolkit preset.
    #[test]
    fn aliases_keep_their_literal_values() {
        assert_eq!(FG_PRIMARY, [0xFF, 0xFF, 0xFF, 0xCC]);
        assert_eq!(FG_SECONDARY, [0xFF, 0xFF, 0xFF, 0xAA]);
        assert_eq!(FG_TERTIARY, [0xFF, 0xFF, 0xFF, 0x88]);
        assert_eq!(THEME.surface_input, [0xFF, 0xFF, 0xFF, 0x0A]);
        assert_eq!(SURFACE_HOVER, [0xFF, 0xFF, 0xFF, 0x10]);
        assert_eq!(SURFACE_SELECTED, [0xFF, 0xFF, 0xFF, 0x18]);
        assert_eq!(BORDER, [0xFF, 0xFF, 0xFF, 0x30]);
        assert_eq!(BORDER_SUBTLE, [0xFF, 0xFF, 0xFF, 0x1A]);
        assert_eq!(ACCENT, [0x76, 0x7D, 0x85, 0xFF]);
        assert_eq!(THEME.dropdown_bg, [0, 0, 0, 0xF0]);
        assert_eq!(THEME.dialog_backdrop, [0, 0, 0, 0x80]);
        assert_eq!(THEME.dialog_footer, [0, 0, 0, 0x24]);
        assert_eq!(THEME.menu_text_primary, [0xFF, 0xFF, 0xFF, 0xCC]);
        assert_eq!(THEME.menu_text_tertiary, [0xFF, 0xFF, 0xFF, 0x88]);
        assert_eq!(THEME.menu_text_quaternary, [0xFF, 0xFF, 0xFF, 0x66]);
        assert_eq!(THEME.menu_popup_border, [0xFF, 0xFF, 0xFF, 0x3A]);
        assert_eq!(THEME.menu_item_hover, [0xFF, 0xFF, 0xFF, 0x16]);
        assert_eq!(THEME.menu_separator, [0xFF, 0xFF, 0xFF, 0x22]);
        assert_eq!(THEME.menu_glass, [44, 44, 44, 157]);
        assert_eq!(THEME.accent_hover, [0x86, 0x8D, 0x95, 0xFF]);
        assert_eq!(THEME.on_accent_text, [0xFF, 0xFF, 0xFF, 0xF0]);
        assert_eq!(
            (FONT_S, FONT_BASE, FONT_M, ITEM_RADIUS),
            (11.0, 13.0, 14.0, 6.0)
        );
    }
}
