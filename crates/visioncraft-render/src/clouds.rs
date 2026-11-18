//! Volumetric clouds (clouds.wgsl): a raymarch through a layer of cloud at a quarter or half of
//! the render size, blended over frames, which the sky pass reads (sky.wgsl `background`).
//!
//! Everything is made the first time the clouds are drawn, never at start-up or per frame:
//! three noise textures filled once by compute shaders (cloud_noise.wgsl: a 128^3 Perlin-Worley
//! and Worley base, a 32^3 Worley detail, a 1024^2 weather map), the march pipeline, and two
//! history images at the cloud resolution that the march writes into in turn, each frame
//! blending its jittered result into the other's (reprojected by direction). A frame that
//! cannot use the history (the first, a new size, a cut, a jump of the sun or the weather)
//! marches four jitters per pixel instead, and a picture (one frame, `Draw::picture`) eight at
//! fixed offsets, so the frames of a film keep one pattern and do not flicker. Without
//! volumetric clouds the sky reads a 1x1 image and nothing else is made.

use glam::{Mat4, Vec3, Vec4};

use crate::{COMMON, Layouts, PipelineDesc, color_pass, pipeline};

/// The noise textures' sizes (texels per side).
const BASE: u32 = 128;
const DETAIL: u32 = 32;
const WEATHER: u32 = 1024;
const HISTORY: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const NOISE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Weight of a frame against the history: about ten frames make the image.
const BLEND: f32 = 0.12;
/// Jitters per pixel when the history is started over, and for a picture.
const RESET_SUBSAMPLES: f32 = 4.0;
const PICTURE_SUBSAMPLES: f32 = 8.0;
/// The sun may move this far between frames (cosine, about a degree) before the history is
/// started over: the clouds' light changes with it.
const SUN_COS: f32 = 0.99985;
/// The view may turn this far between frames (cosine) before the history is started over.
const TURN_COS: f32 = 0.97;

/// The weather map's period in metres (common.wgsl `CLOUD_PERIOD`): the drift wraps here.
pub(crate) const CLOUD_PERIOD: f64 = 40960.0;

const CLOUDS_WGSL: &str = include_str!("clouds.wgsl");
const NOISE_WGSL: &str = include_str!("cloud_noise.wgsl");

/// What one frame's march needs.
pub(crate) struct Draw<'a> {
    pub layouts: &'a Layouts,
    pub globals: &'a wgpu::BindGroup,
    /// The render size (texels).
    pub size: (u32, u32),
    /// The cloud image's divisor of it (2 half, 4 quarter).
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
}

pub(crate) struct CloudVolume {
    sky_off: wgpu::BindGroup,
    gpu: Option<Gpu>,
    frame: u32,
    /// The last march's view-projection without translation, its view axis and the sun.
    prev: Option<(Mat4, Vec3, Vec3)>,
    /// The history image the sky reads (`None`: the 1x1 one).
    showing: Option<usize>,
}

struct Gpu {
    base: wgpu::TextureView,
    detail: wgpu::TextureView,
    weather: wgpu::TextureView,
    repeat: wgpu::Sampler,
    clamp: wgpu::Sampler,
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    history: Option<History>,
}

