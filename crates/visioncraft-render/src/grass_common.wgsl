// GPU grass (grass.rs): what the compute pass that generates the blades (grass_compute.wgsl)
// and the passes that draw them (grass.wgsl) share. Group 1, bindings 40+, so it never collides
// with the material's, the terrain's, the impostors' or the sea's.

struct GrassFrame {
    view_proj: mat4x4<f32>,
    planes: array<vec4<f32>, 6>, // the view frustum's planes (xyz inward normal, w offset)
    eye: vec4<f32>,              // xyz the eye, w the grid's reach (m)
    field: vec4<f32>,            // xy the mask's corner (world x, z), z 1 / cell, w cell (m)
    field_size: vec4<f32>,       // xy cells along x, z; z the lowest ground, w the highest (m)
    lod: vec4<f32>,              // x blades per m2 at full density, y full-density radius (m), z near/far list switch (m), w fade radius of a 0.2 m blade (m)
    tiles: vec4<i32>,            // xy the first tile (x, z), z tiles per side
    caps: vec4<u32>,             // blades each list holds: near, far, flowers
    verts: vec4<u32>,            // vertices of a blade in each list
    kinds: array<vec4<f32>, 16>, // per lawn material: [rgb blade colour, stripes width (m)], [stripes direction x, z, dry blades 0..1, 0]
    flowers: array<vec4<f32>, 4>, // rgb flower colours of the season
}
@group(1) @binding(40) var<uniform> grass: GrassFrame;

// Metres per tile of the generation grid (one workgroup each).
const GRASS_TILE: f32 = 2.0;
// The tallest blade the mask codes (core grass.rs `MAX_BLADE_HEIGHT_M`).
const GRASS_MAX_HEIGHT: f32 = 1.2;
const GRASS_TAU: f32 = 6.2831853;

// A blade as the compute pass writes it and the vertex stage reads it (32 bytes).
// root: xyz its foot, w its height (m).
// data.x: width (m) / 0.1 and bend (rad) / 2 as two unorm halves.
// data.y: facing (low 16 bits of a turn), a seed (high 16 bits).
// data.z: colour variation, dry, occlusion at the root, palette entry (four bytes).
// data.w: flower colour, kind (0 blade, 1 seed stem, 2 flower), widening / 8 (bytes).

fn grass_pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn grass_unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// The square-root coded blade height (core grass.rs `encode_height`), from 0..1.
fn grass_height(v: f32) -> f32 {
    return v * v * GRASS_MAX_HEIGHT;
}
