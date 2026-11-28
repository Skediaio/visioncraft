// Terrain: heightmap chunks with up to 8 painted material layers (per-vertex
// u8 weights, summing to 255). The layers' maps live in three 2D-array
// textures (base colour, normal, occlusion-roughness-metallic) of 512^2 x 8
// (lib.rs blits each layer's own textures in), so eight layers cost three
// texture bindings. Mapping is planar from world XZ, switching to triplanar
// on slopes steeper than ~45 degrees. Layers blend by weight and height
// (the layer's height map, carried in the ORM array's alpha; the occlusion
// stands in for a layer without one): Mishkinis, "Advanced
// Terrain Texture Splatting" (Gamasutra, 2013) - only layers within a
// small depth of the highest weight+height contribute, so gravel fills the
// gaps between paving stones instead of cross-fading over them. Near the
// terrain's outer edge it blends into the infinite ground's colour.
// Hex tiling (per layer, seen from above): the plane is covered by a lattice of hexagons, each
// with its own random shift and turn of the maps, and every texel blends the three hexagons round
// it by their barycentric weights sharpened by each copy's height, so no repeat of the tile shows
// (Heitz & Neyret, "High-Performance By-Example Noise using a Histogram-Preserving Blending
// Operator", HPG 2018; Mikkelsen, "Practical Real-Time Hex-Tiling", JCGT 11(3), 2022). The turned
// copies' normals are turned back by the same angle.
// Detail textures (per layer, terrain.set_layer_detail): a finer map over a layer's own near the
// eye (`terrain_detail`), so a field or a desert neither blurs up close nor repeats from afar.
// Module: common.wgsl + scene.wgsl + this file (group 1 is the terrain's
// own layout, bindings 13-19, so it never collides with the material's).

struct TerrainLayer {
    color: vec4<f32>,  // rgb linear base colour factor, a added to the roughness
    params: vec4<f32>, // x roughness, y 1 / scale (m), z macro variation, w normal map strength
    extra: vec4<f32>,  // x puddles may form on this layer (0/1; 0 on lawns), y hex tiling (0/1)
    detail: vec4<f32>, // x 1 / detail tile (m), y detail strength, z fade distance (m), w flags: 1 a detail, 2 a normal map
}

