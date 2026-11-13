// HDR (pre-exposed, linear Rec.709) -> display. AgX by Troy Sobotka, in the
// formulation of three.js's `AgXToneMapping` (MIT licence, three.js r160+
// ShaderChunk tonemapping_pars_fragment), which ports Filament's AgX
// (Apache-2.0, google/filament PR 7236) with the 6th-order sigmoid fit from
// Benjamin Wrensch, "Minimal AgX Implementation" (iolite engine blog, 2023).
// tonemap.rs carries the same function on the CPU for unit tests.
// The target is an sRGB format, so the hardware does the transfer function;
// we only dither before its 8-bit quantisation to avoid banding in the sky.

@group(0) @binding(0) var hdr: texture_2d<f32>;

const SRGB_TO_REC2020 = mat3x3<f32>(
    vec3<f32>(0.6274, 0.0691, 0.0164),
    vec3<f32>(0.3293, 0.9195, 0.0880),
    vec3<f32>(0.0433, 0.0113, 0.8956),
);
const REC2020_TO_SRGB = mat3x3<f32>(
    vec3<f32>(1.6605, -0.1246, -0.0182),
    vec3<f32>(-0.5876, 1.1329, -0.1006),
    vec3<f32>(-0.0728, -0.0083, 1.1187),
);
const AGX_INSET = mat3x3<f32>(
    vec3<f32>(0.856627153315983, 0.137318972929847, 0.11189821299995),
    vec3<f32>(0.0951212405381588, 0.761241990602591, 0.0767994186031903),
    vec3<f32>(0.0482516061458583, 0.101439036467562, 0.811302368396859),
);
const AGX_OUTSET = mat3x3<f32>(
    vec3<f32>(1.1271005818144368, -0.1413297634984383, -0.14132976349843826),
    vec3<f32>(-0.11060664309660323, 1.157823702216272, -0.11060664309660294),
    vec3<f32>(-0.016493938717834573, -0.016493938717834257, 1.2519364065950405),
);
const AGX_MIN_EV: f32 = -12.47393; // log2(2^-10 * 0.18)
const AGX_MAX_EV: f32 = 4.026069; // log2(2^6.5 * 0.18)

fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(color: vec3<f32>) -> vec3<f32> {
    var c = AGX_INSET * (SRGB_TO_REC2020 * color);
    c = clamp((log2(max(c, vec3<f32>(1e-10))) - AGX_MIN_EV) / (AGX_MAX_EV - AGX_MIN_EV), vec3<f32>(0.0), vec3<f32>(1.0));
    c = AGX_OUTSET * agx_contrast(c);
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
    return clamp(REC2020_TO_SRGB * c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Out(vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let c = agx(textureLoad(hdr, vec2<i32>(in.clip.xy), 0).rgb);
    // Dither by +-0.5 LSB (interleaved gradient noise) in the encoded domain.
    let n = fract(52.9829189 * fract(dot(in.clip.xy, vec2<f32>(0.06711056, 0.00583715)))) - 0.5;
    let e = clamp(srgb_encode(c) + n / 255.0, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_decode(e), 1.0);
}
