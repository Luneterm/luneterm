//! The terminal colour palettes (Lunegit's `TerminalPalettePresets.cs`): ten named sets of the 16
//! ANSI colours and a foreground. The background stays fixed (#1E1E1E), as in Lunegit. A palette
//! is saved by its name (plan M10 D5), so adding or reordering one cannot remap a saved choice.

use lune_termview::TermTheme;

use crate::settings::DEFAULT_PALETTE;

/// The terminal background every palette is drawn on.
const BACKGROUND: u32 = 0x1E1E1E;

/// One palette: its name, the 16 ANSI colours and the foreground, as 0xRRGGBB.
#[derive(Debug)]
pub struct Palette {
    pub name: &'static str,
    pub ansi: [u32; 16],
    pub fg: u32,
}

fn rgb(v: u32) -> [u8; 4] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 0xFF]
}

impl Palette {
    /// The terminal theme of this palette.
    pub fn theme(&self) -> TermTheme {
        TermTheme::from_ansi16(self.ansi.map(rgb), rgb(self.fg), rgb(BACKGROUND))
    }
}

pub static PALETTES: [Palette; 10] = [
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

/// The palette called `name`, `None` for a name that is none of the ten.
pub fn by_name(name: &str) -> Option<&'static Palette> {
    PALETTES.iter().find(|p| p.name == name)
}

/// The theme of the palette called `name`; an unknown name gives the default palette's.
pub fn theme_of(name: &str) -> TermTheme {
    by_name(name)
        .or_else(|| by_name(DEFAULT_PALETTE))
        .map_or_else(TermTheme::lunegit_one_dark, Palette::theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_ten_uniquely_named_palettes_and_the_default_is_one_of_them() {
        assert_eq!(PALETTES.len(), 10);
        for (i, p) in PALETTES.iter().enumerate() {
            assert!(
                PALETTES.iter().skip(i + 1).all(|q| q.name != p.name),
                "{} twice",
                p.name
            );
            assert!(!p.name.is_empty() && p.name.len() <= crate::settings::MAX_NAME_BYTES);
        }
        assert!(by_name(DEFAULT_PALETTE).is_some());
        assert_eq!(PALETTES[0].name, "One Dark");
    }

    #[test]
    fn one_dark_is_the_engines_default_theme_and_the_others_differ_in_colour_not_background() {
        assert_eq!(theme_of("One Dark"), TermTheme::lunegit_one_dark());
        let dracula = theme_of("Dracula");
        let one = theme_of("One Dark");
        assert_ne!(dracula.palette[1], one.palette[1]);
        assert_ne!(dracula.foreground, one.foreground);
        for p in PALETTES.iter() {
            let t = p.theme();
            assert_eq!(
                t.background, one.background,
                "{}: background is fixed",
                p.name
            );
            assert_eq!(t.palette[15], rgb(p.ansi[15]));
            // 16-255 follow the xterm cube, whatever the palette.
            assert_eq!(t.palette[16..], one.palette[16..]);
        }
    }

    #[test]
    fn an_unknown_name_is_no_palette_and_themes_as_the_default() {
        assert!(by_name("one dark").is_none());
        assert!(by_name("").is_none());
        assert_eq!(theme_of("nope"), theme_of(DEFAULT_PALETTE));
    }
}
