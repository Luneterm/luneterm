//! Offscreen scene + backdrop blur, so a menu can show a blurred copy of what is under it
//! (Luneium's context-menu glass: blur 30px, saturate 1.8, then acrylic on top).

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    src_texel: [f32; 2],
    dir: [f32; 2],
    sigma: f32,
    radius: f32,
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BackdropUniform {
    rect: [f32; 4],
    target_size: [f32; 2],
    radius: f32,
    alpha: f32,
    saturation: f32,
    _pad: [f32; 3],
}

/// A blurred-backdrop region to draw, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct Backdrop {
    pub rect: [f32; 4],
    pub radius: f32,
    pub alpha: f32,
}

// The blur strength comes from Settings (`BlurLevel::sigma`). Luneium's CSS says `blur(30px)`, but
// taken literally that erases text to flat grey; its real menus use the lighter native macOS blur.
/// Blur runs at 1/8 resolution.
const DOWNSCALE: u32 = 8;

#[derive(Debug)]
struct Targets {
    scene: wgpu::TextureView,
    levels: [wgpu::TextureView; 3],
    pong: wgpu::TextureView,
    /// Bind groups: scene, level0, level1, blur-h (level2 → pong), blur-v (pong → level2), level2.
    groups: [wgpu::BindGroup; 6],
}

#[derive(Debug)]
pub struct Blur {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    copy: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    backdrop: wgpu::RenderPipeline,
    copy_params: wgpu::Buffer,
    h_params: wgpu::Buffer,
    v_params: wgpu::Buffer,
    backdrop_buf: wgpu::Buffer,
    size: (u32, u32),
    targets: Option<Targets>,
}

impl Blur {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform(2),
                uniform(3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let make = |label: &str, vs: &str, fs: &str, blend: wgpu::BlendState, strip: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: if strip {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                cache: None,
                multiview_mask: None,
            })
        };
        let copy = make(
            "blur copy",
            "vs_full",
            "fs_copy",
            wgpu::BlendState::REPLACE,
            false,
        );
        let blur = make(
            "blur gauss",
            "vs_full",
            "fs_blur",
            wgpu::BlendState::REPLACE,
            false,
        );
        let backdrop = make(
            "blur backdrop",
            "vs_backdrop",
            "fs_backdrop",
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            true,
        );
        let buffer = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blur"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            format,
            layout,
            sampler,
            copy,
            blur,
            backdrop,
            copy_params: buffer("blur copy params", size_of::<Params>()),
            h_params: buffer("blur h params", size_of::<Params>()),
            v_params: buffer("blur v params", size_of::<Params>()),
            backdrop_buf: buffer("blur backdrop", size_of::<BackdropUniform>()),
            size: (0, 0),
            targets: None,
        }
    }

    /// (Re)creates the offscreen targets for the surface size; cheap when unchanged.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        scale: f32,
        sigma_px: f32,
    ) {
        let (w, h) = (size.0.max(1), size.1.max(1));
        let small = ((w / DOWNSCALE).max(1), (h / DOWNSCALE).max(1));
        let sigma = sigma_px * scale / DOWNSCALE as f32;
        for (buf, dir) in [(&self.h_params, [1.0, 0.0]), (&self.v_params, [0.0, 1.0])] {
            let params = Params {
                src_texel: [1.0 / small.0 as f32, 1.0 / small.1 as f32],
                dir,
                sigma,
                radius: (sigma * 3.0).ceil().min(48.0),
                _pad: [0.0; 2],
            };
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&params));
        }
        if self.size == (w, h) && self.targets.is_some() {
            return;
        }
        self.size = (w, h);
        let texture = |label, tw: u32, th: u32| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: tw,
                        height: th,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let scene = texture("scene", w, h);
        let levels = [
            texture("blur 1/2", (w / 2).max(1), (h / 2).max(1)),
            texture("blur 1/4", (w / 4).max(1), (h / 4).max(1)),
            texture("blur 1/8", small.0, small.1),
        ];
        let pong = texture("blur pong", small.0, small.1);
        let group = |src: &wgpu::TextureView, params: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blur"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.backdrop_buf.as_entire_binding(),
                    },
                ],
            })
        };
        let groups = [
            group(&scene, &self.copy_params),
            group(&levels[0], &self.copy_params),
            group(&levels[1], &self.copy_params),
            group(&levels[2], &self.h_params),
            group(&pong, &self.v_params),
            group(&levels[2], &self.copy_params),
        ];
        self.targets = Some(Targets {
            scene,
            levels,
            pong,
            groups,
        });
    }

    pub fn scene_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.scene)
    }

    /// Downsample the scene to 1/8 and blur it. Call after the scene pass, before the main pass.
    pub fn run(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(t) = &self.targets else { return };
        let steps: [(&wgpu::TextureView, &wgpu::BindGroup, &wgpu::RenderPipeline); 5] = [
            (&t.levels[0], &t.groups[0], &self.copy),
            (&t.levels[1], &t.groups[1], &self.copy),
            (&t.levels[2], &t.groups[2], &self.copy),
            (&t.pong, &t.groups[3], &self.blur),
            (&t.levels[2], &t.groups[4], &self.blur),
        ];
        for (target, group, pipeline) in steps {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blur step"),
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Copy the scene into the current pass (first draw of the main pass).
    pub fn blit_scene(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(t) = &self.targets else { return };
        pass.set_pipeline(&self.copy);
        pass.set_bind_group(0, &t.groups[0], &[]);
        pass.draw(0..3, 0..1);
    }

    pub fn set_backdrop(&self, queue: &wgpu::Queue, b: Backdrop, saturation: f32) {
        let u = BackdropUniform {
            rect: b.rect,
            target_size: [self.size.0 as f32, self.size.1 as f32],
            radius: b.radius,
            alpha: b.alpha,
            saturation,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.backdrop_buf, 0, bytemuck::bytes_of(&u));
    }

    /// Draw the blurred scene inside the backdrop rect (set with `set_backdrop`).
    pub fn draw_backdrop(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(t) = &self.targets else { return };
        pass.set_pipeline(&self.backdrop);
        pass.set_bind_group(0, &t.groups[5], &[]);
        pass.draw(0..4, 0..1);
    }
}

impl Blur {
    /// Debug: write the scene and the blurred 1/8 texture as PNGs (premultiplied, over black).
    pub fn dump(&self, device: &wgpu::Device, queue: &wgpu::Queue, dir: &std::path::Path) {
        let Some(t) = &self.targets else { return };
        for (name, view) in [("scene", &t.scene), ("blurred", &t.levels[2])] {
            let tex = view.texture();
            let path = dir.join(format!("{name}.png"));
            let mut rgba = match crate::dump::read_rgba(device, queue, tex) {
                Ok(rgba) => rgba,
                Err(e) => {
                    eprintln!("dump {name}: {e}");
                    return;
                }
            };
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
            let size = (tex.width(), tex.height());
            match crate::dump::write_png(&path, size, &rgba) {
                Ok(()) => eprintln!("dumped {} ({}x{})", path.display(), size.0, size.1),
                Err(e) => eprintln!("dump {name}: {e}"),
            }
        }
    }
}
