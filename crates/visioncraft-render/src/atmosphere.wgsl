// The physical sky's look-up tables, after Hillaire, "A Scalable and
// Production Ready Sky and Atmosphere Rendering Technique", EGSR 2020
// (paper and its MIT-licensed sample code, sebh/UnrealEngineSkyAtmosphere,
// for the Earth constants), with the transmittance parametrisation of
// Bruneton, "A Qualitative and Quantitative Evaluation of 8 Clear Sky
// Models" / "Precomputed Atmospheric Scattering: a New Implementation"
// (2017, BSD licence), section "Transmittance".
//
// 1. Transmittance LUT (256x64): transmittance from (radius, cos zenith) to
//    the top of the atmosphere. Once.
// 2. Multiple-scattering LUT (32x32): Hillaire's Psi_ms (eq. 10) per (cos
//    sun zenith, altitude): second-order scattering with an isotropic phase,
//    summed to infinite orders as a geometric series. Once.
// 3. Sky-view LUT (192x108): single scattering plus Psi_ms along view rays
//    from the ground, for the current sun. Whenever the sun moves.
// All radiance is per unit solar illuminance at the top of the atmosphere;
// lib.rs scales by 128 klx. Distances in km.
// Module: common.wgsl + sky.wgsl (for `sky_view_uv`) + this file.

@group(1) @binding(0) var transmittance_lut: texture_2d<f32>;
@group(1) @binding(1) var multi_lut: texture_2d<f32>;
@group(1) @binding(2) var lut_sampler: sampler;

const BOTTOM: f32 = 6360.0;
const TOP: f32 = 6460.0;
const VIEWER: f32 = 0.002; // viewer altitude for the sky-view LUT (km)
const RAYLEIGH: vec3<f32> = vec3<f32>(0.005802, 0.013558, 0.0331);
const RAYLEIGH_H: f32 = 8.0;
const MIE_SCATTER: f32 = 0.003996;
const MIE_EXTINCT: f32 = 0.00444;
const MIE_H: f32 = 1.2;
const MIE_G: f32 = 0.8;
// Ozone absorption per km at the layer's peak. Hillaire's three values are
// the Chappuis band sampled at three wavelengths; spread over our broad
// sRGB primaries the band (peaking at ~600 nm, between green and red)
// covers red about as much as green, and three point samples put too
// little of it on red: with the sun 2-8 degrees below the horizon the light
// reaching the sky has crossed the ozone layer edge-on, and the sky turned
// magenta. Red raised a little above green (picked by eye across -2..-8
// degrees against blue-hour photographs): blue hour is blue, and sunsets
// (where Rayleigh extinction dominates) stay warm.
const OZONE: vec3<f32> = vec3<f32>(0.0024, 0.001881, 0.000085);
const GROUND_ALBEDO: f32 = 0.3;

struct Medium {
    rayleigh: vec3<f32>, // scattering
    mie: f32,            // scattering
    extinction: vec3<f32>,
}

fn medium(altitude: f32) -> Medium {
    let h = max(altitude, 0.0);
    let r = RAYLEIGH * exp(-h / RAYLEIGH_H);
    let m = exp(-h / MIE_H);
    let ozone = OZONE * max(0.0, 1.0 - abs(h - 25.0) / 15.0);
    return Medium(r, MIE_SCATTER * m, r + MIE_EXTINCT * m + ozone);
}

// Distance along a ray from radius r with cosine mu to a sphere of radius
// `radius`; negative when it misses (or for the near hit, behind).
fn to_sphere(r: f32, mu: f32, radius: f32) -> f32 {
    let disc = r * r * (mu * mu - 1.0) + radius * radius;
    if disc < 0.0 {
        return -1.0;
    }
    return -r * mu + sqrt(disc);
}

fn hits_ground(r: f32, mu: f32) -> bool {
    return mu < 0.0 && r * r * (mu * mu - 1.0) + BOTTOM * BOTTOM >= 0.0;
}

fn to_ground(r: f32, mu: f32) -> f32 {
    return -r * mu - sqrt(max(r * r * (mu * mu - 1.0) + BOTTOM * BOTTOM, 0.0));
}

