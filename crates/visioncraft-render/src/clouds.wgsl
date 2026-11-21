// Volumetric clouds: a raymarch through a layer of cloud over the curved ground, at a quarter,
// a third or half of the render size, upsampled over frames to twice that from a quarter or a
// third (clouds.rs). Module: common.wgsl + this.
//
// The cloud model is the one of Schneider, "The Real-time Volumetric Cloudscapes of Horizon
// Zero Dawn" (SIGGRAPH 2015 Advances) and "Nubis: Authoring Real-Time Volumetric Cloudscapes
// with the Decima Engine" (SIGGRAPH 2017 Advances), from their public slides:
// - a weather map (common.wgsl `cloud_weather`) says where clouds form and of what type;
// - a height gradient per type shapes the column from flat stratus to towering cumulonimbus,
//   and the cover narrows towards the tops, so a cell rounds into a dome instead of rising as
//   a block with flat sides;
// - a Perlin-Worley noise eroded by a low-frequency Worley fractal gives the billows, and the
//   coverage cuts them (remap by 1 - coverage);
// - a high-frequency Worley fractal, twisted by a curl noise, erodes what the coverage left: a
//   remap by the base density, so only the thin edges go and the core stays whole; billowy
//   (the inverted noise: round puffs) in the body and at the base, wispy towards the tops;
// - the tops lean downwind;
// - far away the noise is read from its coarser levels and the detail fades, by the size of a
//   texel there, so a distant layer seen edge-on does not alias into thin streaks.
// Light, after Hillaire, "Physically Based Sky, Atmosphere and Cloud Rendering in
// Frostbite" (SIGGRAPH 2016 Physically Based Shading course):
// - the sun's light through a short march towards the sun (Beer-Lambert);
// - a dual-lobe Henyey-Greenstein phase (a strong forward lobe for the silver lining and a
//   weak backward one);
// - multiple scattering approximated by octaves of weaker extinction, scattering and
//   anisotropy (Wrenninge et al., "Oz: The Great and Volumetric", SIGGRAPH 2013 Talks);
// - Schneider's powder term for the dark edges of dense cloud seen away from the sun, and his
//   2017 in-scattering probability: light gathers in the dense middle of a cloud and towards
//   its top, so a base is darker where the cloud over it is thin and lighter under its
//   billows, not one flat grey;
// - the sun's light diffused through thick cloud, which the octaves above lose: the diffuse
//   transmittance of a scattering slab in the two-stream approximation, 1 / (1 + 3/4 (1 - g)
//   tau) less its direct part (Bohren, "Multiple scattering of light and some of its
//   observable consequences", Am. J. Phys. 1987), so an overcast base is the grey of the sun's
//   light soaked through it rather than the blue of the sky above;
// - the sky's light from its SH as ambient, brighter towards the tops and the less the deeper
//   in the cloud a point lies, shut out by a closed layer, plus the ground's;
// - a lightning flash in a storm: a point of light inside the cloud, spread by the cloud as
//   diffusion spreads it (falling off with the distance over a diffusion length);
// - the energy-conserving integration of each step's in-scattering (Hillaire 2015).
// Rays start at an interleaved-gradient jitter turned by the frame. Each frame marches one ray
// per texel of the march image, through one of the four texels of a 2x2 block of the history,
// in turn (or through its own texel, where the history keeps the march's size); the resolve pass blends the new samples into the history, reprojected by direction
// (the clouds are far enough that the camera's position does not matter) and clamped to the
// new samples round it, so the history keeps twice the march's detail without ghosting (the
// temporal upsampling of Schneider 2015 with the neighbourhood clamp of Karis, "High-Quality
// Temporal Supersampling", SIGGRAPH 2014 Advances).
// What leaves the march: rgb the clouds' light hazed by the air in front of them (the aerosol
// haze, and the clear air's Rayleigh extinction towards the colour of the sky behind), a the
// share of the sky behind left (sky.wgsl `background`).

