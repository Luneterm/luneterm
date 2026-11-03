//! Tests for the paste sanitizer (plan M7 step 6, D7, user decision Q22): the cases of the plan,
//! the xterm.js `Clipboard.ts` behaviour they extend, and a seeded property test of the bracketed
//! invariant over a hostile alphabet. No test prints the pasted text: failures name a seed, a
//! length and a position only.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

const START_BYTES: &[u8] = b"\x1b[200~";
const END_BYTES: &[u8] = b"\x1b[201~";

/// Bracketed output without its markers; panics (without the text) if they are missing.
fn middle(out: &[u8]) -> &[u8] {
    assert!(out.starts_with(START_BYTES), "no start marker");
    assert!(out.ends_with(END_BYTES), "no end marker");
    out.get(START_BYTES.len()..out.len() - END_BYTES.len())
        .unwrap()
}

fn bracketed(text: &str) -> Vec<u8> {
    paste_bytes(text, true)
}

// ---- plan cases -----------------------------------------------------------------------------

#[test]
fn line_breaks_become_cr_in_both_modes() {
    assert_eq!(paste_bytes("a\r\nb\nc\rd", false), b"a\rb\rc\rd");
    assert_eq!(middle(&bracketed("a\r\nb\nc\rd")), b"a\rb\rc\rd");
}

#[test]
fn line_break_normalization_is_left_to_right_like_the_regex() {
    // /\r?\n/g over "\r\r\n" matches the second CR with the LF only.
    assert_eq!(paste_bytes("\r\r\n", false), b"\r\r");
    assert_eq!(paste_bytes("\n\n", false), b"\r\r");
    assert_eq!(paste_bytes("\r\n\r\n", false), b"\r\r");
    assert_eq!(paste_bytes("\r", false), b"\r");
    assert_eq!(paste_bytes("\n\r", false), b"\r\r");
}

#[test]
fn a_pasted_end_marker_is_removed_and_the_rest_stays_inside_the_brackets() {
    let out = bracketed("x\x1b[201~rm -rf ~\n");
    assert_eq!(out, b"\x1b[200~xrm -rf ~\r\x1b[201~");
    // Only the two markers are escapes: the one at each end.
    assert_eq!(out.iter().filter(|b| **b == 0x1b).count(), 2);
}

#[test]
fn a_pasted_start_marker_is_removed_too() {
    assert_eq!(bracketed("a\x1b[200~b"), b"\x1b[200~ab\x1b[201~");
}

#[test]
fn c0_controls_are_dropped_except_tab_and_cr_and_del_is_kept() {
    assert_eq!(middle(&bracketed("\x00\x07\t\x7f")), b"\t\x7f");
    for b in 0u8..0x20 {
        let text = String::from(char::from(b));
        let want: &[u8] = match b {
            0x09 => b"\t",
            0x0d => b"\r",
            0x0a => b"\r", // LF is a line break first
            _ => b"",
        };
        assert_eq!(middle(&bracketed(&text)), want, "C0 byte {b:#04x}");
    }
}

#[test]
fn c1_controls_are_dropped_including_the_8_bit_csi() {
    for cp in 0x80u32..=0x9f {
        let text = format!("a{}b", char::from_u32(cp).unwrap());
        assert_eq!(middle(&bracketed(&text)), b"ab", "U+{cp:04X}");
    }
    // The neighbours stay: U+00A0 (NBSP) and U+007F's other side.
    assert_eq!(
        middle(&bracketed("\u{a0}\u{ff}\u{100}")),
        "\u{a0}\u{ff}\u{100}".as_bytes()
    );
}

#[test]
fn a_marker_assembled_from_fragments_cannot_survive() {
    // One pass removes the inner marker, leaving "ESC [ 2 0 0 ~" assembled from the pieces;
    // the ESC is dropped next, so the output has no marker inside the brackets.
    let out = bracketed("\x1b[20\x1b[201~0~");
    assert_eq!(out, b"\x1b[200~[200~\x1b[201~");
    assert_eq!(middle(&out).iter().filter(|b| **b == 0x1b).count(), 0);
}

#[test]
fn non_bracketed_changes_line_breaks_and_nothing_else() {
    let text = "a\x1b[201~b\u{9b}c\x00d\r\ne";
    assert_eq!(
        paste_bytes(text, false),
        "a\x1b[201~b\u{9b}c\x00d\re".as_bytes()
    );
}

