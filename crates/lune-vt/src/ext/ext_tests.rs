//! Tests for the extension infrastructure, XTVERSION and the unknown-OSC report (plan M7
//! step 2). There is no upstream test file: xterm.js has none of these. The XTVERSION cases are
//! Lunegit's `XtermJsVtEngineXtVersionTests.cs` (9 Fact; their literals are verified against
//! real xterm.js in V8 by Lunegit's suite), with the reply built from the host-supplied name
//! (plan M7 D10) in place of `TerminalIdentity.XtVersionReply`:
//!  - ported: `Csi_Gt_0_q_RepliesWithTheDcsIdentity`, `Csi_Gt_q_WithNoParam_IsTheSameQuery`,
//!    `Csi_Gt_1_q_IsNotXtversion_AndStaysSilent`, `Csi_Gt_MultipleParams_StaySilent`,
//!    `InjectedReply_IsEchoedVerbatim`, `EmptyReply_DeclinesAndStaysSilent` (as `xtversion: None`
//!    and the invalid names, which are refused at install),
//!    `Reply_IsOrderedAfterAnEarlierDaReplyInTheSameWrite` (as two events in order, not one
//!    concatenated host response), `Query_PrintsNothingToTheGrid`,
//!    `Query_DoesNotWedgeSubsequentWrites`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::engine::HostEvent;
use crate::headless::terminal::Terminal;
use crate::input_handler::UNKNOWN_OSC_LIMIT;
use crate::services::log_service::Logger;
use crate::services::options_service::{LogLevel, OptionValue};

const NAME: &str = "TestTerm(9.9.9)";
const REPLY: &[u8] = b"\x1bP>|TestTerm(9.9.9)\x1b\\";

fn terminal() -> Terminal {
    Terminal::new([OptionValue::Cols(40), OptionValue::Rows(4)])
}

fn installed(name: Option<&str>) -> Terminal {
    let mut t = terminal();
    t.install_standard_extensions(&StandardExtensions {
        xtversion: name.map(str::to_owned),
    })
    .unwrap();
    t
}

fn responses(t: &mut Terminal) -> Vec<Vec<u8>> {
    t.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::Response(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

fn row_text(t: &Terminal, y: usize) -> String {
    t.buffers()
        .active()
        .translate_buffer_line_to_string(y, true, 0, None)
}

// --- installation ----------------------------------------------------------------------------

#[test]
fn install_twice_is_refused_and_registers_nothing_twice() {
    let mut t = installed(Some(NAME));
    let second = t.install_standard_extensions(&StandardExtensions {
        xtversion: Some("Other".to_owned()),
    });
    assert_eq!(second, Err(ExtensionError::AlreadyInstalled));
    // the first registration answers once, with the first name
    t.write_str("\x1b[>0q");
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
    // and a third attempt changes nothing either
    assert_eq!(
        t.install_standard_extensions(&StandardExtensions::default()),
        Err(ExtensionError::AlreadyInstalled)
    );
    t.write_str("\x1b]1337;x\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc {
            ident: 1337,
            len: 1
        }]
    );
}

#[test]
fn a_terminal_that_never_installs_behaves_as_xterm() {
    let mut t = terminal();
    t.write_str("\x1b[>0q\x1b]1337;x\x07");
    assert_eq!(t.drain_events(), []);
    assert_eq!(t.modes().kitty_keyboard_flags, 0);
}

#[test]
fn invalid_names_are_refused_and_nothing_is_registered() {
    let long = "x".repeat(65);
    for name in [
        "",
        long.as_str(),
        "a\u{1b}b",
        "a\u{7f}b",
        "caf\u{e9}",
        "a\nb",
        "a\u{1b}\\",
    ] {
        let mut t = terminal();
        let result = t.install_standard_extensions(&StandardExtensions {
            xtversion: Some(name.to_owned()),
        });
        assert_eq!(result, Err(ExtensionError::InvalidXtversion), "{name:?}");
        // nothing was registered, and the terminal is not marked installed: a valid install works
        t.write_str("\x1b[>0q\x1b]1337;x\x07");
        assert_eq!(t.drain_events(), [], "{name:?}");
        t.install_standard_extensions(&StandardExtensions {
            xtversion: Some(NAME.to_owned()),
        })
        .unwrap();
    }
}

