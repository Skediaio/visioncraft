// Geometry: depth pre-pass, shadow-map pass, the forward PBR pass and the
// sorted transparent pass (alpha-blended materials, thin glass, water).
//
// BRDF: glTF 2.0 specification, Appendix B (metallic-roughness), with the
// terms as written in Filament's "Physically Based Rendering in Filament"
// documentation: GGX normal distribution, height-correlated Smith visibility,
// Schlick Fresnel, Lambert diffuse.
//
// Every pass that draws a mesh runs the same vertex animation (wind), so
// shadows and the depth pre-pass move with the leaves; vs_depth and vs_main
// compute the clip position with the same function, so @invariant makes the
// forward pass's depth equal to the pre-pass (cutout draws test Equal).

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: u32,
    @location(3) uv: vec2<f32>,
    @location(4) wind: vec2<f32>, // bend weight 0..1, phase (cycles)
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
}

// Depth-only passes fetch the position, UV (alpha test), wind and transform.
struct DepthIn {
    @location(0) position: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) wind: vec2<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
}

// Frame resources next to the globals (group 0; the environment cube and
// the sky source are declared in common.wgsl): the light lists.
struct Light {
    position: vec4<f32>, // xyz world, w range (m)
    color: vec4<f32>,    // rgb linear colour * luminous intensity (cd)
    spot: vec4<f32>,     // xyz unit direction the spot points along, w angle scale
    extra: vec4<f32>,    // x angle offset (a point light has scale 0, offset 1)
}
@group(0) @binding(3) var<storage, read> lights: array<Light>;
// Per grid cell (row-major over world x, z): first index and count into this
// same array, then the light indices of all cells.
@group(0) @binding(4) var<storage, read> light_grid: array<u32>;

struct Material {
    base_color: vec4<f32>, // linear RGBA
    params: vec4<f32>,     // x metallic, y perceptual roughness, z relief, w gloss
    emissive: vec4<f32>,   // rgb emitted luminance (cd/m2), w weathering
    alpha: vec4<f32>,      // x cutout threshold, y 1 / triplanar scale (0: UVs), z surface (0 standard, 1 glass, 2 water), w foliage
    flags: vec4<u32>,      // x texture bits (1 base, 2 normal, 4 orm, 8 roughness, 16 metallic, 32 occlusion), y double-sided, z macro variation (f32 bits)
}
@group(1) @binding(0) var<uniform> material: Material;
// Absent maps are bound to a 1x1 white texture (a neutral factor).
@group(1) @binding(1) var base_tex: texture_2d<f32>;
@group(1) @binding(2) var normal_tex: texture_2d<f32>;
@group(1) @binding(3) var orm_tex: texture_2d<f32>;
@group(1) @binding(4) var rough_tex: texture_2d<f32>;
@group(1) @binding(5) var metal_tex: texture_2d<f32>;
@group(1) @binding(6) var ao_tex: texture_2d<f32>;
// One sampler per map: repeat or clamp, as each texture was added.
@group(1) @binding(7) var base_smp: sampler;
@group(1) @binding(8) var normal_smp: sampler;
@group(1) @binding(9) var orm_smp: sampler;
@group(1) @binding(10) var rough_smp: sampler;
@group(1) @binding(11) var metal_smp: sampler;
@group(1) @binding(12) var ao_smp: sampler;

struct Draw {
    mesh: vec4<u32>, // x mesh id (for the face highlight)
}
@group(2) @binding(0) var<uniform> draw: Draw;

@group(3) @binding(0) var shadow_map: texture_depth_2d_array;
@group(3) @binding(1) var shadow_sampler: sampler_comparison;
@group(3) @binding(2) var occlusion: texture_2d<f32>; // half res: r ambient occlusion, g contact shadow
@group(3) @binding(3) var linear_depth: texture_2d<f32>;
// The scene mirrored about the water plane (reflection pass), pre-exposed HDR.
@group(3) @binding(4) var planar_reflection: texture_2d<f32>;

// MSAA sample count of the pipeline (depth pre-pass coverage mask).
override SAMPLES: u32 = 1u;

const HAS_BASE: u32 = 1u;
const HAS_NORMAL: u32 = 2u;

// ---- wind -----------------------------------------------------------------

