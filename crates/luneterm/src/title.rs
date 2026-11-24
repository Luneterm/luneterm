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

/// The longest notification title kept, in chars (OSC 777 `notify` title).
pub const NOTICE_TITLE_CHARS: usize = 80;
/// The longest notification body kept, in chars.
pub const NOTICE_BODY_CHARS: usize = 240;

/// The text to show for a notification field `raw` (bell-notifications plan D9): from at most the
/// first [`MAX_SCAN_CHARS`] chars, every whitespace char (a newline, a tab, U+2028, ...) becomes
/// one space and a run of them collapses to one, then Unicode control characters and bidi
/// controls are dropped (they do not break a run: `a`, space, ESC, space, `b` reads `a b`),
/// trimmed, at most `max_chars` chars. May be empty; unlike [`sanitize`] there is no default.
pub fn sanitize_text(raw: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut pending_space = false;
    for c in raw.chars().take(MAX_SCAN_CHARS) {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if c.is_control() || is_bidi_control(c) {
            continue;
        }
        if count >= max_chars {
            break;
        }
        if pending_space {
            pending_space = false;
            out.push(' ');
            count += 1;
            if count >= max_chars {
                break;
            }
        }
        out.push(c);
        count += 1;
    }
    out.truncate(out.trim_end().len());
    out
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

    #[test]
    fn notice_text_whitespace_becomes_one_space() {
        assert_eq!(sanitize_text("a\nb\tc", 80), "a b c");
        assert_eq!(sanitize_text("a\u{2028}b", 80), "a b");
        assert_eq!(sanitize_text("a \r\n  \t b", 80), "a b");
        assert_eq!(sanitize_text("a\u{85}b", 80), "a b");
        // a dropped control does not make two spaces
        assert_eq!(sanitize_text("a \u{1} b", 80), "a b");
        assert_eq!(sanitize_text("  lead and trail \n", 80), "lead and trail");
    }

    #[test]
    fn notice_text_drops_controls_and_bidi_controls() {
        assert_eq!(sanitize_text("a\u{1}b", 80), "ab");
        assert_eq!(sanitize_text("\u{202E}evil", 80), "evil");
        assert_eq!(sanitize_text("\u{9b}x", 80), "x");
        assert_eq!(sanitize_text("a\u{1b}[31mb\u{7}\0c\u{7f}", 80), "a[31mbc");
        for c in ['\u{061C}', '\u{200E}', '\u{2066}', '\u{2069}'] {
            assert_eq!(sanitize_text(&format!("ab{c}cd"), 80), "abcd");
        }
    }

    #[test]
    fn notice_text_keeps_emoji_sequences_and_caps_in_chars() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(sanitize_text(family, 80), family);
        assert_eq!(sanitize_text("日本語 🎉", 80), "日本語 🎉");
        let long = "é".repeat(500);
        let t = sanitize_text(&long, NOTICE_BODY_CHARS);
        assert_eq!(t.chars().count(), 240);
        assert_eq!(sanitize_text(&"a".repeat(81), NOTICE_TITLE_CHARS).len(), 80);
        // the cap never leaves a trailing space
        let spaced = format!("{} {}", "a".repeat(79), "b".repeat(10));
        assert_eq!(sanitize_text(&spaced, 80), "a".repeat(79));
        let spaced = format!("{} {}", "a".repeat(80), "b");
        assert_eq!(sanitize_text(&spaced, 80), "a".repeat(80));
        assert_eq!(sanitize_text("abc", 0), "");
    }

    #[test]
    fn notice_text_with_nothing_printable_is_empty() {
        for raw in [
            "",
            "   ",
            " \t\n ",
            "\u{1b}\u{7}",
            "\u{202E}\u{200F}",
            " \u{7} ",
        ] {
            assert_eq!(sanitize_text(raw, 80), "", "{raw:?}");
        }
    }

    #[test]
    fn notice_text_scans_only_the_first_4096_chars() {
        let raw = format!("{}visible", "\u{7}".repeat(MAX_SCAN_CHARS));
        assert_eq!(sanitize_text(&raw, 80), "");
        let raw = format!("{}visible", "\u{7}".repeat(MAX_SCAN_CHARS - 1));
        assert_eq!(sanitize_text(&raw, 80), "v");
        // whitespace counts as scanned too
        let raw = format!("{}x", " ".repeat(MAX_SCAN_CHARS));
        assert_eq!(sanitize_text(&raw, 80), "");
    }

    #[test]
    fn a_ten_megabyte_notice_returns_in_under_ten_milliseconds() {
        for unit in ["x\u{202E}", "x ", "\n"] {
            let raw = unit.repeat(10 * 1024 * 1024 / unit.len());
            let started = Instant::now();
            let t = sanitize_text(&raw, NOTICE_BODY_CHARS);
            let took = started.elapsed();
            assert!(t.chars().count() <= 240);
            assert!(took.as_millis() < 10, "{unit:?} took {took:?}");
        }
    }
}
