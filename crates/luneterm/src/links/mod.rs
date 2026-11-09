//! Links (plan M10 step 4): finding the link under the pointer and opening it.
//!
//! Two kinds of link are activatable: an OSC 8 hyperlink a program printed, and plain
//! `http(s)://` text ([`scan`]). Hovering one that [`open::check_url`] accepts underlines it and,
//! after the toolkit's tooltip delay, shows where it goes ([`tip_url`]) and how to open it; the
//! pointer becomes a hand while the link modifier is held, and a click with the modifier opens it,
//! after the check has passed it a second time, wherever the string came from. A link the check
//! refuses is not underlined and has no tooltip, so what is shown is what would open.
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

/// The most characters of a URL the tooltip shows.
pub const TIP_MAX_CHARS: usize = 96;
/// The most characters of `scheme://authority` shown whole; a longer one loses its middle.
const TIP_HEAD_MAX: usize = 64;
/// How much of an over-long head is kept at its start (the rest of the budget is its end, where
/// the registered domain is).
const TIP_HEAD_START: usize = 40;

/// The string-table key of the hint line under the tooltip's URL: how to open the link.
pub const fn hint_key(macos: bool) -> &'static str {
    if macos {
        "link.hint.open.mac"
    } else {
        "link.hint.open"
    }
}

/// Every string key the link tooltip asks for.
pub fn required_keys() -> Vec<String> {
    [hint_key(true), hint_key(false)]
        .iter()
        .map(|k| (*k).to_owned())
        .collect()
}

/// What the tooltip shows of `uri`: the URI itself when [`open::check_url`] accepts it, shortened
/// for display, and `None` when it does not (a link that would not open has no tooltip). The check
/// has already refused every control, format and bidirectional character, and a host with
/// non-ASCII text, so nothing of the kind reaches the text, and the host reads as it is: it is
/// never cut at its start, and an over-long one loses only its middle. Only the display is
/// shortened; the open takes the full URI.
pub fn tip_url(uri: &str) -> Option<String> {
    open::check_url(uri).ok()?;
    if uri.chars().count() <= TIP_MAX_CHARS {
        return Some(uri.to_owned());
    }
    // The scheme and the authority are ASCII (the check), so byte offsets are char offsets there.
    let scheme = uri.find("://")? + 3;
    let end = uri[scheme..]
        .find(['/', '?', '#'])
        .map_or(uri.len(), |i| scheme + i);
    let head = &uri[..end];
    if head.len() > TIP_HEAD_MAX {
        let tail = TIP_HEAD_MAX - TIP_HEAD_START - 1;
        return Some(format!(
            "{}\u{2026}{}",
            &head[..TIP_HEAD_START],
            &head[head.len() - tail..]
        ));
    }
    // At least 31 characters of the path or query follow, so the cut is plainly in the path.
    let keep = TIP_MAX_CHARS - 1 - head.len();
    let rest = &uri[end..];
    let cut = rest.char_indices().nth(keep).map_or(rest.len(), |(i, _)| i);
    Some(format!("{head}{}\u{2026}", &rest[..cut]))
}

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

    #[test]
    fn a_short_checked_url_is_shown_whole() {
        assert_eq!(
            tip_url("https://example.com/a?b=c#d").as_deref(),
            Some("https://example.com/a?b=c#d")
        );
        // The scheme's case is the text's own: nothing is rewritten.
        assert_eq!(
            tip_url("HTTP://x.example").as_deref(),
            Some("HTTP://x.example")
        );
        // Non-ASCII is allowed in the path and the query.
        assert_eq!(
            tip_url("https://example.com/caf\u{e9}").as_deref(),
            Some("https://example.com/caf\u{e9}")
        );
    }

    #[test]
    fn a_link_the_check_refuses_has_no_tip() {
        for url in [
            "",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://example.com/",
            "http:\\\\evil",
            "http://user@host/",
            "http://trusted.example@evil.example/",
            "-https://example.com",
            "https://example.com/a b",
            "https://example.com/a\tb",
            "https://example.com/a\nb",
            "https://example.com/a\u{1b}b",
            "https://ex\u{e4}mple.com/",
            "https://example.com/a\"b",
            "http://",
        ] {
            assert_eq!(tip_url(url), None, "{url:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(open::MAX_URL_BYTES));
        assert_eq!(tip_url(&long), None, "over the open limit");
    }

    #[test]
    fn no_control_format_or_bidi_character_reaches_the_tip() {
        // Every character the check calls a format character, in the path, the query and the
        // authority, with the tooltip refusing the whole link each time.
        let mut formats: Vec<char> = Vec::new();
        for range in [
            0x80..=0x9F,
            0xAD..=0xAD,
            0x61C..=0x61C,
            0x200B..=0x200F,
            0x2028..=0x202E,
            0x2060..=0x206F,
            0xFEFF..=0xFEFF,
            0xFFF9..=0xFFFB,
            0xE0000..=0xE007F,
        ] {
            formats.extend(range.filter_map(char::from_u32));
        }
        assert!(formats.contains(&'\u{202E}') && formats.contains(&'\u{2066}'));
        for c in formats {
            for url in [
                format!("https://example.com/a{c}b"),
                format!("https://example.com/?q={c}"),
                format!("https://exam{c}ple.com/"),
                format!("https://example.com/#{c}"),
            ] {
                assert_eq!(tip_url(&url), None, "U+{:04X} in {url:?}", u32::from(c));
            }
        }
        // And what is accepted and shortened has none either, whatever the path holds.
        let path: String = "\u{e9}\u{4f60}x".repeat(60);
        let tip = tip_url(&format!("https://example.com/{path}")).expect("tip");
        assert!(
            tip.chars().all(|c| !c.is_control() && c != '\u{202e}'),
            "{tip:?}"
        );
    }

    #[test]
    fn a_long_url_is_cut_in_the_path_with_the_host_whole() {
        let url = format!("https://docs.example.com:8443/{}", "p/".repeat(100));
        let tip = tip_url(&url).expect("tip");
        assert_eq!(tip.chars().count(), TIP_MAX_CHARS);
        assert!(
            tip.starts_with("https://docs.example.com:8443/p/p/"),
            "{tip}"
        );
        assert!(tip.ends_with('\u{2026}'));
        assert!(url.len() > TIP_MAX_CHARS, "only the display is cut");
    }

    #[test]
    fn a_long_host_loses_its_middle_and_keeps_both_ends() {
        let host = format!("{}.example.com", "label-".repeat(40));
        let url = format!("http://{host}/path");
        let tip = tip_url(&url).expect("tip");
        assert_eq!(tip.chars().count(), TIP_HEAD_MAX);
        assert!(tip.starts_with("http://label-label-"), "{tip}");
        assert!(
            tip.ends_with(".example.com"),
            "the domain is visible: {tip}"
        );
        assert!(tip.contains('\u{2026}'));
        assert!(
            !tip.contains("/path"),
            "the path is not shown past a cut host"
        );
    }

    #[test]
    fn the_hint_names_the_platform_modifier() {
        assert_ne!(hint_key(true), hint_key(false));
        assert_eq!(required_keys().len(), 2);
    }

    // The engine's ids are a `u32` counter from 1; one session would need over two billion
    // distinct hyperlinks to reach the synthetic one.
    const _: () = assert!(SYNTHETIC_LINK > u32::MAX / 2);
}
