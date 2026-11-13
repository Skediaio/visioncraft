// Sky background: Preetham, Shirley & Smits, "A Practical Analytic Model for
// Daylight", SIGGRAPH 1999 (Perez luminance distribution, Appendix A). The
// coefficients come from sky.rs, which is the same model on the CPU (it also
// feeds the ambient light), so background and ambient agree.

// Perez et al. 1993 all-weather distribution, per channel (Y, x, y).
fn perez(cos_theta: f32, gamma: f32, cos_gamma: f32) -> vec3<f32> {
    let a = g.perez[0].xyz;
    let b = g.perez[1].xyz;
    let c = g.perez[2].xyz;
    let d = g.perez[3].xyz;
    let e = g.perez[4].xyz;
    return (1.0 + a * exp(b / cos_theta)) * (1.0 + c * exp(d * gamma) + e * cos_gamma * cos_gamma);
}

// Pre-exposed linear sRGB radiance of the sky in direction `dir` (y up).
fn sky_radiance(dir: vec3<f32>) -> vec3<f32> {
    // Below the horizon the model is undefined: use the horizon, dimmed.
    let cos_theta = max(dir.y, 0.001);
    let cos_gamma = clamp(dot(normalize(vec3<f32>(dir.x, cos_theta, dir.z)), g.sun_dir.xyz), -1.0, 1.0);
    let yxy = g.zenith.xyz * perez(cos_theta, acos(cos_gamma), cos_gamma);
    // CIE xyY -> XYZ -> linear sRGB (IEC 61966-2-1 matrix, D65).
    let big_y = yxy.x * g.night.w;
    let big_x = yxy.y / yxy.z * big_y;
    let big_z = (1.0 - yxy.y - yxy.z) / yxy.z * big_y;
    let rgb = mat3x3<f32>(
        vec3<f32>(3.2406, -0.9689, 0.0557),
        vec3<f32>(-1.5372, 1.8758, -0.2040),
        vec3<f32>(-0.4986, 0.0415, 1.0570),
    ) * vec3<f32>(big_x, big_y, big_z);
    let below = mix(1.0, 0.35, smoothstep(0.0, -0.15, dir.y));
    return mix(g.night.rgb, max(rgb, vec3<f32>(0.0)) * below, g.zenith.w);
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
    let dir = normalize(p.xyz / p.w - g.camera.xyz);
    var color = sky_radiance(dir);
    // Sun disc with a soft, slightly limb-darkened edge. Its luminance is the
    // sun's illuminance over the disc's solid angle (clamped on the CPU).
    let cos_sun = dot(dir, g.sun_dir.xyz);
    let edge = smoothstep(g.sun_light.w - 2e-5, g.sun_light.w + 2e-5, cos_sun);
    let tint = g.sun_light.rgb / max(dot(g.sun_light.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-9);
    color += tint * g.sun_dir.w * edge * mix(0.6, 1.0, smoothstep(g.sun_light.w, 1.0, cos_sun));
    return vec4<f32>(color, 1.0);
}
