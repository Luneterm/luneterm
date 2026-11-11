//! Tab switches under background load (plan step 3, pass 2): measures, checks nothing.
//!
//! `cargo test -p luneterm --release --test switch_load -- --ignored --nocapture`
//!
//! Several tabs, each a real session on a real PTY running `scripts/tui-load.py` (synthetic TUI
//! output, no real work), and the real `Renderer` drawing offscreen. The loop does what the app's
//! frame does (take every tab's Update, apply it, draw the active grid with the crossfade) on a
//! simulated 60 Hz display, and switches tabs the way `App::active_changed` does. Every frame of
//! the generator shows a `t=<ms>` wall-clock stamp, so the age of what a frame shows is read off
//! the grid. Printed: numbers, tab ids, modes and sizes only (no screen text, no environment).
//!
//! Knobs (all optional): `LUNETERM_SWITCH_TABS` = comma-separated modes, default
//! `ink,ink,tui0,flood` (`ink`/`tui` at 60 fps, `ink0`/`tui0` unthrottled, `flood`, `idle`),
//! `LUNETERM_SWITCH_GRID` = `<cols>x<rows>` (default 240x63, scale 2), `LUNETERM_SWITCH_ROUNDS`
//! (default 3 rounds over every tab), `LUNETERM_SWITCH_FRAMES` (frames printed per switch,
//! default 45), `LUNETERM_SWITCH_DRAW=0` skips the GPU (pump only).

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lune_pty::{PtyBackend, PtySize, SpawnOptions};
use lune_termview::{Cursor, Grid, TermTheme};
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::RawInput;

use lune_ui::widgets::tone::Tone;
use luneterm::fade::{self, Fade};
use luneterm::render::{Content, FadingGrid, Renderer};
use luneterm::session::{
    Command, Geometry, HostPalette, Session, SessionConfig, Update, apply_update,
};
use luneterm::settings::Settings;
use luneterm::shell::{HOST, StripTab};
use luneterm::tabs::TabId;
use luneterm::{fonts, spawn};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const SCALE: f32 = 2.0;
const REFRESH: Duration = Duration::from_micros(16_667);
/// Shown content at most this old counts as caught up (about 3 frames).
const FRESH_MS: f64 = 50.0;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/tui-load.py")
}

/// `ink`, `ink0`, `tui`, `tui0`, `flood` to the generator's arguments.
fn args_of(mode: &str) -> Vec<String> {
    let (m, fps) = match mode {
        "ink" => ("ink", "60"),
        "ink0" => ("ink", "0"),
        "tui" => ("tui", "60"),
        "tui0" => ("tui", "0"),
        "flood" => ("flood", "0"),
        "idle" => ("idle", "0"),
        other => panic!("unknown mode {other:?} (ink, ink0, tui, tui0, flood, idle)"),
    };
    vec![
        script().to_string_lossy().into_owned(),
        "--mode".into(),
        m.into(),
        "--fps".into(),
        fps.into(),
    ]
}

fn wall_ms() -> u64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_millis() % 1_000_000_000) as u64
}

/// The newest `t=<9 digits>` stamp on the screen, if any.
fn newest_stamp(grid: &Grid) -> Option<u64> {
    let mut best: Option<u64> = None;
    let mut line = String::new();
    for row in 0..grid.rows() {
        line.clear();
        for cell in grid.row(row).unwrap_or_default() {
            line.push_str(cell.text());
        }
        let b = line.as_bytes();
        let mut i = 0;
        while i + 11 <= b.len() {
            if b[i] == b't' && b[i + 1] == b'=' && b[i + 2..i + 11].iter().all(u8::is_ascii_digit) {
                let v = std::str::from_utf8(&b[i + 2..i + 11])
                    .ok()
                    .and_then(|s| s.parse().ok());
                if let Some(v) = v {
                    best = Some(best.map_or(v, |o: u64| o.max(v)));
                }
                i += 11;
            } else {
                i += 1;
            }
        }
    }
    best
}

