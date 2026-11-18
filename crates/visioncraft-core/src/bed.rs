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
pub(crate) const WALL: usize = 0;
pub(crate) const RIM: usize = 1;

/// The wall thickness a plain bed starts with (metres).
pub const DEFAULT_WALL_M: f32 = 0.2;
/// How far the soil lies below the rim (metres).
pub const SOIL_BELOW_RIM_M: f32 = 0.05;
/// A sleeper's course height (the bed's height is split in whole courses of about this).
pub const BEAM_H: f32 = 0.2;
/// The longest sleeper; a longer wall is laid in several, with a joint.
pub const BEAM_MAX_LEN: f32 = 2.4;
/// The groove between sleepers (metres).
pub(crate) const JOINT: f32 = 0.004;
/// A steel plate is drawn at least this thick so it reads (metres).
pub const MIN_PLATE_M: f32 = 0.01;
/// The folded edge of a steel bed: height of the flange, how far it reaches out past the plate and
/// in over the soil, and how far the lip hangs down the outside.
pub(crate) const FLANGE_H: f32 = 0.012;
pub(crate) const FLANGE_OUT: f32 = 0.015;
pub(crate) const FLANGE_IN: f32 = 0.02;
pub(crate) const LIP_H: f32 = 0.05;
/// A masonry bed's coping: height and how far it overhangs.
pub(crate) const COPING_H: f32 = 0.04;
pub(crate) const COPING_OUT: f32 = 0.02;

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
    /// A stone-filled wire cage (a terrace's retaining wall): a thick stone wall with a rim of
    /// steel and wire bars across its face.
    Gabion,
}

impl BedStyle {
    pub fn default_wall_m(self) -> f32 {
        match self {
            Self::Plain | Self::Masonry => DEFAULT_WALL_M,
            Self::Sleepers => 0.15,
            Self::Corten => 0.01,
            Self::Gabion => 0.4,
        }
    }

    /// The wall thicknesses the style takes (metres).
    pub fn wall_range(self) -> (f32, f32) {
        match self {
            Self::Plain | Self::Masonry => (0.05, 0.3),
            Self::Sleepers => (0.1, 0.2),
            Self::Corten => (0.005, 0.02),
            Self::Gabion => (0.2, 0.6),
        }
    }

    /// The palette material the walls start with.
    pub fn default_material(self) -> &'static str {
        match self {
            Self::Plain | Self::Sleepers => "timber",
            Self::Masonry => "brick",
            Self::Corten => "corten",
            Self::Gabion => "gravel",
        }
    }

    /// The palette material of the rim (a gabion's wire frame is steel).
    pub fn rim_material(self) -> &'static str {
        match self {
            Self::Gabion => "metal",
            _ => self.default_material(),
        }
    }

    pub(crate) fn is_plain(&self) -> bool {
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

/// How far a sleeper laid along a curve may lie from it (metres).
pub const BEAM_CHORD_TOL_M: f32 = 0.02;
/// The largest turn (radians, 15 degrees) between two edges of an outline that is joined as a
/// smooth curve, with mitred sleepers, and not as a corner, where one beam overlaps the other.
pub const SMOOTH_TURN: f32 = 0.2618;
/// How far a corner may be from a right angle (radians, 10 degrees) to keep the overlapping
/// beams of a square corner; any other is mitred.
const RIGHT_ANGLE_TOL: f32 = 0.1745;
/// The sharpest turn (radians, 150 degrees) that is mitred: a mitre of a sharper corner would
/// reach far past the beam.
const MAX_MITRE_TURN: f32 = 2.618;

/// One sleeper: a box `u` along the edge from `from`, `t` across (inward from the outer face) and
/// `y` high. `miter` is how far the inner face's end lies past the outer face's end along `dir`
/// at each of the two ends (negative: short of it): a straight end is `(0, 0)`, the ends of the
/// beams along a curve are cut to the bisector of the corner between two chords.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    pub from: P2,
    pub dir: P2,
    pub inward: P2,
    pub u: (f32, f32),
    pub y: (f32, f32),
    pub top: bool,
    pub miter: (f32, f32),
}

