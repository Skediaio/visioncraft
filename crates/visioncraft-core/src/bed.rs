//! Raised beds: what turns a closed extrusion into a hollow ring of walls with a rim, drawn by
//! `shape_mesh` with stable face names so materials attach: `<id>:wall` (outside, inside and
//! underside) and `<id>:rim` (the top of the walls, and a coping or folded edge).
//!
//! The ring is open in the middle. The soil is a separate area inside it (`soil` names it), kept
//! in step by `bed.set`; plants stand on that area like on any other ground.
//!
//! The wall has a [`BedStyle`]: a plain ring (the default), masonry (a ring with a coping slab),
//! stacked timber sleepers (every course a row of separate beams, the corners overlapping like
//! real railway-sleeper beds) or a thin weathering-steel plate with a folded top edge.

use crate::house::Sink;
use serde::{Deserialize, Serialize};

type P2 = [f32; 2];

/// Faces a bed has, in this order.
pub const BED_FACES: [&str; 2] = ["wall", "rim"];
const WALL: usize = 0;
const RIM: usize = 1;

/// The wall thickness a plain bed starts with (metres).
pub const DEFAULT_WALL_M: f32 = 0.2;
/// How far the soil lies below the rim (metres).
pub const SOIL_BELOW_RIM_M: f32 = 0.05;
/// A sleeper's course height (the bed's height is split in whole courses of about this).
pub const BEAM_H: f32 = 0.2;
/// The longest sleeper; a longer wall is laid in several, with a joint.
pub const BEAM_MAX_LEN: f32 = 2.4;
/// The groove between sleepers (metres).
const JOINT: f32 = 0.004;
/// A steel plate is drawn at least this thick so it reads (metres).
pub const MIN_PLATE_M: f32 = 0.01;
/// The folded edge of a steel bed: height of the flange, how far it reaches out past the plate and
/// in over the soil, and how far the lip hangs down the outside.
const FLANGE_H: f32 = 0.012;
const FLANGE_OUT: f32 = 0.015;
const FLANGE_IN: f32 = 0.02;
const LIP_H: f32 = 0.05;
/// A masonry bed's coping: height and how far it overhangs.
const COPING_H: f32 = 0.04;
const COPING_OUT: f32 = 0.02;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BedStyle {
    /// A plain ring of wall, timber to begin with.
    #[default]
    Plain,
    /// A brick ring with a coping slab on top.
    Masonry,
    /// Stacked timber beams, 0.2 m high courses, with joints and overlapping corners.
    Sleepers,
    /// A thin weathering-steel plate with a folded top edge.
    Corten,
}

impl BedStyle {
    pub fn default_wall_m(self) -> f32 {
        match self {
            Self::Plain | Self::Masonry => DEFAULT_WALL_M,
            Self::Sleepers => 0.15,
            Self::Corten => 0.01,
        }
    }

    /// The wall thicknesses the style takes (metres).
    pub fn wall_range(self) -> (f32, f32) {
        match self {
            Self::Plain | Self::Masonry => (0.05, 0.3),
            Self::Sleepers => (0.1, 0.2),
            Self::Corten => (0.005, 0.02),
        }
    }

