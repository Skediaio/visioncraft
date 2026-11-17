//! How the MSAA depth pre-pass writes the coverage of cutout foliage, soft area edges and the
//! dithered hand-over between a plant's mesh and its impostor.
//!
//! Three stages decide per pixel which of its samples a fragment covers and write that as
//! `@builtin(sample_mask)`: `fs_depth_cutout` (scene.wgsl), `fs_impostor_depth_ms` and
//! `fs_depth_fade_ms` (impostor.wgsl). naga 30 writes that output into SPIR-V as a scalar
//! `Output` variable decorated `BuiltIn SampleMask`, where Vulkan requires an array of 32-bit
//! integers (`spirv-val`: VUID-SampleMask-SampleMask-04359). Mesa takes the invalid module;
//! Intel's and AMD's Vulkan drivers on Windows crash while they build the pipeline (found by
//! 3dcraft, which shares this stack). So:
//!
//! - [`Coverage::Masked`]: every backend but Vulkan (WebGPU, Metal, DirectX 12, OpenGL) keeps
//!   the masked stages unchanged.
//! - [`Coverage::PerSample`]: on Vulkan the same masks are computed by stages that run once
//!   per sample (`coverage_per_sample.wgsl`, `@builtin(sample_index)`), each sample keeping or
//!   discarding itself. No sample-mask output at all.
//! - [`Coverage::OneSample`]: a Vulkan device that cannot shade per sample
//!   (`DownlevelFlags::MULTISAMPLED_SHADING`) draws without MSAA, so no multisampled coverage
//!   stage is needed.

/// The per-sample stages, appended to the scene module where they are used.
pub(crate) const PER_SAMPLE_SOURCE: &str = include_str!("coverage_per_sample.wgsl");

/// The masked stages and the per-sample stages that stand in for them on Vulkan.
pub(crate) const STAGES: [(&str, &str); 3] = [
    ("fs_depth_cutout", "fs_depth_cutout_per_sample"),
    ("fs_impostor_depth_ms", "fs_impostor_depth_per_sample"),
    ("fs_depth_fade_ms", "fs_depth_fade_per_sample"),
];

/// A stage that only takes `sample_index`: it validates where the device shades per sample.
const PROBE: &str = "@fragment
fn probe(@builtin(sample_index) sample: u32) -> @location(0) vec4<f32> {
    return vec4<f32>(f32(sample));
}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coverage {
    /// The stages write `@builtin(sample_mask)`.
    Masked,
    /// The stages run per sample and discard the samples outside the mask (Vulkan).
    PerSample,
    /// No MSAA (Vulkan without per-sample shading).
    OneSample,
}

impl Coverage {
    pub(crate) fn choose(backend: wgpu::Backend, shades_per_sample: bool) -> Coverage {
        match backend {
            wgpu::Backend::Vulkan if shades_per_sample => Coverage::PerSample,
            wgpu::Backend::Vulkan => Coverage::OneSample,
            _ => Coverage::Masked,
        }
    }

    /// The form for `device`. The device does not report its downlevel flags, so on Vulkan a
    /// tiny module that takes `sample_index` is made inside an error scope: wgpu validates it
    /// with the device's capabilities (naga's `MULTISAMPLED_SHADING`), synchronously on native
    /// backends.
    pub(crate) fn of(device: &wgpu::Device) -> Coverage {
        let backend = device.adapter_info().backend;
        if backend != wgpu::Backend::Vulkan {
            return Coverage::Masked;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _probe = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("per-sample shading probe"),
            source: wgpu::ShaderSource::Wgsl(PROBE.into()),
        });
        // Native error scopes resolve at once; one poll does. Not ready counts as refused: the
        // safe side (no MSAA) rather than invalid SPIR-V.
        let mut pop = std::pin::pin!(scope.pop());
        let polled = std::future::Future::poll(
            pop.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        );
        let shades_per_sample = matches!(polled, std::task::Poll::Ready(None));
        Coverage::choose(backend, shades_per_sample)
    }

    /// The entry point that draws `masked` (a name in [`STAGES`]) in this form.
    pub(crate) fn stage(self, masked: &'static str) -> &'static str {
        match self {
            Coverage::PerSample => STAGES
                .iter()
                .find(|(m, _)| *m == masked)
                .map_or(masked, |(_, per_sample)| per_sample),
            Coverage::Masked | Coverage::OneSample => masked,
        }
    }

    /// `quality` as this form can draw it: without MSAA for [`Coverage::OneSample`].
    pub(crate) fn quality(self, quality: crate::Quality) -> crate::Quality {
        crate::Quality {
            msaa: quality.msaa && self != Coverage::OneSample,
            ..quality
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vulkan_alone_leaves_the_masked_stages() {
        use wgpu::Backend::*;
        assert_eq!(Coverage::choose(Vulkan, true), Coverage::PerSample);
        assert_eq!(Coverage::choose(Vulkan, false), Coverage::OneSample);
        for backend in [Metal, Dx12, Gl, BrowserWebGpu, Noop] {
            for shades in [true, false] {
                assert_eq!(Coverage::choose(backend, shades), Coverage::Masked);
            }
        }
        for (masked, per_sample) in STAGES {
            assert_eq!(Coverage::Masked.stage(masked), masked);
            assert_eq!(Coverage::OneSample.stage(masked), masked);
            assert_eq!(Coverage::PerSample.stage(masked), per_sample);
        }
        assert!(!Coverage::OneSample.quality(crate::Quality::HIGH).msaa);
        assert!(Coverage::PerSample.quality(crate::Quality::HIGH).msaa);
    }
}
