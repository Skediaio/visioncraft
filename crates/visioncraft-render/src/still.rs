//! Offscreen stills: one frame at any size up to 3840x2160, always at
//! `Quality::HIGH`, read back as RGBA8, without touching the live view's
//! targets, quality, camera, cached shadow cascades, snow cover map or
//! level-of-detail state.
//!
//! The frame is submitted at once; the pixels arrive asynchronously (on the
//! web nothing can block), so `render_still` returns a [`Still`] to poll.
//! Call it between frames, not between `render` and the submit of that
//! frame's encoder (both write the same uniform buffers). Allocation
//! failures (a 4K target with 4x MSAA may not fit a small GPU) and
//! validation errors are caught with error scopes and come back as `Err`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use glam::{Mat4, Vec3};

use crate::{FramePart, LOD_UNSET, Pipelines, Quality, Renderer, SampledPipelines, Targets};

/// Pipelines an offscreen frame uses instead of the live ones (none where those do): all of
/// them for the other shading (`Quality::fast_shading`), the multisampled ones for other samples.
type Offscreen = (Option<Pipelines>, Option<SampledPipelines>);

pub const MAX_STILL: (u32, u32) = (3840, 2160);

type ErrorFuture = Pin<Box<dyn Future<Output = Option<wgpu::Error>>>>;

/// A still being rendered; poll with [`Still::try_take`] (or block natively
/// with [`Still::wait`]).
pub struct Still {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // only `wait` polls it
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    size: (u32, u32),
    stride: u32,
    mapped: Arc<Mutex<Option<Result<(), String>>>>,
    errors: Vec<ErrorFuture>,
}

impl Still {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// `None` while the GPU is still working; then the tightly packed RGBA8
    /// rows (sRGB-encoded, top to bottom) or the error.
    pub fn try_take(&mut self) -> Option<Result<Vec<u8>, String>> {
        let mut cx = Context::from_waker(Waker::noop());
        while let Some(f) = self.errors.first_mut() {
            match f.as_mut().poll(&mut cx) {
                Poll::Ready(Some(e)) => return Some(Err(format!("still failed: {e}"))),
                Poll::Ready(None) => {
                    drop(self.errors.remove(0));
                }
                Poll::Pending => return None,
            }
        }
        let state = self.mapped.lock().ok()?.take()?;
        if let Err(e) = state {
            return Some(Err(format!("still readback failed: {e}")));
        }
        let (w, h) = self.size;
        let pixels = match self.buffer.slice(..).get_mapped_range() {
            Ok(view) => view
                .chunks(self.stride as usize)
                .take(h as usize)
                .flat_map(|row| &row[..(w * 4) as usize])
                .copied()
                .collect(),
            Err(e) => return Some(Err(format!("still readback failed: {e}"))),
        };
        self.buffer.unmap();
        Some(Ok(pixels))
    }

    /// Blocks until the pixels are back (native only: on the web `poll`
    /// does not wait, use `try_take` from the event loop).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait(mut self) -> Result<Vec<u8>, String> {
        loop {
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| format!("poll: {e}"))?;
            if let Some(result) = self.try_take() {
                return result;
            }
        }
    }
}

