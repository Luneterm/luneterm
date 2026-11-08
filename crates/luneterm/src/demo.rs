//! The placeholder body of M9 step 4: a static grid that shows the renderer working before the
//! session is wired in (step 6 replaces it). Frame dumps draw it too.

use lune_termview::{Attrs, Cell, Color, Cursor, CursorShape, Grid, UnderlineStyle};

/// Writes `text` (one cell per char, width 1) at `row`, `col`; returns the next column.
fn put(grid: &mut Grid, row: usize, col: usize, text: &str, fg: Color, attrs: Attrs) -> usize {
    let mut col = col;
    for c in text.chars() {
        let mut cell = Cell::default();
        cell.set_char(c);
        cell.fg = fg;
        cell.attrs = attrs;
        if !grid.set(row, col, cell) {
            break;
        }
        col += 1;
    }
    col
}

/// Writes one wide cluster (a width-2 cell and its spacer); returns the next column.
fn put_wide(grid: &mut Grid, row: usize, col: usize, text: &str) -> usize {
    let mut cell = Cell::default();
    cell.set_text(text);
    cell.width = 2;
    let mut spacer = Cell::default();
    spacer.width = 0;
    if grid.set(row, col, cell) && grid.set(row, col + 1, spacer) {
        col + 2
    } else {
        col
    }
}

/// The demo screen at `cols` x `rows`: a prompt, the 16 palette colours, bold, underline, a wide
/// CJK pair and an emoji.
pub fn demo_grid(cols: usize, rows: usize) -> Grid {
    let mut g = Grid::new(cols, rows);
    let plain = Attrs::NONE;
    let mut c = put(&mut g, 0, 0, "luneterm", Color::Palette(2), Attrs::BOLD);
    c = put(&mut g, 0, c, " ~ % ", Color::Palette(4), plain);
    put(&mut g, 0, c, "ls --color", Color::Default, plain);
    put(
        &mut g,
        1,
        0,
        "Documents  Downloads  Projects",
        Color::Palette(12),
        plain,
    );

    // The 16 palette colours as foreground blocks and as backgrounds.
    let mut c = put(&mut g, 3, 0, "fg ", Color::Default, plain);
    for i in 0..16u8 {
        c = put(&mut g, 3, c, "\u{2588}\u{2588}", Color::Palette(i), plain);
    }
    let mut c = put(&mut g, 4, 0, "bg ", Color::Default, plain);
    for i in 0..16u8 {
        for _ in 0..2 {
            let mut cell = Cell::default();
            cell.bg = Color::Palette(i);
            cell.set_char(' ');
            if g.set(4, c, cell) {
                c += 1;
            }
        }
    }

    let c = put(&mut g, 6, 0, "bold ", Color::Default, Attrs::BOLD);
    let c = put(&mut g, 6, c, "dim ", Color::Default, Attrs::DIM);
    let c = put(&mut g, 6, c, "inverse ", Color::Default, Attrs::INVERSE);
    let c = put(
        &mut g,
        6,
        c,
        "strike ",
        Color::Default,
        Attrs::STRIKETHROUGH,
    );
    let c = put(&mut g, 6, c, "overline", Color::Default, Attrs::OVERLINE);
    let _ = c;

    // Underlines: a cell run per style.
    let mut c = put(&mut g, 7, 0, "underline ", Color::Default, plain);
    for style in [
        UnderlineStyle::Single,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ] {
        for ch in "abcd ".chars() {
            let mut cell = Cell::default();
            cell.set_char(ch);
            cell.underline = style;
            if g.set(7, c, cell) {
                c += 1;
            }
        }
    }

    let c = put(&mut g, 9, 0, "wide ", Color::Default, plain);
    let c = put_wide(&mut g, 9, c, "\u{4F60}");
    let c = put_wide(&mut g, 9, c, "\u{597D}");
    let c = put(&mut g, 9, c, "  emoji ", Color::Default, plain);
    let _ = put_wide(&mut g, 9, c, "\u{1F642}");

    put(&mut g, 11, 0, "luneterm", Color::Palette(2), Attrs::BOLD);
    put(&mut g, 11, 8, " ~ % ", Color::Palette(4), plain);
    g
}

/// The demo cursor: a block after the last prompt. It does not blink, so an idle window stays idle.
pub fn demo_cursor() -> Cursor {
    Cursor {
        row: 11,
        col: 13,
        shape: CursorShape::Block,
        blink: false,
        visible: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_has_a_wide_cell_with_its_spacer_and_fits_small_grids() {
        let g = demo_grid(80, 24);
        let (row, col) = (0..g.rows())
            .find_map(|r| {
                g.row(r)?
                    .iter()
                    .position(|c| c.text() == "\u{4F60}")
                    .map(|c| (r, c))
            })
            .expect("a CJK cell");
        assert_eq!(g.get(row, col).map(|c| c.width), Some(2));
        assert_eq!(g.get(row, col + 1).map(|c| c.width), Some(0));
        // A grid smaller than the demo is cut, not a panic.
        let small = demo_grid(5, 3);
        assert_eq!((small.cols(), small.rows()), (5, 3));
        let empty = demo_grid(0, 0);
        assert_eq!((empty.cols(), empty.rows()), (0, 0));
    }
}
