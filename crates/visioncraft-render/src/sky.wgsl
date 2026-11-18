// The sky: three sources of radiance (kcd/m2, without exposure), chosen by
// g.sky.x, used for the background, the environment cube, the ambient SH
// and the haze colour.
//
// 0. Preetham, Shirley & Smits, "A Practical Analytic Model for Daylight",
//    SIGGRAPH 1999 (Perez luminance distribution, Appendix A). The
//    coefficients come from sky.rs, which is the same model on the CPU.
// 1. The physical sky of Hillaire, "A Scalable and Production Ready Sky and
//    Atmosphere Rendering Technique", EGSR 2020: atmosphere.wgsl renders its
//    sky-view LUT (sky_tex) whenever the sun moves; this file only looks it up.
// 2. An equirectangular HDRI (sky_tex, mipmapped), rotated about the vertical.

// Perez et al. 1993 all-weather distribution, per channel (Y, x, y).
fn perez(cos_theta: f32, gamma: f32, cos_gamma: f32) -> vec3<f32> {
    let a = g.perez[0].xyz;
    let b = g.perez[1].xyz;
    let c = g.perez[2].xyz;
    let d = g.perez[3].xyz;
    let e = g.perez[4].xyz;
    return (1.0 + a * exp(b / cos_theta)) * (1.0 + c * exp(d * gamma) + e * cos_gamma * cos_gamma);
}

// Preetham sky in kcd/m2 (linear sRGB, y up), without the night floor.
fn preetham(dir: vec3<f32>) -> vec3<f32> {
    // Below the horizon the model is undefined: use the horizon, dimmed.
    let cos_theta = max(dir.y, 0.001);
    let cos_gamma = clamp(dot(normalize(vec3<f32>(dir.x, cos_theta, dir.z)), g.sun_dir.xyz), -1.0, 1.0);
    let yxy = g.zenith.xyz * perez(cos_theta, acos(cos_gamma), cos_gamma);
    // CIE xyY -> XYZ -> linear sRGB (IEC 61966-2-1 matrix, D65).
    let big_y = yxy.x;
    let big_x = yxy.y / yxy.z * big_y;
    let big_z = (1.0 - yxy.y - yxy.z) / yxy.z * big_y;
    let rgb = mat3x3<f32>(
        vec3<f32>(3.2406, -0.9689, 0.0557),
        vec3<f32>(-1.5372, 1.8758, -0.2040),
        vec3<f32>(-0.4986, 0.0415, 1.0570),
    ) * vec3<f32>(big_x, big_y, big_z);
    let below = mix(1.0, 0.35, smoothstep(0.0, -0.15, dir.y));
    return max(rgb, vec3<f32>(0.0)) * below * g.zenith.w;
}

// Sky-view LUT parametrisation (shared with atmosphere.wgsl): u is the
// azimuth from the sun's azimuth, 0..pi (the sky is symmetric about the
// sun's vertical plane); v is the elevation, square-root mapped so texels
// crowd at the horizon where the sky changes fastest (Hillaire 2020, sec. 5.3
// uses the same idea).
fn sky_view_uv(dir: vec3<f32>, sun: vec3<f32>) -> vec2<f32> {
    let e = asin(clamp(dir.y, -1.0, 1.0));
    let v = 0.5 + 0.5 * sign(e) * sqrt(abs(e) / (0.5 * PI));
    let a = vec2<f32>(dir.x, dir.z);
    let b = vec2<f32>(sun.x, sun.z);
    let la = length(a);
    let lb = length(b);
    var u = 0.0;
    if la > 1e-5 && lb > 1e-5 {
        u = acos(clamp(dot(a, b) / (la * lb), -1.0, 1.0)) / PI;
    }
    return vec2<f32>(u, 1.0 - v); // texture v runs down: zenith at the top row
}

// Equirectangular lookup: u is the compass azimuth (0.5 = north, -Z;
// 0.75 = east, +X) plus the rotation, v = 0 at the zenith.
fn equirect_uv(dir: vec3<f32>) -> vec2<f32> {
    let azimuth = atan2(dir.x, -dir.z) - g.sky.y;
    let u = fract(azimuth / (2.0 * PI) + 0.5);
    let v = acos(clamp(dir.y, -1.0, 1.0)) / PI;
    return vec2<f32>(u, v);
}

// The physical sky per unit sun illuminance. The LUT stores parameter 0
// and 1 at the first and last texel centres (the sampler repeats in u for
// the HDRI; this lookup never reaches the edge, so it does not wrap).
fn sky_view(dir: vec3<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(sky_tex));
    let uv = 0.5 / size + sky_view_uv(dir, g.sun_dir.xyz) * (1.0 - 1.0 / size);
    return textureSampleLevel(sky_tex, env_sampler, uv, 0.0).rgb;
}