// Vegetation animation after Sousa, "Vegetation Procedural Animation and
// Shading in Crysis", GPU Gems 3, ch. 16 (2007): a main bend of the whole
// plant along the wind, growing with the squared bend weight and length-
// preserved about the plant's origin, plus detail (branch) motion from the
// per-vertex weight and phase, driven by smoothed triangle waves. The
// per-instance phase is a hash of the instance position, so a hedge does not
// sway in unison; gusts travel across the garden along the wind.
fn smooth_triangle(x: vec4<f32>) -> vec4<f32> {
    let t = abs(fract(x + 0.5) * 2.0 - 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn animated_world(position: vec3<f32>, wind: vec2<f32>, m: mat4x4<f32>, animate: bool) -> vec3<f32> {
    let p = (m * vec4<f32>(position, 1.0)).xyz;
    let strength = g.wind.z;
    if !animate || wind.x <= 0.0 || strength <= 0.0 {
        return p;
    }
    let origin = m[3].xyz;
    let dir = vec3<f32>(g.wind.x, 0.0, g.wind.y);
    let t = g.time.x;
    // R2 low-discrepancy weights (Roberts 2018): neighbouring plants get
    // well-spread phases for two multiply-adds.
    let instance_phase = fract(dot(origin.xz, vec2<f32>(0.7548777, 0.5698403)));
    let travel = dot(origin.xz, g.wind.xy) * 0.05; // gusts move downwind (~20 m per cycle)
    let waves = smooth_triangle(vec4<f32>(
        t * 0.07 - travel,              // slow gust envelope
        t * 0.31 + instance_phase,      // whole-plant sway
        t * 1.13 + wind.y + instance_phase, // branch motion
        t * 2.37 + wind.y * 1.7,        // leaf flutter
    )) * 2.0 - 1.0;
    let gust = 1.0 + g.wind.w * waves.x;
    let rel = p - origin;
    let size = length(rel);
    // Main bend: tips lean downwind by up to ~4 % of their distance from the
    // base at strength 1, plus a sway around that.
    let lean = strength * gust * (0.6 + 0.4 * waves.y);
    var q = rel + dir * (wind.x * wind.x * lean * 0.04 * size);
    q = q * (size / max(length(q), 1e-6));
    // Detail: branches bob along the wind and vertically, leaves flutter.
    let detail = strength * gust * wind.x * (0.02 + 0.01 * min(size, 6.0));
    q += (dir * waves.z + vec3<f32>(0.0, 0.6, 0.0) * waves.w) * detail;
    return origin + q;
}

fn model(m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, m3: vec4<f32>) -> mat4x4<f32> {
    return mat4x4<f32>(m0, m1, m2, m3);
}

// Normals with the cofactor matrix, correct for any invertible model matrix
// (non-uniform scale, mirroring) without an inverse: the cofactor of M is
// det(M) * inverse-transpose(M) (standard linear algebra; see e.g. Eric
// Lengyel, "Foundations of Game Engine Development", vol. 1, 2016).
fn world_normal(m: mat4x4<f32>, normal: vec3<f32>) -> vec3<f32> {
    let a = m[0].xyz;
    let b = m[1].xyz;
    let c = m[2].xyz;
    let cof = mat3x3<f32>(cross(b, c), cross(c, a), cross(a, b));
    return cof * normal;
}

// ---- alpha ------------------------------------------------------------------

// Cutout alpha with the mip-level rescale of Ben Golus, "Anti-aliased Alpha
// Test: The Esoteric Alpha To Coverage" (2017): averaged mips lose coverage,
// so alpha is scaled up by 25 % per mip level, which keeps foliage from
// thinning out in the distance.
fn cutout_alpha(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(base_tex));
    let texels_x = dpdx(uv * size);
    let texels_y = dpdy(uv * size);
    let lod = max(0.0, 0.5 * log2(max(dot(texels_x, texels_x), dot(texels_y, texels_y))));
    // Explicit level: an isotropic lookup, cheaper than the material
    // sampler's anisotropic one in these overdraw-heavy passes.
    let a = material.base_color.a * textureSampleLevel(base_tex, base_smp, uv, lod).a;
    return a * (1.0 + lod * 0.25);
}

// ---- depth pre-pass -----------------------------------------------------

struct DepthOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
}

@vertex
fn vs_depth(v: DepthIn) -> DepthOut {
    var out: DepthOut;
    let world = animated_world(v.position, v.wind, model(v.m0, v.m1, v.m2, v.m3), true);
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.uv = v.uv;
    out.world = world;
    return out;
}

// Foliage cards seen edge-on (from the camera, or from the sun in the
// shadow pass) fade out instead of showing as thin dark slivers: alpha is
// scaled by how squarely the card's geometric face is seen. Leaves only
// (their shading normals are bent, so the face normal comes from
// derivatives of the position).
fn edge_fade(world: vec3<f32>, dir: vec3<f32>) -> f32 {
    let face = normalize(cross(dpdx(world), dpdy(world)));
    let fade = smoothstep(0.06, 0.3, abs(dot(face, dir)));
    return select(1.0, fade, material.alpha.w > 0.0);
}

// Cutout coverage with MSAA: alpha to
// coverage done by hand through the sample mask, because a depth-only pass
// has no colour target to drive the hardware's: the alpha is sharpened to a
// one-pixel ramp across the cutout edge (Golus 2017) and turned into that many
// of the 4 samples, rotated per pixel. The forward pass then draws cutout
// materials with an Equal depth test, so it inherits exactly this coverage.
// Without MSAA a separate entry point that only discards: writing the
// sample mask turns early depth testing off on some GPUs.
@fragment
fn fs_depth_cutout_1x(in: DepthOut) {
    if cutout_alpha(in.uv) * edge_fade(in.world, normalize(to_eye(in.world))) < material.alpha.x {
        discard;
    }
}

@fragment
fn fs_depth_cutout(in: DepthOut) -> @builtin(sample_mask) u32 {
    let a = cutout_alpha(in.uv) * edge_fade(in.world, normalize(to_eye(in.world)));
    let cutoff = material.alpha.x;
    let sharp = (a - cutoff) / max(fwidth(a), 1e-4) + 0.5;
    let n = u32(clamp(round(sharp * f32(SAMPLES)), 0.0, f32(SAMPLES)));
    if n == 0u {
        discard;
    }
    let bits = (1u << n) - 1u;
    let r = u32(ign(in.clip.xy) * f32(SAMPLES)) % SAMPLES;
    let full = (1u << SAMPLES) - 1u;
    return ((bits << r) | (bits >> (SAMPLES - r))) & full;
}

// ---- shadow maps ---------------------------------------------------------

// Per cascade, dynamic offset. Binding 1 so it never aliases `draw`.
struct Cascade {
    view_proj: mat4x4<f32>,
    flags: vec4<f32>, // x: animate wind (0 in the cached far cascade)
}
@group(2) @binding(1) var<uniform> cascade: Cascade;

@vertex
fn vs_shadow(v: DepthIn) -> DepthOut {
    var out: DepthOut;
    let world = animated_world(v.position, v.wind, model(v.m0, v.m1, v.m2, v.m3), cascade.flags.x > 0.5);
    out.clip = cascade.view_proj * vec4<f32>(world, 1.0);
    out.uv = v.uv;
    out.world = world;
    return out;
}

// Kept minimal (shadow maps are where foliage overdraw is worst): no edge
// fade.
@fragment
fn fs_shadow_cutout(in: DepthOut) {
    if cutout_alpha(in.uv) < material.alpha.x {
        discard;
    }
}

// Thin glass casts a faint shadow: it writes depth on two texels of every
// five in a fixed stipple, which the 3x3 / 2x2 percentage-closer filter
// averages to a ~40 % occluder (screen-door transparency in the shadow map;
// a depth map cannot carry the tint).
@fragment
fn fs_shadow_glass(in: DepthOut) {
    let p = vec2<u32>(in.clip.xy);
    if (p.x + 2u * p.y) % 5u >= 2u {
        discard;
    }
}

