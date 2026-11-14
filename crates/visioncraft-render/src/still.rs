//! Offscreen stills: one frame at any size up to 3840x2160, always at
//! `Quality::HIGH`, read back as RGBA8, without touching the live view's
//! targets, quality, camera, cached shadow cascade or level-of-detail state.
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

use crate::{LOD_UNSET, Quality, Renderer, SampledPipelines, Targets};

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
        let quality = Quality::HIGH;
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
            false,
        );
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("still readback"),
            size: stride as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let sampled = (quality.msaa != self.quality.msaa).then(|| {
            SampledPipelines::new(
                &self.device,
                &self.layouts,
                &self.modules,
                quality.samples(),
            )
        });

        let (_, _, encoder) = self.render_offscreen(
            targets,
            sampled,
            quality,
            (width, height),
            (view, proj, eye),
            Some((&buffer, stride)),
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

    /// Renders one frame into `targets` (and the multisampled pipelines `sampled` when the
    /// quality's samples differ from the live ones) from `view`/`proj`/`eye`, copies the output
    /// into `buffer`, and puts the live state back. Returns the targets for another frame and
    /// the encoder to submit.
    #[allow(clippy::too_many_arguments)]
    fn render_offscreen(
        &mut self,
        targets: Targets,
        sampled: Option<SampledPipelines>,
        quality: Quality,
        (width, height): (u32, u32),
        (view, proj, eye): (Mat4, Mat4, Vec3),
        copy: Option<(&wgpu::Buffer, u32)>,
    ) -> (Targets, Option<SampledPipelines>, wgpu::CommandEncoder) {
        // Swap the live state out, render, swap it back.
        let live_targets = std::mem::replace(&mut self.targets, targets);
        let live_sampled = sampled.map(|s| std::mem::replace(&mut self.sampled, s));
        let live = (self.size, self.quality, self.view, self.proj, self.eye);
        let live_cache = self.far_cache.take();
        let live_cascades = std::mem::take(&mut self.cascade_rendered);
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
        self.render(&mut encoder);
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
        let used_sampled = live_sampled.map(|s| std::mem::replace(&mut self.sampled, s));
        (self.size, self.quality, self.view, self.proj, self.eye) = live;
        self.far_cache = live_cache;
        self.cascade_rendered = live_cascades;
        if let Some(t) = &self.timer {
            t.paused.set(false);
        }
        for (item, lods) in self.items.iter_mut().flatten().zip(live_lods) {
            for (c, live) in item.clusters.iter_mut().zip(lods) {
                (c.lod, c.target, c.fade, c.to_far) = live;
            }
        }

        (used_targets, used_sampled, encoder)
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

/// A photo: many frames of the same view averaged on the GPU, each with a sub-pixel shift
/// (antialiasing), the sun's light direction moved within its disc (shadows with true penumbrae
/// that widen with the distance to what casts them) and ambient occlusion sampled in other
/// directions; and, with a lens aperture, the eye moved over it (depth of field). Nothing waits
/// for a sample, so it runs on the web too: `photo_finish` gives a [`Still`] to poll.
pub struct Photo {
    targets: Option<Targets>,
    sampled: Option<SampledPipelines>,
    quality: Quality,
    size: (u32, u32),
    /// Linear RGB sums (16-bit float: 64 samples of values up to 1 keep about 0.05 %).
    accum: wgpu::Texture,
    /// Samples summed.
    pub done: u32,
}

/// The accumulate and resolve passes: one fullscreen triangle reading a texture and scaling it.
pub(crate) struct PhotoPipes {
    layout: wgpu::BindGroupLayout,
    add: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
}

const ACCUM: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const RESOLVED: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl PhotoPipes {
    fn new(device: &wgpu::Device) -> PhotoPipes {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photo"),
            source: wgpu::ShaderSource::Wgsl(include_str!("photo.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photo"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipe = |format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("photo"),
                layout: Some(&pl),
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
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let one = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        PhotoPipes {
            add: pipe(
                ACCUM,
                Some(wgpu::BlendState {
                    color: one,
                    alpha: one,
                }),
            ),
            resolve: pipe(RESOLVED, None),
            layout,
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

impl Renderer {
    /// Starts a photo at `width` x `height` with `quality` (its targets are kept for every
    /// sample). Plants keep their finest levels (and leaf-cut shadows) twice as far as `quality`
    /// says: a still has no frame rate to keep.
    pub fn photo_begin(
        &mut self,
        width: u32,
        height: u32,
        mut quality: Quality,
    ) -> Result<Photo, String> {
        quality.lod_distance_percent *= 2;
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
            true,
        );
        let sampled = (quality.msaa != self.quality.msaa).then(|| {
            SampledPipelines::new(
                &self.device,
                &self.layouts,
                &self.modules,
                quality.samples(),
            )
        });
        let accum = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo sum"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ACCUM,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Ok(Photo {
            targets: Some(targets),
            sampled,
            quality,
            size: (width, height),
            accum,
            done: 0,
        })
    }

    /// Draws `src` scaled by `scale` into `dst`: into the sum (`Some(first)`: cleared first),
    /// or (`None`) into the sRGB picture.
    fn photo_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        scale: f32,
        sum: Option<bool>,
    ) {
        let pipes = self
            .photo_pipes
            .get_or_insert_with(|| PhotoPipes::new(&self.device));
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo scale"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(
            &uniform,
            0,
            &[scale, 0.0, 0.0, 0.0].map(f32::to_le_bytes).concat(),
        );
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo"),
            layout: &pipes.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("photo"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if sum == Some(false) {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(if sum.is_some() {
            &pipes.add
        } else {
            &pipes.resolve
        });
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Renders sample `k` of the photo from `view`/`proj`/`eye` and adds it on the GPU.
    /// `aperture_m` (0: none) is the lens' radius, focused at `focus_m` along the view.
    #[allow(clippy::too_many_arguments)]
    pub fn photo_sample(
        &mut self,
        photo: &mut Photo,
        view: Mat4,
        proj: Mat4,
        eye: Vec3,
        k: u32,
        aperture_m: f32,
        focus_m: f32,
    ) -> Result<(), String> {
        let (w, h) = photo.size;
        // Sub-pixel shift: Halton (2, 3), centred.
        let (jx, jy) = (halton(k + 1, 2) - 0.5, halton(k + 1, 3) - 0.5);
        let proj =
            Mat4::from_translation(Vec3::new(2.0 * jx / w as f32, 2.0 * jy / h as f32, 0.0)) * proj;
        // The sun's light from a point of its disc (0.27 degrees in radius), uniformly.
        let (u, v) = (halton(k + 1, 5), halton(k + 1, 7));
        let (r, a) = (u.sqrt() * 0.27f32.to_radians(), v * std::f32::consts::TAU);
        let d = self.sun.direction;
        let side = d.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = side.cross(d);
        self.shadow_jitter = (side * a.cos() + up * a.sin()) * r;
        // Ambient occlusion turned per sample.
        self.photo_seed = k as f32;
        // The eye over the lens, looking at the same focus point.
        let view = if aperture_m > 0.0 {
            let (lu, lv) = (halton(k + 1, 11), halton(k + 1, 13));
            let (lr, la) = (lu.sqrt() * aperture_m, lv * std::f32::consts::TAU);
            let inv = view.inverse();
            let (right, upv, fwd) = (
                inv.x_axis.truncate(),
                inv.y_axis.truncate(),
                -inv.z_axis.truncate(),
            );
            let focus = eye + fwd * focus_m;
            let e = eye + right * (lr * la.cos()) + upv * (lr * la.sin());
            Mat4::look_at_rh(e, focus, upv)
        } else {
            view
        };
        // `render_offscreen` starts without cached shadows, so each sample casts its own.
        let targets = photo.targets.take().ok_or("photo targets missing")?;
        let sampled = photo.sampled.take();
        let (targets, sampled, mut encoder) = self.render_offscreen(
            targets,
            sampled,
            photo.quality,
            photo.size,
            (view, proj, eye),
            None,
        );
        self.shadow_jitter = Vec3::ZERO;
        self.photo_seed = 0.0;
        // The next sample gathers its bounce light from this one.
        encoder.copy_texture_to_texture(
            targets.hdr_texture.as_image_copy(),
            targets.prev_hdr.as_image_copy(),
            targets.prev_hdr.size(),
        );
        let src = targets.output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.output_format.add_srgb_suffix()),
            ..Default::default()
        });
        let dst = photo.accum.create_view(&Default::default());
        self.photo_pass(&mut encoder, &src, &dst, 1.0, Some(photo.done == 0));
        photo.targets = Some(targets);
        photo.sampled = sampled;
        self.queue.submit([encoder.finish()]);
        photo.done += 1;
        Ok(())
    }

    /// The average of the samples so far, read back as a [`Still`].
    pub fn photo_finish(&mut self, photo: &Photo) -> Still {
        let (width, height) = photo.size;
        let resolved = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RESOLVED,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo readback"),
            size: stride as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let src = photo.accum.create_view(&Default::default());
        let dst = resolved.create_view(&Default::default());
        self.photo_pass(
            &mut encoder,
            &src,
            &dst,
            1.0 / photo.done.max(1) as f32,
            None,
        );
        copy_out(&mut encoder, &resolved, &buffer, stride, (width, height));
        self.queue.submit([encoder.finish()]);
        let mapped = Arc::new(Mutex::new(None));
        let slot = mapped.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(r.map_err(|e| e.to_string()));
            }
        });
        Still {
            device: self.device.clone(),
            buffer,
            size: (width, height),
            stride,
            mapped,
            errors: Vec::new(),
        }
    }
}

impl Photo {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn halton_fills_the_unit_interval_evenly() {
        let xs: Vec<f32> = (1..=8).map(|i| super::halton(i, 2)).collect();
        assert_eq!(xs[..4], [0.5, 0.25, 0.75, 0.125]);
    }
}
