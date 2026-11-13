// Impostors: a plant baked from a ring of horizontal view directions plus
// one from above into an atlas (unlit base colour + alpha, and model-space
// normal), drawn far away as one camera-facing quad per instance, lit per
// pixel by the same `light` as meshes with the baked normals. Views are
// chosen per instance from the direction to the viewer in the plant's own
// frame: the two nearest ring views are blended (colour, normal and alpha
// mixed before the alpha test, so the silhouette morphs instead of
// popping), and the top view is blended in as the viewer climbs above 45
// degrees. The ring-plus-top layout is the simple variant of the atlas
// impostors in Brucks, "Octahedral Impostors" (Epic Games / Shaderbits,
// 2018); a hemi-octahedral layout would add elevation steps and was not
// needed for eye-level gardens. Module: common.wgsl + scene.wgsl + this
// file (group 1 bindings 20-23 never collide with the material's 0-12).

struct ImpostorInfo {
    centre: vec4<f32>, // xyz model-space centre of the bounds
    grid: vec4<f32>,   // x ring views, y atlas columns, z 1 / columns, w alpha cutoff
    extent: vec4<f32>, // x horizontal radius, y half height (model units)
}
@group(1) @binding(20) var<uniform> impostor: ImpostorInfo;
@group(1) @binding(21) var impostor_albedo: texture_2d<f32>;
@group(1) @binding(22) var impostor_normal: texture_2d<f32>;
@group(1) @binding(23) var impostor_smp: sampler;

struct ImpostorIn {
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
}

struct ImpostorVarying {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) st: vec2<f32>,                          // quad position, -1..1
    @location(2) @interpolate(flat) right: vec3<f32>,    // quad axes in model space
    @location(3) @interpolate(flat) up: vec3<f32>,
    @location(4) @interpolate(flat) views: vec4<f32>,    // ring view a, b, blend a->b, top weight
    @location(5) @interpolate(flat) n0: vec3<f32>,       // model -> world normal matrix columns
    @location(6) @interpolate(flat) n1: vec3<f32>,
    @location(7) @interpolate(flat) n2: vec3<f32>,
    @location(8) @interpolate(flat) fade: f32,           // 0 mesh .. 1 impostor
    @location(9) @interpolate(flat) ext: vec2<f32>,      // quad half width, height (model units)
}

// Which ring views see the plant from model-space direction `d` (towards
// the viewer): views k at azimuth 2 pi k / n, d_k = (sin, 0, cos). lib.rs
// has the same function (`impostor_views`), unit-tested.
fn impostor_views(d: vec3<f32>) -> vec4<f32> {
    let n = impostor.grid.x;
    let a = atan2(d.x, d.z) / (2.0 * PI);
    let f = fract(a + 1.0) * n;
    let i0 = floor(f);
    let i1 = (i0 + 1.0) % n;
    let elevation = asin(clamp(d.y / max(length(d), 1e-6), -1.0, 1.0));
    let top = smoothstep(0.785, 1.22, elevation);
    return vec4<f32>(i0 % n, i1, f - i0, top);
}

// How far a dissolve from the last mesh level (0) to the impostor or to
// nothing (1) has run: per draw, from the draw slot (lib.rs `fade_slot`).
fn lod_fade() -> f32 {
    return bitcast<f32>(draw.mesh.z);
}

