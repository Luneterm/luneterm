//! The link scanner (plan M10 step 4): the activatable links of one viewport row.
//!
//! A port of Lunegit's `TerminalLinkScanner.cs` over this app's mirror cells. Two sources: OSC 8
//! hyperlinks (the cells carry the engine's link id, and the app has the id's URI) and bare
//! `http(s)://` text. A bare match that overlaps an OSC 8 span is dropped (the program said what it
//! meant). The scanner reads one cell at a time and never goes back: linear in the row's length,
//! no allocation per cell, and at most [`MAX_SPANS_PER_ROW`] spans.
//!
//! Every span's URI passed [`check_url`]; one that fails it is not a span at all (no underline, no
//! click). The check runs again when a link is opened.
//!
//! Differences from Lunegit, each on purpose:
//!  - the span cap is per row, not per snapshot (the scanner runs on the row under the pointer);
//!  - the check is the closed grammar of [`super::open`], not `Uri.TryCreate`;
//!  - a blank cell is a space, and a cell whose text is not one character of the basic plane
//!    (a wide character, an emoji, a combined mark) ends a match, as Lunegit's `Glyph` length test.

use std::collections::HashMap;

use lune_termview::Cell;

use super::open::check_url;

/// The most spans one row gives.
pub const MAX_SPANS_PER_ROW: usize = 256;

/// Where a span comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// An OSC 8 hyperlink: the engine's link id.
    Osc8(u32),
    /// Plain text that reads as a URL.
    BareUrl,
}

/// One link on a row: columns `start..=end` (inclusive) and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub uri: String,
    pub kind: LinkKind,
}

impl LinkSpan {
    pub fn contains(&self, col: usize) -> bool {
        (self.start..=self.end).contains(&col)
    }
}

/// The URI table of the OSC 8 ids the engine has published.
pub type UriTable = HashMap<u32, String>;

const HTTP: &str = "http://";
const HTTPS: &str = "https://";

/// The one character of a cell as the scanner sees it: a blank is a space, text of exactly one
/// basic-plane character is that character, anything else is `None` (it ends a match).
fn single_char(cell: &Cell) -> Option<char> {
    let text = cell.text();
    if text.is_empty() {
        return Some(' ');
    }
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if u32::from(c) < 0x1_0000 => Some(c),
        _ => None,
    }
}

/// The characters a bare URL's body is made of (`IsUrlBodyChar`).
pub fn is_url_body_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}

fn count(s: &str, c: char) -> usize {
    s.chars().filter(|x| *x == c).count()
}

/// Strips trailing sentence punctuation from a matched URL, repeatedly (`TrimTrailingPunctuation`).
/// A closing bracket goes only when unbalanced against its opener, so `.../Foo_(bar)` keeps its `)`
/// and the `)` of `(see https://example.com)` goes.
pub fn trim_trailing_punctuation(candidate: &str) -> &str {
    let mut end = candidate.len();
    loop {
        let head = &candidate[..end];
        let Some(last) = head.chars().next_back() else {
            return head;
        };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => count(head, ')') > count(head, '('),
            ']' => count(head, ']') > count(head, '['),
            '}' => count(head, '}') > count(head, '{'),
            _ => false,
        };
        if !strip {
            return head;
        }
        end -= last.len_utf8();
    }
}

/// Appends the OSC 8 spans of `cells`: runs of one non-zero link id with a URI that passes the
/// check.
fn scan_osc8(cells: &[Cell], row: usize, uris: &UriTable, out: &mut Vec<LinkSpan>) {
    if uris.is_empty() {
        return;
    }
    let flush = |id: u32, start: usize, end: usize, out: &mut Vec<LinkSpan>| {
        if id == 0 || out.len() >= MAX_SPANS_PER_ROW {
            return;
        }
        if let Some(uri) = uris.get(&id)
            && check_url(uri).is_ok()
        {
            out.push(LinkSpan {
                row,
                start,
                end,
                uri: uri.clone(),
                kind: LinkKind::Osc8(id),
            });
        }
    };
    let (mut run_id, mut run_start) = (0u32, 0usize);
    for (x, cell) in cells.iter().enumerate() {
        if cell.link != run_id {
            if x > 0 {
                flush(run_id, run_start, x - 1, out);
            }
            run_id = cell.link;
            run_start = x;
        }
    }
    if !cells.is_empty() {
        flush(run_id, run_start, cells.len() - 1, out);
    }
}

