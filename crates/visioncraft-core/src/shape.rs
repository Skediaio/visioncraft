//! Shapes a private person draws instead of using CAD: flat areas and extruded outlines, meshed
//! on demand with stable face names.

use crate::project::Extensions;
use crate::{Body, Face, ImportedModel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A flat polygon (lawn, terrace, path, pond, plant bed) at `elevation` metres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Area {
    /// `S1`, `S2`, ...: face names are `<id>:top` and so on.
    pub id: String,
    pub name: String,
    pub layer: String,
    /// Points `(x, z)` in metres; either winding.
    pub outline: Vec<[f32; 2]>,
    pub elevation: f32,
    /// Lie on the terrain: use [`shape_mesh_on`], with `elevation` as the height above the ground.
    #[serde(default)]
    pub drape: bool,
    /// Data of other tools by namespace, kept untouched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

/// A closed polygon extruded up (house volume, block), or an open polyline given `thickness` and
/// extruded (wall, fence, hedge block).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extrusion {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub outline: Vec<[f32; 2]>,
    pub closed: bool,
    pub base: f32,
    pub height: f32,
    /// Width of an open extrusion; unused when `closed`.
    pub thickness: f32,
    /// Data of other tools by namespace, kept untouched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Area(Area),
    Extrusion(Extrusion),
}

impl Shape {
    pub fn id(&self) -> &str {
        match self {
            Self::Area(a) => &a.id,
            Self::Extrusion(e) => &e.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Area(a) => &a.name,
            Self::Extrusion(e) => &e.name,
        }
    }

    pub fn layer(&self) -> &str {
        match self {
            Self::Area(a) => &a.layer,
            Self::Extrusion(e) => &e.layer,
        }
    }

    pub(crate) fn layer_mut(&mut self) -> &mut String {
        match self {
            Self::Area(a) => &mut a.layer,
            Self::Extrusion(e) => &mut e.layer,
        }
    }

    pub(crate) fn name_mut(&mut self) -> &mut String {
        match self {
            Self::Area(a) => &mut a.name,
            Self::Extrusion(e) => &mut e.name,
        }
    }

    pub fn outline(&self) -> &[[f32; 2]] {
        match self {
            Self::Area(a) => &a.outline,
            Self::Extrusion(e) => &e.outline,
        }
    }

    pub(crate) fn outline_mut(&mut self) -> &mut Vec<[f32; 2]> {
        match self {
            Self::Area(a) => &mut a.outline,
            Self::Extrusion(e) => &mut e.outline,
        }
    }

    /// Whether the last point connects back to the first.
    pub fn is_closed(&self) -> bool {
        match self {
            Self::Area(_) => true,
            Self::Extrusion(e) => e.closed,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShapeError {
    TooFewPoints {
        needed: usize,
    },
    /// A repeated point, or two edges of one outline that touch or cross.
    NotSimple(String),
    /// A height, thickness, elevation or coordinate that is not a usable number.
    BadNumber(&'static str),
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPoints { needed } => write!(f, "an outline needs at least {needed} points"),
            Self::NotSimple(why) => write!(f, "the outline is not a simple polygon: {why}"),
            Self::BadNumber(what) => write!(f, "{what} is not a usable number"),
        }
    }
}

impl std::error::Error for ShapeError {}

type P2 = [f32; 2];

fn cross2(o: P2, a: P2, b: P2) -> f64 {
    f64::from(a[0] - o[0]) * f64::from(b[1] - o[1])
        - f64::from(a[1] - o[1]) * f64::from(b[0] - o[0])
}

fn on_segment(a: P2, b: P2, p: P2) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Whether segments `ab` and `cd` share a point (touching counts).
fn segments_meet(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (d1, d2, d3, d4) = (
        cross2(c, d, a),
        cross2(c, d, b),
        cross2(a, b, c),
        cross2(a, b, d),
    );
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return true;
    }
    (d1 == 0.0 && on_segment(c, d, a))
        || (d2 == 0.0 && on_segment(c, d, b))
        || (d3 == 0.0 && on_segment(a, b, c))
        || (d4 == 0.0 && on_segment(a, b, d))
}

/// Checks a polygon (`closed`) or polyline for repeated points, spikes and self-intersection.
fn check_simple(points: &[P2], closed: bool) -> Result<(), ShapeError> {
    let needed = if closed { 3 } else { 2 };
    if points.len() < needed {
        return Err(ShapeError::TooFewPoints { needed });
    }
    if points.iter().flatten().any(|c| !c.is_finite()) {
        return Err(ShapeError::BadNumber("a coordinate"));
    }
    let edges = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    let edge = |i: usize| (points[i], points[(i + 1) % points.len()]);
    for i in 0..edges {
        let (a, b) = edge(i);
        if a == b {
            return Err(ShapeError::NotSimple(format!("edge {i} has no length")));
        }
        for j in i + 1..edges {
            let (c, d) = edge(j);
            let adjacent = j == i + 1 || (closed && i == 0 && j == edges - 1);
            if adjacent {
                // Neighbours share a point by design: only a fold back onto itself is wrong.
                let (shared, other) = if j == i + 1 { (b, d) } else { (a, c) };
                let before = if j == i + 1 { a } else { d };
                if cross2(before, shared, other) == 0.0
                    && (shared[0] - before[0]) * (other[0] - shared[0])
                        + (shared[1] - before[1]) * (other[1] - shared[1])
                        < 0.0
                {
                    return Err(ShapeError::NotSimple(format!(
                        "edges {i} and {j} fold back"
                    )));
                }
            } else if segments_meet(a, b, c, d) {
                return Err(ShapeError::NotSimple(format!("edges {i} and {j} meet")));
            }
        }
    }
    Ok(())
}

fn shoelace(ring: &[P2]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1])
        })
        .sum::<f64>()
        / 2.0
}

