//! The prefiltered environment cube (see env.wgsl): `SIZE`-texel faces,
//! `MIPS` levels, level `k` prefiltered for perceptual roughness
//! `k / (MIPS - 1)`, and the ambient SH. When the sun moves, the SH (one
//! 9x1 pass, copied into the globals) is redone in that frame, the cube two
//! mips (12 small passes) per frame, round-robin, so dragging the sun never
//! re-renders all 36 faces in one frame: rough mips lag by up to three
//! frames, which is invisible on a smooth sky. Any other change of the sky
//! (a new source, the ground, the first frame, a still) redoes it all at
//! once.

use crate::{Bytes, SLOT};

pub const SIZE: u32 = 64;
pub const MIPS: u32 = 6;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// Mips re-rendered per frame while the cube is out of date.
const MIPS_PER_FRAME: u32 = 2;

pub(crate) struct Env {
    /// Cube view for sampling.
    pub view: wgpu::TextureView,
    targets: Vec<wgpu::TextureView>,
    faces_bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    sh_pipeline: wgpu::RenderPipeline,
    sh: wgpu::Texture,
    /// Mips (bits) still showing an old sky.
    pending: u32,
    next: u32,
    /// Render every pending mip in the next update.
    all: bool,
    pub sh_dirty: bool,
    pub bytes: u64,
}

impl Env {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        module: &wgpu::ShaderModule,
        sky_layout: &wgpu::BindGroupLayout,
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
        let mut slots = vec![0u8; (SLOT * (6 * MIPS) as u64) as usize];
        for mip in 0..MIPS {
            for face in 0..6 {
                let roughness = mip as f32 / (MIPS - 1) as f32;
                // Narrow lobes (many texels) need few samples.
                let samples = if mip == 0 { 1.0 } else { 24.0 * mip as f32 };
                let mut b = Bytes::default();
                b.floats(&[face as f32, roughness, (SIZE >> mip) as f32, samples]);
                let at = ((mip * 6 + face) as u64 * SLOT) as usize;
                slots[at..at + b.0.len()].copy_from_slice(&b.0);
            }
        }
        queue.write_buffer(&faces, 0, &slots);
        let faces_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("environment face"),
            entries: &[crate::uniform_entry(
                0,
                wgpu::ShaderStages::FRAGMENT,
                Some(16),
            )],
        });
        let faces_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("environment face"),
            layout: &faces_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: crate::slot_binding(&faces, 16),
            }],
        });
        let make = |fs, format: wgpu::TextureFormat| {
            crate::pipeline(
                device,
                crate::PipelineDesc {
                    label: "environment",
                    module,
                    vs: "vs_env",
                    fs: Some(fs),
                    groups: &[Some(sky_layout), Some(&faces_layout)],
                    buffers: &[],
                    target: Some(format.into()),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                    front_cw: false,
                },
            )
        };
        let sh = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ambient SH"),
            size: wgpu::Extent3d {
                width: 9,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            view,
            targets,
            faces_bind,
            pipeline: make("fs_env", FORMAT),
            sh_pipeline: make("fs_sh", SH_FORMAT),
            sh,
            pending: (1 << MIPS) - 1,
            next: 0,
            all: true,
            sh_dirty: true,
            bytes: crate::texture::mip_bytes(SIZE, SIZE, 8) * 6 + 9 * 16,
        }
    }

    /// The sky changed: redo the SH now and the cube now (`all`) or over
    /// the next frames.
    pub fn invalidate(&mut self, all: bool) {
        self.pending = (1 << MIPS) - 1;
        self.sh_dirty = true;
        self.all |= all;
    }

    /// The next update finishes the cube (stills).
    pub fn finish(&mut self) {
        self.all = true;
    }

    /// Records this frame's share of the work; the globals (sky source,
    /// ground radiance) must already hold this frame's sky. The SH lands in
    /// each of `globals` at byte `sh_offset`.
    pub fn update(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        sky: &wgpu::BindGroup,
        globals: &[&wgpu::Buffer],
        sh_offset: u64,
        timer: Option<&crate::timing::Timer>,
    ) {
        if self.sh_dirty {
            self.sh_dirty = false;
            let view = self.sh.create_view(&Default::default());
            let mut pass = crate::color_pass(
                encoder,
                "ambient SH",
                &view,
                None,
                None,
                timer.and_then(|t| t.pass("env")),
            );
            pass.set_pipeline(&self.sh_pipeline);
            pass.set_bind_group(0, sky, &[]);
            pass.set_bind_group(1, &self.faces_bind, &[0]);
            pass.draw(0..3, 0..1);
            drop(pass);
            for buffer in globals {
                encoder.copy_texture_to_buffer(
                    self.sh.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: sh_offset,
                            bytes_per_row: None,
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: 9,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        let mut budget = if self.all { MIPS } else { MIPS_PER_FRAME };
        self.all = false;
        while self.pending != 0 && budget > 0 {
            let mip = self.next;
            self.next = (self.next + 1) % MIPS;
            if self.pending & (1 << mip) == 0 {
                continue;
            }
            self.pending &= !(1 << mip);
            budget -= 1;
            for face in 0..6 {
                let i = (mip * 6 + face) as usize;
                let mut pass = crate::color_pass(
                    encoder,
                    "environment",
                    &self.targets[i],
                    None,
                    None,
                    timer.and_then(|t| t.pass("env")),
                );
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, sky, &[]);
                pass.set_bind_group(1, &self.faces_bind, &[(i as u64 * SLOT) as u32]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