impl Renderer {
    /// Renders one frame of the current scene from `view`/`proj`/`eye` at
    /// `width` x `height` with `Quality::HIGH`, independent of the live
    /// quality and size.
    pub fn render_still(
        &mut self,
        width: u32,
        height: u32,
        view: Mat4,
        proj: Mat4,
        eye: Vec3,
    ) -> Result<Still, String> {
        if width == 0 || height == 0 || width > MAX_STILL.0 || height > MAX_STILL.1 {
            return Err(format!(
                "still size {width}x{height} outside 1x1..={}x{}",
                MAX_STILL.0, MAX_STILL.1
            ));
        }
        let quality = self.coverage.quality(Quality::HIGH);
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let targets = Targets::new(
            &self.device,
            &self.layouts,
            self.output_format,
            quality,
            (width, height),
            &self.shadow_sampler,
            &self.post_sampler,
            &self.white,
            &self.physical.aerial,
            false,
        );
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("still readback"),
            size: stride as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let sampled = self.offscreen_pipelines(quality);

        let (_, _, encoder) = self.render_offscreen(
            targets,
            sampled,
            quality,
            (width, height),
            (view, proj, eye),
            Some((&buffer, stride)),
            FramePart::Whole,
        );
        self.queue.submit([encoder.finish()]);
        let mapped = Arc::new(Mutex::new(None));
        let slot = mapped.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(r.map_err(|e| e.to_string()));
            }
        });
        let errors: Vec<ErrorFuture> = vec![Box::pin(validation.pop()), Box::pin(oom.pop())];
        Ok(Still {
            device: self.device.clone(),
            buffer,
            size: (width, height),
            stride,
            mapped,
            errors,
        })
    }

    /// The pipelines a frame at `quality` needs where the live ones do not do (`Offscreen`).
    fn offscreen_pipelines(&self, quality: Quality) -> Offscreen {
        let (device, layouts) = (&self.device, &self.layouts);
        if quality.fast_shading != self.quality.fast_shading {
            let modules = crate::shader_modules(device, quality.fast_shading, self.coverage);
            return (
                Some(Pipelines::new(
                    device,
                    layouts,
                    &modules,
                    self.output_format,
                )),
                Some(SampledPipelines::new(
                    device,
                    layouts,
                    &modules,
                    quality.samples(),
                    self.coverage,
                )),
            );
        }
        let sampled = (quality.msaa != self.quality.msaa).then(|| {
            SampledPipelines::new(
                device,
                layouts,
                &self.modules,
                quality.samples(),
                self.coverage,
            )
        });
        (None, sampled)
    }

    /// Renders `part` of one frame into `targets` (and with the pipelines `sampled` where the live ones
    /// do not do, `offscreen_pipelines`) from `view`/`proj`/`eye`, copies the output
    /// into `buffer`, and puts the live state back. Returns the targets for another frame and
    /// the encoder to submit. `quality` is sanitized as in `set_quality` (`Quality::sanitized`).
    #[allow(clippy::too_many_arguments)]
    fn render_offscreen(
        &mut self,
        targets: Targets,
        sampled: Offscreen,
        quality: Quality,
        (width, height): (u32, u32),
        (view, proj, eye): (Mat4, Mat4, Vec3),
        copy: Option<(&wgpu::Buffer, u32)>,
        part: FramePart,
    ) -> (Targets, Offscreen, wgpu::CommandEncoder) {
        let quality = quality.sanitized();
        // What changed since the window's last frame (moved instances, new bounds) the window's
        // shadow caches no longer cover: they go now, since the still's frame below makes the
        // bounds anew and takes the list of moved items, and the window's next frame would not
        // see that anything changed.
        if self.bounds_dirty || !self.moved.is_empty() {
            self.invalidate_shadows();
        }
        // Swap the live state out, render, swap it back.
        let live_targets = std::mem::replace(&mut self.targets, targets);
        let live_pipelines = sampled.0.map(|p| std::mem::replace(&mut self.pipelines, p));
        let live_sampled = sampled.1.map(|s| std::mem::replace(&mut self.sampled, s));
        let live = (self.size, self.quality, self.view, self.proj, self.eye);
        let live_cache = self.far_cache.take();
        let live_cascades = std::mem::take(&mut self.cascade_rendered);
        let live_stale = std::mem::take(&mut self.cascade_stale);
        // The snow cover map too: these targets never drew one, and the next offscreen frame's
        // must not count a map of targets gone since (every still makes new ones; a photo keeps
        // its targets, but the live view's map may be the one remembered). Without this the
        // second still in a row from one camera skipped the map, and read its empty layer as a
        // cover over everything: with snow, rain or a sea on, it showed no snow on open ground.
        let live_snow = self.snow_drawn.take();
        if let Some(t) = &self.timer {
            t.paused.set(true);
        }
        // A still starts settled: no level carried over, no dissolve.
        let live_lods: Vec<Vec<(u8, u8, u8, bool)>> = self
            .items
            .iter_mut()
            .flatten()
            .map(|item| {
                item.clusters
                    .iter_mut()
                    .map(|c| {
                        let live = (c.lod, c.target, c.fade, c.to_far);
                        (c.lod, c.target, c.fade) = (LOD_UNSET, LOD_UNSET, 0);
                        live
                    })
                    .collect()
            })
            .collect();
        self.size = (width, height);
        self.quality = quality;
        (self.view, self.proj, self.eye) = (view, proj, eye);

        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.env.finish();
        self.render_part(&mut encoder, part);
        if let Some((buffer, stride)) = copy {
            copy_out(
                &mut encoder,
                &self.targets.output,
                buffer,
                stride,
                (width, height),
            );
        }

        let used_targets = std::mem::replace(&mut self.targets, live_targets);
        let used_pipelines = live_pipelines.map(|p| std::mem::replace(&mut self.pipelines, p));
        let used_sampled = live_sampled.map(|s| std::mem::replace(&mut self.sampled, s));
        (self.size, self.quality, self.view, self.proj, self.eye) = live;
        self.far_cache = live_cache;
        self.cascade_rendered = live_cascades;
        self.cascade_stale = live_stale;
        self.snow_drawn = live_snow;
        if let Some(t) = &self.timer {
            t.paused.set(false);
        }
        for (item, lods) in self.items.iter_mut().flatten().zip(live_lods) {
            for (c, live) in item.clusters.iter_mut().zip(lods) {
                (c.lod, c.target, c.fade, c.to_far) = live;
            }
        }

        (used_targets, (used_pipelines, used_sampled), encoder)
    }
}

