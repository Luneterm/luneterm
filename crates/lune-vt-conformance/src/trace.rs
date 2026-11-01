//! Parser-trace runner: drives a fresh `lune-vt` decoder and `EscapeSequenceParser` with fallback
//! handlers only and renders the events in the normalised line format of FORMAT.md ("Parser
//! trace"), so a trace can be compared line by line with a golden's `expect`.
//!
//! Depends only on `lune-vt` and `crate::escape`. The `lune-vt` fuzz test (`tests/no_panic.rs`)
//! includes this file and `escape.rs` by path, because `lune-vt` cannot dev-depend on this crate
//! (this crate depends on it).
#![allow(unreachable_pub, dead_code)]

use lune_vt::input::text_decoder::Utf8ToUtf32;
use lune_vt::parser::escape_sequence_parser::EscapeSequenceParser;
use lune_vt::parser::params::{Params, ParamsArrayItem};
use lune_vt::parser::types::{DcsFallbackAction, OscFallbackAction, ParserHost};

use crate::escape::escape_code_points;

/// One recorded event, before rendering. Runs of `print` / `osc put` / `dcs put` are merged so
/// chunk boundaries are invisible.
#[derive(Debug)]
enum Event {
    Line(String),
    Print(Vec<u32>),
    OscPut(Vec<u32>),
    DcsPut(Vec<u32>),
}

/// Host state of the trace parser.
#[derive(Debug, Default)]
pub struct TraceHost {
    join: u32,
    events: Vec<Event>,
}

impl ParserHost for TraceHost {
    fn preceding_join_state(&mut self) -> &mut u32 {
        &mut self.join
    }
}

impl TraceHost {
    fn line(&mut self, text: String) {
        self.events.push(Event::Line(text));
    }

    fn print(&mut self, data: &[u32]) {
        if let Some(Event::Print(run)) = self.events.last_mut() {
            run.extend_from_slice(data);
        } else {
            self.events.push(Event::Print(data.to_vec()));
        }
    }

    fn osc_put(&mut self, data: &[u32]) {
        if let Some(Event::OscPut(run)) = self.events.last_mut() {
            run.extend_from_slice(data);
        } else {
            self.events.push(Event::OscPut(data.to_vec()));
        }
    }

    fn dcs_put(&mut self, data: &[u32]) {
        if let Some(Event::DcsPut(run)) = self.events.last_mut() {
            run.extend_from_slice(data);
        } else {
            self.events.push(Event::DcsPut(data.to_vec()));
        }
    }

    fn render(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| match event {
                Event::Line(text) => text.clone(),
                Event::Print(cps) => format!("print {}", escape_code_points(cps.iter().copied())),
                Event::OscPut(cps) => {
                    format!("osc put {}", escape_code_points(cps.iter().copied()))
                }
                Event::DcsPut(cps) => {
                    format!("dcs put {}", escape_code_points(cps.iter().copied()))
                }
            })
            .collect()
    }
}

/// `toArray()` as JSON without spaces: `[1,2,[3,-1]]`.
fn params_json(params: &Params) -> String {
    let items: Vec<String> = params
        .to_array()
        .iter()
        .map(|item| match item {
            ParamsArrayItem::Param(value) => value.to_string(),
            ParamsArrayItem::Sub(subs) => {
                let inner: Vec<String> = subs.iter().map(i32::to_string).collect();
                format!("[{}]", inner.join(","))
            }
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// OSC id: decimal, `-1` for none, `max` where `lune-vt` saturates (and the oracle's JS number
/// exceeds 2147483646).
fn osc_id(id: i32) -> String {
    if id == i32::MAX {
        "max".to_string()
    } else {
        id.to_string()
    }
}

fn trace_parser() -> EscapeSequenceParser<TraceHost> {
    let mut p = EscapeSequenceParser::<TraceHost>::new();
    p.set_print_handler(|h, data| h.print(data));
    p.set_execute_handler_fallback(|h, code| h.line(format!("exec {code:02x}")));
    p.set_csi_handler_fallback(|h, ident, params| {
        h.line(format!("csi {ident:08x} {}", params_json(params)));
    });
    p.set_esc_handler_fallback(|h, ident| h.line(format!("esc {ident:08x}")));
    p.set_osc_handler_fallback(|h, id, action| match action {
        OscFallbackAction::Start => h.line(format!("osc {} start", osc_id(id))),
        OscFallbackAction::Put(data) => h.osc_put(data),
        OscFallbackAction::End(success) => h.line(format!("osc end {}", u8::from(success))),
    });
    p.set_dcs_handler_fallback(|h, ident, action| match action {
        DcsFallbackAction::Hook(params) => {
            h.line(format!("dcs {ident:08x} hook {}", params_json(params)));
        }
        DcsFallbackAction::Put(data) => h.dcs_put(data),
        DcsFallbackAction::Unhook(success) => h.line(format!("dcs unhook {}", u8::from(success))),
    });
    p.set_error_handler(|h, state| {
        h.line(format!(
            "error {} {:x}",
            state.current_state as u8, state.code
        ));
        false
    });
    p
}

/// A decoder plus a trace parser; feed it byte chunks, then read the normalised trace.
#[derive(Debug)]
pub struct Tracer {
    parser: EscapeSequenceParser<TraceHost>,
    decoder: Utf8ToUtf32,
    host: TraceHost,
    scratch: Vec<u32>,
}

impl Default for Tracer {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracer {
    pub fn new() -> Self {
        Tracer {
            parser: trace_parser(),
            decoder: Utf8ToUtf32::new(),
            host: TraceHost::default(),
            scratch: Vec::new(),
        }
    }

    /// One write: bytes through the decoder, code points through the parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.decoder.decode(bytes, &mut self.scratch);
        self.parser.parse(&mut self.host, &self.scratch);
    }

    /// The trace so far.
    pub fn lines(&self) -> Vec<String> {
        self.host.render()
    }
}

/// Trace of `chunks` fed one by one to a fresh decoder and parser.
pub fn trace_chunks<'a, I>(chunks: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut tracer = Tracer::new();
    for chunk in chunks {
        tracer.feed(chunk);
    }
    tracer.lines()
}

/// Index of the first line where the traces differ (the shorter one's length if one is a
/// prefix of the other), or `None` when equal.
pub fn first_difference(a: &[String], b: &[String]) -> Option<usize> {
    if a == b {
        return None;
    }
    Some(
        a.iter()
            .zip(b.iter())
            .position(|(x, y)| x != y)
            .unwrap_or_else(|| a.len().min(b.len())),
    )
}

/// A line cut to at most `max` characters, for failure messages (never a whole payload).
pub fn clip(line: Option<&String>, max: usize) -> String {
    match line {
        None => "<end of trace>".to_string(),
        Some(l) if l.chars().count() <= max => l.clone(),
        Some(l) => {
            let head: String = l.chars().take(max).collect();
            format!("{head}... ({} chars)", l.chars().count())
        }
    }
}
