//! Picking a starting quality from the hardware: what wgpu reports about the
//! adapter (`HardwareProbe`), a short calibration that times a fixed
//! workload (`calibrate` -> `Score`), and the mapping of both onto a
//! preset and render scale for an output size (`starting_quality`). The
//! adapter's kind is only a prior; the measured score decides. Afterwards
//! `suggest_quality` fine-tunes from frame times.
//!
//! In a browser wgpu fills little of the adapter info: the name is
//! `GPUAdapterInfo.description` (empty in Chromium unless unmasked), vendor
//! and device ids are 0, the kind is `Other` (`Cpu` for a fallback adapter),
//! the backend `BrowserWebGpu`; the subgroup sizes are filled when the
//! browser exposes them. Limits and features are real (Chromium buckets
//! some limits). So on the web the calibration is what tells GPUs apart.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::Quality;
use crate::stats::{TARGET_MS, ladder};

/// What kind of adapter wgpu says this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuKind {
    Integrated,
    Discrete,
    Virtual,
    Cpu,
    /// Not reported (browsers, some drivers).
    Unknown,
}

/// The adapter and device as wgpu reports them (see the module docs for
/// what a browser leaves empty).
#[derive(Clone, Debug, PartialEq)]
pub struct HardwareProbe {
    pub name: String,
    /// PCI vendor id (0 when unknown).
    pub vendor: u32,
    /// PCI device id (0 when unknown).
    pub device: u32,
    pub kind: GpuKind,
    /// "Vulkan", "Metal", "Dx12", "Gl", "BrowserWebGpu", ...
    pub backend: String,
    pub driver: String,
    /// (min, max) subgroup size; Intel 8..32, NVIDIA 32, AMD 32..64.
    pub subgroup_sizes: (u32, u32),
    pub max_texture_dimension_2d: u32,
    pub max_bind_groups: u32,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_uniform_buffer_binding_size: u64,
    pub max_storage_buffer_binding_size: u64,
    pub timestamp_query: bool,
    pub float32_filterable: bool,
    pub compression_bc: bool,
    pub compression_astc: bool,
    pub compression_etc2: bool,
}

impl HardwareProbe {
    /// From an adapter: every feature it offers (before a device asks).
    pub fn from_adapter(adapter: &wgpu::Adapter) -> HardwareProbe {
        Self::new(adapter.get_info(), adapter.features(), adapter.limits())
    }

    /// From a device: the features it was created with.
    pub fn from_device(device: &wgpu::Device) -> HardwareProbe {
        Self::new(device.adapter_info(), device.features(), device.limits())
    }

    fn new(info: wgpu::AdapterInfo, f: wgpu::Features, l: wgpu::Limits) -> HardwareProbe {
        use wgpu::Features as F;
        HardwareProbe {
            name: info.name,
            vendor: info.vendor,
            device: info.device,
            kind: match info.device_type {
                wgpu::DeviceType::IntegratedGpu => GpuKind::Integrated,
                wgpu::DeviceType::DiscreteGpu => GpuKind::Discrete,
                wgpu::DeviceType::VirtualGpu => GpuKind::Virtual,
                wgpu::DeviceType::Cpu => GpuKind::Cpu,
                wgpu::DeviceType::Other => GpuKind::Unknown,
            },
            backend: format!("{:?}", info.backend),
            driver: info.driver,
            subgroup_sizes: (info.subgroup_min_size, info.subgroup_max_size),
            max_texture_dimension_2d: l.max_texture_dimension_2d,
            max_bind_groups: l.max_bind_groups,
            max_storage_buffers_per_shader_stage: l.max_storage_buffers_per_shader_stage,
            max_uniform_buffer_binding_size: l.max_uniform_buffer_binding_size,
            max_storage_buffer_binding_size: l.max_storage_buffer_binding_size,
            timestamp_query: f.contains(F::TIMESTAMP_QUERY),
            float32_filterable: f.contains(F::FLOAT32_FILTERABLE),
            compression_bc: f.contains(F::TEXTURE_COMPRESSION_BC),
            compression_astc: f.contains(F::TEXTURE_COMPRESSION_ASTC),
            compression_etc2: f.contains(F::TEXTURE_COMPRESSION_ETC2),
        }
    }
}

/// The calibration workload's cost on this GPU: frame time ~ `fixed_ms` +
/// `ms_per_megapixel` x megapixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score {
    pub fixed_ms: f32,
    pub ms_per_megapixel: f32,
    /// Timed on the GPU with timestamp queries (otherwise with the caller's
    /// clock around submit-and-wait, which includes some CPU and queue time).
    pub gpu_timed: bool,
}

