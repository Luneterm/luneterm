//! Tests for OSC 133 and the 2026 latch (plan M7 step 8). There is no upstream test file. The
//! cases are Lunegit's `TerminalCommandScannerTests.cs` (29 Fact + 13 Theory rows + 3), as
//! `Terminal::write` and events (`CommandMark`, `SyncUpdateInCommand`):
//!  - ported: `StartsNotRunning`, `C_SetsRunning`, `D_ClearsRunning`, `A_ClearsRunning`,
//!    `D_WithExitCode_*`, `B_LeavesStateUnchanged`, `D_WithNoPrecedingC_IsNoOp`,
//!    `BothTerminators_*`, `UnrelatedOsc_*` (3), `Interleaved_Osc94AndOsc133_InSameChunk_*`,
//!    `OverlongGarbageBody_*`, `ParameterisedThirdPartyMarker_UnderCap_Parses`,
//!    `C_SplitAcrossReads_AtEveryBoundary_*`, `D_SplitAcrossReads_AtEveryBoundary_*`,
//!    `C_OneByteAtATime_*`, `D_OneByteAtATime_*`, `SyncUpdate_*` (latch, order, clear by D and A,
//!    every split, one byte at a time, the 8 negative framings, multi-param);
//!  - adapted: `EmbeddedEsc_InBody_AbortsAndRecovers` (the parser ends an OSC at an ESC, as
//!    upstream: the first mark is complete here, so a C is reported before the D);
//!  - replaced: `Reset_ClearsValueAndPartialSequence`, `Reset_ClearsSyncUpdateAndCommandStartCount`
//!    (no scanner to reset; `CommandStartCount` is the host's count of C marks),
//!    `Feed_NeverMutatesInputBytes` (`write` takes `&[u8]`).
//!
//! The latch is read from `ExtState` here (it is host-visible only while a command runs).
//! `goldens/lunegit-input/osc-command.golden` (about 230 rows) is replayed in
//! `lune-vt-conformance/tests/lunegit_input.rs`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::osc_command::MAX_PAYLOAD_BYTES;
use super::*;
use crate::engine::{CommandMark, CommandMarkKind, HostEvent};
use crate::headless::terminal::Terminal;

fn terminal() -> Terminal {
    let mut t = Terminal::new([]);
    t.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    t
}

fn running(t: &Terminal) -> bool {
    t.core().input_handler().ext.command.running()
}

fn latch(t: &Terminal) -> bool {
    t.core().input_handler().ext.command.sync_latch()
}

fn marks(t: &mut Terminal) -> Vec<CommandMark> {
    t.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::CommandMark(mark) => Some(mark),
            _ => None,
        })
        .collect()
}

fn mark(kind: CommandMarkKind, exit_code: Option<i32>) -> CommandMark {
    CommandMark { kind, exit_code }
}

fn feed(t: &mut Terminal, text: &str) {
    t.write(text.as_bytes());
}

// --- marks ------------------------------------------------------------------------------------

#[test]
fn starts_not_running_and_c_d_a_toggle_running() {
    let mut t = terminal();
    assert!(!running(&t));
    feed(&mut t, "\x1b]133;C\x1b\\");
    assert!(running(&t));
    feed(&mut t, "\x1b]133;D\x1b\\");
    assert!(!running(&t));
    feed(&mut t, "\x1b]133;C\x1b\\\x1b]133;A\x1b\\");
    assert!(!running(&t));
    assert_eq!(
        marks(&mut t),
        [
            mark(CommandMarkKind::OutputStart, None),
            mark(CommandMarkKind::CommandFinished, None),
            mark(CommandMarkKind::OutputStart, None),
            mark(CommandMarkKind::PromptStart, None)
        ]
    );
}