/// The sleepers of a bed with `outline` (its outer face) from `lo` to `hi`, every vertex a corner:
/// see [`beams_curved`].
pub fn beams(bed: &RaisedBed, outline: &[P2], lo: f32, hi: f32) -> Vec<Beam> {
    beams_curved(bed, outline, &[], lo, hi)
}

fn cross(a: P2, b: P2) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn unit(a: P2, b: P2) -> (P2, f32) {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-6);
    ([(b[0] - a[0]) / len, (b[1] - a[1]) / len], len)
}

/// A polyline along a curve with its running length, to cut into chords. The polyline's own
/// segments are chords of the curve (within [`crate::curve::CHORD_TOL_M`]); points taken between
/// its vertices are put back on the curve from the turn at the segment's ends, so that a circle
/// stays a circle and a beam's ends lie on it.
struct Chain {
    pts: Vec<P2>,
    cum: Vec<f32>,
    /// Closed: the last point joins the first and `cum` has one more entry, the full length.
    closed: bool,
    /// The signed turn (radians, positive towards +left in x-z numbers) at each vertex where the
    /// chain bends smoothly; `None` at the two ends of an open chain.
    turn: Vec<Option<f32>>,
    /// Whether each vertex lies along a curved edge (and is no corner of the control polygon).
    interior: Vec<bool>,
}

impl Chain {
    fn new(pts: Vec<P2>, closed: bool, interior: Vec<bool>) -> Self {
        let mut cum = vec![0.0];
        let m = pts.len();
        let count = if closed { m } else { m - 1 };
        for i in 0..count {
            let (_, l) = unit(pts[i], pts[(i + 1) % m]);
            cum.push(cum[i] + l);
        }
        let turn = (0..m)
            .map(|i| {
                if !closed && (i == 0 || i + 1 == m) {
                    return None;
                }
                let (d0, _) = unit(pts[(i + m - 1) % m], pts[i]);
                let (d1, _) = unit(pts[i], pts[(i + 1) % m]);
                Some(cross(d0, d1).atan2(d0[0] * d1[0] + d0[1] * d1[1]))
            })
            .collect();
        Self {
            pts,
            cum,
            closed,
            turn,
            interior,
        }
    }

    fn length(&self) -> f32 {
        *self.cum.last().expect("a chain has points")
    }

    /// The point `s` along the chain (wrapping when closed), on the curve.
    fn at(&self, s: f32) -> P2 {
        let total = self.length();
        let s = if self.closed {
            s.rem_euclid(total)
        } else {
            s.clamp(0.0, total)
        };
        let m = self.pts.len();
        let i = self
            .cum
            .partition_point(|&c| c <= s)
            .saturating_sub(1)
            .min(self.cum.len() - 2);
        let l = (self.cum[i + 1] - self.cum[i]).max(1e-9);
        let (a, b) = (self.pts[i], self.pts[(i + 1) % m]);
        let w = (s - self.cum[i]) / l;
        let mut p = [a[0] + (b[0] - a[0]) * w, a[1] + (b[1] - a[1]) * w];
        // The curve bulges out of a chord of a curved edge: a circle through the chord's ends
        // whose central angle is the turn at its ends that lie along the curve (a corner of the
        // control polygon turns by only half as much, and a straight edge has no bulge).
        let ends = [i, (i + 1) % m];
        let turns: Vec<f32> = ends
            .into_iter()
            .filter(|&v| self.interior[v])
            .filter_map(|v| self.turn[v])
            .collect();
        if !turns.is_empty() {
            let theta = turns.iter().sum::<f32>() / turns.len() as f32;
            if theta.abs() > 1e-4 {
                let rho = l / (2.0 * (theta.abs() / 2.0).sin());
                let half = l / 2.0;
                let off = (l * (w - 0.5)).abs();
                let sag = (rho * rho - off * off).max(0.0).sqrt()
                    - (rho * rho - half * half).max(0.0).sqrt();
                let (d, _) = unit(a, b);
                // Positive turn: the curve bends towards the left, so it bulges to the right.
                let out = [d[1] * theta.signum(), -d[0] * theta.signum()];
                p = [p[0] + out[0] * sag, p[1] + out[1] * sag];
            }
        }
        p
    }