/// ms since `stamp` on the wall clock (modulo the stamp's period).
fn age(stamp: Option<u64>) -> f64 {
    stamp.map_or(f64::NAN, |s| {
        ((wall_ms() + 1_000_000_000 - s) % 1_000_000_000) as f64
    })
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

struct LoadTab {
    mode: String,
    session: Session,
    grid: Grid,
    cursor: Cursor,
    updates: u64,
    last_update: Option<Instant>,
    /// Rows the last Update carried.
    last_rows: usize,
}

fn renderer() -> Option<Renderer> {
    let gpu = match pollster::block_on(Gpu::new(gpu::instance(None), None)) {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("SKIPPED: no wgpu adapter: {e}");
            return None;
        }
    };
    let mut r = Renderer::new(
        gpu,
        fonts::bundled_text_cache(),
        FORMAT,
        &Settings::default().clamped(),
        true,
    );
    r.set_phase_trace(true);
    Some(r)
}

/// The smallest logical window (in 8 px steps) whose grid holds `cols` x `rows`.
fn window_for(r: &mut Renderer, cols: usize, rows: usize) -> (f32, f32) {
    let mut size = (400.0, 300.0);
    while r.grid_size(size, SCALE).0 < cols {
        size.0 += 8.0;
    }
    while r.grid_size(size, SCALE).1 < rows {
        size.1 += 8.0;
    }
    size
}

fn start(mode: &str, cols: usize, rows: usize) -> LoadTab {
    let geometry = Geometry {
        cols,
        rows,
        cell_px: (8.0, 17.0),
        pixel_size: (cols as u32 * 16, rows as u32 * 34),
    };
    let options = SpawnOptions {
        program: Some(PathBuf::from("python3")),
        args: args_of(mode),
        cwd: None,
        env: vec![("TERM".to_owned(), "xterm-256color".to_owned())],
    };
    let size = PtySize {
        columns: cols as u16,
        rows: rows as u16,
        pixel_width: geometry.pixel_size.0 as u16,
        pixel_height: geometry.pixel_size.1 as u16,
    };
    let pty = lune_pty::PortablePtyBackend::new()
        .spawn(&options, size)
        .unwrap_or_else(|e| panic!("spawn python3: {:?}", e.kind()));
    let cfg = SessionConfig {
        geometry,
        scrollback: 1000,
        xtversion: spawn::xtversion(),
        clipboard_write: false,
        windows_pty: None,
        palette: HostPalette::from_theme(&TermTheme::lunegit_one_dark()),
        focused: false,
    };
    // The app's waker asks for a pump; this loop pumps every frame anyway.
    let session = Session::start(pty, cfg, Box::new(|| {}))
        .unwrap_or_else(|e| panic!("Session::start: {:?}", e.kind()));
    LoadTab {
        mode: mode.to_owned(),
        session,
        grid: Grid::new(cols, rows),
        cursor: Cursor::default(),
        updates: 0,
        last_update: None,
        last_rows: 0,
    }
}

fn apply(tab: &mut LoadTab, update: &Update) {
    apply_update(&mut tab.grid, update);
    tab.cursor = luneterm::app::cursor_of(&update.cursor, tab.grid.cols());
    tab.updates += 1;
    tab.last_update = Some(Instant::now());
    tab.last_rows = update.rows_changed.len();
}

#[allow(clippy::too_many_arguments)]
fn draw(
    r: &mut Renderer,
    target: &wgpu::TextureView,
    size: (u32, u32),
    logical: (f32, f32),
    tabs: &mut [LoadTab],
    active: usize,
    alphas: &fade::Alphas,
    theme: &TermTheme,
) {
    let input = RawInput {
        size: logical,
        scale: SCALE,
        time: Duration::ZERO,
        focused: true,
        events: Vec::new(),
    };
    let titles: Vec<String> = (0..tabs.len()).map(|i| format!("t{i}")).collect();
    let strip: Vec<StripTab<'_>> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| StripTab {
            id: i as TabId,
            title: t,
            progress: None,
        })
        .collect();
    let mut grid = None;
    let mut cursor = Cursor::default();
    let mut fading = Vec::new();
    for (i, tab) in tabs.iter_mut().enumerate() {
        if i == active {
            cursor = tab.cursor;
            grid = Some(&mut tab.grid);
        } else if let Some((_, alpha)) = alphas.outgoing.iter().find(|(id, _)| *id == i as TabId) {
            fading.push(FadingGrid {
                id: i as TabId,
                grid: &mut tab.grid,
                cursor: Cursor::default(),
                theme,
                alpha: *alpha,
            });
        }
    }
    let content = Content {
        title: None,
        tabs: &strip,
        active_tab: Some(active as TabId),
        modal: None,
        maximized: false,
        grid: grid.expect("active tab"),
        selection: &[],
        hover_link: 0,
        link_hand: false,
        link_tip: None,
        toasts: None,
        cursor,
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
        busy_tone: Tone::Neutral,
        grid_alpha: alphas.incoming,
        fading,
    };
    r.draw(&input, content, target, size).expect("draw");
    r.gpu.end_frame(&mut r.text);
    let _ = r.gpu.device.poll(wgpu::PollType::wait_indefinitely());
}