fn hdri(dir: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(sky_tex, env_sampler, equirect_uv(dir), lod).rgb * g.sky.z;
}

// Sky radiance in kcd/m2 towards `dir`: above the horizon the chosen source
// (with the night floor under the analytic ones), below it the ground
// radiance from the CPU, or the HDRI's own lower half. `lod`: HDRI mip.
fn sky_kcd(dir: vec3<f32>, lod: f32) -> vec3<f32> {
    let source = u32(g.sky.x + 0.5);
    if source == 2u {
        if dir.y < 0.0 && g.ground.w < 0.5 {
            return g.ground.rgb;
        }
        return hdri(dir, lod);
    }
    if dir.y < 0.0 {
        return ground_kcd(sky_up_sh()) + g.night.rgb;
    }
    var c: vec3<f32>;
    if source == 1u {
        c = sky_view(dir) * g.sky.z + g.night.rgb;
    } else {
        c = preetham(dir) + g.night.rgb;
    }
    let cl = cloud(dir);
    return mist_over(mix(c, cl.rgb, cl.a), dir);
}

// The sky as seen from the ground through the mist (for the environment and the ambient light, so
// what lights a garden under a mist is the mist): what the layer lets through of the sky, plus
// the sun's light it took out of the beam on the way down (wet.w is the share it left), sent on
// as diffuse light, most of it downwards and forwards: half spread evenly over the sky, half in
// a Henyey-Greenstein glow round the sun (each part carries about half of it to a level
// surface, so the light on the ground is kept: what leaves the beam arrives as sky). The sky's own light is scattered too but stays about
// as bright on average, so it is left as it is. Rain fog alone takes no sun here (its sky is
// already a closed overcast layer).
fn mist_over(c: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    if g.mist.x <= 0.0 || dir.y <= 0.0 || g.wet.w >= 1.0 {
        return c;
    }
    let share = max(g.wet.w, 1e-3);
    let top = g.sun_light.rgb / max(g.night.w, 1e-12) / share;
    let down = top * max(g.sun_dir.y, 0.0) * (1.0 - share) * 0.8;
    let gg = 0.6;
    let hg = (1.0 - gg * gg) / (4.0 * PI * pow(1.0 + gg * gg - 2.0 * gg * dot(dir, g.sun_dir.xyz), 1.5));
    return c + down * (0.5 / PI + hg);
}

// ---- clouds over the analytic skies -----------------------------------------

fn cloud_noise(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<i32>(1, 0));
    let c = hash2(i + vec2<i32>(0, 1));
    let d = hash2(i + vec2<i32>(1, 1));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn cloud_fbm(p_in: vec2<f32>) -> f32 {
    var p = p_in;
    var v = 0.0;
    var amp = 0.5;
    for (var k = 0; k < 5; k++) {
        v += amp * cloud_noise(p);
        p = vec2<f32>(p.x * 1.6 - p.y * 1.2, p.x * 1.2 + p.y * 1.6) + vec2<f32>(17.3, 9.1);
        amp *= 0.5;
    }
    return v / 0.97;
}

// The cloud layer seen in `dir` (kcd/m2 and coverage): fractal noise on a plane 1.5 km up,
// drifting with the wind. Fair-weather clouds are lit by the sun (brighter where the cloud
// thins towards it, a forward-scattering glow round the sun) and the sky; an overcast layer
// is an even grey of the sun's diffused light. Their share of the sun goes from the direct
// light (lib.rs `Clouds::sun_share`); here it is put back for the light on the clouds.
// The SH's clear-sky pass (env.wgsl `fs_sh`): no clouds.
var<private> clouds_off: bool = false;

