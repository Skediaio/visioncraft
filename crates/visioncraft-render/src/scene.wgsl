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
    flags: vec4<u32>,      // x texture bits (1 base, 2 normal, 4 orm, 8 roughness, 16 metallic, 32 occlusion, 64 height in the occlusion slot), y double-sided, z macro variation (f32 bits), w mowing stripes (cm | deg << 16)
    relief: vec4<f32>,     // x parallax depth of the height map (m; 0 off), yz true north's direction on the ground (x, z; unit), w puddles may form (0/1)
    aging: vec4<f32>,      // how strongly dirt, moss, wear and rain streaks show (0..1 each; all 0 off)
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
@group(3) @binding(2) var occlusion: texture_2d<f32>; // half (or full) res: r ambient occlusion, g contact shadow
@group(3) @binding(3) var linear_depth: texture_2d<f32>;
// The scene mirrored about the water plane (reflection pass), pre-exposed HDR.
@group(3) @binding(4) var planar_reflection: texture_2d<f32>;
// A photo's previous sample, pre-exposed HDR (1x1 outside photos and without screen-space
// reflections): the bounce light's source; with those, the opaque scene before the glass.
@group(3) @binding(5) var previous_hdr: texture_2d<f32>;

// MSAA sample count of the pipeline (depth pre-pass coverage mask).
override SAMPLES: u32 = 1u;

const HAS_BASE: u32 = 1u;
const HAS_NORMAL: u32 = 2u;
// ao_tex holds the height map (white = top) instead of an occlusion map.
const HAS_HEIGHT: u32 = 64u;

// ---- wind -----------------------------------------------------------------

// The wind sway itself is in common.wgsl (`wind_sway`), shared with the
// impostors. Every pass that draws a mesh calls this with the same inputs.
fn animated_world(position: vec3<f32>, wind: vec2<f32>, m: mat4x4<f32>, animate: bool) -> vec3<f32> {
    let p = (m * vec4<f32>(position, 1.0)).xyz;
    if !animate || wind.x <= 0.0 || g.wind_amp.x <= 0.0 {
        return p;
    }
    let origin = m[3].xyz;
    // m[0].w carries the plant's sway frequency (lib.rs `set_instances`).
    return origin + wind_sway(wind_state(), p - origin, origin, wind.x, wind.y, m[0].w);
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
    // Derivatives first, in uniform control flow (no early return before them).
    let a = cutout_alpha(in.uv) * edge_fade(in.world, normalize(to_eye(in.world)));
    let soft = edge_coverage(in.uv, in.world);
    let cut = a < material.alpha.x;
    let dither = ign(in.clip.xy) >= soft;
    if select(cut, dither, soft >= 0.0) {
        discard;
    }
}

// The samples of the pixel at `in.clip.xy` (its centre) that a cutout covers; 0 for none.
fn depth_cutout_mask(in: DepthOut) -> u32 {
    let a = cutout_alpha(in.uv) * edge_fade(in.world, normalize(to_eye(in.world)));
    let soft = edge_coverage(in.uv, in.world);
    // A soft edge covers `soft` of the samples; any other cutout as alpha to coverage.
    let n = select(
        alpha_samples(a, material.alpha.x),
        u32(round(soft * f32(SAMPLES))),
        soft >= 0.0,
    );
    return select(sample_run(n, u32(ign(in.clip.xy) * f32(SAMPLES))), 0u, n == 0u);
}

// Not on Vulkan: `coverage_per_sample.wgsl` (`coverage.rs`).
@fragment
fn fs_depth_cutout(in: DepthOut) -> @builtin(sample_mask) u32 {
    let m = depth_cutout_mask(in);
    if m == 0u {
        discard;
    }
    return m;
}

// The soft edge of a drawn area (`Area::edge_blend_m`): the share of the surface drawn, 0..1, or
// -1 for any other surface. Only triplanar materials carry it, in the UVs of the area's band
// triangles: x the edge value (0 on the outline, 1 at the inner side of the band), y = 1.
// The area gives way to what lies under it the way height-blended terrain layers do (Mishkinis,
// "Advanced Terrain Texture Splatting", Gamasutra 2013: the blend weight is shifted by each layer's
// height, so the transition breaks up along the surface's own relief instead of a straight
// cross-fade): the edge value, moved by a two-octave noise standing in for the height, gives the
// coverage; the depth pre-pass turns it into samples or discards (dithered), and the forward pass,
// testing depth Equal, inherits it.
// shortcut: noise, not the material's own height map (terrain.wgsl's layers have one); upgrade
// when area materials ship height maps.
fn edge_coverage(uv: vec2<f32>, world: vec3<f32>) -> f32 {
    if material.alpha.y <= 0.0 || uv.y < 0.5 {
        return -1.0;
    }
    let n = 0.6 * noise2(world.xz * 2.5, 7.0) + 0.4 * noise2(world.xz * 9.0, 11.0);
    return clamp(uv.x * 1.7 - 0.35 + (n - 0.5) * 0.7, 0.0, 1.0);
}

// `n` of the pixel's SAMPLES samples: a run starting at sample `start`,
// wrapping around.
fn sample_run(n: u32, start: u32) -> u32 {
    let full = (1u << SAMPLES) - 1u;
    if n == 0u {
        return 0u;
    }
    if n >= SAMPLES {
        return full;
    }
    let bits = (1u << n) - 1u;
    let r = start % SAMPLES;
    return ((bits << r) | (bits >> (SAMPLES - r))) & full;
}

