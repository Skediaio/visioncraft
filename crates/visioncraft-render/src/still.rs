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
        encoder.copy_texture_to_buffer(
            self.targets.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
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

        self.targets = live_targets;
        if let Some(s) = live_sampled {
            self.sampled = s;
        }
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
}
