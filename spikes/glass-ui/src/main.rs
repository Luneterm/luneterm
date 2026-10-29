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
mod terminal_view;
mod theme;
mod widgets;

use std::sync::Arc;

use glyphon::{
    Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use lune_ui::gpu::Gpu;
use lune_ui::paint::{LAYER_COUNT, Painter, Rect};
use lune_ui::text::TextCache;
use lune_ui::{blur, gpu};
use state::{Effect, Hit, MenuKind, Modal, Page, Popup, State};

/// Luneium context-menu glass: blur 30px, saturate 1.8, then acrylic on top.
const BACKDROP_SATURATION: f32 = 1.8;

struct Renderer {
    gpu: Gpu,
    text: TextCache,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    /// One text renderer per paint layer, so text interleaves correctly with shapes.
    layers: Vec<TextRenderer>,
    /// Offscreen scene + blur for glass menus.
    blur: blur::Blur,
    dumped: bool,
}

struct App {
    renderer: Option<Renderer>,
    state: State,
    /// Process exit status (non-zero when a frame dump failed).
    exit_code: u8,
    last_title_click: Option<std::time::Instant>,
    /// Borderless window edge under the mouse (Windows only).
    resize_edge: Option<winit::window::ResizeDirection>,
}

fn main() -> std::process::ExitCode {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        renderer: None,
        state: State::new(),
        exit_code: 0,
        last_title_click: None,
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
        // `attrs` is only reassigned under the macOS/Windows cfg blocks below.
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title("Luneterm")
            .with_inner_size(LogicalSize::new(1040.0, 660.0))
            .with_min_inner_size(LogicalSize::new(560.0, 360.0))
            .with_transparent(true)
            // Frame-dump runs render offscreen only; keep their window out of sight.
            .with_visible(dump_dir().is_none());
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        #[cfg(target_os = "windows")]
        {
            // Borderless so the glass reaches the top edge; we draw the caption buttons. No
            // redirection bitmap: wgpu presents through DirectComposition, which keeps alpha.
            use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
            attrs = attrs
                .with_decorations(false)
                .with_no_redirection_bitmap(true)
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round)
                .with_theme(Some(winit::window::Theme::Dark));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            self.state.settings.glass = state::GlassStyle::Off;
        }
        apply_glass(&window, &self.state);

        #[allow(unused_mut)]
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        ));
        #[cfg(target_os = "windows")]
        {
            // DX12 through a DirectComposition visual is the wgpu path that supports transparency.
            desc.backends = wgpu::Backends::DX12;
            desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }
        let instance = wgpu::Instance::new(desc);
        let gpu = pollster::block_on(Gpu::new(window.clone(), &instance)).expect("gpu");
        eprintln!(
            "surface: {:?}, alpha {:?}, scale {}",
            gpu.config.format,
            gpu.alpha_mode,
            window.scale_factor()
        );
        let cache = Cache::new(&gpu.device);
        let viewport = Viewport::new(&gpu.device, &cache);
        let mut atlas = TextAtlas::with_color_mode(
            &gpu.device,
            &gpu.queue,
            &cache,
            gpu.config.format,
            glyphon::ColorMode::Web,
        );
        let layers = (0..LAYER_COUNT)
            .map(|_| {
                TextRenderer::new(
                    &mut atlas,
                    &gpu.device,
                    wgpu::MultisampleState::default(),
                    None,
                )
            })
            .collect();
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
        window.request_redraw();
        let blur = blur::Blur::new(&gpu.device, gpu.config.format);
        let mut renderer = Renderer {
            blur,
            dumped: false,
            gpu,
            text,
            swash: SwashCache::new(),
            viewport,
            atlas,
            layers,
        };
        // Scripted offscreen frames (`GLASS_UI_DUMP_FRAMES=<dir>`): render, write, exit.
        if let Some(dir) = dump_dir() {
            if let Err(e) = dump::dump_frames(&mut renderer, &mut self.state, &dir) {
                eprintln!("dump: {e}");
                self.exit_code = 1;
            }
            self.renderer = Some(renderer);
            event_loop.exit();
            return;
        }
        self.renderer = Some(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
                            let now = std::time::Instant::now();
                            let double = self.last_title_click.is_some_and(|t| {
                                now.duration_since(t) < std::time::Duration::from_millis(400)
                            });
                            self.last_title_click = Some(now);
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
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

/// Which edge of a borderless window the mouse is on, for resizing.
fn edge_at(p: (f32, f32), w: f32, h: f32) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection as D;
    const B: f32 = 6.0;
    let (l, r, t, b) = (p.0 < B, p.0 >= w - B, p.1 < B, p.1 >= h - B);
    Some(match (l, r, t, b) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (.., true) => D::South,
        _ => return None,
    })
}

/// `GLASS_UI_DUMP_FRAMES`: the directory for scripted offscreen frame dumps, if requested.
fn dump_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("GLASS_UI_DUMP_FRAMES")
        .filter(|v| !v.is_empty())
        .map(Into::into)
}