/// Copies `texture` into `buffer` rows `stride` bytes apart.
fn copy_out(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    buffer: &wgpu::Buffer,
    stride: u32,
    (width, height): (u32, u32),
) {
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Where a photo's lens is focused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    /// At this distance along the view (m).
    Metres(f32),
    /// At the depth the first sample sees at this point of the picture (0..1 from the top
    /// left; 0.5, 0.5 the centre), read back from the GPU before the second sample.
    Auto([f32; 2]),
}

/// A photo's lens and shutter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// The aperture's radius (m); 0 a pinhole, everything sharp.
    pub radius_m: f32,
    pub focus: Focus,
    /// How long the shutter is open (s) from the picture's animation time: wind-blown plants,
    /// water and rain move over it (motion blur); 0 a frozen instant.
    pub shutter_s: f32,
}

impl Lens {
    /// A pinhole at an instant: no depth of field, no motion blur.
    pub const SHARP: Lens = Lens {
        radius_m: 0.0,
        focus: Focus::Metres(10.0),
        shutter_s: 0.0,
    };
}

/// The nearest and farthest distances an autofocus settles on (m).
pub const FOCUS_RANGE: (f32, f32) = (0.1, 1000.0);

/// The focus distance read back from the GPU.
enum FocusState {
    Known(f32),
    /// To be read after the first sample, at this point.
    Wanted([f32; 2]),
    Reading {
        buffer: wgpu::Buffer,
        mapped: Arc<Mutex<Option<Result<(), String>>>>,
    },
}

