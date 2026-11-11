//! The end-to-end cases E1-E9 (plan M9 step 8): the real [`Session`] on real PTY children, its
//! Updates mirrored into a termview `Grid`, assertions on the grid's text (and, for E8, on
//! offscreen PNGs).
//!
//! Timing bounds are at least 5x the macOS median measured in this milestone (see notes.md); a
//! timing failure carries its numbers and is never retried.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lune_pty::{Pty, PtyBackend, PtySize, PtyWriter, RawMode, SpawnOptions};
use lune_termview::{Attrs, Cell, Color, Grid, TermTheme, UnderlineStyle};
use lune_ui::dump::{read_rgba, target, write_png};
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::RawInput;
use lune_vt::services::options_service::OptionValue;
use lune_vt::{
    CellColor, CommandMarkKind, StandardExtensions, Terminal, UnderlineStyle as VtUnderline,
    Viewport, ViewportCell, ViewportCursor,
};
use luneterm::app::cursor_of;
use luneterm::links::{LinkKind, UriTable, scan_row};
use luneterm::render::{Content, Renderer};
use luneterm::session::{
    Command, CopyResult, Exit, Geometry, HostPalette, SelectCmd, Session, SessionConfig,
    SessionStats, UpdateModes, apply_update,
};
use luneterm::settings::Settings;
use luneterm::shell::HOST;
use luneterm::{fonts, spawn};

use super::child::{FLOOD_DONE, SENTINEL, flood_payload, hex_encode};
use super::rss;

/// What a case reports.
pub(crate) enum Status {
    /// Passed, with the numbers worth keeping.
    Ok(String),
    Skipped(String),
}

pub(crate) type CaseResult = Result<Status, String>;

pub(crate) struct Case {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) run: fn() -> CaseResult,
}

pub(crate) const CASES: &[Case] = &[
    Case {
        id: "E1",
        name: "round trip",
        run: e1_round_trip,
    },
    Case {
        id: "E2",
        name: "DSR",
        run: e2_dsr,
    },
    Case {
        id: "E3",
        name: "paste",
        run: e3_paste,
    },
    Case {
        id: "E4",
        name: "OSC 52",
        run: e4_osc52,
    },
    Case {
        id: "E5",
        name: "mode 2026",
        run: e5_sync,
    },
    Case {
        id: "E6",
        name: "floods and memory",
        run: e6_floods,
    },
    Case {
        id: "E6k",
        name: "key latency mid-flood",
        run: e6_key_latency,
    },
    Case {
        id: "E7",
        name: "exit and drop",
        run: e7_exit,
    },
    Case {
        id: "E8",
        name: "PNG",
        run: e8_png,
    },
    Case {
        id: "E9",
        name: "environment",
        run: e9_env,
    },
    Case {
        id: "E10",
        name: "tabs are isolated",
        run: e10_tabs,
    },
    Case {
        id: "E11",
        name: "selection and copy",
        run: e11_copy,
    },
    Case {
        id: "E12",
        name: "links",
        run: e12_links,
    },
    Case {
        id: "E13",
        name: "shell integration",
        run: e13_shell_integration,
    },
];

// ------------------------------------------------------------------------------- constants

const COLS: usize = 80;
const ROWS: usize = 24;
const SCROLLBACK: usize = 5000;
/// How long a wait for something that takes milliseconds may take.
const WAIT: Duration = Duration::from_secs(10);
/// The I3 bounds the floods assert (the same numbers the session's own tests use).
const INBOX_BOUND: usize = (1 << 20) + 64 * 1024;
const RESPONSES_BOUND: usize = 64 * 1024;
const INPUT_BOUND: usize = 1 << 20;
const PASTES_BOUND: usize = 2;
const EVENTS_BOUND: usize = 64 * 1024;

// ------------------------------------------------------------------------------- the rig

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn exe() -> PathBuf {
    std::env::current_exe().expect("the test binary's path")
}

fn geometry() -> Geometry {
    Geometry {
        cols: COLS,
        rows: ROWS,
        cell_px: (8.0, 17.0),
        pixel_size: (COLS as u32 * 8, ROWS as u32 * 17),
    }
}

fn config(clipboard_write: bool) -> SessionConfig {
    SessionConfig {
        geometry: geometry(),
        scrollback: SCROLLBACK,
        xtversion: spawn::xtversion(),
        clipboard_write,
        windows_pty: spawn::windows_pty(),
        palette: HostPalette::from_theme(&TermTheme::lunegit_one_dark()),
        focused: true,
    }
}

fn pty_size() -> PtySize {
    geometry().pty_size()
}

/// Spawns `program` with `args` on a real PTY.
fn spawn_pty(program: &Path, args: &[String]) -> Result<Box<dyn Pty>, String> {
    let options = SpawnOptions {
        program: Some(program.to_path_buf()),
        args: args.to_vec(),
        cwd: None,
        env: vec![("TERM".to_owned(), "xterm-256color".to_owned())],
    };
    lune_pty::PortablePtyBackend::new()
        .spawn(&options, pty_size())
        .map_err(|e| format!("spawn {}: {:?}", program.display(), e.kind()))
}

/// Spawns this binary as a `--child` of `mode`.
fn spawn_child(mode: &str, args: &[String]) -> Result<Box<dyn Pty>, String> {
    let mut all = vec!["--child".to_owned(), mode.to_owned()];
    all.extend_from_slice(args);
    spawn_pty(&exe(), &all)
}

fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// A PTY that records every byte it reads, for the reference replay of E8.
struct TeePty {
    inner: Box<dyn Pty>,
    rec: Arc<Mutex<Vec<u8>>>,
}

struct TeeReader {
    inner: Box<dyn Read + Send>,
    rec: Arc<Mutex<Vec<u8>>>,
}

impl Read for TeeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        lock(&self.rec).extend_from_slice(&buf[..n]);
        Ok(n)
    }
}

impl Pty for TeePty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.inner.write_all(data)
    }
    fn resize(&mut self, size: PtySize) -> io::Result<()> {
        self.inner.resize(size)
    }
    fn try_exit_code(&mut self) -> io::Result<Option<i32>> {
        self.inner.try_exit_code()
    }
    fn kill(&mut self) -> io::Result<()> {
        self.inner.kill()
    }
    fn take_reader(&mut self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(TeeReader {
            inner: self.inner.take_reader()?,
            rec: Arc::clone(&self.rec),
        }))
    }
    fn writer(&self) -> PtyWriter {
        self.inner.writer()
    }
    fn process_id(&self) -> Option<u32> {
        self.inner.process_id()
    }
    fn raw_mode(&self) -> RawMode {
        self.inner.raw_mode()
    }
    fn foreground_process(&self) -> Option<u32> {
        self.inner.foreground_process()
    }
}

/// The session's wake-up: a flag and a condition variable.
#[derive(Default)]
struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

/// What the UI side has seen so far.
struct Seen {
    updates: u64,
    clipboard: Vec<String>,
    exited: Option<Exit>,
    modes: UpdateModes,
    cursor: ViewportCursor,
    /// What `CopySelection` answered, in order.
    copied: Vec<CopyResult>,
    /// The OSC 8 ids and URIs the Updates carried.
    link_uris: Vec<(u32, String)>,
    /// The directories the shell reported (OSC 7 or OSC 9;9), in order.
    cwds: Vec<String>,
    /// The OSC 133 marks the Updates carried, in order.
    marks: Vec<CommandMarkKind>,
}