struct Terrain {
    layers: array<TerrainLayer, 8>,
    edge: vec4<f32>,   // terrain bounds in world x, z: min x, min z, max x, max z
    ground: vec4<f32>, // rgb ground albedo, w width of the blend band (0: none)
    detail: vec4<f32>, // x any layer has a detail (0/1), y the farthest fade distance (m), zw spare
}
@group(1) @binding(13) var<uniform> terrain: Terrain;
@group(1) @binding(14) var terrain_base: texture_2d_array<f32>;
@group(1) @binding(15) var terrain_normal: texture_2d_array<f32>;
@group(1) @binding(16) var terrain_orm: texture_2d_array<f32>;
@group(1) @binding(17) var terrain_smp: sampler;
// The layers' detail maps (colour, normal), one array layer per terrain layer.
@group(1) @binding(18) var terrain_detail_base: texture_2d_array<f32>;
@group(1) @binding(19) var terrain_detail_normal: texture_2d_array<f32>;

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
fn terrain_normal_sample(l: Lookup, n: vec3<f32>, layer: i32, relief: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        if l.weight[i] > 0.0 {
            // X and Y with Z rebuilt (scene.wgsl decode_normal): a BC5 layer map keeps no Z.
            var tn = decode_normal(textureSampleGrad(terrain_normal, terrain_smp, l.uv[i], layer, l.dx[i], l.dy[i]));
            tn = vec3<f32>(tn.xy * relief, tn.z);
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

// ---- detail textures ----

// The whiteout blend (as terrain_normal_sample) of a tangent-space normal onto `n` in the
// projection's frame (t image right, b image up): with a flat `tn` it gives `n` back.
fn whiteout(tn: vec3<f32>, n: vec3<f32>, t: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let axis = cross(t, b);
    let base = vec3<f32>(dot(n, t), dot(n, b), dot(n, axis));
    let w = vec3<f32>(tn.xy + base.xy, tn.z * base.z);
    return w.x * t + w.y * b + w.z * axis;
}

// The layers' detail textures (terrain.set_layer_detail) over what the layers drew (`s` so far):
// `blend` is each layer's height-blend weight from the layer loop and `k` its normaliser, so a
// layer's share is blend * k. A detail weighs strength x share x (1 - smoothstep(0.6 fade, fade,
// distance to the eye)), as Unreal's landscape detail textures and Unity's terrain detail maps
// fade. The map is taken relative to its own mean (the array's top mip): the layer keeps its
// brightness whatever map is chosen, and only the map's detail modulates the colour (0..4 times).
// Steep ground projects it triplanar like the layer's own maps, so it does not stretch, and the
// detail normal perturbs the shaded normal by the whiteout blend, its slope scaled by the total
// weight. Nothing runs when no layer has a detail or the eye is past the farthest fade: with
// explicit gradients and levels the samples need no derivatives, so the branches may be as they
// are. A project without a detail shades exactly as before.
fn terrain_detail(
    world: vec3<f32>,
    dwx: vec3<f32>,
    dwy: vec3<f32>,
    n: vec3<f32>,
    steep: bool,
    blend: array<f32, 8>,
    k: f32,
    s: ptr<function, Surface>,
) {
    if terrain.detail.x < 0.5 {
        return;
    }
    let dist = length(to_eye(world));
    if dist >= terrain.detail.y {
        return;
    }
    var amount: array<f32, 8>;
    var total = 0.0;
    for (var i = 0; i < 8; i++) {
        let d = terrain.layers[i].detail;
        if blend[i] > 0.0 && d.w >= 1.0 {
            amount[i] = blend[i] * k * d.y * (1.0 - smoothstep(d.z * 0.6, d.z, dist));
            total += amount[i];
        }
    }
    if total < 1e-4 {
        return;
    }
    // The projections and their frames follow the normal, so they are the same for every layer.
    var frame = triplanar(world, dwx, dwy, n, 1.0);
    if !steep {
        frame.weight = vec3<f32>(0.0, 1.0, 0.0);
    }
    let mean_lod = f32(textureNumLevels(terrain_detail_base) - 1u);
    var factor = vec3<f32>(0.0);
    // The detail normals per projection, weighted, and the weights.
    var tn: array<vec3<f32>, 3>;
    var nw = vec3<f32>(0.0);
    for (var i = 0; i < 8; i++) {
        if amount[i] > 0.0 {
            let d = terrain.layers[i].detail;
            let l = triplanar(world, dwx, dwy, n, d.x);
            let mean = textureSampleLevel(terrain_detail_base, terrain_smp, vec2<f32>(0.5), i, mean_lod).rgb;
            for (var p = 0; p < 3; p++) {
                if frame.weight[p] > 0.0 {
                    let w = amount[i] * frame.weight[p];
                    let c = textureSampleGrad(terrain_detail_base, terrain_smp, l.uv[p], i, l.dx[p], l.dy[p]).rgb;
                    factor += w * clamp(c / max(mean, vec3<f32>(1e-3)), vec3<f32>(0.0), vec3<f32>(4.0));
                    if d.w >= 2.0 {
                        tn[p] += decode_normal(textureSampleGrad(terrain_detail_normal, terrain_smp, l.uv[p], i, l.dx[p], l.dy[p])) * w;
                        nw[p] += w;
                    }
                }
            }
        }
    }
    let a = min(total, 1.0);
    (*s).base = vec4<f32>((*s).base.rgb * mix(vec3<f32>(1.0), factor / total, a), (*s).base.a);
    var nsum = vec3<f32>(0.0);
    var perturbed = false;
    for (var p = 0; p < 3; p++) {
        if nw[p] > 0.0 {
            // The layers' mean detail normal, its slope scaled by their total weight here. `nw`
            // holds that weight times the projection's; the projection's is applied below.
            let scale = min(nw[p] / frame.weight[p], 1.0);
            let xy = (tn[p] / nw[p]).xy * scale;
            let d = vec3<f32>(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
            nsum += whiteout(d, (*s).n, frame.t[p], frame.b[p]) * frame.weight[p];
            perturbed = true;
        }
    }
    if perturbed {
        (*s).n = normalize(nsum);
    }
}

// ---- hex tiling ----

// Hexagon centres per texture repeat: a little under two, so a hexagon holds about half a tile.
const HEX_DENSITY: f32 = 1.8;
const SQRT3_2: f32 = 0.8660254;

// The three hexagon centres round `uv` (lattice vertices of equilateral triangles) and the
// barycentric weights of `uv` in their triangle.
struct HexCell {
    v: array<vec2<i32>, 3>,
    w: vec3<f32>,
}

fn hex_cell(uv: vec2<f32>) -> HexCell {
    let p = uv * HEX_DENSITY;
    // Into the lattice's skewed frame: p = i * (1, 0) + j * (1/2, sqrt(3)/2).
    let j = p.y / SQRT3_2;
    let i = p.x - 0.5 * j;
    let base = vec2<i32>(floor(vec2<f32>(i, j)));
    let f = fract(vec2<f32>(i, j));
    var c: HexCell;
    if f.x + f.y < 1.0 {
        c.v = array<vec2<i32>, 3>(base, base + vec2<i32>(1, 0), base + vec2<i32>(0, 1));
        c.w = vec3<f32>(1.0 - f.x - f.y, f.x, f.y);
    } else {
        c.v = array<vec2<i32>, 3>(base + vec2<i32>(1, 1), base + vec2<i32>(0, 1), base + vec2<i32>(1, 0));
        c.w = vec3<f32>(f.x + f.y - 1.0, 1.0 - f.x, 1.0 - f.y);
    }
    return c;
}

// A hexagon's turn (cos, sin) and shift, from its lattice vertex.
fn hex_turn(v: vec2<i32>) -> vec2<f32> {
    let a = hash3(vec3<i32>(v, 17)) * 6.2831853;
    return vec2<f32>(cos(a), sin(a));
}

fn hex_rotate(r: vec2<f32>, v: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(r.x * v.x - r.y * v.y, r.y * v.x + r.x * v.y);
}

// `uv` as hexagon `v` maps it: turned about the hexagon's centre and shifted.
fn hex_uv(v: vec2<i32>, r: vec2<f32>, uv: vec2<f32>) -> vec2<f32> {
    let centre = vec2<f32>(f32(v.x) + 0.5 * f32(v.y), f32(v.y) * SQRT3_2) / HEX_DENSITY;
    let shift = vec2<f32>(hash3(vec3<i32>(v, 3)), hash3(vec3<i32>(v, 5)));
    return hex_rotate(r, uv - centre) + centre + shift;
}

// The blend weights of the three copies: the barycentric ones raised to a power (narrow seams)
// and scaled by each copy's height, so the higher stones of one copy cover the other.
fn hex_weights(w: vec3<f32>, heights: vec3<f32>) -> vec3<f32> {
    let k = pow(w, vec3<f32>(6.0)) * mix(vec3<f32>(1.0), heights + 0.05, 0.75);
    return k / max(k.x + k.y + k.z, 1e-6);
}

// One layer's maps from above through the hex tiling: the packed ORM (height in alpha) and the
// three weights it gives.
struct HexOrm {
    orm: vec4<f32>,
    w: vec3<f32>,
}

fn hex_orm(l: Lookup, layer: i32) -> HexOrm {
    let c = hex_cell(l.uv[1]);
    var s: array<vec4<f32>, 3>;
    for (var k = 0; k < 3; k++) {
        let r = hex_turn(c.v[k]);
        s[k] = textureSampleGrad(terrain_orm, terrain_smp, hex_uv(c.v[k], r, l.uv[1]), layer, hex_rotate(r, l.dx[1]), hex_rotate(r, l.dy[1]));
    }
    var out: HexOrm;
    out.w = hex_weights(c.w, vec3<f32>(s[0].a, s[1].a, s[2].a));
    out.orm = s[0] * out.w.x + s[1] * out.w.y + s[2] * out.w.z;
    return out;
}

fn hex_base(l: Lookup, layer: i32, w: vec3<f32>) -> vec4<f32> {
    let c = hex_cell(l.uv[1]);
    var sum = vec4<f32>(0.0);
    for (var k = 0; k < 3; k++) {
        if w[k] > 0.001 {
            let r = hex_turn(c.v[k]);
            sum += textureSampleGrad(terrain_base, terrain_smp, hex_uv(c.v[k], r, l.uv[1]), layer, hex_rotate(r, l.dx[1]), hex_rotate(r, l.dy[1])) * w[k];
        }
    }
    return sum;
}

// The normal from above through the hex tiling: each copy's tangent normal turned back by its
// hexagon's angle (the map was sampled turned by it), blended, then whiteout-blended onto `n` in
// the top projection's frame (as terrain_normal_sample).
fn hex_normal(l: Lookup, n: vec3<f32>, layer: i32, relief: f32, w: vec3<f32>) -> vec3<f32> {
    let c = hex_cell(l.uv[1]);
    var tn = vec3<f32>(0.0);
    for (var k = 0; k < 3; k++) {
        if w[k] > 0.001 {
            let r = hex_turn(c.v[k]);
            let s = decode_normal(textureSampleGrad(terrain_normal, terrain_smp, hex_uv(c.v[k], r, l.uv[1]), layer, hex_rotate(r, l.dx[1]), hex_rotate(r, l.dy[1])));
            tn += vec3<f32>(hex_rotate(r, s.xy), s.z) * w[k];
        }
    }
    tn = vec3<f32>(tn.xy * relief, max(tn.z, 1e-3));
    let t = l.t[1];
    let b = l.b[1];
    let axis = cross(t, b);
    let base = vec3<f32>(dot(n, t), dot(n, b), dot(n, axis));
    let wo = vec3<f32>(tn.xy + base.xy, tn.z * base.z);
    return wo.x * t + wo.y * b + wo.z * axis;
}

// One layer's projection: planar from above unless the slope is steep.
fn layer_lookup(world: vec3<f32>, dwx: vec3<f32>, dwy: vec3<f32>, n: vec3<f32>, inv_scale: f32, steep: bool) -> Lookup {
    var l = triplanar(world, dwx, dwy, n, inv_scale);
    if !steep {
        l.weight = vec3<f32>(0.0, 1.0, 0.0);
    }
    return l;
}

fn terrain_surface(in: TerrainVarying) -> Surface {
    let n = normalize(in.normal);
    let dwx = dpdx(in.world);
    let dwy = dpdy(in.world);
    var weights = array<f32, 8>(in.w0.x, in.w0.y, in.w0.z, in.w0.w, in.w1.x, in.w1.y, in.w1.z, in.w1.w);
    let steep = abs(n.y) < 0.7;
    // The lookups are made again in the second loop: kept in an array of eight (over 300
    // floats) they spilled out of registers on Intel GPUs, which cost more than the arithmetic.
    var orm: array<vec4<f32>, 8>;
    // The hex tiling's copy weights per layer (from above only: steep ground keeps triplanar).
    var hexw: array<vec3<f32>, 8>;
    var top = 0.0;
    for (var i = 0; i < 8; i++) {
        if weights[i] > 0.004 {
            let l = layer_lookup(in.world, dwx, dwy, n, terrain.layers[i].params.y, steep);
            if !steep && terrain.layers[i].extra.y > 0.5 {
                let h = hex_orm(l, i);
                orm[i] = h.orm;
                hexw[i] = h.w;
            } else {
                orm[i] = terrain_sample(terrain_orm, l, i);
            }
            top = max(top, orm[i].a + weights[i]);
        }
    }
    var s: Surface;
    var base = vec3<f32>(0.0);
    var nsum = vec3<f32>(0.0);
    var rough = 0.0;
    var ao = 0.0;
    var metal = 0.0;
    var variation = 0.0;
    var puddles = 0.0;
    var total = 0.0;
    // Each layer's height-blend weight, for its detail texture (terrain_detail).
    var blend: array<f32, 8>;
    for (var i = 0; i < 8; i++) {
        if weights[i] > 0.004 {
            let b = max(orm[i].a + weights[i] - (top - HEIGHT_DEPTH), 0.0);
            if b > 0.0 {
                blend[i] = b;
                let layer = terrain.layers[i];
                let l = layer_lookup(in.world, dwx, dwy, n, layer.params.y, steep);
                if !steep && layer.extra.y > 0.5 {
                    base += hex_base(l, i, hexw[i]).rgb * layer.color.rgb * b;
                    nsum += hex_normal(l, n, i, layer.params.w, hexw[i]) * b;
                } else {
                    base += terrain_sample(terrain_base, l, i).rgb * layer.color.rgb * b;
                    nsum += terrain_normal_sample(l, n, i, layer.params.w) * b;
                }
                rough += (layer.params.x * orm[i].g + layer.color.a) * b;
                metal += orm[i].b * b;
                ao += orm[i].r * b;
                variation += layer.params.z * b;
                puddles += layer.extra.x * b;
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
    s.puddle = puddles * k;
    terrain_detail(in.world, dwx, dwy, n, steep, blend, k, &s);
    s = macro_variation(s, in.world, variation * k);
    // The outer band blends into the infinite ground: no border.
    if terrain.ground.w > 0.0 {
        let e = terrain.edge;
        let d = min(min(in.world.x - e.x, e.z - in.world.x), min(in.world.z - e.y, e.w - in.world.z));
        let t = 1.0 - smoothstep(0.0, terrain.ground.w, d);
        s.base = vec4<f32>(mix(s.base.rgb, terrain.ground.rgb, t), 1.0);
        s.n = normalize(mix(s.n, vec3<f32>(0.0, 1.0, 0.0), t));
        s.puddle *= 1.0 - t;
    }
    return s;
}

fn terrain_varying(in: TerrainVarying) -> Varying {
    return Varying(in.clip, in.world, in.normal, in.face, in.uv, 0u, 0u);
}

@fragment
fn fs_terrain(in: TerrainVarying) -> @location(0) vec4<f32> {
    if scratch_floor_taken() {
        return scratch_floor(in.clip);
    }
    return vec4<f32>(light(terrain_varying(in), terrain_surface(in), false, 0.0, NO_AGING, true).color, 1.0);
}

@fragment
fn fs_terrain_reflect(in: TerrainVarying) -> @location(0) vec4<f32> {
    if scratch_floor_taken() {
        return scratch_floor(in.clip);
    }
    let shaded = light(terrain_varying(in), terrain_surface(in), false, 0.0, NO_AGING, false);
    if g.reflection.z > 0.5 && in.world.y < g.reflection.w {
        discard;
    }
    return vec4<f32>(shaded.color, 1.0);
}
