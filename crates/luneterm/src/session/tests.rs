//! Session tests (plan M9 step 3, T1-T10).
//!
//! Single-threaded tests step `EngineCore` with a fake clock (`Rig`); threaded tests run a real
//! `Session` on the fake `Pty`. Every threaded wait is a loop with a deadline at least 10x the
//! expected time, never a bare sleep.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use lune_termview::{Attrs, Cell, Color, Grid, TermTheme, UnderlineStyle};
use lune_vt::services::options_service::{OptionValue, WindowOptions};
use lune_vt::{CellColor, UnderlineStyle as VtUnderline, Viewport, ViewportCell};

use super::engine::{
    Clock, EngineCore, HIDDEN_INTERVAL, SLICE, STEP_MAX, STEP_MIN, SYNC_TIMEOUT, TickInput,
    build_terminal,
};
use super::fake_pty::{FakePty, Gate, Probe, Script};
use super::threads::{
    INBOX_HIGH, Inbox, READ_SIZE, RESPONSE_CAP, Stats, WRITE_PIECE, WriteKind, WriteQueue, lock,
};
use super::*;

// ---------------------------------------------------------------- helpers

/// A clock the test moves; optionally charges per parsed byte.
#[derive(Clone, Default)]
struct FakeClock {
    nanos: Arc<AtomicU64>,
    per_byte_ns: Arc<AtomicU64>,
}

impl FakeClock {
    fn advance(&self, d: Duration) {
        self.nanos.fetch_add(d.as_nanos() as u64, Ordering::SeqCst);
    }

    fn set_cost(&self, per_byte: Duration) {
        self.per_byte_ns
            .store(per_byte.as_nanos() as u64, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
    }

    fn charge(&self, bytes: usize) {
        let cost = self.per_byte_ns.load(Ordering::SeqCst);
        self.nanos
            .fetch_add(cost.saturating_mul(bytes as u64), Ordering::SeqCst);
    }
}

fn config(cols: usize, rows: usize) -> SessionConfig {
    SessionConfig {
        geometry: Geometry {
            cols,
            rows,
            cell_px: (10.0, 20.0),
            pixel_size: (cols as u32 * 20, rows as u32 * 40),
        },
        scrollback: 1000,
        xtversion: "Luneterm(0.1.0)".to_owned(),
        clipboard_write: true,
        windows_pty: None,
        palette: HostPalette::from_theme(&TermTheme::lunegit_one_dark()),
        focused: true,
    }
}

fn geometry(cols: usize, rows: usize) -> Geometry {
    config(cols, rows).geometry
}

/// `EngineCore` with no threads.
struct Rig {
    core: EngineCore,
    out: Arc<WriteQueue>,
    probe: Arc<Probe>,
    clock: FakeClock,
    _tx: Sender<Script>,
}

impl Rig {
    fn new(cfg: &SessionConfig) -> Rig {
        let (pty, probe, tx) = FakePty::scripted(Gate::Open);
        let stats = Arc::new(Stats::default());
        let out = Arc::new(WriteQueue::new(Arc::clone(&stats)));
        let clock = FakeClock::default();
        let terminal = build_terminal(cfg).expect("terminal");
        let core = EngineCore::new(
            terminal,
            pty,
            cfg,
            Box::new(clock.clone()),
            Arc::clone(&out),
            stats,
        );
        Rig {
            core,
            out,
            probe,
            clock,
            _tx: tx,
        }
    }

    fn tick_with(&mut self, input: TickInput) -> Option<Update> {
        self.core.tick(input, &|| false)
    }

    /// Parse `bytes` and publish if anything changed.
    fn feed(&mut self, bytes: &[u8]) -> Option<Update> {
        self.tick_with(TickInput {
            chunks: vec![bytes.to_vec()],
            ui_ready: true,
            visible: true,
            ..TickInput::default()
        })
    }

    fn command(&mut self, c: Command) -> Option<Update> {
        self.tick_with(TickInput {
            commands: vec![c],
            ui_ready: true,
            visible: true,
            ..TickInput::default()
        })
    }

    fn idle(&mut self) -> Option<Update> {
        self.tick_with(TickInput {
            ui_ready: true,
            visible: true,
            ..TickInput::default()
        })
    }

    /// Everything queued for the writer, by kind.
    fn written(&self) -> Vec<(WriteKind, Vec<u8>)> {
        self.out
            .take_all()
            .into_iter()
            .map(|i| (i.kind, i.bytes))
            .collect()
    }

