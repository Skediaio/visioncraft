// Rain and wet surfaces. Part of the scene module (after scene.wgsl, terrain.wgsl, impostor.wgsl and
// sea.wgsl): `light()` calls `wet_state` and `wet_apply`, the water shaders `rain_ripple_slope`,
// and the forward pass draws the streaks with `vs_rain` / `fs_rain`. Everything here is behind
// `g.wet.x > 0` (wet surfaces) or `g.wet.y > 0` (rain): a dry scene runs none of it.
//
// What is drawn, and the physics it follows (invented from public descriptions, no code taken):
//
// - Wet porous surfaces darken. Water that fills the pores raises the refractive index of the
//   medium the grains scatter in and traps light by internal reflection at the surface: the
//   albedo falls about as its square (Lekner & Dorf, "Why some things are darker when wet",
//   Applied Optics 27(7), 1988; Jensen, Legakis, Dorsey, "Rendering of Wet Materials",
//   EGSR 1999). The exponent here runs from 1 (dry) to 1.6 (soaked), with a floor at a third
//   of the dry albedo so soil does not go black.
// - A film of water also smooths the surface: the roughness falls towards a water film's, and
//   a wet surface reflects more of the sky at grazing angles.
// - Porosity is not stored: it follows from the roughness and metalness the material already
//   has (`porosity`): rough dielectrics soak, metals and glossy paint hardly do.
// - Puddles collect on flat, open, upward-facing ground in low-frequency patches that grow
//   with the wetness; a puddle is a smooth dielectric film (Fresnel from the water's 0.02) that
//   mirrors the sky and what the sky volume sees (`ray_open`).
// - Rain falls in thin streaks (a drop at terminal speed, about 8 m/s, blurred over the exposure
//   into a line) slanted by the wind; each drop that lands makes a ring that spreads and fades
//   (capillary-gravity waves of about 4 cm).

// How much of a surface soaks up water, 0 (metal, polished paint) to 1 (plaster, soil, wood).
fn porosity(roughness: f32, metallic: f32) -> f32 {
    return (1.0 - metallic) * smoothstep(0.3, 0.75, roughness);
}

// Slope (d height / d x, d height / d z) of the rain's ripple rings at `p` (world x, z). A jittered
// grid of 0.4 m cells; each cell lands a drop at a random spot once in a while, and its ring
// spreads from the spot to 0.3 m in about a second, a few waves wide, fading as it goes. The
// nine cells round the point cover every ring that can reach it. `fade` (0..1) thins them
// with distance, where they would only shimmer.
fn rain_ripple_slope(p: vec2<f32>, rain: f32, fade: f32) -> vec2<f32> {
    var slope = vec2<f32>(0.0);
    if rain <= 0.0 || fade <= 0.0 {
        return slope;
    }
    let size = 0.4;
    let cell0 = vec2<i32>(floor(p / size));
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = cell0 + vec2<i32>(i, j);
            let phase = g.time.x * 1.1 + hash3(vec3<i32>(c, 11));
            let cycle = i32(floor(phase));
            let age = fract(phase);
            // Whether a drop lands in this cell this time round, and where.
            let lands = hash3(vec3<i32>(c, cycle * 4 + 100003)) < rain * 0.85;
            if lands {
                let at = vec2<f32>(hash3(vec3<i32>(c, cycle * 4 + 100001)), hash3(vec3<i32>(c, cycle * 4 + 100002)));
                let d = p - (vec2<f32>(c) + at) * size;
                let dist = max(length(d), 1e-4);
                let x = dist - age * 0.3;
                let fall = (1.0 - age) * (1.0 - age);
                let envelope = exp(-(x * x) / (0.05 * 0.05));
                let k = 6.2831853 / 0.045;
                // d/dx of cos(k x) exp(-x^2 / w^2), times the ring's amplitude.
                let dh = fall * envelope * (-k * sin(k * x) - 2.0 * x / (0.05 * 0.05) * cos(k * x)) * 0.0016;
                slope += dh * d / dist;
            }
        }
    }
    return slope * fade;
}

