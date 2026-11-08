//! Tests for `log_service.rs`, written from `LogService.ts` (upstream has no test file): the
//! level gate for each method, lazy formatting, and the sink.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use std::sync::{Arc, Mutex};

type Captured = Arc<Mutex<Vec<(LogLevel, String)>>>;

struct Capture(Captured);

impl Logger for Capture {
    fn log(&self, level: LogLevel, message: &str) {
        self.0.lock().unwrap().push((level, message.to_string()));
    }
}

/// Formatting this panics: proves a message was never formatted.
struct Boom;

impl fmt::Display for Boom {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("formatted below the level");
    }
}

fn service(level: LogLevel) -> (LogService, Captured) {
    let captured = Captured::default();
    let mut log = LogService::new(level);
    log.set_logger(Some(Box::new(Capture(captured.clone()))));
    (log, captured)
}

fn emit_all(log: &LogService) {
    log.trace(format_args!("t"));
    log.debug(format_args!("d"));
    log.info(format_args!("i"));
    log.warn(format_args!("w"));
    log.error(format_args!("e"));
}

fn seen(captured: &Captured) -> Vec<String> {
    captured
        .lock()
        .unwrap()
        .iter()
        .map(|(_, message)| message.clone())
        .collect()
}

#[test]
fn only_messages_at_or_above_the_level_reach_the_logger() {
    let cases = [
        (LogLevel::Trace, "tdiwe"),
        (LogLevel::Debug, "diwe"),
        (LogLevel::Info, "iwe"),
        (LogLevel::Warn, "we"),
        (LogLevel::Error, "e"),
        (LogLevel::Off, ""),
    ];
    for (level, expected) in cases {
        let (log, captured) = service(level);
        emit_all(&log);
        assert_eq!(seen(&captured).concat(), expected, "level {level:?}");
    }
}

#[test]
fn logger_receives_the_level_and_an_unprefixed_message() {
    let (log, captured) = service(LogLevel::Trace);
    log.warn(format_args!("x {} y", 5));
    assert_eq!(
        captured.lock().unwrap().as_slice(),
        &[(LogLevel::Warn, "x 5 y".to_string())]
    );
}

#[test]
fn arguments_below_the_level_are_never_formatted() {
    let (log, captured) = service(LogLevel::Info);
    log.trace(format_args!("{}", Boom));
    log.debug(format_args!("{}", Boom));
    assert!(captured.lock().unwrap().is_empty());
    let (off, _) = service(LogLevel::Off);
    off.error(format_args!("{}", Boom));
}

#[test]
fn no_sink_discards_without_formatting() {
    let log = LogService::new(LogLevel::Trace);
    log.error(format_args!("{}", Boom));
}

#[test]
fn set_level_and_set_logger() {
    let (mut log, captured) = service(LogLevel::Error);
    log.info(format_args!("a"));
    assert!(captured.lock().unwrap().is_empty());
    log.set_level(LogLevel::Info);
    assert_eq!(log.level(), LogLevel::Info);
    log.info(format_args!("b"));
    assert_eq!(seen(&captured), ["b"]);
    log.set_logger(None);
    log.info(format_args!("c"));
    assert_eq!(seen(&captured), ["b"]);
}

#[test]
fn default_level_is_info() {
    assert_eq!(LogService::default().level(), LogLevel::Info);
}
