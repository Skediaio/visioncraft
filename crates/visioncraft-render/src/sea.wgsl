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
    surf: vec4<f32>,    // x peak angular frequency (rad/s), y nearshore bed slope, zw unit direction the swell travels (world x, z)
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

fn coast_point(i: u32) -> vec2<f32> {
    let e = g.coast[i / 2u];
    return select(e.xy, e.zw, (i & 1u) == 1u);
}

// Signed distance (m) from `p` to the coastline in the globals, positive on the sea side (its
// right), the first and last stretches running on without end; as the core's `coast_distance`.
fn coast_distance(p: vec2<f32>) -> f32 {
    let n = u32(g.coast_info.x);
    var best = 1e30;
    var side = 1.0;
    for (var i = 0u; i + 1u < n; i++) {
        let a = coast_point(i);
        let d = coast_point(i + 1u) - a;
        let q = p - a;
        let lo = select(0.0, -1e30, i == 0u);
        let hi = select(1.0, 1e30, i + 2u == n);
        let t = clamp(dot(q, d) / max(dot(d, d), 1e-9), lo, hi);
        let dist = length(q - t * d);
        if dist < best {
            best = dist;
            side = select(-1.0, 1.0, d.x * q.y - d.y * q.x >= 0.0);
        }
    }
    return best * side;
}

// The endless ground's height at `p`: its own on the land side of a coast, under the water on
// the sea side.
fn ground_height_at(p: vec2<f32>) -> f32 {
    if g.coast_info.x > 1.5 && g.options.w > g.coast_info.y && coast_distance(p) > 0.0 {
        return g.coast_info.z;
    }
    return g.options.w;
}

// The wave's phase at the undisplaced point `p`; a river carries the pattern along.
fn sea_phase(w: SeaWave, p: vec2<f32>, t: f32) -> f32 {
    let q = p - sea.flow.xy * t;
    return w.a.z * dot(w.a.xy, q) - w.b.x * t + w.b.y;
}

// The top of whatever lies under (x, z): the sea bed, the beach, a plant on it. From the cover
// map (lib.rs draws it while there is a sea); -1e9 outside it.
fn sea_bed(p: vec2<f32>) -> f32 {
    return snow_top(vec3<f32>(p.x, 0.0, p.y));
}

// Distance along the shore (m), across the swell's direction.
fn surf_alongshore(p: vec2<f32>) -> f32 {
    return dot(p, vec2<f32>(-sea.surf.w, sea.surf.z));
}

// Shift of the crest lines along the shore (rad), so they are not ruler-straight: bends of
// about 70 m and 17 m, and a small wobble of 6 m.
fn surf_along(p: vec2<f32>) -> f32 {
    let a = surf_alongshore(p);
    return value_noise(vec3<f32>(a * 0.015, 0.0, 0.37)) * 7.0 + value_noise(vec3<f32>(a * 0.06, 1.3, 0.0)) * 2.0
        + value_noise(vec3<f32>(a * 0.17, 2.9, 0.5)) * 0.6;
}

// The local height of a crest relative to the swell's: the wave groups (every seventh wave or
// so is the highest; the envelope travels with the crests in shallow water, where the group
// and phase speeds meet) with their envelope drifting along the shore, and each crest higher
// and lower along its length, so one wave breaks here and still rolls in there.
fn surf_group(p: vec2<f32>, theta: f32) -> f32 {
    let a = surf_alongshore(p);
    let group = 0.65 + 0.35 * sin(theta / 7.0 + 1.3 + a * 0.012);
    let crest = floor(theta / (2.0 * PI));
    return group * (0.75 + 0.5 * value_noise(vec3<f32>(a * 0.035, crest * 0.37, 2.7)));
}

// Vertical run-up of the swash above the mean level (m): Stockdon et al., "Empirical
// parameterization of setup, swash, and runup", Coastal Engineering 53 (2006), eq. 18 for
// dissipative beaches, R2 = 0.043 sqrt(H0 L0).
fn surf_runup() -> f32 {
    let omega = max(sea.surf.x, 0.05);
    let l0 = 2.0 * PI * GRAVITY / (omega * omega);
    return 0.043 * sqrt(max(sea.extra.z, 0.0) * l0) + 0.02;
}