/// A running session and the UI side of it: the wake-up, the mirrored grid, what was seen.
struct Rig {
    session: Option<Session>,
    grid: Grid,
    wake: Arc<Wake>,
    seen: Seen,
}

impl Rig {
    fn start(pty: Box<dyn Pty>, cfg: SessionConfig) -> Result<Rig, String> {
        let wake = Arc::new(Wake::default());
        let w = Arc::clone(&wake);
        let session = Session::start(
            pty,
            cfg,
            Box::new(move || {
                *lock(&w.flag) = true;
                w.cv.notify_one();
            }),
        )
        .map_err(|e| format!("Session::start: {:?}", e.kind()))?;
        Ok(Rig {
            session: Some(session),
            grid: Grid::new(COLS, ROWS),
            wake,
            seen: Seen {
                updates: 0,
                clipboard: Vec::new(),
                exited: None,
                modes: UpdateModes::default(),
                cursor: ViewportCursor::default(),
                copied: Vec::new(),
                link_uris: Vec::new(),
                cwds: Vec::new(),
                marks: Vec::new(),
            },
        })
    }

    fn session(&self) -> &Session {
        self.session.as_ref().expect("the session is running")
    }

    fn send(&self, command: Command) {
        self.session().send(command);
    }

    fn type_text(&self, text: &str) {
        self.send(Command::Text(text.to_owned()));
    }

    fn stats(&self) -> SessionStats {
        self.session().stats()
    }

    /// Takes the outstanding Update, if any, and applies it. `true` if there was one.
    fn take(&mut self) -> bool {
        let Some(update) = self.session().take_update() else {
            return false;
        };
        apply_update(&mut self.grid, &update);
        self.seen.updates += 1;
        if let Some(text) = &update.clipboard_write {
            self.seen.clipboard.push(text.clone());
        }
        if update.exited.is_some() {
            self.seen.exited = update.exited;
        }
        self.seen.modes = update.modes;
        self.seen.cursor = update.cursor;
        if let Some(copied) = update.selection_text {
            self.seen.copied.push(copied);
        }
        self.seen.link_uris.extend(update.link_uris);
        self.seen.cwds.extend(update.cwd);
        self.seen.marks.extend(update.marks.iter().map(|m| m.kind));
        true
    }

    /// [`Rig::take`], waiting up to `max` for the wake-up first.
    fn pump(&mut self, max: Duration) -> bool {
        if self.take() {
            return true;
        }
        {
            let mut flag = lock(&self.wake.flag);
            if !*flag {
                flag = self
                    .wake
                    .cv
                    .wait_timeout(flag, max)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            *flag = false;
        }
        self.take()
    }

    /// Pumps for `d`.
    fn pump_for(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            self.pump(Duration::from_millis(5));
        }
    }

    /// Pumps until `pred` holds; the error names `what` and shows the screen.
    fn wait(
        &mut self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&Rig) -> bool,
    ) -> Result<Duration, String> {
        let t0 = Instant::now();
        loop {
            if pred(self) {
                return Ok(t0.elapsed());
            }
            if t0.elapsed() > timeout {
                return Err(format!(
                    "timed out after {} ms waiting for {what}; screen:\n{}",
                    t0.elapsed().as_millis(),
                    self.screen()
                ));
            }
            self.pump(Duration::from_millis(10));
        }
    }

    /// Waits for the Update that reports the child's exit, then takes what is left.
    fn wait_exit(&mut self, timeout: Duration) -> Result<Exit, String> {
        self.wait("the child's exit", timeout, |r| r.seen.exited.is_some())?;
        while self.pump(Duration::from_millis(30)) {}
        self.seen.exited.ok_or_else(|| "no exit".to_owned())
    }

    /// The grid's rows as text, trailing blanks trimmed.
    fn rows(&self) -> Vec<String> {
        grid_rows(&self.grid)
    }

    fn screen(&self) -> String {
        self.rows().join("\n")
    }

    /// All rows joined with nothing between: a long hex string wrapped over rows reads straight.
    fn flat(&self) -> String {
        self.rows().concat()
    }

    fn contains(&self, needle: &str) -> bool {
        self.rows().iter().any(|r| r.contains(needle))
    }

    /// What the `echo-hex` child printed after its `RAW:S` marker, once the marker is there.
    fn listing(&self) -> Option<String> {
        let flat = self.flat();
        flat.split_once("RAW:S").map(|(_, rest)| rest.to_owned())
    }

    /// Shuts the session down and waits for its threads.
    fn close(mut self) -> bool {
        self.session
            .take()
            .is_none_or(|s| s.close(Duration::from_secs(3)))
    }
}

fn grid_rows(grid: &Grid) -> Vec<String> {
    (0..grid.rows())
        .map(|y| {
            let mut s = String::new();
            for x in 0..grid.cols() {
                let Some(cell) = grid.get(y, x) else { continue };
                if cell.width == 0 {
                    continue;
                }
                if cell.text().is_empty() {
                    s.push(' ');
                } else {
                    s.push_str(cell.text());
                }
            }
            s.trim_end().to_owned()
        })
        .collect()
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn ok(detail: impl Into<String>) -> CaseResult {
    Ok(Status::Ok(detail.into()))
}

fn skipped(why: impl Into<String>) -> CaseResult {
    Ok(Status::Skipped(why.into()))
}

fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(msg()) }
}

/// An `echo-hex` session: raw tty, the `emit` arguments written, `RAW:S` on screen.
fn echo_session(emit: &[String], clipboard_write: bool) -> Result<Rig, String> {
    let pty = spawn_child("echo-hex", emit)?;
    let mut rig = Rig::start(pty, config(clipboard_write))?;
    rig.wait("the echo-hex child's RAW:S", WAIT, |r| {
        r.listing().is_some()
    })?;
    Ok(rig)
}

/// Types the sentinel and waits for the `!` it prints; returns the whole listing.
fn end_listing(rig: &mut Rig) -> Result<String, String> {
    rig.type_text(&char::from(SENTINEL).to_string());
    rig.wait("the sentinel's !", WAIT, |r| {
        r.listing().is_some_and(|l| l.contains('!'))
    })?;
    rig.listing().ok_or_else(|| "no listing".to_owned())
}

// ------------------------------------------------------------------------------- E1

/// What E1 leaves behind for E8.
struct RoundTrip {
    grid: Grid,
    cursor: ViewportCursor,
    recorded: Vec<u8>,
    typed_echo: Duration,
}

