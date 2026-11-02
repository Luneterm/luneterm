//! Tests for OSC 9;4 progress (plan M7 step 8). There is no upstream test file. The cases are
//! Lunegit's `Osc94ProgressScannerTests.cs` (14 Fact + the 4-row Theory), run through
//! `Terminal::write`:
//!  - ported: `StartsAtNone` (no event), `Parses_BelTerminatedStates` (4 rows),
//!    `State0_ClearsToNone`, `Sequence_SplitAcrossTwoFeeds_*`, `StTerminator_Recognized`,
//!    `StTerminator_SplitAcrossFeeds_*`, `Percent_AboveHundred_*`, `MissingPercent_*`,
//!    `NonTargetOsc_*`, `Osc9_OtherSubCommand_Ignored`, `OverlongGarbageBody_*`,
//!    `NonDigitState_Ignored`;
//!  - adapted: `EmbeddedEsc_InBody_AbortsAndRecovers` (the parser ends an OSC at an ESC, as
//!    upstream: the cut sequence is complete here, `9;4;1;` giving Normal 0, and the next one
//!    wins), `Reset_MidSequence_DiscardsPartial` (no `Reset`; CAN aborts the partial one);
//!  - replaced: `Reset_ReturnsToNone` (the engine keeps no scanner state; the host clears the
//!    indicator on process exit).
//!
//! `goldens/lunegit-input/osc-progress.golden` (about 120 rows) is replayed in
//! `lune-vt-conformance/tests/lunegit_input.rs`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::osc_progress::MAX_PAYLOAD_BYTES;
use super::*;
use crate::engine::{HostEvent, Progress, ProgressState};
use crate::headless::terminal::Terminal;

fn terminal() -> Terminal {
    let mut t = Terminal::new([]);
    t.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    t
}

fn progress_events(t: &mut Terminal) -> Vec<Progress> {
    t.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::Progress(p) => Some(p),
            _ => None,
        })
        .collect()
}

/// The report one `ESC ] <body> BEL` produces.
fn report(body: &str) -> Option<(ProgressState, u8)> {
    let mut t = terminal();
    t.write(format!("\x1b]{body}\x07").as_bytes());
    let mut events = progress_events(&mut t);
    assert!(events.len() <= 1);
    events.pop().map(|p| (p.state, p.percent))
}

#[test]
fn bel_terminated_states() {
    for (body, state, percent) in [
        ("9;4;1;42", ProgressState::Normal, 42),
        ("9;4;2;0", ProgressState::Error, 0),
        ("9;4;3;0", ProgressState::Indeterminate, 0),
        ("9;4;4;0", ProgressState::Warning, 0),
        ("9;4;0", ProgressState::None, 0),
        ("9;4;5;10", ProgressState::None, 10),
        ("9;4;6", ProgressState::None, 0),
        ("9;4;0;77", ProgressState::None, 77),
    ] {
        assert_eq!(report(body), Some((state, percent)), "{body:?}");
    }
}

#[test]
fn st_terminated_and_split_sequences() {
    let mut t = terminal();
    assert!(progress_events(&mut t).is_empty());
    t.write(b"\x1b]9;4;1;30\x1b\\");
    assert_eq!(
        progress_events(&mut t),
        [Progress {
            state: ProgressState::Normal,
            percent: 30
        }]
    );
    t.write(b"\x1b]9;4;1");
    assert!(progress_events(&mut t).is_empty());
    t.write(b";55\x07");
    assert_eq!(progress_events(&mut t)[0].percent, 55);
    t.write(b"\x1b]9;4;1;30\x1b");
    t.write(b"\\");
    assert_eq!(progress_events(&mut t).len(), 1);
}

