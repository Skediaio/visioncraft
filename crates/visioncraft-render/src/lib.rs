//! VisionCraft's daylight renderer: plain mesh, material and instance data in,
//! a tone-mapped sRGB texture out. No UI, no window, WebGPU core only.
//!
//! Passes per frame, in order (see `Renderer::render`):
//! 1. shadow cascades, depth only, `Depth32Float` 2D array (one layer each)
//! 2. depth pre-pass, `Depth32Float` (4x MSAA when enabled)
//! 3. depth resolve: sample 0 -> linear view depth, `R32Float`
//! 4. AO + contact shadows at half resolution, `Rg8Unorm`, depth-aware blur (H, V)
//! 5. forward PBR + sky, `Rgba16Float` (MSAA, resolved into a 1x target);
//!    it upsamples the occlusion with a joint bilateral filter
//! 6. AgX tone map into the output format (`Rgba8UnormSrgb`)
//! 7. overlay lines and points, alpha-blended onto the output
//!
//! MSAA: only the pre-pass and forward pass are multisampled. AO, contact
//! shadows and the overlay's depth test use sample 0 of each pixel (step 3),
//! so at silhouettes the occlusion of one sample is applied to all samples of
//! the pixel; the depth-aware blur and upsample keep that to a one-pixel seam.
//!
//! Quality: on the reference laptop GPU (Intel UHD 620), 1920x1080, budget
//! scene (200k static + 5000 x 500 instanced triangles), native offscreen:
//! `LOW` and `MEDIUM` hold 30 fps, `HIGH` does not (about 18-20 fps).

use std::num::NonZeroU64;

use glam::{Mat4, Vec2, Vec3, Vec4};

pub mod demo;
pub mod shadow;
pub mod sky;
pub mod tonemap;

pub use sky::Sun;

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const LINEAR_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const OCCLUSION: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
const MSAA_SAMPLES: u32 = 4;
/// Uniform slot size: WebGPU's default `minUniformBufferOffsetAlignment`.
const SLOT: u64 = 256;
const VERTEX_STRIDE: u64 = 28; // position, normal, face id
const AO_RADIUS: f32 = 0.6; // metres
const AO_INTENSITY: f32 = 1.5;
/// Instances are grouped into square ground cells of this size (metres) so
/// shadow cascades and the camera can skip the ones they do not see.
const CLUSTER_CELL: f32 = 8.0;

const COMMON: &str = include_str!("common.wgsl");
/// Every shader module as (name, source parts); WGSL has no includes, so the
/// shared declarations are prepended. Also validated with naga in a unit test.
const SHADERS: [(&str, &[&str]); 7] = [
    ("scene", &[COMMON, include_str!("scene.wgsl")]),
    ("sky", &[COMMON, include_str!("sky.wgsl")]),
    ("depth", &[COMMON, include_str!("depth.wgsl")]),
    ("ssao", &[COMMON, include_str!("ssao.wgsl")]),
    ("blur", &[COMMON, include_str!("blur.wgsl")]),
    ("tonemap", &[include_str!("tonemap.wgsl")]),
    ("overlay", &[COMMON, include_str!("overlay.wgsl")]),
];

/// Triangle mesh in plain slices. `normals` match `positions`; `face_ids`
/// (optional, per vertex) name the CAD face each triangle belongs to.
#[derive(Clone, Copy, Debug)]
pub struct Mesh<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub face_ids: Option<&'a [u32]>,
}

/// Metallic-roughness material (glTF 2.0 core parameters).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA. Alpha is carried but ignored until transparency lands.
    pub base_color: [f32; 4],
    pub metallic: f32,
    /// Perceptual roughness, 0..1.
    pub roughness: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(u32);

/// One end of an overlay segment. Consecutive pairs form segments; a pair
/// with equal positions draws a round point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineVertex {
    pub position: [f32; 3],
    /// sRGB-encoded RGBA, as UI colours usually are.
    pub color: [u8; 4],
    /// Thickness in physical pixels.
    pub width: f32,
}

/// Effect switches so the app can fall back on slow GPUs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quality {
    pub shadow_resolution: u32,
    /// 1..=4.
    pub cascades: u32,
    /// Ambient-occlusion taps per pixel, 0 = off.
    pub ao_samples: u32,
    pub contact_shadows: bool,
    /// 4x MSAA on the depth pre-pass and forward pass.
    pub msaa: bool,
}

impl Quality {
    pub const LOW: Quality = Quality {
        shadow_resolution: 1024,
        cascades: 2,
        ao_samples: 0,
        contact_shadows: false,
        msaa: false,
    };
    pub const MEDIUM: Quality = Quality {
        shadow_resolution: 2048,
        cascades: 2,
        ao_samples: 8,
        contact_shadows: true,
        msaa: false,
    };
    pub const HIGH: Quality = Quality {
        shadow_resolution: 2048,
        cascades: 4,
        ao_samples: 16,
        contact_shadows: true,
        msaa: true,
    };
    pub const PRESETS: [(&'static str, Quality); 3] = [
        ("low", Quality::LOW),
        ("medium", Quality::MEDIUM),
        ("high", Quality::HIGH),
    ];

    fn samples(&self) -> u32 {
        if self.msaa { MSAA_SAMPLES } else { 1 }
    }
}

type Bounds = (Vec3, Vec3);

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    bounds: Option<Bounds>,
}

struct Item {
    mesh: MeshId,
    material: MaterialId,
    /// Sorted by `CLUSTER_CELL` so each cluster is a contiguous range.
    transforms: Vec<Mat4>,
    instances: wgpu::Buffer,
    bounds: Option<Bounds>,
    clusters: Vec<Cluster>,
}

/// Instances in one ground cell: culled as a unit, drawn as one range.
struct Cluster {
    range: std::ops::Range<u32>,
    bounds: Option<Bounds>,
}

struct FarCache {
    eye: Vec3,
    proj: Mat4,
    matrix: Mat4,
}