// Alpha to coverage: how many samples a cutout alpha `a` covers, the alpha
// sharpened to a one-pixel ramp across the cutoff (Golus 2017).
fn alpha_samples(a: f32, cutoff: f32) -> u32 {
    let sharp = (a - cutoff) / max(fwidth(a), 1e-4) + 0.5;
    return u32(clamp(round(sharp * f32(SAMPLES)), 0.0, f32(SAMPLES)));
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

// A tree's shadow caster (flora's shadow.rs): covers `uv.x` of the shadow map's texels, chosen
// by a hash of the texel (white noise: a regular pattern showed as a checkerboard), which the
// percentage-closer filter averages to a partial shadow, darker for a denser crown.
@fragment
fn fs_shadow_dither(in: DepthOut) {
    let p = vec2<u32>(in.clip.xy);
    var h = p.x * 747796405u + p.y * 2891336453u + 1013904223u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    if (f32(h & 1023u) + 0.5) / 1024.0 >= in.uv.x {
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
    // 1 - noh^2 (1 - a2) is at least a2 for noh <= 1; with a glass pane's a2 (1.6e-7), rounding
    // in noh can cancel it to zero or below: an infinite or negative lamp highlight on glass
    // (in Chromium the estate's panes lit up with lamp glints this way).
    let f = max((noh * a2 - noh) * noh + 1.0, a2);
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
    // Shade is lit by more than the open sky: the walls, trees and ground round it pass the
    // sun's white on, which the two-band harmonics of an open site miss. Part of the sky's blue
    // goes, so a shadow reads cool, not blue (as a camera balanced for daylight shows it).
    let luma = dot(e, vec3<f32>(0.2126, 0.7152, 0.0722));
    e = mix(vec3<f32>(luma), e, AMBIENT_CHROMA);
    return max(e, vec3<f32>(0.0)) * g.night.w;
}

// How much of the sky's colour the ambient light keeps (see `sh_irradiance`).
const AMBIENT_CHROMA: f32 = 0.55;

// The mean reflectance of a room's surfaces (pale walls and ceiling, a wooden floor).
const ROOM_REFLECTANCE: f32 = 0.45;

// Irradiance bounced round an enclosed space, for a surface that sees only room (`volume_at`'s
// room light, gathered round the rooms on the CPU, sky_volume.rs): the sky's share per unit of
// its mean illuminance (half the up- and down-facing irradiance: the sky and the sunlit ground
// that windows show), and the lamps' and the sun patches' light in lux. The daylight passed
// round a room takes the colour of its surfaces, not of the sky (`ROOM_CHROMA`, `ROOM_TINT`).
// Nothing outside a volume.
fn room_bounce(room: vec4<f32>) -> vec3<f32> {
    if g.sky_vol_size.w < 0.5 {
        return vec3<f32>(0.0);
    }
    let sky = 0.5 * (sh_irradiance(vec3<f32>(0.0, 1.0, 0.0)) + sh_irradiance(vec3<f32>(0.0, -1.0, 0.0)));
    let luma = dot(sky, vec3<f32>(0.2126, 0.7152, 0.0722));
    let daylight = mix(vec3<f32>(luma), sky, ROOM_CHROMA) * max(room.a, 0.0);
    return (daylight + room.rgb * g.time.y) * ROOM_TINT;
}

// The room a reflection along `r` meets (radiance): a surface of the room's reflectance facing
// back along it, lit by the sky it sees from here (`vis`) and the light passed round (`bounce`).
fn room_seen(vis: vec4<f32>, bounce: vec3<f32>, r: vec3<f32>) -> vec3<f32> {
    let back = sky_seen(vis, -r);
    return (sh_irradiance(-r) * back + bounce * (1.0 - back)) * (ROOM_REFLECTANCE / PI);
}

// How much of the sky's colour the light passed round a room keeps after a few reflections off
// its surfaces, and the colour those reflections give it: pale walls and a wooden floor and
// ceiling, a mean reflectance of about (0.50, 0.45, 0.40) over several bounces, relative to the
// grey `ROOM_REFLECTANCE` the volume is gathered with.
const ROOM_CHROMA: f32 = 0.3;
const ROOM_TINT: vec3<f32> = vec3<f32>(1.15, 1.0, 0.86);

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
    if !FAST_SHADING && g.options.x > 4.5 {
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
    let div = i32(g.options.z);
    let u = (pixel - 0.5) / f32(div);
    let base = vec2<i32>(floor(u));
    let f = fract(u);
    var sum = vec2<f32>(0.0);
    var weight = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i & 1, i >> 1u);
        let q = clamp(base + o, vec2<i32>(0), size - 1);
        let d = textureLoad(linear_depth, min(q * div, full), 0).r;
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
// A tangent-space normal from its X and Y alone (Z rebuilt): the same for RGBA8 maps and
// two-channel BC5 ones, which keep no Z.
fn decode_normal(t: vec4<f32>) -> vec3<f32> {
    let xy = t.xy * 2.0 - 1.0;
    return vec3<f32>(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
}

fn triplanar_normal(l: Lookup, n: vec3<f32>, relief: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        if l.weight[i] > 0.0 {
            var tn = decode_normal(textureSampleGrad(normal_tex, normal_smp, l.uv[i], l.dx[i], l.dy[i]));
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

// Parallax occlusion on the Y (top) projection, which on ground up to ~20 degrees of slope is
// the only one `triplanar` keeps. Steep parallax (McGuire & McGuire, "Steep Parallax Mapping",
// I3D 2005 poster) with the last step interpolated between the two layers that straddle the
// height field (the parallax occlusion mapping of Tatarchuk, "Dynamic Parallax Occlusion
// Mapping with Approximate Soft Shadows", I3D 2006 / GDC 2006): layers along the tangent-space
// view ray, more of them at grazing angles, the mip level computed once before the loop
// (Tatarchuk's adaptive LOD) and the effect faded to plain normal mapping by mip level and at
// grazing view. The height map's white is the top, and the ray is pushed down from it.
// Shading only: no depth is written (the depth pre-pass and SSAO keep the flat surface) and
// the shadow map is looked up at the flat point. Then a second short march from the hit
// towards the sun (Tatarchuk's soft self-shadowing, a separate ray as in Policarpo, Oliveira
// & Comba, "Real-Time Relief Mapping on Arbitrary Polygonal Surfaces", I3D 2005): the most
// the relief rises above that ray, over the distance along it, shadows the sun's direct light.
struct Parallax {
    uv: vec2<f32>,
    shadow: f32,
}

// Guesses, tuned on renders: the march is full strength up to mip LOD 1 of the height map and
// gone by LOD 3, and fades out below 6 degrees of view elevation; the ray's run per unit of
// depth is capped (Welsh's offset limiting, loosened to 3: a ratio of 1 flattened the relief
// at a standing eye's view of the ground).
const PARALLAX_LOD: vec2<f32> = vec2<f32>(1.0, 3.0);
const PARALLAX_RUN: f32 = 3.0;

fn parallax(l: Lookup, v: vec3<f32>, n: vec3<f32>, depth_uv: f32) -> Parallax {
    var out = Parallax(l.uv[1], 0.0);
    let t = l.t[1];
    let b = l.b[1];
    let axis = cross(t, b);
    // Image directions: u along t, v against b (as `triplanar` maps them).
    let vt = vec3<f32>(dot(v, t), -dot(v, b), dot(v, axis));
    let size = vec2<f32>(textureDimensions(ao_tex));
    let fx = l.dx[1] * size;
    let fy = l.dy[1] * size;
    // The finer of the two footprint axes: at a standing eye's grazing view the coarser one
    // faded the relief out a few metres away.
    let lod = max(0.0, 0.5 * log2(min(dot(fx, fx), dot(fy, fy))));
    let fade = (1.0 - smoothstep(PARALLAX_LOD.x, PARALLAX_LOD.y, lod)) * smoothstep(0.05, 0.1, vt.z);
    if fade <= 0.0 {
        return out;
    }
    let run = vt.xy / vt.z;
    let full = -run * min(1.0, PARALLAX_RUN / max(length(run), 1e-4)) * depth_uv * fade;
    let lo = 4.0;
    let hi = select(16.0, 8.0, FAST_SHADING);
    let count = i32(mix(hi, lo, vt.z));
    let dl = 1.0 / f32(count);
    var uv = l.uv[1];
    var depth = 0.0;
    var field = 1.0 - textureSampleLevel(ao_tex, ao_smp, uv, lod).r;
    var prev_uv = uv;
    var prev_gap = field;
    for (var i = 0; i < count; i++) {
        if depth >= field {
            break;
        }
        prev_uv = uv;
        prev_gap = field - depth;
        uv += full * dl;
        depth += dl;
        field = 1.0 - textureSampleLevel(ao_tex, ao_smp, uv, lod).r;
    }
    // Where the segment between the last two layers crosses the height field.
    let gap = field - depth;
    let k = clamp(prev_gap / max(prev_gap - gap, 1e-5), 0.0, 1.0);
    out.uv = mix(prev_uv, uv, k);
    let hit = clamp(mix(depth - dl, depth, k), 0.0, 1.0);
    let sun = g.sun_dir.xyz;
    if FAST_SHADING || dot(n, sun) <= 0.0 || sun.y <= 0.0 || hit < 0.02 {
        return out;
    }
    let st = vec3<f32>(dot(sun, t), -dot(sun, b), dot(sun, axis));
    let srun = st.xy / max(st.z, 1e-3);
    let up = srun * min(1.0, PARALLAX_RUN / max(length(srun), 1e-4)) * depth_uv * fade * hit;
    var block = 0.0;
    for (var i = 1; i <= 6; i++) {
        let f = f32(i) / 6.0;
        let ray = hit * (1.0 - f);
        let d = 1.0 - textureSampleLevel(ao_tex, ao_smp, out.uv + up * f, lod).r;
        // Guess: blocked fully where the relief stands a quarter of the way along above the ray.
        block = max(block, (ray - d) / (hit * f) * 4.0);
    }
    out.shadow = clamp(block, 0.0, 1.0) * fade;
    return out;
}

// Value noise for the weathering pattern: trilinear over hashed lattice
// values, smoothstep-faded. The corners are `hash3`'s values with its nested hashes of z and of
// (y, z) shared by the corners that have them in common: 14 hashes instead of 24, same bits.
fn value_noise(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x = u32(i.x);
    let y = u32(i.y);
    let z0 = pcg(u32(i.z));
    let z1 = pcg(u32(i.z) + 1u);
    let y00 = pcg(y + z0);
    let y10 = pcg(y + 1u + z0);
    let y01 = pcg(y + z1);
    let y11 = pcg(y + 1u + z1);
    let n000 = unit(pcg(x + y00));
    let n100 = unit(pcg(x + 1u + y00));
    let n010 = unit(pcg(x + y10));
    let n110 = unit(pcg(x + 1u + y10));
    let n001 = unit(pcg(x + y01));
    let n101 = unit(pcg(x + 1u + y01));
    let n011 = unit(pcg(x + y11));
    let n111 = unit(pcg(x + 1u + y11));
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
    self_shadow: f32,    // 0 lit .. 1: the sun's direct light blocked by the height map's relief
    puddle: f32,         // 0..1: how far puddles may form here when it is wet (0 on lawns, leaves, glass; wet.wgsl)
}

// Base colour, normal and the occlusion / roughness / metallic maps, for
// either projection. Called from uniform control flow (it takes derivatives). `ground`: the
// endless ground's entry points, the only ones that paint the surroundings (a constant at each
// call, so the other entry points compile without that code).
fn surface(in: Varying, front: bool, ground: bool) -> Surface {
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
    if (bits & HAS_HEIGHT) != 0u && material.relief.x > 0.0 && triplanar_on && l.weight.y > 0.999 {
        let r = parallax(l, normalize(to_eye(in.world)), n, material.relief.x * material.alpha.y);
        l.uv[1] = r.uv;
        s.self_shadow = r.shadow;
    }
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
            var tn = decode_normal(textureSampleGrad(normal_tex, normal_smp, l.uv[0], l.dx[0], l.dy[0]));
            tn = vec3<f32>(tn.xy * relief, tn.z);
            s.n = uv_normal(tn, n, in.world, in.uv);
        }
    }
    s.emissive = material.emissive.rgb;
    s.puddle = select(0.0, material.relief.w, material.alpha.z == 0.0 && material.alpha.w == 0.0);
    s = macro_variation(s, in.world, bitcast<f32>(material.flags.z));
    if ground && in.face == GROUND_FACE {
        s = surroundings(s, in.world, length(dwx) + length(dwy));
        // The endless ground is grass, fields and woods: no puddles.
        s.puddle = 0.0;
    }
    // Mowing stripes: the blades of each pass lean the way the mower went, so a band is lighter
    // seen along its lean and darker seen against it, alternating band by band.
    let stripes = material.flags.w;
    if stripes != 0u {
        let width = max(f32(stripes & 0xFFFFu) / 100.0, 0.1);
        let a = radians(f32(stripes >> 16u));
        let along = vec2<f32>(sin(a), -cos(a));
        let across = vec2<f32>(-along.y, along.x);
        let band = i32(floor(dot(in.world.xz, across) / width));
        let lean = select(-1.0, 1.0, (band & 1) == 0);
        let to_eye_xz = to_eye(in.world).xz;
        let look = dot(normalize(to_eye_xz + vec2<f32>(1e-4)), along);
        s.base = vec4<f32>(s.base.rgb * (1.0 + 0.14 * lean * clamp(look * 1.5, -1.0, 1.0)), s.base.a);
    }
    // A gust over a lawn: paler, glossier grass where it passes (`gust_sheen`). Lawn tufts and
    // the lawn's ground are materials; the terrain's layers keep their look.
    if g.gust.z > 0.0 {
        let opaque = material.alpha.z == 0.0 && material.alpha.w == 0.0;
        let sheen = gust_sheen(in.face, in.world, !opaque || material.relief.w > 0.0, n.y);
        if sheen > 0.0 {
            let c = s.base.rgb;
            s.base = vec4<f32>(mix(c, vec3<f32>(luminance(c)), 0.35 * sheen) * (1.0 + 1.1 * sheen), s.base.a);
        }
    }
    return s;
}

// Macro variation against the carpet look of large single-material
// surfaces: two octaves of world-space value noise (features of ~5 and
// ~15 m) modulate brightness and roughness, a third paints drier, yellower
// patches, and fields of ~100 m keep a large ground from being one flat
// colour in the middle distance where the finer octaves average out.
// The octaves run on turned axes: on the world's own axes the lattice of the value noise showed
// from above as a regular grid of blotches.
const MACRO_TURN_A = mat3x3<f32>(vec3<f32>(0.8, 0.0, 0.6), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(-0.6, 0.0, 0.8));
const MACRO_TURN_B = mat3x3<f32>(vec3<f32>(0.28, 0.0, -0.96), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.96, 0.0, 0.28));
fn macro_variation(surf: Surface, world: vec3<f32>, variation: f32) -> Surface {
    var s = surf;
    if variation > 0.0 {
        let m = value_noise(world * 0.21) * 0.65 + value_noise(MACRO_TURN_A * world * 0.067 + 31.0) * 0.35 - 0.5;
        let big = value_noise(MACRO_TURN_B * world * 0.011 + 3.0) - 0.5;
        let dry = smoothstep(0.5, 0.85, value_noise(MACRO_TURN_B * world * 0.13 + 7.0)) * variation;
        // Subtle: from a few tens of metres up stronger patches read as a blotched carpet.
        var c = s.base.rgb * (1.0 + m * 0.4 * variation) * (1.0 + big * 0.4 * variation);
        c = mix(c, c * vec3<f32>(1.15, 1.05, 0.7), dry * 0.5);
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

// Ageing of standard surfaces: dirt, moss, wear and rain streaks, procedural and in world space
// (no textures: a few value-noise lookups), changing albedo, roughness and metallic only, so the
// lighting stays correct. `k` holds how strongly each shows (the material's `weathering` and the
// view's, already combined by the app); all zero costs nothing (the caller skips this). A look,
// not a physical model: the pieces follow what is seen on old walls and paving. Dirt collects
// in the splash zone above the ground (`g.options.w`, the ground plane: a hill's own ground is not
// known here), in crevices (ambient occlusion) and under overhangs; moss grows on up-facing
// surfaces and on the wall side away from the sun (`north`, true north in the drawing; the app
// turns it to the south in the southern hemisphere), low down and where it is damp, in patches,
// more in the crevices; wear lightens and smooths up-facing patches where feet go; streaks
// run down walls in stretched noise.
struct Aging {
    k: vec4<f32>,     // dirt, moss, wear, streaks
    north: vec2<f32>, // unit direction of true north on the ground (world x, z)
}
const NO_AGING = Aging(vec4<f32>(0.0), vec2<f32>(0.0, -1.0));
struct Aged {
    albedo: vec3<f32>,
    roughness: f32,
    metallic: f32,
}

fn age(base: vec3<f32>, roughness: f32, metallic: f32, world: vec3<f32>, n: vec3<f32>, ao: f32, aging: Aging, snow: f32) -> Aged {
    var out = Aged(base, roughness, metallic);
    let k = aging.k;
    // Where the surface faces and how far above the ground it is.
    let up = smoothstep(0.55, 0.92, n.y);
    let wall = 1.0 - smoothstep(0.25, 0.65, abs(n.y));
    let down = max(-n.y, 0.0);
    let h = max(world.y - g.options.w, 0.0);
    // Patches of a metre or so, and the finer breakup inside them.
    let broad = value_noise(world * 0.65);
    let fine = value_noise(world * 3.7 + vec3<f32>(11.0, 3.0, 7.0));
    let crevice = clamp((1.0 - ao) * 1.6, 0.0, 1.0);

    // Wear first: it scrubs the dirt and moss off.
    var worn = 0.0;
    if k.z > 0.0 && up > 0.0 {
        let tread = value_noise(world * vec3<f32>(0.45, 0.45, 0.45) + vec3<f32>(5.0, 1.0, 13.0));
        worn = up * smoothstep(0.62 - 0.3 * k.z, 0.82 - 0.2 * k.z, tread * 0.75 + fine * 0.25) * min(k.z * 1.6, 1.0);
    }

    // Dirt: darker, greyer, warm.
    let splash = exp(-h / 0.9) * (0.65 + 0.7 * fine);
    let grime = clamp(splash * (0.35 + 1.2 * wall + 0.3 * up) + crevice * 0.7 + down * 0.6 + 0.16 * broad, 0.0, 1.0);
    var dirt = clamp(k.x * grime * 1.5, 0.0, 0.85) * (1.0 - 0.7 * worn);

    // Rain streaks: stretched along y, strongest high on a wall where the water leaves the top
    // edge and the sills, gone near the splash zone's own darkening.
    if k.w > 0.0 && wall > 0.0 {
        let across = dot(world.xz, vec2<f32>(-n.z, n.x));
        let into = dot(world.xz, n.xz);
        let run = value_noise(vec3<f32>(across * 8.0, world.y * 0.32, into * 0.5 + 9.0));
        let thin = value_noise(vec3<f32>(across * 17.0 + 3.0, world.y * 0.9, into * 0.5));
        let streak = smoothstep(0.62 - 0.1 * k.w, 0.86, run * 0.7 + thin * 0.3);
        let sheltered = 1.0 - 0.6 * crevice;
        dirt = max(dirt, clamp(k.w * 1.1 * streak * wall * sheltered * smoothstep(0.2, 2.5, h), 0.0, 0.7));
    }

    // Moss: low, damp and shaded; patchy; thickest in the joints.
    var moss = 0.0;
    if k.y > 0.0 {
        let shade = dot(normalize(n.xz + vec2<f32>(1e-4, 0.0)), aging.north) * 0.5 + 0.5;
        // Up-facing surfaces take it anywhere (a little less in the open sun, the south side of
        // a wall's foot); a wall only on its shaded side and low down.
        let suits = max(up * (0.55 + 0.45 * shade), wall * shade * shade * (1.0 - smoothstep(0.15, 1.6, h)));
        let damp = mix(0.55, 1.0, 1.0 - smoothstep(0.1, 1.2, h)) + 0.35 * crevice;
        // Ragged edges: a finer grain across the patches' rims.
        let grain = value_noise(world * 13.0 + vec3<f32>(2.0, 9.0, 4.0));
        let clump = broad * 0.55 + fine * 0.3 + (grain - 0.5) * 0.3 + 0.3 * crevice * suits;
        let edge = 0.62 - 0.4 * k.y;
        moss = smoothstep(edge, edge + 0.12, clump * damp) * suits * (1.0 - 0.85 * worn);
        moss = clamp(moss * min(k.y * 2.6, 1.0), 0.0, 0.9);
    }

    var c = out.albedo;
    // Wear: lighter and smoother, like a polished stone.
    c = mix(c, c * 1.22 + vec3<f32>(0.02), worn * 0.8);
    // Dirt: towards a warm grey-brown of under half the brightness.
    let l = luminance(c);
    c = mix(c, vec3<f32>(l) * vec3<f32>(0.5, 0.44, 0.37) + vec3<f32>(0.004, 0.003, 0.0), dirt * 0.95);
    // Moss: dark olive green, browner and drier at the fringe of a patch.
    let mossy = mix(vec3<f32>(0.06, 0.065, 0.02), vec3<f32>(0.03, 0.07, 0.012), smoothstep(0.1, 0.6, moss));
    c = mix(c, mossy * (0.7 + 0.7 * fine), moss);
    let keep = 1.0 - snow;
    out.albedo = mix(base, c, keep);
    out.roughness = mix(roughness, clamp(roughness + 0.12 * dirt + (0.95 - roughness) * moss - 0.2 * worn, 0.05, 1.0), keep);
    out.metallic = mix(metallic, metallic * (1.0 - clamp(dirt + moss, 0.0, 1.0)), keep);
    return out;
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
fn shade(in: Varying, front: bool, screen_effects: bool, ground: bool) -> Shaded {
    // Glass drawn as an opaque pane (the water's reflection): what the glass passes show, its
    // reflection over the dim room behind it, so the reflected house matches the house.
    if material.alpha.z == 1.0 && g.time.w < 0.5 {
        let t = glass_terms(in);
        let room = t.transmitted * ROOM_ALBEDO * sh_irradiance(normalize(in.normal)) / PI;
        return Shaded(apply_fog(t.reflected + room, in.world), 1.0);
    }
    var s = surface(in, front, ground);
    // Leaves, and the branches among them (both tagged by the app).
    s.ao *= crown_sky(in.face);
    // Standard surfaces age (no glass, water or leaves).
    var aging = NO_AGING;
    if material.alpha.z == 0.0 && material.alpha.w == 0.0 {
        aging = Aging(material.aging, material.relief.yz);
    }
    return light(in, s, material.alpha.w > 0.0, material.emissive.w, aging, screen_effects);
}

// A plant's leaves carry the sky their place in the crown sees (the app's crown_occlusion)
// in a tagged face id: without it, every leaf took the full sky and crowns read pale and
// flat from above.
const CROWN_TAG: u32 = 0xC0000000u;
// A thin leaf's diffuse transmittance relative to its reflectance (the albedo), and its hue:
// the light through a leaf is greener than the light off it: chlorophyll absorbs red and blue
// on both passes, so a green leaf's transmittance relative to its reflectance is about 1 at
// 550 nm, a half at 670 nm and a quarter at 450 nm (Jacquemoud & Baret 1990, PROSPECT spectra).
// The earlier yellow tint (red kept at 0.9 of green) made sunlit crowns read lime-yellow.
const LEAF_T: f32 = 0.8;
const LEAF_T_TINT = vec3<f32>(0.48, 0.95, 0.29);
// Lawn tufts carry the same, tagged apart (ground-level shade at their base; the app's
// `tuft_occlusion`): snow, which settles on a crown's leaves, leaves them alone.
const TUFT_TAG: u32 = 0x80000000u;
fn crown_sky(face: u32) -> f32 {
    if (face & 0xFFFF0000u) == CROWN_TAG || (face & 0xFFFF0000u) == TUFT_TAG {
        return f32(face & 0xFFFFu) / 65535.0;
    }
    return 1.0;
}

// A gust over a lawn: the blades bow over and turn their paler, glossier sides and the light of
// the sky to the eye, so a gust reads as a lighter wave travelling across the grass (the
// familiar look of wind over a field). 0..1: strongest at the tufts' tips, which bend most; the
// lawn's ground under them (the only opaque materials that never take puddles, `other` false
// for any else, facing up) shows the sward's average, about half of it.
fn gust_sheen(face: u32, world: vec3<f32>, other: bool, up: f32) -> f32 {
    var reach = 0.0;
    if (face & 0xFFFF0000u) == TUFT_TAG {
        reach = smoothstep(0.45, 0.95, crown_sky(face));
    } else if !other && up > 0.8 {
        reach = 0.5;
    }
    if reach <= 0.0 {
        return 0.0;
    }
    return g.gust.z * gust_field(wind_state(), world.xz) * reach;
}

// Lighting of a surface (any source: a material, the terrain layers, an
// impostor atlas), without reading the material uniform.
fn light(in: Varying, surf: Surface, foliage: bool, weathering: f32, aging: Aging, screen_effects: bool) -> Shaded {
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
    // Snow over the surface (`snow_lying`): matt and smoother, its bumps buried; its albedo goes
    // over the surface's after weathering below.
    var snow = 0.0;
    // On leaves only where a crown carries them (a hedge, an evergreen; tagged by the app, or an
    // impostor): bare twig cards, lawn blades and fallen leaves stay as they are.
    let leafy = !foliage || (in.face & 0xFFFF0000u) == CROWN_TAG || in.face == 0xfffffffdu;
    if !clay && g.snow.x > 0.0 && leafy {
        // One `snow_lying` for both (a select of the two calls compiled both).
        let up = select(snow_up(s.geometric), snow_up_leaves(s.geometric, in.face), foliage);
        let lying = snow_lying(in.world, up);
        // The band under a ledge's snow is for built things: a plant's (tagged) bends and
        // leaves are no ledges.
        let plant = (in.face & 0xFFFF0000u) == CROWN_TAG;
        snow = max(lying, select(snow_edge(in.world, s.geometric), 0.0, plant));
        s.metallic = mix(s.metallic, 0.0, snow);
        s.roughness = mix(s.roughness, 0.6, snow);
        s.n = normalize(mix(s.n, s.geometric, snow * 0.8));
    }
    var base = s.base.rgb;
    if in.face == g.highlight.y && draw.mesh.x == g.highlight.x {
        base = mix(base, vec3<f32>(0.25, 0.55, 1.0), 0.35);
    }
    // Wet surfaces and puddles (wet.wgsl): the wetness here and the puddle on it, and a puddle's
    // flat normal with the rain's ripples. The darker, glossier surface follows the weathering.
    var wet = vec2<f32>(0.0);
    if g.wet.x > 0.0 && !clay {
        wet = wet_state(in.world, s.geometric, s.puddle, foliage, snow);
        if wet.y > 0.0 {
            let ripple = rain_ripple_slope(in.world.xz, g.wet.y, 1.0 - smoothstep(6.0, 24.0, length(to_eye(in.world))));
            s.n = normalize(mix(s.n, normalize(s.geometric + vec3<f32>(-ripple.x, 0.0, -ripple.y)), wet.y));
        }
    }
    let n = s.n;
    let v = normalize(to_eye(in.world));
    let l = g.sun_dir.xyz;
    // Bent leaf normals often face away from the viewer; clamping n.v keeps
    // those cards from turning into grazing-angle mirrors of the sky.
    let nov = select(max(dot(n, v), 1e-4), max(abs(dot(n, v)), 0.35), foliage);

    let depth = -(g.view * vec4<f32>(in.world, 1.0)).z;
    var screen = vec2<f32>(1.0);
    if screen_effects {
        screen = upsample_occlusion(in.clip.xy, depth);
    }
    let ao = screen.r * s.ao;
    if !clay {
        base = weather(base, in.world, s.geometric, ao, weathering);
        if aging.k.x + aging.k.y + aging.k.z + aging.k.w > 0.0 {
            let aged = age(base, s.roughness, s.metallic, in.world, s.geometric, ao, aging, snow);
            base = aged.albedo;
            s.roughness = aged.roughness;
            s.metallic = aged.metallic;
        }
        if foliage {
            // Leaves differ in hue across a crown: a slow, seamless drift between a yellower
            // and a bluer, darker green over half a metre or so.
            let w = in.world;
            let k = sin(w.x * 3.1 + 1.7 * sin(w.z * 2.3)) * sin(w.z * 2.7 + w.y * 1.9);
            base *= vec3<f32>(1.0 + 0.16 * k, 1.0 + 0.05 * k, 1.0 - 0.14 * k);
        }
    }
    var wet_boost = 1.0;
    if wet.x > 0.0 {
        let wetted = wet_apply(base, s.roughness, s.metallic, wet.x, wet.y, foliage);
        base = wetted.albedo;
        s.roughness = wetted.roughness;
        s.metallic = wetted.metallic;
        wet_boost = wetted.boost;
    }
    base = mix(base, SNOW_ALBEDO, snow);
    // After the weathering, which changes the roughness.
    let a = s.roughness * s.roughness;
    // Leaves have a waxy but thin cuticle: half the specular of other dielectrics.
    let f0 = mix(vec3<f32>(select(0.04, 0.02, foliage)), base, s.metallic);
    let diffuse_color = base * (1.0 - s.metallic);

    var sun = vec3<f32>(0.0);
    // Light through a leaf is yellower than off it; clay stays grey.
    let t_tint = select(LEAF_T_TINT, vec3<f32>(LEAF_T), clay);
    // How much of the sun reaches the surface (1 without a sun): snow in shade turns blue-grey.
    var sunlit = 0.0;
    let nol_raw = dot(n, l);
    let nol = max(nol_raw, 0.0);
    if g.sun_light.r + g.sun_light.g + g.sun_light.b > 0.0 && (nol > 0.0 || foliage) {
        let shadow = min(sun_shadow(in.world, s.geometric, max(dot(s.geometric, l), 0.0), depth), screen.g) * (1.0 - s.self_shadow);
        sunlit = shadow * smoothstep(0.0, 0.15, nol);
        var diffuse = nol;
        // Snow is no Lambertian surface: its grains scatter a low sun forward and up, so a
        // snowfield under a winter sun is far brighter than its cosine says (and than its shade).
        if snow > 0.0 {
            diffuse = mix(nol, min(sqrt(nol) * 1.6, 1.0), snow);
        }
        if foliage {
            // A thin leaf reflects and transmits about equally (green leaf: reflectance and
            // transmittance both near 0.1; Jacquemoud & Baret, "PROSPECT: A model of leaf
            // optical properties spectra", Remote Sens. Environ. 34, 1990), so the side the sun
            // is on matters little: the side facing the viewer reflects, the other transmits
            // (`LEAF_T`, greener). A card is a cluster of leaves facing every way, and with a
            // spherical leaf-angle distribution half the irradiance falls on them whatever the
            // card's bent normal (G = 0.5; J. Ross, "The Radiation Regime and Architecture of
            // Plant Stands", 1981, ch. 2): the cosine is half the card's, half that mean.
            diffuse = mix(abs(nol_raw), 0.5, 0.5) * mix(1.0, LEAF_T, 1.0 - smoothstep(-0.1, 0.1, nol_raw * dot(n, v)));
            // Forward scattering through the leaf toward the viewer when looking into the sun:
            // the translucency lobe of Barré-Brisebois & Bouchard, "Approximating Translucency
            // for a Fast, Cheap and Convincing Subsurface Scattering Look", GDC 2011
            // (distortion 0.3, power 4), in the transmitted hue.
            let lt = normalize(l + n * 0.3);
            let through = pow(clamp(dot(v, -lt), 0.0, 1.0), 4.0) * 0.35;
            sun += diffuse_color * t_tint * through * g.sun_light.rgb * shadow;
        }
        var fr = vec3<f32>(0.0);
        // No sun glints off leaves facing away from the viewer.
        if nol > 0.0 && (!foliage || dot(n, v) > 0.0) {
            fr = specular(n, v, l, nol, nov, f0, a) * nol;
            if foliage {
                // A waxy leaf's sheen, not a mirror: grazing Fresnel would whiten crown tops.
                // A card is many leaves facing every way, so the sun's mirror image is spread
                // over them: kept below a third of a sunlit leaf's diffuse (about 0.1 / PI in
                // green). At 0.06 it was twice the diffuse, and white: pale crown tops from above.
                fr = min(fr, vec3<f32>(0.01 + 0.04 * wet.x));
            }
        }
        let dc = select(diffuse_color, diffuse_color * mix(vec3<f32>(1.0), t_tint / LEAF_T, 1.0 - smoothstep(-0.1, 0.1, nol_raw * dot(n, v))), foliage);
        sun += (dc / PI * diffuse + fr) * g.sun_light.rgb * shadow;
        // Snow sparkles: a few crystals catch the sun, fixed in place.
        if snow > 0.0 {
            let cell = floor(in.world * 180.0);
            let h = fract(sin(dot(cell, vec3<f32>(12.9898, 78.233, 37.719))) * 43758.547);
            sun += step(0.9985, h) * snow * nol * 3.0 * g.sun_light.rgb * shadow;
        }
        if foliage {
            // Leaves deep in a crown are mostly in the shade of the leaves above them, finer
            // than the shadow map resolves: their place in the crown (`crown_sky`, in the
            // surface's occlusion) dims the sun on them a little too. Only a little: the
            // shadow map already has the leaves above, and both together made crowns black.
            sun *= mix(1.0, s.ao, 0.25);
        }
    }

    // Ambient: diffuse irradiance from the sky's SH, specular from the
    // prefiltered environment cube with the split-sum BRDF fit.
    let r = reflect(-v, n);
    // Under a roof and between walls only the sky the openings show lights a surface; the rest
    // of its hemisphere is the room, lit by what comes in (`room_bounce`).
    let vol = volume_at(in.world, n);
    let vis = vol.sky;
    let seen = sky_seen(vis, n);
    let bounce = room_bounce(vol.room);
    var sky_irr = sh_irradiance(n);
    var ambient_ao = ao;
    var fill = vec3<f32>(0.0);
    if foliage && !clay {
        // A leaf is thin: of the sky light on its far side about as much comes through as its
        // near side reflects (diffuse transmission of thin leaves, as in Habel, Kusternig &
        // Wimmer, "Physically Based Real-Time Translucency for Leaves", EGSR 2007). Without it
        // the leaves under a crown, facing the dark ground, read near black.
        sky_irr = sky_irr + sh_irradiance(-n) * LEAF_T_TINT;
        // The crown's sky view (`crown_sky`, in s.ao) already is the occlusion by the other
        // leaves; screen-space AO of alpha-tested cards counts the same leaves again.
        ambient_ao = s.ao * mix(1.0, screen.r, 0.3);
        // Light scattered among the leaves (the canopy's multiple scattering, as in the
        // two-stream model of P. J. Sellers, "Canopy reflectance, photosynthesis and
        // transpiration", Int. J. Remote Sensing 6(8), 1985): a leaf deep in the crown sees
        // other leaves where it does not see sky, and they pass on about R + T = 2 x albedo of
        // the sun and sky falling on the crown, about half of them lit. Green, never black.
        let on_crown = g.sun_light.rgb * max(l.y, 0.0) + sh_irradiance(vec3<f32>(0.0, 1.0, 0.0));
        fill = diffuse_color * (1.0 + LEAF_T_TINT) * (diffuse_color * on_crown) * (1.0 - s.ao) / PI;
    }
    var ambient_diffuse = diffuse_color * (sky_irr * seen + bounce * (1.0 - seen)) / PI * ambient_ao + fill;
    // Photos from their second sample on: the diffuse light arriving from the scene itself,
    // gathered along two cosine-distributed rays per sample against the depth buffer and lit
    // by the previous sample (so bounces add up over the samples); where a ray leaves the
    // picture or finds nothing, the sky's light as before.
    if g.grade.w >= 1.0 {
        ambient_diffuse = diffuse_color * photo_bounce(in.world, n, in.clip.xy, ao);
    }
    // Snow out of the sun is blue-grey, clearly darker than sunlit snow: under a winter sun the
    // sky's light alone made shade and sun on snow almost the same white.
    if snow > 0.0 {
        ambient_diffuse *= mix(vec3<f32>(1.0), mix(vec3<f32>(0.75, 0.8, 0.92), vec3<f32>(1.0), sunlit), snow);
    }
    // Specular occlusion from AO: Lagarde & de Rousiers 2014, section 4.10.2.
    let spec_ao = clamp(pow(nov + ao, exp2(-16.0 * s.roughness - 1.0)) - 1.0 + ao, 0.0, 1.0);
    // A reflection that the sky volume says meets the room shows the room's own light, not sky.
    let seen_r = sky_seen(vis, r);
    var reflected = environment(r, s.roughness) * seen_r + room_seen(vis, bounce, r) * (1.0 - seen_r);
    // A mirror (drawn after the opaque scene, lib.rs `Material::mirror`) shows what the screen
    // does, else the sky or the room as its exact ray finds them.
    if screen_effects && s.metallic >= 0.5 && s.roughness <= 0.1 {
        let open = ray_open(in.world, r);
        reflected = environment(r, s.roughness) * open + room_seen(vis, bounce, r) * (1.0 - open);
        let ssr = screen_reflection(in.world, r);
        reflected = mix(reflected, ssr.rgb, ssr.a);
    } else if wet.y > 0.0 {
        // A puddle mirrors the sky, and the room's light where the sky volume says a wall or a
        // crown stands in the way (the screen-space copy of the scene is not there yet in this pass).
        let open = ray_open(in.world, r);
        let mirrored = environment(r, s.roughness) * open + room_seen(vis, bounce, r) * (1.0 - open);
        reflected = mix(reflected, mirrored, wet.y);
    }
    let ambient_specular = env_brdf_approx(f0, s.roughness, nov) * reflected * spec_ao * wet_boost;
    let local = local_lights(in.world, n, v, nov, diffuse_color, f0, a, foliage);
    let emitted = s.emissive * g.time.y;
    return Shaded(apply_fog(sun + ambient_diffuse + ambient_specular + local + emitted, in.world), s.base.a);
}

// ---- snow ------------------------------------------------------------------

// Whether the sky is open straight above `world` (1) or something lies over it (0: a roof, a
// canopy, a dense crown): the snow cover map, a depth map rendered looking straight down.
// Straight above decides; round it a ring of samples softens the edge of a cover inwards. A
// ring sample only counts what rises above an upward cone (a hand plus `SNOW_CONE` per metre
// out), so the next bump of a hedge top, the slope of a roof and the wall behind a sill do not
// cover the surface, while an eave or a crown over it does. Outside the map: open.
fn snow_open(world: vec3<f32>) -> f32 {
    if g.snow.z < 0.5 {
        return 1.0;
    }
    let p = g.snow_cover * vec4<f32>(world, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return 1.0;
    }
    // Depth per metre up, and metres per unit of the map's uv (orthographic: both constant).
    let dz = (g.snow_cover * vec4<f32>(0.0, 1.0, 0.0, 0.0)).z;
    let m_per_uv = 2.0 / max(abs(g.snow_cover[0].x), 1e-9);
    let layer = i32(g.snow.y);
    let above = textureSampleCompareLevel(shadow_map, shadow_sampler, uv, layer, p.z + dz * SNOW_HAND);
    var ring = 0.0;
    for (var k = 0; k < 8; k++) {
        let t = f32(k) * 0.7853982;
        let o = vec2<f32>(cos(t), sin(t)) * g.shadow.z * 2.5;
        let lift = SNOW_HAND + length(o) * m_per_uv * SNOW_CONE;
        ring += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, layer, p.z + dz * lift);
    }
    return max(above, (above + ring) / 9.0);
}

// How far above a surface the snow cover map is read, so the surface itself does not cover
// it (m), and how steeply the cone round it rises (m per m: a 50 degree roof does not cover
// itself).
const SNOW_HAND: f32 = 0.15;
const SNOW_CONE: f32 = 1.2;

// How much a surface facing `n` faces up for the snow (`snow_lying`): snow settles on what faces
// up (steeper as there is more of it), not under cover, broken up at its edge by noise into
// drifts and bare patches.
fn snow_up(n: vec3<f32>) -> f32 {
    let a = g.snow.x;
    // Flat ground takes it first, then roofs: at 0.6 a 30 degree roof is covered, a 38 degree
    // one (n.y 0.79) mostly with its tiles showing in patches, 48 degrees and steeper shed it.
    return smoothstep(mix(0.85, 0.55, a), mix(0.97, 0.8, a), n.y);
}

// Snow on a crown's leaves: a leaf card's own slope says little, so the leaves at the crown's
// top (the sky share the app gives them, scene.rs `crown_occlusion`; an impostor's are all
// top) that are not steep take it, where the sky above them is open.
fn snow_up_leaves(n: vec3<f32>, face: u32) -> f32 {
    let top = select(smoothstep(0.5, 0.75, crown_sky(face)), 1.0, face == 0xfffffffdu);
    return top * smoothstep(0.05, 0.45, abs(n.y));
}

// Snow over a surface that faces up by `up` (0..1).
fn snow_lying(world: vec3<f32>, up: f32) -> f32 {
    let a = g.snow.x;
    if a <= 0.0 || up <= 0.0 {
        return 0.0;
    }
    let nz = value_noise(world * 1.7) * 0.6 + value_noise(world * 7.3 + 11.0) * 0.4;
    let c = up * snow_open(world) * (0.45 + a) + (nz - 0.5) * 0.7 - 0.2;
    return smoothstep(0.0, 0.45, c) * smoothstep(0.0, 0.12, a);
}

// The height of the topmost surface over (x, z) by the snow cover map (an orthographic depth
// map looking straight down: its depth is linear in the height), or a large negative number
// outside it.
fn snow_top(world: vec3<f32>) -> f32 {
    if g.snow.z < 0.5 {
        return -1e9;
    }
    let p0 = g.snow_cover * vec4<f32>(world.x, 0.0, world.z, 1.0);
    let p1 = g.snow_cover * vec4<f32>(world.x, 1.0, world.z, 1.0);
    let uv = vec2<f32>(p0.x * 0.5 + 0.5, 0.5 - p0.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) || abs(p1.z - p0.z) < 1e-9 {
        return -1e9;
    }
    return snow_top_texel(p0.z, p1.z, uv, vec2<i32>(0));
}

// `snow_top` read from the texel `offset` away from the one under `uv`.
fn snow_top_texel(z0: f32, z1: f32, uv: vec2<f32>, offset: vec2<i32>) -> f32 {
    let dims = textureDimensions(shadow_map);
    let t = clamp(vec2<i32>(uv * vec2<f32>(dims)) + offset, vec2<i32>(0), vec2<i32>(dims) - 1);
    let d = textureLoad(shadow_map, t, i32(g.snow.y), 0);
    // Nothing drawn there (open sea beyond the terrain): unknown, not a floor at the map's far
    // plane (the sea took that for a shallow bed, a dark disc round the eye far out at sea).
    if d >= 0.99999 {
        return -1e9;
    }
    return (d - z0) / (z1 - z0);
}

// Snow's thickness seen from the side: the band just under a snowy top edge (a roof verge, a
// sill, a wall top, the coping), on the faces that drop from it.
fn snow_edge(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let a = g.snow.x;
    if a <= 0.0 || abs(n.y) > 0.35 {
        return 0.0;
    }
    if g.snow.z < 0.5 {
        return 0.0;
    }
    let inside = world - normalize(vec3<f32>(n.x, 0.0, n.z)) * 0.04;
    let p0 = g.snow_cover * vec4<f32>(inside.x, 0.0, inside.z, 1.0);
    let p1 = g.snow_cover * vec4<f32>(inside.x, 1.0, inside.z, 1.0);
    let uv = vec2<f32>(p0.x * 0.5 + 0.5, 0.5 - p0.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) || abs(p1.z - p0.z) < 1e-9 {
        return 0.0;
    }
    // The texel under the point and its four neighbours: a sill or a wall top narrower than a
    // texel lies in one of them, if not in the one under the point.
    let thick = 0.02 + 0.08 * a;
    var best = 0.0;
    var top = 0.0;
    var offsets = array<vec2<i32>, 5>(vec2<i32>(0), vec2<i32>(-1, 0), vec2<i32>(1, 0), vec2<i32>(0, -1), vec2<i32>(0, 1));
    for (var k = 0; k < 5; k++) {
        let below = snow_top_texel(p0.z, p1.z, uv, offsets[k]) - world.y;
        let band = (1.0 - smoothstep(thick * 0.6, thick, below)) * step(0.0, below);
        if band > best {
            best = band;
            top = below;
        }
    }
    if best <= 0.0 {
        return 0.0;
    }
    return best * snow_open(vec3<f32>(inside.x, world.y + top, inside.z));
}

// Fresh snow: a high, slightly blue-white albedo, matt with a soft sheen.
const SNOW_ALBEDO: vec3<f32> = vec3<f32>(0.80, 0.83, 0.88);

// One cosine-weighted ray from `p` along a direction about `n`: the radiance it brings (the
// previous sample's colour where it meets a surface in the depth buffer, else the sky, dimmed
// by `ao` for the occlusion the coarse march cannot see).
fn bounce_ray(p: vec3<f32>, n: vec3<f32>, r1: f32, r2: f32, ao: f32) -> vec3<f32> {
    let phi = 2.0 * PI * r1;
    let s = sqrt(r2);
    let t = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
    let a = normalize(cross(t, n));
    let b = cross(n, a);
    let d = normalize(a * cos(phi) * s + b * sin(phi) * s + n * sqrt(max(1.0 - r2, 0.0)));
    let full = vec2<f32>(textureDimensions(previous_hdr));
    let ld = vec2<f32>(textureDimensions(linear_depth));
    var dist = 0.08;
    for (var i = 0; i < 12; i++) {
        let q = p + n * 0.02 + d * dist;
        let c = g.view_proj * vec4<f32>(q, 1.0);
        if c.w <= 0.0 {
            break;
        }
        let ndc = c.xy / c.w;
        if any(abs(ndc) > vec2<f32>(1.0)) {
            break;
        }
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let behind = -(g.view * vec4<f32>(q, 1.0)).z - textureLoad(linear_depth, vec2<i32>(uv * ld), 0).r;
        if behind > 0.03 && behind < 0.6 + dist * 0.2 {
            return textureLoad(previous_hdr, vec2<i32>(uv * full), 0).rgb;
        }
        dist *= 1.45;
    }
    return sh_irradiance(d) / PI * ao;
}

fn photo_bounce(p: vec3<f32>, n: vec3<f32>, pixel: vec2<f32>, ao: f32) -> vec3<f32> {
    let k = g.grade.w;
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 2; i++) {
        let o = vec2<f32>(f32(i) * 37.0, f32(i) * 11.0);
        let r1 = fract(ign(pixel + o) + k * 0.618034);
        let r2 = fract(ign(pixel.yx + o + vec2<f32>(13.0, 7.0)) + k * 0.754878);
        sum += bounce_ray(p, n, r1, r2, ao);
    }
    return sum * 0.5;
}

@fragment
fn fs_main(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in, front, true, false).color, 1.0);
}

// ---- surroundings -----------------------------------------------------------

// The land round the design on the endless ground (lib.rs `Surroundings`): g.coast_info.w the
// kind (0 none, 1 fields, 2 woodland, 3 dunes), g.surround the plot's rectangle (min x, min z,
// max x, max z). Nothing changes inside the plot or 15 m round it; then the land fades in over 45 m (colour)
// and rises over a few hundred metres (relief), and never on a coast's sea side. Procedural,
// from world position only, so it costs a few noise lookups per pixel and vertex.

// Metres from the plot's rectangle (0 inside).
fn plot_distance(p: vec2<f32>) -> f32 {
    let q = max(max(g.surround.xy - p, p - g.surround.zw), vec2<f32>(0.0));
    return length(q);
}

fn surround_kind(p: vec2<f32>) -> u32 {
    let kind = u32(g.coast_info.w + 0.5);
    if kind == 0u || (g.coast_info.x > 1.5 && coast_distance(p) > 0.0) {
        return 0u;
    }
    return kind;
}

fn noise2(p: vec2<f32>, salt: f32) -> f32 {
    return value_noise(vec3<f32>(p.x, salt, p.y));
}

// Woods (0..1): blobs of a few hundred metres, sparse among fields, the rule in woodland.
fn wood_mask(p: vec2<f32>, kind: u32) -> f32 {
    let n = noise2(p / 700.0, 3.1) * 0.8 + noise2(p / 140.0, 5.7) * 0.2;
    let cut = select(0.6, 0.4, kind == 2u);
    return smoothstep(cut, cut + 0.03, n);
}

// Height of the surroundings over the ground plane (m), for `surround_kind(p)` (taken once
// per vertex: the coast test is the dear part).
fn surround_height(p: vec2<f32>, kind: u32) -> f32 {
    let d = plot_distance(p) - 15.0;
    if kind == 0u || d <= 0.0 {
        return 0.0;
    }
    if kind == 3u {
        // Dune ridges across the prevailing wind, 3-12 m, and a low swell.
        let q = vec2<f32>(p.x * 0.8 + p.y * 0.6, -p.x * 0.6 + p.y * 0.8);
        let ridge = 1.0 - abs(2.0 * noise2(q / vec2<f32>(260.0, 90.0), 1.3) - 1.0);
        return smoothstep(0.0, 300.0, d) * (3.0 + 9.0 * noise2(p / 600.0, 2.2)) * ridge * ridge;
    }
    // Gently rolling within the first kilometre, hills on the horizon from a few km out, and
    // the woods' canopy standing 14 m over the fields.
    let roll = smoothstep(0.0, 900.0, d) * 14.0 * noise2(p / 1100.0, 0.7);
    let hill = noise2(p / 5000.0, 9.4);
    let hills = smoothstep(2000.0, 9000.0, d) * 160.0 * hill * hill;
    return roll + hills + smoothstep(0.0, 120.0, d) * 14.0 * wood_mask(p, kind);
}

// Fraction of a pixel (`fw` m wide) covered by a line of half width `hw` at `dist` m from its
// centre line, falling back to the line's mean share of a `cell` when the pixel grows wider
// than the line (no shimmer far away).
fn line_cover(dist: f32, hw: f32, fw: f32, cell: f32) -> f32 {
    let sharp = clamp((hw - dist) / max(fw, 1e-3) + 0.5, 0.0, 1.0);
    return mix(sharp, 2.0 * hw / cell, smoothstep(hw, 6.0 * hw, fw));
}

// A parcel of the fields (m). A module constant, not a `let` in `surroundings`: naga's override
// processing (wgpu's, for every pipeline whose entry point reaches an override) emitted that
// `let` twice and failed validation.
const PARCEL_M: vec2<f32> = vec2<f32>(230.0, 150.0);

fn surroundings(surf: Surface, world: vec3<f32>, fw: f32) -> Surface {
    var s = surf;
    let p = world.xz;
    let kind = surround_kind(p);
    let fade = smoothstep(15.0, 60.0, plot_distance(p));
    if kind == 0u || fade <= 0.0 {
        return s;
    }
    let base = s.base.rgb;
    var c = base;
    var rough = s.roughness;
    let wood = wood_mask(p, kind);
    // The canopy's texture only where a wood or hedge needs it (noise is the cost here).
    var canopy = vec3<f32>(0.03, 0.05, 0.02);
    if kind == 3u {
        // Sand with marram grass, scrub and pines in patches.
        let grass = smoothstep(0.42, 0.62, noise2(p / 55.0, 6.6) * 0.7 + noise2(p / 12.0, 7.7) * 0.3);
        let scrub = smoothstep(0.66, 0.72, noise2(p / 70.0, 8.8) * 0.75 + noise2(p / 9.0, 9.9) * 0.25);
        c = mix(base * (0.9 + 0.2 * noise2(p / 30.0, 1.9)), vec3<f32>(0.11, 0.12, 0.06), grass * 0.8);
        c = mix(c, vec3<f32>(0.05, 0.07, 0.03) * (0.8 + 0.4 * noise2(p / 4.0, 3.7)), scrub * 0.85);
    } else {
        // Fields: a grid of parcels of 160-280 m turned a little region by region; each its own
        // crop, worked in lines; hedgerows along most borders.
        let a = (noise2(p / 3500.0, 2.8) - 0.5) * 1.4;
        let q = vec2<f32>(cos(a) * p.x - sin(a) * p.y, sin(a) * p.x + cos(a) * p.y);
        // Rows of parcels, each row shifted by its own amount, and some parcels split in two
        // along their length: no checkerboard.
        let row = floor(q.y / PARCEL_M.y);
        let shifted = vec2<f32>(q.x + hash2(vec2<i32>(i32(row), 71)) * PARCEL_M.x, q.y);
        var id = vec2<i32>(floor(shifted / PARCEL_M));
        var f = fract(shifted / PARCEL_M) * PARCEL_M;
        let cut = 0.3 + 0.4 * hash2(id + vec2<i32>(5, 3));
        if hash2(id + vec2<i32>(2, 9)) < 0.5 {
            let second = f.x > cut * PARCEL_M.x;
            f.x = select(f.x, f.x - cut * PARCEL_M.x, second);
            id = id * 2 + vec2<i32>(select(0, 1, second), 0);
        }
        let crop = hash2(id + vec2<i32>(9, 17));
        var field: vec3<f32>;
        if crop < 0.38 {
            field = vec3<f32>(0.065, 0.115, 0.035); // grass
        } else if crop < 0.58 {
            field = vec3<f32>(0.22, 0.19, 0.1); // ripe cereal
        } else if crop < 0.72 {
            field = vec3<f32>(0.12, 0.085, 0.055); // ploughed
        } else if crop < 0.85 {
            field = vec3<f32>(0.05, 0.09, 0.028); // maize, beet
        } else {
            field = vec3<f32>(0.11, 0.115, 0.05); // fallow, meadow
        }
        field *= 0.85 + 0.3 * noise2(p / 45.0, 3.3);
        let drill = 1.0 + 0.1 * sin(f.x * 2.5) * (1.0 - smoothstep(0.4, 1.2, fw));
        c = field * drill;
        // Hedgerows: on the parcel's left and bottom borders when that border's hash says so.
        let hedge_x = select(0.0, 1.0, hash2(id * 3 + vec2<i32>(1, 0)) < 0.45);
        let hedge_z = select(0.0, 1.0, hash2(id * 3 + vec2<i32>(0, 1)) < 0.4);
        var cover = 0.0;
        if hedge_x + hedge_z > 0.0 && min(f.x, f.y) < 8.0 + fw {
            let along = noise2(p / 7.0, 6.1); // gaps and crowns along it
            let hw = 2.0 + 2.5 * along;
            cover = max(
                hedge_x * line_cover(f.x, hw, fw, PARCEL_M.x),
                hedge_z * line_cover(f.y, hw, fw, PARCEL_M.y),
            );
        } else if hedge_x + hedge_z > 0.0 {
            // Far beyond the line itself only its mean share remains (`line_cover`).
            cover = max(hedge_x * 3.25 / PARCEL_M.x, hedge_z * 3.25 / PARCEL_M.y) * smoothstep(2.0, 20.0, fw);
        }
        if cover > 0.0 {
            canopy *= 0.75 + 0.5 * noise2(p / 9.0, 4.4);
        }
        c = mix(c, canopy, cover);
        rough = mix(rough, 1.0, cover);
    }
    if wood > 0.0 {
        canopy = vec3<f32>(0.03, 0.05, 0.02) * (0.75 + 0.5 * noise2(p / 9.0, 4.4));
    }
    c = mix(c, canopy, wood);
    s.base = vec4<f32>(mix(base, c, fade), s.base.a);
    s.roughness = mix(s.roughness, mix(rough, 1.0, wood), fade);
    return s;
}

// ---- infinite ground -------------------------------------------------------

// A backdrop, not geometry: a polar grid of rings around the camera's
// position (radius 0.5 m growing by a constant factor to ~40 km) at the
// ground height, drawn after the opaque scene at a depth just in front of
// the far plane, so it only fills pixels nothing else covered: it can never
// z-fight with or hide the user's own lawn, pond basin or basement. It is
// shaded like any surface (its material is made from `Ground`), so it gets
// sun, shadows of the near cascades, sky light, lamps and the haze.
const GROUND_RINGS: u32 = 80u;
const GROUND_SEGMENTS: u32 = 192u;
const GROUND_GROWTH: f32 = 1.15359; // 0.5 m * 1.15359^79 = 40 km
const GROUND_FACE: u32 = 0xfffffffeu;
// Behind the sea's far rings (sea.wgsl SEA_DEPTH, 0.999999): at the same depth the two fought
// near the horizon, rows of ground showing through the far sea as pale flecks.
const GROUND_DEPTH: f32 = 0.9999995;
const EYE_SLOPE_NEAR: f32 = 1.7 / 40.0; // where the horizon fade begins

// Whether the ground's cell (`ring`, `seg`) may lie within reach of the sky volume's lookup
// (`volume_lifted`: the point lifted up to one and a half cells along its normal, inside the
// volume's box): its box (corners, plus the arc's bulge) against the volume's in x and z, two
// cells wider. False without a volume.
fn ground_cell_near_volume(ring: u32, seg: u32) -> bool {
    if g.sky_vol_size.w < 0.5 {
        return false;
    }
    let r0 = select(0.0, 0.5 * pow(GROUND_GROWTH, f32(ring) - 1.0), ring > 0u);
    let r1 = 0.5 * pow(GROUND_GROWTH, f32(ring));
    let step = 2.0 * PI / f32(GROUND_SEGMENTS);
    let d0 = vec2<f32>(cos(f32(seg) * step), sin(f32(seg) * step));
    let d1 = vec2<f32>(cos(f32(seg + 1u) * step), sin(f32(seg + 1u) * step));
    let lo = g.camera.xz + min(min(d0 * r0, d1 * r0), min(d0 * r1, d1 * r1));
    let hi = g.camera.xz + max(max(d0 * r0, d1 * r0), max(d0 * r1, d1 * r1));
    let cell = 1.0 / g.sky_vol.w;
    let margin = 2.0 * cell + r1 * (1.0 - cos(step * 0.5)) + 0.01;
    let vlo = g.sky_vol.xz - margin;
    let vhi = g.sky_vol.xz + (1.0 / g.sky_vol_size.xz - 1.0) * cell + margin;
    return all(lo <= vhi) && all(hi >= vlo);
}

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
    var out: Varying;
    if VOLUME_PART != 0u && ground_cell_near_volume(ring, seg) != (VOLUME_PART == 1u) {
        // Another pipeline draws this cell: a point, nothing.
        out.clip = vec4<f32>(0.0, 0.0, 0.5, 1.0);
        return out;
    }
    let xz = g.camera.xz + radius * vec2<f32>(cos(angle), sin(angle));
    let kind = surround_kind(xz);
    let h = surround_height(xz, kind);
    let world = vec3<f32>(xz.x, ground_height_at(xz) + h, xz.y);
    // The relief's normal from its slope over about a vertex spacing (forward differences).
    let e = max(1.0, 0.12 * radius);
    let sx = surround_height(xz + vec2<f32>(e, 0.0), kind) - h;
    let sz = surround_height(xz + vec2<f32>(0.0, e), kind) - h;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.clip.z = out.clip.w * GROUND_DEPTH;
    out.world = world;
    out.normal = normalize(vec3<f32>(-sx, e, -sz));
    out.face = GROUND_FACE;
    out.uv = world.xz;
    return out;
}

