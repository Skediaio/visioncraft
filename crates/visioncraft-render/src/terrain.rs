//! Terrain: heightmap chunks with painted material layers (see
//! terrain.wgsl). Chunks are ordinary meshes with a second vertex buffer of
//! eight u8 layer weights; they go through the depth pre-pass, shadows and
//! culling like any opaque mesh and only the forward and reflection passes
//! use the terrain pipelines. The layers' maps are copied (resampled on the
//! GPU) into three 512^2 x 8 2D-array textures, their detail textures into
//! two more.

use crate::texture::{Mipmapper, TextureId, mip_count};
use crate::{Bytes, Layouts, PipelineDesc, depth_state, module, pipeline, vertex_layouts};

/// Layers per terrain.
pub const MAX_LAYERS: usize = 8;
/// Side of every layer image in the arrays (texels).
pub const LAYER_SIZE: u32 = 512;
/// Width of the band at the terrain's outer edge that blends into the
/// infinite ground (metres), when a ground is set.
pub const EDGE_BAND_M: f32 = 8.0;
pub(crate) const UNIFORM_SIZE: u64 = (MAX_LAYERS as u64) * 64 + 48;

/// A layer's detail texture (the core's `LayerDetail`): a finer map over the
/// layer's own near the eye, faded out with distance (terrain.wgsl).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainDetail {
    /// sRGB colour map (absent: white, which changes nothing).
    pub base_color_map: Option<TextureId>,
    /// Tangent-space normal map, OpenGL convention (absent: flat).
    pub normal_map: Option<TextureId>,
    /// One texture repeat in metres.
    pub tile_m: f32,
    /// 0..1: how strongly the detail modulates the layer.
    pub strength: f32,
    /// The detail has faded out at this distance to the eye (metres).
    pub fade_m: f32,
}

/// One terrain chunk: per vertex position, normal and the weights of up to
/// eight layers (u8, summing to 255), indices as triangles.
#[derive(Clone, Copy, Debug)]
pub struct TerrainChunk<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub weights: &'a [[u8; 8]],
}

/// A painted terrain material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainLayer {
    /// sRGB colour map (absent: white).
    pub base_color_map: Option<TextureId>,
    /// Tangent-space normal map, OpenGL convention (absent: flat).
    pub normal_map: Option<TextureId>,
    /// Packed occlusion / roughness / metallic (absent: 1 / 1 / 0).
    pub orm_map: Option<TextureId>,
    /// Height (red, white = top) for the height blend, carried in the ORM layer's alpha;
    /// absent: the occlusion stands in for it.
    pub height_map: Option<TextureId>,
    /// One texture repeat in metres.
    pub scale_m: f32,
    /// Linear RGBA factor on the colour map.
    pub base_color: [f32; 4],
    pub roughness: f32,
    /// As `Material::variation`.
    pub variation: f32,
    /// Strength of the normal map (1: as drawn).
    pub relief: f32,
    /// Added to the roughness after the map (negative glossier, positive rougher).
    pub roughness_offset: f32,
    /// Puddles may form on this layer when it is wet (`Material::puddles`; false for lawn).
    pub puddles: bool,
    /// Hex tiling of the maps seen from above (terrain.wgsl), against visible repetition.
    pub hex: bool,
    /// A second, finer texture over the layer's maps near the eye (absent: none).
    pub detail: Option<TerrainDetail>,
}

impl Default for TerrainLayer {
    fn default() -> Self {
        Self {
            base_color_map: None,
            normal_map: None,
            orm_map: None,
            height_map: None,
            scale_m: 2.0,
            base_color: [1.0; 4],
            roughness: 0.9,
            variation: 0.4,
            relief: 1.0,
            roughness_offset: 0.0,
            puddles: true,
            hex: false,
            detail: None,
        }
    }
}

pub(crate) const ARRAY_FORMATS: [wgpu::TextureFormat; 3] = [
    wgpu::TextureFormat::Rgba8UnormSrgb,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba8Unorm,
];

pub(crate) struct TerrainGpu {
    pub buffer: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    /// Placeholder maps for absent layer textures: white, flat, (1, 1, 0).
    defaults: [wgpu::TextureView; 3],
    pub layers: Vec<TerrainLayer>,
    /// World x, z rectangle of all chunks (min x, min z, max x, max z).
    pub edge: [f32; 4],
    pub bytes: u64,
}