struct CloudFrame {
    // The last frame's view-projection without its translation: directions to its pixels.
    prev_view_proj: mat4x4<f32>,
    // x frame counter (jitter), y weight of a texel's own new sample against the history,
    // z sub-samples per pixel, w march steps.
    params: vec4<f32>,
    // x steps towards the sun, yz the march image's size (texels), w history texels per march
    // texel (1: the march writes the history, or the resolve keeps the march's size; 2: it
    // upsamples it).
    more: vec4<f32>,
    // xy this frame's march samples' offset in their 2x2 block of the history (texels), z the
    // weight of the new samples interpolated to the other texels (1: starting over, the
    // samples at the blocks' centres, xy 0.5, interpolated to every texel), w the history's clamp
    // (standard deviations of the new samples round it).
    up: vec4<f32>,
}

@group(1) @binding(0) var<uniform> cf: CloudFrame;
@group(1) @binding(1) var base_noise: texture_3d<f32>;
@group(1) @binding(2) var detail_noise: texture_3d<f32>;
@group(1) @binding(3) var weather_map: texture_2d<f32>;
@group(1) @binding(4) var repeat_sampler: sampler;
@group(1) @binding(5) var curl_map: texture_2d<f32>;
// The resolve's: this frame's march, the last history and their sampler.
@group(2) @binding(0) var current: texture_2d<f32>;
@group(2) @binding(1) var history: texture_2d<f32>;
@group(2) @binding(2) var clamp_sampler: sampler;

const EARTH_R: f32 = 6360000.0;
// The furthest cloud drawn (m along the ray to the layer's base).
const CLOUD_FAR_M: f32 = 60000.0;
// The longest path marched through the layer (m): near the horizon it would be tens of km.
const CLOUD_PATH_M: f32 = 24000.0;
// Rayleigh scattering of sea-level air (1/m, linear Rec.709), Hillaire 2020's values.
const RAYLEIGH: vec3<f32> = vec3<f32>(5.802e-6, 13.558e-6, 33.1e-6);
// The noise textures' texels (m): clouds.rs BASE and DETAIL over their tiles.
const BASE_TEXEL_M: f32 = CLOUD_BASE_TILE / 128.0;
const DETAIL_TEXEL_M: f32 = CLOUD_DETAIL_TILE / 32.0;
// The most of a cell's cover separate clouds take (`density`).
const FAIR_COVER: f32 = 0.7;
// The eroded base noise's low end (`density`).
const SHAPE_LOW: f32 = 0.6;
// How far the curl noise twists the detail noise (m).
const CURL_M: f32 = 45.0;
// A lightning flash's light spreads through the cloud over about this distance (m).
const FLASH_DIFFUSION_M: f32 = 900.0;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_clouds(@builtin(vertex_index) i: u32) -> Out {
    return Out(fullscreen(i));
}

fn remap(v: f32, lo: f32, hi: f32, new_lo: f32, new_hi: f32) -> f32 {
    return new_lo + (v - lo) / (hi - lo) * (new_hi - new_lo);
}

// The distance along a ray from height `y0` with vertical component `dy` to altitude `h` over
// the curved ground (a parabola about the camera, ample for a few hundred km): the root of
// y0 + dy t + t^2 (1 - dy^2) / 2R = h, in the form that stays accurate for a tiny 1 / R.
fn to_altitude(y0: f32, dy: f32, h: f32) -> f32 {
    let a = (1.0 - dy * dy) / (2.0 * EARTH_R);
    let c = y0 - h;
    let disc = dy * dy - 4.0 * a * c;
    if disc < 0.0 {
        return -1.0;
    }
    return -2.0 * c / (sqrt(disc) + dy);
}

// Altitude of a point over the curved ground, for a camera at `eye`.
fn altitude(p: vec3<f32>, eye: vec3<f32>) -> f32 {
    let d = p.xz - eye.xz;
    return p.y + dot(d, d) / (2.0 * EARTH_R);
}

// The top of a cloud type's column as a share of the layer: stratus fill the bottom fifth,
// cumulus about the lower two thirds, cumulonimbus all of it.
fn type_top(ty: f32) -> f32 {
    return mix(0.22, 1.0, ty);
}

// The density height gradient of a cloud type at height `h` (0 the layer's base, 1 its top):
// flat bottoms, tops that thin out.
fn height_gradient(h: f32, ty: f32) -> f32 {
    let top = type_top(ty);
    return smoothstep(0.0, mix(0.03, 0.08, ty), h) * (1.0 - smoothstep(top * mix(0.35, 0.65, ty), top, h));
}