#[test]
fn d_with_an_exit_code_and_b_and_d_without_c() {
    for (body, code) in [
        ("D;0", Some(0)),
        ("D;1", Some(1)),
        ("D;255", Some(255)),
        ("D;2147483647", Some(i32::MAX)),
        ("D;2147483648", None),
        ("D;99999999999999999999", None),
        ("D;-1", None),
        ("D;+1", None),
        ("D;abc", None),
        ("D;", None),
        ("D", None),
        ("D;12;extra", Some(12)),
        ("D;;12", None),
    ] {
        let mut t = terminal();
        feed(&mut t, "\x1b]133;C\x1b\\");
        feed(&mut t, &format!("\x1b]133;{body}\x1b\\"));
        assert!(!running(&t));
        let all = marks(&mut t);
        assert_eq!(
            all.last().copied(),
            Some(mark(CommandMarkKind::CommandFinished, code)),
            "{body:?}"
        );
    }
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x1b\\\x1b]133;B\x1b\\");
    assert!(running(&t), "B changes nothing");
    let mut t = terminal();
    feed(&mut t, "\x1b]133;D\x1b\\\x1b]133;A\x1b\\");
    assert!(!running(&t));
    assert_eq!(
        marks(&mut t),
        [
            mark(CommandMarkKind::CommandFinished, None),
            mark(CommandMarkKind::PromptStart, None)
        ]
    );
}

#[test]
fn only_a_single_character_mark_counts() {
    for body in [
        "", "AB", "c", "E;cmd", "Z", ";", ";;", "a", "CC", "\u{e9}", "DD;0",
    ] {
        let mut t = terminal();
        feed(&mut t, &format!("\x1b]133;{body}\x07"));
        assert!(marks(&mut t).is_empty(), "{body:?}");
        assert!(!running(&t));
    }
    // a bare `133` and other identifiers
    for osc in [
        "\x1b]133\x07",
        "\x1b]134;C\x07",
        "\x1b]13;C\x07",
        "\x1b]1330;C\x07",
    ] {
        let mut t = terminal();
        feed(&mut t, osc);
        assert!(marks(&mut t).is_empty());
    }
}

#[test]
fn parameterised_third_party_markers_parse() {
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x1b\\");
    feed(&mut t, "\x1b]133;A;special_key=1;click_events=1\x1b\\");
    assert!(!running(&t));
    feed(&mut t, "\x1b]133;C;extra=1\x07\x1b]133;C;\x07");
    assert!(running(&t));
    assert_eq!(marks(&mut t).len(), 4);
}

#[test]
fn both_terminators_and_unrelated_oscs() {
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x07");
    assert!(running(&t));
    feed(
        &mut t,
        "\x1b]0;some-title\x07\x1b]7;file://host/some/path\x1b\\\x1b]9;4;3;0\x1b\\",
    );
    assert!(running(&t));
}

#[test]
fn osc94_and_osc133_in_the_same_chunk_both_parse() {
    let mut t = terminal();
    feed(&mut t, "\x1b]9;4;1;50\x1b\\\x1b]133;C\x1b\\");
    let events = t.drain_events();
    assert!(events.iter().any(|e| matches!(e, HostEvent::Progress(_))));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HostEvent::CommandMark(_)))
    );
    assert!(running(&t));
}

#[test]
fn an_esc_ends_the_sequence_so_the_cut_mark_is_complete() {
    let mut t = terminal();
    // Lunegit aborts the C at `ESC X`; the parser ends it normally at the ESC
    feed(&mut t, "\x1b]133;C\x1bX\x1b]133;D\x1b\\");
    assert!(!running(&t));
    assert_eq!(
        marks(&mut t),
        [
            mark(CommandMarkKind::OutputStart, None),
            mark(CommandMarkKind::CommandFinished, None)
        ]
    );
    // CAN aborts: nothing
    feed(&mut t, "\x1b]133;C\x18");
    assert!(marks(&mut t).is_empty());
    assert!(!running(&t));
}

