//! The app: the event loop, the window and the frame loop (plan M9 step 4).
//!
//! Composition follows the toolkit's gallery (`apps/gallery/src/window.rs`): the app owns the event
//! loop, the window, the GPU stack and the clock. The window is created hidden and shown after its
//! first frame. A frame is `Translator::take` → [`Renderer::draw`] → `OutputState::apply` →
//! `Clipboard::service` → present, and the frame's `Repaint` (merged with the cursor blink) sets
//! the loop's control flow, so an idle window waits with no timer.
//!
//! The terminal owns the keyboard (D4): key and IME events do not reach the toolkit's translator.
//! Step 6 routes them to the session.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use lune_termview::Grid;
use lune_ui::clipboard::Clipboard;
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::{ClipboardRequest, OutputState, Translator, control_flow};
use lune_ui::material::{self, Material};
use lune_ui::surface::{Acquired, WindowSurface};
use lune_ui::window::glass_attributes;

use crate::demo::{demo_cursor, demo_grid};
use crate::dump;
use crate::fonts;
use crate::render::{Content, Renderer};
use crate::settings::Settings;
use crate::shell::{self, HOST};

/// The window's first size and its minimum, logical px.
const WINDOW_SIZE: (f64, f64) = (1040.0, 660.0);
const WINDOW_MIN: (f64, f64) = (560.0, 360.0);
/// The cursor blink half-period (xterm.js and Lunegit's 600 ms).
const BLINK_PERIOD: Duration = Duration::from_millis(600);
/// The title until a session sets one.
const DEFAULT_TITLE: &str = "Luneterm";

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
    let mut app = App {
        settings,
        glass: false,
        win: None,
        stack: None,
        grid: Grid::new(0, 0),
        start: Instant::now(),
        blink: Blink::default(),
        occluded: false,
        shown: false,
        exit_code: 0,
    };
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
    ) -> Result<Stack, String> {
        let instance = gpu::instance(Some(Box::new(event_loop.owned_display_handle())));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface)))?;
        let size = window.inner_size();
        let surface = WindowSurface::new(&gpu, surface, (size.width, size.height))?;
        let (text, _took) = fonts::text_cache();
        let renderer = Renderer::new(gpu, text, surface.format(), settings, glass);
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
    /// The terminal body: a static placeholder until step 6 wires the session in.
    grid: Grid,
    /// The app's time origin (`RawInput.time` and `Repaint::At` count from it).
    start: Instant,
    blink: Blink,
    occluded: bool,
    shown: bool,
    exit_code: u8,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, msg: &str) {
        eprintln!("{msg}");
        self.exit_code = 1;
        event_loop.exit();
    }

    /// Rebuilds the GPU stack after the device was lost; every row is drawn again.
    fn rebuild(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.win.as_ref().map(|w| w.window.clone()) else {
            return;
        };
        self.stack = None;
        match Stack::build(event_loop, &window, &self.settings, self.glass) {
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

        let cursor = demo_cursor();
        let next_blink = self
            .blink
            .tick(Instant::now(), input.focused && cursor.blink);
        let content = Content {
            title: DEFAULT_TITLE,
            maximized: win.window.is_maximized(),
            grid: &mut self.grid,
            cursor,
            blink_on: self.blink.on(),
            scroll: None,
            chrome: HOST,
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
        self.glass = apply_material(&window, &self.settings);
        let stack = match Stack::build(event_loop, &window, &self.settings, self.glass) {
            Ok(s) => s,
            Err(e) => {
                self.fail(event_loop, &e);
                return;
            }
        };
        self.grid = demo_grid(0, 0);
        self.win = Some(Win {
            translator: Translator::new(window.inner_size(), window.scale_factor()),
            output: OutputState::new(),
            clipboard: Clipboard::new(),
            window,
        });
        self.stack = Some(stack);
        // The first frame is drawn here, not on a redraw event a hidden window may never get;
        // the window is shown right after it.
        self.redraw(event_loop);
        if let Some(win) = &self.win {
            win.window.request_redraw();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                if let Some(win) = self.win.as_ref().filter(|_| !self.occluded) {
                    win.window.request_redraw();
                }
            }
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // A `Repaint::At` or blink deadline came due.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && !self.occluded
            && let Some(win) = &self.win
        {
            win.window.request_redraw();
        }
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
                if !occluded && let Some(win) = &self.win {
                    win.window.request_redraw();
                }
                return;
            }
            // The terminal owns the keyboard (D4): step 6 routes these to the session.
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => return,
            WindowEvent::Resized(size) => {
                if let Some(stack) = &mut self.stack {
                    stack
                        .surface
                        .resize(&stack.renderer.gpu, size.width, size.height);
                }
            }
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
}