/// The shell round trip: a shell prints READY, reads a line, and answers it.
fn round_trip() -> Result<RoundTrip, String> {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let (program, args, line, wanted): (PathBuf, Vec<String>, &str, &str) = if cfg!(windows) {
        let comspec = std::env::var_os("ComSpec").ok_or("no ComSpec")?;
        (
            PathBuf::from(comspec),
            strs(&["/d", "/q", "/k", "echo READY"]),
            "echo hi\r",
            "hi",
        )
    } else {
        (
            PathBuf::from("/bin/sh"),
            strs(&["-c", r#"printf READY; read l; printf "got:%s\n" "$l""#]),
            "hi\r",
            "got:hi",
        )
    };
    let pty = TeePty {
        inner: spawn_pty(&program, &args)?,
        rec: Arc::clone(&recorded),
    };
    let mut rig = Rig::start(Box::new(pty), config(true))?;
    rig.wait("READY", WAIT, |r| r.contains("READY"))?;
    let sent = Instant::now();
    rig.type_text(line);
    // Windows: a row that is exactly `hi`, not the typed `echo hi`.
    let typed_echo = rig.wait("the answer", WAIT, |r| {
        if cfg!(windows) {
            r.rows().iter().any(|row| row == wanted)
        } else {
            r.contains(wanted)
        }
    })?;
    let _ = sent;
    if cfg!(windows) {
        // cmd /k stays; end it so the stream is complete.
        rig.type_text("exit\r");
    }
    rig.wait_exit(WAIT)?;
    let cursor = rig.seen.cursor;
    let grid = std::mem::replace(&mut rig.grid, Grid::new(1, 1));
    ensure(rig.close(), || "close timed out".to_owned())?;
    let recorded = lock(&recorded).clone();
    Ok(RoundTrip {
        grid,
        cursor,
        recorded,
        typed_echo,
    })
}

fn e1_round_trip() -> CaseResult {
    let rt = round_trip()?;
    ok(format!(
        "answer in {} ms, {} bytes of output",
        ms(rt.typed_echo),
        rt.recorded.len()
    ))
}

// ------------------------------------------------------------------------------- E2

fn unix_only() -> Option<CaseResult> {
    cfg!(unix)
        .then_some(())
        .map_or_else(|| Some(skipped("needs a Unix tty (stty)")), |()| None)
}

fn e2_dsr() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    // Cursor report: after "RAW:" the cursor is at row 1, column 5 (1-based).
    let mut rig = echo_session(&[hex_encode(b"\x1b[6n")], true)?;
    let expected = hex_encode(b"\x1b[1;5R");
    let took = rig.wait("the DSR reply", WAIT, |r| {
        r.listing().is_some_and(|l| l.starts_with(&expected))
    })?;
    let listing = end_listing(&mut rig)?;
    ensure(listing == format!("{expected}!"), || {
        format!("the child read {listing:?}, wanted {expected:?} then !")
    })?;
    ok(format!("reply after {} ms", ms(took)))
}

// ------------------------------------------------------------------------------- E3

const PASTED: &str = "a\x1b[201~b\r\nc\x07";

fn paste_listing(bracketed: bool) -> Result<String, String> {
    let emit: Vec<String> = if bracketed {
        vec![hex_encode(b"\x1b[?2004h")]
    } else {
        Vec::new()
    };
    let mut rig = echo_session(&emit, true)?;
    ensure(rig.seen.modes.bracketed_paste == bracketed, || {
        format!("bracketed_paste is {}", rig.seen.modes.bracketed_paste)
    })?;
    rig.send(Command::Paste(PASTED.to_owned()));
    let expected = if bracketed {
        // Markers inside the text removed, controls other than HT and CR dropped, line breaks CR.
        let mut e = hex_encode(b"\x1b[200~");
        e.push_str(&hex_encode(b"ab\rc"));
        e.push_str(&hex_encode(b"\x1b[201~"));
        e
    } else {
        // Only line breaks change.
        hex_encode(b"a\x1b[201~b\rc\x07")
    };
    rig.wait("the pasted bytes", WAIT, |r| {
        r.listing().is_some_and(|l| l.starts_with(&expected))
    })?;
    let listing = end_listing(&mut rig)?;
    ensure(listing == format!("{expected}!"), || {
        format!("the child read {listing:?}, wanted {expected:?} then !")
    })?;
    Ok(expected)
}

fn e3_paste() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    paste_listing(true)?;
    paste_listing(false)?;
    ok("bracketed and plain")
}

// ------------------------------------------------------------------------------- E4

fn osc52_set(clipboard_write: bool) -> Result<Vec<String>, String> {
    let emit = [hex_encode(b"\x1b]52;c;aGVsbG8=\x07DONE")];
    let pty = spawn_child("emit", &emit)?;
    let mut rig = Rig::start(pty, config(clipboard_write))?;
    rig.wait("DONE", WAIT, |r| r.contains("DONE"))?;
    // The Update that shows DONE carries everything before it.
    Ok(rig.seen.clipboard.clone())
}

fn e4_osc52() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let on = osc52_set(true)?;
    ensure(on == ["hello"], || {
        format!("with the setting on, Updates carried {} writes", on.len())
    })?;
    let off = osc52_set(false)?;
    ensure(off.is_empty(), || {
        format!("with the setting off, Updates carried {} writes", off.len())
    })?;
    // The query: the child gets nothing back.
    let mut rig = echo_session(&[hex_encode(b"\x1b]52;c;?\x07")], true)?;
    rig.pump_for(Duration::from_millis(300));
    let t0 = Instant::now();
    rig.type_text(&char::from(SENTINEL).to_string());
    rig.wait("the sentinel's !", Duration::from_millis(500), |r| {
        r.listing().is_some_and(|l| l.contains('!'))
    })?;
    let echoed = t0.elapsed();
    rig.pump_for(Duration::from_millis(300));
    let listing = rig.listing().unwrap_or_default();
    ensure(listing == "!", || {
        format!("after the query the child read {listing:?}")
    })?;
    ok(format!("sentinel round trip {} ms", ms(echoed)))
}

// ------------------------------------------------------------------------------- E5

/// How long after the first Update the held screen may show, from the child's pauses: GO, 100 ms,
/// set + AAAA, 300 ms, reset.
const SYNC_PAUSE_BEFORE_SET: u64 = 100;
const SYNC_HOLD: u64 = 300;

/// Time of the Update that shows `needle`, relative to the Update that showed `GO`.
fn time_to_show(
    emit: &[String],
    needle: &str,
    timeout: Duration,
    mode_must_be_reset: bool,
) -> Result<Duration, String> {
    let pty = spawn_child("emit", emit)?;
    let mut rig = Rig::start(pty, config(true))?;
    rig.wait("GO", WAIT, |r| r.contains("GO"))?;
    let go = Instant::now();
    ensure(!rig.contains(needle), || format!("{needle} showed with GO"))?;
    loop {
        rig.pump(Duration::from_millis(5));
        if rig.contains(needle) {
            ensure(
                !(mode_must_be_reset && rig.seen.modes.synchronized_output),
                || "the Update that showed the held text still had the mode set".to_owned(),
            )?;
            return Ok(go.elapsed());
        }
        if go.elapsed() > timeout {
            return Err(format!(
                "{needle} did not show within {} ms; screen:\n{}",
                ms(timeout),
                rig.screen()
            ));
        }
    }
}