fn cloud(dir: vec3<f32>) -> vec4<f32> {
    let cover = g.clouds.x;
    if clouds_off || cover <= 0.0 || dir.y <= 0.0 {
        return vec4<f32>(0.0);
    }
    // g.clouds.y is how overcast the layer is, 0 (fair-weather clouds) to 1 (a closed grey layer);
    // in between (rain thickening the sky, lib.rs `cloud_state`) both are drawn and mixed.
    let o = g.clouds.y;
    let p = (dir.xz / max(dir.y, 0.04) * 1500.0 + g.clouds.zw) * 0.0011;
    let n = cloud_fbm(p);
    var a_over = 0.0;
    var a_fair = 0.0;
    if o > 0.0 {
        a_over = cover * mix(0.9, 1.0, n);
    }
    if o < 1.0 {
        a_fair = smoothstep(1.0 - cover - 0.06, 1.0 - cover + 0.22, n);
    }
    var a = select(a_fair, a_over, o > 0.5);
    if o > 0.0 && o < 1.0 {
        a = mix(a_fair, a_over, o);
    }
    // A rain sky is closed: no blue shows through by 0.7.
    if g.wet.y > 0.0 {
        a = mix(a, 1.0, smoothstep(0.45, 0.7, g.wet.y) * o);
    }
    // Far clouds thin into the haze at the horizon.
    a *= smoothstep(0.02, 0.15, dir.y);
    if a <= 0.0 {
        return vec4<f32>(0.0);
    }
    // In rain the renderer says how much sun gets through (wet.z).
    let share = select(mix(1.0 - 0.15 * cover, 1.0 - 0.85 * cover, o), g.wet.z, g.wet.y > 0.0);
    let sun = g.sun_light.rgb / max(g.night.w, 1e-12) / max(share, 0.05);
    let high = max(g.sun_dir.y, 0.05);
    let sky = sky_up_sh();
    var rad_over = vec3<f32>(0.0);
    var rad_fair = vec3<f32>(0.0);
    if o > 0.0 {
        rad_over = (sun * high * 0.35 + sky * 0.5) / PI * mix(0.8, 1.05, n) * (1.0 + 0.5 * smoothstep(0.1, 0.8, g.wet.y));
    }
    if o < 1.0 {
        let toward = cloud_fbm(p + normalize(g.sun_dir.xz + vec2<f32>(1e-4, 0.0)) * 0.08);
        let lit = clamp(0.6 + (n - toward) * 3.0, 0.3, 1.0);
        let glow = 1.0 + 1.2 * pow(max(dot(dir, g.sun_dir.xyz), 0.0), 12.0);
        rad_fair = (sun * high * lit * glow * 0.85 + sky * 0.6) / PI;
    }
    var rad = select(rad_fair, rad_over, o > 0.5);
    if o > 0.0 && o < 1.0 {
        rad = mix(rad_fair, rad_over, o);
    }
    return vec4<f32>(rad, a);
}

// The ground below the horizon under the analytic skies (kcd/m2): a
// Lambertian plane of albedo g.ground.rgb lit by the sun and by `sky_up`,
// the sky's irradiance on a horizontal surface (kcd/m2 * sr). Taking the
// sky light from the sky that is drawn keeps the two in proportion at every
// sun height: in twilight the ground is lit by the dim sky only, not by a
// daylight model extrapolated below the horizon.
fn ground_kcd(sky_up: vec3<f32>) -> vec3<f32> {
    let sun = g.sun_light.rgb * max(g.sun_dir.y, 0.0) / g.night.w;
    return g.ground.rgb * (sky_up + sun) / PI;
}

// `sky_up_sh` (the sky's irradiance on a horizontal surface) is in common.wgsl.

// The background as seen (pre-exposed): the source above the horizon, and
// below it the horizon dimmed (Preetham) or the source as is, never the flat
// ground radiance (the infinite ground plane draws the ground).
fn background(dir: vec3<f32>) -> vec3<f32> {
    let source = u32(g.sky.x + 0.5);
    var c: vec3<f32>;
    if source == 2u {
        c = hdri(dir, 0.0);
    } else {
        if source == 1u {
            let up = vec3<f32>(dir.x, max(dir.y, 0.0), dir.z);
            let below = mix(1.0, 0.35, smoothstep(0.0, -0.15, dir.y));
            c = sky_view(up) * g.sky.z * below + g.night.rgb;
        } else {
            c = preetham(dir) + g.night.rgb;
        }
        // The haze between the eye and the cloud layer (1.5 km up) hides far clouds in the sky
        // behind them, as aerial perspective does: at a clear day's 30 km visibility a cloud
        // 3 degrees up keeps a tenth of its contrast, one 10 degrees up half.
        let cl = cloud(dir);
        let through = fog_transmittance(g.camera.y, dir.y, max(1500.0 - g.camera.y, 0.0) / max(dir.y, 0.01));
        c = mix(c + stars(dir), cl.rgb, cl.a * through);
    }
    return c * g.night.w;
}

