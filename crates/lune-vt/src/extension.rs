//! Extension point: sequence handlers registered on top of the ported core.
//!
//! An [`Extension`] registers handlers on the [`EscapeSequenceParser`] itself, the same registry
//! xterm.js's `parser.registerOscHandler` / `registerCsiHandler` / ... feed (plan M4, decision
//! D6): handlers for one identifier are tried newest-first, and the first that returns `true`
//! consumes the sequence. Built-in behaviour is registered first, so an extension can override
//! it without touching ported code. There is no second dispatch mechanism beside the parser's.
//!
//! **The CSI `t` guard does not apply here.** [`crate::input_handler::InputHandler::register_csi_handler`]
//! (what `Terminal::register_csi_handler` calls) wraps a plain CSI `t` handler so it runs only
//! for window options the `window_options` setting allows, as xterm.js's `registerCsiHandler`
//! does. [`Extension::register`] receives the raw parser, so a plain CSI `t` handler registered
//! here is *not* wrapped and sees every XTWINOPS sequence the application sends. An extension
//! that registers CSI `t` must check `TerminalOptions::window_options` itself, as
//! [`crate::input_handler::param_to_window_option`] does (the standard extensions register none).

use crate::parser::escape_sequence_parser::EscapeSequenceParser;
use crate::parser::types::{IdentifierError, ParserHost};

/// A unit of optional behaviour (e.g. kitty keyboard protocol, XTVERSION, OSC 7 cwd tracking).
///
/// `C` is the host state the parser dispatches into (the same `C` as the parser's). Handlers an
/// extension registers receive `&mut C` per call and own whatever else they capture; they never
/// hold a reference back to the host or the parser.
pub trait Extension<C: ParserHost> {
    /// Stable identifier, used in logs and settings.
    fn name(&self) -> &'static str;

    /// Register this extension's handlers (and fallbacks) on `parser`. Returns the first
    /// registration error (an invalid function identifier).
    fn register(&self, parser: &mut EscapeSequenceParser<C>) -> Result<(), IdentifierError>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::parser::osc_parser::OscStringHandler;
    use crate::parser::types::{FunctionIdentifier, OscFallbackAction};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct TestHost {
        join_state: u32,
    }

    impl ParserHost for TestHost {
        fn preceding_join_state(&mut self) -> &mut u32 {
            &mut self.join_state
        }
    }

    type Log = Rc<RefCell<Vec<String>>>;

    /// Registers one OSC handler (ident 7) that logs `<tag>:<payload>` and returns `result`.
    struct OscLogger {
        tag: &'static str,
        ident: i32,
        result: bool,
        log: Log,
    }

    impl Extension<TestHost> for OscLogger {
        fn name(&self) -> &'static str {
            self.tag
        }

        fn register(
            &self,
            parser: &mut EscapeSequenceParser<TestHost>,
        ) -> Result<(), IdentifierError> {
            let (tag, result, log) = (self.tag, self.result, Rc::clone(&self.log));
            parser.register_osc_handler(
                self.ident,
                OscStringHandler::new(move |_, payload| {
                    log.borrow_mut().push(format!("{tag}:{payload}"));
                    result
                }),
            );
            Ok(())
        }
    }

    /// Registers a CSI handler with an invalid (out of range) final byte.
    struct BadIdentifier;

    impl Extension<TestHost> for BadIdentifier {
        fn name(&self) -> &'static str {
            "bad"
        }

        fn register(
            &self,
            parser: &mut EscapeSequenceParser<TestHost>,
        ) -> Result<(), IdentifierError> {
            parser
                .register_csi_handler(FunctionIdentifier::new('\u{1f}'), |_, _| true)
                .map(|_| ())
        }
    }

    fn logger(tag: &'static str, ident: i32, result: bool, log: &Log) -> OscLogger {
        OscLogger {
            tag,
            ident,
            result,
            log: Rc::clone(log),
        }
    }

    /// `ESC ] <ident> ; <payload> BEL` as code points.
    fn osc(ident: i32, payload: &str) -> Vec<u32> {
        format!("\u{1b}]{ident};{payload}\u{7}")
            .chars()
            .map(u32::from)
            .collect()
    }

    #[test]
    fn newest_handler_wins_and_stops_dispatch() {
        let log: Log = Rc::default();
        let mut parser = EscapeSequenceParser::<TestHost>::new();
        let mut host = TestHost::default();
        logger("old", 7, true, &log).register(&mut parser).unwrap();
        logger("new", 7, true, &log).register(&mut parser).unwrap();

        parser.parse(&mut host, &osc(7, "file:///tmp"));
        assert_eq!(*log.borrow(), ["new:file:///tmp"]);
    }

    #[test]
    fn declining_handler_falls_through_to_older() {
        let log: Log = Rc::default();
        let mut parser = EscapeSequenceParser::<TestHost>::new();
        let mut host = TestHost::default();
        logger("old", 52, true, &log).register(&mut parser).unwrap();
        logger("new", 52, false, &log)
            .register(&mut parser)
            .unwrap();

        parser.parse(&mut host, &osc(52, "c;?"));
        assert_eq!(*log.borrow(), ["new:c;?", "old:c;?"]);
    }

    #[test]
    fn unhandled_ident_reports_to_the_fallback() {
        let log: Log = Rc::default();
        let mut parser = EscapeSequenceParser::<TestHost>::new();
        let mut host = TestHost::default();
        logger("seven", 7, true, &log)
            .register(&mut parser)
            .unwrap();
        let fallback_log = Rc::clone(&log);
        parser.set_osc_handler_fallback(move |_, ident, action| {
            if let OscFallbackAction::End(success) = action {
                fallback_log
                    .borrow_mut()
                    .push(format!("end:{ident}:{success}"));
            }
        });

        parser.parse(&mut host, &osc(1337, "x"));
        assert_eq!(*log.borrow(), ["end:1337:true"]);
    }

    #[test]
    fn registration_errors_are_returned_and_extensions_are_object_safe() {
        let log: Log = Rc::default();
        let mut parser = EscapeSequenceParser::<TestHost>::new();
        let extensions: Vec<Box<dyn Extension<TestHost>>> = vec![
            Box::new(logger("ok", 7, true, &log)),
            Box::new(BadIdentifier),
        ];

        let results: Vec<_> = extensions
            .iter()
            .map(|e| (e.name(), e.register(&mut parser)))
            .collect();
        assert_eq!(results[0], ("ok", Ok(())));
        assert!(matches!(
            results[1],
            ("bad", Err(IdentifierError::FinalRange { .. }))
        ));
    }
}
