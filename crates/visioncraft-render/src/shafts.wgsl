// What the air does to the picture after the forward pass, composited onto the HDR image
// before bloom (lib.rs `Renderer::render`, step 6b):
//
// 1. Light shafts (`fs_march`, half resolution, then `fs_blur` across and down): the haze, mist
//    and rain fog of common.wgsl add the sun's in-scattered light as if the whole medium were
//    lit. Here a ray marches from the eye through the view up to the surface (at most the
//    shadow distance), samples the four sun cascades and sums the light the analytic fog added
//    where the medium is in shadow, with the same densities, transmittance and phase
//    functions (`fog_depth`, `mist_depth`, the Henyey-Greenstein lobes of `fog_inscatter` and
//    `mist_inscatter`). Taking that light out again leaves lit beams between shadowed lanes:
//    shafts appear exactly where there is medium to light, and with no medium nothing changes.
//    The march along the view through the shadow map at reduced resolution is the one of the
//    public GTA V frame study (Courrèges, "GTA V - Graphics Study", 2015) and RDR2's
//    half-resolution far march (Bauer, "Creating the Atmospheric World of Red Dead Redemption
//    2", SIGGRAPH 2019 Advances); the integral is Hillaire's (Frostbite, SIGGRAPH 2015): the
//    in-scattered light times the transmittance from the eye, per unit length. Steps are
//    spaced quadratically (short near the eye, where a beam's angle is widest), jittered per
//    pixel with interleaved gradient noise (Jimenez 2014) and the noise removed by a
//    depth-aware (bilateral) blur, then upsampled depth-aware (Kopf et al. 2007).
// 2. The sun's glare (`fs_sun_seen` into one texel, then in `fs_composite`): the light a lens
//    and the eye scatter round a bright source, as a point spread function round the sun's
//    direction (two lobes of the form w^2 / (pi (theta^2 + w^2)^2), each integrating to one
//    over the sphere), times the share of the sun's disc the scene leaves open (its texels in
//    the depth buffer), so a low sun behind leaves glows through their gaps. After Spencer et
//    al., "Physically-Based Glare Effects for Digital Images", SIGGRAPH 1995.
// 3. Aerial perspective (`aerial_at`): the clear air's light and transmittance up to the
//    surface from the 32^3 froxel LUT (atmosphere.wgsl `fs_aerial`; Hillaire, EGSR 2020).
//
// `fs_composite` writes (light, transmittance) with the blend `src + dst * src.a`: the image
// times the air's transmittance, plus its light and the glare, minus the shadowed fog's light.
// Module: common.wgsl + this.

@group(1) @binding(0) var shadow_map: texture_depth_2d_array;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
@group(1) @binding(2) var depth_tex: texture_2d<f32>;
// The half-resolution shafts being blurred or composited: rgb the light to take out
// (pre-exposed), a the linear depth at that texel.
@group(1) @binding(3) var source: texture_2d<f32>;
@group(1) @binding(4) var aerial_lut: texture_2d<f32>;
@group(1) @binding(5) var lut_sampler: sampler;
// r: the share of the sun's disc left open (`fs_sun_seen`).
@group(1) @binding(6) var sun_seen: texture_2d<f32>;

override HORIZONTAL: bool = true;

const AERIAL_SIZE: f32 = 32.0;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    return Out(fullscreen(i));
}

// The world direction through a pixel (render size, top-left origin).
fn pixel_dir(pixel: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x * g.viewport.z * 2.0 - 1.0, 1.0 - pixel.y * g.viewport.w * 2.0);
    let p = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    return normalize(p.xyz / p.w - g.camera.xyz);
}

// The camera's viewing axis (world): linear depth is the distance along it.
fn view_axis() -> vec3<f32> {
    return -normalize(vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]));
}

// The sun's light at a point in the air: one hardware 2x2 PCF tap of the first cascade that
// covers it (by view depth, as scene.wgsl `sun_shadow`); lit beyond the cascades.
fn air_lit(world: vec3<f32>, depth: f32) -> f32 {
    let count = u32(g.shadow.x);
    var c = 0u;
    while c < count && depth > g.splits[c] {
        c++;
    }
    while c < count {
        let p = g.cascades[c] * vec4<f32>(world, 1.0);
        let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
        if all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0)) && p.z <= 1.0 {
            return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, i32(c), p.z);
        }
        c++;
    }
    return 1.0;
}

// The phase functions of the haze and of the mist (common.wgsl `fog_inscatter`,
// `mist_inscatter`).
fn haze_phase(c: f32) -> f32 {
    return 0.64 / (4.0 * PI * pow(1.36 - 1.2 * c, 1.5));
}

fn mist_phase(c: f32) -> f32 {
    let gg = 0.75;
    return (1.0 - gg * gg) / (4.0 * PI * pow(1.0 + gg * gg - 2.0 * gg * c, 1.5));
}