    fn written_bytes(&self) -> Vec<u8> {
        self.written().into_iter().flat_map(|(_, b)| b).collect()
    }
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `lo..=hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() % (hi - lo + 1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range(0, items.len() - 1)]
    }
}

fn row_text(grid: &Grid, y: usize) -> String {
    grid.row(y)
        .map(|r| r.iter().map(Cell::text).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- T1 mirror differential (I7)

fn color_eq(g: Color, v: CellColor) -> bool {
    match (g, v) {
        (Color::Default, CellColor::Default) => true,
        (Color::Palette(a), CellColor::Palette(b)) => a == b,
        (Color::Rgb(a, b, c), CellColor::Rgb(x, y, z)) => (a, b, c) == (x, y, z),
        _ => false,
    }
}

fn underline_eq(g: UnderlineStyle, v: VtUnderline) -> bool {
    matches!(
        (g, v),
        (UnderlineStyle::None, VtUnderline::None)
            | (UnderlineStyle::Single, VtUnderline::Single)
            | (UnderlineStyle::Double, VtUnderline::Double)
            | (UnderlineStyle::Curly, VtUnderline::Curly)
            | (UnderlineStyle::Dotted, VtUnderline::Dotted)
            | (UnderlineStyle::Dashed, VtUnderline::Dashed)
    )
}

/// Field by field, independent of the conversion under test.
fn cell_eq(g: &Cell, v: &ViewportCell) -> bool {
    let a = |bit| g.attrs.contains(bit);
    g.text() == v.text
        && g.width == v.width
        && color_eq(g.fg, v.fg)
        && color_eq(g.bg, v.bg)
        && a(Attrs::BOLD) == v.bold
        && a(Attrs::DIM) == v.dim
        && a(Attrs::ITALIC) == v.italic
        && a(Attrs::BLINK) == v.blink
        && a(Attrs::INVERSE) == v.inverse
        && a(Attrs::INVISIBLE) == v.invisible
        && a(Attrs::STRIKETHROUGH) == v.strikethrough
        && a(Attrs::OVERLINE) == v.overline
        && underline_eq(g.underline, v.underline)
        && color_eq(g.underline_color, v.underline_color)
        && g.link == v.link_id
}

/// The UI's state after applying `last` equals the viewport `last` was built from. Messages
/// name positions only, never cell text.
fn check_mirror(grid: &Grid, last: &Update, vp: &Viewport, ctx: &str) {
    assert_eq!(
        (grid.cols(), grid.rows()),
        (vp.cols, vp.rows),
        "{ctx}: grid size"
    );
    for y in 0..vp.rows {
        for x in 0..vp.cols {
            let ok = grid.get(y, x).is_some_and(|g| cell_eq(g, &vp.cells[y][x]));
            assert!(ok, "{ctx}: cell row {y} col {x} differs");
        }
    }
    assert!(last.cursor == vp.cursor, "{ctx}: cursor");
    assert_eq!(
        (
            last.cols,
            last.rows,
            last.viewport_top,
            last.base_y,
            last.total_lines,
            last.is_at_bottom,
            last.alt_screen
        ),
        (
            vp.cols,
            vp.rows,
            vp.viewport_top,
            vp.base_y,
            vp.total_lines,
            vp.is_at_bottom,
            vp.alt_screen
        ),
        "{ctx}: scroll fields"
    );
}

const WIDE: &[&str] = &["中", "文", "字", "한", "😀", "🎉"];
const COMBINING: &[&str] = &["e\u{301}", "a\u{308}\u{323}", "o\u{302}", "\u{301}"];
const VS16: &[&str] = &[
    "\u{2764}\u{FE0F}",
    "\u{263A}\u{FE0F}",
    "#\u{FE0F}\u{20E3}",
    "\u{2702}\u{FE0F}",
];

fn sgr(rng: &mut Rng) -> String {
    let n = rng.range(0, 255);
    let (r, g, b) = (rng.range(0, 255), rng.range(0, 255), rng.range(0, 255));
    match rng.range(0, 14) {
        0 => "\x1b[0m".to_owned(),
        1 => format!(
            "\x1b[{}m",
            rng.pick(&[
                1, 2, 3, 4, 5, 7, 8, 9, 53, 21, 22, 23, 24, 25, 27, 28, 29, 55
            ])
        ),
        2 => format!("\x1b[4:{}m", rng.range(0, 5)),
        3 => format!("\x1b[{}m", rng.range(30, 37)),
        4 => format!("\x1b[{}m", rng.range(90, 97)),
        5 => format!("\x1b[{}m", rng.range(40, 47)),
        6 => format!("\x1b[{}m", rng.range(100, 107)),
        7 => format!("\x1b[38;5;{n}m"),
        8 => format!("\x1b[48;5;{n}m"),
        9 => format!("\x1b[38;2;{r};{g};{b}m"),
        10 => format!("\x1b[48;2;{r};{g};{b}m"),
        11 => format!("\x1b[58;5;{n}m"),
        12 => format!("\x1b[58:2::{r}:{g}:{b}m"),
        13 => "\x1b[59m".to_owned(),
        _ => "\x1b[39;49m".to_owned(),
    }
}

/// One piece of a seeded stream: text, wide, combining, VS16, SGR, OSC 8, cursor and edit
/// controls, scroll regions, 1049, REP, RIS, 2026.
fn piece(rng: &mut Rng, out: &mut Vec<u8>) {
    let s: String = match rng.range(0, 29) {
        0..=5 => {
            let len = if rng.chance(10) {
                rng.range(50, 400)
            } else {
                rng.range(1, 20)
            };
            (0..len)
                .map(|_| *rng.pick(&['a', 'b', 'Z', '0', ' ', '-', 'q', '~']))
                .collect()
        }
        6 => rng.pick(WIDE).to_string(),
        7 => rng.pick(COMBINING).to_string(),
        8 => rng.pick(VS16).to_string(),
        9..=11 => sgr(rng),
        12 => format!(
            "\x1b]8;id={};https://e.x/{}\x1b\\",
            rng.range(0, 3),
            rng.range(0, 9)
        ),
        13 => "\x1b]8;;\x1b\\".to_owned(),
        14 => format!("\x1b[{};{}H", rng.range(1, 60), rng.range(1, 320)),
        15 => rng.pick(&["\r", "\n", "\r\n", "\x08", "\t"]).to_string(),
        16 => "\r\n".to_owned(),
        17 => format!("\x1b[{}J", rng.range(0, 3)),
        18 => format!("\x1b[{}K", rng.range(0, 2)),
        19 => format!("\x1b[{}X", rng.range(1, 20)),
        20 => format!("\x1b[{}L", rng.range(1, 5)),
        21 => format!("\x1b[{}M", rng.range(1, 5)),
        22 => format!("\x1b[{}S", rng.range(1, 5)),
        23 => format!("\x1b[{}T", rng.range(1, 5)),
        24 => {
            if rng.chance(30) {
                "\x1b[r".to_owned()
            } else {
                let t = rng.range(1, 20);
                format!("\x1b[{};{}r", t, t + rng.range(1, 30))
            }
        }
        25 => rng.pick(&["\x1b[?1049h", "\x1b[?1049l"]).to_string(),
        26 => format!(
            "{}\x1b[{}b",
            rng.pick(&["x", "中", "e\u{301}", "\u{2764}\u{FE0F}"]),
            rng.range(1, 300)
        ),
        27 => {
            if rng.chance(15) {
                "\x1bc".to_owned()
            } else {
                "\n\n\n".to_owned()
            }
        }
        _ => rng.pick(&["\x1b[?2026h", "\x1b[?2026l"]).to_string(),
    };
    out.extend_from_slice(s.as_bytes());
}

/// T1: after every Update the UI grid equals `viewport()` at the state it was built from.
/// Axes varied per seed: chunking (1 B, 7 B, random up to 64 KiB, by seed % 3); random commands
/// (scroll lines/pages/top/bottom, resize 2..300 x 1..100 with odd and even widths, focus);
/// random publish points (the UI takes the outstanding Update at random); the fake clock jumps so
/// 2026 timeouts fire; scrollback 0, 5, 50 and 1000 (0 reaches lune-vt's lines-missing state).
/// `LUNETERM_MIRROR_SEEDS` (default 200), `LUNETERM_MIRROR_SEED` replays one.
#[test]
fn session_mirror_differential() {
    let seeds: Vec<u64> = match std::env::var("LUNETERM_MIRROR_SEED") {
        Ok(s) => vec![s.parse().expect("LUNETERM_MIRROR_SEED")],
        Err(_) => {
            let n = std::env::var("LUNETERM_MIRROR_SEEDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(200u64);
            // 2985: the scrollback-0 state with lines missing below the viewport (damage
            // cannot be trusted there), which the first 200 seeds do not reach.
            (0..n).chain([2985]).collect()
        }
    };
    for seed in seeds {
        mirror_seed(seed);
    }
}

fn mirror_seed(seed: u64) {
    let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x5EED);
    let mut cfg = config(rng.range(2, 120), rng.range(1, 40));
    cfg.scrollback = *rng.pick(&[0usize, 5, 50, 1000]);
    let mut rig = Rig::new(&cfg);
    let chunking = seed % 3;
    // Byte-at-a-time streams publish far more often; keep their runtime in line.
    let target = match chunking {
        0 => rng.range(200, 1200),
        1 => rng.range(400, 3000),
        _ => rng.range(500, 6000),
    };
    let mut stream = Vec::new();
    while stream.len() < target {
        piece(&mut rng, &mut stream);
    }
    let mut grid = Grid::new(1, 1);
    let mut outstanding: Option<(Update, Viewport)> = None;
    let mut taken = 0usize;
    let mut pos = 0usize;

    let take = |outstanding: &mut Option<(Update, Viewport)>, grid: &mut Grid, n: &mut usize| {
        if let Some((u, vp)) = outstanding.take() {
            apply_update(grid, &u);
            *n += 1;
            check_mirror(grid, &u, &vp, &format!("seed {seed} update {}", u.seq));
        }
    };

    let mut step = 0usize;
    loop {
        step += 1;
        let mut input = TickInput {
            visible: true,
            ..TickInput::default()
        };
        let roll = rng.range(0, 99);
        if roll < 12 {
            let c = match rng.range(0, 9) {
                0 | 1 => Command::Scroll(Scroll::Lines(rng.range(0, 60) as i64 - 30)),
                2 => Command::Scroll(Scroll::Pages(rng.range(0, 6) as i32 - 3)),
                3 => Command::Scroll(Scroll::Top),
                4 => Command::Scroll(Scroll::Bottom),
                5 | 6 => Command::Focus(rng.chance(50)),
                _ => Command::Resize(geometry(rng.range(2, 300), rng.range(1, 100))),
            };
            input.commands.push(c);
        } else if roll < 20 {
            take(&mut outstanding, &mut grid, &mut taken);
        } else if roll < 25 {
            rig.clock
                .advance(Duration::from_millis(rng.range(0, 600) as u64));
        } else if pos < stream.len() {
            let len = match chunking {
                0 => 1,
                1 => 7,
                _ => rng.range(1, 64 * 1024),
            };
            let end = (pos + len).min(stream.len());
            input.chunks.push(stream[pos..end].to_vec());
            pos = end;
        } else if outstanding.is_none() {
            // Drained: let any 2026 hold time out, then stop when nothing is left to publish.
            rig.clock.advance(SYNC_TIMEOUT * 2);
            input.ui_ready = true;
            match rig.tick_with(input) {
                Some(u) => {
                    let vp = rig.core.terminal.viewport();
                    outstanding = Some((u, vp));
                    continue;
                }
                None => break,
            }
        }
        input.ui_ready = outstanding.is_none();
        if let Some(u) = rig.tick_with(input) {
            assert!(
                outstanding.is_none(),
                "seed {seed}: published while one was outstanding"
            );
            let vp = rig.core.terminal.viewport();
            outstanding = Some((u, vp));
        }
        assert!(step < 1_000_000, "seed {seed}: did not settle");
    }
    take(&mut outstanding, &mut grid, &mut taken);
    // The final grid is the final screen.
    let vp = rig.core.terminal.viewport();
    for y in 0..vp.rows {
        for x in 0..vp.cols {
            let ok = grid.get(y, x).is_some_and(|g| cell_eq(g, &vp.cells[y][x]));
            assert!(ok, "seed {seed}: final cell row {y} col {x} differs");
        }
    }
    assert!(taken > 0, "seed {seed}: no Update");
}

/// The conversion keeps every field (one cell per field, against the field-wise comparison).
#[test]
fn session_cell_conversion_keeps_every_field() {
    let mut rig = Rig::new(&config(20, 2));
    rig.feed(
        b"\x1b]8;;https://e.x\x1b\\\x1b[1;2;3;5;7;8;9;53;4:3;38;5;9;48;2;1;2;3;58;5;4mX\x1b[0m\x1b]8;;\x1b\\\xe4\xb8\xad",
    )
    .expect("update");
    let vp = rig.core.terminal.viewport();
    let mut grid = Grid::new(1, 1);
    // Rebuild from a full update.
    let u = rig.command(Command::Scroll(Scroll::Bottom)).expect("full");
    apply_update(&mut grid, &u);
    assert!(u.full);
    for x in 0..3 {
        assert!(
            cell_eq(grid.get(0, x).expect("cell"), &vp.cells[0][x]),
            "col {x}"
        );
    }
    let c = grid.get(0, 0).expect("cell");
    assert!(c.attrs.contains(Attrs::BOLD | Attrs::OVERLINE) && c.link != 0);
    assert_eq!(c.underline, UnderlineStyle::Curly);
    assert_eq!(grid.get(0, 1).expect("wide").width, 2);
    assert_eq!(grid.get(0, 2).expect("spacer").width, 0);
}

// ---------------------------------------------------------------- T2 slices (I4)

#[test]
fn session_slices_end_at_4ms_plus_one_write() {
    let mut rig = Rig::new(&config(80, 24));
    rig.clock.set_cost(Duration::from_micros(1));
    let chunk = vec![b'a'; 1 << 20];
    let mut first = true;
    let mut slices = 0;
    loop {
        rig.tick_with(TickInput {
            chunks: if first {
                vec![chunk.clone()]
            } else {
                Vec::new()
            },
            ..TickInput::default()
        });
        first = false;
        slices += 1;
        let log = rig.core.slice_log;
        // The slice was under 4 ms when its last write started, so it ends at <= 4 ms plus one
        // sub-write.
        assert!(
            log.elapsed - log.last_write < SLICE,
            "slice {slices}: {:?} before the last write",
            log.elapsed - log.last_write
        );
        assert!(log.elapsed <= SLICE + log.last_write);
        if rig.core.wants_chunks() {
            break;
        }
        assert!(slices < 10_000);
    }
    assert!(slices > 100, "1 MiB at 1 us/byte takes many 4 ms slices");
}

#[test]
fn session_pending_command_ends_slice_after_current_write() {
    let mut rig = Rig::new(&config(80, 24));
    rig.clock.set_cost(Duration::from_nanos(1));
    let input = TickInput {
        chunks: vec![vec![b'a'; 1 << 20]],
        ..TickInput::default()
    };
    rig.core.tick(input, &|| true);
    assert_eq!(rig.core.slice_log.writes, 1);
    assert!(!rig.core.wants_chunks(), "the rest stays pending");
}

#[test]
fn session_step_adapts_between_256_and_64k() {
    let mut rig = Rig::new(&config(80, 24));
    rig.clock.set_cost(Duration::from_micros(10));
    let mut first = true;
    for _ in 0..200 {
        if rig.core.step() == STEP_MIN {
            break;
        }
        rig.tick_with(TickInput {
            chunks: if first {
                vec![vec![b'a'; 1 << 20]]
            } else {
                Vec::new()
            },
            ..TickInput::default()
        });
        first = false;
    }
    assert_eq!(rig.core.step(), STEP_MIN);
    rig.clock.set_cost(Duration::from_nanos(1));
    for _ in 0..200 {
        if rig.core.step() == STEP_MAX {
            break;
        }
        rig.tick_with(TickInput {
            chunks: if rig.core.wants_chunks() {
                vec![vec![b'b'; 1 << 20]]
            } else {
                Vec::new()
            },
            ..TickInput::default()
        });
    }
    assert_eq!(rig.core.step(), STEP_MAX);
}

// ---------------------------------------------------------------- T6 2026 (I6, D10)

fn rig_published() -> Rig {
    let mut rig = Rig::new(&config(40, 5));
    rig.idle().expect("first update");
    rig
}

#[test]
fn session_2026_holds_until_reset() {
    let mut rig = rig_published();
    assert!(rig.feed(b"\x1b[?2026habc").is_none());
    rig.clock.advance(Duration::from_millis(500));
    assert!(rig.feed(b"def").is_none());
    assert!(rig.idle().is_none());
    let u = rig
        .feed(b"\x1b[?2026l")
        .expect("one Update after the reset");
    assert!(!u.modes.synchronized_output);
    let mut grid = Grid::new(40, 5);
    apply_update(&mut grid, &u);
    assert!(row_text(&grid, 0).starts_with("abcdef"));
    assert!(rig.idle().is_none());
}

#[test]
fn session_2026_times_out_at_1000ms() {
    let mut rig = rig_published();
    assert!(rig.feed(b"\x1b[?2026habc").is_none());
    let since = rig.core.now();
    assert_eq!(
        rig.core
            .next_deadline(rig.core.now(), true, true, rig.core.unpublished()),
        Some(since + SYNC_TIMEOUT)
    );
    rig.clock.advance(Duration::from_millis(999));
    assert!(rig.idle().is_none());
    rig.clock.advance(Duration::from_millis(1));
    let u = rig.idle().expect("Update at 1000 ms");
    assert!(
        u.modes.synchronized_output,
        "the engine's mode is untouched"
    );
    // Timed out: publishes normally while the mode stays set.
    assert!(rig.feed(b"x").is_some());
    // Reset, then set again: the timer restarts.
    assert!(rig.feed(b"\x1b[?2026l").is_some());
    assert!(rig.feed(b"\x1b[?2026hzz").is_none());
    rig.clock.advance(Duration::from_millis(999));
    assert!(rig.idle().is_none());
    rig.clock.advance(Duration::from_millis(1));
    assert!(rig.idle().is_some());
}

#[test]
fn session_2026_set_and_reset_inside_one_slice() {
    let mut rig = rig_published();
    let u = rig
        .tick_with(TickInput {
            chunks: vec![
                b"\x1b[?2026h".to_vec(),
                b"one".to_vec(),
                b"\rfinal".to_vec(),
                b"\x1b[?2026l".to_vec(),
            ],
            ui_ready: true,
            visible: true,
            ..TickInput::default()
        })
        .expect("one Update");
    let mut grid = Grid::new(40, 5);
    apply_update(&mut grid, &u);
    assert!(row_text(&grid, 0).starts_with("final"));
    assert!(rig.idle().is_none());
}

#[test]
fn session_hidden_publishes_at_most_every_100ms() {
    let mut rig = rig_published();
    let hidden = |chunks: Vec<Vec<u8>>| TickInput {
        chunks,
        ui_ready: true,
        visible: false,
        ..TickInput::default()
    };
    assert!(rig.tick_with(hidden(vec![b"a".to_vec()])).is_none());
    rig.clock
        .advance(HIDDEN_INTERVAL - Duration::from_millis(1));
    assert!(rig.tick_with(hidden(vec![b"b".to_vec()])).is_none());
    rig.clock.advance(Duration::from_millis(1));
    assert!(rig.tick_with(hidden(Vec::new())).is_some());
    assert!(rig.tick_with(hidden(vec![b"c".to_vec()])).is_none());
}

#[test]
fn session_publishes_only_after_the_ui_took_the_last() {
    let mut rig = rig_published();
    let not_ready = TickInput {
        chunks: vec![b"abc".to_vec()],
        visible: true,
        ..TickInput::default()
    };
    assert!(rig.tick_with(not_ready).is_none());
    assert!(rig.idle().is_some());
    assert!(rig.idle().is_none(), "nothing changed");
}

// ---------------------------------------------------------------- T7 host answers (I8)

#[test]
fn session_color_reports_follow_the_session_palette() {
    let mut rig = rig_published();
    rig.feed(b"\x1b]4;1;?\x1b\\");
    assert_eq!(rig.written_bytes(), b"\x1b]4;1;rgb:e0e0/6c6c/7575\x1b\\");
    rig.feed(b"\x1b]10;?\x1b\\");
    assert_eq!(rig.written_bytes(), b"\x1b]10;rgb:abab/b2b2/bfbf\x1b\\");
    rig.feed(b"\x1b]11;?\x1b\\");
    assert_eq!(rig.written_bytes(), b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\");

    let u = rig.feed(b"\x1b]4;1;rgb:12/34/56\x1b\\").expect("update");
    let palette = u.palette.expect("palette changed");
    assert_eq!(palette.ansi[1], [0x12, 0x34, 0x56]);
    assert!(u.full);
    assert!(rig.written().is_empty(), "a set sends nothing");
    rig.feed(b"\x1b]4;1;?\x1b\\");
    assert_eq!(rig.written_bytes(), b"\x1b]4;1;rgb:1212/3434/5656\x1b\\");
    let u = rig.feed(b"\x1b]104;1\x1b\\").expect("update");
    assert!(u.palette.is_some());
    rig.feed(b"\x1b]4;1;?\x1b\\");
    assert_eq!(rig.written_bytes(), b"\x1b]4;1;rgb:e0e0/6c6c/7575\x1b\\");
    // Restore-all resets the ansi colours only.
    rig.feed(b"\x1b]4;2;rgb:00/00/00\x1b\\\x1b]10;rgb:01/02/03\x1b\\\x1b]104\x1b\\");
    rig.feed(b"\x1b]4;2;?\x1b\\\x1b]10;?\x1b\\");
    assert_eq!(
        rig.written_bytes(),
        b"\x1b]4;2;rgb:9898/c3c3/7979\x1b\\\x1b]10;rgb:0101/0202/0303\x1b\\"
    );
}

#[test]
fn session_window_reports_use_grid_area_and_cell_size() {
    let mut rig = rig_published();
    rig.command(Command::Resize(geometry(80, 24)));
    rig.core
        .terminal
        .set_option(OptionValue::WindowOptions(WindowOptions {
            get_win_size_pixels: true,
            get_cell_size_pixels: true,
            ..WindowOptions::default()
        }))
        .expect("window options");
    rig.written();
    rig.feed(b"\x1b[14t");
    assert_eq!(rig.written_bytes(), b"\x1b[4;480;800t");
    rig.feed(b"\x1b[16t");
    assert_eq!(rig.written_bytes(), b"\x1b[6;20;10t");
}

#[test]
fn session_focus_reports_use_the_stored_focus() {
    let mut rig = rig_published();
    rig.feed(b"\x1b[?1004h");
    assert_eq!(
        rig.written(),
        vec![(WriteKind::Response, b"\x1b[I".to_vec())]
    );
    rig.command(Command::Focus(false));
    assert_eq!(rig.written(), vec![(WriteKind::Input, b"\x1b[O".to_vec())]);
    rig.feed(b"\x1b[?1004l\x1b[?1004h");
    assert_eq!(rig.written_bytes(), b"\x1b[O");
    rig.feed(b"\x1b[?1004l");
    rig.command(Command::Focus(true));
    assert!(rig.written().is_empty(), "no report while 1004 is off");
}

#[test]
fn session_xtversion_names_luneterm() {
    let mut rig = rig_published();
    rig.feed(b"\x1b[>0q");
    let reply = String::from_utf8(rig.written_bytes()).expect("utf8");
    assert!(reply.contains("Luneterm("), "XTVERSION reply");
}

#[test]
fn session_responses_keep_their_order() {
    let mut rig = rig_published();
    rig.feed(b"\x1b[6n\x1b]10;?\x1b\\\x1b[c\x1b]11;?\x1b\\\x1b[5n");
    let got = rig.written();
    assert!(got.iter().all(|(k, _)| *k == WriteKind::Response));
    let bytes: Vec<u8> = got.into_iter().flat_map(|(_, b)| b).collect();
    let s = String::from_utf8(bytes).expect("utf8");
    let order = ["\x1b[1;1R", "]10;", "\x1b[?", "]11;", "\x1b[0n"]
        .map(|p| s.find(p).expect("reply present"));
    assert!(order.windows(2).all(|w| w[0] < w[1]), "in stream order");
}

// ---------------------------------------------------------------- input and scroll (I15)

#[test]
fn session_text_and_wheel_arrows_are_typed_input() {
    let mut rig = rig_published();
    rig.command(Command::Text("héllo".to_owned()));
    assert_eq!(
        rig.written(),
        vec![(WriteKind::Input, "héllo".as_bytes().to_vec())]
    );
    rig.command(Command::WheelArrows { up: true });
    rig.command(Command::WheelArrows { up: false });
    assert_eq!(rig.written_bytes(), b"\x1b[A\x1b[B");
    rig.feed(b"\x1b[?1h");
    rig.command(Command::WheelArrows { up: true });
    rig.command(Command::WheelArrows { up: false });
    assert_eq!(rig.written_bytes(), b"\x1bOA\x1bOB");
}

#[test]
fn session_scrolls_force_full_updates() {
    let mut rig = Rig::new(&config(10, 4));
    rig.idle();
    let mut lines = Vec::new();
    for i in 0..30 {
        lines.extend_from_slice(format!("line{i}\r\n").as_bytes());
    }
    rig.feed(&lines);
    let u = rig
        .command(Command::Scroll(Scroll::Pages(-1)))
        .expect("update");
    assert!(u.full && u.rows_changed.len() == 4);
    assert_eq!(u.viewport_top, u.base_y - 3, "a page is rows - 1 lines");
    // Scrolled back: output below still gives full updates.
    let u = rig.feed(b"more").expect("update");
    assert!(u.full);
    let u = rig.command(Command::Scroll(Scroll::Top)).expect("update");
    assert_eq!(u.viewport_top, 0);
    let u = rig
        .command(Command::Scroll(Scroll::Bottom))
        .expect("update");
    assert!(u.full && u.is_at_bottom);
    let u = rig.feed(b"x").expect("update");
    assert!(!u.full, "at the bottom, damage rows only");
}

#[test]
fn session_events_reach_the_update() {
    let mut rig = rig_published();
    let u = rig
        .feed(b"\x07\x07\x1b]0;hi\x07\x1b]52;c;YWJj\x07\x1b]52;c;eHl6\x07\x1b]7;file:///tmp\x07\x1b]133;A\x07\x1b]133;D;3\x07")
        .expect("update");
    assert_eq!(u.bells, 2);
    assert_eq!(u.title.as_deref(), Some("hi"));
    assert_eq!(u.clipboard_write.as_deref(), Some("xyz"), "last wins");
    assert!(u.cwd.is_some());
    assert_eq!(u.marks.len(), 2);
    assert_eq!(u.marks[1].exit_code, Some(3));
    let u = rig.feed(b"z").expect("update");
    assert!(u.title.is_none() && u.bells == 0 && u.clipboard_write.is_none());
    let mut many = Vec::new();
    for _ in 0..100 {
        many.extend_from_slice(b"\x1b]133;B\x07");
    }
    many.extend_from_slice(b"\x1b]133;C\x07");
    let u = rig.feed(&many).expect("update");
    assert_eq!(u.marks.len(), MARKS_CAP);
    assert_eq!(
        u.marks.last().map(|m| m.kind),
        Some(lune_vt::CommandMarkKind::OutputStart),
        "the oldest are dropped"
    );
    // The engine gate.
    rig.command(Command::SetClipboardWrite(false));
    let u = rig.feed(b"\x1b]52;c;YWJj\x07q").expect("update");
    assert!(u.clipboard_write.is_none());
}

// ---------------------------------------------------------------- T8 caps (I10, I11)

#[test]
fn session_caps_size_and_scrollback() {
    let mut cfg = config(0, 0);
    cfg.scrollback = 1_000_000;
    let mut rig = Rig::new(&cfg);
    assert_eq!((rig.core.terminal.cols(), rig.core.terminal.rows()), (2, 1));
    assert_eq!(rig.core.terminal.options().scrollback, MAX_SCROLLBACK);

    let mut g = geometry(10_000, 5_000);
    g.pixel_size = (100_000, 70_000);
    rig.core.apply_command(Command::Resize(g));
    assert_eq!(
        (rig.core.terminal.cols(), rig.core.terminal.rows()),
        (MAX_COLS, MAX_ROWS)
    );
    let sizes = lock(&rig.probe.resizes).clone();
    assert_eq!(
        sizes,
        vec![PtySize {
            columns: 4096,
            rows: 2048,
            pixel_width: u16::MAX,
            pixel_height: u16::MAX
        }]
    );
    rig.core.apply_command(Command::Resize(geometry(0, 0)));
    assert_eq!((rig.core.terminal.cols(), rig.core.terminal.rows()), (2, 1));
    assert_eq!(
        lock(&rig.probe.resizes).last().map(|s| (s.columns, s.rows)),
        Some((2, 1))
    );
}

#[test]
fn session_queued_resizes_collapse() {
    let mut rig = rig_published();
    let shared = Shared::new(Arc::new(Stats::default()));
    for c in [10, 20, 30, 40, 51] {
        shared.send(Command::Resize(geometry(c, 7)));
    }
    shared.send(Command::Text("k".to_owned()));
    let (resize, commands) = {
        let mut inbox: std::sync::MutexGuard<'_, Inbox> = lock(&shared.inbox);
        (inbox.resize.take(), inbox.commands.drain(..).collect())
    };
    let u = rig
        .tick_with(TickInput {
            resize,
            commands,
            ui_ready: true,
            visible: true,
            ..TickInput::default()
        })
        .expect("update");
    let sizes = lock(&rig.probe.resizes).clone();
    assert_eq!(sizes.len(), 1, "five queued resizes apply once");
    assert_eq!((sizes[0].columns, sizes[0].rows), (51, 7));
    assert!(u.full && u.cols == 51 && u.rows == 7);
}

// ---------------------------------------------------------------- T10 paste (I14), single-threaded

#[test]
fn session_bracketed_paste_is_sanitized() {
    let mut rig = rig_published();
    rig.feed(b"\x1b[?2004h");
    rig.command(Command::Paste(
        "a\x1b[201~b\nc\x07d\te\u{9b}f\r\ng\x1b[20\x1b[201~0~".to_owned(),
    ));
    let got = rig.written();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, WriteKind::Paste);
    let b = &got[0].1;
    assert!(b.starts_with(b"\x1b[200~") && b.ends_with(b"\x1b[201~"));
    let inner = &b[6..b.len() - 6];
    assert!(
        inner.iter().all(|&c| c >= 0x20 || c == b'\t' || c == b'\r'),
        "only text, HT and CR between the markers"
    );
    let s = std::str::from_utf8(inner).expect("utf8");
    assert!(!s.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c)));
}

#[test]
fn session_plain_paste_and_empty_paste() {
    let mut rig = rig_published();
    rig.command(Command::Paste("a\nb\r\nc".to_owned()));
    assert_eq!(rig.written(), vec![(WriteKind::Paste, b"a\rb\rc".to_vec())]);
    rig.command(Command::Paste(String::new()));
    assert!(rig.written().is_empty());
}

#[test]
fn session_write_caps_drop_and_count() {
    let stats = Arc::new(Stats::default());
    let q = WriteQueue::new(Arc::clone(&stats));
    for _ in 0..(RESPONSE_CAP / 8 + 10) {
        q.push(WriteKind::Response, vec![b'r'; 8]);
    }
    for _ in 0..3 {
        q.push(WriteKind::Paste, vec![b'p'; 4]);
    }
    q.push(WriteKind::Input, vec![b'i'; super::threads::INPUT_CAP]);
    q.push(WriteKind::Input, vec![b'i']);
    let s = stats.snapshot();
    assert_eq!(s.responses_high_water, RESPONSE_CAP);
    assert_eq!(s.responses_dropped, 10);
    assert_eq!((s.pastes_high_water, s.pastes_dropped), (2, 1));
    assert_eq!(s.input_dropped, 1);
}

// ---------------------------------------------------------------- threaded

struct Woken {
    calls: Arc<AtomicU64>,
    rx: mpsc::Receiver<()>,
}

fn waker() -> (Box<dyn Fn() + Send>, Woken) {
    let (tx, rx) = mpsc::channel();
    let calls = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&calls);
    let waker = Box::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(());
    });
    (waker, Woken { calls, rx })
}

/// Takes Updates until one reports the exit; returns it.
fn take_until_exit(session: &Session, woken: &Woken, timeout: Duration) -> Option<Update> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let _ = woken.rx.recv_timeout(Duration::from_millis(10));
        while let Some(u) = session.take_update() {
            if u.exited.is_some() {
                return Some(u);
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug)]
enum Pace {
    EveryWake,
    Every50ms,
    Stall500ms,
}

/// T3: a child that writes 64 MiB as fast as it can. Axes: consumer pace (every wake, every
/// 50 ms, stalled 500 ms then every wake) x content (plain, BEL flood, `ESC[6n` flood). The inbox
/// never holds more than 1 MiB + one read, the reader waits, nothing is lost (all 64 MiB read
/// before the exit is reported).
#[test]
fn session_threaded_backpressure() {
    const TOTAL: u64 = 64 << 20;
    let contents: [(&str, &[u8]); 3] = [
        ("plain", b"the quick brown fox 0123456789\r\n"),
        ("bel", b"\x07"),
        ("dsr", b"\x1b[6n"),
    ];
    for (name, pattern) in contents {
        for pace in [Pace::EveryWake, Pace::Every50ms, Pace::Stall500ms] {
            let (pty, probe) = FakePty::flood(pattern, TOTAL, Gate::Discard);
            let (w, woken) = waker();
            let session = Session::start(pty, config(80, 24), w).expect("start");
            let start = Instant::now();
            let deadline = start + Duration::from_secs(300);
            let mut exited = false;
            while Instant::now() < deadline && !exited {
                match pace {
                    Pace::EveryWake => {
                        let _ = woken.rx.recv_timeout(Duration::from_millis(10));
                    }
                    Pace::Every50ms => thread::sleep(Duration::from_millis(50)),
                    Pace::Stall500ms => {
                        if start.elapsed() < Duration::from_millis(500) {
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        let _ = woken.rx.recv_timeout(Duration::from_millis(10));
                    }
                }
                while let Some(u) = session.take_update() {
                    exited |= u.exited.is_some();
                }
            }
            let s = session.stats();
            assert!(exited, "{name}/{pace:?}: no exit within the deadline");
            assert_eq!(s.bytes_read, TOTAL, "{name}/{pace:?}");
            assert_eq!(probe.output_read.load(Ordering::SeqCst), TOTAL);
            assert!(
                s.inbox_high_water <= INBOX_HIGH + READ_SIZE,
                "{name}/{pace:?}: inbox high water {}",
                s.inbox_high_water
            );
            assert!(
                s.reader_waits > 0,
                "{name}/{pace:?}: the reader never waited"
            );
            assert!(s.responses_high_water <= RESPONSE_CAP);
            assert!(s.events_per_drain_high_water <= 64 * 1024 + 16);
            assert!(session.close(Duration::from_secs(10)));
        }
    }
}

/// Splits a writer stream into DSR replies and everything else.
fn split_replies(bytes: &[u8]) -> (usize, Vec<u8>) {
    let mut replies = 0;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"\x1b[") {
            if let Some(end) = bytes[i..].iter().position(|&b| b == b'R') {
                let body = &bytes[i + 2..i + end];
                if body.iter().all(|b| b.is_ascii_digit() || *b == b';') {
                    replies += 1;
                    i += end + 1;
                    continue;
                }
            }
        }
        rest.push(bytes[i]);
        i += 1;
    }
    (replies, rest)
}

/// T4: the child accepts no input until its output has been read, and floods `ESC[6n`.
/// Axes: response flood alone, typed input, a 32 MiB paste. No deadlock (done within 10 s),
/// pending responses stay capped with drops counted, and queued items arrive in order.
///
/// The gate opens once 1 MiB more than the session can hold unparsed has been read: the inbox
/// and the engine's handed-over batch each hold at most `INBOX_HIGH + READ_SIZE`, so by then
/// the engine has parsed about 1 MiB of `ESC[6n` (less one read in flight; over 1 MiB of
/// replies) with input closed, and the 64 KiB cap must drop. A gate at 1 MiB read opens before the engine parses
/// anything (the reader fills the inbox at once), which made the drop count a race.
#[test]
fn session_threaded_no_deadlock_and_writer_caps() {
    const GATE: u64 = (2 * (INBOX_HIGH + READ_SIZE) + (1 << 20)) as u64;
    const OUT: u64 = GATE + (1 << 20);
    for axis in ["responses", "typed", "paste"] {
        let (pty, probe) = FakePty::flood(b"\x1b[6n", OUT, Gate::AfterOutput(GATE));
        let (w, woken) = waker();
        let started = Instant::now();
        let session = Session::start(pty, config(80, 24), w).expect("start");
        let mut typed = Vec::new();
        let paste_len = 32usize << 20;
        match axis {
            "typed" => {
                for i in 0..200 {
                    let t = format!("<{i}>");
                    typed.extend_from_slice(t.as_bytes());
                    session.send(Command::Text(t));
                }
            }
            "paste" => session.send(Command::Paste("p".repeat(paste_len))),
            _ => {}
        }
        let exit = take_until_exit(&session, &woken, Duration::from_secs(10));
        assert!(exit.is_some(), "{axis}: deadlock (no exit in 10 s)");
        let s = session.stats();
        assert!(s.responses_high_water <= RESPONSE_CAP, "{axis}");
        assert!(
            s.responses_dropped > 0,
            "{axis}: the flood overflows the cap"
        );
        // Input opened at 1 MiB: wait for the writer to drain.
        let expected_min = match axis {
            "paste" => paste_len,
            _ => typed.len(),
        };
        assert!(
            wait_until(Duration::from_secs(10), || probe.written_len()
                >= expected_min
                && session.stats().bytes_written as usize == probe.written_len()),
            "{axis}: the writer did not drain"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{axis}: took too long"
        );
        let written = lock(&probe.written).clone();
        let (replies, rest) = split_replies(&written);
        assert!(replies > 0, "{axis}: replies arrive once input opens");
        match axis {
            "typed" => assert_eq!(rest, typed, "typed input in order"),
            "paste" => {
                assert_eq!(rest.len(), paste_len);
                // Contiguous: the paste is one item, written in pieces.
                let first = written.iter().position(|&b| b == b'p').expect("paste");
                assert!(written[first..first + paste_len].iter().all(|&b| b == b'p'));
            }
            _ => assert!(rest.is_empty()),
        }
        assert!(lock(&probe.write_sizes).iter().all(|&n| n <= WRITE_PIECE));
        assert!(session.close(Duration::from_secs(10)), "{axis}");
    }
}

/// T5: 10,000 publish/take cycles against racing consumers (one woken, one spinning). Every
/// Update is taken exactly once, in order, and waker calls equal Updates published.
#[test]
fn session_threaded_publish_and_wake() {
    let (pty, _probe, tx) = FakePty::scripted(Gate::Open);
    let (w, woken) = waker();
    let session = Arc::new(Session::start(pty, config(40, 5), w).expect("start"));
    let seqs = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Paced by Updates taken, at most 16 messages ahead. Pacing by bytes read let the reader
    // run ahead to a full inbox of 2-byte chunks (one `write` each), so each Update covered
    // ~10 KB of parsing and 10,000 cycles needed most of the 60 s deadline (one timeout in
    // five debug runs).
    let producer = {
        let (stop, seqs) = (Arc::clone(&stop), Arc::clone(&seqs));
        thread::spawn(move || {
            let mut sent = 0usize;
            while !stop.load(Ordering::SeqCst) {
                if sent > lock(&seqs).len() + 16 {
                    thread::yield_now();
                    continue;
                }
                let _ = tx.send(Script::Data(b"x\r".to_vec()));
                sent += 1;
            }
            // tx dropped: EOF.
        })
    };
    let spinner = {
        let (session, seqs, stop) = (Arc::clone(&session), Arc::clone(&seqs), Arc::clone(&stop));
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Some(u) = session.take_update() {
                    lock(&seqs).push(u.seq);
                }
                thread::yield_now();
            }
        })
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while lock(&seqs).len() < 10_000 && Instant::now() < deadline {
        if woken.rx.recv_timeout(Duration::from_millis(10)).is_ok() {
            if let Some(u) = session.take_update() {
                lock(&seqs).push(u.seq);
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    producer.join().expect("producer");
    spinner.join().expect("spinner");
    // Drain to the exit with the woken consumer only.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exited = false;
    while !exited && Instant::now() < deadline {
        let _ = woken.rx.recv_timeout(Duration::from_millis(10));
        while let Some(u) = session.take_update() {
            exited |= u.exited.is_some();
            lock(&seqs).push(u.seq);
        }
    }
    assert!(exited);
    let mut seqs = lock(&seqs).clone();
    assert!(seqs.len() >= 10_000, "only {} cycles", seqs.len());
    seqs.sort_unstable();
    let published = session.stats().updates_published;
    assert_eq!(seqs.len() as u64, published, "every Update taken");
    assert!(
        seqs.iter().enumerate().all(|(i, &s)| s == i as u64 + 1),
        "each exactly once"
    );
    assert_eq!(
        woken.calls.load(Ordering::SeqCst),
        published,
        "one wake per Update"
    );
}

/// T9: the end of output is reported once, after the remaining bytes, with the polled code.
#[test]
fn session_threaded_exit_on_eof_and_eio() {
    // EOF, code known.
    let (pty, probe, tx) = FakePty::scripted(Gate::Open);
    *lock(&probe.exit_code) = Some(7);
    let (w, woken) = waker();
    let session = Session::start(pty, config(20, 3), w).expect("start");
    tx.send(Script::Data(b"bye".to_vec())).expect("send");
    drop(tx);
    let mut grid = Grid::new(1, 1);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exits = Vec::new();
    while Instant::now() < deadline && exits.is_empty() {
        let _ = woken.rx.recv_timeout(Duration::from_millis(10));
        while let Some(u) = session.take_update() {
            apply_update(&mut grid, &u);
            if let Some(e) = u.exited {
                exits.push(e);
            }
        }
    }
    assert_eq!(exits, vec![Exit { code: Some(7) }]);
    assert!(
        row_text(&grid, 0).starts_with("bye"),
        "bytes before the exit"
    );
    // Reported once.
    thread::sleep(Duration::from_millis(100));
    assert!(session.take_update().is_none_or(|u| u.exited.is_none()));
    assert!(
        session.close(Duration::from_secs(1)),
        "close in the normal case"
    );

    // EIO, code unknown: reported after the 1 s poll with no code.
    let (pty, _probe, tx) = FakePty::scripted(Gate::Open);
    let (w, woken) = waker();
    let session = Session::start(pty, config(20, 3), w).expect("start");
    tx.send(Script::Error(5)).expect("send");
    let exit = take_until_exit(&session, &woken, Duration::from_secs(10)).expect("exit");
    assert_eq!(exit.exited, Some(Exit { code: None }));
    assert!(session.close(Duration::from_secs(1)));
}

/// T9: `Drop` returns at once while the writer is blocked forever, and the engine still exits
/// (it drops the `Pty`).
#[test]
fn session_threaded_drop_does_not_wait_for_a_blocked_writer() {
    let (pty, probe, _tx) = FakePty::scripted(Gate::Forever);
    let (w, _woken) = waker();
    let session = Session::start(pty, config(20, 3), w).expect("start");
    session.send(Command::Text("x".to_owned()));
    assert!(wait_until(Duration::from_secs(5), || probe
        .blocked
        .load(Ordering::SeqCst)));
    let t = Instant::now();
    drop(session);
    assert!(
        t.elapsed() < Duration::from_millis(50),
        "Drop took {:?}",
        t.elapsed()
    );
    assert!(
        wait_until(Duration::from_secs(5), || probe
            .dropped
            .load(Ordering::SeqCst)),
        "the engine thread exits and drops the Pty"
    );
    probe.release();
}

/// T10: a 32 MiB paste reaches the child whole, in writes of at most 64 KiB.
#[test]
fn session_threaded_big_paste_in_64k_writes() {
    let (pty, probe, _tx) = FakePty::scripted(Gate::Open);
    let (w, _woken) = waker();
    let session = Session::start(pty, config(20, 3), w).expect("start");
    let text = "abc\n".repeat(8 << 20);
    session.send(Command::Paste(text));
    let want = 32usize << 20;
    assert!(wait_until(Duration::from_secs(60), || probe.written_len() >= want));
    assert_eq!(probe.written_len(), want);
    let written = lock(&probe.written).clone();
    assert!(written.chunks(4).all(|c| c == b"abc\r"));
    let sizes = lock(&probe.write_sizes).clone();
    assert!(sizes.iter().all(|&n| n <= WRITE_PIECE));
    assert!(sizes.len() >= want / WRITE_PIECE);
    assert!(session.close(Duration::from_secs(5)));
}

#[test]
fn session_start_names_its_threads_and_fails_cleanly() {
    // An invalid XTVERSION fails `start` before any thread runs; the Pty is dropped.
    let (pty, probe, _tx) = FakePty::scripted(Gate::Open);
    let mut cfg = config(20, 3);
    cfg.xtversion = "bad\x1bname".to_owned();
    let (w, _woken) = waker();
    assert!(Session::start(pty, cfg, w).is_err());
    assert!(probe.dropped.load(Ordering::SeqCst));
}
