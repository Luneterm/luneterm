//! Seeded engine-level fuzz of `lune_vt::Terminal` (plan M6 step 10).
//!
//! Per seed: a terminal at a random small size and a list of random operations over its public
//! API: `write` (grammar-biased escape sequences with extreme and huge parameters, raw byte soup,
//! OSC/DCS payload bursts, OSC 8 link floods that evict links through the scrollback), `resize`
//! (0x0, 1x1 and 300x100 included, through `Terminal` and `VtEngine`), `scroll_*`, `set_option`
//! (scrollback including 0, tab stop width, convert_eol, windows_mode, screen_reader_mode, every
//! window option on, ...), `reset`, `clear`, `input`, `idle`, `viewport`/`read_row`/`modes`,
//! `drain_events`, `take_damage` and markers, and (M7 step 9) the input encoders `key_event`,
//! `mouse_event`, `focus_event` and `paste` and the clipboard gate. About half of the terminals
//! have the standard extensions installed (`install_standard_extensions`, a random OSC 52 gate and
//! VS16, a fixed valid XTVERSION name); the grammar then reaches them with hostile parameters
//! (kitty and modifyOtherKeys CSI forms, XTVERSION forms, `CSI ? 2026 h`) and with OSC 7, 9;9,
//! 9;4, 52 and 133 payloads around each cap, and one seed in 30 writes an OSC 52 of exactly the
//! character cap, one over, and half. Each seed runs under `catch_unwind`; a failure reports the
//! seed and the last operations (kind and sizes, never the input bytes or the payloads).
//!
//! Checked per seed:
//!  - no panic (debug builds keep integer-overflow checks on);
//!  - bounds after every operation (`check_bounds`): buffer line counts, title stacks, link table
//!    against live markers, event bytes against the bytes written, viewport shape, kitty stacks
//!    (at most 8 entries, flags within the supported bits) and modifyOtherKeys (at most 2); per
//!    drain at most one each of `ClipboardWrite`, `CwdChanged` and `Progress` and at most 64
//!    `UnknownOsc`; a reset leaves the keyboard protocol cleared;
//!  - chunk invariance: the same operations with each write cut into 1-byte chunks, and into
//!    random chunks, end with the same `screen::dump` lines, the same `Viewport` and `Modes`, the
//!    same event sequence (payloads as length and hash, `A11yText` and `A11yTab` left out: their
//!    merge granularity is not specified) and the same encoder outputs as the operations as
//!    generated, at a few checkpoints and at the end.
//!
//! The event bound holds on every seed since the engine coalesces the two amplifiers this test
//! found (plan M6 D2 amendment, user decisions Q10/Q11): one payload-free `TitleChanged` per
//! drain (XTWINOPS 23 used to re-queue a stored title of up to 10 MB per pop) and merged
//! `A11yText` capped per drain (REP with screen reader mode on used to queue one event per
//! repeated character). The unit tests for both rules are in `headless/terminal_tests.rs`.
//!
//! Known upstream chunk dependence (step 9 hand-off (b), root cause found here): with autowrap off
//! (`CSI ? 7 l`) a wide character that does not fit at the right margin is skipped and leaves
//! the cursor on the tail cell of the wide character before it; the next printable then blanks
//! that wide character's head cell only if it starts a `print` call (`InputHandler.ts:526`), so
//! `ESC [ ? 7 l \u4f60 \u4f60 a` at 2 columns reads differently in one write and byte by byte.
//! xterm.js has the same dependence (checked against the oracle) and the port reproduces it. A
//! seed whose writes mention `CSI ? ... 7 ... l` and whose replays differ is counted as
//! `wrap_off_skips` and not failed; every other seed must replay identically.
//!
//! `LUNE_VT_ENGINE_FUZZ_SEEDS=<n>` runs seeds `0..n`; `LUNE_VT_ENGINE_FUZZ_SEED=<s>` replays one.
//!
//! `lune-vt` cannot dev-depend on `lune-vt-conformance`, so its runners are included by path (as
//! `no_panic.rs` does).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../../lune-vt-conformance/src/escape.rs"]
mod escape;
#[path = "../../lune-vt-conformance/src/screen.rs"]
mod screen;
#[path = "../../lune-vt-conformance/src/trace.rs"]
mod trace;

use std::panic::{AssertUnwindSafe, catch_unwind};

use lune_vt::ext::keyboard_protocol::{KITTY_STACK_MAX, KITTY_SUPPORTED, MODIFY_OTHER_KEYS_MAX};
use lune_vt::input::mouse_event::{
    TerminalMouseButton, TerminalMouseEvent, TerminalMouseEventType,
};
use lune_vt::services::options_service::{
    CursorStyle, OptionValue, PtyBackendKind, WindowOptions, WindowsPty,
};
use lune_vt::services::osc_link_service::{MAX_LINK_MARKERS, MAX_LINKS};
use lune_vt::{
    HostEvent, StandardExtensions, Terminal, TerminalKey, TerminalKeyEvent, TerminalModifiers,
    VtEngine,
};

use screen::{Run, SplitRng, dump_run};
use trace::{clip, first_difference};

const DEFAULT_SEEDS: u64 = 60;
const OPS_PER_SEED: usize = 36;
/// Title stacks are capped at ten entries (`InputHandler` STACK_LIMIT).
const TITLE_STACK_LIMIT: usize = 10;
/// Events may weigh this many times the bytes that caused them, plus a flat allowance.
const EVENT_RATIO: u64 = 16;
const EVENT_SLACK: u64 = 4096;
/// The OSC 52 character cap (`ext::osc52::MAX_BASE64_CHARS`, 4 * ceil(1 MiB / 3)).
const OSC52_CHAR_CAP: usize = 1_398_104;
/// One seed in this many writes the OSC 52 cap cases.
const OSC52_CAP_SEED_EVERY: u64 = 30;
/// The fixed XTVERSION name of the terminals with extensions.
const XTVERSION_NAME: &str = "lune-vt-fuzz";

/// Event kinds counted by the instrument check and the per-drain weights.
const SLOT_NAMES: [&str; 13] = [
    "response",
    "bell",
    "title",
    "color",
    "report",
    "a11y text",
    "a11y tab",
    "unknown osc",
    "clipboard write",
    "cwd changed",
    "progress",
    "command mark",
    "sync update in command",
];

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[usize::try_from(self.below(items.len() as u64)).unwrap()]
    }

    fn usize_between(&mut self, lo: usize, hi: usize) -> usize {
        usize::try_from(self.between(lo as u64, hi as u64)).unwrap()
    }
}

// ------------------------------------------------------------------------------------------
// Input generator.

