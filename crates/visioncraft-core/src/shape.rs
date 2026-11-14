//! Shapes a private person draws instead of using CAD: flat areas and extruded outlines, meshed
//! on demand with stable face names.
//!
//! # How shapes sit on the ground
//!
//! - An area with `drape` (the default for new areas; `terrain.create` turns it on for areas that
//!   lay at height 0) lies on the ground: its surface follows the terrain, `elevation` above it.
//!   Without a terrain the ground is height 0, so it is flat at `elevation`. An area without
//!   `drape` is flat at `elevation` whatever the ground does (a raised deck, a pond's water).
//! - Where areas overlap, the later one in the shape list is on top. Each area is lifted
//!   [`AREA_LIFT_M`] above its surface plus [`STACK_STEP_M`] for every earlier area it overlaps
//!   ([`area_lifts`]), so a terrace drawn on a lawn, or a path on the terrace, never fights with
//!   what is under it; lawn grass does not grow where an area lies on top of it ([`areas_over`]).
//!   Water is an area too: a pond drawn in a lawn covers the lawn's grass.
//! - An extrusion (house block, wall) stands on the lowest ground under its outline
//!   ([`footing`]): `base` is measured from there, so a house on a slope does not float on its
//!   low side nor sink on its high side.

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
    /// A closed block's roof, windows, doors and plinth (`crate::house`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub house: Option<crate::house::House>,
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

impl crate::house::Sink for Builder {
    fn tri(&mut self, face: usize, v: [[f32; 3]; 3], outward: [f32; 3]) {
        Builder::tri(self, face, v, outward);
    }
}

/// `shoelace` for the house module.
pub(crate) fn shoelace_f32(ring: &[P2]) -> f64 {
    shoelace(ring)
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
            let house = e.house.as_ref().filter(|_| e.closed);
            let first_house = names.len();
            if house.is_some() {
                names.extend(crate::house::HOUSE_FACES.map(|f| format!("{id}:{f}")));
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
            if let Some(h) = house {
                crate::house::walls(
                    &mut b,
                    h,
                    &ring,
                    sign,
                    (lo, hi),
                    &|i| ring_face[i],
                    first_house,
                );
                let note = crate::house::roof(&mut b, h, &ring, hi, first_house);
                let mut m = b.finish(id, &e.name, names);
                m.warnings.extend(note);
                return Ok(m);
            }
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

/// Every area sits this far above its surface, so it never fights with the terrain under it.
pub const AREA_LIFT_M: f32 = 0.02;
/// And this much more for every earlier area it overlaps.
pub const STACK_STEP_M: f32 = 0.01;

pub(crate) fn overlaps(a: &[P2], b: &[P2]) -> bool {
    let inside = |p: P2, poly: &[P2]| crate::planting::point_in_polygon(p, poly);
    let crosses = |p: P2, q: P2, r: P2, s: P2| {
        let o = |a: P2, b: P2, c: P2| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        (o(p, q, r) > 0.0) != (o(p, q, s) > 0.0) && (o(r, s, p) > 0.0) != (o(r, s, q) > 0.0)
    };
    let edges = |poly: &[P2]| {
        let n = poly.len();
        (0..n)
            .map(move |i| (poly[i], poly[(i + 1) % n]))
            .collect::<Vec<_>>()
    };
    a.iter().any(|p| inside(*p, b))
        || b.iter().any(|p| inside(*p, a))
        || edges(a)
            .iter()
            .any(|&(p, q)| edges(b).iter().any(|&(r, s)| crosses(p, q, r, s)))
}

/// How far each area is lifted over its surface (see the module docs): `(id, metres)` in shape
/// order.
pub fn area_lifts(shapes: &[Shape]) -> Vec<(String, f32)> {
    let areas: Vec<&Area> = shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Area(a) => Some(a),
            Shape::Extrusion(_) => None,
        })
        .collect();
    areas
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let under = areas[..i]
                .iter()
                .filter(|b| overlaps(&a.outline, &b.outline))
                .count();
            (a.id.clone(), AREA_LIFT_M + STACK_STEP_M * under as f32)
        })
        .collect()
}

