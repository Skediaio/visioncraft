// Geometry: depth pre-pass, shadow-map pass and the forward PBR pass.
//
// BRDF: glTF 2.0 specification, Appendix B (metallic-roughness), with the
// terms as written in Filament's "Physically Based Rendering in Filament"
// documentation: GGX normal distribution, height-correlated Smith visibility,
// Schlick Fresnel, Lambert diffuse.

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: u32,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
}

// Depth-only passes fetch just the position and the instance transform.
struct DepthIn {
    @location(0) position: vec3<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
}

struct Material {
    base_color: vec4<f32>, // linear RGBA
    params: vec4<f32>,     // x metallic, y perceptual roughness
}
// Binding 1 (base colour texture) and 2 (sampler) are in the layout already,
// bound to a 1x1 white texture; textured materials only add the sampling.
@group(1) @binding(0) var<uniform> material: Material;

struct Draw {
    mesh: vec4<u32>, // x mesh id (for the face highlight)
}
@group(2) @binding(0) var<uniform> draw: Draw;

@group(3) @binding(0) var shadow_map: texture_depth_2d_array;
@group(3) @binding(1) var shadow_sampler: sampler_comparison;
@group(3) @binding(2) var occlusion: texture_2d<f32>; // half res: r ambient occlusion, g contact shadow
@group(3) @binding(3) var linear_depth: texture_2d<f32>;

fn model_matrix(v: VertexIn) -> mat4x4<f32> {
    return mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
}

fn world_position(v: VertexIn) -> vec4<f32> {
    return model_matrix(v) * vec4<f32>(v.position, 1.0);
}

// Same arithmetic as `world_position`, so @invariant positions match.
fn depth_world_position(v: DepthIn) -> vec4<f32> {
    return mat4x4<f32>(v.m0, v.m1, v.m2, v.m3) * vec4<f32>(v.position, 1.0);
}

// Normals with the cofactor matrix, correct for any invertible model matrix
// (non-uniform scale, mirroring) without an inverse: the cofactor of M is
// det(M) * inverse-transpose(M) (standard linear algebra; see e.g. Eric
// Lengyel, "Foundations of Game Engine Development", vol. 1, 2016).
fn world_normal(v: VertexIn) -> vec3<f32> {
    let m = model_matrix(v);
    let a = m[0].xyz;
    let b = m[1].xyz;
    let c = m[2].xyz;
    let cof = mat3x3<f32>(cross(b, c), cross(c, a), cross(a, b));
    return cof * v.normal;
}

// ---- depth pre-pass ----------------------------------------------------

struct DepthOut {
    @builtin(position) @invariant clip: vec4<f32>,
}

@vertex
fn vs_depth(v: DepthIn) -> DepthOut {
    var out: DepthOut;
    out.clip = g.view_proj * depth_world_position(v);
    return out;
}

// ---- shadow maps ---------------------------------------------------------

// Per cascade, dynamic offset. Binding 1 so it never aliases `draw`.
@group(2) @binding(1) var<uniform> light_view_proj: mat4x4<f32>;

@vertex
fn vs_shadow(v: DepthIn) -> DepthOut {
    var out: DepthOut;
    out.clip = light_view_proj * depth_world_position(v);
    return out;
}

// ---- forward pass --------------------------------------------------------

struct Varying {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
}

@vertex
fn vs_main(v: VertexIn) -> Varying {
    let world = world_position(v);
    var out: Varying;
    out.clip = g.view_proj * world;
    out.world = world.xyz;
    out.normal = world_normal(v);
    out.face = v.face;
    return out;
}

fn d_ggx(noh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let f = (noh * a2 - noh) * noh + 1.0;
    return a2 / (PI * f * f);
}

fn v_smith_ggx_correlated(nov: f32, nol: f32, a: f32) -> f32 {
    let a2 = a * a;
    let ggxl = nov * sqrt((nol - nol * a2) * nol + a2);
    let ggxv = nol * sqrt((nov - nov * a2) * nov + a2);
    return 0.5 / (ggxv + ggxl);
}

fn f_schlick(voh: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - voh, 5.0);
}

// Irradiance from L2 spherical harmonics: Ramamoorthi & Hanrahan, "An
// Efficient Representation for Irradiance Environment Maps", SIGGRAPH 2001.
// The CPU already folded the cosine-lobe convolution into the coefficients.
fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    var e = g.sh[0].rgb * 0.282095;
    e += g.sh[1].rgb * 0.488603 * n.y;
    e += g.sh[2].rgb * 0.488603 * n.z;
    e += g.sh[3].rgb * 0.488603 * n.x;
    e += g.sh[4].rgb * 1.092548 * n.x * n.y;
    e += g.sh[5].rgb * 1.092548 * n.y * n.z;
    e += g.sh[6].rgb * 0.315392 * (3.0 * n.z * n.z - 1.0);
    e += g.sh[7].rgb * 1.092548 * n.x * n.z;
    e += g.sh[8].rgb * 0.546274 * (n.x * n.x - n.y * n.y);
    return max(e, vec3<f32>(0.0));
}