const H_MAX: f32 = 1131.7; // sqrt(TOP^2 - BOTTOM^2)

fn transmittance_uv(r: f32, mu: f32) -> vec2<f32> {
    let rho = sqrt(max(r * r - BOTTOM * BOTTOM, 0.0));
    let d = max(to_sphere(r, mu, TOP), 0.0);
    let d_min = TOP - r;
    let d_max = rho + H_MAX;
    let x = vec2<f32>((d - d_min) / (d_max - d_min), rho / H_MAX);
    let size = vec2<f32>(256.0, 64.0);
    return 0.5 / size + x * (1.0 - 1.0 / size);
}

// Transmittance from radius r towards cos-zenith mu up to the top (0 when
// the ray hits the ground).
fn transmittance(r: f32, mu: f32) -> vec3<f32> {
    if hits_ground(r, mu) {
        return vec3<f32>(0.0);
    }
    return textureSampleLevel(transmittance_lut, lut_sampler, transmittance_uv(r, mu), 0.0).rgb;
}

fn multi_scattering(r: f32, mu_s: f32) -> vec3<f32> {
    let uv = vec2<f32>(mu_s * 0.5 + 0.5, (r - BOTTOM) / (TOP - BOTTOM));
    let size = vec2<f32>(32.0);
    return textureSampleLevel(multi_lut, lut_sampler, 0.5 / size + clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * (1.0 - 1.0 / size), 0.0).rgb;
}

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    return Out(fullscreen(i));
}

@fragment
fn fs_transmittance(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(256.0, 64.0);
    let x = (in.clip.xy - 0.5) / (size - 1.0);
    let rho = H_MAX * x.y;
    let r = sqrt(rho * rho + BOTTOM * BOTTOM);
    let d_min = TOP - r;
    let d_max = rho + H_MAX;
    let d = d_min + x.x * (d_max - d_min);
    var mu = 1.0;
    if d > 0.0 {
        mu = clamp((H_MAX * H_MAX - rho * rho - d * d) / (2.0 * r * d), -1.0, 1.0);
    }
    let steps = 40;
    let dt = d / f32(steps);
    var depth = vec3<f32>(0.0);
    for (var i = 0; i < steps; i++) {
        let t = (f32(i) + 0.5) * dt;
        let ri = sqrt(r * r + 2.0 * r * mu * t + t * t);
        depth += medium(ri - BOTTOM).extinction * dt;
    }
    return vec4<f32>(exp(-depth), 1.0);
}

// Second-order luminance (isotropic phase) and the transfer factor f_ms
// along one direction from (0, r, 0), per unit sun illuminance.
struct MsRay {
    l: vec3<f32>,
    f: vec3<f32>,
}

fn ms_ray(r: f32, dir: vec3<f32>, sun: vec3<f32>) -> MsRay {
    var out = MsRay(vec3<f32>(0.0), vec3<f32>(0.0));
    let mu = dir.y;
    let ground = hits_ground(r, mu);
    var t_max = to_sphere(r, mu, TOP);
    if ground {
        t_max = to_ground(r, mu);
    }
    let steps = 20;
    let dt = t_max / f32(steps);
    var through = vec3<f32>(1.0);
    let iso = 1.0 / (4.0 * PI);
    for (var i = 0; i < steps; i++) {
        let p = vec3<f32>(0.0, r, 0.0) + dir * ((f32(i) + 0.5) * dt);
        let rp = length(p);
        let m = medium(rp - BOTTOM);
        let scatter = m.rayleigh + vec3<f32>(m.mie);
        let step_t = exp(-m.extinction * dt);
        let integral = (1.0 - step_t) / max(m.extinction, vec3<f32>(1e-9));
        let sun_t = transmittance(rp, dot(p / rp, sun));
        out.l += through * scatter * iso * sun_t * integral;
        out.f += through * scatter * integral;
        through *= step_t;
    }
    if ground {
        let p = vec3<f32>(0.0, r, 0.0) + dir * t_max;
        let n = normalize(p);
        out.l += through * transmittance(BOTTOM, dot(n, sun)) * max(dot(n, sun), 0.0) * GROUND_ALBEDO / PI;
    }
    return out;
}

