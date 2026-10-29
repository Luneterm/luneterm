//! Lunegit design tokens (`DarkTheme.axaml`, `App.axaml`) and terminal palettes
//! (`TerminalPalettePresets.cs`). Colours are straight-alpha RGBA.

pub type Rgba = [u8; 4];

pub const FG_PRIMARY: Rgba = [0xFF, 0xFF, 0xFF, 0xCC];
pub const FG_SECONDARY: Rgba = [0xFF, 0xFF, 0xFF, 0xAA];
pub const FG_TERTIARY: Rgba = [0xFF, 0xFF, 0xFF, 0x88];
pub const FG_MUTED: Rgba = [0xB0, 0xB0, 0xB0, 0xFF];
pub const SURFACE_INPUT: Rgba = [0xFF, 0xFF, 0xFF, 0x0A];
pub const SURFACE_HOVER: Rgba = [0xFF, 0xFF, 0xFF, 0x10];
pub const SURFACE_SELECTED: Rgba = [0xFF, 0xFF, 0xFF, 0x18];
pub const BORDER: Rgba = [0xFF, 0xFF, 0xFF, 0x30];
pub const BORDER_SUBTLE: Rgba = [0xFF, 0xFF, 0xFF, 0x1A];
pub const SELECTION: Rgba = [0x76, 0x7D, 0x85, 0x33];
pub const ACCENT: Rgba = [0x76, 0x7D, 0x85, 0xFF];
pub const CURSOR: Rgba = [0xCC, 0xCC, 0xCC, 0x80];
pub const DROPDOWN_BG: Rgba = [0x00, 0x00, 0x00, 0xF0];
pub const DIALOG_BACKDROP: Rgba = [0x00, 0x00, 0x00, 0x80];
pub const DIALOG_FOOTER: Rgba = [0x00, 0x00, 0x00, 0x24];
/// Lunegit's window fallback (tint lightened 20%), shown while the window is inactive.
pub const INACTIVE_SOLID: Rgba = [0x33, 0x33, 0x33, 0xFF];
/// Base under the glass layers when there is no OS glass (Linux).
pub const OPAQUE_BASE: Rgba = [0x1F, 0x1F, 0x1F, 0xFF];

// Luneium browser context-menu tokens (`tokens.css`, `ContextMenu.css`).
pub const LUNEIUM_TEXT_PRIMARY: Rgba = [0xFF, 0xFF, 0xFF, 0xCC];
pub const LUNEIUM_TEXT_TERTIARY: Rgba = [0xFF, 0xFF, 0xFF, 0x88];
pub const LUNEIUM_TEXT_QUATERNARY: Rgba = [0xFF, 0xFF, 0xFF, 0x66];
pub const LUNEIUM_POPUP_BORDER: Rgba = [0xFF, 0xFF, 0xFF, 0x3A];
pub const LUNEIUM_ITEM_HOVER: Rgba = [0xFF, 0xFF, 0xFF, 0x16];
pub const LUNEIUM_SEPARATOR: Rgba = [0xFF, 0xFF, 0xFF, 0x22];
/// `linear-gradient(rgba(0,0,0,.45)) , rgba(165,165,165,.3)` flattened: alpha .615, grey 44.
pub const LUNEIUM_MENU_GLASS: Rgba = [44, 44, 44, 157];

pub const FONT_S: f32 = 11.0;
pub const FONT_BASE: f32 = 13.0;
pub const FONT_M: f32 = 14.0;
pub const FONT_L: f32 = 18.0;
pub const ITEM_RADIUS: f32 = 6.0;

pub const UI_FONT: &str = "RznSans";
/// Lunegit's bundled terminal font. RznMono replaces it once it exists.
pub const MONO_FONT: &str = "RobotoMono Nerd Font Mono";

/// Codicons, as mapped into the Nerd Font (Lunegit uses the Codicons icon pack).
pub mod icon {
    pub const ADD: &str = "\u{ea60}";
    pub const GEAR: &str = "\u{eb51}";
    pub const ELLIPSIS: &str = "\u{ea7c}";
    pub const CHROME_CLOSE: &str = "\u{eab8}";
    pub const EDIT: &str = "\u{ea73}";
    pub const COPY: &str = "\u{ebcc}";
    pub const INFO: &str = "\u{ea74}";
    pub const TERMINAL: &str = "\u{ea85}";
    pub const CHEVRON_DOWN: &str = "\u{eab4}";
    pub const ARROW_LEFT: &str = "\u{ea9b}";
    pub const CHECK: &str = "\u{eab2}";
    pub const CHROME_MINIMIZE: &str = "\u{eaba}";
    pub const CHROME_MAXIMIZE: &str = "\u{eab9}";
    pub const CHROME_RESTORE: &str = "\u{eabb}";
}

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

pub fn premul(c: Rgba) -> [f32; 4] {
    let a = f32::from(c[3]) / 255.0;
    [
        f32::from(c[0]) / 255.0 * a,
        f32::from(c[1]) / 255.0 * a,
        f32::from(c[2]) / 255.0 * a,
        a,
    ]
}

pub fn over(top: [f32; 4], below: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - top[3];
    [
        top[0] + below[0] * k,
        top[1] + below[1] * k,
        top[2] + below[2] * k,
        top[3] + below[3] * k,
    ]
}

pub fn with_alpha(c: Rgba, k: f32) -> Rgba {
    [c[0], c[1], c[2], (f32::from(c[3]) * k).round() as u8]
}
