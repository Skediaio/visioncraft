// Photo mode (still.rs): adds a sample into the 16-bit float sum (blend One, One) or writes
// the sum times 1 / samples into the sRGB picture. `src` is read texel for texel.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> scale: vec4<f32>;

struct Out {
    @builtin(position) pos: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(in.pos.xy), 0);
    return vec4<f32>(c.rgb * scale.x, 1.0);
}
