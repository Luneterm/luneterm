//! The wgpu half of the renderer (plan step 5): [`TermView`].
//!
//! [`crate::build::Builder`] turns the grid into per-row instance vectors; this file keeps one
//! persistent GPU buffer per instance kind (background rects, mask glyphs, colour glyphs,
//! decorations). Row `r` owns the byte range `[r * cap * stride, (r + 1) * cap * stride)` of each
//! buffer, so rows never overlap and an unchanged row is never rewritten. Growing `cap` (or the
//! row count) re-creates the buffers and rewrites every row in the same `prepare`. Draws are one
//! instanced draw per non-empty row and kind, in the D9 order; the overlay (cursor, selection,
//! link hover) lives in one small buffer rewritten every frame.

use cosmic_text::FontSystem;
use lune_ui::custom::{CustomDraw, CustomFrame};
use lune_ui::text::TextRendering;

use crate::atlas::{Atlas, Page};
use crate::build::{Builder, DecoInst, GlyphInst, RectInst, RowInstances};
use crate::layout::CellMetrics;
use crate::model::{Grid, ViewState};
use crate::text::TermFonts;
use crate::theme::TermTheme;

/// Smallest per-row slot, in instances.
const MIN_CAP: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    size: [f32; 2],
    opacity: f32,
    _pad: f32,
}

const STRIDE: [usize; 4] = [
    std::mem::size_of::<RectInst>(),
    std::mem::size_of::<GlyphInst>(),
    std::mem::size_of::<GlyphInst>(),
    std::mem::size_of::<DecoInst>(),
];

fn counts(r: &RowInstances) -> [usize; 4] {
    [r.bgs.len(), r.mask.len(), r.color.len(), r.decos.len()]
}

fn bytes(r: &RowInstances, kind: usize) -> &[u8] {
    match kind {
        0 => bytemuck::cast_slice(&r.bgs),
        1 => bytemuck::cast_slice(&r.mask),
        2 => bytemuck::cast_slice(&r.color),
        _ => bytemuck::cast_slice(&r.decos),
    }
}

#[derive(Debug)]
struct RowBuffers {
    /// Rows the buffers have slots for.
    rows: usize,
    /// Slot capacity per kind, in instances.
    cap: [usize; 4],
    bufs: [wgpu::Buffer; 4],
}

#[derive(Debug)]
struct Pipelines {
    rect: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    color: wgpu::RenderPipeline,
    deco: wgpu::RenderPipeline,
}

/// A view's work counters ([`TermView::stats`]): plain counts, no clock, so the app's tab-switch
/// trace can say what a frame cost besides how long it took.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewStats {
    /// Rows built since the view was made.
    pub rows_built: u64,
    /// Rasterizer calls (atlas misses) since the view was made: font glyphs.
    pub font_rasters: u64,
    /// The same for procedural glyphs (box, block, powerline).
    pub custom_rasters: u64,
    /// Rows the latest `update` built.
    pub last_rows: usize,
    /// Whether the latest `update` rebuilt every row.
    pub last_full: bool,
}

/// The terminal grid as a [`CustomDraw`]: call [`update`](Self::update) when the grid or view
/// changes, then let the frame renderer call `prepare` and `render`.
#[derive(Debug)]
pub struct TermView {
    builder: Builder,
    atlas: Atlas,
    pipelines: Pipelines,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    /// The whole grid's opacity, 0..=1 (see [`set_opacity`](Self::set_opacity)).
    opacity: f32,
    rows_buf: Option<RowBuffers>,
    /// Per row, the counts uploaded by the last `prepare`: [bgs, mask, color, decos].
    drawn: Vec<[u32; 4]>,
    overlay_buf: Option<(wgpu::Buffer, usize)>,
    overlay_n: u32,
    overlay_scratch: Vec<RectInst>,
}