/// The outlines of the areas on top of area `id` (later in the list and overlapping it): its
/// grass does not grow under them.
pub fn areas_over<'a>(shapes: &'a [Shape], id: &str) -> Vec<&'a [P2]> {
    let Some(i) = shapes.iter().position(|s| s.id() == id) else {
        return Vec::new();
    };
    let Shape::Area(a) = &shapes[i] else {
        return Vec::new();
    };
    shapes[i + 1..]
        .iter()
        .filter_map(|s| match s {
            Shape::Area(b) if overlaps(&a.outline, &b.outline) => Some(b.outline.as_slice()),
            _ => None,
        })
        .collect()
}

/// `outline` moved `d` outward along its edges' normals (mitred corners, capped at 3 d).
pub fn offset_outline(outline: &[[f32; 2]], d: f32) -> Vec<[f32; 2]> {
    let n = outline.len();
    if d <= 0.0 || n < 3 {
        return outline.to_vec();
    }
    let area2: f32 = (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    // Outward for a counter-clockwise ring (positive area in x, z) is to the right of an edge.
    let side = if area2 > 0.0 { 1.0 } else { -1.0 };
    let normal = |a: [f32; 2], b: [f32; 2]| {
        let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dz).max(1e-6);
        [side * dz / l, -side * dx / l]
    };
    (0..n)
        .map(|i| {
            let p = outline[i];
            let n1 = normal(outline[(i + n - 1) % n], p);
            let n2 = normal(p, outline[(i + 1) % n]);
            let dot = (n1[0] * n2[0] + n1[1] * n2[1]).max(-0.9);
            let k = (d / (1.0 + dot)).min(3.0 * d);
            [p[0] + (n1[0] + n2[0]) * k, p[1] + (n1[1] + n2[1]) * k]
        })
        .collect()
}

/// A model's faces this close to the ground lie on it (a terrace slab, a model's own lawn).
pub const GROUND_FACE_M: f32 = 0.05;

/// The flat triangles of a model lying on the ground, as `[x, z]` triangles: `place` takes a
/// model position to the world, `ground` is the ground's height (0 where there is none).
///
/// The rule for a model on the ground: a model with any is drawn [`AREA_LIFT_M`] up, as an area
/// is, and the draped areas are cut out under these triangles and grow no grass there, so a
/// terrace slab level with the lawn never fights with it.
pub fn ground_faces(
    model: &ImportedModel,
    place: &dyn Fn([f32; 3]) -> [f32; 3],
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> Vec<[P2; 3]> {
    let world: Vec<[f32; 3]> = model.positions.iter().map(|&p| place(p)).collect();
    model
        .triangles
        .iter()
        .filter_map(|t| {
            let [p, q, r] = t.map(|i| world[i as usize]);
            let (u, v) = (
                [q[0] - p[0], q[1] - p[1], q[2] - p[2]],
                [r[0] - p[0], r[1] - p[1], r[2] - p[2]],
            );
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let flat = len > 1e-8 && n[1].abs() > 0.9 * len;
            let low = [p, q, r]
                .iter()
                .all(|c| (c[1] - ground(c[0], c[2]).unwrap_or(0.0)).abs() <= GROUND_FACE_M);
            (flat && low).then_some([[p[0], p[2]], [q[0], q[2]], [r[0], r[2]]])
        })
        .collect()
}

/// Whether `p` lies inside `outline` (either winding).
pub fn inside_outline(p: P2, outline: &[P2]) -> bool {
    outline.len() >= 3 && crate::planting::point_in_polygon(p, outline)
}

/// The lowest ground under an outline (its corners and edge midpoints), 0 where there is none.
pub fn footing(outline: &[P2], ground: &dyn Fn(f32, f32) -> Option<f32>) -> f32 {
    let n = outline.len();
    (0..n)
        .flat_map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            [a, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]]
        })
        .map(|p| ground(p[0], p[1]).unwrap_or(0.0))
        .reduce(f32::min)
        .unwrap_or(0.0)
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
            house: None,
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
            house: None,
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

#[cfg(test)]
mod stacking_tests {
    use super::*;

    fn area(id: &str, outline: Vec<P2>) -> Shape {
        Shape::Area(Area {
            id: id.into(),
            name: id.into(),
            layer: "shapes".into(),
            outline,
            elevation: 0.0,
            drape: true,
            extensions: Default::default(),
        })
    }

