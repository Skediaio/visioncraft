// Depth buffer -> linear view depth (metres, R32Float, one sample per pixel)
// for the screen-space passes and the overlay's depth test. With MSAA this
// takes sample 0, so screen-space effects are evaluated once per pixel.

@group(1) @binding(0) var depth_ms: texture_depth_multisampled_2d;
@group(1) @binding(1) var depth_1x: texture_depth_2d;

fn linearize(d: f32) -> f32 {
    if d >= 1.0 {
        return SKY_DEPTH;
    }
    return linear_depth_of(d);
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
