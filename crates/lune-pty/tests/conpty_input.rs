//! ConPTY input measurement (M3 step 4, ledger 10, decision D5). `harness = false`: Windows
//! only; elsewhere it checks the case table and prints "skipped".
//!
//! What a Windows program actually receives when the terminal writes a VT key sequence into the
//! pseudoconsole. The test binary re-executes itself through `PortablePtyBackend` (our real
//! stack, portable-pty's ConPTY flags included) as `--child <mode> <log>`:
//!
//! - `vt`: `CONIN$` in `ENABLE_VIRTUAL_TERMINAL_INPUT` only, `SetConsoleCP(65001)`, `ReadFile`;
//!   what VT-aware programs (ssh, vim, WSL) see;
//! - `keys`: `CONIN$` in `ENABLE_EXTENDED_FLAGS` only, `ReadConsoleInputW` key-down records;
//!   what cmd and PSReadLine see.
//!
//! The child reports through a side file, never through the PTY output (conhost re-renders the
//! output instead of passing bytes through). Every case is one write, then after a pacing gap a
//! mark byte (0x07), so each observation is attributed to exactly one case. The measured table is
//! compared with `tests/data/conpty-input.tsv`; `docs/conpty-input.md` explains it.

use std::fmt::Write as _;

/// Separates cases; no case may contain it (checked at startup).
const MARK: u8 = 0x07;

/// (name, bytes the terminal writes). Names are unique; the order is the write order.
const CASES: &[(&str, &[u8])] = &[
    // printable ASCII and UTF-8
    ("a", b"a"),
    ("A", b"A"),
    ("tilde", b"~"),
    ("space", b" "),
    ("utf8-2 e-acute", "\u{e9}".as_bytes()),
    ("utf8-3 euro", "\u{20ac}".as_bytes()),
    ("utf8-4 emoji", "\u{1f600}".as_bytes()),
    // editing keys
    ("enter", b"\r"),
    ("tab", b"\t"),
    ("shift-tab", b"\x1b[Z"),
    ("backspace", b"\x7f"),
    ("ctrl-h", b"\x08"),
    ("esc", b"\x1b"),
    // control characters
    ("ctrl-c", b"\x03"),
    ("ctrl-d", b"\x04"),
    ("ctrl-z", b"\x1a"),
    ("ctrl-space", b"\x00"),
    ("ctrl-backslash", b"\x1c"),
    ("ctrl-]", b"\x1d"),
    ("ctrl-^", b"\x1e"),
    ("ctrl-_", b"\x1f"),
    // Alt as ESC prefix
    ("alt-a", b"\x1ba"),
    ("alt-shift-a", b"\x1bA"),
    ("alt-backspace", b"\x1b\x7f"),
    // arrows
    ("up", b"\x1b[A"),
    ("down", b"\x1b[B"),
    ("right", b"\x1b[C"),
    ("left", b"\x1b[D"),
    ("up-app", b"\x1bOA"),
    ("down-app", b"\x1bOB"),
    ("right-app", b"\x1bOC"),
    ("left-app", b"\x1bOD"),
    ("shift-up", b"\x1b[1;2A"),
    ("alt-up", b"\x1b[1;3A"),
    ("ctrl-up", b"\x1b[1;5A"),
    ("ctrl-shift-up", b"\x1b[1;6A"),
    ("shift-left", b"\x1b[1;2D"),
    ("alt-left", b"\x1b[1;3D"),
    ("ctrl-left", b"\x1b[1;5D"),
    ("ctrl-shift-left", b"\x1b[1;6D"),
    // navigation block
    ("home-csi", b"\x1b[H"),
    ("end-csi", b"\x1b[F"),
    ("home-ss3", b"\x1bOH"),
    ("end-ss3", b"\x1bOF"),
    ("home-tilde", b"\x1b[1~"),
    ("end-tilde", b"\x1b[4~"),
    ("insert", b"\x1b[2~"),
    ("delete", b"\x1b[3~"),
    ("ctrl-delete", b"\x1b[3;5~"),
    ("pgup", b"\x1b[5~"),
    ("pgdn", b"\x1b[6~"),
    // function keys
    ("f1", b"\x1bOP"),
    ("f2", b"\x1bOQ"),
    ("f3", b"\x1bOR"),
    ("f4", b"\x1bOS"),
    ("f5", b"\x1b[15~"),
    ("f6", b"\x1b[17~"),
    ("f7", b"\x1b[18~"),
    ("f8", b"\x1b[19~"),
    ("f9", b"\x1b[20~"),
    ("f10", b"\x1b[21~"),
    ("f11", b"\x1b[23~"),
    ("f12", b"\x1b[24~"),
    ("shift-f5", b"\x1b[15;2~"),
    ("ctrl-f1", b"\x1b[1;5P"),
    // focus reports
    ("focus-in", b"\x1b[I"),
    ("focus-out", b"\x1b[O"),
    // bracketed paste
    ("paste", b"\x1b[200~hi there\x1b[201~"),
    // mouse
    ("mouse-sgr-press", b"\x1b[<0;10;5M"),
    ("mouse-sgr-release", b"\x1b[<0;10;5m"),
    ("mouse-x10-press", b"\x1b[M *%"),
    // other key protocols
    ("kitty-ctrl-a", b"\x1b[97;5u"),
    ("kitty-shift-enter", b"\x1b[13;2u"),
    ("modify-other-keys-ctrl-a", b"\x1b[27;5;97~"),
    // replies a terminal sends
    ("cpr-reply", b"\x1b[12;34R"),
    ("da1-reply", b"\x1b[?1;2c"),
    // win32-input-mode: key down 'a' (Vk 65, Sc 30, Uc 97, Kd 1, Cs 0, Rc 1)
    ("win32-input-a", b"\x1b[65;30;97;1;0;1_"),
];

