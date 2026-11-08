//! Handler coverage: every registration of upstream's `InputHandler` constructor
//! (`InputHandler.ts:192-381`) is exercised by the screen corpus and reaches a real handler.
//!
//! The table below is that registration list: each CSI, ESC, execute, OSC and DCS identifier,
//! with an input that dispatches it. Two checks per entry:
//!  1. its identifier appears in the parser trace of at least one screen golden (the oracle
//!     corpus drives it), and
//!  2. feeding the input to a fresh `Terminal` with a capturing logger at `Debug` logs no
//!     `Unknown ... code` message, so the sequence reached a registered handler and not the
//!     fallback. A control run with an unregistered sequence proves the logger sees fallbacks.
//!
//! Charset designators are all seven intermediates times all sixteen flags of `Charsets.ts`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use lune_vt::Terminal;
use lune_vt::services::log_service::Logger;
use lune_vt::services::options_service::{LogLevel, OptionValue};
use lune_vt_conformance::trace::trace_chunks;
use lune_vt_conformance::{Kind, crate_dir, load};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct Registration {
    /// Human-readable name for failure messages.
    name: String,
    /// The trace line (or its identifying prefix) a golden's parser trace must contain.
    key: String,
    /// Bytes that dispatch the registration.
    input: Vec<u8>,
}

/// Upstream's identifier: collected bytes shifted in 8 bits at a time, then the final byte.
fn ident(collected: &str, final_byte: char) -> u32 {
    let mut value: u32 = 0;
    for byte in collected.bytes() {
        value = (value << 8) | u32::from(byte);
    }
    (value << 8) | u32::from(final_byte)
}

fn csi(prefix: &str, intermediates: &str, final_byte: char, params: &str) -> Registration {
    let collected = format!("{prefix}{intermediates}");
    Registration {
        name: format!("CSI {prefix}{params}{intermediates}{final_byte}"),
        key: format!("csi {:08x}", ident(&collected, final_byte)),
        input: format!("\x1b[{prefix}{params}{intermediates}{final_byte}").into_bytes(),
    }
}

fn esc(intermediates: &str, final_byte: char) -> Registration {
    Registration {
        name: format!("ESC {intermediates}{final_byte}"),
        key: format!("esc {:08x}", ident(intermediates, final_byte)),
        input: format!("\x1b{intermediates}{final_byte}").into_bytes(),
    }
}

fn exec(code: u32) -> Registration {
    let input = match char::from_u32(code) {
        Some(c) => c.to_string().into_bytes(),
        None => Vec::new(),
    };
    Registration {
        name: format!("EXECUTE {code:02x}"),
        key: format!("exec {code:02x}"),
        input,
    }
}

fn osc(id: u32, payload: &str) -> Registration {
    Registration {
        name: format!("OSC {id}"),
        key: format!("osc {id} start"),
        input: format!("\x1b]{id};{payload}\x07").into_bytes(),
    }
}

fn dcs(intermediates: &str, final_byte: char, body: &str) -> Registration {
    Registration {
        name: format!("DCS {intermediates}{final_byte}"),
        key: format!("dcs {:08x} hook", ident(intermediates, final_byte)),
        input: format!("\x1bP{intermediates}{final_byte}{body}\x1b\\").into_bytes(),
    }
}

/// `charsets` of `Charsets.ts`, in source order.
const CHARSET_FLAGS: [char; 16] = [
    '0', 'A', 'B', '4', 'C', '5', 'R', 'Q', 'K', 'Y', 'E', '6', 'Z', 'H', '7', '=',
];
const CHARSET_INTERMEDIATES: [&str; 7] = ["(", ")", "*", "+", "-", ".", "/"];

