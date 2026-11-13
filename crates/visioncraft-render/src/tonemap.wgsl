// HDR -> display: fullscreen triangle, ACES fit (Narkowicz). The target is an
// sRGB format, so the hardware does the gamma encode.
@group(0) @binding(0) var hdr: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(hdr, vec2<i32>(pos.xy), 0).rgb;
    let m = clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(m, 1.0);
}
