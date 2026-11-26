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

// ---- scratch floor ----------------------------------------------------------
//
// Mesa's Intel Vulkan driver (anv) before Gfx 12.5 (the integrated GPUs from Skylake to Raptor
// Lake; Chrome's WebGPU on Linux runs on it too) gives the fragment shaders that spill 1 KB of
// registers per thread and those that spill 2 KB one scratch buffer (anv_allocator.c
// `anv_scratch_pool_alloc` rounds sizes below 2 KB up to the 2 KB bucket), but programs each
// with its own stride (genX_shader.c `get_scratch_space`: 1 KB): a thread of a 1 KB shader
// and one of a 2 KB shader drawing at the same time overwrite each other's spilled registers.
// That shaded blocks of pixels with garbage (white, green and magenta specks in impostors and
// hedges, checker blobs, coloured dots on roofs and glass), different from frame to frame, and
// any shader edit that moved a shader across 1 KB brought it back or took it away. Every lit
// fragment shader calls `scratch_floor` first: a branch that never runs (the render size is
// positive) with an array the compiler keeps in scratch, 2.1 KB at SIMD8, so every lit shader
// takes 4 KB or more and those sharing a buffer share its stride. 17 is the fewest that go to
// scratch (Mesa 26.2 keeps arrays up to 256 bytes in registers); each one more spreads the
// spills of the big shaders further (20 cost Iris Xe 4 to 8 % of a LOW frame, 17 1 to 4 %).
// Other drivers get an array of one (lib.rs `needs_scratch_floor`).
// lib.rs `every_lit_fragment_stage_has_the_scratch_floor` checks the calls; reference.rs
// `shading_is_the_same_every_time` the pictures.
const SCRATCH_FLOOR_VEC4S: u32 = 17u;

// Never true: lets `scratch_floor`'s array in without running it.
fn scratch_floor_taken() -> bool {
    return g.viewport.x < 0.0;
}