struct GpuMaterial {
    buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

struct Layouts {
    globals: wgpu::BindGroupLayout,
    material: wgpu::BindGroupLayout,
    draw: wgpu::BindGroupLayout,
    cascade: wgpu::BindGroupLayout,
    forward: wgpu::BindGroupLayout,
    depth_ms: wgpu::BindGroupLayout,
    depth_1x: wgpu::BindGroupLayout,
    texture: wgpu::BindGroupLayout,
    blur: wgpu::BindGroupLayout,
}

/// Pipelines that depend on the MSAA sample count.
struct SampledPipelines {
    depth: wgpu::RenderPipeline,
    forward: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
}

struct Pipelines {
    shadow: wgpu::RenderPipeline,
    ssao: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    overlay: wgpu::RenderPipeline,
}

/// Everything sized by the viewport or the quality settings.
struct Targets {
    depth: wgpu::TextureView,
    hdr_msaa: Option<wgpu::TextureView>,
    hdr: wgpu::TextureView,
    linear_depth: wgpu::TextureView,
    occlusion: [wgpu::TextureView; 2],
    output: wgpu::Texture,
    /// sRGB view the passes render through.
    output_view: wgpu::TextureView,
    /// View in the texture's own format, returned by `render`.
    display_view: wgpu::TextureView,
    shadow_layers: Vec<wgpu::TextureView>,
    resolve_bind: wgpu::BindGroup,
    ssao_bind: wgpu::BindGroup,
    blur_bind: [wgpu::BindGroup; 2],
    tonemap_bind: wgpu::BindGroup,
    overlay_bind: wgpu::BindGroup,
    forward_bind: wgpu::BindGroup,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    output_format: wgpu::TextureFormat,
    layouts: Layouts,
    modules: Vec<wgpu::ShaderModule>,
    pipelines: Pipelines,
    sampled: SampledPipelines,
    targets: Targets,
    size: (u32, u32),
    quality: Quality,

    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    cascade_buffer: wgpu::Buffer,
    cascade_bind: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    draw_bind: wgpu::BindGroup,
    draw_capacity: usize,
    white: wgpu::TextureView,
    material_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,

    meshes: Vec<Option<GpuMesh>>,
    materials: Vec<GpuMaterial>,
    items: Vec<Option<Item>>,
    bounds_dirty: bool,
    scene_bounds: Option<Bounds>,