// The quad around an instance facing `towards` (world, unit, from the
// plant to the viewer or the sun).
fn impostor_vertex(vi: u32, m: mat4x4<f32>, towards: vec3<f32>) -> ImpostorVarying {
    var out: ImpostorVarying;
    let corner = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let st = corner[vi % 6u];
    let scale = length(m[0].xyz);
    let centre = (m * vec4<f32>(impostor.centre.xyz, 1.0)).xyz;
    // Model space: undo rotation and scale of the instance.
    let inv = transpose(mat3x3<f32>(m[0].xyz / scale, m[1].xyz / scale, m[2].xyz / scale));
    let views = impostor_views(inv * towards);
    // The quad hugs the plant: wide by its horizontal radius, tall by its
    // half height from the side, growing to the radius seen from above.
    let ext = vec2<f32>(impostor.extent.x, mix(impostor.extent.y, impostor.extent.x, views.w));
    var right = cross(vec3<f32>(0.0, 1.0, 0.0), towards);
    if dot(right, right) < 1e-8 {
        right = vec3<f32>(1.0, 0.0, 0.0);
    }
    right = normalize(right);
    let up = cross(towards, right);
    var world = centre + (right * st.x * ext.x + up * st.y * ext.y) * scale;
    // Wind: the whole crown sways gently, the base stays.
    let sway = g.wind.z * 0.03 * ext.y * scale * (st.y + 1.0) * 0.5
        * sin(g.time.x * 0.9 + dot(m[3].xz, vec2<f32>(0.7548777, 0.5698403)) * 6.28);
    world += vec3<f32>(g.wind.x, 0.0, g.wind.y) * sway;
    out.world = world;
    out.ext = ext;
    out.st = st;
    out.right = inv * right;
    out.up = inv * up;
    out.views = views;
    let cof = mat3x3<f32>(cross(m[1].xyz, m[2].xyz), cross(m[2].xyz, m[0].xyz), cross(m[0].xyz, m[1].xyz));
    out.n0 = cof[0];
    out.n1 = cof[1];
    out.n2 = cof[2];
    return out;
}

fn viewer_towards(m: mat4x4<f32>) -> vec3<f32> {
    let centre = (m * vec4<f32>(impostor.centre.xyz, 1.0)).xyz;
    return normalize(to_eye(centre));
}

@vertex
fn vs_impostor(@builtin(vertex_index) vi: u32, v: ImpostorIn) -> ImpostorVarying {
    let m = model(v.m0, v.m1, v.m2, v.m3);
    var out = impostor_vertex(vi, m, viewer_towards(m));
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    out.fade = lod_fade();
    return out;
}

@vertex
fn vs_impostor_shadow(@builtin(vertex_index) vi: u32, v: ImpostorIn) -> ImpostorVarying {
    let m = model(v.m0, v.m1, v.m2, v.m3);
    var out = impostor_vertex(vi, m, g.sun_dir.xyz);
    out.clip = cascade.view_proj * vec4<f32>(out.world, 1.0);
    out.fade = 1.0;
    return out;
}

// Atlas texture coordinates of quad position `st` in tile `tile`: ring
// tiles are the ortho views (u right, v down); the top tile maps model x to
// u and model z to v, so the quad's own axes pick the point.
fn impostor_uv(in: ImpostorVarying, tile: f32, top: bool) -> vec2<f32> {
    // Model-space offset from the centre, then into the tile's extents.
    let e = impostor.extent;
    var local = vec2<f32>(in.st.x * in.ext.x / e.x * 0.5 + 0.5, 0.5 - in.st.y * in.ext.y / e.y * 0.5);
    if top {
        let p = (in.right * in.st.x * in.ext.x + in.up * in.st.y * in.ext.y) / e.x;
        local = vec2<f32>(p.x * 0.5 + 0.5, p.z * 0.5 + 0.5);
    }
    let cols = impostor.grid.y;
    let cell = vec2<f32>(tile % cols, floor(tile / cols));
    let inset = clamp(local, vec2<f32>(0.002), vec2<f32>(0.998));
    return (cell + inset) * impostor.grid.z;
}

struct ImpostorSample {
    albedo: vec4<f32>,
    normal: vec3<f32>,
}

