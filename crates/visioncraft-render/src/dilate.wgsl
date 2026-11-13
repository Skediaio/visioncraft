// Impostor atlas dilation: empty texels (no baked surface) take the mean of
// their filled neighbours, repeatedly, so mip levels and bilinear filtering
// at silhouettes average in plant colours and normals rather than the black
// background (the "edge padding" every texture baker does). Albedo alpha
// stays 0 there (the cutout); the normal target's alpha marks filled texels.

@group(0) @binding(0) var albedo: texture_2d<f32>;
@group(0) @binding(1) var normal: texture_2d<f32>;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Out(vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0));
}

struct Targets {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
}

@fragment
fn fs_dilate(in: Out) -> Targets {
    let p = vec2<i32>(in.clip.xy);
    let size = vec2<i32>(textureDimensions(albedo));
    let a = textureLoad(albedo, p, 0);
    let n = textureLoad(normal, p, 0);
    if n.a > 0.0 {
        return Targets(a, n);
    }
    var sa = vec3<f32>(0.0);
    var sn = vec3<f32>(0.0);
    var k = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = clamp(p + vec2<i32>(dx, dy), vec2<i32>(0), size - 1);
            let nq = textureLoad(normal, q, 0);
            if nq.a > 0.0 {
                sa += textureLoad(albedo, q, 0).rgb;
                sn += nq.rgb;
                k += 1.0;
            }
        }
    }
    if k == 0.0 {
        return Targets(a, n);
    }
    return Targets(vec4<f32>(sa / k, 0.0), vec4<f32>(sn / k, 1.0));
}
