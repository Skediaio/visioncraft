// Terrain: heightmap chunks with up to 8 painted material layers (per-vertex
// u8 weights, summing to 255). The layers' maps live in three 2D-array
// textures (base colour, normal, occlusion-roughness-metallic) of 512^2 x 8
// (lib.rs blits each layer's own textures in), so eight layers cost three
// texture bindings. Mapping is planar from world XZ, switching to triplanar
// on slopes steeper than ~45 degrees. Layers blend by weight and height
// (the ORM map's occlusion channel as the height): Mishkinis, "Advanced
// Terrain Texture Splatting" (Gamasutra, 2013) - only layers within a
// small depth of the highest weight+height contribute, so gravel fills the
// gaps between paving stones instead of cross-fading over them. Near the
// terrain's outer edge it blends into the infinite ground's colour.
// Module: common.wgsl + scene.wgsl + this file (group 1 is the terrain's
// own layout, bindings 13-17, so it never collides with the material's).

struct TerrainLayer {
    color: vec4<f32>,  // linear base colour factor
    params: vec4<f32>, // x roughness, y 1 / scale (m), z macro variation
}

struct Terrain {
    layers: array<TerrainLayer, 8>,
    edge: vec4<f32>,   // terrain bounds in world x, z: min x, min z, max x, max z
    ground: vec4<f32>, // rgb ground albedo, w width of the blend band (0: none)
}
@group(1) @binding(13) var<uniform> terrain: Terrain;
@group(1) @binding(14) var terrain_base: texture_2d_array<f32>;
@group(1) @binding(15) var terrain_normal: texture_2d_array<f32>;
@group(1) @binding(16) var terrain_orm: texture_2d_array<f32>;
@group(1) @binding(17) var terrain_smp: sampler;

struct TerrainIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: u32,
    @location(3) uv: vec2<f32>,
    @location(4) wind: vec2<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) w0: vec4<f32>,
    @location(10) w1: vec4<f32>,
}

struct TerrainVarying {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) uv: vec2<f32>,
    @location(4) w0: vec4<f32>,
    @location(5) w1: vec4<f32>,
}

@vertex
fn vs_terrain(v: TerrainIn) -> TerrainVarying {
    let m = model(v.m0, v.m1, v.m2, v.m3);
    let world = animated_world(v.position, v.wind, m, true);
    var out: TerrainVarying;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = world_normal(m, v.normal);
    out.face = v.face;
    out.uv = v.uv;
    out.w0 = v.w0;
    out.w1 = v.w1;
    return out;
}

fn terrain_sample(tex: texture_2d_array<f32>, l: Lookup, layer: i32) -> vec4<f32> {
    var c = vec4<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        if l.weight[i] > 0.0 {
            c += textureSampleGrad(tex, terrain_smp, l.uv[i], layer, l.dx[i], l.dy[i]) * l.weight[i];
        }
    }
    return c;
}

// Whiteout-blended triplanar normal (as triplanar_normal) from the array.
fn terrain_normal_sample(l: Lookup, n: vec3<f32>, layer: i32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        if l.weight[i] > 0.0 {
            let tn = textureSampleGrad(terrain_normal, terrain_smp, l.uv[i], layer, l.dx[i], l.dy[i]).xyz * 2.0 - 1.0;
            let t = l.t[i];
            let b = l.b[i];
            let axis = cross(t, b);
            let base = vec3<f32>(dot(n, t), dot(n, b), dot(n, axis));
            let w = vec3<f32>(tn.xy + base.xy, tn.z * base.z);
            sum += (w.x * t + w.y * b + w.z * axis) * l.weight[i];
        }
    }
    return sum;
}

const HEIGHT_DEPTH: f32 = 0.2;

fn terrain_surface(in: TerrainVarying) -> Surface {
    let n = normalize(in.normal);
    let dwx = dpdx(in.world);
    let dwy = dpdy(in.world);
    var weights = array<f32, 8>(in.w0.x, in.w0.y, in.w0.z, in.w0.w, in.w1.x, in.w1.y, in.w1.z, in.w1.w);
    // Planar from above unless the slope is steep.
    let steep = abs(n.y) < 0.7;
    var lookups: array<Lookup, 8>;
    var orm: array<vec3<f32>, 8>;
    var top = 0.0;
    for (var i = 0; i < 8; i++) {
        if weights[i] > 0.004 {
            var l = triplanar(in.world, dwx, dwy, n, terrain.layers[i].params.y);
            if !steep {
                l.weight = vec3<f32>(0.0, 1.0, 0.0);
            }
            lookups[i] = l;
            orm[i] = terrain_sample(terrain_orm, l, i).rgb;
            top = max(top, orm[i].r + weights[i]);
        }
    }
    var s: Surface;
    var base = vec3<f32>(0.0);
    var nsum = vec3<f32>(0.0);
    var rough = 0.0;
    var ao = 0.0;
    var metal = 0.0;
    var variation = 0.0;
    var total = 0.0;
    for (var i = 0; i < 8; i++) {
        if weights[i] > 0.004 {
            let b = max(orm[i].r + weights[i] - (top - HEIGHT_DEPTH), 0.0);
            if b > 0.0 {
                let layer = terrain.layers[i];
                base += terrain_sample(terrain_base, lookups[i], i).rgb * layer.color.rgb * b;
                nsum += terrain_normal_sample(lookups[i], n, i) * b;
                rough += layer.params.x * orm[i].g * b;
                metal += orm[i].b * b;
                ao += orm[i].r * b;
                variation += layer.params.z * b;
                total += b;
            }
        }
    }
    let k = 1.0 / max(total, 1e-5);
    s.base = vec4<f32>(base * k, 1.0);
    s.n = normalize(select(nsum, n, dot(nsum, nsum) < 1e-8));
    s.geometric = n;
    s.roughness = clamp(rough * k, 0.045, 1.0);
    s.metallic = metal * k;
    s.ao = mix(1.0, ao * k, 0.6);
    s.emissive = vec3<f32>(0.0);
    s = macro_variation(s, in.world, variation * k);
    // The outer band blends into the infinite ground: no border.
    if terrain.ground.w > 0.0 {
        let e = terrain.edge;
        let d = min(min(in.world.x - e.x, e.z - in.world.x), min(in.world.z - e.y, e.w - in.world.z));
        let t = 1.0 - smoothstep(0.0, terrain.ground.w, d);
        s.base = vec4<f32>(mix(s.base.rgb, terrain.ground.rgb, t), 1.0);
        s.n = normalize(mix(s.n, vec3<f32>(0.0, 1.0, 0.0), t));
    }
    return s;
}

fn terrain_varying(in: TerrainVarying) -> Varying {
    return Varying(in.clip, in.world, in.normal, in.face, in.uv);
}

@fragment
fn fs_terrain(in: TerrainVarying) -> @location(0) vec4<f32> {
    return vec4<f32>(light(terrain_varying(in), terrain_surface(in), false, 0.0, true).color, 1.0);
}

@fragment
fn fs_terrain_reflect(in: TerrainVarying) -> @location(0) vec4<f32> {
    let shaded = light(terrain_varying(in), terrain_surface(in), false, 0.0, false);
    if g.reflection.z > 0.5 && in.world.y < g.reflection.w {
        discard;
    }
    return vec4<f32>(shaded.color, 1.0);
}