struct History {
    size: (u32, u32),
    views: [wgpu::TextureView; 2],
    /// Writing into image i, reading the other.
    march: [wgpu::BindGroup; 2],
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
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: size.2,
        },
        mip_level_count: 1,
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

    /// Marches this frame's clouds into the next history image (making what is missing first).
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
        let mut reset = d.jumped || d.picture;
        if gpu.history.as_ref().is_none_or(|h| h.size != size) {
            gpu.history = Some(History::new(device, d.layouts, gpu, size));
            reset = true;
        }
        let mut rotation = d.view;
        rotation.w_axis = Vec4::W;
        let view_proj = d.proj * rotation;
        let axis = -Vec3::new(d.view.x_axis.z, d.view.y_axis.z, d.view.z_axis.z);
        let prev = match self.prev {
            Some((m, a, s))
                if self.showing.is_some() && a.dot(axis) >= TURN_COS && s.dot(d.sun) >= SUN_COS =>
            {
                m
            }
            _ => {
                reset = true;
                view_proj
            }
        };
        let subsamples = match (d.picture, reset) {
            (true, _) => PICTURE_SUBSAMPLES,
            (false, true) => RESET_SUBSAMPLES,
            (false, false) => 1.0,
        };
        let mut b = Vec::with_capacity(96);
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
            if reset { 1.0 } else { BLEND },
            subsamples,
            d.steps.max(1) as f32,
            d.light_steps as f32,
            size.0 as f32,
            size.1 as f32,
            0.0,
        ];
        for f in params {
            b.extend_from_slice(&f.to_le_bytes());
        }
        queue.write_buffer(&gpu.uniform, 0, &b);
        let Some(history) = gpu.history.as_mut() else {
            return;
        };
        let next = 1 - history.current;
        {
            let mut pass = color_pass(
                encoder,
                "clouds",
                &history.views[next],
                None,
                None,
                timestamp,
            );
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, d.globals, &[]);
            pass.set_bind_group(1, &history.march[next], &[]);
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
        );
        let detail = texture(
            device,
            "cloud detail noise",
            (DETAIL, DETAIL, DETAIL),
            d3,
            NOISE,
            usage,
        );
        let weather = texture(
            device,
            "cloud weather",
            (WEATHER, WEATHER, 1),
            wgpu::TextureDimension::D2,
            NOISE,
            usage,
        );
        let views = [&base, &detail, &weather].map(|t| t.create_view(&Default::default()));
        fill_noise(device, encoder, &views);
        let [base, detail, weather] = views;

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
                smp(6),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("clouds"),
            source: wgpu::ShaderSource::Wgsl(march_source().into()),
        });
        let pipeline = pipeline(
            device,
            PipelineDesc {
                label: "clouds",
                module: &module,
                vs: "vs_clouds",
                fs: Some("fs_clouds"),
                groups: &[Some(&layouts.globals), Some(&layout)],
                buffers: &[],
                target: Some(HISTORY.into()),
                depth: None,
                cull: None,
                samples: 1,
                constants: &[],
                front_cw: false,
            },
        );
        Gpu {
            base,
            detail,
            weather,
            repeat: sampler(device, "clouds repeat", wgpu::AddressMode::Repeat),
            clamp: sampler(device, "clouds clamp", wgpu::AddressMode::ClampToEdge),
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("clouds frame"),
                size: 96,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            layout,
            pipeline,
            history: None,
        }
    }
}

impl History {
    fn new(device: &wgpu::Device, layouts: &Layouts, gpu: &Gpu, size: (u32, u32)) -> History {
        let views = [0, 1].map(|_| {
            texture(
                device,
                "clouds",
                (size.0, size.1, 1),
                wgpu::TextureDimension::D2,
                HISTORY,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        let march = [0, 1].map(|i| {
            let view = |v| wgpu::BindingResource::TextureView(v);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("clouds"),
                layout: &gpu.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: gpu.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: view(&gpu.base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(&gpu.detail),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: view(&gpu.weather),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&gpu.repeat),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: view(&views[1 - i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&gpu.clamp),
                    },
                ],
            })
        });
        let sky = [0, 1].map(|i| sky_bind(device, layouts, &views[i], &gpu.clamp));
        History {
            size,
            views,
            march,
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

/// Fills the base, detail and weather textures (`views`, in that order) once.
fn fill_noise(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    views: &[wgpu::TextureView; 3],
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
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cloud noise"),
        entries: &[storage(0, D::D3), storage(1, D::D3), storage(2, D::D2)],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cloud noise"),
        layout: &layout,
        entries: &[0, 1, 2].map(|i| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(&views[i]),
        }),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cloud noise"),
        bind_group_layouts: &[None, Some(&layout)],
        immediate_size: 0,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cloud noise"),
        source: wgpu::ShaderSource::Wgsl(noise_source().into()),
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("cloud noise"),
        timestamp_writes: None,
    });
    pass.set_bind_group(1, &bind, &[]);
    for (entry, groups) in [
        ("cs_base", (BASE / 4, BASE / 4, BASE / 4)),
        ("cs_detail", (DETAIL / 4, DETAIL / 4, DETAIL / 4)),
        ("cs_weather", (WEATHER / 8, WEATHER / 8, 1)),
    ] {
        let p = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        });
        pass.set_pipeline(&p);
        pass.dispatch_workgroups(groups.0, groups.1, groups.2);
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

    #[test]
    fn the_noise_tiles_divide_the_weather_period() {
        // Wrapping the drift at the period must not move the noise (common.wgsl constants).
        for tile in [2560.0, 320.0] {
            assert_eq!(CLOUD_PERIOD % tile, 0.0);
        }
        assert!(COMMON.contains("const CLOUD_PERIOD: f32 = 40960.0;"));
    }
}
