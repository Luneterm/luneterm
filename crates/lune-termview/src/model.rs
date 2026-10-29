//! Engine-agnostic cell grid (plan decision D2).
//!
//! A wide character is a width-2 leading cell followed by a width-0 spacer (xterm.js convention).
//! Cluster segmentation and widths are the producer's job; nothing here computes Unicode widths.
//! No public method panics: out-of-range arguments give `None`, `false` or a no-op.

use std::ops::{BitOr, BitOrAssign, Range};

/// A cell colour as the terminal stored it. Resolution to RGBA happens in `theme::resolve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Palette(u8),
    Rgb(u8, u8, u8),
}

/// Text attribute bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attrs(pub u16);

impl Attrs {
    pub const NONE: Attrs = Attrs(0);
    pub const BOLD: Attrs = Attrs(1);
    pub const DIM: Attrs = Attrs(1 << 1);
    pub const ITALIC: Attrs = Attrs(1 << 2);
    pub const BLINK: Attrs = Attrs(1 << 3);
    pub const INVERSE: Attrs = Attrs(1 << 4);
    pub const INVISIBLE: Attrs = Attrs(1 << 5);
    pub const STRIKETHROUGH: Attrs = Attrs(1 << 6);
    pub const OVERLINE: Attrs = Attrs(1 << 7);

    /// True when every bit of `other` is set.
    pub const fn contains(self, other: Attrs) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Attrs) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Attrs) {
        self.0 &= !other.0;
    }
}

impl BitOr for Attrs {
    type Output = Attrs;
    fn bitor(self, rhs: Attrs) -> Attrs {
        Attrs(self.0 | rhs.0)
    }
}

impl BitOrAssign for Attrs {
    fn bitor_assign(&mut self, rhs: Attrs) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Cell text: one scalar (or any string up to four bytes) stays inline; longer clusters box.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CellText {
    Inline { len: u8, buf: [u8; 4] },
    Heap(Box<str>),
}

impl CellText {
    const EMPTY: CellText = CellText::Inline {
        len: 0,
        buf: [0; 4],
    };

    fn new(s: &str) -> CellText {
        let b = s.as_bytes();
        if b.len() <= 4 {
            let mut buf = [0u8; 4];
            buf[..b.len()].copy_from_slice(b);
            CellText::Inline {
                len: b.len() as u8,
                buf,
            }
        } else {
            CellText::Heap(s.into())
        }
    }

    fn as_str(&self) -> &str {
        match self {
            // The bytes were copied from a `&str` at a char boundary, so this never falls back.
            CellText::Inline { len, buf } => {
                std::str::from_utf8(&buf[..usize::from(*len)]).unwrap_or("")
            }
            CellText::Heap(s) => s,
        }
    }
}

/// One grid cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// Columns this cell covers: 0 = spacer after a wide cell, 1, or 2.
    pub width: u8,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub underline: UnderlineStyle,
    /// `Default` = use the foreground.
    pub underline_color: Color,
    /// Hyperlink id; 0 = none.
    pub link: u32,
    text: CellText,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            width: 1,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::NONE,
            underline: UnderlineStyle::None,
            underline_color: Color::Default,
            link: 0,
            text: CellText::EMPTY,
        }
    }
}

impl Cell {
    /// The cluster to draw; "" = blank.
    pub fn text(&self) -> &str {
        self.text.as_str()
    }

    pub fn set_text(&mut self, s: &str) {
        self.text = CellText::new(s);
    }

    pub fn set_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        let n = c.encode_utf8(&mut buf).len();
        self.text = CellText::Inline { len: n as u8, buf };
    }
}

/// The cell grid plus per-row dirty flags.
#[derive(Debug, Clone)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    dirty: Vec<bool>,
}

