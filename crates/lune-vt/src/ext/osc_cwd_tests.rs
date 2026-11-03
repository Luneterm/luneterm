//! Tests for OSC 7 / OSC 9;9 (plan M7 step 8). There is no upstream test file. The cases are
//! Lunegit's `OscCwdScannerTests.cs` (18 Fact), run through `Terminal::write`:
//!  - ported: `StartsNull`, `Osc7_WithHost_*`, `Osc7_EmptyHost_*`, `Osc7_WindowsDriveLetter_*`,
//!    `Osc7_PercentEncodedSpaces_*`, `Osc7_PercentEncodedUnicode_*`,
//!    `Osc7_LiteralPercentInDirName_*`, `Osc7_StTerminated_*`, `Osc7_SplitAcrossTwoFeeds_*`,
//!    `Osc99_WindowsPath_*`, `Osc99_UnixPath_*`, `Osc99_FileUriWrapped_*`,
//!    `Osc94Progress_Ignored_*`, `OscTitle_Ignored_*`, `UnrelatedOsc_LeavesPriorValueUnchanged`,
//!    `OverlongGarbageBody_*`;
//!  - adapted: `EmbeddedEsc_InBody_AbortsAndRecovers` (the parser ends an OSC at an ESC, as
//!    upstream: the first sequence is a complete one here, and the last one still wins),
//!    `Reset_KeepsLatest_ButDiscardsPartial` (there is no `Reset`; CAN aborts a partial
//!    sequence, so the discarded text cannot complete later).
//!
//! The rows of `goldens/lunegit-input/osc-cwd.golden` are replayed in
//! `lune-vt-conformance/tests/lunegit_input.rs`. The table of `System.Uri` cases below that the
//! golden does not hold was probed against .NET 10 on this machine (a copy of `ParseFileUri`);
//! the control-character tightening is an addition (plan M7 step 8). Payload text is a test
//! literal here; nothing in an assertion message carries clipboard or user data.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::osc_cwd::MAX_PAYLOAD_BYTES;
use super::*;
use crate::engine::HostEvent;
use crate::headless::terminal::Terminal;

fn terminal() -> Terminal {
    let mut t = Terminal::new([]);
    t.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    t
}

fn cwd_events(t: &mut Terminal) -> Vec<String> {
    t.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::CwdChanged(path) => Some(path),
            _ => None,
        })
        .collect()
}

/// The cwd one `ESC ] <body> BEL` produces, `None` for no event.
fn cwd(body: &str) -> Option<String> {
    let mut t = terminal();
    t.write(format!("\x1b]{body}\x07").as_bytes());
    let mut events = cwd_events(&mut t);
    assert!(events.len() <= 1);
    events.pop()
}

// --- Lunegit's OscCwdScannerTests -------------------------------------------------------------

#[test]
fn starts_with_no_cwd() {
    let mut t = terminal();
    assert!(cwd_events(&mut t).is_empty());
}

#[test]
fn osc7_with_host_empty_host_and_drive_letter() {
    assert_eq!(cwd("7;file://myhost/home/u").as_deref(), Some("/home/u"));
    assert_eq!(cwd("7;file:///home/u").as_deref(), Some("/home/u"));
    assert_eq!(cwd("7;file:///C:/Users/x").as_deref(), Some("C:/Users/x"));
}

#[test]
fn osc7_percent_escapes_are_decoded_once() {
    assert_eq!(
        cwd("7;file:///home/u/My%20Docs").as_deref(),
        Some("/home/u/My Docs")
    );
    // %E2%9C%93 is U+2713
    assert_eq!(
        cwd("7;file:///home/u/%E2%9C%93").as_deref(),
        Some("/home/u/\u{2713}")
    );
    // a directory literally named "a%41b" is sent as a%2541b
    assert_eq!(
        cwd("7;file:///home/u/a%2541b").as_deref(),
        Some("/home/u/a%41b")
    );
}

#[test]
fn osc7_st_terminated_and_split_across_writes() {
    let mut t = terminal();
    t.write(b"\x1b]7;file:///var/log\x1b\\");
    assert_eq!(cwd_events(&mut t), ["/var/log"]);
    t.write(b"\x1b]7;file:///home/u");
    assert!(cwd_events(&mut t).is_empty(), "not terminated yet");
    t.write(b"/deep\x07");
    assert_eq!(cwd_events(&mut t), ["/home/u/deep"]);
}

#[test]
fn osc99_takes_the_path_as_it_is() {
    assert_eq!(cwd("9;9;C:\\Users\\x").as_deref(), Some("C:\\Users\\x"));
    let mut t = terminal();
    t.write(b"\x1b]9;9;/home/u\x1b\\");
    assert_eq!(cwd_events(&mut t), ["/home/u"]);
    // a file: URI goes through the URI parser
    assert_eq!(cwd("9;9;file:///home/u").as_deref(), Some("/home/u"));
}

