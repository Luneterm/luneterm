//! Terminal colours and their resolution to RGBA (plan decision D7, rules from xterm.js
//! `addons/addon-webgl/src/TextureAtlas.ts:300-395` and `CellColorResolver.ts`).

use lune_ui::color::Rgba;

use crate::model::{Attrs, Cell, Color};

/// Colours the grid is drawn with.
#[derive(Debug, Clone, PartialEq)]
pub struct TermTheme {
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub selection: Rgba,
    pub link_hover: Rgba,
    pub palette: [Rgba; 256],
    /// Bold text in palette colours 0-7 uses 8-15 (xterm.js `drawBoldTextInBrightColors`).
    pub bold_is_bright: bool,
    pub dim_opacity: f32,
}

fn rgb(v: u32) -> Rgba {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 0xFF]
}

impl TermTheme {
    /// Builds a theme from the 16 ANSI colours; 16-255 follow xterm.js `Types.ts:203-222`.
    pub fn from_ansi16(ansi: [Rgba; 16], foreground: Rgba, background: Rgba) -> TermTheme {
        let mut palette = [[0, 0, 0, 0xFF]; 256];
        palette[..16].copy_from_slice(&ansi);
        const V: [u8; 6] = [0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF];
        for i in 0..216usize {
            palette[16 + i] = [V[(i / 36) % 6], V[(i / 6) % 6], V[i % 6], 0xFF];
        }
        for i in 0..24usize {
            let c = 8 + 10 * i as u8;
            palette[232 + i] = [c, c, c, 0xFF];
        }
        TermTheme {
            foreground,
            background,
            cursor: [0xCC, 0xCC, 0xCC, 0x80],
            selection: [0x76, 0x7D, 0x85, 0x33],
            link_hover: [0x76, 0x7D, 0x85, 0xFF],
            palette,
            bold_is_bright: true,
            dim_opacity: 0.5,
        }
    }

    /// Lunegit's default "One Dark" (`TerminalPalettePresets.cs:51-60`) on #1E1E1E.
    pub fn lunegit_one_dark() -> TermTheme {
        const ANSI: [u32; 16] = [
            0x282C34, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xABB2BF,
            0x5C6370, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xFFFFFF,
        ];
        TermTheme::from_ansi16(ANSI.map(rgb), rgb(0xABB2BF), rgb(0x1E1E1E))
    }
}

/// A cell's colours after attributes are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub fg: Rgba,
    /// `None` = not painted (default background, or exactly the theme background).
    pub bg: Option<Rgba>,
    /// Underline / strikethrough / overline colour.
    pub decoration: Rgba,
    /// False for INVISIBLE, and for BLINK during the off phase. The background stays.
    pub show_fg: bool,
}

fn with_bold_bright(n: u8, bold: bool, theme: &TermTheme) -> Rgba {
    let i = if bold && theme.bold_is_bright && n < 8 {
        n + 8
    } else {
        n
    };
    theme.palette[usize::from(i)]
}

fn rgb_of(c: Color, theme: &TermTheme, bold: bool) -> Option<Rgba> {
    match c {
        Color::Default => None,
        Color::Palette(n) => Some(with_bold_bright(n, bold, theme)),
        Color::Rgb(r, g, b) => Some([r, g, b, 0xFF]),
    }
}

fn dim(c: Rgba, k: f32) -> Rgba {
    // Round half up; alpha is non-negative.
    let a = (f32::from(c[3]) * k + 0.5).floor().clamp(0.0, 255.0) as u8;
    [c[0], c[1], c[2], a]
}