#[test]
fn percent_rules() {
    for (body, percent) in [
        ("9;4;1;150", 100),
        ("9;4;1;101", 100),
        ("9;4;1;100", 100),
        ("9;4;1;2147483647", 100),
        ("9;4;1;99999999999", 0),
        ("9;4;1;", 0),
        ("9;4;1;abc", 0),
        ("9;4;1;-5", 0),
        ("9;4;1;+5", 0),
        ("9;4;1;5 0", 0),
        ("9;4;1;\u{663}", 0),
        ("9;4;01;050", 50),
        ("9;4;1;50;extra", 50),
        ("9;4;1;50;", 50),
        ("9;4;1;5;5;5", 5),
        ("9;4;3", 0),
    ] {
        assert_eq!(report(body).map(|r| r.1), Some(percent), "{body:?}");
    }
}

#[test]
fn malformed_sequences_produce_nothing() {
    for body in [
        "9;4",
        "9;4;",
        "9;4;;5",
        "9;4;x;5",
        "9;4;99999999999;5",
        "9;4;-1",
        "9;5;1;50",
        "9;9;/x",
        "9;1;hello",
        "9;;4;1;5",
        "94;1;5",
        ";4;1;5",
        "0;title",
        "9",
        "9;",
    ] {
        assert_eq!(report(body), None, "{body:?}");
    }
}

#[test]
fn the_payload_cap_is_exact_and_discards_whole() {
    // body `9;4;1;50;` + n: 64 bytes in all is accepted, 65 is not
    let ok = "a".repeat(MAX_PAYLOAD_BYTES - "4;1;50;".len());
    assert_eq!(
        report(&format!("9;4;1;50;{ok}")),
        Some((ProgressState::Normal, 50))
    );
    assert_eq!(report(&format!("9;4;1;50;{ok}a")), None);
    assert_eq!(report(&format!("9;4;{}", "9".repeat(200))), None);
    // a discarded sequence does not stop the next one
    let mut t = terminal();
    t.write(format!("\x1b]9;4;1;50;{ok}a\x07").as_bytes());
    t.write(b"\x1b]9;4;2;7\x07");
    assert_eq!(
        progress_events(&mut t),
        [Progress {
            state: ProgressState::Error,
            percent: 7
        }]
    );
}

#[test]
fn an_esc_ends_the_sequence_and_can_aborts_it() {
    let mut t = terminal();
    // Lunegit aborts the first sequence at `ESC X`; the parser ends it normally at the ESC
    t.write(b"\x1b]9;4;1;\x1bX\x1b]9;4;2;0\x07");
    assert_eq!(
        progress_events(&mut t),
        [Progress {
            state: ProgressState::Error,
            percent: 0
        }]
    );
    t.write(b"\x1b]9;4;1;5\x18");
    t.write(b"5\x07");
    assert!(progress_events(&mut t).is_empty());
}

#[test]
fn other_oscs_leave_the_state_alone_and_last_wins() {
    let mut t = terminal();
    t.write(b"\x1b]9;4;1;42\x07\x1b]0;some-title\x07\x1b]9;1;hello\x07\x1b]9;4;x;0\x07");
    let events = t.drain_events();
    assert!(events.contains(&HostEvent::TitleChanged));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, HostEvent::Progress(_)))
            .count(),
        1
    );
    t.write(b"\x1b]9;4;3\x07\x1b]9;4;1;10\x07\x1b]9;4;1;60\x07");
    assert_eq!(
        progress_events(&mut t),
        [Progress {
            state: ProgressState::Normal,
            percent: 60
        }]
    );
}

#[test]
fn osc_9_forms_are_never_reported_unknown_and_share_the_identifier_with_the_cwd_handler() {
    let mut t = terminal();
    t.write(b"\x1b]9;4;1;5\x07\x1b]9;9;/x\x07\x1b]9;1;hello\x07\x1b]9;plain notification\x07");
    assert_eq!(
        t.drain_events(),
        [
            HostEvent::Progress(Progress {
                state: ProgressState::Normal,
                percent: 5
            }),
            HostEvent::CwdChanged("/x".to_owned())
        ]
    );
}
