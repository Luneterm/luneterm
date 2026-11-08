//! Links (plan M10 step 4): finding the link under the pointer and opening it.
//!
//! Two kinds of link are activatable: an OSC 8 hyperlink a program printed, and plain
//! `http(s)://` text ([`scan`]). Hovering one with the link modifier held underlines it; a click
//! with the modifier opens it, after [`open::check_url`] has passed it a second time, wherever the
//! string came from.
//!
//! The scan runs on demand, on the one row under the pointer, not on every changed row: a program
//! that prints a flood pays nothing for links it prints. The termview underlines the cells whose
//! `link` equals the hovered id, so a bare URL's cells carry [`SYNTHETIC_LINK`] while it is
//! hovered (an id the engine never hands out: its ids count up from 1) and get their own back
//! after.

pub mod open;
pub mod scan;

use lune_termview::Grid;

pub use scan::{LinkKind, LinkSpan, MAX_SPANS_PER_ROW, UriTable, scan_row};

/// The link id of a hovered bare URL's cells. The engine's ids count up from 1 and never get near
/// this.
pub const SYNTHETIC_LINK: u32 = 1 << 31;

/// The most OSC 8 URIs a tab remembers. Past it the table starts over: the ids still on screen
/// then have no URI and are not links (fail closed) until the engine says them again.
pub const MAX_URI_TABLE: usize = 1 << 17;

/// The link under cell (`row`, `col`) of `grid`, if there is one.
pub fn link_at(grid: &Grid, row: usize, col: usize, uris: &UriTable) -> Option<LinkSpan> {
    let cells = grid.row(row)?;
    let mut spans = Vec::new();
    scan_row(cells, row, uris, &mut spans);
    spans.into_iter().find(|s| s.contains(col))
}

/// A link shown as hovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub span: LinkSpan,
    /// A bare URL's cells before they carried [`SYNTHETIC_LINK`].
    saved: Vec<u32>,
}

impl Hover {
    /// Shows `span` as hovered on `grid`; the id the termview should underline.
    pub fn show(span: LinkSpan, grid: &mut Grid) -> (Hover, u32) {
        let mut saved = Vec::new();
        let id = match span.kind {
            LinkKind::Osc8(id) => id,
            LinkKind::BareUrl => {
                if let Some(cells) = grid.row_mut(span.row) {
                    for cell in cells
                        .iter_mut()
                        .skip(span.start)
                        .take(span.end + 1 - span.start)
                    {
                        saved.push(cell.link);
                        cell.link = SYNTHETIC_LINK;
                    }
                }
                SYNTHETIC_LINK
            }
        };
        (Hover { span, saved }, id)
    }

    /// Puts the cells back. A row the engine rewrote since no longer carries the id and is left
    /// alone.
    pub fn hide(self, grid: &mut Grid) {
        if self.span.kind != LinkKind::BareUrl {
            return;
        }
        if let Some(cells) = grid.row_mut(self.span.row) {
            for (cell, saved) in cells.iter_mut().skip(self.span.start).zip(&self.saved) {
                if cell.link == SYNTHETIC_LINK {
                    cell.link = *saved;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lune_termview::Cell;

    fn grid_with(text: &str, cols: usize) -> Grid {
        let mut grid = Grid::new(cols, 2);
        for (x, c) in text.chars().enumerate() {
            let mut cell = Cell::default();
            cell.set_char(c);
            grid.set(0, x, cell);
        }
        grid
    }

    #[test]
    fn the_link_under_a_cell_is_found_by_column() {
        let grid = grid_with("go https://example.com now", 40);
        let uris = UriTable::new();
        assert_eq!(link_at(&grid, 0, 2, &uris), None);
        let hit = link_at(&grid, 0, 3, &uris).expect("link");
        assert_eq!((hit.start, hit.end), (3, 21));
        assert_eq!(hit.uri, "https://example.com");
        assert!(link_at(&grid, 0, 21, &uris).is_some());
        assert_eq!(link_at(&grid, 0, 22, &uris), None);
        assert_eq!(link_at(&grid, 1, 4, &uris), None, "another row");
        assert_eq!(link_at(&grid, 9, 4, &uris), None, "no such row");
    }

    #[test]
    fn a_hovered_bare_url_carries_the_synthetic_id_and_gets_its_own_back() {
        let mut grid = grid_with("go https://example.com now", 40);
        grid.row_mut(0).expect("row")[6].link = 42;
        let span = link_at(&grid, 0, 5, &UriTable::new()).expect("link");
        let (hover, id) = Hover::show(span.clone(), &mut grid);
        assert_eq!(id, SYNTHETIC_LINK);
        let row = grid.row(0).expect("row");
        assert!(
            row[span.start..=span.end]
                .iter()
                .all(|c| c.link == SYNTHETIC_LINK)
        );
        assert_eq!(row[span.start - 1].link, 0);
        assert_eq!(row[span.end + 1].link, 0);
        hover.hide(&mut grid);
        let row = grid.row(0).expect("row");
        assert_eq!(row[6].link, 42, "its own id is back");
        assert_eq!(row[5].link, 0);
    }

    #[test]
    fn hiding_leaves_a_row_the_engine_rewrote() {
        let mut grid = grid_with("https://example.com x", 40);
        let span = link_at(&grid, 0, 0, &UriTable::new()).expect("link");
        let (hover, _) = Hover::show(span, &mut grid);
        // The engine's update rewrites the row: its cells carry their own ids again.
        for cell in grid.row_mut(0).expect("row") {
            cell.link = 9;
        }
        hover.hide(&mut grid);
        assert!(grid.row(0).expect("row").iter().all(|c| c.link == 9));
    }

    #[test]
    fn an_osc8_hover_underlines_by_the_engines_id_and_changes_no_cell() {
        let mut grid = grid_with("click", 40);
        for cell in grid.row_mut(0).expect("row").iter_mut().take(5) {
            cell.link = 7;
        }
        let mut uris = UriTable::new();
        uris.insert(7, "https://example.com".to_owned());
        let span = link_at(&grid, 0, 2, &uris).expect("link");
        assert_eq!(span.kind, LinkKind::Osc8(7));
        let (hover, id) = Hover::show(span, &mut grid);
        assert_eq!(id, 7);
        hover.hide(&mut grid);
        assert!(
            grid.row(0)
                .expect("row")
                .iter()
                .take(5)
                .all(|c| c.link == 7)
        );
    }

    // The engine's ids are a `u32` counter from 1; one session would need over two billion
    // distinct hyperlinks to reach the synthetic one.
    const _: () = assert!(SYNTHETIC_LINK > u32::MAX / 2);
}