#[test]
fn names_at_the_limits_are_accepted() {
    for name in ["a", " ", "~", &"x".repeat(64)] {
        let mut t = installed(Some(name));
        t.write_str("\x1b[>q");
        assert_eq!(
            responses(&mut t),
            [format!("\x1bP>|{name}\x1b\\").into_bytes()]
        );
    }
}

#[test]
fn extension_error_displays() {
    assert!(!ExtensionError::AlreadyInstalled.to_string().is_empty());
    assert!(!ExtensionError::InvalidXtversion.to_string().is_empty());
    assert!(
        ExtensionError::from(crate::parser::types::IdentifierError::PrefixRange)
            .to_string()
            .contains("identifier")
    );
}

// --- XTVERSION (XtermJsVtEngineXtVersionTests.cs) --------------------------------------------

#[test]
fn csi_gt_0_q_replies_with_the_dcs_identity() {
    let mut t = installed(Some(NAME));
    t.write_str("\x1b[>0q");
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
}

#[test]
fn csi_gt_q_with_no_param_is_the_same_query() {
    let mut t = installed(Some(NAME));
    t.write_str("\x1b[>q");
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
}

#[test]
fn csi_gt_1_q_is_not_xtversion_and_stays_silent() {
    let mut t = installed(Some(NAME));
    t.write_str("\x1b[>1q");
    assert_eq!(responses(&mut t), Vec::<Vec<u8>>::new());
    // positive re-check: the handler still exists
    t.write_str("\x1b[>0q");
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
}

#[test]
fn csi_gt_multiple_params_stay_silent() {
    let mut t = installed(Some(NAME));
    for silent in [
        "\x1b[>0;1q",
        "\x1b[>0:1q",
        "\x1b[>0;0q",
        "\x1b[>;q",
        "\x1b[>0:0q",
    ] {
        t.write_str(silent);
        assert_eq!(responses(&mut t), Vec::<Vec<u8>>::new(), "{silent:?}");
    }
    t.write_str("\x1b[>0q");
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
}

#[test]
fn injected_reply_is_echoed_verbatim() {
    let mut t = installed(Some("TestTerm(9.9.9)"));
    t.write_str("\x1b[>0q");
    assert_eq!(
        responses(&mut t),
        [b"\x1bP>|TestTerm(9.9.9)\x1b\\".to_vec()]
    );
}

#[test]
fn no_name_declines_and_stays_silent() {
    let mut t = installed(None);
    t.write_str("\x1b[>0q\x1b[>q");
    assert_eq!(t.drain_events(), []);
}

#[test]
fn reply_is_ordered_after_an_earlier_da_reply_in_the_same_write() {
    let mut t = installed(Some(NAME));
    t.write_str("\x1b[c\x1b[>0q");
    let replies = responses(&mut t);
    assert_eq!(replies.len(), 2);
    assert!(replies[0].starts_with(b"\x1b[?"), "{:?}", replies[0]);
    assert_eq!(replies[1], REPLY);

    // and the other way round
    t.write_str("\x1b[>0q\x1b[c");
    let replies = responses(&mut t);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0], REPLY);
    assert!(replies[1].starts_with(b"\x1b[?"));
}

#[test]
fn query_prints_nothing_to_the_grid() {
    let mut t = installed(Some(NAME));
    t.write_str("\x1b[>0q");
    assert_eq!(row_text(&t, 0), "");
    assert_eq!(t.buffers().active().x, 0);
    // the handler fired (grid silence is also what no handler looks like)
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
}

#[test]
fn query_does_not_wedge_subsequent_writes() {
    let mut t = installed(Some(NAME));
    t.write_str("\x1b[>0q");
    t.write_str("hello");
    assert_eq!(row_text(&t, 0), "hello");
    assert_eq!(responses(&mut t), [REPLY.to_vec()]);
}

