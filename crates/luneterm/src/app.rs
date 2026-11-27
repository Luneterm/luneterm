//! The app: the event loop, the window, the frame loop and the tabs' session wiring (plan M9 steps
//! 4 and 6, M10 step 2).
//!
//! Composition follows the toolkit's gallery (`apps/gallery/src/window.rs`): the app owns the event
//! loop, the window, the GPU stack and the clock. The window is created hidden and shown after its
//! first frame. A frame is `Translator::take` → [`Renderer::draw`] → `OutputState::apply` →
//! `Clipboard::service` → present. Every frame ask (the frame's `Repaint`, the cursor blink, the
//! crossfade, input, a session's Update) goes through the frame pacer (`crate::pacing`), and the
//! loop's control flow is set from its deadline after every callback, so an idle window waits with
//! no timer.
//!
//! The terminal owns the keyboard (D4): key and IME events go to the active tab's session (through
//! the shortcut table and the key router) and never reach the toolkit's translator, so `Ui::begin`
//! cannot eat Tab or Escape; while a dialog is open they are the toolkit's instead. Modifier,
//! pointer, wheel, focus, resize and scale events always reach it. Each tab's session runs on its
//! own threads; this thread only sends commands and takes the Updates, one per tab per frame
//! (D1, D2).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{
    ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{UserAttentionType, Window, WindowId};

use lune_pty::PortablePtyBackend;
use lune_termview::{Cell, Cursor, CursorShape, Grid, SelectionRange, TermTheme};
use lune_ui::clipboard::Clipboard;
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::{ClipboardRequest, OutputState, Repaint, Translator, WindowCommand, keymap};
use lune_ui::material::{self, Material};
use lune_ui::motion::Cadence;
use lune_ui::paint::Rect;
use lune_ui::surface::{Acquired, SurfaceOptions, WindowSurface};
use lune_ui::widgets::menu::MenuPlace;
use lune_ui::widgets::popover::{PlaceAlign, Side};
use lune_ui::widgets::tab_strip::{TabEvent, TabKey};
use lune_ui::widgets::toast::{ToastId, ToastKind, ToastSpec, Toasts};
use lune_ui::widgets::tone::Tone;
use lune_ui::window::glass_attributes;
use lune_vt::{
    CursorStyle, MouseEncoding, MouseTracking, TerminalModifiers, TerminalMouseButton,
    TerminalMouseEvent, ViewportCursor,
};

use crate::ctl::handler::{
    self as control, Call, ControlTab, Ctx, Effect as ControlEffect, INPUT_HOLD, OpenSpec, Outcome,
};
use crate::ctl::server::{Reply, Server};
use crate::ctl::{self, Code};
use crate::cwd;
use crate::dump;
use crate::efficiency::{Controller, HostOs};
use crate::fade::{self, Fade};
use crate::fonts;
use crate::input::{
    AppCommand, AppShortcuts, ClickCounter, KeyRoute, MouseCtx, MouseState, PointerEvent, RouteCtx,
    WheelAction, WheelCtx, WheelState, key_input, mouse_to_terminal, pointer_event, route_key,
    selection_position, selection_scroll, terminal_mods, wheel, wheel_delta, wheel_report,
};
use crate::links::{self, open::OpenError};
use crate::menus::{self, MenuAction, MenuKind, MenuModel, TabOp};
use crate::notices::{self, Admit, DrawnRows, Inbox, PressClock, TOAST_LIFE, ToastSlots, route};
use crate::pacing::{self, EventKind, Pacer, PresentAsk};
use crate::palettes;
use crate::persist::{
    Persist, SaveGuard, Startup, Store, TabInfo, TabState, Workspace, restore_dirs, restore_layout,
    snapshot,
};
use crate::render::{Content, FadingGrid, InboxDraw, LinkTip, MenuDraw, Placed, Renderer};
use crate::session::{
    Command, CopyResult, HostPalette, MAX_COPY_BYTES, Scroll, SelectCmd, Session, SessionConfig,
    UpdateModes, view_range,
};
use crate::settings::{Effect, Settings, effects};
use crate::settings_page::{PageState, PageView, Section, UpdateAsk};
use crate::shell::UpdateToastKind;
use crate::shell::{
    self, HOST, InboxButton, InboxList, Modal, ModalOut, ScrollView, StripTab, TAB_CLOSABLE,
    ThumbDrag,
};
use crate::spawn::{self, ProgramFailure, ProgramSpec, Refused, SpawnFailure};
use crate::tabs::{
    ClosePlan, CwdCheck, CwdState, Message, Origin, PasteState, Tab, TabId, TabSet,
    confirm_message, plan_close,
};
use crate::title::DEFAULT_TITLE;
use crate::update::auto::{AutoCheck, ToastRule, keeps_restart_toast};
use crate::update::{self, Launch, UpdateState, Updater};

/// The window's first size and its minimum, logical px.
const WINDOW_SIZE: (f64, f64) = (1040.0, 660.0);
const WINDOW_MIN: (f64, f64) = (560.0, 360.0);
/// The cursor blink half-period (xterm.js and Lunegit's 600 ms).
const BLINK_PERIOD: Duration = Duration::from_millis(600);
/// The IME's composing text is drawn up to this many chars.
const MAX_PREEDIT_CHARS: usize = 256;
/// The dock or taskbar attention request is repeated at most this often (D13).
const BELL_INTERVAL: Duration = Duration::from_secs(2);
/// How long closing waits for the session's threads before the process leaves anyway.
const CLOSE_WAIT: Duration = Duration::from_millis(500);
/// The stand-in tab of the update toast in its slot table (a toast of no tab; real tab ids count
/// up from 1).
const UPDATE_SLOT_TAB: TabId = TabId::MAX;
/// How long quitting waits for the update worker once its operation is cancelled.
const UPDATE_WAIT: Duration = Duration::from_secs(1);

/// Events other threads post to the event loop.
#[derive(Debug)]
pub enum UserEvent {
    /// The session has an Update for the UI: draw a frame.
    Wake,
    /// The update worker's new state (update.rs).
    Update(UpdateState),
    /// The update worker handed the staged version to lunepack's runtime: quit through the
    /// normal path so the runtime can apply it.
    UpdateRestart,
    /// `LUNETERM_TRACE_SWITCH_AUTO`: switch to the next tab, as the shortcut does; posted at the
    /// instant it carries, so the trace sees how long the event loop took to get to it.
    TraceSwitch(Instant),
    /// A `luneterm ctl` request from the control listener (`ctl::handler::Bridge`), answered on
    /// this thread.
    Control(Call),
}

/// The cursor blink phase. It runs only while the window is focused and the cursor blinks; when it
/// does not run the cursor is on and no timer is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blink {
    on: bool,
    next: Option<Instant>,
}

impl Default for Blink {
    fn default() -> Self {
        Blink {
            on: true,
            next: None,
        }
    }
}

impl Blink {
    /// Whether the cursor is drawn now.
    pub fn on(&self) -> bool {
        self.on
    }

    /// Advances the phase to `now`. `active` is "focused and the cursor blinks". Returns when the
    /// next toggle is due, `None` while not active.
    pub fn tick(&mut self, now: Instant, active: bool) -> Option<Instant> {
        if !active {
            *self = Blink::default();
            return None;
        }
        match self.next {
            None => self.next = Some(now + BLINK_PERIOD),
            Some(due) if now >= due => {
                self.on = !self.on;
                self.next = Some(now + BLINK_PERIOD);
            }
            Some(_) => {}
        }
        self.next
    }
}

/// The earlier of two control flows: polling beats a deadline, a deadline beats waiting.
pub fn merge_flow(a: ControlFlow, b: ControlFlow) -> ControlFlow {
    match (a, b) {
        (ControlFlow::Poll, _) | (_, ControlFlow::Poll) => ControlFlow::Poll,
        (ControlFlow::WaitUntil(x), ControlFlow::WaitUntil(y)) => ControlFlow::WaitUntil(x.min(y)),
        (ControlFlow::WaitUntil(t), ControlFlow::Wait)
        | (ControlFlow::Wait, ControlFlow::WaitUntil(t)) => ControlFlow::WaitUntil(t),
        (ControlFlow::Wait, ControlFlow::Wait) => ControlFlow::Wait,
    }
}

/// The control flow after a callback (I1): the pacer's deadline (app time, from `start`) merged
/// with the app's own deadlines (`pending`); waiting while the window is hidden, where no frame
/// would replace a deadline and a `WaitUntil` in the past makes the loop spin (un-occlusion asks
/// a frame, which sets them again). Reads no clock.
pub fn settled_flow(
    deadline: Option<Duration>,
    pending: ControlFlow,
    occluded: bool,
    start: Instant,
) -> ControlFlow {
    if occluded {
        return ControlFlow::Wait;
    }
    let paced = deadline
        .and_then(|d| start.checked_add(d))
        .map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
    merge_flow(paced, pending)
}

/// Whether `flow` waits for a moment at or before `now`.
pub fn flow_due(flow: ControlFlow, now: Instant) -> bool {
    match flow {
        ControlFlow::Poll => true,
        ControlFlow::WaitUntil(t) => t <= now,
        ControlFlow::Wait => false,
    }
}

/// Rate-limits the bell's attention request (D13): while unfocused, at most one per 2 s; while
/// focused, none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BellLimiter {
    last: Option<Instant>,
}

impl BellLimiter {
    /// Whether a bell heard at `now` asks for attention.
    pub fn due(&mut self, now: Instant, focused: bool) -> bool {
        if focused
            || self
                .last
                .is_some_and(|l| now.saturating_duration_since(l) < BELL_INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// A message that earned a toast (D6): the tab it came from and its sanitised text. `pump` turns
/// them into toasts at its end (`show_notice_toasts`).
struct ToastCandidate {
    tab: TabId,
    title: String,
    body: String,
}

/// What `hear` found in one tab's Update.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Heard {
    /// A row was recorded: the bell, a dot or the list changed.
    recorded: bool,
    /// A bell or an accepted message wants the OS attention request (the limiter still decides).
    attention: bool,
}

/// Routes one tab's bells and messages through the D6 table into the inbox. `messages_on` is the
/// "Notifications from programs" setting: off, the messages change nothing at all; bells are not
/// affected. Message texts are already sanitised (`Tab::apply`). Toast candidates are appended to
/// `candidates`. Nothing here logs or formats text.
#[allow(clippy::too_many_arguments)]
fn hear(
    inbox: &mut Inbox,
    candidates: &mut Vec<ToastCandidate>,
    tab: TabId,
    bells: u32,
    messages: Vec<Message>,
    focused: bool,
    active: bool,
    messages_on: bool,
    now: Duration,
) -> Heard {
    let mut heard = Heard::default();
    if bells > 0 {
        let r = route(true, focused, active);
        if r.record {
            inbox.bell(tab, bells, now);
            heard.recorded = true;
        }
        heard.attention |= r.os_attention;
    }
    if messages_on {
        for m in messages {
            let r = route(false, focused, active);
            heard.attention |= r.os_attention;
            if r.record {
                inbox.message(tab, m.title.clone(), m.body.clone(), now);
                heard.recorded = true;
            }
            if r.toast {
                candidates.push(ToastCandidate {
                    tab,
                    title: m.title,
                    body: m.body,
                });
            }
        }
    }
    heard
}

/// The start of the bell's last ring after a tab's Update was heard (D19): a new ring begins at
/// `now` when a row was recorded in a focused window and the last ring began [`shell::RING_GAP`]
/// ago or more; an unfocused window never starts one, and arrivals inside the gap change nothing.
fn ring_after(
    last: Option<Duration>,
    recorded: bool,
    focused: bool,
    now: Duration,
) -> Option<Duration> {
    if recorded && focused && shell::ring_due(last, now) {
        Some(now)
    } else {
        last
    }
}

/// The ring the button is handed for a frame at `now`: the start of the last ring while anything is
/// unread, the window is focused and the ring has not run its [`shell::RING_LEN`] out.
fn ring_for_frame(
    start: Option<Duration>,
    unread: usize,
    focused: bool,
    now: Duration,
) -> Option<Duration> {
    start.filter(|s| unread > 0 && focused && now < *s + shell::RING_LEN)
}

/// Whether the inbox list is open after a frame: a press on the button toggles it; Escape, a
/// press outside, a focus loss (all `dismissed`), Clear all and a row press close it.
fn inbox_open_after(was_open: bool, clicked: bool, out: Option<&shell::InboxOut>) -> bool {
    if clicked {
        return !was_open;
    }
    match out {
        Some(o) if o.dismissed.is_some() || o.clear_all || o.clicked.is_some() => false,
        _ => was_open,
    }
}

/// Keeps the inbox true against the tab set (D13): rows of closed tabs go, and while the window is
/// focused the active tab has nothing unread. One call covers every way the active tab changes or
/// a tab closes. Returns whether anything changed.
fn settle_inbox<T>(inbox: &mut Inbox, tabs: &TabSet<T>, focused: bool) -> bool {
    inbox.settle(tabs.active(), focused, |id| tabs.get(id).is_some())
}

/// Whether `pos` is inside one of `rects` (the toasts of the last drawn frame).
fn over_any(rects: &[Rect], pos: (f32, f32)) -> bool {
    rects.iter().any(|r| r.contains(pos))
}

/// Whether the pointer at `pos` is the terminal's: inside the body and under no toast. Every
/// pointer path into the terminal (press, move report, selection start, link hover, the context
/// menu, the wheel) asks this; a drag that began in the terminal does not (it keeps its capture
/// across a toast).
fn terminal_hit(body: Rect, toasts: &[Rect], pos: (f32, f32)) -> bool {
    body.contains(pos) && !over_any(toasts, pos)
}

/// The termview cursor of the engine's. A pending wrap puts the column one past the last; the cursor
/// stays on the last cell.
pub fn cursor_of(c: &ViewportCursor, cols: usize) -> Cursor {
    Cursor {
        row: c.y,
        col: c.x.min(cols.saturating_sub(1)),
        shape: match c.style {
            CursorStyle::Block => CursorShape::Block,
            CursorStyle::Underline => CursorShape::Underline,
            CursorStyle::Bar => CursorShape::Bar,
        },
        blink: c.blink,
        visible: c.visible,
    }
}

/// `base` with the session palette's colours (a child's OSC 4/10/11/12 sets and restores). Alpha is
/// the base's.
pub fn theme_with_palette(base: &TermTheme, palette: &HostPalette) -> TermTheme {
    let mut theme = base.clone();
    for (dst, src) in theme.palette.iter_mut().zip(&palette.ansi) {
        dst[..3].copy_from_slice(src);
    }
    theme.foreground[..3].copy_from_slice(&palette.foreground);
    theme.background[..3].copy_from_slice(&palette.background);
    theme.cursor[..3].copy_from_slice(&palette.cursor);
    theme
}

/// The grid that tells why no shell started (D12): the lines from the top-left, no cursor.
pub fn failure_grid(cols: usize, rows: usize, failure: &SpawnFailure) -> Grid {
    let mut grid = Grid::new(cols, rows);
    for (row, line) in spawn::failure_lines(failure).iter().enumerate() {
        for (col, c) in line.chars().enumerate() {
            let mut cell = Cell::default();
            cell.set_char(c);
            if !grid.set(row, col, cell) {
                break;
            }
        }
    }
    grid
}

/// Runs the app: a frame dump when `LUNETERM_DUMP_FRAMES` is set (this is the only place the
/// variable is read), otherwise the window.
/// `launch` is what `update::startup` found.
pub fn run(launch: Launch) -> ExitCode {
    if let Some(dir) = std::env::var_os("LUNETERM_DUMP_FRAMES").filter(|v| !v.is_empty()) {
        return dump::run(Path::new(&dir));
    }
    run_window(launch)
}

fn run_window(launch: Launch) -> ExitCode {
    // The present mode knob (this is the only place the variable is read); checked before
    // anything is built, a value that is not UTF-8 is as wrong as any other.
    let present = {
        let var = match std::env::var(pacing::PRESENT_VAR) {
            Ok(v) => Some(v),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
        };
        match pacing::present_from_env(var.as_deref()) {
            Ok(ask) => ask.unwrap_or(pacing::DEFAULT_PRESENT),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(2);
            }
        }
    };
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_activate_ignoring_other_apps(true);
    }
    let event_loop = match builder.build() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("event loop: {e}");
            return ExitCode::from(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    // The config directory: the platform's, or the one `LUNETERM_CONFIG_DIR` names (tests and
    // trial runs use it to leave the real one alone); found as `luneterm ctl` finds it.
    let dir = ctl::client::config_dir();
    let config_dir = dir.clone();
    let (persist, start) = match dir {
        Some(dir) => Persist::load(Store::new(dir)),
        None => (
            Persist::none(),
            Startup {
                settings: Settings::default(),
                workspace: None,
                notices: Vec::new(),
            },
        ),
    };
    let mut app = App::new(
        start.settings.clamped(),
        event_loop.create_proxy(),
        persist,
        start.workspace,
        start.notices,
        config_dir,
    );
    app.present = present;
    // The update worker (none for a copy lunepack did not install); it touches the network only
    // when a check or download is asked for.
    app.health_pending = launch.pending();
    let update_proxy = event_loop.create_proxy();
    // The channel this copy follows: recorded in the settings on the first run of an installed
    // copy and on the first start after a setup file installed it, then kept whatever a later
    // pack's own config says (Q17, update::channel). Saved now: lunepack delivers the setup's
    // event only once.
    let settled = update::channel::settle(&launch, &app.settings.update_channel);
    if settled.rewrite
        && let Some(channel) = &settled.channel
    {
        app.settings.update_channel.clone_from(channel);
        app.persist.save_settings(&app.settings);
    }
    app.update = Updater::spawn(&launch, settled.channel, move |event| {
        let _ = update_proxy.send_event(match event {
            update::Event::State(state) => UserEvent::Update(state),
            update::Event::Restart => UserEvent::UpdateRestart,
        });
    });
    app.update_state = app.update.initial_state();
    app.sync_auto_check();
    app.ctl = start_control(
        app.config_dir.as_deref(),
        &app.instance,
        event_loop.create_proxy(),
    );
    // The tab-switch trace (this is the only place the variable is read).
    if std::env::var_os("LUNETERM_TRACE_SWITCH").is_some_and(|v| v == "1") {
        app.trace = Some(Box::default());
        // A switch every <ms>, posted from a thread of its own like a session's wake (measures
        // the loop's latency without a hand on the mouse). Only read while the trace is on.
        if let Some(v) = std::env::var_os("LUNETERM_TRACE_SWITCH_AUTO") {
            match v.to_str().and_then(|v| v.parse::<u64>().ok()) {
                Some(ms @ 200..=60_000) => {
                    let proxy = event_loop.create_proxy();
                    let _ = std::thread::Builder::new()
                        .name("luneterm-trace-switch".into())
                        .spawn(move || {
                            loop {
                                std::thread::sleep(Duration::from_millis(ms));
                                let at = Instant::now();
                                if proxy.send_event(UserEvent::TraceSwitch(at)).is_err() {
                                    return;
                                }
                            }
                        });
                }
                _ => eprintln!("luneterm: LUNETERM_TRACE_SWITCH_AUTO must be ms in 200..=60000"),
            }
        }
        // A synthetic left click into the window every <ms> (macOS), posted into the app's own
        // event queue from a thread: the known answer for the OS-timestamp ages (os_event.rs).
        if let Some(v) = std::env::var_os("LUNETERM_TRACE_SWITCH_CLICK") {
            match v.to_str().and_then(|v| v.parse::<u64>().ok()) {
                Some(ms @ 50..=60_000) => {
                    if let Some(t) = &mut app.trace {
                        t.click_every = Some(Duration::from_millis(ms));
                    }
                }
                _ => eprintln!("luneterm: LUNETERM_TRACE_SWITCH_CLICK must be ms in 50..=60000"),
            }
        }
    }
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop: {e}");
        return ExitCode::from(1);
    }
    ExitCode::from(app.exit_code)
}

/// The control listener for this run (D19): it runs whatever the setting says (`ctl status`
/// answers when control is off). When it cannot run, one line says why (never a path) and the
/// app runs without it.
fn start_control(
    config_dir: Option<&Path>,
    instance: &str,
    proxy: EventLoopProxy<UserEvent>,
) -> Option<Server> {
    let off = |why: &dyn std::fmt::Display| {
        eprintln!("luneterm: tab control is unavailable for this run: {why}");
    };
    if instance.is_empty() {
        off(&"no instance token (the OS gave no randomness)");
        return None;
    }
    let Some(dir) = config_dir else {
        off(&"no config directory");
        return None;
    };
    let bridge =
        control::Bridge::new(move |call| proxy.send_event(UserEvent::Control(call)).is_ok());
    match Server::start(dir, instance, bridge) {
        Ok(server) => Some(server),
        Err(e) => {
            off(&e);
            None
        }
    }
}

/// Whether input typed, pasted or composed into `tab` at `now` goes through: not while a control
/// request's hold lasts (D9; dropped, not queued). Input that goes through is the tab's latest
/// (`tab list`'s `input_idle_ms`).
fn admit_input(hold_until: Option<Instant>, tab: Option<&mut Tab>, now: Instant) -> bool {
    if hold_until.is_some_and(|until| now < until) {
        return false;
    }
    if let Some(tab) = tab {
        tab.note_input(now);
    }
    true
}

/// What a save keeps of each tab, in order ([`snapshot`] decides which are written).
fn tab_infos(tabs: &TabSet<Tab>) -> Vec<TabInfo> {
    tabs.iter()
        .map(|(id, t)| TabInfo {
            started: t.session.is_some(),
            launch_dir: t.launch_dir.clone(),
            live_dir: t
                .cwd
                .as_deref()
                .filter(|c| cwd::syntax(c).is_some())
                .map(str::to_owned),
            title: t.manual_name().map(str::to_owned),
            program: t.program,
            group: tabs.group_of(id).and_then(|g| tabs.group(g)).cloned(),
        })
        .collect()
}

/// What `luneterm ctl` reads of a tab (`ctl::handler`): never its session or what it shows.
impl ControlTab for Tab {
    fn origin(&self) -> Origin {
        self.origin
    }
    fn title(&self) -> &str {
        self.strip_title()
    }
    fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }
    fn pid(&self) -> Option<u32> {
        self.pid
    }
    fn last_input(&self) -> Option<Instant> {
        self.last_input
    }
}

/// The window and what turns its events into frames and its frames into window actions.
struct Win {
    window: Arc<Window>,
    translator: Translator,
    output: OutputState,
    clipboard: Clipboard,
}

