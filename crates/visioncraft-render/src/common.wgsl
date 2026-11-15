// Shared declarations. lib.rs prepends this file to every shader module but
// the atmosphere LUTs (WGSL has no includes); keep `Globals` in sync with
// `Renderer::write_globals`.
// Everything photometric is pre-exposed on the CPU (Frostbite's
// pre-exposure, Lagarde & de Rousiers, "Moving Frostbite to PBR", 2014), so
// the HDR target stays well inside f16 range. Sky and environment radiance
// is kept in kcd/m2 without exposure and scaled by `night.w` where it is
// used, so a new exposure never re-renders the environment.

// The shaders of MEDIUM and LOW (lib.rs `Quality::fast_shading` compiles them with this line
// set to true): code only the slower presets run is left out.
const FAST_SHADING: bool = false;

// What a pipeline draws of the endless ground (lib.rs `ground_pipeline`): 0 all of it, 1 the
// cells near the sky volume, 2 the others, which the volume leaves in the open sky: their
// shaders are compiled without its lookup, which costs Intel's compiler its registers.
override VOLUME_PART: u32 = 0u;

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cascades: array<mat4x4<f32>, 4>,
    camera: vec4<f32>,          // xyz world position, w haze-free distance (m)
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
    ground: vec4<f32>,          // rgb: HDRI radiance below the horizon (kcd/m2), analytic skies the ground albedo (sky.wgsl ground_kcd); w HDRI below the horizon (0/1)
    options: vec4<f32>,         // x PCF taps (4 or 9), y bloom strength, z AO target divisor (1 full, 2 half), w ground height
    reflection: vec4<f32>,      // x planar reflection available (0/1), y water height (m), z clip below (0/1), w clip height (m)
    grade: vec4<f32>,           // x look contrast power, y look saturation (1, 1: plain AgX), z camera indoors (0/1), w photo sample seed
    clouds: vec4<f32>,          // x cover 0..1, y overcast (0/1), zw drift (m)
    snow_cover: mat4x4<f32>,    // world -> the snow cover map (straight down, last shadow layer)
    snow: vec4<f32>,            // x snow 0..1, y the cover map's shadow layer, z cover map on (0/1)
    sky_vol: vec4<f32>,         // sky visibility volume: xyz origin (m), w 1 / cell (m)
    sky_vol_size: vec4<f32>,    // xyz 1 / cells, w volume on (0/1)
    coast_info: vec4<f32>,      // x coast points (0: none), y sea level (m), z endless ground on the sea side (m), w surroundings kind (scene.wgsl)
    coast: array<vec4<f32>, 8>, // the coastline, two [x, z] points per entry, the sea to its right
    surround: vec4<f32>,        // the plot's rectangle (min x, min z, max x, max z) for the surroundings (kind in coast_info.w)
    ssr: vec4<f32>,             // screen-space reflections: x on (0/1), y step (px), z most steps, w thickness (m)
    sh: array<vec4<f32>, 9>,    // sky irradiance as L2 SH (kcd/m2 * sr), written on the GPU (env.wgsl)
}
@group(0) @binding(0) var<uniform> g: Globals;
// The prefiltered environment cube and the sky source: the HDRI
// (equirectangular, mipmapped) or the physical sky's sky-view LUT.
@group(0) @binding(1) var env_map: texture_cube<f32>;
@group(0) @binding(2) var env_sampler: sampler;
@group(0) @binding(5) var sky_tex: texture_2d<f32>;
// The sky's visibility round the buildings (lib.rs `set_sky_volume`): r the visible fraction of
// directions, gba the mean visible direction as 0.5 + 2 a; mipmapped.
@group(0) @binding(6) var sky_vis: texture_3d<f32>;
// Its room light: rgb the lamps' and the sun's light passed round the rooms (lux), a the sky's
// (per unit of its mean illuminance), or -1 minus its neighbours' inside a wall or a thin gap;
// one level (sky_volume.rs).
@group(0) @binding(7) var room_vis: texture_3d<f32>;
// Its occluders' voxels, 0.2 m from the same origin: r 1 where solid.
@group(0) @binding(8) var occupancy: texture_3d<f32>;
// Voxels per metre (sky_volume.rs `VOXEL_M` is 0.2 m); a cell is 1 / g.sky_vol.w metres.
const VOXELS_PER_M: f32 = 5.0;

// The sky volume (`sky`: visible fraction a0, mean visible direction a) and its room light
// (`room`, see `room_vis`) for a surface at `world` facing `n`: the eight samples round the
// point half a cell in front of the surface, weighted trilinearly but leaving out those behind
// the surface, those inside a wall (room light's a < 0) and those a wall parts from the point
// (the occluders' voxels, `occupancy`), so a wall does not take the light of the room on its
// other side, nor a floor the void under it (after the backface and visibility weights of Majercik et al., "Dynamic Diffuse Global Illumination with
// Ray-Traced Irradiance Fields", JCGT 8(2), 2019).
// Open sky and no room light outside the volume; none of either where only walls are round.
struct Volume {
    sky: vec4<f32>,
    room: vec4<f32>,
    // The weight of the valid samples it was made of (0: only samples inside walls).
    valid: f32,
}

