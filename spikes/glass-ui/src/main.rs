//! UI go/no-go spike: a Lunegit-style glass terminal window with mock data.
//!
//! "…" (or right-click a tab) opens the context menu; Settings… (⌘,) opens the settings page.
//! ⌘T / ⌘W / ⌘1–9 manage tabs; R toggles "running" on the active tab (to preview the outline
//! animation); Esc closes menus, dialogs and settings. Drag in the body to select; hold ⌘ over
//! the URL for the link underline.

#![allow(unreachable_pub)] // binary crate: `pub` marks items shared between its modules

mod backdrop;
mod dump;
mod mock;
mod overlays;
mod settings_view;
mod state;
mod term_grid;
mod terminal_view;
mod theme;
mod widgets;

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use lune_ui::custom::CustomDraw;
use lune_ui::frame::{FrameParams, FrameRenderer};
use lune_ui::gpu::{self, Gpu};
use lune_ui::lunegit_dark;
use lune_ui::paint::{Painter, Rect};
use lune_ui::text::TextCache;
use lune_ui::window::{self, DoubleClick, edge_at};
use state::{Effect, Hit, MenuKind, Modal, Page, Popup, State};
use term_grid::TermState;

/// Luneium context-menu glass: blur 30px, saturate 1.8, then acrylic on top.
const BACKDROP_SATURATION: f32 = 1.8;

struct Renderer {
    gpu: Gpu,
    text: TextCache,
    frame: FrameRenderer,
    dumped: bool,
    /// Termview demo (`GLASS_UI_TERMVIEW=1`, read once at startup); `None` = glyphon body.
    term: Option<TermState>,
}

struct App {
    renderer: Option<Renderer>,
    state: State,
    /// Process exit status (non-zero when a frame dump failed).
    exit_code: u8,
    title_click: DoubleClick,
    /// Borderless window edge under the mouse (Windows only).
    resize_edge: Option<winit::window::ResizeDirection>,
}

fn main() -> std::process::ExitCode {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        renderer: None,
        state: State::new(),
        exit_code: 0,
        title_click: DoubleClick::new(),
        resize_edge: None,
    };
    event_loop.run_app(&mut app).expect("run app");
    std::process::ExitCode::from(app.exit_code)
}

