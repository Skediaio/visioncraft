// Environment cube for specular ambient: the sky source of sky.wgsl above
// the horizon and the ground below it, prefiltered per mip for one GGX
// roughness (split-sum approximation: Karis, "Real Shading in Unreal Engine
// 4", SIGGRAPH 2013 course notes, with its N = V = R assumption and its
// importance-sampled GGX, NoL-weighted). The analytic skies are smooth
// enough to sample directly with a fixed Hammersley set; the HDRI is read
// from its mip chain at the level that matches each sample's solid angle
// (filtered importance sampling: Krivanek & Colbert, "Real-time Shading with
// Filtered Importance Sampling", EGSR 2008). The sky has no sun disc here
// (the sun's specular is analytic in scene.wgsl). Output is radiance in
// kcd/m2 without exposure.
//
// fs_sh projects the same radiance onto L2 spherical harmonics, already
// convolved with the clamped cosine (Ramamoorthi & Hanrahan, "An Efficient
// Representation for Irradiance Environment Maps", SIGGRAPH 2001: A0 = pi,
// A1 = 2 pi / 3, A2 = pi / 4), one coefficient per pixel of a 9x1 target
// that lib.rs copies into the globals.
// Module: common.wgsl + sky.wgsl + this file.

struct EnvFace {
    info: vec4<f32>, // x face index, y roughness, z face size (texels), w sample count
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

// HDRI mip for a sample covering `solid_angle` steradians.
fn hdri_lod(solid_angle: f32) -> f32 {
    let size = vec2<f32>(textureDimensions(sky_tex));
    let texel = 4.0 * PI / (size.x * size.y);
    return clamp(0.5 * log2(solid_angle / texel), 0.0, g.sky.w - 1.0);
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
    let size = env_face.info.z;
    let st = in.clip.xy / size * 2.0 - 1.0;
    let n = normalize(face_direction(u32(env_face.info.x), st));
    let roughness = env_face.info.y;
    if roughness <= 0.0 {
        return vec4<f32>(sky_kcd(n, hdri_lod(4.0 * PI / (6.0 * size * size))), 1.0);
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
            // pdf of l with N = V: D(h) / 4.
            let d = a * a / (PI * pow(cos_theta * cos_theta * (a * a - 1.0) + 1.0, 2.0));
            let lod = hdri_lod(4.0 / (f32(count) * max(d, 1e-6)));
            sum += sky_kcd(l, lod) * nol;
            weight += nol;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-6), 1.0);
}

fn sh_basis(i: u32, n: vec3<f32>) -> f32 {
    switch i {
        case 0u: { return 0.282095; }
        case 1u: { return 0.488603 * n.y; }
        case 2u: { return 0.488603 * n.z; }
        case 3u: { return 0.488603 * n.x; }
        case 4u: { return 1.092548 * n.x * n.y; }
        case 5u: { return 1.092548 * n.y * n.z; }
        case 6u: { return 0.315392 * (3.0 * n.z * n.z - 1.0); }
        case 7u: { return 1.092548 * n.x * n.z; }
        default: { return 0.546274 * (n.x * n.x - n.y * n.y); }
    }
}

@fragment
fn fs_sh(in: EnvOut) -> @location(0) vec4<f32> {
    let k = u32(in.clip.x);
    // 512 directions: plenty for nine smooth coefficients, and each of the
    // nine fragments loops over all of them serially.
    let rows = 16;
    let cols = 32;
    let d_theta = PI / f32(rows);
    let d_phi = 2.0 * PI / f32(cols);
    let lod = hdri_lod(4.0 * PI / f32(rows * cols));
    // The analytic skies' ground takes the sky's light on it from the
    // upper rows of this same sum (sky.wgsl ground_kcd).
    let analytic = u32(g.sky.x + 0.5) != 2u;
    var up = vec3<f32>(0.0);
    if analytic {
        for (var i = 0; i < rows / 2; i++) {
            let theta = (f32(i) + 0.5) * d_theta;
            let w = sin(theta) * d_theta * d_phi;
            for (var j = 0; j < cols; j++) {
                let phi = (f32(j) + 0.5) * d_phi;
                let dir = vec3<f32>(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
                up += sky_kcd(dir, lod) * dir.y * w;
            }
        }
    }
    let ground = ground_kcd(up) + g.night.rgb;
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < rows; i++) {
        let theta = (f32(i) + 0.5) * d_theta;
        let w = sin(theta) * d_theta * d_phi;
        for (var j = 0; j < cols; j++) {
            let phi = (f32(j) + 0.5) * d_phi;
            let dir = vec3<f32>(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
            var c = sky_kcd(dir, lod);
            if analytic && dir.y < 0.0 {
                c = ground;
            }
            sum += c * sh_basis(k, dir) * w;
        }
    }
    let band = select(select(PI / 4.0, 2.0 * PI / 3.0, k < 4u), PI, k == 0u);
    return vec4<f32>(sum * band, 0.0);
}
