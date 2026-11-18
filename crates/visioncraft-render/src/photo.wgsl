// Photo mode (still.rs): each sample's HDR image, before bloom and the tone map, is added into
// a sum of 32-bit floats (a storage buffer, one vec4 per pixel: WebGPU cannot blend into
// rgba32float, and 16-bit sums of 1024 samples of a bright sky would overflow or lose the last
// samples); once the samples are in, `develop` writes their mean into the HDR image for the
// post chain to run on once.

@group(0) @binding(0) var sample_src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> sum: array<vec4<f32>>;
// x width, y height, z 1 for the first sample (it replaces the sum)
@group(0) @binding(2) var<uniform> add_params: vec4<u32>;

@compute @workgroup_size(8, 8)
fn add(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= add_params.x || id.y >= add_params.y {
        return;
    }
    let c = textureLoad(sample_src, vec2<i32>(id.xy), 0);
    let i = id.y * add_params.x + id.x;
    if add_params.z == 1u {
        sum[i] = c;
    } else {
        sum[i] = sum[i] + c;
    }
}

@group(0) @binding(3) var<storage, read> total: array<vec4<f32>>;
// x 1 / samples, y width
@group(0) @binding(4) var<uniform> develop_params: vec4<f32>;

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
fn develop(in: Out) -> @location(0) vec4<f32> {
    let p = vec2<u32>(in.pos.xy);
    return total[p.y * u32(develop_params.y) + p.x] * develop_params.x;
}