#[test]
fn query_is_chunk_invariant() {
    let mut t = installed(Some(NAME));
    for byte in "\x1b[c\x1b[>0q\x1b[>0:1q\x1b[>q".bytes() {
        t.write(&[byte]);
    }
    let replies = responses(&mut t);
    assert_eq!(replies.len(), 3);
    assert_eq!(
        (replies[1].as_slice(), replies[2].as_slice()),
        (REPLY, REPLY)
    );
}

#[test]
fn disable_stdin_suppresses_the_reply() {
    let mut t = installed(Some(NAME));
    t.set_option(OptionValue::DisableStdin(true)).unwrap();
    t.write_str("\x1b[>0q");
    assert_eq!(t.drain_events(), []);
}

// --- unknown OSC -----------------------------------------------------------------------------

#[test]
fn unknown_osc_reports_ident_and_payload_length() {
    let mut t = installed(None);
    t.write_str("\x1b]1337;x\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc {
            ident: 1337,
            len: 1
        }]
    );
    // length counts code points, not bytes; ST terminates as well as BEL; 777 is not handled
    t.write_str("\x1b]777;h\u{e9}\u{4e2d}\u{1f600}\x1b\\");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc { ident: 777, len: 4 }]
    );
    // no payload at all: `OSC 99 BEL`
    t.write_str("\x1b]99\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc { ident: 99, len: 0 }]
    );
}

#[test]
fn unknown_osc_is_chunk_invariant() {
    let stream = "\x1b]1337;ab\x07\x1b]777;h\u{e9}\u{4e2d}\u{1f600}\x1b\\x\x1b]99\x07";
    let mut whole = installed(None);
    whole.write_str(stream);
    let mut bytes = installed(None);
    for byte in stream.bytes() {
        bytes.write(&[byte]);
    }
    let expected = [
        HostEvent::UnknownOsc {
            ident: 1337,
            len: 2,
        },
        HostEvent::UnknownOsc { ident: 777, len: 4 },
        HostEvent::UnknownOsc { ident: 99, len: 0 },
    ];
    assert_eq!(whole.drain_events(), expected);
    assert_eq!(bytes.drain_events(), expected);
}

#[test]
fn an_osc_without_an_identifier_is_not_reported() {
    let mut t = installed(None);
    // identifier -1: no digits before the `;`, or no `;` at all
    t.write_str("\x1b];x\x07\x1b]\x07\x1b];\x07");
    assert_eq!(t.drain_events(), []);
    // an identifier with a non-digit aborts the sequence: nothing to report either
    t.write_str("\x1b]1x;y\x07");
    assert_eq!(t.drain_events(), []);
}

#[test]
fn handled_osc_idents_are_not_reported() {
    let mut t = installed(None);
    t.write_str("\x1b]0;title\x07\x1b]2;title\x07\x1b]8;;http://x\x07\x1b]8;;\x07");
    t.write_str("\x1b]4;1;?\x07\x1b]10;?\x07\x1b]104\x07");
    let events = t.drain_events();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, HostEvent::UnknownOsc { .. })),
        "{events:?}"
    );
}

#[test]
fn sixty_five_unknown_osc_in_one_write_queue_64_and_the_counter_resets_at_the_drain() {
    let mut t = installed(None);
    let mut stream = String::new();
    for i in 0..=UNKNOWN_OSC_LIMIT {
        stream.push_str(&format!("\x1b]{};{}\x07", 1000 + i, "p".repeat(i % 5)));
    }
    t.write_str(&stream);
    let events = t.drain_events();
    assert_eq!(events.len(), 64);
    assert_eq!(
        events[63],
        HostEvent::UnknownOsc {
            ident: 1063,
            len: 63 % 5
        }
    );
    // the first 64, in order
    for (i, event) in events.iter().enumerate() {
        assert_eq!(
            *event,
            HostEvent::UnknownOsc {
                ident: 1000 + i as i32,
                len: i % 5
            }
        );
    }
    // the next drain queues anew
    t.write_str("\x1b]5000;x\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc {
            ident: 5000,
            len: 1
        }]
    );
}