fn e5_sync() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let go = hex_encode(b"GO");
    let pause = format!("@{SYNC_PAUSE_BEFORE_SET}");
    let set = hex_encode(b"\x1b[?2026hAAAA");
    // With a reset: nothing shows until the reset, then the screen does.
    let reset = [
        go.clone(),
        pause.clone(),
        set.clone(),
        format!("@{SYNC_HOLD}"),
        hex_encode(b"\x1b[?2026l"),
    ];
    let with_reset = time_to_show(&reset, "AAAA", Duration::from_secs(5), true)?;
    // The reset is SYNC_PAUSE_BEFORE_SET + SYNC_HOLD after GO was written; GO was seen a little
    // after that, so 3/4 of the hold is a bound a correct session cannot go under.
    let floor = Duration::from_millis(SYNC_PAUSE_BEFORE_SET + SYNC_HOLD * 3 / 4);
    ensure(with_reset >= floor, || {
        format!(
            "the held screen showed {} ms after GO, before the reset (at least {} ms)",
            ms(with_reset),
            ms(floor)
        )
    })?;
    // Without a reset: the hold times out after 1.0 s (measured from the set, which is
    // SYNC_PAUSE_BEFORE_SET after GO).
    let none = [go, pause, set];
    let timed_out = time_to_show(&none, "AAAA", Duration::from_secs(5), false)?;
    // The set's time is estimated as GO seen + the child's pause, and GO is seen a little after
    // it was written, so the estimate runs late: the floor allows 50 ms of that. The ceiling is
    // the contract's 1.4 s (measured on macOS: 1007-1022 ms).
    let after_set = timed_out.saturating_sub(Duration::from_millis(SYNC_PAUSE_BEFORE_SET));
    let (lo, hi) = (Duration::from_millis(950), Duration::from_millis(1400));
    ensure(after_set >= lo && after_set <= hi, || {
        format!(
            "with no reset the screen showed {} ms after the set (wanted {} to {})",
            ms(after_set),
            ms(lo),
            ms(hi)
        )
    })?;
    ok(format!(
        "with reset {} ms after GO; without {} ms after the set",
        ms(with_reset),
        ms(after_set)
    ))
}

// ------------------------------------------------------------------------------- E6

#[derive(Clone, Copy, Debug)]
enum Pace {
    /// Takes every Update as it is announced.
    EveryWake,
    /// Takes one every 100 ms.
    Every100ms,
    /// Takes none for 2 s, then every one.
    Stalled2s,
}

impl Pace {
    const ALL: [Pace; 3] = [Pace::EveryWake, Pace::Every100ms, Pace::Stalled2s];
}

/// Takes Updates at `pace` until `done` holds or `timeout` passes.
fn drive(
    rig: &mut Rig,
    pace: Pace,
    timeout: Duration,
    done: impl Fn(&Rig) -> bool,
) -> Result<Duration, String> {
    let t0 = Instant::now();
    let mut next_tick = t0;
    loop {
        match pace {
            Pace::EveryWake => {
                rig.pump(Duration::from_millis(5));
            }
            Pace::Every100ms => {
                std::thread::sleep(next_tick.saturating_duration_since(Instant::now()));
                next_tick += Duration::from_millis(100);
                rig.take();
            }
            Pace::Stalled2s => {
                if t0.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(10));
                } else {
                    rig.pump(Duration::from_millis(5));
                }
            }
        }
        if done(rig) {
            return Ok(t0.elapsed());
        }
        if t0.elapsed() > timeout {
            return Err(format!(
                "not done after {} ms ({pace:?}); stats {:?}",
                ms(t0.elapsed()),
                rig.stats()
            ));
        }
    }
}

fn check_bounds(stats: &SessionStats) -> Result<(), String> {
    ensure(stats.inbox_high_water <= INBOX_BOUND, || {
        format!(
            "inbox high water {} > {INBOX_BOUND}",
            stats.inbox_high_water
        )
    })?;
    ensure(stats.events_per_drain_high_water <= EVENTS_BOUND, || {
        format!(
            "events per drain {} > {EVENTS_BOUND}",
            stats.events_per_drain_high_water
        )
    })?;
    ensure(stats.responses_high_water <= RESPONSES_BOUND, || {
        format!(
            "pending responses {} > {RESPONSES_BOUND}",
            stats.responses_high_water
        )
    })?;
    ensure(stats.input_high_water <= INPUT_BOUND, || {
        format!("pending input {} > {INPUT_BOUND}", stats.input_high_water)
    })?;
    ensure(stats.pastes_high_water <= PASTES_BOUND, || {
        format!("queued pastes {} > {PASTES_BOUND}", stats.pastes_high_water)
    })
}

/// One flood: `bytes` of `kind` from the child, the UI taking Updates at `pace`.
fn flood_run(kind: &str, bytes: usize, pace: Pace) -> Result<String, String> {
    let pty = spawn_child("flood", &[bytes.to_string(), kind.to_owned()])?;
    let mut rig = Rig::start(pty, config(true))?;
    let base = if rss::SUPPORTED {
        Some(rss::resident_bytes()?)
    } else {
        None
    };
    let peak = rss::SUPPORTED.then(|| rss::Peak::start(Duration::from_millis(200)));
    let took = drive(&mut rig, pace, Duration::from_secs(180), |r| {
        r.seen.exited.is_some()
    })?;
    while rig.pump(Duration::from_millis(30)) {}
    let stats = rig.stats();
    check_bounds(&stats)?;
    let exit = rig.seen.exited.ok_or("no exit")?;
    ensure(exit.code == Some(0), || {
        format!("the flood child exited with {:?}", exit.code)
    })?;
    if kind != "dsr" {
        ensure(rig.contains(FLOOD_DONE), || {
            format!("{FLOOD_DONE} is not on the final screen:\n{}", rig.screen())
        })?;
    }
    // Backpressure loses nothing: the reader read at least what the child wrote. ConPTY
    // re-renders its output, so only Unix has that equality.
    let payload = flood_payload(bytes, kind) as u64;
    if cfg!(unix) {
        ensure(stats.bytes_read >= payload, || {
            format!("read {} bytes of a {payload} byte flood", stats.bytes_read)
        })?;
    }
    if kind == "plain" && cfg!(unix) {
        ensure(stats.reader_waits > 0, || {
            "the reader never had to wait for room".to_owned()
        })?;
    }
    let mut detail = format!(
        "{kind} {} MiB {pace:?}: {} ms, inbox {} KiB, waits {}, responses {} B (dropped {}), {} updates",
        bytes >> 20,
        ms(took),
        stats.inbox_high_water >> 10,
        stats.reader_waits,
        stats.responses_high_water,
        stats.responses_dropped,
        stats.updates_published
    );
    if let (Some(base), Some(peak)) = (base, peak) {
        let high = peak.finish()?;
        let growth = high.saturating_sub(base);
        let limit = (SCROLLBACK * COLS * 16 + (64 << 20)) as u64;
        ensure(growth <= limit, || {
            format!(
                "RSS grew {} MiB, limit {} MiB ({detail})",
                growth >> 20,
                limit >> 20
            )
        })?;
        detail.push_str(&format!(", RSS +{} MiB", growth >> 20));
    }
    ensure(rig.close(), || "close timed out".to_owned())?;
    Ok(detail)
}

