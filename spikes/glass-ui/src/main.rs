//! UI go/no-go spike: a Lunegit-style glass terminal window with mock data.
//!
//! "…" (or right-click a tab) opens the context menu; Settings… (⌘,) opens the settings page.
//! ⌘T / ⌘W / ⌘1–9 manage tabs; R toggles "running" on the active tab (to preview the outline
//! animation); Esc closes menus, dialogs and settings. Drag in the body to select; hold ⌘ over
//! the URL for the link underline.

#![allow(unreachable_pub)] // binary crate: `pub` marks items shared between its modules

mod backdrop;
mod chrome;
mod gpu;
mod mock;
mod overlays;
mod paint;
mod settings_view;
mod state;
mod terminal_view;
mod text;
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

use gpu::Gpu;
use paint::{LAYER_COUNT, Painter, Rect};
use state::{Effect, Hit, MenuKind, Modal, Page, Popup, State};
use text::TextCache;

struct Renderer {
    gpu: Gpu,
    text: TextCache,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    /// One text renderer per paint layer, so text interleaves correctly with shapes.
    layers: Vec<TextRenderer>,
}

struct App {
    renderer: Option<Renderer>,
    state: State,
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        renderer: None,
        state: State::new(),
    };
    event_loop.run_app(&mut app).expect("run app");
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
        let mut attrs = Window::default_attributes()
            .with_title("Luneterm")
            .with_inner_size(LogicalSize::new(1040.0, 660.0))
            .with_min_inner_size(LogicalSize::new(560.0, 360.0))
            .with_transparent(true);
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        if !cfg!(target_os = "macos") {
            self.state.settings.glass = state::GlassStyle::Off;
        }
        apply_glass(&window, &self.state);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
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
        let text = TextCache::new(window.scale_factor() as f32);
        window.request_redraw();
        self.renderer = Some(Renderer {
            gpu,
            text,
            swash: SwashCache::new(),
            viewport,
            atlas,
            layers,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.renderer else { return };
        let st = &mut self.state;
        let window = r.gpu.window.clone();
        let scale = window.scale_factor() as f32;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => r.gpu.resize(size.width, size.height),
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
                            let _ = window.drag_window();
                        }
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
                    st.popup = Some(Popup::Menu {
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
        let fading = st.switched_at.elapsed().as_secs_f32() < terminal_view::SWITCH_FADE;
        if fading || st.tabs.iter().any(|t| t.running) {
            r.gpu.window.request_redraw();
        }
    }
}

fn render(r: &mut Renderer, st: &mut State, s: f32) -> Result<(), String> {
    let win = Rect::new(
        0.0,
        0.0,
        r.gpu.config.width as f32 / s,
        r.gpu.config.height as f32 / s,
    );
    r.text.set_scale(s);
    let mut p = Painter::new(s, win, &mut r.text, st.hover);
    terminal_view::draw(&mut p, st);
    settings_view::draw(&mut p, st);
    overlays::draw_popup(&mut p, st);
    overlays::draw_modal(&mut p, st);
    let Painter { layers, hits, .. } = p;
    st.hits = hits;
    st.hover = st.hit_at(st.mouse);

    r.viewport.update(
        &r.gpu.queue,
        Resolution {
            width: r.gpu.config.width,
            height: r.gpu.config.height,
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
                scale: 1.0,
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
    let geometry: Vec<gpu::LayerGeometry> = layers.into_iter().map(|l| l.geometry).collect();
    let uploaded = r.gpu.upload(&geometry, s);
    let view = frame.texture.create_view(&Default::default());
    let mut encoder = r.gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        for (i, renderer) in r.layers.iter().enumerate() {
            r.gpu.draw_below_text(&mut pass, &uploaded, i);
            renderer
                .render(&r.atlas, &r.viewport, &mut pass)
                .map_err(|e| e.to_string())?;
            r.gpu.draw_above_text(&mut pass, &uploaded, i);
        }
    }
    r.gpu.queue.submit(Some(encoder.finish()));
    r.gpu.window.pre_present_notify();
    r.gpu.queue.present(frame);
    r.atlas.trim();
    r.text.end_frame();
    Ok(())
}