    /// The palette material the walls start with.
    pub fn default_material(self) -> &'static str {
        match self {
            Self::Plain | Self::Sleepers => "timber",
            Self::Masonry => "brick",
            Self::Corten => "corten",
        }
    }

    fn is_plain(&self) -> bool {
        *self == Self::Plain
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaisedBed {
    /// How thick the walls are, 0.005..=1.0 (a sleeper's thickness for sleepers, the plate's for
    /// corten); the inner ring is the outline moved in by this.
    pub wall_thickness_m: f32,
    #[serde(skip_serializing_if = "BedStyle::is_plain")]
    pub style: BedStyle,
    /// The id of the area that is the soil inside (`bed.add` makes it, `bed.set` keeps it in
    /// step, `shape.remove` takes it away with the bed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soil: Option<String>,
}

impl Default for RaisedBed {
    fn default() -> Self {
        Self {
            wall_thickness_m: DEFAULT_WALL_M,
            style: BedStyle::Plain,
            soil: None,
        }
    }
}

impl RaisedBed {
    /// The wall thickness as drawn: a steel plate is at least [`MIN_PLATE_M`] so it reads.
    pub fn wall(&self) -> f32 {
        match self.style {
            BedStyle::Corten => self.wall_thickness_m.max(MIN_PLATE_M),
            _ => self.wall_thickness_m,
        }
    }

    /// The ring inside `outline`, the walls' inner side; the reason if there is none.
    pub fn inner(&self, outline: &[P2]) -> Result<Vec<P2>, String> {
        crate::shape::inner_ring(outline, self.wall())
    }

    /// Whether the style takes the wall thickness; the reason if not.
    pub fn check(&self) -> Result<(), String> {
        let (lo, hi) = self.style.wall_range();
        let t = self.wall_thickness_m;
        if t.is_finite() && (lo..=hi).contains(&t) {
            Ok(())
        } else {
            Err(format!(
                "`wall_thickness_m` must be within {lo}..={hi} m for this style"
            ))
        }
    }

    /// Height of the soil's surface above the bed's bottom for a bed `height` tall.
    pub fn soil_top(height: f32) -> f32 {
        height - SOIL_BELOW_RIM_M
    }
}

/// One sleeper: a box `u` along the edge from `from`, `t` across (inward from the outer face) and
/// `y` high.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    pub from: P2,
    pub dir: P2,
    pub inward: P2,
    pub u: (f32, f32),
    pub y: (f32, f32),
    pub top: bool,
}

/// The sleepers of a bed with `outline` (its outer face) from `lo` to `hi`: whole courses of about
/// [`BEAM_H`], each edge laid as long beams of at most [`BEAM_MAX_LEN`]. At a corner one beam runs
/// right out to the corner and the other stops against it, swapping every course. Exact at right
/// angles; at other corners a small notch is left.
pub fn beams(bed: &RaisedBed, outline: &[P2], lo: f32, hi: f32) -> Vec<Beam> {
    let n = outline.len();
    let t = bed.wall();
    let sign = if crate::shape::shoelace_f32(outline) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let edge = |i: usize| {
        let (p, q) = (outline[i % n], outline[(i + 1) % n]);
        let len = (q[0] - p[0]).hypot(q[1] - p[1]).max(1e-6);
        (p, [(q[0] - p[0]) / len, (q[1] - p[1]) / len], len)
    };
    let courses = (((hi - lo) / BEAM_H).round() as usize).max(1);
    let ch = (hi - lo) / courses as f32;
    let mut out = Vec::new();
    for c in 0..courses {
        let y = (
            lo + c as f32 * ch + if c > 0 { JOINT / 2.0 } else { 0.0 },
            lo + (c + 1) as f32 * ch - if c + 1 < courses { JOINT / 2.0 } else { 0.0 },
        );
        let runs_out = |i: usize| (i + c).is_multiple_of(2);
        for i in 0..n {
            let (p, d, len) = edge(i);
            let (_, dn, _) = edge(i + 1);
            let (_, dp, _) = edge(i + n - 1);
            // How far a beam stops short of the corner it stops against.
            let trim = |other: P2| t / (d[0] * other[1] - d[1] * other[0]).abs().max(0.2);
            let u0 = if runs_out(i + n - 1) { trim(dp) } else { 0.0 };
            let u1 = len - if runs_out(i) { 0.0 } else { trim(dn) };
            if u1 - u0 <= JOINT {
                continue;
            }
            let pieces = (((u1 - u0) / BEAM_MAX_LEN).ceil() as usize).max(1);
            for j in 0..pieces {
                let a = u0 + (u1 - u0) * j as f32 / pieces as f32;
                let b = u0 + (u1 - u0) * (j + 1) as f32 / pieces as f32;
                out.push(Beam {
                    from: p,
                    dir: d,
                    inward: [-sign * d[1], sign * d[0]],
                    u: (
                        a + if j > 0 { JOINT / 2.0 } else { 0.0 },
                        b - if j + 1 < pieces { JOINT / 2.0 } else { 0.0 },
                    ),
                    y,
                    top: c + 1 == courses,
                });
            }
        }
    }
    out
}

fn quad(s: &mut dyn Sink, face: usize, v: [[f32; 3]; 4], outward: [f32; 3]) {
    s.tri(face, [v[0], v[1], v[2]], outward);
    s.tri(face, [v[0], v[2], v[3]], outward);
}