    view: Mat4,
    proj: Mat4,
    eye: Vec3,
    sun: Sun,
    sky: sky::Sky,
    sh: [Vec3; 9],
    ev100: f32,
    shadow_distance: f32,
    highlight: Option<(MeshId, u32)>,
    /// The cached last shadow cascade (see `shadow`): where the camera was
    /// when it was rendered, and with which projection and light matrix.
    far_cache: Option<FarCache>,
    /// Depth-tested and on-top segments; kept so either list can be
    /// replaced alone.
    overlay_cpu: (Vec<LineVertex>, Vec<LineVertex>),
    overlay: Option<wgpu::Buffer>,
    overlay_count: u32,
    overlay_on_top: u32,
}

impl Renderer {
    /// `format` is the output texture's format: `Rgba8UnormSrgb`, or
    /// `Rgba8Unorm` for sRGB-encoded bytes in a plain texture (what egui
    /// expects of user textures). Either way the passes write through an sRGB
    /// view, so the encode and the overlay's blending happen in hardware.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layouts = Layouts::new(device);
        let modules: Vec<_> = SHADERS
            .iter()
            .map(|(name, parts)| {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(name),
                    source: wgpu::ShaderSource::Wgsl(parts.concat().into()),
                })
            })
            .collect();
        let quality = Quality::HIGH;
        let pipelines = Pipelines::new(device, &layouts, &modules, format.add_srgb_suffix());
        let sampled = SampledPipelines::new(device, &layouts, &modules, quality.samples());

        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = uniform("globals", globals_size());
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layouts.globals,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let cascade_buffer = uniform("cascades", SLOT * shadow::MAX_CASCADES as u64);
        let cascade_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cascade"),
            layout: &layouts.cascade,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: slot_binding(&cascade_buffer, 64),
            }],
        });
        let draw_capacity = 64;
        let draw_buffer = uniform("draws", SLOT * draw_capacity as u64);
        let draw_bind = draw_bind_group(device, &layouts.draw, &draw_buffer);

        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            white_texture.as_image_copy(),
            &[255; 4],
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
        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let targets = Targets::new(device, &layouts, format, quality, (1, 1), &shadow_sampler);
        let sun = Sun::from_angles(180.0, 45.0);
        let sky = sky::Sky::new(sun.direction);
        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            output_format: format,
            layouts,
            modules,
            pipelines,
            sampled,
            targets,
            size: (1, 1),
            quality,
            globals,
            globals_bind,
            cascade_buffer,
            cascade_bind,
            draw_buffer,
            draw_bind,
            draw_capacity,
            white: white_texture.create_view(&Default::default()),
            material_sampler,
            shadow_sampler,
            meshes: Vec::new(),
            materials: Vec::new(),
            items: Vec::new(),
            bounds_dirty: false,
            scene_bounds: None,
            view: Mat4::look_at_rh(Vec3::new(0.0, 1.7, 10.0), Vec3::ZERO, Vec3::Y),
            proj: Mat4::perspective_rh(1.0, 1.0, 0.1, 1000.0),
            eye: Vec3::new(0.0, 1.7, 10.0),
            sun,
            sky,
            sh: [Vec3::ZERO; 9],
            ev100: 15.0,
            shadow_distance: 80.0,
            highlight: None,
            far_cache: None,
            overlay_cpu: (Vec::new(), Vec::new()),
            overlay: None,
            overlay_count: 0,
            overlay_on_top: 0,
        };
        renderer.update_ambient();
        renderer
    }

    /// Recreates the size-dependent targets. Call before the first `render`
    /// and whenever the output size changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
        self.rebuild_targets();
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn set_quality(&mut self, quality: Quality) {
        let quality = Quality {
            cascades: quality.cascades.clamp(1, shadow::MAX_CASCADES as u32),
            ..quality
        };
        if quality == self.quality {
            return;
        }
        if quality.msaa != self.quality.msaa {
            self.sampled = SampledPipelines::new(
                &self.device,
                &self.layouts,
                &self.modules,
                quality.samples(),
            );
        }
        self.quality = quality;
        self.rebuild_targets();
    }

    /// Camera: world-to-view matrix, a perspective projection with depth
    /// 0..1 (`Mat4::perspective_rh` or `perspective_infinite_rh`), and the
    /// eye position in world space.
    pub fn set_camera(&mut self, view: Mat4, proj: Mat4, position: Vec3) {
        self.view = view;
        self.proj = proj;
        self.eye = position;
    }

    pub fn set_sun(&mut self, sun: &Sun) {
        let sun = Sun {
            direction: sun.direction.normalize(),
            ..*sun
        };
        if sun == self.sun {
            return; // keeps the cached cascade and skips the ambient update
        }
        self.sun = sun;
        self.far_cache = None;
        self.sky = sky::Sky::new(self.sun.direction);
        self.update_ambient();
    }

    /// Camera exposure as EV at ISO 100 (sunny day ~15, overcast ~12).
    pub fn set_exposure(&mut self, ev100: f32) {
        if ev100 == self.ev100 {
            return;
        }
        self.ev100 = ev100;
        self.update_ambient();
    }

    /// How far from the camera sun shadows reach (metres, default 80).
    pub fn set_shadow_distance(&mut self, metres: f32) {
        self.shadow_distance = metres.max(1.0);
        self.far_cache = None;
    }

    pub fn add_mesh(&mut self, mesh: &Mesh) -> Result<MeshId, String> {
        let gpu = self.upload_mesh(mesh)?;
        self.meshes.push(Some(gpu));
        Ok(MeshId(self.meshes.len() as u32 - 1))
    }

    /// Replaces a mesh in place: every item drawing it picks up the new data.
    pub fn replace_mesh(&mut self, id: MeshId, mesh: &Mesh) -> Result<(), String> {
        let slot = self.meshes.get_mut(id.0 as usize).ok_or("unknown mesh")?;
        if slot.is_none() {
            return Err("mesh was removed".into());
        }
        let gpu = upload_mesh(&self.device, &self.queue, mesh)?;
        self.meshes[id.0 as usize] = Some(gpu);
        self.bounds_dirty = true;
        Ok(())
    }

    /// Frees a mesh. Items still referring to it draw nothing.
    pub fn remove_mesh(&mut self, id: MeshId) {
        if let Some(slot) = self.meshes.get_mut(id.0 as usize) {
            *slot = None;
            self.bounds_dirty = true;
        }
    }

    fn upload_mesh(&self, mesh: &Mesh) -> Result<GpuMesh, String> {
        upload_mesh(&self.device, &self.queue, mesh)
    }

    pub fn add_material(&mut self, material: &Material) -> MaterialId {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.layouts.material,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.white),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.material_sampler),
                },
            ],
        });
        self.materials.push(GpuMaterial { buffer, bind });
        let id = MaterialId(self.materials.len() as u32 - 1);
        self.set_material(id, material);
        id
    }

    pub fn set_material(&mut self, id: MaterialId, material: &Material) {
        let Some(m) = self.materials.get(id.0 as usize) else {
            return;
        };
        let mut b = Bytes::default();
        b.floats(&material.base_color);
        b.floats(&[
            material.metallic.clamp(0.0, 1.0),
            material.roughness.clamp(0.0, 1.0),
            0.0,
            0.0,
        ]);
        self.queue.write_buffer(&m.buffer, 0, &b.0);
    }

    /// A draw item: one mesh, one material, any number of instance
    /// transforms (model-to-world). One instance buffer and one draw call.
    pub fn add_item(&mut self, mesh: MeshId, material: MaterialId, instances: &[Mat4]) -> ItemId {
        let id = ItemId(self.items.len() as u32);
        self.items.push(Some(Item {
            mesh,
            material,
            transforms: Vec::new(),
            instances: self.instance_buffer(0),
            bounds: None,
            clusters: Vec::new(),
        }));
        if self.items.len() > self.draw_capacity {
            self.draw_capacity = self.items.len().next_power_of_two();
            self.draw_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("draws"),
                size: SLOT * self.draw_capacity as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.draw_bind = draw_bind_group(&self.device, &self.layouts.draw, &self.draw_buffer);
            for (i, item) in self.items.iter().enumerate() {
                if let Some(item) = item {
                    self.queue.write_buffer(
                        &self.draw_buffer,
                        i as u64 * SLOT,
                        &mesh_slot(item.mesh),
                    );
                }
            }
        } else {
            self.queue
                .write_buffer(&self.draw_buffer, id.0 as u64 * SLOT, &mesh_slot(mesh));
        }
        self.set_instances(id, instances);
        id
    }

    pub fn set_instances(&mut self, id: ItemId, instances: &[Mat4]) {
        let grow = match self.items.get(id.0 as usize) {
            Some(Some(item)) => item.instances.size() < (instances.len() as u64 * 64).max(64),
            _ => return,
        };
        let buffer = grow.then(|| self.instance_buffer(instances.len()));
        let Some(Some(item)) = self.items.get_mut(id.0 as usize) else {
            return;
        };
        if let Some(buffer) = buffer {
            item.instances = buffer;
        }
        // Group by ground cell. This reorders the instances on the GPU; fine
        // while nothing reads `instance_index` (per-instance data later must
        // be permuted the same way).
        let cell = |m: &Mat4| {
            (
                (m.w_axis.x / CLUSTER_CELL).floor() as i32,
                (m.w_axis.z / CLUSTER_CELL).floor() as i32,
            )
        };
        let mut sorted = instances.to_vec();
        sorted.sort_by_key(cell);
        item.clusters.clear();
        for (i, m) in sorted.iter().enumerate() {
            match item.clusters.last_mut() {
                Some(c) if cell(&sorted[c.range.start as usize]) == cell(m) => c.range.end += 1,
                _ => item.clusters.push(Cluster {
                    range: i as u32..i as u32 + 1,
                    bounds: None,
                }),
            }
        }
        let mut b = Bytes::default();
        for m in &sorted {
            b.floats(&m.to_cols_array());
        }
        self.queue.write_buffer(&item.instances, 0, &b.0);
        item.transforms = sorted;
        self.bounds_dirty = true;
    }

    pub fn remove_item(&mut self, id: ItemId) {
        if let Some(slot) = self.items.get_mut(id.0 as usize) {
            *slot = None;
            self.bounds_dirty = true;
        }
    }

    fn instance_buffer(&self, count: usize) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (count as u64 * 64).max(64),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Tints one CAD face (by the mesh's per-vertex face ids), or none.
    pub fn set_highlight(&mut self, highlight: Option<(MeshId, u32)>) {
        self.highlight = highlight;
    }

    /// Overlay segments tested against the scene depth (grid, dimensions).
    pub fn set_overlay_lines(&mut self, lines: &[LineVertex]) {
        self.overlay_cpu.0 = lines.to_vec();
        self.upload_overlay();
    }

    /// Overlay segments drawn over everything (selection outlines, tools).
    pub fn set_overlay_lines_on_top(&mut self, lines: &[LineVertex]) {
        self.overlay_cpu.1 = lines.to_vec();
        self.upload_overlay();
    }

    /// The pixel (top-left origin, physical pixels of the output) where a
    /// world position appears, for placing labels. `None` behind the camera.
    pub fn project(&self, world: Vec3) -> Option<Vec2> {
        let c = self.proj * self.view * world.extend(1.0);
        if c.w <= 1e-6 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        Some(Vec2::new(
            (ndc.x * 0.5 + 0.5) * self.size.0 as f32,
            (0.5 - ndc.y * 0.5) * self.size.1 as f32,
        ))
    }

    /// The tone-mapped output texture, for readback (`COPY_SRC`).
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.targets.output
    }

    /// Records every pass of a frame; returns the output view.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder) -> &wgpu::TextureView {
        if self.bounds_dirty {
            self.far_cache = None; // the scene changed
        }
        self.update_bounds();
        let mut cascades = shadow::compute(
            self.view,
            self.proj,
            self.sun.direction,
            self.quality.cascades as usize,
            self.quality.shadow_resolution,
            self.shadow_distance,
            self.scene_bounds,
        );
        // Reuse the camera-centred last cascade while it still covers us.
        let last = cascades.count - 1;
        let sun_lit = self.sun.illuminance > 0.0;
        let reuse = match &self.far_cache {
            Some(c) => c.proj == self.proj && c.eye.distance(self.eye) < cascades.margin,
            None => false,
        };
        if reuse {
            cascades.view_proj[last] = self.far_cache.as_ref().map_or(Mat4::IDENTITY, |c| c.matrix);
        } else if cascades.margin > 0.0 && sun_lit {
            self.far_cache = Some(FarCache {
                eye: self.eye,
                proj: self.proj,
                matrix: cascades.view_proj[last],
            });
        }
        self.write_globals(&cascades);
        let mut slots = vec![0u8; SLOT as usize * shadow::MAX_CASCADES];
        for (i, m) in cascades.view_proj.iter().enumerate() {
            let mut b = Bytes::default();
            b.floats(&m.to_cols_array());
            slots[i * SLOT as usize..i * SLOT as usize + 64].copy_from_slice(&b.0);
        }
        self.queue.write_buffer(&self.cascade_buffer, 0, &slots);

        let t = &self.targets;
        for (c, layer) in t.shadow_layers.iter().enumerate() {
            if reuse && c == last {
                continue;
            }
            let mut pass = depth_pass(encoder, "shadow", layer, wgpu::StoreOp::Store);
            if !sun_lit {
                continue;
            }
            pass.set_pipeline(&self.pipelines.shadow);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(2, &self.cascade_bind, &[(c as u64 * SLOT) as u32]);
            self.draw_items(&mut pass, cascades.view_proj[c], false, false);
        }
        {
            let mut pass = depth_pass(encoder, "depth pre-pass", &t.depth, wgpu::StoreOp::Store);
            pass.set_pipeline(&self.sampled.depth);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            self.draw_items(&mut pass, self.proj * self.view, true, false);
        }
        {
            let mut pass = color_pass(encoder, "depth resolve", &t.linear_depth, None, None);
            pass.set_pipeline(&self.sampled.resolve);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &t.resolve_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        if self.quality.ao_samples > 0 || self.quality.contact_shadows {
            let mut pass = color_pass(encoder, "ssao", &t.occlusion[0], None, None);
            pass.set_pipeline(&self.pipelines.ssao);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &t.ssao_bind, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            for (i, pipeline) in [&self.pipelines.blur_h, &self.pipelines.blur_v]
                .into_iter()
                .enumerate()
            {
                let mut pass = color_pass(encoder, "blur", &t.occlusion[1 - i], None, None);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.globals_bind, &[]);
                pass.set_bind_group(1, &t.blur_bind[i], &[]);
                pass.draw(0..3, 0..1);
            }
        } else {
            color_pass(
                encoder,
                "no ssao",
                &t.occlusion[0],
                Some(wgpu::Color::WHITE),
                None,
            );
        }
        {
            let (view, resolve) = match &t.hdr_msaa {
                Some(msaa) => (msaa, Some(&t.hdr)),
                None => (&t.hdr, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forward"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: if resolve.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.sampled.forward);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(3, &t.forward_bind, &[]);
            self.draw_items(&mut pass, self.proj * self.view, true, true);
            pass.set_pipeline(&self.sampled.sky);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = color_pass(
                encoder,
                "tonemap",
                &t.output_view,
                Some(wgpu::Color::BLACK),
                None,
            );
            pass.set_pipeline(&self.pipelines.tonemap);
            pass.set_bind_group(0, &t.tonemap_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        if let (Some(buffer), true) = (&self.overlay, self.overlay_count > 0) {
            let mut pass = color_pass(encoder, "overlay", &t.output_view, None, Some(()));
            pass.set_pipeline(&self.pipelines.overlay);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &t.overlay_bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..self.overlay_count);
        }
        &self.targets.display_view
    }

    /// Draws every live item whose bounds overlap `cull` (a view-projection).
    /// `near_far` also culls on depth; `forward` binds material and draw data.
    fn draw_items(&self, pass: &mut wgpu::RenderPass, cull: Mat4, near_far: bool, forward: bool) {
        for (i, item) in self.items.iter().enumerate() {
            let Some(item) = item else { continue };
            let Some(Some(mesh)) = self.meshes.get(item.mesh.0 as usize) else {
                continue;
            };
            let Some(bounds) = item.bounds else { continue };
            if outside(cull, bounds, near_far) {
                continue;
            }
            if forward {
                let Some(material) = self.materials.get(item.material.0 as usize) else {
                    continue;
                };
                pass.set_bind_group(1, &material.bind, &[]);
                pass.set_bind_group(2, &self.draw_bind, &[(i as u64 * SLOT) as u32]);
            }
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_vertex_buffer(1, item.instances.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            // One draw per run of consecutive visible clusters.
            let mut run: Option<std::ops::Range<u32>> = None;
            for c in &item.clusters {
                if c.bounds.is_none_or(|b| outside(cull, b, near_far)) {
                    continue;
                }
                run = match run {
                    Some(r) if r.end == c.range.start => Some(r.start..c.range.end),
                    Some(r) => {
                        pass.draw_indexed(0..mesh.index_count, 0, r);
                        Some(c.range.clone())
                    }
                    None => Some(c.range.clone()),
                };
            }
            if let Some(r) = run {
                pass.draw_indexed(0..mesh.index_count, 0, r);
            }
        }
    }

    fn update_bounds(&mut self) {
        if !self.bounds_dirty {
            return;
        }
        self.bounds_dirty = false;
        let mut scene: Option<Bounds> = None;
        for item in self.items.iter_mut().flatten() {
            let mesh_bounds = self
                .meshes
                .get(item.mesh.0 as usize)
                .and_then(|m| m.as_ref())
                .and_then(|m| m.bounds);
            for c in &mut item.clusters {
                c.bounds = mesh_bounds.and_then(|b| {
                    item.transforms[c.range.start as usize..c.range.end as usize]
                        .iter()
                        .map(|m| transform_bounds(*m, b))
                        .reduce(union)
                });
            }
            item.bounds = item.clusters.iter().filter_map(|c| c.bounds).reduce(union);
            if let Some(b) = item.bounds {
                scene = Some(scene.map_or(b, |s| union(s, b)));
            }
        }
        self.scene_bounds = scene;
    }

    fn exposure(&self) -> f32 {
        // Lagarde & de Rousiers 2014, eq. 12: saturation-based sensitivity.
        1.0 / (1.2 * 2f32.powf(self.ev100))
    }

    fn update_ambient(&mut self) {
        let night = sky::NIGHT / self.exposure();
        self.sh = self.sky.irradiance_sh(&self.sun, night);
    }

    fn write_globals(&self, cascades: &shadow::Cascades) {
        let exposure = self.exposure();
        let view_proj = self.proj * self.view;
        let q = &self.quality;
        let mut b = Bytes::default();
        for m in [view_proj, self.view, self.proj, view_proj.inverse()] {
            b.floats(&m.to_cols_array());
        }
        for m in &cascades.view_proj {
            b.floats(&m.to_cols_array());
        }
        let sun_light = self.sun.color * self.sun.illuminance * exposure;
        let disc_solid_angle = std::f32::consts::PI * sky::SUN_DISC_RADIUS * sky::SUN_DISC_RADIUS;
        let disc = (self.sun.illuminance / disc_solid_angle * exposure).min(64.0);
        b.floats(&[self.eye.x, self.eye.y, self.eye.z, 0.0]);
        b.floats(&self.sun.direction.extend(disc).to_array());
        b.floats(&sun_light.extend(sky::SUN_DISC_RADIUS.cos()).to_array());
        for c in &self.sh {
            b.floats(&(*c * exposure).extend(0.0).to_array());
        }
        b.floats(&cascades.splits);
        b.floats(&cascades.texel);
        let res = q.shadow_resolution as f32;
        b.floats(&[cascades.count as f32, res, 1.0 / res, self.shadow_distance]);
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        b.floats(&[w, h, 1.0 / w, 1.0 / h]);
        b.floats(&[
            q.ao_samples as f32,
            if q.contact_shadows { 1.0 } else { 0.0 },
            AO_RADIUS,
            AO_INTENSITY,
        ]);
        for p in &self.sky.perez {
            b.floats(&p.extend(0.0).to_array());
        }
        b.floats(&self.sky.zenith.extend(self.sky.day).to_array());
        b.floats(&sky::NIGHT.extend(1000.0 * exposure).to_array());
        let (mesh, face) = self
            .highlight
            .map_or((u32::MAX, u32::MAX), |(m, f)| (m.0, f));
        for u in [mesh, face, self.overlay_on_top, 0] {
            b.0.extend_from_slice(&u.to_le_bytes());
        }
        debug_assert_eq!(b.0.len() as u64, globals_size());
        self.queue.write_buffer(&self.globals, 0, &b.0);
    }

    fn upload_overlay(&mut self) {
        let (tested, on_top) = &self.overlay_cpu;
        let mut b = Vec::with_capacity((tested.len() + on_top.len()) / 2 * 40);
        for list in [tested, on_top] {
            for v in list.as_chunks::<2>().0.iter().flatten() {
                for f in v.position {
                    b.extend_from_slice(&f.to_le_bytes());
                }
                b.extend_from_slice(&v.color);
                b.extend_from_slice(&v.width.to_le_bytes());
            }
        }
        self.overlay_on_top = (tested.len() / 2) as u32;
        self.overlay_count = (b.len() / 40) as u32;
        if b.is_empty() {
            return;
        }
        if self
            .overlay
            .as_ref()
            .is_none_or(|o| o.size() < b.len() as u64)
        {
            self.overlay = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay"),
                size: (b.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(o) = &self.overlay {
            self.queue.write_buffer(o, 0, &b);
        }
    }

    fn rebuild_targets(&mut self) {
        self.far_cache = None;
        self.targets = Targets::new(
            &self.device,
            &self.layouts,
            self.output_format,
            self.quality,
            self.size,
            &self.shadow_sampler,
        );
    }
}

fn upload_mesh(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Result<GpuMesh, String> {
    let n = mesh.positions.len();
    if mesh.normals.len() != n {
        return Err(format!("{} normals for {n} positions", mesh.normals.len()));
    }
    if let Some(f) = mesh.face_ids
        && f.len() != n
    {
        return Err(format!("{} face ids for {n} positions", f.len()));
    }
    if !mesh.indices.len().is_multiple_of(3) {
        return Err(format!(
            "{} indices is not a whole number of triangles",
            mesh.indices.len()
        ));
    }
    if let Some(i) = mesh.indices.iter().find(|&&i| i as usize >= n) {
        return Err(format!("index {i} out of range for {n} vertices"));
    }
    let mut v = Bytes::default();
    for i in 0..n {
        v.floats(&mesh.positions[i]);
        v.floats(&mesh.normals[i]);
        let face = mesh.face_ids.map_or(u32::MAX, |f| f[i]);
        v.0.extend_from_slice(&face.to_le_bytes());
    }
    let mut ix = Vec::with_capacity(mesh.indices.len() * 4);
    for i in mesh.indices {
        ix.extend_from_slice(&i.to_le_bytes());
    }
    let buffer = |label, bytes: &[u8], usage| {
        let b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (bytes.len() as u64).max(4).next_multiple_of(4),
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&b, 0, bytes);
        b
    };
    let bounds = mesh
        .positions
        .iter()
        .map(|p| (Vec3::from(*p), Vec3::from(*p)))
        .reduce(union);
    Ok(GpuMesh {
        vertices: buffer("vertices", &v.0, wgpu::BufferUsages::VERTEX),
        indices: buffer("indices", &ix, wgpu::BufferUsages::INDEX),
        index_count: mesh.indices.len() as u32,
        bounds,
    })
}

fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.max(b.1))
}

fn corners((lo, hi): Bounds) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    })
}