impl Grid {
    /// All cells blank, all rows dirty.
    pub fn new(cols: usize, rows: usize) -> Grid {
        Grid {
            cols,
            rows,
            cells: vec![Cell::default(); cols.saturating_mul(rows)],
            dirty: vec![true; rows],
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Keeps the top-left overlap; new cells are blank; all rows become dirty.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let mut cells = vec![Cell::default(); cols.saturating_mul(rows)];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                cells[r * cols + c] = std::mem::take(&mut self.cells[r * self.cols + c]);
            }
        }
        self.cells = cells;
        self.cols = cols;
        self.rows = rows;
        self.dirty = vec![true; rows];
    }

    pub fn get(&self, row: usize, col: usize) -> Option<&Cell> {
        if row < self.rows && col < self.cols {
            self.cells.get(row * self.cols + col)
        } else {
            None
        }
    }

    pub fn row(&self, row: usize) -> Option<&[Cell]> {
        if row < self.rows {
            self.cells.get(row * self.cols..(row + 1) * self.cols)
        } else {
            None
        }
    }

    /// Mutable row access; marks the row dirty.
    pub fn row_mut(&mut self, row: usize) -> Option<&mut [Cell]> {
        if row < self.rows {
            self.dirty[row] = true;
            self.cells.get_mut(row * self.cols..(row + 1) * self.cols)
        } else {
            None
        }
    }

    /// Replaces a cell and marks its row dirty; false when out of range.
    pub fn set(&mut self, row: usize, col: usize, cell: Cell) -> bool {
        if row < self.rows && col < self.cols {
            self.cells[row * self.cols + col] = cell;
            self.dirty[row] = true;
            true
        } else {
            false
        }
    }

    /// Blanks every cell and marks all rows dirty.
    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
        self.mark_all_dirty();
    }

    pub fn mark_dirty(&mut self, row: usize) {
        if let Some(d) = self.dirty.get_mut(row) {
            *d = true;
        }
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty.fill(true);
    }

    pub fn is_dirty(&self, row: usize) -> bool {
        self.dirty.get(row).copied().unwrap_or(false)
    }

    pub fn dirty_rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.dirty
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| d.then_some(i))
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.fill(false);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    /// The cursor blinks; it is hidden while `ViewState::blink_on` is false.
    pub blink: bool,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            blink: false,
            visible: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Linear,
    Block,
}

/// A selection as (row, col) corners, end inclusive, in either order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub kind: SelectionKind,
}

impl SelectionRange {
    /// Selected columns of `row` in a grid `cols` wide; `None` when the row is outside the
    /// selection or the clamped range is empty.
    pub fn cols_in_row(&self, row: usize, cols: usize) -> Option<Range<usize>> {
        let (lo, hi) = match self.kind {
            SelectionKind::Linear => {
                let (a, b) = if self.start <= self.end {
                    (self.start, self.end)
                } else {
                    (self.end, self.start)
                };
                if row < a.0 || row > b.0 {
                    return None;
                }
                let lo = if row == a.0 { a.1 } else { 0 };
                let hi = if row == b.0 {
                    b.1.saturating_add(1)
                } else {
                    cols
                };
                (lo, hi)
            }
            SelectionKind::Block => {
                let (r0, r1) = (self.start.0.min(self.end.0), self.start.0.max(self.end.0));
                if row < r0 || row > r1 {
                    return None;
                }
                let (c0, c1) = (self.start.1.min(self.end.1), self.start.1.max(self.end.1));
                (c0, c1.saturating_add(1))
            }
        };
        let (lo, hi) = (lo.min(cols), hi.min(cols));
        (lo < hi).then_some(lo..hi)
    }
}

