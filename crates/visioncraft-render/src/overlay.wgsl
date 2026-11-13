// Overlay lines and points, drawn on the tone-mapped output (full output
// resolution, after any upscale). Each instance is
// one segment (two LineVertex records), expanded to a screen-space quad with
// round caps and a one-pixel anti-aliased edge; a segment with equal ends is
// a round point. Depth-tested by hand against the linear scene depth, so it
// works whether or not the scene pass was multisampled.

@group(1) @binding(0) var depth_tex: texture_2d<f32>;

struct Segment {
    @location(0) a_pos: vec3<f32>,
    @location(1) a_color: vec4<f32>, // sRGB-encoded RGBA (unorm8x4)
    @location(2) a_width: f32,       // pixels
    @location(3) b_pos: vec3<f32>,
    @location(4) b_color: vec4<f32>,
    @location(5) b_width: f32,
}

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(linear) local: vec2<f32>, // pixels: along the segment, across it
    @location(1) @interpolate(flat) extent: vec2<f32>,  // segment length, half width (pixels)
    @location(2) color: vec4<f32>,
    @location(3) view_depth: f32,
    @location(4) @interpolate(flat) on_top: u32,
}

fn to_pixels(c: vec4<f32>) -> vec2<f32> {
    return c.xy / c.w * 0.5 * g.output.xy;
}

@vertex
fn vs_main(s: Segment, @builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
    var ca = g.view_proj * vec4<f32>(s.a_pos, 1.0);
    var cb = g.view_proj * vec4<f32>(s.b_pos, 1.0);
    var out: Out;
    // Clip against the near plane (z = 0 in WebGPU clip space).
    if ca.z < 0.0 && cb.z < 0.0 {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0); // outside: degenerate
        return out;
    }
    if ca.z < 0.0 {
        ca = mix(ca, cb, ca.z / (ca.z - cb.z));
    } else if cb.z < 0.0 {
        cb = mix(cb, ca, cb.z / (cb.z - ca.z));
    }
    let pa = to_pixels(ca);
    let pb = to_pixels(cb);
    let delta = pb - pa;
    let len = length(delta);
    let dir = select(vec2<f32>(1.0, 0.0), delta / len, len > 1e-4);
    let normal = vec2<f32>(-dir.y, dir.x);

    // Corners: (end, side) for two triangles.
    var ends = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    var sides = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let end = ends[vi];
    let side = sides[vi];
    let half_width = max(mix(s.a_width, s.b_width, end), 1.0) * 0.5;
    let reach = half_width + 1.0; // room for the anti-aliased edge
    let along = mix(-reach, len + reach, end);
    let p = pa + dir * along + normal * side * reach;
    let c = select(ca, cb, end > 0.5);

    out.clip = vec4<f32>(p / (0.5 * g.output.xy) * c.w, c.z, c.w);
    out.local = vec2<f32>(along, side * reach);
    out.extent = vec2<f32>(len, half_width);
    out.color = mix(s.a_color, s.b_color, end);
    out.view_depth = linear_depth_of(c.z / c.w);
    out.on_top = u32(ii >= g.highlight.z);
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    if in.on_top == 0u {
        // The scene may render at a lower resolution than the output.
        let at = min(vec2<i32>(in.clip.xy * g.output.z), vec2<i32>(g.viewport.xy) - 1);
        let scene = textureLoad(depth_tex, at, 0).r;
        // Tolerance lets lines lying on a surface (grid on the ground) win.
        if in.view_depth > scene * 1.002 + 0.01 {
            discard;
        }
    }
    let x = in.local.x - clamp(in.local.x, 0.0, in.extent.x);
    let distance = length(vec2<f32>(x, in.local.y));
    let coverage = clamp(in.extent.y + 0.5 - distance, 0.0, 1.0);
    let c = in.color;
    let linear = select(pow((c.rgb + 0.055) / 1.055, vec3<f32>(2.4)), c.rgb / 12.92, c.rgb <= vec3<f32>(0.04045));
    return vec4<f32>(linear, c.a * coverage);
}