/// A key typed mid-flood comes back on the pinned first row: 20 samples, p95 within 500 ms.
fn key_latency() -> Result<String, String> {
    // Far more than the 20 samples need: the case ends by dropping the session, which kills the
    // child.
    let bytes: usize = 4 << 30;
    let pty = spawn_child("flood", &[bytes.to_string(), "plain-keys".to_owned()])?;
    let mut rig = Rig::start(pty, config(true))?;
    let t0 = Instant::now();
    let mut latencies: Vec<Duration> = Vec::new();
    let mut waiting: Option<(usize, Instant)> = None;
    let mut last = t0;
    while latencies.len() < 20 {
        rig.pump(Duration::from_millis(2));
        if let Some((n, sent)) = waiting {
            let marker = format!("K{n:04}");
            if rig.rows().first().is_some_and(|r| r.contains(&marker)) {
                latencies.push(sent.elapsed());
                waiting = None;
                last = Instant::now();
            } else if sent.elapsed() > Duration::from_secs(5) {
                return Err(format!(
                    "key {n} did not come back within 5 s; stats {:?}; screen:\n{}",
                    rig.stats(),
                    rig.screen()
                ));
            }
        } else if t0.elapsed() > Duration::from_millis(600)
            && last.elapsed() > Duration::from_millis(100)
        {
            if rig.seen.exited.is_some() {
                return Err(format!(
                    "the flood ended after {} samples ({:?} ms); make it longer",
                    latencies.len(),
                    latencies.iter().map(|d| ms(*d)).collect::<Vec<_>>()
                ));
            }
            let n = latencies.len() + 1;
            rig.type_text(if cfg!(windows) { "x\r" } else { "x" });
            waiting = Some((n, Instant::now()));
        }
        ensure(t0.elapsed() < Duration::from_secs(120), || {
            "key latency run took over 120 s".to_owned()
        })?;
    }
    let mut sorted = latencies;
    sorted.sort();
    let (median, p95, max) = (sorted[10], sorted[18], sorted[19]);
    check_bounds(&rig.stats())?;
    // 500 ms with an optimised engine. The unoptimised one the test normally runs with parses
    // about 6 MB/s, so the 1 MiB inbox in front of the marker alone takes ~250 ms (the macOS
    // median): the debug limit is 5x that median.
    let limit = Duration::from_millis(if cfg!(debug_assertions) { 1250 } else { 500 });
    ensure(p95 <= limit, || {
        format!(
            "key latency p95 {} ms (median {}, max {}), limit {}",
            ms(p95),
            ms(median),
            ms(max),
            ms(limit)
        )
    })?;
    drop(rig);
    Ok(format!(
        "key latency median {} ms, p95 {} ms, max {} ms",
        ms(median),
        ms(p95),
        ms(max)
    ))
}

/// Flood sizes. The contract's are for an optimised build (256 MiB of text on Unix); the debug
/// build the test normally runs in parses about 6 MB/s, so it uses smaller floods to keep the case
/// under 30 s. `LUNETERM_E2E_FULL=1` forces the contract sizes.
fn flood_size(kind: &str) -> usize {
    let full =
        !cfg!(debug_assertions) || std::env::var("LUNETERM_E2E_FULL").is_ok_and(|v| v == "1");
    match (kind, full, cfg!(windows)) {
        ("plain", true, false) => 256 << 20,
        ("plain", true, true) => 32 << 20,
        ("plain", false, false) => 24 << 20,
        ("plain", false, true) => 16 << 20,
        ("bel", true, _) => 16 << 20,
        ("bel", false, _) => 8 << 20,
        _ => 4 << 20,
    }
}

fn e6_floods() -> CaseResult {
    if rss::SUPPORTED {
        let seen = rss::calibrate()?;
        println!(
            "    RSS instrument: 64 MiB allocation read as +{} MiB",
            seen >> 20
        );
    } else {
        println!("    RSS: no instrument on this OS, memory not measured");
    }
    let mut runs = 0;
    for kind in ["plain", "bel", "dsr"] {
        for pace in Pace::ALL {
            let line = flood_run(kind, flood_size(kind), pace)?;
            println!("    {line}");
            runs += 1;
        }
    }
    ok(format!("{runs} floods"))
}

fn e6_key_latency() -> CaseResult {
    let line = key_latency()?;
    ok(line)
}

// ------------------------------------------------------------------------------- E7

fn e7_exit() -> CaseResult {
    let (program, args) = if cfg!(windows) {
        let comspec = std::env::var_os("ComSpec").ok_or("no ComSpec")?;
        (PathBuf::from(comspec), strs(&["/d", "/c", "exit 3"]))
    } else {
        (exe(), strs(&["--child", "exit", "3"]))
    };
    let mut rig = Rig::start(spawn_pty(&program, &args)?, config(true))?;
    let exit = rig.wait_exit(WAIT)?;
    ensure(exit.code == Some(3), || {
        format!("exit code {:?}, wanted 3", exit.code)
    })?;
    ensure(rig.close(), || "close timed out".to_owned())?;

    // Dropping the session stops the child: its heartbeat file stops growing.
    let file = fresh_temp_file("heartbeat")?;
    let result = heartbeat_stops(&file);
    let _ = std::fs::remove_file(&file);
    let stopped = result?;
    ok(format!(
        "exit code 3; heartbeat stopped {} ms after the drop",
        ms(stopped)
    ))
}