// ---- forward pass --------------------------------------------------------

struct Varying {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) uv: vec2<f32>,
}

@vertex
fn vs_main(v: VertexIn) -> Varying {
    let m = model(v.m0, v.m1, v.m2, v.m3);
    let world = animated_world(v.position, v.wind, m, true);
    var out: Varying;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = world_normal(m, v.normal);
    out.face = v.face;
    out.uv = v.uv;
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

fn specular(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, nol: f32, nov: f32, f0: vec3<f32>, a: f32) -> vec3<f32> {
    let h = normalize(v + l);
    let noh = max(dot(n, h), 0.0);
    let voh = max(dot(v, h), 0.0);
    return d_ggx(noh, a) * v_smith_ggx_correlated(nov, nol, a) * f_schlick(voh, f0);
}

// Irradiance from L2 spherical harmonics: Ramamoorthi & Hanrahan, "An
// Efficient Representation for Irradiance Environment Maps", SIGGRAPH 2001.
// env.wgsl already folded the cosine-lobe convolution into the coefficients
// (kcd/m2 * sr, so the result is scaled to pre-exposed lux here).
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
    return max(e, vec3<f32>(0.0)) * g.night.w;
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

// Prefiltered environment radiance (pre-exposed) for a perceptual roughness.
fn environment(r: vec3<f32>, roughness: f32) -> vec3<f32> {
    return textureSampleLevel(env_map, env_sampler, r, roughness * g.time.z).rgb * g.night.w;
}

// One cascade with hardware 2x2 PCF taps: 3x3 of them (smooth 4x4-texel
// footprint) on HIGH, 2x2 at +-0.75 texel (3x3-texel footprint, less than
// half the cost) below. -1 when the point is outside this cascade's map
// (cascades refreshed every other frame lag the camera by a frame).
// Normal offset: Holbert, "Saying Goodbye to Shadow Acne", GDC 2011 poster;
// the slope-scaled raster bias is in the shadow pipeline.
fn cascade_shadow(c: u32, world: vec3<f32>, n: vec3<f32>, nol: f32) -> f32 {
    let offset = n * g.texel[c] * (0.6 + 1.4 * (1.0 - nol));
    let p = g.cascades[c] * vec4<f32>(world + offset, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || p.z > 1.0 {
        return -1.0;
    }
    var sum = 0.0;
    if g.options.x > 4.5 {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let o = vec2<f32>(f32(x), f32(y)) * g.shadow.z * 1.5;
                sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, i32(c), p.z);
            }
        }
        return sum / 9.0;
    }
    for (var i = 0; i < 4; i++) {
        let o = (vec2<f32>(f32(i & 1), f32(i >> 1u)) - 0.5) * g.shadow.z * 1.5;
        sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, i32(c), p.z);
    }
    return sum * 0.25;
}

// Cascade selection by view depth, blending into the next cascade over the
// last 15% of each range and fading out at the end of the last one; a point
// a cascade does not cover falls through to the next.
fn sun_shadow(world: vec3<f32>, n: vec3<f32>, nol: f32, depth: f32) -> f32 {
    let count = u32(g.shadow.x);
    var c = 0u;
    while c < count && depth > g.splits[c] {
        c++;
    }
    var s = -1.0;
    while c < count && s < 0.0 {
        s = cascade_shadow(c, world, n, nol);
        if s < 0.0 {
            c++;
        }
    }
    if s < 0.0 {
        return 1.0;
    }
    let start = select(0.0, g.splits[max(c, 1u) - 1u], c > 0u);
    let band = 0.15 * (g.splits[c] - start);
    let t = clamp((depth - (g.splits[c] - band)) / band, 0.0, 1.0);
    if t <= 0.0 {
        return s;
    }
    if c + 1u < count {
        let next = cascade_shadow(c + 1u, world, n, nol);
        return mix(s, select(next, s, next < 0.0), t);
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

// ---- surface: projections and maps --------------------------------------

// Up to three planar lookups with explicit gradients (textureSampleGrad works
// in non-uniform control flow, so lookups with zero weight are skipped).
struct Lookup {
    uv: array<vec2<f32>, 3>,
    dx: array<vec2<f32>, 3>,
    dy: array<vec2<f32>, 3>,
    weight: vec3<f32>,
    // Triplanar: tangent (image right) and bitangent (image up) per axis.
    t: array<vec3<f32>, 3>,
    b: array<vec3<f32>, 3>,
}

fn sample_lookup(tex: texture_2d<f32>, smp: sampler, l: Lookup) -> vec4<f32> {
    var c = vec4<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        if l.weight[i] > 0.0 {
            c += textureSampleGrad(tex, smp, l.uv[i], l.dx[i], l.dy[i]) * l.weight[i];
        }
    }
    return c;
}

// Triplanar projection (world axes, blended by the geometric normal to the
// 4th power). Per axis the image's right and up are chosen so that, seen from
// outside along that axis, the texture is neither mirrored nor upside down;
// texture v runs down the image, so v = -dot(p, up).
fn triplanar(world: vec3<f32>, dwx: vec3<f32>, dwy: vec3<f32>, n: vec3<f32>, inv_scale: f32) -> Lookup {
    var l: Lookup;
    var w = pow(abs(n), vec3<f32>(4.0));
    w = w / (w.x + w.y + w.z);
    w = select(w, vec3<f32>(0.0), w < vec3<f32>(0.02));
    l.weight = w / (w.x + w.y + w.z);
    let s = select(vec3<f32>(1.0), vec3<f32>(-1.0), n < vec3<f32>(0.0));
    l.t[0] = vec3<f32>(0.0, 0.0, -s.x);
    l.b[0] = vec3<f32>(0.0, 1.0, 0.0);
    l.t[1] = vec3<f32>(1.0, 0.0, 0.0);
    l.b[1] = vec3<f32>(0.0, 0.0, -s.y);
    l.t[2] = vec3<f32>(s.z, 0.0, 0.0);
    l.b[2] = vec3<f32>(0.0, 1.0, 0.0);
    for (var i = 0; i < 3; i++) {
        let t = l.t[i];
        let b = l.b[i];
        l.uv[i] = vec2<f32>(dot(world, t), -dot(world, b)) * inv_scale;
        l.dx[i] = vec2<f32>(dot(dwx, t), -dot(dwx, b)) * inv_scale;
        l.dy[i] = vec2<f32>(dot(dwy, t), -dot(dwy, b)) * inv_scale;
    }
    return l;
}

fn uv_lookup(uv: vec2<f32>) -> Lookup {
    var l: Lookup;
    l.uv[0] = uv;
    l.dx[0] = dpdx(uv);
    l.dy[0] = dpdy(uv);
    l.weight = vec3<f32>(1.0, 0.0, 0.0);
    return l;
}

// Tangent-space normal map (OpenGL convention: green points up the image)
// with the tangent frame from screen-space derivatives instead of stored
// tangents: Schüler, "Normal Mapping without Precomputed Tangents", ShaderX5
// (2006) and his 2013 follow-up. WGSL's dpdy runs down the framebuffer, so
// both screen derivatives along y are negated to keep the frame's handedness.
fn uv_normal(tn: vec3<f32>, n: vec3<f32>, world: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let dp1 = dpdx(world);
    let dp2 = -dpdy(world);
    let duv1 = dpdx(uv);
    let duv2 = -dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y; // towards +v, i.e. down the image
    let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
    return normalize(tn.x * t * inv - tn.y * b * inv + tn.z * n);
}

// Triplanar normal mapping: Ben Golus, "Normal Mapping for a Triplanar
// Shader" (2017), the whiteout blend per projection (Barré-Brisebois & Hill,
// "Blending in Detail", 2012): the geometric normal is expressed in each
// projection's tangent frame, blended with that projection's map normal,
// taken back to world space and the three are mixed by the triplanar weights.
fn triplanar_normal(l: Lookup, n: vec3<f32>, relief: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        if l.weight[i] > 0.0 {
            var tn = textureSampleGrad(normal_tex, normal_smp, l.uv[i], l.dx[i], l.dy[i]).xyz * 2.0 - 1.0;
            tn = vec3<f32>(tn.xy * relief, tn.z);
            let t = l.t[i];
            let b = l.b[i];
            let axis = cross(t, b);
            let base = vec3<f32>(dot(n, t), dot(n, b), dot(n, axis));
            let w = vec3<f32>(tn.xy + base.xy, tn.z * base.z);
            sum += (w.x * t + w.y * b + w.z * axis) * l.weight[i];
        }
    }
    return normalize(sum);
}