// The cloud type at a point of the weather map (`w.y` its slow variation).
fn cloud_type(w: vec2<f32>) -> f32 {
    return clamp(g.cloud_vol.w + (w.y - 0.5) * 0.3 * (1.0 - g.clouds.y), 0.0, 1.0);
}

// A sample of the cloud.
struct Sample {
    // Density 0..1.
    d: f32,
    // Height within its type's column, 0 (base) .. 1 (top).
    hn: f32,
}

// Cloud density 0..1 at `p` (world) of altitude `alt`; `lod` log2 of a texel's footprint there
// over the base noise's texel (the noise's level of detail; below 0 where a texel is smaller),
// `detail` 0..1 how much of the detail erosion to take (0: none, as the march towards the sun,
// where it would not show).
fn density(p: vec3<f32>, alt: f32, lod: f32, detail: f32) -> Sample {
    var s = Sample(0.0, 0.0);
    let h = (alt - g.cloud_vol.y) / g.cloud_vol.z;
    if h <= 0.0 || h >= 1.0 {
        return s;
    }
    // Tops lean downwind (the wind's direction, g.wind.xy), cells and billows alike.
    let lean = g.wind.xy * (h * 500.0);
    let xz = p.xz + g.cloud_wind.xy + lean;
    let w = textureSampleLevel(weather_map, repeat_sampler, xz / CLOUD_PERIOD, 0.0);
    let cover_here = cloud_coverage(w.xy, p.xz);
    if cover_here <= 0.004 {
        return s;
    }
    let ty = cloud_type(w.xy);
    let gradient = height_gradient(h, ty);
    if gradient <= 0.0 {
        return s;
    }
    s.hn = clamp(h / type_top(ty), 0.0, 1.0);
    // Separate clouds take at most FAIR_COVER of their cell's cover, so the noise still carves
    // billows out of the middle of a cell, not only its outline; and the cover narrows towards
    // their tops, so only the strongest billows rise that high (domes, not blocks).
    let most = mix(FAIR_COVER, 1.0, g.clouds.y);
    let cover = cover_here * most * mix(1.0, 0.55, smoothstep(0.2, 1.0, s.hn) * (1.0 - g.clouds.y));
    let q = vec3<f32>(xz.x, alt, xz.y);
    let n = textureSampleLevel(base_noise, repeat_sampler, q / CLOUD_BASE_TILE, max(lod, 0.0));
    let fbm = n.g * 0.625 + n.b * 0.25 + n.a * 0.125;
    // The eroded Perlin-Worley keeps to about 0.7..1; spread over 0..1, so the cover cuts
    // into it (less so for a closed layer, which must not open).
    let low = mix(SHAPE_LOW, 0.1, g.clouds.y);
    let billows = clamp(remap(remap(n.r, fbm - 1.0, 1.0, 0.0, 1.0), low, 1.0, 0.0, 1.0), 0.0, 1.0);
    let shape = billows * gradient;
    s.d = clamp(remap(shape, 1.0 - cover, 1.0, 0.0, 1.0), 0.0, 1.0) * cover / most;
    if s.d <= 0.0 || detail <= 0.0 {
        return s;
    }
    // The detail, twisted by the curl noise: most at the base, where the air churns.
    let c = textureSampleLevel(curl_map, repeat_sampler, xz / CLOUD_CURL_TILE, 0.0).rg * 2.0 - 1.0;
    let qd = q + vec3<f32>(c.x, 0.5 * (c.x + c.y), c.y) * (CURL_M * (1.0 - 0.6 * s.hn));
    let detail_lod = max(lod + log2(BASE_TEXEL_M / DETAIL_TEXEL_M), 0.0);
    let e = textureSampleLevel(detail_noise, repeat_sampler, qd / CLOUD_DETAIL_TILE, detail_lod).rgb;
    let hf = e.r * 0.625 + e.g * 0.25 + e.b * 0.125;
    // Billowy (the inverted noise rounds puffs) in the body and at the base, wispy at the top.
    let wisp = smoothstep(0.55, 0.95, s.hn);
    let modifier = mix(1.0 - hf, hf, wisp);
    let erosion = mix(0.6, 0.8, wisp) * detail;
    s.d = clamp(remap(s.d, modifier * erosion, 1.0, 0.0, 1.0), 0.0, 1.0);
    return s;
}