#[test]
fn ordinary_text_passes_through_both_modes() {
    let text = "héllo, 世界 \u{1f600} tab\there";
    assert_eq!(paste_bytes(text, false), text.as_bytes());
    assert_eq!(middle(&bracketed(text)), text.as_bytes());
}

#[test]
fn empty_text() {
    assert_eq!(paste_bytes("", false), b"");
    assert_eq!(bracketed(""), b"\x1b[200~\x1b[201~");
}

// ---- property: the bracketed invariant --------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fragments a hostile paste is made of.
fn alphabet() -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for cp in 0u32..0x20 {
        parts.push(char::from_u32(cp).unwrap().to_string());
    }
    parts.push("\u{7f}".into());
    for cp in 0x80u32..=0x9f {
        parts.push(char::from_u32(cp).unwrap().to_string());
    }
    for fragment in [
        "\x1b[200~",
        "\x1b[201~",
        "\x1b[20",
        "\x1b[201",
        "\x1b[",
        "\x1b",
        "0~",
        "~",
        "[",
        "2",
        "0",
        "1",
        "\x1b[20\x1b[201~0~",
        "\x1b[2\x1b[200~01~",
        "\r\n",
        "\r",
        "\n",
        "a",
        "z",
        " ",
        "rm -rf ~",
        "\u{a0}",
        "\u{c2}",
        "\u{e9}",
        "\u{4e16}",
        "\u{1f600}",
    ] {
        parts.push(fragment.to_string());
    }
    parts
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\r").replace('\n', "\r")
}

#[test]
fn bracketed_output_never_leaves_the_brackets() {
    let parts = alphabet();
    for seed in 1u64..=40 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for case in 0..500 {
            let len = rng.below(24);
            let mut text = String::new();
            for _ in 0..len {
                text.push_str(&parts[rng.below(parts.len())]);
            }
            let out = paste_bytes(&text, true);
            let label = format!("seed {seed} case {case} input_len {}", text.len());

            assert!(out.starts_with(START_BYTES), "{label}: start");
            assert!(out.ends_with(END_BYTES), "{label}: end");
            let inner = &out[START_BYTES.len()..out.len() - END_BYTES.len()];
            // The invariant, by position (never by content).
            if let Some(i) = inner.iter().position(|b| *b == 0x1b) {
                panic!("{label}: ESC at {i}");
            }
            if let Some(i) = inner
                .iter()
                .position(|b| *b < 0x20 && *b != 0x09 && *b != 0x0d)
            {
                panic!("{label}: control byte at {i}");
            }
            if let Some(i) = inner
                .windows(2)
                .position(|w| w[0] == 0xc2 && (0x80..=0x9f).contains(&w[1]))
            {
                panic!("{label}: C1 at {i}");
            }
            assert!(
                std::str::from_utf8(inner).is_ok(),
                "{label}: inner is not UTF-8"
            );
            assert!(
                out.len() <= text.len() + 12,
                "{label}: grew to {}",
                out.len()
            );

            // Non-bracketed: exactly the line-break normalization.
            let plain = paste_bytes(&text, false);
            assert_eq!(plain, normalize(&text).into_bytes(), "{label}: plain");

            // Nothing is invented: every inner char comes from the normalized input.
            let normalized = normalize(&text);
            let inner_text = std::str::from_utf8(inner).unwrap();
            let mut source = normalized.chars();
            for c in inner_text.chars() {
                assert!(
                    source.any(|s| s == c),
                    "{label}: inner is not a subsequence of the input"
                );
            }
        }
    }
}

#[test]
fn bracketed_output_is_idempotent_on_its_own_inner_text() {
    // Re-pasting what a paste produced changes nothing more: the inner text is already clean.
    let parts = alphabet();
    let mut rng = Rng(0xC0FFEE);
    for case in 0..2000 {
        let mut text = String::new();
        for _ in 0..rng.below(20) {
            text.push_str(&parts[rng.below(parts.len())]);
        }
        let first = paste_bytes(&text, true);
        let inner = String::from_utf8(middle(&first).to_vec()).unwrap();
        let second = paste_bytes(&inner, true);
        assert_eq!(middle(&second), inner.as_bytes(), "case {case}");
    }
}