// Split-sum environment BRDF, analytic fit: Karis, "Physically Based Shading
// on Mobile" (Unreal Engine blog, 2014).
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, nov: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nov)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// One cascade, 3x3 taps of hardware 2x2 PCF (smooth 4x4-texel footprint).
// Normal offset: Holbert, "Saying Goodbye to Shadow Acne", GDC 2011 poster;
// the slope-scaled raster bias is in the shadow pipeline.
fn cascade_shadow(c: u32, world: vec3<f32>, n: vec3<f32>, nol: f32) -> f32 {
    let offset = n * g.texel[c] * (0.6 + 1.4 * (1.0 - nol));
    let p = g.cascades[c] * vec4<f32>(world + offset, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || p.z > 1.0 {
        return 1.0;
    }
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * g.shadow.z * 1.5;
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, i32(c), p.z);
        }
    }
    return sum / 9.0;
}

// Cascade selection by view depth, blending into the next cascade over the
// last 15% of each range and fading out at the end of the last one.
fn sun_shadow(world: vec3<f32>, n: vec3<f32>, nol: f32, depth: f32) -> f32 {
    let count = u32(g.shadow.x);
    var c = 0u;
    while c < count && depth > g.splits[c] {
        c++;
    }
    if c >= count {
        return 1.0;
    }
    let start = select(0.0, g.splits[max(c, 1u) - 1u], c > 0u);
    let band = 0.15 * (g.splits[c] - start);
    let t = clamp((depth - (g.splits[c] - band)) / band, 0.0, 1.0);
    let s = cascade_shadow(c, world, n, nol);
    if t <= 0.0 {
        return s;
    }
    if c + 1u < count {
        return mix(s, cascade_shadow(c + 1u, world, n, nol), t);
    }
    return mix(s, 1.0, t);
}

// Joint bilateral upsampling of the half-res occlusion: bilinear weights
// times depth similarity (Kopf et al., "Joint Bilateral Upsampling",
// SIGGRAPH 2007), so occlusion does not leak across silhouettes.
fn upsample_occlusion(pixel: vec2<f32>, depth: f32) -> vec2<f32> {
    let size = vec2<i32>(textureDimensions(occlusion));
    let full = vec2<i32>(g.viewport.xy) - 1;
    let u = (pixel - 0.5) * 0.5;
    let base = vec2<i32>(floor(u));
    let f = fract(u);
    var sum = vec2<f32>(0.0);
    var weight = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i & 1, i >> 1u);
        let q = clamp(base + o, vec2<i32>(0), size - 1);
        let d = textureLoad(linear_depth, min(q * 2, full), 0).r;
        let bilinear = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let w = (bilinear + 1e-3) / (1e-3 + abs(d - depth) / depth);
        sum += textureLoad(occlusion, q, 0).rg * w;
        weight += w;
    }
    return sum / weight;
}

@fragment
fn fs_main(in: Varying) -> @location(0) vec4<f32> {
    var base = material.base_color.rgb;
    if in.face == g.highlight.y && draw.mesh.x == g.highlight.x {
        base = mix(base, vec3<f32>(0.25, 0.55, 1.0), 0.35);
    }
    let metallic = material.params.x;
    // Filament clamps perceptual roughness to 0.045 to keep GGX in f16 range.
    let roughness = clamp(material.params.y, 0.045, 1.0);
    let a = roughness * roughness;

    let n = normalize(in.normal);
    let to_eye = g.camera.xyz - in.world;
    let v = normalize(to_eye);
    let l = g.sun_dir.xyz;
    let h = normalize(v + l);
    let nov = max(dot(n, v), 1e-4);
    let nol = max(dot(n, l), 0.0);
    let noh = max(dot(n, h), 0.0);
    let voh = max(dot(v, h), 0.0);

    let f0 = mix(vec3<f32>(0.04), base, metallic);
    let diffuse_color = base * (1.0 - metallic);

    let depth = -(g.view * vec4<f32>(in.world, 1.0)).z;
    let screen = upsample_occlusion(in.clip.xy, depth);
    let ao = screen.r;

    var sun = vec3<f32>(0.0);
    if nol > 0.0 {
        let shadow = min(sun_shadow(in.world, n, nol, depth), screen.g);
        let fr = d_ggx(noh, a) * v_smith_ggx_correlated(nov, nol, a) * f_schlick(voh, f0);
        let fd = diffuse_color / PI;
        sun = (fd + fr) * g.sun_light.rgb * nol * shadow;
    }

    // Ambient from the sky model: diffuse irradiance, and the irradiance in
    // the reflection direction as a (very blurry) stand-in for a prefiltered
    // environment map until there is one.
    let r = reflect(-v, n);
    let ambient_diffuse = diffuse_color * sh_irradiance(n) / PI * ao;
    // Specular occlusion from AO: Lagarde & de Rousiers 2014, section 4.10.2.
    let spec_ao = clamp(pow(nov + ao, exp2(-16.0 * roughness - 1.0)) - 1.0 + ao, 0.0, 1.0);
    let ambient_specular = env_brdf_approx(f0, roughness, nov) * sh_irradiance(r) / PI * spec_ao;

    return vec4<f32>(sun + ambient_diffuse + ambient_specular, 1.0);
}
