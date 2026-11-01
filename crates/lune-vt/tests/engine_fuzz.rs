//! Seeded engine-level fuzz of `lune_vt::Terminal` (plan M6 step 10).
//!
//! Per seed: a terminal at a random small size and a list of random operations over its public
//! API: `write` (grammar-biased escape sequences with extreme and huge parameters, raw byte soup,
//! OSC/DCS payload bursts, OSC 8 link floods that evict links through the scrollback), `resize`
//! (0x0, 1x1 and 300x100 included, through `Terminal` and `VtEngine`), `scroll_*`, `set_option`
//! (scrollback including 0, tab stop width, convert_eol, windows_mode, screen_reader_mode, every
//! window option on, ...), `reset`, `clear`, `input`, `idle`, `viewport`/`read_row`/`modes`,
//! `drain_events`, `take_damage` and markers. Each seed runs under `catch_unwind`; a failure
//! reports the seed and the last operations (kind and sizes, never the input bytes).
//!
//! Checked per seed:
//!  - no panic (debug builds keep integer-overflow checks on);
//!  - bounds after every operation (`check_bounds`): buffer line counts, title stacks, link table
//!    against live markers, event bytes against the bytes written, viewport shape;
//!  - chunk invariance: the same operations with each write cut into 1-byte chunks, and into
//!    random chunks, end with the same `screen::dump` lines and the same `Viewport` as the
//!    operations as generated, at a few checkpoints and at the end.
//!
//! The event bound is the one check that fails on the engine today: `PENDING_EVENT_BOUND` lists
//! the seeds (two amplifiers, a title pop and REP with screen reader mode on) and the test asserts
//! that exactly those fail it; each is re-run without the bound so nothing hides behind it.
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

use lune_vt::services::options_service::{
    CursorStyle, OptionValue, PtyBackendKind, WindowOptions, WindowsPty,
};
use lune_vt::{HostEvent, Terminal, VtEngine};

use screen::{Run, SplitRng, dump_run};
use trace::{clip, first_difference};