/// Per-frame state the app supplies alongside the grid.
#[derive(Debug, Clone, Copy)]
pub struct ViewState<'a> {
    pub cursor: Cursor,
    pub focused: bool,
    /// Blink phase, owned by the app (this crate reads no clock).
    pub blink_on: bool,
    pub selection: &'a [SelectionRange],
    /// Link id under the pointer; 0 = none.
    pub hover_link: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_out_of_range_is_false_and_leaves_dirty_alone() {
        let mut g = Grid::new(4, 3);
        g.clear_dirty();
        assert!(!g.set(3, 0, Cell::default()));
        assert!(!g.set(0, 4, Cell::default()));
        assert_eq!(g.dirty_rows().count(), 0);
        assert!(g.set(1, 2, Cell::default()));
        assert_eq!(g.dirty_rows().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn resize_keeps_overlap_and_dirties_all() {
        let mut g = Grid::new(4, 3);
        let mut c = Cell::default();
        c.set_char('x');
        assert!(g.set(0, 0, c));
        g.clear_dirty();
        g.resize(6, 2);
        assert_eq!(g.get(0, 0).map(Cell::text), Some("x"));
        assert_eq!(g.get(1, 5).map(Cell::text), Some(""));
        assert_eq!(g.dirty_rows().count(), 2);
        assert!(g.get(2, 0).is_none());
    }

    #[test]
    fn row_mut_out_of_range_is_none() {
        let mut g = Grid::new(4, 3);
        assert!(g.row_mut(5).is_none());
        assert!(g.row(3).is_none());
        assert!(g.row_mut(2).is_some());
    }

    #[test]
    fn zero_by_zero_grid_is_valid() {
        let mut g = Grid::new(0, 0);
        assert!(g.get(0, 0).is_none());
        assert!(g.row(0).is_none());
        assert!(!g.set(0, 0, Cell::default()));
        g.clear();
        g.mark_dirty(0);
        assert!(!g.is_dirty(0));
        g.resize(2, 2);
        assert_eq!(g.dirty_rows().count(), 2);
    }

    #[test]
    fn cell_text_inline_and_boxed() {
        let mut c = Cell::default();
        assert_eq!(c.text(), "");
        c.set_char('é');
        assert_eq!(c.text(), "é");
        assert!(matches!(c.text, CellText::Inline { .. }));
        c.set_char('🎨');
        assert_eq!(c.text(), "🎨");
        assert!(matches!(c.text, CellText::Inline { .. }));
        c.set_text("👍🏽");
        assert_eq!(c.text(), "👍🏽");
        assert!(matches!(c.text, CellText::Heap(_)));
        assert_eq!(Cell::default().width, 1);
    }

    #[test]
    fn attrs_ops() {
        let mut a = Attrs::BOLD | Attrs::DIM;
        assert!(a.contains(Attrs::BOLD) && a.contains(Attrs::DIM));
        a.remove(Attrs::BOLD);
        assert!(!a.contains(Attrs::BOLD));
        a.insert(Attrs::OVERLINE);
        assert!(a.contains(Attrs::DIM | Attrs::OVERLINE));
    }

    fn lin(s: (usize, usize), e: (usize, usize)) -> SelectionRange {
        SelectionRange {
            start: s,
            end: e,
            kind: SelectionKind::Linear,
        }
    }

    #[test]
    fn linear_selection_columns() {
        for sel in [lin((2, 5), (4, 3)), lin((4, 3), (2, 5))] {
            assert_eq!(sel.cols_in_row(2, 10), Some(5..10));
            assert_eq!(sel.cols_in_row(3, 10), Some(0..10));
            assert_eq!(sel.cols_in_row(4, 10), Some(0..4));
            assert_eq!(sel.cols_in_row(1, 10), None);
            assert_eq!(sel.cols_in_row(5, 10), None);
        }
        assert_eq!(lin((1, 2), (1, 4)).cols_in_row(1, 10), Some(2..5));
        assert_eq!(lin((1, 8), (1, 20)).cols_in_row(1, 10), Some(8..10));
        assert_eq!(lin((1, 12), (1, 20)).cols_in_row(1, 10), None);
    }

    #[test]
    fn block_selection_columns() {
        let sel = SelectionRange {
            start: (4, 7),
            end: (2, 3),
            kind: SelectionKind::Block,
        };
        assert_eq!(sel.cols_in_row(3, 20), Some(3..8));
        assert_eq!(sel.cols_in_row(2, 20), Some(3..8));
        assert_eq!(sel.cols_in_row(5, 20), None);
        assert_eq!(sel.cols_in_row(3, 5), Some(3..5));
    }
}