/// The two sizes the workload renders at.
const SIZES: [(u32, u32); 2] = [(640, 360), (1280, 720)];
const WARMUP: usize = 2;
const MEASURED: usize = 4;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Renders a fixed workload offscreen (a procedural 64k-triangle grid,
/// six layers deep back to front, each fragment sixteen filtered texture
/// reads and some arithmetic into an `Rgba16Float` target: roughly what a
/// shaded, overdrawn frame costs) for a few frames at 640x360 and
/// 1280x720, and fits the time per frame to `fixed + per megapixel`.
/// Timed with timestamp queries when the device has them, else with
/// `clock` (milliseconds, any origin) around each frame's submit and wait;
/// with neither it fails. About 12 frames; on an Intel UHD 620 under
/// 0.2 s. Works on the web (await it; nothing blocks there).
pub async fn calibrate(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    clock: Option<&dyn Fn() -> f64>,
) -> Result<Score, String> {
    let timestamps = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
    if !timestamps && clock.is_none() {
        return Err("no timestamp queries and no clock".into());
    }
    let workload = Workload::new(device, queue);
    let mut ms = [0.0f64; 2];
    for (k, &size) in SIZES.iter().enumerate() {
        let (color, depth) = workload.targets(device, size);
        let mut samples = Vec::new();
        for frame in 0..WARMUP + MEASURED {
            let t = if timestamps {
                workload.timed_frame(device, queue, &color, &depth).await?
            } else {
                let clock = clock.ok_or("no clock")?;
                let start = clock();
                workload.frame(device, queue, &color, &depth, None);
                done(device, queue).await?;
                clock() - start
            };
            if frame >= WARMUP {
                samples.push(t);
            }
        }
        samples.sort_by(f64::total_cmp);
        ms[k] = samples[samples.len() / 2];
    }
    let mp = SIZES.map(|(w, h)| f64::from(w * h) / 1e6);
    let per_mp = ((ms[1] - ms[0]) / (mp[1] - mp[0])).max(0.0);
    Ok(Score {
        fixed_ms: (ms[0] - per_mp * mp[0]).max(0.0) as f32,
        ms_per_megapixel: per_mp as f32,
        gpu_timed: timestamps,
    })
}

/// How many times the calibration workload's cost per megapixel one frame
/// of each preset costs per megapixel it renders. Measured on an Intel UHD
/// 620: calibration 10.0 ms per megapixel (4.4 ms fixed); the garden scene
/// of `examples/bench.rs --garden` at 1280x720 took 9.4-10.9 ms on LOW,
/// 15.0 on MEDIUM, 37.5-39.9 on HIGH and 60.3-63.2 on ULTRA.
const PRESET_COST: [f32; 4] = [1.5, 2.0, 3.7, 6.2];
/// What a frame at quality `q` costs per output megapixel, relative to the calibration
/// workload: its preset's `PRESET_COST` times the share of the pixels it renders (`render_scale`
/// squared). The ratio of two of these predicts the frame time of one quality from another's.
pub fn relative_cost(q: &Quality) -> f32 {
    let preset = Quality::PRESETS
        .iter()
        .position(|(_, p)| crate::stats::same_except_scale(p, q))
        .unwrap_or(1);
    let s = q.render_scale.clamp(0.5, 1.0);
    PRESET_COST[preset] * s * s
}

/// The share of the frame budget a starting quality may take: the rest is
/// headroom for heavier views, the UI and `suggest_quality`'s first step.
const BUDGET: f32 = 0.7;

/// The best preset and render scale whose predicted frame time at `size`
/// (output pixels) fits `BUDGET` of the 30 fps budget, from the ladder
/// `suggest_quality` walks (so it fine-tunes from there). The predicted
/// time is the score's cost per megapixel times the preset's
/// `PRESET_COST` times the megapixels it renders, plus the score's fixed
/// cost. The adapter kind is a prior that only shifts the prediction: an
/// integrated GPU is assumed 15 % slower than measured (it shares power and
/// memory with the CPU and throttles), a discrete one 10 % faster; a CPU
/// adapter starts at LOW. Without a score the prior alone picks: MEDIUM for
/// integrated or unknown GPUs, HIGH for discrete ones.
pub fn starting_quality(probe: &HardwareProbe, score: Option<Score>, size: (u32, u32)) -> Quality {
    if probe.kind == GpuKind::Cpu {
        return ladder()[0];
    }
    let Some(score) = score else {
        return if probe.kind == GpuKind::Discrete {
            Quality::HIGH
        } else {
            Quality::MEDIUM
        };
    };
    let prior = match probe.kind {
        GpuKind::Integrated => 1.15,
        GpuKind::Discrete => 0.9,
        _ => 1.0,
    };
    let mp = size.0 as f32 * size.1 as f32 / 1e6;
    let predict = |q: &Quality| {
        let preset = Quality::PRESETS
            .iter()
            .position(|(_, p)| crate::stats::same_except_scale(p, q))
            .unwrap_or(1);
        let pixels = mp * q.render_scale * q.render_scale;
        prior * (score.fixed_ms + score.ms_per_megapixel * PRESET_COST[preset] * pixels)
    };
    ladder()
        .into_iter()
        .rev()
        .find(|q| predict(q) <= BUDGET * TARGET_MS)
        .unwrap_or_else(|| ladder()[0])
}

/// The fixed workload's pipeline and texture.
struct Workload {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    queries: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
    period_ns: f32,
}

const GRID: u32 = 256; // cells per row; 128 rows: 65 536 triangles a layer
const LAYERS: u32 = 6;

const SHADER: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) layer: u32) -> Out {
    let cell = vi / 6u;
    let corner = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0))[vi % 6u];
    let p = (vec2<f32>(f32(cell % 256u), f32(cell / 256u)) + corner) / vec2<f32>(256.0, 128.0);
    // Back to front: every layer is shaded (worst-case overdraw).
    let depth = 0.9 - f32(layer) * 0.1;
    var out: Out;
    out.clip = vec4<f32>(p * 2.0 - 1.0, depth, 1.0);
    out.uv = p * 4.0 + f32(layer) * 0.37;
    return out;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    var c = vec3<f32>(0.0);
    for (var i = 0; i < 16; i++) {
        let o = vec2<f32>(cos(f32(i) * 2.4), sin(f32(i) * 2.4)) * 0.01 * f32(i);
        c += textureSample(tex, smp, in.uv + o).rgb;
    }
    let n = normalize(vec3<f32>(c.x - 0.5, c.y - 0.5, 1.0));
    return vec4<f32>(c / 16.0 * pow(max(n.z, 0.0), 8.0), 1.0);
}
"#;

