// Post-processing after the forward pass: bloom (on HDR), FXAA and the
// upscale to the output size with a sharpen (on the tone-mapped, sRGB-encoded
// image). Every pass reads `src` (and the tone map `src2`) through `smp`, a
// bilinear clamp sampler. Module: common.wgsl + this file.

@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;
@group(1) @binding(2) var src2: texture_2d<f32>;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Out(vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0));
}

// ---- bloom -----------------------------------------------------------------

// Jimenez, "Next Generation Post Processing in Call of Duty: Advanced
// Warfare", SIGGRAPH 2014: a chain of half-size downsamples with a 13-tap
// filter (five overlapping 2x2 boxes: the centre one weighted 0.5, the four
// corner ones 0.125 each), then back up with a 3x3 tent, each level added to
// the next larger one. No threshold: everything blooms a little, in
// proportion to its energy (the tone map mixes the result in). The first
// downsample weights each box by 1 / (1 + luma) (Karis average, same talk)
// so single bright pixels (sun glints) do not flicker into large blobs.

override KARIS: bool = false;

fn box4(uv: vec2<f32>, t: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, d: vec2<f32>) -> vec3<f32> {
    let s = textureSampleLevel(src, smp, uv + a * t, 0.0).rgb + textureSampleLevel(src, smp, uv + b * t, 0.0).rgb
        + textureSampleLevel(src, smp, uv + c * t, 0.0).rgb + textureSampleLevel(src, smp, uv + d * t, 0.0).rgb;
    return s * 0.25;
}

fn karis(c: vec3<f32>, w: f32) -> vec4<f32> {
    let k = select(1.0, 1.0 / (1.0 + luminance(c)), KARIS);
    return vec4<f32>(c * k, k) * w;
}

@fragment
fn fs_bloom_down(in: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    let uv = in.clip.xy / vec2<f32>(textureDimensions(src) / 2u);
    let centre = box4(uv, t, vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0));
    let tl = box4(uv, t, vec2<f32>(-2.0, -2.0), vec2<f32>(0.0, -2.0), vec2<f32>(-2.0, 0.0), vec2<f32>(0.0, 0.0));
    let tr = box4(uv, t, vec2<f32>(0.0, -2.0), vec2<f32>(2.0, -2.0), vec2<f32>(0.0, 0.0), vec2<f32>(2.0, 0.0));
    let bl = box4(uv, t, vec2<f32>(-2.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(-2.0, 2.0), vec2<f32>(0.0, 2.0));
    let br = box4(uv, t, vec2<f32>(0.0, 0.0), vec2<f32>(2.0, 0.0), vec2<f32>(0.0, 2.0), vec2<f32>(2.0, 2.0));
    let sum = karis(centre, 0.5) + karis(tl, 0.125) + karis(tr, 0.125) + karis(bl, 0.125) + karis(br, 0.125);
    return vec4<f32>(sum.rgb / sum.w, 1.0);
}

// Additively blended onto the next larger level.
@fragment
fn fs_bloom_up(in: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    let uv = in.clip.xy / vec2<f32>(textureDimensions(src) * 2u);
    var c = textureSampleLevel(src, smp, uv, 0.0).rgb * 4.0;
    c += (textureSampleLevel(src, smp, uv + vec2<f32>(-t.x, 0.0), 0.0).rgb + textureSampleLevel(src, smp, uv + vec2<f32>(t.x, 0.0), 0.0).rgb
        + textureSampleLevel(src, smp, uv + vec2<f32>(0.0, -t.y), 0.0).rgb + textureSampleLevel(src, smp, uv + vec2<f32>(0.0, t.y), 0.0).rgb) * 2.0;
    c += textureSampleLevel(src, smp, uv - t, 0.0).rgb + textureSampleLevel(src, smp, uv + t, 0.0).rgb
        + textureSampleLevel(src, smp, uv + vec2<f32>(t.x, -t.y), 0.0).rgb + textureSampleLevel(src, smp, uv + vec2<f32>(-t.x, t.y), 0.0).rgb;
    return vec4<f32>(c / 16.0, 1.0);
}

