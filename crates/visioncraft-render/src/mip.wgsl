// Mip chain generation for material textures: each level is the bilinear
// average of the level above it (a 2x2 box for even sizes), drawn as a
// fullscreen triangle into one mip of the same texture. sRGB textures are
// filtered in linear space because the views have sRGB formats.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Out(vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // Destination texel centre in normalised coordinates of the next level.
    let size = vec2<f32>(max(textureDimensions(source) / 2u, vec2<u32>(1u)));
    return textureSampleLevel(source, linear_sampler, in.clip.xy / size, 0.0);
}