const MODES: [&str; 2] = ["vt", "keys"];

fn hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Panics unless every case name is unique, no case is empty and no case contains [`MARK`].
fn check_cases() {
    let mut names = std::collections::HashSet::new();
    for (name, bytes) in CASES {
        assert!(names.insert(*name), "duplicate case name {name:?}");
        assert!(!bytes.is_empty(), "case {name:?} is empty");
        assert!(
            !bytes.contains(&MARK),
            "case {name:?} contains the mark byte 0x07"
        );
        assert!(
            !name.contains('\t') && !name.contains('|'),
            "case name {name:?} must not contain a tab or '|'"
        );
    }
}

// ---- child log format ------------------------------------------------------------------------
//
// One line per observation, written with a single write each:
//   READY | ERROR <text> | vt <hex bytes> | vt0 (ReadFile returned 0 bytes)
//   key vk=XX sc=XX ch=XXXX cs=XXXX[ rep=N] | focus <0|1> | mouse ... | event <type>

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Mark(String),
    Obs(String),
}

/// Parses the complete lines of a child log into items. `READY`/`ERROR` lines are skipped
/// (the parent checks them separately).
fn parse_log(log: &str, mode: &str) -> Vec<Item> {
    let mut items = Vec::new();
    // Only lines terminated by '\n' are complete; the last one may still be being written.
    let complete = match log.rfind('\n') {
        Some(end) => &log[..end],
        None => "",
    };
    for line in complete.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("vt ") {
            for b in rest.split(' ').filter(|t| !t.is_empty()) {
                if u8::from_str_radix(b, 16).ok() == Some(MARK) {
                    items.push(Item::Mark("07".into()));
                } else {
                    items.push(Item::Obs(b.into()));
                }
            }
        } else if line == "vt0" {
            items.push(Item::Obs("[read0]".into()));
        } else if let Some(record) = line.strip_prefix("key ") {
            if mode == "keys" && record.split(' ').any(|t| t == "ch=0007") {
                // conhost synthesizes a Ctrl press before Ctrl+G: those modifier-only records
                // directly before the mark belong to the mark, not to the case.
                let mut parts = vec![record.to_string()];
                while let Some(Item::Obs(prev)) = items.last() {
                    if !is_modifier_press(prev) {
                        break;
                    }
                    parts.insert(0, prev.clone());
                    items.pop();
                }
                items.push(Item::Mark(parts.join(" + ")));
            } else {
                items.push(Item::Obs(record.into()));
            }
        } else if line.starts_with("focus ")
            || line.starts_with("mouse ")
            || line.starts_with("event ")
        {
            items.push(Item::Obs(format!("[{line}]")));
        }
    }
    items
}