// ---- FXAA ------------------------------------------------------------------

// Fast approximate anti-aliasing after Lottes, "FXAA" (NVIDIA whitepaper,
// 2009) and the structure of its 3.11 "quality" variant as described there:
// local luma contrast decides whether a pixel is on an edge, the edge's
// orientation comes from second differences of the 3x3 neighbourhood, a
// search walks along the edge in both directions (growing steps) until the
// luma change ends, and the pixel is resampled half a pixel across the edge
// in proportion to its position along the edge span; a sub-pixel term
// smooths single-pixel features (kept at 0.25: at 0.5 brick mortar lines
// visibly softened in 100 % crops).
// Written from that description for this renderer, on sRGB-encoded colour;
// the 3x3 neighbourhood comes from two gathers of the luma the tone map
// stored in alpha, and the edge search takes at most six steps (1-8 px).

const FXAA_EDGE_MIN: f32 = 0.0625;
const FXAA_EDGE: f32 = 0.166;
const FXAA_SUBPIX: f32 = 0.25;

// Luma (of the sRGB-encoded colour) is in alpha: the tone map wrote it.
fn luma(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(src, smp, uv, 0.0).a;
}

@fragment
fn fs_fxaa(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let px = 1.0 / size;
    let uv = in.clip.xy * px;
    // Two gathers give the 3x3 neighbourhood but its NE and SW corners.
    let a = textureGather(3, src, smp, uv - 0.5 * px); // w NW, z N, x W, y M
    let b = textureGather(3, src, smp, uv + 0.5 * px); // w M, z E, x S, y SE
    let m = a.y;
    let n = a.z;
    let w = a.x;
    let e = b.z;
    let s = b.x;
    let lo = min(m, min(min(n, s), min(e, w)));
    let hi = max(m, max(max(n, s), max(e, w)));
    let range = hi - lo;
    if range < max(FXAA_EDGE_MIN, hi * FXAA_EDGE) {
        return vec4<f32>(textureSampleLevel(src, smp, uv, 0.0).rgb, 1.0);
    }
    let nw = a.w;
    let se = b.y;
    let ne = luma(uv + vec2<f32>(px.x, -px.y));
    let sw = luma(uv + vec2<f32>(-px.x, px.y));
    let edge_h = abs(n + s - 2.0 * m) * 2.0 + abs(ne + se - 2.0 * e) + abs(nw + sw - 2.0 * w);
    let edge_v = abs(e + w - 2.0 * m) * 2.0 + abs(ne + nw - 2.0 * n) + abs(se + sw - 2.0 * s);
    let horizontal = edge_h >= edge_v;
    // The neighbour across the edge with the larger luma step.
    let l1 = select(w, n, horizontal);
    let l2 = select(e, s, horizontal);
    let g1 = l1 - m;
    let g2 = l2 - m;
    let steep1 = abs(g1) >= abs(g2);
    let scaled = 0.25 * max(abs(g1), abs(g2));
    var step = select(px.x, px.y, horizontal);
    var local_avg = 0.5 * (l2 + m);
    if steep1 {
        step = -step;
        local_avg = 0.5 * (l1 + m);
    }
    var edge_uv = uv;
    if horizontal {
        edge_uv.y += 0.5 * step;
    } else {
        edge_uv.x += 0.5 * step;
    }
    let along = select(vec2<f32>(0.0, px.y), vec2<f32>(px.x, 0.0), horizontal);
    var steps = array<f32, 6>(1.0, 1.5, 2.0, 2.0, 4.0, 8.0);
    var uv1 = edge_uv - along;
    var uv2 = edge_uv + along;
    var end1 = luma(uv1) - local_avg;
    var end2 = luma(uv2) - local_avg;
    var done1 = abs(end1) >= scaled;
    var done2 = abs(end2) >= scaled;
    for (var i = 1; i < 6 && !(done1 && done2); i++) {
        if !done1 {
            uv1 -= along * steps[i];
            end1 = luma(uv1) - local_avg;
            done1 = abs(end1) >= scaled;
        }
        if !done2 {
            uv2 += along * steps[i];
            end2 = luma(uv2) - local_avg;
            done2 = abs(end2) >= scaled;
        }
    }
    let d1 = select(uv.y - uv1.y, uv.x - uv1.x, horizontal);
    let d2 = select(uv2.y - uv.y, uv2.x - uv.x, horizontal);
    let near1 = d1 < d2;
    let span = d1 + d2;
    let pixel_offset = -min(d1, d2) / span + 0.5;
    let center_smaller = m < local_avg;
    let good = (select(end2, end1, near1) < 0.0) != center_smaller;
    var offset = select(0.0, pixel_offset, good);
    let average = (2.0 * (n + s + e + w) + ne + nw + se + sw) / 12.0;
    let sub1 = clamp(abs(average - m) / range, 0.0, 1.0);
    let sub2 = (-2.0 * sub1 + 3.0) * sub1 * sub1;
    offset = max(offset, sub2 * sub2 * FXAA_SUBPIX);
    var final_uv = uv;
    if horizontal {
        final_uv.y += offset * step;
    } else {
        final_uv.x += offset * step;
    }
    return vec4<f32>(textureSampleLevel(src, smp, final_uv, 0.0).rgb, 1.0);
}