/// How many cells from `start` spell `prefix` (ASCII case-insensitive), or 0.
fn match_prefix(cells: &[Cell], start: usize, prefix: &str) -> usize {
    let Some(window) = cells.get(start..start + prefix.len()) else {
        return 0;
    };
    let same = window
        .iter()
        .zip(prefix.chars())
        .all(|(cell, p)| single_char(cell).is_some_and(|c| c.to_ascii_lowercase() == p));
    if same { prefix.len() } else { 0 }
}

/// The length of the bare URL match at `start` (scheme and at least one body character), or 0.
fn match_bare_url(cells: &[Cell], start: usize) -> usize {
    let mut prefix = match_prefix(cells, start, HTTPS);
    if prefix == 0 {
        prefix = match_prefix(cells, start, HTTP);
    }
    if prefix == 0 {
        return 0;
    }
    let body = start + prefix;
    let mut x = body;
    while x < cells.len() && single_char(&cells[x]).is_some_and(is_url_body_char) {
        x += 1;
    }
    if x > body { x - start } else { 0 }
}

fn overlaps_osc8(out: &[LinkSpan], from: usize, start: usize, end: usize) -> bool {
    out[from..]
        .iter()
        .any(|s| matches!(s.kind, LinkKind::Osc8(_)) && !(end < s.start || start > s.end))
}

fn scan_bare(cells: &[Cell], row: usize, out: &mut Vec<LinkSpan>, osc8_from: usize) {
    let last_col = cells.len().saturating_sub(1);
    let mut x = 0;
    while x < cells.len() {
        if out.len() >= MAX_SPANS_PER_ROW {
            return;
        }
        if !single_char(&cells[x]).is_some_and(|c| c == 'h' || c == 'H') {
            x += 1;
            continue;
        }
        // A boundary before the scheme: the start of the row or a character a URL does not hold.
        if x > 0 && single_char(&cells[x - 1]).is_some_and(is_url_body_char) {
            x += 1;
            continue;
        }
        let len = match_bare_url(cells, x);
        if len == 0 {
            x += 1;
            continue;
        }
        let raw: String = cells[x..x + len].iter().filter_map(single_char).collect();
        let trimmed = trim_trailing_punctuation(&raw);
        if trimmed.is_empty() {
            x += len;
            continue;
        }
        // A match that reaches the last column may be a URL that wraps onto the next row:
        // opening the first half lands somewhere else. Judged on the raw match, before the trim.
        if x + len > last_col {
            x += len;
            continue;
        }
        let end = x + trimmed.chars().count() - 1;
        if check_url(trimmed).is_ok() && !overlaps_osc8(out, osc8_from, x, end) {
            out.push(LinkSpan {
                row,
                start: x,
                end,
                uri: trimmed.to_owned(),
                kind: LinkKind::BareUrl,
            });
        }
        x += len;
    }
}