const SMALL_PARAMS: [&str; 18] = [
    "", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "12", "20", "22", "23", "25", "24",
];
const EXTREME_PARAMS: [&str; 8] = [
    "65535",
    "65536",
    "2147483647",
    "2147483648",
    "4294967295",
    "4294967296",
    "99999999999999999999",
    "00000000000000000000001",
];
const DEC_MODES: [&str; 28] = [
    "1", "3", "5", "6", "7", "8", "9", "12", "25", "45", "47", "66", "69", "80", "95", "1000",
    "1001", "1002", "1003", "1004", "1005", "1006", "1007", "1015", "1016", "1047", "1049", "2004",
];
const SGR_SPECIAL: [&str; 14] = [
    "38;2;1;2;3",
    "48;2;255;255;255",
    "58;2;9;8;7",
    "38;5;200",
    "48;5;1",
    "58;5;77",
    "38:2::10:20:30",
    "48:2:0:1:2:3",
    "4:3",
    "4:5",
    "4:0",
    "38;2;999;999;999",
    "38;5;999",
    "38;2",
];
/// `CSI <prefix><params><intermediates><final>`: the registered CSI sequences and a few the
/// engine does not know.
const CSI_TABLE: [(&str, &str, char); 54] = [
    ("", "", '@'),
    ("", " ", '@'),
    ("", "", 'A'),
    ("", " ", 'A'),
    ("", "", 'B'),
    ("", "", 'C'),
    ("", "", 'D'),
    ("", "", 'E'),
    ("", "", 'F'),
    ("", "", 'G'),
    ("", "", 'H'),
    ("", "", 'I'),
    ("", "", 'J'),
    ("?", "", 'J'),
    ("", "", 'K'),
    ("?", "", 'K'),
    ("", "", 'L'),
    ("", "", 'M'),
    ("", "", 'P'),
    ("", "", 'S'),
    ("", "", 'T'),
    ("", "", 'X'),
    ("", "", 'Z'),
    ("", "", '`'),
    ("", "", 'a'),
    ("", "", 'b'),
    ("", "", 'c'),
    (">", "", 'c'),
    ("", "", 'd'),
    ("", "", 'e'),
    ("", "", 'f'),
    ("", "", 'g'),
    ("", "", 'h'),
    ("?", "", 'h'),
    ("", "", 'l'),
    ("?", "", 'l'),
    ("", "", 'm'),
    ("", "", 'n'),
    ("?", "", 'n'),
    ("", "!", 'p'),
    ("", " ", 'q'),
    ("", "", 'r'),
    ("", "", 's'),
    ("", "", 't'),
    ("", "", 'u'),
    ("", "'", '}'),
    ("", "'", '~'),
    ("", "\"", 'q'),
    ("", "$", 'p'),
    ("?", "$", 'p'),
    ("", "", 'z'),
    ("=", "", 'c'),
    ("", "", 't'),
    ("", "", 'm'),
];
const ESC_FINALS: [&str; 24] = [
    "7", "8", "D", "E", "H", "M", "=", ">", "c", "n", "o", "|", "}", "~", "%@", "%G", "#8", "#3",
    "(0", "(B", ")0", "*A", "+K", "Z",
];
const TEXT: [&str; 20] = [
    "a",
    "bc",
    "hello",
    "xyz",
    "0123456789",
    "The quick brown fox",
    "\u{4F60}\u{597D}",
    "\u{D55C}\u{AE00}",
    "\u{1F600}",
    "\u{1F44D}\u{1F3FD}",
    "\u{1F1EF}\u{1F1F5}",
    "e\u{301}",
    "a\u{301}\u{302}",
    "\u{301}",
    "\u{26A0}\u{FE0F}",
    "\u{26A0}\u{FE0E}",
    "\u{FE0F}",
    "\u{3042}",
    "\u{AD}",
    "\u{200B}",
];
const URIS: [&str; 5] = [
    "http://a/",
    "https://example.com/path?q=1#frag",
    "file:///tmp/x",
    "",
    "x",
];

/// A parameter string for `CSI`: small values mostly, extremes now and then, `;` and `:` lists.
fn csi_params(rng: &mut Rng, final_byte: char, prefix: &str) -> String {
    let count = match rng.below(20) {
        0 => rng.between(8, 40),
        1..=3 => rng.between(2, 5),
        4..=11 => 1,
        _ => 0,
    };
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push(if rng.chance(15) { ':' } else { ';' });
        }
        let value: String = if final_byte == 'm' && prefix.is_empty() && rng.chance(40) {
            if rng.chance(50) {
                (*rng.pick(&SGR_SPECIAL)).to_string()
            } else {
                rng.below(110).to_string()
            }
        } else if (final_byte == 'h' || final_byte == 'l' || final_byte == 'p') && prefix == "?" {
            (*rng.pick(&DEC_MODES)).to_string()
        } else if rng.chance(12) {
            (*rng.pick(&EXTREME_PARAMS)).to_string()
        } else {
            (*rng.pick(&SMALL_PARAMS)).to_string()
        };
        out.push_str(&value);
    }
    out
}

fn csi(rng: &mut Rng, out: &mut Vec<u8>) {
    let &(prefix, inter, final_byte) = rng.pick(&CSI_TABLE);
    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(prefix.as_bytes());
    out.extend_from_slice(csi_params(rng, final_byte, prefix).as_bytes());
    out.extend_from_slice(inter.as_bytes());
    // One in twelve is cut short: the next token interrupts it.
    if !rng.chance(8) {
        out.push(u8::try_from(final_byte).unwrap());
    }
}

fn terminator(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(6) {
        0 => out.push(0x07),
        1 | 2 => out.extend_from_slice(b"\x1b\\"),
        3 => out.extend_from_slice(&[0xC2, 0x9C]),
        4 => out.push(0x18),
        _ => {}
    }
}

fn payload_text(rng: &mut Rng, out: &mut Vec<u8>, len: usize) {
    for _ in 0..len {
        if rng.chance(6) {
            out.extend_from_slice(rng.pick(&TEXT).as_bytes());
        } else {
            out.push(0x20 + u8::try_from(rng.below(95)).unwrap());
        }
    }
}

/// Parameters of the keyboard-protocol sequences: extremes, sub-parameters, lists around and past
/// the 32-parameter limit.
fn hostile_params(rng: &mut Rng) -> String {
    let count = match rng.below(10) {
        0 => rng.between(30, 45),
        1 | 2 => rng.between(2, 4),
        3 => 0,
        _ => 1,
    };
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push(if rng.chance(12) { ':' } else { ';' });
        }
        let value = if rng.chance(30) {
            *rng.pick(&EXTREME_PARAMS)
        } else {
            *rng.pick(&["", "0", "1", "2", "3", "4", "5", "7", "8", "31"])
        };
        out.push_str(value);
    }
    out
}

/// A kitty keyboard, modifyOtherKeys, XTVERSION, DECSTR or DECSET 2026 sequence.
fn ext_csi(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b[");
    match rng.below(100) {
        0..=34 => {
            // CSI ? u, > u, < u, = u
            out.extend_from_slice(rng.pick(&["?", ">", "<", "=", ">", "<"]).as_bytes());
            out.extend_from_slice(hostile_params(rng).as_bytes());
            out.push(b'u');
        }
        35..=49 => {
            // modifyOtherKeys: set (> m), reset (> n), query (? m)
            let (prefix, final_byte) = *rng.pick(&[(">", 'm'), (">", 'n'), ("?", 'm'), (">", 'm')]);
            out.extend_from_slice(prefix.as_bytes());
            out.extend_from_slice(
                rng.pick(&[
                    "4",
                    "4;0",
                    "4;1",
                    "4;2",
                    "4;3",
                    "4;99999999999",
                    "4:1",
                    "5",
                    "",
                ])
                .as_bytes(),
            );
            if rng.chance(30) {
                out.extend_from_slice(hostile_params(rng).as_bytes());
            }
            out.push(u8::try_from(final_byte).unwrap());
        }
        50..=64 => {
            // XTVERSION forms
            out.push(b'>');
            out.extend_from_slice(
                rng.pick(&["", "0", "1", "0;0", "0:1", "99999999999", "0;1;2"])
                    .as_bytes(),
            );
            out.push(b'q');
        }
        65..=69 => out.extend_from_slice(b"!p"),
        _ => {
            // DECSET / DECRST 2026, alone, in a list, with a sub-parameter, past the limit
            let filler = *rng.pick(&[0usize, 0, 1, 5, 31, 32, 40]);
            out.push(b'?');
            for _ in 0..filler {
                out.extend_from_slice(b"1;");
            }
            out.extend_from_slice(
                rng.pick(&["2026", "2026", "2026:1", "2026;1", "1;2026"])
                    .as_bytes(),
            );
            out.push(*rng.pick(b"hhl"));
        }
    }
}

/// How a sequence of the extensions ends: mostly properly, now and then like any OSC.
fn ext_terminator(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(10) {
        0 => out.push(0x07),
        1..=3 => out.extend_from_slice(b"\x1b\\"),
        4 => out.extend_from_slice(&[0xC2, 0x9C]),
        5..=8 => out.push(0x07),
        _ => terminator(rng, out),
    }
}

/// Base64 of `len` random bytes (a quick encoder: the grammar only needs the shape).
fn base64_of_random(rng: &mut Rng, len: usize) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let bytes: Vec<u8> = (0..len)
        .map(|_| u8::try_from(rng.below(256)).unwrap())
        .collect();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// An OSC 7, 9;9, 9;4, 52 or 133 with a payload that is valid, hostile, or around its cap.
fn ext_osc(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b]");
    match rng.below(100) {
        0..=14 => {
            out.extend_from_slice(b"7;");
            out.extend_from_slice(
                rng.pick(&[
                    "file:///tmp/a%20b",
                    "file://host/home/user",
                    "file:///C:/Users/x",
                    "file:///a%00b",
                    "file:///a%C2%85b",
                    "file:///%E4%BD%A0",
                    "file://",
                    "/plain/path",
                    "",
                    "file:///a/../../b",
                ])
                .as_bytes(),
            );
        }
        15..=24 => {
            out.extend_from_slice(b"9;9;");
            out.extend_from_slice(
                rng.pick(&[
                    "/home/u",
                    "C:\\Users\\x",
                    "file:///x%20y",
                    " /padded ",
                    "",
                    "/a\x01b",
                ])
                .as_bytes(),
            );
        }
        25..=39 => {
            out.extend_from_slice(b"9;4;");
            out.extend_from_slice(
                rng.pick(&["0", "1", "2", "3", "4", "5", "99", "", "x"])
                    .as_bytes(),
            );
            out.push(b';');
            out.extend_from_slice(
                rng.pick(&[
                    "0",
                    "50",
                    "100",
                    "101",
                    "99999999999999999999",
                    "",
                    "x",
                    "-5",
                ])
                .as_bytes(),
            );
        }
        40..=59 => {
            out.extend_from_slice(b"133;");
            out.extend_from_slice(
                rng.pick(&[
                    "A",
                    "B",
                    "C",
                    "C",
                    "D",
                    "D;0",
                    "D;7",
                    "D;99999999999",
                    "D;x",
                    "AB",
                    "",
                    "E",
                    "A;cl=m;aid=1",
                ])
                .as_bytes(),
            );
        }
        60..=79 => {
            out.extend_from_slice(b"52;");
            out.extend_from_slice(rng.pick(&["c;", "p;", ";", "", "cp;"]).as_bytes());
            match rng.below(8) {
                0 => out.push(b'?'),
                1 => {} // empty
                2 => out.extend_from_slice(b"not base64!"),
                3 => out.extend_from_slice(b"QUJD="),
                _ => {
                    let len = rng.usize_between(1, 2000);
                    out.extend_from_slice(base64_of_random(rng, len).as_bytes());
                }
            }
        }
        _ => {
            // a payload around the cap of its form (the caps count the identifier too)
            let (head, cap): (&[u8], usize) = *rng.pick(&[
                (b"7;file:///".as_slice(), 4096usize),
                (b"9;9;/".as_slice(), 4096),
                (b"133;C;".as_slice(), 64),
                (b"9;4;1;".as_slice(), 64),
            ]);
            let len = cap + rng.usize_between(0, 4) - 2;
            out.extend_from_slice(head);
            let mut written = head.len();
            while written < len {
                out.push(b'a');
                written += 1;
            }
        }
    }
    ext_terminator(rng, out);
}

