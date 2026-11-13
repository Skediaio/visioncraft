// Shared declarations. lib.rs prepends this file to every shader module but
// the atmosphere LUTs (WGSL has no includes); keep `Globals` in sync with
// `Renderer::write_globals`.
// Everything photometric is pre-exposed on the CPU (Frostbite's
// pre-exposure, Lagarde & de Rousiers, "Moving Frostbite to PBR", 2014), so
// the HDR target stays well inside f16 range. Sky and environment radiance
// is kept in kcd/m2 without exposure and scaled by `night.w` where it is
// used, so a new exposure never re-renders the environment.
struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cascades: array<mat4x4<f32>, 4>,
    camera: vec4<f32>,          // xyz world position
    sun_dir: vec4<f32>,         // xyz towards the sun, w disc luminance (pre-exposed)
    sun_light: vec4<f32>,       // rgb sun illuminance (pre-exposed), w cos(disc radius)
    splits: vec4<f32>,          // far view depth of each cascade
    texel: vec4<f32>,           // world size of one shadow texel per cascade
    shadow: vec4<f32>,          // x cascade count, y map size, z 1 / size, w shadow distance
    viewport: vec4<f32>,        // render width, height, 1 / width, 1 / height (scaled)
    effects: vec4<f32>,         // x AO samples, y contact shadows on, z AO radius (m), w AO intensity
    perez: array<vec4<f32>, 5>, // Preetham's Perez coefficients A..E for (Y, x, y)
    zenith: vec4<f32>,          // zenith (Y, x, y) / F(0, theta_sun); w day factor
    night: vec4<f32>,           // rgb night sky (kcd/m2); w kcd/m2 -> pre-exposed HDR (1000 * exposure)
    highlight: vec4<u32>,       // x mesh id, y face id, z first on-top overlay segment
    wind: vec4<f32>,            // xy direction (world x, z; unit), z strength, w gustiness
    time: vec4<f32>,            // x seconds, y exposure, z env cube's last mip, w clay mode (0/1)
    light_grid: vec4<f32>,      // xy grid origin (world x, z), z 1 / cell size, w light count
    grid_dims: vec4<u32>,       // x, y cells along world x and z
    output: vec4<f32>,          // output width, height, render scale, 1 / render scale
    fog: vec4<f32>,             // x extinction at fog.z (1/m), y height falloff (1/m), z base height (m), w sun glow
    sky: vec4<f32>,             // x source (0 Preetham, 1 physical, 2 HDRI), y HDRI rotation (rad), z source -> kcd/m2, w HDRI mip count
    ground: vec4<f32>,          // rgb radiance below the horizon (kcd/m2), w HDRI below the horizon (0/1)
    options: vec4<f32>,         // x PCF taps (4 or 9), y bloom strength, z cascades refreshed this frame (bits), w unused
    reflection: vec4<f32>,      // x planar reflection available (0/1), y water height (m), z clip below (0/1), w clip height (m)
    sh: array<vec4<f32>, 9>,    // sky irradiance as L2 SH (kcd/m2 * sr), written on the GPU (env.wgsl)
}
@group(0) @binding(0) var<uniform> g: Globals;
// The prefiltered environment cube and the sky source: the HDRI
// (equirectangular, mipmapped) or the physical sky's sky-view LUT.
@group(0) @binding(1) var env_map: texture_cube<f32>;
@group(0) @binding(2) var env_sampler: sampler;
@group(0) @binding(5) var sky_tex: texture_2d<f32>;

const PI: f32 = 3.14159265;
const SKY_DEPTH: f32 = 1e9; // linear depth written where nothing was drawn

// An orthographic projection has no perspective divide (its w row is
// (0, 0, 0, 1)); plan and elevation views use one.
fn is_ortho() -> bool {
    return abs(g.proj[2][3]) < 1e-7;
}

// From a world position towards the eye: the eye itself for a perspective
// camera, straight back along the view axis (to the view plane) for an
// orthographic one, so view vectors, distances and paths stay meaningful.
fn to_eye(world: vec3<f32>) -> vec3<f32> {
    if is_ortho() {
        let back = normalize(vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]));
        return back * max(-(g.view * vec4<f32>(world, 1.0)).z, 1e-3);
    }
    return g.camera.xyz - world;
}

