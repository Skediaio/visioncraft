// Draws the GPU grass's blades (grass.rs, made by grass_compute.wgsl): each blade a strip along
// a quadratic Bezier curve from its foot through a control point over it to its tip, bent by the
// shared wind and gust field (common.wgsl `wind_sway`, as the lawn tufts), narrowing to a point;
// its normals are tilted across the width so a flat strip shades as a rounded blade, and a blade
// seen edge-on turns part of the way to the eye so it never thins to nothing ("Procedural Grass
// in Ghost of Tsushima", E. Wohllaib, GDC 2021). A flower adds a small round head at the top.
// Lit as leaves (`grass_light`: the sun's shadow, the baked sky visibility, light through the
// blade when backlit), with a colour from root to tip and the sward's own occlusion low down.
// The depth pre-pass and the forward pass compute the same positions (`@invariant`).
//
// Module: the scene module's parts + grass_common.wgsl + this file.

// Rows of a blade (segments) in this pipeline's list, and whether it draws flowers.
override GRASS_SEGMENTS: u32 = 4u;
override GRASS_FLOWER: bool = false;

struct GrassIn {
    @builtin(vertex_index) vi: u32,
    @location(0) root: vec4<f32>,
    @location(1) data: vec4<u32>,
}

struct GrassVarying {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) shade: vec2<f32>, // x occlusion, y along the blade (0 foot, 1 tip; 2 a flower's head)
}

struct GrassPoint {
    world: vec3<f32>,
    normal: vec3<f32>,
    t: f32,
}

fn grass_bezier(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, t: f32) -> vec3<f32> {
    let s = 1.0 - t;
    return s * s * a + 2.0 * s * t * b + t * t * c;
}

fn grass_point(vi: u32, root4: vec4<f32>, data: vec4<u32>) -> GrassPoint {
    let root = root4.xyz;
    let h = root4.w;
    let wb = unpack2x16unorm(data.x) * vec2<f32>(0.1, 2.0);
    let facing = f32(data.y & 0xFFFFu) / 65535.0 * GRASS_TAU;
    let seed = data.y >> 16u;
    let misc = unpack4x8unorm(data.w);
    let flower = GRASS_FLOWER && misc.y * 255.0 > 1.5;
    let row = vi / 2u;
    let side = select(-1.0, 1.0, (vi & 1u) == 1u);
    let lean = vec3<f32>(cos(facing), 0.0, sin(facing));
    var across = vec3<f32>(-lean.z, 0.0, lean.x);
    // Seen edge-on (looking along `across`), the blade turns part of the way to face the eye.
    let to_eye = g.camera.xz - root.xz;
    let e = to_eye / max(length(to_eye), 1e-4);
    let edge_on = abs(dot(e, across.xz));
    var facing_eye = vec3<f32>(-e.y, 0.0, e.x);
    facing_eye *= select(-1.0, 1.0, dot(facing_eye, across) >= 0.0);
    across = normalize(mix(across, facing_eye, 0.6 * edge_on * edge_on));

    let up = vec3<f32>(0.0, 1.0, 0.0);
    let bend = wb.y;
    let rel0 = (up * cos(bend) + lean * sin(bend)) * h;
    var tip = root + rel0;
    var mid = root + up * (h * 0.6) + lean * (h * sin(bend) * 0.15);
    let ws = wind_state();
    if ws.amp.x > 0.0 {
        let hz = -(0.8 + 0.6 * f32(seed & 255u) / 255.0);
        let q = wind_sway(ws, rel0, root, 1.0, f32(seed) / 65535.0, hz);
        tip = root + q;
        mid += (q - rel0) * 0.4;
    }
    var out: GrassPoint;
    let segs = f32(GRASS_SEGMENTS);
    if row <= GRASS_SEGMENTS {
        let t = f32(row) / segs;
        let p = grass_bezier(root, mid, tip, t);
        let tangent = normalize(mix(mid - root, tip - mid, t) + vec3<f32>(0.0, 1e-5, 0.0));
        var w = wb.x * (1.0 - pow(t, 1.6));
        if flower {
            w = wb.x;
            if row == GRASS_SEGMENTS {
                w = 0.0;
            }
        }
        out.world = p + across * (side * 0.5 * w);
        let n = normalize(cross(tangent, across));
        // A rounded blade: the normals lean out across the width.
        out.normal = normalize(n + across * (side * 0.45));
        out.t = t;
    } else {
        // A flower's head: a small flat disc of petals over the stem's top.
        let top = tip;
        let widen = misc.z * 8.0;
        let r = (0.007 + 0.008 * f32((seed >> 8u) & 255u) / 255.0) * sqrt(max(widen, 1.0)) * select(0.0, 1.0, flower);
        // Turned up towards the sky and the sun, a little to one side.
        let fwd = normalize(vec3<f32>(lean.x, 0.8, lean.z));
        // Four rows over the disc from the stem's top across to its far side, each as wide as
        // the circle there: a round head, not a diamond.
        let k = f32(row - GRASS_SEGMENTS) * 0.5;
        let half = sqrt(max(1.0 - (k - 1.0) * (k - 1.0), 0.0)) * r;
        out.world = top + fwd * (k * r) + across * (side * half);
        out.normal = normalize(cross(across, fwd));
        out.t = 2.0;
    }
    return out;
}