/// A photo: many frames of the same view, each sample's HDR image (with the air, before bloom
/// and the tone map) summed on the GPU, and the post chain run once on the mean. Each sample
/// has a sub-pixel shift (antialiasing), the sun's light direction moved within its disc
/// (shadows with true penumbrae that widen with the distance to what casts them), ambient
/// occlusion, light shafts and alpha to coverage turned by the sample (their noise averages
/// out); with a lens aperture the eye moved over it (depth of field, a thin lens), with a
/// shutter time the animation clock moved through it (motion blur). Sample 0 is the plain
/// frame, so one sample without a lens is the still. Nothing waits for a sample, so it runs on
/// the web too: `photo_finish` gives a [`Still`] to poll.
pub struct Photo {
    targets: Option<Targets>,
    sampled: Offscreen,
    quality: Quality,
    size: (u32, u32),
    /// The view, projection and eye the samples scatter round.
    camera: (Mat4, Mat4, Vec3),
    lens: Lens,
    focus: FocusState,
    /// The animation clock at the start (s): every sample is taken then (plus its moment of
    /// the shutter), however long the photo takes in a window that keeps drawing.
    clock: f64,
    /// The sum of the samples' HDR images, one `vec4<f32>` per pixel (photo.wgsl).
    sum: wgpu::Buffer,
    add_bind: wgpu::BindGroup,
    add_params: wgpu::Buffer,
    /// The HDR image's size (the render scale's, not the picture's).
    hdr_size: (u32, u32),
    /// Samples summed.
    pub done: u32,
}

/// The accumulate (compute) and develop (fullscreen) passes.
pub(crate) struct PhotoPipes {
    add_layout: wgpu::BindGroupLayout,
    develop_layout: wgpu::BindGroupLayout,
    add: wgpu::ComputePipeline,
    develop: wgpu::RenderPipeline,
}

impl PhotoPipes {
    fn new(device: &wgpu::Device) -> PhotoPipes {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photo"),
            source: wgpu::ShaderSource::Wgsl(include_str!("photo.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let storage = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let add_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo add"),
            entries: &[
                entry(
                    0,
                    compute,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(1, compute, storage(false)),
                entry(2, compute, uniform),
            ],
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let develop_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo develop"),
            entries: &[
                entry(3, fragment, storage(true)),
                entry(4, fragment, uniform),
            ],
        });
        let layout = |l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("photo"),
                bind_group_layouts: &[Some(l)],
                immediate_size: 0,
            })
        };
        let add = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("photo add"),
            layout: Some(&layout(&add_layout)),
            module: &module,
            entry_point: Some("add"),
            compilation_options: Default::default(),
            cache: None,
        });
        let develop = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photo develop"),
            layout: Some(&layout(&develop_layout)),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("develop"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::HDR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        PhotoPipes {
            add_layout,
            develop_layout,
            add,
            develop,
        }
    }
}

/// The `i`-th point of the base-`b` Halton sequence, in [0, 1).
fn halton(mut i: u32, b: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while i > 0 {
        f /= b as f32;
        r += f * (i % b) as f32;
        i /= b;
    }
    r
}

/// Sample `k`'s sub-pixel shift in pixels, in [-0.5, 0.5)^2: Halton (2, 3) moved by half a
/// period, so sample 0 sits on the pixel's centre (the still's own frame).
pub fn pixel_jitter(k: u32) -> (f32, f32) {
    let centred = |x: f32| (x + 0.5).fract() - 0.5;
    (centred(halton(k, 2)), centred(halton(k, 3)))
}

/// Sample `k`'s point on the unit disc of the lens: the R2 sequence (Roberts 2018, the
/// generalised golden ratio; independent of the pixel shift's Halton points) mapped to the disc
/// by area. Sample 0 is the centre.
pub fn lens_point(k: u32) -> (f32, f32) {
    const A1: f64 = 0.754_877_666_246_692_8;
    const A2: f64 = 0.569_840_290_998_053_2;
    let (u, v) = (
        (f64::from(k) * A1).fract() as f32,
        (f64::from(k) * A2).fract() as f32,
    );
    let (r, a) = (u.sqrt(), v * std::f32::consts::TAU);
    (r * a.cos(), r * a.sin())
}

