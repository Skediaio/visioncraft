// The sea level (sea.rs): a camera-centred polar grid at the water height out
// to ~40 km, drawn in the forward pass after the opaque scene, the endless
// ground and the sky, so the terrain above the water hides it and the bed
// below shows through it.
//
// Waves: a sum of Gerstner (trochoidal) waves, Tessendorf, "Simulating Ocean
// Water" (SIGGRAPH course notes, 2001), section 3.2, in the form of Finch,
// "Effective Water Simulation from Physical Models", GPU Gems ch. 1 (2004),
// eqs. 9 and 12, with deep-water dispersion omega^2 = g k. sea.rs draws the
// amplitudes from a Pierson-Moskowitz spectrum. The long waves displace the
// grid where its cells resolve them; every wave tilts the per-pixel normal.
// A wave shorter than a few pixel footprints is dropped from the normal and
// its slope variance moved into the roughness of the sun's and sky's
// reflection, so the far sea turns from glitter into a broad sun path instead
// of aliasing (Bruneton, Neyret & Holzschuch, "Real-time Realistic Ocean
// Lighting using Seamless Transitions from Geometry to BRDF", Computer
// Graphics Forum 29(2), 2010). Crest foam where the surface's Jacobian
// folds towards zero (Tessendorf 2001, section 4.5), shore foam over shallow
// beds, depth absorption as the pond's (scene.wgsl `water`).

struct SeaWave {
    a: vec4<f32>, // xy unit direction (world x, z), z wavenumber k (rad/m), w amplitude (m)
    b: vec4<f32>, // x angular frequency (rad/s), y phase (rad), z Gerstner steepness Q, w 0
}

struct SeaParams {
    surface: vec4<f32>, // x level (m), y wave count, z foam 0..1, w turbidity 0..1
    color: vec4<f32>,   // rgb deep colour (linear), w wet band height above the level (m)
    flow: vec4<f32>,    // xy surface flow (m/s), z freezes (0/1), w outline points
    extra: vec4<f32>,   // x endless ground above the water (0/1), y ripple strength, z significant wave height (m), w on (0/1)
    waves: array<SeaWave, 16>,
    outline: array<vec4<f32>, 32>, // two [x, z] points per entry
}
@group(1) @binding(30) var<uniform> sea: SeaParams;

const SEA_RINGS: u32 = 96u;
const SEA_SEGMENTS: u32 = 128u;
const SEA_GROWTH: f32 = 1.1361; // 0.25 m * 1.1361^94 = 40 km
const SEA_DEPTH: f32 = 0.999999;
const GRAVITY: f32 = 9.81;

fn sea_outline_point(i: u32) -> vec2<f32> {
    let e = sea.outline[i / 2u];
    return select(e.xy, e.zw, (i & 1u) == 1u);
}