/// Everything built on one GPU device. The device can be lost: all of it is rebuilt together.
struct Stack {
    surface: WindowSurface,
    renderer: Renderer,
    /// The concrete present mode the surface was given (never an `Auto*` mode).
    mode: wgpu::PresentMode,
    /// The installed curated system fonts the font combo offers, found once from the text cache
    /// the renderer was built with.
    system_fonts: Vec<&'static str>,
}

impl Stack {
    fn build(
        event_loop: &ActiveEventLoop,
        window: &Arc<Window>,
        settings: &Settings,
        glass: bool,
        theme: &TermTheme,
        trace: bool,
        present: PresentAsk,
    ) -> Result<Stack, String> {
        let instance = gpu::instance(Some(Box::new(event_loop.owned_display_handle())));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface)))?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&gpu.adapter).present_modes;
        let mode = pacing::resolve(present, &caps);
        let options = SurfaceOptions {
            present_mode: mode,
            frame_latency: 2,
            // `Auto` depth on a glass window: the 8-bit format of 0.6 on every OS (luneui-rs D330).
            ..SurfaceOptions::default()
        };
        let surface =
            WindowSurface::with_options(&gpu, surface, (size.width, size.height), options)?;
        // Mode names and a rate only: what was asked, what the surface lists and what it was
        // given, and the frame grid (D8).
        let hz = pacing::grid_hz(mode, refresh_mhz(window));
        eprintln!(
            "luneterm: present {mode:?} (asked {}), frames <= {hz}/s, caps {caps:?}",
            present.name()
        );
        let (text, _took) = fonts::text_cache();
        let system_fonts = fonts::system_fonts(&text);
        let mut renderer = Renderer::new(gpu, text, surface.format(), settings, glass);
        if renderer.term_theme() != theme {
            renderer.set_term_theme(theme.clone());
        }
        renderer.set_phase_trace(trace);
        Ok(Stack {
            surface,
            renderer,
            mode,
            system_fonts,
        })
    }
}

/// The refresh rate of the display `window` is on, in mHz, when the OS says.
fn refresh_mhz(window: &Window) -> Option<u32> {
    window
        .current_monitor()
        .and_then(|m| m.refresh_rate_millihertz())
}

/// Puts the OS material the settings ask for on `window`. Returns whether the window has glass
/// behind it; when it does not (Linux, or the OS refused) the theme is opaque.
fn apply_material(window: &Window, settings: &Settings) -> bool {
    let wanted = if shell::HOST_GLASS {
        settings.material()
    } else {
        Material::Opaque
    };
    if wanted.is_glass() {
        match material::apply(window, wanted) {
            Ok(()) => return true,
            Err(e) => eprintln!("glass: {e}; continuing opaque"),
        }
    }
    if let Err(e) = material::apply(window, Material::Opaque) {
        eprintln!("glass: {e}");
    }
    false
}

/// A dialog over the window. It takes the keyboard (the toolkit's, not the terminal's) and the
/// pointer until it is answered.
#[derive(Debug)]
enum ModalState {
    /// Close-confirm over `ids` (the tabs to close if confirmed).
    Confirm {
        ids: Vec<TabId>,
        message: String,
        dont_ask: bool,
    },
    /// Rename Tab. `frames` counts the frames drawn: the caret goes into the field on the second
    /// (the dialog's own open focus lands on the confirm button).
    Rename {
        id: TabId,
        text: String,
        frames: u32,
    },
}

/// A menu that is open: which, and where.
#[derive(Debug, Clone, Copy)]
struct OpenMenu {
    kind: MenuKind,
    place: MenuPlace,
    /// The tab a tab menu was opened on (its picks act on it, not on the active tab); `None` for
    /// the other menus.
    tab: Option<TabId>,
}

/// The settings file is written this long after the last change (typing in a field changes it
/// with every key).
const SETTINGS_DEBOUNCE: Duration = Duration::from_millis(400);
/// The workspace is written at most this often while it changes (and once more at exit).
const WORKSPACE_DEBOUNCE: Duration = Duration::from_secs(1);
/// How long the start waits for the saved directories to be checked (off the UI thread).
const RESTORE_CHECK_WAIT: Duration = Duration::from_millis(500);

/// How far (logical px) the pointer may move with a link pressed before it is a drag.
const LINK_DRAG_SLOP: f32 = 4.0;

/// Where the pointer is. A pointer that leaves the window with a button down keeps its last
/// position until the button is up: the release (which the window system still delivers) then
/// ends what the press started (a selection, a program's press, the scroll thumb), instead of
/// being dropped for want of a position and leaving the hold set.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Pointer {
    /// The last position, logical px.
    pos: Option<(f32, f32)>,
    /// The pointer left the window while a button was held.
    gone: bool,
}

impl Pointer {
    fn moved(&mut self, pos: (f32, f32)) {
        *self = Pointer {
            pos: Some(pos),
            gone: false,
        };
    }

    /// The pointer left the window; `held` is whether a press of ours is still down.
    fn left(&mut self, held: bool) {
        if held && self.pos.is_some() {
            self.gone = true;
        } else {
            *self = Pointer::default();
        }
    }

    /// A button went up; `held` is whether a press of ours is still down after it. Returns whether
    /// the pointer, which had left, is now forgotten.
    fn released(&mut self, held: bool) -> bool {
        let forget = self.gone && !held;
        if forget {
            *self = Pointer::default();
        }
        forget
    }

    /// The position the next press or release is at (the last one, also after leaving).
    fn last(&self) -> Option<(f32, f32)> {
        self.pos
    }

    /// The position while the pointer is in the window: what hover reads.
    fn over(&self) -> Option<(f32, f32)> {
        if self.gone { None } else { self.pos }
    }
}

/// A key that is only a modifier: holding Cmd or Shift to open a link is not typing.
fn is_modifier_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Named(
            NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::Hyper
                | NamedKey::CapsLock
                | NamedKey::Fn
                | NamedKey::FnLock
                | NamedKey::NumLock
                | NamedKey::ScrollLock
                | NamedKey::Symbol
                | NamedKey::SymbolLock
        )
    )
}

/// A press on a link, waiting for its release.
#[derive(Debug)]
struct PendingLink {
    uri: String,
    down: (f32, f32),
}

/// Ends a session without keeping the UI thread: it stops in the background and the child is
/// killed there. (The last session of the app is closed by [`App::shutdown`], which waits.)
fn retire(session: Session) {
    session.begin_close();
    let spawned = thread::Builder::new()
        .name("luneterm-retire".into())
        .spawn(move || {
            let _ = session.close(CLOSE_WAIT);
        });
    if let Err(e) = spawned {
        // The closure, and the session in it, was dropped: that signals shutdown too.
        eprintln!("luneterm: retire thread: {:?}", e.kind());
    }
}

struct App {
    settings: Settings,
    /// OS glass is behind the window.
    glass: bool,
    win: Option<Win>,
    stack: Option<Stack>,
    proxy: EventLoopProxy<UserEvent>,
    /// The tabs, one session each (D1).
    tabs: TabSet<Tab>,
    /// Sessions of tabs closed as the app leaves: closed with a wait in `shutdown`.
    leaving: Vec<Session>,
    /// A new tab waiting for the check of the directory it should start in.
    pending_new: Option<CwdCheck>,
    modal: Option<ModalState>,
    /// The settings page, covering the card while it is open.
    page: Option<PageState>,
    /// The page's notice banner: a shell, directory or identity setting changed.
    restart_notice: bool,
    /// An open menu: the `⋯` app menu or the terminal's context menu.
    menu: Option<OpenMenu>,
    /// The settings changed and the file has not been written; when they last changed.
    settings_dirty: Option<Instant>,
    persist: Persist,
    /// The app's config directory: where the shell-integration scripts live (D12).
    config_dir: Option<PathBuf>,
    /// This run's executable, as every tab is told it (`LUNETERM_BIN`).
    bin: String,
    /// This run's instance token (`LUNETERM_INSTANCE`); empty if the OS gave no randomness.
    instance: String,
    /// The tabs of the last run, restored when the window exists.
    startup_workspace: Option<Workspace>,
    /// What the files had wrong, shown once the first frame can show it.
    startup_notices: Vec<String>,
    guard: SaveGuard,
    /// The tabs changed since the workspace was last written.
    ws_dirty: bool,
    ws_last_write: Instant,
    /// The last tabs are closing as the app leaves: what was saved stands.
    ws_frozen: bool,
    /// The OS window's title as last set.
    window_title: String,
    /// The last frame's placement: the pointer maps through it.
    placed: Option<Placed>,
    /// The IME's composing text.
    preedit: Option<String>,
    paste: PasteState,
    /// An OSC 52 write waiting for the next frame.
    clipboard_write: Option<String>,
    shortcuts: AppShortcuts,
    /// The table asked when a Copy chord finds nothing selected: it has no Copy.
    shortcuts_no_copy: AppShortcuts,
    mods: ModifiersState,
    /// The pointer in logical px, `None` outside the window.
    pointer: Pointer,
    /// The link tooltip was hidden by a key, a press, a tab switch or the window losing focus; it
    /// stays hidden until the pointer moves.
    tip_hidden: bool,
    /// The tab-switch crossfade in progress, if any (`crate::fade`).
    fade: Fade,
    /// The workspace is being restored: its tab switches do not fade.
    restoring: bool,
    mouse: MouseState,
    /// A mouse press went to the program: so do its moves and its release.
    mouse_captured: bool,
    /// A press started a selection and the button is still down: moves drag it.
    selecting: bool,
    /// A press on a link with the link modifier held: a release without a drag opens it, a drag
    /// makes it a selection.
    pending_link: Option<PendingLink>,
    clicks: ClickCounter,
    toasts: Toasts,
    /// The notification toasts that are up (one per tab, three in all) and when each may take a
    /// press; learned from every drawn frame (`sync_toasts`).
    toast_slots: ToastSlots<ToastId, Rect>,
    /// Where the toasts were in the last drawn frame: the pointer is theirs there, not the
    /// terminal's.
    toast_rects: Vec<Rect>,
    /// When the pointer's latest primary press was: a toast button or an inbox row is judged at
    /// the press, not at the frame that reports the click (D14).
    press_clock: PressClock,
    /// The inbox rows the last frame drew and since when (a row press acts only on a still list).
    inbox_drawn: DrawnRows,
    wheel: WheelState,
    thumb_drag: Option<ThumbDrag>,
    bell: BellLimiter,
    /// Bells and program notifications, the one store behind the tab dots, the bell and the list.
    /// In memory only (D15).
    inbox: Inbox,
    /// When the bell's last ring started (the `start.elapsed()` clock): the next ring waits
    /// [`shell::RING_GAP`] after it (D19).
    inbox_ring: Option<Duration>,
    /// The inbox list is open (a popover under the header button). While it is, the toolkit has
    /// the keyboard and the pointer, like an open menu.
    inbox_open: bool,
    /// Messages that earned a toast in this pump; shown (or skipped by the slots) at its end.
    toast_candidates: Vec<ToastCandidate>,
    focused: bool,
    /// Efficiency mode in the background (UI thread only; every child creation runs inside its
    /// `lifted`).
    efficiency: Controller<HostOs>,
    /// The app's time origin (`RawInput.time` and `Repaint::At` count from it).
    start: Instant,
    blink: Blink,
    occluded: bool,
    shown: bool,
    exit_code: u8,
    /// `LUNETERM_TRACE_SWITCH=1`: where a tab switch's time goes; `None` (the default) costs one
    /// check per hook.
    trace: Option<Box<SwitchTrace>>,
    /// The present mode asked for (the knob or the default); the surface resolves it.
    present: PresentAsk,
    /// When the frames asked for are drawn (D4); its deadline is the only frame deadline.
    pacer: Pacer,
    /// The callback in progress handles discrete input (or is the post-frame section of a frame
    /// that took some): its asks draw at once.
    urgent_scope: bool,
    /// The update worker's handle (update.rs).
    update: Updater,
    /// The update worker's last state (Settings > About shows it).
    update_state: UpdateState,
    /// The 24 h check timer: runs only while Check for updates automatically is on and the copy
    /// can update itself (`sync_auto_check`).
    auto_check: AutoCheck,
    /// Which versions had their update toast in this run.
    update_toast_rule: ToastRule,
    /// The update toast that is up, if any.
    update_toast: Option<(ToastId, UpdateToastKind)>,
    /// When the update toast was shown and last moved: its buttons take a press only after the
    /// arm delay, like a notification toast's (a slot table of its own, with a stand-in tab).
    update_slots: ToastSlots<ToastId, Rect>,
    /// The user pressed Update on the toast: when the download is staged, a Restart toast follows.
    restart_toast_pending: bool,
    /// This version waits for its health confirm (plan D8: after the first frame is presented and
    /// the first shell spawned).
    health_pending: bool,
    /// The control listener (`luneterm ctl`); `None` when control is unavailable for this run.
    ctl: Option<Server>,
    /// Keyboard, paste and IME input is dropped until then: a control request just changed the
    /// active tab (D9).
    input_hold_until: Option<Instant>,
}

