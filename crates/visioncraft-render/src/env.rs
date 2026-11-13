//! The prefiltered environment cube (see env.wgsl): `SIZE`-texel faces,
//! `MIPS` levels, level `k` prefiltered for perceptual roughness
//! `k / (MIPS - 1)`. Re-rendered only when the sun changes: 36 small passes
//! (6 faces x 6 mips), all in the frame that follows the change.

use glam::Vec3;

use crate::{Bytes, SLOT, sky};

pub const SIZE: u32 = 64;
pub const MIPS: u32 = 6;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(crate) struct Env {
    /// Cube view for sampling.
    pub view: wgpu::TextureView,
    targets: Vec<wgpu::TextureView>,
    faces: wgpu::Buffer,
    faces_bind: wgpu::BindGroup,
    globals_bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    pub dirty: bool,
}

impl Env {
    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        globals_layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 6,
            },
            mip_level_count: MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("environment cube"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        let targets = (0..MIPS)
            .flat_map(|mip| (0..6).map(move |face| (mip, face)))
            .map(|(mip, face)| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("environment face"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    base_array_layer: face,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let faces = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("environment faces"),
            size: SLOT * (6 * MIPS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let faces_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("environment face"),
            entries: &[crate::uniform_entry(
                0,
                wgpu::ShaderStages::FRAGMENT,
                Some(48),
            )],
        });
        let faces_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("environment face"),
            layout: &faces_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: crate::slot_binding(&faces, 48),
            }],
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("environment globals"),
            layout: globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let pipeline = crate::pipeline(
            device,
            crate::PipelineDesc {
                label: "environment",
                module,
                vs: "vs_env",
                fs: Some("fs_env"),
                groups: &[Some(globals_layout), Some(&faces_layout)],
                buffers: &[],
                target: Some(FORMAT.into()),
                depth: None,
                cull: None,
                samples: 1,
                constants: &[],
            },
        );
        Self {
            view,
            targets,
            faces,
            faces_bind,
            globals_bind,
            pipeline,
            dirty: true,
        }
    }

    /// Records the 36 passes; the globals (sky coefficients) must already
    /// hold this frame's sun.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        ground: Vec3,
    ) {
        self.dirty = false;
        let mut slots = vec![0u8; (SLOT * (6 * MIPS) as u64) as usize];
        let ground = ground / 1000.0;
        let night = sky::NIGHT / 1000.0;
        for mip in 0..MIPS {
            for face in 0..6 {
                let roughness = mip as f32 / (MIPS - 1) as f32;
                let samples = if mip == 0 {
                    1.0
                } else {
                    24.0 * mip as f32 // narrow lobes (many texels) need few samples
                };
                let mut b = Bytes::default();
                b.floats(&[face as f32, roughness, (SIZE >> mip) as f32, samples]);
                b.floats(&ground.extend(0.0).to_array());
                b.floats(&night.extend(0.0).to_array());
                let at = ((mip * 6 + face) as u64 * SLOT) as usize;
                slots[at..at + b.0.len()].copy_from_slice(&b.0);
            }
        }
        queue.write_buffer(&self.faces, 0, &slots);
        for (i, target) in self.targets.iter().enumerate() {
            let mut pass = crate::color_pass(encoder, "environment", target, None, None);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &self.faces_bind, &[(i as u64 * SLOT) as u32]);
            pass.draw(0..3, 0..1);
        }
    }
}
