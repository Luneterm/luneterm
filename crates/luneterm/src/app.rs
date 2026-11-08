//! The app: the event loop, the window, the frame loop and the session wiring (plan M9 steps 4
//! and 6).
//!
//! Composition follows the toolkit's gallery (`apps/gallery/src/window.rs`): the app owns the event
//! loop, the window, the GPU stack and the clock. The window is created hidden and shown after its
//! first frame. A frame is `Translator::take` → [`Renderer::draw`] → `OutputState::apply` →
//! `Clipboard::service` → present, and the frame's `Repaint` (merged with the cursor blink) sets
//! the loop's control flow, so an idle window waits with no timer.
//!
//! The terminal owns the keyboard (D4): key and IME events go to the session (through the shortcut
//! table and the key router) and never reach the toolkit's translator, so `Ui::begin` cannot eat
//! Tab or Escape. Modifier, pointer, wheel, focus, resize and scale events always reach it. The
//! session runs on its own threads; this thread only sends it commands and takes its Updates, one
//! per frame (D1, D2).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{
    ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{UserAttentionType, Window, WindowId};

use lune_pty::PortablePtyBackend;
use lune_termview::{Cell, Cursor, CursorShape, Grid, TermTheme};
use lune_ui::clipboard::Clipboard;
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::{ClipboardRequest, Event, OutputState, Translator, control_flow, keymap};
use lune_ui::material::{self, Material};
use lune_ui::surface::{Acquired, WindowSurface};
use lune_ui::window::glass_attributes;
use lune_vt::{CursorStyle, MouseEncoding, MouseTracking, TerminalMouseButton, ViewportCursor};

use crate::dump;
use crate::fonts;
use crate::input::{
    AppCommand, AppShortcuts, KeyRoute, MouseCtx, MouseState, PointerEvent, RouteCtx, WheelAction,
    WheelCtx, WheelState, key_input, mouse_to_terminal, pointer_event, route_key, terminal_mods,
    wheel, wheel_delta, wheel_report,
};
use crate::render::{Content, Placed, Renderer};
use crate::session::{
    Command, Geometry, HostPalette, Scroll, Session, SessionConfig, Update, UpdateModes,
    apply_update,
};
use crate::settings::Settings;
use crate::shell::{self, HOST, ScrollView, ThumbDrag};
use crate::spawn::{self, SpawnFailure};
use crate::title::{self, DEFAULT_TITLE};

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

/// A paste in flight (D8): Cmd+V or Ctrl+Shift+V asks the toolkit for the clipboard's text, which
/// arrives in a later frame's input. The text goes to the PTY only through `Command::Paste`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PasteState {
    /// A paste was asked for and not answered yet.
    pending: bool,
    /// The next frame must carry the read request.
    read: bool,
}

impl PasteState {
    /// A paste shortcut was pressed. A second press while one is pending does nothing.
    pub fn request(&mut self) {
        if !self.pending {
            self.pending = true;
            self.read = true;
        }
    }

    /// Whether the frame being built carries the read request (once per paste).
    pub fn take_read(&mut self) -> bool {
        std::mem::take(&mut self.read)
    }