// ---- upscale ---------------------------------------------------------------

// Bilinear upscale to the output size followed by robust contrast-adaptive
// sharpening as in AMD FidelityFX FSR 1 "RCAS" (MIT licence; ffx_fsr1.h):
// a cross of five taps, a negative lobe limited so the result never leaves
// the neighbourhood's min/max (no ringing), strength exp2(-0.5) here. The
// five taps are bilinear samples of the low-resolution image one output
// pixel apart.
const RCAS_LIMIT: f32 = 0.1875; // 0.25 - 1/16
const RCAS_SHARPNESS: f32 = 0.7071; // exp2(-0.5 stops)

@fragment
fn fs_upscale(in: Out) -> @location(0) vec4<f32> {
    let px = 1.0 / g.output.xy;
    let uv = in.clip.xy * px;
    let b = textureSampleLevel(src, smp, uv + vec2<f32>(0.0, -px.y), 0.0).rgb;
    let d = textureSampleLevel(src, smp, uv + vec2<f32>(-px.x, 0.0), 0.0).rgb;
    let e = textureSampleLevel(src, smp, uv, 0.0).rgb;
    let f = textureSampleLevel(src, smp, uv + vec2<f32>(px.x, 0.0), 0.0).rgb;
    let h = textureSampleLevel(src, smp, uv + vec2<f32>(0.0, px.y), 0.0).rgb;
    let mn = min(min(b, d), min(f, h));
    let mx = max(max(b, d), max(f, h));
    let hit_min = min(mn, e) / max(4.0 * mx, vec3<f32>(1e-5));
    let hit_max = (1.0 - max(mx, e)) / min(4.0 * mn - 4.0, vec3<f32>(-1e-5));
    let lobe3 = max(-hit_min, hit_max);
    let lobe = max(-RCAS_LIMIT, min(max(lobe3.r, max(lobe3.g, lobe3.b)), 0.0)) * RCAS_SHARPNESS;
    let c = (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0);
    // The bilinear taps fall between the dithered 8-bit levels of the tone
    // map: dither again by +-0.5 LSB before this output's quantisation, or
    // a dim sky gradient bands (the tone map's dither alone averages out).
    let n = (ign(in.clip.xy) - 0.5) / 255.0;
    return vec4<f32>(clamp(c + n, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
