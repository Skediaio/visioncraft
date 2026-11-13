// Ground plane and instanced cubes into the HDR target. Instance 0 is the
// ground (grey checker); the rest are cubes coloured by a hash of their index.
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>, // xyz: direction towards the light, w: ambient
}
@group(0) @binding(0) var<uniform> g: Globals;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @builtin(instance_index) instance: u32,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world: vec3<f32>,
    @location(2) @interpolate(flat) instance: u32,
}

fn hash(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

@vertex
fn vs_main(v: Vertex) -> Varying {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    var out: Varying;
    out.clip = g.view_proj * world;
    // Right for axis-aligned scales, which is all the spike uses.
    out.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    out.world = world.xyz;
    out.instance = v.instance;
    return out;
}

@fragment
fn fs_main(in: Varying) -> @location(0) vec4<f32> {
    var albedo: vec3<f32>;
    if in.instance == 0u {
        let c = (i32(floor(in.world.x / 4.0)) + i32(floor(in.world.z / 4.0))) & 1;
        albedo = vec3<f32>(0.32 + 0.12 * f32(c));
    } else {
        let h = hash(in.instance);
        albedo = 0.25 + 0.6 * vec3<f32>(
            f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u)) / 255.0;
    }
    let lambert = max(dot(normalize(in.normal), g.light_dir.xyz), 0.0);
    return vec4<f32>(albedo * (g.light_dir.w + 1.4 * lambert), 1.0);
}