// Mixed sample of the two ring views and the top view. Alpha gets the
// cutout's mip rescale (Golus 2017) as `cutout_alpha` does.
fn impostor_sample(in: ImpostorVarying) -> ImpostorSample {
    let uv0 = impostor_uv(in, in.views.x, false);
    let uv1 = impostor_uv(in, in.views.y, false);
    let uvt = impostor_uv(in, impostor.grid.x, true);
    let size = vec2<f32>(textureDimensions(impostor_albedo));
    let dx = dpdx(uv0 * size);
    let dy = dpdy(uv0 * size);
    let lod = max(0.0, 0.5 * log2(max(dot(dx, dx), dot(dy, dy))));
    var a = mix(textureSampleLevel(impostor_albedo, impostor_smp, uv0, lod), textureSampleLevel(impostor_albedo, impostor_smp, uv1, lod), in.views.z);
    var n = mix(textureSampleLevel(impostor_normal, impostor_smp, uv0, lod), textureSampleLevel(impostor_normal, impostor_smp, uv1, lod), in.views.z);
    if in.views.w > 0.0 {
        a = mix(a, textureSampleLevel(impostor_albedo, impostor_smp, uvt, lod), in.views.w);
        n = mix(n, textureSampleLevel(impostor_normal, impostor_smp, uvt, lod), in.views.w);
    }
    // Averaged mips of a sparse crown lose coverage faster than a leaf
    // texture: twice the cutout rescale keeps the silhouette.
    a.a *= 1.0 + lod * 0.5;
    return ImpostorSample(a, n.xyz * 2.0 - 1.0);
}

// Alpha only (the depth-only passes): the same mix as `impostor_sample`, or
// with `nearest` just the nearest view, one fetch (shadow maps: the sun's
// view of a plant need not morph, and these passes pay for every layer of
// a forest).
fn impostor_alpha(in: ImpostorVarying, nearest: bool) -> f32 {
    let uv0 = impostor_uv(in, in.views.x, false);
    let size = vec2<f32>(textureDimensions(impostor_albedo));
    let dx = dpdx(uv0 * size);
    let dy = dpdy(uv0 * size);
    let lod = max(0.0, 0.5 * log2(max(dot(dx, dx), dot(dy, dy))));
    var a: f32;
    if nearest {
        var uv = select(uv0, impostor_uv(in, in.views.y, false), in.views.z > 0.5);
        if in.views.w > 0.5 {
            uv = impostor_uv(in, impostor.grid.x, true);
        }
        a = textureSampleLevel(impostor_albedo, impostor_smp, uv, lod).a;
    } else {
        let uv1 = impostor_uv(in, in.views.y, false);
        a = mix(textureSampleLevel(impostor_albedo, impostor_smp, uv0, lod).a, textureSampleLevel(impostor_albedo, impostor_smp, uv1, lod).a, in.views.z);
        if in.views.w > 0.0 {
            let uvt = impostor_uv(in, impostor.grid.x, true);
            a = mix(a, textureSampleLevel(impostor_albedo, impostor_smp, uvt, lod).a, in.views.w);
        }
    }
    return a * (1.0 + lod * 0.5);
}

// The dithered hand-over: the impostor keeps the pixels the fading mesh
// (fs_depth_fade) gives up.
fn impostor_keep(in: ImpostorVarying, alpha: f32) -> bool {
    return alpha >= impostor.grid.w && ign(in.clip.xy) < in.fade;
}

@fragment
fn fs_impostor_depth(in: ImpostorVarying) {
    if !impostor_keep(in, impostor_alpha(in, false)) {
        discard;
    }
}

// With MSAA the hand-over splits each pixel's samples instead of the
// pixels: the mesh keeps round((1 - fade) * SAMPLES) of a run starting at a
// per-pixel offset, the impostor the rest, each also cut to its own alpha
// coverage (alpha to coverage, a second offset). Resolved, a dissolve and
// an impostor's silhouette are a blend, not a screen-door stipple.
// (mesh samples, impostor samples)
fn fade_split(clip: vec2<f32>, fade: f32) -> vec2<u32> {
    let start = u32(ign(clip) * f32(SAMPLES));
    let n = u32(round((1.0 - fade) * f32(SAMPLES)));
    return vec2<u32>(sample_run(n, start), sample_run(SAMPLES - n, start + n));
}

fn alpha_mask(clip: vec2<f32>, a: f32, cutoff: f32) -> u32 {
    return sample_run(alpha_samples(a, cutoff), u32(ign(clip + vec2<f32>(17.0, 29.0)) * f32(SAMPLES)));
}

