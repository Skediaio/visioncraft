// Generates this frame's grass blades (grass.rs) from the grass mask, after "Procedural Grass in
// Ghost of Tsushima" (E. Wohllaib, GDC 2021): a grid of tiles round the eye, one workgroup per
// tile; the tiles out of reach or out of view stop at once. A tile's blades are the first ones of
// an R2 low-discrepancy sequence (M. Roberts, "The Unreasonable Effectiveness of Quasirandom
// Sequences", 2018) from the tile's own offset, so any prefix of it covers the tile evenly: the
// density falls with the distance by keeping a shorter prefix, and the blade at the prefix's end
// shrinks into the ground as the eye moves away (no rings of popping blades). Each blade takes
// its height, width, bend and facing from hashes, and from its clump (the nearest of jittered
// points, a Voronoi cell): the blades of a clump share a height and lean out of its centre. Fewer
// blades further off are wider, so the grass covers the ground as densely seen from afar.
//
// Module: grass_common.wgsl + this file.

@group(1) @binding(41) var grass_params: texture_2d<f32>;  // rgba8: density, height, flowers, palette entry
@group(1) @binding(42) var grass_ground: texture_2d<f32>;  // r32float: the ground's height (m)
@group(1) @binding(43) var grass_sampler: sampler;

struct GrassBlade {
    root: vec4<f32>,
    data: vec4<u32>,
}
@group(1) @binding(44) var<storage, read_write> blades: array<GrassBlade>;

// The three lists' indirect draws: near blades, far blades, flowers.
struct GrassArgs {
    vertex_count: u32,
    instance_count: atomic<u32>,
    first_vertex: u32,
    first_instance: u32,
}
@group(1) @binding(45) var<storage, read_write> args: array<GrassArgs, 3>;

@compute @workgroup_size(1)
fn cs_grass_reset() {
    for (var i = 0u; i < 3u; i++) {
        args[i].vertex_count = grass.verts[i];
        atomicStore(&args[i].instance_count, 0u);
        args[i].first_vertex = 0u;
        args[i].first_instance = 0u;
    }
}

@compute @workgroup_size(1)
fn cs_grass_finish() {
    for (var i = 0u; i < 3u; i++) {
        atomicStore(&args[i].instance_count, min(atomicLoad(&args[i].instance_count), grass.caps[i]));
    }
}

// The share of the full density kept at distance `d`: all of it near, then falling as 1 / d (and
// the blades widening as d), so the blades' cover of the ground stays the same.
fn grass_share(d: f32) -> f32 {
    return min(1.0, grass.lod.y / max(d, 1e-3));
}

// The ground's height at `c` (cells, continuous): bilinear over the four cells round it.
fn grass_ground_at(c: vec2<f32>) -> f32 {
    let size = vec2<i32>(grass.field_size.xy);
    let p = c - 0.5;
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let a = textureLoad(grass_ground, clamp(i, vec2<i32>(0), size - 1), 0).r;
    let b = textureLoad(grass_ground, clamp(i + vec2<i32>(1, 0), vec2<i32>(0), size - 1), 0).r;
    let e = textureLoad(grass_ground, clamp(i + vec2<i32>(0, 1), vec2<i32>(0), size - 1), 0).r;
    let d = textureLoad(grass_ground, clamp(i + vec2<i32>(1, 1), vec2<i32>(0), size - 1), 0).r;
    return mix(mix(a, b, f.x), mix(e, d, f.x), f.y);
}

fn grass_outside(center: vec3<f32>, radius: f32) -> bool {
    for (var k = 0; k < 6; k++) {
        let p = grass.planes[k];
        if dot(p.xyz, center) + p.w < -radius {
            return true;
        }
    }
    return false;
}

fn grass_seed2(c: vec2<i32>, salt: u32) -> u32 {
    return grass_pcg(bitcast<u32>(c.x) * 73856093u ^ bitcast<u32>(c.y) * 19349663u ^ salt);
}