impl App {
    fn new(
        settings: Settings,
        proxy: EventLoopProxy<UserEvent>,
        persist: Persist,
        startup_workspace: Option<Workspace>,
        startup_notices: Vec<String>,
        config_dir: Option<PathBuf>,
    ) -> App {
        let efficiency = Controller::new(HostOs::default(), settings.efficiency_mode);
        App {
            config_dir,
            bin: spawn::own_binary(),
            instance: ctl::token::new().unwrap_or_default(),
            persist,
            startup_workspace,
            startup_notices,
            guard: SaveGuard::new(),
            ws_dirty: false,
            ws_last_write: Instant::now(),
            ws_frozen: false,
            settings,
            glass: false,
            win: None,
            stack: None,
            proxy,
            tabs: TabSet::new(),
            leaving: Vec::new(),
            pending_new: None,
            modal: None,
            page: None,
            restart_notice: false,
            menu: None,
            settings_dirty: None,
            window_title: DEFAULT_TITLE.to_owned(),
            placed: None,
            preedit: None,
            paste: PasteState::default(),
            clipboard_write: None,
            shortcuts: AppShortcuts::standard(cfg!(target_os = "macos")),
            shortcuts_no_copy: AppShortcuts::standard(cfg!(target_os = "macos")).without_copy(),
            mods: ModifiersState::empty(),
            pointer: Pointer::default(),
            tip_hidden: false,
            fade: Fade::default(),
            restoring: false,
            mouse: MouseState::default(),
            mouse_captured: false,
            selecting: false,
            pending_link: None,
            clicks: ClickCounter::default(),
            toasts: Toasts::default(),
            toast_slots: ToastSlots::new(),
            toast_rects: Vec::new(),
            press_clock: PressClock::default(),
            inbox_drawn: DrawnRows::default(),
            wheel: WheelState::default(),
            thumb_drag: None,
            bell: BellLimiter::default(),
            inbox: Inbox::new(),
            inbox_ring: None,
            inbox_open: false,
            toast_candidates: Vec::new(),
            focused: false,
            efficiency,
            start: Instant::now(),
            blink: Blink::default(),
            occluded: false,
            shown: false,
            exit_code: 0,
            trace: None,
            present: pacing::DEFAULT_PRESENT,
            pacer: Pacer::new(Cadence::hz(60)),
            urgent_scope: false,
            update: Updater::unavailable(update::Unavailable::NotInstalled),
            update_state: UpdateState::Unavailable(update::Unavailable::NotInstalled),
            auto_check: AutoCheck::default(),
            update_toast_rule: ToastRule::default(),
            update_toast: None,
            update_slots: ToastSlots::new(),
            restart_toast_pending: false,
            health_pending: false,
            ctl: None,
            input_hold_until: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, msg: &str) {
        eprintln!("{msg}");
        self.exit_code = 1;
        event_loop.exit();
    }

    /// Ends every session, waiting briefly for the threads so the children are killed before the
    /// process leaves. All are signalled first, so the wait is one deadline, not one per tab.
    fn shutdown(&mut self) {
        let mut sessions = std::mem::take(&mut self.leaving);
        sessions.extend(self.tabs.iter_mut().filter_map(|(_, t)| t.session.take()));
        for s in &sessions {
            s.begin_close();
        }
        let deadline = Instant::now() + CLOSE_WAIT;
        for s in sessions {
            if !s.close(deadline.saturating_duration_since(Instant::now())) {
                eprintln!("luneterm: a session's threads did not stop in time");
            }
        }
    }

    /// The terminal colours the window starts with and a new tab starts from.
    fn base_theme(&self) -> TermTheme {
        palettes::theme_of(&self.settings.palette)
    }

    /// Rebuilds the GPU stack after the device was lost; every row is drawn again.
    fn rebuild(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.win.as_ref().map(|w| w.window.clone()) else {
            return;
        };
        self.stack = None;
        let theme = self
            .tabs
            .active_item()
            .map_or_else(|| self.base_theme(), |t| t.theme.clone());
        let trace = self.trace.is_some();
        match Stack::build(
            event_loop,
            &window,
            &self.settings,
            self.glass,
            &theme,
            trace,
            self.present,
        ) {
            Ok(stack) => {
                self.stack = Some(stack);
                self.refresh_grid();
                if let Some(tab) = self.tabs.active_item_mut() {
                    tab.grid.mark_all_dirty();
                }
                // Direct, as urgent: the frame lost with the device is drawn again at once.
                window.request_redraw();
            }
            Err(e) => self.fail(event_loop, &format!("rebuild after device loss: {e}")),
        }
    }

    /// Shows the window once it has had its first frame (or could not have one).
    fn show_once(&mut self) {
        if !self.shown
            && let Some(win) = &self.win
        {
            win.window.set_visible(true);
            self.shown = true;
        }
    }

    /// Asks for a frame now, through the pacer: drawn at once when the callback is urgent or the
    /// grid slot is free, else at the pacer's deadline.
    fn request_redraw(&mut self) {
        self.ask_frame(Duration::ZERO);
    }

    /// Asks for a frame no earlier than `not_before` (app time). Nothing while the window is
    /// hidden (I4) or before it exists.
    fn ask_frame(&mut self, not_before: Duration) {
        if self.occluded {
            return;
        }
        let Some(win) = &self.win else { return };
        if self
            .pacer
            .ask(self.start.elapsed(), not_before, self.urgent_scope)
        {
            win.window.request_redraw();
        }
    }

    /// Sets the control flow from the pacer and the app's deadlines: the only setter after
    /// start-up (I1), called at the end of every callback.
    fn settle_flow(&self, event_loop: &ActiveEventLoop) {
        let flow = settled_flow(
            self.pacer.deadline(),
            self.pending_flow(),
            self.occluded,
            self.start,
        );
        // The efficiency controller's quiet-period deadline also holds while the window is
        // hidden (a minimized window is where it matters); `efficiency.tick` runs first in every
        // callback, so the instant is consumed when it comes and cannot spin the loop.
        let efficiency = self
            .efficiency
            .deadline()
            .map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
        // The update timer's deadline holds while the window is hidden too; `tick_update` runs
        // first in every callback, so the instant is consumed when it comes.
        let auto = self
            .auto_check
            .deadline()
            .map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
        event_loop.set_control_flow(merge_flow(merge_flow(flow, efficiency), auto));
    }

    /// Sets the frame grid's rate from the display the window is on now (D5); the surface is
    /// not touched.
    fn refresh_grid(&mut self) {
        if let (Some(win), Some(stack)) = (&self.win, &self.stack) {
            let hz = pacing::grid_hz(stack.mode, refresh_mhz(&win.window));
            self.pacer.set_rate(Cadence::hz(hz));
        }
    }

    /// Sends `command` to the session of tab `id`, whether or not it is the active one.
    fn send_to(&self, id: TabId, command: Command) {
        if let Some(session) = self.tabs.get(id).and_then(|t| t.session.as_ref()) {
            session.send(command);
        }
    }

    /// Sends `command` to the active tab's session.
    fn send(&self, command: Command) {
        if let Some(session) = self.tabs.active_item().and_then(|t| t.session.as_ref()) {
            session.send(command);
        }
    }

    fn scroll_view(&self) -> Option<ScrollView> {
        self.tabs.active_item().and_then(|t| t.scroll)
    }

    fn modes(&self) -> UpdateModes {
        self.tabs
            .active_item()
            .map_or_else(UpdateModes::default, |t| t.modes)
    }

    /// The modifier that makes a click open a link: Cmd on macOS, Shift on every OS.
    fn link_modifier_held(&self) -> bool {
        self.mods.shift_key() || (cfg!(target_os = "macos") && self.mods.super_key())
    }

    /// The cell under the pointer when it is over the grid.
    fn cell_under_pointer(&self) -> Option<(usize, usize)> {
        let pos = self.pointer.over()?;
        let ctx = self.mouse_ctx()?;
        if self.overlay_open() || !self.over_body(pos) {
            return None;
        }
        ctx.layout.cell_at(pos, ctx.cols, ctx.rows)
    }

    /// Whether a press of ours is down: the pointer leaving the window must not forget where it was.
    fn button_held(&self) -> bool {
        self.holds().any()
    }

    /// What the presses of ours that are down hold.
    fn holds(&self) -> Holds {
        Holds {
            selecting: self.selecting,
            captured: self.mouse_captured,
            thumb: self.thumb_drag.is_some(),
            link: self.pending_link.is_some(),
        }
    }

    /// The program's release of the press it has seen, at the pointer's last position. Nothing when
    /// there is no such press or no position to report at (the grid is not placed yet).
    fn send_program_release(&mut self) {
        let at = self.pointer.last().zip(self.mouse_ctx());
        let mods = terminal_mods(self.mods);
        if let Some(report) = program_release(&mut self.mouse, at, mods) {
            self.send(Command::Mouse(report));
        }
    }

    /// Tab `id` is being left: its selection drag ends in its engine and
    /// its program sees its button go up, so nothing stays held in a tab the pointer has left. The
    /// release is built from that tab's grid and modes; the tab need not be active any more.
    fn end_holds_of(&mut self, id: TabId) {
        let holds = self.holds();
        let at = self.pointer.last().zip(self.mouse_ctx_of(id));
        let mods = terminal_mods(self.mods);
        let mut commands = Vec::new();
        end_left_tab_holds(holds, &mut self.mouse, at, mods, &mut commands);
        for command in commands {
            self.send_to(id, command);
        }
    }

    /// A release while a menu, a dialog or the settings page covers the terminal. A press of ours
    /// that was down before it opened (a right press during a left drag opens the context menu, a
    /// shortcut opens the close dialog mid-drag) ends here, with no pointer position needed: the
    /// selection ends, the thumb and a pending link press are dropped (an overlay never opens a
    /// link), and the program sees its release. A release that belongs to a press the overlay
    /// itself took holds nothing and does nothing. Moves are ignored while it is open
    /// (`on_cursor_moved`), so the hold only waits for its release.
    fn release_under_overlay(&mut self, state: ElementState, button: MouseButton) {
        let program_button = match pointer_event(state, button) {
            Some(PointerEvent::Up(b)) => Some(b) == self.mouse.held_button(),
            _ => false,
        };
        let (after, ended) =
            release_under_overlay(self.holds(), button == MouseButton::Left, program_button);
        if !after.thumb {
            self.thumb_drag = None;
        }
        if !after.link {
            self.pending_link = None;
        }
        self.selecting = after.selecting;
        if ended.end_selection {
            self.send(Command::Select(SelectCmd::End { copy: false }));
        }
        if ended.release_to_program {
            self.send_program_release();
        }
        self.mouse_captured = after.captured;
    }

    /// Shows the link under the pointer as hovered (underlined, tooltip), and stops showing it when
    /// the pointer is off it, outside the window, dragging a selection or under an overlay. It
    /// reads the one row under the pointer. Redraws when it changed.
    fn refresh_hover(&mut self) {
        let at = (!self.selecting && !self.overlay_open())
            .then(|| self.cell_under_pointer())
            .flatten();
        if let Some(tab) = self.tabs.active_item_mut()
            && tab.set_link_hover(at)
        {
            self.request_redraw();
        }
    }

    /// After output: looks again at the pointer's row only when an update hid a hover that was
    /// shown (its cells are put back first), so a flood costs no scan and a link that is gone, or
    /// changed, under a still pointer leaves no stale underline or tooltip.
    fn refresh_hover_after_output(&mut self) {
        if self.tabs.active_item_mut().is_some_and(Tab::take_rehover) {
            self.refresh_hover();
        }
    }

    /// Hides the link tooltip until the pointer moves.
    fn hide_tip(&mut self) {
        let showing = self
            .tabs
            .active_item()
            .is_some_and(|t| t.hover_span().is_some());
        if !self.tip_hidden {
            self.tip_hidden = true;
            if showing {
                self.request_redraw();
            }
        }
    }

    /// Opens a link the user clicked: checked again here, whatever produced it.
    fn open_link(&mut self, uri: &str) {
        // The launcher is a child (or `ShellExecuteW` in this process): created unthrottled.
        match self
            .efficiency
            .lifted(Instant::now(), || links::open::open_url(uri))
        {
            Ok(()) => {}
            Err(e) => {
                // The reason, never the URL (its query can hold a token).
                match e {
                    OpenError::Rejected(why) => eprintln!("luneterm: link refused: {why:?}"),
                    OpenError::Failed(kind) => eprintln!("luneterm: link not opened: {kind:?}"),
                }
                self.toasts.push(
                    ToastSpec::new(ToastKind::Warning, e.message()),
                    self.start.elapsed(),
                );
                self.request_redraw();
            }
        }
    }

    fn has_selection(&self) -> bool {
        self.tabs
            .active_item()
            .is_some_and(|t| t.selection.is_some())
    }

    fn has_session(&self) -> bool {
        self.tabs.active_item().is_some_and(|t| t.session.is_some())
    }

    // ------------------------------------------------------------------- settings, menus, page

    /// Whether the keyboard and the pointer belong to the toolkit: a dialog, the settings page
    /// or an open menu is over the terminal.
    fn overlay_open(&self) -> bool {
        self.modal.is_some() || self.page.is_some() || self.menu.is_some() || self.inbox_open
    }

    /// Opens the settings page on `section` (the app menu's `About` opens the About section).
    fn open_page(&mut self, section: Section) {
        self.menu = None;
        self.restart_notice = false;
        self.page = Some(PageState::new(section, &self.settings));
        self.pointer_left_terminal();
        self.request_redraw();
    }

    /// Leaves the settings page; the notice goes with it.
    fn close_page(&mut self) {
        if self.page.take().is_some() {
            self.restart_notice = false;
            self.flush_settings(true);
            if let Some(tab) = self.tabs.active_item_mut() {
                tab.grid.mark_all_dirty();
            }
            self.request_redraw();
        }
    }

    /// What belonged to the terminal's pointer is dropped when something covers it.
    fn pointer_left_terminal(&mut self) {
        self.pending_link = None;
        self.thumb_drag = None;
        if std::mem::take(&mut self.selecting) {
            self.send(Command::Select(SelectCmd::End { copy: false }));
        }
        // The program's press is released to it, or it would see the button down for good.
        if std::mem::take(&mut self.mouse_captured) {
            self.send_program_release();
        }
    }

    /// The settings were changed (by the page): applies what each change does, queues the save.
    fn apply_settings(&mut self, new: Settings) {
        let new = new.clamped();
        if new == self.settings {
            return;
        }
        let old = std::mem::replace(&mut self.settings, new);
        self.settings_dirty = Some(Instant::now());
        for effect in effects(&old, &self.settings) {
            self.apply_effect(effect);
        }
        self.request_redraw();
    }

    /// One effect of a changed setting (plan M10 step 6's table).
    fn apply_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Glass => {
                if let Some(win) = &self.win {
                    self.glass = apply_material(&win.window, &self.settings);
                }
                if let Some(stack) = &mut self.stack {
                    stack.renderer.set_glass(self.glass, self.settings.theme);
                }
            }
            Effect::WindowOpacity => {
                // The theme only: the OS material is the Glass effect's, and does not change.
                if let Some(stack) = &mut self.stack {
                    stack.renderer.set_window_opacity(
                        self.settings.window_opacity,
                        self.glass,
                        self.settings.theme,
                    );
                }
            }
            Effect::MenuBlur => {
                if let Some(stack) = &mut self.stack {
                    stack.renderer.set_blur(self.settings.menu_blur.sigma());
                }
            }
            Effect::Motion => {
                if let Some(stack) = &mut self.stack {
                    stack.renderer.set_tier(self.settings.tier());
                }
            }
            Effect::Font => self.apply_font(),
            Effect::TextRendering => {
                if let Some(stack) = &mut self.stack {
                    stack
                        .renderer
                        .set_text_rendering(self.settings.text_rendering());
                }
            }
            Effect::Palette => self.apply_palette(),
            Effect::Scrollback => {
                let lines = self.settings.scrollback;
                self.send_all(|| Command::SetScrollback(lines));
            }
            Effect::OscClipboard => {
                let on = self.settings.osc_clipboard;
                self.send_all(|| Command::SetClipboardWrite(on));
            }
            Effect::OptionAsMeta =>
            {
                #[cfg(target_os = "macos")]
                if let Some(win) = &self.win {
                    use winit::platform::macos::WindowExtMacOS;
                    win.window.set_option_as_alt(crate::input::option_as_alt(
                        self.settings.option_as_meta,
                    ));
                }
            }
            // Read each frame (the strip's cue) and when a tab closes.
            Effect::Outline | Effect::ConfirmClose => {}
            // Read on each mouse-up that ends a selection (step 2).
            Effect::CopyOnSelect => {}
            // Nothing to do live: `pump` reads the field (step 5).
            Effect::OscNotifications => {}
            // Read per request (`control`).
            Effect::TabControl => {}
            Effect::Efficiency => self
                .efficiency
                .set_enabled(self.settings.efficiency_mode, Instant::now()),
            Effect::AutoUpdateCheck => self.sync_auto_check(),
            Effect::NextSpawn => self.restart_notice = self.page.is_some(),
        }
    }

    /// `make()` to every tab that has a session.
    fn send_all(&self, make: impl Fn() -> Command) {
        for (_, tab) in self.tabs.iter() {
            if let Some(session) = &tab.session {
                session.send(make());
            }
        }
    }

    /// New fonts: the renderer takes them, and every tab hears the new cell size and grid.
    fn apply_font(&mut self) {
        let (Some(win), Some(stack)) = (&self.win, &mut self.stack) else {
            return;
        };
        stack.renderer.set_fonts(&self.settings);
        let scale = win.window.scale_factor() as f32;
        let size = win.window.inner_size();
        let logical = (size.width as f32 / scale, size.height as f32 / scale);
        let geometry = stack.renderer.place(logical, scale).geometry();
        for (_, tab) in self.tabs.iter_mut() {
            tab.grid.mark_all_dirty();
            if geometry.cols > 0
                && geometry.rows > 0
                && let Some(session) = &tab.session
            {
                session.send(Command::Resize(geometry));
                tab.sent_geometry = Some(geometry);
            }
        }
    }

    /// A new palette: every tab's colours at once. A colour a child set itself stays (the
    /// session keeps it); the tab's own mirror follows in the session's next Update.
    fn apply_palette(&mut self) {
        let theme = palettes::theme_of(&self.settings.palette);
        let host = HostPalette::from_theme(&theme);
        let active = self.tabs.active();
        for (id, tab) in self.tabs.iter_mut() {
            match &tab.session {
                Some(session) => session.send(Command::SetTheme(Box::new(host.clone()))),
                None => {
                    tab.theme = theme.clone();
                    tab.grid.mark_all_dirty();
                    if Some(id) == active
                        && let Some(stack) = &mut self.stack
                    {
                        stack.renderer.set_term_theme(theme.clone());
                    }
                }
            }
        }
    }

    /// Writes the settings when they changed, a [`SETTINGS_DEBOUNCE`] after the last change
    /// (always when `force`).
    fn flush_settings(&mut self, force: bool) {
        let Some(since) = self.settings_dirty else {
            return;
        };
        if force || since.elapsed() >= SETTINGS_DEBOUNCE {
            self.settings_dirty = None;
            self.persist.save_settings(&self.settings);
        }
    }

    /// A menu row was picked.
    fn run_menu_action(&mut self, action: MenuAction, event_loop: &ActiveEventLoop) {
        match action {
            MenuAction::NewTab => self.new_tab_request(),
            MenuAction::Close => {
                if let Some(id) = self.tabs.active() {
                    self.request_close(vec![id], event_loop);
                }
            }
            MenuAction::CloseOthers => {
                if let Some(keep) = self.tabs.active() {
                    let ids = self
                        .tabs
                        .ids()
                        .into_iter()
                        .filter(|id| *id != keep)
                        .collect();
                    self.request_close(ids, event_loop);
                }
            }
            MenuAction::CloseAll => self.request_close(self.tabs.ids(), event_loop),
            MenuAction::Rename => {
                if let Some(id) = self.tabs.active() {
                    self.open_rename(id);
                }
            }
            // A duplicate starts where the source tab is now (a new tab already does).
            MenuAction::Duplicate => self.new_tab_request(),
            MenuAction::CopyCwd => self.copy_cwd(),
            MenuAction::Settings => self.open_page(Section::Appearance),
            MenuAction::About => self.open_page(Section::About),
            MenuAction::Copy => {
                if self.has_selection() {
                    self.send(Command::CopySelection);
                }
            }
            MenuAction::Paste => {
                if let Some(id) = self.tabs.active().filter(|_| self.has_session()) {
                    self.paste.request(id);
                }
            }
            MenuAction::SelectAll => self.send(Command::Select(SelectCmd::All)),
            MenuAction::Clear => self.send(Command::ClearBuffer),
        }
        self.request_redraw();
    }

    fn open_rename(&mut self, id: TabId) {
        if let Some(tab) = self.tabs.get(id) {
            self.modal = Some(ModalState::Rename {
                id,
                text: tab.strip_title().to_owned(),
                frames: 0,
            });
        }
    }

    /// Puts the active tab's working directory on the clipboard when the shell has reported one
    /// that is a plain absolute path (the text is the shell's: nothing else is copied).
    fn copy_cwd(&mut self) {
        let raw = self
            .tabs
            .active_item()
            .and_then(|t| t.cwd.as_deref())
            .filter(|raw| cwd::syntax(raw).is_some())
            .map(str::to_owned);
        if let Some(raw) = raw {
            self.clipboard_write = Some(raw);
        }
    }

    // ------------------------------------------------------------------------------- the tabs

    /// A new tab was asked for: in the active tab's directory when the shell reported one that
    /// checks out (off the UI thread, [`CwdCheck`]), else in the default one.
    fn new_tab_request(&mut self) {
        if self.pending_new.is_some() || self.win.is_none() {
            return;
        }
        let now = Instant::now();
        let raw = self
            .tabs
            .active_item()
            .and_then(|t| t.cwd.as_deref())
            .filter(|raw| cwd::syntax(raw).is_some())
            .map(str::to_owned);
        let check = raw.and_then(|raw| {
            let proxy = self.proxy.clone();
            CwdCheck::start(raw, now, move || {
                let _ = proxy.send_event(UserEvent::Wake);
            })
            .ok()
        });
        match check {
            Some(check) => {
                self.pending_new = Some(check);
                // The check's timeout reaches the event loop only through a frame's
                // `pending_flow`: every new-tab path (the strip, the menu and the shortcut) asks
                // for one, or a worker hung in `stat` would wake nothing.
                self.request_redraw();
            }
            None => {
                self.open_tab(None, None);
                self.workspace_changed(true);
            }
        }
    }

    /// Spawns a shell and adds its tab, active, at the window's grid size: a restored tab at the
    /// end, a new one where the user's new tab goes (after the active tab's group, D22). On
    /// failure the tab's grid tells why (D12) and it can be closed.
    /// `restored` is the saved tab this one comes back as: its launch directory and its name.
    /// The tab's id, or `None` when none was made (no window yet).
    fn open_tab(&mut self, dir: Option<PathBuf>, restored: Option<&TabState>) -> Option<TabId> {
        // A shell is never refused (only a program is).
        let Some(Ok(tab)) = self.new_tab(dir, restored, None) else {
            return None;
        };
        let failed = tab.failure.is_some();
        let before = self.tabs.active();
        let id = if restored.is_some() {
            self.tabs.push(tab)
        } else {
            self.tabs.insert_by_hand(tab, before)
        };
        if failed {
            // Ledger 12: while this tab shows its failure the workspace is not written.
            self.guard.spawn_failed(id);
        }
        self.active_changed(before);
        Some(id)
    }

    /// A tab with its child started at the window's grid size, not yet in the list: the user's
    /// shell, or `program` (a control tab's). `None` without a window; a refused program starts
    /// nothing. A child that fails to start leaves the tab with its failure (D12). The child is
    /// told the id [`TabSet::next_id`] gives, so the caller adds the tab before any other.
    fn new_tab(
        &mut self,
        dir: Option<PathBuf>,
        restored: Option<&TabState>,
        program: Option<&ProgramSpec>,
    ) -> Option<Result<Tab, Refused>> {
        let (Some(win), Some(stack)) = (&self.win, &mut self.stack) else {
            return None;
        };
        let scale = win.window.scale_factor() as f32;
        let size = win.window.inner_size();
        let logical = (size.width as f32 / scale, size.height as f32 / scale);
        let geometry = stack.renderer.place(logical, scale).geometry();
        let adapter = stack.renderer.gpu.adapter.get_info();

        let mut tab = Tab::empty(palettes::theme_of(&self.settings.palette));
        tab.focused = self.focused;
        tab.launch_dir = match restored {
            Some(saved) => saved.working_directory.clone(),
            None => dir
                .as_deref()
                .and_then(Path::to_str)
                .map(str::to_owned)
                .or_else(|| {
                    (!self.settings.default_cwd.is_empty())
                        .then(|| self.settings.default_cwd.clone())
                }),
        };
        // Started in `dir`: that is where the shell is until it says otherwise.
        tab.cwd = dir.as_deref().and_then(Path::to_str).map(str::to_owned);
        if let Some(title) = restored.and_then(|r| r.manual_title.as_deref()) {
            tab.rename(title);
        }
        // The shell (and conhost on Windows) is created unthrottled: the idle class is inherited.
        let tab_env = spawn::tab_env(&self.bin, &self.instance, self.tabs.next_id());
        let settings = &self.settings;
        let config_dir = self.config_dir.as_deref();
        let launched = self.efficiency.lifted(Instant::now(), || match program {
            None => spawn::launch_with(
                &PortablePtyBackend,
                settings,
                geometry.pty_size(),
                dir.as_deref(),
                config_dir,
                &tab_env,
            )
            .map_err(ProgramFailure::Spawn),
            Some(spec) => spawn::launch_program(
                &PortablePtyBackend,
                settings,
                geometry.pty_size(),
                spec,
                &tab_env,
            ),
        });
        match launched {
            Err(ProgramFailure::Refused(refused)) => return Some(Err(refused)),
            Err(ProgramFailure::Spawn(failure)) => {
                // A program's path is the caller's text: it is not logged.
                let what = if program.is_some() {
                    "a program tab"
                } else {
                    failure.program.as_str()
                };
                eprintln!(
                    "luneterm: failed to start a shell: {:?} ({what})",
                    failure.kind
                );
                tab.failure = Some(failure);
            }
            Ok((pty, started)) => {
                let pid = pty.process_id();
                let proxy = self.proxy.clone();
                let config = SessionConfig {
                    geometry,
                    scrollback: self.settings.scrollback as usize,
                    xtversion: spawn::xtversion(),
                    clipboard_write: self.settings.osc_clipboard,
                    windows_pty: spawn::windows_pty(),
                    palette: HostPalette::from_theme(&tab.theme),
                    focused: self.focused,
                };
                let waker = Box::new(move || {
                    let _ = proxy.send_event(UserEvent::Wake);
                });
                // Its threads need no lift: a new thread starts at the default QoS on macOS (not
                // its creator's, measured) and Windows has one priority class per process.
                match Session::start(pty, config, waker) {
                    Ok(session) => {
                        let shell = if program.is_some() {
                            "(program tab)".to_owned()
                        } else if started == "login shell" {
                            std::env::var("SHELL").unwrap_or(started)
                        } else {
                            started
                        };
                        eprintln!(
                            "luneterm: gpu {} ({:?}); shell {shell} (pid {}); grid {}x{}",
                            adapter.name,
                            adapter.backend,
                            pid.map_or_else(|| "?".to_owned(), |p| p.to_string()),
                            geometry.cols,
                            geometry.rows
                        );
                        tab.session = Some(session);
                        tab.sent_geometry = Some(geometry);
                        tab.pid = pid;
                    }
                    Err(e) => {
                        tab.failure = Some(SpawnFailure {
                            kind: e.kind(),
                            program: "session".to_owned(),
                        });
                        eprintln!("luneterm: session: {:?}", e.kind());
                    }
                }
            }
        }
        Some(Ok(tab))
    }

    /// The tabs of the last run come back: each starts in its live directory, else the one it was
    /// started in, else the default (the directories are checked on a thread of their own, since a
    /// check stats the disk), with the name the user gave it and in its group. With nothing to
    /// restore, one tab.
    fn start_tabs(&mut self) {
        let Some(workspace) = self.startup_workspace.take() else {
            self.open_tab(None, None);
            return;
        };
        let saved = workspace.tabs.clone();
        let (tx, rx) = mpsc::channel();
        let checked = saved.clone();
        let spawned = thread::Builder::new()
            .name("luneterm-restore".into())
            .spawn(move || {
                let _ = tx.send(restore_dirs(&checked, &|p| cwd::validated(p)));
            });
        let dirs = spawned
            .ok()
            .and_then(|_| rx.recv_timeout(RESTORE_CHECK_WAIT).ok())
            .unwrap_or_else(|| vec![None; saved.len()]);
        // Restoring a workspace is a batch of programmatic switches: none of them fades.
        self.restoring = true;
        let ids: Vec<Option<TabId>> = saved
            .iter()
            .zip(dirs)
            .map(|(state, dir)| self.open_tab(dir, Some(state)))
            .collect();
        let before = self.tabs.active();
        restore_layout(&mut self.tabs, &ids, &workspace);
        self.active_changed(before);
        self.restoring = false;
        // What was restored is what the file says: nothing to write yet.
        self.ws_dirty = false;
    }

    /// Something the workspace saves changed: the tabs, their order, the active one, a name, or a
    /// group (its tabs, colour or collapsed state). `user`: the user did it (not a shell dying, a
    /// directory report or a `ctl` request).
    fn workspace_changed(&mut self, user: bool) {
        if self.ws_frozen {
            return;
        }
        if user {
            self.guard.user_acted();
            self.persist.arm_workspace();
        }
        self.ws_dirty = true;
    }

    /// Writes the workspace when it changed, at most once a [`WORKSPACE_DEBOUNCE`] (always when
    /// `force`). Ledger 12: nothing goes out while the guard says a tab failed.
    fn flush_workspace(&mut self, force: bool) {
        if self.ws_frozen || !self.ws_dirty {
            return;
        }
        let now = Instant::now();
        if !force && now.saturating_duration_since(self.ws_last_write) < WORKSPACE_DEBOUNCE {
            return;
        }
        self.ws_dirty = false;
        self.ws_last_write = now;
        let infos = tab_infos(&self.tabs);
        let active = self
            .tabs
            .active()
            .and_then(|a| self.tabs.index_of(a))
            .unwrap_or(0);
        if let Some(ws) = snapshot(
            &infos,
            active,
            &self.guard,
            self.persist.workspace_unknown(),
        ) {
            self.persist.save_workspace(&ws);
        }
    }

    /// The last tabs are about to close with the app: write what is open now and stop writing, so
    /// closing them does not save an empty list (the next start brings them back).
    fn freeze_workspace(&mut self) {
        self.flush_workspace(true);
        self.ws_frozen = true;
    }

    /// The active tab changed from `before`: what belonged to the old tab's pointer and keyboard
    /// state is dropped, the sessions' visibility and focus follow, and the new tab's grid is
    /// drawn from scratch.
    fn active_changed(&mut self, before: Option<TabId>) {
        let now = self.tabs.active();
        if now == before {
            return;
        }
        if let (Some(t), Some(new)) = (&mut self.trace, now) {
            t.switched(new, Instant::now());
        }
        // Every way of leaving a tab comes through here. A tab that was closed has nobody to tell.
        if let Some(id) = before.filter(|id| self.tabs.get(*id).is_some()) {
            self.end_holds_of(id);
        }
        // The crossfade between the old tab's content and the new one's. The first tab and a
        // restored workspace do not fade; a tab that was closed has nothing to fade out, so the
        // new one fades in alone (Lunegit's `ApplyCarouselVisibility`).
        if let Some(new) = now
            && before.is_some()
            && !self.restoring
        {
            let old = before.filter(|id| self.tabs.get(*id).is_some());
            self.fade.switch(
                Instant::now(),
                fade::duration(self.settings.motion),
                old,
                new,
            );
            if let Some(tab) = old.and_then(|id| self.tabs.get_mut(id)) {
                // Its grid is drawn by a view of its own from here on: every row again.
                tab.grid.mark_all_dirty();
            }
        }
        self.preedit = None;
        self.mouse_captured = false;
        self.selecting = false;
        self.pending_link = None;
        self.tip_hidden = true;
        if let Some(tab) = before.and_then(|id| self.tabs.get_mut(id)) {
            tab.hide_link_hover();
        }
        self.clicks.reset();
        self.thumb_drag = None;
        self.mouse = MouseState::default();
        self.wheel = WheelState::default();
        self.blink = Blink::default();
        if let Some(tab) = before.and_then(|id| self.tabs.get_mut(id)) {
            tab.set_focus(false);
        }
        let focused = self.focused;
        if let Some(tab) = self.tabs.active_item_mut() {
            tab.set_focus(focused);
            tab.grid.mark_all_dirty();
            if let Some(stack) = &mut self.stack
                && stack.renderer.term_theme() != &tab.theme
            {
                stack.renderer.set_term_theme(tab.theme.clone());
            }
        }
        self.refresh_visibility();
        self.request_redraw();
    }

    /// Only the active tab of a shown window is visible; a hidden session's Updates come at most
    /// every 100 ms.
    fn refresh_visibility(&mut self) {
        let active = self.tabs.active();
        let occluded = self.occluded;
        for (id, tab) in self.tabs.iter_mut() {
            if let Some(session) = &tab.session {
                session.set_visible(!occluded && Some(id) == active);
            }
        }
    }

    /// Closes the tabs `ids` that are open, asking first when the setting and the tabs say so.
    fn request_close(&mut self, ids: Vec<TabId>, event_loop: &ActiveEventLoop) {
        let plan = plan_close(ids, self.settings.confirm_close, |id| {
            self.tabs.get(id).is_some_and(Tab::needs_confirm)
        });
        match plan {
            ClosePlan::Now(ids) => self.close_tabs(&ids, event_loop, true),
            ClosePlan::Ask { ids, n } => {
                self.modal = Some(ModalState::Confirm {
                    ids,
                    message: confirm_message(n),
                    dont_ask: false,
                });
                self.request_redraw();
            }
        }
    }

    /// Closes `ids` now. The last tab closing exits the app.
    fn close_tabs(&mut self, ids: &[TabId], event_loop: &ActiveEventLoop, user: bool) {
        let before = self.tabs.active();
        if ids.len() >= self.tabs.len() && self.tabs.ids().iter().all(|id| ids.contains(id)) {
            self.freeze_workspace();
        }
        for id in ids {
            let Some(tab) = self.tabs.close(*id) else {
                continue;
            };
            if user {
                self.guard.tab_closed(*id);
            }
            if let Some(session) = tab.session {
                if self.tabs.is_empty() {
                    // The app is leaving: `shutdown` closes it with a wait.
                    self.leaving.push(session);
                } else {
                    retire(session);
                }
            }
        }
        if self.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        self.drop_overlays_of_gone_tabs();
        self.workspace_changed(user);
        self.active_changed(before);
        self.request_redraw();
    }

    /// A dialog or a tab menu about tabs that are all gone has nothing to act on: it closes. One
    /// path for every way a tab goes without the user (its child exits, `ctl tab close`).
    fn drop_overlays_of_gone_tabs(&mut self) {
        let gone = match &mut self.modal {
            Some(ModalState::Rename { id, .. }) => self.tabs.get(*id).is_none(),
            Some(ModalState::Confirm { ids, .. }) => {
                ids.retain(|id| self.tabs.get(*id).is_some());
                ids.is_empty()
            }
            None => false,
        };
        if gone {
            self.modal = None;
        }
        if self
            .menu
            .and_then(|m| m.tab)
            .is_some_and(|id| self.tabs.get(id).is_none())
        {
            self.menu = None;
        }
    }

    // ------------------------------------------------------------------------------ control

    /// A `luneterm ctl` request: answered from the tabs (`ctl::handler`), then what it asks is
    /// done. One that is overdue or given up on is dropped unanswered: its client has been told
    /// "did not answer", so nothing may happen after it.
    fn control(&mut self, call: Call, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if event_loop.exiting() || !call.take(now) {
            return;
        }
        let ctx = Ctx {
            control_on: self.settings.tab_control,
            window: self.win.is_some() && self.stack.is_some(),
            version: spawn::VERSION,
            instance: &self.instance,
            now,
        };
        let before = self.tabs.active();
        let (reply, effects) =
            match control::handle(&mut self.tabs, call.request.clone(), call.cwd_ok, &ctx) {
                Outcome::Answer(reply, effects) => (reply, effects),
                Outcome::Spawn(spec) => match self.control_tab(&spec) {
                    Ok(tab) => {
                        let id = self.tabs.next_id();
                        let placed = control::place(&mut self.tabs, tab, &spec);
                        debug_assert!(self.tabs.get(id).is_some(), "the id its child was told");
                        // A shell tab is saved like any tab; a program tab is not (D15).
                        self.workspace_changed(false);
                        placed
                    }
                    Err(reply) => (reply, Vec::new()),
                },
            };
        // Answered before the effects: closing the last tab ends the app.
        call.answer(reply);
        self.active_changed(before);
        for effect in effects {
            match effect {
                ControlEffect::Close(ids) => self.close_tabs(&ids, event_loop, false),
                ControlEffect::Raise => {
                    // The normal window calls only: the OS may decline to bring the app to the
                    // front (CC5).
                    if let Some(win) = &self.win {
                        win.window.set_minimized(false);
                        win.window.focus_window();
                    }
                }
                // The handler made it active and expanded its group (both saved); the side pane
                // scrolls to the active row.
                ControlEffect::Reveal(_) => self.workspace_changed(false),
                ControlEffect::Hold => self.input_hold_until = Some(now + INPUT_HOLD),
                ControlEffect::Regrouped(id) => {
                    if self.tabs.get(id).is_some_and(|t| !t.program) {
                        self.workspace_changed(false);
                    }
                }
            }
        }
        self.request_redraw();
    }

    /// The tab a `tab new` asked for, its child started, or the reply that says why there is
    /// none. A child that does not start is no tab: the caller hears it (1).
    fn control_tab(&mut self, spec: &OpenSpec) -> Result<Tab, Reply> {
        let program = (!spec.argv.is_empty()).then(|| ProgramSpec {
            cwd: spec.cwd.clone(),
            argv: spec.argv.clone(),
        });
        let refused = |code: Code, reason: &str| Err(Reply::new(code, reason));
        match self.new_tab(Some(spec.cwd.clone()), None, program.as_ref()) {
            None => refused(Code::Failed, "Luneterm is starting"),
            Some(Err(Refused::BatchFile)) => refused(
                Code::Usage,
                "a .bat or .cmd program is refused (cmd.exe would parse its arguments again)",
            ),
            Some(Err(Refused::NotFound)) => refused(Code::Failed, "program not found"),
            Some(Err(Refused::Directory)) => {
                refused(Code::Usage, "--cwd cannot be handed to the shell")
            }
            Some(Ok(tab)) if tab.session.is_none() => {
                refused(Code::Failed, "the tab's program could not be started")
            }
            Some(Ok(mut tab)) => {
                tab.origin = Origin::Control;
                tab.program = program.is_some();
                if let Some(title) = &spec.title {
                    tab.rename(title);
                }
                Ok(tab)
            }
        }
    }

    /// Typed, pasted or composed input for tab `id`, unless the hold drops it.
    fn send_input_to(&mut self, id: TabId, command: Command) {
        if admit_input(self.input_hold_until, self.tabs.get_mut(id), Instant::now()) {
            self.send_to(id, command);
        }
    }

    /// [`App::send_input_to`] the active tab.
    fn send_input(&mut self, command: Command) {
        if let Some(id) = self.tabs.active() {
            self.send_input_to(id, command);
        }
    }

    /// What the user asked of the strip this frame.
    fn tab_event(&mut self, event: TabEvent, event_loop: &ActiveEventLoop) {
        let before = self.tabs.active();
        match event {
            TabEvent::Activate(key) => {
                self.tabs.activate(key.0);
                self.active_changed(before);
                self.workspace_changed(true);
            }
            TabEvent::Close(key) => self.request_close(vec![key.0], event_loop),
            TabEvent::New => self.new_tab_request(),
            TabEvent::Reorder { from, to } => {
                self.tabs.reorder_bar(from, to);
                self.workspace_changed(true);
            }
        }
        self.request_redraw();
    }

    /// A pick of the tab menu opened on `target`: it acts on that tab, not on the active one.
    fn run_tab_menu_action(
        &mut self,
        action: MenuAction,
        target: TabId,
        event_loop: &ActiveEventLoop,
    ) {
        // The tab may be gone since the menu opened.
        if self.tabs.get(target).is_none() {
            return;
        }
        let before = self.tabs.active();
        match menus::tab_op(action, target) {
            Some(TabOp::Close(id)) => self.request_close(vec![id], event_loop),
            Some(TabOp::CloseOthers(keep)) => {
                let ids = self
                    .tabs
                    .ids()
                    .into_iter()
                    .filter(|id| *id != keep)
                    .collect();
                self.request_close(ids, event_loop);
            }
            Some(TabOp::CloseAll) => self.request_close(self.tabs.ids(), event_loop),
            Some(TabOp::Rename(id)) => self.open_rename(id),
            Some(TabOp::ActivateThen(id, action)) => {
                self.tabs.activate(id);
                self.active_changed(before);
                self.run_menu_action(action, event_loop);
            }
            None => {}
        }
        self.request_redraw();
    }

    /// An app shortcut (D13).
    fn app_command(&mut self, cmd: AppCommand, event_loop: &ActiveEventLoop) {
        let before = self.tabs.active();
        match cmd {
            AppCommand::Paste => {
                if let Some(id) = self.tabs.active().filter(|_| self.has_session()) {
                    self.paste.request(id);
                    self.request_redraw();
                }
            }
            AppCommand::Copy => {
                // The table has this chord only while something is selected (`on_key`).
                if self.has_selection() {
                    self.send(Command::CopySelection);
                }
            }
            AppCommand::Scroll(scroll) => self.send(Command::Scroll(scroll)),
            AppCommand::NewTab => self.new_tab_request(),
            AppCommand::CloseTab => {
                if let Some(id) = self.tabs.active() {
                    self.request_close(vec![id], event_loop);
                }
            }
            AppCommand::NextTab => self.tabs.activate_step(1),
            AppCommand::PrevTab => self.tabs.activate_step(-1),
            AppCommand::SelectTab(i) => {
                self.tabs.activate_index(i);
            }
            AppCommand::Settings => {
                if self.page.is_some() {
                    self.close_page();
                } else {
                    self.open_page(Section::Appearance);
                }
            }
        }
        if self.tabs.active() != before {
            self.workspace_changed(true);
        }
        self.active_changed(before);
    }

    /// The dialog's answer: a confirm acts, a cancel closes it.
    fn resolve_modal(&mut self, out: ModalOut, event_loop: &ActiveEventLoop) {
        if !out.confirmed && !out.cancelled {
            return;
        }
        let modal = self.modal.take();
        if out.confirmed {
            match modal {
                Some(ModalState::Confirm { ids, dont_ask, .. }) => {
                    if dont_ask {
                        self.settings.confirm_close = false;
                        self.persist.save_settings(&self.settings);
                    }
                    self.close_tabs(&ids, event_loop, true);
                }
                Some(ModalState::Rename { id, text, .. }) => {
                    if let Some(tab) = self.tabs.get_mut(id) {
                        tab.rename(&text);
                        self.workspace_changed(true);
                    }
                }
                None => {}
            }
        }
        self.request_redraw();
    }

    // ------------------------------------------------------------------------- the sessions

    /// Takes every tab's published Update and applies it; a hidden tab's mirror must keep up
    /// too (I7). Closes the tabs whose child exited, starts a waiting new tab whose directory
    /// check answered. Returns whether a frame is worth drawing.
    fn pump(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let active = self.tabs.active();
        let mut seen = Vec::new();
        for (id, tab) in self.tabs.iter_mut() {
            if let Some(update) = tab.session.as_ref().and_then(Session::take_update) {
                seen.push((id, tab.apply(update)));
            }
        }
        // Output in any tab, hidden ones too: the Windows throttle waits for a quiet period.
        if seen.iter().any(|(_, fx)| fx.output) {
            self.efficiency.output(Instant::now());
        }
        let mut worth_a_frame = false;
        let mut exited = Vec::new();
        for (id, mut fx) in seen {
            worth_a_frame |= Some(id) == active || fx.chrome;
            if fx.palette
                && Some(id) == active
                && let (Some(stack), Some(tab)) = (&mut self.stack, self.tabs.get(id))
            {
                stack.renderer.set_term_theme(tab.theme.clone());
            }
            let now = self.start.elapsed();
            let heard = hear(
                &mut self.inbox,
                &mut self.toast_candidates,
                id,
                fx.bells,
                std::mem::take(&mut fx.messages),
                self.focused,
                Some(id) == active,
                self.settings.osc_notifications,
                now,
            );
            worth_a_frame |= heard.recorded;
            self.inbox_ring = ring_after(self.inbox_ring, heard.recorded, self.focused, now);
            if heard.attention
                && self.bell.due(Instant::now(), self.focused)
                && let Some(win) = &self.win
            {
                win.window
                    .request_user_attention(Some(UserAttentionType::Informational));
            }
            if let Some(text) = fx.clipboard_write
                && self.settings.osc_clipboard
            {
                self.clipboard_write = Some(text);
                worth_a_frame = true;
            }
            match fx.copy {
                Some(CopyResult::Text(text)) => {
                    self.clipboard_write = Some(text);
                    worth_a_frame = true;
                }
                Some(CopyResult::TooLarge) => {
                    let mib = MAX_COPY_BYTES >> 20;
                    self.toasts.push(
                        ToastSpec::new(
                            ToastKind::Warning,
                            format!("The selection is over {mib} MiB and was not copied."),
                        ),
                        self.start.elapsed(),
                    );
                    worth_a_frame = true;
                }
                None => {}
            }
            if fx.cwd {
                self.workspace_changed(false);
            }
            if fx.exited {
                exited.push(id);
            }
        }
        if !exited.is_empty() {
            // The child is gone: its tab closes, the others stay (the last one takes the app).
            for id in &exited {
                // A program tab is not saved, so how soon it ends says nothing about the list.
                if let Some(tab) = self.tabs.get(*id).filter(|t| !t.program) {
                    // A child that dies at its start is a failed start, not a decision.
                    self.guard.child_exited(*id, tab.started.elapsed());
                }
            }
            self.close_tabs(&exited, event_loop, false);
            worth_a_frame = true;
        }
        let done = self.pending_new.as_ref().map(|c| c.poll(Instant::now()));
        if let Some(CwdState::Done(dir)) = done {
            self.pending_new = None;
            self.open_tab(dir, None);
            self.workspace_changed(true);
            worth_a_frame = true;
        }
        self.show_notice_toasts();
        // The one place that keeps the inbox true (D13), after every change of the tab set.
        worth_a_frame |= self.settle_inbox();
        // An update hid the hovered link's cells; show the link again if it is still there.
        self.refresh_hover_after_output();
        worth_a_frame
    }

    /// `settle_inbox` on the app's own state, and the toasts that settling makes pointless: the
    /// toast of a tab that is closed or is the focused active tab leaves (what it said is read,
    /// or its tab is gone, so it can never switch to it).
    fn settle_inbox(&mut self) -> bool {
        let mut changed = settle_inbox(&mut self.inbox, &self.tabs, self.focused);
        let (tabs, focused) = (&self.tabs, self.focused);
        let active = tabs.active();
        let stale = self
            .toast_slots
            .drop_where(|tab| tabs.get(tab).is_none() || (focused && Some(tab) == active));
        let now = self.start.elapsed();
        for id in stale {
            self.toasts.dismiss(id, now);
            changed = true;
        }
        changed
    }

    /// Turns the pump's toast candidates into notification toasts through the slots: a new tab
    /// gets a toast (the oldest goes when three are up), a tab that has one gets its text replaced
    /// at most once a second, anything else only keeps its inbox row.
    fn show_notice_toasts(&mut self) {
        let candidates = std::mem::take(&mut self.toast_candidates);
        if candidates.is_empty() {
            return;
        }
        let Some(stack) = &self.stack else { return };
        let texts = shell::NoticeTexts::of(stack.renderer.strings());
        let accent = stack.renderer.theme().attention();
        let now = self.start.elapsed();
        for c in candidates {
            let Some(tab) = self.tabs.get(c.tab) else {
                continue;
            };
            // The tab's name as it is now (D17); the toast keeps it until its text is replaced.
            let source = tab.strip_title().to_owned();
            let (spec, extras) =
                shell::notice_toast(&source, &c.title, &c.body, &texts, accent, TOAST_LIFE);
            match self.toast_slots.admit(c.tab, now) {
                Admit::New { evict } => {
                    if let Some(old) = evict {
                        self.toasts.dismiss(old, now);
                        self.toast_slots.gone(old);
                    }
                    let id = self.toasts.push_with(spec, extras, now);
                    self.toast_slots.shown(id, c.tab, now);
                }
                Admit::Replace(id) => {
                    self.toasts.update_with(id, spec, extras);
                    self.toast_slots.shown(id, c.tab, now);
                }
                Admit::Skip => {}
            }
        }
    }

    /// A button of a notification toast was clicked, pressed at `now` (the press, not the frame
    /// that reports the release; the toast leaves with it). Dismiss only frees the slot. Go to tab
    /// switches to the toast's tab only when the toast was drawn and had stood still for the arm
    /// delay at the press (D14, `press_target`) and the tab
    /// still exists; the inbox rows of that tab turn read at the next settle. Called before
    /// `sync_toasts`, which would already have dropped the toast from the frame's list.
    fn on_toast_action(
        &mut self,
        id: ToastId,
        action: u64,
        now: Duration,
        event_loop: &ActiveEventLoop,
    ) {
        if self.update_toast.is_some_and(|(t, _)| t == id) {
            self.on_update_toast_action(id, action, now);
            return;
        }
        let target = self
            .toast_slots
            .press_target(id, now)
            .filter(|tab| self.tabs.get(*tab).is_some());
        self.toast_slots.gone(id);
        if action == shell::TOAST_GO_TO_TAB
            && let Some(tab) = target
        {
            self.tab_event(TabEvent::Activate(TabKey(tab)), event_loop);
        }
        self.request_redraw();
    }

    // ------------------------------------------------------------------------ updates

    /// Starts or stops the 24 h timer to match the setting and whether this copy can update
    /// itself (a copy that cannot never checks, so it never touches the network).
    fn sync_auto_check(&mut self) {
        let can = !matches!(self.update_state, UpdateState::Unavailable(_));
        self.auto_check
            .set_enabled(self.settings.auto_update_check && can, Instant::now());
    }

    /// Runs the automatic check when the timer says so. The worker ignores it while an operation
    /// runs.
    fn tick_update(&mut self, now: Instant) {
        if self.auto_check.take_due(now) {
            self.update.check(true);
        }
    }

    /// The worker's new state: About shows it; an automatic find of a version not yet toasted
    /// earns the toast; the Restart toast follows a download the Update toast started.
    fn on_update_state(&mut self, state: UpdateState) {
        self.update_state = state;
        self.sync_auto_check();
        let state = self.update_state.clone();
        // The Restart toast belongs to the download its Update toast started: a download that
        // ends any other way (stale, cancelled, failed) takes the wish with it, so a later
        // download started from About raises no Ready toast.
        if !keeps_restart_toast(&state) {
            self.restart_toast_pending = false;
        }
        match &state {
            UpdateState::Available { .. } => {
                if let Some(version) = self.update_toast_rule.wants(&state).map(str::to_owned)
                    && self.show_update_toast(UpdateToastKind::Available, &version)
                {
                    self.update_toast_rule.shown(&version);
                }
            }
            UpdateState::Ready(version) => {
                // The offer to download is answered: only the restart one stays.
                self.drop_update_toast(Some(UpdateToastKind::Available));
                if std::mem::take(&mut self.restart_toast_pending) {
                    self.show_update_toast(UpdateToastKind::Ready, version);
                }
            }
            UpdateState::Downloading(_) | UpdateState::Unavailable(_) => {
                self.drop_update_toast(Some(UpdateToastKind::Available));
            }
            _ => {}
        }
        self.request_redraw();
    }

    /// Puts up the update toast of `kind` (the one before it leaves). `false` when there is no
    /// window to draw it in yet.
    fn show_update_toast(&mut self, kind: UpdateToastKind, version: &str) -> bool {
        let Some(stack) = &self.stack else {
            return false;
        };
        let accent = stack.renderer.theme().attention();
        let (spec, extras) = shell::update_toast(kind, version, stack.renderer.strings(), accent);
        let now = self.start.elapsed();
        self.drop_update_toast(None);
        let id = self.toasts.push_with(spec, extras, now);
        self.update_slots.shown(id, UPDATE_SLOT_TAB, now);
        self.update_toast = Some((id, kind));
        true
    }

    /// Takes the update toast down, when it is of `kind` (any kind for `None`).
    fn drop_update_toast(&mut self, kind: Option<UpdateToastKind>) {
        if let Some((id, k)) = self.update_toast
            && kind.is_none_or(|want| want == k)
        {
            self.toasts.dismiss(id, self.start.elapsed());
            self.update_slots.gone(id);
            self.update_toast = None;
        }
    }

    /// A button of the update toast, pressed at `now`: Dismiss always works; Update and Restart
    /// only on a toast that was drawn and stood still for the arm delay at the press.
    fn on_update_toast_action(&mut self, id: ToastId, action: u64, now: Duration) {
        let armed = self.update_slots.press_target(id, now).is_some();
        self.update_slots.gone(id);
        self.update_toast = None;
        if armed {
            match action {
                shell::TOAST_UPDATE => {
                    if self.update.download() {
                        self.restart_toast_pending = true;
                    }
                }
                shell::TOAST_RESTART => {
                    self.update.restart();
                }
                _ => {}
            }
        }
        self.request_redraw();
    }

    /// An About button: a manual check (no toast), the download of the version found, or the
    /// restart that applies the staged one. The worker ignores a command while one runs.
    fn on_update_ask(&mut self, ask: UpdateAsk) {
        match ask {
            UpdateAsk::Check => {
                self.update.check(false);
            }
            UpdateAsk::Download => {
                self.update.download();
            }
            UpdateAsk::Restart => {
                self.update.restart();
            }
        }
    }

    /// Learns from the frame just drawn which toasts are up and where (D18): frees the slots of
    /// those that left, restarts the arm of those that appeared or moved, and keeps the places
    /// the pointer is kept off the terminal at.
    fn sync_toasts(&mut self, now: Duration) {
        let toasts = &self.toasts;
        self.toast_slots
            .sync(toasts.live(), now, |id| toasts.is_live(id));
        let freed = self
            .update_slots
            .sync(toasts.live(), now, |id| toasts.is_live(id));
        if self.update_toast.is_some_and(|(id, _)| freed.contains(&id)) {
            self.update_toast = None;
        }
        let rects: Vec<Rect> = toasts.live().iter().map(|(_, r)| *r).collect();
        if rects != self.toast_rects {
            self.toast_rects = rects;
            // A toast appearing under a still pointer hides the link hover there, and the other
            // way round.
            self.refresh_hover();
        }
    }

    /// When a waiting new tab gives up on its directory check.
    fn pending_flow(&self) -> ControlFlow {
        let check = self
            .pending_new
            .as_ref()
            .map_or(ControlFlow::Wait, |c| ControlFlow::WaitUntil(c.deadline()));
        // The workspace write waits for its debounce.
        let write = if self.ws_dirty && !self.ws_frozen {
            ControlFlow::WaitUntil(self.ws_last_write + WORKSPACE_DEBOUNCE)
        } else {
            ControlFlow::Wait
        };
        let settings = self.settings_dirty.map_or(ControlFlow::Wait, |t| {
            ControlFlow::WaitUntil(t + SETTINGS_DEBOUNCE)
        });
        merge_flow(merge_flow(check, write), settings)
    }

    // ----------------------------------------------------------------------------- the frame

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.occluded {
            return;
        }
        // The frame serves every ask made so far; it starts in the pacer before the pump, so an
        // ask the pump makes is the next frame's.
        self.pacer.frame_started(self.start.elapsed());
        if let Some(t) = &mut self.trace {
            t.taken = None;
            t.frame = FrameStamps {
                start: Some(Instant::now()),
                ..FrameStamps::default()
            };
        }
        if self
            .stack
            .as_ref()
            .is_some_and(|s| s.renderer.gpu.is_lost())
        {
            self.rebuild(event_loop);
            return;
        }
        self.pump(event_loop);
        if let Some(t) = &mut self.trace {
            t.frame.pump = Some(Instant::now());
        }
        self.flush_workspace(false);
        self.flush_settings(false);
        if event_loop.exiting() || self.tabs.is_empty() {
            return;
        }
        // Nothing between the pump and the frame changes the tabs, but the frame is what the user
        // sees: it never shows a row for a closed tab or an unread one for the tab in view.
        self.settle_inbox();
        // The list closes with its last entry, and when a dialog, the page or a menu opens over it.
        if self.inbox.is_empty()
            || self.modal.is_some()
            || self.page.is_some()
            || self.menu.is_some()
        {
            self.inbox_open = false;
        }
        // The hovered link: underlined, and while the modifier is held a hand. (Read before the
        // window and the renderer are borrowed.)
        let overlay = self.overlay_open();
        let hover_id = self.tabs.active_item().map_or(0, |t| t.hover_id);
        let link_hand = !overlay && hover_id != 0 && self.link_modifier_held();
        // The tab-switch crossfade at this instant: drawn while it runs, and asked for again only
        // while it runs (an idle window stays idle).
        let (alphas, fading) = {
            let tabs = &self.tabs;
            self.fade.tick(Instant::now(), |id| tabs.get(id).is_some())
        };
        let (Some(win), Some(stack)) = (self.win.as_mut(), self.stack.as_mut()) else {
            return;
        };
        let before_acquire = self.trace.is_some().then(Instant::now);
        let acquired = stack.surface.acquire(&stack.renderer.gpu);
        if let (Some(t), Some(before)) = (&mut self.trace, before_acquire) {
            t.acquire_waited(before.elapsed());
        }
        let frame = match acquired {
            Ok(Acquired::Frame(frame)) => frame,
            Ok(Acquired::Skip) => {
                // Nothing to draw and nothing to ask for: the platform redraws when it can.
                self.pacer.clear();
                self.show_once();
                return;
            }
            Ok(Acquired::Redraw) => {
                // Direct, as urgent: the surface was reconfigured, draw again at once.
                win.window.request_redraw();
                return;
            }
            Err(e) => {
                self.fail(event_loop, &e);
                return;
            }
        };
        if let Some(t) = &mut self.trace {
            t.frame.acquire = Some(Instant::now());
            t.frame.fading = fading;
        }
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let size = stack.surface.size();
        let scale = win.window.scale_factor() as f32;
        let input = win.translator.take(self.start.elapsed());
        if let Some(t) = &mut self.trace {
            // The press this frame takes is what a switch it asks for started from.
            t.taken = t.pending.take();
            t.press_taken();
        }

        // The clipboard's answer to a paste goes to the tab that asked, as a paste, nowhere else.
        for event in &input.events {
            if let Some((id, text)) = self.paste.answer(event)
                && let Some(tab) = self.tabs.get_mut(id)
                && admit_input(self.input_hold_until, Some(&mut *tab), Instant::now())
                && let Some(session) = &tab.session
            {
                session.send(Command::Paste(text));
            }
        }

        // Where things are in this frame; a changed grid size or cell size is the active
        // session's news (a hidden tab hears it when it is activated).
        let placed = stack.renderer.place(input.size, input.scale);
        self.placed = Some(placed);
        let geometry = placed.geometry();
        if let Some(tab) = self.tabs.active_item_mut() {
            if geometry.cols > 0
                && geometry.rows > 0
                && tab.sent_geometry != Some(geometry)
                && let Some(session) = &tab.session
            {
                session.send(Command::Resize(geometry));
                tab.sent_geometry = Some(geometry);
            }
            if let Some(failure) = &tab.failure {
                let (cols, rows) = (placed.placement.cols, placed.placement.rows);
                if (tab.grid.cols(), tab.grid.rows()) != (cols, rows) {
                    tab.grid = failure_grid(cols, rows, failure);
                }
                tab.cursor.visible = false;
            }
        }

        // The strip's tabs and the window's title, from the tabs as they are now.
        let active_tab = self.tabs.active();
        // The cue (D2): an unread entry in a tab that is not the active one, focused or not.
        let titles: Vec<(TabId, String, Option<Tone>, bool)> = self
            .tabs
            .iter()
            .map(|(id, t)| {
                let attention = self.inbox.tab_unread(id) > 0 && Some(id) != active_tab;
                (id, t.strip_title().to_owned(), t.progress_tone(), attention)
            })
            .collect();
        let strip: Vec<StripTab<'_>> = titles
            .iter()
            .map(|(id, title, progress, attention)| StripTab {
                id: *id,
                title,
                progress: *progress,
                attention: *attention,
            })
            .collect();
        if let Some(t) = &mut self.trace {
            t.frame.slot0 = active_tab;
        }
        let label = self
            .tabs
            .active_item()
            .and_then(|t| t.label())
            .map(str::to_owned);
        let os_title = label.as_deref().unwrap_or(DEFAULT_TITLE);
        if self.window_title != os_title {
            self.window_title = os_title.to_owned();
            win.window.set_title(os_title);
        }
        let modal = match &mut self.modal {
            Some(ModalState::Confirm {
                message, dont_ask, ..
            }) => Some(Modal::Confirm { message, dont_ask }),
            Some(ModalState::Rename { text, frames, .. }) => {
                let focus = *frames == 1;
                *frames = frames.saturating_add(1);
                Some(Modal::Rename { text, focus })
            }
            None => None,
        };

        let macos = cfg!(target_os = "macos");
        // A tab menu closes with its tab.
        if self
            .menu
            .and_then(|m| m.tab)
            .is_some_and(|id| self.tabs.get(id).is_none())
        {
            self.menu = None;
        }
        // The inbox: its rows (while the list is open) from the sanitised entries, the tab names as
        // they are now (D17), and the tab each row belongs to.
        let inbox_now = input.time;
        let inbox_rows: Vec<(TabId, shell::RowText)> = if self.inbox_open {
            let tabs = &self.tabs;
            shell::inbox_rows(&self.inbox, inbox_now, stack.renderer.strings(), |id| {
                tabs.get(id).map(|t| t.strip_title().to_owned())
            })
        } else {
            Vec::new()
        };
        let inbox_message_rows: Vec<_> = inbox_rows.iter().map(|(_, r)| r.row()).collect();
        // The rows' ids, in the same order (`inbox_rows` maps every entry, in order).
        let inbox_ids: Vec<u64> = if self.inbox_open {
            self.inbox.entries().map(|n| n.id).collect()
        } else {
            Vec::new()
        };
        let inbox_unread = self.inbox.unread();
        let (inbox_list_name, inbox_title, inbox_clear_all) = {
            let strings = stack.renderer.strings();
            (
                strings.tr("inbox.name").to_owned(),
                strings.tr("inbox.title").to_owned(),
                strings.tr("inbox.clearAll").to_owned(),
            )
        };
        let inbox_name = shell::inbox_button_name(&inbox_list_name, inbox_unread);
        let menu_model = self
            .menu
            .map(|m| (m, MenuModel::new(m.kind, stack.renderer.strings(), macos)));
        let menu_name = menu_model
            .as_ref()
            .map(|(_, m)| stack.renderer.strings().tr(m.name_key()).to_owned());
        let (busy, outline_cue, busy_tone) = {
            let cue = self.settings.busy_cue();
            let active = self.tabs.active_item();
            let busy = active.is_some_and(|t| t.busy(self.settings.outline_full_screen));
            (busy, cue, active.and_then(Tab::progress_tone))
        };
        // The hovered link's tooltip hangs from the link's cells and is shown only when nothing
        // covers the terminal and no key, press, tab switch or focus loss hid it since the
        // pointer last moved.
        let tip: Option<(Rect, String, u64)> = (!overlay && !self.tip_hidden)
            .then(|| {
                let span = self.tabs.active_item()?.hover_span()?;
                let url = links::tip_url(&span.uri)?;
                let rect =
                    placed
                        .grid_layout()
                        .cell_rect(span.row, span.start, span.end + 1 - span.start);
                let mut h = DefaultHasher::new();
                (span.row, span.start, span.end, &span.uri).hash(&mut h);
                Some((rect, url, h.finish()))
            })
            .flatten();
        let mut edited = self.settings.clone();
        // The active tab and the ones fading out under it, borrowed together.
        let mut active_tab_ref = None;
        let mut fading_grids: Vec<FadingGrid<'_>> = Vec::new();
        for (id, t) in self.tabs.iter_mut() {
            if Some(id) == active_tab {
                active_tab_ref = Some(t);
            } else if let Some((_, alpha)) = alphas.outgoing.iter().find(|(i, _)| *i == id) {
                fading_grids.push(FadingGrid {
                    id,
                    grid: &mut t.grid,
                    cursor: t.cursor,
                    theme: &t.theme,
                    alpha: *alpha,
                });
            }
        }
        let Some(tab) = active_tab_ref else {
            return;
        };
        let cursor = tab.cursor;
        let ranges: Vec<SelectionRange> = tab
            .selection
            .zip(tab.scroll)
            .and_then(|(view, scroll)| {
                view_range(&view, scroll.viewport_top, tab.grid.rows(), tab.grid.cols())
            })
            .into_iter()
            .collect();
        let next_blink = self.blink.tick(
            Instant::now(),
            input.focused && cursor.blink && cursor.visible,
        );
        let content = Content {
            title: label.as_deref(),
            tabs: &strip,
            active_tab,
            modal,
            maximized: win.window.is_maximized(),
            grid: &mut tab.grid,
            selection: &ranges,
            hover_link: if overlay { 0 } else { tab.hover_id },
            link_hand,
            link_tip: tip.as_ref().map(|(rect, url, key)| LinkTip {
                rect: *rect,
                url,
                key: *key,
            }),
            toasts: Some(&mut self.toasts),
            cursor,
            blink_on: self.blink.on(),
            scroll: tab.scroll,
            chrome: HOST,
            ime: tab.session.is_some(),
            preedit: self.preedit.as_deref(),
            clipboard_write: self.clipboard_write.take(),
            clipboard_read: self.paste.take_read(),
            page: self.page.as_mut().map(|state| PageView {
                state: {
                    state.update = self.update_state.clone();
                    state
                },
                settings: &mut edited,
                restart: self.restart_notice,
                focused: input.focused,
                system_fonts: &stack.system_fonts,
            }),
            menu: menu_model.as_ref().map(|(m, model)| MenuDraw {
                rows: model.rows(),
                place: m.place,
                name: menu_name.as_deref().unwrap_or(""),
            }),
            inbox: (!self.inbox.is_empty()).then(|| InboxDraw {
                button: InboxButton {
                    name: &inbox_name,
                    unread: inbox_unread > 0,
                    ring: ring_for_frame(self.inbox_ring, inbox_unread, input.focused, input.time),
                    open: self.inbox_open,
                },
                list: self.inbox_open.then(|| InboxList {
                    name: &inbox_list_name,
                    title: &inbox_title,
                    clear_all: &inbox_clear_all,
                    rows: &inbox_message_rows,
                }),
            }),
            busy: busy.then_some(outline_cue).flatten(),
            busy_tone: busy_tone.unwrap_or_default(),
            grid_alpha: alphas.incoming,
            fading: fading_grids,
        };
        let drawn = match stack.renderer.draw(&input, content, &view, size) {
            Ok(d) => d,
            Err(e) => {
                self.fail(event_loop, &format!("render: {e}"));
                return;
            }
        };
        let mut out = drawn.output;
        let (tab_events, modal_out) = (drawn.tab_events, drawn.modal);
        let (page_out, menu_out, overflow) = (drawn.page, drawn.menu, drawn.overflow);
        let (inbox_clicked, inbox_out) = (drawn.inbox_clicked, drawn.inbox);
        let tab_context = drawn.tab_context;
        // Closing the window is closing every tab: it asks like they do.
        let close_asked = out
            .commands
            .iter()
            .any(|c| matches!(c, WindowCommand::Close));
        out.commands.retain(|c| !matches!(c, WindowCommand::Close));
        if win.output.apply(&win.window, &out, scale) {
            event_loop.exit();
        }
        // A write goes to the OS now; a read is answered with the next frame's input.
        win.clipboard.service(&out, &mut win.translator);
        let clipboard_read = out
            .clipboard
            .iter()
            .any(|r| matches!(r, ClipboardRequest::Read));
        win.window.pre_present_notify();
        stack.surface.present(&stack.renderer.gpu, frame);
        if let Some(t) = &mut self.trace {
            t.frame.present = Some(Instant::now());
            t.frame.phases = stack.renderer.phases().copied();
        }
        let Stack { renderer, .. } = stack;
        renderer.gpu.end_frame(&mut renderer.text);

        // The next frame, through the pacer: urgent only right after a frame that took discrete
        // input (the click's tab switch, a menu opening), and then only for an ask due now (a
        // timer ahead, the blink or a toolkit `At`, stays paced); the window_event wrapper
        // resets it. Every time-based ask is re-derived here, each frame (I3).
        self.urgent_scope = pacing::follow_up(&input.events);
        match out.repaint {
            Repaint::Idle => {}
            Repaint::Now => self.ask_frame(Duration::ZERO),
            Repaint::At(t) => self.ask_frame(t),
        }
        // A read is answered with the next frame's input.
        if clipboard_read || fading {
            self.ask_frame(Duration::ZERO);
        }
        if let Some(b) = next_blink {
            self.ask_frame(b.saturating_duration_since(self.start));
        }

        // The toasts first: a pressed button's toast is already gone from this frame's list, so
        // its slot is read before the list frees it.
        // A click is judged at its press (the toolkit reports it on the release).
        let press = self.press_clock.moment(input.time);
        let toast_acted = drawn.toast.action.is_some();
        if let Some((id, action)) = drawn.toast.action {
            self.on_toast_action(id, action, press, event_loop);
        }
        self.sync_toasts(input.time);
        // Plan D8: a version just updated to confirms its health once a frame is on screen and
        // a shell runs (lunepack rolls it back otherwise).
        if self.health_pending && self.tabs.iter().any(|(_, t)| t.session.is_some()) {
            self.health_pending = false;
            update::confirm_healthy();
        }

        // What the frame asked of the page, the menu, the tabs and the dialog, now that the
        // frame is out.
        if edited != self.settings {
            self.apply_settings(edited);
        }
        if let Some(out) = page_out {
            if let Some(ask) = out.update {
                self.on_update_ask(ask);
            }
            if out.back {
                self.close_page();
            } else if out.restart_action {
                self.close_page();
                self.new_tab_request();
            }
        }
        if let Some(r) = menu_out {
            let action = r.picked.and_then(|i| menu_model.as_ref()?.1.action_at(i));
            let target = menu_model.as_ref().and_then(|(m, _)| m.tab);
            if r.picked.is_some() || r.dismissed {
                self.menu = None;
            }
            if let Some(action) = action {
                match target {
                    Some(tab) => self.run_tab_menu_action(action, tab, event_loop),
                    None => self.run_menu_action(action, event_loop),
                }
            }
        }
        let row_acted = inbox_out.as_ref().is_some_and(|o| o.clicked.is_some());
        self.on_inbox(
            inbox_clicked,
            inbox_out,
            &inbox_ids,
            press,
            inbox_now,
            event_loop,
        );
        self.inbox_drawn.drawn(&inbox_ids, input.time);
        if toast_acted || row_acted {
            self.press_clock.clear();
        }
        self.press_clock.frame_done();
        if let Some(ctx) = tab_context {
            self.menu = Some(OpenMenu {
                kind: MenuKind::Tab {
                    closable: TAB_CLOSABLE,
                },
                place: MenuPlace::At(ctx.at),
                tab: Some(ctx.id),
            });
            self.request_redraw();
        }
        if let Some(rect) = overflow {
            self.menu = Some(OpenMenu {
                kind: MenuKind::App,
                place: MenuPlace::Anchor {
                    rect,
                    side: Side::Bottom,
                    align: PlaceAlign::End,
                    gap: 4.0,
                },
                tab: None,
            });
            self.request_redraw();
        }
        if let Some(out) = modal_out {
            self.resolve_modal(out, event_loop);
        } else {
            if close_asked && self.modal.is_none() {
                self.request_close(self.tabs.ids(), event_loop);
            }
            for event in tab_events {
                self.tab_event(event, event_loop);
            }
        }
        self.urgent_scope = false;
        if let Some(line) = self.trace.as_mut().and_then(|t| t.frame_done()) {
            eprintln!("{line}");
        }
        self.show_once();
        if self
            .stack
            .as_ref()
            .is_some_and(|s| s.renderer.gpu.is_lost())
        {
            self.rebuild(event_loop);
        }
    }

    /// What the frame's inbox button and list asked. `ids` are the rows this frame built, in
    /// order; `press` is when the click was pressed. A press on the button toggles the list; a row
    /// press activates the tab of the row the previous frame drew at that index (the strip-click
    /// path) and closes the list, only when that list had stood still for the arm delay at the press
    /// and this frame changes nothing in it (D14, `DrawnRows::press_target`); otherwise the press
    /// does nothing and the list stays. The activated tab's rows turn read at the next settle. Clear all empties the inbox; Escape, a press outside
    /// and a focus loss closed it in the toolkit already.
    fn on_inbox(
        &mut self,
        clicked: bool,
        out: Option<shell::InboxOut>,
        ids: &[u64],
        press: Duration,
        now: Duration,
        event_loop: &ActiveEventLoop,
    ) {
        let was_open = self.inbox_open;
        let mut open = inbox_open_after(was_open, clicked, out.as_ref());
        if let Some(out) = out {
            if out.clear_all {
                self.inbox.clear();
            } else if let Some(i) = out.clicked {
                match self
                    .inbox_drawn
                    .press_target(i, press, ids)
                    .and_then(|id| self.inbox.tab_of(id))
                    .filter(|tab| self.tabs.get(*tab).is_some())
                {
                    Some(tab) => self.tab_event(TabEvent::Activate(TabKey(tab)), event_loop),
                    // Too soon after the rows changed: the press does nothing, the list stays.
                    None if !clicked && out.dismissed.is_none() => open = was_open,
                    None => {}
                }
            }
        }
        if open && !was_open {
            // The list covers the terminal: what belonged to its pointer is dropped, a menu closes.
            self.menu = None;
            self.pointer_left_terminal();
        }
        self.inbox_open = open && !self.inbox.is_empty();
        if self.inbox_open != was_open {
            self.request_redraw();
        }
        // The age labels change on the minute (the hour, the day): one frame for the next change.
        if self.inbox_open
            && let Some(next) = self
                .inbox
                .entries()
                .map(|n| notices::next_change(now.saturating_sub(n.at)))
                .min()
        {
            self.ask_frame(now + next);
        }
    }

    // ----------------------------------------------------------------------------- keyboard, IME

    /// A key event (D4): the shortcut table, then the key router; the translator never sees it
    /// (unless a dialog is open: then the toolkit takes the keyboard, see `window_event`).
    fn on_key(&mut self, event: &KeyEvent, synthetic: bool, event_loop: &ActiveEventLoop) {
        let input = key_input(event, self.mods, synthetic);
        if input.pressed && !is_modifier_key(&input.logical) {
            self.hide_tip();
        }
        let route_with = |shortcuts: &AppShortcuts| {
            let ctx = RouteCtx {
                macos: cfg!(target_os = "macos"),
                option_as_meta: self.settings.option_as_meta,
                keymap: keymap(),
                shortcuts,
            };
            route_key(&input, &ctx)
        };
        let mut route = route_with(&self.shortcuts);
        // A Copy chord with nothing selected is no copy: the key goes where it would have gone
        // without the shortcut (the terminal's Ctrl+Shift+C; nothing for Cmd+C), as in M9.
        if route == KeyRoute::Shortcut(AppCommand::Copy) && !self.has_selection() {
            route = route_with(&self.shortcuts_no_copy);
        }
        let live = self.has_session();
        match route {
            // Tab shortcuts work on a tab whose shell never started too: it can be closed.
            KeyRoute::Shortcut(cmd) => self.app_command(cmd, event_loop),
            KeyRoute::Terminal(key) if live => {
                self.typed();
                self.send_input(Command::Key(key));
            }
            KeyRoute::Text(text) if live => {
                self.typed();
                self.send_input(Command::Text(text));
            }
            _ => {}
        }
    }

    /// A key while the settings page is open. Returns whether the app took it (Escape with no
    /// list open, or the settings chord): the page closes and the toolkit never sees it.
    fn page_key(&mut self, event: &KeyEvent, synthetic: bool) -> bool {
        let input = key_input(event, self.mods, synthetic);
        if !input.pressed || input.repeat {
            return false;
        }
        let list_open = self.page.as_ref().is_some_and(|p| p.combo.is_some());
        let escape =
            input.logical == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape);
        let ctx = RouteCtx {
            macos: cfg!(target_os = "macos"),
            option_as_meta: self.settings.option_as_meta,
            keymap: keymap(),
            shortcuts: &self.shortcuts,
        };
        let chord = route_key(&input, &ctx) == KeyRoute::Shortcut(AppCommand::Settings);
        if (escape && !list_open) || chord {
            self.close_page();
            return true;
        }
        false
    }

    /// Typing shows the cursor: the blink starts over.
    fn typed(&mut self) {
        self.blink = Blink::default();
        self.request_redraw();
    }

    /// An input-method event (D6): composing text is drawn at the cursor, a commit is typed.
    fn on_ime(&mut self, ime: &Ime) {
        match ime {
            Ime::Preedit(text, _) => {
                self.preedit =
                    (!text.is_empty()).then(|| text.chars().take(MAX_PREEDIT_CHARS).collect());
                self.request_redraw();
            }
            Ime::Commit(text) => {
                self.preedit = None;
                if !text.is_empty() && self.has_session() {
                    self.typed();
                    self.send_input(Command::Text(text.clone()));
                }
            }
            Ime::Disabled => {
                self.preedit = None;
                self.request_redraw();
            }
            Ime::Enabled => {}
        }
    }

    // ----------------------------------------------------------------------------- pointer

    fn mouse_ctx(&self) -> Option<MouseCtx> {
        self.mouse_ctx_of(self.tabs.active()?)
    }

    /// The pointer mapping for tab `id`: the grid's place is shared, its size and modes are the
    /// tab's own.
    fn mouse_ctx_of(&self, id: TabId) -> Option<MouseCtx> {
        let placed = self.placed?;
        let tab = self.tabs.get(id)?;
        Some(MouseCtx {
            layout: placed.grid_layout(),
            cols: tab.grid.cols(),
            rows: tab.grid.rows(),
            pixel_reports: tab.modes.mouse_encoding == MouseEncoding::SgrPixels,
        })
    }

    fn tracking(&self) -> bool {
        self.has_session() && self.modes().mouse_tracking != MouseTracking::None
    }

    fn over_body(&self, pos: (f32, f32)) -> bool {
        self.placed
            .is_some_and(|p| terminal_hit(p.layout.body, &self.toast_rects, pos))
    }

    /// Whether `pos` is over a toast of the last drawn frame: a toast has the pointer there.
    fn over_toast(&self, pos: (f32, f32)) -> bool {
        over_any(&self.toast_rects, pos)
    }

    /// Sends the report for `ev` at `pos`, if there is one. Returns whether it was sent.
    fn report(&mut self, ev: PointerEvent, pos: (f32, f32)) -> bool {
        let Some(ctx) = self.mouse_ctx() else {
            return false;
        };
        let mods = terminal_mods(self.mods);
        match mouse_to_terminal(&mut self.mouse, ev, pos, mods, &ctx) {
            Some(report) => {
                self.send(Command::Mouse(report));
                true
            }
            None => false,
        }
    }

    fn on_cursor_moved(&mut self, pos: (f32, f32)) {
        self.pointer.moved(pos);
        self.tip_hidden = false;
        if self.overlay_open() {
            return;
        }
        let Some(placed) = self.placed else { return };
        if let Some(p) = &self.pending_link {
            if (pos.0 - p.down.0).abs() <= LINK_DRAG_SLOP
                && (pos.1 - p.down.1).abs() <= LINK_DRAG_SLOP
            {
                return;
            }
            // Moved past the slop: it was a drag, not a click on a link.
            let down = p.down;
            self.pending_link = None;
            self.start_selection(down);
        }
        if self.selecting {
            self.drag_selection(pos);
            return;
        }
        self.refresh_hover();
        if let Some(drag) = &mut self.thumb_drag {
            let scroll = self.tabs.active_item().and_then(|t| t.scroll);
            if let Some(scroll) = scroll {
                let lines = drag.moved(pos.1, placed.layout.body, placed.cell_h_logical(), scroll);
                if lines != 0 {
                    self.send(Command::Scroll(Scroll::Lines(lines)));
                }
            }
            return;
        }
        if self.tracking() && (self.over_body(pos) || self.mouse_captured) {
            self.report(PointerEvent::Move, pos);
        }
    }

    fn on_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        // Every primary press, the ones on a toast or the open list included (D14).
        if button == MouseButton::Left {
            if state.is_pressed() {
                self.press_clock.press(self.start.elapsed());
            } else {
                self.press_clock.release();
            }
        }
        if state.is_pressed() {
            self.hide_tip();
        }
        self.mouse_button(state, button);
        // A release after the pointer left the window ended the hold above, from the last
        // position; now the pointer is simply outside.
        if !state.is_pressed() && self.pointer.released(self.button_held()) {
            self.refresh_hover();
        }
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        if self.overlay_open() {
            if !state.is_pressed() && self.button_held() {
                self.release_under_overlay(state, button);
            }
            return;
        }
        let (Some(pos), Some(placed)) = (self.pointer.last(), self.placed) else {
            return;
        };
        if button == MouseButton::Left {
            if state.is_pressed() {
                if let Some(scroll) = self.scroll_view()
                    && !self.over_toast(pos)
                    && shell::thumb_hit(placed.layout.body, placed.cell_h_logical(), scroll, pos)
                {
                    self.thumb_drag = Some(ThumbDrag::start(pos.1));
                    return;
                }
            } else if self.thumb_drag.take().is_some() {
                return;
            }
        }
        let Some(ev) = pointer_event(state, button) else {
            return;
        };
        // The terminal's context menu: a secondary press the program does not take (it takes the
        // mouse unless Shift forces the app's own handling).
        if ev == PointerEvent::Down(TerminalMouseButton::Right)
            && self.over_body(pos)
            && self.has_session()
            && (!self.tracking() || self.mods.shift_key())
        {
            self.menu = Some(OpenMenu {
                kind: MenuKind::Terminal {
                    selection: self.has_selection(),
                },
                place: MenuPlace::At(pos),
                tab: None,
            });
            self.request_redraw();
            return;
        }
        // A press on a link with the link modifier held is the link's, not the program's and not
        // a selection's, until it is seen whether the pointer moves.
        if ev == PointerEvent::Down(TerminalMouseButton::Left)
            && self.link_modifier_held()
            && let Some((row, col)) = self.cell_under_pointer()
            && let Some(span) = self.tabs.active_item().and_then(|t| t.link_at(row, col))
        {
            self.pending_link = Some(PendingLink {
                uri: span.uri,
                down: pos,
            });
            return;
        }
        match ev {
            PointerEvent::Down(button) => {
                let reported = self.tracking() && self.over_body(pos) && self.report(ev, pos);
                if reported {
                    self.mouse_captured = true;
                } else if button == TerminalMouseButton::Left
                    && self.over_body(pos)
                    && self.has_session()
                {
                    // The program does not take the press (tracking off, or Shift held): the
                    // selection does.
                    self.start_selection(pos);
                }
            }
            PointerEvent::Up(button) => {
                if button == TerminalMouseButton::Left
                    && let Some(link) = self.pending_link.take()
                {
                    self.open_link(&link.uri);
                    return;
                }
                if button == TerminalMouseButton::Left && std::mem::take(&mut self.selecting) {
                    self.send(Command::Select(SelectCmd::End {
                        copy: self.settings.copy_on_select,
                    }));
                }
                // Always through the mapping, which also ends a Shift bypass; sent only when the
                // press went to the program.
                let captured = std::mem::take(&mut self.mouse_captured);
                if let Some(ctx) = self.mouse_ctx() {
                    let mods = terminal_mods(self.mods);
                    let up = mouse_to_terminal(&mut self.mouse, ev, pos, mods, &ctx);
                    if captured && let Some(report) = up {
                        self.send(Command::Mouse(report));
                    }
                }
            }
            PointerEvent::Move => {}
        }
    }

    /// A press the app keeps: a click of a series at the cell edge nearest the pointer. Shift
    /// extends the selection (incremental click) when the program is not taking the mouse; with
    /// it taking the mouse, Shift only forces a new selection. Alt makes a column selection.
    fn start_selection(&mut self, pos: (f32, f32)) {
        let Some(ctx) = self.mouse_ctx() else { return };
        let cell = selection_position(&ctx, pos);
        let clicks = self.clicks.press(self.start.elapsed(), pos);
        let extend = self.mods.shift_key() && self.modes().mouse_tracking == MouseTracking::None;
        let column = self.mods.alt_key();
        let top = self.scroll_view().map_or(0, |s| s.viewport_top);
        self.send(Command::Select(SelectCmd::Start {
            cell,
            top,
            clicks,
            extend,
            column,
        }));
        self.selecting = true;
    }

    /// The pointer moved with the selecting button down.
    fn drag_selection(&mut self, pos: (f32, f32)) {
        let Some(ctx) = self.mouse_ctx() else { return };
        let cell = selection_position(&ctx, pos);
        let scroll = selection_scroll(&ctx, pos);
        let top = self.scroll_view().map_or(0, |s| s.viewport_top);
        self.send(Command::Select(SelectCmd::Drag { cell, top, scroll }));
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        if self.overlay_open() {
            return;
        }
        let (Some(pos), Some(placed)) = (self.pointer.over(), self.placed) else {
            return;
        };
        if !self.has_session() || !terminal_hit(placed.layout.body, &self.toast_rects, pos) {
            return;
        }
        let Some(delta) = wheel_delta(delta) else {
            return;
        };
        let modes = self.modes();
        let alt_screen = self.tabs.active_item().is_some_and(|t| t.alt_screen);
        let ctx = WheelCtx {
            tracking: modes.mouse_tracking != MouseTracking::None,
            shift: self.mods.shift_key(),
            no_scrollback: alt_screen || self.settings.scrollback == 0,
            cell_h: placed.metrics.cell_h as f32,
        };
        match wheel(&mut self.wheel, delta, &ctx) {
            WheelAction::Reports(button, n) => {
                if let Some(mouse) = self.mouse_ctx() {
                    let up = button == TerminalMouseButton::WheelUp;
                    let mods = terminal_mods(self.mods);
                    for _ in 0..n {
                        self.send(Command::Mouse(wheel_report(&mouse, pos, up, mods)));
                    }
                }
            }
            WheelAction::Scroll(lines) => self.send(Command::Scroll(Scroll::Lines(lines))),
            WheelAction::Arrows { up } => self.send(Command::WheelArrows { up }),
            WheelAction::None => {}
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        let attrs = glass_attributes("Luneterm", WINDOW_SIZE, WINDOW_MIN, false);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.fail(event_loop, &format!("window: {e}"));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowExtMacOS;
            window.set_option_as_alt(crate::input::option_as_alt(self.settings.option_as_meta));
        }
        if let Some(t) = &mut self.trace {
            if let Some(every) = t.click_every {
                t.synth = crate::os_event::start_synth_clicks(&window, every);
                if t.synth.is_none() {
                    eprintln!("luneterm: LUNETERM_TRACE_SWITCH_CLICK: no synthetic clicks here");
                }
            }
        }
        self.glass = apply_material(&window, &self.settings);
        let stack = match Stack::build(
            event_loop,
            &window,
            &self.settings,
            self.glass,
            &self.base_theme(),
            self.trace.is_some(),
            self.present,
        ) {
            Ok(s) => s,
            Err(e) => {
                self.fail(event_loop, &e);
                return;
            }
        };
        self.win = Some(Win {
            translator: Translator::new(window.inner_size(), window.scale_factor()),
            output: OutputState::new(),
            clipboard: Clipboard::new(),
            window,
        });
        self.stack = Some(stack);
        self.refresh_grid();
        self.start_tabs();
        let now = self.start.elapsed();
        for notice in std::mem::take(&mut self.startup_notices) {
            self.toasts
                .push(ToastSpec::new(ToastKind::Warning, notice), now);
        }
        // The first frame is drawn here, not on a redraw event a hidden window may never get;
        // the window is shown right after it.
        self.redraw(event_loop);
        // Direct, as urgent: the frame after the first (the window is shown by now).
        if let Some(win) = &self.win {
            win.window.request_redraw();
        }
        self.settle_flow(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if let Some(t) = &mut self.trace {
            t.callback_begin();
        }
        let kind = match event {
            UserEvent::Wake
            | UserEvent::Update(_)
            | UserEvent::UpdateRestart
            | UserEvent::Control(_) => Callback::Wake,
            UserEvent::TraceSwitch(_) => Callback::Auto,
        };
        self.efficiency.tick(Instant::now());
        self.urgent_scope = pacing::is_urgent(match event {
            UserEvent::Wake => EventKind::Wake,
            UserEvent::TraceSwitch(_) => EventKind::TraceSwitch,
            UserEvent::Update(_) | UserEvent::UpdateRestart | UserEvent::Control(_) => {
                EventKind::Other
            }
        });
        self.user_event_inner(event_loop, event);
        self.urgent_scope = false;
        self.settle_flow(event_loop);
        if let Some(t) = &mut self.trace {
            t.callback_end(kind);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(t) = &mut self.trace {
            t.callback_begin();
        }
        let kind = if matches!(event, WindowEvent::RedrawRequested) {
            Callback::Redraw
        } else {
            Callback::Window
        };
        self.efficiency.tick(Instant::now());
        self.urgent_scope = pacing::is_urgent(event_kind(&event));
        self.window_event_inner(event_loop, id, event);
        self.urgent_scope = false;
        self.settle_flow(event_loop);
        if let Some(t) = &mut self.trace {
            t.callback_end(kind);
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        // The quiet period of a background window may have passed (its deadline is in the flow).
        self.efficiency.tick(Instant::now());
        self.tick_update(Instant::now());
        // A paced frame's moment, or a `Repaint::At`, blink, workspace, settings or new-tab
        // deadline came due: one ask serves them all (a due pacer deadline means the slot is
        // free). An app deadline (`pending_flow`) that is due asks at once: held for the slot,
        // its past `WaitUntil` would spin the loop until the frame (D7: one frame per write).
        // While the window is hidden nothing asks, and `settle_flow` waits.
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.urgent_scope = pacing::is_urgent(EventKind::Deadline)
                || flow_due(self.pending_flow(), Instant::now());
            self.request_redraw();
            self.urgent_scope = false;
            self.settle_flow(event_loop);
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // An update check or download in flight stops now; the worker is waited for last.
        self.update.cancel();
        // The saves below must not run at the idle class (Windows) while the window is in the
        // background.
        self.efficiency.release_for_exit(Instant::now());
        if let Some(win) = &self.win {
            win.window.set_visible(false);
        }
        self.flush_workspace(true);
        self.flush_settings(true);
        self.shutdown();
        // The last writes: a disk that hangs does not hang the exit for long.
        if !self.persist.flush(Duration::from_secs(2)) {
            eprintln!("luneterm: the config files were not all written in time");
        }
        if !self.update.shutdown(UPDATE_WAIT) {
            eprintln!("luneterm: the update thread did not stop in time");
        }
        // Last, so the answer to the request that closed the last tab is out first; this takes
        // the socket away.
        self.ctl = None;
    }
}

impl App {
    fn user_event_inner(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                if event_loop.exiting() {
                    return;
                }
                // Every tab's Update is taken here (a hidden tab's too), and a frame is asked
                // for only when something the window shows changed.
                let worth_a_frame = self.pump(event_loop);
                if worth_a_frame && !self.occluded {
                    self.request_redraw();
                }
            }
            UserEvent::TraceSwitch(at) => {
                if event_loop.exiting() || self.modal.is_some() {
                    return;
                }
                if let Some(t) = &mut self.trace {
                    t.pending = Some(at);
                    t.press_arrived(at);
                }
                self.app_command(AppCommand::NextTab, event_loop);
            }
            UserEvent::Update(state) => self.on_update_state(state),
            UserEvent::Control(call) => self.control(call, event_loop),
            UserEvent::UpdateRestart => {
                // The normal quit: `exiting` saves the workspace and settings and ends the
                // sessions; the runtime applies the update once this process is gone.
                if !event_loop.exiting() {
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event_inner(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        // After `exit()` winit still delivers queued events; ignore them so no frame is acquired.
        if event_loop.exiting() {
            return;
        }
        if let Some(t) = &mut self.trace {
            t.event(&event);
        }
        match &event {
            WindowEvent::CloseRequested => {
                // Closing the window closes every tab: it asks like they do.
                if self.modal.is_none() {
                    self.request_close(self.tabs.ids(), event_loop);
                }
                return;
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = *occluded;
                if *occluded {
                    self.pacer.clear();
                }
                self.refresh_visibility();
                if !occluded {
                    self.request_redraw();
                }
                return;
            }
            // The terminal owns the keyboard (D4): these never reach the translator.
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } if !self.overlay_open() => {
                self.on_key(event, *is_synthetic, event_loop);
                return;
            }
            // The page's own keys: Escape and the settings chord leave it; the rest is the
            // toolkit's (fields, lists, Tab).
            WindowEvent::KeyboardInput {
                event: key,
                is_synthetic,
                ..
            } if self.page.is_some() && self.modal.is_none() && self.menu.is_none() => {
                if self.page_key(key, *is_synthetic) {
                    return;
                }
            }
            WindowEvent::Ime(ime) if !self.overlay_open() => {
                self.on_ime(ime);
                return;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.refresh_hover();
            }
            WindowEvent::Focused(focused) => {
                self.efficiency.set_focused(*focused, Instant::now());
                self.focused = *focused;
                if let Some(tab) = self.tabs.active_item_mut() {
                    tab.set_focus(*focused);
                }
                if !focused {
                    self.inbox_open = false;
                    self.press_clock.clear();
                    self.tip_hidden = true;
                    // Every press of ours ends here, the program's too: the window may never
                    // see the release.
                    self.pointer_left_terminal();
                    self.pointer.released(false);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(stack) = &mut self.stack {
                    stack
                        .surface
                        .resize(&stack.renderer.gpu, size.width, size.height);
                }
            }
            // The window may be on another display now: its refresh rate sets the grid (D5).
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Moved(_) => self.refresh_grid(),
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(win) = &self.win {
                    let scale = win.window.scale_factor();
                    self.on_cursor_moved((
                        (position.x / scale) as f32,
                        (position.y / scale) as f32,
                    ));
                }
            }
            WindowEvent::CursorLeft { .. } => {
                let held = self.button_held();
                self.pointer.left(held);
                self.refresh_hover();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_button(*state, *button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(*delta),
            _ => {}
        }
        let Some(win) = &mut self.win else { return };
        win.translator.window_event(&event);
        self.request_redraw();
    }
}

/// What pacing knows of a window event (D4).
fn event_kind(event: &WindowEvent) -> EventKind {
    match event {
        WindowEvent::MouseInput { .. } => EventKind::MouseButton,
        WindowEvent::KeyboardInput { .. } => EventKind::Key,
        WindowEvent::Ime(_) => EventKind::Ime,
        WindowEvent::Focused(_) => EventKind::Focus,
        WindowEvent::Resized(_) => EventKind::Resized,
        WindowEvent::ScaleFactorChanged { .. } => EventKind::ScaleFactorChanged,
        WindowEvent::Occluded(false) => EventKind::Unoccluded,
        WindowEvent::Occluded(true) => EventKind::Occluded,
        WindowEvent::DroppedFile(_) => EventKind::DroppedFile,
        WindowEvent::CloseRequested => EventKind::CloseRequested,
        WindowEvent::CursorMoved { .. } => EventKind::CursorMoved,
        WindowEvent::CursorEntered { .. } => EventKind::CursorEntered,
        WindowEvent::CursorLeft { .. } => EventKind::CursorLeft,
        WindowEvent::MouseWheel { .. } => EventKind::Wheel,
        WindowEvent::ModifiersChanged(_) => EventKind::ModifiersChanged,
        WindowEvent::HoveredFile(_) => EventKind::HoveredFile,
        WindowEvent::HoveredFileCancelled => EventKind::HoveredFileCancelled,
        WindowEvent::Moved(_) => EventKind::Moved,
        WindowEvent::RedrawRequested => EventKind::Redraw,
        _ => EventKind::Other,
    }
}

/// `LUNETERM_TRACE_SWITCH=1` (plan D11): one stderr line per frame from a tab switch until its
/// crossfade ends, every time in ms since `t0`, the arrival of the press or key that asked for the
/// switch (the switch itself when nothing did: a tab closing on its own). It prints numbers and
/// tab ids only: no terminal content, no paths, no environment.
#[derive(Debug, Default)]
struct SwitchTrace {
    /// The latest press (mouse button or key) no frame has taken yet.
    pending: Option<Instant>,
    /// The press the frame in progress took: a switch its output asks for started there.
    taken: Option<Instant>,
    switches: u32,
    run: Option<SwitchRun>,
    /// The frame in progress.
    frame: FrameStamps,
    /// The event loop's health over the current second.
    lp: LoopStats,
    /// `LUNETERM_TRACE_SWITCH_CLICK`: post a synthetic click this often once the window exists.
    click_every: Option<Duration>,
    /// The synthetic clicks' posting times (the known answer for the OS ages).
    synth: Option<std::sync::Arc<crate::os_event::SynthClicks>>,
}

/// Which `ApplicationHandler` callback ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Callback {
    Redraw,
    Window,
    Wake,
    Auto,
}

impl Callback {
    fn name(self) -> &'static str {
        match self {
            Callback::Redraw => "redraw",
            Callback::Window => "window",
            Callback::Wake => "wake",
            Callback::Auto => "auto",
        }
    }
}

/// What the event loop did over one window of about a second: how many callbacks of each kind,
/// how long they kept the UI thread, the longest one, the longest quiet gap between two, and how
/// long the presses (or auto switches) waited for the frame that took them.
#[derive(Debug, Clone, Copy, Default)]
struct LoopStats {
    since: Option<Instant>,
    begun: Option<Instant>,
    last_end: Option<Instant>,
    redraws: u32,
    windows: u32,
    wakes: u32,
    autos: u32,
    busy: Duration,
    wake_busy: Duration,
    max_cb: Duration,
    max_cb_kind: Option<Callback>,
    max_gap: Duration,
    /// Presses that arrived and wait for a frame to take them (the first one's arrival).
    press_waiting: Option<Instant>,
    presses: u32,
    max_press_to_frame: Duration,
    /// Time spent in the surface's acquire this span, and the longest single wait.
    acquire: Duration,
    acquire_max: Duration,
    /// OS timestamp → winit handing it over, real input (motion, presses, keys): count, sum, max.
    os: AgeStats,
    /// The same for the trace's synthetic clicks, and the largest disagreement between their
    /// OS-timestamp age and their `Instant` age (the instrument's own error).
    syn: AgeStats,
    syn_clock_err: f64,
}

/// Count, sum and largest of a set of ages in ms.
#[derive(Debug, Clone, Copy, Default)]
struct AgeStats {
    n: u32,
    sum: f64,
    max: f64,
}

impl AgeStats {
    fn add(&mut self, ms: f64) {
        self.n += 1;
        self.sum += ms;
        self.max = self.max.max(ms);
    }

    fn mean(&self) -> f64 {
        if self.n == 0 {
            f64::NAN
        } else {
            self.sum / f64::from(self.n)
        }
    }
}

/// One switch being traced.
#[derive(Debug, Clone, Copy)]
struct SwitchRun {
    n: u32,
    t0: Instant,
    new: TabId,
    /// Frames printed so far.
    frame: u32,
    /// A frame drawing `new` in slot 0 was presented.
    shown: bool,
}

/// When each part of one frame ended.
#[derive(Debug, Default, Clone, Copy)]
struct FrameStamps {
    start: Option<Instant>,
    pump: Option<Instant>,
    acquire: Option<Instant>,
    present: Option<Instant>,
    /// The crossfade was running when the frame began.
    fading: bool,
    /// The tab drawn in slot 0 (the active one).
    slot0: Option<TabId>,
    phases: Option<crate::render::DrawPhases>,
}

/// `t` in ms after `t0` (negative before it), or NaN when the frame never got there.
fn ms_since(t0: Instant, t: Option<Instant>) -> f64 {
    match t {
        Some(t) if t >= t0 => (t - t0).as_secs_f64() * 1e3,
        Some(t) => -((t0 - t).as_secs_f64() * 1e3),
        None => f64::NAN,
    }
}

impl SwitchTrace {
    /// A window event arrived: a press is what a switch may start from.
    fn event(&mut self, event: &WindowEvent) {
        let press = matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } | WindowEvent::KeyboardInput {
                event: KeyEvent {
                    state: ElementState::Pressed,
                    ..
                },
                ..
            }
        );
        use crate::os_event::{InputKind, current_age};
        let kind = match event {
            WindowEvent::MouseInput { .. } if press => Some(InputKind::Press),
            WindowEvent::KeyboardInput { .. } if press => Some(InputKind::Key),
            WindowEvent::CursorMoved { .. } => Some(InputKind::Motion),
            _ => None,
        };
        let age = kind.and_then(current_age);
        let now = Instant::now();
        // A synthetic click's ages by both clocks (its press; its motions are not counted).
        let synthetic = age.and_then(|a| {
            let by_instant = self.synth.as_ref()?.posted_ms_ago(a.stamp, now)?;
            Some((a.ms, by_instant))
        });
        match (age, synthetic) {
            (Some(a), None) => self.lp.os.add(a.ms),
            (Some(_), Some((ms, by_instant))) if kind == Some(InputKind::Press) => {
                self.lp.syn.add(ms);
                self.lp.syn_clock_err = self.lp.syn_clock_err.max((ms - by_instant).abs());
            }
            _ => {}
        }
        if press {
            self.pending = Some(now);
            self.press_arrived(now);
            let what = if kind == Some(InputKind::Key) {
                "key"
            } else {
                "mouse"
            };
            match age {
                Some(a) => eprintln!(
                    "press: kind={what} os_age_ms={:.2} synthetic={}",
                    a.ms,
                    u8::from(synthetic.is_some())
                ),
                // The event AppKit dispatches is not this one (winit queued it), or no OS stamp.
                None => eprintln!("press: kind={what} os_age_ms=NaN"),
            }
        }
    }

    /// One surface acquire took `d`.
    fn acquire_waited(&mut self, d: Duration) {
        self.lp.acquire += d;
        self.lp.acquire_max = self.lp.acquire_max.max(d);
    }

    /// A press (or an auto switch posted at `at`) reached the app.
    fn press_arrived(&mut self, at: Instant) {
        self.lp.presses += 1;
        self.lp.press_waiting.get_or_insert(at);
    }

    /// The frame in progress took the input that was waiting.
    fn press_taken(&mut self) {
        if let (Some(at), Some(start)) = (self.lp.press_waiting.take(), self.frame.start) {
            let waited = start.saturating_duration_since(at);
            self.lp.max_press_to_frame = self.lp.max_press_to_frame.max(waited);
        }
    }

    /// An `ApplicationHandler` callback starts.
    fn callback_begin(&mut self) {
        let now = Instant::now();
        if let Some(end) = self.lp.last_end {
            self.lp.max_gap = self.lp.max_gap.max(now.saturating_duration_since(end));
        }
        self.lp.since.get_or_insert(now);
        self.lp.begun = Some(now);
    }

    /// The callback begun last ended; about once a second, one `loop:` line.
    fn callback_end(&mut self, kind: Callback) {
        let now = Instant::now();
        let lp = &mut self.lp;
        let took = lp
            .begun
            .map_or(Duration::ZERO, |b| now.saturating_duration_since(b));
        lp.busy += took;
        match kind {
            Callback::Redraw => lp.redraws += 1,
            Callback::Window => lp.windows += 1,
            Callback::Wake => {
                lp.wakes += 1;
                lp.wake_busy += took;
            }
            Callback::Auto => lp.autos += 1,
        }
        if took > lp.max_cb {
            lp.max_cb = took;
            lp.max_cb_kind = Some(kind);
        }
        lp.last_end = Some(now);
        let Some(since) = lp.since else { return };
        let span = now.saturating_duration_since(since);
        if span < Duration::from_secs(1) {
            return;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        eprintln!(
            "loop: span={:.0} redraws={} window_events={} wakes={} autos={} presses={} \
             busy={:.1} wake_busy={:.1} max_cb={:.2} max_cb_kind={} max_gap={:.1} \
             max_press_to_frame={:.1} os_n={} os_mean={:.2} os_max={:.2} syn_n={} \
             syn_mean={:.2} syn_max={:.2} syn_clock_err={:.2} acquire_ms={:.1} acquire_max={:.2}",
            ms(span),
            lp.redraws,
            lp.windows,
            lp.wakes,
            lp.autos,
            lp.presses,
            ms(lp.busy),
            ms(lp.wake_busy),
            ms(lp.max_cb),
            lp.max_cb_kind.map_or("none", Callback::name),
            ms(lp.max_gap),
            ms(lp.max_press_to_frame),
            lp.os.n,
            lp.os.mean(),
            lp.os.max,
            lp.syn.n,
            lp.syn.mean(),
            lp.syn.max,
            lp.syn_clock_err,
            ms(lp.acquire),
            ms(lp.acquire_max),
        );
        *lp = LoopStats {
            last_end: lp.last_end,
            press_waiting: lp.press_waiting,
            ..LoopStats::default()
        };
    }

    /// The active tab became `new` at `now`.
    fn switched(&mut self, new: TabId, now: Instant) {
        let t0 = self.taken.or_else(|| self.pending.take()).unwrap_or(now);
        self.switches += 1;
        if let Some(old) = self.run.take() {
            eprintln!("switch: n={} superseded after {} frames", old.n, old.frame);
        }
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        eprintln!(
            "switch: n={} begin build={build} to=tab{new} asked_at=-{:.2}",
            self.switches,
            ms_since(t0, Some(now)),
        );
        self.run = Some(SwitchRun {
            n: self.switches,
            t0,
            new,
            frame: 0,
            shown: false,
        });
    }

    /// The frame in progress is done: its line, while a switch is being traced.
    fn frame_done(&mut self) -> Option<TraceLine> {
        self.taken = None;
        let run = self.run.as_mut()?;
        let f = self.frame;
        let first_new = !run.shown && f.present.is_some() && f.slot0 == Some(run.new);
        run.shown |= first_new;
        let end = run.shown && !f.fading;
        let line = TraceLine {
            n: run.n,
            f: run.frame,
            t0: run.t0,
            stamps: f,
            first_new,
            end,
        };
        run.frame += 1;
        if end {
            self.run = None;
        }
        Some(line)
    }
}

/// One frame of a traced switch.
#[derive(Debug, Clone, Copy)]
struct TraceLine {
    n: u32,
    f: u32,
    t0: Instant,
    stamps: FrameStamps,
    /// The first presented frame with the new tab in slot 0.
    first_new: bool,
    /// The crossfade is over: the trace of this switch ends here.
    end: bool,
}

impl std::fmt::Display for TraceLine {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let f = &self.stamps;
        let ms = |t| ms_since(self.t0, t);
        let p = f.phases;
        let at = |d: fn(&crate::render::DrawPhases) -> Duration| p.map(|p| p.start + d(&p));
        write!(
            out,
            "switch: n={} f={} start={:.2} pump={:.2} acquire={:.2} ui={:.2} active={:.2} \
             assign={:.2} fading={:.2} draw_with={:.2} present={:.2}",
            self.n,
            self.f,
            ms(f.start),
            ms(f.pump),
            ms(f.acquire),
            ms(at(|p| p.ui)),
            ms(at(|p| p.active)),
            ms(at(|p| p.assign)),
            ms(at(|p| p.fading)),
            ms(at(|p| p.draw_with)),
            ms(f.present),
        )?;
        if let Some(p) = p {
            write!(
                out,
                " views_created={} rows_active={} full={} rows_fading={} rasters_font={} \
                 rasters_custom={}",
                p.views_created,
                p.active_rows,
                u8::from(p.active_full),
                p.fading_rows,
                p.font_rasters,
                p.custom_rasters,
            )?;
        }
        write!(out, " fade_running={}", u8::from(f.fading))?;
        if self.first_new {
            out.write_str(" FIRST_NEW_PRESENTED")?;
        }
        if self.end {
            out.write_str(" FADE_END")?;
        }
        Ok(())
    }
}

/// What the presses of ours that are down hold (see [`App::holds`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Holds {
    /// A selection follows the pointer.
    selecting: bool,
    /// The program has seen a press and not its release.
    captured: bool,
    /// The scroll thumb is being dragged.
    thumb: bool,
    /// A press on a link waits to be a click or a drag.
    link: bool,
}