/// A new empty file in the temp dir that did not exist before.
fn fresh_temp_file(tag: &str) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path =
        std::env::temp_dir().join(format!("luneterm-e2e-{tag}-{}-{nanos}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("create temp file: {:?}", e.kind()))?;
    Ok(path)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Starts the heartbeat child, drops the session, and returns how long the file took to stop
/// growing (it must within 2 s).
fn heartbeat_stops(file: &Path) -> Result<Duration, String> {
    let pty = spawn_child("heartbeat", &[file.display().to_string()])?;
    let rig = Rig::start(pty, config(true))?;
    // The instrument: the file grows while the child lives.
    let t0 = Instant::now();
    while file_len(file) < 5 {
        ensure(t0.elapsed() < WAIT, || {
            "the heartbeat child never wrote".to_owned()
        })?;
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    let before = file_len(file);
    ensure(before >= 5, || "no heartbeat".to_owned())?;
    let dropped = Instant::now();
    drop(rig);
    // Stopped: the length does not change over 300 ms.
    let mut last = file_len(file);
    let mut last_change = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let now = file_len(file);
        if now != last {
            last = now;
            last_change = Instant::now();
        } else if last_change.elapsed() >= Duration::from_millis(300) {
            let stopped = last_change.duration_since(dropped);
            ensure(stopped <= Duration::from_secs(2), || {
                format!("the heartbeat stopped {} ms after the drop", ms(stopped))
            })?;
            return Ok(stopped);
        }
        ensure(dropped.elapsed() < Duration::from_secs(5), || {
            "the heartbeat was still going 5 s after the drop".to_owned()
        })?;
    }
}

// ------------------------------------------------------------------------------- E8

/// An independent conversion of the engine's cell (the session has its own; this one is the
/// oracle's).
fn reference_cell(c: &ViewportCell) -> Cell {
    let color = |c: CellColor| match c {
        CellColor::Default => Color::Default,
        CellColor::Palette(i) => Color::Palette(i),
        CellColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    };
    let mut attrs = Attrs::NONE;
    for (on, bit) in [
        (c.bold, Attrs::BOLD),
        (c.dim, Attrs::DIM),
        (c.italic, Attrs::ITALIC),
        (c.blink, Attrs::BLINK),
        (c.inverse, Attrs::INVERSE),
        (c.invisible, Attrs::INVISIBLE),
        (c.strikethrough, Attrs::STRIKETHROUGH),
        (c.overline, Attrs::OVERLINE),
    ] {
        if on {
            attrs.insert(bit);
        }
    }
    let mut cell = Cell::default();
    cell.width = c.width;
    cell.fg = color(c.fg);
    cell.bg = color(c.bg);
    cell.attrs = attrs;
    cell.underline = match c.underline {
        VtUnderline::None => UnderlineStyle::None,
        VtUnderline::Single => UnderlineStyle::Single,
        VtUnderline::Double => UnderlineStyle::Double,
        VtUnderline::Curly => UnderlineStyle::Curly,
        VtUnderline::Dotted => UnderlineStyle::Dotted,
        VtUnderline::Dashed => UnderlineStyle::Dashed,
    };
    cell.underline_color = color(c.underline_color);
    cell.link = c.link_id;
    cell.set_text(&c.text);
    cell
}

/// The screen a fresh engine shows after the same bytes, as a grid.
fn reference_grid(recorded: &[u8]) -> Result<(Grid, ViewportCursor), String> {
    let mut options = vec![
        OptionValue::Cols(COLS),
        OptionValue::Rows(ROWS),
        OptionValue::Scrollback(SCROLLBACK),
    ];
    if let Some(pty) = spawn::windows_pty() {
        options.push(OptionValue::WindowsPty(Some(pty)));
    }
    let mut terminal = Terminal::new(options);
    terminal
        .install_standard_extensions(&StandardExtensions {
            xtversion: Some(spawn::xtversion()),
            vs16: true,
            clipboard_write: true,
        })
        .map_err(|e| e.to_string())?;
    terminal.write(recorded);
    let vp: Viewport = terminal.viewport();
    let mut grid = Grid::new(vp.cols, vp.rows);
    for (y, row) in vp.cells.iter().enumerate() {
        let dst = grid.row_mut(y).ok_or("reference row")?;
        for (d, s) in dst.iter_mut().zip(row) {
            *d = reference_cell(s);
        }
    }
    Ok((grid, vp.cursor))
}

const PNG_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const PNG_WINDOW: (f32, f32) = (1040.0, 660.0);

/// Draws `grid` through the app's renderer (a warm-up frame, then the captured one) and returns
/// the pixels.
fn draw_png(r: &mut Renderer, grid: &mut Grid, cursor: &ViewportCursor) -> Result<Vec<u8>, String> {
    let size = (PNG_WINDOW.0 as u32, PNG_WINDOW.1 as u32);
    let texture = target(&r.gpu.device, PNG_FORMAT, size);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let input = RawInput {
        size: PNG_WINDOW,
        scale: 1.0,
        time: Duration::ZERO,
        focused: true,
        events: Vec::new(),
    };
    grid.mark_all_dirty();
    for _ in 0..2 {
        let content = Content {
            title: None,
            tabs: &[],
            active_tab: None,
            modal: None,
            maximized: false,
            grid,
            selection: &[],
            hover_link: 0,
            link_hand: false,
            link_tip: None,
            toasts: None,
            cursor: cursor_of(cursor, COLS),
            blink_on: true,
            scroll: None,
            chrome: HOST,
            ime: false,
            preedit: None,
            clipboard_write: None,
            clipboard_read: false,
            page: None,
            menu: None,
            busy: None,
            busy_tone: lune_ui::widgets::tone::Tone::Neutral,
            grid_alpha: 1.0,
            fading: Vec::new(),
        };
        r.draw(&input, content, &view, size)?;
        r.gpu.end_frame(&mut r.text);
    }
    read_rgba(&r.gpu.device, &r.gpu.queue, &texture)
}

fn png_dir() -> PathBuf {
    // <target dir>/e2e; CARGO_TARGET_TMPDIR is <target dir>/tmp and honours CARGO_TARGET_DIR.
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    tmp.parent().unwrap_or(tmp).join("e2e")
}

fn require_gpu() -> bool {
    std::env::var("LUNE_REQUIRE_GPU").is_ok_and(|v| v == "1")
}

fn e8_png() -> CaseResult {
    let mut rt = round_trip()?;
    let gpu = match pollster::block_on(Gpu::new(gpu::instance(None), None)) {
        Ok(gpu) => gpu,
        Err(e) if require_gpu() => {
            return Err(format!("LUNE_REQUIRE_GPU=1 but no usable adapter: {e}"));
        }
        Err(e) => return skipped(format!("no wgpu adapter: {e}")),
    };
    let (reference, ref_cursor) = reference_grid(&rt.recorded)?;
    let mut reference = reference;
    let mut renderer = Renderer::new(
        gpu,
        fonts::bundled_text_cache(),
        PNG_FORMAT,
        &Settings::default().clamped(),
        true,
    );
    let size = (PNG_WINDOW.0 as u32, PNG_WINDOW.1 as u32);
    let mirror_px = draw_png(&mut renderer, &mut rt.grid, &rt.cursor)?;
    let reference_px = draw_png(&mut renderer, &mut reference, &ref_cursor)?;
    let dir = png_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create {}: {:?}", dir.display(), e.kind()))?;
    write_png(&dir.join("e1.png"), size, &mirror_px)?;
    write_png(&dir.join("e1-reference.png"), size, &reference_px)?;
    ensure(mirror_px == reference_px, || {
        let differing = mirror_px
            .chunks_exact(4)
            .zip(reference_px.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        format!(
            "the session's grid and the engine's viewport render differently: {differing} of {} pixels (files in {})",
            mirror_px.len() / 4,
            dir.display()
        )
    })?;
    // The comparison can see a difference: one more character on the reference's screen changes
    // the image.
    let mut altered = reference.clone();
    if let Some(cell) = altered.row_mut(ROWS - 1).and_then(|row| row.first_mut()) {
        cell.set_text("Z");
    }
    let altered_px = draw_png(&mut renderer, &mut altered, &ref_cursor)?;
    ensure(altered_px != reference_px, || {
        "a changed cell did not change the image: the comparison is blind".to_owned()
    })?;
    // The instrument saw something: the image is not one flat colour.
    let first = &mirror_px[..4];
    ensure(mirror_px.chunks_exact(4).any(|p| p != first), || {
        "the rendered image is a single colour".to_owned()
    })?;
    ok(format!(
        "{}x{} identical, in {}",
        size.0,
        size.1,
        dir.display()
    ))
}

// ------------------------------------------------------------------------------- E9

fn e9_env() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let settings = Settings {
        shell: exe().display().to_string(),
        shell_args: "--child env3".to_owned(),
        ..Settings::default()
    };
    let (pty, program) = spawn::launch(&lune_pty::PortablePtyBackend::new(), &settings, pty_size())
        .map_err(|f| format!("launch: {:?} {}", f.kind, f.program))?;
    let mut rig = Rig::start(pty, config(true))?;
    rig.wait("the child's line", WAIT, |r| {
        r.rows().iter().any(|row| row.contains("Luneterm"))
    })?;
    let wanted = "xterm-256color truecolor Luneterm";
    let row = rig.rows().into_iter().next().unwrap_or_default();
    ensure(row == wanted, || {
        format!("the child printed {row:?}, wanted {wanted:?} (via {program})")
    })?;
    ok("TERM, COLORTERM and TERM_PROGRAM as set")
}

// ------------------------------------------------------------------------------- E10

/// A shell that names itself, then answers each line it reads with `<name>-got:<line>`.
fn named_shell(name: &str) -> Result<Rig, String> {
    let script =
        format!(r#"printf '{name}-READY\n'; while read l; do printf '{name}-got:%s\n' "$l"; done"#);
    let pty = spawn_pty(Path::new("/bin/sh"), &strs(&["-c", &script]))?;
    let mut rig = Rig::start(pty, config(true))?;
    let ready = format!("{name}-READY");
    rig.wait(&format!("{name}'s READY"), WAIT, |r| r.contains(&ready))?;
    Ok(rig)
}

/// Pumps `rigs` (every tab's Updates are taken, as the app's pump does) until `pred` holds.
fn wait_all(
    rigs: &mut [&mut Rig],
    what: &str,
    pred: impl Fn(&[&mut Rig]) -> bool,
) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        if pred(rigs) {
            return Ok(());
        }
        ensure(t0.elapsed() < WAIT, || {
            format!(
                "timed out waiting for {what}; screens:\n{}",
                rigs.iter()
                    .map(|r| r.screen())
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            )
        })?;
        for r in rigs.iter_mut() {
            r.pump(Duration::from_millis(5));
        }
    }
}

/// Two real sessions side by side (two tabs): what one is sent and prints never reaches the
/// other; closing one leaves the other running; a flood in a hidden tab (visible = false, as the
/// app sets it) keeps the other tab's keys answered and the hidden one's queues within the I3
/// bounds.
fn e10_tabs() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let mut a = named_shell("ALPHA")?;
    let mut b = named_shell("BRAVO")?;

    // Input to A reaches A only.
    a.type_text("one\r");
    wait_all(&mut [&mut a, &mut b], "ALPHA's answer", |r| {
        r[0].contains("ALPHA-got:one")
    })?;
    let mut both = [&mut a, &mut b];
    for r in both.iter_mut() {
        r.pump_for(Duration::from_millis(300));
    }
    ensure(!b.contains("one") && !b.contains("ALPHA"), || {
        format!("ALPHA's line reached BRAVO:\n{}", b.screen())
    })?;
    // And B's reaches B only.
    b.type_text("two\r");
    wait_all(&mut [&mut a, &mut b], "BRAVO's answer", |r| {
        r[1].contains("BRAVO-got:two")
    })?;
    let mut both = [&mut a, &mut b];
    for r in both.iter_mut() {
        r.pump_for(Duration::from_millis(300));
    }
    ensure(!a.contains("two") && !a.contains("BRAVO"), || {
        format!("BRAVO's line reached ALPHA:\n{}", a.screen())
    })?;

    // Close A: B goes on.
    ensure(a.close(), || "closing ALPHA timed out".to_owned())?;
    b.type_text("three\r");
    b.wait("BRAVO after ALPHA closed", WAIT, |r| {
        r.contains("BRAVO-got:three")
    })?;

    // A flood in a hidden tab: C floods while hidden, B keeps answering.
    let bytes: usize = 4 << 30;
    let pty = spawn_child("flood", &[bytes.to_string(), "plain".to_owned()])?;
    let mut c = Rig::start(pty, config(true))?;
    c.session().set_visible(false);
    c.pump_for(Duration::from_millis(300));
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    for i in 0..10 {
        let line = format!("k{i}");
        let sent = Instant::now();
        b.type_text(&format!("{line}\r"));
        let want = format!("BRAVO-got:{line}");
        wait_all(&mut [&mut b, &mut c], "BRAVO's answer mid-flood", |r| {
            r[0].contains(&want)
        })?;
        worst = worst.max(sent.elapsed());
    }
    let stats = c.stats();
    check_bounds(&stats)?;
    ensure(stats.bytes_read > 1 << 20, || {
        format!("the flood barely ran: {} bytes read", stats.bytes_read)
    })?;
    ensure(c.close(), || {
        "closing the hidden flood timed out".to_owned()
    })?;
    ensure(b.close(), || "closing BRAVO timed out".to_owned())?;
    ok(format!(
        "isolated both ways; B outlived A; 10 keys in {} ms (worst {} ms) beside a hidden flood (inbox {} KiB)",
        ms(started.elapsed()),
        ms(worst),
        stats.inbox_high_water >> 10
    ))
}