// Value noise for the weathering pattern: trilinear over hashed lattice
// values, smoothstep-faded.
fn value_noise(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash3(i);
    let n100 = hash3(i + vec3<i32>(1, 0, 0));
    let n010 = hash3(i + vec3<i32>(0, 1, 0));
    let n110 = hash3(i + vec3<i32>(1, 1, 0));
    let n001 = hash3(i + vec3<i32>(0, 0, 1));
    let n101 = hash3(i + vec3<i32>(1, 0, 1));
    let n011 = hash3(i + vec3<i32>(0, 1, 1));
    let n111 = hash3(i + vec3<i32>(1, 1, 1));
    let x0 = mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y);
    let x1 = mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y);
    return mix(x0, x1, u.z);
}


struct Surface {
    base: vec4<f32>,
    n: vec3<f32>,        // shading normal (normal map applied)
    geometric: vec3<f32>,
    roughness: f32,
    metallic: f32,
    ao: f32,             // from the maps only
    emissive: vec3<f32>,
}

// Base colour, normal and the occlusion / roughness / metallic maps, for
// either projection. Called from uniform control flow (it takes derivatives).
fn surface(in: Varying, front: bool) -> Surface {
    var s: Surface;
    var n = normalize(in.normal);
    // Double-sided: back faces shade with the flipped normal, except foliage,
    // whose normals are bent outwards from the crown so both sides of a card
    // shade alike.
    if material.flags.y != 0u && material.alpha.w == 0.0 && !front {
        n = -n;
    }
    s.geometric = n;
    let dwx = dpdx(in.world);
    let dwy = dpdy(in.world);
    let triplanar_on = material.alpha.y > 0.0;
    var l: Lookup;
    if triplanar_on {
        l = triplanar(in.world, dwx, dwy, n, material.alpha.y);
    } else {
        l = uv_lookup(in.uv);
    }
    // Absent maps are skipped (uniform branches), not sampled as white.
    let bits = material.flags.x;
    s.base = material.base_color;
    if (bits & HAS_BASE) != 0u {
        s.base *= sample_lookup(base_tex, base_smp, l);
    }
    var orm = vec3<f32>(1.0);
    if (bits & 4u) != 0u {
        orm = sample_lookup(orm_tex, orm_smp, l).rgb;
    }
    var rough_map = 1.0;
    if (bits & 8u) != 0u {
        rough_map = sample_lookup(rough_tex, rough_smp, l).r;
    }
    var metal_map = 1.0;
    if (bits & 16u) != 0u {
        metal_map = sample_lookup(metal_tex, metal_smp, l).r;
    }
    var ao_map = 1.0;
    if (bits & 32u) != 0u {
        ao_map = sample_lookup(ao_tex, ao_smp, l).r;
    }
    s.ao = orm.r * ao_map;
    // Gloss is an artist offset: positive makes the surface glossier.
    s.roughness = clamp(material.params.y * orm.g * rough_map - material.params.w, 0.045, 1.0);
    s.metallic = clamp(material.params.x * orm.b * metal_map, 0.0, 1.0);
    s.n = n;
    if (bits & HAS_NORMAL) != 0u {
        let relief = material.params.z;
        if triplanar_on {
            s.n = triplanar_normal(l, n, relief);
        } else {
            var tn = textureSampleGrad(normal_tex, normal_smp, l.uv[0], l.dx[0], l.dy[0]).xyz * 2.0 - 1.0;
            tn = vec3<f32>(tn.xy * relief, tn.z);
            s.n = uv_normal(tn, n, in.world, in.uv);
        }
    }
    s.emissive = material.emissive.rgb;
    return macro_variation(s, in.world, bitcast<f32>(material.flags.z));
}

