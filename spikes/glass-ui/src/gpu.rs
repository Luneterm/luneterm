//! wgpu setup and the two non-text pipelines (shapes, strokes).

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::window::Window;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Globals {
    pub size: [f32; 2],
    pub scale: f32,
    pub _pad: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Shape {
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub color: [f32; 4],
    pub under: [f32; 4],
    pub params: [f32; 4],
    /// Physical clip rect (x, y, w, h); zero width = no clip.
    pub clip: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct StrokeVertex {
    pub pos: [f32; 2],
    pub aa: [f32; 2],
    pub color: [f32; 4],
    pub style: [f32; 4],
    pub extra: [f32; 4],
    pub clip: [f32; 4],
}

pub struct Gpu {
    pub window: Arc<Window>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub alpha_mode: wgpu::CompositeAlphaMode,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    acrylic_pipeline: wgpu::RenderPipeline,
    shape_pipeline: wgpu::RenderPipeline,
    stroke_pipeline: wgpu::RenderPipeline,
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gpu")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// One z-layer of the frame: acrylic (replace blend), shapes under text, text (drawn by the
/// caller), then shapes and strokes over text.
#[derive(Debug, Default)]
pub struct LayerGeometry {
    pub acrylic: Vec<Shape>,
    pub under: Vec<Shape>,
    pub over: Vec<Shape>,
    pub strokes: Vec<StrokeVertex>,
}

type Range = std::ops::Range<u32>;

pub struct Uploaded {
    shapes: wgpu::Buffer,
    strokes: Option<wgpu::Buffer>,
    /// Per layer: acrylic, under, over shape ranges and the stroke vertex range.
    ranges: Vec<(Range, Range, Range, Range)>,
}

impl Gpu {
    pub async fn new(window: Arc<Window>, instance: &wgpu::Instance) -> Result<Self, String> {
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| e.to_string())?;

        let caps = surface.get_capabilities(&adapter);
        // Blend in gamma space like Skia/Avalonia, so colours match Lunegit.
        let format = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]
        .into_iter()
        .find(|f| caps.formats.contains(f))
        .unwrap_or(caps.formats[0]);
        // Our output is premultiplied. DirectComposition (Windows) offers PreMultiplied; Metal only
        // offers PostMultiplied, which CoreAnimation composites as premultiplied anyway.
        let alpha_mode = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m))
        .unwrap_or(caps.alpha_modes[0]);

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (noise_view, noise_sampler) = noise_texture(&device, &queue)?;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&noise_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&noise_sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shapes"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shapes"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });

        let shape_attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4
        ];
        let shape_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Shape>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &shape_attrs,
        };
        let stroke_attrs = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4
        ];
        let stroke_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<StrokeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &stroke_attrs,
        };

        let premultiplied_over = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffer: wgpu::VertexBufferLayout<'_>,
                    topology: wgpu::PrimitiveTopology,
                    blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
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
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                cache: None,
                multiview_mask: None,
            })
        };
        let strip = wgpu::PrimitiveTopology::TriangleStrip;
        let acrylic_pipeline = make(
            "acrylic",
            "vs_shape",
            "fs_shape",
            shape_layout.clone(),
            strip,
            Some(wgpu::BlendState::REPLACE),
        );
        let shape_pipeline = make(
            "shapes",
            "vs_shape",
            "fs_shape",
            shape_layout,
            strip,
            Some(premultiplied_over),
        );
        let stroke_pipeline = make(
            "strokes",
            "vs_stroke",
            "fs_stroke",
            stroke_layout,
            wgpu::PrimitiveTopology::TriangleList,
            Some(premultiplied_over),
        );

        Ok(Self {
            window,
            device,
            queue,
            surface,
            config,
            alpha_mode,
            globals,
            bind_group,
            acrylic_pipeline,
            shape_pipeline,
            stroke_pipeline,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn upload(&self, layers: &[LayerGeometry], size: (u32, u32), scale: f32) -> Uploaded {
        self.queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                size: [size.0 as f32, size.1 as f32],
                scale,
                _pad: 0.0,
            }),
        );
        let mut shapes: Vec<Shape> = Vec::new();
        let mut strokes: Vec<StrokeVertex> = Vec::new();
        let mut ranges = Vec::with_capacity(layers.len());
        for layer in layers {
            let mut push = |items: &[Shape]| {
                let start = shapes.len() as u32;
                shapes.extend_from_slice(items);
                start..shapes.len() as u32
            };
            let acrylic = push(&layer.acrylic);
            let under = push(&layer.under);
            let over = push(&layer.over);
            let start = strokes.len() as u32;
            strokes.extend_from_slice(&layer.strokes);
            ranges.push((acrylic, under, over, start..strokes.len() as u32));
        }
        if shapes.is_empty() {
            shapes.push(Shape::zeroed());
        }
        let shapes = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shapes"),
                contents: bytemuck::cast_slice(&shapes),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let strokes = (!strokes.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("strokes"),
                    contents: bytemuck::cast_slice(&strokes),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        Uploaded {
            shapes,
            strokes,
            ranges,
        }
    }

    /// Everything of `layer` that goes below its text.
    pub fn draw_below_text(&self, pass: &mut wgpu::RenderPass<'_>, up: &Uploaded, layer: usize) {
        let (acrylic, under, _, _) = up.ranges[layer].clone();
        self.draw_shapes(pass, up, &self.acrylic_pipeline, acrylic);
        self.draw_shapes(pass, up, &self.shape_pipeline, under);
    }

    /// Everything of `layer` that goes above its text.
    pub fn draw_above_text(&self, pass: &mut wgpu::RenderPass<'_>, up: &Uploaded, layer: usize) {
        let (_, _, over, stroke_range) = up.ranges[layer].clone();
        self.draw_shapes(pass, up, &self.shape_pipeline, over);
        if let Some(strokes) = &up.strokes
            && !stroke_range.is_empty()
        {
            pass.set_pipeline(&self.stroke_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, strokes.slice(..));
            pass.draw(stroke_range, 0..1);
        }
    }

    fn draw_shapes(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        up: &Uploaded,
        pipeline: &wgpu::RenderPipeline,
        range: Range,
    ) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, up.shapes.slice(..));
        pass.draw(0..4, range);
    }
}

/// Avalonia's 256×256 acrylic noise (MIT, `licenses/avalonia-LICENSE`), as straight-alpha RGBA.
fn noise_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(wgpu::TextureView, wgpu::Sampler), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!(
        "../assets/acrylic-noise.png"
    )));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png size")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => return Err("png palette not expanded".into()),
    };
    let size = wgpu::Extent3d {
        width: info.width,
        height: info.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("acrylic noise"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("noise"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        ..Default::default()
    });
    Ok((texture.create_view(&Default::default()), sampler))
}
