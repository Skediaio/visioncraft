//! A photo of a facade as the texture of a wall face, straightened and scaled.
//!
//! Conventions, fixed here once:
//! - Photo pixels are continuous with `(0, 0)` the top-left corner of the first pixel, x to the
//!   right, y down (as for an underlay).
//! - `corners_px` are the facade's four corners as they stand in the photo, in the order
//!   bottom-left, bottom-right, top-right, top-left *as seen from outside the building*.
//! - A face has its own frame ([`FaceFrame`]): `v` runs up (world up projected into the face),
//!   `u` runs to the right seen from outside (`v x normal`), and `(0, 0)` is the lowest, leftmost
//!   point of the face. `region` is a rectangle in `(u, v)` metres; the photo's facade covers
//!   exactly that rectangle. Without a region it is the whole face's bounding rectangle.
//! - The straightened picture ([`rectify`]) has its top-left at `(u0, v1)`: row 0 is the top.

use crate::ImportedModel;
use crate::material::Target;
use crate::project::Extensions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The longest side of a straightened picture, pixels.
pub const MAX_RECTIFIED_PX: u32 = 2048;

/// The shortest long side of a straightened picture, pixels.
pub const MIN_RECTIFIED_PX: u32 = 256;

/// A rectangle on a face, metres in the face's `(u, v)` frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl Region {
    pub fn width(&self) -> f32 {
        self.u1 - self.u0
    }

    pub fn height(&self) -> f32 {
        self.v1 - self.v0
    }

    pub fn validate(&self) -> Result<(), String> {
        let all = [self.u0, self.v0, self.u1, self.v1];
        if !all.iter().all(|c| c.is_finite() && c.abs() < 1e5) {
            return Err("region must be four finite numbers (metres)".into());
        }
        if self.u1 <= self.u0 || self.v1 <= self.v0 {
            return Err("region needs u1 above u0 and v1 above v0".into());
        }
        Ok(())
    }
}

/// A facade photo on a wall face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WallPhoto {
    /// `p1`, `p2`...
    pub id: String,
    pub name: String,
    /// A path relative to the project file, or `photos/<name>` inside a bundle.
    pub file: String,
    /// Width and height of the photo, recorded at import so no decoding is needed to check it.
    pub size_px: [u32; 2],
    /// The face it covers: a `Target::Face` (an owner's face by its persistent name).
    pub target: Target,
    /// The facade's corners in the photo: bottom-left, bottom-right, top-right, top-left as
    /// seen from outside.
    pub corners_px: [[f32; 2]; 4],
    /// Where on the face the facade lies; `None`: the whole face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// Data of other tools by namespace, kept untouched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

/// The corners of an image that is already straight: bottom-left, bottom-right, top-right,
/// top-left.
pub fn image_corners(size_px: [u32; 2]) -> [[f32; 2]; 4] {
    let (w, h) = (size_px[0] as f32, size_px[1] as f32);
    [[0.0, h], [w, h], [w, 0.0], [0.0, 0.0]]
}

impl WallPhoto {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.file.trim().is_empty() {
            return Err("a wall photo needs a name and a file".into());
        }
        let max = crate::underlay::MAX_SIDE_PX;
        let [w, h] = self.size_px;
        if w == 0 || h == 0 || w > max || h > max {
            return Err(format!("size_px must be two whole numbers from 1 to {max}"));
        }
        if !matches!(self.target, Target::Face { .. }) {
            return Err(
                "target must name one face: {\"type\": \"face\", \"owner\": \"S1\", \"face\": \"S1:side(2)\"}"
                    .into(),
            );
        }
        check_corners(&self.corners_px)?;
        if let Some(r) = &self.region {
            r.validate()?;
        }
        Ok(())
    }
}