fn scratch_floor(clip: vec4<f32>) -> vec4<f32> {
    var pad: array<vec4<f32>, SCRATCH_FLOOR_VEC4S>;
    pad[u32(clip.x) % SCRATCH_FLOOR_VEC4S] = clip;
    return pad[u32(clip.y) % SCRATCH_FLOOR_VEC4S];
}

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
    shadow_filter: vec4<f32>,  // x the sun's shadow filter's penumbra radius in the world (m): the 16-tap disk's, PCSS's largest (the 4- and 9-tap grids ignore it; lib.rs `ShadowFilter`); y the 16-tap branch's mode, 0 PCF, 1 PCSS; zw spare
    viewport: vec4<f32>,        // render width, height, 1 / width, 1 / height (scaled)
    effects: vec4<f32>,         // x AO samples, y contact shadows on, z AO radius (m), w AO intensity
    perez: array<vec4<f32>, 5>, // Preetham's Perez coefficients A..E for (Y, x, y)
    zenith: vec4<f32>,          // zenith (Y, x, y) / F(0, theta_sun); w day factor
    night: vec4<f32>,           // rgb night sky (kcd/m2); w kcd/m2 -> pre-exposed HDR (1000 * exposure)
    highlight: vec4<u32>,       // x mesh id, y face id, z first on-top overlay segment
    wind: vec4<f32>,            // xy direction (world x, z; unit), z relative strength (speed / 6.5 m/s), w gustiness
    wind_phase: vec4<f32>,      // x gust cycles (0..1), y sway clock (s, wraps at WIND_CLOCK), z flutter cycles (0..1), w speed (m/s)
    wind_amp: vec4<f32>,        // x mean lean per unit length, y detail amplitude (1 at 6.5 m/s)
    time: vec4<f32>,            // x seconds, y exposure, z env cube's last mip, w clay mode (0/1)
    light_grid: vec4<f32>,      // xy grid origin (world x, z), z 1 / cell size, w light count
    grid_dims: vec4<u32>,       // x, y cells along world x and z
    output: vec4<f32>,          // output width, height, render scale, 1 / render scale
    fog: vec4<f32>,             // x extinction at fog.z (1/m), y height falloff (1/m), z base height (m), w sun glow
    sky: vec4<f32>,             // x source (0 Preetham, 1 physical, 2 HDRI), y HDRI rotation (rad), z source -> kcd/m2, w HDRI mip count
    ground: vec4<f32>,          // rgb: HDRI radiance below the horizon (kcd/m2), analytic skies the ground albedo (sky.wgsl ground_kcd); w HDRI below the horizon (0/1)
    options: vec4<f32>,         // x sun shadow comparison taps (1, 4, 9 or 16; scene.wgsl `cascade_shadow`), y bloom strength, z AO target divisor (1 full, 2 half), w ground height
    reflection: vec4<f32>,      // x planar reflection available (0/1), y water height (m), z clip below (0/1), w clip height (m)
    grade: vec4<f32>,           // x look contrast power, y look saturation (1, 1: plain AgX), z camera indoors (0/1), w photo sample seed
    clouds: vec4<f32>,          // x cover 0..1, y overcast (0/1), zw drift (m)
    snow_cover: mat4x4<f32>,    // world -> the snow cover map (straight down, last shadow layer)
    snow: vec4<f32>,            // x snow 0..1, y the cover map's shadow layer, z cover map on (0/1), w film grain 0..1 (tonemap.wgsl)
    sky_vol: vec4<f32>,         // sky visibility volume: xyz origin (m), w 1 / cell (m)
    sky_vol_size: vec4<f32>,    // xyz 1 / cells, w volume on (0/1)
    coast_info: vec4<f32>,      // x coast points (0: none), y sea level (m), z endless ground on the sea side (m), w surroundings kind (scene.wgsl)
    coast: array<vec4<f32>, 8>, // the coastline, two [x, z] points per entry, the sea to its right
    surround: vec4<f32>,        // the plot's rectangle (min x, min z, max x, max z) for the surroundings (kind in coast_info.w)
    ssr: vec4<f32>,             // screen-space reflections: x on (0/1), y step (px), z most steps, w thickness (m)
    wet: vec4<f32>,             // x wetness drawn 0..1 (the larger of wetness and rain), y rain 0..1, z sun share in rain, w the share the mist leaves of the sun
    mist: vec4<f32>,            // x mist and rain fog extinction at mist.z (1/m), y height falloff (1/m), z base height (m), w sun glow
    gust: vec4<f32>,            // x, y the gust pattern's coarse and fine layers travelled downwind (m, wrapped), z gusts 0..1
    shafts: vec4<f32>,          // light shafts (shafts.wgsl): x strength 0..1 (0 off), y march steps, z march distance (m), w half-res divisor
    glare: vec4<f32>,           // sun glare (shafts.wgsl): x strength (0 off), y narrow lobe width (rad), z wide lobe width (rad), w wide lobe share
    aerial: vec4<f32>,          // aerial perspective LUT (atmosphere.wgsl): x on (0/1), y farthest slice (km), z camera altitude (km); w vignette 0..1 (tonemap.wgsl)
    white: vec4<f32>,           // white balance (tonemap.wgsl): rgb the light taken for white, w mode (0 off, 1 fixed, 2 auto from the sun and sky)
    white_ref: vec4<f32>,       // rgb the white it is adapted to, w degree of adaptation 0..1
    color_grade: array<vec4<f32>, 3>, // grade matrix columns (linear Rec.709); [0].w contrast exponent about mid grey, [1].w grade on (0/1)
    sky_occ: vec4<f32>,         // baked sky visibility (`sky_occ_at`): xy (x, z) of the first column (m), z 1 / cell (m), w the lowest layer's height over the ground (m)
    sky_occ_size: vec4<f32>,    // x columns along x, y layers, z columns along z, w the layers' height span (m; 0: sky occlusion off)
    cloud_vol: vec4<f32>,       // volumetric clouds (clouds.wgsl): x drawn in this view (0/1), y base altitude (m), z layer thickness (m), w cloud type 0 (stratus) .. 1 (cumulonimbus)
    cloud_shade: vec4<f32>,     // x cloud shadows on the ground 0..1 (0 off), y the gap over the site 0..1, zw the gap's centre (world x, z; m)
    cloud_wind: vec4<f32>,      // xy the clouds' drift (m, wrapped to CLOUD_PERIOD), z extinction per unit density (1/m), w storm 0..1
    flash: vec4<f32>,           // lightning (lightning.rs): xyz the flash in the cloud (world m), w its light in the cloud (kcd/m2; 0 none)
    bolt: vec4<f32>,            // x the bolt's seed, y its brightness (0 none), z the flash's light on the scene as a radiance of the sky over it (kcd/m2, `flash_sky`; 0 none), w the bolt's ground height (m)
    sh: array<vec4<f32>, 9>,    // sky irradiance as L2 SH (kcd/m2 * sr), written on the GPU (env.wgsl)
}
@group(0) @binding(0) var<uniform> g: Globals;
// The prefiltered environment cube and the sky source: the HDRI
// (equirectangular, mipmapped) or the physical sky's sky-view LUT.
@group(0) @binding(1) var env_map: texture_cube<f32>;
@group(0) @binding(2) var env_sampler: sampler;
@group(0) @binding(5) var sky_tex: texture_2d<f32>;
// The sky's visibility round the buildings (lib.rs `set_sky_volume`, sky_volume.rs), one level,
// two tiles side by side along x: its room light, rgb the lamps' and the sun's light passed round
// the rooms (lux), a the sky's (per unit of its mean illuminance), or -1 minus its neighbours'
// inside a wall or a thin gap; then the sky's visibility, r the visible fraction of directions,
// gba the mean visible direction.
@group(0) @binding(7) var room_vis: texture_3d<f32>;
// Its occluders' voxels, 0.2 m from the same origin: r 1 where solid.
@group(0) @binding(8) var occupancy: texture_3d<f32>;
// The baked sky visibility (sky_visibility.rs, lib.rs `set_sky_visibility`): per probe the
// visibility of the sky as L2 SH, three tiles side by side along x (coefficients 0-3, 4-7, and 8
// with the probe's weight, its column's ground height and its occluders' reflectance), the
// coefficients and the reflectance multiplied by the weight (0 indoors): a filtered read over the
// filtered weight interpolates over the outdoor probes only. One level, filtered.
@group(0) @binding(6) var sky_occ_tex: texture_3d<f32>;
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
    let dims = vec3<i32>(textureDimensions(room_vis)) / vec3<i32>(2, 1, 1);
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
        sky += w * textureLoad(room_vis, at + vec3<i32>(dims.x, 0, 0), 0);
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