@fragment
fn fs_multi_scattering(in: Out) -> @location(0) vec4<f32> {
    let x = (in.clip.xy - 0.5) / 31.0;
    let mu_s = x.x * 2.0 - 1.0;
    let r = BOTTOM + max(x.y, 1e-4) * (TOP - BOTTOM);
    let sun = vec3<f32>(sqrt(max(1.0 - mu_s * mu_s, 0.0)), mu_s, 0.0);
    // 8 x 8 directions uniform over the sphere (equal-area bands).
    var l2 = vec3<f32>(0.0);
    var f = vec3<f32>(0.0);
    for (var i = 0; i < 8; i++) {
        for (var j = 0; j < 8; j++) {
            let cos_t = 1.0 - 2.0 * (f32(i) + 0.5) / 8.0;
            let sin_t = sqrt(1.0 - cos_t * cos_t);
            let phi = 2.0 * PI * (f32(j) + 0.5) / 8.0;
            let ray = ms_ray(r, vec3<f32>(sin_t * cos(phi), cos_t, sin_t * sin(phi)), sun);
            l2 += ray.l;
            f += ray.f;
        }
    }
    // Uniform phase over the sphere: integrals become means (eq. 6-9).
    l2 /= 64.0;
    f /= 64.0;
    return vec4<f32>(l2 / (1.0 - f), 1.0);
}

fn phase_rayleigh(c: f32) -> f32 {
    return 3.0 / (16.0 * PI) * (1.0 + c * c);
}

// Cornette-Shanks phase function (Hillaire 2020, eq. 4).
fn phase_mie(c: f32) -> f32 {
    let g2 = MIE_G * MIE_G;
    let k = 3.0 / (8.0 * PI) * (1.0 - g2) / (2.0 + g2);
    return k * (1.0 + c * c) / pow(max(1.0 + g2 - 2.0 * MIE_G * c, 1e-4), 1.5);
}

@fragment
fn fs_sky_view(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(192.0, 108.0);
    let uv = (in.clip.xy - 0.5) / (size - 1.0);
    // Inverse of sky_view_uv (sky.wgsl).
    let v = 1.0 - uv.y;
    let s = (v - 0.5) * 2.0;
    let elevation = sign(s) * s * s * 0.5 * PI;
    let phi = uv.x * PI;
    let sun = g.sun_dir.xyz;
    var forward = vec2<f32>(sun.x, sun.z);
    if length(forward) < 1e-5 {
        forward = vec2<f32>(1.0, 0.0);
    }
    forward = normalize(forward);
    let side = vec2<f32>(-forward.y, forward.x);
    let h = forward * cos(phi) + side * sin(phi);
    let dir = vec3<f32>(h.x * cos(elevation), sin(elevation), h.y * cos(elevation));

    let r = BOTTOM + VIEWER;
    let mu = dir.y;
    var t_max = to_sphere(r, mu, TOP);
    if hits_ground(r, mu) {
        t_max = to_ground(r, mu);
    }
    let c = dot(dir, sun);
    let pr = phase_rayleigh(c);
    let pm = phase_mie(c);
    let steps = 32;
    var l = vec3<f32>(0.0);
    var through = vec3<f32>(1.0);
    var t0 = 0.0;
    for (var i = 0; i < steps; i++) {
        // Quadratic step distribution: short steps near the viewer.
        let f = (f32(i) + 1.0) / f32(steps);
        let t1 = t_max * f * f;
        let dt = t1 - t0;
        let p = vec3<f32>(0.0, r, 0.0) + dir * (0.5 * (t0 + t1));
        t0 = t1;
        let rp = length(p);
        let m = medium(rp - BOTTOM);
        let mu_s = dot(p / rp, sun);
        let single = (m.rayleigh * pr + vec3<f32>(m.mie * pm)) * transmittance(rp, mu_s);
        let multi = (m.rayleigh + vec3<f32>(m.mie)) * multi_scattering(rp, mu_s);
        let step_t = exp(-m.extinction * dt);
        l += through * (single + multi) * (1.0 - step_t) / max(m.extinction, vec3<f32>(1e-9));
        through *= step_t;
    }
    return vec4<f32>(l, 1.0);
}