/// Four finite corners that make a convex quadrilateral in the order bottom-left, bottom-right,
/// top-right, top-left (a photo is not mirrored).
pub fn check_corners(c: &[[f32; 2]; 4]) -> Result<(), String> {
    if !c.iter().flatten().all(|v| v.is_finite() && v.abs() < 1e6) {
        return Err("corners_px must be four [x, y] positions in pixels".into());
    }
    let cross = |a: [f32; 2], b: [f32; 2], p: [f32; 2]| {
        f64::from(b[0] - a[0]) * f64::from(p[1] - b[1])
            - f64::from(b[1] - a[1]) * f64::from(p[0] - b[0])
    };
    // In pixels y runs down: bottom-left, bottom-right, top-right, top-left turn left on screen,
    // which is a negative cross product here.
    let ok = (0..4).all(|i| cross(c[i], c[(i + 1) % 4], c[(i + 2) % 4]) < -1e-6);
    if ok {
        Ok(())
    } else {
        Err(
            "corners_px must be a four-sided shape with no dent, in the order bottom-left, \
             bottom-right, top-right, top-left as seen in the photo (from outside the building)"
                .into(),
        )
    }
}

/// A projective map of the plane, `[h0..h8]` row by row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// The map that takes each `src[i]` to `dst[i]`; `None` when the points are degenerate
    /// (three on a line).
    pub fn from_points(src: [[f64; 2]; 4], dst: [[f64; 2]; 4]) -> Option<Self> {
        // direct linear transform with h8 = 1: 8 equations, 8 unknowns
        let mut m = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let ([x, y], [u, v]) = (src[i], dst[i]);
            m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u, u];
            m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v, v];
        }
        let h = solve8(m)?;
        Some(Self([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]))
    }

    /// The map from the photo's four facade corners to the unit square: bottom-left to
    /// `(0, 0)`, bottom-right `(1, 0)`, top-right `(1, 1)`, top-left `(0, 1)`.
    pub fn photo_to_unit(corners_px: [[f32; 2]; 4]) -> Option<Self> {
        Self::from_points(corners_f64(corners_px), UNIT)
    }

    /// The map from the unit square (`s` right, `t` up) to photo pixels: the inverse of
    /// [`Self::photo_to_unit`], solved directly.
    pub fn unit_to_photo(corners_px: [[f32; 2]; 4]) -> Option<Self> {
        Self::from_points(UNIT, corners_f64(corners_px))
    }

    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let h = &self.0;
        let w = h[6] * p[0] + h[7] * p[1] + h[8];
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        [
            (h[0] * p[0] + h[1] * p[1] + h[2]) / w,
            (h[3] * p[0] + h[4] * p[1] + h[5]) / w,
        ]
    }
}

const UNIT: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

fn corners_f64(c: [[f32; 2]; 4]) -> [[f64; 2]; 4] {
    c.map(|p| [f64::from(p[0]), f64::from(p[1])])
}