impl TermView {
    /// Pipelines are built for `format`, the colour format of the pass the view renders into.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        fonts: TermFonts,
        theme: TermTheme,
    ) -> TermView {
        let atlas = Atlas::new(device);
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("termview globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("termview globals layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("termview globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let pipelines = make_pipelines(device, format, &globals_layout, &atlas);
        TermView {
            builder: Builder::new(fonts, theme),
            atlas,
            pipelines,
            globals,
            globals_bind,
            opacity: 1.0,
            rows_buf: None,
            drawn: Vec::new(),
            overlay_buf: None,
            overlay_n: 0,
            overlay_scratch: Vec::new(),
        }
    }

    /// The opacity the whole grid is drawn at (cells, glyphs, decorations, cursor, selection and
    /// link hover alike), 0..=1; a value outside that range is clamped and a NaN is 1. The default
    /// 1 draws exactly as before. The app uses it for a tab-switch crossfade between two views.
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = clamp_opacity(opacity);
    }

    /// The opacity set by [`set_opacity`](Self::set_opacity).
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// New colours; everything is rebuilt on the next update.
    pub fn set_theme(&mut self, theme: TermTheme) {
        self.builder.set_theme(theme);
    }

    /// New fonts: clears the shaper and the atlas; everything is rebuilt on the next update.
    pub fn set_fonts(&mut self, fonts: TermFonts) {
        self.builder.set_fonts(fonts);
        for page in [Page::Mask, Page::Color] {
            self.atlas.core_mut().reset(page);
        }
    }

    /// Cell metrics for the current fonts at `scale`, cached per (fonts, scale).
    pub fn metrics(&mut self, fs: &mut FontSystem, scale: f32) -> CellMetrics {
        self.builder.metrics(fs, scale)
    }

    /// Rebuilds the instances of the dirty rows (every row when the grid size, scale, origin,
    /// metrics, theme, fonts or atlas changed), the overlay, then clears the grid's dirty flags.
    /// `origin` is the grid's top-left in logical px. A scale that is not finite and positive
    /// draws nothing. `text` is how glyphs are rasterized: pass the one the frame's
    /// `FrameParams.text` carries, so the grid and the UI text draw the same look (a different value
    /// rebuilds every row; a pool view created later takes it from its first update).
    pub fn update(
        &mut self,
        fs: &mut FontSystem,
        grid: &mut Grid,
        view: &ViewState<'_>,
        origin: (f32, f32),
        scale: f32,
        text: TextRendering,
    ) {
        self.builder
            .update(self.atlas.core_mut(), fs, grid, view, origin, scale, text);
    }

    /// The view's work counters.
    pub fn stats(&self) -> ViewStats {
        let (rows_built, last_rows, last_full) = self.builder.row_counts();
        let (font_rasters, custom_rasters) = self.atlas.core().rasters();
        ViewStats {
            rows_built,
            font_rasters,
            custom_rasters,
            last_rows,
            last_full,
        }
    }

    pub fn builder(&self) -> &Builder {
        &self.builder
    }

    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    pub fn atlas_mut(&mut self) -> &mut Atlas {
        &mut self.atlas
    }

    fn upload_rows(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.drawn.clear();
        let nrows = self.builder.rows().len();
        if self.builder.is_skipped() || nrows == 0 {
            self.builder.take_queued();
            return;
        }
        let mut need = [0usize; 4];
        for r in self.builder.rows() {
            for (n, c) in need.iter_mut().zip(counts(r)) {
                *n = (*n).max(c);
            }
        }
        let want = need.map(|n| n.next_power_of_two().max(MIN_CAP));
        let recreate = match &self.rows_buf {
            None => true,
            Some(b) => b.rows < nrows || (0..4).any(|k| want[k] > b.cap[k]),
        };
        if recreate {
            let old = self.rows_buf.as_ref();
            let cap: [usize; 4] = std::array::from_fn(|k| want[k].max(old.map_or(0, |b| b.cap[k])));
            let rows = nrows.max(old.map_or(0, |b| b.rows));
            let limit = device.limits().max_buffer_size;
            let size = |k: usize| (rows as u64) * (cap[k] as u64) * (STRIDE[k] as u64);
            if (0..4).any(|k| size(k) > limit) {
                // too big for this device: draw nothing rather than fail validation
                self.rows_buf = None;
                self.builder.take_queued();
                return;
            }
            let make = |k: usize| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("termview rows"),
                    size: size(k),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            self.rows_buf = Some(RowBuffers {
                rows,
                cap,
                bufs: std::array::from_fn(make),
            });
            // every slot moved: all rows must be rewritten
            self.builder.queue_all();
        }
        let Some(buffers) = &self.rows_buf else {
            return;
        };
        let queued = self.builder.take_queued();
        let rows = self.builder.rows();
        for r in queued {
            let Some(inst) = rows.get(r) else { continue };
            for (k, (buf, stride)) in buffers.bufs.iter().zip(STRIDE).enumerate() {
                let data = bytes(inst, k);
                if !data.is_empty() {
                    queue.write_buffer(buf, (r * buffers.cap[k] * stride) as u64, data);
                }
            }
        }
        self.drawn = rows.iter().map(|r| counts(r).map(|c| c as u32)).collect();
    }

    fn upload_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.overlay_scratch.clear();
        if !self.builder.is_skipped() {
            self.overlay_scratch
                .extend(self.builder.overlay().rects().copied());
        }
        self.overlay_n = self.overlay_scratch.len() as u32;
        if self.overlay_scratch.is_empty() {
            return;
        }
        let n = self.overlay_scratch.len();
        if self.overlay_buf.as_ref().is_none_or(|(_, cap)| *cap < n) {
            let cap = n.next_power_of_two().max(16);
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("termview overlay"),
                size: (cap * STRIDE[0]) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.overlay_buf = Some((buf, cap));
        }
        if let Some((buf, _)) = &self.overlay_buf {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.overlay_scratch));
        }
    }
}