fn transform_bounds(m: Mat4, b: Bounds) -> Bounds {
    corners(b)
        .map(|c| m.transform_point3(c))
        .into_iter()
        .map(|p| (p, p))
        .reduce(union)
        .unwrap_or(b)
}

/// True when the box is entirely outside one clip plane of `view_proj`.
fn outside(view_proj: Mat4, b: Bounds, near_far: bool) -> bool {
    let c = corners(b).map(|p| view_proj * p.extend(1.0));
    let all = |f: &dyn Fn(Vec4) -> bool| c.iter().all(|&p| f(p));
    all(&|p| p.x < -p.w)
        || all(&|p| p.x > p.w)
        || all(&|p| p.y < -p.w)
        || all(&|p| p.y > p.w)
        || (near_far && (all(&|p| p.z < 0.0) || all(&|p| p.z > p.w)))
}

/// Little-endian byte packing for uniforms and vertex data (no bytemuck,
/// no unsafe).
#[derive(Default)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn floats(&mut self, f: &[f32]) {
        for x in f {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
    }
}

fn globals_size() -> u64 {
    // 8 matrices, 25 vec4s; see common.wgsl.
    8 * 64 + 25 * 16
}

fn mesh_slot(mesh: MeshId) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&mesh.0.to_le_bytes());
    out
}