#[test]
fn the_payload_cap_is_exact_and_discards_whole() {
    // body `133;C;` + n: 64 bytes in all is accepted, 65 is not
    let ok = "a".repeat(MAX_PAYLOAD_BYTES - "C;".len());
    let mut t = terminal();
    feed(&mut t, &format!("\x1b]133;C;{ok}\x07"));
    assert!(running(&t));
    let mut t = terminal();
    feed(&mut t, &format!("\x1b]133;C;{ok}a\x07"));
    assert!(!running(&t));
    assert!(marks(&mut t).is_empty());
    // never terminated, then the next sequence ends it (ESC) and starts a valid one
    let mut t = terminal();
    feed(&mut t, &format!("\x1b]133;{}", "9".repeat(200)));
    feed(&mut t, "\x1b]133;C\x1b\\");
    assert!(running(&t));
    assert_eq!(marks(&mut t), [mark(CommandMarkKind::OutputStart, None)]);
}

#[test]
fn every_split_and_one_byte_writes_match_the_whole_write() {
    for full in ["\x1b]133;C\x1b\\", "\x1b]133;D;3\x1b\\", "\x1b]133;A\x07"] {
        let mut whole = terminal();
        feed(&mut whole, "\x1b]133;C\x1b\\");
        feed(&mut whole, full);
        let want = whole.drain_events();
        for split in 0..=full.len() {
            let mut t = terminal();
            feed(&mut t, "\x1b]133;C\x1b\\");
            t.write(full.as_bytes().get(..split).unwrap());
            t.write(full.as_bytes().get(split..).unwrap());
            assert_eq!(t.drain_events(), want, "{full:?} at {split}");
        }
        let mut t = terminal();
        feed(&mut t, "\x1b]133;C\x1b\\");
        for byte in full.bytes() {
            t.write(&[byte]);
        }
        assert_eq!(t.drain_events(), want, "{full:?} bytewise");
    }
}

// --- the 2026 latch ---------------------------------------------------------------------------

fn sync_events(t: &mut Terminal) -> usize {
    t.drain_events()
        .iter()
        .filter(|e| **e == HostEvent::SyncUpdateInCommand)
        .count()
}

/// C, then `sequence`: the latch and the number of `SyncUpdateInCommand` events.
fn after_c(sequence: &str) -> (bool, usize) {
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x1b\\");
    t.drain_events();
    feed(&mut t, sequence);
    let latched = latch(&t);
    (latched, sync_events(&mut t))
}

#[test]
fn the_latch_sets_after_c_and_queues_one_event() {
    assert_eq!(after_c("\x1b[?2026h"), (true, 1));
    // a second set in the same command queues nothing more
    assert_eq!(after_c("\x1b[?2026h\x1b[?2026l\x1b[?2026h"), (true, 1));
    assert_eq!(after_c("\x1b[?2026;2026h"), (true, 1));
}

#[test]
fn lists_set_the_latch_like_lunegit() {
    for sequence in [
        "\x1b[?2026;1h",
        "\x1b[?1;2026h",
        "\x1b[?1;2;2026;3h",
        "\x1b[?;2026h",
        "\x1b[?2026;h",
        "\x1b[?1;2;3;4;5;6;7;8;9;10;2026h",
        "\x1b[?100000;2026h",
        "\x1b[?100001;2026h",
        "\x1b[?99999999999;2026h",
        "\x1b[?02026h",
        "\x1b[?1;2026;1049h",
    ] {
        assert_eq!(after_c(sequence), (true, 1), "{sequence:?}");
    }
}