@fragment
fn fs_impostor_depth_ms(in: ImpostorVarying) -> @builtin(sample_mask) u32 {
    let a = impostor_alpha(in, false);
    let m = fade_split(in.clip.xy, in.fade).y & alpha_mask(in.clip.xy, a, impostor.grid.w);
    if m == 0u {
        discard;
    }
    return m;
}

@fragment
fn fs_impostor_shadow(in: ImpostorVarying) {
    if impostor_alpha(in, true) < impostor.grid.w {
        discard;
    }
}

fn impostor_shade(in: ImpostorVarying, screen_effects: bool) -> vec3<f32> {
    let t = impostor_sample(in);
    let n_world = normalize(mat3x3<f32>(in.n0, in.n1, in.n2) * t.normal);
    var s: Surface;
    s.base = vec4<f32>(t.albedo.rgb, 1.0);
    s.n = n_world;
    s.geometric = n_world;
    s.roughness = 0.7;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.emissive = vec3<f32>(0.0);
    let v = Varying(in.clip, in.world, n_world, 0xfffffffdu, vec2<f32>(0.0));
    return light(v, s, true, 0.0, screen_effects).color;
}

// Forward: drawn with an Equal depth test, the pre-pass decided coverage.
@fragment
fn fs_impostor(in: ImpostorVarying) -> @location(0) vec4<f32> {
    return vec4<f32>(impostor_shade(in, true), 1.0);
}

@fragment
fn fs_impostor_reflect(in: ImpostorVarying) -> @location(0) vec4<f32> {
    let c = impostor_shade(in, false);
    let a = impostor_sample(in).albedo.a;
    if a < impostor.grid.w || (g.reflection.z > 0.5 && in.world.y < g.reflection.w) {
        discard;
    }
    return vec4<f32>(c, 1.0);
}

// ---- the mesh side of the hand-over -------------------------------------

// The last mesh level inside the fade band: alpha test as usual (when the
// material is a cutout) and give up the pixels the impostor takes.
struct FadeOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) fade: f32,
}

@vertex
fn vs_depth_fade(v: DepthIn) -> FadeOut {
    let m = model(v.m0, v.m1, v.m2, v.m3);
    let world = animated_world(v.position, v.wind, m, true);
    return FadeOut(g.view_proj * vec4<f32>(world, 1.0), v.uv, lod_fade());
}

@fragment
fn fs_depth_fade(in: FadeOut) {
    let a = cutout_alpha(in.uv);
    if (material.alpha.x > 0.0 && a < material.alpha.x) || ign(in.clip.xy) < in.fade {
        discard;
    }
}

@fragment
fn fs_depth_fade_ms(in: FadeOut) -> @builtin(sample_mask) u32 {
    var m = fade_split(in.clip.xy, in.fade).x;
    if material.alpha.x > 0.0 {
        m &= alpha_mask(in.clip.xy, cutout_alpha(in.uv), material.alpha.x);
    }
    if m == 0u {
        discard;
    }
    return m;
}

// ---- baking -------------------------------------------------------------

struct BakeOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
}

@vertex
fn vs_bake(v: VertexIn) -> BakeOut {
    var out: BakeOut;
    out.clip = cascade.view_proj * vec4<f32>(v.position, 1.0);
    out.uv = v.uv;
    out.normal = v.normal;
    return out;
}

struct BakeTargets {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
}

@fragment
fn fs_bake(in: BakeOut, @builtin(front_facing) front: bool) -> BakeTargets {
    var base = material.base_color;
    if (material.flags.x & HAS_BASE) != 0u {
        base *= textureSample(base_tex, base_smp, in.uv);
    }
    if material.alpha.x > 0.0 && base.a < material.alpha.x {
        discard;
    }
    var n = normalize(in.normal);
    if material.flags.y != 0u && material.alpha.w == 0.0 && !front {
        n = -n;
    }
    return BakeTargets(vec4<f32>(base.rgb, 1.0), vec4<f32>(n * 0.5 + 0.5, 1.0));
}