fn hg(c: f32, k: f32) -> f32 {
    let k2 = k * k;
    return (1.0 - k2) / (4.0 * PI * pow(max(1.0 + k2 - 2.0 * k * c, 1e-4), 1.5));
}

// Dual-lobe phase: a forward lobe (silver lining) and a weaker backward one.
fn phase(c: f32, scale: f32) -> f32 {
    return mix(hg(c, 0.8 * scale), hg(c, -0.3 * scale), 0.25);
}

// The optical depth towards the sun from `p`, by steps that grow away from it.
fn sun_depth(p: vec3<f32>, alt: f32, lod: f32) -> f32 {
    let l = g.sun_dir.xyz;
    let steps = i32(cf.more.x);
    var depth = 0.0;
    var t0 = 0.0;
    var t = 30.0;
    for (var i = 0; i < steps; i++) {
        let mid = 0.5 * (t0 + t);
        // The detail only on the first, short steps.
        let s = density(p + l * mid, alt + l.y * mid, lod, select(0.0, 1.0, i == 0));
        depth += s.d * (t - t0);
        t0 = t;
        t *= 2.2;
    }
    // A closed layer goes on past the steps' reach (a few km; a cumulonimbus is 7 km tall):
    // the rest of the way to its top, at a density of a half.
    let rest = max(g.cloud_vol.y + g.cloud_vol.z - (alt + l.y * t0), 0.0) / max(l.y, 0.05);
    depth += g.clouds.y * 0.5 * rest;
    return depth * g.cloud_wind.z * cloud_dense();
}

struct March {
    light: vec3<f32>,
    transmit: f32,
    // The distance the clouds' light comes from, weighted by it (m).
    distance: f32,
}

