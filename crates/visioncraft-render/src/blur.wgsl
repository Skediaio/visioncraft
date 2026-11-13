// Separable depth-aware (bilateral) blur of the AO / contact-shadow target:
// Gaussian spatial weights times a depth-similarity weight, so occlusion does
// not bleed across silhouettes (the "bilateral filter" of Tomasi & Manduchi,
// ICCV 1998, as used for SSAO in the SAO paper above). Run once horizontally,
// once vertically.

override HORIZONTAL: bool = true;

@group(1) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(1) var depth_tex: texture_2d<f32>;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    return Out(fullscreen(i));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // Half resolution (g.options.z = 2): texel q stands for full-res pixel 2q.
    let div = i32(g.options.z);
    let p = vec2<i32>(in.clip.xy);
    let size = vec2<i32>(textureDimensions(source));
    let full = vec2<i32>(g.viewport.xy) - 1;
    let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), HORIZONTAL);
    let center = textureLoad(depth_tex, min(p * div, full), 0).r;
    if center >= SKY_DEPTH * 0.5 {
        return vec4<f32>(1.0);
    }
    let tolerance = 0.02 * center + 0.02;
    var sum = vec2<f32>(0.0);
    var weight = 0.0;
    for (var i = -4; i <= 4; i++) {
        let q = clamp(p + step * i, vec2<i32>(0), size - 1);
        let d = textureLoad(depth_tex, min(q * div, full), 0).r;
        let w = exp(-f32(i * i) / 8.0) * max(0.0, 1.0 - abs(d - center) / tolerance);
        sum += textureLoad(source, q, 0).rg * w;
        weight += w;
    }
    return vec4<f32>(sum / max(weight, 1e-4), 1.0, 1.0);
}
