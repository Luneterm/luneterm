//! Scripted offscreen frame dumps (`GLASS_UI_DUMP_FRAMES=<dir>`): the pixel instrument that the
//! library extraction is gated on.
//!
//! Each scene renders into its own offscreen texture of the surface format with a frozen clock,
//! and the texture is read back (GPU readback only: no window or screen capture) and written as
//! an RGBA PNG with alpha preserved. Compare two dump directories with the `dump_compare`
//! example.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::state::{GlassStyle, Hit, MenuKind, Modal, Page, Popup, Section, State};
use crate::{Renderer, draw_frame};

/// Logical window size of every scene (the spike window's initial size).
const W: u32 = 1040;
const H: u32 = 660;

/// Default frame clock offset from the anchors: fades are done, menus fully open.
const SETTLED: Duration = Duration::from_secs(10);

type AfterWarmUp = fn(&mut State) -> Result<(), String>;

struct Scene {
    name: &'static str,
    scale: f32,
    /// Frame clock offset from the anchors.
    age: Duration,
    setup: fn(&mut State),
    /// Runs after the warm-up frame, once `st.hits` is known.
    after_warm_up: Option<AfterWarmUp>,
}

const fn scene(name: &'static str, setup: fn(&mut State)) -> Scene {
    Scene {
        name,
        scale: 2.0,
        age: SETTLED,
        setup,
        after_warm_up: None,
    }
}

const APP_MENU: Popup = Popup::Menu {
    kind: MenuKind::App,
    at: (500.0, 120.0),
};

/// The fixed scene list, in file-number order.
fn scenes() -> Vec<Scene> {
    vec![
        scene("tab-zsh", |_| {}),
        scene("tab-build", |st| st.active = 1),
        scene("tab-claude", |st| st.active = 2),
        scene("tab-colors", |st| st.active = 3),
        scene("settings-appearance", |st| {
            st.page = Page::Settings(Section::Appearance);
        }),
        scene("settings-terminal", |st| {
            st.page = Page::Settings(Section::Terminal);
        }),
        scene("settings-about", |st| {
            st.page = Page::Settings(Section::About);
        }),
        scene("menu-app", |st| st.popup = Some(APP_MENU)),
        Scene {
            age: Duration::from_millis(90),
            ..scene("menu-app-opening", |st| st.popup = Some(APP_MENU))
        },
        scene("menu-tab", |st| {
            st.popup = Some(Popup::Menu {
                kind: MenuKind::Tab(1),
                at: (300.0, 60.0),
            });
        }),
        Scene {
            after_warm_up: Some(|st| {
                let (id, anchor) = st
                    .hits
                    .iter()
                    .find_map(|(r, h)| match h {
                        Hit::Combo(id) => Some((*id, *r)),
                        _ => None,
                    })
                    .ok_or("no combo on the Appearance page")?;
                st.popup = Some(Popup::Combo { id, anchor });
                Ok(())
            }),
            ..scene("combo", |st| {
                st.page = Page::Settings(Section::Appearance);
            })
        },
        scene("modal-confirm-close", |st| {
            st.modal = Some(Modal::ConfirmClose {
                tabs: vec![1],
                dont_ask: false,
            });
        }),
        scene("modal-rename", |st| {
            st.modal = Some(Modal::Rename {
                tab: 0,
                text: "zsh".to_owned(),
            });
        }),
        scene("modal-about", |st| st.modal = Some(Modal::About)),
        scene("glass-off", |st| st.settings.glass = GlassStyle::Off),
        scene("unfocused", |st| st.focused = false),
        scene("selection", |st| {
            st.tabs[0].selection = Some(((2, 1), (20, 3)));
        }),
        Scene {
            scale: 1.0,
            ..scene("tab-zsh", |_| {})
        },
    ]
}

/// A fresh state for one scene: all clock anchors at `base`, the mouse parked off the window
/// so nothing is hovered, focused.
fn fresh_state(base: Instant) -> State {
    let mut st = State::new();
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        // Same as the live app: no glass on this OS.
        st.settings.glass = GlassStyle::Off;
    }
    st.focused = true;
    st.mouse = (-100.0, -100.0);
    st.hover = None;
    st.started = base;
    st.switched_at = base;
    st.popup_at = base;
    st.closing = None;
    st.now = base;
    st
}

