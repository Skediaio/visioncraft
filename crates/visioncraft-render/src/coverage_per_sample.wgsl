// The MSAA depth pre-pass's coverage stages run per sample, for Vulkan (`coverage.rs`).
//
// naga 30 writes a `@builtin(sample_mask)` fragment output into SPIR-V as a scalar Output
// variable; Vulkan requires an array (VUID-SampleMask-SampleMask-04359). Mesa takes it, Intel's
// and AMD's Vulkan drivers on Windows crash at pipeline creation. So on Vulkan the three stages
// that write a mask (`fs_depth_cutout`, `fs_impostor_depth_ms`, `fs_depth_fade_ms`) are replaced
// by these: taking `@builtin(sample_index)` makes the stage run once per sample; each sample
// computes the same mask as the masked stage, from the pixel's centre, and keeps itself only
// when its bit is set. The depth written per sample is the rasterizer's either way.
//
// Under per-sample shading the fragment position is the sample's, so it is moved back to the
// pixel's centre (what the masked stage sees) before the per-pixel noise reads it: the same
// samples are kept as with the mask. The other inputs keep their default interpolation; a
// driver may evaluate them at the sample rather than the centre, which shifts the alpha by
// less than a pixel's ramp at a cutout edge.
//
// Appended after `impostor.wgsl` in the scene module, and only on a device that shades per
// sample (`DownlevelFlags::MULTISAMPLED_SHADING`): naga refuses `sample_index` elsewhere.

fn pixel_centre(clip: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(floor(clip.xy) + 0.5, clip.zw);
}

fn keeps_sample(mask: u32, sample: u32) -> bool {
    return ((mask >> sample) & 1u) != 0u;
}

@fragment
fn fs_depth_cutout_per_sample(in: DepthOut, @builtin(sample_index) sample: u32) {
    var at = in;
    at.clip = pixel_centre(in.clip);
    if !keeps_sample(depth_cutout_mask(at), sample) {
        discard;
    }
}

@fragment
fn fs_impostor_depth_per_sample(in: ImpostorVarying, @builtin(sample_index) sample: u32) {
    var at = in;
    at.clip = pixel_centre(in.clip);
    if !keeps_sample(impostor_depth_mask(at), sample) {
        discard;
    }
}

@fragment
fn fs_depth_fade_per_sample(in: FadeOut, @builtin(sample_index) sample: u32) {
    var at = in;
    at.clip = pixel_centre(in.clip);
    if !keeps_sample(depth_fade_mask(at), sample) {
        discard;
    }
}