@fragment
fn fs_march(in: Out) -> @location(0) vec4<f32> {
    let div = i32(g.shafts.w);
    let full = vec2<i32>(g.viewport.xy) - 1;
    let pixel = min(vec2<i32>(in.clip.xy) * div, full);
    let depth = textureLoad(depth_tex, pixel, 0).r;
    let centre = vec2<f32>(pixel) + 0.5;
    let dir = pixel_dir(centre);
    let cos_view = max(dot(dir, view_axis()), 1e-3);
    let sky = depth >= SKY_DEPTH * 0.5;
    let t_surface = select(depth / cos_view, 1e9, sky);
    let t_end = min(t_surface, g.shafts.z);
    let steps = i32(g.shafts.y);
    // A photo's samples each take another jitter (`grade.w` is its seed).
    let jitter = fract(ign(in.clip.xy) + g.grade.w * 0.618034);
    let c = dot(dir, g.sun_dir.xyz);
    let haze_w = g.fog.w * haze_phase(c);
    let mist_w = g.mist.w * mist_phase(c) * smoothstep(-0.05, 0.05, g.sun_dir.y);
    // The fog's sun light along the march: all of it, and the part in shadow.
    var total = 0.0;
    var lost = 0.0;
    // The optical depth from the eye, summed along the march (midpoint rule).
    var depth_sum = 0.0;
    for (var i = 0; i < steps; i++) {
        // t = t_end u^2: dt = 2 u t_end du.
        let u = (f32(i) + jitter) / f32(steps);
        let t = t_end * u * u;
        let dt = 2.0 * u * t_end / f32(steps);
        let p = g.camera.xyz + dir * t;
        // The haze's first camera.w metres are clear air (common.wgsl `fog_depth`).
        let haze = select(0.0, g.fog.x * exp(min(-g.fog.y * (p.y - g.fog.z), 80.0)), t > g.camera.w);
        let mist = g.mist.x * exp(min(-g.mist.y * (p.y - g.mist.z), 80.0));
        let transmit = exp(-(depth_sum + 0.5 * (haze + mist) * dt));
        depth_sum += (haze + mist) * dt;
        let w = transmit * dt * (haze * haze_w + mist * mist_w);
        total += w;
        lost += w * (1.0 - air_lit(p, t * cos_view));
    }
    if total <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, depth);
    }
    // The sun's light the forward pass put into this ray (common.wgsl `veil`, sky.wgsl
    // `fs_sky`), per unit of its illuminance.
    let ta = fog_depth(g.camera.y, dir.y, t_surface);
    let tb = mist_depth(g.camera.y, dir.y, t_surface);
    var added = 0.0;
    if sky {
        // The sky takes the haze only in a band at the horizon (sky.wgsl `fs_sky`).
        let band = 0.25 * clamp(g.fog.x * 1000.0 / 2.5, 0.0, 1.0);
        let haze_share = 1.0 - smoothstep(-0.02, band, dir.y);
        added = haze_w * haze_share * (1.0 - exp(-ta)) * exp(-tb) + mist_w * (1.0 - exp(-tb));
    } else if ta + tb > 0.0 {
        let transmit = exp(-(ta + tb));
        added = (haze_w * ta + mist_w * tb) / (ta + tb) * (1.0 - transmit) * fog_near(transmit);
    }
    // In the march's reach the shadowed share is what it is; beyond it (the shadow maps end
    // there) the ray is taken to be shadowed in the same share, so the beams run on into the
    // glow round the sun (as Bauer 2019 lets the sun's visibility modulate the sky's light:
    // "not physically correct but plausible").
    let s = g.shafts.x;
    let lit = 1.0 - s * lost / total;
    // The analytic fog scatters only part of the sun's light (its sun glow, lib.rs
    // `Atmosphere::sun_glow`): unshadowed, the full forward lobe glowed through crowns and
    // walls. Shadowed, the medium takes its full single scattering again (glow 1).
    let glow = max(max(g.fog.w, g.mist.w), 1e-3);
    let boost = (glow + s * (1.0 - glow)) / glow;
    // The light to take out (negative: to add, where the medium is lit).
    return vec4<f32>(added * (1.0 - boost * lit) * g.sun_light.rgb, depth);
}

// Depth similarity of two linear depths (relative).
fn depth_gap(a: f32, b: f32) -> f32 {
    return abs(a - b) / max(min(a, b), 1e-3);
}

@fragment
fn fs_blur(in: Out) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let last = vec2<i32>(textureDimensions(source)) - 1;
    let centre = textureLoad(source, p, 0);
    let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), HORIZONTAL);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = -6; i <= 6; i++) {
        let s = textureLoad(source, clamp(p + step * i, vec2<i32>(0), last), 0);
        let w = exp(-f32(i * i) / 18.0) * exp(-depth_gap(s.a, centre.a) * 10.0);
        sum += s.rgb * w;
        weight += w;
    }
    return vec4<f32>(sum / weight, centre.a);
}

