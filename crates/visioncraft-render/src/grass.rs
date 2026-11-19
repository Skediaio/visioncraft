//! GPU grass over lawns and meadows, after "Procedural Grass in Ghost of Tsushima" (E. Wohllaib,
//! GDC 2021): the app gives a grass mask once per design change (`GrassField`: per cell of a grid
//! seen from above the density, blade height, flowers and lawn material, and the ground's
//! height); every frame a compute pass (grass_compute.wgsl) fills three instance lists round the
//! eye (near blades, far blades with fewer segments, flowers) and counts them into indirect draw
//! arguments, which the depth pre-pass and the forward pass draw (grass.wgsl). Nothing is built
//! on the CPU per frame but a uniform of a few hundred bytes.
//!
//! LOD: full density while a mown lawn's blade is at least `min_px` pixels wide, then a share
//! falling as 1 / distance with the blades widening as the distance, so the ground stays as
//! covered and no blade is thinner than a pixel or two (thin blades cost most: several shade
//! one pixel); the blades past the fade radius (further for taller grass) sink into the ground,
//! where the lawn's own texture takes over. Blades receive the sun's shadow but cast none (a
//! shadow map texel is wider than a blade); the screen-space occlusion and contact shadows see
//! them in the depth pre-pass. They are lit by a lean leaf model (grass.wgsl `grass_light`).
//!
//! Cost per quality (`Quality::grass`, `settings`): MEDIUM a short reach, under half of HIGH's
//! blades and none thinner than 3 pixels; ULTRA further and denser. LOW has none (the app draws
//! its lawn tufts instead). Measured on an Iris Xe at 1280x720 (the garden example, lawn seen
//! from eye height): about 0.5 ms at MEDIUM, 5 to 10 ms at HIGH and 20 ms at ULTRA, the
//! compute pass 0.3 to 2.5 ms of it.
//!
//! WebGPU: compute, storage buffers written in compute and read as vertex buffers, and indirect
//! draws are all core WebGPU (no optional feature: the lists start at instance 0 and each list
//! has its own vertex buffer offset, so no `INDIRECT_FIRST_INSTANCE`).

use crate::{Layouts, PipelineDesc, depth_state, pipeline_with};
use glam::{Mat4, Vec3, Vec4};

pub(crate) const COMMON: &str = include_str!("grass_common.wgsl");
const COMPUTE: &str = include_str!("grass_compute.wgsl");
const DRAW: &str = include_str!("grass.wgsl");
/// Must match grass_common.wgsl `GRASS_TILE`.
const TILE_M: f32 = 2.0;
/// Must match grass_common.wgsl `GrassFrame`.
const FRAME_SIZE: u64 = 592;
const BLADE_SIZE: u64 = 32;
/// The narrowest blade (m), of a mown lawn: full density as long as it is `min_px` wide.
const BLADE_WIDTH_M: f32 = 0.0045;
/// Lawn materials with their own colour (grass_common.wgsl `kinds`).
pub const MAX_KINDS: usize = 8;

/// A lawn material's look for its blades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrassKind {
    /// Linear RGB of a blade (its middle; the foot is paler, the tip fresher).
    pub color: [f32; 3],
    /// Mowing stripes this wide (m; 0 none), whose blades lean along `stripes_dir` and against it
    /// in turn.
    pub stripes_m: f32,
    /// The mower's direction (world x, z; unit).
    pub stripes_dir: [f32; 2],
    /// The share of dead, straw-coloured blades (0..1).
    pub dry: f32,
}

/// Where grass grows: the core's grass mask (`visioncraft_core::grass`), row-major cells from
/// `origin`, x along a row.
#[derive(Clone, Copy, Debug)]
pub struct GrassField<'a> {
    /// World x, z of the first cell's corner.
    pub origin: [f32; 2],
    pub cell_m: f32,
    pub size: [u32; 2],
    /// Density (0..255), blade height (square-root coded up to 1.2 m), flowers (0..255) and the
    /// entry of `kinds`, per cell.
    pub params: &'a [[u8; 4]],
    /// The ground's world height per cell (m).
    pub heights: &'a [f32],
    pub kinds: &'a [GrassKind],
    /// The season's flower colours (linear RGB).
    pub flowers: [[f32; 3]; 4],
}

