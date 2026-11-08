//! The app: the event loop, the window, the frame loop and the tabs' session wiring (plan M9 steps
//! 4 and 6, M10 step 2).
//!
//! Composition follows the toolkit's gallery (`apps/gallery/src/window.rs`): the app owns the event
//! loop, the window, the GPU stack and the clock. The window is created hidden and shown after its
//! first frame. A frame is `Translator::take` → [`Renderer::draw`] → `OutputState::apply` →
//! `Clipboard::service` → present, and the frame's `Repaint` (merged with the cursor blink) sets
//! the loop's control flow, so an idle window waits with no timer.
//!
//! The terminal owns the keyboard (D4): key and IME events go to the active tab's session (through
//! the shortcut table and the key router) and never reach the toolkit's translator, so `Ui::begin`
//! cannot eat Tab or Escape; while a dialog is open they are the toolkit's instead. Modifier,
//! pointer, wheel, focus, resize and scale events always reach it. Each tab's session runs on its
//! own threads; this thread only sends commands and takes the Updates, one per tab per frame
//! (D1, D2).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{
    ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{UserAttentionType, Window, WindowId};

use lune_pty::PortablePtyBackend;
use lune_termview::{Cell, Cursor, CursorShape, Grid, SelectionRange, TermTheme};
use lune_ui::clipboard::Clipboard;
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::{
    ClipboardRequest, OutputState, Translator, WindowCommand, control_flow, keymap,
};
use lune_ui::material::{self, Material};
use lune_ui::surface::{Acquired, WindowSurface};
use lune_ui::widgets::tab_strip::TabEvent;
use lune_ui::widgets::toast::{ToastKind, ToastSpec, Toasts};
use lune_ui::window::glass_attributes;
use lune_vt::{CursorStyle, MouseEncoding, MouseTracking, TerminalMouseButton, ViewportCursor};

use crate::cwd;
use crate::dump;
use crate::fonts;
use crate::input::{
    AppCommand, AppShortcuts, ClickCounter, KeyRoute, MouseCtx, MouseState, PointerEvent, RouteCtx,
    WheelAction, WheelCtx, WheelState, key_input, mouse_to_terminal, pointer_event, route_key,
    selection_position, selection_scroll, terminal_mods, wheel, wheel_delta, wheel_report,
};
use crate::render::{Content, Placed, Renderer};
use crate::session::{
    Command, CopyResult, HostPalette, MAX_COPY_BYTES, Scroll, SelectCmd, Session, SessionConfig,
    UpdateModes, view_range,
};
use crate::settings::Settings;
use crate::shell::{self, HOST, Modal, ModalOut, ScrollView, StripTab, ThumbDrag};
use crate::spawn::{self, SpawnFailure};
use crate::tabs::{
    ClosePlan, CwdCheck, CwdState, PasteState, Tab, TabId, TabSet, confirm_message, plan_close,
};
use crate::title::DEFAULT_TITLE;

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

/// Events other threads post to the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEvent {
    /// The session has an Update for the UI: draw a frame.
    Wake,
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
pub fn run() -> ExitCode {
    if let Some(dir) = std::env::var_os("LUNETERM_DUMP_FRAMES").filter(|v| !v.is_empty()) {
        return dump::run(Path::new(&dir));
    }
    run_window()
}

fn run_window() -> ExitCode {
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
    let settings = Settings::default().clamped();
    let mut app = App::new(settings, event_loop.create_proxy());
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop: {e}");
        return ExitCode::from(1);
    }
    ExitCode::from(app.exit_code)
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
}

impl Stack {
    fn build(
        event_loop: &ActiveEventLoop,
        window: &Arc<Window>,
        settings: &Settings,
        glass: bool,
        theme: &TermTheme,
    ) -> Result<Stack, String> {
        let instance = gpu::instance(Some(Box::new(event_loop.owned_display_handle())));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface)))?;
        let size = window.inner_size();
        let surface = WindowSurface::new(&gpu, surface, (size.width, size.height))?;
        let (text, _took) = fonts::text_cache();
        let mut renderer = Renderer::new(gpu, text, surface.format(), settings, glass);
        if renderer.term_theme() != theme {
            renderer.set_term_theme(theme.clone());
        }
        Ok(Stack { surface, renderer })
    }
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
    pointer: Option<(f32, f32)>,
    mouse: MouseState,
    /// A mouse press went to the program: so do its moves and its release.
    mouse_captured: bool,
    /// A press started a selection and the button is still down: moves drag it.
    selecting: bool,
    clicks: ClickCounter,
    toasts: Toasts,
    wheel: WheelState,
    thumb_drag: Option<ThumbDrag>,
    bell: BellLimiter,
    focused: bool,
    /// The app's time origin (`RawInput.time` and `Repaint::At` count from it).
    start: Instant,
    blink: Blink,
    occluded: bool,
    shown: bool,
    exit_code: u8,
}

