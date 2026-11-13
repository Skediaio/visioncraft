//! VisionCraft renderer spike: ground plane plus instanced cubes into an HDR
//! target, then a tone-map pass into an sRGB texture. No UI, no window.

use glam::{Mat4, Vec3};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const CLEAR: wgpu::Color = wgpu::Color { r: 0.05, g: 0.07, b: 0.10, a: 1.0 };

/// Unit cube centred on the origin: 36 vertices of position + normal. Packed
/// by hand (`to_le_bytes`), so no `unsafe` and no bytemuck.
fn cube_bytes() -> Vec<u8> {
    // (normal, u, v) with u x v = normal: counter-clockwise seen from outside.
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let mut bytes = Vec::with_capacity(36 * 24);
    for (n, u, v) in faces {
        let corner = |a: f32, b: f32| n * 0.5 + u * a + v * b;
        let quad = [corner(-0.5, -0.5), corner(0.5, -0.5), corner(0.5, 0.5), corner(-0.5, 0.5)];
        for i in [0, 1, 2, 0, 2, 3] {
            push_floats(&mut bytes, &quad[i].to_array());
            push_floats(&mut bytes, &n.to_array());
        }
    }
    bytes
}

fn push_floats(bytes: &mut Vec<u8>, floats: &[f32]) {
    for f in floats {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
}

struct Targets {
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    tonemap_bind: wgpu::BindGroup,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    output_format: wgpu::TextureFormat,
    scene_pipeline: wgpu::RenderPipeline,
    tonemap_pipeline: wgpu::RenderPipeline,
    tonemap_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    cube: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// Instance slots in `instances`; slot 0 is the ground, the rest cubes.
    capacity: usize,
    count: u32,
    targets: Targets,
}

impl Renderer {
    /// `format` is the tone-mapped output format (use `Rgba8UnormSrgb`).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let scene = device.create_shader_module(wgpu::include_wgsl!("scene.wgsl"));
        let tonemap = device.create_shader_module(wgpu::include_wgsl!("tonemap.wgsl"));

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&globals_layout)],
                immediate_size: 0,
            })),
            vertex: wgpu::VertexState {
                module: &scene,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 24,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4
                        ],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &scene,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(HDR.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let tonemap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&tonemap_layout)],
                immediate_size: 0,
            })),
            vertex: wgpu::VertexState {
                module: &tonemap,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &tonemap,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let cube_bytes = cube_bytes();
        let cube = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cube"),
            size: cube_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&cube, 0, &cube_bytes);

        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            output_format: format,
            scene_pipeline,
            targets: make_targets(device, format, &tonemap_layout, 1, 1),
            tonemap_layout,
            tonemap_pipeline,
            globals,
            globals_bind,
            cube,
            instances: device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            }),
            capacity: 0,
            count: 0,
        };
        renderer.set_instances(&[]);
        renderer.set_camera(Mat4::IDENTITY);
        renderer
    }
    /// Recreates the HDR, depth and output textures. Call before the first
    /// `render`, and whenever the target size changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.targets = make_targets(&self.device, self.output_format, &self.tonemap_layout, width, height);
    }

    pub fn set_camera(&mut self, view_proj: Mat4) {
        let mut bytes = Vec::with_capacity(80);
        push_floats(&mut bytes, &view_proj.to_cols_array());
        // Direction towards the light, then the ambient term in w.
        let light = Vec3::new(0.4, 0.8, 0.3).normalize();
        push_floats(&mut bytes, &[light.x, light.y, light.z, 0.15]);
        self.queue.write_buffer(&self.globals, 0, &bytes);
    }

    /// One model matrix per unit cube (centred on the origin). The ground
    /// plane is prepended as instance 0.
    pub fn set_instances(&mut self, models: &[Mat4]) {
        let ground = Mat4::from_translation(Vec3::new(0.0, -0.1, 0.0))
            * Mat4::from_scale(Vec3::new(2000.0, 0.2, 2000.0));
        let mut bytes = Vec::with_capacity((models.len() + 1) * 64);
        push_floats(&mut bytes, &ground.to_cols_array());
        for m in models {
            push_floats(&mut bytes, &m.to_cols_array());
        }
        let slots = models.len() + 1;
        if slots > self.capacity {
            self.instances = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: (slots * 64) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = slots;
        }
        self.queue.write_buffer(&self.instances, 0, &bytes);
        self.count = slots as u32;
    }

    /// The tone-mapped output texture, for readback (`COPY_SRC`).
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.targets.output
    }

    /// Records the scene pass and the tone-map pass; returns the output view.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder) -> &wgpu::TextureView {
        let t = &self.targets;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.scene_pipeline);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_vertex_buffer(0, self.cube.slice(..));
            pass.set_vertex_buffer(1, self.instances.slice(..));
            pass.draw(0..36, 0..1); // ground
            pass.draw(0..36, 1..self.count); // all cubes, one call
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &t.tonemap_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        &t.output_view
    }
}

fn make_targets(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    tonemap_layout: &wgpu::BindGroupLayout,
    width: u32,
    height: u32,
) -> Targets {
    let texture = |label, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let view = |t: &wgpu::Texture| t.create_view(&Default::default());
    let hdr = view(&texture(
        "hdr",
        HDR,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    ));
    let depth = view(&texture("depth", DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT));
    let output = texture(
        "output",
        format,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    );
    let tonemap_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: tonemap_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr) }],
    });
    Targets { hdr, depth, output_view: view(&output), output, tonemap_bind }
}

/// Adapter and device without a surface, for tests and the offscreen bench.
/// Fails (never falls back silently) when no adapter exists.
pub async fn headless_device() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| format!("no wgpu adapter: {e}"))?;
    eprintln!("adapter: {:?}", adapter.get_info());
    adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| format!("no wgpu device: {e}"))
}
