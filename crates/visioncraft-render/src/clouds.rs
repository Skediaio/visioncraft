//! Volumetric clouds (clouds.wgsl): a raymarch through a layer of cloud at a quarter, a third
//! or half of the render size, upsampled over frames to twice that from a quarter or a third,
//! which the sky pass reads (sky.wgsl `background`).
//!
//! Everything is made the first time the clouds are drawn, never at start-up or per frame:
//! three noise textures filled once by compute shaders (cloud_noise.wgsl: a 128^3 Perlin-Worley
//! and Worley base and a 32^3 Worley detail, each with its smaller levels, a 1024^2 weather
//! map and a 128^2 curl noise), the march and resolve pipelines, the march image and two history
//! images at twice its size (at its size from half the render size). Each frame marches one ray
//! per march texel, through one texel of each 2x2 block of the history in turn, and the resolve
//! blends those samples into the other history image (reprojected by direction, clamped to the new samples round each texel). A
//! frame that cannot use the history (the first, a new size, a cut, a jump of the sun or the
//! weather) fills the history from the march alone, four jitters a march texel (where the
//! history keeps the march's size: marches every history texel), and a picture (one frame, `Draw::picture`)
//! eight jitters of each at fixed offsets, so the frames of a film keep one pattern and do not
//! flicker. Without volumetric clouds the sky reads a 1x1 image and nothing else is made.

use glam::{Mat4, Vec3, Vec4};

use crate::{COMMON, Layouts, PipelineDesc, color_pass, pipeline};

/// The noise textures' sizes (texels per side).
const BASE: u32 = 128;
const DETAIL: u32 = 32;
const WEATHER: u32 = 1024;
const CURL: u32 = 128;
const HISTORY: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const NOISE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Weight of a texel's own new sample against the history (a texel has one every fourth
/// frame), and of the new samples interpolated to it in the frames between; and of the new
/// sample where the history keeps the march's size (one every frame).
const BLEND_OWN: f32 = 0.25;
const BLEND_NEAR: f32 = 0.06;
const BLEND_SAME: f32 = 0.12;
/// The march divisors (`Draw::div`) the history doubles; at half the render size it is sharp
/// enough as it is, and the history at the full size would cost more than the march saves.
const UPSAMPLE_DIV: u32 = 3;
/// The history is clamped to the new samples' mean round a texel plus or minus this many of
/// their standard deviations.
const CLAMP_SIGMAS: f32 = 1.5;
/// The order a 2x2 block of the history is sampled in (texels): each frame the opposite corner
/// of the last, so two frames already cover a diagonal.
const BLOCK: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)];
/// Jitters per history texel for a picture.
const PICTURE_SUBSAMPLES: f32 = 8.0;
/// Jitters per march texel when a history at twice the march's size starts over (`draw`).
const FILL_SUBSAMPLES: f32 = 4.0;
/// The uniform block's size (bytes; clouds.wgsl `CloudFrame`).
const UNIFORM_SIZE: u64 = 112;
/// The sun may move this far between frames (cosine, about a degree) before the history is
/// told its light changed: the clouds' light moves with it.
const SUN_COS: f32 = 0.99985;
/// A sun that turned further than this in one frame (cosine, about 5 degrees: a jump of the
/// time of day, not a drag) starts the history over. Below it the history stays: the sun does
/// not change where a cloud texel reprojects (that is the view's), only how it is lit, and a
/// temporal cloud renderer follows a lighting change by weighting the new frame more, not by
/// dropping the history (Schneider, "The Real-Time Volumetric Cloudscapes of Horizon Zero
/// Dawn", SIGGRAPH 2015; Hillaire, "Physically Based Sky, Atmosphere and Cloud Rendering in
/// Frostbite", 2016).
const SUN_JUMP_COS: f32 = 0.996;
/// The weight of the new frame while the sun moves more than `SUN_COS` per frame (and less than
/// a jump). The old light's share left after n frames is `(1 - w)^n`: at 0.45 it is 0.55^4 =
/// 0.0915 after four frames, below 10 %. It applies to every texel, the own sample of a block
/// and the interpolated ones alike (clouds.wgsl `fs_resolve` mixes them with `params[1]` and
/// `params[10]`), so with the checkerboard march (scale 2), where a texel is marched one frame
/// in four, all of them still converge at this rate.
const BLEND_RELIGHT: f32 = 0.45;
/// The view may turn this far between frames (cosine) before the history is started over.
const TURN_COS: f32 = 0.97;