fn march(dir: vec3<f32>, jitter: f32, pixel: f32) -> March {
    var m = March(vec3<f32>(0.0), 1.0, 0.0);
    let eye = g.camera.xyz;
    let base = g.cloud_vol.y;
    let top = base + g.cloud_vol.z;
    if dir.y < -0.02 || eye.y >= top {
        return m;
    }
    var t0 = 0.0;
    if eye.y < base {
        t0 = to_altitude(eye.y, dir.y, base);
    }
    var t1 = to_altitude(eye.y, dir.y, top);
    if t0 < 0.0 || t1 <= t0 || t0 > CLOUD_FAR_M {
        return m;
    }
    t1 = min(t1, t0 + CLOUD_PATH_M);
    let steps = i32(cf.params.w);
    let dt = (t1 - t0) / f32(steps);
    let sigma = g.cloud_wind.z * cloud_dense();
    let c = dot(dir, g.sun_dir.xyz);
    let sun_up = smoothstep(-0.08, 0.02, g.sun_dir.y);
    let sun = cloud_sun_light() * sun_up;
    let sky = sky_up_sh() / PI;
    let ground = g.ground.rgb * (sky_up_sh() + g.sun_light.rgb / max(g.night.w, 1e-12) * max(g.sun_dir.y, 0.0)) / PI;
    let flash = g.flash.w;
    var weight = 0.0;
    var t = t0 + dt * jitter;
    for (var i = 0; i < steps; i++) {
        let p = eye + dir * t;
        let alt = altitude(p, eye);
        // The footprint of a history texel here, over the base noise's texel; the detail
        // fades out as it falls under a texel.
        let lod = log2(t * pixel / BASE_TEXEL_M);
        let detail = 1.0 - smoothstep(1.5, 3.0, lod);
        let s = density(p, alt, lod, detail);
        if s.d > 0.0 {
            let extinction = s.d * sigma;
            var direct = vec3<f32>(0.0);
            var od = 0.0;
            if sun_up > 0.0 {
                od = sun_depth(p, alt, lod);
                // Multiple scattering: three octaves (Wrenninge 2013), each with half the
                // extinction, 0.6 of the scattering and half the anisotropy of the last.
                var a = 1.0;
                var b = 1.0;
                var k = 1.0;
                var ms = 0.0;
                for (var o = 0; o < 3; o++) {
                    ms += b * exp(-a * od) * phase(c, k);
                    a *= 0.5;
                    b *= 0.6;
                    k *= 0.5;
                }
                // Powder (Schneider 2015): thin cloud seen away from the sun has scattered
                // little of the light towards the eye yet.
                let powder = mix(1.0, 1.0 - exp(-2.0 * extinction * 60.0), 0.5 - 0.5 * c);
                // Diffused through the cloud (g = 0.8): spread evenly, as the flux it carries;
                // through a closed layer by its depth straight down (once diffused, the light
                // has forgotten the slant it came in at).
                let slab = od * mix(1.0, max(g.sun_dir.y, 0.05), g.clouds.y);
                let soaked = max(1.0 / (1.0 + mix(0.3, 0.12, g.clouds.y) * slab) - exp(-od), 0.0);
                direct = sun * (ms * powder + soaked * max(g.sun_dir.y, 0.05) / PI);
            }
            // In-scattering probability (Schneider 2017): light gathers where the cloud is
            // dense and towards its top; its bottom edge is darker. Separate clouds only: a
            // closed layer's base is lit by what soaks through it.
            let depth_p = 0.05 + pow(s.d, remap(clamp(s.hn, 0.3, 0.85), 0.3, 0.85, 0.5, 2.0));
            let vertical_p = pow(clamp(remap(s.hn, 0.07, 0.22, 0.25, 1.0), 0.25, 1.0), 0.8);
            let gather = mix(clamp(depth_p * vertical_p * 1.6, 0.0, 1.2), 1.0, g.clouds.y * 0.7);
            // The sky's light: brighter up the column, less deep in the cloud (the optical depth
            // towards the sun, brought to the vertical, standing in for the depth beneath the
            // cloud's top).
            let open = 0.3 + 0.7 * exp(-0.08 * od * max(g.sun_dir.y, 0.25));
            let h = clamp((alt - base) / g.cloud_vol.z, 0.0, 1.0);
            let ambient = sky * mix(0.25, 1.0, max(h, s.hn * 0.6)) * open * (1.0 - 0.3 * g.clouds.y) + ground * (1.0 - h) * 0.5;
            var lit = direct * gather + ambient * mix(1.0, gather, 0.5);
            if flash > 0.0 {
                let r = distance(p, g.flash.xyz);
                let f = FLASH_DIFFUSION_M;
                lit += vec3<f32>(0.85, 0.88, 1.0) * flash * exp(-r / f) / (1.0 + r * r / (0.25 * f * f));
            }
            let step_t = exp(-extinction * dt);
            m.light += m.transmit * lit * (1.0 - step_t);
            let w = m.transmit * (1.0 - step_t);
            m.distance += w * t;
            weight += w;
            m.transmit *= step_t;
            if m.transmit < 0.01 {
                break;
            }
        }
        t += dt;
    }
    m.distance = select(t0, m.distance / weight, weight > 0.0);
    return m;
}

// The direction through a point of the view at `uv` (0..1 from the top left).
fn view_dir(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    return normalize(far.xyz / far.w - g.camera.xyz);
}

