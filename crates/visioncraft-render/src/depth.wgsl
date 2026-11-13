// Depth buffer -> linear view depth (metres, R32Float, one sample per pixel)
// for the screen-space passes and the overlay's depth test. With MSAA this
// takes sample 0, so screen-space effects are evaluated once per pixel.

@group(1) @binding(0) var depth_ms: texture_depth_multisampled_2d;
@group(1) @binding(1) var depth_1x: texture_depth_2d;

fn linearize(d: f32) -> f32 {
    if d >= 1.0 {
        return SKY_DEPTH;
    }
    // Only the projection's z and w rows matter: solve
    // d = (P22 z + P32) / (P23 z + P33) for view z (P[column][row]).
    let p22 = g.proj[2][2];
    let p32 = g.proj[3][2];
    let p23 = g.proj[2][3];
    let p33 = g.proj[3][3];
    return -(p32 - d * p33) / (d * p23 - p22);
}

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    return Out(fullscreen(i));
}

@fragment
fn fs_resolve_ms(in: Out) -> @location(0) vec4<f32> {
    let d = textureLoad(depth_ms, vec2<i32>(in.clip.xy), 0);
    return vec4<f32>(linearize(d), 0.0, 0.0, 0.0);
}

@fragment
fn fs_resolve_1x(in: Out) -> @location(0) vec4<f32> {
    let d = textureLoad(depth_1x, vec2<i32>(in.clip.xy), 0);
    return vec4<f32>(linearize(d), 0.0, 0.0, 0.0);
}
