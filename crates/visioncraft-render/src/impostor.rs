//! Impostors (see impostor.wgsl): baking a plant into an atlas of views,
//! and the CPU twins of the shader's view choice and atlas layout.

use glam::{Mat4, Vec3};

use crate::{Bytes, SLOT};

/// A baked plant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImpostorId(pub(crate) u32);

/// Texels per view.
pub const TILE: u32 = 256;
/// Alpha below which an impostor texel is cut.
pub(crate) const CUTOFF: f32 = 0.5;
const DILATE_PASSES: usize = 8;
const ALBEDO: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const NORMAL: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Atlas layout for `views` ring views plus the top view: (columns, atlas
/// side in texels). Tile `k` sits at column k % columns, row k / columns.
pub fn atlas_layout(views: u32) -> (u32, u32) {
    let tiles = views + 1;
    let cols = (tiles as f32).sqrt().ceil() as u32;
    (cols, cols * TILE)
}

/// The views the shader blends for a model-space direction towards the
/// viewer: (ring view a, ring view b, blend a -> b, weight of the top view).
/// Ring view k looks from azimuth 2 pi k / views, direction (sin, 0, cos).
pub fn impostor_views(d: Vec3, views: u32) -> (u32, u32, f32, f32) {
    let n = views as f32;
    let a = d.x.atan2(d.z) / std::f32::consts::TAU;
    let f = (a + 1.0).fract() * n;
    let i0 = f.floor();
    let elevation = (d.y / d.length().max(1e-6)).clamp(-1.0, 1.0).asin();
    let t = ((elevation - 0.785) / (1.22 - 0.785)).clamp(0.0, 1.0);
    (
        i0 as u32 % views,
        (i0 as u32 + 1) % views,
        f - i0,
        t * t * (3.0 - 2.0 * t),
    )
}

/// Half-extents of the quads: the horizontal radius of the bounds (the
/// widest any azimuth sees them) and their half height; and the radius of
/// the bounding sphere (depth range).
pub(crate) fn extents((lo, hi): (Vec3, Vec3)) -> (f32, f32, f32) {
    let h = (hi - lo) * 0.5;
    (
        (h.x * h.x + h.z * h.z).sqrt().max(1e-3),
        h.y.max(1e-3),
        h.length().max(1e-3),
    )
}

/// The orthographic view-projection that bakes view `k` (`k == views`: from
/// above) of `bounds` in model space: ring views see the horizontal radius
/// by the half height, the top view the horizontal radius both ways, so
/// the quads hug the plant instead of its bounding sphere.
pub(crate) fn bake_matrix(bounds: (Vec3, Vec3), k: u32, views: u32) -> Mat4 {
    let centre = (bounds.0 + bounds.1) * 0.5;
    let (w, h, r) = extents(bounds);
    let h = if k == views { w } else { h };
    let proj = Mat4::orthographic_rh(-w, w, -h, h, 0.0, 4.0 * r);
    let view = if k == views {
        Mat4::look_at_rh(centre + Vec3::Y * 2.0 * r, centre, Vec3::NEG_Z)
    } else {
        let phi = std::f32::consts::TAU * k as f32 / views as f32;
        let d = Vec3::new(phi.sin(), 0.0, phi.cos());
        Mat4::look_at_rh(centre + d * 2.0 * r, centre, Vec3::Y)
    };
    proj * view
}

pub(crate) struct GpuImpostor {
    pub bind: wgpu::BindGroup,
    pub bytes: u64,
}

/// Group 1 of the impostor pipelines: info, albedo, normal, sampler.
pub(crate) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("impostor"),
        entries: &[
            crate::uniform_entry(20, wgpu::ShaderStages::VERTEX_FRAGMENT, None),
            crate::texture_entry(
                21,
                wgpu::TextureSampleType::Float { filterable: true },
                wgpu::TextureViewDimension::D2,
                false,
            ),
            crate::texture_entry(
                22,
                wgpu::TextureSampleType::Float { filterable: true },
                wgpu::TextureViewDimension::D2,
                false,
            ),
            crate::sampler_entry(23, wgpu::SamplerBindingType::Filtering),
        ],
    })
}