// Macro variation against the carpet look of large single-material
// surfaces: two octaves of world-space value noise (features of ~5 and
// ~15 m) modulate brightness and roughness, a third paints drier, yellower
// patches, and fields of ~100 m keep a large ground from being one flat
// colour in the middle distance where the finer octaves average out.
fn macro_variation(surf: Surface, world: vec3<f32>, variation: f32) -> Surface {
    var s = surf;
    if variation > 0.0 {
        let m = value_noise(world * 0.21) * 0.65 + value_noise(world * 0.067 + 31.0) * 0.35 - 0.5;
        let big = value_noise(world * 0.011 + 3.0) - 0.5;
        let dry = smoothstep(0.45, 0.8, value_noise(world * 0.13 + 7.0)) * variation;
        var c = s.base.rgb * (1.0 + m * 0.6 * variation) * (1.0 + big * 0.6 * variation);
        c = mix(c, c * vec3<f32>(1.3, 1.08, 0.55), dry * 0.6);
        s.base = vec4<f32>(c, s.base.a);
        s.roughness = clamp(s.roughness + m * 0.2 * variation, 0.045, 1.0);
    }
    return s;
}

// Weathering: a procedural look, not a physical model. Occluded and
// downward-facing areas darken and desaturate (grime), up-facing ones take a
// greenish tint (algae, moss), both modulated by low-frequency noise.
fn weather(base: vec3<f32>, world: vec3<f32>, n: vec3<f32>, ao: f32, amount: f32) -> vec3<f32> {
    if amount <= 0.0 {
        return base;
    }
    let noise = value_noise(world * 0.8) * 0.65 + value_noise(world * 3.1) * 0.35;
    let grime = clamp(amount * ((1.0 - ao) * 1.6 + max(-n.y, 0.0) * 0.5 + 0.15) * (0.4 + noise), 0.0, 1.0);
    let grey = vec3<f32>(luminance(base)) * vec3<f32>(0.9, 0.86, 0.8);
    var c = mix(base, grey * 0.55, grime * 0.8);
    let moss = amount * max(n.y, 0.0) * smoothstep(0.35, 0.8, noise);
    c = mix(c, vec3<f32>(0.11, 0.16, 0.05) * (0.6 + noise), clamp(moss, 0.0, 0.7));
    return c;
}

// Point and spot lights from the light list of the fragment's grid cell.
// Inverse-square falloff windowed to the light's range: Karis, "Real Shading
// in Unreal Engine 4" (2013), eq. 9, with the distance clamp of Lagarde & de
// Rousiers, "Moving Frostbite to PBR" (2014), eq. 26; spot cone attenuation
// as in Filament's documentation (scale and offset on the cosine, squared).
// Leaves get the sun's wrapped diffuse and translucency (see shade); glass
// and water pass a black diffuse colour for the specular highlights only.
fn local_lights(world: vec3<f32>, n: vec3<f32>, v: vec3<f32>, nov: f32, diffuse_color: vec3<f32>, f0: vec3<f32>, a: f32, foliage: bool) -> vec3<f32> {
    if g.light_grid.w <= 0.0 {
        return vec3<f32>(0.0);
    }
    let dims = vec2<i32>(g.grid_dims.xy);
    let cell = vec2<i32>(floor((world.xz - g.light_grid.xy) * g.light_grid.z));
    if any(cell < vec2<i32>(0)) || any(cell >= dims) {
        return vec3<f32>(0.0);
    }
    let c = u32(cell.y * dims.x + cell.x);
    let first = light_grid[2u * c];
    let count = light_grid[2u * c + 1u];
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < count; i++) {
        let light = lights[light_grid[first + i]];
        let d = light.position.xyz - world;
        let dist2 = dot(d, d);
        let range = light.position.w;
        let ratio = dist2 / (range * range);
        let window = clamp(1.0 - ratio * ratio, 0.0, 1.0);
        let l = d * inverseSqrt(max(dist2, 1e-8));
        let nol = dot(n, l);
        let cone = clamp(dot(-l, light.spot.xyz) * light.spot.w + light.extra.x, 0.0, 1.0);
        let attenuation = window * window * cone * cone / max(dist2, 1e-4);
        var diffuse = max(nol, 0.0);
        var through = 0.0;
        if foliage {
            diffuse = max((nol + 0.5) / 1.5, 0.0) / 1.5;
            through = pow(clamp(dot(v, -normalize(l + n * 0.3)), 0.0, 1.0), 4.0) * 0.8 + 0.15 * max(-nol, 0.0);
        }
        if attenuation > 0.0 && (diffuse > 0.0 || through > 0.0) {
            var spec = vec3<f32>(0.0);
            if nol > 0.0 && (!foliage || dot(n, v) > 0.0) {
                spec = specular(n, v, l, nol, nov, f0, a) * nol;
            }
            let leaf = diffuse_color * diffuse_color * 2.0 * through;
            sum += (diffuse_color / PI * diffuse + spec + leaf) * light.color.rgb * attenuation;
        }
    }
    return sum * g.time.y;
}

struct Shaded {
    color: vec3<f32>,
    alpha: f32,
}

// The full opaque shading model. `screen_effects`: use the screen-space AO
// and contact shadows (not for transparent surfaces, which are not in the
// depth pre-pass).
fn shade(in: Varying, front: bool, screen_effects: bool) -> Shaded {
    return light(in, surface(in, front), material.alpha.w > 0.0, material.emissive.w, screen_effects);
}