#[test]
fn other_oscs_produce_no_cwd_and_leave_the_last_one() {
    assert_eq!(cwd("9;4;1;42"), None);
    assert_eq!(cwd("0;window-title"), None);
    let mut t = terminal();
    t.write(b"\x1b]7;file:///first\x07");
    assert_eq!(cwd_events(&mut t), ["/first"]);
    t.write(b"\x1b]2;title\x07\x1b]9;4;2;0\x07");
    assert!(cwd_events(&mut t).is_empty());
}

#[test]
fn an_overlong_body_is_discarded_and_the_next_sequence_works() {
    let mut t = terminal();
    // never terminated, then the next sequence ends it (ESC) and starts a valid one
    t.write(format!("\x1b]7;file:///{}", "a".repeat(8000)).as_bytes());
    t.write(b"\x1b]7;file:///home/u\x07");
    assert_eq!(cwd_events(&mut t), ["/home/u"]);
}

#[test]
fn an_esc_ends_the_sequence_so_the_last_one_still_wins() {
    let mut t = terminal();
    // Lunegit aborts the first sequence at `ESC X`; the parser ends it normally at the ESC
    t.write(b"\x1b]7;file:///a\x1bX\x1b]7;file:///b\x07");
    assert_eq!(cwd_events(&mut t), ["/b"]);
    let mut t = terminal();
    t.write(b"\x1b]7;file:///a\x1bX");
    assert_eq!(cwd_events(&mut t), ["/a"]);
}

#[test]
fn can_discards_a_partial_sequence() {
    let mut t = terminal();
    t.write(b"\x1b]7;file:///home/u\x07");
    assert_eq!(cwd_events(&mut t), ["/home/u"]);
    t.write(b"\x1b]7;file:///partial\x18");
    t.write(b"/deep\x07");
    assert!(cwd_events(&mut t).is_empty());
}

// --- limits and the tightening ----------------------------------------------------------------

#[test]
fn the_payload_cap_is_exact_and_discards_whole() {
    // body `7;file:///` + n bytes: 4096 bytes in all is accepted, 4097 is not
    let prefix = "file:///";
    let ok = "a".repeat(MAX_PAYLOAD_BYTES - prefix.len());
    let too_long = "a".repeat(MAX_PAYLOAD_BYTES - prefix.len() + 1);
    assert_eq!(cwd(&format!("7;{prefix}{ok}")).map(|p| p.len()), Some(4087));
    assert_eq!(cwd(&format!("7;{prefix}{too_long}")), None);
    // OSC 9;9: `9;9;/` + n
    let ok = "a".repeat(MAX_PAYLOAD_BYTES - "9;/".len());
    assert_eq!(cwd(&format!("9;9;/{ok}")).map(|p| p.len()), Some(4092));
    assert_eq!(cwd(&format!("9;9;/{ok}a")), None);
    // multi-byte characters count as bytes
    let wide = "\u{4e2d}".repeat(MAX_PAYLOAD_BYTES / 3 + 1);
    assert_eq!(cwd(&format!("9;9;{wide}")), None);
    // the same under 1-byte writes, with a valid sequence after the discarded one
    let mut t = terminal();
    for byte in format!("\x1b]7;file:///{}\x07", "a".repeat(5000)).bytes() {
        t.write(&[byte]);
    }
    assert!(cwd_events(&mut t).is_empty());
    t.write(b"\x1b]7;file:///after\x07");
    assert_eq!(cwd_events(&mut t), ["/after"]);
}

#[test]
fn a_path_with_a_control_character_is_dropped() {
    for body in [
        "7;file:///a%00b",
        "7;file:///a%0Ab",
        "7;file:///a%0D",
        "7;file:///a%1B[31m",
        "7;file:///a%7F",
        "7;file:///a%C2%85",
        "7;file:///a%C2%9B31m",
        "7;file:///a\u{7f}b",
        "9;9;/a\u{7f}b",
        "9;9;file:///a%00",
    ] {
        assert_eq!(cwd(body), None, "{body:?}");
    }
    // a lone NBSP is not a control
    assert_eq!(cwd("9;9;/a\u{a0}b").as_deref(), Some("/a\u{a0}b"));
    // the previous cwd stands
    let mut t = terminal();
    t.write(b"\x1b]7;file:///keep\x07\x1b]7;file:///a%00\x07");
    assert_eq!(cwd_events(&mut t), ["/keep"]);
}

#[test]
fn an_empty_or_blank_path_is_dropped() {
    for body in [
        "7;",
        "7;   ",
        "9;9;",
        "9;9;  ",
        "7;file:",
        "7;http://example.com/x",
    ] {
        assert_eq!(cwd(body), None, "{body:?}");
    }
}