#[test]
fn other_sequences_do_not_latch() {
    for sequence in [
        "\x1b[?2026l",
        "\x1b[2026h",
        "\x1b[?1049h",
        "\x1b[?2026$p",
        "\x1b[?20260h",
        "\x1b[?12026h",
        "\x1b[?2026:1h",
        "\x1b[?202:6h",
        "\x1b[?:2026h",
        "\x1b[?4294969322h",
        "\x1b[?2025h",
        "\x1b[?2027h",
        "\x1b[?25h",
        "\x1b[>2026h",
        "\x1b[<2026h",
        "\x1b[=2026h",
        "\x1b[1;?2026h",
        "\x1b[?2026c",
        // an intermediate makes it another function (the one deliberate difference)
        "\x1b[?2026 h",
    ] {
        assert_eq!(after_c(sequence), (false, 0), "{sequence:?}");
    }
}

#[test]
fn c_clears_the_latch_and_d_and_a_end_it() {
    let mut t = terminal();
    feed(&mut t, "\x1b[?2026h");
    assert!(latch(&t), "set before any command");
    assert_eq!(sync_events(&mut t), 0, "no command is running");
    feed(&mut t, "\x1b]133;C\x1b\\");
    assert!(!latch(&t), "a new command starts clean");
    feed(&mut t, "\x1b[?2026h");
    assert!(latch(&t));
    feed(&mut t, "\x1b]133;D\x1b\\");
    assert!(!latch(&t));
    feed(&mut t, "\x1b]133;C\x1b\\\x1b[?2026h\x1b]133;A\x1b\\");
    assert!(!latch(&t));
    // one event per command
    assert_eq!(sync_events(&mut t), 2);
}

#[test]
fn same_chunk_order_decides() {
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x1b\\\x1b[?2026h");
    assert!(latch(&t));
    assert_eq!(sync_events(&mut t), 1);
    let mut t = terminal();
    feed(&mut t, "\x1b[?2026h\x1b]133;C\x1b\\");
    assert!(!latch(&t));
    assert_eq!(sync_events(&mut t), 0);
    // two commands in one write
    let mut t = terminal();
    feed(
        &mut t,
        "\x1b]133;C\x07\x1b[?2026h\x1b]133;D\x07\x1b]133;C\x07\x1b[?2026h",
    );
    assert_eq!(sync_events(&mut t), 2);
}

#[test]
fn the_latch_is_chunk_invariant_at_every_split() {
    let full = "\x1b[?1;2026;1049h";
    let mut whole = terminal();
    feed(&mut whole, "\x1b]133;C\x1b\\");
    feed(&mut whole, full);
    let want = whole.drain_events();
    for split in 0..=full.len() {
        let mut t = terminal();
        feed(&mut t, "\x1b]133;C\x1b\\");
        t.write(full.as_bytes().get(..split).unwrap());
        t.write(full.as_bytes().get(split..).unwrap());
        assert_eq!(t.drain_events(), want, "split {split}");
        assert!(latch(&t));
    }
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x1b\\");
    for byte in full.bytes() {
        t.write(&[byte]);
    }
    assert_eq!(t.drain_events(), want);
}

#[test]
fn a_csi_cut_by_an_osc_does_not_latch() {
    let mut t = terminal();
    feed(&mut t, "\x1b]133;C\x1b\\\x1b[?20");
    feed(&mut t, "\x1b]0;title\x07");
    feed(&mut t, "2668");
    assert!(!latch(&t));
}

#[test]
fn the_observer_leaves_the_core_decset_alone() {
    // DECSET ?25 (cursor), ?1 (application cursor keys) and ?2026 in one list: the core still
    // handles the others
    let mut t = terminal();
    feed(&mut t, "\x1b[?25l");
    assert!(!t.viewport().cursor.visible);
    feed(&mut t, "\x1b[?25;2026h");
    assert!(t.viewport().cursor.visible);
    feed(&mut t, "\x1b[?1;2026h");
    assert!(t.modes().application_cursor_keys);
}

#[test]
fn mark_events_are_never_merged_or_dropped() {
    let mut t = terminal();
    for _ in 0..300 {
        feed(
            &mut t,
            "\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;1\x07",
        );
    }
    assert_eq!(marks(&mut t).len(), 1200);
}