fn slot_binding(buffer: &wgpu::Buffer, size: u64) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset: 0,
        size: NonZeroU64::new(size),
    })
}

fn draw_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("draw"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: slot_binding(buffer, 16),
        }],
    })
}

fn depth_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    store: wgpu::StoreOp,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A single-target colour pass. `clear: None` with `load: None` clears to
/// black for fullscreen passes; `load: Some(())` keeps the contents.
fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
    load: Option<()>,
) -> wgpu::RenderPass<'e> {
    let load = match (load, clear) {
        (Some(()), _) => wgpu::LoadOp::Load,
        (None, Some(c)) => wgpu::LoadOp::Clear(c),
        (None, None) => wgpu::LoadOp::Clear(wgpu::Color::BLACK),
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
    multisampled: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: Option<u64>,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic.is_some(),
            min_binding_size: dynamic.and_then(NonZeroU64::new),
        },
        count: None,
    }
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        use wgpu::TextureSampleType as S;
        use wgpu::TextureViewDimension as D;
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let unfilterable = S::Float { filterable: false };
        let fs = wgpu::ShaderStages::FRAGMENT;
        Self {
            globals: layout(
                "globals",
                &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, None)],
            ),
            material: layout(
                "material",
                &[
                    uniform_entry(0, fs, None),
                    texture_entry(1, S::Float { filterable: true }, D::D2, false),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: fs,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            ),
            draw: layout("draw", &[uniform_entry(0, fs, Some(16))]),
            cascade: layout(
                "cascade",
                &[uniform_entry(1, wgpu::ShaderStages::VERTEX, Some(64))],
            ),
            forward: layout(
                "forward",
                &[
                    texture_entry(0, S::Depth, D::D2Array, false),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: fs,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                    texture_entry(2, unfilterable, D::D2, false),
                    texture_entry(3, unfilterable, D::D2, false),
                ],
            ),
            depth_ms: layout("depth ms", &[texture_entry(0, S::Depth, D::D2, true)]),
            depth_1x: layout("depth 1x", &[texture_entry(1, S::Depth, D::D2, false)]),
            texture: layout("texture", &[texture_entry(0, unfilterable, D::D2, false)]),
            blur: layout(
                "blur",
                &[
                    texture_entry(0, unfilterable, D::D2, false),
                    texture_entry(1, unfilterable, D::D2, false),
                ],
            ),
        }
    }
}