#[test]
#[ignore = "measures; run by hand (plan step 3, pass 2)"]
fn switch_under_load() {
    let modes: Vec<String> = env("LUNETERM_SWITCH_TABS")
        .unwrap_or_else(|| "ink,ink,tui0,flood".into())
        .split(',')
        .map(str::to_owned)
        .collect();
    let (cols, rows) = env("LUNETERM_SWITCH_GRID")
        .and_then(|g| {
            let (c, r) = g.split_once('x')?;
            Some((c.parse().ok()?, r.parse().ok()?))
        })
        .unwrap_or((240usize, 63usize));
    let rounds: usize = env("LUNETERM_SWITCH_ROUNDS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let printed: u32 = env("LUNETERM_SWITCH_FRAMES")
        .and_then(|v| v.parse().ok())
        .unwrap_or(45);
    let gpu_on = env("LUNETERM_SWITCH_DRAW").as_deref() != Some("0");
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let mut r = if gpu_on { renderer() } else { None };
    if gpu_on && r.is_none() {
        return;
    }
    let logical = r.as_mut().map_or((0.0, 0.0), |r| window_for(r, cols, rows));
    let size = (
        (logical.0 * SCALE).round() as u32,
        (logical.1 * SCALE).round() as u32,
    );
    let target = r.as_ref().map(|r| {
        lune_ui::dump::target(&r.gpu.device, FORMAT, size)
            .create_view(&wgpu::TextureViewDescriptor::default())
    });
    let theme = TermTheme::lunegit_one_dark();
    let mut tabs: Vec<LoadTab> = modes.iter().map(|m| start(m, cols, rows)).collect();
    eprintln!(
        "load: build={build} grid={cols}x{rows} draw={} tabs={}",
        u8::from(gpu_on),
        modes.join(",")
    );
    let mut active = 0usize;
    for (i, tab) in tabs.iter().enumerate() {
        tab.session.set_visible(i == active);
        tab.session.send(Command::Focus(i == active));
    }
    let mut fade = Fade::default();
    let mut deadline = Instant::now();
    let total_switches = rounds * tabs.len();
    // Let every generator fill its screen and the hidden ones settle at their 100 ms pace.
    let warmup = Instant::now() + Duration::from_secs(3);
    let mut switch_n = 0usize;
    let mut next_switch = warmup;
    // The switch being followed: (n, t0, new, frame, first update seen, fresh seen).
    struct Follow {
        n: usize,
        t0: Instant,
        new: usize,
        frame: u32,
        upd_before: u64,
        first_update: Option<f64>,
        fresh: Option<f64>,
        max_frame: f64,
        max_pump: f64,
        age0: f64,
    }
    let mut follow: Option<Follow> = None;
    let mut last_frame = Instant::now();
    let mut summary = Vec::new();
    loop {
        let frame_start = Instant::now();
        let interval = ms(frame_start - last_frame);
        last_frame = frame_start;
        if frame_start >= next_switch && follow.is_none() {
            if switch_n == total_switches {
                break;
            }
            // `App::active_changed`.
            let old = active;
            active = (active + 1) % tabs.len();
            switch_n += 1;
            let t0 = Instant::now();
            fade.switch(t0, fade::FULL, Some(old as TabId), active as TabId);
            tabs[old].grid.mark_all_dirty();
            tabs[old].session.send(Command::Focus(false));
            tabs[active].session.send(Command::Focus(true));
            tabs[active].grid.mark_all_dirty();
            for (i, tab) in tabs.iter().enumerate() {
                tab.session.set_visible(i == active);
            }
            let age0 = age(newest_stamp(&tabs[active].grid));
            let since_upd = tabs[active].last_update.map_or(f64::NAN, |t| ms(t0 - t));
            eprintln!(
                "switch: n={switch_n} to=tab{active} mode={} shown_age={age0:.0} \
                 since_last_update={since_upd:.0}",
                tabs[active].mode
            );
            follow = Some(Follow {
                n: switch_n,
                t0,
                new: active,
                frame: 0,
                upd_before: tabs[active].updates,
                first_update: None,
                fresh: None,
                max_frame: 0.0,
                max_pump: 0.0,
                age0,
            });
        }
        // pump: every tab's Update.
        let mut taken = 0;
        let mut rows_taken = 0;
        for tab in tabs.iter_mut() {
            if let Some(update) = tab.session.take_update() {
                apply(tab, &update);
                taken += 1;
                rows_taken += update.rows_changed.len();
            }
        }
        let pump_ms = ms(frame_start.elapsed());
        let (alphas, _running) = fade.tick(Instant::now(), |_| true);
        if let (Some(r), Some(target)) = (r.as_mut(), target.as_ref()) {
            draw(r, target, size, logical, &mut tabs, active, &alphas, &theme);
        }
        let frame_ms = ms(frame_start.elapsed());
        let shown_age = age(newest_stamp(&tabs[active].grid));
        if let Some(fl) = follow.as_mut() {
            let since = ms(frame_start - fl.t0);
            if fl.first_update.is_none() && tabs[fl.new].updates > fl.upd_before {
                fl.first_update = Some(ms(Instant::now() - fl.t0));
            }
            if fl.fresh.is_none() && shown_age <= FRESH_MS {
                fl.fresh = Some(ms(Instant::now() - fl.t0));
            }
            fl.max_frame = fl.max_frame.max(frame_ms);
            fl.max_pump = fl.max_pump.max(pump_ms);
            if fl.frame < printed {
                let p = r.as_ref().and_then(|r| r.phases().copied());
                eprintln!(
                    "frame: n={} f={} at={since:.1} interval={interval:.1} pump={pump_ms:.2} \
                     frame={frame_ms:.2} active_upd={:.2} fading_upd={:.2} updates={taken} \
                     rows={rows_taken} new_tab_updates={} new_last_rows={} shown_age={shown_age:.0}",
                    fl.n,
                    fl.frame,
                    p.map_or(f64::NAN, |p| ms(p.active - p.ui)),
                    p.map_or(f64::NAN, |p| ms(p.fading - p.assign)),
                    tabs[fl.new].updates - fl.upd_before,
                    tabs[fl.new].last_rows,
                );
            }
            fl.frame += 1;
            if fl.frame >= printed.max(30) && (fl.fresh.is_some() || tabs[fl.new].mode == "idle")
                || since > 3000.0
            {
                let st = tabs[fl.new].session.stats();
                let line = format!(
                    "result: n={} to=tab{} mode={} shown_age_at_switch={:.0} \
                     first_update_ms={:.1} fresh_ms={:.1} max_frame_ms={:.2} max_pump_ms={:.2} \
                     inbox_high_water={} reader_waits={} updates_published={}",
                    fl.n,
                    fl.new,
                    tabs[fl.new].mode,
                    fl.age0,
                    fl.first_update.unwrap_or(f64::NAN),
                    fl.fresh.unwrap_or(f64::NAN),
                    fl.max_frame,
                    fl.max_pump,
                    st.inbox_high_water,
                    st.reader_waits,
                    st.updates_published,
                );
                eprintln!("{line}");
                summary.push(line);
                follow = None;
                next_switch = Instant::now() + Duration::from_millis(1500);
            }
        }
        // The display: the next refresh after this frame.
        deadline += REFRESH;
        let now = Instant::now();
        while deadline <= now {
            deadline += REFRESH;
        }
        std::thread::sleep(deadline - now);
    }
    eprintln!(
        "summary: build={build} grid={cols}x{rows} tabs={}",
        modes.join(",")
    );
    for line in &summary {
        eprintln!("{line}");
    }
    for tab in tabs.drain(..) {
        tab.session.begin_close();
        let _ = tab.session.close(Duration::from_millis(500));
    }
}