// Lighting of a surface (any source: a material, the terrain layers, an
// impostor atlas), without reading the material uniform.
fn light(in: Varying, surf: Surface, foliage: bool, weathering: f32, screen_effects: bool) -> Shaded {
    var s = surf;
    let clay = g.time.w > 0.5;
    if clay {
        s.base = vec4<f32>(0.8, 0.8, 0.8, s.base.a);
        s.n = s.geometric;
        s.roughness = 0.85;
        s.metallic = 0.0;
        s.ao = 1.0;
        s.emissive = vec3<f32>(0.0);
    }
    var base = s.base.rgb;
    if in.face == g.highlight.y && draw.mesh.x == g.highlight.x {
        base = mix(base, vec3<f32>(0.25, 0.55, 1.0), 0.35);
    }
    let n = s.n;
    let v = normalize(to_eye(in.world));
    let l = g.sun_dir.xyz;
    // Bent leaf normals often face away from the viewer; clamping n.v keeps
    // those cards from turning into grazing-angle mirrors of the sky.
    let nov = select(max(dot(n, v), 1e-4), max(abs(dot(n, v)), 0.35), foliage);
    let a = s.roughness * s.roughness;

    let depth = -(g.view * vec4<f32>(in.world, 1.0)).z;
    var screen = vec2<f32>(1.0);
    if screen_effects {
        screen = upsample_occlusion(in.clip.xy, depth);
    }
    let ao = screen.r * s.ao;
    if !clay {
        base = weather(base, in.world, s.geometric, ao, weathering);
    }
    let f0 = mix(vec3<f32>(0.04), base, s.metallic);
    let diffuse_color = base * (1.0 - s.metallic);

    var sun = vec3<f32>(0.0);
    let nol_raw = dot(n, l);
    let nol = max(nol_raw, 0.0);
    if g.sun_light.r + g.sun_light.g + g.sun_light.b > 0.0 && (nol > 0.0 || foliage) {
        let shadow = min(sun_shadow(in.world, s.geometric, max(dot(s.geometric, l), 0.0), depth), screen.g);
        var diffuse = nol;
        if foliage {
            // Leaves: wrapped diffuse (Green, "Real-Time Approximations to
            // Subsurface Scattering", GPU Gems 1, ch. 16, 2004) plus light
            // through the leaf when the sun is behind it, the view-dependent
            // translucency term of Barré-Brisebois & Bouchard, "Approximating
            // Translucency for a Fast, Cheap and Convincing Subsurface
            // Scattering Look", GDC 2011 (distortion 0.3, power 4, scale 0.8).
            let wrap = 0.5;
            diffuse = max((nol_raw + wrap) / (1.0 + wrap), 0.0) / (1.0 + wrap);
            let lt = normalize(l + n * 0.3);
            let through = pow(clamp(dot(v, -lt), 0.0, 1.0), 4.0) * 0.8 + 0.15 * max(-nol_raw, 0.0);
            sun += base * base * 2.0 * through * g.sun_light.rgb * shadow;
        }
        var fr = vec3<f32>(0.0);
        // No sun glints off leaves facing away from the viewer.
        if nol > 0.0 && (!foliage || dot(n, v) > 0.0) {
            fr = specular(n, v, l, nol, nov, f0, a) * nol;
        }
        sun += (diffuse_color / PI * diffuse + fr) * g.sun_light.rgb * shadow;
    }

    // Ambient: diffuse irradiance from the sky's SH, specular from the
    // prefiltered environment cube with the split-sum BRDF fit.
    let r = reflect(-v, n);
    let ambient_diffuse = diffuse_color * sh_irradiance(n) / PI * ao;
    // Specular occlusion from AO: Lagarde & de Rousiers 2014, section 4.10.2.
    let spec_ao = clamp(pow(nov + ao, exp2(-16.0 * s.roughness - 1.0)) - 1.0 + ao, 0.0, 1.0);
    let ambient_specular = env_brdf_approx(f0, s.roughness, nov) * environment(r, s.roughness) * spec_ao;
    let local = local_lights(in.world, n, v, nov, diffuse_color, f0, a, foliage);
    let emitted = s.emissive * g.time.y;
    return Shaded(apply_fog(sun + ambient_diffuse + ambient_specular + local + emitted, in.world), s.base.a);
}

@fragment
fn fs_main(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in, front, true).color, 1.0);
}

// ---- infinite ground -------------------------------------------------------

// A backdrop, not geometry: a polar grid of rings around the camera's
// position (radius 0.5 m growing by a constant factor to ~40 km) at the
// ground height, drawn after the opaque scene at a depth just in front of
// the far plane, so it only fills pixels nothing else covered: it can never
// z-fight with or hide the user's own lawn, pond basin or basement. It is
// shaded like any surface (its material is made from `Ground`), so it gets
// sun, shadows of the near cascades, sky light, lamps and the haze.
const GROUND_RINGS: u32 = 48u;
const GROUND_SEGMENTS: u32 = 64u;
const GROUND_GROWTH: f32 = 1.2716; // 0.5 m * 1.2716^47 = 40 km
const GROUND_DEPTH: f32 = 0.999999;

@vertex
fn vs_ground(@builtin(vertex_index) vi: u32) -> Varying {
    let tri = vi / 3u;
    let quad = tri / 2u;
    let ring = quad / GROUND_SEGMENTS;
    let seg = quad % GROUND_SEGMENTS;
    var dr = array<u32, 6>(0u, 1u, 1u, 0u, 1u, 0u);
    var ds = array<u32, 6>(0u, 0u, 1u, 0u, 1u, 1u);
    let k = vi % 3u + 3u * (tri & 1u);
    let r = ring + dr[k];
    let radius = select(0.0, 0.5 * pow(GROUND_GROWTH, f32(r) - 1.0), r > 0u);
    let angle = f32(seg + ds[k]) * 2.0 * PI / f32(GROUND_SEGMENTS);
    let world = vec3<f32>(g.camera.x + radius * cos(angle), g.options.w, g.camera.z + radius * sin(angle));
    var out: Varying;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.clip.z = out.clip.w * GROUND_DEPTH;
    out.world = world;
    out.normal = vec3<f32>(0.0, 1.0, 0.0);
    out.face = 0xfffffffeu;
    out.uv = world.xz;
    return out;
}