// Even-odd rule over the outline (no outline: everywhere).
fn sea_covers(p: vec2<f32>) -> bool {
    let n = u32(sea.flow.w);
    if n < 3u {
        return true;
    }
    var inside = false;
    var j = n - 1u;
    for (var i = 0u; i < n; i++) {
        let a = sea_outline_point(i);
        let b = sea_outline_point(j);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    return inside;
}

// The wave's phase at the undisplaced point `p`; a river carries the pattern along.
fn sea_phase(w: SeaWave, p: vec2<f32>, t: f32) -> f32 {
    let q = p - sea.flow.xy * t;
    return w.a.z * dot(w.a.xy, q) - w.b.x * t + w.b.y;
}

@vertex
fn vs_sea(@builtin(vertex_index) vi: u32) -> Varying {
    let tri = vi / 3u;
    let quad = tri / 2u;
    let ring = quad / SEA_SEGMENTS;
    let seg = quad % SEA_SEGMENTS;
    var dr = array<u32, 6>(0u, 1u, 1u, 0u, 1u, 0u);
    var ds = array<u32, 6>(0u, 0u, 1u, 0u, 1u, 1u);
    let k = vi % 3u + 3u * (tri & 1u);
    let r = ring + dr[k];
    let radius = select(0.0, 0.25 * pow(SEA_GROWTH, f32(r) - 1.0), r > 0u);
    let angle = f32(seg + ds[k]) * 2.0 * PI / f32(SEA_SEGMENTS);
    // Centred on the camera, snapped to half metres so the near cells do not slide.
    let centre = floor(g.camera.xz * 2.0) * 0.5;
    let p = centre + radius * vec2<f32>(cos(angle), sin(angle));
    // Only waves several cells long move the grid; the rest is left to the normals.
    let cell = max(radius * (SEA_GROWTH - 1.0), 0.05);
    var offset = vec3<f32>(0.0);
    let t = g.time.x;
    let count = u32(sea.surface.y);
    for (var i = 0u; i < count; i++) {
        let w = sea.waves[i];
        let wavelength = 2.0 * PI / w.a.z;
        let keep = smoothstep(4.0, 8.0, wavelength / cell);
        let ph = sea_phase(w, p, t);
        let c = cos(ph);
        offset += keep * vec3<f32>(w.b.z * w.a.w * w.a.x * c, w.a.w * sin(ph), w.b.z * w.a.w * w.a.y * c);
    }
    let world = vec3<f32>(p.x + offset.x, sea.surface.x + offset.y, p.y + offset.z);
    var out: Varying;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.clip.z = min(out.clip.z, out.clip.w * SEA_DEPTH);
    out.world = world;
    out.normal = vec3<f32>(0.0, 1.0, 0.0);
    out.face = 0xfffffffdu;
    out.uv = p;
    return out;
}

struct SeaSurface {
    n: vec3<f32>,
    // Slope variance of the waves too small to resolve here.
    variance: f32,
    // Jacobian determinant of the horizontal displacement (1 flat, towards 0 at folding crests).
    jacobian: f32,
}

// Normal, unresolved slope variance and Jacobian at undisplaced `p`, for a
// pixel footprint of `footprint` metres.
fn sea_surface(p: vec2<f32>, footprint: f32) -> SeaSurface {
    let t = g.time.x;
    var nx = 0.0;
    var nz = 0.0;
    var ny = 1.0;
    var variance = 0.0;
    var jxx = 1.0;
    var jzz = 1.0;
    var jxz = 0.0;
    let count = u32(sea.surface.y);
    for (var i = 0u; i < count; i++) {
        let w = sea.waves[i];
        let wavelength = 2.0 * PI / w.a.z;
        // Resolved while it spans more than about four footprints.
        let keep = smoothstep(2.0, 6.0, wavelength / max(footprint, 1e-4));
        let wa = w.a.z * w.a.w;
        variance += (1.0 - keep * keep) * wa * wa * 0.5;
        let ph = sea_phase(w, p, t);
        let s = sin(ph) * keep;
        let c = cos(ph) * keep;
        nx -= w.a.x * wa * c;
        nz -= w.a.y * wa * c;
        ny -= w.b.z * wa * s;
        jxx -= w.b.z * wa * w.a.x * w.a.x * s;
        jzz -= w.b.z * wa * w.a.y * w.a.y * s;
        jxz -= w.b.z * wa * w.a.x * w.a.y * s;
    }
    // Wind ripples finer than the smallest wave: the pond's noise, fading below a footprint.
    let wind = vec2<f32>(g.wind.x, g.wind.y);
    let ripple = sea.extra.y * (1.0 - smoothstep(0.15, 0.6, footprint));
    let q = p - sea.flow.xy * t;
    let slope = (wave_slope(q * 2.1 + wind * t * 0.4) * 0.06 + wave_slope(q * 6.3 - vec2<f32>(0.23, -0.19) * t) * 0.03) * ripple;
    variance += sea.extra.y * 0.002 * (1.0 - ripple / max(sea.extra.y, 1e-4));
    var out: SeaSurface;
    out.n = normalize(vec3<f32>(nx - slope.x, max(ny, 0.2), nz - slope.y));
    out.variance = variance;
    out.jacobian = jxx * jzz - jxz * jxz;
    return out;
}

@fragment
fn fs_sea(in: Varying) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let footprint = max(length(fwidth(in.uv)), 1e-3);
    let s = sea_surface(in.uv, footprint);
    if !sea_covers(in.uv) {
        discard;
    }
    let pixel = vec2<i32>(in.clip.xy);
    let below = textureLoad(linear_depth, pixel, 0).r;
    // The endless ground above the water hides it wherever no terrain lies under it.
    if sea.extra.x > 0.5 && below > SKY_DEPTH * 0.5 {
        discard;
    }
    let up = vec3<f32>(0.0, 1.0, 0.0);
    if g.time.w > 0.5 {
        // Clay: a pale, matte surface.
        let lit = vec3<f32>(0.6) * (sh_irradiance(up) + g.sun_light.rgb * max(g.sun_dir.y, 0.0)) / PI;
        return vec4<f32>(apply_fog(lit, in.world), 1.0);
    }
    let eye_vec = to_eye(in.world);
    let dist = length(eye_vec);
    let v = eye_vec / dist;
    let depth = -(g.view * vec4<f32>(in.world, 1.0)).z;
    var n = s.n;
    if dot(n, v) < 0.02 {
        // A back-facing slope seen at a grazing angle: bend it just towards the eye.
        n = normalize(n + v * (0.02 - dot(n, v)));
    }
    let nov = max(dot(n, v), 1e-4);
    // Roughness: glassy water plus the unresolved waves (GGX alpha^2 ~ 2 sigma^2).
    let alpha = clamp(sqrt(0.0004 + 2.0 * s.variance), 0.02, 0.6);
    let rough = sqrt(alpha);
    let f = f_schlick(nov, vec3<f32>(0.02)).r;
    let r = reflect(-v, n);
    var sky = environment(vec3<f32>(r.x, max(r.y, 0.0), r.z), rough);
    var reflected = sky;
    if g.reflection.x > 0.5 && abs(sea.surface.x - g.reflection.y) < 0.05 {
        // The planar reflection at the mean level, shifted by the slope (less with distance);
        // where the surface is rough it blurs into the sky's prefiltered reflection.
        let tilt = (g.view * vec4<f32>(n.x, 0.0, n.z, 0.0)).xy;
        let size = vec2<f32>(textureDimensions(planar_reflection));
        let uv = in.clip.xy * g.viewport.zw + tilt * vec2<f32>(1.0, -1.0) * 1.2 / max(depth, 1.0);
        let planar = textureSampleLevel(planar_reflection, env_sampler, clamp(uv, 0.5 / size, 1.0 - 0.5 / size), 0.0).rgb;
        reflected = mix(planar, sky, smoothstep(0.15, 0.45, rough));
    }
    // Water depth under this point, straight down (perspective: along the ray to the bed).
    let path = max(below - depth, 0.0) * dist / max(depth, 1e-3);
    let bed = in.world - v * path;
    let water_depth = select(1e4, max(sea.surface.x - bed.y, 0.0), below < SKY_DEPTH * 0.5);
    let l = g.sun_dir.xyz;
    let nol = max(dot(n, l), 0.0);
    var sun_on = 0.0;
    var glint = local_lights(in.world, n, v, nov, vec3<f32>(0.0), vec3<f32>(0.02), alpha, false);
    if l.y > 0.0 {
        sun_on = sun_shadow(in.world, up, max(l.y, 0.05), depth);
        glint += specular(n, v, l, max(nol, 1e-4), nov, vec3<f32>(0.02), alpha) * nol * g.sun_light.rgb * sun_on;
    }
    let transmit = water_transmittance(path, sea.surface.w);
    // Light scattered back out of the water body: the deep colour lit by the sky and the sun,
    // a little brighter where a crest faces the sun (thinner water, Fresnel aside).
    let crest = 1.0 + 0.6 * max(dot(n.xz, l.xz), 0.0) * smoothstep(0.0, 0.3, sea.extra.z);
    let deep = sea.color.rgb * crest * (sh_irradiance(up) + g.sun_light.rgb * max(l.y, 0.0) * sun_on) / PI;
    var color = f * reflected + glint + (1.0 - f) * (1.0 - transmit) * deep;
    var alpha_out = f + (1.0 - f) * (1.0 - transmit);
    // Foam: crests that fold (more and lower thresholds at higher sea states), and a band over
    // shallow beds that the swell runs up and back, broken up by noise.
    let t = g.time.x;
    let noise = value_noise(vec3<f32>(in.uv.x * 0.9, t * 0.15, in.uv.y * 0.9)) * 0.6
        + value_noise(vec3<f32>(in.uv.x * 3.7, t * 0.4, in.uv.y * 3.7)) * 0.4;
    let state = smoothstep(0.3, 2.5, sea.extra.z);
    let crest_foam = smoothstep(0.55, 0.15, s.jacobian) * state * smoothstep(0.35, 0.7, noise + 0.15 * state);
    let run = 0.25 + 0.5 * sea.extra.z;
    let surge = 0.5 + 0.5 * sin(t * 0.9 + dot(in.uv, vec2<f32>(0.21, 0.17)));
    let shore_foam = (1.0 - smoothstep(0.0, run * (0.5 + 0.5 * surge), water_depth))
        * smoothstep(0.3, 0.75, noise + 0.25) * smoothstep(0.0, 0.03, water_depth);
    let foam = clamp((crest_foam + shore_foam) * sea.surface.z * 2.0, 0.0, 1.0);
    if foam > 0.0 {
        let white = vec3<f32>(0.8) * (sh_irradiance(n) + g.sun_light.rgb * nol * sun_on) / PI;
        color = mix(color, white, foam);
        alpha_out = mix(alpha_out, 1.0, foam);
    }
    // Where the water thins to nothing at the waterline, fade it out.
    let edge = smoothstep(0.0, 0.05, path);
    color *= edge;
    alpha_out *= edge;
    // Still lake water freezes in a snowy winter; the sea and rivers do not.
    let ice = smoothstep(0.45, 0.75, g.snow.x) * sea.flow.z;
    if ice > 0.0 {
        let nz = value_noise(in.world * 0.9) * 0.6 + value_noise(in.world * 4.1) * 0.4;
        let drift = smoothstep(0.62, 0.85, nz + (g.snow.x - 0.7) * 0.6) * snow_open(in.world);
        let fi = f_schlick(max(v.y, 1e-4), vec3<f32>(0.02)).r;
        let albedo = mix(vec3<f32>(0.08, 0.11, 0.14), SNOW_ALBEDO, drift);
        let lit = albedo * (sh_irradiance(up) + g.sun_light.rgb * max(l.y, 0.0) * sun_on) / PI
            + reflected * (0.05 + 0.7 * fi) * (1.0 - drift);
        color = mix(color, lit, ice);
        alpha_out = mix(alpha_out, 1.0, ice);
    }
    return fog_premultiplied(vec4<f32>(color, alpha_out), in.world);
}