/// Mesh and instance buffer layouts; `depth_only` declares just the
/// position, which is all the depth and shadow passes fetch.
fn vertex_layouts(depth_only: bool) -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    const MESH: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];
    const INSTANCE: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: VERTEX_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: if depth_only { &MESH[..1] } else { &MESH },
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: 64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE,
        }),
    ]
}

struct PipelineDesc<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: Option<&'a str>,
    groups: &'a [Option<&'a wgpu::BindGroupLayout>],
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    target: Option<wgpu::ColorTargetState>,
    depth: Option<wgpu::DepthStencilState>,
    cull: Option<wgpu::Face>,
    samples: u32,
    constants: &'a [(&'a str, f64)],
}

fn pipeline(device: &wgpu::Device, d: PipelineDesc) -> wgpu::RenderPipeline {
    let options = wgpu::PipelineCompilationOptions {
        constants: d.constants,
        ..Default::default()
    };
    let targets = [d.target];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(d.label),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(d.label),
                bind_group_layouts: d.groups,
                immediate_size: 0,
            }),
        ),
        vertex: wgpu::VertexState {
            module: d.module,
            entry_point: Some(d.vs),
            compilation_options: options.clone(),
            buffers: d.buffers,
        },
        fragment: d.fs.map(|fs| wgpu::FragmentState {
            module: d.module,
            entry_point: Some(fs),
            compilation_options: options,
            targets: &targets,
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: d.cull,
            ..Default::default()
        },
        depth_stencil: d.depth,
        multisample: wgpu::MultisampleState {
            count: d.samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn depth_state(
    write: bool,
    compare: wgpu::CompareFunction,
    bias: wgpu::DepthBiasState,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias,
    }
}

fn module<'a>(modules: &'a [wgpu::ShaderModule], name: &str) -> &'a wgpu::ShaderModule {
    &modules[SHADERS
        .iter()
        .position(|(n, _)| *n == name)
        .expect("shader name")]
}