// Past ~40 m the ground fades into the colour the sky has
// just above the horizon (by day, at dusk and at night alike), so there is
// never a line between a dark field and the sky, haze or no haze. A plan or
// elevation view has no horizon: the ground stays as it is.
@fragment
fn fs_ground(in: Varying) -> @location(0) vec4<f32> {
    let c = shade(in, true, false).color;
    if is_ortho() {
        return vec4<f32>(c, 1.0);
    }
    let to = in.world - g.camera.xyz;
    let dist = length(to);
    // The horizon colour in this pixel's own view direction (from the
    // pixel, not the interpolated position: see below).
    let ndc = vec2<f32>(in.clip.x * g.viewport.z * 2.0 - 1.0, 1.0 - in.clip.y * g.viewport.w * 2.0);
    let p = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    let ray = p.xyz / p.w - g.camera.xyz;
    let flat = vec3<f32>(ray.x, 0.0, ray.z);
    let horizon = fog_inscatter(flat / max(length(flat), 1e-6), 1.0);
    // Over log distance from 40 m to 2 km: seen from eye height that spans a
    // few degrees below the horizon, wide enough not to read as a line.
    let fade = smoothstep(0.0, 1.0, log2(max(dist, 1.0) / 40.0) / log2(2000.0 / 40.0));
    // Fully faded, take the horizon as is: with MSAA a pixel on the far rim
    // shades at its centre, which may lie past the horizon, where the
    // interpolated position (and so the shading) runs off to huge values
    // that even a mix by 1 would not cancel.
    let ahead = dot(to, -normalize(vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]))) > 0.0;
    if fade > 0.999 || !ahead || !(dist < 30000.0) {
        return vec4<f32>(horizon, 1.0);
    }
    return vec4<f32>(mix(c, horizon, fade), 1.0);
}

// ---- planar reflection -----------------------------------------------------

// The scene seen in the water plane: drawn with the mirrored camera (lib.rs
// writes a second globals block), cut at the water height (a clip plane done
// by discard), alpha-tested cutouts, no screen-space effects. Shading comes
// first and the discard last, so derivatives stay in uniform control flow.
@fragment
fn fs_reflect(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let shaded = shade(in, front, false);
    let a = cutout_alpha(in.uv);
    if (g.reflection.z > 0.5 && in.world.y < g.reflection.w) || (material.alpha.x > 0.0 && a < material.alpha.x) {
        discard;
    }
    return vec4<f32>(shaded.color, 1.0);
}

// ---- transparent pass ------------------------------------------------------

// Haze in front of a premultiplied transparent surface: its own colour is
// attenuated and the in-scattered light added in proportion to its alpha;
// what shows through already carries the haze of its own, longer path.
fn fog_premultiplied(c: vec4<f32>, world: vec3<f32>) -> vec4<f32> {
    if g.fog.x <= 0.0 {
        return c;
    }
    let to = -to_eye(world);
    let t = length(to);
    let dir = to / max(t, 1e-6);
    let transmit = fog_transmittance(g.camera.y, dir.y, t);
    return vec4<f32>(c.rgb * transmit + fog_inscatter(dir, 1.0) * fog_near(transmit) * (1.0 - transmit) * c.a, c.a);
}

// Thin architectural glass, drawn in two passes (lib.rs): first the
// transmitted light, multiplying what is behind per channel (blend Zero,
// Src): 50 % at normal incidence for the two faces' absorption and an
// interior that reads as an interior in daylight, times the soda-lime
// green-blue tint raised to the path length through the pane (Beer-Lambert:
// 1 / cos theta, so edges and grazing views turn greener and darker); then
// the reflected light added on top (blend One, One): two interfaces of a
// thin slab (reflectance R' = 2R / (1 + R), from the geometric series of
// inter-reflections) with a floor of 10 % so a window shows a little sky
// even head-on, reflecting the environment cube, the sun and the lamps.
// base_color.rgb tints the pane's own scattered light, base_color.a is its
// opacity (frosted / opaque panes). No refraction offset.
struct GlassTerms {
    reflected: vec3<f32>,
    transmitted: vec3<f32>,
}

fn glass_terms(in: Varying) -> GlassTerms {
    let v = normalize(to_eye(in.world));
    var n = normalize(in.normal);
    if dot(n, v) < 0.0 {
        n = -n;
    }
    let nov = max(dot(n, v), 1e-4);
    let roughness = clamp(material.params.y - material.params.w, 0.02, 1.0);
    let f = f_schlick(nov, vec3<f32>(0.04)).r;
    let fr = max(2.0 * f / (1.0 + f), 0.1);
    let depth = -(g.view * vec4<f32>(in.world, 1.0)).z;
    let l = g.sun_dir.xyz;
    let nol = max(dot(n, l), 0.0);
    let a = roughness * roughness;
    var sun = vec3<f32>(0.0);
    if nol > 0.0 {
        let shadow = sun_shadow(in.world, n, nol, depth);
        sun = specular(n, v, l, nol, nov, vec3<f32>(0.04), a) * nol * g.sun_light.rgb * shadow * 2.0;
    }
    let lamps = local_lights(in.world, n, v, nov, vec3<f32>(0.0), vec3<f32>(0.04), a, false) * 2.0;
    let opacity = clamp(material.base_color.a, 0.0, 1.0);
    let lit = material.base_color.rgb * (sh_irradiance(n) / PI + g.sun_light.rgb * nol / PI);
    var out: GlassTerms;
    out.reflected = fr * environment(reflect(-v, n), roughness) + sun + lamps + (1.0 - fr) * opacity * lit;
    let tint = pow(vec3<f32>(0.88, 0.96, 0.94), vec3<f32>(1.0 / max(nov, 0.15)));
    out.transmitted = tint * ((1.0 - fr) * (1.0 - opacity) * 0.5);
    return out;
}

// Pass 1: what shows through (multiplied into the target).
@fragment
fn fs_glass_transmit(in: Varying) -> @location(0) vec4<f32> {
    if g.time.w > 0.5 {
        return vec4<f32>(0.65); // clay: a pale veil
    }
    return vec4<f32>(glass_terms(in).transmitted, 1.0);
}

// Pass 2: the reflection, plus the haze in front of the pane in proportion
// to how much the pane blocks.
@fragment
fn fs_glass_reflect(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if g.time.w > 0.5 {
        return vec4<f32>(shade(in, front, false).color * 0.35, 0.0);
    }
    let t = glass_terms(in);
    let block = 1.0 - dot(t.transmitted, vec3<f32>(1.0 / 3.0));
    let c = fog_premultiplied(vec4<f32>(t.reflected, block), in.world);
    return vec4<f32>(c.rgb, 0.0);
}