impl CustomDraw for TermView {
    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &CustomFrame) {
        self.atlas.flush(queue);
        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                size: [frame.size.0.max(1) as f32, frame.size.1.max(1) as f32],
                opacity: self.opacity,
                _pad: 0.0,
            }),
        );
        self.upload_rows(device, queue);
        self.upload_overlay(device, queue);
    }

    fn render(&self, pass: &mut wgpu::RenderPass<'_>, _frame: &CustomFrame) {
        if self.drawn.is_empty() && self.overlay_n == 0 {
            return;
        }
        pass.set_bind_group(0, &self.globals_bind, &[]);
        pass.set_bind_group(1, self.atlas.bind_group(), &[]);
        if let Some(b) = &self.rows_buf {
            // D9: backgrounds, mask glyphs, colour glyphs, decorations
            let kinds = [
                (0, &self.pipelines.rect),
                (1, &self.pipelines.mask),
                (2, &self.pipelines.color),
                (3, &self.pipelines.deco),
            ];
            for (k, pipeline) in kinds {
                pass.set_pipeline(pipeline);
                pass.set_vertex_buffer(0, b.bufs[k].slice(..));
                for (r, n) in self.drawn.iter().enumerate() {
                    if n[k] > 0 {
                        let base = (r * b.cap[k]) as u32;
                        pass.draw(0..4, base..base + n[k]);
                    }
                }
            }
        }
        if self.overlay_n > 0
            && let Some((buf, _)) = &self.overlay_buf
        {
            pass.set_pipeline(&self.pipelines.rect);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..4, 0..self.overlay_n);
        }
    }
}

/// The rule of [`TermView::set_opacity`]: 0..=1, a NaN is 1 (draw as it is rather than nothing).
fn clamp_opacity(opacity: f32) -> f32 {
    if opacity.is_nan() {
        1.0
    } else {
        opacity.clamp(0.0, 1.0)
    }
}

fn make_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    globals_layout: &wgpu::BindGroupLayout,
    atlas: &Atlas,
) -> Pipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("termview"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("termview"),
        bind_group_layouts: &[Some(globals_layout), Some(atlas.bind_group_layout())],
        ..Default::default()
    });
    let rect_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
    let glyph_attrs =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
    let deco_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
    fn instance_layout(
        stride: usize,
        attributes: &[wgpu::VertexAttribute],
    ) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: stride as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes,
        }
    }
    let make = |label: &str,
                vs: &str,
                fs: &str,
                buffer: wgpu::VertexBufferLayout<'_>|
     -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vs),
                buffers: &[Some(buffer)],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            cache: None,
            multiview_mask: None,
        })
    };
    let rect_layout = instance_layout(STRIDE[0], &rect_attrs);
    let glyph_layout = instance_layout(STRIDE[1], &glyph_attrs);
    let deco_layout = instance_layout(STRIDE[3], &deco_attrs);
    Pipelines {
        rect: make("termview rects", "vs_rect", "fs_rect", rect_layout),
        mask: make(
            "termview mask glyphs",
            "vs_glyph",
            "fs_glyph_mask",
            glyph_layout.clone(),
        ),
        color: make(
            "termview colour glyphs",
            "vs_glyph",
            "fs_glyph_color",
            glyph_layout,
        ),
        deco: make("termview decorations", "vs_deco", "fs_deco", deco_layout),
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_opacity;

    #[test]
    fn opacity_is_clamped_to_0_1_and_nan_is_1() {
        assert_eq!(clamp_opacity(0.5), 0.5);
        assert_eq!(clamp_opacity(0.0), 0.0);
        assert_eq!(clamp_opacity(1.0), 1.0);
        assert_eq!(clamp_opacity(-0.25), 0.0);
        assert_eq!(clamp_opacity(7.0), 1.0);
        assert_eq!(clamp_opacity(f32::INFINITY), 1.0);
        assert_eq!(clamp_opacity(f32::NEG_INFINITY), 0.0);
        assert_eq!(clamp_opacity(f32::NAN), 1.0);
    }
}