fn array_texture(
    device: &wgpu::Device,
    size: u32,
    layers: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain layers"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        mip_level_count: mip_count(size, size),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

#[allow(clippy::too_many_arguments)]
fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    arrays: [&wgpu::TextureView; 3],
    detail: [&wgpu::TextureView; 2],
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 13,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: wgpu::BindingResource::TextureView(arrays[0]),
            },
            wgpu::BindGroupEntry {
                binding: 15,
                resource: wgpu::BindingResource::TextureView(arrays[1]),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: wgpu::BindingResource::TextureView(arrays[2]),
            },
            wgpu::BindGroupEntry {
                binding: 17,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 18,
                resource: wgpu::BindingResource::TextureView(detail[0]),
            },
            wgpu::BindGroupEntry {
                binding: 19,
                resource: wgpu::BindingResource::TextureView(detail[1]),
            },
        ],
    })
}

fn array_view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

impl TerrainGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        // 1x1 placeholders, both as 2D (blit sources) and as 1-layer arrays.
        let texel = [
            [255u8, 255, 255, 255],
            [128, 128, 255, 255],
            [255, 255, 0, 255],
        ];
        let one = |format, rgba: [u8; 4]| {
            let t = array_texture(device, 1, 1, format);
            queue.write_texture(
                t.as_image_copy(),
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            t
        };
        let textures: [wgpu::Texture; 3] = std::array::from_fn(|i| one(ARRAY_FORMATS[i], texel[i]));
        let arrays = textures.each_ref().map(array_view);
        let defaults = textures.each_ref().map(|t| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                ..Default::default()
            })
        });
        let bind = bind(
            device,
            layout,
            &buffer,
            arrays.each_ref(),
            [&arrays[0], &arrays[1]],
            &sampler,
        );
        Self {
            buffer,
            bind,
            sampler,
            defaults,
            layers: Vec::new(),
            edge: [0.0; 4],
            bytes: 0,
        }
    }

    /// Copies each layer's maps into fresh arrays and rebinds. `source`
    /// resolves a texture id to its view (`None` when absent or removed).
    pub fn set_layers<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        mipmapper: &Mipmapper,
        layers: &[TerrainLayer],
        source: impl Fn(Option<TextureId>) -> Option<&'a wgpu::TextureView>,
    ) -> Result<(), String> {
        if layers.is_empty() || layers.len() > MAX_LAYERS {
            return Err(format!(
                "{} terrain layers (1..={MAX_LAYERS})",
                layers.len()
            ));
        }
        let count = layers.len() as u32;
        let textures: [wgpu::Texture; 3] =
            std::array::from_fn(|i| array_texture(device, LAYER_SIZE, count, ARRAY_FORMATS[i]));
        // The layers' detail maps: white and flat for a layer without a detail, so a lookup
        // there would change nothing (the shader skips it anyway).
        let detail: [wgpu::Texture; 2] = [
            array_texture(device, LAYER_SIZE, count, ARRAY_FORMATS[0]),
            array_texture(device, LAYER_SIZE, count, ARRAY_FORMATS[1]),
        ];
        let mut encoder = device.create_command_encoder(&Default::default());
        for (k, layer) in layers.iter().enumerate() {
            let maps = [layer.base_color_map, layer.normal_map, layer.orm_map];
            for (i, map) in maps.into_iter().enumerate() {
                let src = source(map).unwrap_or(&self.defaults[i]);
                let alpha = (i == 2).then(|| source(layer.height_map).unwrap_or(src));
                mipmapper.blit_into_layer(
                    device,
                    &mut encoder,
                    src,
                    &textures[i],
                    k as u32,
                    i > 0,
                    alpha,
                );
            }
            let d = layer.detail;
            let base = source(d.and_then(|d| d.base_color_map)).unwrap_or(&self.defaults[0]);
            let normal = source(d.and_then(|d| d.normal_map)).unwrap_or(&self.defaults[1]);
            mipmapper.blit_into_layer(
                device,
                &mut encoder,
                base,
                &detail[0],
                k as u32,
                false,
                None,
            );
            mipmapper.blit_into_layer(
                device,
                &mut encoder,
                normal,
                &detail[1],
                k as u32,
                true,
                None,
            );
        }
        queue.submit([encoder.finish()]);
        let arrays = textures.each_ref().map(array_view);
        let detail_views = detail.each_ref().map(array_view);
        self.bind = bind(
            device,
            layout,
            &self.buffer,
            arrays.each_ref(),
            [&detail_views[0], &detail_views[1]],
            &self.sampler,
        );
        self.layers = layers.to_vec();
        self.bytes = 5 * crate::texture::mip_bytes(LAYER_SIZE, LAYER_SIZE, 4) * count as u64;
        Ok(())
    }

    /// The uniform: layer factors, the chunks' rectangle, the ground blend
    /// (`ground`: albedo when an infinite ground is set) and whether any layer
    /// has a detail texture (`detail`: on, and the farthest fade distance).
    pub fn write(&self, queue: &wgpu::Queue, ground: Option<[f32; 3]>) {
        let mut b = Bytes::default();
        for i in 0..MAX_LAYERS {
            let l = self.layers.get(i).copied().unwrap_or_default();
            let c = l.base_color;
            b.floats(&[c[0], c[1], c[2], l.roughness_offset.clamp(-1.0, 1.0)]);
            b.floats(&[
                l.roughness.clamp(0.0, 1.0),
                1.0 / l.scale_m.max(1e-3),
                l.variation.clamp(0.0, 1.0),
                l.relief.clamp(0.0, 2.0),
            ]);
            b.floats(&[l.puddles as u32 as f32, l.hex as u32 as f32, 0.0, 0.0]);
            // x 1 / tile, y strength, z fade distance, w flags (1 a detail, 2 a normal map).
            let d = l.detail;
            b.floats(&[
                d.map_or(0.0, |d| 1.0 / d.tile_m.max(1e-3)),
                d.map_or(0.0, |d| d.strength.clamp(0.0, 1.0)),
                d.map_or(0.0, |d| d.fade_m.max(0.1)),
                d.map_or(0.0, |d| 1.0 + d.normal_map.is_some() as u32 as f32),
            ]);
        }
        b.floats(&self.edge);
        let (rgb, band) = ground.map_or(([0.0; 3], 0.0), |c| (c, EDGE_BAND_M));
        b.floats(&[rgb[0], rgb[1], rgb[2], band]);
        let (any, far) = self
            .layers
            .iter()
            .filter_map(|l| l.detail)
            .fold((0.0f32, 0.0f32), |(_, far), d| {
                (1.0, far.max(d.fade_m.max(0.1)))
            });
        b.floats(&[any, far, 0.0, 0.0]);
        debug_assert_eq!(b.0.len() as u64, UNIFORM_SIZE);
        queue.write_buffer(&self.buffer, 0, &b.0);
    }
}