    fn rect(x0: f32, z0: f32, x1: f32, z1: f32) -> Vec<P2> {
        vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]]
    }

    #[test]
    fn later_areas_lie_on_top_of_the_ones_they_overlap() {
        let shapes = vec![
            area("lawn", rect(0.0, 0.0, 30.0, 20.0)),
            area("terrace", rect(5.0, 5.0, 17.0, 8.0)),
            area("path", rect(10.0, 6.0, 12.0, 25.0)),
            area("far", rect(40.0, 0.0, 50.0, 10.0)),
            // A crossing with no corner inside the other still overlaps.
            area("cross", rect(-5.0, 9.0, 35.0, 10.0)),
        ];
        let lifts: std::collections::HashMap<_, _> = area_lifts(&shapes).into_iter().collect();
        let base = AREA_LIFT_M;
        assert_eq!(lifts["lawn"], base);
        assert_eq!(lifts["terrace"], base + STACK_STEP_M);
        assert_eq!(lifts["path"], base + 2.0 * STACK_STEP_M);
        assert_eq!(lifts["far"], base);
        assert_eq!(lifts["cross"], base + 2.0 * STACK_STEP_M, "lawn and path");
        let over = areas_over(&shapes, "lawn");
        assert_eq!(over.len(), 3, "terrace, path, cross");
        assert!(inside_outline([6.0, 6.0], over[0]));
        assert!(areas_over(&shapes, "far").is_empty());
    }

    #[test]
    fn an_extrusion_stands_on_the_lowest_ground_under_it() {
        let slope = |x: f32, _: f32| Some(0.1 * x);
        assert_eq!(footing(&rect(2.0, 0.0, 10.0, 5.0), &slope), 0.2);
        assert_eq!(footing(&rect(2.0, 0.0, 10.0, 5.0), &|_, _| None), 0.0);
    }
}