/// Draws one frame into the window's surface.
fn render(r: &mut Renderer, st: &mut State, s: f32) -> Result<(), String> {
    st.now = std::time::Instant::now();
    let frame = match r.gpu.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t) => t,
        wgpu::CurrentSurfaceTexture::Outdated
        | wgpu::CurrentSurfaceTexture::Suboptimal(_)
        | wgpu::CurrentSurfaceTexture::Lost => {
            let (w, h) = (r.gpu.config.width, r.gpu.config.height);
            r.gpu.resize(w, h);
            r.gpu.window.request_redraw();
            return Ok(());
        }
        // Hidden or minimised: skip; macOS sends a redraw when the window is visible again.
        wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
            return Ok(());
        }
        other => return Err(format!("surface: {other:?}")),
    };
    let view = frame.texture.create_view(&Default::default());
    let size = (r.gpu.config.width, r.gpu.config.height);
    draw_frame(r, st, s, &view, size)?;
    r.gpu.window.pre_present_notify();
    r.gpu.queue.present(frame);
    r.atlas.trim();
    r.text.end_frame();
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
    let mut p = Painter::new(s, win, &mut r.text, st.hover);
    terminal_view::draw(&mut p, st);
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

    r.viewport.update(
        &r.gpu.queue,
        Resolution {
            width: size.0,
            height: size.1,
        },
    );
    for (layer, renderer) in layers.iter().zip(&mut r.layers) {
        let (fonts, buffer_of) = r.text.split();
        let areas = layer.texts.iter().filter_map(|t| {
            let buffer = buffer_of(t.key)?;
            let c = t.clip;
            Some(TextArea {
                buffer,
                left: t.x * s,
                top: t.y * s,
                scale: t.scale,
                bounds: TextBounds {
                    left: (c.x * s) as i32,
                    top: (c.y * s) as i32,
                    right: (c.right() * s) as i32,
                    bottom: (c.bottom() * s) as i32,
                },
                default_color: Color::rgb(0xCC, 0xCC, 0xCC),
                custom_glyphs: &[],
            })
        });
        renderer
            .prepare(
                &r.gpu.device,
                &r.gpu.queue,
                fonts,
                &mut r.atlas,
                &r.viewport,
                areas,
                &mut r.swash,
            )
            .map_err(|e| e.to_string())?;
    }

    let geometry: Vec<gpu::LayerGeometry> = layers.into_iter().map(|l| l.geometry).collect();
    let uploaded = r.gpu.upload(&geometry, size, s);
    let sigma = st.settings.menu_blur.sigma();
    r.blur.prepare(&r.gpu.device, &r.gpu.queue, size, s, sigma);
    let mut encoder = r.gpu.device.create_command_encoder(&Default::default());
    // Layers under popups go to an offscreen scene, so a glass menu can blur them.
    const SCENE_LAYERS: usize = 2;
    let pass_desc = |target: &wgpu::TextureView,
                     f: &mut dyn FnMut(&mut wgpu::RenderPass<'_>) -> Result<(), String>,
                     encoder: &mut wgpu::CommandEncoder| {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        f(&mut pass)
    };
    let scene = r.blur.scene_view().ok_or("no scene target")?;
    pass_desc(
        scene,
        &mut |pass| {
            for i in 0..SCENE_LAYERS {
                r.gpu.draw_below_text(pass, &uploaded, i);
                r.layers[i]
                    .render(&r.atlas, &r.viewport, pass)
                    .map_err(|e| e.to_string())?;
                r.gpu.draw_above_text(pass, &uploaded, i);
            }
            Ok(())
        },
        &mut encoder,
    )?;
    if let Some(b) = backdrop {
        r.blur.set_backdrop(&r.gpu.queue, b, BACKDROP_SATURATION);
        r.blur.run(&mut encoder);
    }
    pass_desc(
        target,
        &mut |pass| {
            r.blur.blit_scene(pass);
            if backdrop.is_some() {
                r.blur.draw_backdrop(pass);
            }
            for i in SCENE_LAYERS..r.layers.len() {
                r.gpu.draw_below_text(pass, &uploaded, i);
                r.layers[i]
                    .render(&r.atlas, &r.viewport, pass)
                    .map_err(|e| e.to_string())?;
                r.gpu.draw_above_text(pass, &uploaded, i);
            }
            Ok(())
        },
        &mut encoder,
    )?;
    r.gpu.queue.submit(Some(encoder.finish()));
    // Debug: GLASS_UI_DUMP=<dir> writes the scene and blurred textures once a menu is fully open.
    if backdrop.is_some_and(|b| b.alpha >= 0.999)
        && let Some(dir) = std::env::var_os("GLASS_UI_DUMP")
        && !r.dumped
    {
        r.dumped = true;
        r.blur
            .dump(&r.gpu.device, &r.gpu.queue, std::path::Path::new(&dir));
    }
    Ok(())
}