/// The ring around an open polyline `width` wide: left side forward, then right side back, with
/// mitred bends. Ring edge `i < n-1` is the left of polyline edge `i`, edge `n-1` the end cap,
/// then the right sides from the last polyline edge back to the first, and the start cap last.
fn thick_ring(line: &[P2], width: f32) -> Result<Vec<P2>, ShapeError> {
    let dir = |i: usize| {
        let (a, b) = (line[i], line[i + 1]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        [(b[0] - a[0]) / len, (b[1] - a[1]) / len]
    };
    let half = width / 2.0;
    let n = line.len();
    let mut left = Vec::with_capacity(n);
    for (i, p) in line.iter().enumerate() {
        let (d0, d1) = (dir(i.saturating_sub(1).min(n - 2)), dir(i.min(n - 2)));
        let (n0, n1) = ([-d0[1], d0[0]], [-d1[1], d1[0]]);
        let k = 1.0 + n0[0] * n1[0] + n0[1] * n1[1];
        if k < 0.1 {
            return Err(ShapeError::NotSimple(format!(
                "the bend at point {i} is too sharp"
            )));
        }
        let m = [(n0[0] + n1[0]) / k, (n0[1] + n1[1]) / k];
        left.push([p[0] + m[0] * half, p[1] + m[1] * half]);
    }
    let right = (0..n).map(|i| {
        let l = left[i];
        [2.0 * line[i][0] - l[0], 2.0 * line[i][1] - l[1]]
    });
    Ok(left.iter().copied().chain(right.rev()).collect())
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Collects flat-shaded faces; every face has its own vertices.
struct Builder {
    model: ImportedModel,
    triangles: Vec<Vec<[u32; 3]>>,
}

impl Builder {
    /// A triangle on `face`, wound so its normal agrees with `outward`; degenerate ones are dropped.
    fn tri(&mut self, face: usize, v: [[f32; 3]; 3], outward: [f32; 3]) {
        let n = cross3(sub3(v[1], v[0]), sub3(v[2], v[0]));
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len == 0.0 {
            return;
        }
        let flip = n[0] * outward[0] + n[1] * outward[1] + n[2] * outward[2] < 0.0;
        let (v, n) = if flip {
            ([v[0], v[2], v[1]], n.map(|c| -c / len))
        } else {
            (v, n.map(|c| c / len))
        };
        let base = self.model.positions.len() as u32;
        self.model.positions.extend(v);
        self.model.normals.extend([n; 3]);
        self.triangles[face].push([base, base + 1, base + 2]);
    }

    /// Triangulates `ring` (any winding) flat at height `y`.
    fn cap(&mut self, face: usize, ring: &[P2], y: f32, normal_y: f32) {
        let flat: Vec<f64> = ring
            .iter()
            .flat_map(|p| [f64::from(p[0]), f64::from(p[1])])
            .collect();
        // check_simple has passed, so earcut has a valid polygon; no triangles is only a degenerate sliver.
        let indices = earcutr::earcut(&flat, &[], 2).unwrap_or_default();
        for t in indices.as_chunks::<3>().0 {
            let v = [t[0], t[1], t[2]].map(|i| [ring[i][0], y, ring[i][1]]);
            self.tri(face, v, [0.0, normal_y, 0.0]);
        }
    }

    fn finish(mut self, id: &str, name: &str, names: Vec<String>) -> ImportedModel {
        let mut m = std::mem::take(&mut self.model);
        for (name, tris) in names.into_iter().zip(self.triangles) {
            let face = m.faces.len() as u32;
            m.triangle_faces.extend(tris.iter().map(|_| face));
            m.triangles.extend(tris);
            m.faces.push(Face { name, body: 0 });
        }
        let end = m.triangles.len() as u32;
        let body = Body::new(&m, (id.into(), name.into()), None, 0..end);
        m.bodies.push(body);
        m
    }
}

/// Triangulates a shape into one body with stable face names: `<id>:top`, and for an extrusion
/// `<id>:bottom`, `<id>:side(<n>)` with `n` the index of the outline edge (so moving a vertex
/// keeps every name), and for an open extrusion also `<id>:start` and `<id>:end`. The sides of an
/// open extrusion's edge `n` are one face holding both its left and right wall. Normals point out
/// whichever way the outline winds. World coordinates, no transform.
pub fn shape_mesh(shape: &Shape) -> Result<ImportedModel, ShapeError> {
    check_simple(shape.outline(), shape.is_closed())?;
    let id = shape.id();
    match shape {
        Shape::Area(a) => {
            if !a.elevation.is_finite() {
                return Err(ShapeError::BadNumber("elevation"));
            }
            let mut b = Builder {
                model: ImportedModel::default(),
                triangles: vec![Vec::new()],
            };
            b.cap(0, &a.outline, a.elevation, 1.0);
            Ok(b.finish(id, &a.name, vec![format!("{id}:top")]))
        }
        Shape::Extrusion(e) => {
            if !(e.base.is_finite() && e.height.is_finite() && e.height > 0.0) {
                return Err(ShapeError::BadNumber("base or height"));
            }
            let (ring, ring_face): (Vec<P2>, Vec<usize>) = if e.closed {
                (
                    e.outline.clone(),
                    (0..e.outline.len()).map(|i| 2 + i).collect(),
                )
            } else {
                if !(e.thickness.is_finite() && e.thickness > 0.0) {
                    return Err(ShapeError::BadNumber("thickness"));
                }
                let ring = thick_ring(&e.outline, e.thickness)?;
                check_simple(&ring, true)?;
                let edges = e.outline.len() - 1;
                // Faces: top, bottom, side(0..edges), start, end.
                let faces = (0..edges)
                    .map(|i| 2 + i)
                    .chain([2 + edges + 1])
                    .chain((0..edges).rev().map(|i| 2 + i))
                    .chain([2 + edges]);
                (ring, faces.collect())
            };
            let sides = if e.closed {
                e.outline.len()
            } else {
                e.outline.len() - 1
            };
            let mut names = vec![format!("{id}:top"), format!("{id}:bottom")];
            names.extend((0..sides).map(|n| format!("{id}:side({n})")));
            if !e.closed {
                names.extend([format!("{id}:start"), format!("{id}:end")]);
            }
            let mut b = Builder {
                model: ImportedModel::default(),
                triangles: vec![Vec::new(); names.len()],
            };
            let (lo, hi) = (e.base, e.base + e.height);
            b.cap(0, &ring, hi, 1.0);
            b.cap(1, &ring, lo, -1.0);
            // For a positive shoelace (counter-clockwise on the x/z plane) the wall of an edge
            // `d` faces along `(dz, -dx)`; for a negative one the other way.
            let sign = if shoelace(&ring) > 0.0 { 1.0 } else { -1.0 };
            for (i, &face) in ring_face.iter().enumerate() {
                let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
                let outward = [sign * (q[1] - p[1]), 0.0, -sign * (q[0] - p[0])];
                let c = |pt: P2, y: f32| [pt[0], y, pt[1]];
                b.tri(face, [c(p, lo), c(q, lo), c(q, hi)], outward);
                b.tri(face, [c(p, lo), c(q, hi), c(p, hi)], outward);
            }
            Ok(b.finish(id, &e.name, names))
        }
    }
}

/// Like [`shape_mesh`], but an [`Area`] with `drape` is subdivided into triangles of at most
/// about 1 m (one uniform subdivision, so neighbouring triangles share their edge vertices; at
/// most 64 steps per triangle edge) and every vertex lifted to `ground(x, z)` (0 where there is
/// none) plus the elevation. Normals follow the ground. Face names are the same.
pub fn shape_mesh_on(
    shape: &Shape,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> Result<ImportedModel, ShapeError> {
    let Shape::Area(a) = shape else {
        return shape_mesh(shape);
    };
    if !a.drape {
        return shape_mesh(shape);
    }
    let flat = shape_mesh(shape)?;
    let h = |x: f32, z: f32| ground(x, z).unwrap_or(0.0) + a.elevation;
    let normal_at = |x: f32, z: f32| {
        let e = 0.5;
        let n = [
            -(h(x + e, z) - h(x - e, z)) / (2.0 * e),
            1.0,
            -(h(x, z + e) - h(x, z - e)) / (2.0 * e),
        ];
        let l = (n[0] * n[0] + 1.0 + n[2] * n[2]).sqrt();
        n.map(|c| c / l)
    };
    let tris: Vec<[[f32; 2]; 3]> = flat
        .triangles
        .iter()
        .map(|t| t.map(|i| [flat.positions[i as usize][0], flat.positions[i as usize][2]]))
        .collect();
    let longest = tris
        .iter()
        .flat_map(|t| {
            (0..3).map(|k| (t[k][0] - t[(k + 1) % 3][0]).hypot(t[k][1] - t[(k + 1) % 3][1]))
        })
        .fold(0.0f32, f32::max);
    let n = (longest.ceil() as usize).clamp(1, 64);
    let mut b = Builder {
        model: ImportedModel::default(),
        triangles: vec![Vec::new()],
    };
    for t in &tris {
        let point = |i: usize, j: usize| {
            // Barycentric grid point (i, j) of the n-step subdivision.
            let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
            let p = [
                t[0][0] + (t[1][0] - t[0][0]) * u + (t[2][0] - t[0][0]) * v,
                t[0][1] + (t[1][1] - t[0][1]) * u + (t[2][1] - t[0][1]) * v,
            ];
            (p, h(p[0], p[1]))
        };
        let mut push = |corners: [(usize, usize); 3]| {
            let pts = corners.map(|(i, j)| point(i, j));
            // Counter-clockwise seen from above: the xz area must be negative (see `Builder::tri`).
            let area = (pts[1].0[0] - pts[0].0[0]) * (pts[2].0[1] - pts[0].0[1])
                - (pts[1].0[1] - pts[0].0[1]) * (pts[2].0[0] - pts[0].0[0]);
            let order = if area < 0.0 { [0, 1, 2] } else { [0, 2, 1] };
            let base = b.model.positions.len() as u32;
            for k in order {
                let (p, y) = pts[k];
                b.model.positions.push([p[0], y, p[1]]);
                b.model.normals.push(normal_at(p[0], p[1]));
            }
            b.triangles[0].push([base, base + 1, base + 2]);
        };
        for i in 0..n {
            for j in 0..n - i {
                push([(i, j), (i + 1, j), (i, j + 1)]);
                if i + j + 1 < n {
                    push([(i + 1, j), (i + 1, j + 1), (i, j + 1)]);
                }
            }
        }
    }
    Ok(b.finish(&a.id, &a.name, vec![format!("{}:top", a.id)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: f32) -> Vec<P2> {
        vec![[0.0, 0.0], [side, 0.0], [side, side], [0.0, side]]
    }

    fn block(outline: Vec<P2>) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Block".into(),
            layer: "shapes".into(),
            outline,
            closed: true,
            base: 1.0,
            height: 3.0,
            thickness: 0.2,
            extensions: Default::default(),
        })
    }

    fn wall(outline: Vec<P2>) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S2".into(),
            name: "Wall".into(),
            layer: "shapes".into(),
            outline,
            closed: false,
            base: 0.0,
            height: 2.0,
            thickness: 0.4,
            extensions: Default::default(),
        })
    }

    /// Signed volume by the divergence theorem: positive for a closed, outward-wound mesh.
    fn volume(m: &ImportedModel) -> f32 {
        m.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                let n = cross3(b, c);
                (a[0] * n[0] + a[1] * n[1] + a[2] * n[2]) / 6.0
            })
            .sum()
    }

    /// Every directed edge must be met by its reverse: no holes, no flipped triangle.
    fn is_closed(m: &ImportedModel) -> bool {
        let key = |p: [f32; 3]| p.map(f32::to_bits);
        let mut edges = std::collections::BTreeMap::new();
        for t in &m.triangles {
            for k in 0..3 {
                let (a, b) = (
                    key(m.positions[t[k] as usize]),
                    key(m.positions[t[(k + 1) % 3] as usize]),
                );
                *edges.entry((a, b)).or_insert(0i32) += 1;
                *edges.entry((b, a)).or_insert(0i32) -= 1;
            }
        }
        edges.values().all(|&n| n == 0)
    }

    #[test]
    fn closed_extrusion_is_a_closed_outward_solid_in_both_windings() {
        for outline in [square(2.0), square(2.0).into_iter().rev().collect()] {
            let m = shape_mesh(&block(outline)).unwrap();
            assert!((volume(&m) - 12.0).abs() < 1e-4, "{}", volume(&m));
            assert!(is_closed(&m));
            let top = m.faces.iter().position(|f| f.name == "S1:top").unwrap() as u32;
            for (t, f) in m.triangles.iter().zip(&m.triangle_faces) {
                if *f == top {
                    assert!(m.normals[t[0] as usize] == [0.0, 1.0, 0.0]);
                }
            }
            assert_eq!(m.bodies.len(), 1);
            assert_eq!(m.bodies[0].bounds, [[0.0, 1.0, 0.0], [2.0, 4.0, 2.0]]);
        }
    }

    #[test]
    fn concave_outline_keeps_volume() {
        let l = vec![
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 1.0],
            [1.0, 1.0],
            [1.0, 3.0],
            [0.0, 3.0],
        ];
        let m = shape_mesh(&block(l)).unwrap();
        assert!((volume(&m) - 6.0 * 3.0).abs() < 1e-4);
        assert!(is_closed(&m));
    }

    #[test]
    fn open_extrusion_is_a_solid_with_start_and_end() {
        let m = shape_mesh(&wall(vec![[0.0, 0.0], [5.0, 0.0], [5.0, 3.0]])).unwrap();
        assert!(is_closed(&m));
        // The mitred L covers 5.2 * 0.4 + 2.8 * 0.4 = 3.2 m2, two metres high.
        assert!((volume(&m) - 6.4).abs() < 1e-3, "{}", volume(&m));
        let names: Vec<_> = m.faces.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "S2:top",
                "S2:bottom",
                "S2:side(0)",
                "S2:side(1)",
                "S2:start",
                "S2:end"
            ]
        );
        let rev = shape_mesh(&wall(vec![[5.0, 3.0], [5.0, 0.0], [0.0, 0.0]])).unwrap();
        assert!((volume(&rev) - volume(&m)).abs() < 1e-3 && volume(&m) > 0.0);
    }

    #[test]
    fn area_is_one_upward_face() {
        let a = Shape::Area(Area {
            id: "S3".into(),
            name: "Lawn".into(),
            layer: "shapes".into(),
            outline: square(3.0).into_iter().rev().collect(),
            elevation: 0.5,
            drape: false,
            extensions: Default::default(),
        });
        let m = shape_mesh(&a).unwrap();
        assert_eq!(m.faces.len(), 1);
        assert_eq!(m.faces[0].name, "S3:top");
        assert!(m.normals.iter().all(|n| *n == [0.0, 1.0, 0.0]));
        assert!(m.positions.iter().all(|p| p[1] == 0.5));
    }

    #[test]
    fn face_names_survive_a_moved_vertex() {
        let names = |o: Vec<P2>| -> Vec<String> {
            shape_mesh(&block(o))
                .unwrap()
                .faces
                .into_iter()
                .map(|f| f.name)
                .collect()
        };
        let mut moved = square(2.0);
        moved[2] = [5.0, 3.0];
        assert_eq!(names(square(2.0)), names(moved));
    }

    #[test]
    fn bad_outlines_are_rejected() {
        let bow_tie = vec![[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]];
        assert!(matches!(
            shape_mesh(&block(bow_tie)),
            Err(ShapeError::NotSimple(_))
        ));
        assert!(matches!(
            shape_mesh(&block(vec![[0.0, 0.0], [1.0, 0.0]])),
            Err(ShapeError::TooFewPoints { needed: 3 })
        ));
        let repeated = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert!(shape_mesh(&block(repeated)).is_err());
        let crossing_wall = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [1.0, -1.0]];
        assert!(shape_mesh(&wall(crossing_wall)).is_err());
        assert!(shape_mesh(&wall(vec![[0.0, 0.0], [2.0, 0.0], [0.0, 0.01]])).is_err());
    }
}

