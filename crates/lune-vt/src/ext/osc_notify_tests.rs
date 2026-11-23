//! Tests for the notification extension (bell-notifications plan step 1). There is no upstream
//! test file: xterm.js has none of this. Each case feeds raw bytes to a [`Terminal`] with the
//! standard extensions and reads `drain_events()`. The grammar is in `osc_notify.rs`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use super::osc_notify::MAX_PAYLOAD_BYTES;
use super::*;
use crate::engine::{HostEvent, Notification, Progress, ProgressState};
use crate::headless::terminal::Terminal;
use crate::input_handler::NOTIFICATION_LIMIT;
use crate::services::log_service::Logger;
use crate::services::options_service::{LogLevel, OptionValue};

fn terminal_with(notifications: bool) -> Terminal {
    let mut t = Terminal::new([]);
    t.install_standard_extensions(&StandardExtensions {
        notifications,
        ..StandardExtensions::default()
    })
    .unwrap();
    t
}

fn terminal() -> Terminal {
    terminal_with(true)
}

fn note(title: &str, body: &str) -> HostEvent {
    HostEvent::Notification(Notification {
        title: title.to_owned(),
        body: body.to_owned(),
    })
}

/// The events of one `ESC ] <body> BEL`.
fn events_of(body: &str) -> Vec<HostEvent> {
    let mut t = terminal();
    t.write(format!("\x1b]{body}\x07").as_bytes());
    t.drain_events()
}

#[test]
fn osc9_message_with_bel_and_st() {
    assert_eq!(events_of("9;hello world"), [note("", "hello world")]);
    let mut t = terminal();
    t.write(b"\x1b]9;hello world\x1b\\");
    assert_eq!(t.drain_events(), [note("", "hello world")]);
}

#[test]
fn osc9_sub_commands_keep_their_own_events() {
    assert_eq!(
        events_of("9;4;1;50"),
        [HostEvent::Progress(Progress {
            state: ProgressState::Normal,
            percent: 50
        })]
    );
    assert_eq!(
        events_of("9;9;/tmp"),
        [HostEvent::CwdChanged("/tmp".to_owned())]
    );
}

#[test]
fn osc9_numbered_forms_and_empty_are_not_messages() {
    for body in [
        "9;4", "9;12", "9;2;msg", "9;404", "9;", "9", "9;4;x;5", "9;0;",
    ] {
        assert_eq!(events_of(body), [], "{body:?}");
    }
}

#[test]
fn osc9_message_that_starts_with_a_digit_or_has_a_non_numeric_first_field() {
    assert_eq!(events_of("9;3 tests failed"), [note("", "3 tests failed")]);
    assert_eq!(events_of("9;v2;x"), [note("", "v2;x")]);
    assert_eq!(events_of("9;;x"), [note("", ";x")]);
    assert_eq!(events_of("9; 4"), [note("", " 4")]);
}

#[test]
fn osc777_notify_title_and_body() {
    assert_eq!(
        events_of("777;notify;Title;Body; with; semis"),
        [note("Title", "Body; with; semis")]
    );
    assert_eq!(events_of("777;notify;Only"), [note("Only", "")]);
    assert_eq!(events_of("777;notify;;Body"), [note("", "Body")]);
}

#[test]
fn osc777_other_forms_produce_nothing_not_even_unknown_osc() {
    for body in [
        "777;notify",
        "777;notify;",
        "777;notify;;",
        "777;precmd",
        "777;Notify;a;b",
        "777;",
        "777",
    ] {
        assert_eq!(events_of(body), [], "{body:?}");
    }
}

#[test]
fn the_payload_cap_counts_bytes_and_discards_whole() {
    let exact = "a".repeat(MAX_PAYLOAD_BYTES);
    assert_eq!(events_of(&format!("9;{exact}")), [note("", &exact)]);
    assert_eq!(events_of(&format!("9;{exact}a")), []);
    // OSC 777: `notify;;` takes 8 bytes of the 4096
    let body = "b".repeat(MAX_PAYLOAD_BYTES - "notify;;".len());
    assert_eq!(events_of(&format!("777;notify;;{body}")), [note("", &body)]);
    assert_eq!(events_of(&format!("777;notify;;{body}b")), []);
    // multi-byte: 2048 two-byte characters fit exactly, one more does not
    let wide = "é".repeat(MAX_PAYLOAD_BYTES / 2);
    assert_eq!(events_of(&format!("9;{wide}")), [note("", &wide)]);
    assert_eq!(events_of(&format!("9;{wide}é")), []);
    // a 3-byte character straddling the cap
    let mixed = format!("{}€", "a".repeat(MAX_PAYLOAD_BYTES - 2));
    assert_eq!(events_of(&format!("9;{mixed}")), []);
    assert_eq!(events_of("9;日本語 ✓"), [note("", "日本語 ✓")]);
}

#[test]
fn an_oversize_payload_does_not_poison_the_next_one() {
    let mut t = terminal();
    t.write(format!("\x1b]9;{}\x07", "x".repeat(MAX_PAYLOAD_BYTES + 1)).as_bytes());
    assert_eq!(t.drain_events(), []);
    t.write(b"\x1b]9;after\x07\x1b]777;notify;T;B\x07");
    assert_eq!(t.drain_events(), [note("", "after"), note("T", "B")]);
}