// Close to the horizon the ground fades into the colour the sky has just
// above it (by day, at dusk and at night alike), so there is never a line
// between a dark field and the sky, haze or no haze. The fade follows the
// angle below the horizon, not the distance: from eye height it spans what
// 40 m to 2 km used to, from an overview 100-250 m up the ground stays
// ground for kilometres. A plan or elevation view has no horizon: the ground
// stays as it is.
//
// With haze the ground needs no fade: it ends where the Earth's curvature hides it, at the
// geometric horizon sqrt(2 R h) for the eye h metres over it (4.6 km at eye level, 57 km from
// 260 m up), and the haze over that finite distance leaves the far fields a shade darker than
// the sky over them: a real horizon line. Ground the flat plane puts beyond the horizon is
// shaded as at the horizon.
const EARTH_RADIUS_M: f32 = 6.36e6;

@fragment
fn fs_ground(in_raw: Varying) -> @location(0) vec4<f32> {
    var in = in_raw;
    // The horizon colour in this pixel's own view direction (from the
    // pixel, not the interpolated position: see below).
    let ndc = vec2<f32>(in.clip.x * g.viewport.z * 2.0 - 1.0, 1.0 - in.clip.y * g.viewport.w * 2.0);
    let p = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    let ray = p.xyz / p.w - g.camera.xyz;
    let hazed = veiled() && !is_ortho();
    if hazed {
        // The eye's horizon plus a hill's own: its top shows over the curve from further.
        let base = g.options.w;
        let horizon_m = sqrt(2.0 * EARTH_RADIUS_M * max(g.camera.y - base, 1.0))
            + sqrt(2.0 * EARTH_RADIUS_M * max(in.world.y - base, 0.0));
        let d = length(in.world - g.camera.xyz);
        if !(d < horizon_m) || ray.y >= 0.0 {
            in.world = g.camera.xyz + normalize(ray) * horizon_m;
        }
    }
    let c = shade(in, true, false, true).color;
    if is_ortho() {
        return vec4<f32>(c, 1.0);
    }
    if hazed {
        // As below: no ceiling of land over the eye.
        if ray.y > 0.0 && in_raw.world.y > g.camera.y {
            discard;
        }
        return vec4<f32>(c, 1.0);
    }
    let to = in.world - g.camera.xyz;
    let dist = length(to);
    // Land of a coast higher than the eye (the camera low on the beach, the terrain holding it
    // up) would show its underside overhead as a ceiling: open sky there instead.
    // shortcut: far land above eye height is lost with it (a band well under a pixel tall from
    // a few hundred metres); upgrade by drawing the ground only beyond the terrain.
    if ray.y > 0.0 && in.world.y > g.camera.y {
        discard;
    }
    let flat = vec3<f32>(ray.x, 0.0, ray.z);
    let horizon = fog_inscatter(flat / max(length(flat), 1e-6), 1.0);
    // Over the slope below the horizon that eye height (1.7 m) sees from
    // 40 m to 2 km: a few degrees, wide enough not to read as a line.
    let slope = max(-ray.y, 0.0) / max(length(flat), 1e-6);
    let fade = smoothstep(0.0, 1.0, log2(EYE_SLOPE_NEAR / max(slope, 1e-7)) / log2(2000.0 / 40.0));
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
    return reflected(in, front, false);
}