impl App {
    fn new(settings: Settings, proxy: EventLoopProxy<UserEvent>) -> App {
        App {
            settings,
            glass: false,
            win: None,
            stack: None,
            proxy,
            tabs: TabSet::new(),
            leaving: Vec::new(),
            pending_new: None,
            modal: None,
            window_title: DEFAULT_TITLE.to_owned(),
            placed: None,
            preedit: None,
            paste: PasteState::default(),
            clipboard_write: None,
            shortcuts: AppShortcuts::standard(cfg!(target_os = "macos")),
            shortcuts_no_copy: AppShortcuts::standard(cfg!(target_os = "macos")).without_copy(),
            mods: ModifiersState::empty(),
            pointer: None,
            mouse: MouseState::default(),
            mouse_captured: false,
            selecting: false,
            clicks: ClickCounter::default(),
            toasts: Toasts::default(),
            wheel: WheelState::default(),
            thumb_drag: None,
            bell: BellLimiter::default(),
            focused: false,
            start: Instant::now(),
            blink: Blink::default(),
            occluded: false,
            shown: false,
            exit_code: 0,
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
    fn base_theme() -> TermTheme {
        TermTheme::lunegit_one_dark()
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
            .map_or_else(Self::base_theme, |t| t.theme.clone());
        match Stack::build(event_loop, &window, &self.settings, self.glass, &theme) {
            Ok(stack) => {
                self.stack = Some(stack);
                if let Some(tab) = self.tabs.active_item_mut() {
                    tab.grid.mark_all_dirty();
                }
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

    fn request_redraw(&self) {
        if !self.occluded
            && let Some(win) = &self.win
        {
            win.window.request_redraw();
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

    fn has_selection(&self) -> bool {
        self.tabs
            .active_item()
            .is_some_and(|t| t.selection.is_some())
    }

    fn has_session(&self) -> bool {
        self.tabs.active_item().is_some_and(|t| t.session.is_some())
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
            Some(check) => self.pending_new = Some(check),
            None => self.open_tab(None),
        }
    }

    /// Spawns a shell and adds its tab, active, at the window's grid size. On failure the tab's
    /// grid tells why (D12) and it can be closed.
    fn open_tab(&mut self, dir: Option<PathBuf>) {
        let (Some(win), Some(stack)) = (&self.win, &mut self.stack) else {
            return;
        };
        let scale = win.window.scale_factor() as f32;
        let size = win.window.inner_size();
        let logical = (size.width as f32 / scale, size.height as f32 / scale);
        let geometry = stack.renderer.place(logical, scale).geometry();
        let adapter = stack.renderer.gpu.adapter.get_info();

        let mut tab = Tab::empty(Self::base_theme());
        tab.focused = self.focused;
        match spawn::launch_in(
            &PortablePtyBackend,
            &self.settings,
            geometry.pty_size(),
            dir.as_deref(),
        ) {
            Err(failure) => {
                eprintln!(
                    "luneterm: failed to start a shell: {:?} ({})",
                    failure.kind, failure.program
                );
                tab.failure = Some(failure);
            }
            Ok((pty, program)) => {
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
                match Session::start(pty, config, waker) {
                    Ok(session) => {
                        let shell = if program == "login shell" {
                            std::env::var("SHELL").unwrap_or(program)
                        } else {
                            program
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
        let before = self.tabs.active();
        self.tabs.push(tab);
        self.active_changed(before);
    }

    /// The active tab changed from `before`: what belonged to the old tab's pointer and keyboard
    /// state is dropped, the sessions' visibility and focus follow, and the new tab's grid is
    /// drawn from scratch.
    fn active_changed(&mut self, before: Option<TabId>) {
        let now = self.tabs.active();
        if now == before {
            return;
        }
        self.preedit = None;
        self.mouse_captured = false;
        self.selecting = false;
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
            ClosePlan::Now(ids) => self.close_tabs(&ids, event_loop),
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
    fn close_tabs(&mut self, ids: &[TabId], event_loop: &ActiveEventLoop) {
        let before = self.tabs.active();
        for id in ids {
            let Some(tab) = self.tabs.close(*id) else {
                continue;
            };
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
        self.active_changed(before);
        self.request_redraw();
    }

    /// What the user asked of the strip this frame.
    fn tab_event(&mut self, event: TabEvent, event_loop: &ActiveEventLoop) {
        let before = self.tabs.active();
        match event {
            TabEvent::Activate(key) => {
                self.tabs.activate(key.0);
                self.active_changed(before);
            }
            TabEvent::Close(key) => self.request_close(vec![key.0], event_loop),
            TabEvent::New => self.new_tab_request(),
            TabEvent::Reorder { from, to } => self.tabs.reorder(from, to),
            TabEvent::CloseOthers(key) => {
                let ids = self
                    .tabs
                    .ids()
                    .into_iter()
                    .filter(|id| *id != key.0)
                    .collect();
                self.request_close(ids, event_loop);
            }
            TabEvent::CloseAll => self.request_close(self.tabs.ids(), event_loop),
            TabEvent::Rename(key) => {
                if let Some(tab) = self.tabs.get(key.0) {
                    self.modal = Some(ModalState::Rename {
                        id: key.0,
                        text: tab.strip_title().to_owned(),
                        frames: 0,
                    });
                }
            }
            TabEvent::MenuExtra(..) => {}
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
                        // In memory until the settings file (step 5).
                        self.settings.confirm_close = false;
                    }
                    self.close_tabs(&ids, event_loop);
                }
                Some(ModalState::Rename { id, text, .. }) => {
                    if let Some(tab) = self.tabs.get_mut(id) {
                        tab.rename(&text);
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
        let mut worth_a_frame = false;
        let mut exited = Vec::new();
        for (id, fx) in seen {
            worth_a_frame |= Some(id) == active || fx.chrome;
            if fx.palette
                && Some(id) == active
                && let (Some(stack), Some(tab)) = (&mut self.stack, self.tabs.get(id))
            {
                stack.renderer.set_term_theme(tab.theme.clone());
            }
            if fx.bells > 0
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
            if fx.exited {
                exited.push(id);
            }
        }
        if !exited.is_empty() {
            // The child is gone: its tab closes, the others stay (the last one takes the app).
            self.close_tabs(&exited, event_loop);
            worth_a_frame = true;
        }
        let done = self.pending_new.as_ref().map(|c| c.poll(Instant::now()));
        if let Some(CwdState::Done(dir)) = done {
            self.pending_new = None;
            self.open_tab(dir);
            worth_a_frame = true;
        }
        // A dialog about a tab that is gone has nothing to act on.
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
            worth_a_frame = true;
        }
        worth_a_frame
    }

    /// When a waiting new tab gives up on its directory check.
    fn pending_flow(&self) -> ControlFlow {
        self.pending_new
            .as_ref()
            .map_or(ControlFlow::Wait, |c| ControlFlow::WaitUntil(c.deadline()))
    }

    // ----------------------------------------------------------------------------- the frame

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.occluded {
            return;
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
        if event_loop.exiting() || self.tabs.is_empty() {
            return;
        }
        let (Some(win), Some(stack)) = (self.win.as_mut(), self.stack.as_mut()) else {
            return;
        };
        let frame = match stack.surface.acquire(&stack.renderer.gpu) {
            Ok(Acquired::Frame(frame)) => frame,
            Ok(Acquired::Skip) => {
                // Nothing to draw and nothing to ask for: the platform redraws when it can.
                event_loop.set_control_flow(ControlFlow::Wait);
                self.show_once();
                return;
            }
            Ok(Acquired::Redraw) => {
                win.window.request_redraw();
                return;
            }
            Err(e) => {
                self.fail(event_loop, &e);
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let size = stack.surface.size();
        let scale = win.window.scale_factor() as f32;
        let input = win.translator.take(self.start.elapsed());

        // The clipboard's answer to a paste goes to the tab that asked, as a paste, nowhere else.
        for event in &input.events {
            if let Some((id, text)) = self.paste.answer(event)
                && let Some(session) = self.tabs.get(id).and_then(|t| t.session.as_ref())
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
        let titles: Vec<(TabId, String)> = self
            .tabs
            .iter()
            .map(|(id, t)| (id, t.strip_title().to_owned()))
            .collect();
        let strip: Vec<StripTab<'_>> = titles
            .iter()
            .map(|(id, title)| StripTab { id: *id, title })
            .collect();
        let active_tab = self.tabs.active();
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

        let Some(tab) = self.tabs.active_item_mut() else {
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
            toasts: Some(&mut self.toasts),
            cursor,
            blink_on: self.blink.on(),
            scroll: tab.scroll,
            chrome: HOST,
            ime: tab.session.is_some(),
            preedit: self.preedit.as_deref(),
            clipboard_write: self.clipboard_write.take(),
            clipboard_read: self.paste.take_read(),
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
        if out
            .clipboard
            .iter()
            .any(|r| matches!(r, ClipboardRequest::Read))
        {
            win.window.request_redraw();
        }
        win.window.pre_present_notify();
        stack.surface.present(&stack.renderer.gpu, frame);
        let Stack { renderer, .. } = stack;
        renderer.gpu.end_frame(&mut renderer.text);

        let (flow, redraw) = control_flow(out.repaint, self.start);
        let blink_flow = next_blink.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
        let base_flow = merge_flow(flow, blink_flow);
        if redraw {
            win.window.request_redraw();
        }

        // What the frame asked of the tabs and the dialog, now that the frame is out.
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
        event_loop.set_control_flow(merge_flow(base_flow, self.pending_flow()));
        self.show_once();
        if self
            .stack
            .as_ref()
            .is_some_and(|s| s.renderer.gpu.is_lost())
        {
            self.rebuild(event_loop);
        }
    }

    // ----------------------------------------------------------------------------- keyboard, IME

    /// A key event (D4): the shortcut table, then the key router; the translator never sees it
    /// (unless a dialog is open: then the toolkit takes the keyboard, see `window_event`).
    fn on_key(&mut self, event: &KeyEvent, synthetic: bool, event_loop: &ActiveEventLoop) {
        let input = key_input(event, self.mods, synthetic);
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
                self.send(Command::Key(key));
            }
            KeyRoute::Text(text) if live => {
                self.typed();
                self.send(Command::Text(text));
            }
            _ => {}
        }
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
                    self.send(Command::Text(text.clone()));
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
        let placed = self.placed?;
        let tab = self.tabs.active_item()?;
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
        self.placed.is_some_and(|p| p.layout.body.contains(pos))
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
        self.pointer = Some(pos);
        if self.modal.is_some() {
            return;
        }
        let Some(placed) = self.placed else { return };
        if self.selecting {
            self.drag_selection(pos);
            return;
        }
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
        if self.modal.is_some() {
            return;
        }
        let (Some(pos), Some(placed)) = (self.pointer, self.placed) else {
            return;
        };
        if button == MouseButton::Left {
            if state.is_pressed() {
                if let Some(scroll) = self.scroll_view()
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
                if button == TerminalMouseButton::Left && std::mem::take(&mut self.selecting) {
                    self.send(Command::Select(SelectCmd::End));
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
        if self.modal.is_some() {
            return;
        }
        let (Some(pos), Some(placed)) = (self.pointer, self.placed) else {
            return;
        };
        if !self.has_session() || !placed.layout.body.contains(pos) {
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
        self.glass = apply_material(&window, &self.settings);
        let stack = match Stack::build(
            event_loop,
            &window,
            &self.settings,
            self.glass,
            &Self::base_theme(),
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
        self.open_tab(None);
        // The first frame is drawn here, not on a redraw event a hidden window may never get;
        // the window is shown right after it.
        self.redraw(event_loop);
        if let Some(win) = &self.win {
            win.window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
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
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // A `Repaint::At` or blink deadline came due.
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(win) = &self.win {
            win.window.set_visible(false);
        }
        self.shutdown();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // After `exit()` winit still delivers queued events; ignore them so no frame is acquired.
        if event_loop.exiting() {
            return;
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
            } if self.modal.is_none() => {
                self.on_key(event, *is_synthetic, event_loop);
                return;
            }
            WindowEvent::Ime(ime) if self.modal.is_none() => {
                self.on_ime(ime);
                return;
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(focused) => {
                self.focused = *focused;
                if let Some(tab) = self.tabs.active_item_mut() {
                    tab.set_focus(*focused);
                }
                if !focused {
                    self.thumb_drag = None;
                    if std::mem::take(&mut self.selecting) {
                        self.send(Command::Select(SelectCmd::End));
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(stack) = &mut self.stack {
                    stack
                        .surface
                        .resize(&stack.renderer.gpu, size.width, size.height);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(win) = &self.win {
                    let scale = win.window.scale_factor();
                    self.on_cursor_moved((
                        (position.x / scale) as f32,
                        (position.y / scale) as f32,
                    ));
                }
            }
            WindowEvent::CursorLeft { .. } => self.pointer = None,
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_button(*state, *button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(*delta),
            _ => {}
        }
        let Some(win) = &mut self.win else { return };
        win.translator.window_event(&event);
        if !self.occluded {
            win.window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