fn volume_at(world: vec3<f32>, n: vec3<f32>) -> Volume {
    // Half a cell out, and where only samples inside walls or thin gaps are round the point
    // (under a vanity, in a corner), a cell further. A loop the compiler cannot unroll (its count
    // is a uniform's; without a volume the first lookup answers): written out, the lookup was
    // compiled twice into every shading entry point.
    var near = Volume(vec4<f32>(1.0, 0.0, 0.0, 0.0), vec4<f32>(0.0), 1.0);
    let lifts = select(1u, 2u, g.sky_vol_size.w >= 0.5);
    for (var k = 0u; k < lifts; k++) {
        let v = volume_lifted(world, n, 0.5 + f32(k));
        if k == 0u {
            near = v;
        }
        if v.valid > 1e-4 {
            return v;
        }
    }
    return near;
}

fn volume_lifted(world: vec3<f32>, n: vec3<f32>, lift: f32) -> Volume {
    let open = Volume(vec4<f32>(1.0, 0.0, 0.0, 0.0), vec4<f32>(0.0), 1.0);
    if VOLUME_PART == 2u || g.sky_vol_size.w < 0.5 {
        return open;
    }
    let dims = vec3<i32>(textureDimensions(room_vis));
    let surface = (world - g.sky_vol.xyz) * g.sky_vol.w;
    let p = surface + n * lift;
    if any(p < vec3<f32>(0.0)) || any(p > vec3<f32>(dims - 1)) {
        return open;
    }
    let base = vec3<i32>(floor(p));
    let f = p - floor(p);
    let vdims = vec3<i32>(textureDimensions(occupancy)) - 1;
    var sky = vec4<f32>(0.0);
    var room = vec4<f32>(0.0);
    var total = 0.0;
    var valid_weight = 0.0;
    for (var c = 0; c < 8; c++) {
        let o = vec3<i32>(c & 1, (c >> 1) & 1, c >> 2);
        let at = min(base + o, dims - 1);
        var r = textureLoad(room_vis, at, 0);
        // Inside a wall or a thin gap: the mean of the samples round it, only as a last resort.
        let valid = r.a >= 0.0;
        r.a = select(-1.0 - r.a, r.a, valid);
        // Behind the surface, or a wall between the point and the sample (its voxels at a
        // quarter, half and three quarters of the way): only as a last resort.
        var seen = valid && dot(vec3<f32>(at) - surface, n) > 0.05;
        for (var s = 1; s < 4 && seen; s++) {
            let q = mix(p, vec3<f32>(at), f32(s) * 0.25) * (VOXELS_PER_M / g.sky_vol.w);
            seen = textureLoad(occupancy, clamp(vec3<i32>(floor(q)), vec3<i32>(0), vdims), 0).r < 0.5;
        }
        let tri = select(1.0 - f, f, o == vec3<i32>(1));
        let w = tri.x * tri.y * tri.z * select(select(1e-5, 1e-3, valid), 1.0, seen) + 1e-7;
        let t = textureLoad(sky_vis, at, 0);
        sky += w * vec4<f32>(t.r, (t.gba - 0.5) * 0.5);
        room += w * r;
        total += w;
        valid_weight += select(0.0, w, valid);
    }
    if total <= 0.0 {
        return Volume(vec4<f32>(0.0), vec4<f32>(0.0), 0.0);
    }
    return Volume(sky / total, room / total, valid_weight);
}

// Whether a sharp reflection from `world` along `r` leaves the volume's occluders (1) or meets
// one within the volume (0): a march through their voxels (`occupancy`) in steps of a voxel, a
// mirror's own answer where the L1 visibility is only a broad lobe's. 1 without a volume.
fn ray_open(world: vec3<f32>, r: vec3<f32>) -> f32 {
    if g.sky_vol_size.w < 0.5 {
        return 1.0;
    }
    let dims = vec3<f32>(textureDimensions(occupancy));
    let start = (world - g.sky_vol.xyz) * VOXELS_PER_M + r * 0.75;
    for (var i = 0; i < 64; i++) {
        let q = start + r * f32(i);
        if any(q < vec3<f32>(0.0)) || any(q >= dims) {
            return 1.0;
        }
        if textureLoad(occupancy, vec3<i32>(floor(q)), 0).r > 0.5 {
            return 0.0;
        }
    }
    return 1.0;
}

// The cosine-weighted visibility of the sky for a surface facing `n` from an L1 sample: 1 under
// the open sky or facing away from a lone wall, 0 in a closed room.
fn sky_seen(v: vec4<f32>, n: vec3<f32>) -> f32 {
    return clamp(v.x + 2.0 * dot(v.yzw, n), 0.0, 1.0);
}

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
    return unit(pcg(u32(p.x) + pcg(u32(p.y) + pcg(u32(p.z)))));
}

// A hash's top 24 bits as [0, 1).
fn unit(h: u32) -> f32 {
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
fn fog_transmittance(origin_y_in: f32, dir_y: f32, t_in: f32) -> f32 {
    let a = g.fog.x;
    if a <= 0.0 {
        return 1.0;
    }
    // The first camera.w metres of the ray are clear air.
    let skip = min(t_in, g.camera.w);
    let origin_y = origin_y_in + dir_y * skip;
    let t = t_in - skip;
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