/// How far and how dense per `Quality::grass` level.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    /// Fade radius of a 0.2 m blade (m): shorter grass ends sooner, taller later.
    reach: f32,
    /// Blades per m² at full density.
    per_m2: f32,
    /// The narrowest a blade is drawn (pixels): further off the blades are fewer and wider, so
    /// none is thinner than this. Larger is cheaper (fewer, broader blades from a few metres).
    min_px: f32,
    /// Near list (more segments) out to here (m).
    near: f32,
    segments: [u32; 3],
    caps: [u32; 3],
}

fn settings(level: u32) -> Settings {
    match level {
        0 | 1 => Settings {
            reach: 16.0,
            per_m2: 900.0,
            min_px: 3.0,
            near: 6.0,
            segments: [4, 2, 3],
            caps: [60_000, 120_000, 8_000],
        },
        2 => Settings {
            reach: 26.0,
            per_m2: 2000.0,
            min_px: 2.0,
            near: 9.0,
            segments: [5, 3, 3],
            caps: [120_000, 240_000, 16_000],
        },
        _ => Settings {
            reach: 34.0,
            per_m2: 3000.0,
            min_px: 1.5,
            near: 13.0,
            segments: [6, 3, 4],
            caps: [320_000, 600_000, 32_000],
        },
    }
}

/// The grid of tiles reaches this far: the fade radius of the tallest grass.
fn grid_reach(s: &Settings) -> f32 {
    s.reach * 1.6
}

fn vertex_counts(s: &Settings) -> [u32; 3] {
    [
        2 * (s.segments[0] + 1),
        2 * (s.segments[1] + 1),
        2 * (s.segments[2] + 1) + 8,
    ]
}

struct Field {
    origin: [f32; 2],
    cell_m: f32,
    size: [u32; 2],
    y_range: [f32; 2],
    kinds: Vec<GrassKind>,
    flowers: [[f32; 3]; 4],
    compute_bind: Option<wgpu::BindGroup>,
    params: wgpu::TextureView,
    heights: wgpu::TextureView,
}

struct Lists {
    level: u32,
    blades: wgpu::Buffer,
    args: wgpu::Buffer,
}

struct DrawPipelines {
    /// (samples, fast shading) they were made for.
    key: (u32, bool),
    depth: [wgpu::RenderPipeline; 3],
    forward: [wgpu::RenderPipeline; 3],
}

struct ComputePipelines {
    reset: wgpu::ComputePipeline,
    generate: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
}

pub(crate) struct GrassGpu {
    frame: wgpu::Buffer,
    compute_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    draw_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    field: Option<Field>,
    lists: Option<Lists>,
    compute: Option<ComputePipelines>,
    draw: Option<DrawPipelines>,
    /// The lists were filled this frame (`generate`).
    pub drawn: bool,
}

/// What one frame's generation needs from the renderer.
pub(crate) struct Frame<'a> {
    pub view_proj: Mat4,
    pub eye: Vec3,
    /// Pixels a metre spans at a metre's distance (the render size's height over the field of
    /// view's).
    pub pixels_per_m: f32,
    pub level: u32,
    pub samples: u32,
    pub fast_shading: bool,
    pub layouts: &'a Layouts,
    pub timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>,
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// The frustum's planes of `m` (Gribb & Hartmann, "Fast Extraction of Viewing Frustum Planes
/// from the World-View-Projection Matrix", 2001), for a 0..1 depth range, inward and normalised.
fn frustum_planes(m: Mat4) -> [Vec4; 6] {
    let r = [m.row(0), m.row(1), m.row(2), m.row(3)];
    [
        r[3] + r[0],
        r[3] - r[0],
        r[3] + r[1],
        r[3] - r[1],
        r[2],
        r[3] - r[2],
    ]
    .map(|p| {
        let l = p.truncate().length();
        if l > 0.0 { p / l } else { p }
    })
}

/// The module that draws the blades: the scene module's parts (for `light`) and the grass's own.
pub(crate) fn draw_source(fast_shading: bool) -> String {
    let scene = crate::SHADERS
        .iter()
        .find(|(n, _)| *n == "scene")
        .expect("the scene module")
        .1;
    let mut parts = scene.to_vec();
    parts.extend([COMMON, DRAW]);
    crate::shader_source(&parts, fast_shading)
}

fn compute_source() -> String {
    [COMMON, COMPUTE].concat()
}

