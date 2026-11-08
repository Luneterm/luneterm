//! The PTY children: the test binary re-runs itself as `e2e --child <mode> ...` (plan M9 D19, the
//! pattern of lune-pty's `conpty_input` test). No `unsafe`; no child prints its environment except
//! `env3`, which prints the three variables E9 names.
//!
//! Modes:
//!
//! * `emit <hex|@ms>...`: writes each hex argument's bytes (an `@<ms>` argument pauses), then waits
//!   for stdin EOF or 10 s.
//! * `flood <bytes> <kind>`: `plain`, `sgr`, `bel` or `dsr` write about `<bytes>` bytes and then
//!   `FLOOD-DONE`; `plain-keys` is `plain` inside a scroll region below a pinned first row, with a
//!   thread that answers every key with `K<count>` on that row.
//! * `echo-hex [hex|@ms]...` (Unix): switches its tty to raw mode, prints `RAW:`, emits the
//!   arguments, prints `S`, then prints every input byte as two lowercase hex digits, and a `!`
//!   for the sentinel byte 0x1E.
//! * `env3`: prints `$TERM $COLORTERM $TERM_PROGRAM`.
//! * `exit <code>`, `sleep`, `heartbeat <file>`.
//!
//! Every child ends by itself after at most [`LIFETIME`], so a crashed test leaves nothing behind.

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// The longest any child runs.
const LIFETIME: Duration = Duration::from_secs(180);
/// How long `emit` and `echo-hex` wait for stdin to close.
const WAIT: Duration = Duration::from_secs(10);

/// What the flood child prints when it is done.
pub(crate) const FLOOD_DONE: &str = "FLOOD-DONE";
/// The byte the tests type to end an `echo-hex` listing: it prints `!` instead of hex.
pub(crate) const SENTINEL: u8 = 0x1e;
/// Set while the key thread wants stdout: the flood loop lets it have the lock (std's mutex is
/// not fair, and a flood that re-locks at once could starve the marker for seconds).
static MARKER_WAITING: AtomicBool = AtomicBool::new(false);
/// A flood chunk, bytes.
const CHUNK: usize = 60 * 1024;

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 == 1 {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

/// One flood unit: `kind`'s repeated bytes. The flood is a whole number of units, so no escape
/// sequence is ever cut.
pub(crate) fn flood_unit(kind: &str) -> Option<&'static [u8]> {
    match kind {
        "plain" | "plain-keys" => Some(
            b"0123456789abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789abcdefgh\r\n",
        ),
        "sgr" => Some(b"\x1b[31mred \x1b[1;32mbold green\x1b[0m \x1b[48;5;21;38;2;255;200;0mrgb on 21\x1b[m plain tail of the line....\r\n"),
        "bel" => Some(b"\x07"),
        "dsr" => Some(b"\x1b[6n"),
        _ => None,
    }
}

