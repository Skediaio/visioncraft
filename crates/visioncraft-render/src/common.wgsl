// Shared declarations. lib.rs prepends this file to every shader module (WGSL
// has no includes); keep `Globals` in sync with `Renderer::write_globals`.
// Everything photometric is pre-exposed on the CPU (Frostbite's
// pre-exposure, Lagarde & de Rousiers, "Moving Frostbite to PBR", 2014), so
// the HDR target stays well inside f16 range.
struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cascades: array<mat4x4<f32>, 4>,
    camera: vec4<f32>,          // xyz world position
    sun_dir: vec4<f32>,         // xyz towards the sun, w disc luminance (pre-exposed)
    sun_light: vec4<f32>,       // rgb sun illuminance (pre-exposed), w cos(disc radius)
    sh: array<vec4<f32>, 9>,    // sky irradiance as L2 spherical harmonics (pre-exposed)
    splits: vec4<f32>,          // far view depth of each cascade
    texel: vec4<f32>,           // world size of one shadow texel per cascade
    shadow: vec4<f32>,          // x cascade count, y map size, z 1 / size, w shadow distance
    viewport: vec4<f32>,        // width, height, 1 / width, 1 / height
    effects: vec4<f32>,         // x AO samples, y contact shadows on, z AO radius (m), w AO intensity
    perez: array<vec4<f32>, 5>, // Preetham's Perez coefficients A..E for (Y, x, y)
    zenith: vec4<f32>,          // zenith (Y, x, y) / F(0, theta_sun); w day factor
    night: vec4<f32>,           // rgb night fallback sky (pre-exposed); w Y scale (cd/m2 * exposure)
    highlight: vec4<u32>,       // x mesh id, y face id, z first on-top overlay segment
}
@group(0) @binding(0) var<uniform> g: Globals;

const PI: f32 = 3.14159265;
const SKY_DEPTH: f32 = 1e9; // linear depth written where nothing was drawn

// Pixel -> view-space position at linear depth `d` (metres along -Z). Valid for
// any perspective projection (also off-centre); orthographic cameras are not
// supported by the screen-space passes.
fn view_position(pixel: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x * g.viewport.z * 2.0 - 1.0, 1.0 - pixel.y * g.viewport.w * 2.0);
    let x = d * (ndc.x + g.proj[2][0]) / g.proj[0][0];
    let y = d * (ndc.y + g.proj[2][1]) / g.proj[1][1];
    return vec3<f32>(x, y, -d);
}

// View-space position -> pixel (top-left origin).
fn view_to_pixel(p: vec3<f32>) -> vec2<f32> {
    let c = g.proj * vec4<f32>(p, 1.0);
    let ndc = c.xy / c.w;
    return vec2<f32>((ndc.x * 0.5 + 0.5) * g.viewport.x, (0.5 - ndc.y * 0.5) * g.viewport.y);
}

// Interleaved gradient noise: Jimenez, "Next Generation Post Processing in
// Call of Duty: Advanced Warfare", SIGGRAPH 2014.
fn ign(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

// Fullscreen triangle; z = 1 puts it on the far plane (the sky uses that).
fn fullscreen(i: u32) -> vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 1.0, 1.0);
}
