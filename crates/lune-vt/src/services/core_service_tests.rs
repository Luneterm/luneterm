//! Tests for `core_service.rs`: `CoreService.test.ts` (reset keeps `is_cursor_initialized`) plus
//! the data/binary event rules from `CoreService.ts`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::services::buffer_service::BufferService;

fn fixture(opts: &TerminalOptions) -> (CoreService, BufferService, LogService) {
    (
        CoreService::new(),
        BufferService::new(opts),
        LogService::default(),
    )
}

#[test]
fn reset_should_not_affect_is_cursor_initialized() {
    let mut core = CoreService::new();
    core.is_cursor_initialized = true;
    core.reset();
    assert!(core.is_cursor_initialized);
    core.is_cursor_initialized = false;
    core.reset();
    assert!(!core.is_cursor_initialized);
}

#[test]
fn defaults_match_upstream() {
    let core = CoreService::new();
    assert!(!core.is_cursor_hidden);
    assert!(!core.modes.insert_mode);
    let dec = core.dec_private_modes;
    assert!(dec.wraparound);
    assert!(!dec.application_cursor_keys && !dec.application_keypad && !dec.bracketed_paste_mode);
    assert!(!dec.origin && !dec.reverse_wraparound && !dec.send_focus && !dec.synchronized_output);
    assert_eq!(dec.cursor_blink, None);
    assert_eq!(dec.cursor_style, None);
}

#[test]
fn reset_restores_modes() {
    let mut core = CoreService::new();
    core.modes.insert_mode = true;
    core.dec_private_modes.wraparound = false;
    core.dec_private_modes.cursor_blink = Some(true);
    core.dec_private_modes.cursor_style = Some(CursorStyle::Bar);
    core.is_cursor_hidden = true;
    core.reset();
    assert_eq!(core.modes, Modes::default());
    assert_eq!(core.dec_private_modes, DecPrivateModes::default());
    assert!(
        core.is_cursor_hidden,
        "reset does not touch the cursor flags"
    );
}

#[test]
fn data_event_plain() {
    let opts = TerminalOptions::default();
    let (mut core, bs, log) = fixture(&opts);
    let ev = core
        .trigger_data_event("abc", false, bs.buffer(), &opts, &log)
        .unwrap();
    assert_eq!(
        ev,
        DataEvent {
            data: "abc".to_string(),
            scroll_to_bottom: false,
            user_input: false
        }
    );
}

#[test]
fn data_event_user_input_requests_scroll_only_when_scrolled_up() {
    let opts = TerminalOptions::default();
    let (mut core, mut bs, log) = fixture(&opts);
    bs.buffer_mut().ybase = 5;
    bs.buffer_mut().ydisp = 5;
    let at_bottom = core
        .trigger_data_event("a", true, bs.buffer(), &opts, &log)
        .unwrap();
    assert!(at_bottom.user_input && !at_bottom.scroll_to_bottom);

    bs.buffer_mut().ydisp = 2;
    let scrolled = core
        .trigger_data_event("a", true, bs.buffer(), &opts, &log)
        .unwrap();
    assert!(scrolled.user_input && scrolled.scroll_to_bottom);

    let not_user = core
        .trigger_data_event("a", false, bs.buffer(), &opts, &log)
        .unwrap();
    assert!(!not_user.user_input && !not_user.scroll_to_bottom);

    let no_scroll_opt = TerminalOptions {
        scroll_on_user_input: false,
        ..TerminalOptions::default()
    };
    let ev = core
        .trigger_data_event("a", true, bs.buffer(), &no_scroll_opt, &log)
        .unwrap();
    assert!(ev.user_input && !ev.scroll_to_bottom);
}

#[test]
fn disable_stdin_drops_data_and_binary() {
    let opts = TerminalOptions {
        disable_stdin: true,
        ..TerminalOptions::default()
    };
    let (mut core, bs, log) = fixture(&opts);
    assert_eq!(
        core.trigger_data_event("a", true, bs.buffer(), &opts, &log),
        None
    );
    assert_eq!(core.trigger_binary_event("a", &opts, &log), None);
}

#[test]
fn binary_event_passes_the_string_through() {
    let opts = TerminalOptions::default();
    let (mut core, _bs, log) = fixture(&opts);
    assert_eq!(
        core.trigger_binary_event("\x1b[M !!", &opts, &log),
        Some("\x1b[M !!".to_string())
    );
}

/// `// LUNETERM:` (user decision 2026-11-02): the events log the length, never the data.
#[test]
fn events_log_the_length_never_the_data() {
    use crate::services::log_service::Logger;
    use crate::services::options_service::LogLevel;
    use std::sync::{Arc, Mutex};

    struct Capture(Arc<Mutex<Vec<String>>>);
    impl Logger for Capture {
        fn log(&self, _level: LogLevel, message: &str) {
            self.0.lock().unwrap().push(message.to_string());
        }
    }
    let opts = TerminalOptions::default();
    let (mut core, bs, mut log) = fixture(&opts);
    let messages = Arc::new(Mutex::new(Vec::new()));
    log.set_logger(Some(Box::new(Capture(Arc::clone(&messages)))));
    log.set_level(LogLevel::Trace);
    core.trigger_data_event("hunter2\u{e9}", true, bs.buffer(), &opts, &log);
    core.trigger_binary_event("\x1b[Msecret\u{ff}", &opts, &log);
    let messages = messages.lock().unwrap();
    assert_eq!(
        *messages,
        vec![
            "sending data (9 bytes)".to_string(),
            "sending binary (10 bytes)".to_string()
        ]
    );
}
