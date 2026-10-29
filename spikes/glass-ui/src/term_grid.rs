//! Termview demo (`GLASS_UI_TERMVIEW=1`): draws the terminal body with `lune-termview`.
//!
//! The mock screens become a `lune_termview::Grid`. Cluster widths here come from
//! `unicode-width`: they are MOCK widths, not engine widths. The real engine will supply
//! its own (plan decision D13); the renderer never computes widths.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use lune_termview::{
    Attrs, Cell, CellMetrics, Color, Cursor, CursorShape, Grid, GridLayout, SelectionKind,
    SelectionRange, TermFonts, TermTheme, TermView, ViewState,
};
use lune_ui::text::TextCache;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::mock::{Ink, Screen};
use crate::state::{Settings, State};
use crate::theme::{MONO_FONT, PALETTES};

/// Slot of the termview in the frame's custom-draw list.
pub const TERM_SLOT: usize = 0;

/// Link id given to every mock link cell.
const LINK_ID: u32 = 1;

/// What a built grid depends on; the grid is rebuilt only when this changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GridKey {
    tab: usize,
    palette: usize,
    font_size: u32,
    cols: usize,
    rows: usize,
    screen: u64,
}

/// The termview, its grid, and the settings it was last configured with.
#[derive(Debug)]
pub struct TermState {
    pub view: TermView,
    grid: Grid,
    key: Option<GridKey>,
    fonts: TermFonts,
    palette: usize,
}

#[cfg(target_os = "macos")]
fn emoji_family() -> &'static str {
    "Apple Color Emoji"
}

#[cfg(target_os = "windows")]
fn emoji_family() -> &'static str {
    "Segoe UI Emoji"
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn emoji_family() -> &'static str {
    "Noto Color Emoji"
}

fn fonts_for(s: &Settings) -> TermFonts {
    TermFonts {
        family: MONO_FONT.to_owned(),
        size: s.font_size as f32,
        line_height: s.line_height(),
        emoji_family: Some(emoji_family().to_owned()),
    }
}

fn theme_for(palette: usize) -> TermTheme {
    let p = &PALETTES[palette];
    let rgb = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8, 0xFF];
    TermTheme::from_ansi16(p.ansi.map(rgb), rgb(p.fg), [0x1E, 0x1E, 0x1E, 0xFF])
}

fn color(ink: Ink) -> Color {
    match ink {
        Ink::Fg => Color::Default,
        Ink::Ansi(i) => Color::Palette(i),
    }
}

fn screen_hash(screen: &Screen) -> u64 {
    let mut h = DefaultHasher::new();
    for line in &screen.lines {
        for span in &line.spans {
            (&span.text, span.fg, span.bold).hash(&mut h);
        }
        '\n'.hash(&mut h);
    }
    for l in &screen.links {
        (l.row, l.col_start, l.col_end).hash(&mut h);
    }
    h.finish()
}

/// Mock screen -> grid of `cols` x `rows`. Lines are truncated at `cols`; rows past the
/// screen stay blank.
fn grid_from(screen: &Screen, cols: usize, rows: usize) -> Grid {
    let mut grid = Grid::new(cols, rows);
    for (row, line) in screen.lines.iter().take(rows).enumerate() {
        let Some(cells) = grid.row_mut(row) else {
            continue;
        };
        let mut col = 0;
        'line: for span in &line.spans {
            for cluster in span.text.graphemes(true) {
                let w = cluster.width().clamp(1, 2);
                if col + w > cols {
                    break 'line;
                }
                let mut cell = Cell::default();
                cell.set_text(cluster);
                cell.width = w as u8;
                cell.fg = color(span.fg);
                if span.bold {
                    cell.attrs = Attrs::BOLD;
                }
                cells[col] = cell;
                if w == 2 {
                    let mut spacer = Cell::default();
                    spacer.width = 0;
                    cells[col + 1] = spacer;
                }
                col += w;
            }
        }
        for l in screen.links.iter().filter(|l| l.row == row) {
            for cell in cells.iter_mut().take(l.col_end).skip(l.col_start) {
                cell.link = LINK_ID;
            }
        }
    }
    grid
}

impl TermState {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, settings: &Settings) -> Self {
        let fonts = fonts_for(settings);
        TermState {
            view: TermView::new(device, format, fonts.clone(), theme_for(settings.palette)),
            grid: Grid::new(0, 0),
            key: None,
            fonts,
            palette: settings.palette,
        }
    }

    /// Cell metrics at `scale`, after following any font, size or palette change in `settings`.
    pub fn metrics(
        &mut self,
        text: &mut TextCache,
        settings: &Settings,
        scale: f32,
    ) -> CellMetrics {
        let fonts = fonts_for(settings);
        if fonts != self.fonts {
            self.view.set_fonts(fonts.clone());
            self.fonts = fonts;
        }
        if settings.palette != self.palette {
            self.view.set_theme(theme_for(settings.palette));
            self.palette = settings.palette;
        }
        self.view.metrics(&mut text.fonts, scale)
    }

    /// Rebuilds the grid if its inputs changed, then updates the view. Call after painting, so
    /// `st.term_body` is this frame's.
    pub fn update(&mut self, text: &mut TextCache, st: &State, scale: f32) {
        let m = self.view.metrics(&mut text.fonts, scale);
        let body = st.term_body;
        let layout = GridLayout::new((body.x, body.y), m);
        let (cols, rows) = layout.grid_size(body.w, body.h);
        let tab = &st.tabs[st.active];
        let key = GridKey {
            tab: st.active,
            palette: st.settings.palette,
            font_size: st.settings.font_size,
            cols,
            rows,
            screen: screen_hash(&tab.screen),
        };
        if self.key.as_ref() != Some(&key) {
            self.grid = grid_from(&tab.screen, cols, rows);
            self.key = Some(key);
        }
        let selection = tab.selection.map(|(start, end)| SelectionRange {
            start,
            end,
            kind: SelectionKind::Linear,
        });
        let (row, col) = tab.screen.cursor;
        let view = ViewState {
            cursor: Cursor {
                row,
                col,
                shape: CursorShape::Block,
                blink: false,
                visible: true,
            },
            focused: st.focused,
            blink_on: false,
            selection: selection.as_slice(),
            hover_link: if st.cmd && crate::terminal_view::link_at(st).is_some() {
                LINK_ID
            } else {
                0
            },
        };
        self.view.update(
            &mut text.fonts,
            &mut self.grid,
            &view,
            (body.x, body.y),
            scale,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{Kind, screen};

    #[test]
    fn wide_cluster_is_leading_cell_plus_spacer() {
        let s = screen(Kind::Agent);
        let g = grid_from(&s, 120, 40);
        // " ⏺ Wide glyphs: 你好世界 …": find 你 and check its spacer.
        let (row, col) = (0..g.rows())
            .find_map(|r| {
                g.row(r)?
                    .iter()
                    .position(|c| c.text() == "你")
                    .map(|c| (r, c))
            })
            .expect("你 in the agent screen");
        assert_eq!(g.get(row, col).map(|c| c.width), Some(2));
        assert_eq!(g.get(row, col + 1).map(|c| c.width), Some(0));
    }

    #[test]
    fn lines_truncate_at_cols_and_rows_past_the_screen_stay_blank() {
        let s = screen(Kind::Colors);
        let g = grid_from(&s, 10, 100);
        assert_eq!(g.cols(), 10);
        assert!(
            g.row(99)
                .is_some_and(|r| r.iter().all(|c| c.text().is_empty()))
        );
    }
}
