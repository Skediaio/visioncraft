//! Ray queries against an imported model, in world space.

use crate::ImportedModel;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Along the (unnormalised) direction: `point = origin + dir * distance`.
    pub distance: f32,
    pub point: [f32; 3],
    /// Geometric normal of the triangle, unit length, facing the ray origin.
    pub normal: [f32; 3],
    /// Index into `model.triangles`.
    pub triangle: usize,
    /// Index into `model.faces`.
    pub face: usize,
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

/// Slab test: does the ray reach the box within `max` distance.
fn hits_box(origin: [f32; 3], dir: [f32; 3], bounds: &[[f32; 3]; 2], max: f32) -> bool {
    let (mut t0, mut t1) = (0.0f32, max);
    for k in 0..3 {
        if dir[k] == 0.0 {
            if origin[k] < bounds[0][k] || origin[k] > bounds[1][k] {
                return false;
            }
            continue;
        }
        let (a, b) = (
            (bounds[0][k] - origin[k]) / dir[k],
            (bounds[1][k] - origin[k]) / dir[k],
        );
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    t0 <= t1
}

/// The nearest hit of the ray `origin + dir * t`, `t > 0`, or `None`. Triangles are hit from both
/// sides. Möller-Trumbore, after a per-body bounding-box reject.
pub fn raycast(model: &ImportedModel, origin: [f32; 3], dir: [f32; 3]) -> Option<Hit> {
    let mut best: Option<(f32, usize)> = None;
    for body in &model.bodies {
        // shortcut: brute force over triangles, add a BVH above about one million triangles
        if !hits_box(
            origin,
            dir,
            &body.bounds,
            best.map_or(f32::INFINITY, |b| b.0),
        ) {
            continue;
        }
        for i in body.triangles.start as usize..body.triangles.end as usize {
            let [a, b, c] = model.triangles[i].map(|v| model.positions[v as usize]);
            let (e1, e2) = (sub(b, a), sub(c, a));
            let p = cross(dir, e2);
            let det = dot(e1, p);
            if det.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / det;
            let s = sub(origin, a);
            let u = dot(s, p) * inv;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = cross(s, e1);
            let v = dot(dir, q) * inv;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let t = dot(e2, q) * inv;
            if t > 1e-6 && best.is_none_or(|(d, _)| t < d) {
                best = Some((t, i));
            }
        }
    }
    let (distance, triangle) = best?;
    let [a, b, c] = model.triangles[triangle].map(|v| model.positions[v as usize]);
    let n = cross(sub(b, a), sub(c, a));
    let len = dot(n, n).sqrt();
    let facing = if dot(n, dir) > 0.0 { -1.0 } else { 1.0 };
    Some(Hit {
        distance,
        point: [0, 1, 2].map(|k| origin[k] + dir[k] * distance),
        normal: n.map(|c| facing * c / len),
        triangle,
        face: model.triangle_faces[triangle] as usize,
    })
}

/// Height of the highest surface at `(x, z)`: a ray straight down from above the model. `None`
/// when nothing is there.
pub fn ground_height(model: &ImportedModel, x: f32, z: f32) -> Option<f32> {
    let top = model
        .bodies
        .iter()
        .map(|b| b.bounds[1][1])
        .fold(f32::NEG_INFINITY, f32::max);
    if !top.is_finite() {
        return None;
    }
    raycast(model, [x, top + 1.0, z], [0.0, -1.0, 0.0]).map(|h| h.point[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import_3dcraft;

    fn plate() -> ImportedModel {
        import_3dcraft(include_bytes!("../tests/data/plate.3dcraft")).unwrap()
    }

    #[test]
    fn hits_the_top_face_of_the_plate() {
        // The plate is 40 x 20 x 10 mm; the extrusion (3dcraft +Z) is world +Y.
        let m = plate();
        let hit = raycast(&m, [0.02, 1.0, -0.01], [0.0, -1.0, 0.0]).unwrap();
        assert!((hit.distance - 0.99).abs() < 1e-5);
        assert!((hit.point[1] - 0.01).abs() < 1e-6);
        assert_eq!(hit.normal, [0.0, 1.0, 0.0]);
        assert_eq!(m.faces[hit.face].name, "F2:endCap(e5,e6,e7,e8)");
        // From below the normal faces the ray origin, and it is the bottom face.
        let up = raycast(&m, [0.02, -1.0, -0.01], [0.0, 1.0, 0.0]).unwrap();
        assert_eq!(up.normal, [0.0, -1.0, 0.0]);
        assert_eq!(m.faces[up.face].name, "F2:startCap(e5,e6,e7,e8)");
    }

    #[test]
    fn misses_return_none() {
        let m = plate();
        assert_eq!(raycast(&m, [0.5, 1.0, 0.5], [0.0, -1.0, 0.0]), None);
        assert_eq!(raycast(&m, [0.02, 1.0, -0.01], [0.0, 1.0, 0.0]), None);
        assert_eq!(ground_height(&m, 0.5, 0.5), None);
    }

    #[test]
    fn ground_height_matches() {
        let h = ground_height(&plate(), 0.01, -0.005).unwrap();
        assert!((h - 0.01).abs() < 1e-6);
    }
}