fn apply_glass(window: &Window, st: &State) {
    let mode = st.settings.glass_mode();
    if let Err(e) = backdrop::apply(window, mode) {
        eprintln!("glass ({}): {e}", mode.label());
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        // Frame-dump runs render offscreen only; keep their window out of sight.
        let attrs = window::glass_attributes(
            "Luneterm",
            (1040.0, 660.0),
            (560.0, 360.0),
            dump_dir().is_none(),
        );
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            self.state.settings.glass = state::GlassStyle::Off;
        }
        apply_glass(&window, &self.state);

        let instance = gpu::instance(event_loop.owned_display_handle());
        let gpu = pollster::block_on(Gpu::new(window.clone(), &instance)).expect("gpu");
        eprintln!(
            "surface: {:?}, alpha {:?}, scale {}",
            gpu.config.format,
            gpu.alpha_mode,
            window.scale_factor()
        );
        let text = TextCache::new(
            window.scale_factor() as f32,
            &[
                include_bytes!("../assets/RobotoMonoNerdFontMono-Regular.ttf").as_slice(),
                include_bytes!("../assets/RobotoMonoNerdFontMono-Bold.ttf"),
                include_bytes!("../assets/RznSans-Regular.ttf"),
                include_bytes!("../assets/RznSans-Medium.ttf"),
            ],
            theme::UI_FONT,
            theme::MONO_FONT,
        );
        // Test hook: open the "…" menu at start (exercises the blur path without clicking).
        if let Some(v) = std::env::var_os("GLASS_UI_OPEN_MENU") {
            if v == "colors" {
                self.state.active = self.state.tabs.len() - 1;
            }
            self.state.open_popup(Popup::Menu {
                kind: MenuKind::App,
                at: (500.0, 120.0),
            });
        }
        let frame = FrameRenderer::new(&gpu);
        let term = termview_enabled()
            .then(|| TermState::new(&gpu.device, gpu.config.format, &self.state.settings));
        let mut renderer = Renderer {
            gpu,
            text,
            frame,
            dumped: false,
            term,
        };
        // Scripted offscreen frames (`GLASS_UI_DUMP_FRAMES=<dir>`): render, write, exit.
        // winit still drains events after `exit()`; `window_event` and `about_to_wait`
        // return early once exiting, so nothing reaches `render()` on this path.
        if let Some(dir) = dump_dir() {
            if let Err(e) = dump::dump_frames(&mut renderer, &mut self.state, &dir) {
                eprintln!("dump: {e}");
                self.exit_code = 1;
            }
            self.renderer = Some(renderer);
            event_loop.exit();
            return;
        }
        window.request_redraw();
        self.renderer = Some(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // After `exit()` winit still delivers queued events (macOS drains `RedrawRequested`,
        // `Resized`, `Focused`, ...). Ignore them so no frame is acquired or presented.
        if event_loop.exiting() {
            return;
        }
        let Some(r) = &mut self.renderer else { return };
        let st = &mut self.state;
        let window = r.gpu.window.clone();
        let scale = window.scale_factor() as f32;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                r.gpu.resize(size.width, size.height);
                st.maximized = window.is_maximized();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                r.text.set_scale(scale_factor as f32);
            }
            WindowEvent::Focused(focused) => st.focused = focused,
            WindowEvent::ModifiersChanged(m) => st.cmd = m.state().super_key(),
            WindowEvent::CursorMoved { position, .. } => {
                st.mouse = (position.x as f32 / scale, position.y as f32 / scale);
                st.hover = st.hit_at(st.mouse);
                if st.selecting {
                    let cell = terminal_view::cell_at(st);
                    if let Some(sel) = &mut st.tabs[st.active].selection {
                        sel.1 = cell;
                    }
                }
                self.resize_edge = if cfg!(target_os = "windows") && !st.maximized {
                    let size = window.inner_size();
                    edge_at(
                        st.mouse,
                        size.width as f32 / scale,
                        size.height as f32 / scale,
                    )
                } else {
                    None
                };
                if let Some(edge) = self.resize_edge {
                    window.set_cursor(CursorIcon::from(edge));
                    window.request_redraw();
                    return;
                }
                let on_link =
                    st.hover == Some(Hit::Body) && st.cmd && terminal_view::link_at(st).is_some();
                window.set_cursor(match st.hover {
                    _ if on_link => CursorIcon::Pointer,
                    Some(Hit::Body | Hit::Field(_)) => CursorIcon::Text,
                    _ => CursorIcon::Default,
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if matches!(st.page, Page::Settings(_)) && st.popup.is_none() && st.modal.is_none()
                {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * 40.0,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / scale,
                    };
                    st.scroll -= dy;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                st.selecting = false;
                let tab = &mut st.tabs[st.active];
                if tab.selection.is_some_and(|(a, b)| a == b) {
                    tab.selection = None;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(edge) = self.resize_edge {
                    let _ = window.drag_resize_window(edge);
                    return;
                }
                let hit = st.hit_at(st.mouse);
                if hit == Some(Hit::Body) {
                    if st.cmd
                        && let Some(url) = terminal_view::link_at(st)
                    {
                        eprintln!("would open {url}");
                    } else {
                        let cell = terminal_view::cell_at(st);
                        st.tabs[st.active].selection = Some((cell, cell));
                        st.selecting = true;
                    }
                    st.editing = None;
                } else {
                    match st.click(hit) {
                        Effect::ApplyGlass => apply_glass(&window, st),
                        Effect::DragWindow => {
                            let double = self.title_click.click(std::time::Instant::now());
                            if double && cfg!(target_os = "windows") {
                                window.set_maximized(!window.is_maximized());
                            } else {
                                let _ = window.drag_window();
                            }
                        }
                        Effect::Caption(state::Caption::Minimize) => window.set_minimized(true),
                        Effect::Caption(state::Caption::Maximize) => {
                            window.set_maximized(!window.is_maximized());
                        }
                        Effect::Caption(state::Caption::Close) => event_loop.exit(),
                        Effect::None => {}
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                if let Some(Hit::Tab(i) | Hit::TabClose(i)) = st.hit_at(st.mouse)
                    && st.popup.is_none()
                    && st.modal.is_none()
                {
                    st.open_popup(Popup::Menu {
                        kind: MenuKind::Tab(i),
                        at: st.mouse,
                    });
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let typing = matches!(st.modal, Some(Modal::Rename { .. })) || st.editing.is_some();
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => st.escape(),
                    Key::Named(NamedKey::Enter) if st.modal.is_some() => {
                        st.dialog(state::DialogBtn::Confirm);
                    }
                    Key::Named(NamedKey::Enter) if typing => st.editing = None,
                    Key::Named(NamedKey::Backspace) if typing => st.backspace(),
                    Key::Character(ch) if st.cmd => match ch.to_lowercase().as_str() {
                        "t" => st.new_tab(),
                        "w" => st.request_close(vec![st.active]),
                        "," => {
                            st.click(Some(Hit::Menu(state::MenuCmd::Settings)));
                        }
                        d => {
                            if let Some(i) = d.parse::<usize>().ok().and_then(|n| n.checked_sub(1))
                            {
                                st.activate(i);
                            }
                        }
                    },
                    _ if typing => {
                        if let Some(text) = &event.text {
                            st.type_text(text);
                        }
                    }
                    Key::Character(ch)
                        if ch.eq_ignore_ascii_case("r")
                            && st.page == Page::Terminal
                            && st.modal.is_none() =>
                    {
                        let tab = &mut st.tabs[st.active];
                        tab.running = !tab.running;
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = render(r, st, scale) {
                    eprintln!("render: {e}");
                }
                return;
            }
            _ => return,
        }
        window.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        let Some(r) = &self.renderer else { return };
        let st = &self.state;
        let now = std::time::Instant::now();
        let fading = now.saturating_duration_since(st.switched_at).as_secs_f32()
            < terminal_view::SWITCH_FADE;
        if fading || st.tabs.iter().any(|t| t.running) || overlays::popup_animating(st, now) {
            r.gpu.window.request_redraw();
        }
    }
}

/// `GLASS_UI_DUMP_FRAMES`: the directory for scripted offscreen frame dumps, if requested.
fn dump_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("GLASS_UI_DUMP_FRAMES")
        .filter(|v| !v.is_empty())
        .map(Into::into)
}

/// `GLASS_UI_TERMVIEW=1`: draw the terminal body with `lune-termview` instead of glyphon text.
fn termview_enabled() -> bool {
    std::env::var_os("GLASS_UI_TERMVIEW").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Draws one frame into the window's surface.
fn render(r: &mut Renderer, st: &mut State, s: f32) -> Result<(), String> {
    st.now = std::time::Instant::now();
    let Some(frame) = r.gpu.acquire()? else {
        return Ok(());
    };
    let view = frame.texture.create_view(&Default::default());
    let size = (r.gpu.config.width, r.gpu.config.height);
    draw_frame(r, st, s, &view, size)?;
    r.gpu.present(frame);
    r.frame.end_frame(&mut r.text);
    Ok(())
}

/// Paints and submits one frame of `size` physical pixels at scale `s` into `target`.
/// Serves both the window surface and the offscreen frame dumps.
fn draw_frame(
    r: &mut Renderer,
    st: &mut State,
    s: f32,
    target: &wgpu::TextureView,
    size: (u32, u32),
) -> Result<(), String> {
    let win = Rect::new(0.0, 0.0, size.0 as f32 / s, size.1 as f32 / s);
    r.text.set_scale(s);
    let mut p = Painter::new(s, win, &mut r.text, st.hover, lunegit_dark::THEME);
    terminal_view::draw(&mut p, st, r.term.as_mut());
    settings_view::draw(&mut p, st);
    overlays::draw_popup(&mut p, st);
    overlays::draw_modal(&mut p, st);
    let Painter {
        layers,
        hits,
        backdrop,
        ..
    } = p;
    st.hits = hits;
    st.hover = st.hit_at(st.mouse);

    let params = FrameParams {
        size,
        scale: s,
        blur_sigma: st.settings.menu_blur.sigma(),
        saturation: BACKDROP_SATURATION,
    };
    match r.term.as_mut() {
        Some(term) => {
            term.update(&mut r.text, st, s);
            let mut customs: [&mut dyn CustomDraw; 1] = [&mut term.view];
            r.frame.draw_with(
                &r.gpu,
                &mut r.text,
                layers,
                backdrop,
                target,
                params,
                &mut customs,
            )?;
        }
        None => r
            .frame
            .draw(&r.gpu, &mut r.text, layers, backdrop, target, params)?,
    }
    // Debug: GLASS_UI_DUMP=<dir> writes the scene and blurred textures once a menu is fully open.
    if backdrop.is_some_and(|b| b.alpha >= 0.999)
        && let Some(dir) = std::env::var_os("GLASS_UI_DUMP")
        && !r.dumped
    {
        r.dumped = true;
        r.frame
            .blur()
            .dump(&r.gpu.device, &r.gpu.queue, std::path::Path::new(&dir));
    }
    Ok(())
}