/// One extension sequence: a CSI form, an OSC, or `OSC 133 ; C` followed by `CSI ? 2026 h`.
fn ext_token(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(100) {
        0..=44 => ext_csi(rng, out),
        45..=54 => {
            out.extend_from_slice(b"\x1b]133;C\x07");
            out.extend_from_slice(rng.pick(&["", "\x1b[?2026l"]).as_bytes());
            out.extend_from_slice(b"\x1b[?2026h");
            if rng.chance(30) {
                out.extend_from_slice(b"\x1b]133;D;0\x07");
            }
        }
        _ => ext_osc(rng, out),
    }
}

/// The OSC 52 cap cases, each as its own OSC: half the cap, one character over (oversize), and a
/// valid payload of exactly the cap (decoding to exactly 1 MiB).
fn osc52_cap_write(rng: &mut Rng) -> Vec<u8> {
    // CAN: whatever an earlier write left unterminated is dropped first
    let mut out = vec![0x18];
    // the exact cap last: a clipboard write replaces the one before it in the same drain
    for len in [OSC52_CHAR_CAP / 2, OSC52_CHAR_CAP + 1, OSC52_CHAR_CAP] {
        out.extend_from_slice(b"\x1b]52;c;");
        if len % 4 == 0 {
            out.resize(out.len() + len - 4, b'A');
            out.extend_from_slice(b"AA==");
        } else {
            out.resize(out.len() + len, b'A');
        }
        if rng.chance(50) {
            out.push(0x07);
        } else {
            out.extend_from_slice(b"\x1b\\");
        }
    }
    out
}

fn osc(rng: &mut Rng, out: &mut Vec<u8>) {
    let ident = *rng.pick(&[
        "0", "1", "2", "4", "8", "8", "8", "10", "11", "12", "52", "104", "110", "111", "112", "7",
        "133", "777", "x", "",
    ]);
    out.extend_from_slice(b"\x1b]");
    out.extend_from_slice(ident.as_bytes());
    out.push(b';');
    match ident {
        "8" => {
            let id_param = match rng.below(4) {
                0 => String::new(),
                1 => format!("id={}", rng.below(4)),
                2 => format!("id={}:foo=bar", rng.below(3)),
                _ => "foo=bar".to_string(),
            };
            out.extend_from_slice(id_param.as_bytes());
            out.push(b';');
            out.extend_from_slice(rng.pick(&URIS).as_bytes());
        }
        "4" => {
            let (a, b) = (rng.below(300), rng.below(300));
            let spec = match rng.below(4) {
                0 => "?".to_string(),
                1 => "rgb:ff/00/80".to_string(),
                2 => "#abcdef".to_string(),
                _ => "nonsense".to_string(),
            };
            out.extend_from_slice(format!("{a};{spec};{b};{spec}").as_bytes());
        }
        "10" | "11" | "12" => {
            out.extend_from_slice(
                rng.pick(&["?", "rgb:00/ff/00", "#123", "?;?", "bad", ""])
                    .as_bytes(),
            );
        }
        "104" => out.extend_from_slice(rng.pick(&["", "1", "1;2;300", "x"]).as_bytes()),
        _ => {
            let len = rng.usize_between(0, 40);
            payload_text(rng, out, len);
        }
    }
    terminator(rng, out);
}

fn dcs(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1bP");
    if rng.chance(15) {
        out.extend_from_slice(b"1;2");
    }
    out.extend_from_slice(rng.pick(&["$q", "$q", "+q", "q", "$"]).as_bytes());
    out.extend_from_slice(
        rng.pick(&["m", "r", "\"q", " q", "s", "x", "", "mm", "0m"])
            .as_bytes(),
    );
    terminator(rng, out);
}

fn xtwinops_title(rng: &mut Rng, out: &mut Vec<u8>) {
    // push and pop, which clone the title.
    let op = if rng.chance(50) { "22" } else { "23" };
    let which = *rng.pick(&["", ";0", ";1", ";2"]);
    out.extend_from_slice(format!("\x1b[{op}{which}t").as_bytes());
}

/// One token of the grammar.
fn token(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(108) {
        100..=107 => ext_token(rng, out),
        0..=29 => csi(rng, out),
        30..=47 => out.extend_from_slice(rng.pick(&TEXT).as_bytes()),
        48..=53 => out.extend_from_slice(b"\r\n"),
        54..=58 => out.extend_from_slice(b"\n"),
        59..=63 => {
            out.push(*rng.pick(&[0x00, 0x07, 0x08, 0x09, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x7f]))
        }
        64..=71 => {
            out.extend_from_slice(b"\x1b");
            out.extend_from_slice(rng.pick(&ESC_FINALS).as_bytes());
        }
        72..=79 => osc(rng, out),
        80..=83 => dcs(rng, out),
        84..=86 => xtwinops_title(rng, out),
        87..=89 => {
            // C1 as UTF-8 and as an invalid lone byte.
            if rng.chance(50) {
                out.extend_from_slice(&[0xC2, 0x80 + u8::try_from(rng.below(32)).unwrap()]);
            } else {
                out.push(0x80 + u8::try_from(rng.below(128)).unwrap());
            }
        }
        90..=93 => {
            let n = rng.between(1, 30);
            for _ in 0..n {
                out.push(0x20 + u8::try_from(rng.below(95)).unwrap());
            }
        }
        94..=95 => {
            // ESC c, the full reset, now and then.
            if rng.chance(30) {
                out.extend_from_slice(b"\x1bc");
            }
        }
        _ => {
            // an incomplete lead of something
            out.extend_from_slice(
                rng.pick(&["\x1b", "\x1b[", "\x1b]", "\x1bP", "\x1b[1;", "\x1b]0;"])
                    .as_bytes(),
            );
        }
    }
}

/// Extension sequences only, 1 to 12 of them.
fn ext_burst(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    if rng.chance(20) {
        // a push flood: past the stack limit of 8, the oldest entry is evicted
        for _ in 0..rng.between(6, 14) {
            out.extend_from_slice(
                rng.pick(&["\x1b[>1u", "\x1b[>5u", "\x1b[>u", "\x1b[>4u"])
                    .as_bytes(),
            );
        }
    }
    for _ in 0..rng.between(1, 12) {
        ext_token(rng, &mut out);
    }
    out
}

fn grammar_write(rng: &mut Rng) -> Vec<u8> {
    let tokens = rng.between(1, 80);
    let mut out = Vec::new();
    for _ in 0..tokens {
        token(rng, &mut out);
    }
    out
}

