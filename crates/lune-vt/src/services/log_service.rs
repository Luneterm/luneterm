//! Port of xterm.js src/common/services/LogService.ts @ f447274 (upstream has no test file;
//! `log_service_tests.rs` is written from the TypeScript behaviour).
//!
//! Rust notes (see `docs/porting-xterm.md`, plan M6 D4):
//!  - A library never prints. Upstream's default sink is `console` (prefixed `xterm.js: `) and
//!    `options.logger` overrides it unprefixed; here the sink is an optional [`Logger`] set with
//!    [`LogService::set_logger`], and with no sink every message is discarded. Messages given to a
//!    [`Logger`] are unprefixed, as upstream's with `options.logger`.
//!  - Upstream's lazy optional parameters (`_evalLazyOptionalParams`: functions are called only
//!    when the level passes) become [`fmt::Arguments`]: the message is formatted only when the
//!    level passes and a sink is set.
//!  - Upstream follows `options.logLevel` through `onSpecificOptionChange`; the host calls
//!    [`LogService::set_level`] when that option changes.
//!  - `setTraceLogger` / `traceCall` (a renderer debug decorator) are not ported.

use crate::services::options_service::LogLevel;
use std::fmt;

/// A log sink (upstream `options.logger`).
pub trait Logger {
    /// Receives one message at `level`.
    fn log(&self, level: LogLevel, message: &str);
}

/// Position in the severity order `trace < debug < info < warn < error < off`.
fn rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Trace => 0,
        LogLevel::Debug => 1,
        LogLevel::Info => 2,
        LogLevel::Warn => 3,
        LogLevel::Error => 4,
        LogLevel::Off => 5,
    }
}

/// Level-gated logging to an optional sink.
pub struct LogService {
    level: LogLevel,
    logger: Option<Box<dyn Logger>>,
}

impl fmt::Debug for LogService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogService")
            .field("level", &self.level)
            .field("has_logger", &self.logger.is_some())
            .finish()
    }
}

impl LogService {
    /// A service at `level` with no sink.
    pub fn new(level: LogLevel) -> Self {
        LogService {
            level,
            logger: None,
        }
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }

    /// Sets (or removes) the sink.
    pub fn set_logger(&mut self, logger: Option<Box<dyn Logger>>) {
        self.logger = logger;
    }

    fn emit(&self, at: LogLevel, args: fmt::Arguments<'_>) {
        if rank(self.level) <= rank(at) {
            if let Some(logger) = &self.logger {
                logger.log(at, &args.to_string());
            }
        }
    }

    pub fn trace(&self, args: fmt::Arguments<'_>) {
        self.emit(LogLevel::Trace, args);
    }

    pub fn debug(&self, args: fmt::Arguments<'_>) {
        self.emit(LogLevel::Debug, args);
    }

    pub fn info(&self, args: fmt::Arguments<'_>) {
        self.emit(LogLevel::Info, args);
    }

    pub fn warn(&self, args: fmt::Arguments<'_>) {
        self.emit(LogLevel::Warn, args);
    }

    pub fn error(&self, args: fmt::Arguments<'_>) {
        self.emit(LogLevel::Error, args);
    }
}

impl Default for LogService {
    /// Level `info` (the `logLevel` default), no sink.
    fn default() -> Self {
        LogService::new(LogLevel::Info)
    }
}

#[cfg(test)]
#[path = "log_service_tests.rs"]
mod tests;