impl Workload {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Workload {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("calibration"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let size = 256u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibration"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // A fixed pattern, so every run reads the same texels.
        let texels: Vec<u8> = (0..size * size)
            .flat_map(|i| {
                let (x, y) = (i % size, i / size);
                [(x * 7 + y * 3) as u8, (x ^ y) as u8, (x * y) as u8, 255]
            })
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            texture.size(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("calibration"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("calibration"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let queries = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                let set = device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("calibration"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                });
                let buffer = |usage| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("calibration timestamps"),
                        size: 16,
                        usage,
                        mapped_at_creation: false,
                    })
                };
                (
                    set,
                    buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
                    buffer(wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
                )
            });
        Workload {
            pipeline,
            bind,
            queries,
            period_ns: queue.get_timestamp_period(),
        }
    }

    fn targets(
        &self,
        device: &wgpu::Device,
        (width, height): (u32, u32),
    ) -> (wgpu::TextureView, wgpu::TextureView) {
        let target = |format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("calibration target"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        (target(FORMAT), target(wgpu::TextureFormat::Depth32Float))
    }

    fn frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        timestamps: Option<&wgpu::QuerySet>,
    ) {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("calibration"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: timestamps.map(|set| wgpu::RenderPassTimestampWrites {
                    query_set: set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..GRID * 128 * 6, 0..LAYERS);
        }
        if let (Some(set), Some((_, resolve, read))) = (timestamps, &self.queries) {
            encoder.resolve_query_set(set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
        }
        queue.submit([encoder.finish()]);
    }

    /// One frame timed on the GPU (milliseconds).
    async fn timed_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) -> Result<f64, String> {
        let (set, _, read) = self.queries.as_ref().ok_or("no timestamp queries")?;
        self.frame(device, queue, color, depth, Some(set));
        map(device, read).await?;
        let ticks = {
            let data = read
                .slice(..)
                .get_mapped_range()
                .map_err(|e| e.to_string())?;
            let at =
                |i: usize| u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
            at(1).saturating_sub(at(0))
        };
        read.unmap();
        Ok(ticks as f64 * f64::from(self.period_ns) / 1e6)
    }
}