struct GrassDepth {
    @builtin(position) @invariant clip: vec4<f32>,
}

@vertex
fn vs_grass_depth(v: GrassIn) -> GrassDepth {
    let p = grass_point(v.vi, v.root, v.data);
    return GrassDepth(g.view_proj * vec4<f32>(p.world, 1.0));
}

@vertex
fn vs_grass(v: GrassIn) -> GrassVarying {
    let p = grass_point(v.vi, v.root, v.data);
    var out: GrassVarying;
    out.clip = g.view_proj * vec4<f32>(p.world, 1.0);
    out.world = p.world;
    out.normal = p.normal;
    let look = unpack4x8unorm(v.data.z);
    let misc = unpack4x8unorm(v.data.w);
    let kind = u32(round(look.w * 255.0)) & 7u;
    let c = grass.kinds[kind * 2u].rgb;
    let t = min(p.t, 1.0);
    // Paler and yellower at the foot, where the blade grows out of its sheath, a fresher green
    // further up; each blade and clump a little lighter or darker, yellower or bluer.
    let v0 = look.x * 2.0 - 1.0;
    var col = mix(c * vec3<f32>(0.78, 0.74, 0.5), c * vec3<f32>(1.06, 1.05, 0.9), smoothstep(0.0, 0.8, t));
    col *= vec3<f32>(1.0 + 0.16 * v0, 1.0 + 0.1 * v0, 1.0 - 0.12 * v0) * (0.9 + 0.2 * fract(look.x * 7.3));
    // Grass is a duller green than the lawn's colour says once the blades stand in it.
    col = mix(vec3<f32>(luminance(col)), col, 0.85);
    // A dead blade among the green ones: straw towards its tip.
    let straw = vec3<f32>(0.3, 0.25, 0.12);
    col = mix(col, straw, look.y * smoothstep(0.2, 0.9, t) * 0.85);
    let kind_of = u32(round(misc.y * 255.0));
    if kind_of == 1u {
        // A seed stem: a brownish, purplish head.
        col = mix(col, vec3<f32>(0.2, 0.15, 0.09), smoothstep(0.7, 0.95, t));
    }
    var ao = mix(look.z, 1.0, pow(t, 0.8));
    if p.t > 1.5 {
        col = grass.flowers[u32(round(misc.x * 255.0)) & 3u].rgb * (0.8 + 0.4 * fract(look.x * 7.3));
        ao = 1.0;
    }
    out.color = col;
    out.shade = vec2<f32>(ao, p.t);
    return out;
}