// ------------------------------------------------------------------------------- E11

fn click(rig: &Rig, row: usize, col: usize, clicks: u8) {
    rig.send(Command::Select(SelectCmd::Start {
        cell: (row, col),
        top: 0,
        clicks,
        extend: false,
        column: false,
    }));
}

/// Asks for a copy and waits for its answer; the text.
fn copy_text(rig: &mut Rig) -> Result<String, String> {
    let before = rig.seen.copied.len();
    rig.send(Command::CopySelection);
    rig.wait("the copy's answer", WAIT, |r| r.seen.copied.len() > before)?;
    match rig.seen.copied.last() {
        Some(CopyResult::Text(t)) => Ok(t.clone()),
        other => Err(format!("copy answered {other:?}")),
    }
}

/// Selection and copy over a real PTY child: a word, a line, a drag across lines, a wrapped line
/// copied without its soft break, and a copy with nothing selected answering nothing.
fn e11_copy() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let long = "0123456789".repeat(10); // 100 chars: wraps at 80 columns
    let script = format!(r#"printf 'hello world\r\nsecond line\r\n%s\r\n' '{long}'; read l"#);
    let pty = spawn_pty(Path::new("/bin/sh"), &strs(&["-c", &script]))?;
    let mut rig = Rig::start(pty, config(true))?;
    rig.wait("the printed lines", WAIT, |r| r.contains("789"))?;

    // Nothing selected: a copy answers nothing.
    rig.send(Command::CopySelection);
    rig.pump_for(Duration::from_millis(300));
    ensure(rig.seen.copied.is_empty(), || {
        format!("a copy with no selection answered {:?}", rig.seen.copied)
    })?;

    // A double click selects the word under it.
    click(&rig, 0, 7, 2);
    let word = copy_text(&mut rig)?;
    ensure(word == "world", || format!("double click copied {word:?}"))?;

    // A triple click selects the line.
    click(&rig, 1, 3, 3);
    let line = copy_text(&mut rig)?;
    ensure(line == "second line", || {
        format!("triple click copied {line:?}")
    })?;

    // A drag across two lines.
    click(&rig, 0, 6, 1);
    rig.send(Command::Select(SelectCmd::Drag {
        cell: (1, 6),
        top: 0,
        scroll: 0,
    }));
    rig.send(Command::Select(SelectCmd::End));
    let drag = copy_text(&mut rig)?;
    ensure(drag == "world\nsecond", || {
        format!("a drag copied {drag:?}")
    })?;

    // The wrapped line comes whole, with no break where it wrapped.
    click(&rig, 2, 5, 3);
    let wrapped = copy_text(&mut rig)?;
    ensure(wrapped == long, || {
        format!("a wrapped line copied {} chars: {wrapped:?}", wrapped.len())
    })?;
    ensure(rig.close(), || "close timed out".to_owned())?;
    ok("word, line, drag and wrapped line copied from a real shell; no selection answers nothing")
}

// ------------------------------------------------------------------------------- E12