/// Like [`shape_mesh_on`], cut along the terrain's own triangles: every piece of a draped area lies
/// in one terrain triangle and its corners on that triangle's plane, so the surface runs exactly
/// `elevation` above the ground the terrain draws, over hill edges and crests too. Areas without
/// `drape` and extrusions are as [`shape_mesh`] makes them.
///
/// `holes` are outlines the area is cut out under (water on top of a lawn: the pond's bed is the
/// terrain's, not grass), to the terrain's cell size.
pub fn shape_mesh_on_terrain(
    shape: &Shape,
    terrain: &crate::Terrain,
    holes: &[&[P2]],
) -> Result<ImportedModel, ShapeError> {
    let Shape::Area(a) = shape else {
        return shape_mesh(shape);
    };
    if !a.drape {
        return shape_mesh(shape);
    }
    let flat = shape_mesh(shape)?;
    let cell = terrain.cell_m;
    let [ox, oz] = terrain.origin;
    // Grid coordinates: cell edges at whole u and v, the cells' (0,0)-(1,1) diagonals at whole u - v.
    let to_uv = |p: P2| [(p[0] - ox) / cell, (p[1] - oz) / cell];
    // The holes as triangles in grid coordinates, cut out of every piece exactly (along the
    // water's own edge, not along the terrain's cells).
    let hole_tris: Vec<[P2; 3]> = holes
        .iter()
        .flat_map(|h| {
            let flat: Vec<f64> = h
                .iter()
                .flat_map(|p| [f64::from(p[0]), f64::from(p[1])])
                .collect();
            earcutr::earcut(&flat, &[], 2)
                .unwrap_or_default()
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| t.map(|i| to_uv(h[i])))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut b = Builder {
        model: ImportedModel::default(),
        triangles: vec![Vec::new()],
    };
    for t in &flat.triangles {
        let tri: Vec<P2> = t
            .iter()
            .map(|&i| to_uv([flat.positions[i as usize][0], flat.positions[i as usize][2]]))
            .collect();
        let mut pieces = vec![tri];
        for dir in [[1.0, 0.0], [0.0, 1.0], [1.0, -1.0]] {
            pieces = pieces.into_iter().flat_map(|p| slice(p, dir)).collect();
        }
        for t in &hole_tris {
            pieces = pieces
                .into_iter()
                .flat_map(|p| subtract_triangle(p, t))
                .collect();
        }
        for piece in pieces {
            let corner = |q: P2| {
                let (x, z) = (ox + q[0] * cell, oz + q[1] * cell);
                [
                    x,
                    terrain.surface_height(x, z).unwrap_or(0.0) + a.elevation,
                    z,
                ]
            };
            let c: Vec<[f32; 3]> = piece.iter().map(|&q| corner(q)).collect();
            for k in 1..c.len().saturating_sub(1) {
                b.tri(0, [c[0], c[k], c[k + 1]], [0.0, 1.0, 0.0]);
            }
        }
    }
    // Smooth like the terrain, not faceted per piece.
    for (p, n) in b.model.positions.iter().zip(b.model.normals.iter_mut()) {
        *n = terrain.normal_at(p[0], p[2]);
    }
    Ok(b.finish(&a.id, &a.name, vec![format!("{}:top", a.id)]))
}

/// Cuts a convex polygon along the lines `dir . p = k` for every whole `k` across it.
fn slice(poly: Vec<P2>, dir: P2) -> Vec<Vec<P2>> {
    let d = |p: &P2| dir[0] * p[0] + dir[1] * p[1];
    let (lo, hi) = poly
        .iter()
        .map(d)
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
    let mut out = Vec::new();
    let mut rest = poly;
    let mut k = lo.floor() + 1.0;
    while k < hi - 1e-5 {
        if k > lo + 1e-5 {
            let (below, above) = cut(&rest, dir, k);
            if below.len() >= 3 {
                out.push(below);
            }
            rest = above;
        }
        k += 1.0;
    }
    if rest.len() >= 3 {
        out.push(rest);
    }
    out
}

/// The convex pieces of convex `poly` that lie outside triangle `t`.
fn subtract_triangle(poly: Vec<P2>, t: &[P2; 3]) -> Vec<Vec<P2>> {
    let (lo, hi) = poly.iter().fold(
        ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
        |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        },
    );
    if t.iter().all(|q| q[0] <= lo[0])
        || t.iter().all(|q| q[0] >= hi[0])
        || t.iter().all(|q| q[1] <= lo[1])
        || t.iter().all(|q| q[1] >= hi[1])
    {
        return vec![poly];
    }
    let [a, b, c] = *t;
    let ccw = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0;
    let mut out = Vec::new();
    let mut rest = poly;
    for i in 0..3 {
        let (p, q) = if ccw {
            (t[i], t[(i + 1) % 3])
        } else {
            (t[(i + 1) % 3], t[i])
        };
        // Outward normal of a counter-clockwise edge; `above` is the side outside the triangle.
        let n = [q[1] - p[1], p[0] - q[0]];
        let (inside, outside) = cut(&rest, n, n[0] * p[0] + n[1] * p[1]);
        if outside.len() >= 3 {
            out.push(outside);
        }
        rest = inside;
        if rest.len() < 3 {
            break;
        }
    }
    out
}

/// The parts of a convex polygon below and above the line `dir . p = k`.
fn cut(poly: &[P2], dir: P2, k: f32) -> (Vec<P2>, Vec<P2>) {
    let d = |p: &P2| dir[0] * p[0] + dir[1] * p[1] - k;
    let (mut below, mut above) = (Vec::new(), Vec::new());
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let (dp, dq) = (d(&p), d(&q));
        if dp <= 0.0 {
            below.push(p);
        }
        if dp >= 0.0 {
            above.push(p);
        }
        if (dp < 0.0 && dq > 0.0) || (dp > 0.0 && dq < 0.0) {
            let t = dp / (dp - dq);
            let x = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
            below.push(x);
            above.push(x);
        }
    }
    (below, above)
}

#[cfg(test)]
mod hill_tests {
    use super::*;

