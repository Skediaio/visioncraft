// HDR (pre-exposed, linear Rec.709) -> display. AgX by Troy Sobotka, in the
// formulation of three.js's `AgXToneMapping` (MIT licence, three.js r160+
// ShaderChunk tonemapping_pars_fragment), which ports Filament's AgX
// (Apache-2.0, google/filament PR 7236) with the 6th-order sigmoid fit from
// Benjamin Wrensch, "Minimal AgX Implementation" (iolite engine blog, 2023).
// tonemap.rs carries the same function on the CPU for unit tests.
// The target is 8-bit unorm holding sRGB-encoded values (the output's
// non-sRGB view, or the LDR image FXAA and the upscale read), dithered
// before quantisation to avoid banding in the sky. Bloom is mixed in first.
// Module: common.wgsl + post.wgsl (bindings: src = HDR, src2 = bloom) + this.

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
    c = agx_contrast(c);
    // The look (Filament's AgX looks): power for contrast, then saturation around Rec.709
    // luma; plain AgX at (1, 1).
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(g.grade.x));
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = AGX_OUTSET * (luma + g.grade.y * (c - luma));
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
    return clamp(REC2020_TO_SRGB * c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

override LUMA_IN_ALPHA: bool = false;

// Night vision: in dim light the rods take over from the cones, colour fades and what is left
// reads blue-grey (the Purkinje shift). Krawczyk, Myszkowski & Seidel, "Perceptual Effects in
// Real-time Tone Mapping", SCCG 2005: the rods' share sigma = 0.04 / (0.04 + Y) for the
// luminance Y (cd/m2), scotopic luminance V = Y (1.33 (1 + (Y + Z) / X) - 1.68), shown in the
// blue-grey (1.05, 0.97, 1.27) of their paper. Twilight (Y of a few cd/m2) keeps nearly all its
// colour; a moonlit lawn (0.01 cd/m2) turns mostly grey-blue; lamps stay warm.
fn night_vision(hdr: vec3<f32>) -> vec3<f32> {
    let exposure = max(g.time.y, 1e-12);
    let rgb = max(hdr, vec3<f32>(0.0)) / exposure;
    // Linear Rec.709 to CIE XYZ (D65).
    let xyz = mat3x3<f32>(
        vec3<f32>(0.4124, 0.2126, 0.0193),
        vec3<f32>(0.3576, 0.7152, 0.1192),
        vec3<f32>(0.1805, 0.0722, 0.9505),
    ) * rgb;
    let y = xyz.y;
    if y <= 0.0 {
        return hdr;
    }
    let sigma = 0.04 / (0.04 + y);
    // Divided by 2.58, V of a white surface, so white keeps its brightness and the auto
    // exposure's night tuning holds; blues and greens gain on reds, as rods see them.
    let v = max(y * (1.33 * (1.0 + (xyz.y + xyz.z) / max(xyz.x, 1e-9)) - 1.68), 0.0) / 2.58;
    return mix(hdr, v * exposure * vec3<f32>(1.05, 0.97, 1.27), sigma);
}

// ---- white balance and grade -----------------------------------------------
// tonemap.rs has the same on the CPU (`adapt`, `grade_matrix`), with the sources.

// Bradford cone response (XYZ -> LMS) times linear Rec.709 -> XYZ, and its inverse.
const RGB_TO_XYZ = mat3x3<f32>(
    vec3<f32>(0.4124, 0.2126, 0.0193),
    vec3<f32>(0.3576, 0.7152, 0.1192),
    vec3<f32>(0.1805, 0.0722, 0.9505),
);
const XYZ_TO_RGB = mat3x3<f32>(
    vec3<f32>(3.2406, -0.9689, 0.0557),
    vec3<f32>(-1.5372, 1.8758, -0.2040),
    vec3<f32>(-0.4986, 0.0415, 1.0570),
);
const BRADFORD = mat3x3<f32>(
    vec3<f32>(0.8951, -0.7502, 0.0389),
    vec3<f32>(0.2664, 1.7135, -0.0685),
    vec3<f32>(-0.1614, 0.0367, 1.0296),
);
const BRADFORD_INV = mat3x3<f32>(
    vec3<f32>(0.9869929, 0.4323053, -0.0085287),
    vec3<f32>(-0.1470543, 0.5183603, 0.0400428),
    vec3<f32>(0.1599627, 0.0492912, 0.9684867),
);

// The light falling on open ground: the sun on a horizontal surface and the sky's irradiance,
// as the atmosphere model and the clouds have them (both pre-exposed lux), as a camera's
// automatic white balance estimates the scene's illuminant (grey world over an open view).
fn scene_white() -> vec3<f32> {
    let sun = g.sun_light.rgb * max(g.sun_dir.y, 0.0);
    return sun + sky_up_sh() * g.night.w;
}

// Von Kries adaptation in Bradford space from `white` to `g.white_ref`, `g.white_ref.w` of the
// way (Lam 1985; as Unreal's and most cameras' temperature setting work).
fn white_balance(c: vec3<f32>) -> vec3<f32> {
    let mode = u32(g.white.w + 0.5);
    if mode == 0u {
        return c;
    }
    var white = g.white.rgb;
    if mode == 2u {
        let e = scene_white();
        let y = luminance(e);
        if y <= 0.0 {
            return c;
        }
        white = e / y;
    }
    let lms_w = BRADFORD * (RGB_TO_XYZ * max(white, vec3<f32>(1e-6)));
    let lms_t = BRADFORD * (RGB_TO_XYZ * g.white_ref.rgb);
    let gain = 1.0 + (lms_t / lms_w - 1.0) * g.white_ref.w;
    return max(XYZ_TO_RGB * (BRADFORD_INV * (gain * (BRADFORD * (RGB_TO_XYZ * c)))), vec3<f32>(0.0));
}

// A grade (tonemap.rs `grade_matrix`): a matrix in linear Rec.709, then a contrast exponent
// about mid grey (in units of the exposure), before AgX as Frostbite grades before the display
// transform (Fry, "High Dynamic Range Color Grading and Display in Frostbite", GDC 2017).
fn color_grade(c: vec3<f32>) -> vec3<f32> {
    if g.color_grade[1].w < 0.5 {
        return c;
    }
    let m = mat3x3<f32>(g.color_grade[0].xyz, g.color_grade[1].xyz, g.color_grade[2].xyz);
    let graded = max(m * c, vec3<f32>(0.0));
    let k = g.color_grade[0].w;
    if k == 1.0 {
        return graded;
    }
    let grey = vec3<f32>(0.18);
    return grey * pow(graded / grey, vec3<f32>(k));
}

// A lens' natural vignetting at `uv` (0..1 over the picture) for `amount` 0..1: the light
// falls off as the square of (1 - k r^2), r the distance from the centre with the corners at 1
// (the cos^4 law's shape for a moderate field of view, flattened in the middle), so 0.3 takes
// about half a stop off the corners and 1 two stops. Before the tone map: it is light the lens
// does not pass, and highlights stay bright where the corners darken.
fn vignette(uv: vec2<f32>, amount: f32) -> f32 {
    let aspect = g.viewport.x * g.viewport.w;
    let d = (uv - 0.5) * vec2<f32>(aspect, 1.0);
    let r2 = dot(d, d) / dot(vec2<f32>(0.5 * aspect, 0.5), vec2<f32>(0.5 * aspect, 0.5));
    let f = 1.0 - 0.5 * amount * r2;
    return f * f;
}

// A uniform hash in [0, 1) of an integer pixel and a seed (PCG, Jarzynski and Olano, "Hash
// Functions for GPU Rendering", JCGT 9(3), 2020).
fn pcg_hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash01(pixel: vec2<f32>, seed: u32) -> f32 {
    let p = vec2<u32>(pixel);
    let h = pcg_hash(p.x ^ pcg_hash(p.y ^ pcg_hash(seed)));
    return f32(h >> 8u) / 16777216.0;
}

// Film grain on the tone-mapped (display linear) colour `c`, `amount` 0..1: one value of noise
// per pixel for all three channels (grain is in the luminance, not the colour), roughly
// Gaussian (the sum of three uniforms, deviation 0.5). As a gain, so black stays black; its
// size in luminance grows with the square root of the luminance (as a film's or a sensor's
// shot noise) and fades out towards white, so it shows most in the mid-tones. Fixed per picture;
// in the live view it changes with the clock.
fn film_grain(c: vec3<f32>, pixel: vec2<f32>, amount: f32) -> vec3<f32> {
    let seed = u32(fract(g.time.x * 0.5) * 24.0);
    let u = hash01(pixel, seed) + hash01(pixel, seed + 101u) + hash01(pixel, seed + 211u) - 1.5;
    let y = clamp(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let gain = 1.0 + 0.16 * amount * u * (1.0 - y) / sqrt(max(y, 1e-3));
    return c * max(gain, 0.0);
}

@fragment
fn fs_tonemap(in: Out) -> @location(0) vec4<f32> {
    // Never below zero: the light shafts take light out of the HDR image (shafts.wgsl).
    var hdr = max(textureLoad(src, vec2<i32>(in.clip.xy), 0).rgb, vec3<f32>(0.0));
    if g.options.y > 0.0 {
        // Four summed bloom levels (lib.rs BLOOM_LEVELS): their mean keeps
        // the energy.
        let bloom = textureSampleLevel(src2, smp, in.clip.xy * g.viewport.zw, 0.0).rgb * 0.25;
        hdr = mix(hdr, bloom, g.options.y);
    }
    if g.aerial.w > 0.0 {
        hdr *= vignette(in.clip.xy * g.viewport.zw, g.aerial.w);
    }
    var c = agx(color_grade(white_balance(night_vision(hdr))));
    if g.snow.w > 0.0 {
        c = film_grain(c, in.clip.xy, g.snow.w);
    }
    // Dither by +-0.5 LSB (interleaved gradient noise) in the encoded domain.
    let n = ign(in.clip.xy) - 0.5;
    let encoded = clamp(srgb_encode(c) + n / 255.0, vec3<f32>(0.0), vec3<f32>(1.0));
    // Into the LDR image, alpha carries luma for FXAA; the output stays opaque.
    let a = select(1.0, dot(encoded, vec3<f32>(0.299, 0.587, 0.114)), LUMA_IN_ALPHA);
    return vec4<f32>(encoded, a);
}
