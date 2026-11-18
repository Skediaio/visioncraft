// Volumetric clouds: a raymarch through a layer of cloud over the curved ground, at a quarter
// or half of the render size, accumulated over frames (clouds.rs). Module: common.wgsl + this.
//
// The cloud model is the one of Schneider, "The Real-time Volumetric Cloudscapes of Horizon
// Zero Dawn" (SIGGRAPH 2015 Advances) and "Nubis: Authoring Real-Time Volumetric Cloudscapes
// with the Decima Engine" (SIGGRAPH 2017 Advances), from their public slides:
// - a weather map (common.wgsl `cloud_weather`) says where clouds form and of what type;
// - a height gradient per type shapes the column from flat stratus to towering cumulonimbus;
// - a Perlin-Worley noise eroded by a low-frequency Worley fractal gives the billows, the
//   coverage cuts them (remap by 1 - coverage), and a high-frequency Worley fractal erodes
//   the edges (wispy at the bottom, billowy at the top);
// - the tops lean downwind.
// Light, after Hillaire, "Physically Based Sky, Atmosphere and Cloud Rendering in
// Frostbite" (SIGGRAPH 2016 Physically Based Shading course):
// - the sun's light through a short march towards the sun (Beer-Lambert);
// - a dual-lobe Henyey-Greenstein phase (a strong forward lobe for the silver lining and a
//   weak backward one);
// - multiple scattering approximated by octaves of weaker extinction, scattering and
//   anisotropy (Wrenninge et al., "Oz: The Great and Volumetric", SIGGRAPH 2013 Talks);
// - Schneider's powder term for the dark edges of dense cloud seen away from the sun;
// - the sun's light diffused through thick cloud, which the octaves above lose: the diffuse
//   transmittance of a scattering slab in the two-stream approximation, 1 / (1 + 3/4 (1 - g)
//   tau) less its direct part (Bohren, "Multiple scattering of light and some of its
//   observable consequences", Am. J. Phys. 1987), so an overcast base is the grey of the sun's
//   light soaked through it rather than the blue of the sky above;
// - the sky's light from its SH as ambient, brighter towards the tops and shut out by a
//   closed layer, plus the ground's;
// - the energy-conserving integration of each step's in-scattering (Hillaire 2015).
// Rays start at an interleaved-gradient jitter turned by the frame, and the result is blended
// into the last frames' result, reprojected by direction (the clouds are far enough that the
// camera's position does not matter), so few steps a frame converge to a smooth image.
// What leaves the pass: rgb the clouds' light hazed by the air in front of them (the aerosol
// haze, and the clear air's Rayleigh extinction towards the colour of the sky behind), a the
// share of the sky behind left (sky.wgsl `background`).

struct CloudFrame {
    // The last frame's view-projection without its translation: directions to its pixels.
    prev_view_proj: mat4x4<f32>,
    // x frame counter (jitter), y weight of this frame against the history (1: start over),
    // z sub-samples per pixel, w march steps.
    params: vec4<f32>,
    // x steps towards the sun, yz this image's size (texels).
    more: vec4<f32>,
}

@group(1) @binding(0) var<uniform> cf: CloudFrame;
@group(1) @binding(1) var base_noise: texture_3d<f32>;
@group(1) @binding(2) var detail_noise: texture_3d<f32>;
@group(1) @binding(3) var weather_map: texture_2d<f32>;
@group(1) @binding(4) var repeat_sampler: sampler;
@group(1) @binding(5) var history: texture_2d<f32>;
@group(1) @binding(6) var clamp_sampler: sampler;

const EARTH_R: f32 = 6360000.0;
// The furthest cloud drawn (m along the ray to the layer's base).
const CLOUD_FAR_M: f32 = 60000.0;
// The longest path marched through the layer (m): near the horizon it would be tens of km.
const CLOUD_PATH_M: f32 = 24000.0;
// Rayleigh scattering of sea-level air (1/m, linear Rec.709), Hillaire 2020's values.
const RAYLEIGH: vec3<f32> = vec3<f32>(5.802e-6, 13.558e-6, 33.1e-6);

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