// Surf over a shallow bed, from linear wave theory in its shallow-water limit (Dean &
// Dalrymple, "Water Wave Mechanics for Engineers and Scientists", 1991, chs. 3-4): the swell's
// celerity falls to sqrt(g d), so its wavelength shrinks with the depth d; conservation of the
// energy flux raises its height by the shoaling coefficient K_s = sqrt(c_g0 / c_g), with
// c_g0 = g / (2 omega) and c_g = sqrt(g d) (Green's law, H ~ d^-1/4); it breaks once the height
// passes 0.78 of the depth (McCowan 1894, as in the USACE Coastal Engineering Manual
// EM 1110-2-1100, part II-4) and runs on as a bore that keeps that ratio. The phase is the
// integral of the local wavenumber towards the shore over a bed of slope m:
// theta = omega t + 2 omega sqrt(d) / (m sqrt(g)), so crests bunch up as they slow.
// Shoaling waves are not sines: their crests grow peaked and high over long flat troughs
// (cnoidal waves; at breaking the crest stands about 0.7-0.8 H above the mean level, CEM II-1),
// here a profile cos^2n(pi s) with n rising from 2 to 7 towards breaking.
struct Surf {
    slope: vec2<f32>, // d eta / d(x, z) of the surf
    eta: f32,         // its elevation over the mean level (m)
    foam: f32,
    weight: f32,      // 0 offshore, 1 in shallow water
    zone: f32,        // 0..1 the broken water (whitewater behind, stirred-up sand)
    k: f32,           // local wavenumber (rad/m)
}