/// One sleeper as a box.
fn beam(s: &mut dyn Sink, b: &Beam, t: f32) {
    let p = |u: f32, w: f32, y: f32| {
        [
            b.from[0] + b.dir[0] * u + b.inward[0] * w,
            y,
            b.from[1] + b.dir[1] * u + b.inward[1] * w,
        ]
    };
    let (d, n) = ([b.dir[0], 0.0, b.dir[1]], [b.inward[0], 0.0, b.inward[1]]);
    let neg = |v: [f32; 3]| v.map(|c| -c);
    let (u0, u1) = b.u;
    let (y0, y1) = b.y;
    let top = if b.top { RIM } else { WALL };
    quad(
        s,
        WALL,
        [p(u0, 0.0, y0), p(u1, 0.0, y0), p(u1, t, y0), p(u0, t, y0)],
        [0.0, -1.0, 0.0],
    );
    quad(
        s,
        top,
        [p(u0, 0.0, y1), p(u1, 0.0, y1), p(u1, t, y1), p(u0, t, y1)],
        [0.0, 1.0, 0.0],
    );
    quad(
        s,
        WALL,
        [p(u0, 0.0, y0), p(u0, t, y0), p(u0, t, y1), p(u0, 0.0, y1)],
        neg(d),
    );
    quad(
        s,
        WALL,
        [p(u1, 0.0, y0), p(u1, t, y0), p(u1, t, y1), p(u1, 0.0, y1)],
        d,
    );
    quad(
        s,
        WALL,
        [
            p(u0, 0.0, y0),
            p(u1, 0.0, y0),
            p(u1, 0.0, y1),
            p(u0, 0.0, y1),
        ],
        neg(n),
    );
    quad(
        s,
        WALL,
        [p(u0, t, y0), p(u1, t, y0), p(u1, t, y1), p(u0, t, y1)],
        n,
    );
}

/// The faces of a ring prism: its walls, and its top and underside if drawn.
struct Faces {
    wall: usize,
    top: Option<usize>,
    bottom: Option<usize>,
}

/// A ring-shaped prism between `outer` and `inner` from `y0` to `y1`.
fn ring(s: &mut dyn Sink, outer: &[P2], inner: &[P2], (y0, y1): (f32, f32), f: Faces) {
    // For a positive shoelace the wall of an edge `d` faces along `(dz, -dx)`.
    let sign = if crate::shape::shoelace_f32(outer) > 0.0 {
        1.0
    } else {
        -1.0
    };
    for (ring, flip) in [(outer, 1.0), (inner, -1.0)] {
        let n = ring.len();
        for i in 0..n {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            let out = [
                flip * sign * (q[1] - p[1]),
                0.0,
                -flip * sign * (q[0] - p[0]),
            ];
            let c = |pt: P2, y: f32| [pt[0], y, pt[1]];
            quad(s, f.wall, [c(p, y0), c(q, y0), c(q, y1), c(p, y1)], out);
        }
    }
    // The top and the underside: the ring between the two outlines, the inner one as a hole.
    let flat: Vec<f64> = outer
        .iter()
        .chain(inner)
        .flat_map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();
    let indices = earcutr::earcut(&flat, &[outer.len()], 2).unwrap_or_default();
    let all: Vec<P2> = outer.iter().chain(inner).copied().collect();
    for t in indices.as_chunks::<3>().0 {
        let at = |y: f32| t.map(|i| [all[i][0], y, all[i][1]]);
        if let Some(top) = f.top {
            s.tri(top, at(y1), [0.0, 1.0, 0.0]);
        }
        if let Some(bottom) = f.bottom {
            s.tri(bottom, at(y0), [0.0, -1.0, 0.0]);
        }
    }
}

/// Faces a terrace's retaining edge has, in this order (`<id>:edge`, `<id>:edge_rim`).
pub const EDGE_FACES: [&str; 2] = ["edge", "edge_rim"];

/// How far a skirt's top lies below its area's top (metres).
pub const SKIRT_SINK_M: f32 = 0.002;

/// The least a raised area stands above the ground to get a retaining edge (metres).
pub const MIN_EDGE_H: f32 = 0.05;