    /// A ridge 3 m high with steep flanks: a lawn over it must stay `elevation` above the
    /// terrain's own surface everywhere, crest and foot included.
    #[test]
    fn a_draped_area_does_not_cut_through_a_hill_edge() {
        let mut t = crate::Terrain::flat([0.0, 0.0], 20.0, 12.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        for (i, h) in t.heights.iter_mut().enumerate() {
            let x = (i % w) as f32 * 0.5;
            *h = (3.0 - 1.7 * (x - 10.2).abs()).max(0.0);
        }
        let a = Area {
            id: "S1".into(),
            name: "Lawn".into(),
            layer: "shapes".into(),
            outline: vec![[0.3, 0.2], [19.7, 0.9], [19.1, 11.6], [0.6, 11.3]],
            elevation: AREA_LIFT_M,
            drape: true,
            extensions: Default::default(),
        };
        let m = shape_mesh_on_terrain(&Shape::Area(a.clone()), &t, &[]).unwrap();
        assert!(m.triangles.len() > 500);
        let mut worst = f32::INFINITY;
        for tri in &m.triangles {
            let [p, q, r] = tri.map(|i| m.positions[i as usize]);
            for w in [
                [0.5, 0.5, 0.0],
                [0.0, 0.5, 0.5],
                [0.5, 0.0, 0.5],
                [1.0 / 3.0; 3],
            ] {
                let at = |k: usize| w[0] * p[k] + w[1] * q[k] + w[2] * r[k];
                let ground = t.surface_height(at(0), at(2)).unwrap();
                worst = worst.min(at(1) - ground);
            }
        }
        assert!(
            (worst - AREA_LIFT_M).abs() < 1e-3,
            "the lawn runs {worst} m over the terrain's surface somewhere"
        );
        // A pond on it: no lawn under the water.
        let pond: Vec<P2> = vec![[3.0, 3.0], [7.0, 3.0], [7.0, 7.0], [3.0, 7.0]];
        let cut = shape_mesh_on_terrain(&Shape::Area(a.clone()), &t, &[&pond]).unwrap();
        assert!(cut.triangles.len() < m.triangles.len());
        assert!(cut.triangles.iter().all(|tri| {
            let [p, q, r] = tri.map(|i| cut.positions[i as usize]);
            let c = [(p[0] + q[0] + r[0]) / 3.0, (p[2] + q[2] + r[2]) / 3.0];
            !(3.2..6.8).contains(&c[0]) || !(3.2..6.8).contains(&c[1])
        }));
        // The cut runs along the water's own edge, not the terrain's cells: a skewed pond off the
        // grid takes exactly its own area out of the lawn (no sawtooth either way).
        let plan_area = |m: &ImportedModel| -> f32 {
            m.triangles
                .iter()
                .map(|tri| {
                    let [p, q, r] = tri.map(|i| m.positions[i as usize]);
                    ((q[0] - p[0]) * (r[2] - p[2]) - (q[2] - p[2]) * (r[0] - p[0])).abs() / 2.0
                })
                .sum()
        };
        let skew: Vec<P2> = vec![[11.3, 3.1], [16.2, 3.4], [16.6, 7.7], [10.9, 7.2]];
        let skew_area = 0.5
            * (0..4)
                .map(|i| {
                    let (p, q) = (skew[i], skew[(i + 1) % 4]);
                    p[0] * q[1] - q[0] * p[1]
                })
                .sum::<f32>()
                .abs();
        let cut = shape_mesh_on_terrain(&Shape::Area(a), &t, &[&skew]).unwrap();
        let taken = plan_area(&m) - plan_area(&cut);
        assert!(
            (taken - skew_area).abs() < 0.01,
            "{taken} m2 cut for a {skew_area} m2 pond"
        );
    }
}

#[cfg(test)]
mod offset_tests {
    use super::offset_outline;

    #[test]
    fn the_coping_band_keeps_a_square_square() {
        // A 4 by 2 square, either winding, grows by d on every side with sharp corners.
        for sq in [
            vec![[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]],
            vec![[0.0, 0.0], [0.0, 2.0], [4.0, 2.0], [4.0, 0.0]],
        ] {
            let o = offset_outline(&sq, 0.15);
            assert_eq!(o.len(), 4);
            for p in &o {
                let x_ok = (p[0] + 0.15).abs() < 1e-4 || (p[0] - 4.15).abs() < 1e-4;
                let z_ok = (p[1] + 0.15).abs() < 1e-4 || (p[1] - 2.15).abs() < 1e-4;
                assert!(x_ok && z_ok, "{o:?}");
            }
        }
    }
}
