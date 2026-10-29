//! Extension point: sequence handlers registered on top of the ported core.
//!
//! Same semantics as xterm.js `parser.registerOscHandler` / `registerCsiHandler`: handlers for
//! one identifier are tried newest-first, and the first that returns `true` consumes the
//! sequence. Built-in behaviour is registered first, so an extension can override it without
//! touching ported code.

use std::collections::HashMap;
use std::fmt;

/// A unit of optional behaviour (e.g. kitty keyboard protocol, XTVERSION, OSC 7 cwd tracking).
pub trait Extension {
    /// Stable identifier, used in logs and settings.
    fn name(&self) -> &'static str;
    fn register(&self, registry: &mut HandlerRegistry);
}

/// Handler for an OSC payload. Returns `true` when it consumed the sequence.
pub type OscHandler = Box<dyn FnMut(&str) -> bool>;

/// Ordered handler table, keyed by OSC identifier.
#[derive(Default)]
pub struct HandlerRegistry {
    osc: HashMap<u32, Vec<OscHandler>>,
}

impl fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut idents: Vec<_> = self.osc.keys().collect();
        idents.sort();
        f.debug_struct("HandlerRegistry")
            .field("osc", &idents)
            .finish()
    }
}

impl HandlerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_osc(&mut self, ident: u32, handler: impl FnMut(&str) -> bool + 'static) {
        self.osc.entry(ident).or_default().push(Box::new(handler));
    }

    /// Dispatch to the handlers for `ident`, newest first. Returns `false` if none consumed it.
    pub fn dispatch_osc(&mut self, ident: u32, payload: &str) -> bool {
        self.osc
            .get_mut(&ident)
            .is_some_and(|handlers| handlers.iter_mut().rev().any(|h| h(payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn newest_handler_wins_and_stops_dispatch() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut reg = HandlerRegistry::new();
        let l = Rc::clone(&log);
        reg.register_osc(7, move |p| {
            l.borrow_mut().push(format!("old:{p}"));
            true
        });
        let l = Rc::clone(&log);
        reg.register_osc(7, move |p| {
            l.borrow_mut().push(format!("new:{p}"));
            true
        });

        assert!(reg.dispatch_osc(7, "file:///tmp"));
        assert_eq!(*log.borrow(), ["new:file:///tmp"]);
    }

    #[test]
    fn declining_handler_falls_through_to_older() {
        let mut reg = HandlerRegistry::new();
        reg.register_osc(52, |_| true);
        reg.register_osc(52, |_| false);
        assert!(reg.dispatch_osc(52, "c;?"));
    }

    #[test]
    fn unhandled_ident_reports_false() {
        let mut reg = HandlerRegistry::new();
        assert!(!reg.dispatch_osc(1337, "x"));
    }
}