// Wet ground just above the waterline: a full-screen pass multiplying the
// opaque scene (and the bed below, which the water then covers) darker within
// the swash band, as wet sand and stone absorb more (a water film lowers the
// albedo: Lekner & Dorf, "Why some things are darker when wet", Applied
// Optics 27(7), 1988).
@vertex
fn vs_sea_wet(@builtin(vertex_index) vi: u32) -> Varying {
    var out: Varying;
    out.clip = fullscreen(vi);
    out.clip.z = 0.0;
    return out;
}

@fragment
fn fs_sea_wet(in: Varying) -> @location(0) vec4<f32> {
    let below = textureLoad(linear_depth, vec2<i32>(in.clip.xy), 0).r;
    if below > SKY_DEPTH * 0.5 || is_ortho() || g.time.w > 0.5 {
        return vec4<f32>(1.0);
    }
    let ndc = vec2<f32>(in.clip.x * g.viewport.z * 2.0 - 1.0, 1.0 - in.clip.y * g.viewport.w * 2.0);
    let h = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    let ray = normalize(h.xyz / h.w - g.camera.xyz);
    let forward = -normalize(vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]));
    let world = g.camera.xyz + ray * (below / max(dot(ray, forward), 1e-3));
    let top = sea.surface.x + sea.color.w;
    let wet = (1.0 - smoothstep(top - 0.5 * sea.color.w, top, world.y))
        * (1.0 - smoothstep(300.0, 500.0, below)) * (1.0 - g.snow.x);
    if wet <= 0.0 || !sea_covers(world.xz) {
        return vec4<f32>(1.0);
    }
    return vec4<f32>(vec3<f32>(mix(1.0, 0.6, wet)), 1.0);
}