@compute @workgroup_size(64)
fn cs_grass(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tile = grass.tiles.xy + vec2<i32>(wg.xy);
    let lo = vec2<f32>(tile) * GRASS_TILE;
    let hi = lo + GRASS_TILE;
    let eye = grass.eye.xyz;
    let f_lo = grass.field.xy;
    let f_hi = f_lo + grass.field_size.xy * grass.field.w;
    if any(hi < f_lo) || any(lo > f_hi) {
        return;
    }
    let y_lo = grass.field_size.z;
    let y_hi = grass.field_size.w + GRASS_MAX_HEIGHT * 1.4;
    let near_xz = clamp(eye.xz, lo, hi);
    let dy = max(max(y_lo - eye.y, eye.y - y_hi), 0.0);
    let d_min = length(vec3<f32>(near_xz.x - eye.x, dy, near_xz.y - eye.z));
    if d_min > grass.eye.w {
        return;
    }
    // The tile's box against the frustum: its corner furthest along each plane's normal.
    for (var k = 0; k < 6; k++) {
        let p = grass.planes[k];
        let c = vec3<f32>(select(lo.x, hi.x, p.x > 0.0), select(y_lo, y_hi, p.y > 0.0), select(lo.y, hi.y, p.z > 0.0));
        if dot(p.xyz, c) + p.w < 0.0 {
            return;
        }
    }
    let per_tile = grass.lod.x * GRASS_TILE * GRASS_TILE;
    let n_max = u32(ceil(per_tile * grass_share(d_min)));
    let tile_seed = grass_seed2(tile, 0x2545F491u);
    let offset = vec2<f32>(grass_unit(tile_seed), grass_unit(grass_pcg(tile_seed)));
    let size = grass.field_size.xy;
    // The R2 points lie close to a lattice, which showed as rows of blades: each is moved by up
    // to half the mean spacing of the tile's blades.
    let spacing = GRASS_TILE / sqrt(max(f32(n_max), 1.0));
    for (var i = li; i < n_max; i += 64u) {
        let r2 = fract(offset + f32(i) * vec2<f32>(0.7548777, 0.5698403));
        let hj = grass_pcg(tile_seed ^ (i * 0x68E31DA4u) ^ 0xB5297A4Du);
        let jitter = vec2<f32>(grass_unit(hj), grass_unit(grass_pcg(hj))) - 0.5;
        let xz = lo + r2 * GRASS_TILE + jitter * spacing;
        let cells = (xz - f_lo) * grass.field.z;
        if any(cells < vec2<f32>(0.0)) || any(cells >= size) {
            continue;
        }
        let m = textureSampleLevel(grass_params, grass_sampler, cells / size, 0.0);
        let bh = grass_height(m.g);
        let h0 = grass_pcg(tile_seed ^ (i * 0x9E3779B9u));
        let h1 = grass_pcg(h0);
        let h2 = grass_pcg(h1);
        let h3 = grass_pcg(h2);
        // Taller grass has fewer, larger blades.
        let share = m.r * clamp(0.08 / max(bh, 0.01), 0.5, 1.0);
        if bh < 0.005 || grass_unit(h0) >= share {
            continue;
        }
        let root = vec3<f32>(xz.x, grass_ground_at(cells), xz.y);
        let d = distance(root, eye);
        var presence = clamp(per_tile * grass_share(d) - f32(i), 0.0, 1.0);
        // Out to the fade radius, further for taller grass, sinking into the lawn's texture.
        let reach = min(grass.lod.w * clamp(0.4 + 3.0 * bh, 0.55, 1.6), grass.eye.w);
        presence *= 1.0 - smoothstep(reach * 0.65, reach, d);
        if presence <= 0.01 {
            continue;
        }
        let kind = u32(round(textureLoad(grass_params, vec2<i32>(cells), 0).a * 255.0)) & 7u;
        let k0 = grass.kinds[kind * 2u];
        let k1 = grass.kinds[kind * 2u + 1u];

        // The clump: the nearest of the jittered points of cells about a blade's height wide.
        let cs = clamp(bh * 0.9, 0.12, 0.45);
        let cp = xz / cs;
        let ci = vec2<i32>(floor(cp));
        var best = 1e9;
        var centre = vec2<f32>(0.0);
        var clump = 0u;
        for (var dz = -1; dz <= 1; dz++) {
            for (var dx = -1; dx <= 1; dx++) {
                let c = ci + vec2<i32>(dx, dz);
                let hc = grass_seed2(c, 0x51ED27u);
                let pt = vec2<f32>(c) + 0.1 + 0.8 * vec2<f32>(grass_unit(hc), grass_unit(grass_pcg(hc)));
                let dd = dot(pt - cp, pt - cp);
                if dd < best {
                    best = dd;
                    centre = pt;
                    clump = hc;
                }
            }
        }
        let hk = grass_pcg(clump);
        let clump_height = 0.7 + 0.6 * grass_unit(hk);
        let clump_angle = grass_unit(grass_pcg(hk)) * GRASS_TAU;
        let out = (cp - centre) * cs; // from the clump's centre to the blade (m)
        let out_len = length(out);

        // Facing: its own, the clump's, and away from the clump's centre.
        let own = grass_unit(h1) * GRASS_TAU;
        var dir = vec2<f32>(cos(own), sin(own)) * 0.6 + vec2<f32>(cos(clump_angle), sin(clump_angle)) * 0.35;
        dir += out / max(out_len, 1e-4) * clamp(out_len / cs, 0.0, 1.0) * 0.8;
        var bend = 0.12 + 0.32 * grass_unit(h2) + 0.35 * clamp(out_len / cs, 0.0, 1.0) + 0.3 * smoothstep(0.1, 0.6, bh);
        // Mowing stripes: the blades of each pass lean the way the mower went.
        if k0.w > 0.0 {
            let along = k1.xy;
            let across = vec2<f32>(-along.y, along.x);
            let band = i32(floor(dot(xz, across) / k0.w));
            let pass_sign = select(-1.0, 1.0, (band & 1) == 0);
            dir = dir * 0.35 + along * pass_sign;
            bend += 0.25;
        }
        let angle = atan2(dir.y, dir.x);

        var height = bh * 1.3 * clump_height * (0.75 + 0.5 * grass_unit(h3)) * presence;
        let widen = clamp(1.0 / grass_share(d), 1.0, 8.0);
        var width = mix(0.0045, 0.009, smoothstep(0.03, 0.5, bh)) * widen;
        let h4 = grass_pcg(h3);
        let h5 = grass_pcg(h4);
        var kind_of = 0u;
        if bh > 0.15 && grass_unit(h4) < 0.18 {
            // A seed stem, over the leaves of a meadow.
            kind_of = 1u;
            height *= 1.15;
            width *= 0.45;
            bend *= 0.5;
        }
        if m.b > 0.0 && grass_unit(h5) < m.b * 0.06 {
            // A flower on a thin stem, over the grass round it.
            kind_of = 2u;
            height = bh * (1.3 + 0.4 * grass_unit(h3)) * presence;
            width = 0.002 * widen;
            bend *= 0.3;
        }
        if grass_outside(root + vec3<f32>(0.0, height * 0.5, 0.0), height * 0.7 + width + 0.05) {
            continue;
        }
        let list = select(select(1u, 0u, d < grass.lod.z), 2u, kind_of == 2u);
        let slot = atomicAdd(&args[list].instance_count, 1u);
        if slot >= grass.caps[list] {
            continue;
        }
        var base = 0u;
        if list >= 1u {
            base += grass.caps[0];
        }
        if list == 2u {
            base += grass.caps[1];
        }
        let h6 = grass_pcg(h5);
        let variation = clamp(0.5 * grass_unit(hk ^ 0xA511E9B3u) + 0.5 * grass_unit(h6), 0.0, 1.0);
        let dry = select(0.0, 1.0, grass_unit(grass_pcg(h6)) < k1.z);
        // The sward's floor sees little sky: less in denser and taller grass.
        let ao_root = mix(0.5, 0.18, clamp(m.r * smoothstep(0.02, 0.45, bh) + 0.35 * m.r, 0.0, 1.0));
        var b: GrassBlade;
        b.root = vec4<f32>(root, height);
        b.data = vec4<u32>(
            pack2x16unorm(vec2<f32>(width / 0.1, bend / 2.0)),
            (u32(fract(angle / GRASS_TAU) * 65535.0) & 0xFFFFu) | (h6 & 0xFFFF0000u),
            pack4x8unorm(vec4<f32>(variation, dry, ao_root, f32(kind) / 255.0)),
            pack4x8unorm(vec4<f32>(f32(h6 & 3u) / 255.0, f32(kind_of) / 255.0, widen / 8.0, 0.0)),
        );
        blades[base + slot] = b;
    }
}