/// Waits until the GPU finished everything submitted so far.
async fn done(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), String> {
    let signal = Signal::default();
    let slot = signal.0.clone();
    queue.on_submitted_work_done(move || complete(&slot, Ok(())));
    wait(device, signal).await
}

fn complete(slot: &Mutex<SignalState>, r: Result<(), String>) {
    if let Ok(mut s) = slot.lock() {
        s.0 = Some(r);
        if let Some(w) = s.1.take() {
            w.wake();
        }
    }
}

/// Natively the device is polled until the callback ran; on the web the
/// browser runs it and the future resolves.
async fn wait(device: &wgpu::Device, signal: Signal) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    let _ = device;
    signal.await
}

/// Maps `buffer` for reading and waits for it.
async fn map(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<(), String> {
    let signal = Signal::default();
    let slot = signal.0.clone();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        complete(&slot, r.map_err(|e| e.to_string()));
    });
    wait(device, signal).await
}

type SignalState = (Option<Result<(), String>>, Option<Waker>);

/// A one-shot future completed by a wgpu callback.
#[derive(Default)]
struct Signal(Arc<Mutex<SignalState>>);

impl Future for Signal {
    type Output = Result<(), String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut s) = self.0.lock() else {
            return Poll::Ready(Err("calibration state poisoned".into()));
        };
        match s.0.take() {
            Some(r) => Poll::Ready(r),
            None => {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(kind: GpuKind) -> HardwareProbe {
        HardwareProbe {
            name: String::new(),
            vendor: 0,
            device: 0,
            kind,
            backend: "BrowserWebGpu".into(),
            driver: String::new(),
            subgroup_sizes: (0, 0),
            max_texture_dimension_2d: 8192,
            max_bind_groups: 4,
            max_storage_buffers_per_shader_stage: 8,
            max_uniform_buffer_binding_size: 65536,
            max_storage_buffer_binding_size: 128 << 20,
            timestamp_query: false,
            float32_filterable: false,
            compression_bc: true,
            compression_astc: false,
            compression_etc2: false,
        }
    }

    fn score(ms_per_megapixel: f32) -> Option<Score> {
        Some(Score {
            fixed_ms: 0.3,
            ms_per_megapixel,
            gpu_timed: true,
        })
    }

    #[test]
    fn a_fast_gpu_starts_on_ultra_and_a_slow_one_low() {
        let unknown = probe(GpuKind::Unknown);
        let wide = (2560, 1300);
        assert_eq!(starting_quality(&unknown, score(0.2), wide), Quality::ULTRA);
        let slow = starting_quality(&unknown, score(40.0), wide);
        assert!(crate::stats::same_except_scale(&slow, &Quality::LOW));
        assert!(slow.render_scale < 0.6, "{slow:?}");
    }

    #[test]
    fn the_size_matters() {
        let p = probe(GpuKind::Unknown);
        let small = starting_quality(&p, score(3.0), (1280, 720));
        let large = starting_quality(&p, score(3.0), (3840, 2160));
        let rank = |q: &Quality| ladder().iter().position(|l| l == q).unwrap();
        assert!(rank(&small) > rank(&large), "{small:?} vs {large:?}");
    }

    #[test]
    fn the_kind_is_only_a_prior() {
        // Borderline: the prior tips it; far from a border it does not.
        let w = (1920, 1080);
        let rank = |q: Quality| ladder().iter().position(|l| *l == q).unwrap();
        let mut tipped = false;
        for tenth in 5..200 {
            let s = score(tenth as f32 / 10.0);
            let i = rank(starting_quality(&probe(GpuKind::Integrated), s, w));
            let d = rank(starting_quality(&probe(GpuKind::Discrete), s, w));
            assert!(d >= i);
            assert!(d - i <= 2, "the prior moved {} rungs", d - i);
            tipped |= d > i;
        }
        assert!(tipped);
        // Without a score the prior alone decides; a CPU adapter starts low.
        assert_eq!(
            starting_quality(&probe(GpuKind::Discrete), None, w),
            Quality::HIGH
        );
        assert_eq!(
            starting_quality(&probe(GpuKind::Integrated), None, w),
            Quality::MEDIUM
        );
        assert_eq!(
            starting_quality(&probe(GpuKind::Cpu), score(0.1), w),
            ladder()[0]
        );
    }
}