/// Applies the xterm.js colour rules (plan step 2, rules 1-7) to one cell.
pub fn resolve(cell: &Cell, theme: &TermTheme, blink_on: bool) -> Resolved {
    let bold = cell.attrs.contains(Attrs::BOLD);
    // (colour, is the unset default background)
    let mut fg = (
        rgb_of(cell.fg, theme, bold).unwrap_or(theme.foreground),
        false,
    );
    let mut bg = match cell.bg {
        // Bold-bright applies to the foreground only.
        Color::Palette(n) => (theme.palette[usize::from(n)], false),
        Color::Rgb(r, g, b) => ([r, g, b, 0xFF], false),
        Color::Default => (theme.background, true),
    };
    if cell.attrs.contains(Attrs::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    let bg_out = (!bg.1 && bg.0 != theme.background).then_some(bg.0);
    let mut fg = fg.0;
    let mut decoration = rgb_of(cell.underline_color, theme, bold).unwrap_or(fg);
    if cell.attrs.contains(Attrs::DIM) {
        fg = dim(fg, theme.dim_opacity);
        decoration = dim(decoration, theme.dim_opacity);
    }
    let hidden =
        cell.attrs.contains(Attrs::INVISIBLE) || (cell.attrs.contains(Attrs::BLINK) && !blink_on);
    Resolved {
        fg,
        bg: bg_out,
        decoration,
        show_fg: !hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> TermTheme {
        TermTheme::lunegit_one_dark()
    }

    fn cell(f: impl FnOnce(&mut Cell)) -> Cell {
        let mut c = Cell::default();
        f(&mut c);
        c
    }

    #[test]
    fn default_cell() {
        let t = th();
        let r = resolve(&Cell::default(), &t, true);
        assert_eq!(r.fg, t.foreground);
        assert_eq!(r.bg, None);
        assert!(r.show_fg);
        assert_eq!(t.foreground, [0xAB, 0xB2, 0xBF, 0xFF]);
        assert_eq!(t.background, [0x1E, 0x1E, 0x1E, 0xFF]);
    }

    #[test]
    fn bold_bright() {
        let mut t = th();
        let fg = |t: &TermTheme, c: Color| {
            resolve(
                &cell(|x| {
                    x.fg = c;
                    x.attrs = Attrs::BOLD;
                }),
                t,
                true,
            )
            .fg
        };
        assert_eq!(fg(&t, Color::Palette(1)), t.palette[9]);
        assert_eq!(fg(&t, Color::Palette(9)), t.palette[9]);
        assert_eq!(fg(&t, Color::Palette(200)), t.palette[200]);
        assert_eq!(fg(&t, Color::Rgb(1, 2, 3)), [1, 2, 3, 0xFF]);
        t.bold_is_bright = false;
        assert_eq!(fg(&t, Color::Palette(1)), t.palette[1]);
    }

    #[test]
    fn inverse_defaults() {
        let t = th();
        let r = resolve(&cell(|c| c.attrs = Attrs::INVERSE), &t, true);
        assert_eq!(r.fg, t.background);
        assert_eq!(r.bg, Some(t.foreground));
    }

    #[test]
    fn inverse_palette() {
        let t = th();
        let r = resolve(
            &cell(|c| {
                c.fg = Color::Palette(2);
                c.bg = Color::Palette(4);
                c.attrs = Attrs::INVERSE;
            }),
            &t,
            true,
        );
        assert_eq!(r.fg, t.palette[4]);
        assert_eq!(r.bg, Some(t.palette[2]));
    }

    #[test]
    fn background_equal_to_theme_is_skipped() {
        let t = th();
        let bg = |b| resolve(&cell(|c| c.bg = b), &t, true).bg;
        assert_eq!(bg(Color::Rgb(0x1E, 0x1E, 0x1E)), None);
        assert_eq!(
            bg(Color::Rgb(0x1E, 0x1E, 0x1F)),
            Some([0x1E, 0x1E, 0x1F, 0xFF])
        );
    }

    #[test]
    fn dim_halves_alpha_of_fg_and_decoration_only() {
        let t = th();
        let r = resolve(
            &cell(|c| {
                c.attrs = Attrs::DIM;
                c.bg = Color::Rgb(9, 9, 9);
                c.underline = crate::model::UnderlineStyle::Single;
            }),
            &t,
            true,
        );
        assert_eq!(r.fg[3], 128);
        assert_eq!(r.decoration[3], 128);
        assert_eq!(r.bg, Some([9, 9, 9, 0xFF]));
    }

    #[test]
    fn invisible_keeps_background() {
        let t = th();
        let r = resolve(
            &cell(|c| {
                c.attrs = Attrs::INVISIBLE;
                c.bg = Color::Palette(4);
            }),
            &t,
            true,
        );
        assert!(!r.show_fg);
        assert_eq!(r.bg, Some(t.palette[4]));
    }

    #[test]
    fn blink_phase() {
        let t = th();
        let c = cell(|c| c.attrs = Attrs::BLINK);
        assert!(!resolve(&c, &t, false).show_fg);
        assert!(resolve(&c, &t, true).show_fg);
    }

    #[test]
    fn underline_colour() {
        let t = th();
        let r = resolve(
            &cell(|c| {
                c.underline_color = Color::Palette(3);
                c.attrs = Attrs::BOLD;
            }),
            &t,
            true,
        );
        assert_eq!(r.decoration, t.palette[11]);
        let d = resolve(&Cell::default(), &t, true);
        assert_eq!(d.decoration, d.fg);
    }

    #[test]
    fn palette_table() {
        let t = th();
        for (i, v) in [
            (16, 0x000000),
            (21, 0x0000FF),
            (196, 0xFF0000),
            (231, 0xFFFFFF),
            (232, 0x080808),
            (255, 0xEEEEEE),
        ] {
            assert_eq!(t.palette[i], rgb(v), "palette[{i}]");
        }
        assert_eq!(t.palette[1], rgb(0xE06C75));
    }

    #[test]
    fn defaults_for_overlays() {
        let t = th();
        assert_eq!(t.cursor, [0xCC, 0xCC, 0xCC, 0x80]);
        assert_eq!(t.selection, [0x76, 0x7D, 0x85, 0x33]);
        assert_eq!(t.link_hover, [0x76, 0x7D, 0x85, 0xFF]);
        assert!(t.bold_is_bright);
        assert_eq!(t.dim_opacity, 0.5);
    }
}