/// Appends every link span of `cells` (viewport row `row`) to `out`: OSC 8 spans first, then
/// the bare URLs that do not overlap them.
pub fn scan_row(cells: &[Cell], row: usize, uris: &UriTable, out: &mut Vec<LinkSpan>) {
    let from = out.len();
    scan_osc8(cells, row, uris, out);
    scan_bare(cells, row, out, from);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of `width` cells (the text's length when 0), blank after the text.
    fn row(text: &str, width: usize) -> Vec<Cell> {
        let width = if width == 0 {
            text.chars().count()
        } else {
            width
        };
        let mut chars = text.chars();
        (0..width)
            .map(|_| {
                let mut cell = Cell::default();
                cell.set_char(chars.next().unwrap_or(' '));
                cell
            })
            .collect()
    }

    fn with_links(text: &str, ids: &[u32]) -> Vec<Cell> {
        text.chars()
            .zip(ids)
            .map(|(c, id)| {
                let mut cell = Cell::default();
                cell.set_char(c);
                cell.link = *id;
                cell
            })
            .collect()
    }

    fn scan(cells: &[Cell], uris: &UriTable) -> Vec<LinkSpan> {
        let mut out = Vec::new();
        scan_row(cells, 3, uris, &mut out);
        out
    }

    fn scan_text(text: &str, width: usize) -> Vec<LinkSpan> {
        scan(&row(text, width), &UriTable::new())
    }

    // ---- bare URL grammar (TerminalLinkScannerTests.cs)

    #[test]
    fn a_bare_url_mid_row_has_exact_inclusive_columns() {
        let spans = scan_text("see https://example.com now", 40);
        let [s] = spans.as_slice() else {
            panic!("{spans:?}")
        };
        assert_eq!(s.kind, LinkKind::BareUrl);
        assert_eq!(s.uri, "https://example.com");
        assert_eq!(
            (s.start, s.end, s.row),
            (4, 4 + "https://example.com".len() - 1, 3)
        );
    }

    #[test]
    fn the_prefix_is_case_insensitive() {
        for text in [
            "https://example.com",
            "HTTPS://example.com",
            "HtTp://example.com",
        ] {
            assert_eq!(scan_text(text, text.len() + 5).len(), 1, "{text}");
        }
    }

    #[test]
    fn a_scheme_inside_a_word_does_not_match() {
        assert!(scan_text("xhttps://example.com", 40).is_empty());
    }

    #[test]
    fn a_scheme_with_no_body_is_not_a_link() {
        assert!(scan_text("go https:// now", 40).is_empty());
    }

    #[test]
    fn trailing_punctuation_is_trimmed() {
        for (text, want) in [
            ("(see https://example.com)", "https://example.com"),
            ("end https://example.com.", "https://example.com"),
            ("end https://example.com,", "https://example.com"),
            ("q https://example.com\"", "https://example.com"),
            (
                "x https://en.wikipedia.org/wiki/Foo_(bar)",
                "https://en.wikipedia.org/wiki/Foo_(bar)",
            ),
        ] {
            let spans = scan_text(text, text.len() + 5);
            assert_eq!(spans.first().map(|s| s.uri.as_str()), Some(want), "{text}");
            assert_eq!(spans.len(), 1, "{text}");
        }
    }

    #[test]
    fn terminator_characters_end_the_match() {
        for (text, want) in [
            ("a https://example.com<b", "https://example.com"),
            ("a https://exam ple.com", "https://exam"),
            ("a https://exam\"ple.com", "https://exam"),
        ] {
            let spans = scan_text(text, text.len() + 5);
            assert_eq!(spans[0].uri, want, "{text}");
        }
    }

    #[test]
    fn a_non_ascii_glyph_ends_the_match() {
        let mut cells = row("https://exam", 0);
        let mut wide = Cell::default();
        wide.set_char('界');
        cells.push(wide);
        cells.extend(row("x          ", 0));
        let spans = scan(&cells, &UriTable::new());
        assert_eq!(
            spans.iter().map(|s| s.uri.as_str()).collect::<Vec<_>>(),
            ["https://exam"]
        );
    }

    #[test]
    fn a_match_running_to_the_last_column_may_wrap_so_it_is_not_emitted() {
        let url = "https://example.com/some/long/path";
        assert!(scan_text(url, url.len()).is_empty());
        // Judged on the raw match: trailing punctuation does not hide that it reached the edge.
        for url in [
            "https://example.com/report.",
            "https://x.io/a?b=c?",
            "https://example.com/a)",
        ] {
            assert!(scan_text(url, url.len()).is_empty(), "{url}");
        }
        // One blank cell after it, and it is a link.
        let url = "https://example.com/some/long/path";
        assert_eq!(scan_text(url, url.len() + 1).len(), 1);
    }

    // ---- the check: an accepted span is always one the open check accepts

    #[test]
    fn a_match_the_open_check_refuses_is_no_link() {
        // Userinfo and a bad port are refused by the grammar, so they are not underlined.
        assert!(scan_text("x http://user@host/ y", 40).is_empty());
        assert!(scan_text("x http://host:99999/ y", 40).is_empty());
        assert!(scan_text("x http://-evil/ y", 40).is_empty());
    }

    // ---- OSC 8

    fn uris(entries: &[(u32, &str)]) -> UriTable {
        entries
            .iter()
            .map(|(id, u)| (*id, (*u).to_owned()))
            .collect()
    }

    #[test]
    fn a_run_of_one_id_is_one_span_with_the_table_uri() {
        let cells = with_links("LINKxx", &[7, 7, 7, 7, 0, 0]);
        let spans = scan(&cells, &uris(&[(7, "https://example.com")]));
        let [s] = spans.as_slice() else {
            panic!("{spans:?}")
        };
        assert_eq!(s.kind, LinkKind::Osc8(7));
        assert_eq!((s.start, s.end), (0, 3));
        assert_eq!(s.uri, "https://example.com");
    }

    #[test]
    fn an_id_change_makes_two_spans_and_a_zero_breaks_a_run() {
        let cells = with_links("AABB", &[1, 1, 2, 2]);
        let spans = scan(&cells, &uris(&[(1, "https://a.com"), (2, "https://b.com")]));
        assert_eq!(
            spans.iter().map(|s| s.uri.as_str()).collect::<Vec<_>>(),
            ["https://a.com", "https://b.com"]
        );
        let cells = with_links("AxA", &[1, 0, 1]);
        let spans = scan(&cells, &uris(&[(1, "https://a.com")]));
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].end, spans[1].start), (0, 2));
    }

    #[test]
    fn an_id_missing_from_the_table_or_with_a_refused_uri_gives_no_span() {
        let cells = with_links("AAAA", &[9, 9, 9, 9]);
        assert!(scan(&cells, &uris(&[(1, "https://a.com")])).is_empty());
        let cells = with_links("AAAA", &[1, 1, 1, 1]);
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://user@host",
            "https://x/\u{202e}",
            "https://x/\n",
        ] {
            assert!(scan(&cells, &uris(&[(1, bad)])).is_empty(), "{bad:?}");
        }
        assert!(scan(&cells, &UriTable::new()).is_empty());
    }

    #[test]
    fn an_osc8_span_wins_over_a_bare_url_it_overlaps() {
        let text = "https://visible.example";
        let cells = with_links(text, &[5; 23]);
        let spans = scan(&cells, &uris(&[(5, "https://real.example")]));
        let [s] = spans.as_slice() else {
            panic!("{spans:?}")
        };
        assert_eq!(s.kind, LinkKind::Osc8(5));
        assert_eq!(s.uri, "https://real.example");
        // A bare URL elsewhere on the row is still found.
        let mut cells = with_links("click here and https://other.example x", &[0; 38]);
        for cell in &mut cells[..10] {
            cell.link = 5;
        }
        let spans = scan(&cells, &uris(&[(5, "https://real.example")]));
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].kind, LinkKind::Osc8(5));
        assert_eq!(spans[1].uri, "https://other.example");
    }

    #[test]
    fn a_row_gives_at_most_256_spans() {
        // 400 one-cell links of distinct ids.
        let mut table = UriTable::new();
        let mut cells = Vec::new();
        for i in 0..400u32 {
            table.insert(i + 1, "https://x.example".to_owned());
            let mut c = Cell::default();
            c.set_char('a');
            c.link = i + 1;
            cells.push(c);
            let mut gap = Cell::default();
            gap.set_char(' ');
            cells.push(gap);
        }
        assert_eq!(scan(&cells, &table).len(), MAX_SPANS_PER_ROW);
        // And bare URLs are capped too.
        let line = "https://x.io ".repeat(400);
        let spans = scan_text(&line, line.len() + 1);
        assert_eq!(spans.len(), MAX_SPANS_PER_ROW);
    }

    #[test]
    fn the_scan_is_linear_in_the_row_length() {
        // Adversarial rows: many scheme starts that fail, and one long match; a quadratic scan
        // would take seconds at this size.
        let cols = 4096;
        for text in [
            "h".repeat(cols),
            "http".repeat(cols / 4),
            "https://".repeat(cols / 8),
            format!("http://{}", "a".repeat(cols)),
            "((((((((((((((((".repeat(cols / 16),
        ] {
            let cells = row(&text, cols);
            let t0 = std::time::Instant::now();
            for _ in 0..200 {
                let mut out = Vec::new();
                scan_row(&cells, 0, &UriTable::new(), &mut out);
            }
            assert!(
                t0.elapsed() < std::time::Duration::from_secs(5),
                "{:?} for {}",
                t0.elapsed(),
                &text[..8.min(text.len())]
            );
        }
    }

    #[test]
    fn body_characters_and_trimming_follow_lunegits_grammar() {
        for (c, want) in [
            ('a', true),
            ('Z', true),
            ('9', true),
            ('/', true),
            ('%', true),
            (' ', false),
            ('<', false),
            ('界', false),
        ] {
            assert_eq!(is_url_body_char(c), want, "{c}");
        }
        for (input, want) in [
            ("https://x.com.", "https://x.com"),
            ("https://x.com/a_(b)", "https://x.com/a_(b)"),
            ("https://x.com)", "https://x.com"),
            ("https://x.com", "https://x.com"),
            ("https://x.com).,", "https://x.com"),
            ("", ""),
        ] {
            assert_eq!(trim_trailing_punctuation(input), want, "{input}");
        }
    }
}