pub(crate) struct Baker {
    bake: wgpu::RenderPipeline,
    dilate_layout: wgpu::BindGroupLayout,
    dilate: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

#[allow(clippy::too_many_arguments)]
fn two_targets(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    layout: &wgpu::PipelineLayout,
    buffers: &[Option<wgpu::VertexBufferLayout>],
    depth: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets: &[Some(ALBEDO.into()), Some(NORMAL.into())],
        }),
        primitive: Default::default(),
        depth_stencil: depth,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl Baker {
    pub fn new(
        device: &wgpu::Device,
        l: &crate::Layouts,
        scene: &wgpu::ShaderModule,
        dilate: &wgpu::ShaderModule,
    ) -> Self {
        let bake_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("impostor bake"),
            bind_group_layouts: &[None, Some(&l.material), Some(&l.cascade)],
            immediate_size: 0,
        });
        let bake = two_targets(
            device,
            "impostor bake",
            scene,
            "vs_bake",
            "fs_bake",
            &bake_layout,
            &crate::vertex_layouts(false),
            Some(crate::depth_state(
                true,
                wgpu::CompareFunction::Less,
                Default::default(),
            )),
        );
        let unfilterable = wgpu::TextureSampleType::Float { filterable: false };
        let dilate_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dilate"),
            entries: &[
                crate::texture_entry(0, unfilterable, wgpu::TextureViewDimension::D2, false),
                crate::texture_entry(1, unfilterable, wgpu::TextureViewDimension::D2, false),
            ],
        });
        let dilate_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("dilate"),
                bind_group_layouts: &[Some(&dilate_layout)],
                immediate_size: 0,
            });
        let dilate = two_targets(
            device,
            "dilate",
            dilate,
            "vs_fullscreen",
            "fs_dilate",
            &dilate_pipeline_layout,
            &[],
            None,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("impostor"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            bake,
            dilate_layout,
            dilate,
            sampler,
        }
    }

    /// Bakes `parts` (vertex buffer, index buffer, index count, material
    /// bind group) of a plant whose model-space bounds are `bounds`.
    #[allow(clippy::too_many_arguments)]
    pub fn bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        l: &crate::Layouts,
        mipmapper: &crate::texture::Mipmapper,
        parts: &[(&wgpu::Buffer, &wgpu::Buffer, u32, &wgpu::BindGroup)],
        bounds: (Vec3, Vec3),
        views: u32,
    ) -> GpuImpostor {
        let (cols, size) = atlas_layout(views);
        let centre = (bounds.0 + bounds.1) * 0.5;
        let (half_w, half_h, _) = extents(bounds);
        let target = |label, format, mips| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let mips = crate::texture::mip_count(size, size);
        let albedo = [
            target("impostor albedo", ALBEDO, mips),
            target("impostor albedo", ALBEDO, 1),
        ];
        let normal = [
            target("impostor normal", NORMAL, mips),
            target("impostor normal", NORMAL, 1),
        ];
        let level0 = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: 0,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("impostor depth"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::DEPTH,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        // One view-projection per tile, in dynamic uniform slots.
        let count = views + 1;
        let matrices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("impostor views"),
            size: SLOT * count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut slots = vec![0u8; (SLOT * count as u64) as usize];
        for k in 0..count {
            let mut b = Bytes::default();
            b.floats(&bake_matrix(bounds, k, views).to_cols_array());
            b.floats(&[0.0; 4]);
            let at = (k as u64 * SLOT) as usize;
            slots[at..at + b.0.len()].copy_from_slice(&b.0);
        }
        queue.write_buffer(&matrices, 0, &slots);
        let matrix_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("impostor views"),
            layout: &l.cascade,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: crate::slot_binding(&matrices, crate::CASCADE_SIZE),
            }],
        });
        let identity = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("identity instance"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &identity,
            0,
            &crate::f32_bytes(&Mat4::IDENTITY.to_cols_array()),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let (a, n) = (level0(&albedo[0]), level0(&normal[0]));
            let attach = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("impostor bake"),
                color_attachments: &[attach(&a), attach(&n)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.bake);
            pass.set_vertex_buffer(1, identity.slice(..));
            for k in 0..count {
                let (x, y) = ((k % cols) * TILE, (k / cols) * TILE);
                pass.set_viewport(x as f32, y as f32, TILE as f32, TILE as f32, 0.0, 1.0);
                pass.set_bind_group(2, &matrix_bind, &[(k as u64 * SLOT) as u32]);
                for (vertices, indices, index_count, material) in parts {
                    pass.set_bind_group(1, *material, &[]);
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*index_count, 0, 0..1);
                }
            }
        }
        // Dilate, ping-pong, ending in the mipmapped pair.
        for i in 0..DILATE_PASSES {
            let (src, dst) = if i % 2 == 0 { (0, 1) } else { (1, 0) };
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dilate"),
                layout: &self.dilate_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&level0(&albedo[src])),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&level0(&normal[src])),
                    },
                ],
            });
            let (a, n) = (level0(&albedo[dst]), level0(&normal[dst]));
            let attach = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("dilate"),
                color_attachments: &[attach(&a), attach(&n)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.dilate);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        mipmapper.generate(device, &mut encoder, &albedo[0], false);
        mipmapper.generate(device, &mut encoder, &normal[0], true);
        queue.submit([encoder.finish()]);
        let info = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("impostor info"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &info,
            0,
            &crate::f32_bytes(&[
                centre.x,
                centre.y,
                centre.z,
                half_w,
                views as f32,
                cols as f32,
                1.0 / cols as f32,
                CUTOFF,
                half_w,
                half_h,
                0.0,
                0.0,
            ]),
        );
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("impostor"),
            layout: &l.impostor,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 20,
                    resource: info.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 21,
                    resource: wgpu::BindingResource::TextureView(
                        &albedo[0].create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 22,
                    resource: wgpu::BindingResource::TextureView(
                        &normal[0].create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 23,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        GpuImpostor {
            bind,
            bytes: 2 * crate::texture::mip_bytes(size, size, 4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_holds_every_view() {
        assert_eq!(atlas_layout(8), (3, 768));
        assert_eq!(atlas_layout(12), (4, 1024));
        for views in [8, 12, 15] {
            let (cols, size) = atlas_layout(views);
            assert!(cols * cols > views, "{views} views and the top fit");
            assert_eq!(size, cols * TILE);
        }
    }

    #[test]
    fn view_choice_follows_the_viewer() {
        // Straight along ring view 3 of 12 (azimuth 90 degrees: +X).
        let (a, b, t, top) = impostor_views(Vec3::X, 12);
        assert_eq!((a, b), (3, 4));
        assert!(t.abs() < 1e-4 && top == 0.0);
        // Half-way between views 11 and 0 wraps around.
        let half = -std::f32::consts::TAU / 24.0;
        let (a, b, t, _) = impostor_views(Vec3::new(half.sin(), 0.0, half.cos()), 12);
        assert_eq!((a, b), (11, 0));
        assert!((t - 0.5).abs() < 1e-3, "{t}");
        // From above the top view takes over; from 30 degrees not yet.
        assert!(impostor_views(Vec3::new(0.0, 1.0, 0.01), 12).3 > 0.99);
        let low = Vec3::new(0.0, 30f32.to_radians().sin(), 30f32.to_radians().cos());
        assert_eq!(impostor_views(low, 12).3, 0.0);
        // The bake looks at the plant from the same azimuth.
        let m = bake_matrix((Vec3::splat(-1.0), Vec3::ONE), 3, 12);
        let p = m.project_point3(Vec3::X * 0.5);
        assert!(
            p.x.abs() < 1e-5 && p.z < 0.5,
            "+X is nearest the view-3 camera: {p}"
        );
    }
}