/// The weather map's period in metres (common.wgsl `CLOUD_PERIOD`): the drift wraps here.
pub(crate) const CLOUD_PERIOD: f64 = 40960.0;

const CLOUDS_WGSL: &str = include_str!("clouds.wgsl");
const NOISE_WGSL: &str = include_str!("cloud_noise.wgsl");

/// Whether the history starts over, and the weight of the new frame (own samples) in it:
/// `sun_dot` and `view_dot` are the cosines of the sun's and the view axis' turn since the last
/// frame, `forced` a reason to start over that is not a turn (a weather jump, a picture, a
/// flash, a new size), `normal` the weight when nothing changed. A view turn or a sun jump
/// starts over; a sun that moved more than `SUN_COS` keeps the history and raises the weight.
fn history_blend(sun_dot: f32, view_dot: f32, forced: bool, normal: f32) -> (bool, f32) {
    if forced || view_dot < TURN_COS || sun_dot < SUN_JUMP_COS {
        (true, normal)
    } else if sun_dot < SUN_COS {
        (false, BLEND_RELIGHT.max(normal))
    } else {
        (false, normal)
    }
}

/// What one frame's march needs.
pub(crate) struct Draw<'a> {
    pub layouts: &'a Layouts,
    pub globals: &'a wgpu::BindGroup,
    /// The render size (texels).
    pub size: (u32, u32),
    /// The march image's divisor of it (2 half, 4 quarter); the history is twice the march.
    pub div: u32,
    pub steps: u32,
    pub light_steps: u32,
    pub view: Mat4,
    pub proj: Mat4,
    /// The clouds changed too much for the history (a weather jump).
    pub jumped: bool,
    /// A picture of one frame: start over, with more jitters at fixed offsets.
    pub picture: bool,
    /// Towards the sun.
    pub sun: Vec3,
    /// A lightning flash is lit (lightning.rs): the clouds' light changes from one frame to
    /// the next, faster than the history follows.
    pub flash: bool,
}

pub(crate) struct CloudVolume {
    sky_off: wgpu::BindGroup,
    gpu: Option<Gpu>,
    frame: u32,
    /// The last march's view-projection without translation, its view axis and the sun.
    prev: Option<(Mat4, Vec3, Vec3)>,
    /// The history image the sky reads (`None`: the 1x1 one).
    showing: Option<usize>,
    /// The last frame had a lightning flash.
    flashed: bool,
}

struct Gpu {
    clamp: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// The uniform, the noise and their sampler (group 1 of both pipelines).
    march_bind: wgpu::BindGroup,
    resolve_layout: wgpu::BindGroupLayout,
    march: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
    history: Option<History>,
}

struct History {
    /// The march image's size; the history's is `scale` times it.
    size: (u32, u32),
    scale: u32,
    march: wgpu::TextureView,
    views: [wgpu::TextureView; 2],
    /// Resolving into image i, reading the other.
    resolve: [wgpu::BindGroup; 2],
    /// The sky reading image i.
    sky: [wgpu::BindGroup; 2],
    current: usize,
}

fn sampler(device: &wgpu::Device, label: &str, mode: wgpu::AddressMode) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: mode,
        address_mode_v: mode,
        address_mode_w: mode,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

fn sky_bind(
    device: &wgpu::Device,
    layouts: &Layouts,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky clouds"),
        layout: &layouts.sky_clouds,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32, u32),
    dimension: wgpu::TextureDimension,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    mips: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: size.2,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension,
        format,
        usage,
        view_formats: &[],
    })
}

/// The f16 bits of 1.
const F16_ONE: u16 = 0x3c00;

