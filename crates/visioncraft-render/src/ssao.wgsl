// Screen-space ambient occlusion and contact shadows, one pass, from the
// linear depth, at half resolution (or full: g.options.z is the divisor):
// each output texel takes the full-res pixel at its top-left (2x, 2y). Output: r = ambient occlusion, g = contact
// shadow (1 = lit). scene.wgsl upsamples it depth-aware.

@group(1) @binding(0) var depth_tex: texture_2d<f32>;

fn depth_at(p: vec2<i32>) -> f32 {
    let size = vec2<i32>(g.viewport.xy);
    return textureLoad(depth_tex, clamp(p, vec2<i32>(0), size - 1), 0).r;
}

fn position_at(p: vec2<i32>) -> vec3<f32> {
    return view_position(vec2<f32>(p) + 0.5, depth_at(p));
}

// Normal from depth: per axis take the side whose two neighbours continue the
// centre's plane best (1/z is linear across a plane in screen space), so
// silhouettes and inner corners do not bend the normal (Yuwen Wu, "Accurate
// Normal Reconstruction from Depth Buffer", 2021 blog post, 5-tap variant).
// The quantity that is linear in screen space across a plane: 1 / depth
// for a perspective camera, depth itself for an orthographic one.
fn planar(d: f32) -> f32 {
    return select(1.0 / d, d, is_ortho());
}

fn side_error(p: vec2<i32>, step: vec2<i32>, inv_c: f32) -> f32 {
    return abs(2.0 * planar(depth_at(p + step)) - planar(depth_at(p + 2 * step)) - inv_c);
}

fn tangent(p: vec2<i32>, step: vec2<i32>, c: vec3<f32>) -> vec3<f32> {
    let inv_c = planar(-c.z);
    if side_error(p, step, inv_c) < side_error(p, -step, inv_c) {
        return position_at(p + step) - c;
    }
    return c - position_at(p - step);
}

fn view_normal(p: vec2<i32>, c: vec3<f32>) -> vec3<f32> {
    let dx = tangent(p, vec2<i32>(1, 0), c);
    let dy = tangent(p, vec2<i32>(0, 1), c);
    var n = normalize(cross(dy, dx));
    let towards = select(-c, vec3<f32>(0.0, 0.0, 1.0), is_ortho());
    if dot(n, towards) < 0.0 {
        n = -n;
    }
    return n;
}

// Ambient obscurance with the spiral sampling of Scalable Ambient Obscurance
// (McGuire, Mara & Luebke, HPG 2012): taps on a per-pixel rotated spiral in
// a disc of fixed world radius. Per tap we use a bounded estimator, the
// cosine to the tap times a quadratic distance falloff (the horizon-cosine
// form of Bavoil & Sainz, "Screen Space Ambient Occlusion", NVIDIA 2008),
// instead of SAO's vn / (vv + eps), which turns concave creases black.
fn ambient_occlusion(p: vec2<i32>, c: vec3<f32>, n: vec3<f32>, noise: f32) -> f32 {
    let samples = i32(g.effects.x);
    let radius = g.effects.z;
    // Pixels per metre at depth 1 is P00 * width / 2.
    let disc = min(radius * pixels_per_metre(-c.z), 96.0);
    if samples <= 0 || disc < 1.0 {
        return 1.0;
    }
    let r2 = radius * radius;
    let spin = noise * 2.0 * PI;
    var sum = 0.0;
    for (var k = 0; k < samples; k++) {
        let alpha = (f32(k) + 0.5) / f32(samples);
        let angle = alpha * 7.0 * 2.0 * PI + spin; // 7 spiral turns (SAO default)
        let offset = vec2<f32>(cos(angle), sin(angle)) * alpha * disc;
        let v = position_at(p + vec2<i32>(round(offset))) - c;
        let vv = dot(v, v);
        let cosine = dot(v, n) * inverseSqrt(vv + 1e-4);
        sum += max(cosine - 0.05, 0.0) * max(1.0 - vv / r2, 0.0);
    }
    return clamp(1.0 - sum * g.effects.w * 2.0 / f32(samples), 0.0, 1.0);
}

// Contact shadows: a short ray march towards the sun through the depth
// buffer, treating every depth sample as a slab of fixed thickness (the
// technique as described by Panos Karabelas, "Screen space shadows", 2017
// blog post, and documented as "contact shadows" in Unreal Engine).
fn contact_shadow(c: vec3<f32>, n: vec3<f32>, noise: f32) -> f32 {
    let l = (g.view * vec4<f32>(g.sun_dir.xyz, 0.0)).xyz;
    if g.effects.y < 0.5 || dot(n, l) <= 0.0 {
        return 1.0;
    }
    let steps = 12;
    let ray_length = 0.3; // metres
    let thickness = 0.12;
    let start = c + n * (0.005 + 0.002 * -c.z);
    var end = start + l * ray_length;
    if end.z > -0.05 {
        // Keep the ray in front of the camera.
        end = start + l * ((-0.05 - start.z) / l.z);
    }
    // March in clip space: linear there, and clip w is the view depth.
    let c0 = g.proj * vec4<f32>(start, 1.0);
    let c1 = g.proj * vec4<f32>(end, 1.0);
    var shadow = 0.0;
    for (var i = 0; i < steps; i++) {
        let t = (f32(i) + noise) / f32(steps);
        let q = mix(c0, c1, t);
        let ndc = q.xy / q.w;
        let pixel = vec2<f32>((ndc.x * 0.5 + 0.5) * g.viewport.x, (0.5 - ndc.y * 0.5) * g.viewport.y);
        if any(pixel < vec2<f32>(0.0)) || any(pixel >= g.viewport.xy) {
            break;
        }
        let scene = depth_at(vec2<i32>(pixel));
        // Clip w is the view depth for a perspective camera only.
        let ray = select(q.w, -mix(start.z, end.z, t), is_ortho());
        let delta = ray - scene;
        if delta > 0.002 * ray && delta < thickness {
            // Fade with distance along the ray so the shadow end is soft.
            shadow = 1.0 - t * t;
            break;
        }
    }
    return 1.0 - shadow;
}

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    return Out(fullscreen(i));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy) * i32(g.options.z);
    let d = depth_at(p);
    if d >= SKY_DEPTH * 0.5 {
        return vec4<f32>(1.0);
    }
    let c = view_position(vec2<f32>(p) + 0.5, d);
    let n = view_normal(p, c);
    // A photo's samples turn the directions on (`grade.w`): accumulated, many directions.
    let noise = fract(ign(in.clip.xy) + g.grade.w * 0.618034);
    return vec4<f32>(ambient_occlusion(p, c, n, noise), contact_shadow(c, n, noise), 1.0, 1.0);
}