impl SampledPipelines {
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        modules: &[wgpu::ShaderModule],
        samples: u32,
    ) -> Self {
        let scene = module(modules, "scene");
        let buffers = vertex_layouts(false);
        let depth_buffers = vertex_layouts(true);
        let depth = pipeline(
            device,
            PipelineDesc {
                label: "depth pre-pass",
                module: scene,
                vs: "vs_depth",
                fs: None,
                groups: &[Some(&l.globals)],
                buffers: &depth_buffers,
                target: None,
                depth: Some(depth_state(
                    true,
                    wgpu::CompareFunction::Less,
                    Default::default(),
                )),
                cull: Some(wgpu::Face::Back),
                samples,
                constants: &[],
            },
        );
        let forward = pipeline(
            device,
            PipelineDesc {
                label: "forward",
                module: scene,
                vs: "vs_main",
                fs: Some("fs_main"),
                groups: &[
                    Some(&l.globals),
                    Some(&l.material),
                    Some(&l.draw),
                    Some(&l.forward),
                ],
                buffers: &buffers,
                target: Some(HDR.into()),
                depth: Some(depth_state(
                    false,
                    wgpu::CompareFunction::LessEqual,
                    Default::default(),
                )),
                cull: Some(wgpu::Face::Back),
                samples,
                constants: &[],
            },
        );
        let sky = pipeline(
            device,
            PipelineDesc {
                label: "sky",
                module: module(modules, "sky"),
                vs: "vs_sky",
                fs: Some("fs_sky"),
                groups: &[Some(&l.globals)],
                buffers: &[],
                target: Some(HDR.into()),
                depth: Some(depth_state(
                    false,
                    wgpu::CompareFunction::LessEqual,
                    Default::default(),
                )),
                cull: None,
                samples,
                constants: &[],
            },
        );
        let multisampled = samples > 1;
        let resolve = pipeline(
            device,
            PipelineDesc {
                label: "depth resolve",
                module: module(modules, "depth"),
                vs: "vs_fullscreen",
                fs: Some(if multisampled {
                    "fs_resolve_ms"
                } else {
                    "fs_resolve_1x"
                }),
                groups: &[
                    Some(&l.globals),
                    Some(if multisampled {
                        &l.depth_ms
                    } else {
                        &l.depth_1x
                    }),
                ],
                buffers: &[],
                target: Some(LINEAR_DEPTH.into()),
                depth: None,
                cull: None,
                samples: 1,
                constants: &[],
            },
        );
        Self {
            depth,
            forward,
            sky,
            resolve,
        }
    }
}