fn sea_surf(p: vec2<f32>, d: f32) -> Surf {
    var out: Surf;
    let omega = max(sea.surf.x, 0.05);
    let h0 = sea.extra.z;
    let l0 = 2.0 * PI * GRAVITY / (omega * omega);
    // The shallow-water limit holds below about L0 / 20.
    out.weight = (1.0 - smoothstep(l0 / 20.0, l0 / 8.0, d)) * smoothstep(0.02, 0.6, h0);
    if out.weight <= 0.0 || d <= 0.0 {
        return out;
    }
    let dd = max(d, 0.02);
    let theta = omega * g.time.x + 2.0 * omega * sqrt(dd) / (sea.surf.y * sqrt(GRAVITY)) + surf_along(p);
    let ks = max(sqrt(GRAVITY / (2.0 * omega) / sqrt(GRAVITY * dd)), 1.0);
    let local = h0 * surf_group(p, theta);
    let ratio = local * ks / (0.78 * dd);
    let height = min(local * ks, 0.78 * dd);
    let breaking = smoothstep(0.85, 1.1, ratio);
    let s = fract(theta / (2.0 * PI));
    // Unbroken: cos^2n(pi s), peaking as it nears breaking; broken: a bore, steep onshore face
    // (s 0.9..1), long back.
    let n = mix(2.0, 7.0, smoothstep(0.35, 0.95, ratio));
    let c = max(0.5 + 0.5 * cos(2.0 * PI * s), 1e-6);
    let f_swell = pow(c, n) - inverseSqrt(PI * n); // minus its mean, about 1 / sqrt(pi n)
    let df_swell = -n * pow(c, n - 1.0) * PI * sin(2.0 * PI * s);
    let f_bore = select(s / 0.9, (1.0 - s) / 0.1, s > 0.9) - 0.5;
    let df_bore = select(1.0 / 0.9, -1.0 / 0.1, s > 0.9);
    let f = mix(f_swell, f_bore, breaking);
    let df = mix(df_swell, df_bore, breaking);
    out.eta = height * f * out.weight;
    // d eta / d theta, times d theta / d(onshore distance) = -k = -omega / sqrt(g d).
    out.k = omega / sqrt(GRAVITY * dd);
    out.slope = -(height * df / (2.0 * PI)) * out.k * sea.surf.zw * out.weight;
    // Whitewater on the face and a fading tail behind the bore, torn along the crest.
    let a = surf_alongshore(p);
    let torn = smoothstep(0.25, 0.6, value_noise(vec3<f32>(a * 0.12, floor(theta / (2.0 * PI)) * 0.53, 0.8)));
    let tail = max(exp(-8.0 * s), select(0.0, 0.9, s > 0.93));
    out.foam = breaking * out.weight * tail * mix(0.35, 1.0, torn);
    out.zone = breaking * out.weight;
    return out;
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
    // Over a known shallow bed the open sea's waves give way to the surf (which lifts its crests
    // as they shoal), and nothing rises through the bed.
    let bed = sea_bed(p);
    if bed > -1e8 {
        let d = sea.surface.x - bed;
        let surf = sea_surf(p, d);
        // Faded out towards the cover map's edge (as fs_sea's depth): no ring where it ends.
        let q = g.snow_cover * vec4<f32>(p.x, 0.0, p.y, 1.0);
        let w = 1.0 - smoothstep(0.6, 0.95, max(abs(q.x), abs(q.y)));
        offset *= mix(1.0, (1.0 - 0.85 * surf.weight) * smoothstep(0.0, 1.5, d), w);
        let keep = smoothstep(3.0, 6.0, 2.0 * PI / max(surf.k, 1e-3) / cell);
        offset.y += surf.eta * keep * w;
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
    // Rain's rings, where a pixel is small enough to show them.
    var rings = vec2<f32>(0.0);
    if g.wet.y > 0.0 {
        rings = rain_ripple_slope(p, g.wet.y, 1.0 - smoothstep(0.04, 0.25, footprint));
    }
    var out: SeaSurface;
    out.n = normalize(vec3<f32>(nx - slope.x - rings.x, max(ny, 0.2), nz - slope.y - rings.y));
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
    // The endless ground above the water hides it wherever no terrain lies under it; with a
    // coast only on the land side, or where the view ray meets the land before the water.
    if sea.extra.x > 0.5 && below > SKY_DEPTH * 0.5 {
        var land = true;
        if g.coast_info.x > 1.5 {
            land = coast_distance(in.uv) < 0.0;
            let h = g.options.w;
            if !land && g.camera.y > h && in.world.y < h {
                let hit = mix(g.camera.xyz, in.world, (g.camera.y - h) / (g.camera.y - in.world.y));
                land = coast_distance(hit.xz) < 0.0;
            }
        }
        if land {
            discard;
        }
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
    // The ground straight under this point, where the cover map knows it: the sea never lies
    // over land above its mean level (the swash there is the wet pass's sheet), and a crest
    // never stands over sand it has not reached.
    let top = sea_bed(in.world.xz);
    let known = top > -1e8;
    if known && (top > sea.surface.x + 0.01 || top > in.world.y) {
        discard;
    }
    // Otherwise the water depth along the ray to whatever lies behind (perspective: to the bed).
    var path = max(below - depth, 0.0) * dist / max(depth, 1e-3);
    var water_depth = select(1e4, max(sea.surface.x - (in.world - v * path).y, 0.0), below < SKY_DEPTH * 0.5);
    if known {
        // Through the column and out: no further than the bed below this point.
        // shortcut: beyond the cover map the depth is taken along the view ray, deeper than the
        // column at a grazing view, so over a shallow shelf the surf can end at the map's edge;
        // blending the two there turned near shallows glassy (v_sea_beach). Upgrade with a
        // coarse whole-terrain height map for the bed beyond the map.
        path = min(path, (in.world.y - top) / max(v.y, 0.03));
        water_depth = sea.surface.x - top;
    }
    let bed = in.world - v * path;
    // Over a shallow bed the swell shoals and breaks: its slope replaces most of the deep
    // waves' (their crests do not follow the bed); the broken water is choppy.
    let surf = sea_surf(in.uv, water_depth);
    let t = g.time.x;
    let chop = wave_slope(in.uv * 0.9 + vec2<f32>(0.31, -0.17) * t) * 0.18 * surf.zone * (1.0 - smoothstep(0.3, 1.5, footprint));
    var n = s.n;
    n = normalize(vec3<f32>(n.x * (1.0 - 0.6 * surf.weight) - surf.slope.x - chop.x, n.y, n.z * (1.0 - 0.6 * surf.weight) - surf.slope.y - chop.y));
    if dot(n, v) < 0.02 {
        // A back-facing slope seen at a grazing angle: bend it just towards the eye.
        n = normalize(n + v * (0.02 - dot(n, v)));
    }
    // Roughness: glassy water plus the unresolved waves (GGX alpha^2 ~ 2 sigma^2).
    let alpha = clamp(sqrt(0.0004 + 2.0 * s.variance), 0.02, 0.6);
    let rough = sqrt(alpha);
    // The facets of the unresolved waves that face the eye are the ones seen, and they mirror
    // sky about twice their tilt higher than the mean surface's grazing reflection, at a lower
    // Fresnel (Bruneton, Neyret & Holzschuch 2010, sec. 4): the far sea stays blue instead of
    // taking the pale horizon.
    let sigma = min(sqrt(s.variance), 0.3);
    let nov = max(max(dot(n, v), 1e-4), sigma);
    let f = f_schlick(nov, vec3<f32>(0.02)).r;
    let r = reflect(-v, n);
    var sky = environment(normalize(vec3<f32>(r.x, max(r.y, 0.0) + 2.0 * sigma, r.z)), rough);
    var reflected = sky;
    if g.reflection.x > 0.5 && abs(sea.surface.x - g.reflection.y) < 0.05 {
        // The planar reflection at the mean level, shifted by the slope (less with distance);
        // where the surface is rough it blurs into the sky's prefiltered reflection.
        let tilt = (g.view * vec4<f32>(n.x, 0.0, n.z, 0.0)).xy;
        let size = vec2<f32>(textureDimensions(planar_reflection));
        let uv = in.clip.xy * g.viewport.zw + tilt * vec2<f32>(1.0, -1.0) * 1.2 / max(depth, 1.0);
        let planar = textureSampleLevel(planar_reflection, env_sampler, clamp(uv, 0.5 / size, 1.0 - 0.5 / size), 0.0).rgb;
        reflected = mix(planar, sky, max(smoothstep(0.15, 0.45, rough), smoothstep(0.05, 0.2, sigma)));
    }
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
    // Over a shallow sandy bed the light the bed sends back up loses its red first (pure water
    // absorbs about 0.35 /m at 650 nm against 0.06 at 550 and 0.01 at 450 nm: Pope & Fry,
    // Applied Optics 36(33), 1997): the water turns turquoise there and deepens to its colour.
    let shallow = exp(-water_depth * vec3<f32>(0.7, 0.12, 0.03)) * (1.0 - smoothstep(0.0, 8.0, water_depth));
    var tint = sea.color.rgb + vec3<f32>(0.0, 0.12, 0.1) * shallow * (1.0 - sea.surface.w);
    // Breakers stir up the sand: the broken water clouds over in drifting patches.
    let cloud = value_noise(vec3<f32>(in.uv.x * 0.15, t * 0.05, in.uv.y * 0.15));
    tint = mix(tint, vec3<f32>(0.10, 0.11, 0.07), surf.zone * smoothstep(0.3, 0.8, cloud) * 0.6);
    let deep = tint * crest * (sh_irradiance(up) + g.sun_light.rgb * max(l.y, 0.0) * sun_on) / PI;
    var color = f * reflected + glint + (1.0 - f) * (1.0 - transmit) * deep;
    var alpha_out = f + (1.0 - f) * (1.0 - transmit);
    // The bed seen through shallow water: darker patches (weed, wet stones) of a few metres that
    // still read from far off, and the sun focused into moving caustic lines by the waves, which
    // turn into their mean brightening once a pixel spans them. Over (premultiplied) the bed.
    let clear = (1.0 - smoothstep(0.5, 7.0, water_depth)) * (1.0 - 0.7 * surf.zone) * (1.0 - alpha_out);
    if clear > 0.01 {
        let bp = bed.xz;
        let spot = smoothstep(0.55, 0.75, value_noise(vec3<f32>(bp.x / 14.0, 2.7, bp.y / 14.0)) * 0.7
            + value_noise(vec3<f32>(bp.x / 3.5, 5.1, bp.y / 3.5)) * 0.3);
        let k = 0.4 * spot * clear;
        color += k * deep;
        alpha_out += k;
        let c1 = value_noise(vec3<f32>(bp.x * 0.7 + t * 0.25, t * 0.35, bp.y * 0.7));
        let c2 = value_noise(vec3<f32>(bp.x * 0.5 - t * 0.2, t * 0.3 + 7.0, bp.y * 0.5));
        let lines = (1.0 - smoothstep(0.0, 0.07, abs(c1 - 0.5))) + (1.0 - smoothstep(0.0, 0.07, abs(c2 - 0.5)));
        let caustic = mix(lines, 0.25, smoothstep(0.08, 0.4, footprint)) * (1.0 - spot);
        color += caustic * clear * 0.22 * vec3<f32>(0.45, 0.42, 0.33) * g.sun_light.rgb * max(l.y, 0.0) * sun_on / PI;
    }
    // Foam: crests that fold (more and lower thresholds at higher sea states), and a band over
    // shallow beds that the swell runs up and back, broken up by noise.
    let noise = value_noise(vec3<f32>(in.uv.x * 0.9, t * 0.15, in.uv.y * 0.9)) * 0.6
        + value_noise(vec3<f32>(in.uv.x * 3.7, t * 0.4, in.uv.y * 3.7)) * 0.4;
    // Whitecaps start around sea state 3 (half-metre waves) and are everywhere by state 5.
    let state = smoothstep(0.4, 2.5, sea.extra.z);
    let crest_foam = smoothstep(0.6, 0.2, s.jacobian) * state * smoothstep(0.35, 0.7, noise + 0.15 * state);
    // The breakers' whitewater, torn up by the noise into streaks and patches; behind them a
    // lace of foam lines on the broken water (the contours of a drifting noise).
    let lace_n = value_noise(vec3<f32>(in.uv.x * 0.8 + t * 0.05, t * 0.03, in.uv.y * 0.8));
    let lace_m = value_noise(vec3<f32>(in.uv.x * 2.6 - t * 0.04, t * 0.05, in.uv.y * 2.6));
    let lace = (1.0 - smoothstep(0.0, 0.025 + 0.05 * smoothstep(0.05, 0.5, footprint), abs(lace_n - 0.5)))
        * smoothstep(0.45, 0.75, lace_m) * smoothstep(0.4, 0.7, value_noise(vec3<f32>(in.uv.x * 0.25, 4.0, in.uv.y * 0.25)))
        * surf.zone * 0.45;
    let shore_foam = surf.foam * smoothstep(0.3, 0.7, noise + 0.25 * surf.foam) + lace;
    let foam = clamp((crest_foam + shore_foam) * sea.surface.z * 2.0, 0.0, 1.0);
    if foam > 0.0 {
        let white = vec3<f32>(0.8) * (sh_irradiance(n) + g.sun_light.rgb * nol * sun_on) / PI;
        color = mix(color, white, foam);
        alpha_out = mix(alpha_out, 1.0, foam);
    }
    // Where the water thins to nothing at the waterline, fade it out.
    // Swash: the grid's crests do not shoal or break, so over a beach above the mean level they
    // would stand as flat floods; the water runs up only a little way above the mean level
    // (a tenth of the wave height) and thins out there.
    let runup = 0.1 + 0.1 * sea.extra.z;
    var swash = select(1.0, 1.0 - smoothstep(sea.surface.x, sea.surface.x + runup, bed.y), below < SKY_DEPTH * 0.5);
    if known {
        swash = smoothstep(0.0, 0.04, in.world.y - top);
    }
    let edge = smoothstep(0.0, 0.05, path) * swash;
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

// Wet ground just above the waterline and the swash running up it: a full-screen
// pass darkening the opaque scene (and the bed below, which the water then covers)
// within the swash band, as wet sand and stone absorb more (a water film lowers the
// albedo: Lekner & Dorf, "Why some things are darker when wet", Applied
// Optics 27(7), 1988). Only ground the water actually reaches: a point in the
// band must have the sea bed (ground below the level) within a few metres
// around it on screen, so low ground inland stays dry.
@vertex
fn vs_sea_wet(@builtin(vertex_index) vi: u32) -> Varying {
    var out: Varying;
    out.clip = fullscreen(vi);
    out.clip.z = 0.0;
    return out;
}

// The opaque scene's world position under `pixel` (perspective only), and whether anything is there.
fn sea_wet_world(pixel: vec2<f32>) -> vec4<f32> {
    let below = textureLoad(linear_depth, vec2<i32>(pixel), 0).r;
    let ndc = vec2<f32>(pixel.x * g.viewport.z * 2.0 - 1.0, 1.0 - pixel.y * g.viewport.w * 2.0);
    let h = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    let ray = normalize(h.xyz / h.w - g.camera.xyz);
    let forward = -normalize(vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]));
    let world = g.camera.xyz + ray * (below / max(dot(ray, forward), 1e-3));
    return vec4<f32>(world, select(1.0, 0.0, below > SKY_DEPTH * 0.5));
}

// 0..1: how close the water's edge is to `world`: 1 at the bed, fading out by 8 m. A spiral of
// samples (turned per pixel and photo frame), each under water counting by its nearness; at
// the edge about half of them are, so twice that share is fully wet.
fn sea_wet_near(world: vec3<f32>, pixel: vec2<f32>) -> f32 {
    if world.y < sea.surface.x {
        return 1.0;
    }
    let jitter = ign(pixel + 17.0 * g.grade.w);
    var near = 0.0;
    var total = 0.0;
    for (var k = 0u; k < 24u; k++) {
        let r = 8.0 * (f32(k) + jitter) / 24.0;
        let a = (f32(k) + jitter) * 2.3999632; // the golden angle
        let c = g.view_proj * vec4<f32>(world + r * vec3<f32>(cos(a), 0.0, sin(a)), 1.0);
        let w = 1.0 - smoothstep(1.0, 8.0, r);
        total += w;
        if c.w <= 0.0 {
            continue;
        }
        let ndc = c.xy / c.w;
        if abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 {
            continue;
        }
        let q = sea_wet_world(vec2<f32>((ndc.x * 0.5 + 0.5) * g.viewport.x, (0.5 - ndc.y * 0.5) * g.viewport.y));
        if q.w > 0.0 && q.y < sea.surface.x && sea_covers(q.xz) {
            near += w;
        }
    }
    return saturate(2.0 * near / max(total, 1e-3));
}

@fragment
fn fs_sea_wet(in: Varying) -> @location(0) vec4<f32> {
    let below = textureLoad(linear_depth, vec2<i32>(in.clip.xy), 0).r;
    if below > SKY_DEPTH * 0.5 || is_ortho() || g.time.w > 0.5 {
        return vec4<f32>(0.0);
    }
    let world = sea_wet_world(in.clip.xy).xyz;
    let runup = surf_runup();
    // The tide line wanders: the last swash reached higher here and lower there, by up to a
    // third of the band over 10-40 m along the shore, so the line is not a contour.
    let band = max(sea.color.w, 1.3 * runup);
    let wander = (value_noise(vec3<f32>(world.xz * 0.025, 4.2)) * 0.7 + value_noise(vec3<f32>(world.xz * 0.1, 8.1)) * 0.3 - 0.5) * 0.66;
    let top = sea.surface.x + band * (1.0 + wander);
    var wet = (1.0 - smoothstep(top - 0.6 * band, top, world.y))
        * (1.0 - smoothstep(300.0, 500.0, below)) * (1.0 - g.snow.x);
    if wet <= 0.0 || !sea_covers(world.xz) {
        return vec4<f32>(0.0);
    }
    wet *= sea_wet_near(world, in.clip.xy);
    // Darker wet ground (premultiplied): a water film takes a dry sand's albedo of about 0.35
    // to 0.2 (Lekner & Dorf), 0.43 of it gone; damp sand higher up loses less (the ramp above).
    let dark = 0.43 * wet;
    // The swash: each bore that reaches the waterline (phase 0 there, as `sea_surf`) rushes up
    // the beach to the run-up in about a third of the period and drains back in the rest, as a
    // thin sheet with a foam front on the way up and lacy foam on the way down.
    let z = world.y - sea.surface.x;
    let theta = sea.surf.x * g.time.x + surf_along(world.xz);
    let s = fract(theta / (2.0 * PI));
    // Each bore runs up further here and less there (bars, cusps, the wave's own crest): the
    // swash's edge wanders along the shore over 5-15 m instead of following a contour.
    let along = value_noise(vec3<f32>(world.xz * 0.12, theta * 0.159)) * 0.6 + value_noise(vec3<f32>(world.xz * 0.4, 3.7)) * 0.4;
    let reach = runup * (0.55 + 0.9 * along) * surf_group(world.xz, theta) * select(pow(1.0 - (s - 0.3) / 0.7, 1.5), sin(s / 0.3 * 0.5 * PI), s < 0.3);
    var sheet = (1.0 - smoothstep(reach - 0.02, reach, z)) * smoothstep(-0.03, 0.02, z) * wet * smoothstep(0.02, 0.6, sea.extra.z);
    let noise = value_noise(vec3<f32>(world.x * 2.3, g.time.x * 0.3, world.z * 2.3));
    let front = (1.0 - smoothstep(0.0, 0.04, abs(reach - 0.015 - z))) * select(0.35, 1.0, s < 0.3);
    let lace = select(0.0, 0.35 * smoothstep(0.55, 0.8, noise), s > 0.3);
    let foam = saturate((front + lace) * sheet) * sea.surface.z * 2.0;
    if sheet <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, dark);
    }
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let v = normalize(to_eye(world));
    let lit = (sh_irradiance(up) + g.sun_light.rgb * max(g.sun_dir.y, 0.0)) / PI;
    let fr = f_schlick(max(v.y, 1e-3), vec3<f32>(0.02)).r;
    // A sheet a few centimetres deep: the sand shows through it, darkened, with the sky
    // mirrored on top.
    let water = environment(reflect(-v, up), 0.2) * fr / max(0.15 + 0.6 * fr, 1e-3);
    let cover = mix(0.15 + 0.6 * fr, 0.95, foam) * sheet;
    let rgb = mix(water, vec3<f32>(0.8) * lit, foam) * cover;
    return vec4<f32>(rgb, 1.0 - (1.0 - dark) * (1.0 - cover));
}