// The endless ground in the reflection.
@fragment
fn fs_reflect_ground(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return reflected(in, front, true);
}

fn reflected(in: Varying, front: bool, ground: bool) -> vec4<f32> {
    let shaded = shade(in, front, false, ground);
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
    if !veiled() {
        return c;
    }
    let to = -to_eye(world);
    let t = length(to);
    let dir = to / max(t, 1e-6);
    let v = veil(dir, t);
    return vec4<f32>(c.rgb * v.w + v.rgb * c.a, c.a);
}

// Thin architectural glass, drawn in two passes (lib.rs): first the
// transmitted light, multiplying what is behind per channel (blend Zero,
// Src): 50 % at normal incidence for the two faces' absorption and an
// interior that reads as an interior in daylight, times a neutral grey
// absorption raised to the path length through the pane (Beer-Lambert:
// 1 / cos theta, so edges and grazing views turn darker); then
// the reflected light added on top (blend One, One): two interfaces of a
// thin slab (reflectance R' = 2R / (1 + R), from the geometric series of
// inter-reflections) with a floor of 10 % so a window shows a little sky
// even head-on, reflecting the environment cube, the sun and the lamps.
// base_color.rgb tints the pane's own scattered light, base_color.a is its
// opacity (frosted / opaque panes). No refraction offset.
// A room seen through or mirrored in a window: dark walls and furniture.
const ROOM_ALBEDO: f32 = 0.05;