// The wetness a surface at `world` shows (x) and the puddle on it (y), both 0..1. `puddles` is the
// material's say (0 on lawns), `foliage` leaves, `snow` what lies over it already.
fn wet_state(world: vec3<f32>, n: vec3<f32>, puddles: f32, foliage: bool, snow: f32) -> vec2<f32> {
    let wetness = g.wet.x;
    // Rain does not fall under a roof or a dense crown (the snow cover map, which is also the
    // map of what lies overhead): what is there stays damp, a third as wet.
    let open = snow_open(world);
    let w = wetness * mix(0.35, 1.0, open) * (1.0 - snow);
    var puddle = 0.0;
    // Puddles from about half wet: the patches (low-frequency noise on the ground plane) grow
    // with the wetness, on flat ground only.
    let grown = smoothstep(0.45, 1.0, wetness);
    if !foliage && puddles > 0.0 && grown > 0.0 {
        let flat_up = smoothstep(0.96, 0.995, n.y);
        if flat_up > 0.0 {
            let q = vec3<f32>(world.x, 0.0, world.z);
            let nz = value_noise(q * 0.9 + vec3<f32>(3.0, 0.0, 5.0)) * 0.7 + value_noise(q * 3.3 + vec3<f32>(8.0, 0.0, 1.0)) * 0.3;
            let level = 0.14 + 0.32 * grown;
            puddle = (1.0 - smoothstep(level - 0.05, level, nz)) * smoothstep(0.0, 0.06, grown) * flat_up * puddles * smoothstep(0.3, 0.9, open) * (1.0 - snow);
        }
    }
    return vec2<f32>(w, puddle);
}

struct Wet {
    albedo: vec3<f32>,
    roughness: f32,
    metallic: f32,
    // Factor on the ambient specular: a wet surface reflects a little more.
    boost: f32,
}

// A dry surface made wet: `w` the wetness (`wet_state`), `puddle` the puddle on it.
fn wet_apply(albedo: vec3<f32>, roughness: f32, metallic: f32, w: f32, puddle: f32, foliage: bool) -> Wet {
    var out = Wet(albedo, roughness, metallic, 1.0);
    if foliage {
        // A leaf's waxy cuticle takes a sheen and its green a little deeper; no more.
        out.albedo = albedo * (1.0 - 0.14 * w);
        out.roughness = mix(roughness, 0.3, 0.7 * w);
        out.boost = 1.0 + 0.5 * w;
        return out;
    }
    let p = porosity(roughness, metallic);
    // Metals and gloss change a little (a film still lies on them), porous surfaces most.
    let k = w * (0.1 + 0.9 * p);
    let soaked = pow(max(albedo, vec3<f32>(1e-4)), vec3<f32>(1.0 + 0.6 * k));
    out.albedo = max(soaked, albedo * mix(1.0, 0.33, k));
    out.roughness = mix(roughness, max(0.08, roughness * 0.3), k);
    out.boost = 1.0 + 0.4 * k;
    // The puddle: a clear water film over a dark, soaked surface; smooth, and not a metal.
    out.albedo = mix(out.albedo, out.albedo * 0.55, puddle);
    out.roughness = mix(out.roughness, 0.045, puddle);
    out.metallic = mix(metallic, 0.0, puddle);
    return out;
}

// ---- falling rain ----------------------------------------------------------------------------

// Streaks in screen space, in a few layers at fixed distances from the eye, drawn in the forward
// pass over the finished scene: a layer's plane is a fullscreen triangle whose fragments take
// the layer's depth (`frag_depth`), so the depth test hides every streak behind a wall or a
// plant. Each layer is a grid of cells in the plane perpendicular to the view, with the
// grid turned to the way the rain falls (`g.wind`: the drops lean downwind) and scrolling
// along it at the drops' speed; a cell holds a streak or not, by a hash and the rain's density.
// The nearest layer with a streak at a pixel wins. Under a roof (the snow cover map) there is none.
const RAIN_LAYERS: i32 = 5;
// Terminal speed of a raindrop of 1-2 mm (m/s).
const RAIN_SPEED: f32 = 8.0;