#[test]
fn unknown_osc_logs_ident_and_length_never_the_payload() {
    struct Capture(Rc<RefCell<Vec<String>>>);
    impl Logger for Capture {
        fn log(&self, _level: LogLevel, message: &str) {
            self.0.borrow_mut().push(message.to_owned());
        }
    }
    let mut t = Terminal::new([OptionValue::LogLevel(LogLevel::Debug)]);
    t.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    let messages = Rc::new(RefCell::new(Vec::new()));
    t.set_logger(Some(Box::new(Capture(Rc::clone(&messages)))));
    t.write_str("\x1b]777;c2VjcmV0\x07");
    let messages = messages.borrow();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("identifier: 777") && m.contains("length: 8")),
        "{messages:?}"
    );
    assert!(messages.iter().all(|m| !m.contains("c2VjcmV0")));
    // the report is the event, not a log line
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc { ident: 777, len: 8 }]
    );
}

// --- last-wins slots (the event queue) -------------------------------------------------------

fn push(t: &mut Terminal, event: HostEvent) {
    t.core_mut().input_handler_mut().push_event(event);
}

#[test]
fn clipboard_write_is_last_wins_at_the_first_ones_position() {
    let mut t = terminal();
    push(&mut t, HostEvent::Bell);
    push(&mut t, HostEvent::ClipboardWrite("first".to_owned()));
    push(&mut t, HostEvent::Bell);
    push(&mut t, HostEvent::ClipboardWrite("second".to_owned()));
    assert_eq!(
        t.drain_events(),
        [
            HostEvent::Bell,
            HostEvent::ClipboardWrite("second".to_owned()),
            HostEvent::Bell
        ]
    );
    // after a drain the next one queues anew
    push(&mut t, HostEvent::ClipboardWrite("third".to_owned()));
    assert_eq!(
        t.drain_events(),
        [HostEvent::ClipboardWrite("third".to_owned())]
    );
}

#[test]
fn cwd_and_progress_are_last_wins_independently() {
    use crate::engine::{Progress, ProgressState};
    let progress = |percent| {
        HostEvent::Progress(Progress {
            state: ProgressState::Normal,
            percent,
        })
    };
    let mut t = terminal();
    push(&mut t, HostEvent::CwdChanged("/a".to_owned()));
    push(&mut t, progress(10));
    push(&mut t, HostEvent::ClipboardWrite("c1".to_owned()));
    push(&mut t, HostEvent::CwdChanged("/b".to_owned()));
    push(&mut t, progress(20));
    push(&mut t, HostEvent::ClipboardWrite("c2".to_owned()));
    push(&mut t, HostEvent::CwdChanged("/c".to_owned()));
    assert_eq!(
        t.drain_events(),
        [
            HostEvent::CwdChanged("/c".to_owned()),
            progress(20),
            HostEvent::ClipboardWrite("c2".to_owned()),
        ]
    );
    push(&mut t, progress(30));
    push(&mut t, HostEvent::CwdChanged("/d".to_owned()));
    assert_eq!(
        t.drain_events(),
        [progress(30), HostEvent::CwdChanged("/d".to_owned())]
    );
}

#[test]
fn command_marks_and_sync_updates_are_never_merged() {
    use crate::engine::{CommandMark, CommandMarkKind};
    let mark = HostEvent::CommandMark(CommandMark {
        kind: CommandMarkKind::PromptStart,
        exit_code: None,
    });
    let mut t = terminal();
    for _ in 0..3 {
        push(&mut t, mark.clone());
        push(&mut t, HostEvent::SyncUpdateInCommand);
    }
    assert_eq!(t.drain_events().len(), 6);
}

#[test]
fn unknown_osc_cap_applies_to_every_pusher() {
    let mut t = terminal();
    for i in 0..200 {
        push(&mut t, HostEvent::UnknownOsc { ident: i, len: 0 });
    }
    assert_eq!(t.drain_events().len(), UNKNOWN_OSC_LIMIT);
}