/// How many bytes a `flood <bytes> <kind>` child writes for its payload (a whole number of units
/// not above `bytes`).
pub(crate) fn flood_payload(bytes: usize, kind: &str) -> usize {
    flood_unit(kind).map_or(0, |u| bytes / u.len() * u.len())
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    thread::spawn(|| {
        thread::sleep(LIFETIME);
        std::process::exit(3);
    });
    let Some(mode) = args.first() else {
        eprintln!("child: no mode");
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    match mode.as_str() {
        "emit" => {
            if let Err(e) = emit(rest) {
                eprintln!("child emit: {e}");
                return ExitCode::from(2);
            }
            wait_for_eof();
            ExitCode::SUCCESS
        }
        "flood" => flood(rest),
        "echo-hex" => echo_hex(rest),
        "env3" => {
            let var = |k: &str| std::env::var(k).unwrap_or_else(|_| "<unset>".to_owned());
            let line = format!(
                "{} {} {}\r\n",
                var("TERM"),
                var("COLORTERM"),
                var("TERM_PROGRAM")
            );
            let mut out = io::stdout().lock();
            let _ = out.write_all(line.as_bytes());
            let _ = out.flush();
            ExitCode::SUCCESS
        }
        "exit" => {
            let code = rest.first().and_then(|c| c.parse::<u8>().ok()).unwrap_or(0);
            ExitCode::from(code)
        }
        "sleep" => {
            thread::sleep(WAIT * 6);
            ExitCode::SUCCESS
        }
        "heartbeat" => heartbeat(rest),
        other => {
            eprintln!("child: unknown mode {other}");
            ExitCode::from(2)
        }
    }
}

/// Writes the arguments' hex bytes (`@<ms>` pauses) to stdout, flushing each.
fn emit(args: &[String]) -> Result<(), String> {
    for arg in args {
        if let Some(ms) = arg.strip_prefix('@') {
            let ms: u64 = ms.parse().map_err(|_| format!("bad pause {arg}"))?;
            thread::sleep(Duration::from_millis(ms));
            continue;
        }
        let bytes = hex_decode(arg).ok_or_else(|| "bad hex".to_owned())?;
        let mut out = io::stdout().lock();
        out.write_all(&bytes).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Returns when stdin reaches EOF (or fails), or after [`WAIT`].
fn wait_for_eof() {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut stdin = io::stdin().lock();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(WAIT);
}

fn write_out(bytes: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}

/// Puts the tty on stdin in the mode `args` ask of `/bin/stty`.
#[cfg(unix)]
fn stty(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new("/bin/stty")
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("/bin/stty: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("/bin/stty {args:?} failed: {status}"))
}

#[cfg(not(unix))]
fn stty(_args: &[&str]) -> Result<(), String> {
    Ok(())
}

fn flood(args: &[String]) -> ExitCode {
    let (Some(bytes), Some(kind)) = (
        args.first().and_then(|b| b.parse::<usize>().ok()),
        args.get(1),
    ) else {
        eprintln!("child flood: usage: flood <bytes> <kind>");
        return ExitCode::from(2);
    };
    let Some(unit) = flood_unit(kind) else {
        eprintln!("child flood: unknown kind");
        return ExitCode::from(2);
    };
    let keys = kind == "plain-keys";
    if keys && let Err(e) = start_key_echo() {
        eprintln!("child flood: {e}");
        return ExitCode::from(2);
    }
    let units = bytes / unit.len();
    let per_chunk = (CHUNK / unit.len()).max(1);
    let chunk: Vec<u8> = unit.repeat(per_chunk);
    let result = (|| -> io::Result<()> {
        if keys {
            // Rows 2..=24 scroll; row 1 keeps the key markers. The tests use 80x24.
            write_out(b"\x1b[2;24r\x1b[24;1H")?;
        }
        let mut left = units;
        while left > 0 {
            while MARKER_WAITING.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_micros(100));
            }
            let n = left.min(per_chunk);
            write_out(&chunk[..n * unit.len()])?;
            left -= n;
        }
        write_out(FLOOD_DONE.as_bytes())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("child flood: {e}");
            ExitCode::from(1)
        }
    }
}

/// A thread that counts the keys it receives and writes `K<count>` on the first row (cursor saved
/// and restored), as one write between the flood's writes.
fn start_key_echo() -> Result<(), String> {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    // Unix: one byte per key, no echo. Windows reads a line per key (the test types `x\r`).
    stty(&["-echo", "-icanon"])?;
    thread::spawn(|| {
        let mut stdin = io::stdin().lock();
        loop {
            let got = if cfg!(unix) {
                let mut b = [0u8; 1];
                matches!(stdin.read(&mut b), Ok(1))
            } else {
                let mut line = String::new();
                matches!(io::BufRead::read_line(&mut stdin, &mut line), Ok(n) if n > 0)
            };
            if !got {
                return;
            }
            let n = COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            MARKER_WAITING.store(true, Ordering::SeqCst);
            let sent = write_out(format!("\x1b7\x1b[1;1HK{n:04}\x1b8").as_bytes());
            MARKER_WAITING.store(false, Ordering::SeqCst);
            if sent.is_err() {
                return;
            }
        }
    });
    Ok(())
}

#[cfg(unix)]
fn echo_hex(args: &[String]) -> ExitCode {
    if let Err(e) = stty(&["raw", "-echo"]) {
        eprintln!("child echo-hex: {e}");
        return ExitCode::from(2);
    }
    if write_out(b"RAW:").is_err() {
        return ExitCode::from(1);
    }
    if let Err(e) = emit(args) {
        eprintln!("child echo-hex: {e}");
        return ExitCode::from(2);
    }
    if write_out(b"S").is_err() {
        return ExitCode::from(1);
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut b = [0u8; 1];
        while matches!(stdin.read(&mut b), Ok(1)) {
            if tx.send(b[0]).is_err() {
                return;
            }
        }
    });
    let deadline = std::time::Instant::now() + WAIT;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(b) => {
                let text = if b == SENTINEL {
                    "!".to_owned()
                } else {
                    format!("{b:02x}")
                };
                if write_out(text.as_bytes()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    ExitCode::SUCCESS
}

#[cfg(not(unix))]
fn echo_hex(_args: &[String]) -> ExitCode {
    eprintln!("child echo-hex: Unix only");
    ExitCode::from(2)
}

/// Appends a byte to the file every 50 ms for as long as it can.
fn heartbeat(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        return ExitCode::from(2);
    };
    let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) else {
        return ExitCode::from(2);
    };
    loop {
        if file.write_all(b".").is_err() || file.flush().is_err() {
            return ExitCode::from(1);
        }
        thread::sleep(Duration::from_millis(50));
    }
}
