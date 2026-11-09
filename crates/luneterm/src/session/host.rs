//! Host answers (plan M9 D9, I8): the per-session palette that colour reports read and colour
//! sets and restores change, and the window-size reports. Formats follow xterm.js
//! `CoreBrowserTerminal.ts` `_handleColorEvent` (:189-229) and `_reportWindowsOptions`
//! (:1301-1317), and `XParseColor.ts` `toRgbString` (each 8-bit channel written doubled in four
//! lowercase hex digits).

use lune_termview::TermTheme;
use lune_vt::{ColorIndex, ColorRequest, WindowReportKind};

use super::Geometry;

/// The colours the session answers colour reports from: the 256-colour palette and the default
/// foreground, background and cursor, as RGB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPalette {
    pub ansi: [[u8; 3]; 256],
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
}

impl HostPalette {
    /// The palette of a termview theme (alpha dropped).
    pub fn from_theme(theme: &TermTheme) -> HostPalette {
        let rgb = |c: [u8; 4]| [c[0], c[1], c[2]];
        HostPalette {
            ansi: theme.palette.map(rgb),
            foreground: rgb(theme.foreground),
            background: rgb(theme.background),
            cursor: rgb(theme.cursor),
        }
    }

    fn slot(&self, index: ColorIndex) -> [u8; 3] {
        match index {
            ColorIndex::Palette(i) => self.ansi[usize::from(i)],
            ColorIndex::Foreground => self.foreground,
            ColorIndex::Background => self.background,
            ColorIndex::Cursor => self.cursor,
        }
    }

    fn slot_mut(&mut self, index: ColorIndex) -> &mut [u8; 3] {
        match index {
            ColorIndex::Palette(i) => &mut self.ansi[usize::from(i)],
            ColorIndex::Foreground => &mut self.foreground,
            ColorIndex::Background => &mut self.background,
            ColorIndex::Cursor => &mut self.cursor,
        }
    }
}

/// The session's live palette and the theme it restores from.
#[derive(Debug, Clone)]
pub(crate) struct ColorState {
    theme: HostPalette,
    current: HostPalette,
}

impl ColorState {
    pub(crate) fn new(theme: HostPalette) -> ColorState {
        ColorState {
            current: theme.clone(),
            theme,
        }
    }

    pub(crate) fn current(&self) -> &HostPalette {
        &self.current
    }

    /// A new theme: every colour the child has not changed (it still equals the old theme's)
    /// becomes the new theme's; a colour the child set stays, and a restore returns to the new
    /// theme. Returns whether the live palette changed.
    pub(crate) fn set_theme(&mut self, theme: HostPalette) -> bool {
        let old = std::mem::replace(&mut self.theme, theme);
        let before = self.current.clone();
        for (cur, (old, new)) in self
            .current
            .ansi
            .iter_mut()
            .zip(old.ansi.iter().zip(&self.theme.ansi))
        {
            if cur == old {
                *cur = *new;
            }
        }
        for (cur, old, new) in [
            (
                &mut self.current.foreground,
                old.foreground,
                self.theme.foreground,
            ),
            (
                &mut self.current.background,
                old.background,
                self.theme.background,
            ),
            (&mut self.current.cursor, old.cursor, self.theme.cursor),
        ] {
            if *cur == old {
                *cur = new;
            }
        }
        self.current != before
    }

    /// Applies one request: a report returns its reply bytes; a set or a restore changes the
    /// palette and returns `None` (`changed` is set).
    pub(crate) fn apply(&mut self, request: ColorRequest, changed: &mut bool) -> Option<Vec<u8>> {
        match request {
            ColorRequest::Report(index) => Some(color_report(index, self.current.slot(index))),
            ColorRequest::Set(index, rgb) => {
                *self.current.slot_mut(index) = rgb;
                *changed = true;
                None
            }
            ColorRequest::Restore(Some(index)) => {
                *self.current.slot_mut(index) = self.theme.slot(index);
                *changed = true;
                None
            }
            // ThemeService._restoreColor(undefined): every ansi colour, not the special ones.
            ColorRequest::Restore(None) => {
                self.current.ansi = self.theme.ansi;
                *changed = true;
                None
            }
        }
    }
}

/// `ESC ] <id> ; rgb:rrrr/gggg/bbbb ESC \`.
fn color_report(index: ColorIndex, rgb: [u8; 3]) -> Vec<u8> {
    let ident = match index {
        ColorIndex::Palette(i) => format!("4;{i}"),
        ColorIndex::Foreground => "10".to_owned(),
        ColorIndex::Background => "11".to_owned(),
        ColorIndex::Cursor => "12".to_owned(),
    };
    let [r, g, b] = rgb;
    format!("\x1b]{ident};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\").into_bytes()
}

/// `ESC[4;<h>;<w>t` (grid area) or `ESC[6;<cell_h>;<cell_w>t`, logical px rounded.
pub(crate) fn window_report(kind: WindowReportKind, geometry: &Geometry) -> Vec<u8> {
    let (cell_w, cell_h) = geometry.cell_px;
    let px = |v: f32| -> u64 {
        if v.is_finite() && v > 0.0 {
            // Bounded by the caps (4096 cols x a finite cell size); saturates beyond u64.
            v.round() as u64
        } else {
            0
        }
    };
    match kind {
        WindowReportKind::WinSizePixels => {
            let w = px(cell_w * geometry.cols as f32);
            let h = px(cell_h * geometry.rows as f32);
            format!("\x1b[4;{h};{w}t").into_bytes()
        }
        WindowReportKind::CellSizePixels => {
            format!("\x1b[6;{};{}t", px(cell_h), px(cell_w)).into_bytes()
        }
    }
}