struct RainOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@vertex
fn vs_rain(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return fullscreen(i);
}

@fragment
fn fs_rain(@builtin(position) frag: vec4<f32>) -> RainOut {
    let rain = g.wet.y;
    if rain <= 0.0 || is_ortho() {
        discard;
        return RainOut(vec4<f32>(0.0), 1.0);
    }
    var depths = array<f32, 5>(1.6, 3.2, 6.5, 13.0, 26.0);
    // Near layers are sparse, far ones dense, so the rain thickens into the distance.
    var density = array<f32, 5>(0.16, 0.28, 0.42, 0.55, 0.62);
    // The way the rain falls in view space, and in the picture's plane.
    let lean = min(0.06 * g.wind_phase.w, 0.9);
    let fall = normalize(vec3<f32>(g.wind.x * lean, -1.0, g.wind.y * lean));
    let rot = mat3x3<f32>(g.view[0].xyz, g.view[1].xyz, g.view[2].xyz);
    let fall_view = rot * fall;
    var dir = vec2<f32>(0.0, -1.0);
    let flat_len = length(fall_view.xy);
    if flat_len > 1e-3 {
        dir = fall_view.xy / flat_len;
    }
    let perp = vec2<f32>(-dir.y, dir.x);
    for (var i = 0; i < RAIN_LAYERS; i++) {
        let d = depths[i];
        let vp = view_position(frag.xy, d);
        let px_m = 1.0 / pixels_per_metre(d);
        // Cells a little under a twentieth of the view across at their distance, three
        // times as tall as the streak is long.
        let cw = d * 0.045;
        let length_m = d * 0.08;
        let ch = length_m * 3.0;
        let along = dot(vp.xy, dir) - RAIN_SPEED * flat_len * g.time.x;
        let across = dot(vp.xy, perp);
        let cell = vec2<i32>(floor(vec2<f32>(across / cw, along / ch)));
        let f = vec2<f32>(across / cw - floor(across / cw), along / ch - floor(along / ch));
        let seed = vec3<i32>(cell, i * 7919 + 13);
        if hash3(seed) >= density[i] * rain {
            continue;
        }
        let x0 = 0.2 + 0.6 * hash3(seed + vec3<i32>(0, 0, 1000003));
        let y0 = 0.65 * hash3(seed + vec3<i32>(0, 0, 2000003));
        // Distance across the streak's line in metres, and the position along it.
        let off = abs(f.x - x0) * cw;
        let half_width = max(px_m * 0.55, 0.0035 * d * 0.35);
        let line = 1.0 - smoothstep(half_width, half_width + px_m, off);
        let t = (f.y - y0) * ch / length_m;
        let body = smoothstep(0.0, 0.25, t) * (1.0 - smoothstep(0.6, 1.0, t));
        let cover = line * body;
        if cover < 0.02 {
            continue;
        }
        // Nothing falls under a roof or a crown.
        let world = g.camera.xyz + transpose(rot) * vp;
        if snow_open(world) < 0.5 {
            continue;
        }
        let sky = sh_irradiance(vec3<f32>(0.0, 1.0, 0.0)) / PI;
        let radiance = sky * 1.1 + g.sun_light.rgb * max(g.sun_dir.y, 0.0) * 0.004;
        let clip = g.proj * vec4<f32>(vp.xy, -d, 1.0);
        let alpha = cover * 0.34 * exp(-d * 0.012) * smoothstep(0.0, 0.3, rain);
        // Premultiplied alpha (blend: One, OneMinusSrcAlpha).
        return RainOut(vec4<f32>(radiance * alpha, alpha), clip.z / clip.w);
    }
    discard;
    return RainOut(vec4<f32>(0.0), 1.0);
}
