// The volumetric clouds' noise, made once on the GPU when the clouds are first drawn
// (clouds.rs `Gpu::new`), never per frame. Module: common.wgsl + this.
//
// - `cs_base`, 128^3: r Perlin-Worley (Perlin noise dilated by Worley noise, Schneider, "The
//   Real-time Volumetric Cloudscapes of Horizon Zero Dawn", SIGGRAPH 2015 Advances), gba
//   Worley fractal noise at three rising frequencies, for the low-frequency fractal that
//   erodes the Perlin-Worley shape.
// - `cs_detail`, 32^3: rgb Worley fractal noise at three rising frequencies, eroding the
//   edges (Schneider 2015; Schneider, "Nubis: Authoring Real-Time Volumetric Cloudscapes with
//   the Decima Engine", SIGGRAPH 2017 Advances).
// - `cs_weather`, 1024^2: common.wgsl `cloud_weather` over one CLOUD_PERIOD, the same
//   function the cloud shadows evaluate directly.
// - `cs_curl`, 128^2: a curl noise over CLOUD_CURL_TILE (clouds.wgsl) that twists the detail
//   noise into curls as rising air would (Bridson et al., "Curl-Noise for Procedural Fluid
//   Flow", SIGGRAPH 2007; Schneider 2015 distorts the detail with it).
// - `cs_mip`: each level of the base and detail noise from the one above (a 2x2x2 box), so a
//   far cloud reads the noise averaged over its texel instead of aliasing into streaks.
//
// Every noise tiles: its lattice wraps at whole cells, so the textures repeat seamlessly.

@group(1) @binding(0) var base_out: texture_storage_3d<rgba8unorm, write>;
@group(1) @binding(1) var detail_out: texture_storage_3d<rgba8unorm, write>;
@group(1) @binding(2) var weather_out: texture_storage_2d<rgba8unorm, write>;
@group(1) @binding(3) var curl_out: texture_storage_2d<rgba8unorm, write>;

fn wrap3(c: vec3<i32>, n: i32) -> vec3<u32> {
    return vec3<u32>(((c % n) + n) % n);
}

fn hash_cell(c: vec3<u32>, seed: u32) -> vec3<f32> {
    let h = pcg(c.x + pcg(c.y + pcg(c.z + seed)));
    return vec3<f32>(unit(h), unit(pcg(h)), unit(pcg(h + 1u)));
}

// Worley (cellular) noise of `n` cells per period at `p` (0..1): one feature point per cell,
// 1 at a point, falling to 0 a cell away (Worley, "A Cellular Texture Basis Function",
// SIGGRAPH 1996).
fn worley(p: vec3<f32>, n: i32, seed: u32) -> f32 {
    let q = p * f32(n);
    let id = vec3<i32>(floor(q));
    let f = q - floor(q);
    var d = 1e9;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let o = vec3<i32>(x, y, z);
                let point = vec3<f32>(o) + hash_cell(wrap3(id + o, n), seed);
                let v = point - f;
                d = min(d, dot(v, v));
            }
        }
    }
    return clamp(1.0 - sqrt(d), 0.0, 1.0);
}

fn worley_fbm(p: vec3<f32>, n: i32, seed: u32) -> f32 {
    return worley(p, n, seed) * 0.625 + worley(p, n * 2, seed + 1u) * 0.25 + worley(p, n * 4, seed + 2u) * 0.125;
}

fn gradient(c: vec3<u32>, seed: u32) -> vec3<f32> {
    return normalize(hash_cell(c, seed) * 2.0 - 1.0 + vec3<f32>(1e-4));
}

// Perlin gradient noise of `n` cells per period, about -1..1 (Perlin, "Improving Noise",
// SIGGRAPH 2002: the quintic fade).
fn perlin(p: vec3<f32>, n: i32, seed: u32) -> f32 {
    let q = p * f32(n);
    let id = vec3<i32>(floor(q));
    let f = q - floor(q);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    var corner: array<f32, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1u) & 1, (k >> 2u) & 1);
        corner[k] = dot(gradient(wrap3(id + o, n), seed), f - vec3<f32>(o));
    }
    let x0 = mix(corner[0], corner[1], u.x);
    let x1 = mix(corner[2], corner[3], u.x);
    let x2 = mix(corner[4], corner[5], u.x);
    let x3 = mix(corner[6], corner[7], u.x);
    return mix(mix(x0, x1, u.y), mix(x2, x3, u.y), u.z);
}