const DEFAULT_SEEDS: u64 = 60;
const OPS_PER_SEED: usize = 36;
/// Title stacks are capped at ten entries (`InputHandler` STACK_LIMIT).
const TITLE_STACK_LIMIT: usize = 10;
/// Events may weigh this many times the bytes that caused them, plus a flat allowance.
const EVENT_RATIO: u64 = 16;
const EVENT_SLACK: u64 = 4096;

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
    match rng.below(100) {
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

fn random_op(rng: &mut Rng) -> Op {
    match rng.below(100) {
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
    /// Every event kind that was seen (for the instrument check).
    saw: [u64; 8],
    /// `Drain` ops and write ops are drained after; others only when `Drain` comes.
    chunking: Chunking,
    cut_rng: SplitRng,
    enforce_events: bool,
    /// The most rows the terminal has had (see the alt buffer bound in `check_bounds`).
    peak_rows: usize,
}

fn event_cost(event: &HostEvent) -> u64 {
    let n = |len: usize| len as u64 + 1;
    match event {
        HostEvent::Response(bytes) => n(bytes.len()),
        HostEvent::Title(text) | HostEvent::A11yChar(text) => n(text.len()),
        HostEvent::Color(requests) => 8 * requests.len() as u64 + 1,
        HostEvent::A11yTab(_) | HostEvent::UnknownOsc { .. } => 16,
        HostEvent::ClipboardWrite { selection, data } => n(selection.len() + data.len()),
        HostEvent::Bell | HostEvent::FocusReportRequested | HostEvent::WindowReport(_) => 1,
    }
}

fn event_slot(event: &HostEvent) -> usize {
    match event {
        HostEvent::Response(_) => 0,
        HostEvent::Bell => 1,
        HostEvent::Title(_) => 2,
        HostEvent::Color(_) => 3,
        HostEvent::FocusReportRequested | HostEvent::WindowReport(_) => 4,
        HostEvent::A11yChar(_) => 5,
        HostEvent::A11yTab(_) => 6,
        HostEvent::UnknownOsc { .. } | HostEvent::ClipboardWrite { .. } => 7,
    }
}

impl Sim {
    fn new(cols: usize, rows: usize, chunking: Chunking, enforce_events: bool) -> Self {
        let seed = match chunking {
            Chunking::Random(seed) => seed,
            _ => 1,
        };
        Sim {
            run: Run::new(cols, rows),
            since_drain: 0,
            saw: [0; 8],
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
        let mut by_kind = [0u64; 8];
        for event in events {
            by_kind[event_slot(&event)] += event_cost(&event);
            self.saw[event_slot(&event)] += 1;
            match event {
                HostEvent::Response(bytes) => self
                    .run
                    .responses
                    .push(String::from_utf8_lossy(&bytes).into_owned()),
                HostEvent::Title(title) => self.run.title = title,
                _ => {}
            }
        }
        if self.enforce_events {
            let cost: u64 = by_kind.iter().sum();
            let allowed = EVENT_RATIO * self.since_drain + EVENT_SLACK;
            assert!(
                cost <= allowed,
                "drain_events weighed {cost} bytes for {} bytes written (allowed {allowed}); by \
                 kind [response, bell, title, color, report, a11y char, a11y tab, other] = \
                 {by_kind:?}",
                self.since_drain
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
            Op::Reset => self.term().reset(),
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
    for id in osc_link.link_ids() {
        let markers = osc_link.link_markers(id).unwrap();
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
    saw: [u64; 8],
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
    (dump_run(&sim.run), sim.run.term.viewport())
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
    let mut reference = Sim::new(cols, rows, Chunking::AsIs, enforce_events);
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
        let mut replay = Sim::new(cols, rows, chunking, false);
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

/// Seeds below 2400 whose events outweigh their input (`drain_events` bound), found by this test
/// and not fixed in M6 step 10: the event queue is a design decision (plan D2), see
/// `known_event_amplifiers_still_amplify`. The test asserts that the seeds failing the event
/// bound are exactly these (within the run's seeds), so a fix must empty the list. Each of
/// these seeds, replayed without the event bound, passes every other check. After a change to
/// the generator the list is regenerated: `LUNE_VT_ENGINE_FUZZ_SEEDS=2400 cargo test -p lune-vt
/// --release --test engine_fuzz` prints the failing seeds.
///
/// Title class: XTWINOPS 22/23 with `push_title`/`pop_title` on re-emits a stored title of up to
/// 10 MB per pop. A11y class: with `screen_reader_mode` on, REP (up to 1 << 20 repeats) fires
/// one `A11yChar` per repeated character.
const PENDING_EVENT_BOUND: &[u64] = &[
    34, 521, 569, 581, 606, 637, 1034, 1049, 1119, 1319, 1331, 1757, 1762, 2036, 2204, 2276,
];

const EVENT_BOUND_MESSAGE: &str = "drain_events weighed";

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
    let mut event_bound_seeds: Vec<u64> = Vec::new();
    let mut wrap_off_skips = 0;
    let mut saw = [0u64; 8];
    for &seed in &seeds {
        let mut log: Vec<String> = Vec::new();
        let mut result = catch_unwind(AssertUnwindSafe(|| run_seed(seed, &mut log, true)));
        if let Err(payload) = &result {
            if panic_text(payload.as_ref()).starts_with(EVENT_BOUND_MESSAGE) {
                // Record it, then run the seed again without the event bound: the assertion
                // ended the first run early and must not hide anything after it.
                event_bound_seeds.push(seed);
                log.clear();
                result = catch_unwind(AssertUnwindSafe(|| run_seed(seed, &mut log, false)));
            }
        }
        match result {
            Ok(outcome) => {
                wrap_off_skips += usize::from(outcome.wrap_off_skipped);
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
        "engine fuzz: {} seeds, {} failed, {wrap_off_skips} wrap-off skips, event bound seeds \
         {event_bound_seeds:?}, events seen [response, bell, title, color, report, a11y char, \
         a11y tab, other] = {saw:?}",
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
    let expected: Vec<u64> = PENDING_EVENT_BOUND
        .iter()
        .copied()
        .filter(|seed| seeds.contains(seed))
        .collect();
    assert_eq!(
        event_bound_seeds, expected,
        "the seeds that break the event bound differ from PENDING_EVENT_BOUND (a fixed seed must \
         be removed from the list, a new one is a finding)"
    );
    // Instrument check: a run of the default size must exercise every event kind but the ones
    // only M7 produces; a generator that never reached the handlers would pass everything above.
    if env_u64("LUNE_VT_ENGINE_FUZZ_SEED").is_none() && seeds.len() >= 60 {
        for (slot, name) in [
            "response",
            "bell",
            "title",
            "color",
            "report",
            "a11y char",
            "a11y tab",
        ]
        .iter()
        .enumerate()
        {
            assert!(saw[slot] > 0, "no {name} event in {} seeds", seeds.len());
        }
    }
}

/// The two amplifiers behind `PENDING_EVENT_BOUND`, as small deterministic inputs, and the
/// instrument check that the event bound sees real amplification. These assert the current
/// (bad) behaviour: when the engine is fixed they fail, and the fix removes them with the list.
#[test]
fn known_event_amplifiers_still_amplify() {
    fn weight(sim: &mut Sim, input: &[u8]) -> (u64, u64) {
        sim.term().write(input);
        let cost = sim
            .run
            .term
            .drain_events()
            .iter()
            .map(event_cost)
            .sum::<u64>();
        (cost, EVENT_RATIO * input.len() as u64 + EVENT_SLACK)
    }
    // Title: a 60 KB title, then ten push/pop pairs (windowOptions pushTitle/popTitle on).
    let mut sim = Sim::new(10, 4, Chunking::AsIs, false);
    sim.apply(&Op::SetOption(OptionValue::WindowOptions(
        all_window_options(),
    )));
    let mut input = b"\x1b]0;".to_vec();
    input.extend(std::iter::repeat_n(b'x', 60_000));
    input.push(0x07);
    let _ = weight(&mut sim, &input);
    let pairs = b"\x1b[22;2t\x1b[23;2t".repeat(10);
    let (cost, allowed) = weight(&mut sim, &pairs);
    assert!(
        cost > allowed,
        "title pop no longer amplifies: {cost} <= {allowed}"
    );
    // A11y: REP with screen reader mode on.
    let mut sim = Sim::new(10, 4, Chunking::AsIs, false);
    sim.apply(&Op::SetOption(OptionValue::ScreenReaderMode(true)));
    let (cost, allowed) = weight(&mut sim, b"a\x1b[100000b");
    assert!(
        cost > allowed,
        "REP no longer amplifies a11y events: {cost} <= {allowed}"
    );
    // And the same input with screen reader mode off stays small.
    let mut sim = Sim::new(10, 4, Chunking::AsIs, false);
    let (cost, allowed) = weight(&mut sim, b"a\x1b[100000b");
    assert!(cost <= allowed, "{cost} > {allowed}");
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
    let mut whole = Sim::new(2, 2, Chunking::AsIs, false);
    whole.apply(&op);
    let mut bytes = Sim::new(2, 2, Chunking::Bytes, false);
    bytes.apply(&op);
    let (a, b) = (snapshot(&whole), snapshot(&bytes));
    assert!(first_difference(&a.0, &b.0).is_some(), "dumps equal");
    assert_ne!(a.1, b.1, "viewports equal");
    // Without autowrap off the same text is chunk-invariant.
    let plain = Op::Write("plain", "\u{4f60}\u{4f60}a".as_bytes().to_vec());
    let mut whole = Sim::new(2, 2, Chunking::AsIs, false);
    whole.apply(&plain);
    let mut bytes = Sim::new(2, 2, Chunking::Bytes, false);
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
    let mut sim = Sim::new(10, 4, Chunking::AsIs, true);
    check_bounds(&sim);
    sim.apply(&Op::Write(
        "lines",
        b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\n\x1b]8;;http://h/\x1b\\x".to_vec(),
    ));
    check_bounds(&sim);
    let caught = catch_unwind(AssertUnwindSafe(|| {
        let mut sim = Sim::new(10, 4, Chunking::AsIs, true);
        // an 8 KiB response for 0 bytes counted as written (`Sim::input` would count them)
        sim.term()
            .input(&"x".repeat(2 * EVENT_SLACK as usize), true);
        sim.drain();
    }));
    assert!(caught.is_err(), "the event bound did not fire");
}
