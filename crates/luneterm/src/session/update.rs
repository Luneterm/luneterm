//! What the engine thread hands the UI (plan M9 step 3 `Update`) and the UI-side grid mirror
//! (`apply_update`, invariant I7).

use std::fmt;
use std::sync::Arc;

use lune_pty::RawMode;
use lune_termview::{Attrs, Cell, Color, Grid, UnderlineStyle};
use lune_vt::{
    CellColor, CommandMark, MouseEncoding, MouseTracking, Progress, UnderlineStyle as VtUnderline,
    ViewportCell, ViewportCursor,
};

use crate::outline::CommandState;

use super::{CopyResult, HostPalette, SelectionView};

/// The modes the UI routes input by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateModes {
    pub mouse_tracking: MouseTracking,
    /// The report encoding; `SgrPixels` (`?1016`) makes a move inside one cell news.
    pub mouse_encoding: MouseEncoding,
    pub application_cursor_keys: bool,
    pub bracketed_paste: bool,
    pub synchronized_output: bool,
}

/// The child's end of output, reported once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// `None` when the exit status was still unknown 1 s after the output ended.
    pub code: Option<i32>,
}

/// What the engine thread's probes of the PTY last saw (plan M10 D3). A session starts at the
/// default (nothing known); an Update carries a new value only when it differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ForegroundState {
    /// Whether the shell (the child the session spawned) is the PTY's foreground process;
    /// `None` when the platform cannot say (Windows) or a probe failed.
    pub shell_is_foreground: Option<bool>,
    /// The PTY's line discipline.
    pub raw: RawMode,
}

/// One published change set. Apply them in order with [`apply_update`].
#[derive(Clone, PartialEq)]
pub struct Update {
    /// 1 for the first Update of a session, then +1 per Update.
    pub seq: u64,
    pub cols: usize,
    pub rows: usize,
    /// `rows_changed` holds every viewport row.
    pub full: bool,
    /// Viewport rows that changed, each `cols` cells.
    pub rows_changed: Vec<(usize, Vec<Cell>)>,
    pub cursor: ViewportCursor,
    pub viewport_top: usize,
    pub base_y: usize,
    pub total_lines: usize,
    pub is_at_bottom: bool,
    pub alt_screen: bool,
    pub modes: UpdateModes,
    /// The window title, only when it changed (shared with the engine, not copied).
    pub title: Option<Arc<str>>,
    /// BELs since the last Update.
    pub bells: u32,
    /// The last OSC 52 write since the last Update.
    pub clipboard_write: Option<String>,
    /// The session palette, when a colour set or restore changed it.
    pub palette: Option<HostPalette>,
    /// The last OSC 7 / OSC 9;9 directory (unvalidated).
    pub cwd: Option<String>,
    /// The last OSC 9;4 report.
    pub progress: Option<Progress>,
    /// OSC 133 marks in order, at most [`MARKS_CAP`] (the oldest are dropped).
    pub marks: Vec<CommandMark>,
    pub sync_update_in_command: bool,
    /// The PTY probes, only when they changed.
    pub foreground: Option<ForegroundState>,
    /// The command tracking, only when it changed.
    pub command: Option<CommandState>,
    /// `(id, uri)` of every OSC 8 link id appearing in `rows_changed` that no earlier Update of
    /// this session carried, at most [`LINK_URIS_CAP`]; a uri over [`MAX_LINK_URI`] bytes is
    /// never carried (that id stays unopenable). Untrusted text: check before use.
    pub link_uris: Vec<(u32, String)>,
    /// The selection, only when it changed: `Some(None)` is "nothing selected now".
    pub selection: Option<Option<SelectionView>>,
    /// The text a `Command::CopySelection` asked for (or why there is none); never in a `Debug`.
    pub selection_text: Option<CopyResult>,
    pub exited: Option<Exit>,
}

impl fmt::Debug for Update {
    /// Sizes, positions and counts only: never cell text, the title, clipboard text or a
    /// directory (I16).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Update")
            .field("seq", &self.seq)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("full", &self.full)
            .field("rows_changed", &self.rows_changed.len())
            .field("viewport_top", &self.viewport_top)
            .field("base_y", &self.base_y)
            .field("total_lines", &self.total_lines)
            .field("is_at_bottom", &self.is_at_bottom)
            .field("alt_screen", &self.alt_screen)
            .field("modes", &self.modes)
            .field("title", &self.title.is_some())
            .field("bells", &self.bells)
            .field("clipboard_write", &self.clipboard_write.is_some())
            .field("palette", &self.palette.is_some())
            .field("cwd", &self.cwd.is_some())
            .field("marks", &self.marks.len())
            .field("foreground", &self.foreground)
            .field("command", &self.command)
            .field("link_uris", &self.link_uris.len())
            .field("selection", &self.selection)
            .field("selection_text", &self.selection_text)
            .field("exited", &self.exited)
            .finish_non_exhaustive()
    }
}

/// At most this many marks per Update.
pub const MARKS_CAP: usize = 64;
/// At most this many link uris per Update; the rest follow in the next ones.
pub const LINK_URIS_CAP: usize = 256;
/// A link uri longer than this (bytes) is not published.
pub const MAX_LINK_URI: usize = 2048;

/// Brings `grid` to the screen `update` was built from: resizes on a size change, then sets the
/// changed rows and marks them dirty.
pub fn apply_update(grid: &mut Grid, update: &Update) {
    if grid.cols() != update.cols || grid.rows() != update.rows {
        grid.resize(update.cols, update.rows);
    }
    for (y, cells) in &update.rows_changed {
        if let Some(row) = grid.row_mut(*y) {
            for (dst, src) in row.iter_mut().zip(cells) {
                dst.clone_from(src);
            }
        }
    }
}

pub(crate) fn to_cell(cell: &ViewportCell) -> Cell {
    let mut attrs = Attrs::NONE;
    for (on, bit) in [
        (cell.bold, Attrs::BOLD),
        (cell.dim, Attrs::DIM),
        (cell.italic, Attrs::ITALIC),
        (cell.blink, Attrs::BLINK),
        (cell.inverse, Attrs::INVERSE),
        (cell.invisible, Attrs::INVISIBLE),
        (cell.strikethrough, Attrs::STRIKETHROUGH),
        (cell.overline, Attrs::OVERLINE),
    ] {
        if on {
            attrs.insert(bit);
        }
    }
    let mut out = Cell::default();
    out.width = cell.width;
    out.fg = to_color(cell.fg);
    out.bg = to_color(cell.bg);
    out.attrs = attrs;
    out.underline = match cell.underline {
        VtUnderline::None => UnderlineStyle::None,
        VtUnderline::Single => UnderlineStyle::Single,
        VtUnderline::Double => UnderlineStyle::Double,
        VtUnderline::Curly => UnderlineStyle::Curly,
        VtUnderline::Dotted => UnderlineStyle::Dotted,
        VtUnderline::Dashed => UnderlineStyle::Dashed,
    };
    out.underline_color = to_color(cell.underline_color);
    out.link = cell.link_id;
    out.set_text(&cell.text);
    out
}

fn to_color(c: CellColor) -> Color {
    match c {
        CellColor::Default => Color::Default,
        CellColor::Palette(i) => Color::Palette(i),
        CellColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