// The density height gradient of a cloud type at height `h` (0 the layer's base, 1 its top):
// stratus fill the bottom quarter, cumulus about the lower 60 %, cumulonimbus all of it;
// rounded bottoms, tops that thin out.
fn height_gradient(h: f32, ty: f32) -> f32 {
    let top = mix(0.22, 1.0, ty);
    return smoothstep(0.0, mix(0.03, 0.08, ty), h) * (1.0 - smoothstep(top * mix(0.35, 0.65, ty), top, h));
}

// Cloud density 0..1 at `p` (world) of altitude `alt`; `cheap` leaves out the detail erosion
// (the march towards the sun, where it would not show).
fn density(p: vec3<f32>, alt: f32, cheap: bool) -> f32 {
    let h = (alt - g.cloud_vol.y) / g.cloud_vol.z;
    if h <= 0.0 || h >= 1.0 {
        return 0.0;
    }
    let xz = p.xz + g.cloud_wind.xy;
    let w = textureSampleLevel(weather_map, repeat_sampler, xz / CLOUD_PERIOD, 0.0).rg;
    let cover = cloud_coverage(w, p.xz);
    if cover <= 0.004 {
        return 0.0;
    }
    let ty = clamp(g.cloud_vol.w + (w.y - 0.5) * 0.3 * (1.0 - g.clouds.y), 0.0, 1.0);
    let gradient = height_gradient(h, ty);
    if gradient <= 0.0 {
        return 0.0;
    }
    // Tops lean downwind (the wind's direction, g.wind.xy).
    let q = vec3<f32>(xz.x, alt, xz.y) + vec3<f32>(g.wind.x, 0.0, g.wind.y) * (h * 500.0);
    let n = textureSampleLevel(base_noise, repeat_sampler, q / CLOUD_BASE_TILE, 0.0);
    let fbm = n.g * 0.625 + n.b * 0.25 + n.a * 0.125;
    let shape = remap(n.r, fbm - 1.0, 1.0, 0.0, 1.0) * gradient;
    var d = clamp(remap(shape, 1.0 - cover, 1.0, 0.0, 1.0), 0.0, 1.0) * cover;
    if d <= 0.0 || cheap {
        return d;
    }
    let e = textureSampleLevel(detail_noise, repeat_sampler, q / CLOUD_DETAIL_TILE, 0.0).rgb;
    let hf = e.r * 0.625 + e.g * 0.25 + e.b * 0.125;
    // Wispy (eroded by the inverted noise) at the bottom, billowy at the top.
    let modifier = mix(hf, 1.0 - hf, clamp(h * 8.0, 0.0, 1.0));
    return clamp(remap(d, modifier * 0.45, 1.0, 0.0, 1.0), 0.0, 1.0);
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
fn sun_depth(p: vec3<f32>, alt: f32) -> f32 {
    let l = g.sun_dir.xyz;
    let steps = i32(cf.more.x);
    var depth = 0.0;
    var t0 = 0.0;
    var t = 30.0;
    for (var i = 0; i < steps; i++) {
        let mid = 0.5 * (t0 + t);
        let d = density(p + l * mid, alt + l.y * mid, i >= 2);
        depth += d * (t - t0);
        t0 = t;
        t *= 2.2;
    }
    return depth * g.cloud_wind.z;
}

struct March {
    light: vec3<f32>,
    transmit: f32,
    // The distance the clouds' light comes from, weighted by it (m).
    distance: f32,
}

fn march(dir: vec3<f32>, jitter: f32) -> March {
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
    let sigma = g.cloud_wind.z;
    let c = dot(dir, g.sun_dir.xyz);
    let sun_up = smoothstep(-0.08, 0.02, g.sun_dir.y);
    // The sun's light over the clouds (kcd/m2 * sr per unit phase): the light on the ground
    // with the clouds', the rain's and the mist's share taken back out, and the haze's and the
    // clear air's below the clouds' base given back, along the sun's air mass (Kasten & Young,
    // "Revised optical air mass tables and approximation formula", Applied Optics 1989): a low
    // sun lights the clouds far more than the garden under them, which is why they glow at
    // sunrise and sunset (capped at e^4).
    let mu = max(g.sun_dir.y, 0.0);
    let air_mass = 1.0 / (mu + 0.50572 * pow(degrees(asin(mu)) + 6.07995, -1.6364));
    var haze_below = 0.0;
    if g.fog.x > 0.0 {
        haze_below = g.fog.x / g.fog.y * (1.0 - exp(-g.fog.y * max(base - g.fog.z, 0.0)));
    }
    let air_below = RAYLEIGH * 8000.0 * (1.0 - exp(-base / 8000.0));
    let lift = exp(min((vec3<f32>(haze_below) + air_below) * air_mass, vec3<f32>(4.0)));
    let sun = g.sun_light.rgb / max(g.night.w, 1e-12) / max(g.wet.z, 0.02) * sun_up * lift;
    let sky = sky_up_sh() / PI;
    let ground = g.ground.rgb * (sky_up_sh() + g.sun_light.rgb / max(g.night.w, 1e-12) * max(g.sun_dir.y, 0.0)) / PI;
    var weight = 0.0;
    var t = t0 + dt * jitter;
    for (var i = 0; i < steps; i++) {
        let p = eye + dir * t;
        let alt = altitude(p, eye);
        let d = density(p, alt, false);
        if d > 0.0 {
            let h = clamp((alt - base) / g.cloud_vol.z, 0.0, 1.0);
            let extinction = d * sigma;
            var direct = vec3<f32>(0.0);
            if sun_up > 0.0 {
                let od = sun_depth(p, alt);
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
                // Diffused through the cloud (g = 0.8): spread evenly, as the flux it carries.
                let soaked = max(1.0 / (1.0 + mix(0.3, 0.12, g.clouds.y) * od) - exp(-od), 0.0);
                direct = sun * (ms * powder + soaked * max(g.sun_dir.y, 0.05) / PI);
            }
            let ambient = sky * mix(0.25, 1.0, h) * (1.0 - 0.3 * g.clouds.y) + ground * (1.0 - h) * 0.5;
            let step_t = exp(-extinction * dt);
            let lit = (direct + ambient) * (1.0 - step_t);
            m.light += m.transmit * lit;
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

@fragment
fn fs_clouds(in: Out) -> @location(0) vec4<f32> {
    let size = cf.more.yz;
    let uv = in.clip.xy / size;
    // The direction through this texel's centre, at the same fraction of the render size.
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    let dir = normalize(far.xyz / far.w - g.camera.xyz);
    let subs = i32(cf.params.z);
    var light = vec3<f32>(0.0);
    var transmit = 0.0;
    var distance = 0.0;
    let noise = ign(in.clip.xy + vec2<f32>(5.588, 5.588) * fract(cf.params.x / 64.0) * 64.0);
    for (var s = 0; s < subs; s++) {
        let jitter = fract(noise + cf.params.x * 0.618034 + f32(s) / f32(subs));
        let m = march(dir, jitter);
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
    let weight = cf.params.y;
    if weight >= 1.0 {
        return now;
    }
    let prev = cf.prev_view_proj * vec4<f32>(dir, 0.0);
    if prev.w <= 1e-5 {
        return now;
    }
    let puv = vec2<f32>(prev.x / prev.w * 0.5 + 0.5, 0.5 - prev.y / prev.w * 0.5);
    if any(puv < vec2<f32>(0.0)) || any(puv > vec2<f32>(1.0)) {
        return now;
    }
    let before = textureSampleLevel(history, clamp_sampler, puv, 0.0);
    return mix(before, now, weight);
}