impl GrassGpu {
    pub fn new(device: &wgpu::Device) -> Self {
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass frame"),
            size: FRAME_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform = |visibility| wgpu::BindGroupLayoutEntry {
            binding: 40,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(FRAME_SIZE),
            },
            count: None,
        };
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass compute"),
            entries: &[
                uniform(wgpu::ShaderStages::COMPUTE),
                texture_entry(41, true),
                texture_entry(42, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 43,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                storage_entry(44),
                storage_entry(45),
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass draw"),
            entries: &[uniform(wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let draw_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grass draw"),
            layout: &draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 40,
                resource: frame.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("grass mask"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            frame,
            compute_layout,
            draw_layout,
            draw_bind,
            sampler,
            field: None,
            lists: None,
            compute: None,
            draw: None,
            drawn: false,
        }
    }

    pub fn has_field(&self) -> bool {
        self.field.is_some()
    }

    /// A new mask, or none (no grass drawn).
    pub fn set_field(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        f: Option<&GrassField>,
    ) {
        let Some(f) = f.filter(|f| {
            let n = f.size[0] as usize * f.size[1] as usize;
            n > 0 && f.params.len() == n && f.heights.len() == n && f.cell_m > 0.0
        }) else {
            self.field = None;
            return;
        };
        let size = wgpu::Extent3d {
            width: f.size[0],
            height: f.size[1],
            depth_or_array_layers: 1,
        };
        let texture = |label, format: wgpu::TextureFormat, bytes: &[u8], texel: u32| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                t.as_image_copy(),
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(f.size[0] * texel),
                    rows_per_image: Some(f.size[1]),
                },
                size,
            );
            t.create_view(&Default::default())
        };
        let params: Vec<u8> = f.params.iter().flatten().copied().collect();
        let heights: Vec<u8> = f.heights.iter().flat_map(|h| h.to_le_bytes()).collect();
        let (lo, hi) = f
            .heights
            .iter()
            .zip(f.params)
            .filter(|(_, p)| p[0] > 0)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (h, _)| {
                (lo.min(*h), hi.max(*h))
            });
        let y_range = if lo.is_finite() { [lo, hi] } else { [0.0, 0.0] };
        self.field = Some(Field {
            origin: f.origin,
            cell_m: f.cell_m,
            size: f.size,
            y_range,
            kinds: f.kinds.iter().take(MAX_KINDS).copied().collect(),
            flowers: f.flowers,
            compute_bind: None,
            params: texture("grass params", wgpu::TextureFormat::Rgba8Unorm, &params, 4),
            heights: texture("grass ground", wgpu::TextureFormat::R32Float, &heights, 4),
        });
    }

    fn frame_bytes(&self, f: &Field, s: &Settings, frame: &Frame) -> Vec<u8> {
        let mut b = crate::Bytes::default();
        b.floats(&frame.view_proj.to_cols_array());
        for p in frustum_planes(frame.view_proj) {
            b.floats(&p.to_array());
        }
        let reach = grid_reach(s);
        b.floats(&[frame.eye.x, frame.eye.y, frame.eye.z, reach]);
        b.floats(&[f.origin[0], f.origin[1], 1.0 / f.cell_m, f.cell_m]);
        b.floats(&[
            f.size[0] as f32,
            f.size[1] as f32,
            f.y_range[0],
            f.y_range[1],
        ]);
        // Full density while the narrowest blade is `min_px` wide, then a share falling as
        // 1 / distance.
        let full = (BLADE_WIDTH_M * frame.pixels_per_m / s.min_px).clamp(1.0, 12.0);
        b.floats(&[s.per_m2, full, s.near, s.reach]);
        let first = |e: f32| ((e - reach) / TILE_M).floor() as i32;
        let n = (2.0 * reach / TILE_M).ceil() as i32 + 1;
        let tiles = [first(frame.eye.x), first(frame.eye.z), n, 0];
        b.0.extend(tiles.iter().flat_map(|t| t.to_le_bytes()));
        b.0.extend(
            [s.caps[0], s.caps[1], s.caps[2], 0]
                .iter()
                .flat_map(|t| t.to_le_bytes()),
        );
        let v = vertex_counts(s);
        b.0.extend([v[0], v[1], v[2], 0].iter().flat_map(|t| t.to_le_bytes()));
        for k in 0..MAX_KINDS {
            let kind = f
                .kinds
                .get(k)
                .or(f.kinds.first())
                .copied()
                .unwrap_or(GrassKind {
                    color: [0.1, 0.2, 0.05],
                    stripes_m: 0.0,
                    stripes_dir: [1.0, 0.0],
                    dry: 0.0,
                });
            b.floats(&[kind.color[0], kind.color[1], kind.color[2], kind.stripes_m]);
            b.floats(&[kind.stripes_dir[0], kind.stripes_dir[1], kind.dry, 0.0]);
        }
        for c in f.flowers {
            b.floats(&[c[0], c[1], c[2], 0.0]);
        }
        debug_assert_eq!(b.0.len() as u64, FRAME_SIZE);
        b.0
    }

    /// Fills this frame's lists (a compute pass) when there is a field and `frame.level` > 0;
    /// `drawn` says whether the passes should draw them.
    pub fn generate(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame,
    ) {
        self.drawn = false;
        if frame.level == 0 || self.field.is_none() {
            return;
        }
        let s = settings(frame.level);
        if self.lists.as_ref().is_none_or(|l| l.level != frame.level) {
            let total: u64 = s.caps.iter().map(|&c| u64::from(c)).sum();
            self.lists = Some(Lists {
                level: frame.level,
                blades: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("grass blades"),
                    size: total * BLADE_SIZE,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
                    mapped_at_creation: false,
                }),
                args: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("grass draws"),
                    size: 3 * 16,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
                    mapped_at_creation: false,
                }),
            });
            if let Some(f) = &mut self.field {
                f.compute_bind = None;
            }
        }
        let key = (frame.samples, frame.fast_shading);
        if self.draw.as_ref().is_none_or(|d| d.key != key) {
            self.draw = Some(self.draw_pipelines(device, frame.layouts, key, &s));
        }
        if self.compute.is_none() {
            self.compute = Some(self.compute_pipelines(device));
        }
        let (Some(lists), Some(f), Some(cp)) = (&self.lists, &mut self.field, &self.compute) else {
            return;
        };
        if f.compute_bind.is_none() {
            f.compute_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grass compute"),
                layout: &self.compute_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 40,
                        resource: self.frame.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 41,
                        resource: wgpu::BindingResource::TextureView(&f.params),
                    },
                    wgpu::BindGroupEntry {
                        binding: 42,
                        resource: wgpu::BindingResource::TextureView(&f.heights),
                    },
                    wgpu::BindGroupEntry {
                        binding: 43,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 44,
                        resource: lists.blades.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 45,
                        resource: lists.args.as_entire_binding(),
                    },
                ],
            }));
        }
        let f = self.field.as_ref().expect("checked above");
        queue.write_buffer(&self.frame, 0, &self.frame_bytes(f, &s, &frame));
        let n = (2.0 * grid_reach(&s) / TILE_M).ceil() as u32 + 1;
        let timestamp_writes =
            frame
                .timestamps
                .as_ref()
                .map(|t| wgpu::ComputePassTimestampWrites {
                    query_set: t.query_set,
                    beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                    end_of_pass_write_index: t.end_of_pass_write_index,
                });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("grass"),
            timestamp_writes,
        });
        pass.set_bind_group(1, f.compute_bind.as_ref(), &[]);
        pass.set_pipeline(&cp.reset);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&cp.generate);
        pass.dispatch_workgroups(n, n, 1);
        pass.set_pipeline(&cp.finish);
        pass.dispatch_workgroups(1, 1, 1);
        self.drawn = true;
    }

    fn compute_pipelines(&self, device: &wgpu::Device) -> ComputePipelines {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grass compute"),
            source: wgpu::ShaderSource::Wgsl(compute_source().into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass compute"),
            bind_group_layouts: &[None, Some(&self.compute_layout)],
            immediate_size: 0,
        });
        let make = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        ComputePipelines {
            reset: make("cs_grass_reset"),
            generate: make("cs_grass"),
            finish: make("cs_grass_finish"),
        }
    }

    fn draw_pipelines(
        &self,
        device: &wgpu::Device,
        l: &Layouts,
        (samples, fast_shading): (u32, bool),
        s: &Settings,
    ) -> DrawPipelines {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grass"),
            source: wgpu::ShaderSource::Wgsl(
                crate::for_device(draw_source(fast_shading), device).into(),
            ),
        });
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: BLADE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32x4],
        })];
        let depth_groups = [Some(&l.globals), Some(&self.draw_layout)];
        let forward_groups = [
            Some(&l.globals),
            Some(&self.draw_layout),
            Some(&l.draw),
            Some(&l.forward),
        ];
        let make = |list: usize, forward: bool| {
            let constants = [
                ("GRASS_SEGMENTS", f64::from(s.segments[list])),
                ("GRASS_FLOWER", if list == 2 { 1.0 } else { 0.0 }),
            ];
            pipeline_with(
                device,
                PipelineDesc {
                    label: if forward { "grass" } else { "grass depth" },
                    module: &module,
                    vs: if forward {
                        "vs_grass"
                    } else {
                        "vs_grass_depth"
                    },
                    fs: forward.then_some("fs_grass"),
                    groups: if forward {
                        &forward_groups
                    } else {
                        &depth_groups
                    },
                    buffers: &buffers,
                    target: forward.then_some(crate::HDR.into()),
                    depth: Some(if forward {
                        depth_state(false, wgpu::CompareFunction::LessEqual, Default::default())
                    } else {
                        depth_state(true, wgpu::CompareFunction::Less, Default::default())
                    }),
                    cull: None,
                    samples,
                    constants: &constants,
                    front_cw: false,
                },
                wgpu::PrimitiveTopology::TriangleStrip,
            )
        };
        DrawPipelines {
            key: (samples, fast_shading),
            depth: std::array::from_fn(|i| make(i, false)),
            forward: std::array::from_fn(|i| make(i, true)),
        }
    }

    fn each_list(&self, pass: &mut wgpu::RenderPass, pipelines: &[wgpu::RenderPipeline; 3]) {
        let Some(lists) = &self.lists else { return };
        let s = settings(lists.level);
        let mut offset = 0u64;
        for (i, p) in pipelines.iter().enumerate() {
            pass.set_pipeline(p);
            pass.set_vertex_buffer(0, lists.blades.slice(offset * BLADE_SIZE..));
            pass.draw_indirect(&lists.args, i as u64 * 16);
            offset += u64::from(s.caps[i]);
        }
    }

    /// The blades' depth, in the depth pre-pass (globals bound in group 0).
    pub fn draw_depth(&self, pass: &mut wgpu::RenderPass) {
        let (true, Some(d)) = (self.drawn, &self.draw) else {
            return;
        };
        pass.set_bind_group(1, &self.draw_bind, &[]);
        self.each_list(pass, &d.depth);
    }

    /// The blades, lit, in the forward pass (globals and the forward group bound); `draw` and
    /// `slot` are a draw slot for group 2.
    pub fn draw(&self, pass: &mut wgpu::RenderPass, draw: &wgpu::BindGroup, slot: u32) {
        let (true, Some(d)) = (self.drawn, &self.draw) else {
            return;
        };
        pass.set_bind_group(1, &self.draw_bind, &[]);
        pass.set_bind_group(2, draw, &[slot]);
        self.each_list(pass, &d.forward);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(source: &str) {
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| {
            panic!("{}", e.emit_to_string(source));
        });
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    }

    #[test]
    fn grass_shaders_validate() {
        validate(&compute_source());
        validate(&draw_source(false));
        validate(&draw_source(true));
    }

    #[test]
    fn the_frame_uniform_matches_the_shader() {
        // GrassFrame: a matrix, six planes, seven vec4, 16 kinds' vec4 and 4 flowers' vec4.
        assert_eq!(FRAME_SIZE, 64 + 6 * 16 + 7 * 16 + 16 * 16 + 4 * 16);
        assert!(COMMON.contains("const GRASS_TILE: f32 = 2.0;"));
        assert_eq!(TILE_M, 2.0);
    }

    #[test]
    fn the_frustum_planes_hold_the_points_in_view() {
        let proj = Mat4::perspective_rh(1.0, 1.5, 0.1, 500.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 2.0, 10.0), Vec3::ZERO, Vec3::Y);
        let planes = frustum_planes(proj * view);
        let inside = |p: Vec3| planes.iter().all(|pl| pl.truncate().dot(p) + pl.w >= 0.0);
        assert!(inside(Vec3::ZERO));
        assert!(!inside(Vec3::new(0.0, 2.0, 20.0)), "behind the eye");
        assert!(!inside(Vec3::new(100.0, 0.0, 0.0)), "far to the side");
    }

    #[test]
    fn higher_levels_reach_further_with_more_blades() {
        let (m, h, u) = (settings(1), settings(2), settings(3));
        assert!(m.reach < h.reach && h.reach < u.reach);
        assert!(m.per_m2 < h.per_m2 && h.per_m2 < u.per_m2);
        assert!(m.min_px > h.min_px && h.min_px > u.min_px);
        for s in [m, h, u] {
            assert!(s.min_px > 0.0 && s.near < s.reach);
            // The flowers' list has the head's four rows on top of its stem.
            assert_eq!(vertex_counts(&s)[2], 2 * (s.segments[2] + 1) + 8);
        }
    }
}