    /// Whether the chord from `a` to `b` along the chain is short enough for a sleeper and keeps
    /// within [`BEAM_CHORD_TOL_M`] of the curve between.
    fn chord_ok(&self, a: f32, b: f32) -> bool {
        let (pa, pb) = (self.at(a), self.at(b));
        let (d, len) = unit(pa, pb);
        if len > BEAM_MAX_LEN {
            return false;
        }
        let total = self.length();
        let m = self.pts.len();
        let segments = if self.closed { m } else { m - 1 };
        for lap in [-1.0f32, 0.0, 1.0] {
            for i in 0..segments {
                let (c0, c1) = (self.cum[i] + lap * total, self.cum[i + 1] + lap * total);
                if c1 <= a || c0 >= b {
                    continue;
                }
                // The vertex and three points along the segment after it.
                for q in [0.0, 0.25, 0.5, 0.75] {
                    let c = c0 + (c1 - c0) * q;
                    if c > a && c < b {
                        let p = self.at(c);
                        if cross(d, [p[0] - pa[0], p[1] - pa[1]]).abs() > BEAM_CHORD_TOL_M {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Where the beams of one course of the range `s0..s1` join: equal stretches, as few as keep
    /// every chord within tolerance and no beam over [`BEAM_MAX_LEN`]. `shift` moves the joints
    /// half a beam along, so a course's joints lie over the middle of the beams below.
    fn joints(&self, s0: f32, s1: f32, shift: bool) -> Vec<f32> {
        let range = s1 - s0;
        let mut n = ((range / BEAM_MAX_LEN).ceil() as usize).max(1);
        let make = |n: usize, shift: bool| -> Vec<f32> {
            let p = range / n as f32;
            if !shift {
                (0..=n).map(|k| s0 + p * k as f32).collect()
            } else if self.closed {
                (0..=n).map(|k| s0 + p * (k as f32 + 0.5)).collect()
            } else {
                let mut v = vec![s0];
                v.extend((0..n).map(|k| s0 + p * (k as f32 + 0.5)));
                v.push(s1);
                v
            }
        };
        let ok = |v: &[f32]| v.windows(2).all(|w| self.chord_ok(w[0], w[1]));
        // An open chain of one beam cannot be shifted.
        let shifted = |n: usize| shift && (self.closed || n >= 2);
        while n < 4000 && !(ok(&make(n, false)) && (!shifted(n) || ok(&make(n, true)))) {
            n += 1;
        }
        make(n, shifted(n))
    }
}

/// The sleepers of a bed with `outline` (its outer face) from `lo` to `hi`: whole courses of about
/// [`BEAM_H`], each edge laid as long beams of at most [`BEAM_MAX_LEN`]. At a corner one beam runs
/// right out to the corner and the other stops against it, swapping every course. Exact at right
/// angles; at other corners a small notch is left.
///
/// `ctrl` lists the vertices of `outline` that are corners of the drawn shape's control polygon
/// (see [`crate::curve::Tess::ctrl`]); the vertices between them lie along a curved edge. Empty:
/// every vertex is a corner. A curved stretch (up to a corner that turns more than
/// [`SMOOTH_TURN`]) is laid as straight beams, each a chord of the curve: equal chords, as few
/// as keep each within [`BEAM_CHORD_TOL_M`] of the curve and none over [`BEAM_MAX_LEN`], their
/// joints staggered by half a beam from one course to the next, and their ends mitred so no gap
/// shows between two chords. The beam's `u` length is its chord.
pub fn beams_curved(
    bed: &RaisedBed,
    outline: &[P2],
    ctrl: &[usize],
    lo: f32,
    hi: f32,
) -> Vec<Beam> {
    let n = outline.len();
    let t = bed.wall();
    let sign = if crate::shape::shoelace_f32(outline) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let seg = |j: usize| unit(outline[j % n], outline[(j + 1) % n]);
    let corner = |i: usize| ctrl.is_empty() || ctrl.contains(&(i % n));
    // A corner joins as one when the outline turns sharply, or both its edges are straight.
    let hard: Vec<usize> = (0..n)
        .filter(|&i| {
            let (dp, _) = seg(i + n - 1);
            let (d, _) = seg(i);
            corner(i)
                && (cross(dp, d).abs().atan2(dp[0] * d[0] + dp[1] * d[1]) > SMOOTH_TURN
                    || (corner(i + n - 1) && corner(i + 1)))
        })
        .collect();
    let courses = (((hi - lo) / BEAM_H).round() as usize).max(1);
    let ch = (hi - lo) / courses as f32;
    // The stretches between hard corners (all the way round when there are none).
    let chains: Vec<Chain> = if hard.is_empty() {
        vec![Chain::new(
            outline.to_vec(),
            true,
            (0..n).map(|i| !corner(i)).collect(),
        )]
    } else {
        (0..hard.len())
            .map(|k| {
                let (a, mut b) = (hard[k], hard[(k + 1) % hard.len()]);
                if b <= a {
                    b += n;
                }
                Chain::new(
                    (a..=b).map(|i| outline[i % n]).collect(),
                    false,
                    (a..=b).map(|i| !corner(i)).collect(),
                )
            })
            .collect()
    };
    let m = chains.len();
    let first_dir = |c: &Chain| unit(c.pts[0], c.pts[1]).0;
    let last_dir = |c: &Chain| unit(c.pts[c.pts.len() - 2], c.pts[c.pts.len() - 1]).0;
    // A corner that is not about a right angle is mitred, both beams cut to its bisector: a beam
    // that ran out past the other would leave a notch there. The shifts of the end of the chain
    // before it and of the start of the chain after it (see `Beam::miter`); right angles keep
    // the overlap, one beam running out over the other and swapping every course.
    let mitred: Vec<Option<(f32, f32)>> = if hard.is_empty() {
        Vec::new()
    } else {
        (0..m)
            .map(|k| {
                let (d0, d1) = (last_dir(&chains[k]), first_dir(&chains[(k + 1) % m]));
                let turn = cross(d0, d1).atan2(d0[0] * d1[0] + d0[1] * d1[1]);
                ((turn.abs() - std::f32::consts::FRAC_PI_2).abs() > RIGHT_ANGLE_TOL
                    && turn.abs() < MAX_MITRE_TURN)
                    .then(|| {
                        let m = t * (turn.abs() / 2.0).tan();
                        if sign * turn > 0.0 { (-m, m) } else { (m, -m) }
                    })
            })
            .collect()
    };
    let mitre_at = |k: usize| mitred.get(k).copied().flatten();
    let mut out = Vec::new();
    for c in 0..courses {
        let y = (
            lo + c as f32 * ch + if c > 0 { JOINT / 2.0 } else { 0.0 },
            lo + (c + 1) as f32 * ch - if c + 1 < courses { JOINT / 2.0 } else { 0.0 },
        );
        let top = c + 1 == courses;
        let runs_out = |k: usize| (k + c).is_multiple_of(2);
        for (k, chain) in chains.iter().enumerate() {
            // How far a beam stops short of the corner it stops against.
            let trim = |d: P2, other: P2| t / cross(d, other).abs().max(0.2);
            let (s0, s1) = if hard.is_empty() {
                (0.0, chain.length())
            } else {
                let prev = &chains[(k + m - 1) % m];
                let next = &chains[(k + 1) % m];
                let before = (k + m - 1) % m;
                (
                    if mitre_at(before).is_some() || !runs_out(before) {
                        0.0
                    } else {
                        trim(first_dir(chain), last_dir(prev))
                    },
                    chain.length()
                        - if mitre_at(k).is_some() || runs_out(k) {
                            0.0
                        } else {
                            trim(last_dir(chain), first_dir(next))
                        },
                )
            };
            if s1 - s0 <= JOINT {
                continue;
            }
            // One straight edge: equal beams, as long as they go, with a joint between.
            let joints = if chain.pts.len() == 2 {
                let pieces = (((s1 - s0) / BEAM_MAX_LEN).ceil() as usize).max(1);
                (0..=pieces)
                    .map(|j| s0 + (s1 - s0) * j as f32 / pieces as f32)
                    .collect()
            } else {
                chain.joints(s0, s1, c % 2 == 1)
            };
            let count = joints.len() - 1;
            let chords: Vec<(P2, P2, P2, f32)> = (0..count)
                .map(|j| {
                    let (a, b) = (chain.at(joints[j]), chain.at(joints[j + 1]));
                    let (d, len) = unit(a, b);
                    (a, b, d, len)
                })
                .collect();
            // The mitre at the joint after chord `j` (and before chord `j + 1`).
            let joint_cut = |j: usize| -> Option<(f32, f32)> {
                let next = if j + 1 < count {
                    j + 1
                } else if chain.closed {
                    0
                } else {
                    return None;
                };
                let (d0, d1) = (chords[j].2, chords[next].2);
                let turn = cross(d0, d1).atan2(d0[0] * d1[0] + d0[1] * d1[1]);
                let m = t * (turn.abs() / 2.0).tan();
                // Convex (the outline turns inwards): the inner face is the shorter.
                Some(if sign * turn > 0.0 { (-m, m) } else { (m, -m) })
            };
            for j in 0..count {
                let (a, _, d, len) = chords[j];
                let straight = chain.pts.len() == 2;
                let before = if j > 0 || chain.closed {
                    joint_cut((j + count - 1) % count)
                } else {
                    mitre_at((k + m - 1) % m).map(|(_, start)| (0.0, start))
                };
                let after = if j + 1 < count || chain.closed {
                    joint_cut(j)
                } else {
                    mitre_at(k).map(|(end, _)| (end, 0.0))
                };
                let (gap0, gap1) = if straight {
                    (
                        if j > 0 { JOINT / 2.0 } else { 0.0 },
                        if j + 1 < count { JOINT / 2.0 } else { 0.0 },
                    )
                } else {
                    (
                        if j > 0 || chain.closed {
                            JOINT / 2.0
                        } else {
                            0.0
                        },
                        if j + 1 < count || chain.closed {
                            JOINT / 2.0
                        } else {
                            0.0
                        },
                    )
                };
                if len - gap0 - gap1 <= 1e-4 {
                    continue;
                }
                // A straight edge keeps its beams' lengths along the edge from its first point.
                let (from, u) = if straight {
                    (chain.pts[0], (joints[j] + gap0, joints[j + 1] - gap1))
                } else {
                    (a, (gap0, len - gap1))
                };
                out.push(Beam {
                    from,
                    dir: d,
                    inward: [-sign * d[1], sign * d[0]],
                    u,
                    y,
                    top,
                    miter: (before.map_or(0.0, |x| x.1), after.map_or(0.0, |x| x.0)),
                });
            }
        }
    }
    out
}

pub(crate) fn quad(s: &mut dyn Sink, face: usize, v: [[f32; 3]; 4], outward: [f32; 3]) {
    s.tri(face, [v[0], v[1], v[2]], outward);
    s.tri(face, [v[0], v[2], v[3]], outward);
}

/// One sleeper as a box (its ends cut to the bisectors of the corners when it has a `miter`).
pub(crate) fn beam(s: &mut dyn Sink, b: &Beam, t: f32) {
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
    let (i0, i1) = (u0 + b.miter.0, u1 + b.miter.1);
    let (y0, y1) = b.y;
    let top = if b.top { RIM } else { WALL };
    // The corners of the beam's plan: outer face first, inner face second, start and end.
    let plan = |y: f32| [p(u0, 0.0, y), p(u1, 0.0, y), p(i1, t, y), p(i0, t, y)];
    quad(s, WALL, plan(y0), [0.0, -1.0, 0.0]);
    quad(s, top, plan(y1), [0.0, 1.0, 0.0]);
    // The end faces: a cut end leans, and its normal with it.
    let lean = |w: f32, m: f32| {
        let v = [d[0] * w + n[0] * m, 0.0, d[2] * w + n[2] * m];
        let l = (v[0] * v[0] + v[2] * v[2]).sqrt().max(1e-9);
        v.map(|c| c / l)
    };
    quad(
        s,
        WALL,
        [p(u0, 0.0, y0), p(i0, t, y0), p(i0, t, y1), p(u0, 0.0, y1)],
        lean(-t, b.miter.0),
    );
    quad(
        s,
        WALL,
        [p(u1, 0.0, y0), p(i1, t, y0), p(i1, t, y1), p(u1, 0.0, y1)],
        lean(t, -b.miter.1),
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
        [p(i0, t, y0), p(i1, t, y0), p(i1, t, y1), p(i0, t, y1)],
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

/// The sleepers of an edged area's skirt from `lo` to `hi`; `ctrl` as in [`beams_curved`].
pub fn skirt_beams(edge: &AreaEdge, outline: &[P2], ctrl: &[usize], lo: f32, hi: f32) -> Vec<Beam> {
    beams_curved(&edge.bed(), &skirt_outer(edge, outline), ctrl, lo, hi)
}

/// Draws the skirt of an area with `outline` from `lo` to `hi`: faces 0 (`edge`) and 1
/// (`edge_rim`) of the sink.
pub(crate) fn skirt(
    s: &mut dyn Sink,
    edge: &AreaEdge,
    outline: &[P2],
    ctrl: &[usize],
    lo: f32,
    hi: f32,
) {
    let gap = skirt_gap(edge);
    let inner = crate::shape::offset_outline(outline, gap);
    mesh(
        s,
        &edge.bed(),
        &skirt_outer(edge, outline),
        &inner,
        ctrl,
        lo,
        hi,
    );
}

/// Draws the bed with walls between `outline` and `inner` from `lo` to `hi`; `ctrl` as in
/// [`beams_curved`] (it only matters for sleepers).
pub(crate) fn mesh(
    s: &mut dyn Sink,
    bed: &RaisedBed,
    outline: &[P2],
    inner: &[P2],
    ctrl: &[usize],
    lo: f32,
    hi: f32,
) {
    let wall = |top: Option<usize>| Faces {
        wall: WALL,
        top,
        bottom: Some(WALL),
    };
    match bed.style {
        BedStyle::Plain | BedStyle::Gabion => ring(s, outline, inner, (lo, hi), wall(Some(RIM))),
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
            for b in beams_curved(bed, outline, ctrl, lo, hi) {
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

    /// A round outline of radius `r` as `shape.add_circle` draws it: 4 control points, 4 arcs.
    fn round(r: f32) -> (Vec<[f32; 2]>, crate::curve::Tess) {
        let pts = vec![[r, 0.0], [0.0, r], [-r, 0.0], [0.0, -r]];
        let sag = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
        let curves = (0..4)
            .map(|i| (i, crate::curve::Curve::Arc { sagitta_m: sag }))
            .collect();
        let tess = crate::curve::tessellate(&pts, true, &curves, false);
        (pts, tess)
    }

    fn sleeper_bed(thickness: f32) -> super::RaisedBed {
        super::RaisedBed {
            style: super::BedStyle::Sleepers,
            wall_thickness_m: thickness,
            soil: None,
        }
    }

    #[test]
    fn sleepers_on_a_round_bed_are_chords_within_two_centimetres_of_the_arc() {
        let r = 3.0f32;
        let (_, tess) = round(r);
        let bed = sleeper_bed(0.15);
        let beams = super::beams_curved(&bed, &tess.pts, &tess.ctrl, 0.0, 0.4);
        assert!(!beams.is_empty());
        for b in &beams {
            // Along the chord from its first to its last point, the beam is no longer than a
            // sleeper and the chord stays inside the circle, within 2 cm of it.
            assert!(b.u.1 - b.u.0 <= super::BEAM_MAX_LEN);
            for k in 0..=8 {
                let u = b.u.0 + (b.u.1 - b.u.0) * k as f32 / 8.0;
                let p = [b.from[0] + b.dir[0] * u, b.from[1] + b.dir[1] * u];
                let off = r - p[0].hypot(p[1]);
                assert!(
                    (0.0..=super::BEAM_CHORD_TOL_M).contains(&(off + 1e-4)),
                    "{off}"
                );
            }
            // The chord's ends are on the circle (the outline's own error, 3 mm, aside).
            for u in [b.u.0, b.u.1] {
                let p = [b.from[0] + b.dir[0] * u, b.from[1] + b.dir[1] * u];
                assert!((p[0].hypot(p[1]) - r).abs() < 0.005);
            }
        }
    }

    #[test]
    fn a_three_metre_radius_round_bed_has_the_fewest_chords_that_keep_to_two_centimetres() {
        let r = 3.0f32;
        let (_, tess) = round(r);
        let beams = super::beams_curved(&sleeper_bed(0.15), &tess.pts, &tess.ctrl, 0.0, 0.4);
        // Two courses of 0.2 m; each lays the same number of chords all the way round.
        let per_course = beams.len() / 2;
        assert_eq!(beams.len(), per_course * 2);
        // A chord of angle a is r (1 - cos(a / 2)) off the arc: 2 cm gives 28 chords.
        let exact = (std::f32::consts::TAU / (2.0 * (1.0 - 0.02 / r).acos())).ceil() as usize;
        assert_eq!(exact, 28);
        // One fewer would put a chord over 2 cm off the arc.
        assert_eq!(per_course, exact, "{per_course} chords a course");
        // The real length: the chords add up to just under the circle.
        let total: f32 = beams.iter().map(|b| b.u.1 - b.u.0).sum::<f32>() / 2.0;
        assert!(
            total < std::f32::consts::TAU * r && total > 0.985 * std::f32::consts::TAU * r,
            "{total}"
        );
    }

    #[test]
    fn the_joints_between_chords_are_mitred_so_no_gap_shows_and_courses_are_staggered() {
        let (_, tess) = round(3.0);
        let t = 0.15;
        let beams = super::beams_curved(&sleeper_bed(t), &tess.pts, &tess.ctrl, 0.0, 0.4);
        let per_course = beams.len() / 2;
        let (first, second) = beams.split_at(per_course);
        let end = |b: &super::Beam, w: f32, e: usize| {
            let (u, m) = if e == 1 {
                (b.u.1, b.miter.1)
            } else {
                (b.u.0, b.miter.0)
            };
            let u = if w > 0.0 { u + m } else { u };
            [
                b.from[0] + b.dir[0] * u + b.inward[0] * w,
                b.from[1] + b.dir[1] * u + b.inward[1] * w,
            ]
        };
        for course in [first, second] {
            for j in 0..per_course {
                let (a, b) = (&course[j], &course[(j + 1) % per_course]);
                // Outer face and inner face meet within the groove between the sleepers.
                for w in [0.0, t] {
                    let (p, q) = (end(a, w, 1), end(b, w, 0));
                    let gap = (p[0] - q[0]).hypot(p[1] - q[1]);
                    assert!(gap < 2.0 * super::JOINT, "{j} {w}: {gap}");
                }
            }
        }
        // The joints of the second course lie over the middle of the first course's beams.
        let joint = |b: &super::Beam| {
            let p = end(b, 0.0, 0);
            p[1].atan2(p[0])
        };
        let pitch = std::f32::consts::TAU / per_course as f32;
        let shift = (joint(&second[0]) - joint(&first[0])).rem_euclid(pitch);
        assert!((shift - pitch / 2.0).abs() < pitch * 0.1, "{shift} {pitch}");
    }

    #[test]
    fn a_curved_sleeper_bed_meshes_and_a_sleeper_wall_on_a_straight_bed_is_unchanged() {
        let (pts, tess) = round(3.0);
        let shape = {
            let mut sh = styled(pts, 0.15, super::BedStyle::Sleepers);
            let Shape::Extrusion(e) = &mut sh else {
                panic!()
            };
            e.curves = (0..4)
                .map(|i| {
                    (
                        i,
                        crate::curve::Curve::Arc {
                            sagitta_m: 3.0 * (1.0 - std::f32::consts::FRAC_1_SQRT_2),
                        },
                    )
                })
                .collect();
            sh
        };
        let m = shape_mesh(&shape).unwrap();
        assert!(m.triangles.len() > 28 * 2 * 8);
        let (lo, hi) = bounds(&m);
        assert!((hi[1] - 0.4).abs() < 1e-5 && lo[1].abs() < 1e-5);
        // Hollow in the middle, a rim over the wall.
        assert!(raycast(&m, [0.0, 2.0, 0.0], [0.0, -1.0, 0.0]).is_none());
        let hit = raycast(&m, [2.92, 2.0, 0.0], [0.0, -1.0, 0.0]).unwrap();
        assert_eq!(m.faces[hit.face].name, "S1:rim");
        // Straight outlines give the same beams whether or not they are told their corners.
        let rect = vec![[0.0, 0.0], [3.0, 0.0], [3.0, 1.2], [0.0, 1.2]];
        let bed = sleeper_bed(0.15);
        assert_eq!(
            super::beams(&bed, &rect, 0.0, 0.4),
            super::beams_curved(&bed, &rect, &[0, 1, 2, 3], 0.0, 0.4)
        );
        let _ = tess;
    }

    #[test]
    fn a_bed_with_a_curved_end_lays_its_straight_sides_whole() {
        // Straight sides 5 m long and a shallow round end that meets them at corners: the
        // straight edges keep their long beams, mitred only where they meet the arc.
        let pts = vec![[0.0, 0.0], [5.0, 0.0], [5.0, 2.0], [0.0, 2.0]];
        let curves: crate::curve::Curves =
            [(1, crate::curve::Curve::Arc { sagitta_m: 0.6 })].into();
        let tess = crate::curve::tessellate(&pts, true, &curves, false);
        let beams = super::beams_curved(&sleeper_bed(0.15), &tess.pts, &tess.ctrl, 0.0, 0.2);
        // The 5 m sides are cut in 3 beams (the corner trims shorten them), the round end in
        // chords; all within the limits.
        assert!(beams.len() > 8);
        assert!(beams.iter().all(|b| b.u.1 - b.u.0 <= super::BEAM_MAX_LEN));
        let straight: Vec<_> = beams.iter().filter(|b| b.dir[1].abs() < 1e-6).collect();
        assert!(
            straight.len() >= 4,
            "{:?}",
            beams
                .iter()
                .map(|b| (b.from, b.dir, b.u))
                .collect::<Vec<_>>()
        );
        // Only the beams at the two corners of the end (about 120 degrees) are cut.
        assert!(straight.iter().filter(|b| b.miter != (0.0, 0.0)).count() <= 2);
    }

    #[test]
    fn corners_that_are_not_right_angles_are_mitred_without_a_notch() {
        // A hexagon: every corner turns 60 degrees. One beam to an edge, each cut to the bisector.
        let hex: Vec<[f32; 2]> = (0..6)
            .map(|k| {
                let a = std::f32::consts::TAU * k as f32 / 6.0;
                [2.0 * a.cos(), 2.0 * a.sin()]
            })
            .collect();
        let t = 0.15;
        let beams = super::beams(&sleeper_bed(t), &hex, 0.0, 0.2);
        assert_eq!(beams.len(), 6);
        let end = |b: &super::Beam, w: f32, e: usize| {
            let u = if e == 1 { b.u.1 } else { b.u.0 };
            let u = if w > 0.0 {
                u + if e == 1 { b.miter.1 } else { b.miter.0 }
            } else {
                u
            };
            [
                b.from[0] + b.dir[0] * u + b.inward[0] * w,
                b.from[1] + b.dir[1] * u + b.inward[1] * w,
            ]
        };
        for j in 0..6 {
            let (a, b) = (&beams[j], &beams[(j + 1) % 6]);
            for w in [0.0, t] {
                let (p, q) = (end(a, w, 1), end(b, w, 0));
                assert!((p[0] - q[0]).hypot(p[1] - q[1]) < 1e-3, "{j} {w}");
            }
        }
        // The right angles of a rectangle keep their overlap: no mitre there.
        let rect = vec![[0.0, 0.0], [3.0, 0.0], [3.0, 1.2], [0.0, 1.2]];
        assert!(
            super::beams(&sleeper_bed(t), &rect, 0.0, 0.4)
                .iter()
                .all(|b| b.miter == (0.0, 0.0))
        );
    }
}