#[test]
fn an_osc_cut_by_can_emits_nothing() {
    for body in ["9;hello", "777;notify;a;b"] {
        let mut t = terminal();
        t.write(format!("\x1b]{body}\x18").as_bytes());
        assert_eq!(t.drain_events(), [], "{body:?}");
        // and the parser is usable afterwards
        t.write(b"\x1b]9;ok\x07");
        assert_eq!(t.drain_events(), [note("", "ok")], "{body:?}");
    }
}

#[test]
fn at_most_eight_notifications_per_drain() {
    assert_eq!(NOTIFICATION_LIMIT, 8);
    let mut t = terminal();
    let mut bytes = String::new();
    for i in 0..13 {
        bytes.push_str(&format!("\x1b]9;m{i}\x07"));
    }
    t.write(bytes.as_bytes());
    let events = t.drain_events();
    let expected: Vec<HostEvent> = (0..8).map(|i| note("", &format!("m{i}"))).collect();
    assert_eq!(events, expected);
    // the counter resets at the drain
    t.write(b"\x1b]777;notify;next;one\x07");
    assert_eq!(t.drain_events(), [note("next", "one")]);
}

#[test]
fn the_limit_counts_both_forms_and_leaves_other_events_alone() {
    let mut t = terminal();
    let mut bytes = String::new();
    for i in 0..6 {
        bytes.push_str(&format!("\x1b]9;m{i}\x07\x1b]777;notify;t{i};b\x07\x07"));
    }
    t.write(bytes.as_bytes());
    let events = t.drain_events();
    let notes = events
        .iter()
        .filter(|e| matches!(e, HostEvent::Notification(_)))
        .count();
    let bells = events.iter().filter(|e| **e == HostEvent::Bell).count();
    assert_eq!((notes, bells), (8, 6));
}

#[test]
fn a_mixed_stream_split_at_every_byte_boundary_gives_the_same_events() {
    let stream: &[u8] = "\x1b]9;one\x07x\x1b]9;4;1;20\x1b\\\x1b]777;notify;T;B; c\x07\
        \x1b]9;9;/tmp\x07\x1b]9;日本\x1b\\\x1b]777;precmd\x07\x1b]9;12\x07\x07\
        \x1b]777;notify;;only body\x1b\\"
        .as_bytes();
    let mut whole = terminal();
    whole.write(stream);
    let want = whole.drain_events();
    assert!(
        want.iter()
            .filter(|e| matches!(e, HostEvent::Notification(_)))
            .count()
            >= 4
    );
    for cut in 1..stream.len() {
        let mut t = terminal();
        t.write(&stream[..cut]);
        let mut got = t.drain_events();
        t.write(&stream[cut..]);
        got.extend(t.drain_events());
        assert_eq!(got, want, "cut at {cut}");
    }
}

#[test]
fn flag_off_is_todays_behaviour() {
    let mut t = terminal_with(false);
    t.write(b"\x1b]777;notify;a;b\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc {
            ident: 777,
            len: 10
        }]
    );
    t.write(b"\x1b]9;hello\x07");
    assert_eq!(t.drain_events(), []);
    // the default is off
    assert!(!StandardExtensions::default().notifications);
    let mut t = Terminal::new([]);
    t.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    t.write(b"\x1b]777;notify;a;b\x07\x1b]9;hello\x07");
    assert_eq!(
        t.drain_events(),
        [HostEvent::UnknownOsc {
            ident: 777,
            len: 10
        }]
    );
}

struct Capture(Arc<Mutex<Vec<(LogLevel, String)>>>);

impl Logger for Capture {
    fn log(&self, level: LogLevel, message: &str) {
        self.0.lock().unwrap().push((level, message.to_owned()));
    }
}

#[test]
fn the_text_is_never_logged() {
    let mut t = Terminal::new([OptionValue::LogLevel(LogLevel::Trace)]);
    t.install_standard_extensions(&StandardExtensions {
        notifications: true,
        ..StandardExtensions::default()
    })
    .unwrap();
    let messages = Arc::new(Mutex::new(Vec::new()));
    t.set_logger(Some(Box::new(Capture(Arc::clone(&messages)))));
    t.write(b"\x1b]9;hunter2\x07\x1b]777;notify;hunter2;hunter2\x07");
    // over the cap, both identifiers, and a cut one
    let long = "hunter2".repeat(MAX_PAYLOAD_BYTES / 7 + 2);
    t.write(format!("\x1b]9;{long}\x07\x1b]777;notify;{long}\x07\x1b]9;hunter2\x18").as_bytes());
    // the events themselves are there (the instrument sees the text)
    assert_eq!(
        t.drain_events(),
        [note("", "hunter2"), note("hunter2", "hunter2")]
    );
    let messages = messages.lock().unwrap();
    assert!(
        messages.iter().all(|(_, m)| !m.contains("hunter2")),
        "{messages:?}"
    );
    // the discard is logged once per payload, with a length
    assert!(
        messages
            .iter()
            .any(|(l, m)| *l == LogLevel::Debug && m.contains("discarded") && m.contains("4096")),
        "{messages:?}"
    );
}