/// Random bytes of every value, 1 to 300.
fn byte_soup(rng: &mut Rng) -> Vec<u8> {
    let len = rng.between(1, 300);
    (0..len)
        .map(|_| u8::try_from(rng.below(256)).unwrap())
        .collect()
}

/// An OSC (title or link) or DCS with a long payload, 1 to 64 KiB.
fn payload_burst(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    let length = usize::try_from(rng.between(1, 65536)).unwrap();
    match rng.below(4) {
        0 => out.extend_from_slice(b"\x1b]0;"),
        1 => out.extend_from_slice(b"\x1b]2;"),
        2 => out.extend_from_slice(b"\x1b]8;;http://"),
        _ => out.extend_from_slice(b"\x1bP$q"),
    }
    payload_text(rng, &mut out, length);
    out.extend_from_slice(b"\x1b\\");
    // With a push/pop pair after it: the clone amplification shape.
    if rng.chance(50) {
        for _ in 0..rng.between(1, 6) {
            out.extend_from_slice(b"\x1b[22t\x1b[23t");
        }
    }
    out
}

/// Many lines, each under a link of its own (or none), so the scrollback evicts links.
fn flood(rng: &mut Rng) -> Vec<u8> {
    let lines = rng.between(40, 400);
    let mut out = Vec::new();
    for n in 0..lines {
        if rng.chance(70) {
            out.extend_from_slice(format!("\x1b]8;id={n};http://h/{n}\x1b\\").as_bytes());
        }
        out.extend_from_slice(format!("line {n} ").as_bytes());
        if rng.chance(30) {
            out.extend_from_slice(rng.pick(&TEXT).as_bytes());
        }
        if rng.chance(70) {
            out.extend_from_slice(b"\x1b]8;;\x1b\\");
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Over 131072 codes in one write, so `InputHandler::parse` cuts it into chunks: autowrap off
/// (no scrolling, cheap to print), text, then sequences that straddle the chunk boundary.
fn big_write(rng: &mut Rng) -> Vec<u8> {
    let mut out = b"\x1b[?7l".to_vec();
    out.resize(131_060, b'x');
    for _ in 0..40 {
        csi(rng, &mut out);
        out.extend_from_slice(rng.pick(&TEXT).as_bytes());
    }
    out.extend_from_slice(b"\x1b]0;");
    payload_text(rng, &mut out, 6000);
    out.extend_from_slice(b"\x07\x1b[?7h");
    out
}

// ------------------------------------------------------------------------------------------
// Operations.

#[derive(Debug, Clone)]
enum Op {
    Write(&'static str, Vec<u8>),
    Resize(usize, usize),
    EngineResize(u16, u16),
    ScrollLines(i64),
    EngineScrollLines(i32),
    ScrollPages(i64),
    ScrollToLine(usize),
    ScrollTop,
    ScrollBottom,
    EngineScrollTop,
    EngineScrollBottom,
    SetOption(OptionValue),
    Reset,
    Clear,
    Input(String, bool),
    Idle(usize),
    Read,
    Drain,
    Damage,
    AddMarker(i64),
    Key(TerminalKeyEvent),
    Mouse(TerminalMouseEvent),
    Focus(bool),
    Paste(String),
    SetClipboard(bool),
    ResetKeyboard,
}

fn all_window_options() -> WindowOptions {
    WindowOptions {
        restore_win: true,
        minimize_win: true,
        set_win_position: true,
        set_win_size_pixels: true,
        raise_win: true,
        lower_win: true,
        refresh_win: true,
        set_win_size_chars: true,
        maximize_win: true,
        fullscreen_win: true,
        get_win_state: true,
        get_win_position: true,
        get_win_size_pixels: true,
        get_screen_size_pixels: true,
        get_cell_size_pixels: true,
        get_win_size_chars: true,
        get_screen_size_chars: true,
        get_icon_title: true,
        get_win_title: true,
        push_title: true,
        pop_title: true,
        set_win_lines: true,
    }
}

fn random_option(rng: &mut Rng) -> OptionValue {
    match rng.below(14) {
        0 | 1 => OptionValue::Scrollback(*rng.pick(&[0, 0, 1, 2, 5, 30, 100, 1000, usize::MAX])),
        2 => OptionValue::TabStopWidth(*rng.pick(&[0, 1, 2, 3, 8, 16, 200, 100_000])),
        3 => OptionValue::ConvertEol(rng.chance(50)),
        4 => OptionValue::WindowsMode(rng.chance(50)),
        5 => OptionValue::ScreenReaderMode(rng.chance(50)),
        6 => OptionValue::WindowOptions(if rng.chance(70) {
            all_window_options()
        } else {
            WindowOptions::default()
        }),
        7 => OptionValue::WindowsPty(match rng.below(3) {
            0 => None,
            1 => Some(WindowsPty {
                backend: Some(PtyBackendKind::Conpty),
                build_number: Some(*rng.pick(&[18000, 21375, 21376, 30000])),
            }),
            _ => Some(WindowsPty {
                backend: Some(PtyBackendKind::Winpty),
                build_number: None,
            }),
        }),
        8 => OptionValue::ReflowCursorLine(rng.chance(50)),
        9 => OptionValue::ScrollOnEraseInDisplay(rng.chance(50)),
        10 => OptionValue::CursorStyle(*rng.pick(&[
            CursorStyle::Block,
            CursorStyle::Underline,
            CursorStyle::Bar,
        ])),
        11 => OptionValue::CursorBlink(rng.chance(50)),
        12 => OptionValue::ScrollOnUserInput(rng.chance(50)),
        _ => OptionValue::DisableStdin(rng.chance(20)),
    }
}

fn random_key(rng: &mut Rng) -> TerminalKeyEvent {
    let modifiers = TerminalModifiers::from_bits(if rng.chance(10) {
        u8::try_from(rng.below(256)).unwrap()
    } else {
        u8::try_from(rng.below(8)).unwrap()
    });
    let mut event = if rng.chance(55) {
        TerminalKeyEvent::for_key(*rng.pick(&TerminalKey::ALL), modifiers)
    } else {
        let codepoint = *rng.pick(&[
            0u32,
            0x61,
            0x41,
            0x31,
            0x20,
            0x7F,
            0x1B,
            0x2F,
            0x4F60,
            0xD800,
            0x1F600,
            0x10FFFF,
            0x11_0000,
            u32::MAX,
        ]);
        TerminalKeyEvent {
            key: TerminalKey::Character,
            modifiers,
            codepoint,
            shifted_codepoint: *rng.pick(&[0u32, 0x41, 0x21, 0xD800, u32::MAX]),
            base_layout_codepoint: *rng.pick(&[0u32, 0x61, 0x2F, 0xDFFF]),
            is_keypad: false,
            alt_is_meta: true,
        }
    };
    event.is_keypad = rng.chance(10);
    event.alt_is_meta = rng.chance(80);
    event
}

fn random_mouse(rng: &mut Rng) -> TerminalMouseEvent {
    let coordinate = |rng: &mut Rng| {
        *rng.pick(&[
            0i32,
            1,
            2,
            5,
            39,
            40,
            79,
            94,
            95,
            222,
            223,
            300,
            -1,
            i32::MAX,
            i32::MIN,
        ])
    };
    TerminalMouseEvent {
        button: *rng.pick(&[
            TerminalMouseButton::None,
            TerminalMouseButton::Left,
            TerminalMouseButton::Middle,
            TerminalMouseButton::Right,
            TerminalMouseButton::WheelUp,
            TerminalMouseButton::WheelDown,
        ]),
        kind: *rng.pick(&[
            TerminalMouseEventType::Down,
            TerminalMouseEventType::Up,
            TerminalMouseEventType::Move,
            TerminalMouseEventType::Drag,
            TerminalMouseEventType::WheelUp,
            TerminalMouseEventType::WheelDown,
        ]),
        col: coordinate(rng),
        row: coordinate(rng),
        x: coordinate(rng),
        y: coordinate(rng),
        modifiers: TerminalModifiers::from_bits(u8::try_from(rng.below(8)).unwrap()),
    }
}

/// A paste text from a hostile alphabet: C0 and C1 controls, bracket markers, CR and LF.
fn random_paste(rng: &mut Rng) -> String {
    const PIECES: [&str; 14] = [
        "a",
        "hello",
        "\r\n",
        "\n",
        "\r",
        "\x1b",
        "\x1b[200~",
        "\x1b[201~",
        "\u{9b}",
        "\u{85}",
        "\x00",
        "\x7f",
        "\u{4F60}",
        "\x1b[20\x1b[201~0~",
    ];
    let mut out = String::new();
    for _ in 0..rng.between(0, 12) {
        let piece = *rng.pick(&PIECES);
        out.push_str(piece);
    }
    out
}

fn random_op(rng: &mut Rng) -> Op {
    match rng.below(112) {
        100 => Op::Key(random_key(rng)),
        101 | 102 => Op::Mouse(random_mouse(rng)),
        103 => Op::Focus(rng.chance(50)),
        104 | 105 => Op::Paste(random_paste(rng)),
        106 | 107 => Op::SetClipboard(rng.chance(50)),
        108 => Op::ResetKeyboard,
        109..=111 => Op::Write("ext", ext_burst(rng)),
        0..=37 => Op::Write("grammar", grammar_write(rng)),
        38..=41 => Op::Write("soup", byte_soup(rng)),
        42..=43 => Op::Write("burst", payload_burst(rng)),
        44..=45 => Op::Write("flood", flood(rng)),
        46..=52 => {
            let sizes = [(0, 0), (1, 1), (2, 1), (300, 100), (80, 24)];
            let (c, r) = if rng.chance(45) {
                *rng.pick(&sizes)
            } else {
                (rng.usize_between(1, 80), rng.usize_between(1, 30))
            };
            if rng.chance(30) {
                Op::EngineResize(u16::try_from(c).unwrap(), u16::try_from(r).unwrap())
            } else {
                Op::Resize(c, r)
            }
        }
        53..=57 => {
            let amounts = [0i64, 1, -1, 3, -3, 1000, -1000, i64::MAX, i64::MIN];
            if rng.chance(30) {
                let engine = [0i32, 1, -1, 7, i32::MAX, i32::MIN, -100_000];
                Op::EngineScrollLines(*rng.pick(&engine))
            } else {
                Op::ScrollLines(*rng.pick(&amounts))
            }
        }
        58 => Op::ScrollPages(*rng.pick(&[0i64, 1, -1, 5, -5, i64::MAX, i64::MIN])),
        59 => Op::ScrollToLine(*rng.pick(&[0usize, 1, 10, 100, usize::MAX])),
        60..=61 => Op::ScrollTop,
        62..=63 => Op::ScrollBottom,
        64 => Op::EngineScrollTop,
        65 => Op::EngineScrollBottom,
        66..=73 => Op::SetOption(random_option(rng)),
        74..=75 => Op::Reset,
        76..=77 => Op::Clear,
        78..=80 => {
            let text = rng
                .pick(&["a", "\r", "\u{1b}[A", "hello \u{4F60}", ""])
                .to_string();
            Op::Input(text, rng.chance(70))
        }
        81..=84 => Op::Idle(usize::try_from(rng.between(1, 4)).unwrap()),
        85..=88 => Op::Read,
        89..=92 => Op::Drain,
        93..=95 => Op::Damage,
        _ => Op::AddMarker(*rng.pick(&[0i64, 1, -1, 50, -50, i64::MAX, i64::MIN])),
    }
}

fn describe(op: &Op) -> String {
    match op {
        Op::Write(kind, data) => format!("write {kind} {} bytes", data.len()),
        Op::Input(text, user) => format!("input {} bytes user={user}", text.len()),
        other => format!("{other:?}"),
    }
}

// ------------------------------------------------------------------------------------------
// Driving a terminal.

#[derive(Clone, Copy, Debug)]
enum Chunking {
    /// Each write as one `write` call.
    AsIs,
    /// One byte per call.
    Bytes,
    /// Cut at random offsets, seeded.
    Random(u64),
}

struct Sim {
    run: Run,
    /// Bytes handed to the engine since the last drain.
    since_drain: u64,
    /// Every event kind that was seen, by `event_slot` (for the instrument check).
    saw: [u64; SLOT_NAMES.len()],
    /// The deepest kitty stack, the highest modifyOtherKeys level and the clipboard writes of
    /// exactly 1 MiB seen (the instrument check: the bounds are reached, not just never broken).
    max_kitty_depth: usize,
    max_modify_other_keys: u8,
    mib_clipboard_writes: u64,
    /// XTVERSION replies seen (a `Response` that starts `DCS > |`).
    xtversion_replies: u64,
    /// The events drained so far as one digest line each (payloads as length and hash).
    event_log: Vec<String>,
    /// What each encoder call returned, as one digest line each.
    input_log: Vec<String>,
    /// `Drain` ops and write ops are drained after; others only when `Drain` comes.
    chunking: Chunking,
    cut_rng: SplitRng,
    enforce_events: bool,
    /// The most rows the terminal has had (see the alt buffer bound in `check_bounds`).
    peak_rows: usize,
}

/// FNV-1a, for payload digests (a failure report must not carry a payload).
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// One line per event, `None` for the kinds the chunk comparison leaves out.
fn event_digest(event: &HostEvent) -> Option<String> {
    Some(match event {
        HostEvent::A11yText(_) | HostEvent::A11yTab(_) => return None,
        HostEvent::Response(bytes) => format!("response {} {:016x}", bytes.len(), fnv(bytes)),
        HostEvent::ClipboardWrite(text) => {
            format!("clipboard {} {:016x}", text.len(), fnv(text.as_bytes()))
        }
        HostEvent::CwdChanged(text) => format!("cwd {} {:016x}", text.len(), fnv(text.as_bytes())),
        other => format!("{other:?}"),
    })
}

/// One line for what an encoder returned.
fn input_digest(kind: &str, bytes: &Option<Vec<u8>>) -> String {
    match bytes {
        None => format!("input {kind} none"),
        Some(bytes) => format!("input {kind} {} {:016x}", bytes.len(), fnv(bytes)),
    }
}

fn event_cost(event: &HostEvent) -> u64 {
    let n = |len: usize| len as u64 + 1;
    match event {
        HostEvent::Response(bytes) => n(bytes.len()),
        HostEvent::A11yText(text) => n(text.len()),
        HostEvent::Color(requests) => 8 * requests.len() as u64 + 1,
        HostEvent::A11yTab(_) | HostEvent::UnknownOsc { .. } => 16,
        HostEvent::ClipboardWrite(text) | HostEvent::CwdChanged(text) => n(text.len()),
        HostEvent::Progress(_) | HostEvent::CommandMark(_) => 8,
        HostEvent::Bell
        | HostEvent::TitleChanged
        | HostEvent::FocusReportRequested
        | HostEvent::WindowReport(_)
        | HostEvent::SyncUpdateInCommand => 1,
    }
}

fn event_slot(event: &HostEvent) -> usize {
    match event {
        HostEvent::Response(_) => 0,
        HostEvent::Bell => 1,
        HostEvent::TitleChanged => 2,
        HostEvent::Color(_) => 3,
        HostEvent::FocusReportRequested | HostEvent::WindowReport(_) => 4,
        HostEvent::A11yText(_) => 5,
        HostEvent::A11yTab(_) => 6,
        HostEvent::UnknownOsc { .. } => 7,
        HostEvent::ClipboardWrite(_) => 8,
        HostEvent::CwdChanged(_) => 9,
        HostEvent::Progress(_) => 10,
        HostEvent::CommandMark(_) => 11,
        HostEvent::SyncUpdateInCommand => 12,
    }
}

impl Sim {
    fn new(
        cols: usize,
        rows: usize,
        chunking: Chunking,
        enforce_events: bool,
        extensions: Option<&StandardExtensions>,
    ) -> Self {
        let seed = match chunking {
            Chunking::Random(seed) => seed,
            _ => 1,
        };
        Sim {
            run: Run::with_extensions(cols, rows, extensions),
            since_drain: 0,
            saw: [0; SLOT_NAMES.len()],
            max_kitty_depth: 0,
            max_modify_other_keys: 0,
            mib_clipboard_writes: 0,
            xtversion_replies: 0,
            event_log: Vec::new(),
            input_log: Vec::new(),
            chunking,
            cut_rng: SplitRng::new(seed),
            enforce_events,
            peak_rows: rows,
        }
    }

    fn term(&mut self) -> &mut Terminal {
        &mut self.run.term
    }

    /// The host duty: drain the events, keep what `Run` keeps for the dump, check the bound.
    fn drain(&mut self) {
        let events = self.run.term.drain_events();
        let mut by_kind = [0u64; SLOT_NAMES.len()];
        let mut counts = [0u64; SLOT_NAMES.len()];
        for event in events {
            by_kind[event_slot(&event)] += event_cost(&event);
            counts[event_slot(&event)] += 1;
            self.saw[event_slot(&event)] += 1;
            if matches!(&event, HostEvent::ClipboardWrite(text) if text.len() == 1 << 20) {
                self.mib_clipboard_writes += 1;
            }
            if let Some(digest) = event_digest(&event) {
                self.event_log.push(digest);
            }
            match event {
                HostEvent::Response(bytes) => {
                    self.xtversion_replies += u64::from(bytes.starts_with(b"\x1bP>|"));
                    self.run
                        .responses
                        .push(String::from_utf8_lossy(&bytes).into_owned());
                }
                HostEvent::TitleChanged => {
                    self.run.title = self
                        .run
                        .term
                        .core()
                        .input_handler()
                        .window_title()
                        .to_owned();
                }
                _ => {}
            }
        }
        if self.enforce_events {
            let cost: u64 = by_kind.iter().sum();
            let allowed = EVENT_RATIO * self.since_drain + EVENT_SLACK;
            assert!(
                cost <= allowed,
                "drain_events weighed {cost} bytes for {} bytes written (allowed {allowed}); by \
                 kind {SLOT_NAMES:?} = {by_kind:?}",
                self.since_drain
            );
            // last-wins kinds (plan M7 D8): at most one per drain; unknown OSC: Lunegit's 64
            for slot in [8, 9, 10] {
                assert!(
                    counts[slot] <= 1,
                    "{} {}-events in one drain",
                    counts[slot],
                    SLOT_NAMES[slot]
                );
            }
            assert!(
                counts[7] <= 64,
                "{} unknown-osc events in one drain",
                counts[7]
            );
        }
        self.since_drain = 0;
    }

    fn write(&mut self, data: &[u8]) {
        self.since_drain += data.len() as u64;
        match self.chunking {
            Chunking::AsIs => self.run.term.write(data),
            Chunking::Bytes => {
                // One byte per call; inside a long payload only the first and last 256 bytes
                // go one by one and the middle in 509-byte chunks (a 64 KiB burst written byte
                // by byte costs more than the rest of a seed).
                let head = data.len().min(256);
                let tail_start = data.len().saturating_sub(256).max(head);
                for byte in &data[..head] {
                    self.run.term.write(std::slice::from_ref(byte));
                }
                for chunk in data[head..tail_start].chunks(509) {
                    self.run.term.write(chunk);
                }
                for byte in &data[tail_start..] {
                    self.run.term.write(std::slice::from_ref(byte));
                }
            }
            Chunking::Random(_) => {
                let mut at = 0;
                while at < data.len() {
                    let len = 1 + self.cut_rng.below(48);
                    let end = (at + len).min(data.len());
                    self.run.term.write(&data[at..end]);
                    at = end;
                }
            }
        }
    }

    fn apply(&mut self, op: &Op) {
        self.apply_op(op);
        self.peak_rows = self.peak_rows.max(self.run.term.rows());
        let (main, alt) = self.run.term.keyboard_protocol().stack_depths();
        self.max_kitty_depth = self.max_kitty_depth.max(main).max(alt);
        self.max_modify_other_keys = self
            .max_modify_other_keys
            .max(self.run.term.modes().modify_other_keys);
    }

    fn apply_op(&mut self, op: &Op) {
        match op {
            Op::Write(_, data) => {
                self.write(data);
                self.drain();
            }
            Op::Resize(c, r) => self.term().resize(*c, *r),
            Op::EngineResize(c, r) => VtEngine::resize(self.term(), *c, *r),
            Op::ScrollLines(n) => self.term().scroll_lines(*n),
            Op::EngineScrollLines(n) => VtEngine::scroll_lines(self.term(), *n),
            Op::ScrollPages(n) => self.term().scroll_pages(*n),
            Op::ScrollToLine(n) => self.term().scroll_to_line(*n),
            Op::ScrollTop => self.term().scroll_to_top(),
            Op::ScrollBottom => self.term().scroll_to_bottom(),
            Op::EngineScrollTop => VtEngine::scroll_to_top(self.term()),
            Op::EngineScrollBottom => VtEngine::scroll_to_bottom(self.term()),
            Op::SetOption(value) => {
                // An invalid value (tab stop width 0) is an error and changes nothing.
                let _ = self.term().set_option(value.clone());
            }
            Op::Reset => {
                self.term().reset();
                // a reset clears the keyboard protocol (kitty stacks, modifyOtherKeys)
                assert_eq!(self.term().keyboard_protocol().stack_depths(), (0, 0));
                assert_eq!(self.term().modes().modify_other_keys, 0);
            }
            Op::Clear => self.term().clear(),
            Op::Input(text, user) => {
                self.since_drain += text.len() as u64;
                self.term().input(text, *user);
            }
            Op::Idle(rounds) => {
                for _ in 0..*rounds {
                    if !self.term().idle() {
                        break;
                    }
                }
            }
            Op::Read => {
                let _ = self.term().viewport();
                let _ = self.term().modes();
                let mut cells = Vec::new();
                let rows = self.term().rows();
                for y in 0..=rows {
                    let _ = self.term().read_row(y, &mut cells);
                }
            }
            Op::Drain => self.drain(),
            Op::Damage => {
                let _ = self.term().take_damage();
            }
            Op::AddMarker(offset) => {
                let _ = self.term().add_marker(*offset);
            }
            Op::Key(event) => {
                let bytes = self.term().key_event(event);
                self.input_log.push(input_digest("key", &bytes));
            }
            Op::Mouse(event) => {
                let bytes = self.term().mouse_event(event);
                self.input_log.push(input_digest("mouse", &bytes));
            }
            Op::Focus(focused) => {
                let bytes = self.term().focus_event(*focused);
                self.input_log.push(input_digest("focus", &bytes));
            }
            Op::Paste(text) => {
                let bytes = self.term().paste(text);
                self.input_log.push(input_digest("paste", &bytes));
                if let Some(bytes) = bytes {
                    check_paste_invariant(text, &bytes, self.term().modes().bracketed_paste);
                }
            }
            Op::SetClipboard(enabled) => self.term().set_clipboard_write(*enabled),
            Op::ResetKeyboard => {
                self.term().reset_keyboard_protocol();
                assert_eq!(self.term().keyboard_protocol().stack_depths(), (0, 0));
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Bounds.

fn check_bounds(sim: &Sim) {
    let term = &sim.run.term;
    let (cols, rows) = (term.cols(), term.rows());
    let scrollback = term.options().scrollback;
    let services = term.core().input_handler().services();
    let buffers = term.buffers();

    // Each buffer's line list is bounded by the screen plus the scrollback (alt: no scrollback).
    let normal = buffers.normal();
    let alt = buffers.alt();
    assert!(
        normal.lines.len() <= rows.saturating_add(scrollback),
        "normal buffer holds {} lines (rows {rows}, scrollback {scrollback})",
        normal.lines.len()
    );
    // The alternate buffer has no scrollback, but shrinking the terminal does not trim its list
    // (xterm.js: `CircularList.maxLength`'s setter keeps `length`; checked against the oracle,
    // `@xterm/headless` 6.0.0 holds 7 lines at 1 row for the same input), so its bound is the
    // most rows the terminal has had, not the current ones. Growth is still bounded: 20000
    // line feeds in the alt buffer at 1 row after a shrink from 24 leave 24 lines, in both.
    assert!(
        alt.lines.len() <= sim.peak_rows,
        "alt buffer holds {} lines (rows {rows}, peak rows {})",
        alt.lines.len(),
        sim.peak_rows
    );

    // Title stacks.
    let (window, icon) = term.core().input_handler().title_stack_lens();
    assert!(window <= TITLE_STACK_LIMIT, "window title stack {window}");
    assert!(icon <= TITLE_STACK_LIMIT, "icon name stack {icon}");

    // Link table against live markers.
    let osc_link = &services.osc_link;
    let live = normal.markers().len() + alt.markers().len();
    assert!(
        osc_link.len() <= live,
        "{} links for {live} live markers",
        osc_link.len()
    );
    assert!(
        osc_link.marker_count() <= live,
        "links hold {} markers, {live} are live",
        osc_link.marker_count()
    );
    // LUNETERM: the caps (review S2): links, and their markers unless one link holds them all.
    assert!(osc_link.len() <= MAX_LINKS, "{} links", osc_link.len());
    assert!(
        osc_link.marker_count() <= MAX_LINK_MARKERS || osc_link.len() == 1,
        "{} links hold {} markers",
        osc_link.len(),
        osc_link.marker_count()
    );
    for id in osc_link.link_ids() {
        let markers: Vec<_> = osc_link.link_markers(id).unwrap().collect();
        assert!(!markers.is_empty(), "link {id} has no marker");
        for marker in markers {
            let set = if marker.alt {
                alt.markers()
            } else {
                normal.markers()
            };
            assert!(
                set.get(marker.id).is_some(),
                "link {id} keeps a disposed marker"
            );
        }
    }

    // The keyboard protocol (plan M7 D1, D3): bounded whatever the stream sent.
    let (main_depth, alt_depth) = term.keyboard_protocol().stack_depths();
    assert!(
        main_depth <= KITTY_STACK_MAX && alt_depth <= KITTY_STACK_MAX,
        "kitty stacks hold {main_depth} and {alt_depth} entries"
    );
    let modes = term.modes();
    assert!(
        modes.modify_other_keys <= MODIFY_OTHER_KEYS_MAX,
        "modifyOtherKeys level {}",
        modes.modify_other_keys
    );
    assert_eq!(
        modes.kitty_keyboard_flags & !KITTY_SUPPORTED,
        0,
        "kitty flags {}",
        modes.kitty_keyboard_flags
    );

    // The active buffer's cursor.
    let active = buffers.active();
    assert!(active.y < rows, "cursor row {} of {rows}", active.y);

    // The viewport has exactly rows x cols cells.
    let viewport = term.viewport();
    assert_eq!((viewport.cols, viewport.rows), (cols, rows));
    assert_eq!(viewport.cells.len(), rows, "viewport rows");
    for (y, row) in viewport.cells.iter().enumerate() {
        assert_eq!(row.len(), cols, "viewport row {y} width");
    }
    let mut cells = Vec::new();
    assert!(term.read_row(rows.saturating_sub(1), &mut cells));
    assert_eq!(cells.len(), cols, "read_row width");
    assert!(!term.read_row(rows, &mut cells), "read_row past the bottom");
}

// ------------------------------------------------------------------------------------------
// One seed.

struct Outcome {
    wrap_off_skipped: bool,
    saw: [u64; SLOT_NAMES.len()],
    xtversion_replies: u64,
    with_extensions: bool,
    max_kitty_depth: usize,
    max_modify_other_keys: u8,
    mib_clipboard_writes: u64,
}

/// The paste invariant (plan M7 D7) on what `Terminal::paste` returned, reporting positions and
/// lengths only, never the text.
fn check_paste_invariant(text: &str, bytes: &[u8], bracketed: bool) {
    if !bracketed {
        let mut expected = Vec::new();
        let raw = text.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'\r' && raw.get(i + 1) == Some(&b'\n') {
                expected.push(b'\r');
                i += 2;
            } else {
                expected.push(if raw[i] == b'\n' { b'\r' } else { raw[i] });
                i += 1;
            }
        }
        assert!(
            bytes == expected.as_slice(),
            "unbracketed paste differs from the normalized input ({} bytes in, {} out)",
            raw.len(),
            bytes.len()
        );
        return;
    }
    assert!(
        bytes.starts_with(b"\x1b[200~") && bytes.ends_with(b"\x1b[201~") && bytes.len() >= 12,
        "paste markers ({} bytes)",
        bytes.len()
    );
    let middle = &bytes[6..bytes.len() - 6];
    for (at, byte) in middle.iter().enumerate() {
        assert!(*byte != 0x1b, "ESC in a bracketed paste at {at}");
        assert!(
            *byte >= 0x20 || *byte == 0x09 || *byte == 0x0d,
            "C0 byte {byte:#x} in a bracketed paste at {at}"
        );
    }
    assert!(
        !middle
            .windows(2)
            .any(|w| w[0] == 0xC2 && (0x80..=0x9F).contains(&w[1])),
        "C1 control in a bracketed paste"
    );
}

/// The extensions of a seed: none on about half, otherwise a random gate and VS16; the seeds that
/// write the OSC 52 cap cases always have the gate on.
fn extensions_of(seed: u64) -> Option<StandardExtensions> {
    let mut rng = Rng(seed ^ 0xE47E_4E55_0000_0001);
    let cap_seed = seed % OSC52_CAP_SEED_EVERY == 3;
    if !cap_seed && !rng.chance(50) {
        return None;
    }
    Some(StandardExtensions {
        xtversion: Some(XTVERSION_NAME.to_string()),
        vs16: rng.chance(50),
        clipboard_write: cap_seed || rng.chance(50),
    })
}

/// Whether `data` holds a `CSI ? <params> l` whose parameters include 7 (DECAWM reset); the
/// check is textual and deliberately generous (it ignores `h` and what follows).
fn resets_autowrap(data: &[u8]) -> bool {
    let mut at = 0;
    while let Some(found) = data[at..].windows(3).position(|w| w == b"\x1b[?") {
        let start = at + found + 3;
        let end = start
            + data[start..]
                .iter()
                .take_while(|b| b.is_ascii_digit() || **b == b';' || **b == b':')
                .count();
        if data.get(end) == Some(&b'l') && data[start..end].split(|b| *b == b';').any(|p| p == b"7")
        {
            return true;
        }
        at = start;
    }
    false
}

/// The state compared across chunkings.
fn snapshot(sim: &Sim) -> (Vec<String>, lune_vt::engine::Viewport) {
    let mut lines = dump_run(&sim.run);
    lines.push(format!("modes {:?}", sim.run.term.modes()));
    lines.push(format!(
        "kitty depths {:?}",
        sim.run.term.keyboard_protocol().stack_depths()
    ));
    lines.extend(sim.event_log.iter().map(|line| format!("event {line}")));
    lines.extend(sim.input_log.iter().cloned());
    (lines, sim.run.term.viewport())
}

fn run_seed(seed: u64, log: &mut Vec<String>, enforce_events: bool) -> Outcome {
    let mut rng = Rng(seed);
    let cols = rng.usize_between(2, 40);
    let rows = rng.usize_between(1, 16);
    let initial_scrollback = *rng.pick(&[0usize, 3, 20, 100]);
    let mut ops: Vec<Op> = (0..OPS_PER_SEED).map(|_| random_op(&mut rng)).collect();
    ops.insert(
        0,
        Op::SetOption(OptionValue::Scrollback(initial_scrollback)),
    );
    if seed % 50 == 7 {
        let at = rng.usize_between(1, ops.len());
        ops.insert(at, Op::Write("big", big_write(&mut rng)));
    }
    if seed % OSC52_CAP_SEED_EVERY == 3 {
        let at = rng.usize_between(1, ops.len());
        ops.insert(at, Op::Write("osc52-cap", osc52_cap_write(&mut rng)));
    }
    let extensions = extensions_of(seed);
    if seed % OSC52_CAP_SEED_EVERY == 3 {
        // the cap cases are only seen if the gate is on when they arrive
        for op in &mut ops {
            if let Op::SetClipboard(enabled) = op {
                *enabled = true;
            }
        }
    }
    let wrap_off = ops
        .iter()
        .any(|op| matches!(op, Op::Write(_, data) if resets_autowrap(data)));
    let checkpoints = [
        ops.len() / 4,
        ops.len() / 2,
        3 * ops.len() / 4,
        ops.len() - 1,
    ];

    // The reference run, with every bound checked after every operation.
    let mut reference = Sim::new(
        cols,
        rows,
        Chunking::AsIs,
        enforce_events,
        extensions.as_ref(),
    );
    let mut snapshots = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        log.push(format!("{index}: {}", describe(op)));
        reference.apply(op);
        check_bounds(&reference);
        if checkpoints.contains(&index) {
            snapshots.push((index, snapshot(&reference)));
        }
    }
    // The host drains after every write; make sure nothing is left that would hide a bound.
    reference.drain();

    // Replays with different chunkings.
    let mut skipped = false;
    log.push("replays".to_string());
    for chunking in [Chunking::Bytes, Chunking::Random(seed ^ 0xA5A5)] {
        let mut replay = Sim::new(cols, rows, chunking, false, extensions.as_ref());
        for (index, op) in ops.iter().enumerate() {
            replay.apply(op);
            let Some((_, (dump, viewport))) = snapshots.iter().find(|(at, _)| *at == index) else {
                continue;
            };
            let (replay_dump, replay_viewport) = snapshot(&replay);
            let difference = first_difference(&replay_dump, dump);
            if difference.is_some() || replay_viewport != *viewport {
                if wrap_off {
                    skipped = true;
                    break;
                }
                let detail = match difference {
                    Some(line) => format!(
                        "dump line {line}\n  replay:    {}\n  reference: {}",
                        clip(replay_dump.get(line), 120),
                        clip(dump.get(line), 120)
                    ),
                    None => "viewport differs, dump equal".to_string(),
                };
                panic!(
                    "{chunking:?} differs after op {index} ({}): {detail}",
                    describe(op)
                );
            }
        }
        if skipped {
            break;
        }
    }
    Outcome {
        wrap_off_skipped: skipped,
        saw: reference.saw,
        xtversion_replies: reference.xtversion_replies,
        with_extensions: extensions.is_some(),
        max_kitty_depth: reference.max_kitty_depth,
        max_modify_other_keys: reference.max_modify_other_keys,
        mib_clipboard_writes: reference.mib_clipboard_writes,
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<non-string panic>".to_string()
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

#[test]
fn random_operations_never_panic_stay_bounded_and_ignore_chunking() {
    let seeds: Vec<u64> = match env_u64("LUNE_VT_ENGINE_FUZZ_SEED") {
        Some(one) => vec![one],
        None => (0..env_u64("LUNE_VT_ENGINE_FUZZ_SEEDS").unwrap_or(DEFAULT_SEEDS)).collect(),
    };
    let mut failures: Vec<String> = Vec::new();
    let mut wrap_off_skips = 0;
    let mut saw = [0u64; SLOT_NAMES.len()];
    let mut xtversion_replies = 0;
    let mut with_extensions = 0;
    let (mut max_depth, mut max_mok, mut mib_writes) = (0, 0, 0);
    for &seed in &seeds {
        let mut log: Vec<String> = Vec::new();
        let result = catch_unwind(AssertUnwindSafe(|| run_seed(seed, &mut log, true)));
        match result {
            Ok(outcome) => {
                wrap_off_skips += usize::from(outcome.wrap_off_skipped);
                xtversion_replies += outcome.xtversion_replies;
                with_extensions += usize::from(outcome.with_extensions);
                max_depth = max_depth.max(outcome.max_kitty_depth);
                max_mok = max_mok.max(outcome.max_modify_other_keys);
                mib_writes += outcome.mib_clipboard_writes;
                for (total, n) in saw.iter_mut().zip(outcome.saw) {
                    *total += n;
                }
            }
            Err(payload) => {
                let tail: Vec<&String> = log.iter().rev().take(6).collect();
                failures.push(format!(
                    "seed {seed}: {}\n  last ops (newest first): {tail:?}",
                    panic_text(payload.as_ref())
                ));
            }
        }
    }
    println!(
        "engine fuzz: {} seeds ({with_extensions} with extensions), {} failed, {wrap_off_skips} \
         wrap-off skips, XTVERSION replies {xtversion_replies}, deepest kitty stack {max_depth}, \
         highest modifyOtherKeys {max_mok}, 1 MiB clipboard writes {mib_writes}, events seen {SLOT_NAMES:?} = \
         {saw:?}",
        seeds.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} seeds failed (replay with LUNE_VT_ENGINE_FUZZ_SEED=<seed>):\n{}",
        failures.len(),
        seeds.len(),
        failures
            .iter()
            .take(10)
            .map(|f| first_line(f).to_string() + "\n" + f.lines().nth(1).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Instrument check: a run of the default size must exercise every event kind, the M7 ones
    // included; a generator that never reached the handlers would pass everything above.
    if env_u64("LUNE_VT_ENGINE_FUZZ_SEED").is_none() && seeds.len() >= 60 {
        for (slot, name) in SLOT_NAMES.iter().enumerate() {
            assert!(saw[slot] > 0, "no {name} event in {} seeds", seeds.len());
        }
        assert!(
            xtversion_replies > 0,
            "no XTVERSION reply in {} seeds",
            seeds.len()
        );
        assert_eq!(
            max_depth, KITTY_STACK_MAX,
            "no kitty stack ever reached its limit"
        );
        assert_eq!(
            max_mok, MODIFY_OTHER_KEYS_MAX,
            "modifyOtherKeys never reached its top level"
        );
        assert!(
            mib_writes > 0,
            "no OSC 52 write of exactly 1 MiB (the cap seeds)"
        );
        assert!(
            with_extensions > 0 && with_extensions < seeds.len(),
            "extensions on {with_extensions} of {} seeds",
            seeds.len()
        );
    }
}

/// The generator is deterministic per seed and reaches the shapes the fuzz relies on.
#[test]
fn generator_is_deterministic_and_varied() {
    let a: Vec<Op> = {
        let mut rng = Rng(5);
        (0..OPS_PER_SEED).map(|_| random_op(&mut rng)).collect()
    };
    let b: Vec<Op> = {
        let mut rng = Rng(5);
        (0..OPS_PER_SEED).map(|_| random_op(&mut rng)).collect()
    };
    assert_eq!(
        a.iter().map(describe).collect::<Vec<_>>(),
        b.iter().map(describe).collect::<Vec<_>>()
    );
    let mut kinds = std::collections::BTreeSet::new();
    let mut esc = 0usize;
    let mut high = 0usize;
    for seed in 0..60 {
        let mut rng = Rng(seed);
        for _ in 0..OPS_PER_SEED {
            let op = random_op(&mut rng);
            kinds.insert(
                describe(&op)
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap()
                    .to_string(),
            );
            if let Op::Write(_, data) = &op {
                esc += data.iter().filter(|&&b| b == 0x1b).count();
                high += data.iter().filter(|&&b| b >= 0x80).count();
            }
        }
    }
    assert!(kinds.len() >= 16, "only {kinds:?}");
    assert!(esc > 5000 && high > 2000, "esc {esc}, high {high}");
}

/// The chunk comparison is not vacuous: it sees the known upstream dependence (the example of the
/// header, checked against the oracle with `@xterm/headless` 6.0.0: one write gives
/// `["你", "a"]`, byte by byte `["", "a"]` in the first row), and `resets_autowrap` flags
/// exactly that class of input.
#[test]
fn chunk_comparison_sees_the_known_upstream_dependence() {
    let input = "\x1b[?7l\u{4f60}\u{4f60}a".as_bytes();
    assert!(resets_autowrap(input));
    let op = Op::Write("known", input.to_vec());
    let mut whole = Sim::new(2, 2, Chunking::AsIs, false, None);
    whole.apply(&op);
    let mut bytes = Sim::new(2, 2, Chunking::Bytes, false, None);
    bytes.apply(&op);
    let (a, b) = (snapshot(&whole), snapshot(&bytes));
    assert!(first_difference(&a.0, &b.0).is_some(), "dumps equal");
    assert_ne!(a.1, b.1, "viewports equal");
    // Without autowrap off the same text is chunk-invariant.
    let plain = Op::Write("plain", "\u{4f60}\u{4f60}a".as_bytes().to_vec());
    let mut whole = Sim::new(2, 2, Chunking::AsIs, false, None);
    whole.apply(&plain);
    let mut bytes = Sim::new(2, 2, Chunking::Bytes, false, None);
    bytes.apply(&plain);
    assert_eq!(snapshot(&whole), snapshot(&bytes));
}

#[test]
fn autowrap_detector_is_textual_and_generous() {
    assert!(resets_autowrap(b"x\x1b[?7l"));
    assert!(resets_autowrap(b"\x1b[?1;7;25l"));
    assert!(resets_autowrap(b"\x1b[?25h\x1b[?7l"));
    assert!(!resets_autowrap(b"\x1b[?7h"));
    assert!(!resets_autowrap(b"\x1b[?17l\x1b[7l\x1b[?70l"));
    assert!(!resets_autowrap(b"\x1b[?"));
}

/// Instrument check for the bounds: a fresh and a busy terminal pass `check_bounds`, and the event
/// bound fires when the events weigh more than the bytes counted as written allow.
#[test]
fn bound_checks_accept_normal_state_and_the_event_bound_fires() {
    let mut sim = Sim::new(10, 4, Chunking::AsIs, true, None);
    check_bounds(&sim);
    sim.apply(&Op::Write(
        "lines",
        b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\n\x1b]8;;http://h/\x1b\\x".to_vec(),
    ));
    check_bounds(&sim);
    let caught = catch_unwind(AssertUnwindSafe(|| {
        let mut sim = Sim::new(10, 4, Chunking::AsIs, true, None);
        // an 8 KiB response for 0 bytes counted as written (`Sim::input` would count them)
        sim.term()
            .input(&"x".repeat(2 * EVENT_SLACK as usize), true);
        sim.drain();
    }));
    assert!(caught.is_err(), "the event bound did not fire");
}
