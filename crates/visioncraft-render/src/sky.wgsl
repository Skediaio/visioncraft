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
        return g.ground.rgb + g.night.rgb;
    }
    if source == 1u {
        return sky_view(dir) * g.sky.z + g.night.rgb;
    }
    return preetham(dir) + g.night.rgb;
}

// The background as seen (pre-exposed): the source above the horizon, and
// below it the horizon dimmed (Preetham) or the source as is, never the flat
// ground radiance (the infinite ground plane draws the ground).
fn background(dir: vec3<f32>) -> vec3<f32> {
    let source = u32(g.sky.x + 0.5);
    var c: vec3<f32>;
    if source == 2u {
        c = hdri(dir, 0.0);
    } else if source == 1u {
        let up = vec3<f32>(dir.x, max(dir.y, 0.0), dir.z);
        let below = mix(1.0, 0.35, smoothstep(0.0, -0.15, dir.y));
        c = sky_view(up) * g.sky.z * below + g.night.rgb;
    } else {
        c = preetham(dir) + g.night.rgb;
    }
    return c * g.night.w;
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
    color += tint * g.sun_dir.w * edge * mix(0.6, 1.0, smoothstep(g.sun_light.w, 1.0, cos_sun));
    // Haze to infinity: the horizon (and any sky below it) dissolves.
    if g.fog.x > 0.0 {
        let transmit = fog_transmittance(g.camera.y, dir.y, 1e9);
        color = color * transmit + fog_inscatter(dir, 1.0) * (1.0 - transmit);
    }
    return vec4<f32>(color, 1.0);
}
