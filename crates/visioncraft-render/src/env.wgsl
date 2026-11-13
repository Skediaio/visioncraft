// Environment cube for specular ambient: the analytic sky above the horizon
// and a flat ground radiance below it, prefiltered per mip for one GGX
// roughness (split-sum approximation: Karis, "Real Shading in Unreal Engine
// 4", SIGGRAPH 2013 course notes, with its N = V = R assumption and its
// importance-sampled GGX, NoL-weighted). Each mip is integrated straight from
// the analytic radiance (no source cube): the sky has no sun disc here (the
// sun's specular is analytic in scene.wgsl), so it is smooth enough for a
// fixed Hammersley set. Output is radiance in kcd/m2 without exposure;
// scene.wgsl scales it, so changing the exposure never re-renders the cube.
// Module: common.wgsl + sky.wgsl + this file.

struct EnvFace {
    info: vec4<f32>,   // x face index, y roughness, z face size (texels), w sample count
    ground: vec4<f32>, // rgb radiance below the horizon (kcd/m2)
    night: vec4<f32>,  // rgb night sky radiance (kcd/m2)
}
@group(1) @binding(0) var<uniform> env_face: EnvFace;

// Direction through a cube face texel; the face orientation table of the
// Vulkan / WebGPU cube map convention (major axis, sc, tc), so sampling the
// cube with this direction returns this texel.
fn face_direction(face: u32, st: vec2<f32>) -> vec3<f32> {
    let s = st.x;
    let t = st.y;
    switch face {
        case 0u: { return vec3<f32>(1.0, -t, -s); }
        case 1u: { return vec3<f32>(-1.0, -t, s); }
        case 2u: { return vec3<f32>(s, 1.0, t); }
        case 3u: { return vec3<f32>(s, -1.0, -t); }
        case 4u: { return vec3<f32>(s, -t, 1.0); }
        default: { return vec3<f32>(-s, -t, -1.0); }
    }
}

fn env_radiance(dir: vec3<f32>) -> vec3<f32> {
    if dir.y < 0.0 {
        return env_face.ground.rgb;
    }
    return sky_eval(dir, 1.0, env_face.night.rgb);
}

struct EnvOut {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_env(@builtin(vertex_index) i: u32) -> EnvOut {
    return EnvOut(fullscreen(i));
}

@fragment
fn fs_env(in: EnvOut) -> @location(0) vec4<f32> {
    let st = in.clip.xy / env_face.info.z * 2.0 - 1.0;
    let n = normalize(face_direction(u32(env_face.info.x), st));
    let roughness = env_face.info.y;
    if roughness <= 0.0 {
        return vec4<f32>(env_radiance(n), 1.0);
    }
    let a = roughness * roughness;
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.999);
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    let count = u32(env_face.info.w);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0u; i < count; i++) {
        // Hammersley point set (Karis 2013, Hammersley via bit reversal).
        let xi = vec2<f32>((f32(i) + 0.5) / f32(count), f32(reverseBits(i)) * 2.3283064365386963e-10);
        let phi = 2.0 * PI * xi.x;
        let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
        let sin_theta = sqrt(1.0 - cos_theta * cos_theta);
        let h = tx * (sin_theta * cos(phi)) + ty * (sin_theta * sin(phi)) + n * cos_theta;
        let l = 2.0 * dot(n, h) * h - n;
        let nol = dot(n, l);
        if nol > 0.0 {
            sum += env_radiance(l) * nol;
            weight += nol;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-6), 1.0);
}