// The baked sky visibility at a surface at `world` facing `n` (lifted half a cell, at most 0.6 m,
// off it, so the probes on its own side of a thin wall weigh most, as sky_visibility.rs
// `SkyVisibility::sky_view` reads it): the visibility SH
// `v`, the weight of the outdoor probes round the point `weight` (0 outside the grid, where only
// indoor probes are round, or with sky occlusion off) and the mean reflectance of what those
// probes see instead of sky. The grid follows the ground: layer k stands at
// lift + span (k / (layers - 1))^2 over its column's ground.
struct SkyOcc {
    v: array<f32, 9>,
    weight: f32,
    albedo: f32,
}

fn sky_occ_at(world: vec3<f32>, n: vec3<f32>) -> SkyOcc {
    var o: SkyOcc;
    o.weight = 0.0;
    o.albedo = 0.3;
    if g.sky_occ_size.w <= 0.0 {
        return o;
    }
    let dims = g.sky_occ_size.xyz;
    let p = world + n * min(0.5 / g.sky_occ.z, 0.6);
    let fx = (p.x - g.sky_occ.x) * g.sky_occ.z;
    let fz = (p.z - g.sky_occ.y) * g.sky_occ.z;
    if fx < 0.0 || fz < 0.0 || fx > dims.x - 1.0 || fz > dims.z - 1.0 {
        return o;
    }
    let width = 3.0 * dims.x;
    let w = (fz + 0.5) / dims.z;
    let u = (fx + 0.5) / width;
    let ground = textureSampleLevel(sky_occ_tex, env_sampler, vec3<f32>(u + 2.0 / 3.0, 0.5 / dims.y, w), 0.0).b;
    let k = (dims.y - 1.0) * sqrt(max(p.y - ground - g.sky_occ.w, 0.0) / g.sky_occ_size.w);
    if k > dims.y - 1.0 {
        return o;
    }
    let v = (k + 0.5) / dims.y;
    let a = textureSampleLevel(sky_occ_tex, env_sampler, vec3<f32>(u, v, w), 0.0);
    let b = textureSampleLevel(sky_occ_tex, env_sampler, vec3<f32>(u + 1.0 / 3.0, v, w), 0.0);
    let c = textureSampleLevel(sky_occ_tex, env_sampler, vec3<f32>(u + 2.0 / 3.0, v, w), 0.0);
    if c.g < 1e-3 {
        return o;
    }
    let inv = 1.0 / c.g;
    o.v = array<f32, 9>(a.x * inv, a.y * inv, a.z * inv, a.w * inv, b.x * inv, b.y * inv, b.z * inv, b.w * inv, c.r * inv);
    // Faded out over the outermost two cells and the top layer, where the grid meets the open
    // sky it is not drawn with: no seam on the ground at the grid's edge.
    let edge = min(min(fx, fz), min(dims.x - 1.0 - fx, dims.z - 1.0 - fz));
    let fade = clamp(edge * 0.5, 0.0, 1.0) * clamp(dims.y - 1.0 - k, 0.0, 1.0);
    o.weight = clamp(c.g, 0.0, 1.0) * fade;
    o.albedo = c.a * inv;
    return o;
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

// The same noise turned by the photo sample (`g.grade.w`, 0 outside photos) along the golden
// ratio, so a pattern dithered with it (alpha to coverage, fades) averages out over a photo's
// samples instead of staying fixed in every one.
fn ign_sampled(pixel: vec2<f32>) -> f32 {
    return fract(ign(pixel) + g.grade.w * 0.618034);
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

// The sky's irradiance on a horizontal surface from the SH (kcd/m2 * sr; env.wgsl fs_sh
// integrates it directly; the cube faces take it from here). Shared by the sky and the
// automatic white balance (tonemap.wgsl).
fn sky_up_sh() -> vec3<f32> {
    let e = g.sh[0].rgb * 0.282095 + g.sh[1].rgb * 0.488603 - g.sh[6].rgb * 0.315392 - g.sh[8].rgb * 0.546274;
    return max(e, vec3<f32>(0.0));
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
    return exp(-fog_depth(origin_y_in, dir_y, t_in));
}

// The haze's optical depth along the ray (`fog_transmittance` is e^-depth).
fn fog_depth(origin_y_in: f32, dir_y: f32, t_in: f32) -> f32 {
    let a = g.fog.x;
    if a <= 0.0 {
        return 0.0;
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
    return start * path;
}

// Mist and rain fog: a second, low layer of water droplets under the aerosol haze (lib.rs
// `mist_uniform`): dense at the ground and thinning with a scale height of tens of metres
// (radiation mist) to a few hundred (the air under a rain cloud), the same closed form as the
// haze (Wenzel 2007; Hillaire, "Physically Based and Unified Volumetric Rendering in
// Frostbite", SIGGRAPH 2015, for height fog as a participating medium). Its optical depth.
fn mist_depth(origin_y: f32, dir_y: f32, t: f32) -> f32 {
    let a = g.mist.x;
    if a <= 0.0 {
        return 0.0;
    }
    let b = g.mist.y;
    let start = a * exp(min(-b * (origin_y - g.mist.z), 80.0));
    let k = b * dir_y;
    var path = t;
    if abs(k * t) > 1e-4 {
        path = (1.0 - exp(min(-k * t, 80.0))) / k;
    }
    return start * path;
}

// Light scattered into the view ray by the mist, per unit (1 - T). Droplets are large next to
// the wavelength, so they scatter all colours alike (a white or grey veil, never blue) and
// strongly forwards: a Henyey-Greenstein lobe of g = 0.75 (cloud and fog droplets, after Mie
// theory) for the sun, plus the rest of the sun's light scattered many times, spread evenly,
// and the whole sky's light (a wide, blurred lobe of the environment above the view's
// azimuth: a mist reflects the sky it lies under, grey under a rain sky). The sun's light is
// the one that reaches the ground (`g.sun_light`, already cut by clouds, rain and the mist).
fn mist_inscatter(dir: vec3<f32>) -> vec3<f32> {
    let up = normalize(vec3<f32>(dir.x, max(dir.y, 0.35), dir.z));
    let sky = textureSampleLevel(env_map, env_sampler, up, g.time.z * 0.7).rgb * g.night.w;
    let c = dot(dir, g.sun_dir.xyz);
    let gg = 0.75;
    let hg = (1.0 - gg * gg) / (4.0 * PI * pow(1.0 + gg * gg - 2.0 * gg * c, 1.5));
    let sun = g.sun_light.rgb * (g.mist.w * hg + 0.35 / (4.0 * PI)) * smoothstep(-0.05, 0.05, g.sun_dir.y);
    return sky * 0.9 + sun;
}

// The haze and the mist between the eye and the point `t` metres along `dir`: rgb the light
// they scatter into the ray, w what is left of the light from behind. Without mist exactly the
// haze alone; with it each medium's light in proportion to its optical depth.
fn veil(dir: vec3<f32>, t: f32) -> vec4<f32> {
    let ta = fog_depth(g.camera.y, dir.y, t);
    if g.mist.x <= 0.0 {
        let transmit = exp(-ta);
        return vec4<f32>(fog_inscatter(dir, 1.0) * fog_near(transmit) * (1.0 - transmit), transmit);
    }
    let tb = mist_depth(g.camera.y, dir.y, t);
    let transmit = exp(-(ta + tb));
    var light = mist_inscatter(dir);
    if ta > 0.0 {
        light = (fog_inscatter(dir, 1.0) * ta + light * tb) / (ta + tb);
    }
    return vec4<f32>(light * fog_near(transmit) * (1.0 - transmit), transmit);
}

// Whether anything hazes the view (the haze or the mist).
fn veiled() -> bool {
    return g.fog.x > 0.0 || g.mist.x > 0.0;
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
    if !veiled() {
        return color;
    }
    let to = -to_eye(world);
    let t = length(to);
    let dir = to / max(t, 1e-6);
    let v = veil(dir, t);
    return color * v.w + v.rgb;
}

// Haze over a short path is lit by the whole sphere around it, darker
// ground included, not only by the horizon sky it converges to over a long
// path: a crude single-scattering correction (about half the horizon's
// radiance for thin haze, all of it as the path becomes opaque) that keeps
// dark hedges at dusk from turning milky.
fn fog_near(transmit: f32) -> f32 {
    return mix(1.0, 0.5, transmit);
}

// ---- wind -----------------------------------------------------------------
//
// One sway for every level of a plant: the mesh levels (scene.wgsl
// `animated_world`) and the impostor's quad (impostor.wgsl) both call
// `wind_sway`, so a crown top is displaced the same whichever draws it.
// rust: wind.rs `sway` is the same function on the CPU (tested to agree).
//
// Vegetation animation after Sousa, "Vegetation Procedural Animation and
// Shading in Crysis", GPU Gems 3, ch. 16 (2007): a main bend of the whole
// plant along the wind, length-preserved about the plant's origin, plus
// detail (branch) motion from the per-vertex weight and phase, driven by
// smoothed triangle waves. The bend weight is squared: w^2 is 0 at the base
// with zero slope, so the foot stays put. Speeds, amplitudes and phases come
// from the CPU (wind.rs): the lean follows drag (growing with speed squared,
// saturating as crowns streamline: de Langre, Ann. Rev. Fluid Mech. 40,
// 2008), the phases are integrated there so they never wrap into a jump, and
// each plant sways at its own natural frequency (`hz`, the instance
// matrix's spare w component).

const WIND_CLOCK: f32 = 64.0;    // the sway clock wraps here (s); per-plant rates are whole cycles per wrap
const GUST_WAVELENGTH: f32 = 80.0; // metres: gusts travel downwind at the wind speed

struct WindState {
    wind: vec4<f32>,
    phase: vec4<f32>,
    amp: vec4<f32>,
    gust: vec4<f32>,
}

fn wind_state() -> WindState {
    return WindState(g.wind, g.wind_phase, g.wind_amp, g.gust);
}

// ---- gusts ------------------------------------------------------------------
//
// Gusts as a field in the world: a low-frequency noise pattern carried downwind at the wind
// speed (Taylor's frozen turbulence: eddies drift with the mean flow), stretched across the
// wind, so a gust is a patch of stronger wind that crosses a lawn as a rolling wave of bent
// blades and reaches one crown after the other. One field drives the grass, the trees and the
// rain's slant, as in the public talks on wind over a field ("Procedural Grass in Ghost of
// Tsushima", GDC 2021) and on vegetation (Sousa, GPU Gems 3 ch. 16). Two layers, a coarse one
// (about 14 m along the wind, 22 m across) and a finer one drifting a little slower, so the
// patches change shape as they travel. Value noise on a lattice that repeats every
// `GUST_CELLS` cells along the wind, so the distance travelled wraps exactly (wind.rs keeps
// it, and `gust_field` there is the same function on the CPU).
const GUST_CELLS: i32 = 64;
const GUST_COARSE: vec2<f32> = vec2<f32>(14.0, 22.0); // metres per cell along and across the wind
const GUST_FINE: vec2<f32> = vec2<f32>(5.0, 7.0);

fn gust_lattice(cell: vec2<i32>, layer: i32) -> f32 {
    let x = ((cell.x % GUST_CELLS) + GUST_CELLS) % GUST_CELLS;
    return hash3(vec3<i32>(x, cell.y, layer));
}

fn gust_noise(p: vec2<f32>, layer: i32) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = gust_lattice(i, layer);
    let b = gust_lattice(i + vec2<i32>(1, 0), layer);
    let c = gust_lattice(i + vec2<i32>(0, 1), layer);
    let d = gust_lattice(i + vec2<i32>(1, 1), layer);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// The gust at `at` (world x, z), 0 (a lull) to 1 (the core of a gust): mostly calm with patches.
fn gust_field(ws: WindState, at: vec2<f32>) -> f32 {
    let along = dot(at, ws.wind.xy);
    let across = dot(at, vec2<f32>(-ws.wind.y, ws.wind.x));
    let coarse = gust_noise(vec2<f32>((along - ws.gust.x) / GUST_COARSE.x, across / GUST_COARSE.y), 0);
    let fine = gust_noise(vec2<f32>((along - ws.gust.y) / GUST_FINE.x, across / GUST_FINE.y), 1);
    return smoothstep(0.42, 0.82, 0.7 * coarse + 0.3 * fine);
}

// The wind's strength factor with the gusts at a point: the slow swell (`wind.w`, the old
// travelling wave) times what a lull leaves, plus a gust's push. Gusts 0: the swell alone.
fn gust_factor(ws: WindState, swell: f32, at: vec2<f32>) -> f32 {
    let k = ws.gust.z;
    return swell * (1.0 - 0.4 * k) + 1.8 * k * gust_field(ws, at);
}

fn smooth_triangle(x: vec4<f32>) -> vec4<f32> {
    let t = abs(fract(x + 0.5) * 2.0 - 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// `rel`: the point from the plant's origin (world, m); `w`: bend weight 0..1
// (0 at the base); `phase`: the limb's phase (cycles); `hz_in`: the plant's sway
// frequency, negative for lawn grass (lib.rs `set_item_grass`). Returns the displaced point,
// still relative to the origin.
fn wind_sway(ws: WindState, rel: vec3<f32>, origin: vec3<f32>, w: f32, phase: f32, hz_in: f32) -> vec3<f32> {
    if w <= 0.0 || ws.amp.x <= 0.0 {
        return rel;
    }
    let hz = abs(hz_in);
    let grass = hz_in < 0.0;
    let dir = vec3<f32>(ws.wind.x, 0.0, ws.wind.y);
    // R2 low-discrepancy weights (Roberts 2018): neighbouring plants get
    // well-spread phases for two multiply-adds.
    let instance_phase = fract(dot(origin.xz, vec2<f32>(0.7548777, 0.5698403)));
    // Whole cycles per clock wrap, so the wrap itself is no jump.
    let k = clamp(round(hz * WIND_CLOCK), 2.0, 6.0 * WIND_CLOCK);
    // Branches at twice the sway, never above 2.5 Hz (wind.rs `BRANCH_RATIO`, `BRANCH_MAX_HZ`).
    let kb = min(2.0 * k, 2.5 * WIND_CLOCK);
    let waves = smooth_triangle(vec4<f32>(
        ws.phase.x - dot(origin.xz, ws.wind.xy) / GUST_WAVELENGTH, // slow gust envelope
        ws.phase.y * (k / WIND_CLOCK) + instance_phase,            // whole-plant sway
        ws.phase.y * (kb / WIND_CLOCK) + phase + instance_phase,   // branch motion
        ws.phase.z + phase * 1.7,                                  // leaf flutter
    )) * 2.0 - 1.0;
    var gust = 1.0 + ws.wind.w * waves.x;
    var gust_detail = gust;
    let size = length(rel);
    if ws.gust.z > 0.0 {
        // The gust field: at the plant's foot for the whole plant's bend (trunk and stem),
        // at the point itself for its branches and leaves, so a gust runs through a big
        // crown from its windward side; one lookup for a grass tuft.
        let swell = gust;
        gust = gust_factor(ws, swell, origin.xz);
        gust_detail = gust;
        if !grass {
            gust_detail = gust_factor(ws, swell, origin.xz + rel.xz);
        }
    }
    // Main bend: tips lean downwind by `amp.x` of their distance from the
    // base, plus a sway around that.
    var bend = ws.amp.x * gust * (0.6 + 0.4 * waves.y);
    if grass {
        // A blade is far more pliant than a stem of wood: with gusts on, lawn grass bends with
        // them, up to about 50 degrees in the core of a strong gust.
        bend = min(bend * (1.0 + 5.0 * ws.gust.z), 1.2);
    }
    var q = rel + dir * (w * w * bend * size);
    q = q * (size / max(length(q), 1e-6));
    // Detail: branches bob along the wind and vertically, leaves flutter.
    // (no detail right at the origin: the very foot of the stem stays put whatever its weight)
    let detail = ws.amp.y * gust_detail * w * (0.02 + 0.01 * min(size, 6.0)) * (0.4 + 0.6 * clamp(size / 6.0, 0.0, 1.0)) * min(size * 2.0, 1.0);
    q += (dir * waves.z + vec3<f32>(0.0, 0.6, 0.0) * waves.w) * detail;
    return q;
}

// The same sway for a point of an impostor's quad: `h` is its height as a
// share of the plant's (0 base, 1 crown top); the bend weight of a mesh
// grows about as h^1.5 up a stem, and 1 at the top; the limb phase is the mean.
fn impostor_sway(ws: WindState, rel: vec3<f32>, origin: vec3<f32>, h: f32, hz: f32) -> vec3<f32> {
    return wind_sway(ws, rel, origin, pow(clamp(h, 0.0, 1.0), 1.5), 0.0, hz);
}

// ---- the clouds' weather map ------------------------------------------------
// Shared by the volumetric clouds (clouds.wgsl, through a texture made once from
// `cloud_weather`, cloud_noise.wgsl) and the cloud shadows on the ground
// (`cloud_shadow`, evaluated here directly): both see the same coverage.

// The weather map repeats every CLOUD_PERIOD metres; the clouds' noise tiles divide it, so the
// drift can wrap without a jump.
const CLOUD_PERIOD: f32 = 40960.0;
const CLOUD_BASE_TILE: f32 = 2560.0;
const CLOUD_DETAIL_TILE: f32 = 160.0;
// Separate clouds' density to extinction, over the renderer's (`cloud_wind.z`): crisp edges.
// A closed layer keeps the renderer's, or its base would turn as dark as a storm's.
const CLOUD_DENSE: f32 = 2.4;

fn cloud_dense() -> f32 {
    return mix(CLOUD_DENSE, 1.0, g.clouds.y);
}
// The curl noise in the weather map's ba repeats this often (clouds.wgsl `density`).
const CLOUD_CURL_TILE: f32 = 1280.0;
// The weather noise's scale to its spread about 0.5 (`cloud_weather`): a cover c clouds about a
// share c of the sky.
const WEATHER_SPREAD: f32 = 3.0;
// The gap kept in the clouds in front of the sun over the site (`cloud_shade.y`), metres.
const CLOUD_GAP_M: f32 = 900.0;

// Value noise on a lattice of `n` cells that repeats every `n` cells.
fn cloud_lattice(i: vec2<i32>, n: i32) -> f32 {
    let w = ((i % n) + n) % n;
    return unit(pcg(u32(w.x) + pcg(u32(w.y) + 40503u)));
}

fn cloud_value(p: vec2<f32>, n: i32) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = cloud_lattice(i, n);
    let b = cloud_lattice(i + vec2<i32>(1, 0), n);
    let c = cloud_lattice(i + vec2<i32>(0, 1), n);
    let d = cloud_lattice(i + vec2<i32>(1, 1), n);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// A gradient on a lattice of `n` cells that repeats every `n` cells (each component -1..1:
// no trigonometry, which the cloud shadows evaluate per pixel).
fn cloud_gradient(i: vec2<i32>, n: i32, seed: u32) -> vec2<f32> {
    let w = ((i % n) + n) % n;
    let h = pcg(u32(w.x) + pcg(u32(w.y) + seed));
    return vec2<f32>(unit(h), unit(pcg(h))) * 2.0 - 1.0;
}

// Gradient noise (Perlin, "Improving Noise", SIGGRAPH 2002) on a lattice of `n` cells that
// repeats every `n` cells, about -0.7..0.7: rounder outlines than value noise, which, cut at a
// threshold, leaves cells with straight sides along the lattice.
fn cloud_perlin(p: vec2<f32>, n: i32, seed: u32) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = dot(cloud_gradient(i, n, seed), f);
    let b = dot(cloud_gradient(i + vec2<i32>(1, 0), n, seed), f - vec2<f32>(1.0, 0.0));
    let c = dot(cloud_gradient(i + vec2<i32>(0, 1), n, seed), f - vec2<f32>(0.0, 1.0));
    let d = dot(cloud_gradient(i + vec2<i32>(1, 1), n, seed), f - vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// The weather at `uv` (one period, 0..1): x where clouds form (spread to about 0..1, so a cover
// c clouds about a share c of the sky), y a slow variation of their type. Two octaves of
// gradient noise and a finer one of value noise, whose domain a slow value noise bends
// (Quilez, "Domain warping", 2002), so the cells have ragged, rounded outlines and no grid
// shows; everything repeats with the map.
fn cloud_weather(uv: vec2<f32>) -> vec2<f32> {
    let warp = vec2<f32>(cloud_value(uv * 10.0, 10), cloud_value(uv * 10.0 + 0.5, 10)) * 1.2 - 0.6;
    let q = uv * 20.0 + warp;
    let n = 0.6 * cloud_perlin(q, 20, 40503u) + 0.28 * cloud_perlin(q * 2.0, 40, 811u) + 0.12 * (cloud_value(q * 4.0, 80) - 0.5);
    let spread = clamp(n * WEATHER_SPREAD + 0.5, 0.0, 1.0);
    return vec2<f32>(spread, cloud_value(uv * 6.0, 6));
}

// The coverage 0..1 the weather `w` gives at world (x, z): fair-weather cells where the noise
// passes the cover, an even layer as the sky turns overcast, less in the gap over the site.
fn cloud_coverage(w: vec2<f32>, xz: vec2<f32>) -> f32 {
    let cover = g.clouds.x;
    let fair = smoothstep(1.0 - cover - 0.04, 1.0 - cover + 0.3, w.x);
    let even = min(cover * 1.1, 1.0) * mix(0.6, 1.0, w.x);
    var c = mix(fair, even, g.clouds.y);
    if g.cloud_shade.y > 0.0 {
        let d = xz - g.cloud_shade.zw;
        c *= 1.0 - g.cloud_shade.y * exp(-dot(d, d) / (CLOUD_GAP_M * CLOUD_GAP_M));
    }
    return c;
}

// The sun's light over the clouds (kcd/m2 * sr per unit phase; volumetric clouds, clouds.wgsl,
// and the flat layer's stand-in for them, sky.wgsl `cloud`): the light on the ground with the
// clouds', the rain's and the mist's share taken back out, and the haze's and the clear air's
// below the clouds' base given back, along the sun's air mass (Kasten & Young, "Revised optical
// air mass tables and approximation formula", Applied Optics 1989): a low sun lights the clouds
// far more than the garden under them, which is why they glow at sunrise and sunset (capped at
// e^4).
fn cloud_sun_light() -> vec3<f32> {
    let mu = max(g.sun_dir.y, 0.0);
    let air_mass = 1.0 / (mu + 0.50572 * pow(degrees(asin(mu)) + 6.07995, -1.6364));
    let base = g.cloud_vol.y;
    var haze_below = 0.0;
    if g.fog.x > 0.0 {
        haze_below = g.fog.x / g.fog.y * (1.0 - exp(-g.fog.y * max(base - g.fog.z, 0.0)));
    }
    let rayleigh = vec3<f32>(5.802e-6, 13.558e-6, 33.1e-6);
    let air_below = rayleigh * 8000.0 * (1.0 - exp(-base / 8000.0));
    let lift = exp(min((vec3<f32>(haze_below) + air_below) * air_mass, vec3<f32>(4.0)));
    return g.sun_light.rgb / max(g.night.w, 1e-12) / max(g.wet.z, 0.02) * lift;
}

// A lightning flash's light on the scene (lightning.rs `fill_kcd`), as a radiance it adds to
// the sky over it (kcd/m2).
fn flash_sky() -> vec3<f32> {
    return vec3<f32>(0.85, 0.88, 1.0) * g.bolt.z;
}

// The share of the sun the clouds let through to `world`: the coverage where the sun's ray
// crosses the clouds' lower part, as a transmittance (Beer-Lambert over the optical depth a
// cloud of that coverage has). The overcast part of the sky dims the sun globally (lib.rs
// `sun_share`); this is only the broken part's shadows, which drift with the clouds.
fn cloud_shadow(world: vec3<f32>) -> f32 {
    if g.cloud_shade.x <= 0.0 || g.sun_dir.y <= 0.02 {
        return 1.0;
    }
    let mid = g.cloud_vol.y + 0.15 * g.cloud_vol.z;
    let l = g.sun_dir.xyz;
    let xz = world.xz + l.xz / l.y * max(mid - world.y, 0.0);
    let w = cloud_weather(fract((xz + g.cloud_wind.xy) / CLOUD_PERIOD));
    let c = cloud_coverage(w, xz);
    return 1.0 - g.cloud_shade.x * (1.0 - exp(-10.0 * c));
}