@fragment
fn fs_clouds(in: Out) -> @location(0) vec4<f32> {
    let scale = cf.more.w;
    let size = cf.more.yz * scale;
    // The history texel this march texel samples this frame (its centre).
    let texel = floor(in.clip.xy) * scale + cf.up.xy;
    let uv = (texel + 0.5) / size;
    let dir = view_dir(uv);
    // The angle a history texel spans (rad).
    let pixel = 2.0 / (abs(g.proj[1][1]) * size.y);
    let subs = i32(cf.params.z);
    var light = vec3<f32>(0.0);
    var transmit = 0.0;
    var distance = 0.0;
    // The jitter follows each history texel's own samples: the n-th, one a frame, or one every
    // fourth frame where the history doubles the march (frame 4n + its place in the block). So
    // a texel's samples step along the golden ratio from one to the next, as at half the size;
    // counted by the frame, every fourth step of it would barely move (the noise's turn and
    // the golden ratio over four frames add up to about a whole turn), and a texel would keep
    // the same jitter, and its banding, for dozens of frames.
    let n = floor(cf.params.x / (scale * scale));
    let noise = ign(texel + vec2<f32>(5.588, 5.588) * fract(n / 64.0) * 64.0);
    for (var s = 0; s < subs; s++) {
        let jitter = fract(noise + n * 0.618034 + f32(s) / f32(subs));
        let m = march(dir, jitter, pixel);
        light += m.light;
        transmit += m.transmit;
        distance += m.distance;
    }
    light /= f32(subs);
    transmit /= f32(subs);
    distance /= f32(subs);
    var now = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if transmit < 1.0 {
        // The air in front: the aerosol haze, and the clear air's own extinction (its light is
        // the sky's, which shows through instead), thinning with height over the clouds' path.
        let haze = fog_transmittance(g.camera.y, dir.y, distance);
        let air = exp(-RAYLEIGH * distance * exp(-0.5 * g.cloud_vol.y / 8000.0));
        let through = haze * air;
        let far_fade = 1.0 - smoothstep(0.7, 1.0, distance / CLOUD_FAR_M);
        // Under a closed layer the air in front is lit by the clouds, not by a clear sky behind
        // them: the layer stays to the horizon.
        let seen = mix(through * far_fade, vec3<f32>(1.0), g.clouds.y);
        now = vec4<f32>(light * seen, 1.0 - (1.0 - transmit) * dot(seen, vec3<f32>(1.0 / 3.0)));
    }
    return now;
}

// The resolve: this frame's samples blended into the history, at `cf.more.w` times the
// march's size (2, or 1 where the march is already at half the render size).
@fragment
fn fs_resolve(in: Out) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.clip.xy));
    let low_size = vec2<i32>(cf.more.yz);
    let scale = i32(cf.more.w);
    let size = cf.more.yz * cf.more.w;
    let low = min(p / scale, low_size - 1);
    // Starting over (`cf.up.z` 1), the history is filled from the new samples alone, marched at
    // the centres of the 2x2 blocks (`cf.up.xy` 0.5).
    if cf.up.z >= 1.0 {
        let at = ((vec2<f32>(p) - cf.up.xy) / cf.more.w + 0.5) / cf.more.yz;
        return textureSampleLevel(current, clamp_sampler, at, 0.0);
    }
    let o = vec2<i32>(cf.up.xy);
    // The new samples round this texel (its own and the four next to it): their mean and
    // spread, for the clamp.
    var m1 = vec4<f32>(0.0);
    var m2 = vec4<f32>(0.0);
    let cross = array<vec2<i32>, 5>(vec2<i32>(0, 0), vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));
    for (var k = 0; k < 5; k++) {
        let c = textureLoad(current, clamp(low + cross[k], vec2<i32>(0), low_size - 1), 0);
        m1 += c;
        m2 += c * c;
    }
    let mean = m1 / 5.0;
    let spread = sqrt(max(m2 / 5.0 - mean * mean, vec4<f32>(0.0)));
    // This frame's sample of this texel, or the new samples interpolated to it.
    let own = all(p - low * scale == o);
    var now: vec4<f32>;
    if own {
        now = textureLoad(current, low, 0);
    } else {
        let at = (vec2<f32>(p - o) / cf.more.w + 0.5) / cf.more.yz;
        now = textureSampleLevel(current, clamp_sampler, at, 0.0);
    }
    let uv = (vec2<f32>(p) + 0.5) / size;
    let prev = cf.prev_view_proj * vec4<f32>(view_dir(uv), 0.0);
    if prev.w <= 1e-5 {
        return now;
    }
    let puv = vec2<f32>(prev.x / prev.w * 0.5 + 0.5, 0.5 - prev.y / prev.w * 0.5);
    if any(puv < vec2<f32>(0.0)) || any(puv > vec2<f32>(1.0)) {
        return now;
    }
    let before = textureSampleLevel(history, clamp_sampler, puv, 0.0);
    let k = cf.up.w;
    let kept = clamp(before, mean - k * spread, mean + k * spread);
    return mix(kept, now, select(cf.up.z, cf.params.y, own));
}