/// The retaining edge of a raised area (`Area::edge`): a skirt in a [`BedStyle`] along its whole
/// outline, from the area's top (less [`SKIRT_SINK_M`]) down to the ground. It sits just outside
/// the outline, so the walkable top keeps its size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AreaEdge {
    pub style: BedStyle,
    /// How thick the skirt is (a sleeper's thickness for sleepers, the plate's for corten); the
    /// style's own when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness_m: Option<f32>,
}

impl AreaEdge {
    fn bed(&self) -> RaisedBed {
        RaisedBed {
            wall_thickness_m: self
                .thickness_m
                .unwrap_or_else(|| self.style.default_wall_m()),
            style: self.style,
            soil: None,
        }
    }

    /// The thickness as drawn (a steel plate is at least [`MIN_PLATE_M`]).
    pub fn wall(&self) -> f32 {
        self.bed().wall()
    }

    /// Whether the style takes the thickness; the reason if not.
    pub fn check(&self) -> Result<(), String> {
        self.bed().check()
    }
}

/// How far a steel skirt's plate stands out from the outline, so its folded edge can lie over the
/// gap and not on the area's top.
fn skirt_gap(edge: &AreaEdge) -> f32 {
    if edge.style == BedStyle::Corten {
        FLANGE_IN
    } else {
        0.0
    }
}

/// The ring the skirt's outer face follows.
fn skirt_outer(edge: &AreaEdge, outline: &[P2]) -> Vec<P2> {
    crate::shape::offset_outline(outline, edge.wall() + skirt_gap(edge))
}

/// The sleepers of an edged area's skirt from `lo` to `hi`.
pub fn skirt_beams(edge: &AreaEdge, outline: &[P2], lo: f32, hi: f32) -> Vec<Beam> {
    beams(&edge.bed(), &skirt_outer(edge, outline), lo, hi)
}

/// Draws the skirt of an area with `outline` from `lo` to `hi`: faces 0 (`edge`) and 1
/// (`edge_rim`) of the sink.
pub(crate) fn skirt(s: &mut dyn Sink, edge: &AreaEdge, outline: &[P2], lo: f32, hi: f32) {
    let gap = skirt_gap(edge);
    let inner = crate::shape::offset_outline(outline, gap);
    mesh(s, &edge.bed(), &skirt_outer(edge, outline), &inner, lo, hi);
}