#[test]
fn leading_zeros_in_the_identifier_are_accepted_by_the_parser() {
    // `OSC 007 ;` is OSC 7 to the parser; Lunegit's scanner needs the exact `7;`
    assert_eq!(cwd("007;file:///x").as_deref(), Some("/x"));
}

/// `System.Uri` behaviour outside the golden, probed on .NET 10 (see the file header).
#[test]
fn file_uri_rules_beyond_the_golden() {
    for (body, want) in [
        ("7;/a/../b", Some("/b")),
        ("7;/a/./b", Some("/a/b")),
        ("7;/a%20b", Some("/a%20b")),
        ("7;/a?q#f", Some("/a?q#f")),
        ("7;//host/p", Some("/p")),
        ("7;//", None),
        ("7;file:/x", None),
        ("7;FILE:/x", None),
        ("7;file:x", None),
        ("7;file://C:/x", Some("C:/x")),
        ("7;file://C:", Some("C:")),
        ("7;file:///C:", Some("C:")),
        ("7;file://host/C:/x", Some("C:/x")),
        ("7;file:////srv/share/x", Some("/share/x")),
        ("7;file://///srv/share/x", Some("/share/x")),
        ("7;file:///a/%2e%2e/b", Some("/b")),
        ("7;file:///a/%2E/b", Some("/a/b")),
        ("7;file:///a//b", Some("/a//b")),
        ("7;file://host?x", Some("/")),
        ("7;file://host#x", Some("/")),
        ("7;file:///a%23b", Some("/a#b")),
        ("7;file:///a%3Fb", Some("/a?b")),
        ("7;file:///%E4%B8%AD", Some("/\u{4e2d}")),
        ("7;file:///%C3", Some("/%C3")),
        ("7;file:///%C3%A4%C3", Some("/\u{e4}%C3")),
        ("7;file:///%ff%41", Some("/%ffA")),
        ("7;file:///%41%2", Some("/A%2")),
        ("7;file:///%ED%A0%80", Some("/%ED%A0%80")),
        ("7;file:///%F0%9F%98%80%F0%9F", Some("/\u{1f600}%F0%9F")),
        ("7;file:///a/b/..", Some("/a/")),
        ("7;file:///a/b/.", Some("/a/b/")),
        ("7;file:///%25", Some("/%")),
        ("7;file:///%2541", Some("/%41")),
        ("7;file:///..", Some("/")),
        ("7;file:///../a", Some("/a")),
        ("7;file://host/a b", Some("/a b")),
        ("7;file:///a%5Cb", Some("/a\\b")),
        ("7;file:///a\\..\\b", Some("/a\\..\\b")),
        ("7;file://[::1]/x", Some("/x")),
        ("7;file://my_host/x", Some("/x")),
        ("7;file://h/%2e%2e/%2e%2E/x", Some("/x")),
        ("7;file://h/a/%2e", Some("/a/")),
        ("7;file://host/a%20b/../c", Some("/c")),
        ("7;file://h/a%", Some("/a%")),
        ("7;file:///?x", Some("/")),
        ("7;file:///a b?c", Some("/a b")),
        // an authority System.Uri refuses: Lunegit strips scheme and host leniently
        ("7;file://localhost:80/x", Some("/x")),
        ("7;file://us@h:8/x", Some("/x")),
        ("7;file://exa mple/x?y", Some("/x?y")),
        ("7;file://exa mple/../x", Some("/../x")),
        ("7;file://h%6Fst/x", Some("/x")),
    ] {
        assert_eq!(cwd(body).as_deref(), want, "{body:?}");
    }
}

#[test]
fn is_chunk_invariant() {
    for body in [
        "7;file:///home/u/My%20Docs",
        "7;file:///a%00",
        "9;9;C:\\x",
        "9;9;file:///x",
        "9;4;1;5",
        "7;",
    ] {
        let bytes = format!("\x1b]{body}\x07");
        let mut whole = terminal();
        whole.write(bytes.as_bytes());
        let mut split = terminal();
        for byte in bytes.bytes() {
            split.write(&[byte]);
        }
        assert_eq!(whole.drain_events(), split.drain_events(), "{body:?}");
    }
}

#[test]
fn last_wins_within_a_drain_and_the_osc_is_not_reported_unknown() {
    let mut t = terminal();
    t.write(b"\x1b]7;file:///a\x07\x1b]9;9;/b\x07");
    assert_eq!(t.drain_events(), [HostEvent::CwdChanged("/b".to_owned())]);
    t.write(b"\x1b]77;file:///a\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc { ident: 77, len: 9 }]
    );
}

#[test]
fn without_the_install_nothing_is_reported() {
    let mut t = Terminal::new([]);
    t.write(b"\x1b]7;file:///a\x07\x1b]9;9;/b\x07\x1b]9;4;1;5\x07\x1b]133;C\x07");
    assert!(t.drain_events().is_empty());
}
