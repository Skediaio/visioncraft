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

/// Möller-Trumbore: the distance `t > 0` along the ray to triangle `i`, from either side.
fn triangle_distance(
    model: &ImportedModel,
    i: usize,
    origin: [f32; 3],
    dir: [f32; 3],
) -> Option<f32> {
    let [a, b, c] = model.triangles[i].map(|v| model.positions[v as usize]);
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(origin, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 1e-6).then_some(t)
}

/// Whether the ray `origin + dir * t`, `t > 0`, hits the model at all: stops at the first
/// triangle found, so it is the cheap test for shadows.
pub fn raycast_any(model: &ImportedModel, origin: [f32; 3], dir: [f32; 3]) -> bool {
    model.bodies.iter().any(|body| {
        hits_box(origin, dir, &body.bounds, f32::INFINITY)
            && (body.triangles.start as usize..body.triangles.end as usize)
                .any(|i| triangle_distance(model, i, origin, dir).is_some())
    })
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
            if let Some(t) = triangle_distance(model, i, origin, dir)
                && best.is_none_or(|(d, _)| t < d)
            {
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

// ------------------------------------------------------------------------------ plants

/// What a plant is to a ray: a crown as an ellipsoid and a trunk as a cylinder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantSolids {
    /// Centre and half sizes of the crown ellipsoid; `None` for a plant without a crown that
    /// shades or blocks (grass, a perennial).
    pub crown: Option<([f32; 3], [f32; 3])>,
    /// Foot, radius and height of the trunk; `None` for a plant without one.
    pub trunk: Option<([f32; 3], f32, f32)>,
}

/// The solids of a plant `age_years` old standing at `position` at `scale`, from the growth
/// table: trees and palms have a crown over the upper three quarters of their height on a
/// trunk, conifers, shrubs and clipped plants (hedges) a crown over the whole height.
pub fn plant_solids(asset: &str, position: [f32; 3], scale: f32, age_years: f32) -> PlantSolids {
    use crate::growth::{Grower, growth_of};
    let gr = growth_of(asset, None);
    let size = gr.size(age_years);
    let height = gr.mature_height_m * size.height * scale;
    let width = gr.mature_width_m * size.width * scale;
    let (centre_k, radius_k, trunk) = match gr.grower {
        Grower::Tree | Grower::Palm => (0.625, 0.375, true),
        Grower::Conifer => (0.5, 0.5, true),
        Grower::Shrub | Grower::Clipped => (0.5, 0.5, false),
        _ => (0.0, 0.0, false),
    };
    let crown = (radius_k > 0.0 && height >= 0.3 && width > 0.0).then(|| {
        (
            [position[0], position[1] + height * centre_k, position[2]],
            [width / 2.0, height * radius_k, width / 2.0],
        )
    });
    let trunk = trunk.then(|| {
        let d = gr.trunk_diameter_m(age_years) * scale;
        (position, (d / 2.0).max(0.04), height * 0.5)
    });
    PlantSolids { crown, trunk }
}

/// The distance `t > 0` along `origin + dir * t` to the ellipsoid, the near side (or the far
/// one from inside).
pub fn ray_ellipsoid(
    origin: [f32; 3],
    dir: [f32; 3],
    center: [f32; 3],
    radii: [f32; 3],
) -> Option<f32> {
    let o: [f32; 3] = std::array::from_fn(|k| (origin[k] - center[k]) / radii[k]);
    let e: [f32; 3] = std::array::from_fn(|k| dir[k] / radii[k]);
    let a = dot(e, e);
    let b = dot(o, e);
    let c = dot(o, o) - 1.0;
    let disc = b * b - a * c;
    if a <= 0.0 || disc < 0.0 {
        return None;
    }
    let r = disc.sqrt();
    let (near, far) = ((-b - r) / a, (-b + r) / a);
    if near > 1e-6 {
        Some(near)
    } else if far > 1e-6 {
        Some(far)
    } else {
        None
    }
}

/// The distance `t > 0` to an upright cylinder of `radius` and `height` standing on `foot`.
pub fn ray_cylinder(
    origin: [f32; 3],
    dir: [f32; 3],
    foot: [f32; 3],
    radius: f32,
    height: f32,
) -> Option<f32> {
    let (ox, oz) = (origin[0] - foot[0], origin[2] - foot[2]);
    let a = dir[0] * dir[0] + dir[2] * dir[2];
    let mut best: Option<f32> = None;
    let mut take = |t: f32| {
        if t > 1e-6 && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    };
    let in_height = |t: f32| {
        let y = origin[1] + dir[1] * t - foot[1];
        (0.0..=height).contains(&y)
    };
    if a > 1e-12 {
        let b = ox * dir[0] + oz * dir[2];
        let c = ox * ox + oz * oz - radius * radius;
        let disc = b * b - a * c;
        if disc >= 0.0 {
            let r = disc.sqrt();
            for t in [(-b - r) / a, (-b + r) / a] {
                if in_height(t) {
                    take(t);
                }
            }
        }
    }
    // The caps.
    if dir[1].abs() > 1e-9 {
        for cap in [foot[1], foot[1] + height] {
            let t = (cap - origin[1]) / dir[1];
            let (x, z) = (ox + dir[0] * t, oz + dir[2] * t);
            if x * x + z * z <= radius * radius {
                take(t);
            }
        }
    }
    best
}

impl PlantSolids {
    /// The nearest hit of the ray: the distance and `crown` or `trunk`.
    pub fn hit(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<(f32, &'static str)> {
        let crown = self
            .crown
            .and_then(|(c, r)| ray_ellipsoid(origin, dir, c, r))
            .map(|t| (t, "crown"));
        let trunk = self
            .trunk
            .and_then(|(f, r, h)| ray_cylinder(origin, dir, f, r, h))
            .map(|t| (t, "trunk"));
        match (crown, trunk) {
            (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
            (a, b) => a.or(b),
        }
    }
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
    fn a_ray_hits_a_crown_and_a_trunk_of_a_grown_tree() {
        // A grown birch: the crown ellipsoid over the upper part, a trunk below it.
        let tree = plant_solids("flora:betula-pendula", [5.0, 0.0, 5.0], 1.0, 60.0);
        let (centre, radii) = tree.crown.expect("a crown");
        assert!(radii[1] > 2.0 && radii[0] > 1.0, "{radii:?}");
        // From the side at the crown's height: a crown hit at its near edge.
        let (t, what) = tree
            .hit([centre[0] - 20.0, centre[1], centre[2]], [1.0, 0.0, 0.0])
            .unwrap();
        assert_eq!(what, "crown");
        assert!((t - (20.0 - radii[0])).abs() < 1e-3, "{t}");
        // Low, below the crown: the trunk.
        let (_, low) = tree.hit([-10.0, 0.5, 5.0], [1.0, 0.0, 0.0]).unwrap();
        assert_eq!(low, "trunk");
        // A miss above the tree and to the side.
        assert!(tree.hit([-10.0, 40.0, 5.0], [1.0, 0.0, 0.0]).is_none());
        assert!(tree.hit([5.0, 0.5, 20.0], [1.0, 0.0, 0.0]).is_none());
        // A lawn has no solids.
        let lawn = plant_solids("flora:lawn-tuft", [0.0; 3], 1.0, 5.0);
        assert!(lawn.crown.is_none() && lawn.trunk.is_none());
        // Straight down onto a cylinder's top cap.
        let t = ray_cylinder([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 0.0], 0.2, 2.0).unwrap();
        assert!((t - 3.0).abs() < 1e-5);
    }

    #[test]
    fn ground_height_matches() {
        let h = ground_height(&plate(), 0.01, -0.005).unwrap();
        assert!((h - 0.01).abs() < 1e-6);
    }
}