fn remap(v: f32, lo: f32, hi: f32, new_lo: f32, new_hi: f32) -> f32 {
    return new_lo + (v - lo) / (hi - lo) * (new_hi - new_lo);
}

@compute @workgroup_size(4, 4, 4)
fn cs_base(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(base_out);
    if any(id >= size) {
        return;
    }
    let p = (vec3<f32>(id) + 0.5) / vec3<f32>(size);
    var fbm = 0.0;
    var amp = 1.0;
    var total = 0.0;
    for (var k = 0; k < 4; k++) {
        fbm += perlin(p, 4 << u32(k), 11u + u32(k)) * amp;
        total += amp;
        amp *= 0.5;
    }
    let pn = clamp(fbm / total * 0.9 + 0.5, 0.0, 1.0);
    let w = worley_fbm(p, 4, 101u);
    let perlin_worley = clamp(remap(pn, 0.0, 1.0, w, 1.0), 0.0, 1.0);
    let out = vec4<f32>(perlin_worley, worley_fbm(p, 4, 201u), worley_fbm(p, 8, 301u), worley_fbm(p, 16, 401u));
    textureStore(base_out, id, out);
}

@compute @workgroup_size(4, 4, 4)
fn cs_detail(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(detail_out);
    if any(id >= size) {
        return;
    }
    let p = (vec3<f32>(id) + 0.5) / vec3<f32>(size);
    let out = vec4<f32>(worley_fbm(p, 2, 501u), worley_fbm(p, 4, 601u), worley_fbm(p, 8, 701u), 1.0);
    textureStore(detail_out, id, out);
}

// The curl of a tiling potential of two gradient-noise octaves at `uv` (one period, 0..1), about
// -1..1 per axis: a flow without sources or sinks.
fn potential(uv: vec2<f32>) -> f32 {
    return cloud_perlin(uv * 8.0, 8, 3u) + 0.5 * cloud_perlin(uv * 16.0, 16, 5u);
}

fn curl(uv: vec2<f32>) -> vec2<f32> {
    let e = 1.0 / 1024.0;
    let dx = potential(uv + vec2<f32>(e, 0.0)) - potential(uv - vec2<f32>(e, 0.0));
    let dy = potential(uv + vec2<f32>(0.0, e)) - potential(uv - vec2<f32>(0.0, e));
    // Per cell of the coarser octave, the gradient of gradient noise stays within about 1.5.
    return vec2<f32>(dy, -dx) / (2.0 * e) / (8.0 * 1.5);
}

@compute @workgroup_size(8, 8, 1)
fn cs_weather(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(weather_out);
    if any(id.xy >= size) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    textureStore(weather_out, id.xy, vec4<f32>(cloud_weather(uv), 0.0, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_curl(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(curl_out);
    if any(id.xy >= size) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let c = clamp(curl(uv) * 0.5 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
    textureStore(curl_out, id.xy, vec4<f32>(c, 0.0, 1.0));
}

// One level of a noise from the level above it.
@group(2) @binding(0) var mip_src: texture_3d<f32>;
@group(2) @binding(1) var mip_dst: texture_storage_3d<rgba8unorm, write>;

@compute @workgroup_size(4, 4, 4)
fn cs_mip(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(mip_dst);
    if any(id >= size) {
        return;
    }
    var sum = vec4<f32>(0.0);
    for (var k = 0u; k < 8u; k++) {
        let o = vec3<u32>(k & 1u, (k >> 1u) & 1u, (k >> 2u) & 1u);
        sum += textureLoad(mip_src, vec3<i32>(id * 2u + o), 0);
    }
    textureStore(mip_dst, id, sum / 8.0);
}
