//! Curved outlines: arcs on single edges and smooth (centripetal Catmull-Rom) outlines.
//!
//! A shape stores its **control vertices** (`points`) and, next to them, a [`Curve`] per edge
//! that is an arc and a `smooth` flag. [`tessellate`] turns both into the polygon that measuring
//! and meshing use (`Shape::outline`); editing keeps to the control vertices.
//!
//! # Conventions
//!
//! - Edge `i` runs from vertex `i` to vertex `i + 1` (wrapping round when the outline is closed).
//! - The sagitta of an arc is the signed distance from the middle of the chord to the arc.
//!   Positive bulges to the **left** of the edge's direction, seen from above with +X east and
//!   -Z north (so walking east, left is north, -Z). For an outline that winds with a positive
//!   shoelace sum in (x, z) (clockwise as seen from above), left is outward.
//! - An arc reaches at most a semicircle: `|sagitta| <= chord / 2`.
//! - `smooth` draws a centripetal Catmull-Rom curve through every vertex, closed or open as the
//!   outline is. An edge with an arc keeps its arc; `smooth` applies to the other edges.
//! - Without arcs and without `smooth` the tessellated outline is exactly the control points.
//!
//! The chord tolerance is [`CHORD_TOL_M`]; every arc has at least [`MIN_ARC_SEGMENTS`] and no
//! edge more than [`MAX_EDGE_SEGMENTS`] segments.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

type P2 = [f32; 2];

/// How far a tessellated segment may lie from the true curve (metres). 3 mm keeps a circle's
/// area within 0.1% at the sizes of a garden bed.
pub const CHORD_TOL_M: f32 = 0.003;
/// The fewest segments an arc is cut into.
pub const MIN_ARC_SEGMENTS: usize = 4;
/// The most segments one edge is cut into.
pub const MAX_EDGE_SEGMENTS: usize = 64;

/// The curve of one edge.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Curve {
    /// A circular arc; `sagitta_m` is the signed distance from the chord's middle to the arc,
    /// positive to the left of the edge's direction (see the module docs).
    Arc { sagitta_m: f32 },
}

/// The curved edges of a shape by edge index.
pub type Curves = BTreeMap<usize, Curve>;