impl Holds {
    fn any(self) -> bool {
        self.selecting || self.captured || self.thumb || self.link
    }
}

/// What a release under an overlay asks the app to send.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Ended {
    /// Tell the engine the selection ended.
    end_selection: bool,
    /// Tell the program its button went up.
    release_to_program: bool,
}

/// The holds that remain, and what to send, when a button is released while an overlay covers the
/// terminal. `left` is the released button; `program_button` says it is the one the program saw
/// pressed. The primary button's holds (selection, thumb, link press) end with it, and a pending
/// link is only dropped, never opened.
fn release_under_overlay(holds: Holds, left: bool, program_button: bool) -> (Holds, Ended) {
    let mut h = holds;
    let mut e = Ended::default();
    if left {
        e.end_selection = h.selecting;
        h.selecting = false;
        h.thumb = false;
        h.link = false;
    }
    if h.captured && program_button {
        e.release_to_program = true;
        h.captured = false;
    }
    (h, e)
}

/// The release of the press the program has seen, at `at` (the pointer's last position and the
/// mapping of the tab that owns the press). `None` when there is no such press or no position.
fn program_release(
    mouse: &mut MouseState,
    at: Option<((f32, f32), MouseCtx)>,
    mods: TerminalModifiers,
) -> Option<TerminalMouseEvent> {
    let button = mouse.held_button()?;
    let (pos, ctx) = at?;
    mouse_to_terminal(mouse, PointerEvent::Up(button), pos, mods, &ctx)
}

