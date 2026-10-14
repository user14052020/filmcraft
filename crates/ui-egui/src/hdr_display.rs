//! Float monitor images drawn directly into a native EDR surface. Ordinary egui textures are
//! gamma encoded and limited to SDR; this callback carries linear scRGB, including highlights
//! above 1 and negative wide-gamut components, all the way to the operating system compositor.

use std::collections::HashMap;

use eframe::{egui_wgpu, wgpu};

use crate::frames::{FrameKey, GpuPlan};

struct CachedImage {
    key: FrameKey,
    bind_group: wgpu::BindGroup,
}

pub(crate) struct EdrPainter {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    images: HashMap<String, CachedImage>,
}

impl EdrPainter {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HDR monitor image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HDR monitor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hdr_display.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HDR monitor"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("HDR monitor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("HDR monitor"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, sampler, images: HashMap::new() }
    }

    pub fn upload(&mut self, name: &str, key: FrameKey, plan: &GpuPlan, rs: &egui_wgpu::RenderState) -> Option<()> {
        if self.images.get(name).is_some_and(|i| i.key == key) {
            return Some(());
        }
        let filmcraft_render::plan::FramePlan::Image(img) = &plan.plan else { return None };
        let bytes = plan.prepared.image_rgba_f16()?;
        let w = u32::try_from(img.w).ok()?;
        let h = u32::try_from(img.h).ok()?;
        let stride = w.checked_mul(8)?;
        let expected = img.w.checked_mul(img.h)?.checked_mul(8)?;
        if w == 0 || h == 0 || bytes.len() != expected || w.max(h) > rs.device.limits().max_texture_dimension_2d {
            return None;
        }
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HDR monitor"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        rs.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(h) },
            size,
        );
        let view = texture.create_view(&Default::default());
        let bind_group = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HDR monitor"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.images.insert(name.to_owned(), CachedImage { key, bind_group });
        Some(())
    }

    pub fn callback(&self, name: &str, rect: egui::Rect, clip: egui::Rect, rs: &egui_wgpu::RenderState) -> Option<(FrameKey, egui::PaintCallback)> {
        let image = self.images.get(name)?;
        // Egui clips callback viewports to the window, which otherwise stretches a zoomed
        // picture. Draw only the visible rectangle and preserve its original texture crop.
        let visible = rect.intersect(clip);
        if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 || !visible.is_positive() {
            return None;
        }
        let uv = (visible.min - rect.min) / rect.size();
        let span = visible.size() / rect.size();
        let vertices = [-1.0, 1.0, uv.x, uv.y, 3.0, 1.0, uv.x + 2.0 * span.x, uv.y, -1.0, -3.0, uv.x, uv.y + 2.0 * span.y];
        let bytes: Vec<u8> = vertices.iter().flat_map(|v| v.to_le_bytes()).collect();
        let vertex_buffer = rs.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HDR monitor crop"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        rs.queue.write_buffer(&vertex_buffer, 0, &bytes);
        Some((
            image.key,
            egui_wgpu::Callback::new_paint_callback(
                visible,
                EdrCallback { pipeline: self.pipeline.clone(), bind_group: image.bind_group.clone(), vertex_buffer },
            ),
        ))
    }
}

struct EdrCallback {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
}

impl egui_wgpu::CallbackTrait for EdrCallback {
    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, _resources: &egui_wgpu::CallbackResources) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..3, 0..1);
    }
}

impl crate::FilmcraftApp {
    /// Called by a host only after configuring an extended-linear-sRGB float surface.
    /// Zero means unavailable. Headroom is live (brightness and the current screen affect it).
    pub fn set_hdr_display_headroom(&mut self, headroom: f32) {
        if let Some(g) = &mut self.gpu {
            g.edr_headroom = if g.render_state.target_format == wgpu::TextureFormat::Rgba16Float && headroom.is_finite() && headroom > 1.01 {
                (headroom.clamp(1.0, 64.0) * 100.0).floor() as u16
            } else {
                0
            };
        }
    }

    pub(crate) fn hdr_headroom(&self) -> Option<u16> {
        let g = self.gpu.as_ref()?;
        (g.edr_headroom > 100 && !g.broken.load(std::sync::atomic::Ordering::Relaxed)).then_some(g.edr_headroom)
    }

    pub(crate) fn hdr_present(&mut self, name: &str, key: FrameKey, plan: &GpuPlan) {
        let Some(g) = self.gpu.as_mut() else { return };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let painter = g.edr.get_or_insert_with(|| EdrPainter::new(&g.render_state.device));
            painter.upload(name, key, plan, &g.render_state)
        }));
        if result.is_err() {
            self.disable_gpu("HDR presentation failed");
        }
    }

    pub(crate) fn hdr_callback(&self, name: &str, rect: egui::Rect, clip: egui::Rect) -> Option<(FrameKey, egui::PaintCallback)> {
        let gpu = self.gpu.as_ref()?;
        gpu.edr.as_ref()?.callback(name, rect, clip, &gpu.render_state)
    }
}