// Stars (kcd/m2) as points of light: one per cell of an equal-area grid over the sky (4096
// cells, so about the 4500 stars a dark sky shows to the naked eye in one hemisphere), at a
// random spot in its cell, of magnitude 6 + 2 log10(u) for a uniform u (the number brighter
// than m grows as 10^(0.5 m), close to the naked-eye count), illuminance 2.54e-6 lux at
// magnitude 0 (Vega), dimmed towards the horizon by 0.25 magnitudes per air mass (a clear
// night's extinction). Each spreads its illuminance over a cone one pixel wide, so it keeps
// its brightness at every resolution and field of view. They are always there: the day sky
// and the exposure hide them as a real sky does, the first ones show in nautical twilight.
// shortcut: the star field does not turn with the sidereal time; fixed to the world axes.
fn stars(dir: vec3<f32>) -> vec3<f32> {
    if dir.y <= 0.0 || u32(g.sky.x + 0.5) == 2u {
        return vec3<f32>(0.0);
    }
    let cells = vec2<f32>(128.0, 32.0);
    let azimuth = atan2(dir.z, dir.x) / (2.0 * PI) + 0.5;
    let uv = vec2<f32>(azimuth, dir.y) * cells;
    // Angular size of one pixel (rad).
    let pixel = 2.0 / (abs(g.proj[1][1]) * g.viewport.y);
    var sum = vec3<f32>(0.0);
    let base = vec2<i32>(floor(uv));
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let cell = base + vec2<i32>(i, j);
            if cell.y < 0 || cell.y >= 32 {
                continue;
            }
            let wrapped = vec2<i32>((cell.x + 128) % 128, cell.y);
            let at = (vec2<f32>(cell) + vec2<f32>(hash2(wrapped), hash2(wrapped + vec2<i32>(311, 97)))) / cells;
            let a = (at.x - 0.5) * 2.0 * PI;
            let y = at.y;
            let r = sqrt(max(1.0 - y * y, 0.0));
            let star = vec3<f32>(r * cos(a), y, r * sin(a));
            let d = acos(clamp(dot(dir, star), -1.0, 1.0)) / pixel;
            if d >= 1.0 {
                continue;
            }
            let u = max(hash2(wrapped + vec2<i32>(17, 513)), 1e-4);
            let magnitude = 6.0 + 2.0 * log(u) / log(10.0) + 0.25 / max(y, 0.03);
            let lux = 2.54e-6 * pow(10.0, -0.4 * magnitude);
            // A cone of radius one pixel holding `lux`: peak 3 E / (pi r^2).
            let peak = 3.0 * lux / (PI * pixel * pixel) * (1.0 - d);
            let tint = mix(vec3<f32>(1.0, 0.82, 0.66), vec3<f32>(0.8, 0.88, 1.0), hash2(wrapped + vec2<i32>(7, 1001)));
            sum += tint * peak;
        }
    }
    return sum * 0.001;
}

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    return SkyOut(fullscreen(i));
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let ndc = vec2<f32>(in.clip.x * g.viewport.z * 2.0 - 1.0, 1.0 - in.clip.y * g.viewport.w * 2.0);
    let p = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    var dir = normalize(p.xyz / p.w - g.camera.xyz);
    if is_ortho() {
        // A plan or elevation: one plain backdrop along the view axis.
        dir = -normalize(vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]));
    }
    var color = background(dir);
    // Sun disc with a soft, slightly limb-darkened edge. Its luminance is the
    // sun's illuminance over the disc's solid angle (clamped on the CPU).
    let cos_sun = dot(dir, g.sun_dir.xyz);
    let edge = smoothstep(g.sun_light.w - 2e-5, g.sun_light.w + 2e-5, cos_sun);
    let tint = g.sun_light.rgb / max(dot(g.sun_light.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-9);
    let behind = select(0.0, cloud(dir).a, u32(g.sky.x + 0.5) != 2u);
    color += tint * g.sun_dir.w * edge * mix(0.6, 1.0, smoothstep(g.sun_light.w, 1.0, cos_sun)) * (1.0 - behind);
    // Haze to infinity below the horizon, and above it over a band that grows with the haze:
    // the sky models already carry clear air's scattering, so on a clear day (0.13 / km) the
    // band is under a degree and the horizon stays a line, while a hazy day (2.5 / km and up)
    // dissolves the horizon over 14 degrees as before. Hazing a clear sky's horizon to the
    // environment's colour put a grey band between a clear sea or land and the sky.
    if g.fog.x > 0.0 {
        let transmit = fog_transmittance(g.camera.y, dir.y, 1e9);
        let hazed = color * transmit + fog_inscatter(dir, 1.0) * (1.0 - transmit);
        let band = 0.25 * clamp(g.fog.x * 1000.0 / 2.5, 0.0, 1.0);
        color = mix(hazed, color, smoothstep(-0.02, band, dir.y));
    }
    // Mist and rain fog over the whole sky: it lies low, so the sky overhead shows through a thin
    // veil while the horizon dissolves into it (the sky models carry none of it).
    if g.mist.x > 0.0 {
        let transmit = exp(-mist_depth(g.camera.y, dir.y, 1e9));
        color = color * transmit + mist_inscatter(dir) * (1.0 - transmit);
    }
    return vec4<f32>(color, 1.0);
}