/// A key record of Shift, Ctrl or Alt alone (no character), as conhost synthesizes around keys.
fn is_modifier_press(record: &str) -> bool {
    // An "up ..." record starts with "up", so it never matches.
    matches!(record.split(' ').next(), Some("vk=10" | "vk=11" | "vk=12"))
        && record.split(' ').any(|t| t == "ch=0000")
}

fn count_marks(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i, Item::Mark(_))).count()
}

/// Observation for marks `start..end` (0-based mark indices; the case owns everything after mark
/// `start - 1` up to and including mark `end - 1`). Mark records that differ from `plain` (the
/// sync mark's own record) are shown, so an ESC folded into a mark is visible.
fn observation(items: &[Item], start: usize, end: usize, plain: &str, sep: &str) -> String {
    let mut marks = 0usize;
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        match item {
            Item::Mark(record) => {
                if marks >= start && marks < end {
                    if record != plain {
                        parts.push(format!("(mark {record})"));
                    }
                    if marks + 1 < end {
                        parts.push("(extra mark)".into());
                    }
                }
                marks += 1;
            }
            Item::Obs(o) => {
                if marks >= start && marks < end {
                    parts.push(o.clone());
                }
            }
        }
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(sep)
    }
}

fn separator(mode: &str) -> &'static str {
    if mode == "vt" { " " } else { " ; " }
}

// ---- golden ----------------------------------------------------------------------------------

/// One measured row: mode, case name, sent bytes (hex), observation.
#[derive(Debug, Clone)]
struct Row {
    mode: String,
    case: String,
    sent: String,
    observed: String,
}

fn tsv_line(row: &Row) -> String {
    format!("{}\t{}\t{}\t{}", row.mode, row.case, row.sent, row.observed)
}

/// Compares measured rows with the golden TSV text; returns the differences (empty = match).
/// A golden observation may list alternatives separated by " | ".
fn compare(rows: &[Row], golden: &str) -> Vec<String> {
    let mut expected: std::collections::BTreeMap<(String, String), (String, Vec<String>)> =
        std::collections::BTreeMap::new();
    let mut diffs = Vec::new();
    for (n, line) in golden.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 4 {
            diffs.push(format!(
                "golden line {}: expected 4 tab-separated fields",
                n + 1
            ));
            continue;
        }
        let alternatives = fields[3].split(" | ").map(str::to_string).collect();
        expected.insert(
            (fields[0].into(), fields[1].into()),
            (fields[2].into(), alternatives),
        );
    }
    for row in rows {
        match expected.remove(&(row.mode.clone(), row.case.clone())) {
            None => diffs.push(format!("+ new row:  {}", tsv_line(row))),
            Some((sent, _)) if sent != row.sent => diffs.push(format!(
                "~ {} {}: case bytes changed (golden {sent}, now {})",
                row.mode, row.case, row.sent
            )),
            Some((_, alternatives)) if !alternatives.contains(&row.observed) => {
                diffs.push(format!(
                    "~ {} {}: expected {:?}, got {:?}",
                    row.mode,
                    row.case,
                    alternatives.join(" | "),
                    row.observed
                ))
            }
            Some(_) => {}
        }
    }
    for (mode, case) in expected.into_keys() {
        diffs.push(format!("- stale golden row: {mode} {case}"));
    }
    diffs
}