/// Gauss with partial pivoting on an 8 x 9 augmented matrix; `None` when singular.
fn solve8(mut m: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|a, b| m[*a][col].abs().total_cmp(&m[*b][col].abs()))?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        for row in 0..8 {
            if row != col {
                let f = m[row][col] / m[col][col];
                for k in col..9 {
                    m[row][k] -= f * m[col][k];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for i in 0..8 {
        x[i] = m[i][8] / m[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The size of the straightened picture for a region of `width_m` by `height_m`: its long side
/// follows the photo (at least [`MIN_RECTIFIED_PX`], at most [`MAX_RECTIFIED_PX`]), the aspect
/// the region's.
pub fn rectified_size(width_m: f32, height_m: f32, photo_size: [u32; 2]) -> [u32; 2] {
    let long = photo_size[0]
        .max(photo_size[1])
        .clamp(MIN_RECTIFIED_PX, MAX_RECTIFIED_PX) as f32;
    let (w, h) = (width_m.max(1e-3), height_m.max(1e-3));
    let k = long / w.max(h);
    [
        ((w * k).round() as u32).clamp(1, MAX_RECTIFIED_PX),
        ((h * k).round() as u32).clamp(1, MAX_RECTIFIED_PX),
    ]
}

/// Straightens a photo: the picture of the quadrilateral `corners_px` as an upright rectangle
/// of `out` pixels (RGBA8, row 0 the top, bilinear, 2 x 2 samples a pixel). `rgba` is the
/// photo's `size` pixels.
pub fn rectify(
    rgba: &[u8],
    size: [u32; 2],
    corners_px: [[f32; 2]; 4],
    out: [u32; 2],
) -> Result<Vec<u8>, String> {
    let [w, h] = size;
    if w == 0 || h == 0 || rgba.len() != (w as usize) * (h as usize) * 4 {
        return Err("the photo's pixels do not match its size".into());
    }
    if out[0] == 0 || out[1] == 0 {
        return Err("the straightened picture would be empty".into());
    }
    let map = Homography::unit_to_photo(corners_px)
        .ok_or("the four corners do not make a four-sided shape")?;
    let sample = |x: f64, y: f64| -> [f32; 4] {
        // pixel centres are at +0.5
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let at = |xi: f64, yi: f64| {
            let xi = (xi.max(0.0) as usize).min(w as usize - 1);
            let yi = (yi.max(0.0) as usize).min(h as usize - 1);
            let i = (yi * w as usize + xi) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]].map(f32::from)
        };
        let (a, b, c, d) = (
            at(x0, y0),
            at(x0 + 1.0, y0),
            at(x0, y0 + 1.0),
            at(x0 + 1.0, y0 + 1.0),
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    };
    let mut px = Vec::with_capacity(out[0] as usize * out[1] as usize * 4);
    const SUB: [f64; 2] = [0.25, 0.75];
    for y in 0..out[1] {
        for x in 0..out[0] {
            let mut sum = [0.0f32; 4];
            for sy in SUB {
                for sx in SUB {
                    let s = (f64::from(x) + sx) / f64::from(out[0]);
                    let t = 1.0 - (f64::from(y) + sy) / f64::from(out[1]);
                    let [px_x, px_y] = map.apply([s, t]);
                    let c = sample(px_x, px_y);
                    for k in 0..4 {
                        sum[k] += c[k];
                    }
                }
            }
            px.extend(sum.map(|v| (v / 4.0).round().clamp(0.0, 255.0) as u8));
        }
    }
    Ok(px)
}

/// A face's own frame: where `(u, v)` metres lie in the model's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceFrame {
    /// The point at `(0, 0)`: the lowest, leftmost corner of the face's bounds.
    pub origin: [f32; 3],
    /// Unit vectors: to the right seen from outside, and up.
    pub u_axis: [f32; 3],
    pub v_axis: [f32; 3],
    /// Unit, pointing out of the building.
    pub normal: [f32; 3],
    /// The face's extent `[width, height]` in metres.
    pub size: [f32; 2],
}

impl FaceFrame {
    /// The `(u, v)` of a point (its distance from the face's plane is ignored).
    pub fn uv(&self, p: [f32; 3]) -> [f32; 2] {
        let d = sub(p, self.origin);
        [dot(d, self.u_axis), dot(d, self.v_axis)]
    }

    /// Whether a triangle of the face faces the way the face does: the parts that stand square to
    /// it (the reveals of a window in a wall) are not covered by a photo.
    pub fn faces(&self, tri: [[f32; 3]; 3]) -> bool {
        let n = cross(sub(tri[1], tri[0]), sub(tri[2], tri[0]));
        let len = dot(n, n).sqrt();
        len > 1e-12 && dot(n, self.normal) / len > FACING_COS
    }

    /// The region that is the whole face.
    pub fn whole(&self) -> Region {
        Region {
            u0: 0.0,
            v0: 0.0,
            u1: self.size[0],
            v1: self.size[1],
        }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f32; 3]) -> Option<[f32; 3]> {
    let n = dot(a, a).sqrt();
    (n > 1e-9).then(|| a.map(|c| c / n))
}

/// The most the parts of a face that face outward may lie behind one another (a window set back
/// in its wall, a plinth) and still count as one flat face, metres.
const SETBACK_M: f32 = 0.3;

/// A triangle counts as facing the face's way when its normal is within about 60 degrees of it.
const FACING_COS: f32 = 0.5;

/// The frame of face number `face` of `model`. `up` is the direction of up in the model's
/// space (world up, turned into the model's own space when the model is). A message for a face
/// that has no triangles, lies flat (it has no up) or is not flat.
pub fn face_frame(model: &ImportedModel, face: usize, up: [f32; 3]) -> Result<FaceFrame, String> {
    // The face's own direction: its triangles' normals weighted by area. A wall with openings
    // also holds their reveals, which stand square to it: they are not part of the picture's
    // plane (see `FaceFrame::faces`).
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    let mut normal = [0.0f32; 3];
    for (t, f) in model.triangles.iter().zip(&model.triangle_faces) {
        if *f as usize != face {
            continue;
        }
        let p = t.map(|v| model.positions[v as usize]);
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        normal = [normal[0] + n[0], normal[1] + n[1], normal[2] + n[2]];
        tris.push(p);
    }
    if tris.is_empty() {
        return Err("the face has no triangles".into());
    }
    let normal = unit(normal).ok_or("the face has no area")?;
    let up = unit(up).ok_or("no direction for up")?;
    let along = dot(up, normal);
    let v_axis = unit([
        up[0] - normal[0] * along,
        up[1] - normal[1] * along,
        up[2] - normal[2] * along,
    ])
    .filter(|v| dot(*v, up) > 1e-3)
    .ok_or(
        "a face that lies flat (a floor, a flat roof) has no up: photos go on walls and slopes",
    )?;
    let u_axis = cross(v_axis, normal);
    let mut frame = FaceFrame {
        origin: [0.0; 3],
        u_axis,
        v_axis,
        normal,
        size: [0.0; 2],
    };
    let covered: Vec<&[[f32; 3]; 3]> = tris.iter().filter(|t| frame.faces(**t)).collect();
    let first = *covered
        .first()
        .and_then(|t| t.first())
        .ok_or("the face has no part that faces outward")?;
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    // The depth of the plane the picture lies in: the face's extreme parts differ by up to
    // `SETBACK_M` (a window set back in its wall); more than that is a bent face.
    let (mut near, mut far) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in covered.iter().flat_map(|t| t.iter()) {
        let d = sub(*p, first);
        let depth = dot(d, normal);
        near = near.min(depth);
        far = far.max(depth);
        for (k, axis) in [u_axis, v_axis].iter().enumerate() {
            let c = dot(d, *axis);
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    if far - near > SETBACK_M {
        return Err(format!(
            "the face is not flat (it bends {:.0} cm out of its plane); a photo goes on a flat face",
            (far - near) * 100.0
        ));
    }
    frame.origin = [
        first[0] + u_axis[0] * lo[0] + v_axis[0] * lo[1],
        first[1] + u_axis[1] * lo[0] + v_axis[1] * lo[1],
        first[2] + u_axis[2] * lo[0] + v_axis[2] * lo[1],
    ];
    frame.size = [hi[0] - lo[0], hi[1] - lo[1]];
    Ok(frame)
}

/// One corner of a triangle being cut: where it is and which way it faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// Cuts a triangle of a face along the rectangle `region` (in the face's `(u, v)`): the
/// triangles inside it, and those outside. Pieces keep the winding.
pub fn split_by_region(
    frame: &FaceFrame,
    region: &Region,
    tri: [Corner; 3],
) -> (Vec<[Corner; 3]>, Vec<[Corner; 3]>) {
    // each plane: inside where s >= 0
    let planes: [Box<dyn Fn([f32; 2]) -> f32>; 4] = [
        Box::new(|p| p[0] - region.u0),
        Box::new(|p| region.u1 - p[0]),
        Box::new(|p| p[1] - region.v0),
        Box::new(|p| region.v1 - p[1]),
    ];
    let mut poly: Vec<Corner> = tri.to_vec();
    let mut outside = Vec::new();
    for s in &planes {
        let (keep, cut) = clip(&poly, |c| s(frame.uv(c.position)));
        fan(&cut, &mut outside);
        poly = keep;
        if poly.len() < 3 {
            return (Vec::new(), outside);
        }
    }
    let mut inside = Vec::new();
    fan(&poly, &mut inside);
    (inside, outside)
}

/// Splits a convex polygon by `s = 0`: the part where `s >= 0` and the part where `s <= 0`.
fn clip(poly: &[Corner], s: impl Fn(&Corner) -> f32) -> (Vec<Corner>, Vec<Corner>) {
    let (mut keep, mut cut) = (Vec::new(), Vec::new());
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (sa, sb) = (s(&a), s(&b));
        if sa >= 0.0 {
            keep.push(a);
        }
        if sa <= 0.0 {
            cut.push(a);
        }
        if (sa > 0.0 && sb < 0.0) || (sa < 0.0 && sb > 0.0) {
            let t = sa / (sa - sb);
            let mix = |p: [f32; 3], q: [f32; 3]| std::array::from_fn(|k| p[k] + (q[k] - p[k]) * t);
            let m = Corner {
                position: mix(a.position, b.position),
                normal: mix(a.normal, b.normal),
            };
            keep.push(m);
            cut.push(m);
        }
    }
    // a part that only touches the line has no area
    let area = |p: &[Corner]| p.len() >= 3;
    (
        if area(&keep) { keep } else { Vec::new() },
        if area(&cut) { cut } else { Vec::new() },
    )
}

fn fan(poly: &[Corner], out: &mut Vec<[Corner; 3]>) {
    for i in 1..poly.len().saturating_sub(1) {
        let t = [poly[0], poly[i], poly[i + 1]];
        // skip slivers with no area
        let n = cross(
            sub(t[1].position, t[0].position),
            sub(t[2].position, t[0].position),
        );
        if dot(n, n) > 1e-14 {
            out.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Body, Face};

    fn close(a: [f64; 2], b: [f64; 2], tol: f64) {
        assert!(
            (a[0] - b[0]).abs() < tol && (a[1] - b[1]).abs() < tol,
            "{a:?} != {b:?}"
        );
    }

    /// A facade seen at an angle: wider at the bottom than the top, a bit turned.
    const SKEWED: [[f32; 2]; 4] = [
        [100.0, 700.0],
        [900.0, 650.0],
        [820.0, 150.0],
        [160.0, 100.0],
    ];

    #[test]
    fn the_four_corners_map_to_the_unit_square_corners() {
        let h = Homography::photo_to_unit(SKEWED).unwrap();
        for (c, u) in SKEWED.iter().zip(UNIT) {
            close(h.apply([f64::from(c[0]), f64::from(c[1])]), u, 1e-9);
        }
        let back = Homography::unit_to_photo(SKEWED).unwrap();
        for (c, u) in SKEWED.iter().zip(UNIT) {
            close(back.apply(u), [f64::from(c[0]), f64::from(c[1])], 1e-6);
        }
    }

    #[test]
    fn the_centre_of_an_upright_rectangle_maps_to_the_centre() {
        // a straight photo of 800 x 600
        let corners = image_corners([800, 600]);
        let h = Homography::photo_to_unit(corners).unwrap();
        close(h.apply([400.0, 300.0]), [0.5, 0.5], 1e-9);
        close(h.apply([200.0, 150.0]), [0.25, 0.75], 1e-9);
        // the unit square's centre is not the photo centre once it is seen in perspective: the
        // far (top, narrower) half is smaller, so the middle of the facade lies above the
        // middle of the quad's height
        let back =
            Homography::unit_to_photo([[0.0, 100.0], [100.0, 100.0], [80.0, 0.0], [20.0, 0.0]])
                .unwrap();
        let mid = back.apply([0.5, 0.5]);
        assert!((mid[0] - 50.0).abs() < 1e-9);
        assert!(mid[1] < 50.0, "{mid:?}");
    }

    #[test]
    fn degenerate_corners_have_no_homography() {
        let line = [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]];
        assert!(Homography::photo_to_unit(line).is_none());
    }

    #[test]
    fn corners_must_come_in_the_right_order_and_convex() {
        assert!(check_corners(&SKEWED).is_ok());
        assert!(check_corners(&image_corners([10, 10])).is_ok());
        // mirrored: bottom-right first
        let mirrored = [SKEWED[1], SKEWED[0], SKEWED[3], SKEWED[2]];
        assert!(
            check_corners(&mirrored)
                .unwrap_err()
                .contains("bottom-left")
        );
        // a dent
        let dent = [[0.0, 10.0], [10.0, 10.0], [5.0, 8.0], [0.0, 0.0]];
        assert!(check_corners(&dent).is_err());
        assert!(check_corners(&[[f32::NAN, 0.0]; 4]).is_err());
    }

    #[test]
    fn a_skewed_photo_is_straightened_with_its_corners_in_the_corners() {
        // 1000 x 800 photo, grey; the facade quad is painted: bottom-left red... via 4 corner
        // blobs placed on the quad corners
        let (w, h) = (1000u32, 800u32);
        let mut px = vec![128u8; (w * h * 4) as usize];
        for p in px.chunks_mut(4) {
            p[3] = 255;
        }
        let paint = |px: &mut Vec<u8>, at: [f32; 2], rgb: [u8; 3]| {
            for dy in -40i32..=40 {
                for dx in -40i32..=40 {
                    let (x, y) = (at[0] as i32 + dx, at[1] as i32 + dy);
                    if (0..w as i32).contains(&x) && (0..h as i32).contains(&y) {
                        let i = ((y as u32 * w + x as u32) * 4) as usize;
                        px[i..i + 3].copy_from_slice(&rgb);
                    }
                }
            }
        };
        // paint slightly inside each corner so the blob lies inside the quad
        let inward = |c: [f32; 2], toward: [f32; 2]| {
            [
                c[0] + (toward[0] - c[0]) * 0.06,
                c[1] + (toward[1] - c[1]) * 0.06,
            ]
        };
        let centre = [490.0, 400.0];
        let colours = [[0, 0, 255], [255, 255, 0], [0, 255, 0], [255, 0, 0]];
        for (c, rgb) in SKEWED.iter().zip(colours) {
            paint(&mut px, inward(*c, centre), rgb);
        }
        let out = rectify(&px, [w, h], SKEWED, [200, 150]).unwrap();
        let at = |x: usize, y: usize| {
            let i = (y * 200 + x) * 4;
            [out[i], out[i + 1], out[i + 2]]
        };
        // top-left red, top-right green, bottom-left blue, bottom-right yellow
        let near = |c: [u8; 3], want: [u8; 3]| c.iter().zip(want).all(|(a, b)| a.abs_diff(b) < 60);
        assert!(near(at(10, 10), [255, 0, 0]), "{:?}", at(10, 10));
        assert!(near(at(189, 10), [0, 255, 0]), "{:?}", at(189, 10));
        assert!(near(at(10, 139), [0, 0, 255]), "{:?}", at(10, 139));
        assert!(near(at(189, 139), [255, 255, 0]), "{:?}", at(189, 139));
        assert_eq!(at(100, 75), [128, 128, 128]);
    }

    #[test]
    fn the_picture_follows_the_region_aspect() {
        assert_eq!(rectified_size(10.0, 5.0, [4000, 3000]), [2048, 1024]);
        assert_eq!(rectified_size(3.0, 6.0, [800, 600]), [400, 800]);
        // a small photo is not stretched below the floor
        assert_eq!(rectified_size(4.0, 2.0, [100, 80]), [256, 128]);
    }

    fn wall_model() -> ImportedModel {
        // a wall facing +Z (south): x from 1 to 5 (4 m), y from 0 to 3, at z = 2; two triangles
        // wound counter-clockwise seen from +Z
        let p = vec![
            [1.0, 0.0, 2.0],
            [5.0, 0.0, 2.0],
            [5.0, 3.0, 2.0],
            [1.0, 3.0, 2.0],
        ];
        ImportedModel {
            normals: vec![[0.0, 0.0, 1.0]; 4],
            positions: p,
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            triangle_faces: vec![0, 0],
            faces: vec![Face {
                name: "S1:side(2)".into(),
                body: 0,
            }],
            bodies: vec![Body {
                id: "S1".into(),
                name: "House".into(),
                color: None,
                triangles: 0..2,
                bounds: [[1.0, 0.0, 2.0], [5.0, 3.0, 2.0]],
            }],
            edge: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_wall_facing_south_has_u_to_the_east_and_v_up() {
        let f = face_frame(&wall_model(), 0, [0.0, 1.0, 0.0]).unwrap();
        assert_eq!(f.normal, [0.0, 0.0, 1.0]);
        // seen from outside (looking toward -Z) east is on the right
        assert_eq!(f.u_axis, [1.0, 0.0, 0.0]);
        assert_eq!(f.v_axis, [0.0, 1.0, 0.0]);
        assert_eq!(f.origin, [1.0, 0.0, 2.0]);
        assert_eq!(f.size, [4.0, 3.0]);
        assert_eq!(f.uv([3.0, 1.5, 2.0]), [2.0, 1.5]);
        assert_eq!(
            f.whole(),
            Region {
                u0: 0.0,
                v0: 0.0,
                u1: 4.0,
                v1: 3.0
            }
        );
    }

    #[test]
    fn the_frame_of_a_real_extrusion_side() {
        let mut s = crate::Session::new(Default::default());
        s.run(
            "shape.add_extrusion",
            serde_json::json!({"name": "House", "outline": [[2, 2], [8, 2], [8, 8], [2, 8]], "height": 3.0}),
        )
        .unwrap();
        let mesh = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        // the side facing +Z (south, outside) is the one whose normal is +Z
        let mut found = 0;
        for (i, face) in mesh.faces.iter().enumerate() {
            if !face.name.contains(":side(") {
                continue;
            }
            let f = face_frame(&mesh, i, [0.0, 1.0, 0.0]).unwrap();
            assert!(
                (f.size[0] - 6.0).abs() < 1e-4 && (f.size[1] - 3.0).abs() < 1e-4,
                "{f:?}"
            );
            assert!(f.v_axis[1] > 0.999, "{f:?}");
            if f.normal[2] > 0.99 {
                found += 1;
                assert!(f.u_axis[0] > 0.99, "{f:?}");
                assert!((f.origin[0] - 2.0).abs() < 1e-4 && (f.origin[2] - 8.0).abs() < 1e-4);
            }
        }
        assert_eq!(found, 1);
    }

    #[test]
    fn a_wall_with_a_window_has_a_frame_that_ignores_the_reveals() {
        let mut s = crate::Session::new(Default::default());
        s.run(
            "shape.add_extrusion",
            serde_json::json!({"name": "House", "outline": [[2, 2], [8, 2], [8, 8], [2, 8]], "height": 3.0}),
        )
        .unwrap();
        s.run(
            "shape.add_opening",
            serde_json::json!({"id": "S1", "side": 2, "offset_m": 2.0}),
        )
        .unwrap();
        let mesh = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        let at = mesh
            .faces
            .iter()
            .position(|f| f.name == "S1:side(2)")
            .unwrap();
        let f = face_frame(&mesh, at, [0.0, 1.0, 0.0]).unwrap();
        assert!(
            (f.size[0] - 6.0).abs() < 1e-3 && (f.size[1] - 3.0).abs() < 1e-3,
            "{f:?}"
        );
        assert!(f.normal[2] > 0.99, "{f:?}");
    }

    #[test]
    fn faces_that_lie_flat_or_bend_have_no_frame() {
        let mut m = wall_model();
        let up = [0.0, 1.0, 0.0];
        // a floor: facing up
        m.positions = vec![
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 4.0],
            [4.0, 0.0, 4.0],
            [4.0, 0.0, 0.0],
        ];
        assert!(face_frame(&m, 0, up).unwrap_err().contains("flat"));
        let mut bent = wall_model();
        // one corner pulled out by two metres: the two triangles no longer share a plane
        bent.positions[2][2] = 4.0;
        assert!(face_frame(&bent, 0, up).unwrap_err().contains("not flat"));
        assert!(face_frame(&wall_model(), 3, up).is_err());
    }

    #[test]
    fn a_triangle_is_cut_along_the_region_without_losing_area() {
        let m = wall_model();
        let f = face_frame(&m, 0, [0.0, 1.0, 0.0]).unwrap();
        let region = Region {
            u0: 1.0,
            v0: 0.5,
            u1: 3.0,
            v1: 2.0,
        };
        let tri = [0usize, 1, 2].map(|i| Corner {
            position: m.positions[i],
            normal: m.normals[i],
        });
        let area = |ts: &[[Corner; 3]]| -> f32 {
            ts.iter()
                .map(|t| {
                    let n = cross(
                        sub(t[1].position, t[0].position),
                        sub(t[2].position, t[0].position),
                    );
                    dot(n, n).sqrt() / 2.0
                })
                .sum()
        };
        let (inside, outside) = split_by_region(&f, &region, tri);
        // the first triangle is half of the 4 x 3 wall: 6 m2; its part inside the 2 x 1.5 box
        // below the diagonal v = 0.75 u: the box corner (3, 2) is above it, so the part is
        // the box less the triangle above the diagonal
        let total = area(&inside) + area(&outside);
        assert!((total - 6.0).abs() < 1e-4, "{total}");
        for t in &inside {
            for c in t {
                let [u, v] = f.uv(c.position);
                assert!(u > 1.0 - 1e-4 && u < 3.0 + 1e-4 && v > 0.5 - 1e-4 && v < 2.0 + 1e-4);
            }
            // winding kept: still facing +Z
            let n = cross(
                sub(t[1].position, t[0].position),
                sub(t[2].position, t[0].position),
            );
            assert!(n[2] > 0.0);
        }
        // a triangle wholly outside stays whole
        let far = Region {
            u0: 10.0,
            v0: 0.0,
            u1: 12.0,
            v1: 1.0,
        };
        let (inside, outside) = split_by_region(&f, &far, tri);
        assert!(inside.is_empty());
        assert!((area(&outside) - 6.0).abs() < 1e-4);
        // one wholly inside stays whole
        let all = Region {
            u0: -1.0,
            v0: -1.0,
            u1: 9.0,
            v1: 9.0,
        };
        let (inside, outside) = split_by_region(&f, &all, tri);
        assert!(outside.is_empty());
        assert_eq!(inside.len(), 1);
    }

    #[test]
    fn a_photo_is_checked_when_validated() {
        let p = WallPhoto {
            id: "p1".into(),
            name: "Facade".into(),
            file: "f.jpg".into(),
            size_px: [100, 80],
            target: Target::Face {
                owner: "S1".into(),
                face: "S1:side(2)".into(),
            },
            corners_px: image_corners([100, 80]),
            region: None,
            extensions: Default::default(),
        };
        assert!(p.validate().is_ok());
        let mut q = p.clone();
        q.target = Target::Owner { owner: "S1".into() };
        assert!(q.validate().unwrap_err().contains("one face"));
        let mut q = p.clone();
        q.region = Some(Region {
            u0: 2.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        });
        assert!(q.validate().is_err());
        let mut q = p;
        q.size_px = [0, 5];
        assert!(q.validate().is_err());
    }
}
