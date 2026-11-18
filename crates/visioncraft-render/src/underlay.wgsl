// Plan images under the drawing: textured quads drawn flat, unlit and alpha-blended onto the
// tone-mapped output (so the image shows in its own colours), after the picture is made and
// before the overlay lines. They are depth-tested by hand against the linear scene depth like the
// overlay is, so a wall or a bed in front of the plan hides it; the tolerance lets the plan win
// over the ground it lies a few centimetres above. No shadow, no lighting, never picked.

@group(1) @binding(0) var depth_tex: texture_2d<f32>;
@group(2) @binding(0) var plan_tex: texture_2d<f32>;
@group(2) @binding(1) var plan_sampler: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
}

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) view_depth: f32,
}

@vertex
fn vs_main(v: Vertex) -> Out {
    var out: Out;
    let c = g.view_proj * vec4<f32>(v.position, 1.0);
    out.clip = c;
    out.uv = v.uv;
    out.opacity = v.opacity;
    out.view_depth = linear_depth_of(c.z / c.w);
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // Sampled before any discard, so the derivatives stay in uniform control flow.
    let colour = textureSample(plan_tex, plan_sampler, in.uv);
    // The scene may render at a lower resolution than the output.
    let at = min(vec2<i32>(in.clip.xy * g.output.z), vec2<i32>(g.viewport.xy) - 1);
    let scene = textureLoad(depth_tex, at, 0).r;
    if in.view_depth > scene * 1.001 + 0.005 {
        discard;
    }
    return vec4<f32>(colour.rgb, colour.a * in.opacity);
}
