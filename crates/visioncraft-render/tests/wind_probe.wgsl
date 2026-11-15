// Test-only compute entry: evaluates common.wgsl's wind_sway and impostor_sway for a list of
// points, so tests/wind.rs can compare the shader with the CPU mirror in src/wind.rs. Vertex
// shaders cannot write storage; a compute shader can, and runs on software Vulkan.
// Module: common.wgsl + this file.

struct Point {
    rel: vec4<f32>,    // xyz: from the plant's origin; w: bend weight (mesh) or height share (impostor)
    origin: vec4<f32>, // xyz: the plant's origin; w: limb phase
    extra: vec4<f32>,  // x: sway frequency (Hz); y: 1 for an impostor point
}

@group(0) @binding(40) var<storage, read> points: array<Point>;
@group(0) @binding(41) var<storage, read_write> displaced: array<vec4<f32>>;
@group(0) @binding(42) var<uniform> wind_in: array<vec4<f32>, 3>;

@compute @workgroup_size(64)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&points) {
        return;
    }
    let p = points[id.x];
    let ws = WindState(wind_in[0], wind_in[1], wind_in[2]);
    var q: vec3<f32>;
    if p.extra.y > 0.5 {
        q = impostor_sway(ws, p.rel.xyz, p.origin.xyz, p.rel.w, p.extra.x);
    } else {
        q = wind_sway(ws, p.rel.xyz, p.origin.xyz, p.rel.w, p.origin.w, p.extra.x);
    }
    displaced[id.x] = vec4<f32>(q, 0.0);
}