// Pixel -> view-space position at linear depth `d` (metres along -Z), for
// any perspective (also off-centre) or orthographic projection.
fn view_position(pixel: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x * g.viewport.z * 2.0 - 1.0, 1.0 - pixel.y * g.viewport.w * 2.0);
    if is_ortho() {
        return vec3<f32>((ndc.x - g.proj[3][0]) / g.proj[0][0], (ndc.y - g.proj[3][1]) / g.proj[1][1], -d);
    }
    let x = d * (ndc.x + g.proj[2][0]) / g.proj[0][0];
    let y = d * (ndc.y + g.proj[2][1]) / g.proj[1][1];
    return vec3<f32>(x, y, -d);
}

// Screen pixels per metre at view depth `d` (horizontally).
fn pixels_per_metre(d: f32) -> f32 {
    return g.proj[0][0] * 0.5 * g.viewport.x / select(d, 1.0, is_ortho());
}

// Linear view depth (metres) from a depth-buffer value, any projection:
// solves d = (P22 z + P32) / (P23 z + P33) for z (P[column][row]).
fn linear_depth_of(d: f32) -> f32 {
    return -(g.proj[3][2] - d * g.proj[3][3]) / (d * g.proj[2][3] - g.proj[2][2]);
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

// 32-bit integer hash (Jarzynski & Olano, "Hash Functions for GPU Rendering",
// JCGT 2020: the PCG variant), mapped to [0, 1).
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash3(p: vec3<i32>) -> f32 {
    let h = pcg(u32(p.x) + pcg(u32(p.y) + pcg(u32(p.z))));
    return f32(h >> 8u) / 16777216.0;
}

fn hash2(p: vec2<i32>) -> f32 {
    return hash3(vec3<i32>(p, 0));
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// ---- distance haze ----------------------------------------------------------

// Exponential height fog: extinction sigma(y) = a * exp(-b (y - y0)). Its
// optical depth along a ray has a closed form (Wenzel, "Real-time
// Atmospheric Effects in Games Revisited", GDC 2007; Quilez, "Better Fog",
// 2010): tau = a e^{-b (o_y - y0)} (1 - e^{-b t d_y}) / (b d_y). lib.rs has
// the same function (`fog_transmittance`), unit-tested against numerical
// integration. The sky passes a very large `t` (1e9 m).
fn fog_transmittance(origin_y: f32, dir_y: f32, t: f32) -> f32 {
    let a = g.fog.x;
    if a <= 0.0 {
        return 1.0;
    }
    let b = g.fog.y;
    let start = a * exp(-b * (origin_y - g.fog.z));
    let k = b * dir_y;
    var path = t;
    if abs(k * t) > 1e-4 {
        // (1 - e^{-k t}) / k; the exponent is clamped so it never overflows.
        path = (1.0 - exp(min(-k * t, 80.0))) / k;
    }
    return exp(-start * path);
}

// Light scattered into the view ray by the haze, per unit (1 - T): the sky
// just above the horizon in the view's azimuth from the environment cube (a
// mildly blurred mip whose lobe stays clear of the ground: the direction is
// lifted to at least 7 degrees, so the haze never takes the colour of the
// ground; above that it is the true direction, so a bright feature of the
// sky never shows up displaced in the haze), plus single scattering of the sun with a Henyey-Greenstein phase
// (g = 0.6, forward-scattering aerosol).
fn fog_inscatter(dir: vec3<f32>, sun_visibility: f32) -> vec3<f32> {
    let h = normalize(vec3<f32>(dir.x, max(dir.y, 0.12), dir.z));
    let sky = textureSampleLevel(env_map, env_sampler, h, g.time.z * 0.3).rgb * g.night.w;
    let c = dot(dir, g.sun_dir.xyz);
    let hg = 0.64 / (4.0 * PI * pow(1.36 - 1.2 * c, 1.5));
    return sky + g.sun_light.rgb * hg * g.fog.w * sun_visibility;
}

// Haze between the camera and a surface at `world` (pre-exposed colour in
// and out).
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    if g.fog.x <= 0.0 {
        return color;
    }
    let to = -to_eye(world);
    let t = length(to);
    let dir = to / max(t, 1e-6);
    let transmit = fog_transmittance(g.camera.y, dir.y, t);
    return color * transmit + fog_inscatter(dir, 1.0) * fog_near(transmit) * (1.0 - transmit);
}

// Haze over a short path is lit by the whole sphere around it, darker
// ground included, not only by the horizon sky it converges to over a long
// path: a crude single-scattering correction (about half the horizon's
// radiance for thin haze, all of it as the path becomes opaque) that keeps
// dark hedges at dusk from turning milky.
fn fog_near(transmit: f32) -> f32 {
    return mix(1.0, 0.5, transmit);
}