// Value noise in 2D with its analytic gradient (Inigo Quilez, "Value Noise
// Derivatives", 2008): returns (value, d/dx, d/dy).
fn noise_grad(p: vec2<f32>) -> vec3<f32> {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let a = hash2(i);
    let b = hash2(i + vec2<i32>(1, 0));
    let c = hash2(i + vec2<i32>(0, 1));
    let d = hash2(i + vec2<i32>(1, 1));
    let k = a - b - c + d;
    let value = a + (b - a) * u.x + (c - a) * u.y + k * u.x * u.y;
    let grad = du * vec2<f32>(b - a + k * u.y, c - a + k * u.x);
    return vec3<f32>(value, grad);
}

// Two octaves of the noise's gradient, rotated between octaves.
fn wave_slope(p: vec2<f32>) -> vec2<f32> {
    let rot = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
    let a = noise_grad(p).yz;
    let b = noise_grad(rot * p * 2.03 + 17.0).yz * 0.5;
    return a + rot * b * 2.03 * 0.5;
}

// Fraction of light left after `path` metres through water of turbidity
// 0..1 (Beer-Lambert with an absorption coefficient from 0.25 to 3 per
// metre); lib.rs has the same curve (`water_transmittance`) for tests.
fn water_transmittance(path: f32, turbidity: f32) -> f32 {
    return exp(-path * mix(0.25, 3.0, turbidity));
}

// Flat water (pond, pool): two scrolling procedural normal layers, Fresnel
// (looking down shows the tinted bottom, grazing angles the reflection),
// the sun's and the lamps' glints, and instead of refraction a depth-based
// absorption: the water path length comes from the linear depth of the
// opaque scene below (shallow edges clear, deep water takes base_color.rgb,
// the deep colour; base_color.a is the turbidity), with a soft fade where
// the water meets the shore. The reflection is the planar reflection pass
// when it rendered this water's plane, distorted by the ripples' slope,
// otherwise the environment cube.
fn water(in: Varying) -> vec4<f32> {
    let eye_vec = to_eye(in.world);
    let dist = length(eye_vec);
    let v = eye_vec / dist;
    let depth = -(g.view * vec4<f32>(in.world, 1.0)).z;
    let below = textureLoad(linear_depth, vec2<i32>(in.clip.xy), 0).r;
    let path = max(below - depth, 0.0) * dist / max(depth, 1e-3);
    let t = g.time.x;
    let wind = vec2<f32>(g.wind.x, g.wind.y);
    let calm = 0.35 + 0.65 * clamp(g.wind.z, 0.0, 2.0);
    let p = in.world.xz;
    let slope = wave_slope(p * 1.7 + wind * t * 0.35) * 0.08 + wave_slope(p * 5.3 - vec2<f32>(0.23, -0.19) * t) * 0.04;
    var n = normalize(vec3<f32>(-slope.x * calm, 1.0, -slope.y * calm));
    // Distant water is mirror-like: fade the waves as they shrink below a pixel.
    n = normalize(mix(n, vec3<f32>(0.0, 1.0, 0.0), clamp(depth / 120.0, 0.0, 0.8)));
    let nov = max(dot(n, v), 1e-4);
    let f = f_schlick(nov, vec3<f32>(0.02)).r;
    let r = reflect(-v, n);
    var reflected: vec3<f32>;
    if g.reflection.x > 0.5 && abs(in.world.y - g.reflection.y) < 0.05 {
        // The ripples' tilt in view space shifts the lookup, less with distance.
        let tilt = (g.view * vec4<f32>(n.x, 0.0, n.z, 0.0)).xy;
        let size = vec2<f32>(textureDimensions(planar_reflection));
        let uv = in.clip.xy * g.viewport.zw + tilt * vec2<f32>(1.0, -1.0) * 0.6 / max(depth, 1.0);
        reflected = textureSampleLevel(planar_reflection, env_sampler, clamp(uv, 0.5 / size, 1.0 - 0.5 / size), 0.0).rgb;
    } else {
        // A garden pond reflects hedges, walls and trees near the horizon,
        // not sky: the cube's reflection is darkened towards the horizon (a
        // crude local-horizon term), which also lets the ripples show.
        reflected = environment(r, 0.04) * mix(0.3, 1.0, smoothstep(0.02, 0.4, r.y));
    }
    let l = g.sun_dir.xyz;
    let nol = max(dot(n, l), 0.0);
    var glint = local_lights(in.world, n, v, nov, vec3<f32>(0.0), vec3<f32>(0.02), 0.01, false);
    var sun_on = 0.0;
    if nol > 0.0 {
        sun_on = sun_shadow(in.world, vec3<f32>(0.0, 1.0, 0.0), nol, depth);
        glint += specular(n, v, l, nol, nov, vec3<f32>(0.02), 0.02) * nol * g.sun_light.rgb * sun_on;
    }
    let transmit = water_transmittance(path, material.base_color.a);
    let deep = material.base_color.rgb * (sh_irradiance(vec3<f32>(0.0, 1.0, 0.0)) + g.sun_light.rgb * max(l.y, 0.0) * sun_on) / PI;
    let shore = smoothstep(0.0, 0.12, path);
    let color = (f * reflected + glint + (1.0 - f) * (1.0 - transmit) * deep) * shore;
    let alpha = (f + (1.0 - f) * (1.0 - transmit)) * shore;
    return fog_premultiplied(vec4<f32>(color, alpha), in.world);
}

// Premultiplied alpha out (blend: One, OneMinusSrcAlpha).
@fragment
fn fs_transparent(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Branches on uniforms only, so shade() may take derivatives.
    let kind = material.alpha.z;
    if g.time.w > 0.5 {
        // Clay: water as a pale matte surface (glass has its own passes).
        return vec4<f32>(shade(in, front, false).color, 1.0);
    }
    if kind > 1.5 {
        return water(in);
    }
    let shaded = shade(in, front, false);
    let alpha = clamp(shaded.alpha, 0.0, 1.0);
    return vec4<f32>(shaded.color * alpha, alpha);
}
