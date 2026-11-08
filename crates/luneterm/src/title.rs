//! The window title (plan M9 D14). A title is set by the child (OSC 0/2) and goes to the OS window
//! and the title band, so it is untrusted: control characters and the bidi controls that can
//! reorder or spoof what the user reads are dropped, and the work is bounded.

/// The title when the child set none or nothing printable.
pub const DEFAULT_TITLE: &str = "Luneterm";
/// Lunegit's `ClampTitle` cap, in chars.
pub const MAX_TITLE_CHARS: usize = 80;
/// At most this many input chars are looked at, whatever the title's length.
pub const MAX_SCAN_CHARS: usize = 4096;

/// A Unicode bidi control that can reorder the text around it: ALM, LRM, RLM, the embeddings and
/// overrides (U+202A to U+202E) and the isolates (U+2066 to U+2069).
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// The title to show for `raw`: Unicode control characters (Cc) and bidi controls removed,
/// trimmed, at most 80 chars, from at most the first 4,096 chars of `raw`; "Luneterm" when
/// nothing is left.
pub fn sanitize(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .take(MAX_SCAN_CHARS)
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect();
    let capped: String = kept.trim().chars().take(MAX_TITLE_CHARS).collect();
    let title = capped.trim_end();
    if title.is_empty() {
        DEFAULT_TITLE.to_owned()
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn plain_titles_pass_through_trimmed() {
        assert_eq!(sanitize("vim main.rs"), "vim main.rs");
        assert_eq!(sanitize("  ~/src  "), "~/src");
        assert_eq!(sanitize("日本語 🎉"), "日本語 🎉");
    }

    #[test]
    fn control_characters_are_removed() {
        // C0 (ESC, BEL, NUL, TAB, LF, CR), DEL and C1 (U+0085, U+009B).
        assert_eq!(sanitize("a\u{1b}[31mb\u{7}c\0d\te\nf\rg"), "a[31mbcdefg");
        assert_eq!(sanitize("x\u{7f}y\u{85}z\u{9b}w"), "xyzw");
    }

    #[test]
    fn bidi_controls_are_removed_and_other_format_characters_stay() {
        for c in [
            '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}',
            '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            assert_eq!(sanitize(&format!("ab{c}cd")), "abcd", "U+{:04X}", c as u32);
        }
        // Not bidi controls: the zero-width joiner (emoji sequences) and the byte-order mark are
        // left to the platform.
        assert_eq!(sanitize("a\u{200D}b"), "a\u{200D}b");
        // The characters next to the ranges stay.
        let near = "a\u{2065}b\u{206A}c\u{202F}d\u{2029}e";
        assert_eq!(sanitize(near), near);
    }

    #[test]
    fn the_title_is_capped_at_80_chars_not_bytes() {
        let long = "é".repeat(200);
        let t = sanitize(&long);
        assert_eq!(t.chars().count(), 80);
        assert_eq!(sanitize(&"a".repeat(80)), "a".repeat(80));
        assert_eq!(sanitize(&"a".repeat(81)), "a".repeat(80));
        // The cap does not leave a trailing space.
        let spaced = format!("{} {}", "a".repeat(79), "b".repeat(10));
        assert_eq!(sanitize(&spaced), "a".repeat(79));
    }

    #[test]
    fn empty_and_unprintable_titles_become_luneterm() {
        for raw in ["", "   ", "\u{1b}\u{7}", "\u{202E}\u{200F}", " \t\n "] {
            assert_eq!(sanitize(raw), DEFAULT_TITLE, "{raw:?}");
        }
    }

    #[test]
    fn only_the_first_4096_chars_are_scanned() {
        // 4,096 removed chars then text: the text is past the scan limit, so nothing is left.
        let raw = format!("{}visible", "\u{7}".repeat(MAX_SCAN_CHARS));
        assert_eq!(sanitize(&raw), DEFAULT_TITLE);
        let raw = format!("{}visible", "\u{7}".repeat(MAX_SCAN_CHARS - 1));
        assert_eq!(sanitize(&raw), "v");
    }

    #[test]
    fn a_ten_megabyte_title_returns_in_under_ten_milliseconds() {
        let raw = "x\u{202E}".repeat(5 * 1024 * 1024 / 2);
        assert!(raw.len() >= 10 * 1024 * 1024 - 8);
        let started = Instant::now();
        let t = sanitize(&raw);
        let took = started.elapsed();
        assert_eq!(t.chars().count(), 80);
        assert!(took.as_millis() < 10, "took {took:?}");
    }
}