/// CSI registrations: (prefix, intermediates, final, a parameter string that reaches the handler).
const CSI_REGISTRATIONS: [(&str, &str, char, &str); 50] = [
    ("", "", '@', "1"),
    ("", " ", '@', "1"),
    ("", "", 'A', "1"),
    ("", " ", 'A', "1"),
    ("", "", 'B', "1"),
    ("", "", 'C', "1"),
    ("", "", 'D', "1"),
    ("", "", 'E', "1"),
    ("", "", 'F', "1"),
    ("", "", 'G', "1"),
    ("", "", 'H', "1;1"),
    ("", "", 'I', "1"),
    ("", "", 'J', "2"),
    ("?", "", 'J', "2"),
    ("", "", 'K', "2"),
    ("?", "", 'K', "2"),
    ("", "", 'L', "1"),
    ("", "", 'M', "1"),
    ("", "", 'P', "1"),
    ("", "", 'S', "1"),
    ("", "", 'T', "1"),
    ("", "", 'X', "1"),
    ("", "", 'Z', "1"),
    ("", "", '`', "1"),
    ("", "", 'a', "1"),
    ("", "", 'b', "1"),
    ("", "", 'c', ""),
    (">", "", 'c', ""),
    ("", "", 'd', "1"),
    ("", "", 'e', "1"),
    ("", "", 'f', "1;1"),
    ("", "", 'g', "0"),
    ("", "", 'h', "4"),
    ("?", "", 'h', "25"),
    ("", "", 'l', "4"),
    ("?", "", 'l', "25"),
    ("", "", 'm', "1"),
    ("", "", 'n', "5"),
    ("?", "", 'n', "6"),
    ("", "!", 'p', ""),
    ("", " ", 'q', "1"),
    ("", "", 'r', "1;2"),
    ("", "", 's', ""),
    ("", "", 't', "18"),
    ("", "", 'u', ""),
    ("", "'", '}', "1"),
    ("", "'", '~', "1"),
    ("", "\"", 'q', "1"),
    ("", "$", 'p', "4"),
    ("?", "$", 'p', "25"),
];

/// Execute registrations (`C0` and `C1` codes).
const EXEC_CODES: [u32; 12] = [
    0x07, 0x0a, 0x0b, 0x0c, 0x0d, 0x08, 0x09, 0x0e, 0x0f, 0x84, 0x85, 0x88,
];

/// ESC registrations without a charset designator.
const ESC_FINALS: [(&str, char); 17] = [
    ("", '7'),
    ("", '8'),
    ("", 'D'),
    ("", 'E'),
    ("", 'H'),
    ("", 'M'),
    ("", '='),
    ("", '>'),
    ("", 'c'),
    ("", 'n'),
    ("", 'o'),
    ("", '|'),
    ("", '}'),
    ("", '~'),
    ("%", '@'),
    ("%", 'G'),
    ("#", '8'),
];

/// OSC registrations: (identifier, a payload that reaches the handler).
const OSC_REGISTRATIONS: [(u32, &str); 12] = [
    (0, "title"),
    (1, "icon"),
    (2, "title"),
    (4, "1;?"),
    (8, ";"),
    (10, "?"),
    (11, "?"),
    (12, "?"),
    (104, "1"),
    (110, ""),
    (111, ""),
    (112, ""),
];