/// The view and projection of a thin lens seen through the point `offset` (m, in the camera's
/// right and up) of its aperture, focused at `focus_m` along the view: the eye moves by the
/// offset and the frustum shears back so that everything at the focus distance lands where the
/// centre of the lens puts it (an off-axis frustum, not a turned camera, so the whole focal
/// plane is sharp, not only its centre). Returns the view, the projection and the moved eye.
pub fn lens_view(
    (view, proj, eye): (Mat4, Mat4, Vec3),
    offset: (f32, f32),
    focus_m: f32,
) -> (Mat4, Mat4, Vec3) {
    let (ox, oy) = offset;
    let view_lens = Mat4::from_translation(Vec3::new(-ox, -oy, 0.0)) * view;
    let shear = Vec3::new(
        proj.x_axis.x * ox / focus_m,
        proj.y_axis.y * oy / focus_m,
        0.0,
    );
    let inv = view.inverse();
    let eye_lens = eye + inv.x_axis.truncate() * ox + inv.y_axis.truncate() * oy;
    (view_lens, Mat4::from_translation(shear) * proj, eye_lens)
}

impl Renderer {
    /// Starts a photo at `width` x `height` with `quality` (its targets are kept for every
    /// sample) of the view `camera` through `lens`. Plants keep their finest levels (and
    /// leaf-cut shadows) twice as far as `quality` says: a still has no frame rate to keep.
    pub fn photo_begin(
        &mut self,
        width: u32,
        height: u32,
        mut quality: Quality,
        camera: (Mat4, Mat4, Vec3),
        lens: Lens,
    ) -> Result<Photo, String> {
        quality.lod_distance_percent *= 2;
        let quality = self.coverage.quality(quality.sanitized());
        if width == 0 || height == 0 || width > MAX_STILL.0 || height > MAX_STILL.1 {
            return Err(format!(
                "photo size {width}x{height} outside 1x1..={}x{}",
                MAX_STILL.0, MAX_STILL.1
            ));
        }
        let targets = Targets::new(
            &self.device,
            &self.layouts,
            self.output_format,
            quality,
            (width, height),
            &self.shadow_sampler,
            &self.post_sampler,
            &self.white,
            &self.physical.aerial,
            true,
        );
        let sampled = self.offscreen_pipelines(quality);
        let extent = targets.hdr_texture.size();
        let hdr_size = (extent.width, extent.height);
        let pipes = self
            .photo_pipes
            .get_or_insert_with(|| PhotoPipes::new(&self.device));
        // 16 bytes a pixel: 133 MB at 3840 x 2160, within WebGPU's default 128 MiB binding.
        let sum = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo sum"),
            size: u64::from(hdr_size.0) * u64::from(hdr_size.1) * 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let add_params = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo add"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let add_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo add"),
            layout: &pipes.add_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&targets.hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sum.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: add_params.as_entire_binding(),
                },
            ],
        });
        let focus = match lens.focus {
            Focus::Metres(m) => FocusState::Known(m.clamp(FOCUS_RANGE.0, FOCUS_RANGE.1)),
            Focus::Auto(p) => FocusState::Wanted(p),
        };
        Ok(Photo {
            targets: Some(targets),
            sampled,
            quality,
            size: (width, height),
            camera,
            lens,
            focus,
            clock: self.clock,
            sum,
            add_bind,
            add_params,
            hdr_size,
            done: 0,
        })
    }

    /// Renders the photo's next sample and adds it on the GPU. `Ok(false)`: not taken, the
    /// autofocus is still being read back from the first sample (call again on a later frame;
    /// natively [`Renderer::wait_gpu`] first).
    pub fn photo_sample(&mut self, photo: &mut Photo) -> Result<bool, String> {
        let k = photo.done;
        let lens_on = photo.lens.radius_m > 0.0 && !crate::shadow::is_orthographic(photo.camera.1);
        let focus_m = match photo.poll_focus()? {
            Some(f) => f,
            // Sample 0 is taken through the lens' centre: it needs no focus.
            None if k == 0 || !lens_on => FOCUS_RANGE.1,
            None => return Ok(false),
        };
        let (w, h) = photo.hdr_size;
        let (mut view, mut proj, mut eye) = photo.camera;
        if lens_on {
            let (u, v) = lens_point(k);
            let r = photo.lens.radius_m;
            (view, proj, eye) = lens_view((view, proj, eye), (u * r, v * r), focus_m);
        }
        // Sub-pixel shift: Halton (2, 3), centred.
        let (jx, jy) = pixel_jitter(k);
        let proj =
            Mat4::from_translation(Vec3::new(2.0 * jx / w as f32, 2.0 * jy / h as f32, 0.0)) * proj;
        // The sun's light from a point of its disc (0.27 degrees in radius), uniformly.
        let (u, v) = (halton(k, 5), halton(k, 7));
        let (r, a) = (u.sqrt() * 0.27f32.to_radians(), v * std::f32::consts::TAU);
        let d = self.sun.direction;
        let side = d.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = side.cross(d);
        self.shadow_jitter = (side * a.cos() + up * a.sin()) * r;
        // Ambient occlusion, shafts and coverage dithering turned per sample.
        self.photo_seed = k as f32;
        // The shutter: the animation clock somewhere in its time.
        let live_clock = self.clock;
        let at = photo.clock + f64::from(halton(k, 11) * photo.lens.shutter_s);
        let moved = at != live_clock;
        if moved {
            self.set_time(at);
        }
        // `render_offscreen` starts without cached shadows, so each sample casts its own.
        let targets = photo.targets.take().ok_or("photo targets missing")?;
        let sampled = std::mem::take(&mut photo.sampled);
        let (targets, sampled, mut encoder) = self.render_offscreen(
            targets,
            sampled,
            photo.quality,
            photo.size,
            (view, proj, eye),
            None,
            FramePart::Scene,
        );
        if moved {
            self.set_time(live_clock);
        }
        self.shadow_jitter = Vec3::ZERO;
        self.photo_seed = 0.0;
        // The next sample gathers its bounce light from this one.
        encoder.copy_texture_to_texture(
            targets.hdr_texture.as_image_copy(),
            targets.prev_hdr.as_image_copy(),
            targets.prev_hdr.size(),
        );
        self.queue.write_buffer(
            &photo.add_params,
            0,
            &[w, h, u32::from(k == 0), 0].map(u32::to_le_bytes).concat(),
        );
        if let Some(pipes) = &self.photo_pipes {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("photo add"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipes.add);
            pass.set_bind_group(0, &photo.add_bind, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        // The autofocus: the depth the first sample sees at the focus point.
        let probe = match photo.focus {
            FocusState::Wanted(p) if lens_on => {
                let (dw, dh) = (w, h);
                let x = ((p[0].clamp(0.0, 1.0) * dw as f32) as u32).min(dw - 1);
                let y = ((p[1].clamp(0.0, 1.0) * dh as f32) as u32).min(dh - 1);
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("photo focus"),
                    size: 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &targets.linear_depth_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: None,
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                Some(buffer)
            }
            _ => None,
        };
        photo.targets = Some(targets);
        photo.sampled = sampled;
        self.queue.submit([encoder.finish()]);
        if let Some(buffer) = probe {
            let mapped = Arc::new(Mutex::new(None));
            let slot = mapped.clone();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                if let Ok(mut s) = slot.lock() {
                    *s = Some(r.map_err(|e| e.to_string()));
                }
            });
            photo.focus = FocusState::Reading { buffer, mapped };
        }
        photo.done += 1;
        Ok(true)
    }

    /// Waits for the GPU's queued work (native only: the web cannot block).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait_gpu(&self) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(|e| format!("poll: {e}"))
    }

    /// The mean of the samples, through bloom, the tone map and the rest of the post chain
    /// from the photo's own view, read back as a [`Still`].
    pub fn photo_finish(&mut self, photo: Photo) -> Result<Still, String> {
        let Photo {
            targets,
            sampled,
            quality,
            size: (width, height),
            camera,
            sum,
            hdr_size,
            done,
            ..
        } = photo;
        let targets = targets.ok_or("photo targets missing")?;
        let pipes = self.photo_pipes.as_ref().ok_or("photo pipelines missing")?;
        let params = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo develop"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(
            &params,
            0,
            &[1.0 / done.max(1) as f32, hdr_size.0 as f32, 0.0, 0.0]
                .map(f32::to_le_bytes)
                .concat(),
        );
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo develop"),
            layout: &pipes.develop_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sum.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo readback"),
            size: stride as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (_, _, encoder) = self.render_offscreen(
            targets,
            sampled,
            quality,
            (width, height),
            camera,
            Some((&buffer, stride)),
            FramePart::Develop(&bind),
        );
        self.queue.submit([encoder.finish()]);
        let mapped = Arc::new(Mutex::new(None));
        let slot = mapped.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(r.map_err(|e| e.to_string()));
            }
        });
        Ok(Still {
            device: self.device.clone(),
            buffer,
            size: (width, height),
            stride,
            mapped,
            errors: Vec::new(),
        })
    }
}