#[cfg(test)]
mod drape_tests {
    use super::*;

    #[test]
    fn draped_area_follows_the_ground_with_the_same_names() {
        let outline = vec![[0.0, 0.0], [6.0, 0.0], [6.0, 4.0], [0.0, 4.0]];
        let mut a = Area {
            id: "S1".into(),
            name: "Lawn".into(),
            layer: "shapes".into(),
            outline,
            elevation: 0.1,
            drape: true,
            extensions: Default::default(),
        };
        let slope = |x: f32, _: f32| Some(0.5 * x);
        let m = shape_mesh_on(&Shape::Area(a.clone()), &slope).unwrap();
        assert_eq!(m.faces.len(), 1);
        assert_eq!(m.faces[0].name, "S1:top");
        assert!(m.triangles.len() > 24);
        assert!(
            m.positions
                .iter()
                .all(|p| (p[1] - (0.5 * p[0] + 0.1)).abs() < 1e-5)
        );
        assert!(m.normals.iter().all(|n| n[1] > 0.8 && n[0] < 0.0));
        // Counter-clockwise from above, and its area is the slope's: 24 m2 * sqrt(1.25).
        let area: f32 = m
            .triangles
            .iter()
            .map(|t| {
                let [p, q, r] = t.map(|i| m.positions[i as usize]);
                let n = cross3(sub3(q, p), sub3(r, p));
                assert!(n[1] > 0.0);
                (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0
            })
            .sum();
        assert!((area - 24.0 * 1.25f32.sqrt()).abs() < 0.01, "{area}");
        a.drape = false;
        assert_eq!(
            shape_mesh_on(&Shape::Area(a.clone()), &slope),
            shape_mesh(&Shape::Area(a))
        );
    }
}