/// Serde for [`Curves`] inside a shape: JSON keys are strings, and a shape sits in an
/// internally tagged enum, whose buffered map cannot turn a string key into a number by itself.
pub mod keyed {
    use super::{Curve, Curves};
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(c: &Curves, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(c.iter().map(|(k, v)| (k.to_string(), v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Curves, D::Error> {
        BTreeMap::<String, Curve>::deserialize(d)?
            .into_iter()
            .map(|(k, v)| {
                k.parse::<usize>()
                    .map(|k| (k, v))
                    .map_err(|_| D::Error::custom(format!("`{k}` is not an edge index")))
            })
            .collect()
    }
}

impl Curve {
    /// The signed sagitta.
    pub fn sagitta_m(self) -> f32 {
        match self {
            Self::Arc { sagitta_m } => sagitta_m,
        }
    }

    /// The arc of `radius` over a chord of `chord` metres, bulging to `left` or to the right: the
    /// shorter of the two arcs of that radius. Refuses a radius under half the chord.
    pub fn from_radius(chord: f32, radius: f32, left: bool) -> Result<Self, String> {
        if !(radius.is_finite() && radius > 0.0) {
            return Err("`radius_m` must be a positive number".into());
        }
        if radius < chord / 2.0 * (1.0 - 1e-5) {
            return Err(format!(
                "a radius of {radius} m is too small for an edge {chord:.3} m long: it must be at least {:.3} m (half the chord)",
                chord / 2.0
            ));
        }
        let half = chord / 2.0;
        let s = radius - (radius * radius - half * half).max(0.0).sqrt();
        Ok(Self::Arc {
            sagitta_m: if left { s } else { -s },
        })
    }

    /// The radius of the arc over a chord of `chord` metres.
    pub fn radius_m(self, chord: f32) -> f32 {
        let s = self.sagitta_m().abs();
        if s < 1e-9 {
            f32::INFINITY
        } else {
            (chord * chord / 4.0 + s * s) / (2.0 * s)
        }
    }
}

/// The number of edges of `n` points.
pub fn edge_count(n: usize, closed: bool) -> usize {
    if closed { n } else { n.saturating_sub(1) }
}

fn dist(a: P2, b: P2) -> f32 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// Whether `curves` and `smooth` suit an outline of `points`; the reason if not.
pub fn check(points: &[P2], closed: bool, curves: &Curves) -> Result<(), String> {
    let edges = edge_count(points.len(), closed);
    for (&i, c) in curves {
        if i >= edges {
            return Err(format!(
                "curve on edge {i}, but the outline has {edges} edges (0..{})",
                edges.saturating_sub(1)
            ));
        }
        let s = c.sagitta_m();
        if !s.is_finite() {
            return Err(format!("the sagitta of edge {i} is not a number"));
        }
        let chord = dist(points[i], points[(i + 1) % points.len()]);
        if s.abs() > chord / 2.0 * (1.0 + 1e-4) {
            return Err(format!(
                "the arc of edge {i} bulges {:.3} m, more than half its chord ({:.3} m): an arc reaches at most a semicircle",
                s.abs(),
                chord / 2.0
            ));
        }
    }
    Ok(())
}

/// The tessellated outline with what ties it back to the control vertices.
#[derive(Clone, Debug, PartialEq)]
pub struct Tess {
    /// The polygon (or polyline when open).
    pub pts: Vec<P2>,
    /// For each segment of `pts` (point `j` to `j + 1`, wrapping when closed) the control edge it
    /// belongs to.
    pub edge_of: Vec<usize>,
    /// The index in `pts` of each control vertex.
    pub ctrl: Vec<usize>,
}

impl Tess {
    /// The polyline of each control edge, both ends included.
    pub fn runs(&self) -> Vec<Vec<P2>> {
        let mut out: Vec<Vec<P2>> = Vec::new();
        let n = self.pts.len();
        for (j, &e) in self.edge_of.iter().enumerate() {
            if out.len() <= e {
                out.resize(e + 1, Vec::new());
            }
            let run = &mut out[e];
            if run.is_empty() {
                run.push(self.pts[j]);
            }
            run.push(self.pts[(j + 1) % n]);
        }
        out
    }
}

/// The polygon of an outline of control `points` with `curves` and `smooth`.
pub fn tessellate(points: &[P2], closed: bool, curves: &Curves, smooth: bool) -> Tess {
    let n = points.len();
    let edges = edge_count(n, closed);
    let mut t = Tess {
        pts: Vec::with_capacity(n),
        edge_of: Vec::new(),
        ctrl: Vec::with_capacity(n),
    };
    for i in 0..edges {
        t.ctrl.push(t.pts.len());
        t.pts.push(points[i]);
        let (a, b) = (points[i], points[(i + 1) % n]);
        let inner = match curves.get(&i) {
            Some(Curve::Arc { sagitta_m }) => arc_interior(a, b, *sagitta_m),
            None if smooth && n >= 3 => {
                let p0 = if closed || i > 0 {
                    points[(i + n - 1) % n]
                } else {
                    [2.0 * a[0] - b[0], 2.0 * a[1] - b[1]]
                };
                let p3 = if closed || i + 2 < n {
                    points[(i + 2) % n]
                } else {
                    [2.0 * b[0] - a[0], 2.0 * b[1] - a[1]]
                };
                smooth_interior([p0, a, b, p3])
            }
            None => Vec::new(),
        };
        t.edge_of.extend(std::iter::repeat_n(i, inner.len() + 1));
        t.pts.extend(inner);
    }
    if !closed && n > 0 {
        t.ctrl.push(t.pts.len());
        t.pts.push(points[n - 1]);
    }
    t
}

/// The points strictly between `a` and `b` of the arc with signed sagitta `s`.
fn arc_interior(a: P2, b: P2, s: f32) -> Vec<P2> {
    let chord = dist(a, b);
    if chord < 1e-9 || s.abs() < 1e-7 {
        return Vec::new();
    }
    let left = [(b[1] - a[1]) / chord, -(b[0] - a[0]) / chord];
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let sa = s.abs();
    let r = (chord * chord / 4.0 + sa * sa) / (2.0 * sa);
    let sign = s.signum();
    let apex = [mid[0] + left[0] * s, mid[1] + left[1] * s];
    let centre = [apex[0] - left[0] * sign * r, apex[1] - left[1] * sign * r];
    // Half the angle the arc spans, up to a quarter turn (a semicircle).
    let half = (chord / 2.0).atan2((r - sa).max(0.0));
    let sweep = 2.0 * half;
    let step = if CHORD_TOL_M < r {
        2.0 * (1.0 - CHORD_TOL_M / r).acos()
    } else {
        std::f32::consts::PI
    };
    let n = ((sweep / step).ceil() as usize).clamp(MIN_ARC_SEGMENTS, MAX_EDGE_SEGMENTS);
    let u = [a[0] - centre[0], a[1] - centre[1]];
    let v = [apex[0] - centre[0], apex[1] - centre[1]];
    let turn = if u[0] * v[1] - u[1] * v[0] >= 0.0 {
        1.0
    } else {
        -1.0
    };
    (1..n)
        .map(|k| {
            let th = turn * sweep * k as f32 / n as f32;
            let (sn, cs) = th.sin_cos();
            [
                centre[0] + u[0] * cs - u[1] * sn,
                centre[1] + u[0] * sn + u[1] * cs,
            ]
        })
        .collect()
}

fn lerp(a: P2, b: P2, w: f32) -> P2 {
    [a[0] + (b[0] - a[0]) * w, a[1] + (b[1] - a[1]) * w]
}

/// The centripetal Catmull-Rom curve through `p[1]` and `p[2]` at `t` in 0..=1 (Barry-Goldman).
fn cr_point(p: [P2; 4], t: f32) -> P2 {
    let mut k = [0.0f32; 4];
    for i in 1..4 {
        k[i] = k[i - 1] + dist(p[i - 1], p[i]).max(1e-6).sqrt();
    }
    let u = k[1] + (k[2] - k[1]) * t;
    let w = |a: usize, b: usize| (u - k[a]) / (k[b] - k[a]);
    let a1 = lerp(p[0], p[1], w(0, 1));
    let a2 = lerp(p[1], p[2], w(1, 2));
    let a3 = lerp(p[2], p[3], w(2, 3));
    let b1 = lerp(a1, a2, w(0, 2));
    let b2 = lerp(a2, a3, w(1, 3));
    lerp(b1, b2, w(1, 2))
}

fn seg_distance(p: P2, a: P2, b: P2) -> f32 {
    let (d, e) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
    let l2 = d[0] * d[0] + d[1] * d[1];
    let w = if l2 < 1e-12 {
        0.0
    } else {
        ((e[0] * d[0] + e[1] * d[1]) / l2).clamp(0.0, 1.0)
    };
    dist(p, [a[0] + d[0] * w, a[1] + d[1] * w])
}

/// The points strictly between `p[1]` and `p[2]` of the smooth curve, none when it is straight.
fn smooth_interior(p: [P2; 4]) -> Vec<P2> {
    let f = |t: f32| cr_point(p, t);
    let (a, b) = (p[1], p[2]);
    if (1..8).all(|k| seg_distance(f(k as f32 / 8.0), a, b) < 1e-4) {
        return Vec::new();
    }
    // Quarters first, then halve a stretch while its curve lies too far off its chord.
    fn refine(f: &dyn Fn(f32) -> P2, t0: f32, t1: f32, depth: u32, out: &mut Vec<f32>) {
        let (p0, p1) = (f(t0), f(t1));
        let off = |t: f32| seg_distance(f(t), p0, p1);
        let dev = off((t0 + t1) / 2.0)
            .max(off(t0 + (t1 - t0) * 0.25))
            .max(off(t0 + (t1 - t0) * 0.75));
        if depth == 0 || dev <= CHORD_TOL_M {
            out.push(t1);
        } else {
            let tm = (t0 + t1) / 2.0;
            refine(f, t0, tm, depth - 1, out);
            refine(f, tm, t1, depth - 1, out);
        }
    }
    let mut ts = Vec::new();
    for k in 0..4 {
        refine(&f, k as f32 / 4.0, (k + 1) as f32 / 4.0, 4, &mut ts);
    }
    ts.pop();
    ts.into_iter().map(f).collect()
}

/// Makes `curves` fit `points` after a vertex moved or the points were replaced: arcs on edges
/// that no longer exist go, and an arc that bulges more than half its (shorter) chord is eased
/// to a semicircle.
pub fn fit(points: &[P2], closed: bool, curves: &mut Curves) {
    let edges = edge_count(points.len(), closed);
    curves.retain(|&i, _| i < edges);
    for (&i, c) in curves.iter_mut() {
        let half = dist(points[i], points[(i + 1) % points.len()]) / 2.0;
        let Curve::Arc { sagitta_m } = c;
        if sagitta_m.abs() > half {
            *sagitta_m = half * sagitta_m.signum();
        }
    }
}

/// `curves` after a vertex is inserted into edge `edge` (the new vertex gets index `edge + 1`):
/// the split edge's arc is dropped, since its two halves are no longer the same arc, and the
/// arcs of later edges move up one.
pub fn after_insert(curves: &Curves, edge: usize) -> Curves {
    curves
        .iter()
        .filter(|&(&i, _)| i != edge)
        .map(|(&i, &c)| (if i > edge { i + 1 } else { i }, c))
        .collect()
}

/// `curves` after vertex `index` of `points` points is removed: the arcs of the two edges it
/// joined are dropped, the arcs of later edges move down one. Removing the end of an open line
/// drops its one edge.
pub fn after_remove(curves: &Curves, index: usize, points: usize, closed: bool) -> Curves {
    let edges = edge_count(points, closed);
    let (before, after) = if !closed && index == 0 {
        (None, 0)
    } else if !closed && index == points - 1 {
        (Some(index - 1), index - 1)
    } else {
        (Some((index + edges - 1) % edges), index)
    };
    curves
        .iter()
        .filter(|&(&i, _)| Some(i) != before && i != after)
        .map(|(&i, &c)| {
            // Edges after the removed vertex's own edge move down one; with the wrap-around of a
            // closed outline, removing vertex 0 leaves its edge 0 gone and the last edge too.
            let j = if i > index.max(after) { i - 1 } else { i };
            (j, c)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_sagitta_bulges_to_the_left() {
        // East along +X: left is -Z (north, as seen from above).
        let pts = arc_interior([0.0, 0.0], [4.0, 0.0], 1.0);
        assert!(pts.iter().all(|p| p[1] < 0.0), "{pts:?}");
        let pts = arc_interior([0.0, 0.0], [4.0, 0.0], -1.0);
        assert!(pts.iter().all(|p| p[1] > 0.0));
        // South along +Z: left is +X (east).
        let pts = arc_interior([0.0, 0.0], [0.0, 4.0], 1.0);
        assert!(pts.iter().all(|p| p[0] > 0.0));
    }

    #[test]
    fn arc_apex_is_the_sagitta_away_and_points_lie_on_the_circle() {
        let (a, b, s) = ([1.0, 2.0], [5.0, 5.0], 0.8);
        let pts = arc_interior(a, b, s);
        let chord = dist(a, b);
        let r = (chord * chord / 4.0 + s * s) / (2.0 * s);
        let left = [(b[1] - a[1]) / chord, -(b[0] - a[0]) / chord];
        let mid = [3.0, 3.5];
        let centre = [mid[0] + left[0] * (s - r), mid[1] + left[1] * (s - r)];
        for p in &pts {
            assert!((dist(*p, centre) - r).abs() < 1e-3);
        }
        // The farthest point from the chord is the sagitta away.
        let far = pts
            .iter()
            .map(|p| seg_distance(*p, a, b))
            .fold(0.0f32, f32::max);
        assert!((far - s).abs() < CHORD_TOL_M * 2.0, "{far}");
    }

    #[test]
    fn tessellation_stays_within_tolerance_and_limits() {
        let pts = arc_interior([0.0, 0.0], [6.0, 0.0], 3.0);
        assert!(pts.len() < MAX_EDGE_SEGMENTS);
        let r = 3.0;
        let mut all = vec![[0.0, 0.0]];
        all.extend(pts);
        all.push([6.0, 0.0]);
        for w in all.windows(2) {
            let mid = lerp(w[0], w[1], 0.5);
            let off = r - dist(mid, [3.0, 0.0]);
            assert!(off.abs() <= CHORD_TOL_M * 1.05, "{off}");
        }
        // A tiny arc still gets its minimum.
        assert_eq!(arc_interior([0.0, 0.0], [1.0, 0.0], 0.01).len() + 1, 4);
    }

    #[test]
    fn radius_converts_to_sagitta_and_refuses_a_small_one() {
        let c = Curve::from_radius(6.0, 3.0, true).unwrap();
        assert!((c.sagitta_m() - 3.0).abs() < 1e-5);
        let c = Curve::from_radius(6.0, 5.0, false).unwrap();
        assert!((c.sagitta_m() + 1.0).abs() < 1e-5);
        assert!((c.radius_m(6.0) - 5.0).abs() < 1e-4);
        assert!(Curve::from_radius(6.0, 2.9, true).is_err());
    }

    #[test]
    fn smooth_passes_through_the_control_points() {
        let pts = [[0.0, 0.0], [4.0, 1.0], [6.0, 5.0], [1.0, 6.0]];
        for closed in [true, false] {
            let t = tessellate(&pts, closed, &Curves::new(), true);
            for (i, p) in pts.iter().enumerate() {
                assert_eq!(t.pts[t.ctrl[i]], *p);
            }
            assert!(t.pts.len() > pts.len());
            assert_eq!(t.edge_of.len(), t.pts.len() - usize::from(!closed));
        }
    }

    #[test]
    fn identity_without_curves() {
        let pts = [[0.0, 0.0], [4.0, 1.0], [6.0, 5.0]];
        let t = tessellate(&pts, true, &Curves::new(), false);
        assert_eq!(t.pts, pts);
        assert_eq!(t.edge_of, [0, 1, 2]);
    }

    #[test]
    fn rekeying_on_insert_and_remove() {
        let a = Curve::Arc { sagitta_m: 0.5 };
        let curves: Curves = [(0, a), (1, a), (3, a)].into();
        // Insert into edge 1: its arc goes, edge 3 becomes 4.
        let c = after_insert(&curves, 1);
        assert_eq!(c.keys().copied().collect::<Vec<_>>(), [0, 4]);
        // Remove vertex 2 of 4 closed: edges 1 and 2 join and lose their arcs; 3 becomes 2.
        let c = after_remove(&curves, 2, 4, true);
        assert_eq!(c.keys().copied().collect::<Vec<_>>(), [0, 2]);
        // Remove vertex 0 of 4 closed: edges 3 and 0 go; 1 becomes 0.
        let c = after_remove(&curves, 0, 4, true);
        assert_eq!(c.keys().copied().collect::<Vec<_>>(), [0]);
        // Open line of 4 points: remove the last vertex drops edge 2.
        let c = after_remove(&[(2, a), (0, a)].into(), 3, 4, false);
        assert_eq!(c.keys().copied().collect::<Vec<_>>(), [0]);
        // Remove the first vertex of an open line: edge 0 goes, the rest move down.
        let c = after_remove(&[(0, a), (1, a)].into(), 0, 4, false);
        assert_eq!(c.keys().copied().collect::<Vec<_>>(), [0]);
    }
}