/// Checks the parsing and attribution logic on a synthetic log (runs on every OS).
fn self_test() {
    let log = "READY\nvt 07\nvt 61 07\nvt 1b\nvt 07 07\nvt0\nvt 07\nvt 62";
    let items = parse_log(log, "vt");
    assert_eq!(count_marks(&items), 5, "self-test: mark count");
    let sep = separator("vt");
    assert_eq!(observation(&items, 1, 2, "07", sep), "61");
    assert_eq!(observation(&items, 2, 3, "07", sep), "1b");
    assert_eq!(observation(&items, 3, 5, "07", sep), "(extra mark) [read0]");
    assert_eq!(hex(b"\x1b[A"), "1b 5b 41");
    let keys = "key vk=11 sc=1d ch=0000 cs=0008\nkey vk=47 sc=22 ch=0007 cs=0008\n\
                key vk=41 sc=1e ch=0061 cs=0000\nkey vk=11 sc=1d ch=0000 cs=0008\n\
                key vk=47 sc=22 ch=0007 cs=000a\n";
    let items = parse_log(keys, "keys");
    assert_eq!(count_marks(&items), 2, "self-test: keys mark count");
    let plain = "vk=11 sc=1d ch=0000 cs=0008 + vk=47 sc=22 ch=0007 cs=0008";
    assert_eq!(
        observation(&items, 1, 2, plain, separator("keys")),
        "vk=41 sc=1e ch=0061 cs=0000 ; \
         (mark vk=11 sc=1d ch=0000 cs=0008 + vk=47 sc=22 ch=0007 cs=000a)"
    );
    let row = |o: &str| Row {
        mode: "vt".into(),
        case: "a".into(),
        sent: "61".into(),
        observed: o.into(),
    };
    let golden = "# comment\nvt\ta\t61\t61 | 41\n";
    assert!(compare(&[row("41")], golden).is_empty());
    assert_eq!(compare(&[row("62")], golden).len(), 1);
    assert_eq!(compare(&[], golden).len(), 1);
}

#[cfg(not(windows))]
fn main() {
    check_cases();
    self_test();
    let _ = MODES; // used by the Windows harness only
    println!("conpty_input: skipped (not Windows; case table and parser self-test ok)");
}