struct GlassTerms {
    reflected: vec3<f32>,
    transmitted: vec3<f32>,
}

fn glass_terms(in: Varying) -> GlassTerms {
    let curvature = length(fwidth(normalize(in.normal)));
    let v = normalize(to_eye(in.world));
    var n = normalize(in.normal);
    if dot(n, v) < 0.0 {
        n = -n;
    }
    // Float glass is never quite flat: a slow tilt of a degree or so with the position, so
    // each pane mirrors the sky and garden a little differently, without seams within one.
    let w = in.world;
    let wob = vec3<f32>(
        sin(w.x * 1.7 + w.z * 0.9 + 1.3 * sin(w.y * 2.1)),
        sin(w.y * 1.1 + w.x * 0.6),
        cos(w.z * 1.9 - w.x * 0.7 + w.y * 1.3),
    ) * 0.018;
    n = normalize(n + wob - n * dot(n, wob));
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
        // The sun's mirror image in a pane: spread over the disc's half degree and the glass's
        // waviness, and held below a few times a white wall's brightness, so a window that
        // catches the sun glints instead of burning a white hole into the picture.
        let a_sun = max(roughness, 0.12) * max(roughness, 0.12);
        sun = min(
            specular(n, v, l, nol, nov, vec3<f32>(0.04), a_sun) * nol * 2.0,
            vec3<f32>(0.35),
        ) * g.sun_light.rgb * shadow;
    }
    let lamps = local_lights(in.world, n, v, nov, vec3<f32>(0.0), vec3<f32>(0.04), a, false) * 2.0;
    let opacity = clamp(material.base_color.a, 0.0, 1.0);
    let lit = material.base_color.rgb * (sh_irradiance(n) / PI + g.sun_light.rgb * nol / PI);
    // What the pane mirrors: the sky where the sky volume says the reflected ray gets out, else
    // the room on this side of the pane, lit by what comes in and by its lamps (seen as a pale
    // wall at the pane). Without a volume, the camera in a room (`grade.z`) means the room.
    let r = reflect(-v, n);
    let vol = volume_at(in.world, n);
    var seen_r = sky_seen(vol.sky, r);
    if g.sky_vol_size.w < 0.5 && g.grade.z > 0.5 {
        seen_r = 0.0;
    }
    let wall = local_lights(in.world, n, v, nov, vec3<f32>(ROOM_REFLECTANCE), vec3<f32>(0.0), 1.0, false);
    let room = ROOM_ALBEDO * sh_irradiance(n) / PI * select(1.0, 0.0, g.sky_vol_size.w > 0.5)
        + room_seen(vol.sky, room_bounce(vol.room), r) + wall;
    // Screen-space reflections only in flat panes: a curved one (a lamp's globe) would sparkle.
    let ssr = screen_reflection(in.world, r) * (1.0 - smoothstep(0.004, 0.02, curvature));
    let mirrored = mix(environment(r, roughness) * seen_r + room * (1.0 - seen_r), ssr.rgb, ssr.a);
    var out: GlassTerms;
    out.reflected = fr * mirrored + sun + lamps + (1.0 - fr) * opacity * lit;
    // Clear float glass: about 0.9 at normal incidence through both faces, a slight grey
    // absorption that deepens with the path (1 / cos theta); a colour only from base_color.
    let tint = pow(vec3<f32>(0.93), vec3<f32>(1.0 / max(nov, 0.15)));
    out.transmitted = tint * ((1.0 - fr) * (1.0 - opacity));
    return out;
}