/// Renders every scene and writes `NN-name@Sx.png` into `dir`.
pub fn dump_frames(r: &mut Renderer, st: &mut State, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let base = Instant::now();
    for (i, scene) in scenes().into_iter().enumerate() {
        let file = format!("{:02}-{}@{}x.png", i + 1, scene.name, scene.scale as u32);
        render_scene(r, st, base, &scene, &dir.join(&file))
            .map_err(|e| format!("scene {file}: {e}"))?;
        eprintln!("dumped {file}");
    }
    Ok(())
}

fn render_scene(
    r: &mut Renderer,
    st: &mut State,
    base: Instant,
    scene: &Scene,
    path: &Path,
) -> Result<(), String> {
    *st = fresh_state(base);
    (scene.setup)(st);
    st.now = base + scene.age;
    let s = scene.scale;
    let size = ((W as f32 * s) as u32, (H as f32 * s) as u32);
    let format = r.gpu.config.format;
    let texture = r.gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame dump"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let frame = |r: &mut Renderer, st: &mut State| -> Result<(), String> {
        draw_frame(r, st, s, &view, size)?;
        st.hover = None;
        r.atlas.trim();
        r.text.end_frame();
        Ok(())
    };
    // Warm-up: fills `st.hits` and the scroll extent; the second frame is the one captured.
    frame(r, st)?;
    if let Some(f) = scene.after_warm_up {
        f(st)?;
    }
    frame(r, st)?;
    let rgba = read_rgba(&r.gpu.device, &r.gpu.queue, &texture)?;
    write_png(path, size, &rgba)
}

/// Copies `texture` back to the CPU as tightly packed RGBA8.
fn read_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<Vec<u8>, String> {
    let (w, h) = (texture.width(), texture.height());
    let row =
        (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame dump readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("poll: {e:?}"))?;
    let data = slice
        .get_mapped_range()
        .map_err(|e| format!("map: {e:?}"))?;
    unpad_to_rgba(&data, w, h, row, texture.format())
}

/// Removes the row padding of a readback and converts to RGBA8, keeping alpha.
/// `Bgra8Unorm` and `Bgra8UnormSrgb` are swizzled; `Rgba8*` pass through.
fn unpad_to_rgba(
    data: &[u8],
    w: u32,
    h: u32,
    row: u32,
    format: wgpu::TextureFormat,
) -> Result<Vec<u8>, String> {
    use wgpu::TextureFormat as F;
    let bgra = match format {
        F::Bgra8Unorm | F::Bgra8UnormSrgb => true,
        F::Rgba8Unorm | F::Rgba8UnormSrgb => false,
        other => return Err(format!("unsupported dump format {other:?}")),
    };
    let (w, h, row) = (w as usize, h as usize, row as usize);
    if row < w * 4 || data.len() < row * h {
        return Err("readback buffer too small".to_owned());
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for px in data[y * row..y * row + w * 4].chunks_exact(4) {
            if bgra {
                rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            } else {
                rgba.extend_from_slice(px);
            }
        }
    }
    Ok(rgba)
}

fn write_png(path: &Path, size: (u32, u32), rgba: &[u8]) -> Result<(), String> {
    let file =
        std::fs::File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpad_swizzles_bgra_and_keeps_alpha() {
        // 2x2 BGRA, rows padded to 256 bytes with 0xEE filler.
        let mut data = vec![0xEE; 512];
        data[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        data[256..264].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let out = unpad_to_rgba(&data, 2, 2, 256, wgpu::TextureFormat::Bgra8Unorm)
            .expect("valid readback");
        assert_eq!(out, [3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]);
        let srgb = unpad_to_rgba(&data, 2, 2, 256, wgpu::TextureFormat::Bgra8UnormSrgb)
            .expect("valid readback");
        assert_eq!(srgb, out);
    }

    #[test]
    fn unpad_passes_rgba_through() {
        let mut data = vec![0xEE; 512];
        data[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        data[256..264].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let out = unpad_to_rgba(&data, 2, 2, 256, wgpu::TextureFormat::Rgba8Unorm)
            .expect("valid readback");
        assert_eq!(out, (1..=16).collect::<Vec<u8>>());
    }

    #[test]
    fn unpad_rejects_other_formats_and_short_buffers() {
        assert!(unpad_to_rgba(&[0; 512], 2, 2, 256, wgpu::TextureFormat::R8Unorm).is_err());
        assert!(unpad_to_rgba(&[0; 100], 2, 2, 256, wgpu::TextureFormat::Bgra8Unorm).is_err());
    }
}