// The half-resolution shafts at a full-resolution pixel: bilinear weights times depth
// similarity (as scene.wgsl `upsample_occlusion`), so a beam does not leak onto a crown's edge.
fn shafts_at(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let last = vec2<i32>(textureDimensions(source)) - 1;
    let u = (pixel - 0.5) / g.shafts.w;
    let base = vec2<i32>(floor(u));
    let f = fract(u);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i & 1, i >> 1u);
        let s = textureLoad(source, clamp(base + o, vec2<i32>(0), last), 0);
        let bilinear = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let w = (bilinear + 1e-3) / (1e-3 + depth_gap(s.a, depth));
        sum += s.rgb * w;
        weight += w;
    }
    return sum / weight;
}

// One slice of the aerial perspective LUT at screen `uv` (bilinear inside the slice).
fn aerial_slice(k: f32, uv: vec2<f32>) -> vec4<f32> {
    let at = vec2<f32>((k + uv.x) / AERIAL_SIZE, uv.y);
    return textureSampleLevel(aerial_lut, lut_sampler, at, 0.0);
}

// The clear air's light (kcd/m2) and transmittance `km` kilometres along the view at `uv`:
// slices at g.aerial.y ((k + 0.5) / 32)^2 km, faded in from nothing in front of the first.
fn aerial_at(uv_in: vec2<f32>, km: f32) -> vec4<f32> {
    let uv = clamp(uv_in, vec2<f32>(0.5 / AERIAL_SIZE), vec2<f32>(1.0 - 0.5 / AERIAL_SIZE));
    let s = sqrt(max(km, 0.0) / g.aerial.y) * AERIAL_SIZE - 0.5;
    if s < 0.0 {
        let first = aerial_slice(0.0, uv);
        let w = clamp(s * 2.0 + 1.0, 0.0, 1.0);
        return vec4<f32>(first.rgb * w, mix(1.0, first.a, w));
    }
    let k = min(floor(s), AERIAL_SIZE - 2.0);
    return mix(aerial_slice(k, uv), aerial_slice(k + 1.0, uv), min(s - k, 1.0));
}

// The glare's lobe of width `w` (rad) at `theta` from the sun: integrates to one.
fn glare_lobe(theta: f32, w: f32) -> f32 {
    let d = theta * theta + w * w;
    return w * w / (PI * d * d);
}

@fragment
fn fs_sun_seen(in: Out) -> @location(0) vec4<f32> {
    let clip = g.view_proj * vec4<f32>(g.camera.xyz + g.sun_dir.xyz * 1000.0, 1.0);
    if clip.w <= 0.0 {
        return vec4<f32>(0.0);
    }
    let ndc = clip.xy / clip.w;
    let centre = vec2<f32>((ndc.x * 0.5 + 0.5) * g.viewport.x, (0.5 - ndc.y * 0.5) * g.viewport.y);
    // The disc's radius in pixels (at least two, so a far camera still sees it).
    let radius = max(acos(g.sun_light.w) * abs(g.proj[1][1]) * 0.5 * g.viewport.y, 2.0);
    let size = vec2<i32>(g.viewport.xy);
    var open = 0.0;
    var count = 0.0;
    for (var j = 0; j < 8; j++) {
        for (var i = 0; i < 8; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 1.0;
            if dot(o, o) > 1.0 {
                continue;
            }
            count += 1.0;
            let q = vec2<i32>(floor(centre + o * radius));
            // Off the picture: open (a sun just out of frame still flares).
            if any(q < vec2<i32>(0)) || any(q >= size) {
                open += 1.0;
                continue;
            }
            open += select(0.0, 1.0, textureLoad(depth_tex, q, 0).r >= SKY_DEPTH * 0.5);
        }
    }
    return vec4<f32>(open / max(count, 1.0));
}

@fragment
fn fs_composite(in: Out) -> @location(0) vec4<f32> {
    let depth = textureLoad(depth_tex, vec2<i32>(in.clip.xy), 0).r;
    let dir = pixel_dir(in.clip.xy);
    var light = vec3<f32>(0.0);
    var transmit = 1.0;
    // The sky already is the air's light; only surfaces take the aerial perspective.
    if g.aerial.x > 0.5 && depth < SKY_DEPTH * 0.5 {
        let t = depth / max(dot(dir, view_axis()), 1e-3);
        let air = aerial_at(in.clip.xy * g.viewport.zw, t / 1000.0);
        light = air.rgb * g.night.w;
        transmit = air.a;
    }
    if g.shafts.x > 0.0 {
        light -= shafts_at(in.clip.xy, depth) * transmit;
    }
    if g.glare.x > 0.0 {
        let seen = textureLoad(sun_seen, vec2<i32>(0), 0).r;
        let theta = 2.0 * asin(min(length(dir - g.sun_dir.xyz) * 0.5, 1.0));
        let lobe = mix(glare_lobe(theta, g.glare.y), glare_lobe(theta, g.glare.z), g.glare.w);
        light += g.sun_light.rgb * (g.glare.x * seen * lobe);
    }
    return vec4<f32>(light, transmit);
}