// Screen-space reflection (McGuire and Mara, "Efficient GPU Screen-Space Ray Tracing", JCGT
// 3(4), 2014): the reflected ray from `world` along `r`, clipped before the near plane, is
// projected and walked across the screen `g.ssr.y` pixels a step (at most `g.ssr.z` steps), its
// depth interpolated perspective-correctly (1/w), until it passes behind the depth pre-pass by
// less than a thickness; the colour there is the opaque scene's before the glass and mirrors
// (`previous_hdr`, lib.rs `Pass::Mirrors`). Returns that colour and a confidence fading out towards the screen's edges and
// the ray's reach; 0 where the ray leaves the screen or meets nothing (the sky): the caller's
// estimate stands there.
fn screen_reflection(world: vec3<f32>, r: vec3<f32>) -> vec4<f32> {
    if g.ssr.x < 0.5 {
        return vec4<f32>(0.0);
    }
    let size = vec2<f32>(textureDimensions(linear_depth));
    let v0 = (g.view * vec4<f32>(world, 1.0)).xyz;
    let vr = (g.view * vec4<f32>(r, 0.0)).xyz;
    var reach = SSR_REACH_M;
    if vr.z > 1e-4 {
        reach = min(reach, (-0.1 - v0.z) / vr.z);
    }
    if reach <= 0.0 {
        return vec4<f32>(0.0);
    }
    let v1 = v0 + vr * reach;
    let h0 = g.proj * vec4<f32>(v0, 1.0);
    let h1 = g.proj * vec4<f32>(v1, 1.0);
    let k0 = 1.0 / h0.w;
    let k1 = 1.0 / h1.w;
    let p0 = (h0.xy * k0 * vec2<f32>(0.5, -0.5) + 0.5) * size;
    let p1 = (h1.xy * k1 * vec2<f32>(0.5, -0.5) + 0.5) * size;
    let steps = min(max(abs(p1.x - p0.x), abs(p1.y - p0.y)) / g.ssr.y, g.ssr.z);
    if steps < 1.0 {
        return vec4<f32>(0.0);
    }
    // A pixel's own offset along the first step hides the steps' bands (interleaved gradient
    // noise: Jimenez, "Next Generation Post Processing in Call of Duty: Advanced Warfare",
    // SIGGRAPH 2014).
    let jitter = fract(52.982918 * fract(dot(floor(p0), vec2<f32>(0.06711056, 0.00583715))));
    var before = 0.0;
    var hit = -1.0;
    for (var i = 0.0; i < steps; i += 1.0) {
        let t = (i + jitter) / steps;
        let p = mix(p0, p1, t);
        if any(p < vec2<f32>(0.0)) || any(p >= size) {
            break;
        }
        let depth = -(v0.z * k0 + (v1.z * k1 - v0.z * k0) * t) / mix(k0, k1, t);
        let scene = textureLoad(linear_depth, vec2<i32>(p), 0).r;
        if depth > scene + 0.01 {
            if depth < scene + g.ssr.w + 0.02 * scene {
                hit = t;
            }
            break;
        }
        before = t;
    }
    if hit < 0.0 {
        return vec4<f32>(0.0);
    }
    // Four halvings between the last step in front and the first behind.
    var lo = before;
    for (var i = 0; i < 4; i++) {
        let t = 0.5 * (lo + hit);
        let depth = -(v0.z * k0 + (v1.z * k1 - v0.z * k0) * t) / mix(k0, k1, t);
        if depth > textureLoad(linear_depth, vec2<i32>(mix(p0, p1, t)), 0).r {
            hit = t;
        } else {
            lo = t;
        }
    }
    let p = mix(p0, p1, hit);
    let edge = min(min(p.x, size.x - p.x), min(p.y, size.y - p.y)) / (0.08 * size.y);
    let fade = clamp(edge, 0.0, 1.0) * (1.0 - smoothstep(0.7, 1.0, hit));
    let colour = textureLoad(previous_hdr, vec2<i32>(p), 0).rgb;
    return vec4<f32>(colour, fade);
}