impl CloudVolume {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, layouts: &Layouts) -> Self {
        // No clouds: nothing over the sky (transmittance 1).
        let off = texture(
            device,
            "no clouds",
            (1, 1, 1),
            wgpu::TextureDimension::D2,
            HISTORY,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            1,
        );
        let texel: Vec<u8> = [0u16, 0, 0, F16_ONE]
            .iter()
            .flat_map(|h| h.to_le_bytes())
            .collect();
        queue.write_texture(
            off.as_image_copy(),
            &texel,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: None,
            },
            wgpu::Extent3d::default(),
        );
        let view = off.create_view(&Default::default());
        let clamp = sampler(device, "clouds clamp", wgpu::AddressMode::ClampToEdge);
        Self {
            sky_off: sky_bind(device, layouts, &view, &clamp),
            gpu: None,
            frame: 0,
            prev: None,
            showing: None,
            flashed: false,
        }
    }

    /// The sky pass's group 2: the clouds marched this frame, or nothing.
    pub fn sky_bind(&self) -> &wgpu::BindGroup {
        match (&self.gpu, self.showing) {
            (
                Some(Gpu {
                    history: Some(h), ..
                }),
                Some(i),
            ) => &h.sky[i],
            _ => &self.sky_off,
        }
    }

    /// No volumetric clouds this frame: the sky reads nothing, the next march starts over.
    pub fn off(&mut self) {
        self.showing = None;
        self.prev = None;
    }

    /// Marches this frame's clouds and resolves them into the next history image (making what
    /// is missing first).
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        d: Draw,
        timestamp: Option<wgpu::RenderPassTimestampWrites>,
    ) {
        let gpu = self
            .gpu
            .get_or_insert_with(|| Gpu::new(device, d.layouts, encoder));
        let div = d.div.max(1);
        let size = (d.size.0.div_ceil(div).max(1), d.size.1.div_ceil(div).max(1));
        // A flash, and the frame after it, start over: its light comes and goes at once.
        let mut reset = d.jumped || d.picture || d.flash || self.flashed;
        self.flashed = d.flash;
        let scale = if div >= UPSAMPLE_DIV { 2 } else { 1 };
        if gpu
            .history
            .as_ref()
            .is_none_or(|h| h.size != size || h.scale != scale)
        {
            gpu.history = Some(History::new(device, gpu, d.layouts, size, scale));
            reset = true;
        }
        let mut rotation = d.view;
        rotation.w_axis = Vec4::W;
        let view_proj = d.proj * rotation;
        let axis = -Vec3::new(d.view.x_axis.z, d.view.y_axis.z, d.view.z_axis.z);
        let normal = if scale == 2 { BLEND_OWN } else { BLEND_SAME };
        let mut blend = normal;
        let prev = match self.prev {
            Some((m, a, s)) if self.showing.is_some() => {
                let (r, b) = history_blend(s.dot(d.sun), a.dot(axis), reset, normal);
                blend = b;
                reset |= r;
                if r { view_proj } else { m }
            }
            _ => {
                reset = true;
                view_proj
            }
        };
        // Starting over, a picture marches every texel of the history itself, with eight jitters
        // each, and so does a history at the march's size, with one. A history at twice the
        // march's size is filled from the march image instead, with four jitters a texel at the
        // centre of each 2x2 block: as many rays as one through every history texel, but no
        // single-jitter banding for the frames after to wear off (a dither of the history's
        // texels for a second or so after every restart).
        let fill = reset && !d.picture && scale == 2;
        let direct = reset && !fill;
        let (march_size, march_scale, offset) = if direct {
            ((size.0 * scale, size.1 * scale), 1, (0.0, 0.0))
        } else if fill {
            (size, 2, (0.5, 0.5))
        } else if scale == 2 {
            (size, 2, BLOCK[self.frame as usize % BLOCK.len()])
        } else {
            (size, 1, (0.0, 0.0))
        };
        let mut b = Vec::with_capacity(UNIFORM_SIZE as usize);
        for f in prev.to_cols_array() {
            b.extend_from_slice(&f.to_le_bytes());
        }
        let params = [
            // A picture's jitter does not turn with the frame.
            if d.picture {
                0.0
            } else {
                (self.frame % 4096) as f32
            },
            blend,
            if d.picture {
                PICTURE_SUBSAMPLES
            } else if fill {
                FILL_SUBSAMPLES
            } else {
                1.0
            },
            d.steps.max(1) as f32,
            d.light_steps as f32,
            march_size.0 as f32,
            march_size.1 as f32,
            march_scale as f32,
            offset.0,
            offset.1,
            // 1: no history, the new samples interpolated to every texel (clouds.wgsl); otherwise
            // the interpolated texels follow a relight at the same rate.
            if fill {
                1.0
            } else if blend > normal {
                blend
            } else {
                BLEND_NEAR
            },
            CLAMP_SIGMAS,
        ];
        for f in params {
            b.extend_from_slice(&f.to_le_bytes());
        }
        queue.write_buffer(&gpu.uniform, 0, &b);
        let Some(history) = gpu.history.as_mut() else {
            return;
        };
        let next = 1 - history.current;
        // One timing for both passes: from the march's start to the resolve's end.
        let (begin, end) = match timestamp {
            Some(t) if !direct => (
                Some(wgpu::RenderPassTimestampWrites {
                    query_set: t.query_set,
                    beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                    end_of_pass_write_index: None,
                }),
                Some(wgpu::RenderPassTimestampWrites {
                    query_set: t.query_set,
                    beginning_of_pass_write_index: None,
                    end_of_pass_write_index: t.end_of_pass_write_index,
                }),
            ),
            t => (t, None),
        };
        {
            let target = if direct {
                &history.views[next]
            } else {
                &history.march
            };
            let mut pass = color_pass(encoder, "clouds", target, None, None, begin);
            pass.set_pipeline(&gpu.march);
            pass.set_bind_group(0, d.globals, &[]);
            pass.set_bind_group(1, &gpu.march_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        if !direct {
            let mut pass = color_pass(
                encoder,
                "clouds resolve",
                &history.views[next],
                None,
                None,
                end,
            );
            pass.set_pipeline(&gpu.resolve);
            pass.set_bind_group(0, d.globals, &[]);
            pass.set_bind_group(1, &gpu.march_bind, &[]);
            pass.set_bind_group(2, &history.resolve[next], &[]);
            pass.draw(0..3, 0..1);
        }
        history.current = next;
        self.showing = Some(next);
        self.prev = Some((view_proj, axis, d.sun));
        self.frame = self.frame.wrapping_add(1);
    }
}

impl Gpu {
    fn new(device: &wgpu::Device, layouts: &Layouts, encoder: &mut wgpu::CommandEncoder) -> Gpu {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let d3 = wgpu::TextureDimension::D3;
        let base = texture(
            device,
            "cloud base noise",
            (BASE, BASE, BASE),
            d3,
            NOISE,
            usage,
            BASE.ilog2() + 1,
        );
        let detail = texture(
            device,
            "cloud detail noise",
            (DETAIL, DETAIL, DETAIL),
            d3,
            NOISE,
            usage,
            DETAIL.ilog2() + 1,
        );
        let flat = |label, side| {
            texture(
                device,
                label,
                (side, side, 1),
                wgpu::TextureDimension::D2,
                NOISE,
                usage,
                1,
            )
        };
        let weather = flat("cloud weather", WEATHER);
        let curl = flat("cloud curl", CURL);
        fill_noise(device, encoder, [&base, &detail], [&weather, &curl]);
        let [base, detail, weather, curl] =
            [&base, &detail, &weather, &curl].map(|t| t.create_view(&Default::default()));

        let fs = wgpu::ShaderStages::FRAGMENT;
        let tex = |binding, dim| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fs,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        };
        let smp = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fs,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        use wgpu::TextureViewDimension as D;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fs,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(1, D::D3),
                tex(2, D::D3),
                tex(3, D::D2),
                smp(4),
                tex(5, D::D2),
            ],
        });
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds resolve"),
            entries: &[tex(0, D::D2), tex(1, D::D2), smp(2)],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("clouds"),
            source: wgpu::ShaderSource::Wgsl(march_source().into()),
        });
        let make = |label, fs, groups: &[Option<&wgpu::BindGroupLayout>]| {
            pipeline(
                device,
                PipelineDesc {
                    label,
                    module: &module,
                    vs: "vs_clouds",
                    fs: Some(fs),
                    groups,
                    buffers: &[],
                    target: Some(HISTORY.into()),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                    front_cw: false,
                },
            )
        };
        let march = make(
            "clouds",
            "fs_clouds",
            &[Some(&layouts.globals), Some(&layout)],
        );
        let resolve = make(
            "clouds resolve",
            "fs_resolve",
            &[Some(&layouts.globals), Some(&layout), Some(&resolve_layout)],
        );
        let repeat = sampler(device, "clouds repeat", wgpu::AddressMode::Repeat);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clouds frame"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = |v| wgpu::BindingResource::TextureView(v);
        let march_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clouds"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: view(&base),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view(&detail),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: view(&weather),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&repeat),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: view(&curl),
                },
            ],
        });
        Gpu {
            clamp: sampler(device, "clouds clamp", wgpu::AddressMode::ClampToEdge),
            uniform,
            march_bind,
            resolve_layout,
            march,
            resolve,
            history: None,
        }
    }
}