/// Links over a real PTY child: an OSC 8 hyperlink (its URI arrives in an Update and its cells
/// carry the id) and a bare URL in the same row, found by the scanner on the mirror; a hostile OSC
/// 8 URI is carried but is not a link.
fn e12_links() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let script = r#"printf '\033]8;;https://example.com/osc\033\\click\033]8;;\033\\ and https://example.org/bare ok\r\n\033]8;;file:///etc/passwd\033\\evil\033]8;;\033\\ http://user@host/x\r\n'; read l"#;
    let pty = spawn_pty(Path::new("/bin/sh"), &strs(&["-c", script]))?;
    let mut rig = Rig::start(pty, config(true))?;
    rig.wait("the printed rows", WAIT, |r| r.contains("evil"))?;
    ensure(
        rig.seen
            .link_uris
            .iter()
            .any(|(_, u)| u == "https://example.com/osc"),
        || {
            format!(
                "the OSC 8 URI did not reach the Update: {:?}",
                rig.seen.link_uris
            )
        },
    )?;
    let uris: UriTable = rig.seen.link_uris.iter().cloned().collect();
    let scan = |row: usize| {
        let mut out = Vec::new();
        scan_row(rig.grid.row(row).expect("row"), row, &uris, &mut out);
        out
    };
    let first = scan(0);
    let kinds: Vec<_> = first
        .iter()
        .map(|s| (s.uri.as_str(), matches!(s.kind, LinkKind::Osc8(_))))
        .collect();
    ensure(
        kinds
            == [
                ("https://example.com/osc", true),
                ("https://example.org/bare", false),
            ],
        || format!("row 0 spans: {first:?}"),
    )?;
    // `click` is columns 0..=4.
    ensure((first[0].start, first[0].end) == (0, 4), || {
        format!("OSC 8 columns {first:?}")
    })?;
    // The hostile ones: a file: hyperlink and userinfo in plain text give nothing to click.
    let second = scan(1);
    ensure(second.is_empty(), || {
        format!("row 1 should have no link: {second:?}")
    })?;
    ensure(rig.close(), || "close timed out".to_owned())?;
    ok("an OSC 8 link and a bare URL found on a real session's rows; file: and userinfo refused")
}

// ------------------------------------------------------------------------------- E13

/// A directory this run creates (it must not exist): the user's real home and config directory
/// are never touched.
fn scratch_dir(tag: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("luneterm-e13-{}-{tag}", std::process::id()));
    ensure(!dir.exists(), || {
        format!("{} already exists", dir.display())
    })?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create scratch: {:?}", e.kind()))?;
    // The shell reports the real path (macOS's temp dir is behind a symlink).
    std::fs::canonicalize(&dir).map_err(|e| format!("canonicalize: {:?}", e.kind()))
}

/// The path of `name` on `PATH`, or `None`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// One real shell started through the integration, with a scratch home (so none of the user's own
/// rc files, history or config is read or written), a scratch config directory for the scripts,
/// and a scratch working directory. It must report its directory and the command marks.
fn shell_reports(shell: &Path, name: &str) -> Result<String, String> {
    let home = scratch_dir(&format!("{name}-home"))?;
    let cfg = scratch_dir(&format!("{name}-cfg"))?;
    let sub = home.join("sub");
    std::fs::create_dir(&sub).map_err(|e| format!("create sub: {:?}", e.kind()))?;
    // A minimal rc file: a short prompt and no history, nothing else.
    let rc = if name == "zsh" { ".zshrc" } else { ".bashrc" };
    let text = if name == "zsh" {
        "PROMPT='%# '\nunsetopt BEEP\n"
    } else {
        "PS1='$ '\n"
    };
    std::fs::write(home.join(rc), text).map_err(|e| format!("write rc: {:?}", e.kind()))?;

    let settings = Settings {
        shell: shell.display().to_string(),
        ..Settings::default()
    };
    let home_text = home.display().to_string();
    let mut env = spawn::env(&settings, &|k| std::env::var(k).ok());
    env.push(("HOME".to_owned(), home_text.clone()));
    let var = |k: &str| match k {
        "HOME" => luneterm::shell_integration::Var::Set(home_text.clone()),
        _ => luneterm::shell_integration::Var::Unset,
    };
    let mut install = || luneterm::shell_integration::install(&cfg).ok();
    let list = spawn::spawn_list(
        &settings,
        spawn::Os::HOST,
        &env,
        Some(&home),
        &var,
        &mut install,
    );
    ensure(
        list.len() == 1 && (list[0].env.len() > env.len() || !list[0].args.is_empty()),
        || format!("{name}: integration did not change the spawn"),
    )?;
    let (pty, _) = spawn::first_spawned(&list, |o| {
        lune_pty::PortablePtyBackend::new().spawn(o, pty_size())
    })
    .map_err(|f| format!("{name}: spawn {:?}", f.kind))?;
    let mut rig = Rig::start(pty, config(true))?;
    let wanted = home.display().to_string();
    // The first prompt: the directory, then D and A.
    rig.wait(&format!("{name}'s first directory report"), WAIT, |r| {
        r.seen.cwds.last().is_some_and(|c| c == &wanted)
            && r.seen.marks.contains(&CommandMarkKind::PromptStart)
    })?;
    // A command: C at its start, D and A at its end, the directory after a cd.
    rig.type_text("echo integrated-ok\r");
    rig.wait(&format!("{name}'s output mark"), WAIT, |r| {
        r.seen.marks.contains(&CommandMarkKind::OutputStart) && r.contains("integrated-ok")
    })?;
    rig.type_text("cd sub\r");
    let want_sub = sub.display().to_string();
    rig.wait(&format!("{name}'s directory after cd"), WAIT, |r| {
        r.seen.cwds.last().is_some_and(|c| c == &want_sub)
    })?;
    let kinds = rig.seen.marks.clone();
    for k in [
        CommandMarkKind::OutputStart,
        CommandMarkKind::CommandFinished,
        CommandMarkKind::PromptStart,
    ] {
        ensure(kinds.contains(&k), || {
            format!("{name}: no {k:?} mark in {kinds:?}")
        })?;
    }
    ensure(rig.close(), || format!("{name}: close timed out"))?;
    // Only our own scratch directories are removed.
    for d in [home, cfg] {
        std::fs::remove_dir_all(&d).map_err(|e| format!("remove scratch: {:?}", e.kind()))?;
    }
    Ok(format!(
        "{name}: cwd and C/D/A marks seen ({} marks)",
        kinds.len()
    ))
}

/// A real zsh and a real bash, started through the integration scripts, report the directory
/// (OSC 7) and the command marks (OSC 133) the app reads; skipped, with the reason printed, for a
/// shell that is not installed.
fn e13_shell_integration() -> CaseResult {
    if let Some(s) = unix_only() {
        return s;
    }
    let mut done = Vec::new();
    let mut absent = Vec::new();
    for name in ["zsh", "bash"] {
        match find_on_path(name) {
            Some(path) => done.push(shell_reports(&path, name)?),
            None => absent.push(name),
        }
    }
    if done.is_empty() {
        return skipped(format!("no zsh or bash on PATH ({absent:?})"));
    }
    if !absent.is_empty() {
        eprintln!("E13: not run, shell absent: {absent:?}");
    }
    ok(done.join("; "))
}