// How far a screen-space reflection ray goes (m).
const SSR_REACH_M: f32 = 40.0;

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
        return vec4<f32>(shade(in, front, false, false).color * 0.35, 0.0);
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
    // Rain's rings spread over the pond.
    if g.wet.y > 0.0 {
        let rings = rain_ripple_slope(p, g.wet.y, 1.0 - smoothstep(8.0, 40.0, dist));
        n = normalize(n + vec3<f32>(-rings.x, 0.0, -rings.y));
    }
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
    var color = (f * reflected + glint + (1.0 - f) * (1.0 - transmit) * deep) * shore;
    var alpha = (f + (1.0 - f) * (1.0 - transmit)) * shore;
    // Deep in a snowy winter the pond freezes: dull grey-blue ice, snow drifted over it in places.
    let ice = smoothstep(0.45, 0.75, g.snow.x);
    if ice > 0.0 {
        let up = vec3<f32>(0.0, 1.0, 0.0);
        let nz = value_noise(in.world * 0.9) * 0.6 + value_noise(in.world * 4.1) * 0.4;
        let drift = smoothstep(0.62, 0.85, nz + (g.snow.x - 0.7) * 0.6) * snow_open(in.world);
        // Dark, slightly blue ice that still mirrors the bank, with snow blown over it in places.
        let albedo = mix(vec3<f32>(0.08, 0.11, 0.14), SNOW_ALBEDO, drift);
        let lit = albedo * (sh_irradiance(up) + g.sun_light.rgb * max(l.y, 0.0) * sun_on) / PI
            + reflected * (0.05 + 0.7 * f) * (1.0 - drift);
        color = mix(color, lit, ice);
        alpha = mix(alpha, 1.0, ice);
    }
    return fog_premultiplied(vec4<f32>(color, alpha), in.world);
}

// Premultiplied alpha out (blend: One, OneMinusSrcAlpha).
@fragment
fn fs_transparent(in: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Branches on uniforms only, so shade() may take derivatives.
    let kind = material.alpha.z;
    if g.time.w > 0.5 {
        // Clay: water as a pale matte surface (glass has its own passes).
        return vec4<f32>(shade(in, front, false, false).color, 1.0);
    }
    if kind > 1.5 {
        return water(in);
    }
    let shaded = shade(in, front, false, false);
    let alpha = clamp(shaded.alpha, 0.0, 1.0);
    return vec4<f32>(shaded.color * alpha, alpha);
}