/// The develop pass (`FramePart::Develop`): the photo's mean into the frame's HDR image.
pub(crate) fn develop(r: &Renderer, encoder: &mut wgpu::CommandEncoder, fill: &wgpu::BindGroup) {
    let Some(pipes) = &r.photo_pipes else {
        return;
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("photo develop"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.targets.hdr,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(&pipes.develop);
    pass.set_bind_group(0, fill, &[]);
    pass.draw(0..3, 0..1);
}

impl Photo {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The distance the lens is focused at (m), once known (an autofocus after the first
    /// sample).
    pub fn focus_m(&self) -> Option<f32> {
        match self.focus {
            FocusState::Known(f) => Some(f),
            _ => None,
        }
    }

    /// True while the autofocus is being read back (samples after the first wait for it).
    pub fn focusing(&self) -> bool {
        matches!(self.focus, FocusState::Reading { .. })
    }

    /// The focus distance once known; reads the autofocus' depth when it has arrived.
    fn poll_focus(&mut self) -> Result<Option<f32>, String> {
        if let FocusState::Reading { buffer, mapped } = &self.focus {
            let state = mapped.lock().ok().and_then(|mut s| s.take());
            match state {
                None => return Ok(None),
                Some(Err(e)) => return Err(format!("autofocus readback failed: {e}")),
                Some(Ok(())) => {
                    let depth = match buffer.slice(..).get_mapped_range() {
                        Ok(view) => f32::from_le_bytes([view[0], view[1], view[2], view[3]]),
                        Err(e) => return Err(format!("autofocus readback failed: {e}")),
                    };
                    buffer.unmap();
                    // The sky (or nothing) reads as far away: focus at the farthest.
                    let depth = if depth.is_finite() && depth > 0.0 {
                        depth
                    } else {
                        FOCUS_RANGE.1
                    };
                    self.focus = FocusState::Known(depth.clamp(FOCUS_RANGE.0, FOCUS_RANGE.1));
                }
            }
        }
        Ok(self.focus_m())
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec3, Vec4};

    #[test]
    fn halton_fills_the_unit_interval_evenly() {
        let xs: Vec<f32> = (1..=8).map(|i| super::halton(i, 2)).collect();
        assert_eq!(xs[..4], [0.5, 0.25, 0.75, 0.125]);
        let ys: Vec<f32> = (1..=4).map(|i| super::halton(i, 3)).collect();
        for (y, want) in ys.iter().zip([1.0 / 3.0, 2.0 / 3.0, 1.0 / 9.0, 4.0 / 9.0]) {
            assert!((y - want).abs() < 1e-6, "{ys:?}");
        }
        // Any 2^n first points of base 2 put one point in each interval of 1 / 2^n.
        let mut seen = [false; 16];
        for i in 0..16 {
            seen[(super::halton(i, 2) * 16.0) as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn sample_zero_is_the_plain_frame_and_the_shifts_stay_in_the_pixel() {
        assert_eq!(super::pixel_jitter(0), (0.0, 0.0));
        assert_eq!(super::lens_point(0), (0.0, 0.0));
        let n = 64;
        let (mut mx, mut my) = (0.0, 0.0);
        for k in 0..n {
            let (x, y) = super::pixel_jitter(k);
            assert!((-0.5..0.5).contains(&x) && (-0.5..0.5).contains(&y));
            (mx, my) = (mx + x / n as f32, my + y / n as f32);
            let (u, v) = super::lens_point(k);
            assert!(u * u + v * v <= 1.0 + 1e-6);
        }
        // Centred on the pixel: no shift of the whole picture.
        assert!(mx.abs() < 0.02 && my.abs() < 0.02, "{mx} {my}");
    }

    /// Where a camera-space point lands in normalised device coordinates.
    fn ndc(view: Mat4, proj: Mat4, p: Vec3) -> (f32, f32) {
        let c = proj * view * Vec4::from((p, 1.0));
        (c.x / c.w, c.y / c.w)
    }

    #[test]
    fn the_lens_keeps_the_focal_plane_sharp_and_blurs_by_the_thin_lens_rule() {
        let fov_y = 40f32.to_radians();
        let aspect = 1.5;
        let proj = Mat4::perspective_rh(fov_y, aspect, 0.1, 3000.0);
        let eye = Vec3::new(1.0, 1.6, 9.0);
        let view = Mat4::look_at_rh(eye, Vec3::new(1.0, 1.0, 0.0), Vec3::Y);
        let camera = (view, proj, eye);
        let (focus, radius) = (4.0, 0.012);
        let to_world = view.inverse();
        // A point straight ahead and one off to the side, on the focal plane: the same
        // wherever on the lens the eye sits.
        for (x, y) in [(0.0, 0.0), (1.1, -0.6)] {
            let p = to_world.transform_point3(Vec3::new(x, y, -focus));
            let at = ndc(view, proj, p);
            for k in 1..16 {
                let (u, v) = super::lens_point(k);
                let (lv, lp, _) = super::lens_view(camera, (u * radius, v * radius), focus);
                let seen = ndc(lv, lp, p);
                assert!((seen.0 - at.0).abs() < 1e-5 && (seen.1 - at.1).abs() < 1e-5);
            }
        }
        // A point at depth d moves by the circle of confusion: from the two edges of the lens
        // the image spreads over 2 R f |1/F - 1/d| on a sensor at f (the thin lens' A f |d - F|
        // / (d (F - f)) for an object far beyond the focal length).
        let sensor_half_height = 0.012; // a 24 mm high sensor; f follows from the field of view
        let f = sensor_half_height / (fov_y / 2.0).tan();
        for d in [2.0, 4.0, 8.0, 30.0] {
            let p = to_world.transform_point3(Vec3::new(0.0, 0.0, -d));
            let (lv0, lp0, _) = super::lens_view(camera, (0.0, -radius), focus);
            let (lv1, lp1, _) = super::lens_view(camera, (0.0, radius), focus);
            let spread_ndc = (ndc(lv1, lp1, p).1 - ndc(lv0, lp0, p).1).abs();
            let spread_m = spread_ndc * sensor_half_height;
            let coc = 2.0 * radius * f * (1.0 / focus - 1.0 / d).abs();
            assert!(
                (spread_m - coc).abs() < 1e-7 + coc * 1e-3,
                "{d}: {spread_m} vs {coc}"
            );
            let thin = 2.0 * radius * f * (d - focus).abs() / (d * (focus - f));
            assert!((spread_m - thin).abs() <= thin * f / focus * 1.01 + 1e-9);
        }
    }
}