/// Draws the bed with walls between `outline` and `inner` from `lo` to `hi`.
pub(crate) fn mesh(
    s: &mut dyn Sink,
    bed: &RaisedBed,
    outline: &[P2],
    inner: &[P2],
    lo: f32,
    hi: f32,
) {
    let wall = |top: Option<usize>| Faces {
        wall: WALL,
        top,
        bottom: Some(WALL),
    };
    match bed.style {
        BedStyle::Plain => ring(s, outline, inner, (lo, hi), wall(Some(RIM))),
        BedStyle::Masonry => {
            let out = crate::shape::offset_outline(outline, COPING_OUT);
            let y = hi - COPING_H;
            ring(s, outline, inner, (lo, y), wall(None));
            ring(
                s,
                &out,
                inner,
                (y, hi),
                Faces {
                    wall: RIM,
                    top: Some(RIM),
                    bottom: Some(WALL),
                },
            );
        }
        BedStyle::Corten => {
            let out = crate::shape::offset_outline(outline, FLANGE_OUT);
            let y = hi - FLANGE_H;
            let flange_in =
                crate::shape::inner_ring(inner, FLANGE_IN).unwrap_or_else(|_| inner.to_vec());
            ring(s, outline, inner, (lo, y), wall(None));
            // The folded edge: a flange over the top and a lip down the outside.
            let fold = Faces {
                wall: RIM,
                top: Some(RIM),
                bottom: Some(RIM),
            };
            ring(s, &out, &flange_in, (y, hi), fold);
            ring(
                s,
                &out,
                outline,
                (y - LIP_H + FLANGE_H, y),
                Faces {
                    wall: RIM,
                    top: None,
                    bottom: Some(RIM),
                },
            );
        }
        BedStyle::Sleepers => {
            for b in beams(bed, outline, lo, hi) {
                beam(s, &b, bed.wall());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::raycast::raycast;
    use crate::shape::{Extrusion, Shape, shape_mesh};

    fn bed(outline: Vec<[f32; 2]>, thickness: f32) -> Shape {
        styled(outline, thickness, super::BedStyle::Plain)
    }

    fn styled(outline: Vec<[f32; 2]>, thickness: f32, style: super::BedStyle) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Bed".into(),
            layer: "shapes".into(),
            points: outline,
            curves: Default::default(),
            smooth: false,
            closed: true,
            base: 0.0,
            height: 0.4,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: None,
            raised_bed: Some(super::RaisedBed {
                style,
                wall_thickness_m: thickness,
                soil: None,
            }),
            furniture: None,
        })
    }

    fn rect() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]]
    }

    #[test]
    fn the_ring_is_hollow_with_named_faces_and_the_right_bounds() {
        let m = shape_mesh(&bed(rect(), 0.2)).unwrap();
        let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["S1:wall", "S1:rim"]);
        for i in 0..2u32 {
            assert!(m.triangle_faces.contains(&i));
        }
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &m.positions {
            for k in 0..3 {
                assert!(p[k].is_finite());
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        assert_eq!((lo, hi), ([0.0, 0.0, 0.0], [2.0, 0.4, 1.0]));
        // Straight down the middle nothing is hit; over the wall it is the rim.
        assert!(raycast(&m, [1.0, 2.0, 0.5], [0.0, -1.0, 0.0]).is_none());
        let hit = raycast(&m, [0.1, 2.0, 0.5], [0.0, -1.0, 0.0]).unwrap();
        assert_eq!(m.faces[hit.face].name, "S1:rim");
        assert!((hit.distance - 1.6).abs() < 1e-4);
        // And through the wall from the side.
        let hit = raycast(&m, [-1.0, 0.2, 0.5], [1.0, 0.0, 0.0]).unwrap();
        assert_eq!(m.faces[hit.face].name, "S1:wall");
    }

    #[test]
    fn the_volume_is_the_ring_alone_for_either_winding() {
        for outline in [rect(), rect().into_iter().rev().collect()] {
            let m = shape_mesh(&bed(outline, 0.2)).unwrap();
            let v: f32 = m
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| m.positions[i as usize]);
                    (a[0] * (b[1] * c[2] - b[2] * c[1])
                        + a[1] * (b[2] * c[0] - b[0] * c[2])
                        + a[2] * (b[0] * c[1] - b[1] * c[0]))
                        / 6.0
                })
                .sum::<f32>()
                .abs();
            // (2 x 1 - 1.6 x 0.6) x 0.4
            assert!((v - 0.416).abs() < 1e-3, "{v}");
        }
    }

    #[test]
    fn a_wall_too_thick_for_the_outline_is_refused() {
        assert!(shape_mesh(&bed(rect(), 0.6)).is_err());
        assert!(shape_mesh(&bed(rect(), 0.0)).is_err());
        assert!(shape_mesh(&bed(rect(), f32::NAN)).is_err());
        assert!(shape_mesh(&bed(rect(), 0.3)).is_ok());
    }

    fn bounds(m: &crate::ImportedModel) -> ([f32; 3], [f32; 3]) {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &m.positions {
            for k in 0..3 {
                assert!(p[k].is_finite());
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn sleepers_are_courses_of_beams_with_joints_and_overlapping_corners() {
        let shape = styled(
            vec![[0.0, 0.0], [3.0, 0.0], [3.0, 1.2], [0.0, 1.2]],
            0.15,
            super::BedStyle::Sleepers,
        );
        let Shape::Extrusion(e) = &shape else {
            panic!()
        };
        let bed = e.raised_bed.as_ref().unwrap();
        // 0.4 m is two courses of 0.2 m; every course lays 3 m twice (2 beams each) and 0.9 m twice.
        let beams = super::beams(bed, &e.points, 0.0, 0.4);
        assert_eq!(beams.len(), 12);
        let total: f32 = beams.iter().map(|b| b.u.1 - b.u.0).sum();
        assert!((total - 15.6).abs() < 0.03, "{total}");
        assert!(
            beams
                .iter()
                .all(|b| b.y.1 - b.y.0 <= 0.2 && b.u.1 - b.u.0 <= 2.4)
        );
        let m = shape_mesh(&shape).unwrap();
        let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["S1:wall", "S1:rim"]);
        let (lo, hi) = bounds(&m);
        for (a, b) in lo
            .iter()
            .zip([0.0, 0.0, 0.0])
            .chain(hi.iter().zip([3.0, 0.4, 1.2]))
        {
            assert!((a - b).abs() < 1e-4, "{lo:?} {hi:?}");
        }
        // The corners overlap: in a course one beam runs out to the corner, so the corner's
        // outer point is on the first course's beam 0 and not on beam 1 of that course.
        assert!(beams.iter().any(|b| (b.u.1 - 3.0).abs() < 1e-4));
        // Hollow: straight down the middle nothing; over a beam it is the rim.
        assert!(raycast(&m, [1.5, 2.0, 0.6], [0.0, -1.0, 0.0]).is_none());
        let hit = raycast(&m, [1.0, 2.0, 0.07], [0.0, -1.0, 0.0]).unwrap();
        assert_eq!(m.faces[hit.face].name, "S1:rim");
        // The wall is the beam's thickness: the soil's inner ring is 0.15 in.
        assert!(
            bed.inner(&e.points)
                .unwrap()
                .iter()
                .any(|p| (p[0] - 0.15).abs() < 1e-5)
        );
    }

    #[test]
    fn corten_is_a_thin_plate_with_a_folded_edge() {
        let shape = styled(rect(), 0.008, super::BedStyle::Corten);
        let Shape::Extrusion(e) = &shape else {
            panic!()
        };
        let bed = e.raised_bed.as_ref().unwrap();
        // Drawn at least 1 cm thick so it reads.
        assert_eq!(bed.wall(), 0.01);
        let m = shape_mesh(&shape).unwrap();
        let (lo, hi) = bounds(&m);
        // The lip reaches 1.5 cm past the plate; the height is the bed's.
        assert!(
            (lo[0] + 0.015).abs() < 1e-4 && (hi[0] - 2.015).abs() < 1e-4,
            "{lo:?} {hi:?}"
        );
        assert!(lo[1].abs() < 1e-5 && (hi[1] - 0.4).abs() < 1e-5);
        assert!(raycast(&m, [1.0, 2.0, 0.5], [0.0, -1.0, 0.0]).is_none());
        // Over the plate it hits the folded edge, which is the rim.
        let hit = raycast(&m, [0.005, 2.0, 0.5], [0.0, -1.0, 0.0]).unwrap();
        assert_eq!(m.faces[hit.face].name, "S1:rim");
        let hit = raycast(&m, [-1.0, 0.2, 0.5], [1.0, 0.0, 0.0]).unwrap();
        assert_eq!(m.faces[hit.face].name, "S1:wall");
        assert!(bed.check().is_ok());
        assert!(
            super::RaisedBed {
                wall_thickness_m: 0.1,
                style: super::BedStyle::Corten,
                soil: None
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn masonry_has_a_coping_and_every_style_meshes_finite() {
        for style in [super::BedStyle::Masonry, super::BedStyle::Plain] {
            let m = shape_mesh(&styled(rect(), 0.2, style)).unwrap();
            let (lo, hi) = bounds(&m);
            let over = if style == super::BedStyle::Masonry {
                0.02
            } else {
                0.0
            };
            assert!(
                (lo[0] + over).abs() < 1e-4 && (hi[1] - 0.4).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
        }
        // Any outline, either winding.
        for style in [
            super::BedStyle::Sleepers,
            super::BedStyle::Corten,
            super::BedStyle::Masonry,
        ] {
            let l = vec![
                [0.0, 0.0],
                [4.0, 0.0],
                [4.0, 2.0],
                [2.0, 2.0],
                [2.0, 4.0],
                [0.0, 4.0],
            ];
            for outline in [l.clone(), l.into_iter().rev().collect()] {
                let t = style.default_wall_m();
                let m = shape_mesh(&styled(outline, t, style)).unwrap();
                assert!(!m.triangles.is_empty());
                assert!(raycast(&m, [3.0, 2.0, 3.0], [0.0, -1.0, 0.0]).is_none());
            }
        }
    }
}
