//! Cascaded shadow maps for the sun, CPU side.
//!
//! Splits: the "practical split scheme" of Zhang et al., "Parallel-Split
//! Shadow Maps for Large-scale Virtual Environments", VRCIA 2006 (a blend of
//! logarithmic and uniform splits). Stability: each cascade is fitted to the
//! bounding sphere of its frustum slice, which depends only on the split
//! distances and the field of view, so its size never changes as the camera
//! turns, and its centre is snapped to whole shadow texels in light space so
//! the rasterised shadow does not crawl as the camera moves (Valient,
//! "Stable Rendering of Cascaded Shadow Maps", ShaderX6, 2008; Microsoft,
//! "Common Techniques to Improve Shadow Depth Maps", Windows Dev Center).
//!
//! With two or more cascades the last one is centred on the camera instead
//! (radius = shadow distance plus a margin): it then does not depend on the
//! view direction at all, so the renderer keeps it until the camera has moved
//! more than `margin`, or the sun or the scene changed. That skips the most
//! expensive cascade (it holds the whole scene) on most frames.

use glam::{Mat4, Vec3, Vec4Swizzles};

pub const MAX_CASCADES: usize = 4;
/// Blend between logarithmic (1) and uniform (0) splits. With only two
/// cascades the near one has to reach across a garden (~20 m at 80 m), so
/// it leans more towards uniform (a shorter near cascade measured no faster
/// on the budget scene).
fn lambda(count: usize) -> f32 {
    if count <= 2 { 0.5 } else { 0.8 }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cascades {
    pub view_proj: [Mat4; MAX_CASCADES],
    /// Far view depth of each cascade (metres).
    pub splits: [f32; MAX_CASCADES],
    /// World size of one shadow texel, per cascade.
    pub texel: [f32; MAX_CASCADES],
    pub count: usize,
    /// How far the camera may move before the cached last cascade must be
    /// re-rendered (0: no cached cascade).
    pub margin: f32,
}

/// Far distance of each of `count` cascades between `near` and `far`.
pub fn split_distances(near: f32, far: f32, count: usize) -> [f32; MAX_CASCADES] {
    let mut out = [far; MAX_CASCADES];
    for (i, s) in out.iter_mut().enumerate().take(count) {
        let f = (i + 1) as f32 / count as f32;
        let log = near * (far / near).powf(f);
        let uniform = near + (far - near) * f;
        let l = lambda(count);
        *s = l * log + (1.0 - l) * uniform;
    }
    out
}

/// Smallest sphere around the frustum slice between view depths `n` and `f`
/// on the view axis: returns (distance of its centre along the view axis,
/// radius). `k2` is tan(half fov x)^2 + tan(half fov y)^2.
fn slice_sphere(n: f32, f: f32, k2: f32) -> (f32, f32) {
    let c = ((f + n) * 0.5 * (1.0 + k2)).min(f);
    let r = ((f - c) * (f - c) + f * f * k2).sqrt();
    (c, r)
}

/// Light-space rotation for a sun direction (towards the sun).
pub fn light_rotation(sun_dir: Vec3) -> Mat4 {
    let up = if sun_dir.y.abs() > 0.99 {
        Vec3::NEG_Z
    } else {
        Vec3::Y
    };
    Mat4::look_to_rh(Vec3::ZERO, -sun_dir, up)
}

/// Inputs: camera `view` and perspective `proj` (glam-style, depth 0..1),
/// `sun_dir` towards the sun, shadow `resolution`, how far shadows reach, and
/// the scene's world bounds (casters outside the view must still be in the
/// light's depth range).
pub fn compute(
    view: Mat4,
    proj: Mat4,
    sun_dir: Vec3,
    count: usize,
    resolution: u32,
    distance: f32,
    bounds: Option<(Vec3, Vec3)>,
) -> Cascades {
    let count = count.clamp(1, MAX_CASCADES);
    let (near, far) = near_far(proj);
    let far = far.min(distance).max(near * 2.0);
    let tan_x = 1.0 / proj.x_axis.x;
    let tan_y = 1.0 / proj.y_axis.y;
    let k2 = tan_x * tan_x + tan_y * tan_y;
    let inv_view = view.inverse();
    let rot = light_rotation(sun_dir);
    let splits = split_distances(near, far, count);
    let corners = bounds.map(|(lo, hi)| {
        (0..8).map(move |i| {
            let p = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            rot.transform_point3(p).z
        })
    });
    let mut out = Cascades {
        splits,
        count,
        ..Default::default()
    };
    let mut start = near;
    let eye = inv_view.w_axis.xyz();
    for (i, &split) in splits.iter().enumerate().take(count) {
        let cached = count > 1 && i == count - 1;
        let (center, r) = if cached {
            out.margin = 0.15 * far;
            (eye, far + out.margin)
        } else {
            let (c, r) = slice_sphere(start, split, k2);
            ((inv_view * glam::Vec4::new(0.0, 0.0, -c, 1.0)).xyz(), r)
        };
        start = split;
        let texel = 2.0 * r / resolution as f32;
        let ls = rot.transform_point3(center);
        let snapped_x = (ls.x / texel).round() * texel;
        let snapped_y = (ls.y / texel).round() * texel;
        // Light space looks down -Z: near/far are distances along -Z.
        let (mut z_lo, mut z_hi) = (ls.z - r, ls.z + r);
        if let Some(zs) = corners.clone() {
            for z in zs {
                z_lo = z_lo.min(z);
                z_hi = z_hi.max(z);
            }
        }
        let z_margin = 1.0;
        let ortho = Mat4::orthographic_rh(
            snapped_x - r,
            snapped_x + r,
            snapped_y - r,
            snapped_y + r,
            -(z_hi + z_margin),
            -(z_lo - z_margin),
        );
        out.view_proj[i] = ortho * rot;
        out.texel[i] = texel;
    }
    out
}

/// Near and far plane of a glam perspective projection (`perspective_rh` or
/// `perspective_infinite_rh`; far is infinite for the latter).
pub fn near_far(proj: Mat4) -> (f32, f32) {
    let r = proj.z_axis.z;
    let near = proj.w_axis.z / r;
    let far = if (1.0 + r).abs() < 1e-7 {
        f32::INFINITY
    } else {
        r * near / (1.0 + r)
    };
    (near, far)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(eye: Vec3, target: Vec3) -> (Mat4, Mat4) {
        (
            Mat4::look_at_rh(eye, target, Vec3::Y),
            Mat4::perspective_rh(60f32.to_radians(), 16.0 / 9.0, 0.1, 500.0),
        )
    }

    #[test]
    fn near_far_roundtrip() {
        let (n, f) = near_far(Mat4::perspective_rh(1.0, 1.5, 0.25, 300.0));
        assert!(
            (n - 0.25).abs() < 1e-5 && (f - 300.0).abs() < 0.1,
            "{n} {f}"
        );
        let (n, f) = near_far(Mat4::perspective_infinite_rh(1.0, 1.5, 0.1));
        assert!((n - 0.1).abs() < 1e-6 && f.is_infinite());
    }

    #[test]
    fn splits_are_increasing_and_end_at_far() {
        for count in 1..=4 {
            let s = split_distances(0.1, 80.0, count);
            assert!((s[count - 1] - 80.0).abs() < 1e-3, "{s:?}");
            assert!(s[0] > 0.1);
            for i in 1..count {
                assert!(s[i] > s[i - 1], "{s:?}");
            }
        }
        // The first cascade is short (log-heavy) for crisp near shadows.
        assert!(split_distances(0.1, 80.0, 4)[0] < 10.0);
    }

    #[test]
    fn sphere_contains_slice_corners() {
        let k2 = 0.6f32 * 0.6 + 0.35 * 0.35;
        for (n, f) in [(0.1, 5.0), (5.0, 20.0), (20.0, 80.0)] {
            let (c, r) = slice_sphere(n, f, k2);
            for z in [n, f] {
                let d = ((z - c) * (z - c) + z * z * k2).sqrt();
                assert!(d <= r + 1e-3, "corner at {z} outside: {d} > {r}");
            }
        }
    }

    #[test]
    fn cascades_do_not_change_size_when_turning() {
        let sun = Vec3::new(0.3, 0.8, 0.5).normalize();
        let (v1, p) = camera(Vec3::new(0.0, 1.7, 0.0), Vec3::new(10.0, 1.7, 0.0));
        let (v2, _) = camera(Vec3::new(0.0, 1.7, 0.0), Vec3::new(-3.0, 0.0, 7.0));
        let a = compute(v1, p, sun, 4, 2048, 80.0, None);
        let b = compute(v2, p, sun, 4, 2048, 80.0, None);
        assert_eq!(a.texel, b.texel);
        for i in 0..4 {
            // Same scale terms: only the translation may differ.
            assert!((a.view_proj[i].x_axis.x - b.view_proj[i].x_axis.x).abs() < 1e-6);
        }
    }

    #[test]
    fn cascades_move_in_whole_texels() {
        let sun = Vec3::new(-0.6, 0.35, 0.4).normalize();
        let bounds = Some((Vec3::new(-25.0, -1.0, -25.0), Vec3::new(25.0, 8.0, 25.0)));
        let mut last: Option<Cascades> = None;
        for step in 0..50 {
            // Walk slowly (a fraction of a texel per step).
            let eye = Vec3::new(step as f32 * 0.013, 1.7, step as f32 * 0.007);
            let (view, proj) = camera(eye, eye + Vec3::new(1.0, -0.1, -1.0));
            let c = compute(view, proj, sun, 3, 1024, 60.0, bounds);
            for i in 0..3 {
                // The world origin lands on the shadow map grid at a whole
                // number of texels from the cascade centre.
                let ndc = c.view_proj[i].transform_point3(Vec3::ZERO);
                let texels = ndc.x * 512.0;
                assert!(
                    (texels - texels.round()).abs() < 2e-2,
                    "cascade {i} step {step}: {texels}"
                );
            }
            if let Some(prev) = last {
                for i in 0..3 {
                    let shift = (c.view_proj[i].w_axis.x - prev.view_proj[i].w_axis.x).abs();
                    let texel_ndc = 2.0 / 1024.0;
                    let n = shift / texel_ndc;
                    assert!((n - n.round()).abs() < 2e-2, "cascade {i} moved {n} texels");
                }
            }
            last = Some(c);
        }
    }

    #[test]
    fn last_cascade_is_camera_centred() {
        let sun = Vec3::new(0.3, 0.7, -0.2).normalize();
        let eye = Vec3::new(3.0, 1.7, -4.0);
        let (view, proj) = camera(eye, Vec3::new(10.0, 0.0, 0.0));
        let c = compute(view, proj, sun, 3, 2048, 80.0, None);
        assert!((c.margin - 12.0).abs() < 1e-3);
        let ndc = c.view_proj[2].transform_point3(eye);
        assert!(ndc.x.abs() < 2e-3 && ndc.y.abs() < 2e-3, "{ndc}");
        // Any point within the shadow distance plus margin is covered.
        let far =
            c.view_proj[2].transform_point3(eye + Vec3::new(91.0, 0.0, 0.0).normalize() * 91.0);
        assert!(far.x.abs() <= 1.0 && far.y.abs() <= 1.0, "{far}");
        assert_eq!(compute(view, proj, sun, 1, 2048, 80.0, None).margin, 0.0);
    }

    #[test]
    fn whole_scene_is_inside_the_light_depth_range() {
        let sun = Vec3::new(0.2, 0.25, 0.9).normalize();
        let lo = Vec3::new(-30.0, -0.5, -30.0);
        let hi = Vec3::new(30.0, 12.0, 30.0);
        let (view, proj) = camera(Vec3::new(0.0, 1.7, 10.0), Vec3::ZERO);
        let c = compute(view, proj, sun, 4, 2048, 80.0, Some((lo, hi)));
        for i in 0..4 {
            for corner in [
                lo,
                hi,
                Vec3::new(lo.x, hi.y, hi.z),
                Vec3::new(hi.x, lo.y, lo.z),
            ] {
                let z = c.view_proj[i].transform_point3(corner).z;
                assert!((0.0..=1.0).contains(&z), "cascade {i}: depth {z}");
            }
        }
    }
}