fn registrations() -> Vec<Registration> {
    let mut all: Vec<Registration> = Vec::new();
    for (prefix, intermediates, final_byte, params) in CSI_REGISTRATIONS {
        all.push(csi(prefix, intermediates, final_byte, params));
    }
    for code in EXEC_CODES {
        all.push(exec(code));
    }
    for (intermediates, final_byte) in ESC_FINALS {
        all.push(esc(intermediates, final_byte));
    }
    for (id, payload) in OSC_REGISTRATIONS {
        all.push(osc(id, payload));
    }
    all.push(dcs("$", 'q', "m"));
    for intermediates in CHARSET_INTERMEDIATES {
        for flag in CHARSET_FLAGS {
            all.push(esc(intermediates, flag));
        }
    }
    all
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Logger for Capture {
    fn log(&self, _level: LogLevel, message: &str) {
        self.0
            .lock()
            .expect("capture lock")
            .push(message.to_string());
    }
}

/// Messages logged while a fresh debug-level terminal processes `input`.
fn logged(input: &[u8]) -> Vec<String> {
    let capture = Capture::default();
    let mut term = Terminal::new([OptionValue::LogLevel(LogLevel::Debug)]);
    term.set_logger(Some(Box::new(capture.clone())));
    term.write(input);
    std::mem::take(&mut *capture.0.lock().expect("capture lock"))
}

fn is_unknown(message: &str) -> bool {
    message.starts_with("Unknown ")
}

#[test]
fn the_capturing_logger_sees_fallbacks() {
    for (name, input) in [
        ("CSI", &b"\x1b[?5c"[..]),
        ("ESC", b"\x1bZ"),
        ("EXECUTE", &[0x05]),
        ("OSC", b"\x1b]9999;x\x07"),
        ("DCS", b"\x1bPxx\x1b\\"),
    ] {
        let messages = logged(input);
        assert!(
            messages
                .iter()
                .any(|m| m.starts_with(&format!("Unknown {name} code"))),
            "{name}: no fallback logged, got {messages:?}"
        );
    }
}

#[test]
fn every_registration_reaches_a_real_handler() {
    let mut bad: Vec<String> = Vec::new();
    for registration in registrations() {
        let unknown: Vec<String> = logged(&registration.input)
            .into_iter()
            .filter(|m| is_unknown(m))
            .collect();
        if !unknown.is_empty() {
            bad.push(format!("{}: {unknown:?}", registration.name));
        }
    }
    assert!(
        bad.is_empty(),
        "{} registration(s) fell through to the fallback:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// The trace key of one parser trace line, or `None` for lines that are not dispatches.
fn trace_key(line: &str) -> Option<String> {
    let mut words = line.split(' ');
    match words.next()? {
        "csi" => Some(format!("csi {}", words.next()?)),
        "esc" => Some(line.to_string()),
        "exec" => Some(line.to_string()),
        "osc" => {
            let id = words.next()?;
            (words.next()? == "start").then(|| format!("osc {id} start"))
        }
        "dcs" => {
            let ident = words.next()?;
            (words.next()? == "hook").then(|| format!("dcs {ident} hook"))
        }
        _ => None,
    }
}

#[test]
fn every_registration_appears_in_a_screen_golden() -> Result<()> {
    let goldens = load(Kind::Screen)?;
    let mut seen: HashSet<String> = HashSet::new();
    for golden in &goldens {
        let flat: Vec<u8> = golden.chunks.iter().flatten().copied().collect();
        for line in trace_chunks([flat.as_slice()]) {
            if let Some(key) = trace_key(&line) {
                seen.insert(key);
            }
        }
    }
    let missing: Vec<String> = registrations()
        .into_iter()
        .filter(|r| !seen.contains(&r.key))
        .map(|r| format!("{} ({})", r.name, r.key))
        .collect();
    assert!(
        missing.is_empty(),
        "{} registration(s) in no screen golden's trace:\n{}",
        missing.len(),
        missing.join("\n")
    );
    Ok(())
}

#[test]
fn table_sizes_are_upstreams() {
    let all = registrations();
    let count = |prefix: &str| all.iter().filter(|r| r.key.starts_with(prefix)).count();
    // 50 CSI identifiers, 12 execute codes, 17 plain ESC identifiers, 12 OSC identifiers, 1 DCS,
    // 7 x 16 charset designators.
    assert_eq!(count("csi "), 50);
    assert_eq!(count("exec "), 12);
    assert_eq!(count("osc "), 12);
    assert_eq!(count("dcs "), 1);
    assert_eq!(count("esc "), 17 + 7 * 16);
    let keys: HashSet<&str> = all.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys.len(), all.len(), "duplicate identifiers in the table");
}

/// Counts of `registerXHandler(` / `setExecuteHandler(` lines in the reference source, when it
/// is checked out (`scripts/fetch-xterm-ref.sh`); skipped otherwise. Guards the table against
/// drifting from `InputHandler.ts:192-381`.
#[test]
fn table_matches_upstream_source() {
    let path = crate_dir().join("../../reference/xterm.js/src/common/InputHandler.ts");
    let Ok(source) = std::fs::read_to_string(&path) else {
        eprintln!("skipped: {} not present", path.display());
        return;
    };
    let lines = |needle: &str| {
        source
            .lines()
            .filter(|l| l.trim_start().starts_with(needle))
            .count()
    };
    assert_eq!(lines("this._parser.registerCsiHandler("), 50);
    assert_eq!(lines("this._parser.setExecuteHandler("), 12);
    assert_eq!(lines("this._parser.registerOscHandler("), 12);
    assert_eq!(lines("this._parser.registerDcsHandler("), 1);
    // 17 plain ESC lines, the `#8` line, the two `%` lines, and 7 loop lines for the charsets.
    assert_eq!(lines("this._parser.registerEscHandler("), 17 + 7);
}