// The blades' light: the leaf model of scene.wgsl `light` (foliage) cut down to what a blade
// shows, since the grass is most of the pixels of a lawn seen from eye height and was the most
// expensive thing in the frame with the full one: the sun with its shadow and the screen-space
// occlusion, light through the blade from behind (Barre-Brisebois & Bouchard, GDC 2011), the
// sky's light from both sides through the baked sky visibility, a faint sheen, the lamps and the
// air. No room volume, wetness, snow, mirror reflection or photo bounce (a lawn under deep snow
// has no blades; LOW draws tufts with all of it).
fn grass_light(clip: vec4<f32>, world: vec3<f32>, n: vec3<f32>, base: vec3<f32>, ao_in: f32) -> vec3<f32> {
    let clay = g.time.w > 0.5;
    let diffuse_color = select(base, vec3<f32>(0.8), clay);
    let ao_blade = select(ao_in, 1.0, clay);
    let v = normalize(to_eye(world));
    let l = g.sun_dir.xyz;
    let depth = -(g.view * vec4<f32>(world, 1.0)).z;
    let screen = upsample_occlusion(clip.xy, depth);
    let nol_raw = dot(n, l);
    let back = 1.0 - smoothstep(-0.1, 0.1, nol_raw * dot(n, v));
    let t_tint = select(LEAF_T_TINT, vec3<f32>(LEAF_T), clay);
    var sun = vec3<f32>(0.0);
    if g.sun_light.r + g.sun_light.g + g.sun_light.b > 0.0 {
        let shadow = min(sun_shadow(clip.xy, world, n, abs(nol_raw), depth), screen.g);
        let diffuse = mix(abs(nol_raw), 0.5, 0.5);
        let dc = diffuse_color * mix(vec3<f32>(1.0), t_tint / LEAF_T, back) * mix(1.0, LEAF_T, back);
        let lt = normalize(l + n * 0.3);
        let through = pow(clamp(dot(v, -lt), 0.0, 1.0), 4.0) * 0.35;
        // A faint sheen off the side facing the sun and the eye (Blinn-Phong, kept small).
        let h = normalize(l + v);
        let sheen = 0.02 * pow(max(dot(n, h), 0.0), 24.0) * step(0.0, nol_raw);
        sun = (dc / PI * diffuse + diffuse_color * t_tint * through + vec3<f32>(sheen))
            * g.sun_light.rgb * shadow * mix(1.0, ao_blade, 0.25);
    }
    var front = sh_irradiance(n);
    var rear = sh_irradiance(-n);
    let occ = sky_occ_at(world, n);
    if occ.weight > 0.0 {
        front = mix(front, sh_irradiance_seen(n, occ.v), occ.weight);
        rear = mix(rear, sh_irradiance_seen(-n, occ.v), occ.weight);
    }
    let ambient_ao = ao_blade * mix(1.0, screen.r, 0.5);
    let ambient = diffuse_color * (front + rear * LEAF_T_TINT) / PI * ambient_ao;
    let a = 0.5;
    let local = local_lights(world, n, v, max(abs(dot(n, v)), 0.35), diffuse_color, vec3<f32>(0.02), a, true);
    return apply_fog(sun + ambient + local, world);
}

@fragment
fn fs_grass(in: GrassVarying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if scratch_floor_taken() {
        return scratch_floor(in.clip);
    }
    var n = normalize(in.normal);
    if !front && in.shade.y < 1.5 {
        n = -n;
    }
    var c = in.color;
    // A gust bows the blades over: paler, glossier grass where it passes (scene.wgsl `gust_sheen`).
    if g.gust.z > 0.0 {
        let sheen = g.gust.z * gust_field(wind_state(), in.world.xz) * smoothstep(0.3, 1.0, min(in.shade.y, 1.0));
        c = mix(c, vec3<f32>(luminance(c)), 0.35 * sheen) * (1.0 + 1.1 * sheen);
    }
    return vec4<f32>(grass_light(in.clip, in.world, n, c, clamp(in.shade.x, 0.0, 1.0)), 1.0);
}