impl Pipelines {
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        modules: &[wgpu::ShaderModule],
        format: wgpu::TextureFormat,
    ) -> Self {
        let buffers = vertex_layouts(true);
        let fullscreen =
            |label, name, groups: &[Option<&wgpu::BindGroupLayout>], target, constants| {
                pipeline(
                    device,
                    PipelineDesc {
                        label,
                        module: module(modules, name),
                        vs: "vs_fullscreen",
                        fs: Some("fs_main"),
                        groups,
                        buffers: &[],
                        target: Some(target),
                        depth: None,
                        cull: None,
                        samples: 1,
                        constants,
                    },
                )
            };
        let segment = wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Unorm8x4, 2 => Float32, 3 => Float32x3, 4 => Unorm8x4, 5 => Float32
            ],
        };
        Self {
            shadow: pipeline(
                device,
                PipelineDesc {
                    label: "shadow",
                    module: module(modules, "scene"),
                    vs: "vs_shadow",
                    fs: None,
                    groups: &[Some(&l.globals), None, Some(&l.cascade)],
                    buffers: &buffers,
                    target: None,
                    // Slope-scaled bias against acne on surfaces at grazing
                    // sun angles; the normal offset in scene.wgsl does the rest.
                    depth: Some(depth_state(
                        true,
                        wgpu::CompareFunction::Less,
                        wgpu::DepthBiasState {
                            constant: 1,
                            slope_scale: 1.5,
                            clamp: 0.0,
                        },
                    )),
                    // Both sides: CAD shells are not always closed.
                    cull: None,
                    samples: 1,
                    constants: &[],
                },
            ),
            ssao: fullscreen(
                "ssao",
                "ssao",
                &[Some(&l.globals), Some(&l.texture)],
                OCCLUSION.into(),
                &[],
            ),
            blur_h: fullscreen(
                "blur h",
                "blur",
                &[Some(&l.globals), Some(&l.blur)],
                OCCLUSION.into(),
                &[("HORIZONTAL", 1.0)],
            ),
            blur_v: fullscreen(
                "blur v",
                "blur",
                &[Some(&l.globals), Some(&l.blur)],
                OCCLUSION.into(),
                &[("HORIZONTAL", 0.0)],
            ),
            tonemap: fullscreen(
                "tonemap",
                "tonemap",
                &[Some(&l.texture)],
                format.into(),
                &[],
            ),
            overlay: pipeline(
                device,
                PipelineDesc {
                    label: "overlay",
                    module: module(modules, "overlay"),
                    vs: "vs_main",
                    fs: Some("fs_main"),
                    groups: &[Some(&l.globals), Some(&l.texture)],
                    buffers: &[Some(segment)],
                    target: Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                },
            ),
        }
    }
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        format: wgpu::TextureFormat,
        quality: Quality,
        (width, height): (u32, u32),
        shadow_sampler: &wgpu::Sampler,
    ) -> Self {
        let samples = quality.samples();
        let texture_sized = |label, format, samples, usage, (width, height): (u32, u32)| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let texture = |label, format, samples, usage| {
            texture_sized(label, format, samples, usage, (width, height))
        };
        let view = |t: wgpu::Texture| t.create_view(&Default::default());
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = attach | wgpu::TextureUsages::TEXTURE_BINDING;
        let depth = view(texture("depth", DEPTH, samples, sampled));
        let hdr_msaa = (samples > 1).then(|| view(texture("hdr msaa", HDR, samples, attach)));
        let hdr = view(texture("hdr", HDR, 1, sampled));
        let linear_depth = view(texture("linear depth", LINEAR_DEPTH, 1, sampled));
        // Half resolution: AO and contact shadows are low-frequency.
        let half = (width.div_ceil(2), height.div_ceil(2));
        let occlusion =
            [0, 1].map(|_| view(texture_sized("occlusion", OCCLUSION, 1, sampled, half)));
        let srgb = format.add_srgb_suffix();
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: sampled | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[srgb],
        });
        let output_view = output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(srgb),
            ..Default::default()
        });
        let display_view = output.create_view(&Default::default());

        let res = quality.shadow_resolution;
        let shadow = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: res,
                height: res,
                depth_or_array_layers: quality.cascades,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: sampled,
            view_formats: &[],
        });
        let shadow_layers = (0..quality.cascades)
            .map(|i| {
                shadow.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("shadow layer"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_array = shadow.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shadow array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        let bind = |label, layout, entries: &[(u32, wgpu::BindingResource)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: resource.clone(),
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let tv = wgpu::BindingResource::TextureView;
        let resolve_bind = if samples > 1 {
            bind("resolve", &l.depth_ms, &[(0, tv(&depth))])
        } else {
            bind("resolve", &l.depth_1x, &[(1, tv(&depth))])
        };
        Targets {
            ssao_bind: bind("ssao", &l.texture, &[(0, tv(&linear_depth))]),
            blur_bind: [
                bind(
                    "blur h",
                    &l.blur,
                    &[(0, tv(&occlusion[0])), (1, tv(&linear_depth))],
                ),
                bind(
                    "blur v",
                    &l.blur,
                    &[(0, tv(&occlusion[1])), (1, tv(&linear_depth))],
                ),
            ],
            tonemap_bind: bind("tonemap", &l.texture, &[(0, tv(&hdr))]),
            overlay_bind: bind("overlay", &l.texture, &[(0, tv(&linear_depth))]),
            forward_bind: bind(
                "forward",
                &l.forward,
                &[
                    (0, tv(&shadow_array)),
                    (1, wgpu::BindingResource::Sampler(shadow_sampler)),
                    (2, tv(&occlusion[0])),
                    (3, tv(&linear_depth)),
                ],
            ),
            resolve_bind,
            depth,
            hdr_msaa,
            hdr,
            linear_depth,
            occlusion,
            output,
            output_view,
            display_view,
            shadow_layers,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every WGSL module parses and validates with naga, without a GPU.
    #[test]
    fn shaders_validate() {
        for (name, parts) in SHADERS {
            let source = parts.concat();
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(&source)));
        }
    }

    #[test]
    fn culling_keeps_visible_boxes() {
        let vp = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0)
            * Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let b = |c: Vec3| (c - Vec3::ONE, c + Vec3::ONE);
        assert!(!outside(vp, b(Vec3::new(0.0, 0.0, -10.0)), true));
        assert!(outside(vp, b(Vec3::new(0.0, 0.0, 10.0)), true)); // behind
        assert!(outside(vp, b(Vec3::new(50.0, 0.0, -10.0)), true)); // right
        assert!(!outside(vp, b(Vec3::new(0.0, 0.0, 0.5)), true)); // around the eye
    }
}