/// What a tab being left is told about the presses still down in it: the selection drag ends (its
/// engine would go on autoscrolling), and a press its program saw is released. `at` is built from
/// the tab being left, not the one taking over.
fn end_left_tab_holds(
    holds: Holds,
    mouse: &mut MouseState,
    at: Option<((f32, f32), MouseCtx)>,
    mods: TerminalModifiers,
    out: &mut Vec<Command>,
) {
    if holds.selecting {
        out.push(Command::Select(SelectCmd::End { copy: false }));
    }
    if holds.captured
        && let Some(report) = program_release(mouse, at, mods)
    {
        out.push(Command::Mouse(report));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::{Group, GroupColor};
    use lune_vt::TerminalMouseEventType;

    /// One frame of the app's loop as the trace sees it: begun, its input taken, drawn with
    /// `slot0` active, presented, done.
    fn traced_frame(t: &mut SwitchTrace, slot0: TabId, fading: bool) -> Option<TraceLine> {
        t.taken = None;
        let now = Instant::now();
        t.frame = FrameStamps {
            start: Some(now),
            pump: Some(now),
            acquire: Some(now),
            present: Some(now),
            fading,
            slot0: Some(slot0),
            phases: None,
        };
        t.taken = t.pending.take();
        t.frame_done()
    }

    #[test]
    fn a_click_switch_starts_at_the_press_the_frame_took_and_ends_with_the_fade() {
        let mut t = SwitchTrace::default();
        let press = Instant::now();
        t.pending = Some(press);
        // The frame that takes the press draws the old tab; its output asks for the switch.
        t.taken = None;
        t.frame = FrameStamps {
            start: Some(press),
            present: Some(press),
            slot0: Some(1),
            ..FrameStamps::default()
        };
        t.taken = t.pending.take();
        t.switched(2, press + Duration::from_millis(5));
        assert_eq!(t.run.map(|r| r.t0), Some(press), "t0 is the press");
        let f0 = t.frame_done().expect("traced");
        assert_eq!((f0.f, f0.first_new, f0.end), (0, false, false));
        let f1 = traced_frame(&mut t, 2, true).expect("traced");
        assert_eq!((f1.f, f1.first_new, f1.end), (1, true, false));
        let f2 = traced_frame(&mut t, 2, true).expect("traced");
        assert_eq!((f2.f, f2.first_new, f2.end), (2, false, false));
        let f3 = traced_frame(&mut t, 2, false).expect("traced");
        assert_eq!((f3.f, f3.first_new, f3.end), (3, false, true));
        assert!(t.run.is_none());
        assert!(traced_frame(&mut t, 2, false).is_none(), "idle: no line");
    }

    #[test]
    fn a_key_switch_starts_at_the_key_and_a_switch_of_its_own_at_itself() {
        let mut t = SwitchTrace::default();
        let key = Instant::now();
        t.pending = Some(key);
        let now = key + Duration::from_millis(1);
        t.switched(2, now);
        assert_eq!(t.run.map(|r| r.t0), Some(key));
        assert_eq!(t.pending, None, "the key is used up");
        // A tab that closes on its own: no press asked for it.
        t.switched(1, now);
        assert_eq!(t.run.map(|r| (r.n, r.t0)), Some((2, now)));
        // No fade (motion off): the first frame of the new tab is also the last.
        let f = traced_frame(&mut t, 1, false).expect("traced");
        assert!(f.first_new && f.end);
    }

    #[test]
    fn age_stats_count_sum_and_max() {
        let mut a = AgeStats::default();
        assert!(a.mean().is_nan());
        for ms in [2.0, 6.0, 1.0] {
            a.add(ms);
        }
        assert_eq!((a.n, a.max), (3, 6.0));
        assert!((a.mean() - 3.0).abs() < 1e-12);
    }

    #[test]
    fn a_trace_line_is_numbers_and_markers_only() {
        let t0 = Instant::now();
        let at = |ms| Some(t0 + Duration::from_millis(ms));
        let line = TraceLine {
            n: 3,
            f: 1,
            t0,
            stamps: FrameStamps {
                start: at(10),
                pump: at(11),
                acquire: at(20),
                present: at(30),
                fading: true,
                slot0: Some(7),
                phases: None,
            },
            first_new: true,
            end: false,
        };
        let text = line.to_string();
        assert!(
            text.starts_with("switch: n=3 f=1 start=10.00 pump=11.00 acquire=20.00 ui=NaN"),
            "{text}"
        );
        assert!(
            text.ends_with("present=30.00 fade_running=1 FIRST_NEW_PRESENTED"),
            "{text}"
        );
        assert!(
            text.chars().all(|c| c.is_ascii_graphic() || c == ' '),
            "{text}"
        );
    }

    fn left_tab_ctx(cols: usize, rows: usize) -> MouseCtx {
        let m = lune_termview::CellMetrics {
            cell_w: 8,
            cell_h: 17,
            char_h: 15,
            char_top: 1,
            baseline: 13,
            font_size: 13.0,
            scale: 1.0,
        };
        MouseCtx {
            layout: lune_termview::GridLayout::new((0.0, 0.0), m),
            cols,
            rows,
            pixel_reports: false,
        }
    }

    /// A press the program saw, on the grid `ctx`.
    fn pressed(ctx: &MouseCtx) -> MouseState {
        let mut m = MouseState::default();
        let down = PointerEvent::Down(TerminalMouseButton::Left);
        assert!(
            mouse_to_terminal(&mut m, down, (4.0, 4.0), TerminalModifiers::default(), ctx)
                .is_some()
        );
        m
    }

    /// F7: leaving a tab mid-drag. The engine's selection ends (it would autoscroll on, hidden),
    /// the program sees its button go up, and the report is clamped to the grid of the tab being
    /// left: a different grid taking over must not change where the button went up.
    #[test]
    fn a_tab_left_during_a_press_is_told_it_ended() {
        let none = TerminalModifiers::default();
        let old = left_tab_ctx(10, 5);
        let far = (4000.0, 4000.0);
        let holds = Holds {
            selecting: true,
            captured: true,
            ..Holds::default()
        };
        let mut mouse = pressed(&old);
        let mut sent = Vec::new();
        end_left_tab_holds(holds, &mut mouse, Some((far, old)), none, &mut sent);
        assert_eq!(sent.len(), 2);
        assert!(matches!(
            sent[0],
            Command::Select(SelectCmd::End { copy: false })
        ));
        let Command::Mouse(up) = &sent[1] else {
            panic!("{:?}", sent[1]);
        };
        assert_eq!(up.kind, TerminalMouseEventType::Up);
        assert_eq!((up.col, up.row), (9, 4), "the left tab's last cell");
        assert_eq!(mouse.held_button(), None);

        // No hold: nothing is said, even with a button the mapping remembers (a Shift bypass goes
        // to the selection, never to the program).
        let mut mouse = pressed(&old);
        let mut sent = Vec::new();
        end_left_tab_holds(
            Holds::default(),
            &mut mouse,
            Some((far, old)),
            none,
            &mut sent,
        );
        assert!(sent.is_empty());

        // Selection only.
        let holds = Holds {
            selecting: true,
            ..Holds::default()
        };
        end_left_tab_holds(holds, &mut MouseState::default(), None, none, &mut sent);
        assert!(matches!(
            sent[..],
            [Command::Select(SelectCmd::End { copy: false })]
        ));

        // Captured with no position to report at: no invented report.
        let holds = Holds {
            captured: true,
            ..Holds::default()
        };
        let mut sent = Vec::new();
        end_left_tab_holds(holds, &mut pressed(&old), None, none, &mut sent);
        assert!(sent.is_empty());
    }

    /// A drag that ends outside the window: the pointer leaves with the button down, and the
    /// release still arrives. It must be handled from the last position, and only then is the
    /// pointer forgotten. (Before: the position was dropped on leaving, the release returned
    /// early, and the selection stayed "down", so later moves dragged it.) The class: every hold
    /// a press sets (selection, program press, scroll thumb, link press) is ended by a release
    /// that needs no pointer position.
    #[test]
    fn a_release_after_the_pointer_left_the_window_is_still_at_the_last_position() {
        let mut p = Pointer::default();
        p.moved((10.0, 20.0));
        // Left with the button down: hover sees nothing, the release sees the last position.
        p.left(true);
        assert_eq!(p.over(), None);
        assert_eq!(p.last(), Some((10.0, 20.0)));
        // Still held (a second button): not forgotten yet.
        assert!(!p.released(true));
        assert_eq!(p.last(), Some((10.0, 20.0)));
        // The last button up: forgotten, and the caller refreshes hover.
        assert!(p.released(false));
        assert_eq!((p.over(), p.last()), (None, None));
        // Nothing to forget a second time.
        assert!(!p.released(false));
    }

    #[test]
    fn leaving_with_no_button_down_forgets_the_pointer_and_moving_back_finds_it() {
        let mut p = Pointer::default();
        p.moved((1.0, 2.0));
        p.left(false);
        assert_eq!((p.over(), p.last()), (None, None));
        // Leaving with a button down, then entering again before the release.
        p.moved((3.0, 4.0));
        p.left(true);
        p.moved((5.0, 6.0));
        assert_eq!(p.over(), Some((5.0, 6.0)));
        assert!(!p.released(false), "it never left for good");
        // A leave with no position known is nothing to keep.
        let mut none = Pointer::default();
        none.left(true);
        assert_eq!(none, Pointer::default());
    }

    #[test]
    fn a_modifier_alone_is_not_a_key_that_hides_the_tooltip() {
        for key in [
            NamedKey::Shift,
            NamedKey::Super,
            NamedKey::Control,
            NamedKey::Alt,
        ] {
            assert!(is_modifier_key(&Key::Named(key)), "{key:?}");
        }
        assert!(!is_modifier_key(&Key::Named(NamedKey::Enter)));
        assert!(!is_modifier_key(&Key::Named(NamedKey::Escape)));
        assert!(!is_modifier_key(&Key::Character("a".into())));
    }

    #[test]
    fn the_earlier_flow_wins() {
        let (soon, later) = (Instant::now(), Instant::now() + Duration::from_secs(1));
        let until = ControlFlow::WaitUntil;
        assert_eq!(
            merge_flow(ControlFlow::Wait, ControlFlow::Wait),
            ControlFlow::Wait
        );
        assert_eq!(merge_flow(until(later), ControlFlow::Wait), until(later));
        assert_eq!(merge_flow(ControlFlow::Wait, until(later)), until(later));
        assert_eq!(merge_flow(until(later), until(soon)), until(soon));
        assert_eq!(
            merge_flow(until(soon), ControlFlow::Poll),
            ControlFlow::Poll
        );
    }

    #[test]
    fn a_deferred_ask_is_in_the_settled_flow() {
        // I1: a paced ask the pacer defers is the control flow's deadline after the callback.
        let start = Instant::now();
        let ms = Duration::from_millis;
        let mut p = Pacer::new(Cadence::hz(60));
        p.frame_started(ms(0));
        assert!(!p.ask(ms(1), Duration::ZERO, false));
        let free_at = Cadence::hz(60).next_tick(ms(0)).expect("a tick");
        assert_eq!(p.deadline(), Some(free_at));
        assert_eq!(
            settled_flow(p.deadline(), ControlFlow::Wait, false, start),
            ControlFlow::WaitUntil(start + free_at)
        );
        // An earlier app deadline wins; a later one does not hide the pacer's.
        let soon = start + ms(5);
        let later = start + ms(500);
        assert_eq!(
            settled_flow(p.deadline(), ControlFlow::WaitUntil(soon), false, start),
            ControlFlow::WaitUntil(soon)
        );
        assert_eq!(
            settled_flow(p.deadline(), ControlFlow::WaitUntil(later), false, start),
            ControlFlow::WaitUntil(start + free_at)
        );
        // Nothing deferred: the app's deadlines alone, or waiting.
        assert_eq!(
            settled_flow(None, ControlFlow::WaitUntil(later), false, start),
            ControlFlow::WaitUntil(later)
        );
        assert_eq!(
            settled_flow(None, ControlFlow::Wait, false, start),
            ControlFlow::Wait
        );
        // An app deadline that has come asks its frame at once.
        assert!(flow_due(ControlFlow::WaitUntil(soon), soon));
        assert!(!flow_due(ControlFlow::WaitUntil(later), soon));
        assert!(!flow_due(ControlFlow::Wait, later));
        // Hidden: wait, whatever is deferred (I4).
        assert_eq!(
            settled_flow(p.deadline(), ControlFlow::WaitUntil(soon), true, start),
            ControlFlow::Wait
        );
    }

    #[test]
    fn the_blink_runs_only_while_active_and_toggles_each_period() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut b = Blink::default();
        // Not active: on, no timer.
        assert_eq!(b.tick(t0, false), None);
        assert!(b.on());
        // Active: the first tick starts the period with the cursor on.
        assert_eq!(b.tick(t0, true), Some(t0 + ms(600)));
        assert!(b.on());
        // Before the deadline nothing changes.
        assert_eq!(b.tick(t0 + ms(599), true), Some(t0 + ms(600)));
        assert!(b.on());
        // At the deadline it toggles and re-arms from now.
        assert_eq!(b.tick(t0 + ms(600), true), Some(t0 + ms(1200)));
        assert!(!b.on());
        assert_eq!(b.tick(t0 + ms(1200), true), Some(t0 + ms(1800)));
        assert!(b.on());
        // A late tick toggles once, not once per missed period.
        assert_eq!(b.tick(t0 + ms(5000), true), Some(t0 + ms(5600)));
        assert!(!b.on());
        // Losing focus (or a non-blinking cursor) stops the timer and shows the cursor.
        assert_eq!(b.tick(t0 + ms(5001), false), None);
        assert!(b.on());
        assert_eq!(b, Blink::default());
    }

    /// D19: a ring starts when a row was recorded in a focused window and the last one began 3 s
    /// or more ago; never unfocused, never inside the gap, never without a recorded row.
    #[test]
    fn a_ring_starts_on_a_recorded_row_in_a_focused_window_outside_the_gap() {
        let ms = Duration::from_millis;
        let t = ms(10_000);
        // Focused + recorded, no ring before: starts now.
        assert_eq!(ring_after(None, true, true, t), Some(t));
        // Unfocused: none, and the last start is kept.
        assert_eq!(ring_after(None, true, false, t), None);
        assert_eq!(ring_after(Some(ms(1)), true, false, t), Some(ms(1)));
        // Nothing recorded: no change.
        assert_eq!(ring_after(None, false, true, t), None);
        // Inside the gap: the last start stays (the ring is not restarted).
        assert_eq!(ring_after(Some(t), true, true, t + ms(2999)), Some(t));
        // At the gap: a new ring.
        assert_eq!(
            ring_after(Some(t), true, true, t + ms(3000)),
            Some(t + ms(3000))
        );
    }

    /// The ring the button is handed: only while unread, focused and inside `RING_LEN`.
    #[test]
    fn the_button_is_handed_a_ring_only_while_unread_focused_and_inside_its_second() {
        let ms = Duration::from_millis;
        let s = Some(ms(5000));
        assert_eq!(ring_for_frame(s, 3, true, ms(5000)), s);
        assert_eq!(ring_for_frame(s, 3, true, ms(5999)), s);
        assert_eq!(
            ring_for_frame(s, 3, true, ms(6000)),
            None,
            "the ring is over"
        );
        assert_eq!(
            ring_for_frame(s, 0, true, ms(5100)),
            None,
            "all read mid-ring"
        );
        assert_eq!(ring_for_frame(s, 3, false, ms(5100)), None, "unfocused");
        assert_eq!(ring_for_frame(None, 3, true, ms(5100)), None);
    }

    /// The arrival path end to end through `hear` and the rule: a bell in a background tab of a
    /// focused window rings; the same bell unfocused does not, and nothing is recorded for the
    /// active tab.
    #[test]
    fn hear_then_the_rule_rings_for_a_background_bell_only() {
        let now = Duration::from_secs(1);
        let (_, _, h) = heard(1, vec![], true, false, true);
        assert_eq!(ring_after(None, h.recorded, true, now), Some(now));
        let (_, _, h) = heard(1, vec![], false, false, true);
        assert!(h.recorded);
        assert_eq!(ring_after(None, h.recorded, false, now), None);
        let (_, _, h) = heard(1, vec![], true, true, true);
        assert!(!h.recorded);
        assert_eq!(ring_after(None, h.recorded, true, now), None);
    }

    fn msg(title: &str, body: &str) -> Message {
        Message {
            title: title.to_owned(),
            body: body.to_owned(),
        }
    }

    /// `hear` over a fresh inbox: (inbox, candidates, heard).
    fn heard(
        bells: u32,
        messages: Vec<Message>,
        focused: bool,
        active: bool,
        on: bool,
    ) -> (Inbox, Vec<ToastCandidate>, Heard) {
        let mut inbox = Inbox::new();
        let mut cands = Vec::new();
        let h = hear(
            &mut inbox,
            &mut cands,
            7,
            bells,
            messages,
            focused,
            active,
            on,
            Duration::from_secs(1),
        );
        (inbox, cands, h)
    }

    #[test]
    fn a_bell_is_routed_by_focus_and_by_whether_its_tab_is_active() {
        // focused, active: nothing.
        let (i, c, h) = heard(1, vec![], true, true, true);
        assert!(i.is_empty() && c.is_empty());
        assert_eq!(h, Heard::default());
        // focused, other tab: a row, no attention, no toast.
        let (i, c, h) = heard(2, vec![], true, false, true);
        assert_eq!((i.unread(), i.tab_unread(7), c.len()), (1, 1, 0));
        assert_eq!(i.entries().next().map(|n| n.count), Some(2));
        assert_eq!(
            h,
            Heard {
                recorded: true,
                attention: false
            }
        );
        // unfocused, active: attention only.
        let (i, _, h) = heard(1, vec![], false, true, true);
        assert!(i.is_empty());
        assert_eq!(
            h,
            Heard {
                recorded: false,
                attention: true
            }
        );
        // unfocused, other tab: row and attention.
        let (i, c, h) = heard(1, vec![], false, false, true);
        assert_eq!((i.unread(), c.len()), (1, 0));
        assert_eq!(
            h,
            Heard {
                recorded: true,
                attention: true
            }
        );
        // No bell: nothing, whatever the state.
        let (i, _, h) = heard(0, vec![], false, false, true);
        assert!(i.is_empty());
        assert_eq!(h, Heard::default());
    }

    #[test]
    fn a_point_under_a_toast_is_not_the_terminals_but_a_point_beside_it_is() {
        let body = Rect::new(10.0, 40.0, 800.0, 500.0);
        let toast = Rect::new(500.0, 400.0, 300.0, 100.0);
        let other = Rect::new(500.0, 290.0, 300.0, 100.0);
        // Inside the body, outside every toast: the terminal's.
        assert!(terminal_hit(body, &[toast, other], (100.0, 100.0)));
        assert!(terminal_hit(body, &[toast, other], (499.0, 450.0)));
        // Inside a toast (any of them, its buttons included): no press, report, selection start,
        // hover or wheel reaches the terminal.
        assert!(!terminal_hit(body, &[toast, other], (600.0, 450.0)));
        assert!(!terminal_hit(body, &[toast, other], (600.0, 300.0)));
        // The rect's edges: left and top inside, right and bottom outside (the toolkit's rule).
        assert!(!terminal_hit(body, &[toast], (500.0, 400.0)));
        assert!(terminal_hit(body, &[toast], (800.0, 450.0)));
        // Outside the body is never the terminal's, a toast or not.
        assert!(!terminal_hit(body, &[], (5.0, 100.0)));
        // No toast live: exactly the body test of before.
        for pos in [(10.0, 40.0), (809.9, 539.9), (810.0, 300.0), (400.0, 540.0)] {
            assert_eq!(terminal_hit(body, &[], pos), body.contains(pos), "{pos:?}");
        }
        assert!(over_any(&[toast], (600.0, 450.0)));
        assert!(!over_any(&[], (600.0, 450.0)));
    }

    #[test]
    fn a_message_is_routed_by_the_d6_table_and_only_a_background_focused_one_earns_a_toast() {
        let m = || vec![msg("T", "B")];
        let (i, c, h) = heard(0, m(), true, true, true);
        assert!(i.is_empty() && c.is_empty());
        assert_eq!(h, Heard::default());
        let (i, c, h) = heard(0, m(), true, false, true);
        assert_eq!((i.unread(), c.len()), (1, 1));
        assert_eq!(
            (c[0].tab, c[0].title.as_str(), c[0].body.as_str()),
            (7, "T", "B")
        );
        assert!(h.recorded && !h.attention);
        let (i, c, h) = heard(0, m(), false, true, true);
        assert!(i.is_empty() && c.is_empty());
        assert!(!h.recorded && h.attention);
        let (i, c, h) = heard(0, m(), false, false, true);
        assert_eq!((i.unread(), c.len()), (1, 0));
        assert!(h.recorded && h.attention);
    }

    #[test]
    fn with_the_setting_off_messages_change_nothing_and_bells_still_work() {
        for (focused, active) in [(true, true), (true, false), (false, true), (false, false)] {
            let (i, c, h) = heard(0, vec![msg("T", "B"), msg("", "x")], focused, active, false);
            assert!(i.is_empty() && c.is_empty(), "{focused} {active}");
            assert_eq!(h, Heard::default());
        }
        // A bell next to messages is the same with the setting on or off.
        let off = heard(1, vec![msg("T", "B")], false, false, false);
        let on_bell_only = heard(1, vec![], false, false, true);
        assert_eq!(off.2, on_bell_only.2);
        assert_eq!(off.0.len(), on_bell_only.0.len());
    }

    #[test]
    fn several_messages_and_a_bell_in_one_update_are_all_recorded() {
        let (i, c, h) = heard(
            3,
            vec![msg("a", "1"), msg("a", "1"), msg("", "2")],
            true,
            false,
            true,
        );
        // A bell row, one coalesced message row (x2) and a title-less one.
        assert_eq!(i.len(), 3);
        assert_eq!(
            c.len(),
            3,
            "a candidate per message; the toast slots decide"
        );
        assert!(h.recorded);
    }

    #[test]
    fn the_inbox_list_opens_on_the_button_and_closes_on_each_way_out() {
        use lune_ui::ui::Dismissal;
        let out = |dismissed, clicked, clear_all| shell::InboxOut {
            rect: Rect::new(0.0, 0.0, 360.0, 100.0),
            dismissed,
            clicked,
            clear_all,
        };
        // The button toggles.
        assert!(inbox_open_after(false, true, None));
        assert!(!inbox_open_after(true, true, Some(&out(None, None, false))));
        // Nothing asked: it stays as it is.
        assert!(inbox_open_after(true, false, Some(&out(None, None, false))));
        assert!(!inbox_open_after(false, false, None));
        // Escape, an outside press, a focus loss, a row press, Clear all.
        for dismissal in [
            Dismissal::Escape,
            Dismissal::OutsidePress,
            Dismissal::FocusLoss,
        ] {
            assert!(!inbox_open_after(
                true,
                false,
                Some(&out(Some(dismissal), None, false))
            ));
        }
        assert!(!inbox_open_after(
            true,
            false,
            Some(&out(None, Some(0), false))
        ));
        assert!(!inbox_open_after(true, false, Some(&out(None, None, true))));
        // An outside press on the button itself, in the one frame: it stays closed (the button
        // toggled it shut, the dismissal agrees).
        assert!(!inbox_open_after(
            true,
            true,
            Some(&out(Some(Dismissal::OutsidePress), None, false))
        ));
    }

    #[test]
    fn settle_keeps_the_inbox_true_after_every_way_the_tabs_change() {
        let mut tabs: TabSet<()> = TabSet::new();
        let a = tabs.push(());
        let b = tabs.push(());
        let c = tabs.push(());
        tabs.activate(a);
        let mut inbox = Inbox::new();
        let now = Duration::from_secs(1);
        for t in [a, b, c] {
            inbox.bell(t, 1, now);
        }
        // Focused: the active tab's row is read, the others stay.
        assert!(settle_inbox(&mut inbox, &tabs, true));
        assert_eq!(
            (
                inbox.tab_unread(a),
                inbox.tab_unread(b),
                inbox.tab_unread(c)
            ),
            (0, 1, 1)
        );
        assert!(
            !settle_inbox(&mut inbox, &tabs, true),
            "nothing more to change"
        );
        // Each way of switching: the new active tab reads at the next settle.
        tabs.activate(b);
        assert!(settle_inbox(&mut inbox, &tabs, true));
        assert_eq!(inbox.tab_unread(b), 0);
        tabs.activate_step(1);
        settle_inbox(&mut inbox, &tabs, true);
        assert_eq!(tabs.active(), Some(c));
        assert_eq!(inbox.tab_unread(c), 0);
        assert_eq!(inbox.unread(), 0);
        // Unfocused: a bell for the active tab stays unread until focus returns.
        inbox.bell(c, 1, now);
        assert!(!settle_inbox(&mut inbox, &tabs, false));
        assert_eq!(inbox.tab_unread(c), 1);
        assert!(settle_inbox(&mut inbox, &tabs, true));
        assert_eq!(inbox.tab_unread(c), 0);
        // A closed tab's rows go.
        inbox.bell(a, 1, now);
        tabs.close(a);
        assert!(settle_inbox(&mut inbox, &tabs, true));
        assert!(inbox.entries().all(|n| n.tab != a));
    }

    #[test]
    fn the_bell_asks_for_attention_only_while_unfocused_and_at_most_every_2_seconds() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut b = BellLimiter::default();
        // Focused: nothing, and the limiter is not used up.
        assert!(!b.due(t0, true));
        assert!(b.due(t0, false));
        assert!(!b.due(t0 + ms(1999), false));
        assert!(!b.due(t0 + ms(1500), true));
        assert!(b.due(t0 + ms(2000), false));
        assert!(!b.due(t0 + ms(3999), false));
        assert!(b.due(t0 + ms(4000), false));
    }

    #[test]
    fn a_save_is_told_each_tabs_group_and_whether_it_runs_a_program() {
        let mut tabs: TabSet<Tab> = TabSet::new();
        let tab = |origin, program| {
            let mut t = Tab::empty(palettes::theme_of(&Settings::default().palette));
            t.origin = origin;
            t.program = program;
            t
        };
        let user = tabs.push(tab(Origin::User, false));
        let shell = tabs.push(tab(Origin::Control, false));
        tabs.push(tab(Origin::Control, true));
        let g = tabs.set_group(user, "work");
        tabs.set_group(shell, "work");
        tabs.recolor_group(g, GroupColor::Pink);
        tabs.set_collapsed(g, true);

        let infos = tab_infos(&tabs);
        assert_eq!(
            infos.iter().map(|i| i.program).collect::<Vec<_>>(),
            [false, false, true],
            "a control tab running a shell is a tab like any other"
        );
        let work = Group {
            name: "work".into(),
            color: GroupColor::Pink,
            collapsed: true,
        };
        assert_eq!(
            infos.into_iter().map(|i| i.group).collect::<Vec<_>>(),
            [Some(work.clone()), Some(work), None]
        );
    }

    #[test]
    fn input_inside_the_control_hold_is_dropped_and_from_its_end_delivered() {
        let t0 = Instant::now();
        let mut tab = Tab::empty(TermTheme::lunegit_one_dark());
        // No hold: through, and noted.
        assert!(admit_input(None, Some(&mut tab), t0));
        assert_eq!(tab.last_input, Some(t0));
        // A control focus at t0 holds until t0 + 400 ms, for every tab.
        let hold = Some(t0 + INPUT_HOLD);
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut other = Tab::empty(TermTheme::lunegit_one_dark());
        for at in [0, 1, 200, 399] {
            assert!(!admit_input(hold, Some(&mut tab), ms(at)), "{at} ms");
            assert!(!admit_input(hold, Some(&mut other), ms(at)), "{at} ms");
        }
        // Dropped input is no input: the idle time still runs from t0.
        assert_eq!(tab.last_input, Some(t0));
        assert_eq!(other.last_input, None);
        for at in [400, 401, 5000] {
            assert!(admit_input(hold, Some(&mut tab), ms(at)), "{at} ms");
            assert_eq!(tab.last_input, Some(ms(at)));
        }
        assert!(admit_input(hold, None, ms(400)));
    }

    #[test]
    fn the_cursor_follows_the_engines_and_stays_on_the_last_column() {
        let engine = ViewportCursor {
            x: 80,
            y: 3,
            visible: true,
            style: CursorStyle::Bar,
            blink: true,
        };
        let c = cursor_of(&engine, 80);
        assert_eq!((c.row, c.col), (3, 79));
        assert_eq!(c.shape, CursorShape::Bar);
        assert!(c.blink && c.visible);
        let c = cursor_of(&ViewportCursor { x: 5, ..engine }, 80);
        assert_eq!(c.col, 5);
        // An empty grid does not underflow.
        assert_eq!(cursor_of(&engine, 0).col, 0);
        let hidden = ViewportCursor {
            visible: false,
            style: CursorStyle::Underline,
            ..engine
        };
        let c = cursor_of(&hidden, 80);
        assert!(!c.visible);
        assert_eq!(c.shape, CursorShape::Underline);
    }

    #[test]
    fn the_session_palette_replaces_the_colours_and_keeps_the_alpha() {
        let base = TermTheme::lunegit_one_dark();
        let mut palette = HostPalette::from_theme(&base);
        assert_eq!(theme_with_palette(&base, &palette), base);
        palette.ansi[1] = [1, 2, 3];
        palette.foreground = [4, 5, 6];
        palette.background = [7, 8, 9];
        palette.cursor = [10, 11, 12];
        let t = theme_with_palette(&base, &palette);
        assert_eq!(t.palette[1], [1, 2, 3, base.palette[1][3]]);
        assert_eq!(t.foreground, [4, 5, 6, base.foreground[3]]);
        assert_eq!(t.background, [7, 8, 9, base.background[3]]);
        assert_eq!(t.cursor, [10, 11, 12, base.cursor[3]]);
        assert_eq!(t.palette[2], base.palette[2]);
    }

    #[test]
    fn the_failure_grid_shows_the_lines_from_the_top_left() {
        let failure = SpawnFailure {
            kind: std::io::ErrorKind::NotFound,
            program: "pwsh.exe".into(),
        };
        let g = failure_grid(40, 5, &failure);
        let row = |y: usize| -> String {
            (0..40)
                .map(|x| {
                    g.get(y, x)
                        .map_or(' ', |c| c.text().chars().next().unwrap_or(' '))
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        assert_eq!(row(0), "Failed to start terminal");
        assert_eq!(row(1), "NotFound: pwsh.exe");
        assert_eq!(row(2), "");
        // A grid too small for the text cuts it instead of failing.
        let tiny = failure_grid(4, 1, &failure);
        assert_eq!(tiny.cols(), 4);
    }

    /// F2, review r10: an overlay opened during a press (a right press, Cmd+W) must not leave the
    /// press held. The class is every hold a press sets.
    #[test]
    fn a_release_under_an_overlay_ends_what_that_button_holds() {
        let all = Holds {
            selecting: true,
            captured: true,
            thumb: true,
            link: true,
        };
        // The primary button: selection, thumb and link press end; the program's press stays
        // (it belongs to its own button, which is not this one).
        let (h, e) = release_under_overlay(all, true, false);
        assert_eq!(
            h,
            Holds {
                captured: true,
                ..Holds::default()
            }
        );
        assert_eq!(
            e,
            Ended {
                end_selection: true,
                release_to_program: false
            }
        );
        // The program's button: it sees the release; the selection is left to its own button.
        let (h, e) = release_under_overlay(all, false, true);
        assert!(!h.captured && h.selecting && h.thumb && h.link);
        assert_eq!(
            e,
            Ended {
                end_selection: false,
                release_to_program: true
            }
        );
        // The primary button is also the program's: everything ends and the program is told.
        let (h, e) = release_under_overlay(all, true, true);
        assert!(!h.any());
        assert!(e.end_selection && e.release_to_program);
    }

    #[test]
    fn a_release_that_belongs_to_the_overlay_does_nothing() {
        // Nothing held: nothing to end, whichever button.
        for (left, program) in [(true, false), (false, false), (true, true)] {
            let (h, e) = release_under_overlay(Holds::default(), left, program);
            assert_eq!(h, Holds::default());
            assert_eq!(e, Ended::default());
        }
        // A press of the program's that is down, released on another button: untouched.
        let held = Holds {
            captured: true,
            ..Holds::default()
        };
        let (h, e) = release_under_overlay(held, false, false);
        assert_eq!(h, held);
        assert_eq!(e, Ended::default());
        assert!(held.any() && !Holds::default().any());
    }
}