    /// An event of a frame's input. Returns the text to paste when it answers a pending paste;
    /// clipboard text nobody asked for is ignored.
    pub fn answer(&mut self, event: &Event) -> Option<String> {
        match event {
            Event::ClipboardText(text) if self.pending => {
                self.pending = false;
                (!text.is_empty()).then(|| text.clone())
            }
            Event::ClipboardUnavailable => {
                self.pending = false;
                None
            }
            _ => None,
        }
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

struct App {
    settings: Settings,
    /// OS glass is behind the window.
    glass: bool,
    win: Option<Win>,
    stack: Option<Stack>,
    proxy: EventLoopProxy<UserEvent>,
    /// The shell's session; `None` before the window exists and after a failed start.
    session: Option<Session>,
    /// Why no shell started (D12): the grid tells, keys are ignored.
    failure: Option<SpawnFailure>,
    /// The terminal body: what the session's Updates make of it.
    grid: Grid,
    cursor: Cursor,
    scroll: Option<ScrollView>,
    modes: UpdateModes,
    alt_screen: bool,
    title: String,
    /// The terminal colours now in force (the session palette on the theme).
    theme: TermTheme,
    /// The last frame's placement: the pointer maps through it.
    placed: Option<Placed>,
    /// The geometry the session was last told.
    sent_geometry: Option<Geometry>,
    /// The IME's composing text.
    preedit: Option<String>,
    paste: PasteState,
    /// An OSC 52 write waiting for the next frame.
    clipboard_write: Option<String>,
    shortcuts: AppShortcuts,
    mods: ModifiersState,
    /// The pointer in logical px, `None` outside the window.
    pointer: Option<(f32, f32)>,
    mouse: MouseState,
    /// A mouse press went to the program: so do its moves and its release.
    mouse_captured: bool,
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
            session: None,
            failure: None,
            grid: Grid::new(0, 0),
            cursor: Cursor::default(),
            scroll: None,
            modes: UpdateModes::default(),
            alt_screen: false,
            title: DEFAULT_TITLE.to_owned(),
            theme: TermTheme::lunegit_one_dark(),
            placed: None,
            sent_geometry: None,
            preedit: None,
            paste: PasteState::default(),
            clipboard_write: None,
            shortcuts: AppShortcuts::standard(cfg!(target_os = "macos")),
            mods: ModifiersState::empty(),
            pointer: None,
            mouse: MouseState::default(),
            mouse_captured: false,
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

    /// Ends the session, waiting briefly for its threads so the child is killed before the
    /// process leaves.
    fn shutdown(&mut self) {
        if let Some(session) = self.session.take()
            && !session.close(CLOSE_WAIT)
        {
            eprintln!("luneterm: the session's threads did not stop in time");
        }
    }

    /// Rebuilds the GPU stack after the device was lost; every row is drawn again.
    fn rebuild(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.win.as_ref().map(|w| w.window.clone()) else {
            return;
        };
        self.stack = None;
        match Stack::build(event_loop, &window, &self.settings, self.glass, &self.theme) {
            Ok(stack) => {
                self.stack = Some(stack);
                self.grid.mark_all_dirty();
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

    fn send(&self, command: Command) {
        if let Some(session) = &self.session {
            session.send(command);
        }
    }

    // ----------------------------------------------------------------------------- the session

    /// Spawns the shell and starts its session at the window's grid size. On failure the grid
    /// tells why (D12).
    fn start_session(&mut self) {
        let (Some(win), Some(stack)) = (&self.win, &mut self.stack) else {
            return;
        };
        let scale = win.window.scale_factor() as f32;
        let size = win.window.inner_size();
        let logical = (size.width as f32 / scale, size.height as f32 / scale);
        let geometry = stack.renderer.place(logical, scale).geometry();
        let adapter = stack.renderer.gpu.adapter.get_info();

        let (pty, program) =
            match spawn::launch(&PortablePtyBackend, &self.settings, geometry.pty_size()) {
                Ok(started) => started,
                Err(failure) => {
                    eprintln!(
                        "luneterm: failed to start a shell: {:?} ({})",
                        failure.kind, failure.program
                    );
                    self.failure = Some(failure);
                    return;
                }
            };
        let pid = pty.process_id();
        let proxy = self.proxy.clone();
        let config = SessionConfig {
            geometry,
            scrollback: self.settings.scrollback as usize,
            xtversion: spawn::xtversion(),
            clipboard_write: self.settings.osc_clipboard,
            // The ConPTY build comes with the Windows host duties (step 7).
            windows_pty: None,
            palette: HostPalette::from_theme(&self.theme),
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
                self.session = Some(session);
                self.sent_geometry = Some(geometry);
            }
            Err(e) => {
                self.failure = Some(SpawnFailure {
                    kind: e.kind(),
                    program: "session".to_owned(),
                });
                eprintln!("luneterm: session: {:?}", e.kind());
            }
        }
    }

    /// Takes the session's published Update, if any, and applies it.
    fn pump(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(update) = self.session.as_ref().and_then(Session::take_update) {
            self.apply(update, event_loop);
        }
    }

    /// Applies one Update: the grid, the cursor and scroll state, the palette, the title, the
    /// bell, an OSC 52 write and the child's exit.
    fn apply(&mut self, update: Update, event_loop: &ActiveEventLoop) {
        apply_update(&mut self.grid, &update);
        self.cursor = cursor_of(&update.cursor, self.grid.cols());
        self.scroll = Some(ScrollView {
            viewport_top: update.viewport_top,
            base_y: update.base_y,
            total_lines: update.total_lines,
        });
        self.modes = update.modes;
        self.alt_screen = update.alt_screen;

        if let Some(palette) = &update.palette {
            self.theme = theme_with_palette(&self.theme, palette);
            if let Some(stack) = &mut self.stack {
                stack.renderer.set_term_theme(self.theme.clone());
            }
            self.grid.mark_all_dirty();
        }
        if let Some(raw) = &update.title {
            self.title = title::sanitize(raw);
            if let Some(win) = &self.win {
                win.window.set_title(&self.title);
            }
        }
        if update.bells > 0
            && self.bell.due(Instant::now(), self.focused)
            && let Some(win) = &self.win
        {
            win.window
                .request_user_attention(Some(UserAttentionType::Informational));
        }
        if let Some(text) = update.clipboard_write
            && self.settings.osc_clipboard
        {
            self.clipboard_write = Some(text);
        }
        if update.exited.is_some() {
            // The child is gone: the window closes (D12).
            event_loop.exit();
        }
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
        if event_loop.exiting() {
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

        // The clipboard's answer to a paste goes to the session as a paste, nowhere else.
        for event in &input.events {
            if let Some(text) = self.paste.answer(event)
                && let Some(session) = &self.session
            {
                session.send(Command::Paste(text));
            }
        }

        // Where things are in this frame; a changed grid size or cell size is the session's news.
        let placed = stack.renderer.place(input.size, input.scale);
        self.placed = Some(placed);
        let geometry = placed.geometry();
        if geometry.cols > 0
            && geometry.rows > 0
            && self.sent_geometry != Some(geometry)
            && let Some(session) = &self.session
        {
            session.send(Command::Resize(geometry));
            self.sent_geometry = Some(geometry);
        }
        if let Some(failure) = &self.failure {
            let (cols, rows) = (placed.placement.cols, placed.placement.rows);
            if (self.grid.cols(), self.grid.rows()) != (cols, rows) {
                self.grid = failure_grid(cols, rows, failure);
            }
            self.cursor.visible = false;
        }

        let cursor = self.cursor;
        let next_blink = self.blink.tick(
            Instant::now(),
            input.focused && cursor.blink && cursor.visible,
        );
        let content = Content {
            title: &self.title,
            maximized: win.window.is_maximized(),
            grid: &mut self.grid,
            cursor,
            blink_on: self.blink.on(),
            scroll: self.scroll,
            chrome: HOST,
            ime: self.session.is_some(),
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
        let out = drawn.output;
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
        event_loop.set_control_flow(merge_flow(flow, blink_flow));
        if redraw {
            win.window.request_redraw();
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

    // ----------------------------------------------------------------------------- keyboard, IME

    /// A key event (D4): the shortcut table, then the key router; the translator never sees it.
    fn on_key(&mut self, event: &KeyEvent, synthetic: bool) {
        if self.session.is_none() {
            return;
        }
        let input = key_input(event, self.mods, synthetic);
        let route = {
            let ctx = RouteCtx {
                macos: cfg!(target_os = "macos"),
                option_as_meta: self.settings.option_as_meta,
                keymap: keymap(),
                shortcuts: &self.shortcuts,
            };
            route_key(&input, &ctx)
        };
        match route {
            KeyRoute::Shortcut(AppCommand::Paste) => {
                self.paste.request();
                self.request_redraw();
            }
            KeyRoute::Shortcut(AppCommand::Scroll(scroll)) => self.send(Command::Scroll(scroll)),
            KeyRoute::Terminal(key) => {
                self.typed();
                self.send(Command::Key(key));
            }
            KeyRoute::Text(text) => {
                self.typed();
                self.send(Command::Text(text));
            }
            KeyRoute::Ignore => {}
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
                if !text.is_empty() && self.session.is_some() {
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
        Some(MouseCtx {
            layout: placed.grid_layout(),
            cols: self.grid.cols(),
            rows: self.grid.rows(),
            pixel_reports: self.modes.mouse_encoding == MouseEncoding::SgrPixels,
        })
    }

    fn tracking(&self) -> bool {
        self.session.is_some() && self.modes.mouse_tracking != MouseTracking::None
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
        let Some(placed) = self.placed else { return };
        if let Some(drag) = &mut self.thumb_drag {
            if let Some(scroll) = self.scroll {
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
        let (Some(pos), Some(placed)) = (self.pointer, self.placed) else {
            return;
        };
        if button == MouseButton::Left {
            if state.is_pressed() {
                if let Some(scroll) = self.scroll
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
            PointerEvent::Down(_) => {
                if self.tracking() && self.over_body(pos) && self.report(ev, pos) {
                    self.mouse_captured = true;
                }
            }
            PointerEvent::Up(_) => {
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

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let (Some(pos), Some(placed)) = (self.pointer, self.placed) else {
            return;
        };
        if self.session.is_none() || !placed.layout.body.contains(pos) {
            return;
        }
        let Some(delta) = wheel_delta(delta) else {
            return;
        };
        let ctx = WheelCtx {
            tracking: self.modes.mouse_tracking != MouseTracking::None,
            shift: self.mods.shift_key(),
            no_scrollback: self.alt_screen || self.settings.scrollback == 0,
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
        let stack = match Stack::build(event_loop, &window, &self.settings, self.glass, &self.theme)
        {
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
        self.start_session();
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
                if self.occluded {
                    // No frame while hidden, but the child's exit and its bell still act.
                    self.pump(event_loop);
                } else {
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
                event_loop.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = *occluded;
                if let Some(session) = &self.session {
                    session.set_visible(!occluded);
                }
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
            } => {
                self.on_key(event, *is_synthetic);
                return;
            }
            WindowEvent::Ime(ime) => {
                self.on_ime(ime);
                return;
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(focused) => {
                self.focused = *focused;
                self.send(Command::Focus(*focused));
                if !focused {
                    self.thumb_drag = None;
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
    fn a_paste_is_one_read_one_answer_and_unasked_text_is_ignored() {
        let text = |t: &str| Event::ClipboardText(t.to_owned());
        let mut p = PasteState::default();
        // Text nobody asked for goes nowhere.
        assert_eq!(p.answer(&text("surprise")), None);
        assert!(!p.take_read());
        // A request is one read, however many presses.
        p.request();
        p.request();
        assert!(p.take_read());
        assert!(!p.take_read());
        p.request();
        assert!(!p.take_read(), "still pending: no second read");
        // Other events do not answer it; the text does, once.
        assert_eq!(p.answer(&Event::Focus(true)), None);
        assert_eq!(p.answer(&text("hello")), Some("hello".to_owned()));
        assert_eq!(p.answer(&text("again")), None);
        // Unavailable clears a pending paste; empty text pastes nothing.
        p.request();
        assert!(p.take_read());
        assert_eq!(p.answer(&Event::ClipboardUnavailable), None);
        p.request();
        assert!(p.take_read(), "a new paste can be asked for");
        assert_eq!(p.answer(&text("")), None);
        p.request();
        assert!(p.take_read());
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