pub(crate) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let array = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain"),
        entries: &[
            crate::uniform_entry(13, wgpu::ShaderStages::FRAGMENT, None),
            array(14),
            array(15),
            array(16),
            crate::sampler_entry(17, wgpu::SamplerBindingType::Filtering),
            array(18),
            array(19),
        ],
    })
}

/// Forward (`fs_terrain`) or reflection (`fs_terrain_reflect`, mirrored).
pub(crate) fn pipeline_for(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    samples: u32,
    reflect: bool,
) -> wgpu::RenderPipeline {
    const WEIGHTS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![9 => Unorm8x4, 10 => Unorm8x4];
    let [mesh, instances] = vertex_layouts(false);
    let buffers = [
        mesh,
        instances,
        Some(wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &WEIGHTS,
        }),
    ];
    pipeline(
        device,
        PipelineDesc {
            label: "terrain",
            module: module(modules, "scene"),
            vs: "vs_terrain",
            fs: Some(if reflect {
                "fs_terrain_reflect"
            } else {
                "fs_terrain"
            }),
            groups: &[
                Some(&l.globals),
                Some(&l.terrain),
                Some(&l.draw),
                Some(&l.forward),
            ],
            buffers: &buffers,
            target: Some(crate::HDR.into()),
            depth: Some(if reflect {
                depth_state(true, wgpu::CompareFunction::Less, Default::default())
            } else {
                depth_state(false, wgpu::CompareFunction::LessEqual, Default::default())
            }),
            cull: Some(wgpu::Face::Back),
            samples,
            constants: &[],
            front_cw: reflect,
        },
    )
}

/// The weights buffer (eight u8 per vertex), checked against the vertex count.
pub(crate) fn weight_bytes(chunk: &TerrainChunk) -> Result<Vec<u8>, String> {
    if chunk.weights.len() != chunk.positions.len() {
        return Err(format!(
            "{} weights for {} positions",
            chunk.weights.len(),
            chunk.positions.len()
        ));
    }
    Ok(chunk.weights.iter().flatten().copied().collect())
}