#[cfg(windows)]
fn main() {
    check_cases();
    self_test();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--child") {
        let mode = args.get(2).map(String::as_str).unwrap_or("");
        let log = args.get(3).map(String::as_str).unwrap_or("");
        imp::child(mode, log);
    } else {
        imp::parent();
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code)]

    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use lune_pty::{PortablePtyBackend, Pty, PtyBackend, PtySize, PtyWriter, SpawnOptions};
    use windows_sys::Win32::Foundation::{HANDLE, TRUE};
    use windows_sys::Win32::System::Console::{
        CAPSLOCK_ON, ENABLE_EXTENDED_FLAGS, ENABLE_VIRTUAL_TERMINAL_INPUT, FOCUS_EVENT,
        INPUT_RECORD, KEY_EVENT, MOUSE_EVENT, NUMLOCK_ON, ReadConsoleInputW, SCROLLLOCK_ON,
        SetConsoleCP, SetConsoleCtrlHandler, SetConsoleMode,
    };

    use super::{
        CASES, MARK, MODES, Row, compare, count_marks, hex, observation, parse_log, separator,
    };

    /// Pacing between a case and its mark, like the gap between two key presses. Not an assertion.
    const PACE: Duration = Duration::from_millis(50);
    /// Upper bound for the child to come up.
    const READY_DEADLINE: Duration = Duration::from_secs(20);
    /// Upper bound for one mark to arrive.
    const MARK_DEADLINE: Duration = Duration::from_secs(5);
    /// After a resent mark, time for the original one to show up late.
    const SETTLE: Duration = Duration::from_millis(300);
    const CP_UTF8: u32 = 65001;
    const VK_MENU: u16 = 0x12;

    // ---- child -------------------------------------------------------------------------------

    struct Log(File);

    impl Log {
        fn line(&mut self, text: &str) {
            // One write per line, so the parent never sees a line interleaved with another.
            let _ = self.0.write_all(format!("{text}\n").as_bytes());
            let _ = self.0.flush();
        }

        fn fail(&mut self, what: &str) -> ! {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
            self.line(&format!("ERROR {what} (os error {code})"));
            std::process::exit(1);
        }
    }

    pub(super) fn child(mode: &str, log_path: &str) {
        let file = OpenOptions::new()
            .append(true)
            .open(log_path)
            .unwrap_or_else(|e| std::process::exit(e.raw_os_error().unwrap_or(3)));
        let mut log = Log(file);
        // The console input buffer of the pseudoconsole, independent of the inherited std handles.
        let conin = match OpenOptions::new().read(true).write(true).open("CONIN$") {
            Ok(f) => f,
            Err(_) => log.fail("open CONIN$"),
        };
        let handle = conin.as_raw_handle() as HANDLE;
        // SAFETY: a null handler routine with TRUE makes the process ignore Ctrl+C; no pointers.
        if unsafe { SetConsoleCtrlHandler(None, TRUE) } == 0 {
            log.fail("SetConsoleCtrlHandler");
        }
        match mode {
            "vt" => {
                // SAFETY: plain value arguments; `handle` is the live CONIN$ handle owned by `conin`.
                unsafe {
                    if SetConsoleCP(CP_UTF8) == 0 {
                        log.fail("SetConsoleCP");
                    }
                    if SetConsoleMode(handle, ENABLE_VIRTUAL_TERMINAL_INPUT) == 0 {
                        log.fail("SetConsoleMode vt");
                    }
                }
                log.line("READY");
                read_vt(conin, &mut log);
            }
            "keys" => {
                // SAFETY: `handle` is the live CONIN$ handle owned by `conin`.
                if unsafe { SetConsoleMode(handle, ENABLE_EXTENDED_FLAGS) } == 0 {
                    log.fail("SetConsoleMode keys");
                }
                log.line("READY");
                read_keys(handle, &mut log);
            }
            other => log.fail(&format!("unknown mode {other:?}")),
        }
    }

    fn read_vt(mut conin: File, log: &mut Log) -> ! {
        let mut buf = [0u8; 1024];
        let mut zeros = 0u32;
        loop {
            // std's File::read is ReadFile on the console handle (not ReadConsoleW like Stdin).
            match conin.read(&mut buf) {
                Ok(0) => {
                    // One zero-byte read is an observation (Ctrl+Z); a stream of them is EOF.
                    zeros += 1;
                    if zeros > 100 {
                        log.fail("ReadFile keeps returning 0 bytes");
                    }
                    log.line("vt0");
                }
                Ok(n) => {
                    zeros = 0;
                    log.line(&format!("vt {}", hex(&buf[..n])));
                }
                Err(_) => log.fail("ReadFile"),
            }
        }
    }

    fn read_keys(handle: HANDLE, log: &mut Log) -> ! {
        let mut records = [INPUT_RECORD::default(); 64];
        loop {
            let mut count = 0u32;
            // SAFETY: `records` is writable for `records.len()` entries and `count` is a valid
            // out pointer; `handle` is the live CONIN$ handle.
            let ok = unsafe {
                ReadConsoleInputW(
                    handle,
                    records.as_mut_ptr(),
                    records.len() as u32,
                    &mut count,
                )
            };
            if ok == 0 {
                log.fail("ReadConsoleInputW");
            }
            for record in &records[..count as usize] {
                let kind = u32::from(record.EventType);
                if kind == KEY_EVENT {
                    // SAFETY: EventType is KEY_EVENT, so KeyEvent is the active union field.
                    let key = unsafe { record.Event.KeyEvent };
                    // SAFETY: UnicodeChar is the field ReadConsoleInputW (W) fills.
                    let ch = unsafe { key.uChar.UnicodeChar };
                    // Key-down records, plus the one key-up that carries input: conhost delivers
                    // a character outside the keyboard layout as Alt+numpad, with the character
                    // on the Alt (VK_MENU) key-up.
                    let up = key.bKeyDown == 0;
                    if up && !(key.wVirtualKeyCode == VK_MENU && ch != 0) {
                        continue;
                    }
                    let cs = key.dwControlKeyState & !(NUMLOCK_ON | SCROLLLOCK_ON | CAPSLOCK_ON);
                    let mut line = format!(
                        "key {}vk={:02x} sc={:02x} ch={ch:04x} cs={cs:04x}",
                        if up { "up " } else { "" },
                        key.wVirtualKeyCode,
                        key.wVirtualScanCode
                    );
                    if key.wRepeatCount > 1 {
                        line.push_str(&format!(" rep={}", key.wRepeatCount));
                    }
                    log.line(&line);
                } else if kind == FOCUS_EVENT {
                    // SAFETY: EventType is FOCUS_EVENT, so FocusEvent is the active union field.
                    let focus = unsafe { record.Event.FocusEvent };
                    log.line(&format!("focus {}", i32::from(focus.bSetFocus != 0)));
                } else if kind == MOUSE_EVENT {
                    // SAFETY: EventType is MOUSE_EVENT, so MouseEvent is the active union field.
                    let m = unsafe { record.Event.MouseEvent };
                    log.line(&format!(
                        "mouse x={} y={} btn={:x} flags={:x}",
                        m.dwMousePosition.X, m.dwMousePosition.Y, m.dwButtonState, m.dwEventFlags
                    ));
                } else {
                    log.line(&format!("event {kind}"));
                }
            }
        }
    }

    // ---- parent ------------------------------------------------------------------------------

    /// Drains the PTY output for the whole session (conhost stalls if nobody reads), answers
    /// ConPTY's cursor-position request `ESC[6n` (D11: the engine does this in the product) and
    /// notes whether conhost asked for win32-input-mode (`ESC[?9001h`, ignored here).
    struct Output {
        asked_win32_input: Arc<AtomicBool>,
        eof: mpsc::Receiver<()>,
        tail: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    fn start_output(mut reader: Box<dyn Read + Send>, writer: PtyWriter) -> Output {
        let asked = Arc::new(AtomicBool::new(false));
        let tail = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let (thread_asked, thread_tail) = (Arc::clone(&asked), Arc::clone(&tail));
        thread::Builder::new()
            .name("conpty-input-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                // Carry the last bytes across reads: a sequence can split between two reads.
                let mut carry: Vec<u8> = Vec::new();
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let mut window = std::mem::take(&mut carry);
                    let old = window.len();
                    window.extend_from_slice(&buf[..n]);
                    // Count only matches that end in the new bytes (old ones were answered).
                    let dsr = find_all(&window, b"\x1b[6n")
                        .filter(|&end| end > old)
                        .count();
                    for _ in 0..dsr {
                        let _ = writer.write_all(b"\x1b[1;1R");
                    }
                    if find_all(&window, b"\x1b[?9001h").next().is_some() {
                        thread_asked.store(true, Ordering::SeqCst);
                    }
                    if let Ok(mut t) = thread_tail.lock() {
                        t.extend_from_slice(&buf[..n]);
                        let excess = t.len().saturating_sub(2048);
                        t.drain(..excess);
                    }
                    let keep = window.len().saturating_sub(16);
                    carry = window.split_off(keep);
                }
                let _ = tx.send(());
            })
            .expect("reader thread");
        Output {
            asked_win32_input: asked,
            eof: rx,
            tail,
        }
    }

    /// End offsets (exclusive) of every occurrence of `needle` in `haystack`.
    fn find_all<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
        haystack
            .windows(needle.len())
            .enumerate()
            .filter(move |(_, w)| *w == needle)
            .map(move |(i, _)| i + needle.len())
    }

    fn read_log(path: &Path) -> String {
        std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }

    fn fail_with_output(what: &str, output: &Output, log: &str) -> ! {
        let tail = output
            .tail
            .lock()
            .map(|t| String::from_utf8_lossy(&t).into_owned())
            .unwrap_or_default();
        panic!("conpty_input: {what}\nchild log:\n{log}\nlast PTY output: {tail:?}");
    }

    /// Waits until the log holds more than `seen` marks, or `deadline` passes. Returns the count.
    fn wait_marks(path: &Path, mode: &str, seen: usize, deadline: Duration) -> usize {
        let start = Instant::now();
        loop {
            let n = count_marks(&parse_log(&read_log(path), mode));
            if n > seen || start.elapsed() >= deadline {
                return n;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    struct Measured {
        rows: Vec<Row>,
        asked_win32_input: bool,
        resent: usize,
    }

    fn measure(mode: &str) -> Measured {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let log_path = std::env::temp_dir().join(format!(
            "lune-pty-conpty-input-{}-{nanos}-{mode}.log",
            std::process::id()
        ));
        File::create(&log_path).expect("create child log");
        let exe = std::env::current_exe().expect("current_exe");
        let options = SpawnOptions {
            program: Some(exe),
            args: vec![
                "--child".into(),
                mode.into(),
                log_path.to_str().expect("temp path is UTF-8").into(),
            ],
            ..SpawnOptions::default()
        };
        let size = PtySize {
            columns: 120,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty: Box<dyn Pty> = PortablePtyBackend::new()
            .spawn(&options, size)
            .expect("spawn child through PortablePtyBackend");
        let writer = pty.writer();
        let output = start_output(pty.take_reader().expect("take_reader"), pty.writer());

        // Wait for READY (the child writes it after its console setup).
        let start = Instant::now();
        loop {
            let log = read_log(&log_path);
            if log.lines().any(|l| l.starts_with("ERROR")) {
                fail_with_output(&format!("{mode} child failed"), &output, &log);
            }
            if log.lines().any(|l| l == "READY") {
                break;
            }
            if start.elapsed() >= READY_DEADLINE {
                fail_with_output(&format!("{mode} child not ready"), &output, &log);
            }
            thread::sleep(Duration::from_millis(10));
        }

        // Sync mark: everything the console produced before it (startup noise) is discarded.
        writer.write_all(&[MARK]).expect("write mark");
        let mut seen = wait_marks(&log_path, mode, 0, MARK_DEADLINE);
        if seen == 0 {
            fail_with_output(
                &format!("{mode}: harness broken (sync mark never arrived)"),
                &output,
                &read_log(&log_path),
            );
        }

        let mut spans = Vec::with_capacity(CASES.len());
        let mut resent = Vec::new();
        for (name, bytes) in CASES {
            let first = seen;
            writer.write_all(bytes).expect("write case"); // one write per case
            thread::sleep(PACE);
            writer.write_all(&[MARK]).expect("write mark");
            seen = wait_marks(&log_path, mode, first, MARK_DEADLINE);
            if seen == first {
                // Lost mark: send another, then give the original time to arrive late.
                writer.write_all(&[MARK]).expect("write mark");
                seen = wait_marks(&log_path, mode, first, MARK_DEADLINE);
                if seen == first {
                    fail_with_output(
                        &format!("{mode}: harness lost sync at case {name:?}"),
                        &output,
                        &read_log(&log_path),
                    );
                }
                thread::sleep(SETTLE);
                seen = count_marks(&parse_log(&read_log(&log_path), mode));
                resent.push(*name);
            }
            spans.push((first, seen));
        }

        let log = read_log(&log_path);
        let items = parse_log(&log, mode);
        let plain = match items.iter().find_map(|i| match i {
            super::Item::Mark(r) => Some(r.clone()),
            super::Item::Obs(_) => None,
        }) {
            Some(r) => r,
            None => fail_with_output("no mark in the log", &output, &log),
        };
        let rows = CASES
            .iter()
            .zip(&spans)
            .map(|((name, bytes), &(first, last))| {
                let mut observed = observation(&items, first, last, &plain, separator(mode));
                if resent.contains(name) {
                    observed.push_str(" (mark resent)");
                }
                Row {
                    mode: mode.into(),
                    case: (*name).into(),
                    sent: hex(bytes),
                    observed,
                }
            })
            .collect();

        let _ = pty.kill();
        drop(writer);
        drop(pty);
        if output.eof.recv_timeout(READY_DEADLINE).is_err() {
            println!("conpty_input: warning: {mode} PTY output did not reach EOF after kill");
        }
        let _ = std::fs::remove_file(&log_path);
        Measured {
            rows,
            asked_win32_input: output.asked_win32_input.load(Ordering::SeqCst),
            resent: resent.len(),
        }
    }

    fn os_version() -> String {
        Command::new("cmd")
            .args(["/d", "/c", "ver"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|e| format!("unknown ({e})"))
    }

    /// Fails with "harness broken" unless `a` reads as itself in both modes.
    fn instrument_check(rows: &[Row]) {
        for row in rows.iter().filter(|r| r.case == "a") {
            let ok = match row.mode.as_str() {
                "vt" => row.observed == "61",
                _ => {
                    let records: Vec<&str> = row.observed.split(" ; ").collect();
                    records.len() == 1
                        && records[0].split(' ').any(|t| t == "vk=41")
                        && records[0].split(' ').any(|t| t == "ch=0061")
                }
            };
            assert!(
                ok,
                "conpty_input: harness broken: case `a` in {} mode read {:?}",
                row.mode, row.observed
            );
        }
    }

    pub(super) fn parent() {
        let system_root = std::env::var_os("SystemRoot").map(PathBuf::from);
        let system_conpty = system_root
            .map(|r| r.join("System32").join("conpty.dll").exists())
            .unwrap_or(false);

        let mut rows = Vec::new();
        let mut asked = Vec::new();
        for mode in MODES {
            let measured = measure(mode);
            asked.push(format!(
                "{mode}={}",
                if measured.asked_win32_input {
                    "yes"
                } else {
                    "no"
                }
            ));
            if measured.resent > 0 {
                println!("conpty_input: {mode}: {} mark(s) resent", measured.resent);
            }
            rows.extend(measured.rows);
        }

        println!("BEGIN conpty-input");
        println!("# os: {}", os_version());
        println!("# System32\\conpty.dll exists: {system_conpty}");
        println!(
            "# portable-pty 0.9.0 flags: INHERIT_CURSOR | RESIZE_QUIRK | WIN32_INPUT_MODE; \
             ESC[?9001h requested by conhost: {} (ignored, plain VT sent)",
            asked.join(" ")
        );
        println!("# mode\tcase\tsent\tobserved");
        for row in &rows {
            println!("{}", super::tsv_line(row));
        }
        println!("END conpty-input");

        instrument_check(&rows);

        let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/conpty-input.tsv");
        let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!(
                "conpty_input: golden {} is missing; review the table above, save it there \
                 (without the BEGIN/END lines) and write docs/conpty-input.md from it",
                golden_path.display()
            )
        });
        let diffs = compare(&rows, &golden);
        if !diffs.is_empty() {
            for d in &diffs {
                println!("{d}");
            }
            panic!(
                "conpty_input: {} difference(s) from {}. ConPTY input behaviour (or the case \
                 table) changed: review the table above, update the TSV (a case may list \
                 alternatives separated by \" | \") and docs/conpty-input.md",
                diffs.len(),
                golden_path.display()
            );
        }
        println!("conpty_input: ok ({} rows match the golden)", rows.len());
    }
}