impl History {
    fn new(
        device: &wgpu::Device,
        gpu: &Gpu,
        layouts: &Layouts,
        size: (u32, u32),
        scale: u32,
    ) -> History {
        let image = |size: (u32, u32)| {
            texture(
                device,
                "clouds",
                (size.0, size.1, 1),
                wgpu::TextureDimension::D2,
                HISTORY,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                1,
            )
            .create_view(&Default::default())
        };
        let march = image(size);
        let views = [0, 1].map(|_| image((size.0 * scale, size.1 * scale)));
        let resolve = [0, 1].map(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("clouds resolve"),
                layout: &gpu.resolve_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&march),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[1 - i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&gpu.clamp),
                    },
                ],
            })
        });
        let sky = [0, 1].map(|i| sky_bind(device, layouts, &views[i], &gpu.clamp));
        History {
            size,
            scale,
            march,
            views,
            resolve,
            sky,
            current: 0,
        }
    }
}

/// The march's module source.
pub(crate) fn march_source() -> String {
    [COMMON, CLOUDS_WGSL].concat()
}

/// The noise's module source.
pub(crate) fn noise_source() -> String {
    [COMMON, NOISE_WGSL].concat()
}

/// Fills the base and detail noise (`noise`, with every level), the weather map and the curl
/// noise (`flat`) once.
fn fill_noise(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    noise: [&wgpu::Texture; 2],
    flat: [&wgpu::Texture; 2],
) {
    let storage = |binding, dim| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: NOISE,
            view_dimension: dim,
        },
        count: None,
    };
    use wgpu::TextureViewDimension as D;
    let level = |t: &wgpu::Texture, mip: u32| {
        t.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: mip,
            mip_level_count: Some(1),
            ..Default::default()
        })
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cloud noise"),
        entries: &[
            storage(0, D::D3),
            storage(1, D::D3),
            storage(2, D::D2),
            storage(3, D::D2),
        ],
    });
    let tops = [
        level(noise[0], 0),
        level(noise[1], 0),
        level(flat[0], 0),
        level(flat[1], 0),
    ];
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cloud noise"),
        layout: &layout,
        entries: &[0, 1, 2, 3].map(|i| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(&tops[i]),
        }),
    });
    let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cloud noise level"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: D::D3,
                    multisampled: false,
                },
                count: None,
            },
            storage(1, D::D3),
        ],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cloud noise"),
        source: wgpu::ShaderSource::Wgsl(noise_source().into()),
    });
    let compute = |entry, groups: &[Option<&wgpu::BindGroupLayout>]| {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(entry),
            bind_group_layouts: groups,
            immediate_size: 0,
        });
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let mip_pipeline = compute("cs_mip", &[None, None, Some(&mip_layout)]);
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("cloud noise"),
        timestamp_writes: None,
    });
    pass.set_bind_group(1, &bind, &[]);
    for (entry, groups) in [
        ("cs_base", (BASE / 4, BASE / 4, BASE / 4)),
        ("cs_detail", (DETAIL / 4, DETAIL / 4, DETAIL / 4)),
        ("cs_weather", (WEATHER / 8, WEATHER / 8, 1)),
        ("cs_curl", (CURL / 8, CURL / 8, 1)),
    ] {
        pass.set_pipeline(&compute(entry, &[None, Some(&layout)]));
        pass.dispatch_workgroups(groups.0, groups.1, groups.2);
    }
    drop(pass);
    // Each level from the one above.
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("cloud noise levels"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&mip_pipeline);
    for t in noise {
        for mip in 1..t.mip_level_count() {
            let (src, dst) = (level(t, mip - 1), level(t, mip));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cloud noise level"),
                layout: &mip_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&dst),
                    },
                ],
            });
            pass.set_bind_group(2, &bind, &[]);
            let side = (t.width() >> mip).max(1).div_ceil(4);
            pass.dispatch_workgroups(side, side, side);
        }
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
    fn cloud_shaders_validate() {
        validate(&march_source());
        validate(&noise_source());
    }

    fn turned(degrees: f32) -> f32 {
        degrees.to_radians().cos()
    }

    #[test]
    fn a_tiny_sun_move_keeps_the_normal_blend() {
        let (reset, blend) = history_blend(turned(0.2), 1.0, false, BLEND_SAME);
        assert!(!reset);
        assert_eq!(blend, BLEND_SAME);
    }

    #[test]
    fn a_two_degree_sun_move_relights_without_a_reset() {
        for normal in [BLEND_SAME, BLEND_OWN] {
            let (reset, blend) = history_blend(turned(2.0), 1.0, false, normal);
            assert!(!reset);
            assert_eq!(blend, BLEND_RELIGHT);
        }
        // Four frames of it leave less than a tenth of the old light.
        assert!((1.0 - BLEND_RELIGHT).powi(4) < 0.1);
    }

    #[test]
    fn a_ten_degree_sun_jump_resets() {
        assert!(history_blend(turned(10.0), 1.0, false, BLEND_SAME).0);
    }

    #[test]
    fn a_picture_a_view_turn_and_other_reasons_reset() {
        assert!(history_blend(1.0, 1.0, true, BLEND_SAME).0);
        assert!(history_blend(1.0, turned(20.0), false, BLEND_SAME).0);
    }

    #[test]
    fn a_drag_of_one_and_a_half_degrees_a_frame_never_resets() {
        let mut sun = 0.0f32;
        for _ in 0..100 {
            let next = sun + 1.5;
            let (reset, blend) =
                history_blend((next - sun).to_radians().cos(), 1.0, false, BLEND_OWN);
            assert!(!reset);
            assert_eq!(blend, BLEND_RELIGHT);
            sun = next;
        }
    }

    #[test]
    fn the_noise_tiles_divide_the_weather_period() {
        // Wrapping the drift at the period must not move the noise (common.wgsl constants).
        for tile in [2560.0, 160.0, 1280.0] {
            assert_eq!(CLOUD_PERIOD % tile, 0.0);
        }
        assert!(COMMON.contains("const CLOUD_PERIOD: f32 = 40960.0;"));
        assert!(COMMON.contains("const CLOUD_BASE_TILE: f32 = 2560.0;"));
        assert!(COMMON.contains("const CLOUD_DETAIL_TILE: f32 = 160.0;"));
        assert!(COMMON.contains("const CLOUD_CURL_TILE: f32 = 1280.0;"));
    }

    #[test]
    fn the_uniform_block_matches_the_shader() {
        // A matrix and three vec4s (clouds.wgsl `CloudFrame`).
        assert_eq!(UNIFORM_SIZE, 64 + 3 * 16);
        assert!(CLOUDS_WGSL.contains("    up: vec4<f32>,\n}"));
    }
}
